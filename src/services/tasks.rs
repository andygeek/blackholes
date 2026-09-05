use crate::paths::AppPaths;
use crate::{
    model::{
        DEFAULT_TASK_ICON, ProjectTask, Repository, TaskRepository, Workspace, WorkspaceColor,
    },
    services::notes::{ProjectTaskInstructionsService, TaskNoteService},
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct RepositoryPreparation {
    pub copy_local_changes: bool,
    pub copy_environment_files: bool,
    pub setup_command: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskBranchSource {
    #[default]
    Current,
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExistingBranchAction {
    #[default]
    Reuse,
    Recreate,
}

/// Where a task branch is rooted when Blackholes has to create it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedBase {
    /// The base as Git spells it, such as `origin/master`, so the caller sees
    /// which side of the name actually answered.
    pub label: String,
    pub revision: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchAvailability {
    pub repository_id: Uuid,
    pub repository_name: String,
    /// The branch name as Git actually stores it, which can differ in casing
    /// from the requested one.
    pub branch: String,
    pub current_branch: Option<String>,
    pub current_revision: String,
    pub local_revision: Option<String>,
    pub remote_revision: Option<String>,
    pub local_checked_out: bool,
    /// How the requested base resolved here. `None` when no base was asked
    /// for; a base that resolves nowhere fails the whole check instead.
    pub base: Option<ResolvedBase>,
}

#[derive(Clone, Debug)]
pub struct CreateTaskRequest {
    pub title: String,
    pub description: Option<String>,
    pub branch_name: Option<String>,
    pub branch_source: TaskBranchSource,
    /// Branch, tag or revision a task branch that does not exist yet is
    /// created from. `None` roots it in each repository's current HEAD.
    pub base_ref: Option<String>,
    pub create_missing_branch: bool,
    pub replace_divergent_local_branches: bool,
    pub existing_branch_action: ExistingBranchAction,
    pub repository_ids: Vec<Uuid>,
    pub preparations: HashMap<Uuid, RepositoryPreparation>,
}

#[derive(Clone, Debug)]
pub struct AddTaskRepositoriesRequest {
    pub repository_ids: Vec<Uuid>,
    pub branch_source: TaskBranchSource,
    /// Same meaning as in [`CreateTaskRequest`]: the base for repositories
    /// where the task branch still has to be created.
    pub base_ref: Option<String>,
    pub create_missing_branch: bool,
    pub replace_divergent_local_branches: bool,
    pub existing_branch_action: ExistingBranchAction,
    pub preparations: HashMap<Uuid, RepositoryPreparation>,
}

#[derive(Clone, Debug, Default)]
pub struct RemoveTaskRepositoriesRequest {
    pub repository_ids: Vec<Uuid>,
    /// Remove the worktree even when it still holds uncommitted work.
    pub discard_uncommitted_changes: bool,
    /// Also delete the local task branch. Adding the repository again then
    /// re-creates the branch, which is the only way a different base can take
    /// effect: an existing branch is reused as it stands.
    pub delete_branch: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedTaskRepository {
    pub repository_id: Uuid,
    pub worktree_path: PathBuf,
    pub branch: String,
    /// False when the branch was kept, either because deletion was not asked
    /// for or because Git still has it checked out somewhere else.
    pub branch_deleted: bool,
}

pub struct TaskService {
    managed_root: PathBuf,
    legacy_managed_roots: Vec<PathBuf>,
}

impl TaskService {
    pub fn new(paths: &AppPaths) -> Self {
        // Tasks imported from the former Electron application keep their
        // original absolute paths. Trust only that exact historical sibling
        // directory; new tasks continue to be created in `managed_root`.
        let legacy_managed_roots = paths
            .data_dir
            .parent()
            .map(|application_support| {
                application_support
                    .join("blackholes-desktop")
                    .join("task-workspaces")
            })
            .into_iter()
            .collect();
        Self {
            managed_root: paths.task_workspaces.clone(),
            legacy_managed_roots,
        }
    }

    fn assert_managed_task_path(&self, path: &Path) -> Result<()> {
        if is_managed_child(&self.managed_root, path)
            || self
                .legacy_managed_roots
                .iter()
                .any(|root| is_managed_child(root, path))
        {
            return Ok(());
        }
        bail!("Task worktree path escaped the managed directory");
    }

    pub fn branch_availability(
        workspace: &Workspace,
        repository_ids: &[Uuid],
        branch: &str,
        source: TaskBranchSource,
        base_ref: Option<&str>,
    ) -> Result<Vec<BranchAvailability>> {
        let branch = branch.trim();
        if branch.is_empty() {
            bail!("Enter an existing branch name");
        }
        let selected: HashSet<_> = repository_ids.iter().copied().collect();
        workspace
            .repositories
            .iter()
            .filter(|repository| selected.contains(&repository.id))
            .map(|repository| {
                validate_branch(&repository.path, branch)?;
                if source == TaskBranchSource::Remote || base_ref.is_some() {
                    run_git(
                        &repository.path,
                        ["fetch", "--prune", "--no-tags", "origin"],
                    )?;
                }
                let base = base_ref
                    .map(|base| resolve_base_ref(&repository.path, base, &repository.name))
                    .transpose()?;
                let branch = canonical_branch_name(&repository.path, branch, source);
                let branch = branch.as_str();
                let current_revision =
                    git_output(&repository.path, ["rev-parse", "--verify", "HEAD^{commit}"])?
                        .trim()
                        .to_string();
                let local_revision =
                    revision_for_ref(&repository.path, &format!("refs/heads/{branch}^{{commit}}"));
                let remote_revision = revision_for_ref(
                    &repository.path,
                    &format!("refs/remotes/origin/{branch}^{{commit}}"),
                );
                Ok(BranchAvailability {
                    repository_id: repository.id,
                    repository_name: repository.name.clone(),
                    branch: branch.to_string(),
                    current_branch: current_branch_name(&repository.path),
                    current_revision,
                    local_checked_out: local_revision.is_some()
                        && branch_is_checked_out(&repository.path, branch),
                    local_revision,
                    remote_revision,
                    base,
                })
            })
            .collect()
    }

    pub fn repair_task_files(&self, workspace: &Workspace, task: &ProjectTask) -> Result<()> {
        write_task_files(
            &task.worktree_root_path,
            task.id,
            workspace,
            &task.title,
            &task.repositories,
        )
    }

    pub fn create(&self, workspace: &Workspace, request: CreateTaskRequest) -> Result<ProjectTask> {
        let title = request.title.trim();
        if title.is_empty() {
            bail!("Enter a task title");
        }
        let unique_ids: HashSet<_> = request.repository_ids.iter().copied().collect();
        if unique_ids.is_empty() {
            bail!("Choose at least one repository for the task");
        }
        if unique_ids.len() != request.repository_ids.len() {
            bail!("Choose each repository once");
        }
        let repositories = request
            .repository_ids
            .iter()
            .map(|id| {
                workspace
                    .repositories
                    .iter()
                    .find(|repository| repository.id == *id)
                    .cloned()
                    .context("A task can only use repositories from its project")
            })
            .collect::<Result<Vec<_>>>()?;

        let task_id = Uuid::new_v4();
        let requested_branch = request
            .branch_name
            .as_deref()
            .filter(|branch| !branch.trim().is_empty());
        if request.branch_source != TaskBranchSource::Current && requested_branch.is_none() {
            bail!("Enter the existing branch name");
        }
        // A requested branch is used verbatim. It normally names a branch that
        // already exists on the remote, such as a pull request head, and
        // rewriting its casing or punctuation would point the task at a
        // different ref than the caller asked for. Only a branch derived from
        // the task title is normalized, because that one is being invented here.
        let branch = match requested_branch {
            Some(requested) => requested.trim().to_string(),
            None => normalize_branch_name(title),
        };
        validate_branch(&repositories[0].path, &branch)?;

        let project_directory = format!(
            "{}-{}",
            safe_segment(workspace.label()),
            &workspace.id.simple().to_string()[..8]
        );
        let task_directory = format!(
            "{}-{}",
            safe_segment(title),
            &task_id.simple().to_string()[..8]
        );
        let project_root = self.managed_root.join(project_directory);
        let task_root = project_root.join(task_directory);
        assert_managed_child(&self.managed_root, &task_root)?;
        fs::create_dir_all(&project_root)?;
        fs::create_dir(&task_root).context("The isolated workspace already exists")?;

        let mut created = Vec::<CreatedWorktree>::new();
        let mut task_repositories = Vec::new();
        let result = (|| {
            let mut directory_names = HashSet::new();
            for repository in &repositories {
                let preparation = request
                    .preparations
                    .get(&repository.id)
                    .cloned()
                    .unwrap_or_default();
                let worktree = create_worktree(
                    task_id,
                    repository,
                    &task_root,
                    &branch,
                    request.branch_source,
                    request.base_ref.as_deref(),
                    request.create_missing_branch,
                    request.replace_divergent_local_branches,
                    request.existing_branch_action,
                    preparation,
                    &mut directory_names,
                )?;
                task_repositories.push(worktree.task_repository.clone());
                created.push(worktree);
            }
            write_task_files(&task_root, task_id, workspace, title, &task_repositories)?;
            Ok::<_, anyhow::Error>(())
        })();

        if let Err(error) = result {
            rollback_worktrees(&created);
            let _ = fs::remove_dir_all(&task_root);
            return Err(error);
        }

        let now = Utc::now();
        Ok(ProjectTask {
            id: task_id,
            workspace_id: workspace.id,
            title: title.into(),
            description: request.description.and_then(non_empty),
            icon: DEFAULT_TASK_ICON.into(),
            color: workspace.color,
            sort_order: 0,
            worktree_root_path: fs::canonicalize(&task_root).unwrap_or(task_root),
            repositories: task_repositories,
            sessions: vec![],
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update(
        &self,
        workspace: &Workspace,
        task: &mut ProjectTask,
        title: String,
        description: Option<String>,
        color: WorkspaceColor,
    ) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("Task title cannot be empty");
        }
        let mut updated = task.clone();
        updated.title = title.into();
        updated.description = description.and_then(non_empty);
        updated.color = color;
        updated.updated_at = Utc::now();
        write_task_files(
            &updated.worktree_root_path,
            updated.id,
            workspace,
            &updated.title,
            &updated.repositories,
        )?;
        *task = updated;
        Ok(())
    }

    pub fn add_repositories(
        &self,
        workspace: &Workspace,
        task: &mut ProjectTask,
        request: AddTaskRepositoriesRequest,
    ) -> Result<()> {
        let branch = task
            .branch()
            .context("The task does not have a branch")?
            .to_string();
        let current: HashSet<_> = task
            .repositories
            .iter()
            .map(|repository| repository.repository_id)
            .collect();
        let mut directory_names = task
            .repositories
            .iter()
            .filter_map(|repository| {
                repository
                    .worktree_path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .map(str::to_string)
            })
            .collect::<HashSet<_>>();
        let mut created = Vec::new();

        for repository_id in request.repository_ids {
            if current.contains(&repository_id) {
                continue;
            }
            let repository = workspace
                .repositories
                .iter()
                .find(|repository| repository.id == repository_id)
                .context("A task can only use repositories from its project")?;
            let preparation = request
                .preparations
                .get(&repository_id)
                .cloned()
                .unwrap_or_default();
            match create_worktree(
                task.id,
                repository,
                &task.worktree_root_path,
                &branch,
                request.branch_source,
                request.base_ref.as_deref(),
                request.create_missing_branch,
                request.replace_divergent_local_branches,
                request.existing_branch_action,
                preparation,
                &mut directory_names,
            ) {
                Ok(worktree) => created.push(worktree),
                Err(error) => {
                    rollback_worktrees(&created);
                    return Err(error);
                }
            }
        }
        let mut repositories = task.repositories.clone();
        repositories.extend(
            created
                .iter()
                .map(|worktree| worktree.task_repository.clone()),
        );
        if let Err(error) = write_task_files(
            &task.worktree_root_path,
            task.id,
            workspace,
            &task.title,
            &repositories,
        ) {
            rollback_worktrees(&created);
            let _ = write_task_files(
                &task.worktree_root_path,
                task.id,
                workspace,
                &task.title,
                &task.repositories,
            );
            return Err(error);
        }
        task.repositories = repositories;
        task.updated_at = Utc::now();
        Ok(())
    }

    /// Detach repositories from a task, undoing a wrong `add_repositories`.
    ///
    /// The Git branch survives by default, so the worktree can be recreated
    /// exactly as it was. Ask for `delete_branch` when the point of removing it
    /// is to add it back on a different base, since an existing branch is
    /// reused untouched.
    pub fn remove_repositories(
        &self,
        workspace: &Workspace,
        task: &mut ProjectTask,
        request: RemoveTaskRepositoriesRequest,
    ) -> Result<Vec<RemovedTaskRepository>> {
        self.assert_managed_task_path(&task.worktree_root_path)?;
        let requested: HashSet<_> = request.repository_ids.iter().copied().collect();
        if requested.is_empty() {
            bail!("Choose at least one repository to remove");
        }
        let selected = task
            .repositories
            .iter()
            .filter(|repository| requested.contains(&repository.repository_id))
            .cloned()
            .collect::<Vec<_>>();
        if selected.len() != requested.len() {
            bail!("A repository to remove is not attached to this task");
        }
        // Every repository is added on the branch of the first one, so a task
        // without repositories no longer knows which branch to reuse.
        if selected.len() == task.repositories.len() {
            bail!(
                "A task keeps at least one repository, because the rest are added on its branch; delete the whole task instead"
            );
        }
        for repository in &selected {
            if task.sessions.iter().any(|session| {
                session.repository_id == Some(repository.repository_id)
                    && !matches!(
                        session.state,
                        crate::model::SessionState::Exited | crate::model::SessionState::Restored
                    )
            }) {
                bail!(
                    "Close the terminals of {} before removing it",
                    repository_label(workspace, repository)
                );
            }
            assert_managed_child(&task.worktree_root_path, &repository.worktree_path)?;
            if !request.discard_uncommitted_changes
                && repository.worktree_path.exists()
                && !git_output(&repository.worktree_path, ["status", "--porcelain"])?
                    .trim()
                    .is_empty()
            {
                bail!(
                    "{} contains uncommitted changes",
                    repository.worktree_path.display()
                );
            }
        }

        let mut removed = Vec::new();
        for repository in &selected {
            let source = repository_source_path(&repository.worktree_path).or_else(|_| {
                workspace
                    .repositories
                    .iter()
                    .find(|candidate| candidate.id == repository.repository_id)
                    .map(|candidate| candidate.path.clone())
                    .context("Unable to resolve the original repository")
            })?;
            let worktree_path = path_text(&repository.worktree_path)?;
            // The uncommitted work was already accounted for above, so the
            // forced retry only covers what Git alone refuses to discard, such
            // as ignored files a setup command left behind.
            if run_git(&source, ["worktree", "remove", "--", worktree_path]).is_err() {
                run_git(
                    &source,
                    ["worktree", "remove", "--force", "--", worktree_path],
                )
                .with_context(|| format!("Unable to remove the worktree at {worktree_path}"))?;
            }
            if repository.worktree_path.exists() {
                remove_directory_with_permission_repair(&repository.worktree_path, || {
                    assert_managed_child(&task.worktree_root_path, &repository.worktree_path)
                })?;
            }
            let branch_deleted = request.delete_branch
                && run_git(&source, ["branch", "-D", "--", &repository.branch]).is_ok();
            let _ = run_git(&source, ["worktree", "prune"]);
            removed.push(RemovedTaskRepository {
                repository_id: repository.repository_id,
                worktree_path: repository.worktree_path.clone(),
                branch: repository.branch.clone(),
                branch_deleted,
            });
        }

        let repositories = task
            .repositories
            .iter()
            .filter(|repository| !requested.contains(&repository.repository_id))
            .cloned()
            .collect::<Vec<_>>();
        write_task_files(
            &task.worktree_root_path,
            task.id,
            workspace,
            &task.title,
            &repositories,
        )?;
        task.repositories = repositories;
        task.updated_at = Utc::now();
        Ok(removed)
    }

    pub fn remove(&self, task: &ProjectTask) -> Result<()> {
        self.assert_managed_task_path(&task.worktree_root_path)?;
        assert_task_repository_paths(task)?;
        if task.sessions.iter().any(|session| {
            !matches!(
                session.state,
                crate::model::SessionState::Exited | crate::model::SessionState::Restored
            )
        }) {
            bail!("Close the task terminals before removing it");
        }
        for repository in &task.repositories {
            if !git_output(&repository.worktree_path, ["status", "--porcelain"])?
                .trim()
                .is_empty()
            {
                bail!(
                    "{} contains uncommitted changes",
                    repository.worktree_path.display()
                );
            }
        }
        for repository in &task.repositories {
            let source = repository_source_path(&repository.worktree_path)?;
            run_git(
                &source,
                [
                    "worktree",
                    "remove",
                    "--",
                    path_text(&repository.worktree_path)?,
                ],
            )?;
        }
        if task.worktree_root_path.exists() {
            fs::remove_dir_all(&task.worktree_root_path)?;
        }
        Ok(())
    }

    pub fn remove_permanently(&self, task: &ProjectTask) -> Result<()> {
        self.assert_managed_task_path(&task.worktree_root_path)?;
        assert_task_repository_paths(task)?;

        let mut repository_sources = Vec::new();
        for repository in &task.repositories {
            let source = repository_source_path(&repository.worktree_path).ok();
            if let Some(source) = source.as_ref() {
                let _ = run_git(
                    source,
                    [
                        "worktree",
                        "remove",
                        "--force",
                        "--force",
                        "--",
                        path_text(&repository.worktree_path)?,
                    ],
                );
                repository_sources.push(source.clone());
            }

            if repository.worktree_path.exists() {
                remove_directory_with_permission_repair(&repository.worktree_path, || {
                    assert_managed_child(&task.worktree_root_path, &repository.worktree_path)
                })
                .with_context(|| {
                    format!(
                        "Could not delete task worktree {}",
                        repository.worktree_path.display()
                    )
                })?;
            }
        }

        if task.worktree_root_path.exists() {
            remove_directory_with_permission_repair(&task.worktree_root_path, || {
                self.assert_managed_task_path(&task.worktree_root_path)
            })
            .with_context(|| {
                format!(
                    "Could not delete task workspace {}",
                    task.worktree_root_path.display()
                )
            })?;
        }

        repository_sources.sort();
        repository_sources.dedup();
        for source in repository_sources {
            let _ = run_git(&source, ["worktree", "prune"]);
        }

        Ok(())
    }
}

#[derive(Clone)]
struct CreatedWorktree {
    repository_path: PathBuf,
    worktree_path: PathBuf,
    branch: String,
    branch_created: bool,
    original_branch_revision: Option<String>,
    backup_branch: Option<String>,
    task_repository: TaskRepository,
}

struct PreparedBranch {
    base_revision: String,
    /// Set only when the branch was rooted in an explicitly requested base, so
    /// the task can record what it branched from instead of guessing.
    base_label: Option<String>,
    branch_created: bool,
    original_revision: Option<String>,
    backup_branch: Option<String>,
}

fn create_worktree(
    _task_id: Uuid,
    repository: &Repository,
    task_root: &Path,
    branch: &str,
    branch_source: TaskBranchSource,
    base_ref: Option<&str>,
    create_missing_branch: bool,
    replace_divergent_local_branches: bool,
    existing_branch_action: ExistingBranchAction,
    preparation: RepositoryPreparation,
    directory_names: &mut HashSet<String>,
) -> Result<CreatedWorktree> {
    let repository_path = fs::canonicalize(&repository.path)?;
    validate_branch(&repository_path, branch)?;
    // An explicit base is read from the refs below, so it has to be as fresh
    // as a remote branch lookup would be.
    if branch_source == TaskBranchSource::Remote || base_ref.is_some() {
        run_git(
            &repository_path,
            ["fetch", "--prune", "--no-tags", "origin"],
        )?;
    }
    let canonical_branch = canonical_branch_name(&repository_path, branch, branch_source);
    let branch = canonical_branch.as_str();
    align_local_branch_case(&repository_path, branch)?;
    let current_revision =
        git_output(&repository_path, ["rev-parse", "--verify", "HEAD^{commit}"])?;
    let current_revision = current_revision.trim().to_string();
    let current_branch = git_output_optional(
        &repository_path,
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map(|branch| branch.trim().to_string())
    .filter(|branch| !branch.is_empty());
    let local_branch_existed =
        revision_for_ref(&repository_path, &format!("refs/heads/{branch}^{{commit}}")).is_some();

    let directory_name = unique_child_name(&repository.name, directory_names);
    let worktree_path = task_root.join(directory_name);
    if worktree_path.exists() {
        bail!("The task worktree destination already exists");
    }

    let base = base_ref
        .map(|base| resolve_base_ref(&repository_path, base, &repository.name))
        .transpose()?;
    let prepared_branch = prepare_branch(
        &repository_path,
        branch,
        &current_revision,
        branch_source,
        base.as_ref(),
        create_missing_branch,
        replace_divergent_local_branches,
        existing_branch_action,
    )?;
    let remote_branch_exists = revision_for_ref(
        &repository_path,
        &format!("refs/remotes/origin/{branch}^{{commit}}"),
    )
    .is_some();
    let base_branch = prepared_branch.base_label.clone().or(match branch_source {
        TaskBranchSource::Remote if remote_branch_exists => Some(format!("origin/{branch}")),
        TaskBranchSource::Local if local_branch_existed => Some(branch.to_string()),
        TaskBranchSource::Current
            if local_branch_existed && existing_branch_action == ExistingBranchAction::Reuse =>
        {
            Some(branch.to_string())
        }
        _ => current_branch,
    });
    if let Err(error) = run_git(
        &repository_path,
        ["worktree", "add", "--", path_text(&worktree_path)?, branch],
    ) {
        rollback_prepared_branch(&repository_path, branch, &prepared_branch);
        return Err(error);
    }

    if branch_source == TaskBranchSource::Remote
        && remote_branch_exists
        && let Err(error) = run_git(
            &worktree_path,
            [
                "branch",
                "--set-upstream-to",
                &format!("origin/{branch}"),
                branch,
            ],
        )
    {
        let _ = run_git(
            &repository_path,
            [
                "worktree",
                "remove",
                "--force",
                "--",
                path_text(&worktree_path)?,
            ],
        );
        rollback_prepared_branch(&repository_path, branch, &prepared_branch);
        return Err(error);
    }

    let preparation_result = prepare_worktree(&repository_path, &worktree_path, &preparation);
    if let Err(error) = preparation_result {
        let _ = run_git(
            &repository_path,
            [
                "worktree",
                "remove",
                "--force",
                "--",
                path_text(&worktree_path)?,
            ],
        );
        rollback_prepared_branch(&repository_path, branch, &prepared_branch);
        return Err(error);
    }

    let copied_environment_files = if preparation.copy_environment_files {
        environment_file_names(&repository_path)
    } else {
        vec![]
    };
    let now = Utc::now();
    Ok(CreatedWorktree {
        repository_path,
        worktree_path: worktree_path.clone(),
        branch: branch.into(),
        branch_created: prepared_branch.branch_created,
        original_branch_revision: prepared_branch.original_revision,
        backup_branch: prepared_branch.backup_branch,
        task_repository: TaskRepository {
            repository_id: repository.id,
            worktree_path,
            branch: branch.into(),
            base_branch,
            base_revision: prepared_branch.base_revision,
            copy_local_changes: preparation.copy_local_changes,
            copy_environment_files: preparation.copy_environment_files,
            copied_environment_files,
            setup_command: preparation.setup_command.and_then(non_empty),
            prepared_at: Some(now),
            added_at: now,
        },
    })
}

fn prepare_branch(
    repository: &Path,
    branch: &str,
    current_revision: &str,
    source: TaskBranchSource,
    base: Option<&ResolvedBase>,
    create_missing: bool,
    replace_divergent: bool,
    existing_action: ExistingBranchAction,
) -> Result<PreparedBranch> {
    // `create_worktree` already fetched origin before resolving the branch name,
    // so the remote refs read below are current.
    let local_revision = revision_for_ref(repository, &format!("refs/heads/{branch}^{{commit}}"));
    let remote_revision = revision_for_ref(
        repository,
        &format!("refs/remotes/origin/{branch}^{{commit}}"),
    );
    let checked_out = local_revision.is_some() && branch_is_checked_out(repository, branch);

    // A branch that has to be invented starts at the requested base when there
    // is one. Without a base it keeps starting at the repository HEAD, which is
    // whatever branch the developer happened to leave checked out.
    let start_revision = base.map_or(current_revision, |base| base.revision.as_str());
    let start_label = base.map(|base| base.label.clone());
    let create_at = |revision: &str| -> Result<PreparedBranch> {
        run_git(repository, ["branch", "--", branch, revision])?;
        Ok(PreparedBranch {
            base_revision: revision.into(),
            base_label: start_label.clone(),
            branch_created: true,
            original_revision: None,
            backup_branch: None,
        })
    };

    match source {
        TaskBranchSource::Current => match local_revision {
            None => create_at(start_revision),
            Some(local) if existing_action == ExistingBranchAction::Reuse => {
                if checked_out {
                    bail!("Branch '{branch}' is already checked out in another worktree");
                }
                Ok(PreparedBranch {
                    base_revision: local,
                    base_label: None,
                    branch_created: false,
                    original_revision: None,
                    backup_branch: None,
                })
            }
            Some(local) => {
                if checked_out {
                    bail!("Branch '{branch}' is already checked out in another worktree");
                }
                replace_branch(
                    repository,
                    branch,
                    &local,
                    start_revision,
                    start_label.clone(),
                )
            }
        },
        TaskBranchSource::Local => match local_revision {
            Some(local) => {
                if checked_out {
                    bail!("Branch '{branch}' is already checked out in another worktree");
                }
                Ok(PreparedBranch {
                    base_revision: local,
                    base_label: None,
                    branch_created: false,
                    original_revision: None,
                    backup_branch: None,
                })
            }
            None if create_missing => create_at(start_revision),
            None => bail!("Local branch '{branch}' was not found"),
        },
        TaskBranchSource::Remote => match (remote_revision, local_revision) {
            (Some(remote), Some(local)) if remote != local => {
                if checked_out {
                    bail!("Branch '{branch}' is already checked out in another worktree");
                }
                if !replace_divergent {
                    bail!(
                        "Local branch '{branch}' differs from origin/{branch}; confirm replacement"
                    );
                }
                replace_branch(repository, branch, &local, &remote, None)
            }
            (Some(remote), Some(_)) => {
                if checked_out {
                    bail!("Branch '{branch}' is already checked out in another worktree");
                }
                Ok(PreparedBranch {
                    base_revision: remote,
                    base_label: None,
                    branch_created: false,
                    original_revision: None,
                    backup_branch: None,
                })
            }
            (Some(remote), None) => create_at(&remote),
            (None, Some(_)) if create_missing => bail!(
                "Remote branch origin/{branch} is missing but a same-named local branch exists"
            ),
            (None, None) if create_missing => create_at(start_revision),
            (None, _) => bail!("Remote branch 'origin/{branch}' was not found"),
        },
    }
}

fn replace_branch(
    repository: &Path,
    branch: &str,
    original_revision: &str,
    replacement_revision: &str,
    base_label: Option<String>,
) -> Result<PreparedBranch> {
    let backup = format!(
        "blackholes/backup/{}-{}",
        branch.replace('/', "-"),
        &Uuid::new_v4().simple().to_string()[..8]
    );
    run_git(repository, ["branch", "--", &backup, original_revision])?;
    if let Err(error) = run_git(
        repository,
        ["branch", "-f", "--", branch, replacement_revision],
    ) {
        let _ = run_git(repository, ["branch", "-D", "--", &backup]);
        return Err(error);
    }
    Ok(PreparedBranch {
        base_revision: replacement_revision.into(),
        base_label,
        branch_created: false,
        original_revision: Some(original_revision.into()),
        backup_branch: Some(backup),
    })
}

fn rollback_prepared_branch(repository: &Path, branch: &str, prepared: &PreparedBranch) {
    if prepared.branch_created {
        let _ = run_git(repository, ["branch", "-D", "--", branch]);
    } else if let Some(original) = prepared.original_revision.as_deref() {
        let _ = run_git(repository, ["branch", "-f", "--", branch, original]);
    }
    if let Some(backup) = prepared.backup_branch.as_deref() {
        let _ = run_git(repository, ["branch", "-D", "--", backup]);
    }
}

fn prepare_worktree(
    source: &Path,
    target: &Path,
    preparation: &RepositoryPreparation,
) -> Result<()> {
    if preparation.copy_local_changes {
        let patch = Command::new("git")
            .current_dir(source)
            .args(["diff", "--binary", "HEAD"])
            .output()?;
        if !patch.status.success() {
            bail!("Unable to capture local changes");
        }
        if !patch.stdout.is_empty() {
            let mut child = Command::new("git")
                .current_dir(target)
                .args(["apply", "--whitespace=nowarn", "-"])
                .stdin(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .context("Unable to open git apply")?
                .write_all(&patch.stdout)?;
            if !child.wait()?.success() {
                bail!("Unable to apply local changes to the task worktree");
            }
        }
        copy_untracked_files(source, target)?;
    }
    if preparation.copy_environment_files {
        for file_name in environment_file_names(source) {
            let source_file = source.join(&file_name);
            let target_file = target.join(&file_name);
            fs::copy(source_file, target_file)?;
        }
    }
    if let Some(command) = preparation.setup_command.as_deref().and_then(non_empty_ref) {
        let status = Command::new("/bin/zsh")
            .current_dir(target)
            .args(["-lc", command])
            .status()?;
        if !status.success() {
            bail!("The setup command failed with status {status}");
        }
    }
    Ok(())
}

fn copy_untracked_files(source: &Path, target: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(source)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;
    if !output.status.success() {
        bail!("Unable to list untracked files");
    }
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = PathBuf::from(String::from_utf8_lossy(bytes).as_ref());
        let source_file = source.join(&relative);
        let target_file = target.join(&relative);
        if !source_file.is_file() {
            continue;
        }
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source_file, target_file)?;
    }
    Ok(())
}

fn environment_file_names(source: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(source) else {
        return vec![];
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (entry.path().is_file() && (name == ".env" || name.starts_with(".env.")))
                .then_some(name)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn rollback_worktrees(worktrees: &[CreatedWorktree]) {
    for worktree in worktrees.iter().rev() {
        let _ = run_git(
            &worktree.repository_path,
            [
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.worktree_path.to_string_lossy().as_ref(),
            ],
        );
        if worktree.branch_created {
            let _ = run_git(
                &worktree.repository_path,
                ["branch", "-D", "--", worktree.branch.as_str()],
            );
        } else if let Some(original) = worktree.original_branch_revision.as_deref() {
            let _ = run_git(
                &worktree.repository_path,
                ["branch", "-f", "--", worktree.branch.as_str(), original],
            );
        }
        if let Some(backup) = worktree.backup_branch.as_deref() {
            let _ = run_git(&worktree.repository_path, ["branch", "-D", "--", backup]);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskManifest<'a> {
    version: u8,
    task_id: Uuid,
    workspace_id: Uuid,
    title: &'a str,
    repositories: &'a [TaskRepository],
}

fn write_task_files(
    task_root: &Path,
    task_id: Uuid,
    workspace: &Workspace,
    title: &str,
    repositories: &[TaskRepository],
) -> Result<()> {
    let task_instructions = ProjectTaskInstructionsService::read(workspace)?;
    let manifest = TaskManifest {
        version: 1,
        task_id,
        workspace_id: workspace.id,
        title,
        repositories,
    };
    write_private_file(
        &task_root.join(".blackholes-task.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;

    let mut context = format!(
        "<!-- Generated by Blackholes. Do not edit this task-specific header. -->\n# Blackholes task context\n\n- Task: `{}`\n- Task ID: `{}`\n- Project: `{}`\n- Project ID: `{}`\n",
        markdown_inline(title),
        task_id,
        markdown_inline(workspace.label()),
        workspace.id,
    );
    if !task_instructions.trim().is_empty() {
        context.push('\n');
        context.push_str(task_instructions.trim_end());
        context.push('\n');
    }
    let claude_path = task_root.join("CLAUDE.md");
    let agents_path = task_root.join("AGENTS.md");
    assert_managed_context_file(&claude_path)?;
    assert_managed_agents_file(&agents_path)?;
    write_private_file(&claude_path, context.as_bytes())?;
    if agents_path.exists() || fs::symlink_metadata(&agents_path).is_ok() {
        fs::remove_file(&agents_path)?;
    }
    if let Err(error) = std::os::unix::fs::symlink("CLAUDE.md", &agents_path) {
        if matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
        ) {
            write_private_file(&agents_path, context.as_bytes())?;
        } else {
            return Err(error.into());
        }
    }
    TaskNoteService::ensure(task_root, "")?;
    Ok(())
}

fn assert_managed_context_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!(
            "{} is not a regular Blackholes context file",
            path.display()
        );
    }
    let current = fs::read_to_string(path)?;
    if !current.starts_with("<!-- Generated") {
        bail!("{} is not managed by Blackholes", path.display());
    }
    Ok(())
}

fn assert_managed_agents_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() && fs::read_link(path)? == PathBuf::from("CLAUDE.md") {
        return Ok(());
    }
    assert_managed_context_file(path)
}

fn write_private_file(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content)?;
    Ok(())
}

fn markdown_inline(value: &str) -> String {
    value
        .replace('`', "'")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

fn repository_source_path(worktree_path: &Path) -> Result<PathBuf> {
    let common = git_output(worktree_path, ["rev-parse", "--git-common-dir"])?;
    let common = PathBuf::from(common.trim());
    let common = if common.is_absolute() {
        common
    } else {
        worktree_path.join(common)
    };
    let common = fs::canonicalize(common)?;
    common
        .parent()
        .map(Path::to_path_buf)
        .context("Unable to resolve the original repository")
}

fn validate_branch(repository: &Path, branch: &str) -> Result<()> {
    if branch.is_empty() || branch.starts_with('-') {
        bail!("Enter a valid Git branch name");
    }
    if !git_success(repository, ["check-ref-format", "--branch", branch]) {
        bail!("'{branch}' is not a valid Git branch name");
    }
    Ok(())
}

/// Find how Git actually spells a ref, ignoring the casing the caller used.
///
/// `git rev-parse refs/heads/cu-x` happily resolves a ref stored as
/// `refs/heads/CU-x`, because on macOS the loose ref file lives on a
/// case-insensitive filesystem. Git servers are case-sensitive, so a worktree
/// checked out under the wrong casing pushes to a brand new remote branch
/// instead of the intended one. Reading the ref listing recovers the real name:
/// `for-each-ref` reports the name as stored, not as asked for.
fn canonical_ref_name(repository: &Path, namespace: &str, branch: &str) -> Option<String> {
    let listing = git_output_optional(
        repository,
        ["for-each-ref", "--format=%(refname)", namespace],
    )?;
    let prefix = format!("{namespace}/");
    listing.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .filter(|name| name.eq_ignore_ascii_case(branch))
            .map(str::to_string)
    })
}

/// Resolve `branch` to the casing Git stores, preferring the side that owns the
/// name for this source: origin decides for a remote branch, the local ref
/// decides otherwise. Unknown branches keep the requested spelling, since those
/// are about to be created.
fn canonical_branch_name(repository: &Path, branch: &str, source: TaskBranchSource) -> String {
    let local = canonical_ref_name(repository, "refs/heads", branch);
    let remote = canonical_ref_name(repository, "refs/remotes/origin", branch);
    match source {
        TaskBranchSource::Remote => remote.or(local),
        _ => local.or(remote),
    }
    .unwrap_or_else(|| branch.to_string())
}

/// Rename a local branch that differs from `branch` only by casing.
///
/// Without this, `git worktree add <path> CU-x` against a local ref stored as
/// `cu-x` leaves the worktree tracking a name that does not exist on the
/// remote. Git refuses a direct case-only rename on a case-insensitive
/// filesystem, so the rename goes through a temporary name. Both names denote
/// the same commit, so no work can be lost here.
fn align_local_branch_case(repository: &Path, branch: &str) -> Result<()> {
    let Some(existing) = canonical_ref_name(repository, "refs/heads", branch) else {
        return Ok(());
    };
    if existing == branch {
        return Ok(());
    }
    if branch_is_checked_out(repository, &existing) {
        bail!("Branch '{existing}' is already checked out in another worktree");
    }
    let temporary = format!("blackholes/rename/{}", Uuid::new_v4().simple());
    run_git(repository, ["branch", "-m", &existing, &temporary])
        .with_context(|| format!("Unable to rename branch '{existing}' to match '{branch}'"))?;
    if let Err(error) = run_git(repository, ["branch", "-m", &temporary, branch]) {
        let _ = run_git(repository, ["branch", "-m", &temporary, &existing]);
        return Err(error)
            .with_context(|| format!("Unable to rename branch '{existing}' to match '{branch}'"));
    }
    Ok(())
}

/// Resolve the base a new task branch should start from.
///
/// Callers name it the way a person does: `master`, `origin/master`,
/// `release/2026-08`, a tag or a raw revision. Origin owns the name first,
/// because a local branch of the same name can lag behind by weeks and a task
/// branch quietly rooted in stale history is the mistake an explicit base is
/// meant to rule out. Casing comes from the ref listing for the same reason
/// [`canonical_branch_name`] reads it.
fn resolve_base_ref(repository: &Path, base: &str, repository_name: &str) -> Result<ResolvedBase> {
    let base = base.trim();
    if base.is_empty() || base.starts_with('-') {
        bail!("'{base}' is not a valid base branch, tag or revision");
    }
    if !base.starts_with("refs/") {
        for (namespace, label_prefix) in [
            ("refs/remotes/origin", "origin/"),
            ("refs/remotes", ""),
            ("refs/heads", ""),
        ] {
            let Some(name) = canonical_ref_name(repository, namespace, base) else {
                continue;
            };
            if let Some(revision) =
                revision_for_ref(repository, &format!("{namespace}/{name}^{{commit}}"))
            {
                return Ok(ResolvedBase {
                    label: format!("{label_prefix}{name}"),
                    revision,
                });
            }
        }
    }
    // Tags and raw revisions, plus any fully spelled `refs/...` ref.
    if let Some(revision) = revision_for_ref(repository, &format!("{base}^{{commit}}")) {
        return Ok(ResolvedBase {
            label: base.to_string(),
            revision,
        });
    }
    bail!("Base '{base}' was not found in {repository_name}")
}

/// Name a task repository the way the project does, falling back to the
/// worktree directory when the project no longer lists it.
fn repository_label(workspace: &Workspace, repository: &TaskRepository) -> String {
    workspace
        .repositories
        .iter()
        .find(|candidate| candidate.id == repository.repository_id)
        .map(|candidate| candidate.name.clone())
        .or_else(|| {
            repository
                .worktree_path
                .file_name()
                .and_then(OsStr::to_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| repository.repository_id.to_string())
}

fn revision_for_ref(repository: &Path, reference: &str) -> Option<String> {
    git_output_optional(repository, ["rev-parse", "--verify", reference])
        .map(|revision| revision.trim().to_string())
        .filter(|revision| !revision.is_empty())
}

fn current_branch_name(repository: &Path) -> Option<String> {
    git_output_optional(repository, ["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

fn branch_is_checked_out(repository: &Path, branch: &str) -> bool {
    let Some(worktrees) = git_output_optional(repository, ["worktree", "list", "--porcelain"])
    else {
        return false;
    };
    let expected = format!("branch refs/heads/{branch}");
    worktrees.lines().any(|line| line.trim() == expected)
}

fn normalize_branch_name(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character);
            separator = false;
        } else if matches!(character, '-' | '_' | '.' | '/') {
            if !separator && !output.is_empty() {
                output.push(character);
                separator = true;
            }
        } else if !separator && !output.is_empty() {
            output.push('-');
            separator = true;
        }
    }
    let output = output.trim_matches(['-', '_', '.', '/']).to_string();
    if output.is_empty() {
        "task".into()
    } else {
        output
    }
}

fn safe_segment(value: &str) -> String {
    let segment = normalize_branch_name(value).replace('/', "-");
    segment.chars().take(64).collect()
}

fn unique_child_name(value: &str, used: &mut HashSet<String>) -> String {
    let base = safe_segment(value);
    if used.insert(base.clone()) {
        return base;
    }
    for index in 2..10_000 {
        let candidate = format!("{base}-{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    Uuid::new_v4().simple().to_string()
}

fn assert_managed_child(managed_root: &Path, child: &Path) -> Result<()> {
    if !is_managed_child(managed_root, child) {
        bail!("Task worktree path escaped the managed directory");
    }
    Ok(())
}

fn is_managed_child(managed_root: &Path, child: &Path) -> bool {
    if fs::symlink_metadata(managed_root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return false;
    }
    let managed = fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
    let normalized_child = fs::canonicalize(child).unwrap_or_else(|_| {
        child
            .parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .map(|parent| parent.join(child.file_name().unwrap_or_default()))
            .unwrap_or_else(|| child.to_path_buf())
    });
    normalized_child != managed && normalized_child.starts_with(&managed)
}

fn assert_task_repository_paths(task: &ProjectTask) -> Result<()> {
    for repository in &task.repositories {
        if !is_managed_child(&task.worktree_root_path, &repository.worktree_path) {
            bail!("Task repository worktree escaped the task directory");
        }
    }
    Ok(())
}

fn remove_directory_with_permission_repair(
    path: &Path,
    validate: impl Fn() -> Result<()>,
) -> Result<()> {
    validate()?;
    match fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => return Err(error.into()),
    }

    // The first attempt may have removed part of the tree. Revalidate the
    // remaining path immediately before changing permissions and retrying.
    if !path.exists() {
        return Ok(());
    }
    validate()?;
    repair_delete_permissions(path)?;
    fs::remove_dir_all(path)?;
    Ok(())
}

fn repair_delete_permissions(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Generated caches can carry an explicit macOS ACL such as
        // `deny delete`. `-P` guarantees recursive chmod never follows a
        // symbolic link outside the already validated task tree.
        let output = Command::new("/bin/chmod")
            .args(["-R", "-P", "-N"])
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .context("Could not remove macOS ACLs from the task workspace")?;
        if !output.status.success() {
            bail!(
                "Could not remove macOS ACLs from the task workspace: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    make_directories_owner_writable(path)
}

fn make_directories_owner_writable(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }

    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o700);
    fs::set_permissions(path, permissions)?;
    for entry in fs::read_dir(path)? {
        make_directories_owner_writable(&entry?.path())?;
    }
    Ok(())
}

fn run_git<'a>(directory: &Path, arguments: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let status = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("Unable to run Git in {}", directory.display()))?;
    if !status.success() {
        bail!("Git failed with status {status}");
    }
    Ok(())
}

fn git_output<'a>(
    directory: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!(
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_output_optional<'a>(
    directory: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_success<'a>(directory: &Path, arguments: impl IntoIterator<Item = &'a str>) -> bool {
    Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str().context("A Git path is not valid UTF-8")
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn non_empty_ref(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
