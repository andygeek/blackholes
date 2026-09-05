use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::webview::WebView;
use serde::Deserialize;
use serde_json::Value;
use wry::WebViewBuilder;

use crate::{
    model::{AppTheme, WorkspaceColor},
    services::orchestrator::{
        AgentAuthMode, AgentAvatarColor, AgentProvider, OrchestratorChatAttachment,
    },
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorChatCommand {
    Ready,
    RefreshModelCatalog { #[serde(default)] force: bool },
    RequestPaste,
    ChooseAttachments,
    CopyText {
        text: String,
    },
    OpenUrl {
        url: String,
    },
    OpenAgent {
        scope: String,
        project_id: Option<uuid::Uuid>,
        task_id: Option<uuid::Uuid>,
    },
    OpenTarget {
        scope: String,
        project_id: Option<uuid::Uuid>,
        task_id: Option<uuid::Uuid>,
    },
    SetAgentIdentity {
        identity: AgentAvatarColor,
    },
    SendMessage {
        id: String,
        message: String,
        created_at: String,
        #[serde(default)]
        attachments: Vec<OrchestratorChatAttachment>,
    },
    EditMessage {
        message_id: uuid::Uuid,
        id: String,
        message: String,
        created_at: String,
        #[serde(default)]
        attachments: Vec<OrchestratorChatAttachment>,
    },
    SwitchBranch {
        branch_id: uuid::Uuid,
    },
    StopAgent,
    NewChat,
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
    RevealAgentContext {
        #[serde(default)]
        project_only: bool,
    },
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
    SetAgentModel {
        model: String,
    },
    SetAgentEffort {
        effort: String,
    },
    SetAgentsFullAccess {
        enabled: bool,
    },
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
    ConfirmRemoveProject {
        workspace_id: uuid::Uuid,
    },
    ConfirmRemoveAgent {
        scope: String,
    },
    ConfirmRemoveTask {
        task_id: uuid::Uuid,
    },
    RevealProjectsRoot,
    ChooseProjectsRoot,
    InstallGitTools,
    RefreshRuntimeStatus,
    RevealAgentSkills,
    ImportAgentSkills,
    SetAgentSkillEnabled {
        name: String,
        enabled: bool,
    },
    SetAgentMcpEnabled {
        name: String,
        enabled: bool,
    },
    SetProjectAgentSkillEnabled {
        workspace_id: uuid::Uuid,
        name: String,
        enabled: bool,
    },
    SetProjectAgentMcpEnabled {
        workspace_id: uuid::Uuid,
        name: String,
        enabled: bool,
    },
    AuthenticateProjectAgentMcp {
        workspace_id: uuid::Uuid,
        name: String,
    },
    CancelProjectAgentMcpAuthentication {
        workspace_id: uuid::Uuid,
        name: String,
    },
    InstallProjectAgentMcp {
        workspace_id: uuid::Uuid,
        name: String,
        transport: String,
        url: Option<String>,
        oauth_client_id: Option<String>,
        oauth_callback_port: Option<u16>,
        command: Option<String>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    },
    RemoveProjectAgentMcp {
        workspace_id: uuid::Uuid,
        name: String,
    },
    UpdateProjectInstructions {
        workspace_id: uuid::Uuid,
        content: String,
    },
    UpdateProjectTaskInstructions {
        workspace_id: uuid::Uuid,
        content: String,
    },
    UpdateNote {
        owner: String,
        id: uuid::Uuid,
        content: String,
        blocks: Value,
    },
    ToggleNotePreview {
        owner: String,
        id: uuid::Uuid,
    },
    ReloadNote {
        owner: String,
        id: uuid::Uuid,
    },
    SetNoteAppearance {
        owner: String,
        id: uuid::Uuid,
        icon: Option<String>,
        color: Option<WorkspaceColor>,
    },
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
    let html = chat_html();
    let raw = WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        .with_accept_first_mouse(true)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |request| {
            let _ = command_sender.send(request.body().clone());
        })
        .build_as_child(window)
        .context("Unable to create the orchestrator WebView")?;
    Ok(cx.new(|cx| WebView::new(raw, window, cx)))
}

pub fn dispatch(webview: &Entity<WebView>, event: Value, cx: &mut App) -> Result<()> {
    let payload = serde_json::to_string(&event)?;
    webview.update(cx, |webview, _| {
        webview
            .raw()
            .evaluate_script(&format!("window.blackholesNative?.receive({payload});"))
            .context("Unable to dispatch an event to the orchestrator WebView")
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

fn chat_html() -> String {
    let logo = STANDARD.encode(include_bytes!("../../assets/app-logo-transparent.png"));
    include_str!("../../assets/chat/index.html")
        .replace(
            "{{AGENT_AVATAR_STYLES}}",
            include_str!("../../assets/agent-avatar.css"),
        )
        .replace(
            "{{CHAT_STYLES}}",
            include_str!("../../assets/chat/styles.css"),
        )
        .replace(
            "{{WORKSPACE_STYLES}}",
            include_str!("../../assets/chat/workspace.css"),
        )
        .replace(
            "{{CHAT_BUNDLE_STYLES}}",
            include_str!("../../assets/generated/chat.css"),
        )
        .replace(
            "{{CHAT_REACT_BUNDLE}}",
            include_str!("../../assets/generated/chat.js"),
        )
        .replace("{{EDITOR_BUNDLE_STYLES}}", include_str!("../../assets/generated/editor.css"))
        .replace("{{EDITOR_BUNDLE_BASE64}}", &STANDARD.encode(include_bytes!("../../assets/generated/editor.js")))
        .replace(
            "{{APP_LOGO_DATA_URL}}",
            &format!("data:image/png;base64,{logo}"),
        )
}
