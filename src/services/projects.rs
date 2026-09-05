use crate::model::{DEFAULT_PROJECT_ICON, Repository, Workspace, WorkspaceColor, WorkspaceLayout};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;
use walkdir::WalkDir;

pub struct ProjectService;

#[derive(Clone, Copy, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRepositoryMode {
    #[default]
    Link,
    Copy,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProjectRepositorySource {
    Local(PathBuf),
    Github(String),
}

/// Probe Apple's selected developer directory without invoking the Git shim
/// (which can otherwise open an unexpected installation dialog).
pub fn git_tools_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/xcode-select").arg("--print-path")
            .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| Path::new(String::from_utf8_lossy(&output.stdout).trim()).join("usr/bin/git").is_file())
    }
    #[cfg(not(target_os = "macos"))]
    { true }
}

#[derive(Clone, Debug, Default)]
pub struct RepositoryGitSummary {
    pub branch: Option<String>,
    pub additions: u64,
    pub deletions: u64,
}

impl ProjectService {
    /// Prepare only the explicitly selected repositories in a new container.
    /// Validate every source first; rollback is limited to this new container.
    pub fn create_with_repositories(
        projects_root: &Path,
        name: &str,
        sources: Vec<ProjectRepositorySource>,
        mode: ProjectRepositoryMode,
    ) -> Result<Workspace> {
        if !sources.is_empty() { require_git_tools()?; }
        let mut seen = HashSet::new();
        let mut validated = Vec::new();
        for source in sources {
            let (key, source) = match source {
                ProjectRepositorySource::Local(path) => {
                    let path = fs::canonicalize(path)?;
                    if !is_git_repository(&path) { bail!("A selected folder is no longer a Git repository"); }
                    (format!("local:{}", path.display()), ProjectRepositorySource::Local(path))
                }
                ProjectRepositorySource::Github(url) => {
                    let (url, _) = normalize_github_url(&url)?;
                    (format!("github:{url}"), ProjectRepositorySource::Github(url))
                }
            };
            if !seen.insert(key) { bail!("The same repository was selected more than once"); }
            validated.push(source);
        }
        let mut workspace = Self::create_empty(projects_root, name)?;
        let root = workspace.root_path.clone().context("Project root missing")?;
        for source in validated {
            let result = match source {
                ProjectRepositorySource::Local(path) => match mode {
                    ProjectRepositoryMode::Link => Self::add_existing_repository(&mut workspace, &path),
                    ProjectRepositoryMode::Copy => Self::copy_existing_repository(&mut workspace, &path),
                },
                ProjectRepositorySource::Github(url) => Self::add_github_repository(&mut workspace, &url),
            };
            if let Err(error) = result {
                if let Err(cleanup) = fs::remove_dir_all(&root) {
                    return Err(error.context(format!("Incomplete project retained at {}: {cleanup}", root.display())));
                }
                return Err(error);
            }
        }
        Ok(workspace)
    }

    pub fn add_github_repository(workspace: &mut Workspace, url: &str) -> Result<()> {
        require_git_tools()?;
        let (url, name) = normalize_github_url(url)?;
        let root = fs::canonicalize(workspace.root_path.as_ref().context("Project root missing")?)?;
        if is_git_repository(&root) { bail!("Import this legacy project into a managed container before adding repositories"); }
        let destination = root.join(unique_directory_name(&root, &name));
        fs::create_dir(&destination)?;
        let status = Command::new("git").args(["clone", "--", &url]).arg(&destination)
            .stdin(Stdio::null()).status().context("Unable to start Git clone")?;
        if !status.success() {
            let _ = fs::remove_dir_all(&destination);
            bail!("Git clone failed. Check the URL and Git authentication.");
        }
        Self::add_existing_repository(workspace, &destination)
    }

    pub fn create_empty(projects_root: &Path, display_name: &str) -> Result<Workspace> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            bail!("Enter a project name");
        }
        fs::create_dir_all(projects_root)?;
        let directory_name = unique_directory_name(projects_root, display_name);
        let root_path = projects_root.join(&directory_name);
        fs::create_dir(&root_path)
            .with_context(|| format!("Unable to create {}", root_path.display()))?;
        Ok(new_workspace(
            directory_name,
            Some(display_name.into()),
            root_path,
            WorkspaceLayout::Empty,
            vec![],
        ))
    }

    pub fn create_git_repository(projects_root: &Path, display_name: &str) -> Result<Workspace> {
        require_git_tools()?;
        let mut workspace = Self::create_empty(projects_root, display_name)?;
        let root_path = workspace
            .root_path
            .clone()
            .context("The new project does not have a root directory")?;
        let repository_path = root_path.join(&workspace.name);
        let initialization = (|| -> Result<()> {
            fs::create_dir(&repository_path).with_context(|| {
                format!(
                    "Unable to create project repository {}",
                    repository_path.display()
                )
            })?;
            initialize_git_repository(&repository_path)?;
            Self::add_existing_repository(&mut workspace, &repository_path)
        })();
        if let Err(error) = initialization {
            if let Err(cleanup_error) = fs::remove_dir_all(&root_path) {
                return Err(error.context(format!(
                    "Additionally, the incomplete project could not be removed: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        Ok(workspace)
    }

    /// Link local repositories to a new container. Managed instructions belong
    /// to the container, never to the linked source directories.
    pub fn import_existing(projects_root: &Path, path: &Path, requested_name: Option<&str>) -> Result<Workspace> {
        require_git_tools()?;
        let source = fs::canonicalize(path)?;
        let repositories = discover_repositories(&source)?;
        if repositories.is_empty() {
            bail!("No Git repositories found. Choose a repository or a folder containing repositories, or create an empty project.");
        }
        let name = requested_name.filter(|name| !name.trim().is_empty())
            .or_else(|| source.file_name().and_then(OsStr::to_str)).unwrap_or("project");
        let mut workspace = Self::create_empty(projects_root, name)?;
        let root = workspace.root_path.clone().context("Project root missing")?;
        let result = (|| -> Result<()> {
            for repository in repositories {
                Self::add_existing_repository(&mut workspace, &repository.path)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Only this newly allocated, unpublished container is rolled back.
            let _ = fs::remove_dir_all(&root);
            return Err(error);
        }
        Ok(workspace)
    }

    fn open_managed(path: &Path, requested_name: Option<&str>) -> Result<Workspace> {
        let root_path =
            fs::canonicalize(path).with_context(|| format!("Unable to open {}", path.display()))?;
        if !root_path.is_dir() {
            bail!("Choose a project directory");
        }
        let repositories = discover_repositories(&root_path)?;
        let layout = match repositories.len() {
            0 => WorkspaceLayout::Empty,
            1 if repositories[0].path == root_path => WorkspaceLayout::SingleRepository,
            _ => WorkspaceLayout::MultiRepository,
        };
        let directory_name = root_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("project")
            .to_string();
        let display_name = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        Ok(new_workspace(
            directory_name,
            display_name,
            root_path,
            layout,
            repositories,
        ))
    }

    pub fn clone_github(projects_root: &Path, url: &str) -> Result<Workspace> {
        Self::clone_github_named(projects_root, url, None)
    }

    pub fn clone_github_named(
        projects_root: &Path,
        url: &str,
        requested_name: Option<&str>,
    ) -> Result<Workspace> {
        let (_, repository_name) = normalize_github_url(url)?;
        require_git_tools()?;
        let name = requested_name.map(str::trim).filter(|name| !name.is_empty()).unwrap_or(&repository_name);
        let mut workspace = Self::create_empty(projects_root, name)?;
        if let Err(error) = Self::add_github_repository(&mut workspace, url) {
            if let Some(root) = &workspace.root_path { let _ = fs::remove_dir_all(root); }
            return Err(error);
        }
        Ok(workspace)
    }

    pub fn add_existing_repository(workspace: &mut Workspace, path: &Path) -> Result<()> {
        require_git_tools()?;
        let path = fs::canonicalize(path)?;
        if !is_git_repository(&path) {
            bail!("Choose a Git repository");
        }
        let root = fs::canonicalize(workspace.root_path.as_ref().context("Project root missing")?)?;
        if is_git_repository(&root) && path != root {
            bail!("This legacy project uses a repository as its root. Import it as a new managed project before adding repositories; its original files will not be moved.");
        }
        if workspace
            .repositories
            .iter()
            .any(|repository| repository.path == path)
        {
            bail!("This repository already belongs to the project");
        }
        if workspace.repositories.iter().any(|repository| {
            repository.path.starts_with(&path) || path.starts_with(&repository.path)
        }) {
            bail!("Repositories inside a project cannot contain one another");
        }
        let mut repository = repository_from_path(&path)?;
        // Different source folders may have the same basename. Task worktrees
        // use these names as child directories, so aliases must be unique.
        let base = repository.name.clone();
        let mut suffix = 2;
        while workspace.repositories.iter().any(|item| item.name == repository.name)
            || ["CLAUDE.md", "AGENTS.md", ".git", ".blackholes-project-note.md", ".blackholes-task-CLAUDE.md"].iter().any(|reserved| repository.name.eq_ignore_ascii_case(reserved))
            || (!path.starts_with(&root)
                && fs::symlink_metadata(root.join(&repository.name)).is_ok()
                && !repository_link_matches(&root.join(&repository.name), &path)) {
            repository.name = format!("{base}-{suffix}");
            suffix += 1;
        }
        ensure_repository_link(&root, &repository)?;
        workspace.repositories.push(repository);
        workspace.layout =
            if workspace.repositories.len() == 1 && workspace.root_path.as_ref() == Some(&path) {
                WorkspaceLayout::SingleRepository
            } else {
                WorkspaceLayout::MultiRepository
            };
        workspace.updated_at = Utc::now();
        Ok(())
    }

    /// Add missing links for older database-only projects; never replace files.
    pub fn ensure_repository_links(workspace: &Workspace) -> Result<()> {
        let root = fs::canonicalize(workspace.root_path.as_ref().context("Project root missing")?)?;
        for repository in &workspace.repositories {
            ensure_repository_link(&root, repository)?;
        }
        Ok(())
    }

    pub fn copy_existing_repository(workspace: &mut Workspace, source: &Path) -> Result<()> {
        require_git_tools()?;
        let source = fs::canonicalize(source)?;
        let root = fs::canonicalize(workspace.root_path.as_ref().context("Project root missing")?)?;
        if root.starts_with(&source) {
            bail!("Choose a projects folder outside the repository being copied");
        }
        let name = source.file_name().and_then(OsStr::to_str).context("Repository name missing")?;
        let destination = root.join(unique_directory_name(&root, name));
        copy_local_repository(&source, &destination)?;
        if let Err(error) = Self::add_existing_repository(workspace, &destination) {
            let _ = fs::remove_dir_all(&destination);
            return Err(error);
        }
        Ok(())
    }

    pub fn duplicate(workspace: &Workspace, projects_root: &Path, name: &str) -> Result<Workspace> {
        let source = workspace
            .root_path
            .as_ref()
            .context("This project does not have a root directory")?;
        let source = fs::canonicalize(source)?;
        let display_name = name.trim();
        if display_name.is_empty() {
            bail!("Enter a name for the duplicate");
        }
        fs::create_dir_all(projects_root)?;
        let directory_name = unique_directory_name(projects_root, display_name);
        let destination = projects_root.join(&directory_name);
        let temporary = projects_root.join(format!(".blackholes-tmp-{}", Uuid::new_v4()));
        copy_tree(&source, &temporary)?;
        fs::rename(&temporary, &destination)?;
        let mut duplicate = Self::open_managed(&destination, Some(display_name))?;
        duplicate.name = directory_name;
        duplicate.icon = workspace.icon.clone();
        duplicate.color = workspace.color;
        // Duplicating the project container must not lose externally linked repos.
        for repository in &workspace.repositories {
            if !repository.path.starts_with(&source) {
                Self::add_existing_repository(&mut duplicate, &repository.path)?;
            }
        }
        Ok(duplicate)
    }

    pub fn update_presentation(
        workspace: &mut Workspace,
        display_name: String,
        icon: String,
        color: WorkspaceColor,
    ) -> Result<()> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            bail!("Project name cannot be empty");
        }
        workspace.display_name = Some(display_name.into());
        workspace.icon = if icon.trim().is_empty() {
            DEFAULT_PROJECT_ICON.into()
        } else {
            icon
        };
        workspace.color = color;
        workspace.updated_at = Utc::now();
        Ok(())
    }
}

fn require_git_tools() -> Result<()> {
    if !git_tools_available() {
        bail!("Install Apple's Git tools from Blackholes Settings → General → Git tools, then retry.");
    }
    Ok(())
}

fn repository_link_matches(link: &Path, source: &Path) -> bool {
    fs::symlink_metadata(link).is_ok_and(|metadata| metadata.file_type().is_symlink())
        && (fs::read_link(link).is_ok_and(|target| target == source)
            || fs::canonicalize(link).is_ok_and(|target| target == source))
}

fn ensure_repository_link(root: &Path, repository: &Repository) -> Result<()> {
    if repository.path.starts_with(root) { return Ok(()); }
    if root.starts_with(&repository.path) { bail!("A project cannot link a repository containing its own project folder"); }
    let mut components = Path::new(&repository.name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_))) || components.next().is_some() {
        bail!("Invalid repository link name");
    }
    let link = root.join(&repository.name);
    if repository_link_matches(&link, &repository.path) { return Ok(()); }
    // Atomic collision protection, including dangling links. Never remove an
    // existing entry or traverse its target while creating a link.
    std::os::unix::fs::symlink(&repository.path, &link)
        .with_context(|| format!("Could not create repository link {}. Existing files were not replaced.", link.display()))
}

fn copy_local_repository(source: &Path, destination: &Path) -> Result<()> {
    // Reserve the destination atomically; never clean up a pre-existing folder.
    fs::create_dir(destination)?;
    // --no-local avoids object hardlinks/alternates tied to the source's lifecycle.
    // No checkout: the source's actual files (including deletions) are copied
    // below. Never stash, commit, reset or clean the source checkout.
    let result = (|| -> Result<()> {
    let status = Command::new("git").args(["clone", "--no-local", "--no-checkout", "--"])
        .arg(source).arg(destination).stdin(Stdio::null()).status()
        .context("Unable to start Git clone")?;
    if !status.success() {
        bail!("Could not clone the local repository; the original is unchanged");
    }
    let origin = Command::new("git").current_dir(source)
        .args(["remote", "get-url", "origin"]).stdin(Stdio::null()).output()?;
    let url = String::from_utf8_lossy(&origin.stdout).trim().to_string();
    // Only keep a network remote. A local-path origin would reconnect the clone
    // to the user's checkout, defeating independent project ownership.
    let network_remote = ["https://", "http://", "ssh://", "git://", "git@"].iter().any(|prefix| url.starts_with(prefix));
    let args = if origin.status.success() && network_remote {
        vec!["remote", "set-url", "origin", url.as_str()]
    } else { vec!["remote", "remove", "origin"] };
    if !Command::new("git").current_dir(destination).args(args).stdin(Stdio::null()).status()?.success() {
        bail!("Could not configure the cloned repository's remote");
    }
    copy_working_files(source, destination)?;
    copy_staged_objects(source, destination)?;
    // Preserve the staged/unstaged distinction, including split indexes, but
    // never copy .git pointers, hooks, locks or alternates from the original.
    for relative in ["index", "info/exclude"] {
        let path = git_metadata_path(source, relative)?;
        if path.is_file() {
            let target = destination.join(".git").join(relative);
            fs::create_dir_all(target.parent().context("Git metadata parent missing")?)?;
            fs::copy(path, target)?;
        }
    }
    let shared_directory = git_metadata_path(source, "index")?.parent().context("Index parent missing")?.to_path_buf();
    for entry in fs::read_dir(shared_directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("sharedindex.") && entry.file_type()?.is_file() {
            fs::copy(entry.path(), destination.join(".git").join(entry.file_name()))?;
        }
    }
    Ok(())
    })();
    if result.is_err() {
        // Destination was reserved by this call; source is never cleaned up.
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn git_metadata_path(repository: &Path, name: &str) -> Result<PathBuf> {
    let output = Command::new("git").current_dir(repository)
        .args(["rev-parse", "--git-path", name]).stdin(Stdio::null()).output()?;
    if !output.status.success() { bail!("Could not locate repository metadata"); }
    Ok(repository.join(String::from_utf8(output.stdout)?.trim()))
}

fn copy_staged_objects(source: &Path, destination: &Path) -> Result<()> {
    // Clone transfers committed objects, not new blobs referenced only by the
    // source index. Import those blobs before installing that index.
    let output = Command::new("git").current_dir(source)
        .args(["ls-files", "--stage", "-z"]).stdin(Stdio::null()).output()?;
    if !output.status.success() { bail!("Could not read the source Git index"); }
    let mut seen = HashSet::new();
    for record in output.stdout.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
        let header = record.split(|byte| *byte == b'\t').next().context("Invalid Git index entry")?;
        let header = std::str::from_utf8(header)?;
        let mut fields = header.split_whitespace();
        let mode = fields.next().context("Git index mode missing")?;
        let oid = fields.next().context("Git index object missing")?;
        if mode == "160000" || oid.bytes().all(|byte| byte == b'0') || !seen.insert(oid.to_string()) { continue; }
    }
    // One bulk check avoids launching Git once per tracked file in large repos.
    let object_list = destination.join(".git").join(format!("blackholes-copy-{}", Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&object_list)?;
        for oid in &seen { writeln!(file, "{oid}")?; }
    }
    let checked = Command::new("git").current_dir(destination).arg("cat-file")
        .arg("--batch-check=%(objectname) %(objecttype)")
        .stdin(fs::File::open(&object_list)?).output();
    fs::remove_file(&object_list)?;
    let checked = checked?;
    if !checked.status.success() { bail!("Could not check copied Git objects"); }
    for line in std::str::from_utf8(&checked.stdout)?.lines() {
        let Some(oid) = line.strip_suffix(" missing") else { continue; };
        let mut reader = Command::new("git").current_dir(source).args(["cat-file", "blob", oid])
            .stdin(Stdio::null()).stdout(Stdio::piped()).spawn()?;
        let imported = Command::new("git").current_dir(destination).args(["hash-object", "-w", "--stdin"])
            .stdin(reader.stdout.take().context("Git object pipe missing")?).output();
        let read_status = reader.wait()?;
        let imported = imported?;
        if !read_status.success() || !imported.status.success() || String::from_utf8_lossy(&imported.stdout).trim() != oid {
            bail!("Could not preserve a staged Git object; source files are unchanged");
        }
    }
    Ok(())
}

fn copy_working_files(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" { continue; }
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            // Preserve links without traversing them or copying outside sources.
            std::os::unix::fs::symlink(fs::read_link(&path)?, &target)?;
        } else if metadata.is_dir() {
            if path.join(".git").exists() {
                // Includes initialized submodules and linked Git worktrees.
                copy_local_repository(&path, &target)?;
            } else {
                fs::create_dir(&target)?;
                copy_working_files(&path, &target)?;
            }
            fs::set_permissions(&target, metadata.permissions())?;
        } else if metadata.is_file() {
            fs::copy(&path, &target).with_context(|| format!("Could not copy {}", path.display()))?;
        } else {
            bail!("Cannot copy special file {}. Stop the process using it and retry, or link this repository instead.", path.display());
        }
    }
    Ok(())
}

fn initialize_git_repository(root_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .arg(root_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Unable to start git init")?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!("Git init failed: {}", details.trim());
    }

    let output = Command::new("git")
        .current_dir(root_path)
        .args([
            "-c",
            "user.name=Blackholes",
            "-c",
            "user.email=blackholes@localhost",
            "commit",
            "--quiet",
            "--allow-empty",
            "--message",
            "Initial commit",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Unable to create the initial Git commit")?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!("Initial Git commit failed: {}", details.trim());
    }
    Ok(())
}

pub fn discover_repositories(root: &Path) -> Result<Vec<Repository>> {
    if is_git_repository(root) {
        return Ok(vec![repository_from_path(root)?]);
    }
    let mut repositories = Vec::new();
    let mut names = HashSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_symlink() || !path.is_dir() || !is_git_repository(&path) {
            continue;
        }
        let repository = repository_from_path(&path)?;
        if names.insert(repository.name.clone()) {
            repositories.push(repository);
        }
    }
    repositories.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(repositories)
}

pub fn repository_from_path(path: &Path) -> Result<Repository> {
    let path = fs::canonicalize(path)?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .context("Repository path has no usable name")?
        .to_string();
    Ok(Repository {
        id: Uuid::new_v4(),
        name,
        branch: current_branch(&path),
        path,
    })
}

pub fn current_branch(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty())
}

pub fn repository_git_summary(path: &Path) -> Result<RepositoryGitSummary> {
    if !is_git_repository(path) {
        bail!("{} is not a Git repository", path.display());
    }

    let branch = current_branch(path).or_else(|| short_revision(path));
    let output = Command::new("git")
        .current_dir(path)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "HEAD",
            "--",
        ])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("Unable to read Git changes in {}", path.display()))?;

    let (additions, deletions) = if output.status.success() {
        parse_numstat(&output.stdout)
    } else {
        (0, 0)
    };

    Ok(RepositoryGitSummary {
        branch,
        additions,
        deletions,
    })
}

fn short_revision(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--short", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

fn parse_numstat(output: &[u8]) -> (u64, u64) {
    String::from_utf8_lossy(output)
        .lines()
        .fold((0_u64, 0_u64), |(additions, deletions), line| {
            let mut columns = line.split('\t');
            let added = columns
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let deleted = columns
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            (
                additions.saturating_add(added),
                deletions.saturating_add(deleted),
            )
        })
}

pub fn is_git_repository(path: &Path) -> bool {
    path.join(".git").exists()
        && Command::new("git")
            .current_dir(path)
            .args(["rev-parse", "--is-inside-work-tree"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

fn new_workspace(
    name: String,
    display_name: Option<String>,
    root_path: PathBuf,
    layout: WorkspaceLayout,
    repositories: Vec<Repository>,
) -> Workspace {
    let now = Utc::now();
    Workspace {
        id: Uuid::new_v4(),
        name,
        display_name,
        icon: DEFAULT_PROJECT_ICON.into(),
        color: WorkspaceColor::Slate,
        root_path: Some(root_path),
        layout,
        repositories,
        ignored_repository_paths: vec![],
        created_at: now,
        updated_at: now,
    }
}

fn normalize_github_url(url: &str) -> Result<(String, String)> {
    let input = url.trim().trim_end_matches('/');
    let https =
        Regex::new(r"^https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?$")?;
    let ssh = Regex::new(r"^git@github\.com:([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?$")?;
    let captures = https.captures(input).or_else(|| ssh.captures(input));
    let Some(captures) = captures else {
        bail!("Enter a credential-free GitHub HTTPS or SSH URL");
    };
    let owner = captures.get(1).unwrap().as_str();
    let repository = captures.get(2).unwrap().as_str();
    if [".", ".."].contains(&repository) || [".", ".."].contains(&owner) {
        bail!("Invalid GitHub repository URL");
    }
    Ok((
        if input.starts_with("git@") { format!("git@github.com:{owner}/{repository}.git") }
        else { format!("https://github.com/{owner}/{repository}.git") },
        repository.into(),
    ))
}

fn safe_directory_name(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_separator = false;
    for character in value.trim().chars() {
        let valid = character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.');
        if valid {
            output.push(character);
            previous_separator = false;
        } else if !previous_separator {
            output.push('-');
            previous_separator = true;
        }
        if output.len() >= 72 {
            break;
        }
    }
    let output = output.trim_matches(['-', '.', ' ']).to_string();
    if output.is_empty() {
        "project".into()
    } else {
        output
    }
}

fn unique_directory_name(parent: &Path, value: &str) -> String {
    let base = safe_directory_name(value);
    if fs::symlink_metadata(parent.join(&base)).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
        return base;
    }
    for suffix in 2..10_000 {
        let candidate = format!("{base}-{suffix}");
        if fs::symlink_metadata(parent.join(&candidate)).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
            return candidate;
        }
    }
    format!("{base}-{}", Uuid::new_v4().simple())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in WalkDir::new(source).follow_links(false).min_depth(1) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        let target = destination.join(relative);
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            let link = fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(link, target)?;
        } else if metadata.is_dir() {
            fs::create_dir(&target)?;
            fs::set_permissions(&target, metadata.permissions())?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), &target)?;
            fs::set_permissions(&target, metadata.permissions())?;
        }
    }
    Ok(())
}
