//! Closing agent session tabs from the MCP. Tabs leave through `close_terminal_internal`,
//! the same path as the × in the project tree; running processes are stopped first with
//! the policies of Worktree Group closure.
use super::*;
use crate::services::tasks::closure::{self, CloseSessionResult, CloseSessionsRequest, MAX_CLOSE_SESSIONS};

impl BlackholesApp {
    pub(super) fn start_close_agent_sessions(
        &mut self,
        request: CloseSessionsRequest,
        reply: flume::Sender<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        request.validate()?;
        if !self.closing_terminal_ids.is_empty() {
            anyhow::bail!("Other agent sessions are closing; wait for that to finish");
        }
        let live = |terminal: &TerminalDescriptor| self.terminals.contains_key(&terminal.id)
            && !matches!(terminal.state, SessionState::Restored | SessionState::Exited);
        // Explicit IDs keep their order; a state filter follows the order of the session.
        let mut order = Vec::new();
        let mut decided = Vec::new();
        let mut selected = Vec::new();
        let mut remaining = 0;
        if request.terminal_ids.is_empty() {
            let now = Utc::now();
            let unused = |terminal: &TerminalDescriptor| request.inactive_for_days.is_none_or(|days|
                now.signed_duration_since(agent_sections::last_active(terminal)) >= chrono::Duration::days(days.into()));
            let matches: Vec<&TerminalDescriptor> = self.session.terminals.iter()
                .filter(|terminal| terminal.agent != AgentKind::Shell
                    && request.project_id.is_none_or(|project| terminal.workspace_id == project)
                    && (request.state.is_none() || !live(terminal))
                    && unused(terminal))
                .collect();
            // A filter never closes a pinned session: the user asked to keep it at hand.
            let (pinned, matches): (Vec<_>, Vec<_>) = matches.into_iter()
                .partition(|terminal| self.terminal_is_pinned(terminal.id));
            remaining = matches.len().saturating_sub(MAX_CLOSE_SESSIONS);
            for terminal in matches.into_iter().take(MAX_CLOSE_SESSIONS) {
                order.push(terminal.id);
                selected.push(self.close_terminal_snapshot(terminal));
            }
            for terminal in pinned {
                order.push(terminal.id);
                decided.push(CloseSessionResult::for_terminal(terminal,
                    "It is pinned, so filters keep it; pass its terminalId to close it"));
            }
        } else {
            for id in &request.terminal_ids {
                order.push(*id);
                match self.session.terminals.iter().find(|terminal| terminal.id == *id) {
                    Some(terminal) => selected.push(self.close_terminal_snapshot(terminal)),
                    None => decided.push(CloseSessionResult::blocked(*id,
                        "No session has this terminal ID; it may already be closed. Read the current ones with list_agent_sessions")),
                }
            }
        }
        // A Worktree Group that is closing owns its terminals until it finishes.
        selected.retain(|terminal| {
            let closing = terminal.descriptor.task_id.is_some_and(|id| self.closing_task_ids.contains(&id));
            if closing {
                decided.push(CloseSessionResult::for_terminal(&terminal.descriptor, "Its Worktree Group is closing"));
            }
            !closing
        });
        let had_process: HashSet<Uuid> = selected.iter()
            .filter(|terminal| terminal.process.is_some())
            .map(|terminal| terminal.descriptor.id)
            .collect();
        let stops_processes = !had_process.is_empty();
        if stops_processes && (self.busy.is_some() || !self.closing_task_ids.is_empty()) {
            anyhow::bail!("The app is busy; wait for the current operation to finish");
        }
        self.closing_terminal_ids.extend(selected.iter().map(|terminal| terminal.descriptor.id));
        if stops_processes {
            self.busy = Some(self.tr("Closing agent sessions…", "Cerrando sesiones de agentes…").into());
        }
        let background = cx.background_executor().spawn(async move {
            closure::close_agent_sessions(&request, &selected)
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let results = background.await;
            let response = weak.update(cx, |app, cx| {
                let mut results: Vec<_> = results.into_iter().map(|mut result| {
                    // A restored tab the user resumed meanwhile is theirs again.
                    if result.status == "closed" && !had_process.contains(&result.terminal_id)
                        && app.terminals.contains_key(&result.terminal_id)
                    {
                        result.status = "blocked";
                        result.reason = Some("The session was resumed while closing, so it was kept".into());
                    }
                    if result.status == "closed" {
                        app.close_terminal_internal(result.terminal_id, true, cx);
                    }
                    result
                }).collect();
                results.extend(decided);
                results.sort_by_key(|result| order.iter().position(|id| *id == result.terminal_id));
                app.closing_terminal_ids.clear();
                if stops_processes {
                    app.busy = None;
                }
                let persistence_error = app.database.save_session(&app.session).err().map(|error| error.to_string());
                app.hydrate_navigation(cx);
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
                let closed = results.iter().filter(|result| result.status == "closed").count();
                serde_json::json!({
                    "accepted": true,
                    "closed": closed,
                    "blocked": results.len() - closed,
                    "remaining": remaining,
                    "results": results,
                    "persistenceError": persistence_error,
                })
            }).unwrap_or_else(|_| serde_json::json!({
                "accepted": false,
                "error": "The app closed before finishing; read list_agent_sessions before retrying",
            }));
            let _ = reply.send(response);
        }).detach();
        cx.notify();
        Ok(())
    }
}
