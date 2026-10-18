//! The destructive task-close path used by both the desktop and MCP bridge.
//! Persisted task-session states are history, never evidence of a live process.
use super::*;
use crate::{
    model::{AgentKind, SessionState, TerminalDescriptor},
    services::terminal::{SharedChild, SharedMasterPty},
};
use std::{
    os::unix::fs::MetadataExt,
    time::{Duration, Instant},
};

pub const MAX_CLOSE_TASKS: usize = 32;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPolicy {
    #[default]
    Fail,
    CloseIdle,
    CloseAll,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilePolicy {
    #[default]
    Fail,
    Discard,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseOptions {
    #[serde(default)]
    pub terminals: TerminalPolicy,
    #[serde(default)]
    pub uncommitted_changes: FilePolicy,
    #[serde(default)]
    pub extra_files: FilePolicy,
    #[serde(default)]
    pub delete_branches: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallerContext {
    pub terminal_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub cwd: Option<PathBuf>,
    pub pid: Option<u32>,
}

impl CallerContext {
    pub fn current() -> Self {
        Self {
            terminal_id: std::env::var("BLACKHOLES_TERMINAL_ID")
                .ok()
                .and_then(|s| s.parse().ok()),
            task_id: std::env::var("BLACKHOLES_TASK_ID")
                .ok()
                .and_then(|s| s.parse().ok()),
            cwd: std::env::current_dir().ok(),
            pid: Some(std::process::id()),
        }
    }

    fn protects(&self, task: &ProjectTask, terminals: &[CloseTerminal]) -> bool {
        self.task_id == Some(task.id)
            || terminals
                .iter()
                .any(|terminal| self.terminal_id == Some(terminal.descriptor.id))
            || self.cwd.as_ref().is_some_and(|cwd| {
                let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
                let root = fs::canonicalize(&task.worktree_root_path)
                    .unwrap_or_else(|_| task.worktree_root_path.clone());
                cwd.starts_with(root)
            })
            || self.pid.is_some_and(|pid| {
                terminals.iter().any(|terminal| {
                    terminal
                        .process
                        .as_ref()
                        .is_some_and(|process| process.contains_pid(pid))
                })
            })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseTasksRequest {
    pub task_ids: Vec<Uuid>,
    pub confirm: bool,
    #[serde(default)]
    pub options: CloseOptions,
    #[serde(default)]
    pub caller: CallerContext,
}

impl CloseTasksRequest {
    pub fn validate(&self) -> Result<()> {
        if !self.confirm {
            bail!("Closing tasks requires confirm=true");
        }
        if self.task_ids.is_empty() || self.task_ids.len() > MAX_CLOSE_TASKS {
            bail!("Choose 1–{MAX_CLOSE_TASKS} tasks to close");
        }
        if self.task_ids.iter().collect::<HashSet<_>>().len() != self.task_ids.len() {
            bail!("Choose each task once");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct LiveTerminal {
    pub child: SharedChild,
    pub master: SharedMasterPty,
    pub process_id: Option<u32>,
    pub state: std::sync::Arc<parking_lot::Mutex<SessionState>>,
}

impl LiveTerminal {
    fn is_live(&self) -> Result<bool> {
        if self.child.lock().try_wait()?.is_none() {
            return Ok(true);
        }
        // A foreground job can outlive its shell. The PTY is authoritative
        // for that group even after the direct child has been reaped.
        let group = self.master.lock().process_group_leader();
        match group {
            Some(group) if group > 1 && group != unsafe { libc::getpgrp() } => {
                groups_alive(&HashSet::from([group]))
            }
            _ => Ok(false),
        }
    }

    fn contains_pid(&self, pid: u32) -> bool {
        self.process_id.is_some_and(|root| {
            process_table()
                .is_ok_and(|table| descendants(&table, root as i32).contains(&(pid as i32)))
        })
    }

    /// Signal only process groups belonging to this PTY, including shell jobs.
    /// Do not signal Blackholes' process group or accept an unverified PID.
    pub fn shutdown(&self) -> Result<()> {
        self.shutdown_with_policy(TerminalPolicy::CloseAll)
    }

    fn shutdown_with_policy(&self, policy: TerminalPolicy) -> Result<()> {
        if !self.is_live()? {
            return Ok(());
        }
        let root = self
            .process_id
            .context("Cannot identify the terminal process safely")? as i32;
        let table = process_table()?;
        let mut members = if self.child.lock().try_wait()?.is_none() {
            descendants(&table, root)
        } else {
            // A reaped shell's numeric PID may already belong to another
            // process. Only its still-owned PTY foreground group is usable.
            HashSet::new()
        };
        let foreground = self.master.lock().process_group_leader();
        if let Some(group) = foreground {
            for process in table.iter().filter(|p| p.group == group) {
                members.insert(process.pid);
            }
        }
        let mut groups: HashSet<i32> = table
            .iter()
            .filter(|p| members.contains(&p.pid))
            .map(|p| p.group)
            .collect();
        if let Some(group) = self.master.lock().process_group_leader()
            && table
                .iter()
                .any(|p| p.group == group && members.contains(&p.pid))
        {
            groups.insert(group);
        }
        // A process group containing a foreign process must never be signalled.
        if groups.is_empty()
            || groups.iter().any(|group| {
                *group <= 1
                    || *group == unsafe { libc::getpgrp() }
                    || table
                        .iter()
                        .any(|p| p.group == *group && !members.contains(&p.pid))
            })
        {
            bail!("Cannot isolate the terminal process groups safely");
        }
        // The UI updates this shared state as lifecycle events arrive. Check
        // immediately before signalling, not only in an earlier snapshot.
        let state = self.state.lock();
        if policy == TerminalPolicy::Fail
            || (policy == TerminalPolicy::CloseIdle
                && !matches!(*state, SessionState::Idle | SessionState::Attention))
        {
            bail!(
                "Terminal is now {:?}; the requested terminal policy blocks closure",
                *state
            );
        }
        signal_groups(&groups, libc::SIGHUP)?;
        drop(state);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            // Reap the shell if it exited. Descendants may still be alive.
            self.child.lock().try_wait()?;
            if !groups_alive(&groups)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        signal_groups(&groups, libc::SIGKILL)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.child.lock().try_wait()?;
            if !groups_alive(&groups)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("Terminal processes survived SIGKILL");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

struct Process {
    pid: i32,
    parent: i32,
    group: i32,
    zombie: bool,
}
fn process_table() -> Result<Vec<Process>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat="])
        .output()?;
    if !output.status.success() {
        bail!("Cannot inspect terminal process groups");
    }
    String::from_utf8(output.stdout)?
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            Ok(Process {
                pid: fields.next().context("Missing process ID")?.parse()?,
                parent: fields.next().context("Missing parent ID")?.parse()?,
                group: fields.next().context("Missing process group")?.parse()?,
                zombie: fields
                    .next()
                    .context("Missing process state")?
                    .starts_with('Z'),
            })
        })
        .collect()
}
fn descendants(table: &[Process], root: i32) -> HashSet<i32> {
    let mut ids = HashSet::from([root]);
    loop {
        let before = ids.len();
        for process in table {
            if ids.contains(&process.parent) {
                ids.insert(process.pid);
            }
        }
        if ids.len() == before {
            return ids;
        }
    }
}
fn signal_groups(groups: &HashSet<i32>, signal: i32) -> Result<()> {
    for group in groups {
        if unsafe { libc::kill(-*group, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
fn groups_alive(groups: &HashSet<i32>) -> Result<bool> {
    Ok(process_table()?
        .iter()
        .any(|p| groups.contains(&p.group) && !p.zombie))
}

#[derive(Clone)]
pub struct CloseTerminal {
    pub descriptor: TerminalDescriptor,
    /// Only the running app can supply this handle. Missing handle = restored tab.
    pub process: Option<LiveTerminal>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedSession {
    pub agent: AgentKind,
    pub session_id: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPreview {
    pub terminal_id: Uuid,
    pub label: String,
    pub state: SessionState,
    pub live: bool,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChanges {
    pub path: PathBuf,
    pub changes: String,
}
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClosePreview {
    pub terminals: Vec<TerminalPreview>,
    pub changes: Vec<WorktreeChanges>,
    pub extra_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseTaskResult {
    pub task_id: Uuid,
    pub status: &'static str,
    pub reason: Option<String>,
    pub closed_terminals: Vec<Uuid>,
    pub sessions: Vec<SavedSession>,
    pub removed_worktrees: Vec<RemovedTaskRepository>,
    pub discarded_extra_files: Vec<PathBuf>,
    /// Allocated bytes removed, excluding Git objects shared with the source.
    /// An estimate on compressed/copy-on-write filesystems, not a volume free-space delta.
    pub bytes_freed: u64,
}

impl CloseTaskResult {
    pub fn blocked(task_id: Uuid, reason: impl Into<String>) -> Self {
        Self {
            task_id,
            status: "blocked",
            reason: Some(reason.into()),
            closed_terminals: vec![],
            sessions: vec![],
            removed_worktrees: vec![],
            discarded_extra_files: vec![],
            bytes_freed: 0,
        }
    }
}

pub struct CloseTaskSnapshot {
    pub task_id: Uuid,
    pub task: Option<ProjectTask>,
    pub sources: Vec<Repository>,
    pub terminals: Vec<CloseTerminal>,
}

impl TaskService {
    pub fn close_tasks(
        &self,
        request: &CloseTasksRequest,
        snapshots: &[CloseTaskSnapshot],
        expected: Option<&ClosePreview>,
    ) -> Result<Vec<CloseTaskResult>> {
        request.validate()?;
        Ok(request
            .task_ids
            .iter()
            .map(
                |id| match snapshots.iter().find(|snapshot| snapshot.task_id == *id) {
                    Some(snapshot) => match &snapshot.task {
                        Some(task) if task.id == *id => self.close_task(
                            task,
                            &snapshot.sources,
                            &request.options,
                            &snapshot.terminals,
                            &request.caller,
                            expected,
                        ),
                        _ => CloseTaskResult::blocked(*id, "Task not found"),
                    },
                    None => CloseTaskResult::blocked(*id, "Task not found"),
                },
            )
            .collect())
    }

    fn validate_close_paths(&self, task: &ProjectTask) -> Result<()> {
        self.assert_managed_task_path(&task.worktree_root_path)?;
        // Do not canonicalize away symlinks or '..' before validating ownership.
        let managed = std::iter::once(&self.managed_root)
            .chain(self.legacy_managed_roots.iter())
            .find(|root| task.worktree_root_path.starts_with(root))
            .context("Task path is not inside a managed task workspace")?;
        let relative = task.worktree_root_path.strip_prefix(managed)?;
        if relative.components().count() < 2 {
            bail!("Refusing to delete a project workspace root");
        }
        let mut path = managed.clone();
        for component in relative.components() {
            if !matches!(component, std::path::Component::Normal(_)) {
                bail!("Invalid managed task path");
            }
            path.push(component);
            reject_symlink(&path)?;
        }
        assert_task_repository_paths(task)?;
        for repository in &task.repositories {
            if repository.worktree_path.parent() != Some(task.worktree_root_path.as_path()) {
                bail!("Worktrees must be direct children of the task directory");
            }
            reject_symlink(&repository.worktree_path)?;
        }
        Ok(())
    }

    pub fn preview_close(
        &self,
        task: &ProjectTask,
        terminals: &[CloseTerminal],
    ) -> Result<ClosePreview> {
        self.validate_close_paths(task)?;
        let mut preview = ClosePreview::default();
        for terminal in terminals {
            preview.terminals.push(TerminalPreview {
                terminal_id: terminal.descriptor.id,
                label: terminal.descriptor.label.clone(),
                state: terminal
                    .process
                    .as_ref()
                    .map(|p| *p.state.lock())
                    .unwrap_or(terminal.descriptor.state),
                live: terminal
                    .process
                    .as_ref()
                    .map(LiveTerminal::is_live)
                    .transpose()?
                    .unwrap_or(false),
            });
        }
        for repository in &task.repositories {
            if repository.worktree_path.exists() {
                // Includes ignored files: discarding build output or local environments also needs consent.
                let changes = git_output(
                    &repository.worktree_path,
                    [
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                        "--ignored=matching",
                    ],
                )?;
                if !changes.trim().is_empty() {
                    preview.changes.push(WorktreeChanges {
                        path: repository.worktree_path.clone(),
                        changes,
                    });
                }
            }
        }
        if task.worktree_root_path.exists() {
            for entry in fs::read_dir(&task.worktree_root_path)? {
                let path = entry?.path();
                if task.repositories.iter().any(|r| r.worktree_path == path) {
                    continue;
                }
                if !generated_file(&path)? {
                    preview.extra_files.push(path);
                }
            }
        }
        preview.extra_files.sort();
        Ok(preview)
    }

    /// Preflight everything before closing any terminal. Errors after deletion begins
    /// keep a blocked task record and report every completed destructive step.
    pub fn close_task(
        &self,
        task: &ProjectTask,
        sources: &[Repository],
        options: &CloseOptions,
        terminals: &[CloseTerminal],
        caller: &CallerContext,
        expected: Option<&ClosePreview>,
    ) -> CloseTaskResult {
        let mut result = CloseTaskResult::blocked(task.id, "Closure did not finish");
        for terminal in terminals {
            for (agent, id) in [
                (
                    AgentKind::Claude,
                    terminal.descriptor.claude_session.as_ref().map(|s| &s.id),
                ),
                (
                    AgentKind::Codex,
                    terminal.descriptor.codex_session.as_ref().map(|s| &s.id),
                ),
            ] {
                if let Some(id) = id {
                    let session = SavedSession {
                        agent,
                        session_id: id.clone(),
                    };
                    if !result.sessions.contains(&session) {
                        result.sessions.push(session);
                    }
                }
            }
        }
        let operation = (|| -> Result<()> {
            if caller.protects(task, terminals) {
                bail!(
                    "The calling MCP terminal/task is protected; close it from another project or the UI"
                );
            }
            let preview = self.preview_close(task, terminals)?;
            if let Some(expected) = expected
                && *expected != preview
            {
                bail!(
                    "Task contents or terminals changed since confirmation; review the task again"
                );
            }
            let mut blockers = Vec::new();
            for terminal in &preview.terminals {
                if terminal.live
                    && (options.terminals == TerminalPolicy::Fail
                        || (options.terminals == TerminalPolicy::CloseIdle
                            && !matches!(
                                terminal.state,
                                SessionState::Idle | SessionState::Attention
                            )))
                {
                    blockers.push(format!(
                        "Terminal {} ({}, {:?}) requires {}",
                        terminal.label,
                        terminal.terminal_id,
                        terminal.state,
                        if options.terminals == TerminalPolicy::Fail {
                            "close_idle or close_all"
                        } else {
                            "close_all"
                        }
                    ));
                }
            }
            if options.uncommitted_changes == FilePolicy::Fail && !preview.changes.is_empty() {
                blockers.push(format!(
                    "Uncommitted or ignored worktree files: {}",
                    preview
                        .changes
                        .iter()
                        .map(|c| format!("{}:\n{}", c.path.display(), c.changes.trim_end()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            if options.extra_files == FilePolicy::Fail && !preview.extra_files.is_empty() {
                blockers.push(format!(
                    "Extra task files: {}",
                    preview
                        .extra_files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !blockers.is_empty() {
                bail!("{}", blockers.join("\n"));
            }
            // Resolve and validate every origin before the first mutation, including
            // worktrees whose directory was already removed in a partial attempt.
            let origins = task
                .repositories
                .iter()
                .map(|r| resolve_origin(task, r, sources))
                .collect::<Result<Vec<_>>>()?;
            let before = allocated_bytes(&task.worktree_root_path)?;
            let mut discarded_candidates = preview.extra_files.clone();
            let deletion = (|| -> Result<()> {
                for terminal in terminals {
                    if let Some(process) = &terminal.process {
                        process.shutdown_with_policy(options.terminals)?;
                    }
                    result.closed_terminals.push(terminal.descriptor.id);
                }
                // Reinspect after agents have stopped writing. Never force a newly
                // dirty worktree without the requested discard policy.
                let after = self.preview_close(task, terminals)?;
                discarded_candidates = after.extra_files.clone();
                if options.uncommitted_changes == FilePolicy::Fail && !after.changes.is_empty() {
                    bail!(
                        "Worktree changed while terminals were closing; retry with explicit discard"
                    );
                }
                if options.extra_files == FilePolicy::Fail && !after.extra_files.is_empty() {
                    bail!(
                        "Extra task files appeared while terminals were closing: {:?}",
                        after.extra_files
                    );
                }
                if expected.is_some()
                    && (after.changes != preview.changes
                        || after.extra_files != preview.extra_files)
                {
                    bail!("Task files changed while terminals were closing; review the task again");
                }
                for (repository, source) in task.repositories.iter().zip(&origins) {
                    self.validate_close_paths(task)?;
                    let path = path_text(&repository.worktree_path)?;
                    let registered = registered_worktrees(source)?
                        .iter()
                        .any(|p| p == &repository.worktree_path);
                    if registered {
                        let mut args = vec!["worktree", "remove"];
                        if options.uncommitted_changes == FilePolicy::Discard {
                            args.push("--force");
                        }
                        args.extend(["--", path]);
                        git_output(source, args)
                            .with_context(|| format!("Unable to remove worktree {path}"))?;
                    } else if repository.worktree_path.exists() {
                        bail!("Worktree is no longer registered: {path}");
                    }
                    result.removed_worktrees.push(RemovedTaskRepository {
                        repository_id: repository.repository_id,
                        worktree_path: repository.worktree_path.clone(),
                        branch: repository.branch.clone(),
                        branch_deleted: false,
                    });
                    git_output(source, ["worktree", "prune"])?;
                    if options.delete_branches
                        && revision_for_ref(source, &format!("refs/heads/{}", repository.branch))
                            .is_some()
                    {
                        git_output(source, ["branch", "-D", "--", &repository.branch])?;
                        result.removed_worktrees.last_mut().unwrap().branch_deleted = true;
                    }
                }
                self.validate_close_paths(task)?;
                let final_extras = self.preview_close(task, &[])?.extra_files;
                if (options.extra_files == FilePolicy::Fail && !final_extras.is_empty())
                    || (expected.is_some() && final_extras != preview.extra_files)
                {
                    bail!(
                        "Extra task files changed before workspace deletion: {:?}",
                        final_extras
                    );
                }
                discarded_candidates = final_extras;
                if task.worktree_root_path.exists() {
                    remove_directory_with_permission_repair(&task.worktree_root_path, || {
                        self.validate_close_paths(task)
                    })?;
                }
                result.discarded_extra_files = after.extra_files;
                Ok(())
            })();
            result.discarded_extra_files = discarded_candidates
                .iter()
                .filter(|path| {
                    fs::symlink_metadata(path)
                        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                })
                .cloned()
                .collect();
            result.bytes_freed =
                before.saturating_sub(allocated_bytes(&task.worktree_root_path).unwrap_or(before));
            deletion
        })();
        match operation {
            Ok(()) => {
                result.status = "deleted";
                result.reason = None;
            }
            Err(error) => result.reason = Some(format!("{error:#}")),
        }
        result
    }

    /// Called by the app after the shared closure operation, outside disk tests.
    pub fn revoke_closed_task_trust(&self, task: &ProjectTask) {
        workspace_trust::revoke(
            &self.agent_profiles,
            &Self::trust_targets(&task.worktree_root_path, &task.repositories),
        );
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("Refusing managed path symlink: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn generated_file(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if name == "AGENTS.md" && metadata.file_type().is_symlink() {
        return Ok(fs::read_link(path)? == Path::new("CLAUDE.md")
            && generated_file(&path.with_file_name("CLAUDE.md"))?);
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    Ok(match name {
        "CLAUDE.md" | "AGENTS.md" => {
            fs::read_to_string(path)?.starts_with("<!-- Generated by Blackholes")
        }
        ".blackholes-task.json"
        | ".blackholes-task-details.md"
        | ".blackholes-note.md"
        | ".blackholes-note.blocks.json" => true,
        _ => false,
    })
}
fn registered_worktrees(source: &Path) -> Result<Vec<PathBuf>> {
    let output = git_output(source, ["worktree", "list", "--porcelain", "-z"])?;
    Ok(output
        .split('\0')
        .filter_map(|field| field.strip_prefix("worktree ").map(PathBuf::from))
        .collect())
}
fn resolve_origin(
    task: &ProjectTask,
    repository: &TaskRepository,
    sources: &[Repository],
) -> Result<PathBuf> {
    let source = sources
        .iter()
        .find(|source| source.id == repository.repository_id)
        .context("Original repository is no longer attached to the project")?
        .path
        .clone();
    let canonical_source = fs::canonicalize(&source)?;
    let canonical_root = fs::canonicalize(&task.worktree_root_path)
        .unwrap_or_else(|_| task.worktree_root_path.clone());
    if canonical_source.starts_with(canonical_root) {
        bail!("Origin must be outside the task workspace");
    }
    // Verify the .git file against the project's known origin. A nested ordinary
    // repository or a foreign worktree must never be removed as a managed one.
    if repository.worktree_path.exists() {
        let actual = fs::canonicalize(repository_source_path(&repository.worktree_path)?)?;
        if actual != canonical_source {
            bail!(
                "Worktree origin differs from the project's repository: {}",
                repository.worktree_path.display()
            );
        }
        if !registered_worktrees(&source)?
            .iter()
            .any(|p| p == &repository.worktree_path)
        {
            bail!(
                "Path is not a registered managed worktree: {}",
                repository.worktree_path.display()
            );
        }
    } else {
        // Also validate the origin before pruning a missing worktree on retry.
        registered_worktrees(&source)?;
    }
    Ok(source)
}

fn allocated_bytes(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut bytes = 0;
    let mut seen = HashSet::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if seen.insert((metadata.dev(), metadata.ino())) {
            bytes += metadata.blocks() * 512;
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
