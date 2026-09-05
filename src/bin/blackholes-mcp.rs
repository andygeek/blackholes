use anyhow::{Context, Result, bail};
use blackholes_rust::{
    model::{ProjectTask, Workspace, WorkspaceColor},
    paths::AppPaths,
    services::{
        database::Database,
        notes::{PROJECT_NOTE_FILE_NAME, ProjectNoteService, TASK_NOTE_FILE_NAME, TaskNoteService},
        projects::ProjectService,
        tasks::{
            AddTaskRepositoriesRequest, CreateTaskRequest, ExistingBranchAction,
            RemoveTaskRepositoriesRequest, RepositoryPreparation, TaskBranchSource, TaskService,
        },
    },
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixDatagram,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uuid::Uuid;

fn main() -> Result<()> {
    run()
}

pub fn run() -> Result<()> {
    let paths = AppPaths::discover()?;
    let database = Database::open(&paths)?;
    let mut server = McpServer { paths, database };
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    while let Some(request) = read_message(&mut reader)? {
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = match server.handle(&request) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": format!("{error:#}") }
            }),
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

const NOTIFICATION_TITLE_LIMIT: usize = 80;
const NOTIFICATION_MESSAGE_LIMIT: usize = 200;
const AGENT_HANDOFF_PROMPT_LIMIT: usize = 16_000;

struct McpServer {
    paths: AppPaths,
    database: Database,
}

impl McpServer {
    fn handle(&mut self, request: &Value) -> Result<Value> {
        match request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "initialize" => Ok(json!({
                "protocolVersion": request
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2025-06-18"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "blackholes-rust", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Blackholes is a local project and task manager exposed through this MCP. At the beginning of every Black Bot turn, call get_current_context before any other tool to verify that this required MCP is available. Whenever the user says \"in Blackholes\" or asks Blackholes to create or manage a project, repository, task, branch, worktree, note, agent handoff, navigation, or notification, use this MCP before filesystem, browser, or Computer Use tools. Never use Computer Use for an operation exposed by this server. When the user asks to create a project and provides only its name, call create_project with that name immediately; do not ask for a path or technology because the tool prepares the project container, Git repository, initial commit, and managed context automatically. When the user explicitly provides an existing local path, pass it to create_project to link its repositories to a new project container. Local sources stay in place, including pending changes and local environments. Linking does not edit files, but subsequent project work changes the originals. Global and project agents may inspect, review, edit, build, and test directly in the intended project repositories; tasks, isolated worktrees, and delegation are optional, not prerequisites. Resolve the intended project and repositories, respect user-authored project instructions and selected permissions, and preserve unrelated changes. Use isolated tasks when the user requests them, selects an existing task, or their project instructions require that workflow. A request to create a task and start or implement it is sufficient authorization: global/project agents must create it then immediately call handoff_to_agent with the returned taskId before implementation in the worktree. Do not ask for redundant delegation approval. Creating a task alone does not authorize execution. Ask only when execution intent or destination is unclear. The receiving task agent implements directly without recreating the task or handing it back. For implementation without a requested task, the global agent normally hands off to the project agent with projectId; that project agent works directly. Honor explicit requests to work here or not delegate. Do not require an isolation opt-out. When working in a task, change its attached worktrees rather than original checkouts. If delegating, use handoff_to_agent with projectId for direct project work or taskId for isolated work; after a successful handoff, the sender stops the delegated work and reports the transfer. Use search tools before asking for IDs. Read details before mutations. When a task must start from a specific base, such as a repository whose task branch does not exist yet, pass baseBranch to create_task or add_task_repositories instead of relying on whatever branch the repository has checked out. remove_task_repositories undoes a wrong add: it deletes those worktrees and keeps their branches unless deleteBranch is true. delete_task is destructive and requires explicit confirmation; it refuses active terminals or uncommitted worktree changes. The task agent that owns implementation finishes with notify_task_ready as its last tool call after black-bot completes. When the user asks to open, go to, or show a Blackholes task or project, call open_task or open_project; the app adds a persistent clickable navigation card to the current answer and changes view only when the user clicks it. Never use these navigation tools merely to announce work."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => {
                let name = request
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .context("missing tool name")?;
                let arguments = request
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match self.call_tool(name, &arguments) {
                    Ok(value) => Ok(tool_result(value, false)),
                    Err(error) => Ok(tool_result(json!({ "error": format!("{error:#}") }), true)),
                }
            }
            method => bail!("unsupported MCP method: {method}"),
        }
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value> {
        match name {
            "get_current_context" => self.get_current_context(arguments),
            "search_projects" => self.search_projects(arguments),
            "list_projects" => Ok(serde_json::to_value(self.database.workspaces()?)?),
            "get_project" => self.get_project(arguments),
            "create_project" => self.create_project(arguments),
            "import_project" => self.import_project(arguments),
            "update_project" => self.update_project(arguments),
            "add_project_repository" => self.add_project_repository(arguments),
            "get_repository" => self.get_repository(arguments),
            "get_repository_status" => self.get_repository_status(arguments),
            "search_tasks" => self.search_tasks(arguments),
            "list_tasks" => self.list_tasks(arguments),
            "get_task" => self.get_task(arguments),
            "check_branch_availability" => self.check_branch_availability(arguments),
            "create_task" => self.create_task(arguments),
            "update_task" => self.update_task(arguments),
            "delete_task" => self.delete_task(arguments),
            "add_task_repositories" => self.add_task_repositories(arguments),
            "remove_task_repositories" => self.remove_task_repositories(arguments),
            "get_project_note" => self.get_project_note(arguments),
            "write_project_note" => self.write_project_note(arguments),
            "append_project_note" => self.append_project_note(arguments),
            "write_task_note" => self.write_task_note(arguments),
            "append_task_note" => self.append_task_note(arguments),
            "handoff_to_agent" => self.handoff_to_agent(arguments),
            "open_project" => self.open_project(arguments),
            "open_task" => self.open_task(arguments),
            "notify_task_ready" => self.notify_task_ready(arguments),
            _ => bail!("unknown Blackholes tool: {name}"),
        }
    }

    fn get_current_context(&self, arguments: &Value) -> Result<Value> {
        let cwd = arguments
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);
        let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
        let workspaces = self.database.workspaces()?;
        let tasks = self.database.all_tasks()?;

        for task in &tasks {
            for repository in &task.repositories {
                if cwd.starts_with(&repository.worktree_path) {
                    return Ok(json!({
                        "kind": "task-repository",
                        "cwd": cwd,
                        "task": task,
                        "repositoryId": repository.repository_id,
                        "writableRoot": repository.worktree_path,
                    }));
                }
            }
            if cwd.starts_with(&task.worktree_root_path) {
                return Ok(json!({
                    "kind": "task",
                    "cwd": cwd,
                    "task": task,
                    "writableRoot": task.worktree_root_path,
                }));
            }
        }
        for workspace in &workspaces {
            for repository in &workspace.repositories {
                if cwd.starts_with(&repository.path) {
                    return Ok(json!({
                        "kind": "project-repository",
                        "cwd": cwd,
                        "project": workspace,
                        "repository": repository,
                        "readOnlyOriginalCheckout": false,
                        "writableRoot": repository.path,
                    }));
                }
            }
            if workspace
                .root_path
                .as_ref()
                .is_some_and(|root| cwd.starts_with(root))
            {
                return Ok(json!({
                    "kind": "project",
                    "cwd": cwd,
                    "project": workspace,
                    "readOnlyOriginalCheckout": false,
                    "writableRoot": workspace.root_path,
                }));
            }
        }
        Ok(json!({ "kind": "outside-blackholes", "cwd": cwd }))
    }

    fn search_projects(&self, arguments: &Value) -> Result<Value> {
        let query = required_string(arguments, "query")?.to_lowercase();
        let limit = search_limit(arguments);
        let tasks = self.database.all_tasks()?;
        let mut matches = Vec::new();

        for project in self.database.workspaces()? {
            let note = ProjectNoteService::read(&project).unwrap_or_default();
            let root_path = project
                .root_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            let project_matches = [
                project.label(),
                project.name.as_str(),
                root_path.as_str(),
                note.as_str(),
            ]
            .iter()
            .any(|value| value.to_lowercase().contains(&query));
            let repository_matches = project.repositories.iter().any(|repository| {
                repository.name.to_lowercase().contains(&query)
                    || repository
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
            });
            let project_tasks = tasks
                .iter()
                .filter(|task| task.workspace_id == project.id)
                .collect::<Vec<_>>();
            let task_matches = project_tasks.iter().any(|task| {
                task.title.to_lowercase().contains(&query)
                    || task
                        .description
                        .as_deref()
                        .is_some_and(|description| description.to_lowercase().contains(&query))
            });

            if project_matches || repository_matches || task_matches {
                matches.push(json!({
                    "project": project,
                    "taskCount": project_tasks.len(),
                    "projectNoteExcerpt": excerpt(&note, 500),
                }));
            }
            if matches.len() >= limit {
                break;
            }
        }

        Ok(json!({ "query": query, "count": matches.len(), "projects": matches }))
    }

    fn get_project(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let tasks = self.database.tasks(project_id)?;
        let note = ProjectNoteService::read(&project)?;
        Ok(json!({
            "project": project,
            "projectNote": {
                "fileName": PROJECT_NOTE_FILE_NAME,
                "content": note,
            },
            "tasks": tasks,
        }))
    }

    fn create_project(&self, arguments: &Value) -> Result<Value> {
        let root = self.database.setting("projects-root-path")?.map(PathBuf::from)
            .unwrap_or_else(|| self.paths.default_projects.clone());
        let project = if let Some(path) = arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let requested_name = arguments.get("name").and_then(Value::as_str);
            ProjectService::import_existing(&root, &PathBuf::from(path), requested_name)?
        } else {
            let name = required_string(arguments, "name")?;
            let root = self
                .database
                .setting("projects-root-path")?
                .map(PathBuf::from)
                .unwrap_or_else(|| self.paths.default_projects.clone());
            ProjectService::create_git_repository(&root, name)?
        };
        if self
            .database
            .workspaces()?
            .iter()
            .any(|current| current.root_path == project.root_path)
        {
            bail!("that project is already registered in Blackholes")
        }
        ProjectNoteService::ensure(&project, "")?;
        let index = self.database.workspaces()?.len();
        self.database.upsert_workspace(&project, index)?;
        self.notify_app("reload");
        Ok(serde_json::to_value(project)?)
    }

    fn import_project(&self, arguments: &Value) -> Result<Value> {
        let path = PathBuf::from(required_string(arguments, "path")?);
        let requested_name = arguments.get("name").and_then(Value::as_str);
        let root = self.database.setting("projects-root-path")?.map(PathBuf::from)
            .unwrap_or_else(|| self.paths.default_projects.clone());
        let project = ProjectService::import_existing(&root, &path, requested_name)?;
        if self
            .database
            .workspaces()?
            .iter()
            .any(|current| current.root_path == project.root_path)
        {
            bail!("that project is already registered in Blackholes")
        }
        ProjectNoteService::ensure(&project, "")?;
        let index = self.database.workspaces()?.len();
        self.database.upsert_workspace(&project, index)?;
        self.notify_app("reload");
        Ok(serde_json::to_value(project)?)
    }

    fn update_project(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let workspaces = self.database.workspaces()?;
        let index = workspaces
            .iter()
            .position(|workspace| workspace.id == project_id)
            .context("project not found")?;
        let mut project = workspaces[index].clone();
        let display_name = arguments
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_else(|| project.label())
            .to_string();
        let icon = arguments
            .get("icon")
            .and_then(Value::as_str)
            .unwrap_or(&project.icon)
            .to_string();
        let color = arguments
            .get("color")
            .cloned()
            .map(serde_json::from_value::<WorkspaceColor>)
            .transpose()
            .context("invalid project color")?
            .unwrap_or(project.color);
        ProjectService::update_presentation(&mut project, display_name, icon, color)?;
        self.database.upsert_workspace(&project, index)?;
        self.notify_app("reload");
        Ok(serde_json::to_value(project)?)
    }

    fn add_project_repository(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let path = arguments.get("path").and_then(Value::as_str).filter(|value| !value.trim().is_empty());
        let url = arguments.get("url").and_then(Value::as_str).filter(|value| !value.trim().is_empty());
        if path.is_some() == url.is_some() { bail!("Provide exactly one of path or url"); }
        let workspaces = self.database.workspaces()?;
        let index = workspaces
            .iter()
            .position(|workspace| workspace.id == project_id)
            .context("project not found")?;
        let mut project = workspaces[index].clone();
        if let Some(url) = url { ProjectService::add_github_repository(&mut project, url)?; }
        else if let Some(path) = path { ProjectService::add_existing_repository(&mut project, Path::new(path))?; }
        let repository = project
            .repositories
            .last()
            .cloned()
            .context("repository was not added")?;
        self.database.upsert_workspace(&project, index)?;
        self.notify_app("reload");
        Ok(json!({ "project": project, "repository": repository }))
    }

    fn get_repository(&self, arguments: &Value) -> Result<Value> {
        let repository_id = required_uuid(arguments, "repositoryId")?;
        let workspaces = self.database.workspaces()?;
        let (project, repository) = workspaces
            .iter()
            .find_map(|workspace| {
                workspace
                    .repositories
                    .iter()
                    .find(|repository| repository.id == repository_id)
                    .map(|repository| (workspace, repository))
            })
            .context("repository not found")?;
        let task_attachments = self
            .database
            .tasks(project.id)?
            .into_iter()
            .filter_map(|task| {
                task.repositories
                    .iter()
                    .find(|attached| attached.repository_id == repository_id)
                    .cloned()
                    .map(|attached| {
                        json!({
                            "taskId": task.id,
                            "taskTitle": task.title,
                            "attachment": attached,
                        })
                    })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "project": project,
            "repository": repository,
            "taskAttachments": task_attachments,
        }))
    }

    fn get_repository_status(&self, arguments: &Value) -> Result<Value> {
        let repository_id = required_uuid(arguments, "repositoryId")?;
        let workspaces = self.database.workspaces()?;
        let mut path = workspaces
            .iter()
            .flat_map(|workspace| &workspace.repositories)
            .find(|repository| repository.id == repository_id)
            .map(|repository| repository.path.clone())
            .context("repository not found")?;
        if let Some(task_id) = optional_uuid(arguments, "taskId")? {
            let tasks = self.database.all_tasks()?;
            let task = find_task(&tasks, task_id)?;
            path = task
                .repositories
                .iter()
                .find(|repository| repository.repository_id == repository_id)
                .map(|repository| repository.worktree_path.clone())
                .context("repository is not attached to that task")?;
        }
        let porcelain = git_output(&path, ["status", "--porcelain"])?;
        let branch = git_output_optional(&path, ["symbolic-ref", "--quiet", "--short", "HEAD"]);
        Ok(json!({
            "repositoryId": repository_id,
            "path": path,
            "branch": branch.map(|value| value.trim().to_string()),
            "changeCount": porcelain.lines().count(),
            "clean": porcelain.trim().is_empty(),
        }))
    }

    fn search_tasks(&self, arguments: &Value) -> Result<Value> {
        let query = required_string(arguments, "query")?.to_lowercase();
        let project_id = optional_uuid(arguments, "projectId")?;
        let limit = search_limit(arguments);
        let workspaces = self.database.workspaces()?;
        let tasks = if let Some(project_id) = project_id {
            self.database.tasks(project_id)?
        } else {
            self.database.all_tasks()?
        };
        let mut matches = Vec::new();

        for task in tasks {
            let note = TaskNoteService::read(&task).unwrap_or_default();
            let project = workspaces
                .iter()
                .find(|workspace| workspace.id == task.workspace_id);
            let repository_matches = task.repositories.iter().any(|attached| {
                attached.branch.to_lowercase().contains(&query)
                    || attached
                        .worktree_path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
                    || project.is_some_and(|project| {
                        project.repositories.iter().any(|repository| {
                            repository.id == attached.repository_id
                                && (repository.name.to_lowercase().contains(&query)
                                    || repository
                                        .path
                                        .to_string_lossy()
                                        .to_lowercase()
                                        .contains(&query))
                        })
                    })
            });
            let task_matches = task.title.to_lowercase().contains(&query)
                || task
                    .description
                    .as_deref()
                    .is_some_and(|description| description.to_lowercase().contains(&query))
                || note.to_lowercase().contains(&query)
                || repository_matches;
            if task_matches {
                matches.push(json!({
                    "task": task,
                    "project": project,
                    "noteExcerpt": excerpt(&note, 500),
                }));
            }
            if matches.len() >= limit {
                break;
            }
        }

        Ok(json!({ "query": query, "count": matches.len(), "tasks": matches }))
    }

    fn list_tasks(&self, arguments: &Value) -> Result<Value> {
        if let Some(project_id) = optional_uuid(arguments, "projectId")? {
            Ok(serde_json::to_value(self.database.tasks(project_id)?)?)
        } else {
            Ok(serde_json::to_value(self.database.all_tasks()?)?)
        }
    }

    fn get_task(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let tasks = self.database.all_tasks()?;
        let task = find_task(&tasks, task_id)?;
        let workspaces = self.database.workspaces()?;
        let project = find_workspace(&workspaces, task.workspace_id)?;
        let note = TaskNoteService::read(task)?;
        let repositories = task
            .repositories
            .iter()
            .map(|attached| {
                let source = project
                    .repositories
                    .iter()
                    .find(|repository| repository.id == attached.repository_id);
                json!({ "source": source, "task": attached })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "task": task,
            "project": project,
            "repositories": repositories,
            "note": {
                "fileName": TASK_NOTE_FILE_NAME,
                "content": note,
            }
        }))
    }

    /// `baseBranch` is where a task branch that does not exist yet gets
    /// created; it never rewrites a branch that already exists.
    fn base_branch(arguments: &Value) -> Option<String> {
        arguments
            .get("baseBranch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .map(str::to_string)
    }

    fn check_branch_availability(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let branch = required_string(arguments, "branch")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let mut requested = optional_uuid_list(arguments, "repositoryIds")?;
        if requested.is_empty() {
            requested = project
                .repositories
                .iter()
                .map(|repository| repository.id)
                .collect();
        }
        let source = match arguments
            .get("branchSource")
            .and_then(Value::as_str)
            .unwrap_or("local")
        {
            "current" => TaskBranchSource::Current,
            "local" => TaskBranchSource::Local,
            "remote" => TaskBranchSource::Remote,
            value => bail!("unsupported branchSource: {value}"),
        };
        let base_branch = Self::base_branch(arguments);
        let repositories = TaskService::branch_availability(
            &project,
            &requested,
            branch,
            source,
            base_branch.as_deref(),
        )?;
        Ok(json!({
            "branch": branch,
            "baseBranch": base_branch,
            "branchSource": arguments
                .get("branchSource")
                .and_then(Value::as_str)
                .unwrap_or("local"),
            "repositories": repositories,
        }))
    }

    fn create_task(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let mut repository_ids = optional_uuid_list(arguments, "repositoryIds")?;
        if repository_ids.is_empty() {
            repository_ids = project
                .repositories
                .iter()
                .map(|repository| repository.id)
                .collect();
        }
        let copy_local_changes = arguments
            .get("copyLocalChanges")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let copy_environment_files = arguments
            .get("copyEnvironmentFiles")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let setup_command = arguments
            .get("setupCommand")
            .and_then(Value::as_str)
            .map(str::to_string);
        let preparations = repository_ids
            .iter()
            .map(|repository_id| {
                (
                    *repository_id,
                    RepositoryPreparation {
                        copy_local_changes,
                        copy_environment_files,
                        setup_command: setup_command.clone(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let branch_source = match arguments
            .get("branchSource")
            .and_then(Value::as_str)
            .unwrap_or("current")
        {
            "current" => TaskBranchSource::Current,
            "local" => TaskBranchSource::Local,
            "remote" => TaskBranchSource::Remote,
            value => bail!("unsupported branchSource: {value}"),
        };
        let existing_branch_action = match arguments
            .get("existingBranchAction")
            .and_then(Value::as_str)
            .unwrap_or("reuse")
        {
            "reuse" => ExistingBranchAction::Reuse,
            "recreate" => ExistingBranchAction::Recreate,
            value => bail!("unsupported existingBranchAction: {value}"),
        };
        let mut task = TaskService::new(&self.paths).create(
            &project,
            CreateTaskRequest {
                title: required_string(arguments, "title")?.into(),
                description: arguments
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                branch_name: arguments
                    .get("branch")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                branch_source,
                base_ref: Self::base_branch(arguments),
                create_missing_branch: arguments
                    .get("createMissingBranch")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                replace_divergent_local_branches: arguments
                    .get("replaceDivergentLocalBranches")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                existing_branch_action,
                repository_ids,
                preparations,
            },
        )?;
        task.sort_order = self.database.tasks(project_id)?.len() as i64;
        self.database.upsert_task(&task)?;
        if let Some(note) = arguments.get("developerContext").and_then(Value::as_str) {
            append_note(&task, note)?;
        }
        self.notify_app("reload");
        self.present_navigation_link(
            "task",
            Some(project_id),
            Some(task.id),
            format!("{} / {}", project.label(), task.title),
        )?;
        Ok(serde_json::to_value(task)?)
    }

    fn update_task(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let mut task = find_task(&self.database.all_tasks()?, task_id)?.clone();
        let project = find_workspace(&self.database.workspaces()?, task.workspace_id)?.clone();
        let title = arguments
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(&task.title)
            .to_string();
        let description = if arguments.get("description").is_some_and(Value::is_null) {
            None
        } else {
            arguments
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| task.description.clone())
        };
        let color = arguments
            .get("color")
            .cloned()
            .map(serde_json::from_value::<WorkspaceColor>)
            .transpose()
            .context("invalid task color")?
            .unwrap_or(task.color);
        TaskService::new(&self.paths).update(&project, &mut task, title, description, color)?;
        self.database.upsert_task(&task)?;
        self.notify_app("reload");
        Ok(serde_json::to_value(task)?)
    }

    fn delete_task(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        if !arguments
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            bail!("delete_task requires confirm=true")
        }
        let task = find_task(&self.database.all_tasks()?, task_id)?.clone();
        TaskService::new(&self.paths).remove(&task)?;
        self.database.remove_task(task_id)?;
        self.notify_app("reload");
        Ok(json!({
            "taskId": task_id,
            "title": task.title,
            "deleted": true,
            "branchesPreserved": true,
        }))
    }

    fn add_task_repositories(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let mut task = find_task(&self.database.all_tasks()?, task_id)?.clone();
        let project = find_workspace(&self.database.workspaces()?, task.workspace_id)?.clone();
        let repository_ids = optional_uuid_list(arguments, "repositoryIds")?;
        if repository_ids.is_empty() {
            bail!("choose at least one repository")
        }
        let branch_source = match arguments
            .get("branchSource")
            .and_then(Value::as_str)
            .unwrap_or("current")
        {
            "current" => TaskBranchSource::Current,
            "local" => TaskBranchSource::Local,
            "remote" => TaskBranchSource::Remote,
            value => bail!("unsupported branchSource: {value}"),
        };
        let existing_branch_action = match arguments
            .get("existingBranchAction")
            .and_then(Value::as_str)
            .unwrap_or("reuse")
        {
            "reuse" => ExistingBranchAction::Reuse,
            "recreate" => ExistingBranchAction::Recreate,
            value => bail!("unsupported existingBranchAction: {value}"),
        };
        let copy_local_changes = arguments
            .get("copyLocalChanges")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let copy_environment_files = arguments
            .get("copyEnvironmentFiles")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let setup_command = arguments
            .get("setupCommand")
            .and_then(Value::as_str)
            .map(str::to_string);
        let preparations = repository_ids
            .iter()
            .map(|repository_id| {
                (
                    *repository_id,
                    RepositoryPreparation {
                        copy_local_changes,
                        copy_environment_files,
                        setup_command: setup_command.clone(),
                    },
                )
            })
            .collect();
        TaskService::new(&self.paths).add_repositories(
            &project,
            &mut task,
            AddTaskRepositoriesRequest {
                repository_ids,
                branch_source,
                base_ref: Self::base_branch(arguments),
                create_missing_branch: arguments
                    .get("createMissingBranch")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                replace_divergent_local_branches: arguments
                    .get("replaceDivergentLocalBranches")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                existing_branch_action,
                preparations,
            },
        )?;
        self.database.upsert_task(&task)?;
        self.notify_app("reload");
        Ok(serde_json::to_value(task)?)
    }

    fn remove_task_repositories(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let mut task = find_task(&self.database.all_tasks()?, task_id)?.clone();
        let project = find_workspace(&self.database.workspaces()?, task.workspace_id)?.clone();
        let repository_ids = optional_uuid_list(arguments, "repositoryIds")?;
        if repository_ids.is_empty() {
            bail!("choose at least one repository")
        }
        let removed = TaskService::new(&self.paths).remove_repositories(
            &project,
            &mut task,
            RemoveTaskRepositoriesRequest {
                repository_ids,
                discard_uncommitted_changes: arguments
                    .get("discardUncommittedChanges")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                delete_branch: arguments
                    .get("deleteBranch")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        )?;
        self.database.upsert_task(&task)?;
        self.notify_app("reload");
        Ok(json!({
            "task": task,
            "removed": removed,
        }))
    }

    fn get_project_note(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let content = ProjectNoteService::read(&project)?;
        Ok(json!({
            "projectId": project_id,
            "fileName": PROJECT_NOTE_FILE_NAME,
            "path": project.root_path.map(|root| root.join(PROJECT_NOTE_FILE_NAME)),
            "content": content,
        }))
    }

    fn write_project_note(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .context("missing content")?;
        ProjectNoteService::write(&project, content)?;
        self.notify_app(&format!("project-note-updated:{project_id}"));
        Ok(json!({
            "projectId": project_id,
            "fileName": PROJECT_NOTE_FILE_NAME,
            "written": true,
        }))
    }

    fn append_project_note(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let project = find_workspace(&self.database.workspaces()?, project_id)?.clone();
        let context = required_string(arguments, "context")?;
        let mut note = ProjectNoteService::read(&project)?;
        append_developer_context(&mut note, context)?;
        ProjectNoteService::write(&project, &note)?;
        self.notify_app(&format!("project-note-updated:{project_id}"));
        Ok(json!({
            "projectId": project_id,
            "fileName": PROJECT_NOTE_FILE_NAME,
            "appended": true,
        }))
    }

    fn write_task_note(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let task = find_task(&self.database.all_tasks()?, task_id)?.clone();
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .context("missing content")?;
        TaskNoteService::write(&task, content)?;
        self.notify_app(&format!("note-updated:{task_id}"));
        Ok(json!({
            "taskId": task_id,
            "fileName": TASK_NOTE_FILE_NAME,
            "written": true,
        }))
    }

    fn append_task_note(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let tasks = self.database.all_tasks()?;
        let task = find_task(&tasks, task_id)?;
        let context = required_string(arguments, "context")?;
        append_note(task, context)?;
        self.notify_app(&format!("note-updated:{task_id}"));
        Ok(
            json!({ "taskId": task_id, "notePath": task.worktree_root_path.join(".blackholes-note.md") }),
        )
    }

    fn open_task(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let tasks = self.database.all_tasks()?;
        let task = find_task(&tasks, task_id)?;
        self.present_navigation_link(
            "task",
            Some(task.workspace_id),
            Some(task_id),
            task.title.clone(),
        )
    }

    fn open_project(&self, arguments: &Value) -> Result<Value> {
        let project_id = required_uuid(arguments, "projectId")?;
        let workspaces = self.database.workspaces()?;
        let project = find_workspace(&workspaces, project_id)?;
        self.present_navigation_link(
            "project",
            Some(project_id),
            None,
            project.label().to_string(),
        )
    }

    fn present_navigation_link(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        task_id: Option<Uuid>,
        label: String,
    ) -> Result<Value> {
        let source_scope = std::env::var("BLACKHOLES_AGENT_SOURCE_SCOPE")
            .ok()
            .filter(|scope| matches!(scope.as_str(), "global" | "project" | "task"));
        let source_global_agent_id = std::env::var("BLACKHOLES_AGENT_SOURCE_GLOBAL_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_project_id = std::env::var("BLACKHOLES_AGENT_SOURCE_PROJECT_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_task_id = std::env::var("BLACKHOLES_AGENT_SOURCE_TASK_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_agent_id = std::env::var("BLACKHOLES_AGENT_SOURCE_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let payload = json!({
            "scope": scope,
            "projectId": project_id,
            "taskId": task_id,
            "sourceScope": source_scope,
            "sourceGlobalAgentId": source_global_agent_id,
            "sourceProjectId": source_project_id,
            "sourceTaskId": source_task_id,
            "sourceAgentId": source_agent_id,
            "label": label,
        });
        let delivered = self.notify_app(&format!("navigation-link:{payload}"));
        Ok(json!({
            "scope": scope,
            "projectId": project_id,
            "taskId": task_id,
            "targetName": label,
            "navigationCardAdded": delivered,
        }))
    }

    fn handoff_to_agent(&self, arguments: &Value) -> Result<Value> {
        let project_id = optional_uuid(arguments, "projectId")?;
        let task_id = optional_uuid(arguments, "taskId")?;
        if project_id.is_some() == task_id.is_some() {
            bail!("provide exactly one of projectId or taskId")
        }
        let prompt = required_string(arguments, "prompt")?;
        if prompt.chars().count() > AGENT_HANDOFF_PROMPT_LIMIT {
            bail!("prompt is limited to {AGENT_HANDOFF_PROMPT_LIMIT} characters")
        }
        let (scope, project_id, task_id, target_name) = if let Some(task_id) = task_id {
            let tasks = self.database.all_tasks()?;
            let task = find_task(&tasks, task_id)?;
            (
                "task",
                Some(task.workspace_id),
                Some(task_id),
                task.title.clone(),
            )
        } else {
            let project_id = project_id.expect("project id was validated above");
            let workspaces = self.database.workspaces()?;
            let project = find_workspace(&workspaces, project_id)?;
            (
                "project",
                Some(project_id),
                None,
                project.label().to_string(),
            )
        };
        let title = optional_truncated(arguments, "title", NOTIFICATION_TITLE_LIMIT);
        let message = optional_truncated(arguments, "message", NOTIFICATION_MESSAGE_LIMIT);
        let source_agent_id = std::env::var("BLACKHOLES_AGENT_SOURCE_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_scope = std::env::var("BLACKHOLES_AGENT_SOURCE_SCOPE")
            .ok()
            .filter(|scope| matches!(scope.as_str(), "global" | "project" | "task"));
        let source_global_agent_id = std::env::var("BLACKHOLES_AGENT_SOURCE_GLOBAL_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_project_id = std::env::var("BLACKHOLES_AGENT_SOURCE_PROJECT_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let source_task_id = std::env::var("BLACKHOLES_AGENT_SOURCE_TASK_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let payload = json!({
            "scope": scope,
            "projectId": project_id,
            "taskId": task_id,
            "sourceScope": source_scope,
            "sourceGlobalAgentId": source_global_agent_id,
            "sourceProjectId": source_project_id,
            "sourceTaskId": source_task_id,
            "prompt": prompt,
            "sourceAgentId": source_agent_id,
            "title": title,
            "message": message,
        });
        let confirmation = blackholes_rust::services::agent_commands::send(
            &self.paths, &format!("agent-handoff:{payload}"),
        )?;
        Ok(json!({
            "scope": scope,
            "projectId": project_id,
            "taskId": task_id,
            "targetName": target_name,
            "agent": "Black Bot",
            "accepted": true,
            "started": confirmation["started"],
            "queued": confirmation["queued"],
        }))
    }

    fn notify_task_ready(&self, arguments: &Value) -> Result<Value> {
        let task_id = required_uuid(arguments, "taskId")?;
        let tasks = self.database.all_tasks()?;
        let task = find_task(&tasks, task_id)?;
        let title = optional_truncated(arguments, "title", NOTIFICATION_TITLE_LIMIT);
        let message = optional_truncated(arguments, "message", NOTIFICATION_MESSAGE_LIMIT);
        let payload = json!({ "taskId": task_id, "title": title, "message": message });
        let delivered = self.notify_app(&format!("task-ready:{payload}"));
        Ok(json!({
            "taskId": task_id,
            "taskTitle": task.title,
            "deliveredToRunningApp": delivered,
        }))
    }

    fn notify_app(&self, message: &str) -> bool {
        let Ok(socket) = UnixDatagram::unbound() else {
            return false;
        };
        socket.connect(&self.paths.events_socket).is_ok() && socket.send(message.as_bytes()).is_ok()
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
        "structuredContent": value,
        "isError": is_error,
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        read_tool(
            "get_current_context",
            "Resolve the Blackholes project, repository, or isolated task containing a working directory.",
            json!({"type":"object","properties":{"cwd":{"type":"string"}}}),
        ),
        read_tool(
            "search_projects",
            "Search projects by name, root path, repository name/path, task title/description, or project note. Returns matching project metadata, task counts, and note excerpts.",
            json!({"type":"object","properties":{"query":{"type":"string","minLength":1},"limit":{"type":"integer","minimum":1,"maximum":100,"default":20}},"required":["query"]}),
        ),
        read_tool(
            "list_projects",
            "List Blackholes projects and their repositories.",
            empty_schema(),
        ),
        read_tool(
            "get_project",
            "Get one project with its metadata, root path, repositories, project note, and tasks.",
            id_schema("projectId"),
        ),
        write_tool(
            "create_project",
            "Create a managed project container in the configured Blackholes_projects folder. With path, link that directory's Git repositories in place, including pending changes and local environments. Linking does not alter originals, but subsequent project edits affect them. Without path, create a same-named Git repository with an initial commit inside the container. Prepare project-level CLAUDE.md, AGENTS.md, and notes. Returns the new project metadata.",
            json!({"type":"object","properties":{"name":{"type":"string","minLength":1},"path":{"type":"string","minLength":1}},"anyOf":[{"required":["name"]},{"required":["path"]}]}),
        ),
        write_tool(
            "import_project",
            "Link local Git repositories to a new managed project container with its own instructions, notes, and skills. Repositories stay in place with pending changes and local environments. Linking itself does not edit files; subsequent project work edits the originals.",
            json!({"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"}},"required":["path"]}),
        ),
        write_tool(
            "update_project",
            "Update a project's display metadata without renaming its directory or repositories. Returns the updated project.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"displayName":{"type":"string"},"icon":{"type":"string"},"color":{"type":"string","enum":["slate","coral","peach","amber","sage","mint","sky","lavender","rose"]}},"required":["projectId"]}),
        ),
        write_tool(
            "add_project_repository",
            "Add a repository to the project. A local path is linked in place, including uncommitted work: subsequent edits affect that original folder. A credential-free GitHub url is cloned into the project container. Returns project and repository metadata.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"path":{"type":"string"},"url":{"type":"string"}},"required":["projectId"],"oneOf":[{"required":["path"]},{"required":["url"]}]}),
        ),
        read_tool(
            "get_repository",
            "Get a source repository, its owning project, source path, and every task worktree attached to it.",
            id_schema("repositoryId"),
        ),
        read_tool(
            "get_repository_status",
            "Inspect the branch and change count in an original repository or task worktree.",
            json!({"type":"object","properties":{"repositoryId":{"type":"string"},"taskId":{"type":"string"}},"required":["repositoryId"]}),
        ),
        read_tool(
            "search_tasks",
            "Search tasks by title, description, note, branch, repository name/path, or worktree path. Returns task metadata and note excerpts.",
            json!({"type":"object","properties":{"query":{"type":"string","minLength":1},"projectId":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100,"default":20}},"required":["query"]}),
        ),
        read_tool(
            "list_tasks",
            "List tasks, optionally limited to a project.",
            json!({"type":"object","properties":{"projectId":{"type":"string"}}}),
        ),
        read_tool(
            "get_task",
            "Get an isolated task, owning project, task note, source repository paths, and all writable worktree paths.",
            id_schema("taskId"),
        ),
        write_tool(
            "check_branch_availability",
            "Validate a proposed branch and report whether it exists in selected project repositories. Remote mode fetches origin before reporting. Pass baseBranch to also resolve, per repository, the commit a missing branch would be created from.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"branch":{"type":"string"},"branchSource":{"type":"string","enum":["current","local","remote"]},"baseBranch":{"type":"string","description":"Branch, tag or revision a missing task branch would start from, such as master or origin/release/2026-08. Resolved against origin first, then local refs."},"repositoryIds":{"type":"array","items":{"type":"string"}}},"required":["projectId","branch"]}),
        ),
        write_tool(
            "create_task",
            "Create a transactional isolated Git worktree task and optional developer note when the user chooses a task-based workflow. Tasks are optional; direct project work does not require this tool. Returns task metadata and writable worktree paths. Pass `branch` exactly as the branch is spelled on the remote, casing included, when continuing existing work such as a pull request head; it is only normalized when omitted and derived from the title. Pass `baseBranch` to root a new branch in a specific base such as master instead of whatever each repository has checked out. If a global/project agent was asked to create and start/implement the task, immediately call handoff_to_agent with the returned task id and a self-contained implementation brief before starting repository implementation. That request already authorizes delegation; no extra confirmation is needed. If asked only to create it, do not start implementation. The receiving task agent works in the returned worktrees without recreating or redelegating the task. The app automatically adds a task navigation card. The agent completing the task calls notify_task_ready.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"branch":{"type":"string"},"branchSource":{"type":"string","enum":["current","local","remote"]},"baseBranch":{"type":"string","description":"Branch, tag or revision to create the task branch from when it does not exist yet, such as master or origin/release/2026-08. Resolved per repository against origin first, then local refs, then tags and raw revisions; origin is fetched first. A repository where the task branch already exists keeps that branch and ignores this. Omit to branch from whatever each repository has checked out."},"createMissingBranch":{"type":"boolean"},"replaceDivergentLocalBranches":{"type":"boolean"},"existingBranchAction":{"type":"string","enum":["reuse","recreate"]},"repositoryIds":{"type":"array","items":{"type":"string"}},"copyLocalChanges":{"type":"boolean"},"copyEnvironmentFiles":{"type":"boolean"},"setupCommand":{"type":"string"},"developerContext":{"type":"string","maxLength":2000}},"required":["projectId","title"]}),
        ),
        write_tool(
            "update_task",
            "Update a task title, description, or color and refresh its generated task context files. A null description clears it.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"title":{"type":"string"},"description":{"type":["string","null"]},"color":{"type":"string","enum":["slate","coral","peach","amber","sage","mint","sky","lavender","rose"]}},"required":["taskId"]}),
        ),
        destructive_tool(
            "delete_task",
            "Delete a task record and its managed worktrees while preserving Git branches. Requires confirm=true and refuses active terminals or uncommitted changes.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"confirm":{"type":"boolean","const":true}},"required":["taskId","confirm"]}),
        ),
        write_tool(
            "add_task_repositories",
            "Provision more project repositories into an existing isolated task. Pass baseBranch when a repository still has to create the task branch and it must start from a specific base such as master.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"repositoryIds":{"type":"array","items":{"type":"string"}},"branchSource":{"type":"string","enum":["current","local","remote"]},"baseBranch":{"type":"string","description":"Branch, tag or revision to create the task branch from when it does not exist yet, such as master or origin/release/2026-08. Resolved per repository against origin first, then local refs, then tags and raw revisions; origin is fetched first. A repository where the task branch already exists keeps that branch and ignores this. Omit to branch from whatever each repository has checked out."},"createMissingBranch":{"type":"boolean"},"replaceDivergentLocalBranches":{"type":"boolean"},"existingBranchAction":{"type":"string","enum":["reuse","recreate"]},"copyLocalChanges":{"type":"boolean"},"copyEnvironmentFiles":{"type":"boolean"},"setupCommand":{"type":"string"}},"required":["taskId","repositoryIds"]}),
        ),
        destructive_tool(
            "remove_task_repositories",
            "Detach repositories from a task and delete their worktrees, undoing a wrong add_task_repositories. The Git branch is preserved unless deleteBranch is true; delete it when the repository will be added again on a different baseBranch, because an existing branch is reused as it stands. Refuses uncommitted changes unless discardUncommittedChanges is true, refuses open terminals, and refuses to leave the task with no repositories.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"repositoryIds":{"type":"array","items":{"type":"string"}},"deleteBranch":{"type":"boolean"},"discardUncommittedChanges":{"type":"boolean"}},"required":["taskId","repositoryIds"]}),
        ),
        read_tool(
            "get_project_note",
            "Read the full local project note and return its file name and path.",
            id_schema("projectId"),
        ),
        write_tool(
            "write_project_note",
            "Replace the full local project note content.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"content":{"type":"string","maxLength":2097152}},"required":["projectId","content"]}),
        ),
        write_tool(
            "append_project_note",
            "Append concise durable developer context to the local project note.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"context":{"type":"string","maxLength":2000}},"required":["projectId","context"]}),
        ),
        write_tool(
            "write_task_note",
            "Replace the full task-local AI note content.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"content":{"type":"string","maxLength":2097152}},"required":["taskId","content"]}),
        ),
        write_tool(
            "append_task_note",
            "Append concise implementation context to the task-local AI note.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"context":{"type":"string","maxLength":2000}},"required":["taskId","context"]}),
        ),
        write_tool(
            "handoff_to_agent",
            "Transfer implementation ownership to a persistent user-visible Black Bot and start it immediately in the destination project or isolated task. Use this immediately after creation when a global/project agent is asked to create a task and start/implement it; that wording is sufficient authorization and does not require another confirmation. Also use it to start an existing task from a global/project chat, or for global-to-project implementation without creating a task. Honor explicit requests not to delegate. Do not delegate for creation-only or planning-only requests, and do not hand off to yourself when already the receiving task/project agent. Carry over all user constraints, including limits on tests, servers and remote writes. If delivery fails, report that the handoff did not start rather than silently implementing in the sender. Provide exactly one of projectId or taskId. `prompt` must be the self-contained work instruction for the receiving agent, including relevant decisions and acceptance criteria; do not pass the orchestration request (for example, do not tell a task agent to create the task that already exists). The desktop keeps the current chat open and shows a clickable Black Bot toast that opens the receiving agent. This is a true handoff: after it succeeds, do not perform the delegated implementation in the sending session.",
            json!({"type":"object","properties":{"projectId":{"type":"string"},"taskId":{"type":"string"},"prompt":{"type":"string","minLength":1,"maxLength":16000},"title":{"type":"string","maxLength":80},"message":{"type":"string","maxLength":200}},"required":["prompt"],"oneOf":[{"required":["projectId"]},{"required":["taskId"]}]}),
        ),
        write_tool(
            "open_project",
            "Add a persistent clickable project card to the current Blackholes chat response. Call only when the user explicitly asks to open, show, or go to a project. The app keeps the current chat visible; clicking the card selects the project, reveals and scrolls to it in the sidebar, and opens its main project view.",
            id_schema("projectId"),
        ),
        write_tool(
            "open_task",
            "Add a persistent clickable task card to the current Blackholes chat response. Call only when the user explicitly asks to open, show, or go to a task. The app keeps the current chat visible; clicking the card selects and expands the task, scrolls to it in the sidebar, and opens its main task view. To report finished work, call notify_task_ready instead.",
            id_schema("taskId"),
        ),
        write_tool(
            "notify_task_ready",
            "Tell the user a task is ready. Shows a toast in the running Blackholes desktop app without changing the user's current view; the user opens the task by clicking the toast. ALWAYS call this as the very last tool call of a task-related request, after every other step is finished. Creating a task shows no notification on its own, so skipping this leaves the user unaware the work is done. Use title and message to say what is ready in the user's language.",
            json!({"type":"object","properties":{"taskId":{"type":"string"},"title":{"type":"string","maxLength":80},"message":{"type":"string","maxLength":200}},"required":["taskId"]}),
        ),
    ]
}

fn read_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(name, description, input_schema, true, false, true)
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(name, description, input_schema, false, false, false)
}

fn destructive_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(name, description, input_schema, false, true, false)
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": idempotent,
            "openWorldHint": false,
        }
    })
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn id_schema(name: &str) -> Value {
    let mut properties = Map::new();
    properties.insert(name.to_string(), json!({ "type": "string" }));
    json!({ "type": "object", "properties": properties, "required": [name] })
}

/// Read an optional string argument, trimmed and clipped to `limit` characters.
/// The bridge to the desktop app is a datagram with a bounded buffer, so an
/// oversized notification would be dropped rather than shortened.
fn optional_truncated(arguments: &Value, name: &str, limit: usize) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(limit).collect())
}

fn required_string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing {name}"))
}

fn required_uuid(arguments: &Value, name: &str) -> Result<Uuid> {
    Uuid::parse_str(required_string(arguments, name)?).with_context(|| format!("invalid {name}"))
}

fn optional_uuid(arguments: &Value, name: &str) -> Result<Option<Uuid>> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(|value| Uuid::parse_str(value).with_context(|| format!("invalid {name}")))
        .transpose()
}

fn optional_uuid_list(arguments: &Value, name: &str) -> Result<Vec<Uuid>> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    Uuid::parse_str(value.as_str().context("repository id must be a string")?)
                        .context("invalid repository id")
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn search_limit(arguments: &Value) -> usize {
    arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize
}

fn excerpt(content: &str, maximum_characters: usize) -> String {
    let mut characters = content.chars();
    let excerpt = characters
        .by_ref()
        .take(maximum_characters)
        .collect::<String>();
    if characters.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

fn find_workspace(workspaces: &[Workspace], id: Uuid) -> Result<&Workspace> {
    workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .context("project not found")
}

fn find_task(tasks: &[ProjectTask], id: Uuid) -> Result<&ProjectTask> {
    tasks
        .iter()
        .find(|task| task.id == id)
        .context("task not found")
}

fn append_note(task: &ProjectTask, context: &str) -> Result<()> {
    let mut note = TaskNoteService::read(task)?;
    append_developer_context(&mut note, context)?;
    TaskNoteService::write(task, &note)
}

fn append_developer_context(note: &mut String, context: &str) -> Result<()> {
    let context = context.trim();
    if context.is_empty() {
        bail!("context cannot be empty")
    }
    if context.chars().count() > 2_000 {
        bail!("context is limited to 2,000 characters")
    }
    if !note.is_empty() && !note.ends_with('\n') {
        note.push('\n');
    }
    note.push_str("\n## Developer context\n\n");
    note.push_str(context);
    note.push_str("\n");
    Ok(())
}

fn git_output<const N: usize>(path: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_output_optional<const N: usize>(path: &Path, args: [&str; N]) -> Option<String> {
    git_output(path, args).ok()
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(length) = trimmed.strip_prefix("Content-Length:") {
            let length: usize = length.trim().parse()?;
            loop {
                line.clear();
                reader.read_line(&mut line)?;
                if line.trim().is_empty() {
                    break;
                }
            }
            let mut payload = vec![0_u8; length];
            reader.read_exact(&mut payload)?;
            return Ok(Some(serde_json::from_slice(&payload)?));
        }
        return Ok(Some(serde_json::from_str(trimmed)?));
    }
}
