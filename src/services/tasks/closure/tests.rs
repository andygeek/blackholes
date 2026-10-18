use super::*;

fn run_git<'a>(directory: &Path, arguments: impl IntoIterator<Item = &'a str>) -> Result<()> {
    git_output(directory, arguments).map(|_| ())
}
use crate::model::{ClaudeProfile, ClaudeSession, CodexProfile, CodexSession, WorkspaceColor};

struct Fixture {
    root: PathBuf,
    service: TaskService,
    sources: Vec<Repository>,
    task: ProjectTask,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("blackholes-close-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let origin = root.join("origin");
        fs::create_dir(&origin).unwrap();
        run_git(&origin, ["init", "--initial-branch=main"]).unwrap();
        fs::write(origin.join("tracked.txt"), "original\n").unwrap();
        fs::write(origin.join(".gitignore"), "ignored/\n").unwrap();
        run_git(&origin, ["add", "."]).unwrap();
        run_git(
            &origin,
            [
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let managed = root.join("task-workspaces");
        let task_root = managed.join("project/task");
        fs::create_dir_all(&task_root).unwrap();
        let worktree = task_root.join("repo");
        run_git(
            &origin,
            [
                "worktree",
                "add",
                "-b",
                "task-branch",
                path_text(&worktree).unwrap(),
            ],
        )
        .unwrap();
        let now = Utc::now();
        let id = Uuid::new_v4();
        let sources = vec![Repository {
            id,
            name: "repo".into(),
            path: origin,
            branch: Some("main".into()),
        }];
        let task = ProjectTask {
            links: vec![],
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            title: "Test".into(),
            description: None,
            acceptance_criteria: None,
            pull_request_url: None,
            external_task_url: None,
            icon: DEFAULT_TASK_ICON.into(),
            color: WorkspaceColor::Slate,
            sort_order: 0,
            worktree_root_path: task_root,
            repositories: vec![TaskRepository {
                repository_id: id,
                worktree_path: worktree,
                branch: "task-branch".into(),
                base_branch: Some("main".into()),
                base_revision: "HEAD".into(),
                copy_local_changes: false,
                copy_environment_files: false,
                copied_environment_files: vec![],
                setup_command: None,
                prepared_at: None,
                added_at: now,
            }],
            sessions: vec![],
            created_at: now,
            updated_at: now,
        };
        let service = TaskService {
            managed_root: managed,
            legacy_managed_roots: vec![],
            agent_profiles: root.join("profiles"),
        };
        Self {
            root,
            service,
            sources,
            task,
        }
    }
    fn tab(&self, state: SessionState) -> CloseTerminal {
        CloseTerminal {
            descriptor: TerminalDescriptor {
                id: Uuid::new_v4(),
                workspace_id: self.task.workspace_id,
                task_id: Some(self.task.id),
                repository_id: None,
                agent: AgentKind::Codex,
                label: "Agent".into(),
                cwd: self.task.worktree_root_path.clone(),
                state,
                codex_session: Some(CodexSession {
                    id: "codex-session".into(),
                    profile: CodexProfile::Default,
                }),
                claude_session: Some(ClaudeSession {
                    id: "claude-session".into(),
                    profile: ClaudeProfile::Default,
                }),
                agent_config_dir: None,
                model: None,
                created_at: Utc::now(),
            },
            process: None,
        }
    }
    fn close(
        &self,
        options: &CloseOptions,
        tabs: &[CloseTerminal],
        caller: &CallerContext,
    ) -> CloseTaskResult {
        self.service
            .close_task(&self.task, &self.sources, options, tabs, caller, None)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// A PTY child created by the test, with no login startup or user configuration.
struct TestProcess {
    live: LiveTerminal,
}
impl TestProcess {
    fn new(ignore_hup: bool) -> Self {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            if ignore_hup {
                "trap '' HUP; printf ready; while :; do sleep 1; done"
            } else {
                "printf ready; exec sleep 60"
            },
        ]);
        let child = pair.slave.spawn_command(command).unwrap();
        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut ready = [0; 5];
        std::io::Read::read_exact(&mut reader, &mut ready).unwrap();
        Self {
            live: LiveTerminal {
                child: std::sync::Arc::new(parking_lot::Mutex::new(child)),
                master: std::sync::Arc::new(parking_lot::Mutex::new(pair.master)),
                process_id: pid,
                state: std::sync::Arc::new(parking_lot::Mutex::new(SessionState::Idle)),
            },
        }
    }
}
impl Drop for TestProcess {
    fn drop(&mut self) {
        // Only the subprocess owned by this fixture, never an app process.
        let _ = self.live.shutdown();
        let _ = self.live.child.lock().kill();
        let _ = self.live.child.lock().wait();
    }
}

#[test]
fn restored_and_stale_history_do_not_block_and_sessions_survive() {
    let mut f = Fixture::new();
    let tab = f.tab(SessionState::Restored);
    // DB history can say working while the authoritative app has no process.
    f.task.sessions.push(crate::model::TaskSession {
        id: Uuid::new_v4(),
        terminal_local_id: tab.descriptor.id,
        repository_id: None,
        agent: AgentKind::Codex,
        label: "stale".into(),
        state: SessionState::Working,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        exited_at: None,
    });
    fs::write(
        f.task.worktree_root_path.join("CLAUDE.md"),
        "<!-- Generated by Blackholes. -->",
    )
    .unwrap();
    std::os::unix::fs::symlink("CLAUDE.md", f.task.worktree_root_path.join("AGENTS.md")).unwrap();
    let result = f.close(
        &CloseOptions::default(),
        &[tab.clone()],
        &CallerContext::default(),
    );
    assert_eq!(result.status, "deleted", "{:?}", result.reason);
    assert_eq!(result.closed_terminals, vec![tab.descriptor.id]);
    assert_eq!(result.sessions.len(), 2);
    assert!(
        result
            .sessions
            .iter()
            .any(|s| s.session_id == "codex-session")
    );
    assert!(!f.task.worktree_root_path.exists());
    assert_eq!(
        registered_worktrees(&f.sources[0].path).unwrap(),
        vec![f.sources[0].path.clone()]
    );
    run_git(
        &f.sources[0].path,
        ["show-ref", "--verify", "refs/heads/task-branch"],
    )
    .unwrap();
    assert!(result.bytes_freed > 0);
    assert_eq!(
        fs::read_to_string(f.sources[0].path.join("tracked.txt")).unwrap(),
        "original\n"
    );
}

#[test]
fn dirty_ignored_and_extra_files_require_explicit_discard() {
    let f = Fixture::new();
    let worktree = &f.task.repositories[0].worktree_path;
    fs::write(worktree.join("tracked.txt"), "modified\n").unwrap();
    fs::write(worktree.join("untracked.txt"), "evidence").unwrap();
    fs::create_dir(worktree.join("ignored")).unwrap();
    fs::write(worktree.join("ignored/local.env"), "local").unwrap();
    let extra = f.task.worktree_root_path.join("pdf-proof");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join("proof.pdf"), vec![7; 16384]).unwrap();
    let r = f.close(&CloseOptions::default(), &[], &CallerContext::default());
    assert_eq!(r.status, "blocked");
    for name in ["tracked.txt", "untracked.txt", "ignored/", "pdf-proof"] {
        assert!(r.reason.as_ref().unwrap().contains(name), "{:?}", r.reason);
    }
    assert_eq!(r.bytes_freed, 0);
    assert!(worktree.is_dir());
    let options = CloseOptions {
        uncommitted_changes: FilePolicy::Discard,
        extra_files: FilePolicy::Discard,
        ..Default::default()
    };
    let r = f.close(&options, &[], &CallerContext::default());
    assert_eq!(r.status, "deleted", "{:?}", r.reason);
    assert_eq!(r.discarded_extra_files, vec![extra]);
    assert!(r.bytes_freed >= 16384);
}

#[test]
fn idle_attention_and_working_terminal_policies_use_real_processes() {
    for (state, policy, deleted) in [
        (SessionState::Idle, TerminalPolicy::Fail, false),
        (SessionState::Idle, TerminalPolicy::CloseIdle, true),
        (SessionState::Attention, TerminalPolicy::CloseIdle, true),
        (SessionState::Working, TerminalPolicy::CloseIdle, false),
        (SessionState::Working, TerminalPolicy::CloseAll, true),
        (SessionState::Restored, TerminalPolicy::Fail, false),
    ] {
        let f = Fixture::new();
        let process = TestProcess::new(false);
        *process.live.state.lock() = state;
        let mut tab = f.tab(state);
        tab.process = Some(process.live.clone());
        let options = CloseOptions {
            terminals: policy,
            ..Default::default()
        };
        let r = f.close(&options, &[tab], &CallerContext::default());
        assert_eq!(
            r.status,
            if deleted { "deleted" } else { "blocked" },
            "{:?}",
            r.reason
        );
        assert_eq!(process.live.is_live().unwrap(), !deleted);
    }
}

#[test]
fn shutdown_escalates_ignoring_hup_process_group_to_kill() {
    let process = TestProcess::new(true);
    let started = Instant::now();
    process.live.shutdown().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(!process.live.is_live().unwrap());
}

#[test]
fn all_caller_signals_protect_the_whole_task() {
    let f = Fixture::new();
    let process = TestProcess::new(false);
    let mut tab = f.tab(SessionState::Idle);
    tab.process = Some(process.live.clone());
    let options = CloseOptions {
        terminals: TerminalPolicy::CloseAll,
        ..Default::default()
    };
    for caller in [
        CallerContext {
            terminal_id: Some(tab.descriptor.id),
            ..Default::default()
        },
        CallerContext {
            task_id: Some(f.task.id),
            ..Default::default()
        },
        CallerContext {
            cwd: Some(f.task.repositories[0].worktree_path.clone()),
            ..Default::default()
        },
        CallerContext {
            pid: process.live.process_id,
            ..Default::default()
        },
    ] {
        let r = f.close(&options, &[tab.clone()], &caller);
        assert_eq!(r.status, "blocked");
        assert!(r.reason.unwrap().contains("protected"));
        assert!(process.live.is_live().unwrap());
    }
}

#[test]
fn extra_symlinks_are_reported_and_never_followed() {
    let f = Fixture::new();
    let outside = f.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "evidence").unwrap();
    std::os::unix::fs::symlink(&outside, f.task.worktree_root_path.join("pdf-proof")).unwrap();
    assert_eq!(
        f.close(&CloseOptions::default(), &[], &CallerContext::default())
            .status,
        "blocked"
    );
    let options = CloseOptions {
        extra_files: FilePolicy::Discard,
        ..Default::default()
    };
    assert_eq!(
        f.close(&options, &[], &CallerContext::default()).status,
        "deleted"
    );
    assert!(outside.join("keep").exists());
}

#[test]
fn worktree_symlink_and_nonmanaged_paths_are_blocked() {
    let mut f = Fixture::new();
    let path = f.task.repositories[0].worktree_path.clone();
    let moved = f.root.join("moved");
    fs::rename(&path, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &path).unwrap();
    assert_eq!(
        f.close(&CloseOptions::default(), &[], &CallerContext::default())
            .status,
        "blocked"
    );
    fs::remove_file(&path).unwrap();
    f.task.worktree_root_path = f.sources[0].path.clone();
    assert_eq!(
        f.close(&CloseOptions::default(), &[], &CallerContext::default())
            .status,
        "blocked"
    );
    assert!(moved.exists());
    assert!(f.sources[0].path.exists());
}

#[test]
fn branch_deletion_is_opt_in_and_missing_worktrees_can_be_retried() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.task.repositories[0].worktree_path).unwrap();
    let options = CloseOptions {
        delete_branches: true,
        ..Default::default()
    };
    let r = f.close(&options, &[], &CallerContext::default());
    assert_eq!(r.status, "deleted", "{:?}", r.reason);
    assert!(r.removed_worktrees[0].branch_deleted);
    assert!(revision_for_ref(&f.sources[0].path, "refs/heads/task-branch").is_none());
    assert_eq!(registered_worktrees(&f.sources[0].path).unwrap().len(), 1);
}

#[test]
fn changed_confirmation_blocks_and_a_blocked_task_does_not_stop_other_tasks() {
    let f = Fixture::new();
    let preview = f.service.preview_close(&f.task, &[]).unwrap();
    fs::write(f.task.worktree_root_path.join("new-evidence"), "new").unwrap();
    let options = CloseOptions {
        extra_files: FilePolicy::Discard,
        ..Default::default()
    };
    let r = f.service.close_task(
        &f.task,
        &f.sources,
        &options,
        &[],
        &CallerContext::default(),
        Some(&preview),
    );
    assert_eq!(r.status, "blocked");
    assert!(r.reason.unwrap().contains("changed since confirmation"));
    let good = Fixture::new();
    let good_root = f.service.managed_root.join("project/good-task");
    // Both snapshots must be inside the same service's managed root.
    fs::rename(&good.task.worktree_root_path, &good_root).unwrap();
    let mut good_task = good.task.clone();
    good_task.worktree_root_path = good_root.clone();
    good_task.repositories[0].worktree_path = good_root.join("repo");
    run_git(
        &good.sources[0].path,
        [
            "worktree",
            "repair",
            path_text(&good_task.repositories[0].worktree_path).unwrap(),
        ],
    )
    .unwrap();
    let missing = Uuid::new_v4();
    let request = CloseTasksRequest {
        task_ids: vec![f.task.id, good_task.id, missing],
        confirm: true,
        options: CloseOptions::default(),
        caller: CallerContext::default(),
    };
    let results = f
        .service
        .close_tasks(
            &request,
            &[
                CloseTaskSnapshot {
                    task_id: f.task.id,
                    task: Some(f.task.clone()),
                    sources: f.sources.clone(),
                    terminals: vec![],
                },
                CloseTaskSnapshot {
                    task_id: good_task.id,
                    task: Some(good_task),
                    sources: good.sources.clone(),
                    terminals: vec![],
                },
            ],
            None,
        )
        .unwrap();
    assert_eq!(results[0].status, "blocked");
    assert_eq!(results[1].status, "deleted", "{:?}", results[1].reason);
    assert_eq!(results[2].status, "blocked");
}

#[test]
fn request_validation_and_policy_defaults_are_safe() {
    let mut request: CloseTasksRequest = serde_json::from_value(serde_json::json!({
        "taskIds": [Uuid::new_v4()], "confirm": true,
    }))
    .unwrap();
    request.validate().unwrap();
    assert_eq!(request.options.terminals, TerminalPolicy::Fail);
    assert_eq!(request.options.extra_files, FilePolicy::Fail);
    assert!(!request.options.delete_branches);
    request.confirm = false;
    assert!(request.validate().is_err());
    request.confirm = true;
    request.task_ids.push(request.task_ids[0]);
    assert!(request.validate().is_err());
    request.task_ids = vec![];
    assert!(request.validate().is_err());
    request.task_ids = (0..33).map(|_| Uuid::new_v4()).collect();
    assert!(request.validate().is_err());
    assert!(
        serde_json::from_value::<CloseOptions>(serde_json::json!({"terminals":"all"})).is_err()
    );
}

#[test]
fn terminal_policy_is_rechecked_at_the_signal_boundary() {
    let process = TestProcess::new(false);
    *process.live.state.lock() = SessionState::Working;
    assert!(
        process
            .live
            .shutdown_with_policy(TerminalPolicy::CloseIdle)
            .is_err()
    );
    assert!(process.live.is_live().unwrap());
    *process.live.state.lock() = SessionState::Idle;
    process
        .live
        .shutdown_with_policy(TerminalPolicy::CloseIdle)
        .unwrap();
    assert!(!process.live.is_live().unwrap());
}

#[test]
fn blocked_preflight_does_not_stop_idle_terminals() {
    let f = Fixture::new();
    let process = TestProcess::new(false);
    let mut tab = f.tab(SessionState::Idle);
    tab.process = Some(process.live.clone());
    fs::write(f.task.worktree_root_path.join("evidence.pdf"), "keep").unwrap();
    let options = CloseOptions {
        terminals: TerminalPolicy::CloseAll,
        ..Default::default()
    };
    let result = f.close(&options, &[tab], &CallerContext::default());
    assert_eq!(result.status, "blocked");
    assert!(result.closed_terminals.is_empty());
    assert!(process.live.is_live().unwrap());
}

#[test]
fn partial_cleanup_reports_removed_worktrees_and_retry_finishes() {
    let mut f = Fixture::new();
    let second_path = f.task.worktree_root_path.join("second");
    run_git(
        &f.sources[0].path,
        [
            "worktree",
            "add",
            "-b",
            "second-branch",
            path_text(&second_path).unwrap(),
        ],
    )
    .unwrap();
    run_git(
        &f.sources[0].path,
        ["worktree", "lock", path_text(&second_path).unwrap()],
    )
    .unwrap();
    let mut second = f.task.repositories[0].clone();
    second.worktree_path = second_path.clone();
    second.branch = "second-branch".into();
    f.task.repositories.push(second);
    let first = f.close(&CloseOptions::default(), &[], &CallerContext::default());
    assert_eq!(first.status, "blocked");
    assert_eq!(first.removed_worktrees.len(), 1);
    assert!(first.reason.as_ref().unwrap().contains("locked"));
    assert!(!f.task.repositories[0].worktree_path.exists());
    assert!(second_path.is_dir());
    run_git(
        &f.sources[0].path,
        ["worktree", "unlock", path_text(&second_path).unwrap()],
    )
    .unwrap();
    let retry = f.close(&CloseOptions::default(), &[], &CallerContext::default());
    assert_eq!(retry.status, "deleted", "{:?}", retry.reason);
    assert!(!f.task.worktree_root_path.exists());
}

#[test]
fn task_record_and_session_result_are_saved_atomically_in_temporary_database() {
    use crate::services::database::Database;
    let f = Fixture::new();
    let paths = AppPaths {
        database: f.root.join("db.sqlite"),
        session: f.root.join("session.json"),
        task_workspaces: f.service.managed_root.clone(),
        agent_profiles: f.root.join("profiles"),
        default_projects: f.root.join("projects"),
        events_socket: f.root.join("events.sock"),
        data_dir: f.root.clone(),
    };
    // Prevent any fallback to a user's historical database/configuration.
    let connection = rusqlite::Connection::open(&paths.database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO app_meta VALUES ('legacy-import-completed', 'test');",
        )
        .unwrap();
    let database = Database::open(&paths).unwrap();
    database.upsert_task(&f.task).unwrap();
    let blocked = CloseTaskResult::blocked(f.task.id, "test blocker");
    database.save_task_closure(&blocked).unwrap();
    assert_eq!(database.all_tasks().unwrap().len(), 1);
    let result = f.close(
        &CloseOptions::default(),
        &[f.tab(SessionState::Restored)],
        &CallerContext::default(),
    );
    assert_eq!(result.status, "deleted");
    connection.execute_batch("CREATE TRIGGER prevent_delete BEFORE DELETE ON project_tasks BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(database.save_task_closure(&result).is_err());
    let key = format!("task-closure:{}", f.task.id);
    let saved: serde_json::Value =
        serde_json::from_str(&database.setting(&key).unwrap().unwrap()).unwrap();
    assert_eq!(saved["status"], "blocked");
    assert_eq!(database.all_tasks().unwrap().len(), 1);
    connection
        .execute_batch("DROP TRIGGER prevent_delete;")
        .unwrap();
    database.save_task_closure(&result).unwrap();
    assert!(database.all_tasks().unwrap().is_empty());
    let saved: serde_json::Value =
        serde_json::from_str(&database.setting(&key).unwrap().unwrap()).unwrap();
    assert_eq!(saved["status"], "deleted");
    assert_eq!(saved["sessions"].as_array().unwrap().len(), 2);
}

#[test]
fn source_symlink_into_task_root_cannot_turn_cleanup_into_source_deletion() {
    let mut f = Fixture::new();
    let moved_origin = f.task.worktree_root_path.join("source");
    fs::rename(&f.sources[0].path, &moved_origin).unwrap();
    run_git(
        &moved_origin,
        [
            "worktree",
            "repair",
            path_text(&f.task.repositories[0].worktree_path).unwrap(),
        ],
    )
    .unwrap();
    let source_link = f.root.join("linked-origin");
    std::os::unix::fs::symlink(&moved_origin, &source_link).unwrap();
    f.sources[0].path = source_link;
    let options = CloseOptions {
        extra_files: FilePolicy::Discard,
        ..Default::default()
    };
    let result = f.close(&options, &[], &CallerContext::default());
    assert_eq!(result.status, "blocked");
    assert!(result.reason.unwrap().contains("Origin must be outside"));
    assert!(moved_origin.join("tracked.txt").exists());
    assert!(f.task.repositories[0].worktree_path.exists());
}
