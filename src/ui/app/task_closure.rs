use super::*;
use crate::services::tasks::closure::{
    ClosePreview, CloseTaskSnapshot, CloseTasksRequest, CloseTerminal, LiveTerminal,
};

impl BlackholesApp {
    pub(super) fn task_close_terminals(&self, task_id: Uuid) -> Vec<CloseTerminal> {
        self.session
            .terminals
            .iter()
            .filter(|t| t.task_id == Some(task_id))
            .map(|t| CloseTerminal {
                descriptor: t.clone(),
                process: self.terminals.get(&t.id).map(|handle| LiveTerminal {
                    child: handle.child.clone(),
                    master: handle.master.clone(),
                    process_id: handle.process_id,
                    state: handle.state.clone(),
                }),
            })
            .collect()
    }

    /// MCP and UI enter here. Only this app owns PTYs and app-session.json;
    /// MCP must never rewrite the persisted snapshot behind its back.
    pub(super) fn start_close_tasks(
        &mut self,
        request: CloseTasksRequest,
        reply: Option<flume::Sender<serde_json::Value>>,
        expected: Option<ClosePreview>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        request.validate()?;
        if self.busy.is_some()
            || !self.closing_task_ids.is_empty()
            || self.task_modal_submitting
            || self.project_modal_submitting
            || self.git_network_busy
        {
            anyhow::bail!("The app is busy; wait for the current operation to finish");
        }
        self.reload_external_data(cx);
        let snapshots = request
            .task_ids
            .iter()
            .map(|id| {
                let task = self.tasks.iter().find(|t| t.id == *id).cloned();
                let sources = task
                    .as_ref()
                    .and_then(|task| self.workspaces.iter().find(|w| w.id == task.workspace_id))
                    .map(|w| w.repositories.clone())
                    .unwrap_or_default();
                CloseTaskSnapshot {
                    task_id: *id,
                    task,
                    sources,
                    terminals: self.task_close_terminals(*id),
                }
            })
            .collect::<Vec<_>>();
        self.closing_task_ids
            .extend(request.task_ids.iter().copied());
        self.busy = Some(self.tr("Closing worktree groups…", "Cerrando grupos de worktrees…").into());
        let paths = self.paths.clone();
        let background = cx.background_executor().spawn(async move {
            let service = TaskService::new(&paths);
            service
                .close_tasks(&request, &snapshots, expected.as_ref())
                .expect("The closure request was validated before scheduling")
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let mut results = background.await;
            let response = weak.update(cx, |app, cx| {
                for result in &mut results {
                    for terminal in app.session.terminals.iter().filter(|t| t.task_id == Some(result.task_id)) {
                        for (agent, id) in [
                            (AgentKind::Claude, terminal.claude_session.as_ref().map(|s| &s.id)),
                            (AgentKind::Codex, terminal.codex_session.as_ref().map(|s| &s.id)),
                        ] {
                            if let Some(id) = id {
                                let session = crate::services::tasks::closure::SavedSession { agent, session_id: id.clone() };
                                if !result.sessions.contains(&session) { result.sessions.push(session); }
                            }
                        }
                    }
                    // Save the session IDs before removing descriptors. The
                    // result and task-record removal commit together.
                    match app.database.save_task_closure(result) {
                        Ok(()) => {
                            if result.status == "blocked" {
                                for id in &result.closed_terminals {
                                    app.close_terminal_internal(*id, false, cx);
                                }
                            }
                            if result.status == "deleted" {
                                if let Some(task) = app.tasks.iter().find(|t| t.id == result.task_id) {
                                    TaskService::new(&app.paths).revoke_closed_task_trust(task);
                                }
                                if app.active_file.as_ref().is_some_and(|doc| app.tasks.iter()
                                    .find(|t| t.id == result.task_id).is_some_and(|t| doc.root.starts_with(&t.worktree_root_path))) {
                                    app.active_file = None;
                                }
                                app.finish_removed_task(result.task_id, Ok(()), cx);
                            }
                        }
                        Err(error) => {
                            result.status = "blocked";
                            result.reason = Some(format!("Could not preserve the closure/session result; worktree group record and tabs kept: {error:#}"));
                        }
                    }
                    if result.status == "blocked" {
                        app.set_status(result.reason.clone().unwrap_or_default(), true, cx);
                    }
                }
                app.closing_task_ids.clear();
                app.busy = None;
                let persistence_error = app.database.save_session(&app.session).err().map(|e| e.to_string());
                app.hydrate_navigation(cx);
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
                serde_json::json!({ "accepted": true, "results": results, "persistenceError": persistence_error })
            }).unwrap_or_else(|_| serde_json::json!({
                "accepted": false, "error": "The app closed before persisting worktree group closure; inspect worktree groups before retrying",
            }));
            if let Some(reply) = reply { let _ = reply.send(response); }
        }).detach();
        cx.notify();
        Ok(())
    }
}

pub(super) fn confirmation_inventory(preview: &ClosePreview) -> String {
    let mut lines = Vec::new();
    for terminal in &preview.terminals {
        lines.push(format!(
            "{} ({:?}, {})",
            terminal.label,
            terminal.state,
            if terminal.live {
                "live"
            } else {
                "restored/exited"
            }
        ));
    }
    for changes in &preview.changes {
        lines.push(format!(
            "{}\n{}",
            changes.path.display(),
            changes.changes.trim_end()
        ));
    }
    for path in &preview.extra_files {
        lines.push(path.display().to_string());
    }
    lines.join("\n")
}
