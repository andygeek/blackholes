use super::*;
use crate::services::git_history as git;
use std::path::Path;

#[derive(Default)]
pub(super) struct HistoryState {
    pub snapshot: Option<git::Snapshot>,
    pub loading: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub request: Option<Uuid>,
    pub limit: usize,
    pub detail: Option<git::CommitDetail>,
    pub selected: Option<String>,
    pub detail_request: Option<Uuid>,
    pub detail_error: Option<String>,
    pub message: Option<String>,
    pub watcher: Option<notify::RecommendedWatcher>,
}

impl BlackholesApp {
    pub(super) fn history_root_matches(&self, root: &str) -> bool {
        self.file_explorer.open && self.file_explorer.root.as_deref() == Some(Path::new(root))
    }

    pub(super) fn history_json(&self) -> serde_json::Value {
        let h = &self.file_explorer.history;
        serde_json::json!({ "snapshot": h.snapshot, "loading": h.loading, "error": h.error,
            "selected_file": self.active_diff.as_ref().filter(|d| d.comparison.is_some()).map(|d| &d.change.relative_path),
            "selected": h.selected, "detail": h.detail, "detail_loading": h.detail_request.is_some(),
            "detail_error": h.detail_error, "busy": self.git_network_busy, "message": h.message })
    }

    pub(super) fn request_git_history(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else { return; };
        let h = &mut self.file_explorer.history;
        if more { h.limit = (h.limit.max(80) + 80).min(1000); }
        if h.loading { h.pending = true; return; }
        h.limit = h.limit.max(80);
        let limit = h.limit;
        let request = Uuid::new_v4();
        h.request = Some(request);
        h.loading = true;
        h.error = None;
        let read_root = root.clone();
        let background = cx.background_executor().spawn(async move { git::snapshot(&read_root, limit) });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                if app.file_explorer.root.as_ref() != Some(&root) || app.file_explorer.history.request != Some(request) { return; }
                let h = &mut app.file_explorer.history;
                h.loading = false;
                let again = std::mem::take(&mut h.pending);
                match result {
                    Ok(snapshot) => {
                        let dirs = snapshot.git_dirs.clone();
                        h.snapshot = Some(snapshot);
                        if h.watcher.is_none() { app.watch_git_history(root.clone(), dirs, cx); }
                    }
                    Err(error) => { h.error = Some(format!("{error:#}")); }
                }
                app.publish_workbench_surface(cx);
                cx.notify();
                if again { app.request_git_history(false, cx); }
            });
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn watch_git_history(&mut self, root: PathBuf, dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::bounded::<()>(1);
        let Ok(mut watcher) = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                if matches!(event.kind, EventKind::Access(_)) { return; }
                if event.paths.iter().any(|path| {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    !name.ends_with(".lock") && (path.components().any(|p| p.as_os_str() == "refs")
                        || matches!(name, "HEAD" | "index" | "config" | "packed-refs" | "FETCH_HEAD" | "shallow"))
                }) { let _ = sender.try_send(()); }
            }
        }) else { return; };
        for dir in dirs {
            let _ = watcher.watch(&dir, RecursiveMode::NonRecursive);
            let refs = dir.join("refs");
            if refs.is_dir() { let _ = watcher.watch(&refs, RecursiveMode::Recursive); }
        }
        self.file_explorer.history.watcher = Some(watcher);
        cx.spawn(async move |this, cx| {
            while receiver.recv_async().await.is_ok() {
                Timer::after(Duration::from_millis(400)).await;
                while receiver.try_recv().is_ok() {}
                let keep = this.update(cx, |app, cx| {
                    if app.file_explorer.root.as_ref() != Some(&root) { return false; }
                    if app.file_explorer.mode == FileExplorerMode::Changes {
                        app.request_git_history(false, cx);
                        app.request_repository_changes(cx);
                        app.refresh_active_repository_diff(cx);
                    }
                    true
                }).unwrap_or(false);
                if !keep { break; }
            }
        }).detach();
    }

    pub(super) fn select_git_commit(&mut self, commit: String, parent: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else { return; };
        if !self.file_explorer.history.snapshot.as_ref().is_some_and(|s| s.commits.iter().any(|c| c.id == commit)) { return; }
        if parent.is_none() && self.file_explorer.history.selected.as_ref() == Some(&commit) && self.active_diff.is_none() {
            let h = &mut self.file_explorer.history;
            h.selected = None;
            h.detail = None;
            h.detail_request = None;
            h.detail_error = None;
            self.publish_workbench_surface(cx);
            cx.notify();
            return;
        }
        self.flush_active_file(cx);
        self.active_file = None;
        self.active_diff = None;
        let request = Uuid::new_v4();
        let h = &mut self.file_explorer.history;
        h.selected = Some(commit.clone());
        h.detail = None;
        h.detail_error = None;
        h.detail_request = Some(request);
        let read_root = root.clone();
        let background = cx.background_executor().spawn(async move { git::detail(&read_root, &commit, parent.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                if app.file_explorer.root.as_ref() != Some(&root) || app.file_explorer.history.detail_request != Some(request) { return; }
                let h = &mut app.file_explorer.history;
                h.detail_request = None;
                match result { Ok(detail) => h.detail = Some(detail), Err(error) => h.detail_error = Some(format!("{error:#}")) }
                app.publish_workbench_surface(cx);
                cx.notify();
            });
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    pub(super) fn open_git_commit_diff(&mut self, commit: &str, path: &str, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else { return; };
        let Some(detail) = self.file_explorer.history.detail.clone().filter(|d| d.id == commit) else { return; };
        let Some(file) = detail.files.iter().find(|f| f.relative_path == path).cloned() else { return; };
        self.next_file_diff_request_id = self.next_file_diff_request_id.wrapping_add(1);
        let request_id = self.next_file_diff_request_id;
        self.active_diff = Some(FileDiffHandle { staged: false, root: root.clone(), change: file.change(&root), load_state: FileDiffLoadState::Loading,
            request_id, request_in_flight: true, refresh_pending: false,
            comparison: Some((detail.parent.as_ref().map(|p| p[..8].to_owned()).unwrap_or_else(|| "∅".into()), detail.id[..8].into())) });
        let read_root = root.clone();
        let background = cx.background_executor().spawn(async move { git::file_diff(&read_root, &detail, &file) });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| app.finish_repository_diff_request(root, request_id, result, cx));
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    pub(super) fn git_remote_operation(&mut self, operation: &str, remote: String, expected_head: Option<String>, expected_branch: Option<String>, expected_upstream: Option<String>, expected_target: Option<String>, cx: &mut Context<Self>) {
        if self.git_network_busy || !matches!(operation, "fetch" | "push") { return; }
        let Some(root) = self.file_explorer.root.clone() else { return; };
        let Some(snapshot) = self.file_explorer.history.snapshot.clone() else { return; };
        let push = operation == "push";
        let target = if snapshot.upstream.is_some() { &snapshot.push_branch } else { &snapshot.branch };
        if push && (snapshot.head != expected_head || snapshot.branch != expected_branch || snapshot.upstream != expected_upstream
            || target != &expected_target || self.file_explorer.history.error.is_some() || self.file_explorer.history.loading) {
            self.file_explorer.history.message = Some(self.tr("The branch changed. Refresh and review the push again.", "La rama cambió. Actualiza y revisa el push otra vez.").into());
            self.request_git_history(false, cx);
            return;
        }
        self.git_network_busy = true;
        self.file_explorer.history.message = None;
        self.sync_update_guard();
        let read_root = root.clone();
        let background = cx.background_executor().spawn(async move {
            if push { git::push(&read_root, &snapshot, &remote) } else { git::fetch(&read_root, &remote) }
        });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                app.git_network_busy = false;
                app.sync_update_guard();
                let message = match &result { Ok(_) => app.tr("Git operation completed", "Operación de Git completada").to_string(), Err(error) => format!("{error:#}") };
                app.status = Some((message.clone(), result.is_err()));
                if app.file_explorer.root.as_ref() == Some(&root) {
                    app.file_explorer.history.message = Some(message);
                    app.request_git_history(false, cx);
                    app.request_repository_changes(cx);
                }
                app.publish_workbench_surface(cx);
                cx.notify();
            });
        }).detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }
}
