use crate::model::{ProjectTask, Workspace};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const TASK_NOTE_FILE_NAME: &str = ".blackholes-note.md";
pub const PROJECT_NOTE_FILE_NAME: &str = ".blackholes-project-note.md";
const TASK_NOTE_BLOCKS_FILE_NAME: &str = ".blackholes-note.blocks.json";
const PROJECT_NOTE_BLOCKS_FILE_NAME: &str = ".blackholes-project-note.blocks.json";
pub const PROJECT_INSTRUCTIONS_FILE_NAME: &str = "CLAUDE.md";
pub const PROJECT_TASK_INSTRUCTIONS_FILE_NAME: &str = ".blackholes-task-CLAUDE.md";
const MAXIMUM_NOTE_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_RICH_NOTE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RichNoteDocument {
    pub markdown: String,
    pub blocks: Option<Value>,
}

#[derive(Deserialize, Serialize)]
struct StoredRichNoteDocument {
    version: u8,
    markdown: String,
    blocks: Value,
}
const DEFAULT_PROJECT_TASK_INSTRUCTIONS: &str = r#"This directory is an isolated Blackholes task workspace containing one or more repository worktrees.

## Required context

- Before doing any work, read `.blackholes-note.md` in this task workspace and use it as the required developer brief.
- Then call the Blackholes MCP `get_current_context` and `get_task` with the task ID above to refresh the task structure and status.
- Treat the note as local developer context, not as a copy of the complete external task.
- Retrieve authoritative ClickUp, Jira, GitHub, or other source details with the available connected tools when needed.

## Filesystem boundaries

- You may inspect the project root and original repository paths returned by Blackholes for context.
- Never create, edit, move, or delete files in an original repository path.
- Make changes only inside worktree paths attached to this task and returned by `get_task`.
- If another project repository is needed, call `add_task_repositories` before changing it.
- Never create, attach, or copy a worktree manually.

## Task notes

- The task note is `./.blackholes-note.md`; read it before starting and update it when durable context changes.
- Keep notes concise and developer-oriented.
- Append only durable implementation context, non-obvious constraints, decisions, or essential acceptance checkpoints.
"#;
const PROJECT_POLICY_START: &str = "<!-- BLACKHOLES PROJECT POLICY START -->";
const PROJECT_POLICY_END: &str = "<!-- BLACKHOLES PROJECT POLICY END -->";
const PROJECT_POLICY: &str = r#"# Blackholes project instructions

## Required Blackholes orchestration

When the Blackholes MCP is available:

- At the beginning of every turn, call `get_current_context` before any other tool. If it is unavailable, report that the required Blackholes MCP is missing and do not fall back to filesystem discovery or UI automation.
- Interpret "create a task" as creating a local Blackholes task. Use the Blackholes MCP for project, task, repository, branch, and worktree orchestration.
- Global and project Black Bots may inspect, review, edit, build, and test directly in the intended project repositories. Resolve the project and repositories through the MCP and respect the user's request, project instructions, selected permissions, and existing changes. A task or worktree is not a prerequisite.
- Tasks and isolated worktrees are optional: use them when requested by the user, when working in an existing selected task, or when user-authored project instructions require them. A request to create a task and start/implement it already authorizes a global or project agent to create it and immediately call `handoff_to_agent` with its `taskId`, before implementing in the new worktrees; no extra delegation confirmation is needed. Creating a task alone does not authorize starting it. For project implementation without a task, the global agent normally hands off with `projectId`; the receiving project agent works directly. The agent of the selected task implements in its attached worktrees and must not delegate to itself or create the task again. Explicit requests to work directly or not delegate take precedence. If execution intent or destination is unclear, ask briefly. Preserve all user constraints in the handoff prompt. After a successful handoff, the sender stops implementing and reports the transfer; on failure, report it rather than silently taking over.
- Do not create a task, issue, comment, branch, pull request, or any other remote resource in ClickUp, Jira, GitHub, GitLab, or another external system. A request to create a Blackholes task is not authorization to mutate those systems.
- External tools may be used only for read-only discovery needed to understand the task, its linked pull requests, repositories, and branch names.
- If the work has associated pull requests or branches, read their metadata to identify the correct source. This does not require a Blackholes task. Do not push, comment, or change remote resources without an explicit request.
- Assume an associated branch may belong to another person or contain unfinished work. Preserve existing changes and do not switch branches or move work into a task without the user's direction.
- When isolation is chosen, use the requested source branch for local Blackholes worktrees. Direct project work remains valid when isolation is not requested or required by user-authored project instructions.
- Only perform a remote mutation when the user explicitly requests that exact remote action in the current conversation. If the request is ambiguous, keep the operation local and ask before touching the remote system.
"#;

pub struct TaskNoteService;

impl TaskNoteService {
    pub fn read(task: &ProjectTask) -> Result<String> {
        read_note(&task_root(task)?, TASK_NOTE_FILE_NAME, "task")
    }

    pub fn write(task: &ProjectTask, content: &str) -> Result<()> {
        write_note(
            &task_root(task)?,
            TASK_NOTE_FILE_NAME,
            ".blackholes-note",
            "task",
            content,
        )
    }

    pub fn read_document(task: &ProjectTask) -> Result<RichNoteDocument> {
        read_rich_note(
            &task_root(task)?,
            TASK_NOTE_FILE_NAME,
            TASK_NOTE_BLOCKS_FILE_NAME,
            "task",
        )
    }

    pub fn write_document(task: &ProjectTask, markdown: &str, blocks: &Value) -> Result<()> {
        write_rich_note(
            &task_root(task)?,
            TASK_NOTE_FILE_NAME,
            TASK_NOTE_BLOCKS_FILE_NAME,
            ".blackholes-note",
            ".blackholes-note-blocks",
            "task",
            markdown,
            blocks,
        )
    }

    pub fn ensure(task_root: &Path, initial_content: &str) -> Result<PathBuf> {
        ensure_note(task_root, TASK_NOTE_FILE_NAME, "task", initial_content)
    }
}

pub struct ProjectNoteService;

impl ProjectNoteService {
    pub fn read(workspace: &Workspace) -> Result<String> {
        read_note(&project_root(workspace)?, PROJECT_NOTE_FILE_NAME, "project")
    }

    pub fn write(workspace: &Workspace, content: &str) -> Result<()> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        write_note(
            &root,
            PROJECT_NOTE_FILE_NAME,
            ".blackholes-project-note",
            "project",
            content,
        )
    }

    pub fn read_document(workspace: &Workspace) -> Result<RichNoteDocument> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        read_rich_note(
            &root,
            PROJECT_NOTE_FILE_NAME,
            PROJECT_NOTE_BLOCKS_FILE_NAME,
            "project",
        )
    }

    pub fn write_document(workspace: &Workspace, markdown: &str, blocks: &Value) -> Result<()> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        write_rich_note(
            &root,
            PROJECT_NOTE_FILE_NAME,
            PROJECT_NOTE_BLOCKS_FILE_NAME,
            ".blackholes-project-note",
            ".blackholes-project-note-blocks",
            "project",
            markdown,
            blocks,
        )
    }

    pub fn ensure(workspace: &Workspace, initial_content: &str) -> Result<PathBuf> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        ProjectInstructionsService::ensure(workspace)?;
        ProjectTaskInstructionsService::ensure(workspace)?;
        ensure_note(&root, PROJECT_NOTE_FILE_NAME, "project", initial_content)
    }
}

pub struct ProjectInstructionsService;

impl ProjectInstructionsService {
    pub fn ensure(workspace: &Workspace) -> Result<PathBuf> {
        let root = project_root(workspace)?;
        ensure_project_agent_context(&root)?;
        Ok(root.join(PROJECT_INSTRUCTIONS_FILE_NAME))
    }

    pub fn read(workspace: &Workspace) -> Result<String> {
        let root = project_root(workspace)?;
        let file_name = project_instructions_file_name(&root)?;
        read_note(&root, file_name, "project instruction")
    }

    pub fn write(workspace: &Workspace, content: &str) -> Result<()> {
        let root = project_root(workspace)?;
        let file_name = project_instructions_file_name(&root)?;
        let temporary_prefix = if file_name == PROJECT_INSTRUCTIONS_FILE_NAME {
            ".blackholes-claude"
        } else {
            ".blackholes-agents"
        };
        write_note(
            &root,
            file_name,
            temporary_prefix,
            "project instruction",
            content,
        )
    }
}

pub struct ProjectTaskInstructionsService;

impl ProjectTaskInstructionsService {
    pub fn ensure(workspace: &Workspace) -> Result<PathBuf> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        ensure_note(
            &root,
            PROJECT_TASK_INSTRUCTIONS_FILE_NAME,
            "project task instruction",
            DEFAULT_PROJECT_TASK_INSTRUCTIONS,
        )
    }

    pub fn read(workspace: &Workspace) -> Result<String> {
        Self::ensure(workspace)?;
        read_note(
            &project_root(workspace)?,
            PROJECT_TASK_INSTRUCTIONS_FILE_NAME,
            "project task instruction",
        )
    }

    pub fn write(workspace: &Workspace, content: &str) -> Result<()> {
        let root = project_root(workspace)?;
        ensure_project_note_locally_ignored(&root)?;
        write_note(
            &root,
            PROJECT_TASK_INSTRUCTIONS_FILE_NAME,
            ".blackholes-task-claude",
            "project task instruction",
            content,
        )
    }
}

fn project_instructions_file_name(root: &Path) -> Result<&'static str> {
    let claude_path = root.join(PROJECT_INSTRUCTIONS_FILE_NAME);
    match fs::symlink_metadata(&claude_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(PROJECT_INSTRUCTIONS_FILE_NAME)
        }
        Ok(metadata)
            if metadata.file_type().is_symlink()
                && fs::read_link(&claude_path)? == PathBuf::from("AGENTS.md") =>
        {
            Ok("AGENTS.md")
        }
        Ok(_) => bail!(
            "{} is not a regular project instruction file or a supported symlink",
            claude_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PROJECT_INSTRUCTIONS_FILE_NAME)
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_project_agent_context(root: &Path) -> Result<()> {
    let claude_path = root.join(PROJECT_INSTRUCTIONS_FILE_NAME);
    match fs::symlink_metadata(&claude_path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                && fs::read_link(&claude_path)? == PathBuf::from("AGENTS.md") =>
        {
            upsert_project_policy(root, "AGENTS.md", ".blackholes-agents")?;
        }
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            upsert_project_policy(root, PROJECT_INSTRUCTIONS_FILE_NAME, ".blackholes-claude")?;
        }
        Ok(_) => bail!(
            "{} is not a regular project instruction file or a supported symlink",
            claude_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            upsert_project_policy(root, PROJECT_INSTRUCTIONS_FILE_NAME, ".blackholes-claude")?;
        }
        Err(error) => return Err(error.into()),
    }

    let agents_path = root.join("AGENTS.md");
    match fs::symlink_metadata(&agents_path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                && fs::read_link(&agents_path)?
                    == PathBuf::from(PROJECT_INSTRUCTIONS_FILE_NAME) => {}
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            upsert_project_policy(root, "AGENTS.md", ".blackholes-agents")?;
        }
        Ok(_) => bail!(
            "{} is not a regular project instruction file or a Blackholes symlink",
            agents_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(PROJECT_INSTRUCTIONS_FILE_NAME, &agents_path)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn upsert_project_policy(root: &Path, file_name: &str, temporary_prefix: &str) -> Result<()> {
    let path = root.join(file_name);
    let existing = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if metadata.len() > MAXIMUM_NOTE_BYTES as u64 {
                bail!("{} is larger than 2 MiB", path.display());
            }
            fs::read_to_string(&path)?
        }
        Ok(_) => bail!(
            "{} is not a regular project instruction file",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let updated = merge_project_policy(&existing)?;
    if updated == existing {
        return Ok(());
    }
    write_note(
        root,
        file_name,
        temporary_prefix,
        "project instruction",
        &updated,
    )
}

fn merge_project_policy(existing: &str) -> Result<String> {
    let managed = format!("{PROJECT_POLICY_START}\n{PROJECT_POLICY}{PROJECT_POLICY_END}");
    match (
        existing.find(PROJECT_POLICY_START),
        existing.find(PROJECT_POLICY_END),
    ) {
        (Some(start), Some(end)) if end >= start => {
            // Migrate only known generated rules. The settings editor also lets users
            // customize this block, so replacing it wholesale would lose their rules.
            let mut policy = existing[start..end].to_string();
            let legacy_rules = [
                (
                    "- Tasks and isolated worktrees are optional: use them when requested by the user, when working in an existing selected task, or when user-authored project instructions require them. Otherwise work directly in the project. Inside a task, change only its attached worktrees, not the original checkouts. Delegation is optional: use `handoff_to_agent` with `projectId` for direct project work or `taskId` for isolated work. After a successful handoff, the sender stops implementing and reports the transfer.",
                    "- Tasks and isolated worktrees are optional:",
                ),
                (
                    "- A global or project Black Bot must not inspect, review, edit, build, or test repository files directly. Search the registered projects, resolve the exact project and required repositories, reuse a clearly matching task or create an isolated task, then call `handoff_to_agent` with that task ID.",
                    "- Global and project Black Bots may",
                ),
                (
                    "- Only the receiving task Black Bot works inside the task's writable worktrees. After a successful handoff, the sender stops implementing and reports the transfer.",
                    "- Tasks and isolated worktrees are optional:",
                ),
                (
                    "- If the external task has associated pull requests or branches, read their metadata and use those branches only as sources for local Blackholes worktrees. Never push to them, update them, comment on them, or change their pull requests.",
                    "- If the work has associated pull requests or branches,",
                ),
                (
                    "- Assume an associated branch may belong to another person or contain unfinished work. Preserve the remote branch exactly as found and continue only in the local worktree.",
                    "- Assume an associated branch may belong",
                ),
                (
                    "- It is acceptable to base local work on that branch, and to reuse the same branch name locally when the user requests it, while keeping all resulting changes local.",
                    "- When isolation is chosen,",
                ),
            ];
            for (legacy, current_prefix) in legacy_rules {
                if let Some(current) = PROJECT_POLICY
                    .lines()
                    .find(|line| line.starts_with(current_prefix))
                {
                    policy = policy
                        .split_inclusive('\n')
                        .map(|line| {
                            if line.trim_end_matches(['\r', '\n']) == legacy {
                                format!("{current}{}", &line[legacy.len()..])
                            } else {
                                line.to_string()
                            }
                        })
                        .collect();
                }
            }
            Ok(format!("{}{policy}{}", &existing[..start], &existing[end..]))
        }
        (None, None) if existing.trim().is_empty() => Ok(format!("{managed}\n")),
        (None, None) => Ok(format!("{}\n\n{managed}\n", existing.trim_end())),
        _ => bail!("The managed Blackholes project policy markers are incomplete"),
    }
}

fn task_root(task: &ProjectTask) -> Result<PathBuf> {
    let root = fs::canonicalize(&task.worktree_root_path).with_context(|| {
        format!(
            "Unable to resolve task workspace {}",
            task.worktree_root_path.display()
        )
    })?;
    if !root.is_dir() {
        bail!("The task workspace is not a directory");
    }
    Ok(root)
}

fn project_root(workspace: &Workspace) -> Result<PathBuf> {
    let root_path = workspace
        .root_path
        .as_ref()
        .context("The project does not have a root directory")?;
    let root = fs::canonicalize(root_path).with_context(|| {
        format!(
            "Unable to resolve project directory {}",
            root_path.display()
        )
    })?;
    if !root.is_dir() {
        bail!("The project root is not a directory");
    }
    Ok(root)
}

fn ensure_project_note_locally_ignored(root: &Path) -> Result<()> {
    let git_directory = root.join(".git");
    let metadata = match fs::symlink_metadata(&git_directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    // Only write through a real, local .git directory. In particular, never
    // follow a repository-controlled symlink or a gitdir indirection to an
    // arbitrary path outside the imported project.
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }

    let info_directory = git_directory.join("info");
    match fs::symlink_metadata(&info_directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => bail!("The Git info directory is not a regular directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&info_directory)?;
        }
        Err(error) => return Err(error.into()),
    }

    let exclude_path = info_directory.join("exclude");
    let existing = match fs::symlink_metadata(&exclude_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if metadata.len() > MAXIMUM_NOTE_BYTES as u64 {
                bail!("The local Git exclude file is larger than 2 MiB");
            }
            fs::read(&exclude_path)?
        }
        Ok(_) => bail!("The local Git exclude file is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let ignored = |file_name: &str| {
        existing.split(|byte| *byte == b'\n').any(|line| {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            line == file_name || line.strip_prefix('/') == Some(file_name)
        })
    };
    let missing = [
        PROJECT_NOTE_FILE_NAME,
        PROJECT_NOTE_BLOCKS_FILE_NAME,
        PROJECT_TASK_INSTRUCTIONS_FILE_NAME,
    ]
    .into_iter()
    .filter(|file_name| !ignored(file_name))
    .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    let mut exclude = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&exclude_path)?;
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        exclude.write_all(b"\n")?;
    }
    for file_name in missing {
        exclude.write_all(format!("/{file_name}\n").as_bytes())?;
    }
    exclude.sync_all()?;
    Ok(())
}

fn read_note(root: &Path, file_name: &str, note_kind: &str) -> Result<String> {
    read_note_with_limit(root, file_name, note_kind, MAXIMUM_NOTE_BYTES)
}

fn read_note_with_limit(
    root: &Path,
    file_name: &str,
    note_kind: &str,
    maximum_bytes: usize,
) -> Result<String> {
    let path = root.join(file_name);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("The {note_kind} note is not a regular file");
    }
    if metadata.len() > maximum_bytes as u64 {
        bail!("The {note_kind} note exceeds its size limit");
    }
    fs::read_to_string(&path).with_context(|| format!("Unable to read {}", path.display()))
}

fn read_rich_note(
    root: &Path,
    markdown_file_name: &str,
    blocks_file_name: &str,
    note_kind: &str,
) -> Result<RichNoteDocument> {
    let markdown = read_note(root, markdown_file_name, note_kind)?;
    let blocks = read_note_with_limit(
        root,
        blocks_file_name,
        &format!("{note_kind} rich"),
        MAXIMUM_RICH_NOTE_BYTES,
    )
    .ok()
    .filter(|content| !content.is_empty())
    .and_then(|content| serde_json::from_str::<StoredRichNoteDocument>(&content).ok())
    .filter(|document| {
        document.version == 1 && document.markdown == markdown && document.blocks.is_array()
    })
    .map(|document| document.blocks);
    Ok(RichNoteDocument { markdown, blocks })
}

#[allow(clippy::too_many_arguments)]
fn write_rich_note(
    root: &Path,
    markdown_file_name: &str,
    blocks_file_name: &str,
    markdown_temporary_prefix: &str,
    blocks_temporary_prefix: &str,
    note_kind: &str,
    markdown: &str,
    blocks: &Value,
) -> Result<()> {
    if !blocks.is_array() {
        bail!("The {note_kind} rich note must contain a block array");
    }
    let stored = serde_json::to_string(&StoredRichNoteDocument {
        version: 1,
        markdown: markdown.to_string(),
        blocks: blocks.clone(),
    })?;
    if stored.len() > MAXIMUM_RICH_NOTE_BYTES {
        bail!("The {note_kind} rich note is larger than 8 MiB");
    }
    write_note(
        root,
        markdown_file_name,
        markdown_temporary_prefix,
        note_kind,
        markdown,
    )?;
    write_note_with_limit(
        root,
        blocks_file_name,
        blocks_temporary_prefix,
        &format!("{note_kind} rich"),
        &stored,
        MAXIMUM_RICH_NOTE_BYTES,
    )
}

fn write_note(
    root: &Path,
    file_name: &str,
    temporary_prefix: &str,
    note_kind: &str,
    content: &str,
) -> Result<()> {
    write_note_with_limit(
        root,
        file_name,
        temporary_prefix,
        note_kind,
        content,
        MAXIMUM_NOTE_BYTES,
    )
}

fn write_note_with_limit(
    root: &Path,
    file_name: &str,
    temporary_prefix: &str,
    note_kind: &str,
    content: &str,
    maximum_bytes: usize,
) -> Result<()> {
    if content.len() > maximum_bytes {
        bail!("The {note_kind} note exceeds its size limit");
    }
    let path = root.join(file_name);
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        bail!("The {note_kind} note is not a regular file");
    }

    let temporary = root.join(format!("{temporary_prefix}-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_note(
    root: &Path,
    file_name: &str,
    note_kind: &str,
    initial_content: &str,
) -> Result<PathBuf> {
    if initial_content.len() > MAXIMUM_NOTE_BYTES {
        bail!("The {note_kind} note is larger than 2 MiB");
    }
    let path = root.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            return Ok(path);
        }
        Ok(_) => bail!("The {note_kind} note is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(initial_content.as_bytes())?;
    Ok(path)
}
