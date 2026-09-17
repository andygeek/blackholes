use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::webview::WebView;
use serde::Deserialize;
use serde_json::Value;
use wry::WebViewBuilder;

use crate::{
    model::{AppTheme, WorkspaceColor},
    services::providers::{AgentAuthMode, AgentProvider},
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceCommand {
    Ready,
    CopyText {
        text: String,
    },
    OpenUrl {
        url: String,
    },
    SetLanguage {
        language: String,
    },
    SetTheme {
        theme: AppTheme,
    },
    CloseSettings,
    SetSidebarWidth {
        width: f32,
        #[serde(default)]
        commit: bool,
    },
    RefreshPlanUsage,
    SetAgentProvider {
        provider: AgentProvider,
    },
    SetAgentAuthMode {
        auth_mode: AgentAuthMode,
    },
    AuthenticateAgentProvider,
    SubmitAgentAuth {
        value: String,
    },
    CancelAgentAuth,
    DismissAppModal,
    CreateTaskModal {
        request_id: uuid::Uuid,
        workspace_id: uuid::Uuid,
        request: crate::services::tasks::CreateTaskRequest,
        #[serde(default)]
        check_only: bool,
    },
    ChooseProjectModalFolder {
        request_id: uuid::Uuid,
    },
    SubmitCreateProject {
        request_id: uuid::Uuid,
        name: String,
        sources: Vec<crate::services::projects::ProjectRepositorySource>,
        #[serde(default)]
        mode: crate::services::projects::ProjectRepositoryMode,
    },
    SubmitEditProject {
        request_id: uuid::Uuid,
        workspace_id: uuid::Uuid,
        name: String,
        icon: String,
        color: WorkspaceColor,
    },
    ConfirmRemoveProject {
        workspace_id: uuid::Uuid,
    },
    SubmitAddRepositories {
        request_id: uuid::Uuid,
        workspace_id: uuid::Uuid,
        sources: Vec<crate::services::projects::ProjectRepositorySource>,
        #[serde(default)]
        mode: crate::services::projects::ProjectRepositoryMode,
    },
    ConfirmRemoveRepository {
        request_id: uuid::Uuid,
    },
    ConfirmCloseTerminal {
        terminal_id: uuid::Uuid,
    },
    ConfirmRemoveTask {
        task_id: uuid::Uuid,
    },
    RevealProjectsRoot,
    ChooseProjectsRoot,
    InstallGitTools,
    RefreshRuntimeStatus,
    RefreshExternalIntegrations,
    SetProjectTerminalSkipPermissions {
        workspace_id: uuid::Uuid,
        enabled: bool,
    },
    UpdateProjectInstructions {
        workspace_id: uuid::Uuid,
        content: String,
    },
    UpdateProjectTaskInstructions {
        workspace_id: uuid::Uuid,
        content: String,
    },
    SaveTaskDetails { task_id: uuid::Uuid, request_id: String, patch: crate::services::task_details::TaskDetailsPatch },
    TaskDetailsDirty { task_id: uuid::Uuid, dirty: bool },
    AddTaskRepositories { task_id: uuid::Uuid },
    RemoveTaskRepositories { task_id: uuid::Uuid },
    FocusTerminal { terminal_id: uuid::Uuid },
    OpenTaskDetails { workspace_id: uuid::Uuid, task_id: uuid::Uuid },
    OpenProjectRepository { workspace_id: uuid::Uuid, repository_id: uuid::Uuid },
    RefreshFileExplorer,
    CloseFileExplorer,
    SetFileExplorerMode {
        mode: String,
    },
    ActivateFileRow {
        path: String,
        kind: String,
        click_count: usize,
    },
    OpenRepositoryDiff {
        relative_path: String,
    },
    CloseRepositoryDiff,
    UpdateFileContent {
        request_id: u64,
        content: String,
    },
    SaveActiveFile,
    CloseFileEditor,
    OpenProjectInstructions {
        workspace_id: uuid::Uuid,
    },
    OpenProjectTaskInstructions {
        workspace_id: uuid::Uuid,
    },
    QuickOpenPaste {
        open_id: u64,
        request_id: String,
    },
    QuickOpenQueryChanged {
        open_id: u64,
        query: String,
    },
    QuickOpenActivate {
        open_id: u64,
        result_index: usize,
    },
    QuickOpenDismiss {
        open_id: u64,
    },
    DismissStatus,
}

pub fn create(
    window: &mut Window,
    cx: &mut App,
    command_sender: flume::Sender<String>,
) -> Result<Entity<WebView>> {
    let html = workspace_html();
    let raw = WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        .with_accept_first_mouse(true)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |request| {
            let _ = command_sender.send(request.body().clone());
        })
        .build_as_child(window)
        .context("Unable to create the workspace WebView")?;
    Ok(cx.new(|cx| WebView::new(raw, window, cx)))
}

pub fn dispatch(webview: &Entity<WebView>, event: Value, cx: &mut App) -> Result<()> {
    let payload = serde_json::to_string(&event)?;
    webview.update(cx, |webview, _| {
        webview
            .raw()
            .evaluate_script(&format!("window.blackholesNative?.receive({payload});"))
            .context("Unable to dispatch an event to the workspace WebView")
    })
}

pub fn set_visible(webview: &Entity<WebView>, visible: bool, cx: &mut App) {
    webview.update(cx, |webview, _| {
        if visible && !webview.visible() {
            webview.show();
        } else if !visible && webview.visible() {
            webview.hide();
        }
    });
}

fn workspace_html() -> String {
    let logo = STANDARD.encode(include_bytes!("../../assets/app-logo-transparent.png"));
    include_str!("../../assets/workspace/index.html")
        .replace(
            "{{WORKSPACE_BASE_STYLES}}",
            include_str!("../../assets/workspace/styles.css"),
        )
        .replace(
            "{{WORKSPACE_STYLES}}",
            include_str!("../../assets/workspace/workspace.css"),
        )
        .replace(
            "{{WORKSPACE_REACT_BUNDLE}}",
            include_str!("../../assets/generated/workspace.js"),
        )
        .replace(
            "{{EDITOR_BUNDLE_STYLES}}",
            include_str!("../../assets/generated/editor.css"),
        )
        .replace(
            "{{EDITOR_BUNDLE_BASE64}}",
            &STANDARD.encode(include_bytes!("../../assets/generated/editor.js")),
        )
        .replace(
            "{{APP_LOGO_DATA_URL}}",
            &format!("data:image/png;base64,{logo}"),
        )
}
