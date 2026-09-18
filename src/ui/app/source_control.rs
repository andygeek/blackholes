use super::*;
use crate::services::repository_search::{self, Options, SearchResult};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct SearchState {
    pub options: Option<Options>, pub request: String, pub result: Option<SearchResult>,
    pub error: Option<String>, pub loading: bool, pub cancelled: Arc<AtomicBool>,
}

impl BlackholesApp {
    pub(super) fn source_control_action(&mut self, request_id: String, operation: String, token: String, path: Option<String>, message: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else { return; };
        let commit = operation == "commit";
        let reject = if self.git_network_busy { Some("Wait for the current Git operation to finish.") }
            else if !matches!(operation.as_str(), "stage" | "unstage" | "commit") { Some("Unknown Git operation.") }
            else if !commit && self.active_file.as_ref().is_some_and(|f| f.root == root && (f.dirty || f.save_state != NoteSaveState::Saved)) {
                self.flush_active_file(cx);
                Some("The file is still saving. Retry when it has finished.")
            } else { None };
        if let Some(error) = reject {
            self.dispatch_workspace_event(serde_json::json!({"type":"source_control_result", "root":root, "request_id":request_id, "operation":operation, "error":error}), cx);
            return;
        }
        self.git_network_busy = true;
        self.sync_update_guard();
        let read_root = root.clone();
        let action = operation.clone();
        let background = cx.background_executor().spawn(async move {
            if commit { scm::commit(&read_root, &token, message.as_deref().unwrap_or("")).map(Some) }
            else { scm::stage(&read_root, &token, path.as_deref(), action == "stage").map(|_| None) }
        });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                app.git_network_busy = false;
                app.sync_update_guard();
                let (id, error) = match result { Ok(id) => (id, None), Err(error) => (None, Some(format!("{error:#}"))) };
                app.dispatch_workspace_event(serde_json::json!({"type":"source_control_result", "root":root, "request_id":request_id,
                    "operation":operation, "commit":id, "error":error}), cx);
                if app.file_explorer.root.as_ref() == Some(&root) {
                    app.file_explorer.scm_info = None;
                    if app.active_diff.as_ref().is_some_and(|d| d.comparison.is_none()) { app.active_diff = None; }
                    app.request_repository_changes(cx);
                    app.request_git_history(false, cx);
                }
                app.publish_workbench_surface(cx);
                cx.notify();
            });
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    pub(super) fn search_json(&self) -> serde_json::Value {
        let s = &self.file_explorer.search;
        serde_json::json!({"options":s.options, "request_id":s.request, "result":s.result, "error":s.error, "loading":s.loading})
    }
    pub(super) fn search_repository(&mut self, request: String, options: Options, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else { return; };
        self.file_explorer.search.cancelled.store(true, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.file_explorer.search = SearchState { options: Some(options.clone()), request: request.clone(), loading: !options.query.is_empty(), cancelled: cancelled.clone(), ..Default::default() };
        if options.query.is_empty() { self.publish_workbench_surface(cx); cx.notify(); return; }
        let read_root = root.clone();
        let background = cx.background_executor().spawn(async move { repository_search::search(&read_root, &options, &cancelled) });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                if app.file_explorer.root.as_ref() != Some(&root) || app.file_explorer.search.request != request { return; }
                let s = &mut app.file_explorer.search;
                s.loading = false;
                match result { Ok(result) => s.result = Some(result), Err(error) => s.error = Some(format!("{error:#}")) }
                app.publish_workbench_surface(cx);
                cx.notify();
            });
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }
    pub(super) fn open_search_match(&mut self, request: &str, path: &str, line: usize, column: usize, cx: &mut Context<Self>) {
        let s = &self.file_explorer.search;
        if s.request != request { return; }
        let Some(found) = s.result.as_ref().and_then(|r| r.files.iter().find(|f| f.path == path))
            .and_then(|f| f.matches.iter().find(|m| m.line == line && m.column == column)).cloned() else { return; };
        let Some(root) = self.file_explorer.root.clone() else { return; };
        let path = root.join(path);
        self.open_file_in_editor(path.clone(), cx);
        self.file_explorer.reveal = Some(serde_json::json!({"id":Uuid::new_v4(), "path":path, "line":line, "column":column, "end_column":found.end_column}));
        self.publish_workbench_surface(cx);
        cx.notify();
    }
}
