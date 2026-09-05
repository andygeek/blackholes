use super::{
    apply_native_theme,
    navigation_webview::{self, NavigationCommand},
    orchestrator_chat::{self, OrchestratorChatCommand},
    terminal::{AgentTerminalSignal, AgentTerminalSignalKind, FastTerminalView},
};
use crate::{
    assets::AppIcon,
    model::{
        AgentKind, AppSession, AppTheme, ClaudeSession, CodexSession, DockNode, DockTab, Language,
        ProjectTask, SessionState, TaskSession, TerminalDescriptor, Workspace, WorkspaceColor,
        WorkspaceLayout, dock_key,
    },
    paths::AppPaths,
    services::{
        claude::{ClaudeSessionBridgePayload, install_claude_session_hooks},
        codex::{CodexSessionBridgePayload, install_codex_session_hooks},
        database::Database,
        files::{
            FileEntry, FileEntryKind, IndexedRepositoryFile, RepositoryChange,
            RepositoryChangeKind, RepositoryDiffLineKind, RepositoryDiffRow, RepositoryFileDiff,
            index_repository_files, read_directory, read_text_file, repository_changes,
            repository_file_diff, write_text_file,
        },
        mcps::{AgentMcpServer, AgentMcpServerConfig, AgentMcpService},
        notes::{
            ProjectInstructionsService, ProjectNoteService, ProjectTaskInstructionsService,
            RichNoteDocument, TaskNoteService,
        },
        orchestrator::{
            AgentAuthEvent, AgentAuthMode, AgentAvatarColor, AgentHistoryMessage, AgentProvider,
            AgentRuntimeControl, AgentRuntimeEvent, AgentModelCatalog, AgentModelInfo, ClaudePlanUsage, ClaudeRateLimitWindow,
            OrchestratorChatActivity, OrchestratorChatAttachment, OrchestratorChatHandoff,
            OrchestratorChatMessage, OrchestratorChatRole, OrchestratorChatScope,
            OrchestratorChatStore, OrchestratorScopeContext, authenticate_agent_mcp,
            refresh_agent_models, refresh_agent_plan_usage, start_agent_authentication, stream_agent_turn,
        },
        projects::{
            ProjectService, RepositoryGitSummary, discover_repositories, repository_git_summary,
        },
        skills::{AgentSkill, AgentSkillService, BLACKHOLES_SKILLS_PLUGIN_NAME},
        tasks::{
            AddTaskRepositoriesRequest, BranchAvailability, CreateTaskRequest,
            ExistingBranchAction, RemoveTaskRepositoriesRequest, RemovedTaskRepository,
            RepositoryPreparation, TaskBranchSource, TaskService,
        },
        terminal::{SharedChild, SharedMasterPty, TerminalService},
    },
};
use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Corner, Edges, Entity, IntoElement, KeyBinding,
    KeyDownEvent, ListSizingBehavior, ParentElement, Render, ScrollHandle, SharedString, Timer,
    WeakEntity, Window, div, img, prelude::*, px, relative, rgb, rgba, uniform_list,
};
use gpui_component::{
    Icon, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState, TabSize},
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    resizable::ResizableState,
    scroll::ScrollableElement as _,
    skeleton::Skeleton,
    text::TextView,
    v_flex,
};
use gpui_terminal::{ColorPalette, TerminalConfig};
use notify::{EventKind, RecursiveMode, Watcher as _, event::ModifyKind};
use parking_lot::Mutex;
use portable_pty::PtySize;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    hash::{Hash, Hasher},
    os::unix::{fs::PermissionsExt as _, net::UnixDatagram},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const MAX_ORCHESTRATOR_IMAGES: usize = 4;
const MAX_ORCHESTRATOR_IMAGE_BYTES: usize = 5 * 1024 * 1024;

fn supported_orchestrator_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

fn orchestrator_image_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn validated_orchestrator_attachments(
    attachments: Vec<OrchestratorChatAttachment>,
) -> Vec<OrchestratorChatAttachment> {
    attachments
        .into_iter()
        .take(MAX_ORCHESTRATOR_IMAGES)
        .filter(|attachment| supported_orchestrator_image(&attachment.media_type))
        .filter(|attachment| {
            attachment.data.len() <= MAX_ORCHESTRATOR_IMAGE_BYTES * 4 / 3 + 4
                && STANDARD.decode(&attachment.data).is_ok_and(|bytes| {
                    !bytes.is_empty() && bytes.len() <= MAX_ORCHESTRATOR_IMAGE_BYTES
                })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn supported_system_clipboard_image() -> Option<(String, Vec<u8>)> {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    let pasteboard = NSPasteboard::generalPasteboard();
    for (uti, media_type) in [
        ("public.png", "image/png"),
        ("public.jpeg", "image/jpeg"),
        ("com.compuserve.gif", "image/gif"),
        ("org.webmproject.webp", "image/webp"),
    ] {
        let pasteboard_type = NSString::from_str(uti);
        if let Some(data) = pasteboard.dataForType(&pasteboard_type) {
            return Some((media_type.to_string(), data.to_vec()));
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn supported_system_clipboard_image() -> Option<(String, Vec<u8>)> {
    None
}

const SIDEBAR_MIN: f32 = 220.0;
const SIDEBAR_MAX: f32 = 420.0;
const FILE_EXPLORER_MIN: f32 = 220.0;
const FILE_EXPLORER_MAX: f32 = 520.0;
const FILE_EXPLORER_DEFAULT: f32 = 300.0;
const QUICK_OPEN_RESULT_LIMIT: usize = 14;
const CODE_FONT_FAMILY: &str = "Menlo";
const CODE_FONT_SIZE: f32 = 13.0;
const FILE_EDITOR_TAB_SIZE: usize = 4;
const FILE_WATCH_BATCH_PATH_LIMIT: usize = 2_048;
const APP_NAME: &str = "BLACKHOLES";
const APP_NAME_FONT_FAMILY: &str = "Geist Mono";
const APP_NAME_FONT_SIZE: f32 = 28.0;
const SIDEBAR_APP_NAME_FONT_SIZE: f32 = 16.0;
const APP_NAME_FONT_WEIGHT: f32 = 650.0;
/// Extra space between letters, as a fraction of the font size.
const APP_NAME_LETTER_SPACING_RATIO: f32 = 0.12;

gpui::actions!(blackholes, [OpenNavigationPalette, OpenFilePalette]);

pub struct TerminalHandle {
    pub view: Entity<FastTerminalView>,
    pub master: SharedMasterPty,
    pub child: SharedChild,
    pub process_id: Option<u32>,
}

#[derive(Clone)]
struct AppToast {
    target: AppToastTarget,
    title: String,
    message: String,
}

#[derive(Default)]
struct OrchestratorTurnCancellation {
    sender: Option<flume::Sender<()>>,
    control: Option<flume::Sender<AgentRuntimeControl>>,
}

impl OrchestratorTurnCancellation {
    fn set(&mut self, sender: flume::Sender<()>, control: flume::Sender<AgentRuntimeControl>) {
        self.sender = Some(sender);
        self.control = Some(control);
    }

    fn steer(
        &self,
        prompt_id: Uuid,
        message: String,
        images: Vec<OrchestratorChatAttachment>,
    ) -> bool {
        self.control.as_ref().is_some_and(|sender| {
            sender
                .send(AgentRuntimeControl::Steer {
                    prompt_id,
                    message,
                    images,
                })
                .is_ok()
        })
    }

    fn cancel(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.send(AgentRuntimeControl::Interrupt);
        }
        if let Some(sender) = self.sender.take() {
            let _ = std::thread::Builder::new()
                .name("blackholes-agent-stop-fallback".into())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(1_800));
                    let _ = sender.send(());
                });
        }
    }

    fn disarm(&mut self) {
        self.sender.take();
        self.control.take();
    }
}

impl Drop for OrchestratorTurnCancellation {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default)]
struct OrchestratorTurn {
    started_at: Option<chrono::DateTime<Utc>>,
    runtime_id: Uuid,
    user_message_id: Uuid,
    response_id: Uuid,
    response_text: String,
    activities: Vec<OrchestratorChatActivity>,
    handoffs: Vec<OrchestratorChatHandoff>,
    cancel: OrchestratorTurnCancellation,
    delegated: bool,
    notification_sent: bool,
}

impl OrchestratorTurn {
    fn duration_ms(&self) -> Option<u64> {
        self.started_at.map(|start| (Utc::now() - start).num_milliseconds().max(0) as u64)
    }
}

struct PendingOrchestratorTurn {
    client_id: String,
    message: String,
    created_at: String,
    attachments: Vec<OrchestratorChatAttachment>,
    delegated: bool,
    user_message_persisted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentAuthStatus {
    Connecting,
    NeedsInput,
    Connected,
    Error,
}

struct AgentAuthentication {
    id: Uuid,
    provider: AgentProvider,
    status: AgentAuthStatus,
    detail: String,
    output: String,
    opened_url: Option<String>,
    input: Entity<InputState>,
    input_sender: flume::Sender<String>,
    cancel: Option<flume::Sender<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectMcpAuthStatus {
    Connecting,
    Connected,
    Error,
}

#[derive(Debug)]
struct ProjectMcpAuthentication {
    attempt_id: Uuid,
    status: ProjectMcpAuthStatus,
    detail: String,
    cancel: Option<flume::Sender<()>>,
}

impl Drop for ProjectMcpAuthentication {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

impl Drop for AgentAuthentication {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

#[derive(Default)]
struct OrchestratorTurnStart {
    revision_group_id: Option<Uuid>,
    source_session_id: Option<String>,
    fork_at_user_turn: Option<usize>,
    user_message_persisted: bool,
}

/// Payload of the `task-ready:` bridge message sent by the MCP server's
/// `notify_task_ready` tool.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskReadyPayload {
    task_id: Uuid,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentHandoffPayload {
    scope: String,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    source_scope: Option<String>,
    source_global_agent_id: Option<Uuid>,
    source_project_id: Option<Uuid>,
    source_task_id: Option<Uuid>,
    source_agent_id: Option<Uuid>,
    prompt: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NavigationLinkPayload {
    scope: String,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    source_scope: Option<String>,
    source_global_agent_id: Option<Uuid>,
    source_project_id: Option<Uuid>,
    source_task_id: Option<Uuid>,
    source_agent_id: Option<Uuid>,
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppToastTarget {
    Terminal { terminal_id: Uuid, agent: AgentKind },
    Task { task_id: Uuid },
    Agent { scope: OrchestratorChatScope },
}

impl AppToastTarget {
    fn element_key(self) -> String {
        match self {
            Self::Terminal { terminal_id, .. } => format!("terminal-{terminal_id}"),
            Self::Task { task_id } => format!("task-{task_id}"),
            Self::Agent { scope } => match scope {
                OrchestratorChatScope::Global => "agent-global".into(),
                OrchestratorChatScope::GlobalAgent(agent_id) => {
                    format!("agent-global-{agent_id}")
                }
                OrchestratorChatScope::Project(workspace_id) => {
                    format!("agent-project-{workspace_id}")
                }
                OrchestratorChatScope::ProjectAgent {
                    project_id,
                    agent_id,
                } => format!("agent-project-{project_id}-{agent_id}"),
                OrchestratorChatScope::Task(task_id) => format!("agent-task-{task_id}"),
                OrchestratorChatScope::TaskAgent { task_id, agent_id } => {
                    format!("agent-task-{task_id}-{agent_id}")
                }
            },
        }
    }

    fn terminal_id(self) -> Option<Uuid> {
        match self {
            Self::Terminal { terminal_id, .. } => Some(terminal_id),
            Self::Task { .. } | Self::Agent { .. } => None,
        }
    }

    fn task_id(self) -> Option<Uuid> {
        match self {
            Self::Task { task_id } => Some(task_id),
            Self::Agent {
                scope: OrchestratorChatScope::Task(task_id),
            } => Some(task_id),
            Self::Agent {
                scope: OrchestratorChatScope::TaskAgent { task_id, .. },
            } => Some(task_id),
            Self::Terminal { .. } | Self::Agent { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RepositoryDraftOptions {
    copy_local_changes: bool,
    copy_environment_files: bool,
}

#[derive(Default)]
struct TaskDraftOptions {
    selected_repositories: HashSet<Uuid>,
    branch_source: TaskBranchSource,
    create_missing_branch: bool,
    replace_divergent_local_branches: bool,
    existing_branch_action: ExistingBranchAction,
    repository_options: HashMap<Uuid, RepositoryDraftOptions>,
    availability: Option<Vec<BranchAvailability>>,
}

#[derive(Default)]
struct DetachRepositoriesDraft {
    selected_repositories: HashSet<Uuid>,
    delete_branch: bool,
    discard_uncommitted_changes: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NoteSaveState {
    #[default]
    Saved,
    Saving,
    Error,
}

struct NoteHandle {
    document_id: Uuid,
    editor: Entity<InputState>,
    blocks: Option<serde_json::Value>,
    preview: bool,
    revision: u64,
    save_state: NoteSaveState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickOpenMode {
    Navigation,
    Files,
}

#[derive(Clone)]
enum QuickOpenTarget {
    Project {
        workspace_id: Uuid,
    },
    Task {
        workspace_id: Uuid,
        task_id: Uuid,
    },
    File {
        root: PathBuf,
        root_label: String,
        path: PathBuf,
    },
}

#[derive(Clone)]
struct QuickOpenItem {
    title: String,
    subtitle: String,
    search_key: String,
    kind_label: String,
    icon: AppIcon,
    color: gpui::Rgba,
    color_css: String,
    target: QuickOpenTarget,
}

enum QuickOpenEntries {
    Loading,
    Ready(Vec<QuickOpenItem>),
    Error(String),
}

struct QuickOpenState {
    id: u64,
    mode: QuickOpenMode,
    placeholder: String,
    query: Entity<InputState>,
    entries: QuickOpenEntries,
    selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteOwner {
    Project(Uuid),
    Task(Uuid),
}

enum NoteSaveTarget {
    Project(Workspace),
    Task(ProjectTask),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProjectDraftMode {
    #[default]
    Empty,
    Existing,
    Github,
}

#[derive(Default)]
struct ProjectDraftOptions {
    mode: ProjectDraftMode,
    existing_path: Option<PathBuf>,
}

struct ProjectAppearanceEditor {
    name: Entity<InputState>,
    icon: String,
    color: WorkspaceColor,
    language: Language,
}

enum DirectoryListing {
    Loading,
    Loaded(Vec<FileEntry>),
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FileExplorerMode {
    #[default]
    Files,
    Changes,
}

#[derive(Default)]
enum RepositoryChangesState {
    #[default]
    Idle,
    Loading,
    Ready(Arc<[RepositoryChange]>),
    Error(String),
}

#[derive(Default)]
struct FileExplorerState {
    open: bool,
    root: Option<PathBuf>,
    root_label: String,
    mode: FileExplorerMode,
    expanded: HashSet<PathBuf>,
    directories: HashMap<PathBuf, DirectoryListing>,
    requests: HashMap<PathBuf, u64>,
    selected: Option<PathBuf>,
    next_request_id: u64,
    changes: RepositoryChangesState,
    changes_request_id: u64,
    changes_request_in_flight: bool,
    changes_refresh_pending: bool,
}

#[derive(Clone)]
enum FileTreeRowKind {
    Entry(FileEntryKind),
    Loading,
    Error,
}

#[derive(Clone)]
struct FileTreeRow {
    path: PathBuf,
    label: String,
    depth: usize,
    hidden: bool,
    expanded: bool,
    kind: FileTreeRowKind,
}

enum FileDocumentLoadState {
    Loading,
    Ready(String),
    Open,
    Error(String),
}

struct FileDocumentHandle {
    root: PathBuf,
    path: PathBuf,
    source: FileDocumentSource,
    language: SharedString,
    editor: Option<Entity<InputState>>,
    load_state: FileDocumentLoadState,
    revision: u64,
    dirty: bool,
    save_state: NoteSaveState,
    request_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileDocumentSource {
    Repository,
    ProjectInstructions(Uuid),
    ProjectTaskInstructions(Uuid),
}

enum FileSaveOperation {
    Repository {
        root: PathBuf,
        path: PathBuf,
        content: String,
    },
    ProjectInstructions {
        workspace: Workspace,
        content: String,
    },
    ProjectTaskInstructions {
        paths: AppPaths,
        workspace: Workspace,
        tasks: Vec<ProjectTask>,
        content: String,
    },
}

impl FileSaveOperation {
    fn execute(self) -> Result<()> {
        match self {
            Self::Repository {
                root,
                path,
                content,
            } => write_text_file(&root, &path, &content),
            Self::ProjectInstructions { workspace, content } => {
                ProjectInstructionsService::write(&workspace, &content)
            }
            Self::ProjectTaskInstructions {
                paths,
                workspace,
                tasks,
                content,
            } => {
                ProjectTaskInstructionsService::write(&workspace, &content)?;
                let task_service = TaskService::new(&paths);
                for task in tasks {
                    task_service
                        .repair_task_files(&workspace, &task)
                        .with_context(|| {
                            format!(
                                "Could not copy the shared task instructions to task '{}'",
                                task.title
                            )
                        })?;
                }
                Ok(())
            }
        }
    }
}

enum FileDiffLoadState {
    Loading,
    Ready(RepositoryFileDiff),
    Error(String),
}

struct FileDiffHandle {
    root: PathBuf,
    change: RepositoryChange,
    load_state: FileDiffLoadState,
    request_id: u64,
    request_in_flight: bool,
    refresh_pending: bool,
}

impl Render for ProjectAppearanceEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let selected_icon = project_icon_kind(&self.icon);
        let selected_icon_label = project_icon_options(self.language)
            .into_iter()
            .find(|(value, _, _)| *value == self.icon)
            .map(|(_, label, _)| label)
            .unwrap_or("Layers");
        let icon_options = project_icon_options(self.language);
        let weak_icon = weak.clone();
        let selected_icon_value = self.icon.clone();
        let accent = workspace_color(self.color);

        let icon_trigger = Button::new("edit-project-icon-picker")
            .w_full()
            .label(selected_icon_label)
            .icon(selected_icon)
            .dropdown_caret(true)
            .outline();

        let icon_picker = Popover::new("edit-project-icon-popover")
            .anchor(Corner::TopLeft)
            .trigger(icon_trigger)
            .content(move |_, _, _| {
                let mut icons = h_flex().w(px(242.)).gap_2().flex_wrap();
                for (value, _, icon) in icon_options.iter() {
                    let selected = *value == selected_icon_value;
                    let value = (*value).to_string();
                    let weak = weak_icon.clone();
                    icons = icons.child(
                        div()
                            .id(SharedString::from(format!(
                                "edit-project-icon-option-{value}"
                            )))
                            .size(px(42.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(9.))
                            .border_1()
                            .border_color(if selected { accent } else { rgb(0x2a2e36) })
                            .bg(if selected {
                                with_alpha(accent, 0.18)
                            } else {
                                rgb(0x15181e)
                            })
                            .text_color(if selected { accent } else { rgb(0x8e97aa) })
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242a35)).text_color(rgb(0xe5e9f0)))
                            .on_click(move |_, _, cx| {
                                let value = value.clone();
                                let _ = weak.update(cx, |editor, cx| {
                                    editor.icon = value;
                                    cx.notify();
                                });
                            })
                            .child(Icon::new(*icon).with_size(px(20.))),
                    );
                }

                icons
            });

        let mut colors = h_flex().w_full().gap_1().flex_wrap();
        for color in project_colors() {
            let weak = weak.clone();
            colors = colors.child(project_color_button(
                format!("edit-project-color-{color:?}"),
                color,
                color == self.color,
                move |_, _, cx| {
                    let _ = weak.update(cx, |editor, cx| {
                        editor.color = color;
                        cx.notify();
                    });
                },
            ));
        }

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xb6bdca))
                            .child(match self.language {
                                Language::English => "Visible name",
                                Language::Spanish => "Nombre visible",
                            }),
                    )
                    .child(Input::new(&self.name)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xb6bdca))
                            .child(match self.language {
                                Language::English => "Project icon",
                                Language::Spanish => "Icono del proyecto",
                            }),
                    )
                    .child(icon_picker),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xb6bdca))
                            .child(match self.language {
                                Language::English => "Project color",
                                Language::Spanish => "Color del proyecto",
                            }),
                    )
                    .child(colors),
            )
    }
}

pub struct BlackholesApp {
    update_state: crate::services::updater::UpdateState,
    paths: AppPaths,
    database: Database,
    app_logo: Arc<gpui::RenderImage>,
    navigation_webview: Option<Entity<gpui_component::webview::WebView>>,
    orchestrator_webview: Option<Entity<gpui_component::webview::WebView>>,
    project_modal_request: Option<Uuid>,
    project_modal_submitting: bool,
    orchestrator_chats: OrchestratorChatStore,
    active_orchestrator_scope: OrchestratorChatScope,
    orchestrator_turns: HashMap<OrchestratorChatScope, OrchestratorTurn>,
    pending_orchestrator_turns: HashMap<OrchestratorChatScope, VecDeque<PendingOrchestratorTurn>>,
    arriving_orchestrator_agents: HashSet<OrchestratorChatScope>,
    agent_authentication: Option<AgentAuthentication>,
    project_mcp_authentications: HashMap<String, ProjectMcpAuthentication>,
    workspaces: Vec<Workspace>,
    tasks: Vec<ProjectTask>,
    session: AppSession,
    terminals: HashMap<Uuid, TerminalHandle>,
    task_notes: HashMap<Uuid, NoteHandle>,
    project_notes: HashMap<Uuid, NoteHandle>,
    file_explorer: FileExplorerState,
    sidebar_scroll: ScrollHandle,
    file_explorer_resize: Entity<ResizableState>,
    file_watcher: Option<notify::RecommendedWatcher>,
    active_file: Option<FileDocumentHandle>,
    active_diff: Option<FileDiffHandle>,
    next_file_request_id: u64,
    next_file_diff_request_id: u64,
    repository_git_summaries: HashMap<PathBuf, RepositoryGitSummary>,
    repository_git_requests: HashSet<PathBuf>,
    repository_git_refresh_pending: HashSet<PathBuf>,
    quick_open: Option<QuickOpenState>,
    next_quick_open_id: u64,
    repository_git_save_requests: HashMap<PathBuf, u64>,
    next_repository_git_save_request_id: u64,
    show_terminal: bool,
    show_task_note: bool,
    show_project_note: bool,
    show_settings: bool,
    settings_return_view: Option<(bool, bool, bool, Option<Uuid>)>,
    plan_usage_refreshing: bool,
    plan_usage_refresh_error: bool,
    active_plan_usage: Option<ClaudePlanUsage>,
    plan_usage_updated_at: Option<chrono::DateTime<Utc>>,
    plan_usage_generation: u64,
    model_catalog: Option<AgentModelCatalog>,
    model_catalog_key: String,
    model_catalog_generation: u64,
    model_catalog_loading: bool,
    model_catalog_error: bool,
    model_catalog_checked: Option<std::time::Instant>,
    model_catalog_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    project_settings_workspace_id: Option<Uuid>,
    app_toasts: Vec<AppToast>,
    status: Option<(String, bool)>,
    status_revision: u64,
    busy: Option<String>,
}

impl BlackholesApp {
    pub fn init(cx: &mut App) {
        FastTerminalView::init(cx);
        cx.bind_keys([
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-o", OpenNavigationPalette, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-o", OpenNavigationPalette, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-p", OpenFilePalette, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-p", OpenFilePalette, None),
        ]);
    }

    pub fn register_global_actions(view: &Entity<Self>, cx: &mut App) {
        let navigation_view = view.downgrade();
        cx.on_action(move |_: &OpenNavigationPalette, cx| {
            let Some(window_handle) = cx.active_window() else {
                return;
            };
            let navigation_view = navigation_view.clone();
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ =
                    navigation_view.update(cx, |app, cx| app.open_navigation_palette(window, cx));
            });
        });

        let file_view = view.downgrade();
        cx.on_action(move |_: &OpenFilePalette, cx| {
            let Some(window_handle) = cx.active_window() else {
                return;
            };
            let file_view = file_view.clone();
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = file_view.update(cx, |app, cx| app.open_file_palette(window, cx));
            });
        });

        // Terminals and editors own independent focus trees. This lightweight event listener
        // makes the application shortcuts independent from whichever child currently has focus.
        let shortcut_view = view.downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let key = event.keystroke.key.to_ascii_lowercase();
            let modifiers = event.keystroke.modifiers;
            #[cfg(target_os = "macos")]
            let application_modifier = modifiers.platform
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.shift
                && !modifiers.function;
            #[cfg(not(target_os = "macos"))]
            let application_modifier = modifiers.control
                && !modifiers.platform
                && !modifiers.alt
                && !modifiers.shift
                && !modifiers.function;

            if application_modifier && matches!(key.as_str(), "o" | "p") {
                cx.stop_propagation();
                let _ = shortcut_view.update(cx, |app, cx| match key.as_str() {
                    "o" => app.open_navigation_palette(window, cx),
                    "p" => app.open_file_palette(window, cx),
                    _ => {}
                });
                return;
            }
        })
        .detach();
    }

    pub fn new(
        paths: AppPaths,
        database: Database,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut status = None;
        let workspaces = database.workspaces().unwrap_or_else(|error| {
            status = Some((format!("Could not load projects: {error:#}"), true));
            Vec::new()
        });
        let tasks = database.all_tasks().unwrap_or_else(|error| {
            status = Some((format!("Could not load tasks: {error:#}"), true));
            Vec::new()
        });
        let mut session = database.load_session();
        session
            .unseen_task_ids
            .retain(|task_id| tasks.iter().any(|task| task.id == *task_id));

        for terminal in &mut session.terminals {
            terminal.state = SessionState::Restored;
            let recorded_session_agent = match (
                terminal.codex_session.is_some(),
                terminal.claude_session.is_some(),
            ) {
                (true, false) => Some(AgentKind::Codex),
                (false, true) => Some(AgentKind::Claude),
                _ => None,
            };
            if terminal.agent != AgentKind::Shell
                && let Some(agent) = recorded_session_agent
            {
                // Repair agent identity persisted by older builds that trusted conflicting
                // third-party Warp plugin events over Blackholes' own session hook.
                terminal.agent = agent;
            } else if let Some(agent) = agent_from_terminal_title(&terminal.label) {
                terminal.agent = agent;
            }
            if terminal.agent == AgentKind::Shell {
                terminal.codex_session = None;
                terminal.claude_session = None;
            }
        }

        if session
            .selected_workspace_id
            .is_none_or(|id| !workspaces.iter().any(|workspace| workspace.id == id))
        {
            session.selected_workspace_id = workspaces.first().map(|workspace| workspace.id);
            session.selected_task_id = None;
            session.selected_repository_id = None;
        }
        // Navigation always starts collapsed. Repository Git summaries are loaded lazily when the
        // user expands a project or task, so startup cost does not grow with the total repo count.
        session.expanded_workspace_ids.clear();
        session.expanded_task_ids.clear();
        session.navigation_expansion_initialized = true;

        for workspace in &workspaces {
            if let Err(error) = ProjectNoteService::ensure(workspace, "") {
                status = Some((
                    format!(
                        "Could not prepare project notes for '{}': {error:#}",
                        workspace.label()
                    ),
                    true,
                ));
            }
        }

        let task_service = TaskService::new(&paths);
        for task in &tasks {
            let Some(workspace) = workspaces
                .iter()
                .find(|workspace| workspace.id == task.workspace_id)
            else {
                continue;
            };
            if task.worktree_root_path.starts_with(&paths.task_workspaces)
                && task.worktree_root_path.is_dir()
                && let Err(error) = task_service.repair_task_files(workspace, task)
            {
                status = Some((
                    format!(
                        "Could not refresh the AI context for '{}': {error:#}",
                        task.title
                    ),
                    true,
                ));
            }
        }

        if let Err(error) = install_event_bridge(&paths, cx) {
            status = Some((format!("Local AI bridge is unavailable: {error:#}"), true));
        }
        if let Err(error) = install_agent_command_bridge(&paths, cx) {
            status = Some((format!("Agent command bridge is unavailable: {error:#}"), true));
        }
        if let Err(error) = install_codex_session_hooks() {
            status = Some((
                format!("Codex session resume is unavailable: {error:#}"),
                true,
            ));
        }
        if let Err(error) = install_claude_session_hooks() {
            status = Some((
                format!("Claude session resume is unavailable: {error:#}"),
                true,
            ));
        }

        let show_task_note = false;
        let file_explorer_resize = cx.new(|_| ResizableState::default());
        let app_logo = gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../assets/app-logo-transparent.png").to_vec(),
        )
        .to_image_data(cx.svg_renderer())
        .expect("the embedded application logo must be a valid PNG");
        let orchestrator_chats = OrchestratorChatStore::load(&paths.orchestrator_chat)
            .unwrap_or_else(|error| {
                status = Some((
                    format!("Could not restore the agent chats: {error:#}"),
                    true,
                ));
                OrchestratorChatStore::default()
            });
        let (orchestrator_command_sender, orchestrator_command_receiver) = flume::unbounded();
        let orchestrator_webview =
            match orchestrator_chat::create(window, cx, orchestrator_command_sender) {
                Ok(webview) => Some(webview),
                Err(error) => {
                    status = Some((
                        format!("The orchestrator chat is unavailable: {error:#}"),
                        true,
                    ));
                    None
                }
            };
        let orchestrator_window = window.window_handle();
        let (navigation_command_sender, navigation_command_receiver) = flume::unbounded();
        let navigation_webview =
            match navigation_webview::create(window, cx, navigation_command_sender) {
                Ok(webview) => Some(webview),
                Err(error) => {
                    status = Some((
                        format!("The WebView navigation is unavailable: {error:#}"),
                        true,
                    ));
                    None
                }
            };
        let navigation_window = window.window_handle();
        let mut app = Self {
            update_state: crate::services::updater::state(),
            paths,
            database,
            app_logo,
            navigation_webview,
            orchestrator_webview,
            project_modal_request: None,
            project_modal_submitting: false,
            orchestrator_chats,
            active_orchestrator_scope: OrchestratorChatScope::Global,
            orchestrator_turns: HashMap::new(),
            pending_orchestrator_turns: HashMap::new(),
            arriving_orchestrator_agents: HashSet::new(),
            agent_authentication: None,
            project_mcp_authentications: HashMap::new(),
            workspaces,
            tasks,
            session,
            terminals: HashMap::new(),
            task_notes: HashMap::new(),
            project_notes: HashMap::new(),
            file_explorer: FileExplorerState::default(),
            sidebar_scroll: ScrollHandle::default(),
            file_explorer_resize,
            file_watcher: None,
            active_file: None,
            active_diff: None,
            next_file_request_id: 0,
            next_file_diff_request_id: 0,
            repository_git_summaries: HashMap::new(),
            repository_git_requests: HashSet::new(),
            repository_git_refresh_pending: HashSet::new(),
            quick_open: None,
            next_quick_open_id: 0,
            repository_git_save_requests: HashMap::new(),
            next_repository_git_save_request_id: 0,
            show_terminal: false,
            show_task_note,
            show_project_note: false,
            show_settings: !crate::services::projects::git_tools_available(),
            settings_return_view: None,
            plan_usage_refreshing: false,
            plan_usage_refresh_error: false,
            active_plan_usage: None,
            plan_usage_updated_at: None,
            plan_usage_generation: 0,
            model_catalog: None,
            model_catalog_key: String::new(),
            model_catalog_generation: 0,
            model_catalog_loading: false,
            model_catalog_error: false,
            model_catalog_checked: None,
            model_catalog_cancel: None,
            project_settings_workspace_id: None,
            app_toasts: Vec::new(),
            status,
            status_revision: 0,
            busy: None,
        };
        cx.spawn(async move |this, cx| {
            while let Ok(raw_command) = orchestrator_command_receiver.recv_async().await {
                if orchestrator_window
                    .update(cx, |_, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.handle_orchestrator_chat_command(&raw_command, window, cx)
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(raw_command) = navigation_command_receiver.recv_async().await {
                let command = match serde_json::from_str::<NavigationCommand>(&raw_command) {
                    Ok(command) => command,
                    Err(error) => {
                        tracing::warn!(?error, "ignored malformed navigation command");
                        continue;
                    }
                };
                if navigation_window
                    .update(cx, |_, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.handle_navigation_command(command, window, cx)
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        if !app.has_navigation_agents() {
            app.schedule_default_global_agent(cx);
        }
        app.refresh_model_catalog(false, cx);
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(2)).await;
                if this.update(cx, |app, cx| {
                    app.sync_update_guard();
                    let next = crate::services::updater::state();
                    if next != app.update_state {
                        app.update_state = next;
                        cx.notify();
                    }
                }).is_err() { break; }
            }
        }).detach();
        app
    }

    fn sync_update_guard(&self) {
        let blocked = self.busy.is_some()
            || !self.orchestrator_turns.is_empty()
            || self.pending_orchestrator_turns.values().any(|queue| !queue.is_empty())
            || !self.terminals.is_empty()
            || self.agent_authentication.is_some()
            || self.active_file.as_ref().is_some_and(|file| file.dirty || file.save_state != NoteSaveState::Saved)
            || self.project_notes.values().chain(self.task_notes.values()).any(|note| note.save_state != NoteSaveState::Saved);
        crate::services::updater::set_blocked(blocked, self.session.language == Language::Spanish);
    }

    fn check_app_update(&mut self, cx: &mut Context<Self>) {
        self.sync_update_guard();
        if !self.update_state.enabled {
            self.set_status(self.tr(
                "Updates are available in the packaged release app. See GitHub Releases for signed downloads.",
                "Las actualizaciones funcionan en la app empaquetada. Consulta GitHub Releases para las descargas firmadas.",
            ), true, cx);
            return;
        }
        if let Err(error) = self.database.save_session(&self.session)
            .and_then(|_| self.orchestrator_chats.save(&self.paths.orchestrator_chat)) {
            self.set_status(format!("Could not save the session before updating: {error:#}"), true, cx);
            return;
        }
        // Sparkle runs modal AppKit UI. Defer it until this GPUI entity borrow
        // is released so native event-loop reentrancy cannot reborrow the app.
        cx.spawn(async move |_, _| {
            Timer::after(Duration::from_millis(1)).await;
            crate::services::updater::check();
        }).detach();
    }

    pub fn wrap_root(view: Entity<Self>, window: &mut Window, cx: &mut Context<Root>) -> Root {
        Root::new(view, window, cx)
    }

    fn selected_workspace(&self) -> Option<&Workspace> {
        let id = self.session.selected_workspace_id?;
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    fn handle_orchestrator_chat_command(
        &mut self,
        raw_command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = match serde_json::from_str::<OrchestratorChatCommand>(raw_command) {
            Ok(command) => command,
            Err(error) => {
                tracing::warn!(?error, "ignored malformed orchestrator chat command");
                return;
            }
        };
        match command {
            OrchestratorChatCommand::Ready => {
                self.refresh_model_catalog(false, cx);
                self.hydrate_orchestrator_chat(cx);
                self.hydrate_active_workspace_surface(cx);
                self.hydrate_workspace_status(cx);
            }
            OrchestratorChatCommand::RequestPaste => {
                let mut clipboard_image = supported_system_clipboard_image();
                let clipboard_item = if clipboard_image.is_none() {
                    cx.read_from_clipboard()
                } else {
                    None
                };
                if clipboard_image.is_none() {
                    clipboard_image = clipboard_item.as_ref().and_then(|item| {
                        item.entries().iter().find_map(|entry| match entry {
                            gpui::ClipboardEntry::Image(image) => {
                                Some((image.format.mime_type().to_string(), image.bytes.clone()))
                            }
                            gpui::ClipboardEntry::String(_) => None,
                        })
                    });
                }

                if let Some((media_type, bytes)) = clipboard_image {
                    if !supported_orchestrator_image(&media_type) {
                        self.dispatch_orchestrator_event(
                            serde_json::json!({
                                "type": "composer_notice",
                                "message": self.tr(
                                    "This image format is not supported. Use PNG, JPEG, GIF, or WebP.",
                                    "Este formato de imagen no es compatible. Usa PNG, JPEG, GIF o WebP.",
                                ),
                            }),
                            cx,
                        );
                    } else if bytes.len() > MAX_ORCHESTRATOR_IMAGE_BYTES {
                        self.dispatch_orchestrator_event(
                            serde_json::json!({
                                "type": "composer_notice",
                                "message": self.tr(
                                    "The image is larger than the 5 MB limit.",
                                    "La imagen supera el límite de 5 MB.",
                                ),
                            }),
                            cx,
                        );
                    } else {
                        self.dispatch_orchestrator_event(
                            serde_json::json!({
                                "type": "paste_image",
                                "attachment": {
                                    "id": Uuid::new_v4(),
                                    "media_type": media_type,
                                    "data": STANDARD.encode(bytes),
                                },
                            }),
                            cx,
                        );
                    }
                } else if let Some(text) = clipboard_item.and_then(|item| item.text()) {
                    self.dispatch_orchestrator_event(
                        serde_json::json!({
                            "type": "paste",
                            "text": text,
                        }),
                        cx,
                    );
                }
            }
            OrchestratorChatCommand::ChooseAttachments => {
                if let Some(paths) = rfd::FileDialog::new()
                    .set_title(self.tr("Attach images", "Adjuntar imágenes"))
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_files()
                {
                    for path in paths {
                        let Some(media_type) = orchestrator_image_media_type(&path) else {
                            self.dispatch_orchestrator_event(
                                serde_json::json!({
                                    "type": "composer_notice",
                                    "message": self.tr(
                                        "This file format is not supported. Use PNG, JPEG, GIF, or WebP.",
                                        "Este formato de archivo no es compatible. Usa PNG, JPEG, GIF o WebP.",
                                    ),
                                }),
                                cx,
                            );
                            continue;
                        };
                        match fs::read(&path) {
                            Ok(bytes) if bytes.is_empty() => {
                                self.dispatch_orchestrator_event(
                                    serde_json::json!({
                                        "type": "composer_notice",
                                        "message": self.tr(
                                            "The selected image is empty.",
                                            "La imagen seleccionada está vacía.",
                                        ),
                                    }),
                                    cx,
                                );
                            }
                            Ok(bytes) if bytes.len() > MAX_ORCHESTRATOR_IMAGE_BYTES => {
                                self.dispatch_orchestrator_event(
                                    serde_json::json!({
                                        "type": "composer_notice",
                                        "message": self.tr(
                                            "The image is larger than the 5 MB limit.",
                                            "La imagen supera el límite de 5 MB.",
                                        ),
                                    }),
                                    cx,
                                );
                            }
                            Ok(bytes) => {
                                self.dispatch_orchestrator_event(
                                    serde_json::json!({
                                        "type": "paste_image",
                                        "attachment": {
                                            "id": Uuid::new_v4(),
                                            "media_type": media_type,
                                            "data": STANDARD.encode(bytes),
                                        },
                                    }),
                                    cx,
                                );
                            }
                            Err(error) => {
                                tracing::warn!(
                                    ?error,
                                    ?path,
                                    "could not read orchestrator attachment"
                                );
                                self.dispatch_orchestrator_event(
                                    serde_json::json!({
                                        "type": "composer_notice",
                                        "message": self.tr(
                                            "The selected image could not be read.",
                                            "No se pudo leer la imagen seleccionada.",
                                        ),
                                    }),
                                    cx,
                                );
                            }
                        }
                    }
                }
            }
            OrchestratorChatCommand::CopyText { text } => {
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            OrchestratorChatCommand::OpenUrl { url } => {
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:")
                    || url.starts_with("tel:")
                {
                    cx.open_url(&url);
                }
            }
            OrchestratorChatCommand::OpenAgent {
                scope,
                project_id,
                task_id,
            } => {
                let target = match scope.as_str() {
                    "project" => project_id.map(OrchestratorChatScope::Project),
                    "task" => task_id.map(OrchestratorChatScope::Task),
                    _ => None,
                };
                if let Some(target) = target {
                    self.show_orchestrator_chat(target, cx);
                }
            }
            OrchestratorChatCommand::OpenTarget {
                scope,
                project_id,
                task_id,
            } => match scope.as_str() {
                "project" => {
                    if let Some(project_id) = project_id {
                        self.open_project_from_chat(project_id, cx);
                    }
                }
                "task" => {
                    if let Some(task_id) = task_id {
                        self.open_task_from_chat(task_id, cx);
                    }
                }
                _ => {}
            },
            OrchestratorChatCommand::SetAgentIdentity { identity } => {
                let scope = self.active_orchestrator_scope;
                self.orchestrator_chats.set_avatar_color(scope, identity);
                self.persist_orchestrator_chats();
                self.hydrate_navigation(cx);
                self.hydrate_orchestrator_chat(cx);
                cx.notify();
            }
            OrchestratorChatCommand::SendMessage {
                id,
                message,
                created_at,
                attachments,
            } => self.start_orchestrator_turn(id, message, created_at, attachments, cx),
            OrchestratorChatCommand::EditMessage {
                message_id,
                id,
                message,
                created_at,
                attachments,
            } => {
                self.edit_orchestrator_message(message_id, id, message, created_at, attachments, cx)
            }
            OrchestratorChatCommand::SwitchBranch { branch_id } => {
                self.switch_orchestrator_branch(branch_id, cx)
            }
            OrchestratorChatCommand::StopAgent => self.stop_orchestrator_turn(cx),
            OrchestratorChatCommand::NewChat => self.start_new_orchestrator_chat(cx),
            OrchestratorChatCommand::SetLanguage { language } => self.set_language(
                if language == "es" {
                    Language::Spanish
                } else {
                    Language::English
                },
                cx,
            ),
            OrchestratorChatCommand::SetTheme { theme } => self.set_theme(theme, cx),
            OrchestratorChatCommand::SetAgentProvider { provider } => {
                self.set_agent_provider(provider, cx)
            }
            OrchestratorChatCommand::CloseSettings => self.close_settings(cx),
            OrchestratorChatCommand::SetSidebarWidth { width, commit } => self.set_sidebar_width(width, commit, cx),
            OrchestratorChatCommand::RefreshPlanUsage => self.refresh_plan_usage(cx),
            OrchestratorChatCommand::InstallGitTools => {
                #[cfg(target_os = "macos")]
                if let Err(error) = std::process::Command::new("/usr/bin/xcode-select").arg("--install").spawn() {
                    self.set_status(format!("Could not open Apple's tools installer: {error}"), true, cx);
                }
            }
            OrchestratorChatCommand::RefreshRuntimeStatus => self.hydrate_active_workspace_surface(cx),
            OrchestratorChatCommand::RefreshModelCatalog { force } => self.refresh_model_catalog(force, cx),
            OrchestratorChatCommand::RevealAgentContext { project_only } => {
                self.reveal_agent_context(project_only, cx);
            }
            OrchestratorChatCommand::SetAgentAuthMode { auth_mode } => {
                self.set_agent_auth_mode(self.agent_provider(), auth_mode, cx)
            }
            OrchestratorChatCommand::AuthenticateAgentProvider => {
                self.authenticate_agent_provider(self.agent_provider(), window, cx)
            }
            OrchestratorChatCommand::SubmitAgentAuth { value } => {
                self.submit_agent_auth_value(value, cx)
            }
            OrchestratorChatCommand::CancelAgentAuth => self.cancel_agent_authentication(cx),
            OrchestratorChatCommand::SetAgentModel { model } => {
                self.set_agent_model(self.agent_provider(), &model, cx)
            }
            OrchestratorChatCommand::SetAgentEffort { effort } => {
                self.set_agent_effort(self.agent_provider(), &effort, cx)
            }
            OrchestratorChatCommand::SetAgentsFullAccess { enabled } => {
                self.set_agents_full_access(enabled, cx)
            }
            OrchestratorChatCommand::DismissAppModal => self.dismiss_app_modal(cx),
            OrchestratorChatCommand::ChooseProjectModalFolder { request_id } => {
                if self.project_modal_request != Some(request_id) || self.project_modal_submitting {
                    return;
                }
                let path = rfd::FileDialog::new()
                    .set_title(self.tr("Choose project folder", "Elegir carpeta del proyecto"))
                    .pick_folder();
                self.dispatch_orchestrator_event(serde_json::json!({
                    "type": "app_modal_feedback", "request_id": request_id,
                    "feedback": { "path": path.map(|path| path.display().to_string()) },
                }), cx);
            }
            OrchestratorChatCommand::SubmitCreateProject { request_id, mode, name, url, path } => {
                self.submit_create_project_modal(request_id, mode, name, url, path, cx);
            }
            OrchestratorChatCommand::ConfirmRemoveProject { workspace_id } => {
                self.remove_project_reference(workspace_id, cx);
                self.dismiss_app_modal(cx);
            }
            OrchestratorChatCommand::ConfirmRemoveAgent { scope } => {
                if let Some(scope) = parse_navigation_scope(&scope) {
                    self.remove_orchestrator_agent(scope, cx);
                }
                self.dismiss_app_modal(cx);
            }
            OrchestratorChatCommand::ConfirmRemoveTask { task_id } => {
                self.start_remove_task(task_id, cx);
                self.dismiss_app_modal(cx);
            }
            OrchestratorChatCommand::RevealProjectsRoot => self.reveal_projects_root(cx),
            OrchestratorChatCommand::ChooseProjectsRoot => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(self.tr(
                        "Choose the projects folder",
                        "Elige la carpeta de proyectos",
                    ))
                    .pick_folder()
                {
                    self.set_projects_root(path, cx);
                    cx.notify();
                }
            }
            OrchestratorChatCommand::RevealAgentSkills => self.reveal_agent_skills(cx),
            OrchestratorChatCommand::ImportAgentSkills => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(self.tr(
                        "Choose a skill folder or a folder containing skills",
                        "Elige una skill o una carpeta que contenga skills",
                    ))
                    .pick_folder()
                {
                    self.import_agent_skills(path, cx);
                }
            }
            OrchestratorChatCommand::SetAgentSkillEnabled { name, enabled } => {
                self.set_agent_skill_enabled(name, enabled, cx)
            }
            OrchestratorChatCommand::SetAgentMcpEnabled { name, enabled } => {
                self.set_agent_mcp_enabled(name, enabled, cx)
            }
            OrchestratorChatCommand::SetProjectAgentSkillEnabled {
                workspace_id,
                name,
                enabled,
            } => self.set_project_agent_skill_enabled(workspace_id, name, enabled, cx),
            OrchestratorChatCommand::SetProjectAgentMcpEnabled {
                workspace_id,
                name,
                enabled,
            } => self.set_project_agent_mcp_enabled(workspace_id, name, enabled, cx),
            OrchestratorChatCommand::AuthenticateProjectAgentMcp { workspace_id, name } => {
                self.authenticate_project_agent_mcp(workspace_id, name, cx)
            }
            OrchestratorChatCommand::CancelProjectAgentMcpAuthentication { workspace_id, name } => {
                self.cancel_project_agent_mcp_authentication(workspace_id, name, cx)
            }
            OrchestratorChatCommand::InstallProjectAgentMcp {
                workspace_id,
                name,
                transport,
                url,
                oauth_client_id,
                oauth_callback_port,
                command,
                args,
                env,
            } => self.install_project_agent_mcp(
                workspace_id,
                name,
                transport,
                url,
                oauth_client_id,
                oauth_callback_port,
                command,
                args,
                env,
                cx,
            ),
            OrchestratorChatCommand::RemoveProjectAgentMcp { workspace_id, name } => {
                self.remove_project_agent_mcp(workspace_id, name, cx)
            }
            OrchestratorChatCommand::UpdateProjectInstructions {
                workspace_id,
                content,
            } => self.update_project_instructions(workspace_id, content, cx),
            OrchestratorChatCommand::UpdateProjectTaskInstructions {
                workspace_id,
                content,
            } => self.update_project_task_instructions(workspace_id, content, cx),
            OrchestratorChatCommand::UpdateNote {
                owner,
                id,
                content,
                blocks,
            } => {
                if let Some(owner) = note_owner_from_command(&owner, id)
                    && blocks.is_array()
                    && let Some(editor) = self.note_handle_mut(owner).map(|handle| {
                        handle.blocks = Some(blocks);
                        handle.editor.clone()
                    })
                {
                    let editor_content = content.clone();
                    editor.update(cx, |input, cx| input.set_value(editor_content, window, cx));
                    self.queue_note_save(owner, content, Duration::ZERO, cx);
                }
            }
            OrchestratorChatCommand::ToggleNotePreview { owner, id } => {
                if let Some(owner) = note_owner_from_command(&owner, id) {
                    self.toggle_note_preview(owner, window, cx);
                }
            }
            OrchestratorChatCommand::ReloadNote { owner, id } => {
                if let Some(owner) = note_owner_from_command(&owner, id) {
                    self.reload_note(owner, cx);
                }
            }
            OrchestratorChatCommand::SetNoteAppearance {
                owner,
                id,
                icon,
                color,
            } => {
                if let Some(owner) = note_owner_from_command(&owner, id) {
                    self.update_note_appearance(owner, icon, color, cx);
                }
            }
            OrchestratorChatCommand::RefreshFileExplorer => self.refresh_file_explorer(cx),
            OrchestratorChatCommand::CloseFileExplorer => self.close_file_explorer(cx),
            OrchestratorChatCommand::SetFileExplorerMode { mode } => self.set_file_explorer_mode(
                if mode == "changes" {
                    FileExplorerMode::Changes
                } else {
                    FileExplorerMode::Files
                },
                cx,
            ),
            OrchestratorChatCommand::ActivateFileRow {
                path,
                kind,
                click_count,
            } => {
                let kind = match kind.as_str() {
                    "directory" => Some(FileEntryKind::Directory),
                    "file" => Some(FileEntryKind::File),
                    "symlink" => Some(FileEntryKind::Symlink),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.activate_file_tree_row(PathBuf::from(path), kind, click_count, cx);
                }
            }
            OrchestratorChatCommand::OpenRepositoryDiff { relative_path } => {
                let change = match &self.file_explorer.changes {
                    RepositoryChangesState::Ready(changes) => changes
                        .iter()
                        .find(|change| change.relative_path == relative_path)
                        .cloned(),
                    _ => None,
                };
                if let Some(change) = change {
                    self.open_repository_diff(change, cx);
                }
            }
            OrchestratorChatCommand::CloseRepositoryDiff => self.close_repository_diff(cx),
            OrchestratorChatCommand::UpdateFileContent {
                request_id,
                content,
            } => {
                if let Some(editor) = self.active_file.as_ref().and_then(|document| {
                    (document.request_id == request_id)
                        .then(|| document.editor.clone())
                        .flatten()
                }) {
                    editor.update(cx, |input, cx| input.set_value(content, window, cx));
                }
            }
            OrchestratorChatCommand::SaveActiveFile => self.flush_active_file(cx),
            OrchestratorChatCommand::CloseFileEditor => self.close_file_editor(cx),
            OrchestratorChatCommand::OpenProjectInstructions { workspace_id } => {
                self.open_project_instructions(workspace_id, cx)
            }
            OrchestratorChatCommand::OpenProjectTaskInstructions { workspace_id } => {
                self.open_project_task_instructions(workspace_id, cx)
            }
            OrchestratorChatCommand::QuickOpenQueryChanged { open_id, query } => {
                let Some(state) = self.quick_open.as_mut() else {
                    return;
                };
                if state.id != open_id {
                    return;
                }
                let input = state.query.clone();
                state.selected = 0;
                input.update(cx, |input, cx| input.set_value(query, window, cx));
                self.hydrate_quick_open_overlay(cx);
            }
            OrchestratorChatCommand::QuickOpenActivate {
                open_id,
                result_index,
            } => {
                if self.quick_open.as_ref().map(|state| state.id) != Some(open_id) {
                    return;
                }
                let Some(target) = self
                    .quick_open_results(cx)
                    .get(result_index)
                    .map(|item| item.target.clone())
                else {
                    return;
                };
                self.activate_quick_open_target(target, cx);
            }
            OrchestratorChatCommand::QuickOpenDismiss { open_id } => {
                if self.quick_open.as_ref().map(|state| state.id) == Some(open_id) {
                    self.close_quick_open(cx);
                }
            }
            OrchestratorChatCommand::DismissStatus => {
                self.status = None;
                self.status_revision = self.status_revision.wrapping_add(1);
                cx.notify();
            }
        }
    }

    fn handle_navigation_command(
        &mut self,
        command: NavigationCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            NavigationCommand::Ready => self.hydrate_navigation(cx),
            NavigationCommand::SetSidebarWidth { width, commit } => self.set_sidebar_width(width, commit, cx),
            NavigationCommand::CreateGlobalAgent => self.create_global_orchestrator_agent(cx),
            NavigationCommand::CreateScopedAgent {
                workspace_id,
                task_id,
            } => self.create_scoped_orchestrator_agent(workspace_id, task_id, cx),
            NavigationCommand::OpenAgent { scope } => {
                if let Some(scope) = parse_navigation_scope(&scope) {
                    self.show_orchestrator_chat(scope, cx);
                }
            }
            NavigationCommand::RemoveAgent { scope } => {
                if let Some(scope) = parse_navigation_scope(&scope) {
                    self.open_remove_orchestrator_agent_confirmation(scope, window, cx);
                }
            }
            NavigationCommand::CollapseAll => self.collapse_all_navigation(cx),
            NavigationCommand::NewProject => self.open_create_project(window, cx),
            NavigationCommand::AddProjectRepository { workspace_id, github } => {
                self.open_add_project_repository(workspace_id, github, window, cx)
            }
            NavigationCommand::SelectProject { workspace_id } => {
                self.select_target(workspace_id, None, None, cx)
            }
            NavigationCommand::ToggleProject { workspace_id } => {
                self.toggle_workspace_expanded(workspace_id, cx)
            }
            NavigationCommand::RefreshProject { workspace_id } => {
                self.refresh_project_repositories(workspace_id, cx)
            }
            NavigationCommand::EditProject { workspace_id } => {
                self.open_edit_project(workspace_id, window, cx)
            }
            NavigationCommand::ProjectSettings { workspace_id } => {
                self.open_project_settings(workspace_id, cx)
            }
            NavigationCommand::RemoveProject { workspace_id } => {
                self.open_remove_project_confirmation(workspace_id, window, cx)
            }
            NavigationCommand::AssignProjectAgent { workspace_id } => {
                self.assign_orchestrator_agent(OrchestratorChatScope::Project(workspace_id), cx)
            }
            NavigationCommand::ProjectNotes { workspace_id } => {
                self.show_project_notes(workspace_id, cx)
            }
            NavigationCommand::NewTask { workspace_id } => {
                self.select_target(workspace_id, None, None, cx);
                self.open_create_task(window, cx);
            }
            NavigationCommand::SelectTask {
                workspace_id,
                task_id,
            } => self.select_target(workspace_id, Some(task_id), None, cx),
            NavigationCommand::ToggleTask { task_id } => {
                self.mark_task_seen(task_id, cx);
                self.toggle_task_expanded(task_id, cx);
            }
            NavigationCommand::EditTask { task_id } => {
                if let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() {
                    self.open_manage_task(task, window, cx);
                }
            }
            NavigationCommand::RemoveTask { task_id } => {
                self.open_remove_task_confirmation(task_id, window, cx)
            }
            NavigationCommand::AssignTaskAgent { task_id } => {
                self.assign_orchestrator_agent(OrchestratorChatScope::Task(task_id), cx)
            }
            NavigationCommand::TaskNotes {
                workspace_id,
                task_id,
            } => self.show_task_notes_for(workspace_id, task_id, cx),
            NavigationCommand::SelectRepository {
                workspace_id,
                task_id,
                repository_id,
            } => self.select_repository_target(workspace_id, task_id, repository_id, cx),
            NavigationCommand::NewTerminal {
                workspace_id,
                task_id,
                repository_id,
                agent,
            } => {
                self.select_target(workspace_id, task_id, repository_id, cx);
                self.new_terminal(agent, window, cx);
            }
            NavigationCommand::FocusTerminal { terminal_id } => {
                self.focus_terminal(terminal_id, window, cx)
            }
            NavigationCommand::CloseTerminal { terminal_id } => {
                self.close_terminal(terminal_id, cx)
            }
            NavigationCommand::ShowSettings => self.show_settings(cx),
        }
    }

    fn dispatch_orchestrator_event(&self, event: serde_json::Value, cx: &mut Context<Self>) {
        let Some(webview) = &self.orchestrator_webview else {
            return;
        };
        if let Err(error) = orchestrator_chat::dispatch(webview, event, cx) {
            tracing::warn!(?error, "failed to update orchestrator chat");
        }
    }

    fn dispatch_navigation_event(&self, event: serde_json::Value, cx: &mut Context<Self>) {
        let Some(webview) = &self.navigation_webview else {
            return;
        };
        if let Err(error) = navigation_webview::dispatch(webview, event, cx) {
            tracing::warn!(?error, "failed to update WebView navigation");
        }
    }

    fn navigation_agent_context(&self, scope: OrchestratorChatScope) -> Option<serde_json::Value> {
        match scope {
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| {
                    serde_json::json!({
                        "kind": "project",
                        "label": workspace.label(),
                        "project_id": workspace.id,
                        "project_label": workspace.label(),
                    })
                }),
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| {
                    serde_json::json!({
                        "kind": "task",
                        "label": task.title,
                        "task_id": task.id,
                        "project_id": task.workspace_id,
                        "project_label": self.workspaces.iter()
                            .find(|workspace| workspace.id == task.workspace_id)
                            .map(|workspace| workspace.label()).unwrap_or_default(),
                    })
                }),
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => None,
        }
    }

    fn reveal_agent_context(&mut self, project_only: bool, cx: &mut Context<Self>) {
        let scope = self.active_orchestrator_scope;
        let task = scope.task_id().and_then(|id| self.tasks.iter().find(|task| task.id == id));
        let workspace_id = task.map(|task| task.workspace_id).or_else(|| scope.project_id());
        let task_id = if project_only { None } else { task.map(|task| task.id) };
        let Some(workspace_id) = workspace_id else { return; };
        if !self.workspaces.iter().any(|workspace| workspace.id == workspace_id) { return; }
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        if let Some(task_id) = task_id {
            insert_unique(&mut self.session.expanded_task_ids, task_id);
        }
        self.persist_session();
        // Expand first, then reveal the row without changing the active chat.
        self.hydrate_navigation(cx);
        self.dispatch_navigation_event(serde_json::json!({
            "type": "reveal_target", "workspace_id": workspace_id, "task_id": task_id,
        }), cx);
        cx.notify();
    }

    fn navigation_agent(&self, scope: OrchestratorChatScope, removable: bool) -> serde_json::Value {
        let identity = self.orchestrator_chats.avatar_color(scope);
        serde_json::json!({
            "scope": navigation_scope_id(scope),
            "name": identity.display_name(),
            "preview": self.orchestrator_chat_preview(scope),
            "selected": self.orchestrator_surface_visible()
                && self.active_orchestrator_scope == scope,
            "busy": self.orchestrator_turns.contains_key(&scope),
            "identity": identity.id(),
            "removable": removable,
            "arriving": self.arriving_orchestrator_agents.contains(&scope),
            "context": self.navigation_agent_context(scope),
        })
    }

    fn navigation_terminal(
        &self,
        terminal: &TerminalDescriptor,
        active_terminal_id: Option<Uuid>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": terminal.id,
            "label": terminal.label,
            "agent": terminal.agent,
            "busy": terminal.state == SessionState::Working,
            "selected": active_terminal_id == Some(terminal.id),
        })
    }

    fn hydrate_navigation(&self, cx: &mut Context<Self>) {
        let active_terminal_id = self
            .show_terminal
            .then(|| self.selected_terminal_id())
            .flatten();
        let mut scoped_agents = Vec::new();
        for workspace in &self.workspaces {
            let project_scope = OrchestratorChatScope::Project(workspace.id);
            if self.orchestrator_chats.has_agent(project_scope) {
                scoped_agents.push(project_scope);
            }
            scoped_agents.extend(
                self.orchestrator_chats
                    .project_agent_ids(workspace.id)
                    .iter()
                    .copied()
                    .map(|agent_id| OrchestratorChatScope::ProjectAgent {
                        project_id: workspace.id,
                        agent_id,
                    }),
            );
            for task in self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace.id)
            {
                let task_scope = OrchestratorChatScope::Task(task.id);
                if self.orchestrator_chats.has_agent(task_scope) {
                    scoped_agents.push(task_scope);
                }
                scoped_agents.extend(
                    self.orchestrator_chats
                        .task_agent_ids(task.id)
                        .iter()
                        .copied()
                        .map(|agent_id| OrchestratorChatScope::TaskAgent {
                            task_id: task.id,
                            agent_id,
                        }),
                );
            }
        }
        scoped_agents.sort_by_key(|scope| {
            (
                !self.orchestrator_turns.contains_key(scope),
                navigation_scope_id(*scope),
            )
        });
        let mut global_agents = scoped_agents
            .into_iter()
            .map(|scope| self.navigation_agent(scope, true))
            .collect::<Vec<_>>();
        if self
            .orchestrator_chats
            .has_agent(OrchestratorChatScope::Global)
        {
            global_agents.push(self.navigation_agent(OrchestratorChatScope::Global, true));
        }
        for agent_id in self.orchestrator_chats.global_agent_ids().iter().copied() {
            global_agents
                .push(self.navigation_agent(OrchestratorChatScope::GlobalAgent(agent_id), true));
        }

        let mut projects = Vec::with_capacity(self.workspaces.len());
        for workspace in &self.workspaces {
            let workspace_id = workspace.id;
            let expanded = self.session.expanded_workspace_ids.contains(&workspace_id);
            let project_scope = OrchestratorChatScope::Project(workspace_id);
            let mut project_agents = Vec::new();
            if self.orchestrator_chats.has_agent(project_scope) {
                project_agents.push(self.navigation_agent(project_scope, true));
            }
            for agent_id in self
                .orchestrator_chats
                .project_agent_ids(workspace_id)
                .iter()
                .copied()
            {
                project_agents.push(self.navigation_agent(
                    OrchestratorChatScope::ProjectAgent {
                        project_id: workspace_id,
                        agent_id,
                    },
                    true,
                ));
            }
            let root_terminals = self
                .session
                .terminals
                .iter()
                .filter(|terminal| {
                    terminal.workspace_id == workspace_id
                        && terminal.task_id.is_none()
                        && terminal.repository_id.is_none()
                })
                .map(|terminal| self.navigation_terminal(terminal, active_terminal_id))
                .collect::<Vec<_>>();
            let project_repositories = workspace
                .repositories
                .iter()
                .map(|repository| {
                    let (branch, additions, deletions, loading) =
                        self.repository_git_details(&repository.path, repository.branch.as_deref());
                    let terminals = self
                        .session
                        .terminals
                        .iter()
                        .filter(|terminal| {
                            terminal.workspace_id == workspace_id
                                && terminal.task_id.is_none()
                                && terminal.repository_id == Some(repository.id)
                        })
                        .map(|terminal| self.navigation_terminal(terminal, active_terminal_id))
                        .collect::<Vec<_>>();
                    serde_json::json!({
                        "id": repository.id,
                        "name": repository.name,
                        "branch": branch,
                        "additions": additions,
                        "deletions": deletions,
                        "loading": loading,
                        "selected": self.session.selected_workspace_id == Some(workspace_id)
                            && self.session.selected_task_id.is_none()
                            && self.session.selected_repository_id == Some(repository.id),
                        "terminals": terminals,
                    })
                })
                .collect::<Vec<_>>();

            let tasks = self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace_id)
                .map(|task| {
                    let task_scope = OrchestratorChatScope::Task(task.id);
                    let mut agents = Vec::new();
                    if self.orchestrator_chats.has_agent(task_scope) {
                        agents.push(self.navigation_agent(task_scope, true));
                    }
                    for agent_id in self
                        .orchestrator_chats
                        .task_agent_ids(task.id)
                        .iter()
                        .copied()
                    {
                        agents.push(self.navigation_agent(
                            OrchestratorChatScope::TaskAgent {
                                task_id: task.id,
                                agent_id,
                            },
                            true,
                        ));
                    }
                    let root_terminals = self
                        .session
                        .terminals
                        .iter()
                        .filter(|terminal| {
                            terminal.workspace_id == workspace_id
                                && terminal.task_id == Some(task.id)
                                && terminal.repository_id.is_none()
                        })
                        .map(|terminal| self.navigation_terminal(terminal, active_terminal_id))
                        .collect::<Vec<_>>();
                    let repositories =
                        task.repositories
                            .iter()
                            .map(|task_repository| {
                                let repository = workspace.repositories.iter().find(|repository| {
                                    repository.id == task_repository.repository_id
                                });
                                let (branch, additions, deletions, loading) = self
                                    .repository_git_details(
                                        &task_repository.worktree_path,
                                        Some(task_repository.branch.as_str()),
                                    );
                                let terminals = self
                                    .session
                                    .terminals
                                    .iter()
                                    .filter(|terminal| {
                                        terminal.workspace_id == workspace_id
                                            && terminal.task_id == Some(task.id)
                                            && terminal.repository_id
                                                == Some(task_repository.repository_id)
                                    })
                                    .map(|terminal| {
                                        self.navigation_terminal(terminal, active_terminal_id)
                                    })
                                    .collect::<Vec<_>>();
                                serde_json::json!({
                                    "id": task_repository.repository_id,
                                    "name": repository.map(|repository| repository.name.as_str())
                                        .unwrap_or("repository"),
                                    "branch": branch,
                                    "additions": additions,
                                    "deletions": deletions,
                                    "loading": loading,
                                    "selected": self.session.selected_task_id == Some(task.id)
                                        && self.session.selected_repository_id
                                            == Some(task_repository.repository_id),
                                    "terminals": terminals,
                                })
                            })
                            .collect::<Vec<_>>();
                    serde_json::json!({
                        "id": task.id,
                        "title": task.title,
                        "icon": task.icon,
                        "color": workspace_color_css(task.color),
                        "expanded": self.session.expanded_task_ids.contains(&task.id),
                        "selected": self.session.selected_task_id == Some(task.id),
                        "unseen": self.session.unseen_task_ids.contains(&task.id),
                        "notes_selected": self.show_task_note
                            && self.session.selected_task_id == Some(task.id)
                            && self.session.selected_repository_id.is_none(),
                        "agents": agents,
                        "terminals": root_terminals,
                        "repositories": repositories,
                    })
                })
                .collect::<Vec<_>>();

            projects.push(serde_json::json!({
                "id": workspace.id,
                "label": workspace.label(),
                "icon": workspace.icon,
                "color": workspace_color_css(workspace.color),
                "expanded": expanded,
                "selected": self.session.selected_workspace_id == Some(workspace.id)
                    && self.session.selected_task_id.is_none(),
                "notes_selected": self.show_project_note
                    && self.session.selected_workspace_id == Some(workspace.id)
                    && self.session.selected_task_id.is_none()
                    && self.session.selected_repository_id.is_none(),
                "agents": project_agents,
                "terminals": root_terminals,
                "repositories": project_repositories,
                "tasks": tasks,
            }));
        }

        let (language, copy) = match self.session.language {
            Language::English => (
                "en",
                serde_json::json!({
                    "projects": "Projects",
                    "project": "Project",
                    "settings": "Settings",
                    "working": "Working",
                    "terminal": "Terminal",
                    "tasks": "Tasks",
                    "task": "Task",
                    "notes": "Notes",
                    "new": "New",
                    "toggle": "Expand or collapse",
                    "options": "Options",
                    "removeAgent": "Remove Black Bot",
                    "closeTerminal": "Close terminal",
                    "newTerminal": "New terminal",
                    "newTask": "Add task",
                    "addAgent": "Add bot",
                    "assignAgent": "Assign Black Bot",
                    "refreshProject": "Find new repositories",
                    "addToProject": "Add to project",
                    "cloneLocalRepository": "Clone local repository…",
                    "cloneGithubRepository": "Clone GitHub repository…",
                    "editProject": "Edit project",
                    "projectSettings": "Project settings",
                    "removeProject": "Remove project",
                    "editTask": "Edit task",
                    "removeTask": "Delete task"
                }),
            ),
            Language::Spanish => (
                "es",
                serde_json::json!({
                    "projects": "Proyectos",
                    "project": "Proyecto",
                    "settings": "Configuración",
                    "working": "Trabajando",
                    "terminal": "Terminal",
                    "tasks": "Tareas",
                    "task": "Tarea",
                    "notes": "Notas",
                    "new": "Nuevo",
                    "toggle": "Expandir o contraer",
                    "options": "Opciones",
                    "removeAgent": "Eliminar Black Bot",
                    "closeTerminal": "Cerrar terminal",
                    "newTerminal": "Nueva terminal",
                    "newTask": "Agregar tarea",
                    "addAgent": "Agregar bot",
                    "assignAgent": "Asignar Black Bot",
                    "refreshProject": "Buscar repositorios nuevos",
                    "addToProject": "Agregar al proyecto",
                    "cloneLocalRepository": "Clonar repositorio local…",
                    "cloneGithubRepository": "Clonar repositorio de GitHub…",
                    "editProject": "Editar proyecto",
                    "projectSettings": "Configuración del proyecto",
                    "removeProject": "Eliminar proyecto",
                    "editTask": "Editar tarea",
                    "removeTask": "Eliminar tarea"
                }),
            ),
        };
        self.dispatch_navigation_event(
            serde_json::json!({
                "type": "hydrate",
                "language": language,
                "theme": app_theme_id(self.session.theme),
                "copy": copy,
                "settings_selected": self.show_settings,
                "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
                "global_agents": global_agents,
                "projects": projects,
            }),
            cx,
        );
    }

    fn hydrate_orchestrator_chat(&self, cx: &mut Context<Self>) {
        let scope = self.active_orchestrator_scope;
        if !self.orchestrator_chats.has_agent(scope) {
            self.hydrate_unassigned_agent_surface(cx);
            return;
        }
        let (agent_name, context_label, placeholder, welcome) = self.orchestrator_chat_copy(scope);
        let agent_identity = self.orchestrator_chats.avatar_color(scope);
        let provider = self.agent_provider();
        let (selected_model, selected_model_label) = self.selected_agent_model(provider);
        let model_options = self.agent_model_choices(provider);
        let language = match self.session.language {
            Language::English => "en",
            Language::Spanish => "es",
        };
        let messages = self
            .orchestrator_chats
            .chat(scope)
            .map(|chat| {
                chat.messages
                    .iter()
                    .filter_map(|message| {
                        let mut value = serde_json::to_value(message).ok()?;
                        if let Some(navigation) = chat.branch_navigation(message) {
                            value["branch_navigation"] = serde_json::to_value(navigation).ok()?;
                        }
                        Some(value)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let active_response = self.orchestrator_turns.get(&scope).map(|turn| {
            serde_json::json!({
                "id": turn.response_id,
                "after_id": turn.user_message_id,
                "text": turn.response_text,
                "created_at": turn.started_at,
                "activities": turn.activities,
                "handoffs": turn.handoffs,
            })
        });
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "hydrate",
                "language": language,
                "theme": app_theme_id(self.session.theme),
                "agent_name": agent_name,
                "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
                "agent_identity": agent_identity.id(),
                "context_label": context_label,
                "agent_context": self.navigation_agent_context(scope),
                "placeholder": placeholder,
                "welcome": welcome,
                "messages": messages,
                "busy": self.orchestrator_turns.contains_key(&scope),
                "active_response": active_response,
                "full_access": self.agents_full_access(),
                "permission_control_supported": provider.supports_permission_mode(),
                "provider_label": provider.model_brand_name(),
                "model": selected_model,
                "model_label": selected_model_label,
                "model_catalog_loading": self.model_catalog_loading,
                "model_catalog_error": self.model_catalog_error,
                "model_options": model_options,
                "model_control_supported": provider.supports_model_selection(),
            }),
            cx,
        );
    }

    fn hydrate_settings_surface(&self, cx: &mut Context<Self>) {
        let language = match self.session.language {
            Language::English => "en",
            Language::Spanish => "es",
        };
        let provider = self.agent_provider();
        let selected_model = self
            .agent_model(provider)
            .unwrap_or_else(|| "automatic".to_string());
        let model_options = self.agent_model_choices(provider);
        let effort_options = self.agent_effort_options(provider).into_iter()
            .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
            .collect::<Vec<_>>();
        let enabled_skills = self.enabled_agent_skill_names();
        let skills = self
            .agent_skills()
            .into_iter()
            .map(|skill| {
                let enabled = enabled_skills.contains(&skill.name);
                serde_json::json!({
                    "name": skill.name,
                    "description": skill.description,
                    "path": skill.path.display().to_string(),
                    "enabled": enabled,
                })
            })
            .collect::<Vec<_>>();
        let enabled_mcps = self.enabled_agent_mcp_names();
        let mcps = self
            .agent_mcp_servers(None)
            .into_iter()
            .map(|mcp| {
                serde_json::json!({
                    "enabled": enabled_mcps.contains(&mcp.name),
                    "name": mcp.name,
                    "source": mcp.source,
                    "required": mcp.required,
                    "managed": mcp.managed,
                    "transport": mcp.config.as_ref().map(AgentMcpServerConfig::transport_label),
                })
            })
            .collect::<Vec<_>>();
        let authentication = self
            .agent_authentication
            .as_ref()
            .filter(|authentication| authentication.provider == provider)
            .map(|authentication| {
                serde_json::json!({
                    "status": match authentication.status {
                        AgentAuthStatus::Connecting => "connecting",
                        AgentAuthStatus::NeedsInput => "needs-input",
                        AgentAuthStatus::Connected => "connected",
                        AgentAuthStatus::Error => "error",
                    },
                    "detail": authentication.detail,
                    "opened_url": authentication.opened_url,
                })
            });
        let plan_usage = self.active_plan_usage.as_ref();
        let usage_totals = self.orchestrator_chats.provider_usage_totals(provider);
        let mut usage_cards = vec![
            serde_json::json!({
                "label": self.tr("Plan", "Plan"),
                "value": provider_plan_name(provider, plan_usage, self.session.language),
                "detail": provider_plan_detail(plan_usage, self.session.language),
                "utilization": serde_json::Value::Null,
            }),
            serde_json::json!({
                "label": self.tr("Estimated API cost", "Costo API estimado"),
                "value": format!("${:.4}", usage_totals.cost_usd),
                "detail": match self.session.language {
                    Language::English => format!("{} requests · {} agent turns", usage_totals.requests, usage_totals.num_turns),
                    Language::Spanish => format!("{} solicitudes · {} turnos de agente", usage_totals.requests, usage_totals.num_turns),
                },
                "utilization": serde_json::Value::Null,
            }),
        ];
        // Window durations are provider-reported, not always five hours / weekly.
        if let Some(usage) = plan_usage {
            for (index, window) in usage.windows.iter().enumerate() {
                let duration = match window.minutes {
                    Some(10080) => self.tr("Weekly limit", "Límite semanal").to_string(),
                    Some(minutes) if minutes % 60 == 0 => format!("{} h", minutes / 60),
                    Some(minutes) => format!("{minutes} min"),
                    None => self.tr("Usage limit", "Límite de uso").to_string(),
                };
                let label = if window.label.is_empty() { duration } else { format!("{} · {duration}", window.label) };
                let (value, detail, utilization) = claude_limit_display(Some(&ClaudeRateLimitWindow {
                    utilization: window.utilization, resets_at: window.resets_at.clone(),
                }), self.session.language);
                usage_cards.insert(index + 1, serde_json::json!({
                    "label": label, "value": value, "detail": detail, "utilization": utilization,
                }));
            }
        }
        let token_detail = format!(
            "{}: {}  ·  {}: {}  ·  {}: {}  ·  {}: {}",
            self.tr("Input", "Entrada"),
            format_token_count(usage_totals.input_tokens),
            self.tr("Output", "Salida"),
            format_token_count(usage_totals.output_tokens),
            self.tr("Cache read", "Caché leída"),
            format_token_count(usage_totals.cache_read_input_tokens),
            self.tr("Cache written", "Caché escrita"),
            format_token_count(usage_totals.cache_creation_input_tokens),
        );
        let usage_updated = self
            .plan_usage_updated_at
            .map(|timestamp| {
                let timestamp = timestamp.with_timezone(&chrono::Local);
                match self.session.language {
                    Language::English => {
                        format!("Last updated {}", timestamp.format("%b %-d, %H:%M"))
                    }
                    Language::Spanish => {
                        format!("Actualizado el {}", timestamp.format("%-d/%m, %H:%M"))
                    }
                }
            })
            .unwrap_or_else(|| {
                self.tr(
                    "Account limits have not been refreshed yet.",
                    "Todavía no se actualizaron los límites de la cuenta.",
                )
                .to_string()
            });
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "settings",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "language": language,
                    "theme": app_theme_id(self.session.theme),
                    "projects_root": self.projects_root().display().to_string(),
                    "git_available": crate::services::projects::git_tools_available(),
                    "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
                    "provider": provider.id(),
                    "provider_label": provider.display_name(),
                    "auth_mode": self.agent_auth_mode(provider).id(),
                    "authentication": authentication,
                    "model": selected_model,
                    "model_options": model_options,
                    "model_catalog_loading": self.model_catalog_loading,
                    "model_catalog_error": self.model_catalog_error,
                    "model_control_supported": provider.supports_model_selection(),
                    "effort": self.agent_effort(provider).unwrap_or_else(|| "automatic".to_string()),
                    "effort_options": effort_options,
                    "full_access": self.agents_full_access(),
                    "permission_control_supported": provider.supports_permission_mode(),
                    "skills": skills,
                    "mcps": mcps,
                    "external_mcp_control_supported": AgentMcpService::supports_external_servers(provider),
                    "usage_cards": usage_cards,
                    "token_detail": token_detail,
                    "usage_updated": usage_updated,
                    "usage_refreshing": self.plan_usage_refreshing,
                    "usage_refresh_error": self.plan_usage_refresh_error,
                }
            }),
            cx,
        );
    }

    fn hydrate_project_settings_surface(&self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let mut errors = Vec::new();
        let project_instructions =
            ProjectInstructionsService::read(workspace).unwrap_or_else(|error| {
                errors.push(format!("Could not read the project CLAUDE.md: {error:#}"));
                String::new()
            });
        let task_instructions =
            ProjectTaskInstructionsService::read(workspace).unwrap_or_else(|error| {
                errors.push(format!(
                    "Could not read the shared task CLAUDE.md: {error:#}"
                ));
                String::new()
            });
        let globally_enabled = self.enabled_agent_skill_names();
        let project_enabled = self.project_enabled_agent_skill_names(workspace_id);
        let skills = self
            .agent_skills()
            .into_iter()
            .filter(|skill| globally_enabled.contains(&skill.name))
            .map(|skill| {
                let enabled = project_enabled.contains(&skill.name);
                serde_json::json!({
                    "name": skill.name,
                    "description": skill.description,
                    "path": skill.path.display().to_string(),
                    "enabled": enabled,
                })
            })
            .collect::<Vec<_>>();
        let globally_configured_mcps = self
            .agent_mcp_servers(None)
            .into_iter()
            .map(|mcp| mcp.name)
            .collect::<HashSet<_>>();
        let globally_enabled_mcps = self.enabled_agent_mcp_names();
        let enabled_mcps = self.project_enabled_agent_mcp_names(workspace_id);
        let mcps = self
            .agent_mcp_servers(Some(workspace_id))
            .into_iter()
            .filter(|mcp| {
                !globally_configured_mcps.contains(&mcp.name)
                    || globally_enabled_mcps.contains(&mcp.name)
            })
            .map(|mcp| {
                let (authentication_status, authentication_detail) =
                    self.project_mcp_authentication_display(workspace_id, &mcp);
                serde_json::json!({
                    "enabled": enabled_mcps.contains(&mcp.name),
                    "name": mcp.name,
                    "source": mcp.source,
                    "required": mcp.required,
                    "managed": mcp.managed,
                    "transport": mcp.config.as_ref().map(AgentMcpServerConfig::transport_label),
                    "authentication_status": authentication_status,
                    "authentication_detail": authentication_detail,
                })
            })
            .collect::<Vec<_>>();

        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "project-settings",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "language": match self.session.language { Language::English => "en", Language::Spanish => "es" },
                    "theme": app_theme_id(self.session.theme),
                    "workspace_id": workspace.id,
                    "title": workspace.label(),
                    "skills": skills,
                    "mcps": mcps,
                    "external_mcp_control_supported": AgentMcpService::supports_external_servers(self.agent_provider()),
                    "project_revision": content_revision(&project_instructions),
                    "project_instructions": project_instructions,
                    "task_revision": content_revision(&task_instructions),
                    "task_instructions": task_instructions,
                    "error": (!errors.is_empty()).then(|| errors.join("\n")),
                }
            }),
            cx,
        );
    }

    fn hydrate_note_surface(&self, owner: NoteOwner, cx: &mut Context<Self>) {
        let Some(note) = self.note_handle(owner) else {
            return;
        };
        let (owner_kind, id, title, icon, color) = match owner {
            NoteOwner::Project(id) => {
                let Some(workspace) = self.workspaces.iter().find(|workspace| workspace.id == id)
                else {
                    return;
                };
                (
                    "project",
                    id,
                    workspace.label().to_string(),
                    workspace.icon.clone(),
                    workspace.color,
                )
            }
            NoteOwner::Task(id) => {
                let Some(task) = self.tasks.iter().find(|task| task.id == id) else {
                    return;
                };
                (
                    "task",
                    id,
                    task.title.clone(),
                    task.icon.clone(),
                    task.color,
                )
            }
        };
        let icon_options = project_icon_options(self.session.language)
            .into_iter()
            .map(|(value, label, _)| serde_json::json!({ "value": value, "label": label }))
            .collect::<Vec<_>>();
        let color_options = project_colors()
            .into_iter()
            .map(|color| {
                serde_json::json!({
                    "value": workspace_color_id(color),
                    "label": workspace_color_id(color),
                    "color": workspace_color_css(color),
                })
            })
            .collect::<Vec<_>>();
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "note",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "language": match self.session.language { Language::English => "en", Language::Spanish => "es" },
                    "theme": app_theme_id(self.session.theme),
                    "owner": owner_kind,
                    "id": id,
                    "document_id": note.document_id,
                    "title": title,
                    "icon": icon,
                    "color": workspace_color_css(color),
                    "color_id": workspace_color_id(color),
                    "content": note.editor.read(cx).value().to_string(),
                    "blocks": &note.blocks,
                    "preview": note.preview,
                    "save_state": note_save_state_id(note.save_state),
                    "revision": note.revision,
                    "icon_options": icon_options,
                    "color_options": color_options,
                }
            }),
            cx,
        );
    }

    fn hydrate_workbench_surface(&self, cx: &mut Context<Self>) {
        let rows = if self.file_explorer.open && self.file_explorer.mode == FileExplorerMode::Files
        {
            self.file_tree_rows()
                .into_iter()
                .map(|row| {
                    serde_json::json!({
                        "path": row.path.display().to_string(),
                        "label": row.label,
                        "depth": row.depth,
                        "hidden": row.hidden,
                        "expanded": row.expanded,
                        "selected": self.file_explorer.selected.as_ref() == Some(&row.path),
                        "kind": match row.kind {
                            FileTreeRowKind::Entry(FileEntryKind::Directory) => "directory",
                            FileTreeRowKind::Entry(FileEntryKind::File) => "file",
                            FileTreeRowKind::Entry(FileEntryKind::Symlink) => "symlink",
                            FileTreeRowKind::Loading => "loading",
                            FileTreeRowKind::Error => "error",
                        },
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let (changes_state, changes_error, changes) = match &self.file_explorer.changes {
            RepositoryChangesState::Idle => ("idle", None, Vec::new()),
            RepositoryChangesState::Loading => ("loading", None, Vec::new()),
            RepositoryChangesState::Error(error) => ("error", Some(error.clone()), Vec::new()),
            RepositoryChangesState::Ready(changes) => (
                "ready",
                None,
                changes
                    .iter()
                    .map(|change| serde_json::json!({
                        "relative_path": change.relative_path,
                        "previous_relative_path": change.previous_relative_path,
                        "kind": repository_change_kind_id(change.kind),
                        "selected": self.active_diff.as_ref().is_some_and(|document| document.change.relative_path == change.relative_path),
                    }))
                    .collect::<Vec<_>>(),
            ),
        };
        let editor = self.active_file.as_ref().map(|document| {
            let (source, workspace_id) = match document.source {
                FileDocumentSource::Repository => ("repository", None),
                FileDocumentSource::ProjectInstructions(id) => ("project-instructions", Some(id)),
                FileDocumentSource::ProjectTaskInstructions(id) => ("task-instructions", Some(id)),
            };
            let file_name = match document.source {
                FileDocumentSource::ProjectTaskInstructions(_) => self
                    .tr("CLAUDE.md for tasks", "CLAUDE.md de tareas")
                    .to_string(),
                _ => document
                    .path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("file")
                    .to_string(),
            };
            let relative_path = match document.source {
                FileDocumentSource::ProjectInstructions(_) => self
                    .tr(
                        "General project instructions · AGENTS.md links to this file",
                        "Instrucciones generales del proyecto · AGENTS.md enlaza este archivo",
                    )
                    .to_string(),
                FileDocumentSource::ProjectTaskInstructions(_) => self
                    .tr(
                        "Shared body copied after each task's generated header",
                        "Cuerpo compartido copiado después del encabezado generado de cada tarea",
                    )
                    .to_string(),
                FileDocumentSource::Repository => document
                    .path
                    .strip_prefix(&document.root)
                    .unwrap_or(&document.path)
                    .display()
                    .to_string(),
            };
            let (state, error) = match &document.load_state {
                FileDocumentLoadState::Loading | FileDocumentLoadState::Ready(_) => {
                    ("loading", None)
                }
                FileDocumentLoadState::Open => ("ready", None),
                FileDocumentLoadState::Error(error) => ("error", Some(error.clone())),
            };
            let content = document
                .editor
                .as_ref()
                .map(|editor| editor.read(cx).value().to_string())
                .or_else(|| match &document.load_state {
                    FileDocumentLoadState::Ready(content) => Some(content.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            serde_json::json!({
                "request_id": document.request_id,
                "state": state,
                "error": error,
                "file_name": file_name,
                "relative_path": relative_path,
                "content": content,
                "language": document.language.to_string(),
                "source": source,
                "workspace_id": workspace_id,
                "save_state": note_save_state_id(document.save_state),
                "revision": document.revision,
            })
        });
        let diff = self.active_diff.as_ref().map(|document| {
            let (original, modified) = match &document.load_state {
                FileDiffLoadState::Ready(diff) => (diff.original.as_deref(), diff.modified.as_deref()),
                _ => (None, None),
            };
            let file_name = document
                .change
                .path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("file");
            let (state, error, rows, truncated) = match &document.load_state {
                FileDiffLoadState::Loading => ("loading", None, Vec::new(), false),
                FileDiffLoadState::Error(error) => {
                    ("error", Some(error.clone()), Vec::new(), false)
                }
                FileDiffLoadState::Ready(diff) if diff.binary => {
                    ("binary", None, Vec::new(), diff.truncated)
                }
                FileDiffLoadState::Ready(diff) if diff.original.is_some() && diff.modified.is_some() => {
                    ("ready", None, Vec::new(), false)
                }
                FileDiffLoadState::Ready(diff) if diff.rows.is_empty() => {
                    ("empty", None, Vec::new(), diff.truncated)
                }
                FileDiffLoadState::Ready(diff) => (
                    "ready",
                    None,
                    diff.rows
                        .iter()
                        .map(repository_diff_row_json)
                        .collect::<Vec<_>>(),
                    diff.truncated,
                ),
            };
            serde_json::json!({
                "request_id": document.request_id,
                "state": state,
                "error": error,
                "file_name": file_name,
                "relative_path": document.change.relative_path,
                "change_kind": repository_change_kind_id(document.change.kind),
                "original": original,
                "modified": modified,
                "rows": rows,
                "truncated": truncated,
            })
        });
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "workbench",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "language": match self.session.language { Language::English => "en", Language::Spanish => "es" },
                    "theme": app_theme_id(self.session.theme),
                    "explorer": {
                        "open": self.file_explorer.open,
                        "root_label": self.file_explorer.root_label,
                        "root_path": self.file_explorer.root.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
                        "mode": if self.file_explorer.mode == FileExplorerMode::Changes { "changes" } else { "files" },
                        "rows": rows,
                        "changes": changes,
                        "changes_state": changes_state,
                        "changes_error": changes_error,
                    },
                    "editor": editor,
                    "diff": diff,
                }
            }),
            cx,
        );
    }

    fn hydrate_workspace_status(&self, cx: &mut Context<Self>) {
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_status",
                "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
                "message": self.status.as_ref().map(|(message, _)| message),
                "error": self.status.as_ref().is_some_and(|(_, error)| *error),
            }),
            cx,
        );
    }

    fn hydrate_unassigned_agent_surface(&self, cx: &mut Context<Self>) {
        let project = match self.active_orchestrator_scope {
            OrchestratorChatScope::Project(id)
            | OrchestratorChatScope::ProjectAgent { project_id: id, .. } => {
                self.workspaces.iter().find(|workspace| workspace.id == id)
            }
            _ => None,
        };
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "unassigned-agent",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "title": project.map(|workspace| workspace.label())
                        .unwrap_or(self.tr("No agent selected", "Ningún agente seleccionado")),
                    "description": if project.is_some() {
                        self.tr("This project has no assigned agents. Open a repository or add an agent from the sidebar.",
                            "Este proyecto no tiene agentes asignados. Abre un repositorio o agrega un agente desde la barra lateral.")
                    } else {
                        self.tr("Select an existing agent or add one from the sidebar.",
                            "Selecciona un agente existente o agrega uno desde la barra lateral.")
                    },
                },
            }),
            cx,
        );
    }

    fn hydrate_active_workspace_surface(&self, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.project_settings_workspace_id {
            self.hydrate_project_settings_surface(workspace_id, cx);
        } else if self.show_settings {
            self.hydrate_settings_surface(cx);
        } else if self.show_project_note {
            if let Some(workspace_id) = self.session.selected_workspace_id {
                self.hydrate_note_surface(NoteOwner::Project(workspace_id), cx);
            }
        } else if self.show_task_note {
            if let Some(task_id) = self.session.selected_task_id {
                self.hydrate_note_surface(NoteOwner::Task(task_id), cx);
            }
        } else if !self.show_terminal
            && (self.file_explorer.open || self.active_file.is_some() || self.active_diff.is_some())
        {
            self.hydrate_workbench_surface(cx);
        } else if !self.show_terminal
            && !self.orchestrator_chats.has_agent(self.active_orchestrator_scope)
        {
            self.hydrate_unassigned_agent_surface(cx);
        }
    }

    fn publish_workbench_surface(&self, cx: &mut Context<Self>) {
        if !self.show_settings
            && self.project_settings_workspace_id.is_none()
            && !self.show_project_note
            && !self.show_task_note
            && !self.show_terminal
            && (self.file_explorer.open || self.active_file.is_some() || self.active_diff.is_some())
        {
            self.hydrate_workbench_surface(cx);
        }
    }

    fn orchestrator_chat_copy(
        &self,
        scope: OrchestratorChatScope,
    ) -> (String, String, String, String) {
        let agent_name = self
            .orchestrator_chats
            .avatar_color(scope)
            .display_name()
            .to_string();
        match scope {
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => (
                agent_name.clone(),
                self.tr("BLACKHOLES AGENT", "AGENTE DE BLACKHOLES").into(),
                match self.session.language {
                    Language::English => {
                        format!("Tell {agent_name} which project to work on…")
                    }
                    Language::Spanish => {
                        format!("Dile a {agent_name} en qué proyecto trabajar…")
                    }
                },
                self.tr(
                    "Coordinates your projects and delegates substantial work to isolated agents.",
                    "Coordina tus proyectos y delega el trabajo pesado a agentes aislados.",
                )
                .into(),
            ),
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => {
                let project = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .map(Workspace::label)
                    .unwrap_or(self.tr("Project", "Proyecto"));
                (
                    agent_name,
                    format!(
                        "{} · {project}",
                        self.tr("PROJECT AGENT", "AGENTE DEL PROYECTO")
                    ),
                    self.tr(
                        "Ask about this project or assign work…",
                        "Pregunta sobre este proyecto o asigna trabajo…",
                    )
                    .into(),
                    format!(
                        "{} {project}. {}",
                        self.tr("Working in", "Trabajando en"),
                        self.tr(
                            "Substantial tasks run in an isolated worktree.",
                            "Las tareas pesadas se ejecutan en un worktree aislado."
                        )
                    ),
                )
            }
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => {
                let task = self
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .map(|task| task.title.as_str())
                    .unwrap_or(self.tr("Task", "Tarea"));
                (
                    agent_name,
                    format!("{} · {task}", self.tr("TASK AGENT", "AGENTE DE LA TAREA")),
                    self.tr(
                        "Continue working on this task…",
                        "Sigue trabajando en esta tarea…",
                    )
                    .into(),
                    format!(
                        "{} {task}. {}",
                        self.tr("Assigned to", "Asignado a"),
                        self.tr(
                            "Works directly inside its isolated worktree.",
                            "Trabaja directamente dentro de su worktree aislado."
                        )
                    ),
                )
            }
        }
    }

    fn start_new_orchestrator_chat(&mut self, cx: &mut Context<Self>) {
        let scope = self.active_orchestrator_scope;
        if !self.orchestrator_chats.has_agent(scope) {
            return;
        }
        if self.orchestrator_turns.contains_key(&scope) {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "error",
                    "id": Uuid::new_v4(),
                    "message": self.tr(
                        "Wait for Black Bot to finish before starting another conversation.",
                        "Espera a que Black Bot termine antes de iniciar otra conversación.",
                    ),
                }),
                cx,
            );
            return;
        }
        self.orchestrator_chats.reset(scope);
        self.persist_orchestrator_chats();
        self.dispatch_orchestrator_event(serde_json::json!({ "type": "new_chat" }), cx);
    }

    fn start_orchestrator_turn(
        &mut self,
        client_id: String,
        message: String,
        created_at: String,
        attachments: Vec<OrchestratorChatAttachment>,
        cx: &mut Context<Self>,
    ) {
        if self
            .orchestrator_turns
            .contains_key(&self.active_orchestrator_scope)
        {
            if self.redirect_orchestrator_turn(
                client_id.clone(),
                message.clone(),
                created_at.clone(),
                attachments.clone(),
                cx,
            ) {
                return;
            }
            self.queue_orchestrator_turn(client_id, message, created_at, attachments, cx);
            return;
        }
        self.start_orchestrator_turn_for_scope(
            self.active_orchestrator_scope,
            client_id,
            message,
            created_at,
            attachments,
            OrchestratorTurnStart::default(),
            false,
            cx,
        );
    }

    fn redirect_orchestrator_turn(
        &mut self,
        client_id: String,
        message: String,
        created_at: String,
        attachments: Vec<OrchestratorChatAttachment>,
        cx: &mut Context<Self>,
    ) -> bool {
        let scope = self.active_orchestrator_scope;
        let message = message.trim().to_string();
        let attachments = validated_orchestrator_attachments(attachments);
        if message.is_empty() && attachments.is_empty() {
            return true;
        }
        let provider = self
            .orchestrator_chats
            .chat(scope)
            .map(|chat| chat.provider)
            .unwrap_or_else(|| self.agent_provider());
        if provider != AgentProvider::Claude {
            return false;
        }
        let id = Uuid::parse_str(&client_id).unwrap_or_else(|_| Uuid::new_v4());
        let redirected = self
            .orchestrator_turns
            .get(&scope)
            .is_some_and(|turn| turn.cancel.steer(id, message.clone(), attachments.clone()));
        if !redirected {
            return false;
        }
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let response_id = Uuid::new_v4();
        let interrupted_fallback = self
            .tr(
                "Work interrupted to answer the new message.",
                "Trabajo interrumpido para responder el mensaje nuevo.",
            )
            .to_string();
        let (
            previous_user_id,
            previous_response_id,
            previous_content,
            mut previous_activities,
            previous_handoffs,
            previous_duration_ms,
        ) = {
            let turn = self
                .orchestrator_turns
                .get_mut(&scope)
                .expect("active turn");
            let previous = (
                turn.user_message_id,
                turn.response_id,
                std::mem::take(&mut turn.response_text),
                std::mem::take(&mut turn.activities),
                std::mem::take(&mut turn.handoffs),
                turn.duration_ms(),
            );
            turn.started_at = Some(Utc::now());
            turn.user_message_id = id;
            turn.response_id = response_id;
            previous
        };
        for activity in &mut previous_activities {
            if matches!(activity.status.as_deref(), Some("running" | "foreground")) {
                activity.status = Some("stopped".to_string());
                if activity.summary.as_deref().is_none_or(str::is_empty) {
                    activity.summary = Some(interrupted_fallback.clone());
                }
            }
        }
        self.insert_orchestrator_message_after(
            scope,
            previous_user_id,
            OrchestratorChatMessage {
                id: previous_response_id,
                role: OrchestratorChatRole::Assistant,
                content: if previous_content.trim().is_empty() {
                    interrupted_fallback.clone()
                } else {
                    previous_content
                },
                created_at: Utc::now(),
                attachments: Vec::new(),
                revision_group_id: None,
                activities: previous_activities,
                duration_ms: previous_duration_ms,
                handoffs: previous_handoffs,
                interrupted: true,
            },
        );
        self.insert_orchestrator_message_after(
            scope,
            previous_response_id,
            OrchestratorChatMessage {
                id,
                role: OrchestratorChatRole::User,
                duration_ms: None,
                content: message,
                created_at,
                attachments,
                revision_group_id: None,
                activities: Vec::new(),
                handoffs: Vec::new(),
                interrupted: false,
            },
        );
        self.persist_orchestrator_chats();
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "assistant_stopped",
                "id": previous_response_id,
                "duration_ms": previous_duration_ms,
                "fallback": interrupted_fallback,
                "label": self.tr("Interrupted", "Interrumpido"),
                "status": self.tr("Answering the new message…", "Respondiendo el mensaje nuevo…"),
            }),
            cx,
        );
        let agent_name = self
            .orchestrator_chats
            .avatar_color(scope)
            .display_name()
            .to_string();
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "assistant_start",
                "id": response_id,
                "after_id": id,
                "created_at": Utc::now(),
                "status": match self.session.language {
                    Language::English => format!("{agent_name} is answering…"),
                    Language::Spanish => format!("{agent_name} está respondiendo…"),
                },
            }),
            cx,
        );
        cx.notify();
        true
    }

    fn queue_orchestrator_turn(
        &mut self,
        client_id: String,
        message: String,
        created_at: String,
        attachments: Vec<OrchestratorChatAttachment>,
        cx: &mut Context<Self>,
    ) {
        let scope = self.active_orchestrator_scope;
        let message = message.trim().to_string();
        let attachments = validated_orchestrator_attachments(attachments);
        if message.is_empty() && attachments.is_empty() {
            return;
        }
        let id = Uuid::parse_str(&client_id).unwrap_or_else(|_| Uuid::new_v4());
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        self.orchestrator_chats
            .chat_mut(scope)
            .messages
            .push(OrchestratorChatMessage {
                id,
                role: OrchestratorChatRole::User,
                duration_ms: None,
                content: message.clone(),
                created_at: created_at.clone(),
                attachments: attachments.clone(),
                revision_group_id: None,
                activities: Vec::new(),
                handoffs: Vec::new(),
                interrupted: false,
            });
        self.persist_orchestrator_chats();
        self.pending_orchestrator_turns
            .entry(scope)
            .or_default()
            .push_back(PendingOrchestratorTurn {
                client_id: id.to_string(),
                message: message.clone(),
                created_at: created_at.to_rfc3339(),
                attachments,
                delegated: false,
                user_message_persisted: true,
            });
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "assistant_queued",
                "status": self.tr(
                    "Message queued · it will start when the current response finishes",
                    "Mensaje en cola · comenzará cuando termine la respuesta actual",
                ),
            }),
            cx,
        );
    }

    fn edit_orchestrator_message(
        &mut self,
        message_id: Uuid,
        client_id: String,
        message: String,
        created_at: String,
        attachments: Vec<OrchestratorChatAttachment>,
        cx: &mut Context<Self>,
    ) {
        let scope = self.active_orchestrator_scope;
        if !self.orchestrator_chats.has_agent(scope) {
            return;
        }
        if self.orchestrator_turns.contains_key(&scope) {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "composer_notice",
                    "message": self.tr(
                        "Wait for the current response before editing a message.",
                        "Espera la respuesta actual antes de editar un mensaje.",
                    ),
                }),
                cx,
            );
            return;
        }
        let message = message.trim().to_string();
        let attachments = validated_orchestrator_attachments(attachments);
        if message.is_empty() && attachments.is_empty() {
            return;
        }
        let edited_message_id = Uuid::parse_str(&client_id).unwrap_or_else(|_| Uuid::new_v4());
        let Some(fork) = self
            .orchestrator_chats
            .chat_mut(scope)
            .prepare_edit(message_id, edited_message_id)
        else {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "composer_notice",
                    "message": self.tr(
                        "The message to edit is no longer in this branch.",
                        "El mensaje que quieres editar ya no está en esta rama.",
                    ),
                }),
                cx,
            );
            return;
        };
        self.persist_orchestrator_chats();
        self.start_orchestrator_turn_for_scope(
            scope,
            edited_message_id.to_string(),
            message,
            created_at,
            attachments,
            OrchestratorTurnStart {
                revision_group_id: Some(fork.revision_group_id),
                source_session_id: fork.source_session_id,
                fork_at_user_turn: Some(fork.user_turn_index),
                user_message_persisted: false,
            },
            false,
            cx,
        );
        self.hydrate_orchestrator_chat(cx);
    }

    fn switch_orchestrator_branch(&mut self, branch_id: Uuid, cx: &mut Context<Self>) {
        let scope = self.active_orchestrator_scope;
        if !self.orchestrator_chats.has_agent(scope) {
            return;
        }
        if self.orchestrator_turns.contains_key(&scope) {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "composer_notice",
                    "message": self.tr(
                        "Wait for the current response before changing branches.",
                        "Espera la respuesta actual antes de cambiar de rama.",
                    ),
                }),
                cx,
            );
            return;
        }
        if self
            .orchestrator_chats
            .chat_mut(scope)
            .switch_branch(branch_id)
        {
            self.persist_orchestrator_chats();
            self.hydrate_orchestrator_chat(cx);
        }
    }

    fn start_orchestrator_turn_for_scope(
        &mut self,
        scope: OrchestratorChatScope,
        client_id: String,
        message: String,
        created_at: String,
        attachments: Vec<OrchestratorChatAttachment>,
        turn_start: OrchestratorTurnStart,
        delegated: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let message = message.trim().to_string();
        if !self.orchestrator_chats.has_agent(scope) {
            return false;
        }
        let attachments = validated_orchestrator_attachments(attachments);
        if message.is_empty() && attachments.is_empty() {
            return false;
        }
        if self.orchestrator_turns.contains_key(&scope) {
            if self.active_orchestrator_scope == scope {
                self.dispatch_orchestrator_event(
                    serde_json::json!({
                        "type": "error",
                        "id": Uuid::new_v4(),
                        "message": self.tr(
                            "Black Bot is already working on the previous message.",
                            "Black Bot todavía está trabajando en el mensaje anterior.",
                        ),
                    }),
                    cx,
                );
            }
            return false;
        }

        let Some((cwd, additional_directories, scope_context)) = self.orchestrator_runtime(scope)
        else {
            if self.active_orchestrator_scope == scope {
                self.dispatch_orchestrator_event(
                    serde_json::json!({
                        "type": "error",
                        "id": Uuid::new_v4(),
                        "message": self.tr(
                            "This project or task no longer exists.",
                            "Este proyecto o esta tarea ya no existe.",
                        ),
                    }),
                    cx,
                );
            }
            return false;
        };

        let id = Uuid::parse_str(&client_id).unwrap_or_else(|_| Uuid::new_v4());
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let provider = self.agent_provider();
        let auth_mode = self.agent_auth_mode(provider);
        let agent_name = self
            .orchestrator_chats
            .avatar_color(scope)
            .display_name()
            .to_string();
        let (session_id, history) = {
            let chat = self.orchestrator_chats.chat_mut(scope);
            chat.prepare_runtime(provider, auth_mode);
            chat.cwd = Some(cwd.clone());
            let session_id =
                if provider != AgentProvider::Claude && turn_start.fork_at_user_turn.is_some() {
                    None
                } else {
                    turn_start
                        .source_session_id
                        .clone()
                        .or_else(|| chat.session_id.clone())
                };
            let history = if session_id.is_none() {
                let history_end = if turn_start.user_message_persisted {
                    chat.messages
                        .iter()
                        .position(|history_message| history_message.id == id)
                        .unwrap_or(chat.messages.len())
                } else {
                    chat.messages.len()
                };
                chat.messages
                    .iter()
                    .take(history_end)
                    .filter(|history_message| !history_message.content.trim().is_empty())
                    .map(|message| AgentHistoryMessage {
                        role: message.role,
                        content: message.content.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if !turn_start.user_message_persisted {
                chat.messages.push(OrchestratorChatMessage {
                    id,
                    role: OrchestratorChatRole::User,
                    duration_ms: None,
                    content: message.clone(),
                    created_at,
                    attachments: attachments.clone(),
                    revision_group_id: turn_start.revision_group_id,
                    activities: Vec::new(),
                    handoffs: Vec::new(),
                    interrupted: false,
                });
            }
            (session_id, history)
        };
        self.persist_orchestrator_chats();

        let runtime_id = Uuid::new_v4();
        let response_id = Uuid::new_v4();
        self.orchestrator_turns.insert(
            scope,
            OrchestratorTurn {
                started_at: Some(Utc::now()),
                runtime_id,
                user_message_id: id,
                response_id,
                response_text: String::new(),
                activities: Vec::new(),
                handoffs: Vec::new(),
                cancel: OrchestratorTurnCancellation::default(),
                delegated,
                notification_sent: false,
            },
        );
        cx.notify();
        if self.active_orchestrator_scope == scope {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "assistant_start",
                    "id": response_id,
                    "after_id": id,
                    "created_at": Utc::now(),
                    "status": match self.session.language {
                        Language::English => format!("{agent_name} is working…"),
                        Language::Spanish => format!("{agent_name} está trabajando…"),
                    },
                }),
                cx,
            );
        }

        let language = match self.session.language {
            Language::English => "en",
            Language::Spanish => "es",
        };
        let model = self.agent_model(provider);
        let effort = self.agent_effort(provider);
        let full_access = self.agents_full_access();
        let enabled_skills = self.enabled_agent_skills_for_scope(scope);
        let available_mcp_servers = self.available_agent_mcp_names_for_scope(scope);
        let enabled_mcp_servers = self.enabled_agent_mcp_names_for_scope(scope);
        let configured_mcp_servers = self.configured_agent_mcps_for_scope(scope);
        let stream = match stream_agent_turn(
            provider,
            auth_mode,
            self.paths.agent_profiles.join(provider.id()),
            agent_name,
            id,
            message,
            history,
            attachments,
            session_id,
            turn_start.fork_at_user_turn,
            model,
            effort,
            full_access,
            enabled_skills,
            available_mcp_servers,
            enabled_mcp_servers,
            configured_mcp_servers,
            self.paths.agent_skills_plugin.clone(),
            cwd,
            additional_directories,
            language,
            scope_context,
        ) {
            Ok(stream) => stream,
            Err(error) => {
                self.fail_orchestrator_turn(scope, format!("{error:#}"), cx);
                return false;
            }
        };
        let events = stream.events;
        if let Some(turn) = self.orchestrator_turns.get_mut(&scope) {
            turn.cancel.set(stream.cancel, stream.control);
        }

        cx.spawn(async move |this, cx| {
            let mut reached_terminal_event = false;
            while let Ok(event) = events.recv_async().await {
                let terminal = match this.update(cx, |app, cx| {
                    app.handle_agent_runtime_event(scope, runtime_id, event, cx)
                }) {
                    Ok(terminal) => terminal,
                    Err(_) => break,
                };
                if terminal {
                    reached_terminal_event = true;
                    break;
                }
            }
            if !reached_terminal_event {
                let _ = this.update(cx, |app, cx| {
                    let turn_is_still_active = app
                        .orchestrator_turns
                        .get(&scope)
                        .is_some_and(|turn| turn.runtime_id == runtime_id);
                    if turn_is_still_active {
                        let message = app
                            .tr(
                                "The agent runtime stopped without returning a result.",
                                "El motor del agente se detuvo sin devolver un resultado.",
                            )
                            .to_string();
                        app.fail_orchestrator_turn(scope, message, cx);
                    }
                });
            }
        })
        .detach();
        true
    }

    fn handle_agent_runtime_event(
        &mut self,
        scope: OrchestratorChatScope,
        runtime_id: Uuid,
        event: AgentRuntimeEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((response_id, user_message_id)) =
            self.orchestrator_turns.get(&scope).and_then(|turn| {
                (turn.runtime_id == runtime_id).then_some((turn.response_id, turn.user_message_id))
            })
        else {
            return false;
        };
        let event_prompt_id = match &event {
            AgentRuntimeEvent::Delta { prompt_id, .. }
            | AgentRuntimeEvent::Tool { prompt_id, .. }
            | AgentRuntimeEvent::BackgroundTask { prompt_id, .. }
            | AgentRuntimeEvent::Done { prompt_id, .. } => *prompt_id,
            _ => None,
        };
        if event_prompt_id.is_some_and(|prompt_id| prompt_id != user_message_id) {
            return false;
        }
        let terminal = matches!(
            &event,
            AgentRuntimeEvent::Done { .. } | AgentRuntimeEvent::Error { .. }
        );
        let provider = self
            .orchestrator_chats
            .chat(scope)
            .map(|chat| chat.provider)
            .unwrap_or_default();
        match event {
            AgentRuntimeEvent::Started { .. } => {}
            AgentRuntimeEvent::Session { session_id } => {
                self.orchestrator_chats.chat_mut(scope).session_id = Some(session_id);
                self.persist_orchestrator_chats();
            }
            AgentRuntimeEvent::Delta { text, .. } => {
                if let Some(turn) = self.orchestrator_turns.get_mut(&scope) {
                    turn.response_text.push_str(&text);
                }
                if self.active_orchestrator_scope == scope {
                    self.dispatch_orchestrator_event(
                        serde_json::json!({
                            "type": "assistant_delta",
                            "id": response_id,
                            "text": text,
                        }),
                        cx,
                    );
                }
            }
            AgentRuntimeEvent::Tool {
                name, agent, input, ..
            } => {
                let agent_name = self.orchestrator_chats.avatar_color(scope).display_name();
                let activity =
                    orchestrator_tool_activity(&name, agent.as_deref(), input.as_ref(), agent_name);
                if let Some(turn) = self.orchestrator_turns.get_mut(&scope) {
                    turn.activities.push(activity.clone());
                }
                if self.active_orchestrator_scope == scope {
                    let status = match name.as_str() {
                        "Agent" => self.tr("Working on the task…", "Trabajando en la tarea…"),
                        "Bash" => self.tr("Running a command…", "Ejecutando un comando…"),
                        "Read" => self.tr("Reading files…", "Leyendo archivos…"),
                        "Write" | "Edit" | "NotebookEdit" => {
                            self.tr("Updating files…", "Actualizando archivos…")
                        }
                        "Glob" | "Grep" => {
                            self.tr("Searching the project…", "Buscando en el proyecto…")
                        }
                        "Skill" => self.tr("Applying a skill…", "Aplicando una skill…"),
                        "WebSearch" | "WebFetch" => {
                            self.tr("Searching the web…", "Buscando en la web…")
                        }
                        "ToolSearch" => self.tr("Preparing tools…", "Preparando herramientas…"),
                        name if name.starts_with("mcp__blackholes__") => {
                            self.tr("Consulting Blackholes…", "Consultando Blackholes…")
                        }
                        _ => agent_name,
                    };
                    self.dispatch_orchestrator_event(
                        serde_json::json!({
                            "type": "assistant_tool",
                            "id": response_id,
                            "activity": activity,
                        }),
                        cx,
                    );
                    self.dispatch_orchestrator_event(
                        serde_json::json!({
                            "type": "assistant_status",
                            "id": response_id,
                            "status": status,
                        }),
                        cx,
                    );
                }
            }
            AgentRuntimeEvent::BackgroundTask {
                task_id,
                status,
                description,
                task_type,
                summary,
                output_file,
                ambient,
                ..
            } => {
                if ambient || task_id.trim().is_empty() {
                    return false;
                }
                let status = match status.as_str() {
                    "running" | "foreground" | "completed" | "failed" | "stopped" | "blocked" => {
                        status
                    }
                    _ => "running".to_string(),
                };
                let tool = match task_type.as_str() {
                    "local_bash" => "Shell",
                    "local_agent" => "Agent",
                    _ => "Process",
                }
                .to_string();
                let detail = (!description.trim().is_empty()).then(|| {
                    let flattened = description.replace(['\r', '\n'], " ");
                    let truncated = flattened.chars().take(280).collect::<String>();
                    if flattened.chars().count() > 280 {
                        format!("{truncated}…")
                    } else {
                        truncated
                    }
                });
                let summary = if !summary.trim().is_empty() {
                    Some(summary)
                } else if !output_file.trim().is_empty() {
                    Some(output_file)
                } else {
                    None
                };
                let agent_name = self
                    .orchestrator_chats
                    .avatar_color(scope)
                    .display_name()
                    .to_string();
                let activity = OrchestratorChatActivity {
                    agent: agent_name,
                    tool,
                    detail,
                    created_at: Utc::now(),
                    task_id: Some(task_id.clone()),
                    status: Some(status),
                    summary,
                    background: true,
                };
                let Some(turn) = self.orchestrator_turns.get_mut(&scope) else {
                    return false;
                };
                let activity = if let Some(existing) = turn
                    .activities
                    .iter_mut()
                    .find(|candidate| candidate.task_id.as_deref() == Some(task_id.as_str()))
                {
                    if activity.detail.is_some() {
                        existing.detail = activity.detail;
                    }
                    if activity.summary.is_some() {
                        existing.summary = activity.summary;
                    }
                    if activity.tool != "Process" || existing.tool.is_empty() {
                        existing.tool = activity.tool;
                    }
                    existing.status = activity.status;
                    existing.created_at = activity.created_at;
                    existing.clone()
                } else {
                    turn.activities.push(activity.clone());
                    activity
                };
                if self.active_orchestrator_scope == scope {
                    self.dispatch_orchestrator_event(
                        serde_json::json!({
                            "type": "assistant_background_task",
                            "id": response_id,
                            "activity": activity,
                        }),
                        cx,
                    );
                }
            }
            AgentRuntimeEvent::Diagnostic { message } => {
                tracing::debug!(diagnostic = %message.trim(), provider = provider.id(), "agent runtime");
            }
            AgentRuntimeEvent::Done {
                session_id,
                result,
                error,
                is_error,
                turn_usage,
                plan_usage,
                ..
            } => {
                if let Some(session_id) = session_id {
                    self.orchestrator_chats.chat_mut(scope).session_id = Some(session_id);
                }
                self.orchestrator_chats.record_provider_usage(provider, turn_usage);
                // Account limits are queried separately for the active account.
                let _ = plan_usage;
                if is_error {
                    let message = error
                        .filter(|error| !error.trim().is_empty())
                        .or_else(|| result.filter(|result| !result.trim().is_empty()))
                        .unwrap_or_else(|| {
                            self.tr(
                                "The agent could not complete this request.",
                                "El agente no pudo completar esta solicitud.",
                            )
                            .to_string()
                        });
                    self.fail_orchestrator_turn(scope, message, cx);
                    return true;
                }
                let response_is_empty = self
                    .orchestrator_turns
                    .get(&scope)
                    .is_none_or(|turn| turn.response_text.is_empty());
                if response_is_empty
                    && let Some(result) = result.filter(|result| !result.trim().is_empty())
                {
                    if let Some(turn) = self.orchestrator_turns.get_mut(&scope) {
                        turn.response_text.push_str(&result);
                    }
                    if self.active_orchestrator_scope == scope {
                        self.dispatch_orchestrator_event(
                            serde_json::json!({
                                "type": "assistant_delta",
                                "id": response_id,
                                "text": result,
                            }),
                            cx,
                        );
                    }
                }
                let response_is_still_empty = self
                    .orchestrator_turns
                    .get(&scope)
                    .is_none_or(|turn| turn.response_text.trim().is_empty());
                if response_is_still_empty {
                    let message = self
                        .tr(
                            "The agent ended the turn without returning an answer. Blackholes did not replace it with a generic success message; please retry the request.",
                            "El agente terminó el turno sin devolver una respuesta. Blackholes no la reemplazó por un éxito genérico; vuelve a intentar la solicitud.",
                        )
                        .to_string();
                    self.fail_orchestrator_turn(scope, message, cx);
                    return true;
                }
                self.finish_orchestrator_turn(scope, response_id, cx);
            }
            AgentRuntimeEvent::Error { message } => self.fail_orchestrator_turn(scope, message, cx),
        }
        terminal
    }

    fn stop_orchestrator_turn(&mut self, cx: &mut Context<Self>) {
        let scope = self.active_orchestrator_scope;
        let Some(mut turn) = self.orchestrator_turns.remove(&scope) else {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "composer_notice",
                    "message": self.tr(
                        "This agent is not running.",
                        "Este agente no está trabajando.",
                    ),
                }),
                cx,
            );
            return;
        };

        turn.cancel.cancel();
        let response_id = turn.response_id;
        let duration_ms = turn.duration_ms();
        let content = if turn.response_text.trim().is_empty() {
            self.tr("Response stopped.", "Respuesta detenida.")
                .to_string()
        } else {
            turn.response_text
        };
        self.insert_orchestrator_message_after(
            scope,
            turn.user_message_id,
            OrchestratorChatMessage {
                id: response_id,
                role: OrchestratorChatRole::Assistant,
                content,
                created_at: Utc::now(),
                attachments: Vec::new(),
                revision_group_id: None,
                activities: turn.activities,
                handoffs: turn.handoffs,
                interrupted: true,
                duration_ms,
            },
        );
        self.persist_orchestrator_chats();
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "assistant_stopped",
                "id": response_id,
                "duration_ms": duration_ms,
                "fallback": self.tr("Response stopped.", "Respuesta detenida."),
                "label": self.tr("Stopped", "Detenido"),
                "status": self.tr("Agent stopped", "Agente detenido"),
            }),
            cx,
        );
        cx.notify();
        self.start_next_pending_orchestrator_turn(scope, cx);
    }

    fn finish_orchestrator_turn(
        &mut self,
        scope: OrchestratorChatScope,
        response_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let mut turn = self.orchestrator_turns.remove(&scope).unwrap_or_default();
        turn.cancel.disarm();
        let duration_ms = turn.duration_ms();
        let should_notify_completion = matches!(
            scope,
            OrchestratorChatScope::Task(_) | OrchestratorChatScope::TaskAgent { .. }
        ) && turn.delegated
            && !turn.notification_sent;
        let content = turn.response_text;
        self.insert_orchestrator_message_after(
            scope,
            turn.user_message_id,
            OrchestratorChatMessage {
                id: response_id,
                role: OrchestratorChatRole::Assistant,
                content,
                created_at: Utc::now(),
                attachments: Vec::new(),
                revision_group_id: None,
                activities: turn.activities,
                handoffs: turn.handoffs,
                interrupted: false,
                duration_ms,
            },
        );
        self.persist_orchestrator_chats();
        if self.active_orchestrator_scope == scope {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "assistant_done",
                    "id": response_id,
                    "duration_ms": duration_ms,
                }),
                cx,
            );
        }
        if should_notify_completion {
            self.announce_finished_agent(scope, cx);
        }
        cx.notify();
        self.start_next_pending_orchestrator_turn(scope, cx);
    }

    fn fail_orchestrator_turn(
        &mut self,
        scope: OrchestratorChatScope,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let mut turn = self.orchestrator_turns.remove(&scope).unwrap_or_default();
        turn.cancel.cancel();
        let duration_ms = turn.duration_ms();
        let response_id = if turn.response_id.is_nil() {
            Uuid::new_v4()
        } else {
            turn.response_id
        };
        let streamed_message_is_error =
            !turn.response_text.trim().is_empty() && turn.response_text.trim() == message.trim();
        if !turn.response_text.trim().is_empty() && !streamed_message_is_error {
            self.insert_orchestrator_message_after(
                scope,
                turn.user_message_id,
                OrchestratorChatMessage {
                    id: response_id,
                    role: OrchestratorChatRole::Assistant,
                    content: turn.response_text.clone(),
                    created_at: Utc::now(),
                    attachments: Vec::new(),
                    revision_group_id: None,
                    activities: turn.activities.clone(),
                    handoffs: turn.handoffs.clone(),
                    interrupted: false,
                    duration_ms,
                },
            );
        }
        let response_exists = self.orchestrator_chats.chat(scope).is_some_and(|chat| {
            chat.messages
                .iter()
                .any(|chat_message| chat_message.id == response_id)
        });
        let error_id = if response_exists {
            Uuid::new_v4()
        } else {
            response_id
        };
        self.insert_orchestrator_message_after(
            scope,
            if response_exists {
                response_id
            } else {
                turn.user_message_id
            },
            OrchestratorChatMessage {
                id: error_id,
                duration_ms: if response_exists { None } else { duration_ms },
                role: OrchestratorChatRole::Assistant,
                content: message.clone(),
                created_at: Utc::now(),
                attachments: Vec::new(),
                revision_group_id: None,
                activities: if response_exists {
                    Vec::new()
                } else {
                    turn.activities
                },
                handoffs: if response_exists {
                    Vec::new()
                } else {
                    turn.handoffs
                },
                interrupted: false,
            },
        );
        self.persist_orchestrator_chats();
        if self.active_orchestrator_scope == scope {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "error",
                    "id": error_id,
                    "response_id": response_id,
                    "message": message,
                    "duration_ms": duration_ms,
                }),
                cx,
            );
        }
        cx.notify();
        self.start_next_pending_orchestrator_turn(scope, cx);
    }

    fn start_next_pending_orchestrator_turn(
        &mut self,
        scope: OrchestratorChatScope,
        cx: &mut Context<Self>,
    ) {
        let pending = self
            .pending_orchestrator_turns
            .get_mut(&scope)
            .and_then(VecDeque::pop_front);
        if self
            .pending_orchestrator_turns
            .get(&scope)
            .is_some_and(VecDeque::is_empty)
        {
            self.pending_orchestrator_turns.remove(&scope);
        }
        if let Some(pending) = pending {
            self.start_orchestrator_turn_for_scope(
                scope,
                pending.client_id,
                pending.message,
                pending.created_at,
                pending.attachments,
                OrchestratorTurnStart {
                    user_message_persisted: pending.user_message_persisted,
                    ..OrchestratorTurnStart::default()
                },
                pending.delegated,
                cx,
            );
        }
    }

    fn persist_orchestrator_chats(&self) {
        if let Err(error) = self.orchestrator_chats.save(&self.paths.orchestrator_chat) {
            tracing::warn!(?error, "failed to persist agent chats");
        }
    }

    fn insert_orchestrator_message_after(
        &mut self,
        scope: OrchestratorChatScope,
        after_id: Uuid,
        message: OrchestratorChatMessage,
    ) {
        let messages = &mut self.orchestrator_chats.chat_mut(scope).messages;
        if !after_id.is_nil()
            && let Some(index) = messages.iter().position(|current| current.id == after_id)
        {
            messages.insert(index + 1, message);
        } else {
            messages.push(message);
        }
    }

    fn orchestrator_runtime(
        &self,
        scope: OrchestratorChatScope,
    ) -> Option<(PathBuf, Vec<PathBuf>, OrchestratorScopeContext)> {
        let mut directories = HashSet::new();
        let (cwd, context) = match scope {
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => {
                directories.extend([
                    self.paths.default_projects.clone(),
                    self.paths.task_workspaces.clone(),
                ]);
                for workspace in &self.workspaces {
                    directories.extend(workspace.root_path.iter().cloned());
                    directories.extend(
                        workspace
                            .repositories
                            .iter()
                            .map(|repository| repository.path.clone()),
                    );
                }
                for task in &self.tasks {
                    directories.insert(task.worktree_root_path.clone());
                    directories.extend(
                        task.repositories
                            .iter()
                            .map(|repository| repository.worktree_path.clone()),
                    );
                }
                let cwd = self
                    .paths
                    .default_projects
                    .is_dir()
                    .then(|| self.paths.default_projects.clone())
                    .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
                (
                    cwd,
                    OrchestratorScopeContext {
                        kind: "global",
                        name: "Blackholes".into(),
                        agent_id: match scope {
                            OrchestratorChatScope::GlobalAgent(agent_id) => Some(agent_id),
                            _ => None,
                        },
                        global_agent_id: match scope {
                            OrchestratorChatScope::GlobalAgent(agent_id) => Some(agent_id),
                            _ => None,
                        },
                        project_id: None,
                        project_name: None,
                        task_id: None,
                    },
                )
            }
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => {
                let workspace = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)?;
                directories.extend(workspace.root_path.iter().cloned());
                directories.extend(
                    workspace
                        .repositories
                        .iter()
                        .map(|repository| repository.path.clone()),
                );
                for task in self
                    .tasks
                    .iter()
                    .filter(|task| task.workspace_id == workspace_id)
                {
                    directories.insert(task.worktree_root_path.clone());
                    directories.extend(
                        task.repositories
                            .iter()
                            .map(|repository| repository.worktree_path.clone()),
                    );
                }
                let cwd = workspace.root_path.clone().or_else(|| {
                    workspace
                        .repositories
                        .first()
                        .map(|repository| repository.path.clone())
                })?;
                (
                    cwd,
                    OrchestratorScopeContext {
                        kind: "project",
                        name: workspace.label().to_string(),
                        agent_id: match scope {
                            OrchestratorChatScope::ProjectAgent { agent_id, .. } => Some(agent_id),
                            _ => None,
                        },
                        global_agent_id: None,
                        project_id: Some(workspace_id),
                        project_name: Some(workspace.label().to_string()),
                        task_id: None,
                    },
                )
            }
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => {
                let task = self.tasks.iter().find(|task| task.id == task_id)?;
                let workspace = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == task.workspace_id)?;
                directories.insert(task.worktree_root_path.clone());
                directories.extend(
                    task.repositories
                        .iter()
                        .map(|repository| repository.worktree_path.clone()),
                );
                (
                    task.worktree_root_path.clone(),
                    OrchestratorScopeContext {
                        kind: "task",
                        name: task.title.clone(),
                        agent_id: match scope {
                            OrchestratorChatScope::TaskAgent { agent_id, .. } => Some(agent_id),
                            _ => None,
                        },
                        global_agent_id: None,
                        project_id: Some(workspace.id),
                        project_name: Some(workspace.label().to_string()),
                        task_id: Some(task.id),
                    },
                )
            }
        };
        directories.retain(|path| path.is_dir());
        Some((cwd, directories.into_iter().collect(), context))
    }

    fn show_orchestrator_chat(&mut self, scope: OrchestratorChatScope, cx: &mut Context<Self>) {
        if !self.orchestrator_chats.has_agent(scope) {
            return;
        }
        match scope {
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => {}
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => {
                if !self
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.id == workspace_id)
                {
                    return;
                }
                self.session.selected_workspace_id = Some(workspace_id);
                self.session.selected_task_id = None;
                self.session.selected_repository_id = None;
                insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
                self.request_workspace_git_summaries(workspace_id, cx);
                self.persist_session();
            }
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => {
                let Some(workspace_id) = self
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .map(|task| task.workspace_id)
                else {
                    return;
                };
                self.mark_task_seen(task_id, cx);
                self.session.selected_workspace_id = Some(workspace_id);
                self.session.selected_task_id = Some(task_id);
                self.session.selected_repository_id = None;
                insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
                insert_unique(&mut self.session.expanded_task_ids, task_id);
                self.request_task_git_summaries(task_id, cx);
                self.persist_session();
            }
        }
        self.flush_active_file(cx);
        self.active_orchestrator_scope = scope;
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.active_file = None;
        self.active_diff = None;
        self.quick_open = None;
        self.close_file_explorer(cx);
        self.hydrate_orchestrator_chat(cx);
        self.refresh_model_catalog(false, cx);
        cx.notify();
    }

    fn assign_orchestrator_agent(&mut self, scope: OrchestratorChatScope, cx: &mut Context<Self>) {
        self.orchestrator_chats.assign_agent(scope);
        self.persist_orchestrator_chats();
        self.show_orchestrator_chat(scope, cx);
        self.hydrate_navigation(cx);
        self.set_status(self.tr("Bot added", "Bot agregado"), false, cx);
    }

    fn create_global_orchestrator_agent(&mut self, cx: &mut Context<Self>) {
        let agent_id = self.orchestrator_chats.create_global_agent();
        self.persist_orchestrator_chats();
        self.show_orchestrator_chat(OrchestratorChatScope::GlobalAgent(agent_id), cx);
        self.set_status(
            self.tr("New Black Bot created", "Nuevo Black Bot creado"),
            false,
            cx,
        );
    }

    fn first_navigation_agent(&self) -> Option<OrchestratorChatScope> {
        for workspace in &self.workspaces {
            let project_scope = OrchestratorChatScope::Project(workspace.id);
            if self.orchestrator_chats.has_agent(project_scope) {
                return Some(project_scope);
            }
            if let Some(agent_id) = self
                .orchestrator_chats
                .project_agent_ids(workspace.id)
                .first()
                .copied()
            {
                return Some(OrchestratorChatScope::ProjectAgent {
                    project_id: workspace.id,
                    agent_id,
                });
            }
            for task in self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace.id)
            {
                let task_scope = OrchestratorChatScope::Task(task.id);
                if self.orchestrator_chats.has_agent(task_scope) {
                    return Some(task_scope);
                }
                if let Some(agent_id) = self
                    .orchestrator_chats
                    .task_agent_ids(task.id)
                    .first()
                    .copied()
                {
                    return Some(OrchestratorChatScope::TaskAgent {
                        task_id: task.id,
                        agent_id,
                    });
                }
            }
        }
        if self
            .orchestrator_chats
            .has_agent(OrchestratorChatScope::Global)
        {
            return Some(OrchestratorChatScope::Global);
        }
        self.orchestrator_chats
            .global_agent_ids()
            .first()
            .copied()
            .map(OrchestratorChatScope::GlobalAgent)
    }

    fn has_navigation_agents(&self) -> bool {
        self.first_navigation_agent().is_some()
    }

    fn schedule_default_global_agent(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            Timer::after(Duration::from_secs(2)).await;
            let _ = weak.update(cx, |app, cx| {
                if app.has_navigation_agents() {
                    return;
                }
                let scope = OrchestratorChatScope::Global;
                app.orchestrator_chats.restore_default_global_agent();
                app.arriving_orchestrator_agents.insert(scope);
                app.persist_orchestrator_chats();
                app.hydrate_navigation(cx);
                if app.active_orchestrator_scope == scope {
                    app.hydrate_orchestrator_chat(cx);
                }
                cx.notify();

                let weak = cx.weak_entity();
                cx.spawn(async move |_, cx| {
                    Timer::after(Duration::from_millis(1_100)).await;
                    let _ = weak.update(cx, |app, cx| {
                        if app.arriving_orchestrator_agents.remove(&scope) {
                            app.hydrate_navigation(cx);
                            cx.notify();
                        }
                    });
                })
                .detach();
            });
        })
        .detach();
    }

    fn create_scoped_orchestrator_agent(
        &mut self,
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        let scope = if let Some(task_id) = task_id {
            let belongs_to_project = self
                .tasks
                .iter()
                .any(|task| task.id == task_id && task.workspace_id == workspace_id);
            if !belongs_to_project {
                return;
            }
            let agent_id = self.orchestrator_chats.create_task_agent(task_id);
            OrchestratorChatScope::TaskAgent { task_id, agent_id }
        } else {
            if !self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == workspace_id)
            {
                return;
            }
            let agent_id = self.orchestrator_chats.create_project_agent(workspace_id);
            OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                agent_id,
            }
        };
        self.persist_orchestrator_chats();
        self.show_orchestrator_chat(scope, cx);
        self.hydrate_navigation(cx);
        self.set_status(self.tr("New bot added", "Nuevo bot agregado"), false, cx);
    }

    fn open_remove_orchestrator_agent_confirmation(
        &mut self,
        scope: OrchestratorChatScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.orchestrator_chats.has_agent(scope) {
            return;
        }
        if self.orchestrator_turns.contains_key(&scope)
            || self
                .pending_orchestrator_turns
                .get(&scope)
                .is_some_and(|pending| !pending.is_empty())
        {
            self.set_status(
                self.tr(
                    "Wait for Black Bot to finish before removing this agent.",
                    "Espera a que Black Bot termine antes de eliminar este agente.",
                ),
                true,
                cx,
            );
            return;
        }

        let language = self.session.language;
        let agent_name = self
            .orchestrator_chats
            .avatar_color(scope)
            .display_name()
            .to_string();
        let context = match scope {
            OrchestratorChatScope::Project(project_id)
            | OrchestratorChatScope::ProjectAgent { project_id, .. } => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == project_id)
                .map(|workspace| match language {
                    Language::English => format!("Project · {}", workspace.label()),
                    Language::Spanish => format!("Proyecto · {}", workspace.label()),
                }),
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| match language {
                    Language::English => format!("Task · {}", task.title),
                    Language::Spanish => format!("Tarea · {}", task.title),
                }),
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => None,
        };
        let title = match language {
            Language::English => "Remove this agent?",
            Language::Spanish => "¿Eliminar este agente?",
        };
        let description = match language {
            Language::English => {
                "The agent and its conversation history will be removed from Blackholes."
            }
            Language::Spanish => {
                "El agente y su historial de conversación se eliminarán de Blackholes."
            }
        };
        let remove_label = match language {
            Language::English => "Remove agent",
            Language::Spanish => "Eliminar agente",
        };

        if !self.show_terminal && self.orchestrator_webview.is_some() {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "app_modal",
                    "modal": {
                        "kind": "remove_agent",
                        "scope": navigation_scope_id(scope),
                        "title": title,
                        "name": agent_name,
                        "context": context,
                        "description": description,
                        "confirm_label": remove_label,
                        "cancel_label": match language {
                            Language::English => "Cancel",
                            Language::Spanish => "Cancelar",
                        },
                        "offset_x": -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0),
                    }
                }),
                cx,
            );
            self.dispatch_navigation_event(
                serde_json::json!({ "type": "modal_visibility", "visible": true }),
                cx,
            );
            return;
        }

        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_submit = weak.clone();
            let mut details = v_flex().gap_2().child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(agent_name.clone()),
            );
            if let Some(context) = context.clone() {
                details = details.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(0x8e97aa))
                        .child(context),
                );
            }
            details = details.child(description);
            dialog
                .title(title)
                .w(px(420.))
                .child(details)
                .button_props(DialogButtonProps::default().ok_text(remove_label))
                .confirm()
                .on_ok(move |_, _, cx| {
                    weak_submit
                        .update(cx, |app, cx| {
                            app.remove_orchestrator_agent(scope, cx);
                            true
                        })
                        .unwrap_or(false)
                })
        });
    }

    fn remove_orchestrator_agent(&mut self, scope: OrchestratorChatScope, cx: &mut Context<Self>) {
        if self.orchestrator_turns.contains_key(&scope)
            || self
                .pending_orchestrator_turns
                .get(&scope)
                .is_some_and(|pending| !pending.is_empty())
        {
            self.set_status(
                self.tr(
                    "Wait for Black Bot to finish before removing this agent.",
                    "Espera a que Black Bot termine antes de eliminar este agente.",
                ),
                true,
                cx,
            );
            return;
        }
        self.orchestrator_chats.remove_agent(scope);
        self.app_toasts.retain(|toast| {
            !matches!(toast.target, AppToastTarget::Agent { scope: current } if current == scope)
        });
        if self.active_orchestrator_scope == scope {
            self.active_orchestrator_scope = self
                .first_navigation_agent()
                .unwrap_or(OrchestratorChatScope::Global);
            self.hydrate_orchestrator_chat(cx);
        }
        self.persist_orchestrator_chats();
        self.hydrate_navigation(cx);
        if !self.has_navigation_agents() {
            self.schedule_default_global_agent(cx);
        }
        self.set_status(self.tr("Agent removed", "Agente eliminado"), false, cx);
    }

    fn handle_agent_handoff(&mut self, payload: AgentHandoffPayload, cx: &mut Context<Self>) -> Result<bool> {
        if payload.prompt.trim().is_empty() { anyhow::bail!("The handoff needs an implementation brief"); }
        self.reload_external_data(cx);
        let scope = match payload.scope.as_str() {
            "project" => {
                let Some(project_id) = payload.project_id else {
                    anyhow::bail!("The handoff destination does not exist");
                };
                if !self
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.id == project_id)
                {
                    anyhow::bail!("The handoff destination does not exist");
                }
                OrchestratorChatScope::Project(project_id)
            }
            "task" => {
                let Some(task_id) = payload.task_id else {
                    anyhow::bail!("The handoff destination does not exist");
                };
                if !self.tasks.iter().any(|task| task.id == task_id) {
                    anyhow::bail!("The handoff destination does not exist");
                }
                OrchestratorChatScope::Task(task_id)
            }
            _ => anyhow::bail!("Unsupported handoff destination"),
        };

        let source_scope = match payload.source_scope.as_deref() {
            Some("global") => Some(
                payload
                    .source_agent_id.or(payload.source_global_agent_id)
                    .map(OrchestratorChatScope::GlobalAgent)
                    .unwrap_or(OrchestratorChatScope::Global),
            ),
            Some("project") => payload.source_project_id.map(|project_id| {
                payload.source_agent_id
                    .map(|agent_id| OrchestratorChatScope::ProjectAgent { project_id, agent_id })
                    .unwrap_or(OrchestratorChatScope::Project(project_id))
            }),
            Some("task") => payload.source_task_id.map(|task_id| {
                payload.source_agent_id
                    .map(|agent_id| OrchestratorChatScope::TaskAgent { task_id, agent_id })
                    .unwrap_or(OrchestratorChatScope::Task(task_id))
            }),
            _ => match scope {
                OrchestratorChatScope::Project(_) | OrchestratorChatScope::ProjectAgent { .. } => {
                    Some(OrchestratorChatScope::Global)
                }
                OrchestratorChatScope::Task(_) | OrchestratorChatScope::TaskAgent { .. } => {
                    payload.project_id.map(OrchestratorChatScope::Project)
                }
                OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => None,
            },
        };
        let destination_identity = self.orchestrator_chats.avatar_color(scope);
        let destination_agent = destination_identity.display_name();
        let handoff_label = match scope {
            OrchestratorChatScope::Project(project_id)
            | OrchestratorChatScope::ProjectAgent { project_id, .. } => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == project_id)
                .map(|workspace| format!("{destination_agent} · {}", workspace.label()))
                .unwrap_or_else(|| destination_agent.to_string()),
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| format!("{destination_agent} · {}", task.title))
                .unwrap_or_else(|| destination_agent.to_string()),
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => anyhow::bail!("Unsupported handoff destination"),
        };
        if source_scope == Some(scope) { anyhow::bail!("This agent already owns the destination; implement here instead of delegating to itself"); }

        self.orchestrator_chats.assign_agent(scope);
        self.persist_orchestrator_chats();
        self.hydrate_navigation(cx);
        let pending = PendingOrchestratorTurn {
            client_id: Uuid::new_v4().to_string(),
            message: payload.prompt.clone(),
            created_at: Utc::now().to_rfc3339(),
            attachments: Vec::new(),
            delegated: true,
            user_message_persisted: false,
        };
        let queued = self.orchestrator_turns.contains_key(&scope);
        if queued {
            self.pending_orchestrator_turns
                .entry(scope)
                .or_default()
                .push_back(pending);
        } else if !self.start_orchestrator_turn_for_scope(
                scope,
                pending.client_id,
                pending.message,
                pending.created_at,
                pending.attachments,
                OrchestratorTurnStart::default(),
                pending.delegated,
                cx,
            ) {
            anyhow::bail!("The destination agent could not start. Check its conversation for the runtime error");
        }

        if let Some(source_scope) = source_scope.filter(|source| *source != scope) {
            self.record_orchestrator_handoff(
                source_scope,
                OrchestratorChatHandoff {
                    scope: payload.scope.clone(),
                    project_id: payload.project_id,
                    task_id: payload.task_id,
                    label: handoff_label,
                    identity: destination_identity,
                    navigation: false,
                },
                cx,
            );
        }

        let target = AppToastTarget::Agent { scope };
        self.app_toasts.retain(|toast| toast.target != target);
        let default_title = if queued {
            self.tr("Work queued for Black Bot", "Trabajo en cola para Black Bot")
        } else { match scope {
            OrchestratorChatScope::Project(_) | OrchestratorChatScope::ProjectAgent { .. } => self
                .tr(
                    "Project Black Bot working",
                    "Black Bot del proyecto trabajando",
                ),
            OrchestratorChatScope::Task(_) | OrchestratorChatScope::TaskAgent { .. } => {
                self.tr("Task created with Black Bot", "Tarea creada con Black Bot")
            }
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => anyhow::bail!("Unsupported handoff destination"),
        }};
        let default_message = self.tr(
            "Click to follow the delegated work.",
            "Haz clic para seguir el trabajo delegado.",
        );
        self.app_toasts.push(AppToast {
            target,
            title: payload.title.unwrap_or_else(|| default_title.to_string()),
            message: payload
                .message
                .unwrap_or_else(|| default_message.to_string()),
        });
        cx.notify();
        Ok(!queued)
    }

    fn handle_navigation_link(&mut self, payload: NavigationLinkPayload, cx: &mut Context<Self>) {
        self.reload_external_data(cx);
        let target_exists = match payload.scope.as_str() {
            "project" => payload.project_id.is_some_and(|project_id| {
                self.workspaces
                    .iter()
                    .any(|workspace| workspace.id == project_id)
            }),
            "task" => payload
                .task_id
                .is_some_and(|task_id| self.tasks.iter().any(|task| task.id == task_id)),
            _ => false,
        };
        if !target_exists {
            return;
        }
        let source_scope = match payload.source_scope.as_deref() {
            Some("global") => Some(
                payload
                    .source_agent_id.or(payload.source_global_agent_id)
                    .map(OrchestratorChatScope::GlobalAgent)
                    .unwrap_or(OrchestratorChatScope::Global),
            ),
            Some("project") => payload.source_project_id.map(|project_id| {
                payload.source_agent_id
                    .map(|agent_id| OrchestratorChatScope::ProjectAgent { project_id, agent_id })
                    .unwrap_or(OrchestratorChatScope::Project(project_id))
            }),
            Some("task") => payload.source_task_id.map(|task_id| {
                payload.source_agent_id
                    .map(|agent_id| OrchestratorChatScope::TaskAgent { task_id, agent_id })
                    .unwrap_or(OrchestratorChatScope::Task(task_id))
            }),
            _ => None,
        };
        let Some(source_scope) = source_scope else {
            return;
        };
        let identity = match payload.scope.as_str() {
            "project" => payload
                .project_id
                .map(OrchestratorChatScope::Project)
                .map(|scope| self.orchestrator_chats.avatar_color(scope))
                .unwrap_or_default(),
            "task" => payload
                .task_id
                .map(OrchestratorChatScope::Task)
                .map(|scope| self.orchestrator_chats.avatar_color(scope))
                .unwrap_or_default(),
            _ => AgentAvatarColor::default(),
        };
        self.record_orchestrator_handoff(
            source_scope,
            OrchestratorChatHandoff {
                scope: payload.scope,
                project_id: payload.project_id,
                task_id: payload.task_id,
                label: payload.label,
                identity,
                navigation: true,
            },
            cx,
        );
    }

    fn record_orchestrator_handoff(
        &mut self,
        source_scope: OrchestratorChatScope,
        handoff: OrchestratorChatHandoff,
        cx: &mut Context<Self>,
    ) {
        if !self.orchestrator_chats.has_agent(source_scope) {
            return;
        }
        let same_target = |existing: &OrchestratorChatHandoff| {
            existing.navigation == handoff.navigation
                && existing.scope == handoff.scope
                && existing.project_id == handoff.project_id
                && existing.task_id == handoff.task_id
        };
        let response_id = if let Some(turn) = self.orchestrator_turns.get_mut(&source_scope) {
            if turn.handoffs.iter().any(same_target) {
                return;
            }
            turn.handoffs.push(handoff.clone());
            Some(turn.response_id)
        } else {
            self.orchestrator_chats
                .chat_mut(source_scope)
                .messages
                .iter_mut()
                .rev()
                .find(|message| matches!(message.role, OrchestratorChatRole::Assistant))
                .filter(|message| !message.handoffs.iter().any(same_target))
                .map(|message| {
                    message.handoffs.push(handoff.clone());
                    message.id
                })
        };
        self.persist_orchestrator_chats();
        if self.active_orchestrator_scope == source_scope
            && let Some(response_id) = response_id
        {
            self.dispatch_orchestrator_event(
                serde_json::json!({
                    "type": "assistant_handoff",
                    "id": response_id,
                    "handoff": handoff,
                }),
                cx,
            );
        }
    }

    fn announce_finished_agent(&mut self, scope: OrchestratorChatScope, cx: &mut Context<Self>) {
        let task_id = match scope {
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => task_id,
            _ => return,
        };
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        let title = self
            .tr(
                "Task Black Bot finished",
                "El Black Bot de la tarea terminó",
            )
            .to_string();
        let message = task.title.clone();
        self.app_toasts
            .retain(|toast| toast.target.task_id() != Some(task_id));
        self.app_toasts.push(AppToast {
            target: AppToastTarget::Agent { scope },
            title: title.clone(),
            message: message.clone(),
        });
        play_agent_attention_sound();
        cx.background_executor()
            .spawn(async move {
                show_native_agent_notification(&title, &message);
            })
            .detach();
    }

    fn open_agent_from_toast(&mut self, scope: OrchestratorChatScope, cx: &mut Context<Self>) {
        self.dismiss_app_toast(AppToastTarget::Agent { scope }, cx);
        self.show_orchestrator_chat(scope, cx);
    }

    fn orchestrator_surface_visible(&self) -> bool {
        self.orchestrator_chats.has_agent(self.active_orchestrator_scope)
            && !self.show_settings
            && self.project_settings_workspace_id.is_none()
            && !self.show_project_note
            && !self.show_task_note
            && !self.show_terminal
            && !self.file_explorer.open
            && self.active_file.is_none()
            && self.active_diff.is_none()
            && self.quick_open.is_none()
    }

    fn orchestrator_chat_preview(&self, scope: OrchestratorChatScope) -> String {
        if self.orchestrator_turns.contains_key(&scope) {
            return self.tr("Working…", "Trabajando…").to_string();
        }
        let preview = self
            .orchestrator_chats
            .chat(scope)
            .and_then(|chat| chat.messages.last())
            .map(|message| {
                message
                    .content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|message| !message.is_empty());
        preview.unwrap_or_else(|| {
            self.tr("Where should we start today?", "¿Por dónde arrancamos hoy?")
                .to_string()
        })
    }

    fn selected_repository_target(&self) -> Option<(PathBuf, String)> {
        let workspace = self.selected_workspace()?;
        let repository_id = self.session.selected_repository_id?;
        let repository = workspace
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)?;
        let path = if let Some(task) = self.selected_task() {
            task.repositories
                .iter()
                .find(|task_repository| task_repository.repository_id == repository_id)?
                .worktree_path
                .clone()
        } else {
            repository.path.clone()
        };
        Some((path, repository.name.clone()))
    }

    fn request_workspace_git_summaries(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let paths = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| {
                workspace
                    .repositories
                    .iter()
                    .map(|repository| repository.path.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.request_repository_git_summaries(paths, cx);
    }

    fn request_task_git_summaries(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        let paths = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| {
                task.repositories
                    .iter()
                    .map(|repository| repository.worktree_path.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.request_repository_git_summaries(paths, cx);
    }

    fn refresh_expanded_repository_git_summaries(&mut self, cx: &mut Context<Self>) {
        let workspace_ids = self.session.expanded_workspace_ids.clone();
        let task_ids = self.session.expanded_task_ids.clone();
        for workspace_id in workspace_ids {
            self.request_workspace_git_summaries(workspace_id, cx);
        }
        for task_id in task_ids {
            self.request_task_git_summaries(task_id, cx);
        }
    }

    fn request_repository_git_summaries(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let mut pending = Vec::new();
        for path in paths {
            if !path.is_dir() {
                continue;
            }
            if self.repository_git_requests.insert(path.clone()) {
                pending.push(path);
            } else {
                self.repository_git_refresh_pending.insert(path);
            }
        }
        if pending.is_empty() {
            return;
        }

        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            for path in pending {
                let background_path = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { repository_git_summary(&background_path) })
                    .await;
                if weak
                    .update(cx, |app, cx| {
                        app.finish_repository_git_summaries(vec![(path, result)], cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_repository_git_summaries(
        &mut self,
        results: Vec<(PathBuf, Result<RepositoryGitSummary>)>,
        cx: &mut Context<Self>,
    ) {
        let mut refresh_again = Vec::new();
        for (path, result) in results {
            self.repository_git_requests.remove(&path);
            if let Ok(summary) = result {
                self.repository_git_summaries.insert(path.clone(), summary);
            }
            if self.repository_git_refresh_pending.remove(&path) {
                refresh_again.push(path);
            }
        }
        self.request_repository_git_summaries(refresh_again, cx);
        cx.notify();
    }

    fn repository_git_details(
        &self,
        path: &PathBuf,
        fallback_branch: Option<&str>,
    ) -> (Option<String>, u64, u64, bool) {
        let summary = self.repository_git_summaries.get(path);
        let branch = summary
            .and_then(|summary| summary.branch.clone())
            .or_else(|| fallback_branch.map(str::to_string));
        let additions = summary.map_or(0, |summary| summary.additions);
        let deletions = summary.map_or(0, |summary| summary.deletions);
        let loading = summary.is_none()
            && (self.repository_git_requests.contains(path)
                || self.repository_git_save_requests.contains_key(path));
        (branch, additions, deletions, loading)
    }

    fn refresh_saved_repository_git_summary(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.next_repository_git_save_request_id =
            self.next_repository_git_save_request_id.wrapping_add(1);
        let request_id = self.next_repository_git_save_request_id;
        self.repository_git_save_requests
            .insert(path.clone(), request_id);

        // If this repository is also part of the startup/global batch, make
        // that batch schedule a final pass instead of letting an older result
        // become the last value shown in the sidebar.
        if self.repository_git_requests.contains(&path) {
            self.repository_git_refresh_pending.insert(path.clone());
        }

        let background_path = path.clone();
        let background = cx
            .background_executor()
            .spawn(async move { repository_git_summary(&background_path) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                if app.repository_git_save_requests.get(&path) != Some(&request_id) {
                    return;
                }
                app.repository_git_save_requests.remove(&path);
                if let Ok(summary) = result {
                    app.repository_git_summaries.insert(path, summary);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn refresh_terminal_repository_git_summary(
        &mut self,
        terminal_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(descriptor) = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()
        else {
            return;
        };
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == descriptor.workspace_id)
        else {
            return;
        };

        let paths = match (descriptor.task_id, descriptor.repository_id) {
            (Some(task_id), Some(repository_id)) => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| {
                    task.repositories
                        .iter()
                        .find(|repository| repository.repository_id == repository_id)
                })
                .map(|repository| vec![repository.worktree_path.clone()])
                .unwrap_or_default(),
            (Some(task_id), None) => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| {
                    task.repositories
                        .iter()
                        .map(|repository| repository.worktree_path.clone())
                        .collect()
                })
                .unwrap_or_default(),
            (None, Some(repository_id)) => workspace
                .repositories
                .iter()
                .find(|repository| repository.id == repository_id)
                .map(|repository| vec![repository.path.clone()])
                .unwrap_or_default(),
            (None, None) => workspace
                .repositories
                .iter()
                .map(|repository| repository.path.clone())
                .collect(),
        };
        self.request_repository_git_summaries(paths, cx);
    }

    fn open_navigation_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.next_quick_open_id = self.next_quick_open_id.wrapping_add(1);
        let id = self.next_quick_open_id;
        let placeholder = self
            .tr("Search projects and tasks…", "Buscar proyectos y tareas…")
            .to_string();
        let input_placeholder = placeholder.clone();
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(input_placeholder));
        let entries = self.navigation_quick_open_items();
        self.quick_open = Some(QuickOpenState {
            id,
            mode: QuickOpenMode::Navigation,
            placeholder,
            query: query.clone(),
            entries: QuickOpenEntries::Ready(entries),
            selected: 0,
        });
        self.subscribe_quick_open_query(id, &query, cx);
        self.show_quick_open_overlay(window, cx);
        cx.notify();
    }

    fn open_file_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, root_label)) = self.selected_repository_target() else {
            let message = self
                .tr(
                    "Select a repository before opening file search",
                    "Selecciona un repositorio antes de buscar archivos",
                )
                .to_string();
            self.set_status(message, true, cx);
            return;
        };

        self.next_quick_open_id = self.next_quick_open_id.wrapping_add(1);
        let id = self.next_quick_open_id;
        let placeholder = format!(
            "{} {}…",
            self.tr("Search files in", "Buscar archivos en"),
            root_label
        );
        let input_placeholder = placeholder.clone();
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(input_placeholder));
        self.quick_open = Some(QuickOpenState {
            id,
            mode: QuickOpenMode::Files,
            placeholder,
            query: query.clone(),
            entries: QuickOpenEntries::Loading,
            selected: 0,
        });
        self.subscribe_quick_open_query(id, &query, cx);
        self.show_quick_open_overlay(window, cx);

        let indexed_root = root.clone();
        let background = cx
            .background_executor()
            .spawn(async move { index_repository_files(&indexed_root) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_palette_index(id, root, root_label, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn subscribe_quick_open_query(
        &mut self,
        id: u64,
        query: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(query, move |app, _, event: &InputEvent, cx| {
            if app.quick_open.as_ref().map(|state| state.id) != Some(id) {
                return;
            }
            match event {
                InputEvent::Change => {
                    if let Some(state) = app.quick_open.as_mut() {
                        state.selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => app.activate_quick_open_selected(cx),
                InputEvent::Focus | InputEvent::Blur => {}
            }
        })
        .detach();
    }

    fn show_quick_open_overlay(&self, _window: &mut Window, cx: &mut Context<Self>) {
        self.hydrate_quick_open_overlay(cx);
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": true }),
            cx,
        );
    }

    fn hydrate_quick_open_overlay(&self, cx: &mut Context<Self>) {
        let Some(state) = self.quick_open.as_ref() else {
            return;
        };
        let results = self.quick_open_results(cx);
        let (status, error) = match &state.entries {
            QuickOpenEntries::Loading => (
                Some(
                    self.tr(
                        "Indexing repository files…",
                        "Indexando archivos del repositorio…",
                    )
                    .to_string(),
                ),
                false,
            ),
            QuickOpenEntries::Error(error) => (Some(error.clone()), true),
            QuickOpenEntries::Ready(_) if results.is_empty() => (
                Some(
                    self.tr("No matching results", "No hay resultados")
                        .to_string(),
                ),
                false,
            ),
            QuickOpenEntries::Ready(_) => (None, false),
        };
        let (shortcut, footer_label) = match state.mode {
            QuickOpenMode::Navigation => (
                "⌘O",
                self.tr(
                    "Projects and tasks · Notes open directly",
                    "Proyectos y tareas · Las notas se abren directamente",
                ),
            ),
            QuickOpenMode::Files => (
                "⌘P",
                self.tr(
                    "Files from the selected repository",
                    "Archivos del repositorio seleccionado",
                ),
            ),
        };
        let query = state.query.read(cx).value().to_string();
        let results = results
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "title": item.title,
                    "subtitle": item.subtitle,
                    "kind_label": item.kind_label,
                    "icon": quick_open_icon_id(item.icon),
                    "color": item.color_css,
                })
            })
            .collect::<Vec<_>>();
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "quick_open",
                "open_id": state.id,
                "query": query,
                "placeholder": state.placeholder.clone(),
                "shortcut": shortcut,
                "footer_label": footer_label,
                "navigation_label": self.tr(
                    "↑↓ Navigate   ↵ Open   esc Close",
                    "↑↓ Navegar   ↵ Abrir   esc Cerrar",
                ),
                "status": status,
                "error": error,
                "results": results,
            }),
            cx,
        );
    }

    fn close_quick_open(&mut self, cx: &mut Context<Self>) {
        self.quick_open = None;
        self.dispatch_orchestrator_event(serde_json::json!({ "type": "quick_open_close" }), cx);
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": false }),
            cx,
        );
        cx.notify();
    }

    fn navigation_quick_open_items(&self) -> Vec<QuickOpenItem> {
        let project_kind = self.tr("Project", "Proyecto").to_string();
        let task_kind = self.tr("Task", "Tarea").to_string();
        let mut items = Vec::with_capacity(self.workspaces.len() + self.tasks.len());

        for workspace in &self.workspaces {
            let task_count = self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace.id)
                .count();
            let subtitle = format!(
                "{} · {} {} · {} {}",
                project_kind,
                workspace.repositories.len(),
                self.tr("repositories", "repositorios"),
                task_count,
                self.tr("tasks", "tareas"),
            );
            let title = workspace.label().to_string();
            items.push(QuickOpenItem {
                search_key: format!("{title} {subtitle}").to_ascii_lowercase(),
                title,
                subtitle,
                kind_label: project_kind.clone(),
                icon: project_icon_kind(&workspace.icon),
                color: workspace_color(workspace.color),
                color_css: workspace_color_css(workspace.color).to_string(),
                target: QuickOpenTarget::Project {
                    workspace_id: workspace.id,
                },
            });
        }

        for task in &self.tasks {
            let workspace_label = self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == task.workspace_id)
                .map(|workspace| workspace.label().to_string())
                .unwrap_or_default();
            let subtitle = format!("{} · {}", task_kind, workspace_label);
            items.push(QuickOpenItem {
                search_key: format!("{} {subtitle}", task.title).to_ascii_lowercase(),
                title: task.title.clone(),
                subtitle,
                kind_label: task_kind.clone(),
                icon: project_icon_kind(&task.icon),
                color: workspace_color(task.color),
                color_css: workspace_color_css(task.color).to_string(),
                target: QuickOpenTarget::Task {
                    workspace_id: task.workspace_id,
                    task_id: task.id,
                },
            });
        }
        items
    }

    fn finish_file_palette_index(
        &mut self,
        id: u64,
        root: PathBuf,
        root_label: String,
        result: Result<Vec<IndexedRepositoryFile>>,
        cx: &mut Context<Self>,
    ) {
        let file_kind = self.tr("File", "Archivo").to_string();
        let Some(state) = self.quick_open.as_mut() else {
            return;
        };
        if state.id != id || state.mode != QuickOpenMode::Files {
            return;
        }

        state.entries = match result {
            Ok(files) => QuickOpenEntries::Ready(
                files
                    .into_iter()
                    .map(|file| {
                        let title = PathBuf::from(&file.relative_path)
                            .file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .unwrap_or(&file.relative_path)
                            .to_string();
                        let subtitle = file.relative_path.clone();
                        let (icon, color) = file_tree_icon(FileEntryKind::File, &file.path, false);
                        QuickOpenItem {
                            search_key: file.relative_path.to_ascii_lowercase(),
                            title,
                            subtitle,
                            kind_label: file_kind.clone(),
                            icon,
                            color,
                            color_css: quick_open_css_color(color),
                            target: QuickOpenTarget::File {
                                root: root.clone(),
                                root_label: root_label.clone(),
                                path: file.path,
                            },
                        }
                    })
                    .collect(),
            ),
            Err(error) => QuickOpenEntries::Error(format!("{error:#}")),
        };
        state.selected = 0;
        self.hydrate_quick_open_overlay(cx);
        cx.notify();
    }

    fn quick_open_results(&self, cx: &App) -> Vec<QuickOpenItem> {
        let Some(state) = self.quick_open.as_ref() else {
            return Vec::new();
        };
        let QuickOpenEntries::Ready(entries) = &state.entries else {
            return Vec::new();
        };
        let query = state.query.read(cx).value().trim().to_ascii_lowercase();
        let mut matches = entries
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                quick_open_score(&query, &item.search_key).map(|score| (score, index, item))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        matches
            .into_iter()
            .take(QUICK_OPEN_RESULT_LIMIT)
            .map(|(_, _, item)| item.clone())
            .collect()
    }

    fn move_quick_open_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let result_count = self.quick_open_results(cx).len();
        let Some(state) = self.quick_open.as_mut() else {
            return;
        };
        if result_count == 0 {
            state.selected = 0;
            return;
        }
        state.selected = state
            .selected
            .saturating_add_signed(offset)
            .min(result_count - 1);
        cx.notify();
    }

    fn handle_quick_open_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "up" | "arrowup" => {
                cx.stop_propagation();
                self.move_quick_open_selection(-1, cx);
            }
            "down" | "arrowdown" => {
                cx.stop_propagation();
                self.move_quick_open_selection(1, cx);
            }
            "escape" => {
                cx.stop_propagation();
                self.close_quick_open(cx);
            }
            _ => {}
        }
    }

    fn activate_quick_open_selected(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .quick_open
            .as_ref()
            .map(|state| state.selected)
            .unwrap_or(0);
        let Some(target) = self
            .quick_open_results(cx)
            .get(selected)
            .map(|item| item.target.clone())
        else {
            return;
        };
        self.activate_quick_open_target(target, cx);
    }

    fn activate_quick_open_target(&mut self, target: QuickOpenTarget, cx: &mut Context<Self>) {
        self.close_quick_open(cx);
        match target {
            QuickOpenTarget::Project { workspace_id } => self.show_project_notes(workspace_id, cx),
            QuickOpenTarget::Task {
                workspace_id,
                task_id,
            } => {
                self.show_task_notes_for(workspace_id, task_id, cx);
                if let Some(row_index) = self.sidebar_task_row_index(task_id) {
                    self.sidebar_scroll.scroll_to_item(row_index);
                }
            }
            QuickOpenTarget::File {
                root,
                root_label,
                path,
            } => {
                self.file_explorer.mode = FileExplorerMode::Files;
                self.open_file_explorer(root, root_label, cx);
                self.open_file_in_editor(path, cx);
            }
        }
    }

    fn sync_file_explorer_to_selection(&mut self, cx: &mut Context<Self>) {
        let Some((root, label)) = self.selected_repository_target() else {
            return;
        };
        self.open_file_explorer(root, label, cx);
    }

    fn open_file_explorer(&mut self, root: PathBuf, root_label: String, cx: &mut Context<Self>) {
        let changed_root = self.file_explorer.root.as_ref() != Some(&root);
        let was_open = self.file_explorer.open;
        self.file_explorer.open = true;
        self.file_explorer.root_label = root_label;

        if changed_root {
            self.flush_active_file(cx);
            self.active_file = None;
            self.active_diff = None;
            self.file_explorer.root = Some(root.clone());
            self.file_explorer.expanded.clear();
            self.file_explorer.directories.clear();
            self.file_explorer.requests.clear();
            self.file_explorer.selected = None;
            self.file_explorer.changes = RepositoryChangesState::Idle;
            self.file_explorer.changes_request_id =
                self.file_explorer.changes_request_id.wrapping_add(1);
            self.file_explorer.changes_request_in_flight = false;
            self.file_explorer.changes_refresh_pending = false;
        }

        if changed_root || !was_open {
            if let Err(error) = self.install_file_watcher(&root, cx) {
                self.file_watcher = None;
                self.status = Some((
                    format!("File explorer live updates are unavailable; use refresh: {error:#}"),
                    true,
                ));
            }
        }

        if changed_root || !self.file_explorer.directories.contains_key(&root) {
            self.request_directory(root, cx);
        }
        if self.file_explorer.mode == FileExplorerMode::Changes {
            self.request_repository_changes(cx);
        }
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn close_file_explorer(&mut self, cx: &mut Context<Self>) {
        self.file_watcher = None;
        self.file_explorer = FileExplorerState::default();
        self.active_diff = None;
        cx.notify();
    }

    fn set_file_explorer_mode(&mut self, mode: FileExplorerMode, cx: &mut Context<Self>) {
        if self.file_explorer.mode == mode {
            if mode == FileExplorerMode::Files && self.active_diff.take().is_some() {
                self.publish_workbench_surface(cx);
                cx.notify();
            }
            return;
        }
        if mode == FileExplorerMode::Changes {
            self.flush_active_file(cx);
        } else {
            self.active_diff = None;
        }
        self.file_explorer.mode = mode;
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        if mode == FileExplorerMode::Changes {
            self.request_repository_changes(cx);
        } else if let Some(root) = self.file_explorer.root.clone() {
            self.refresh_file_explorer_if_root(&root, cx);
        }
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn request_repository_changes(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else {
            return;
        };
        if self.file_explorer.changes_request_in_flight {
            self.file_explorer.changes_refresh_pending = true;
            return;
        }
        self.file_explorer.changes_request_in_flight = true;
        self.file_explorer.changes_request_id =
            self.file_explorer.changes_request_id.wrapping_add(1);
        let request_id = self.file_explorer.changes_request_id;
        let show_loading = matches!(
            &self.file_explorer.changes,
            RepositoryChangesState::Idle | RepositoryChangesState::Error(_)
        );
        if show_loading {
            self.file_explorer.changes = RepositoryChangesState::Loading;
            self.publish_workbench_surface(cx);
            cx.notify();
        }
        let read_root = root.clone();
        let background = cx
            .background_executor()
            .spawn(async move { repository_changes(&read_root) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_repository_changes_request(root, request_id, result, cx)
            });
        })
        .detach();
    }

    fn finish_repository_changes_request(
        &mut self,
        root: PathBuf,
        request_id: u64,
        result: Result<Vec<RepositoryChange>>,
        cx: &mut Context<Self>,
    ) {
        if self.file_explorer.root.as_ref() != Some(&root)
            || self.file_explorer.changes_request_id != request_id
        {
            return;
        }
        self.file_explorer.changes_request_in_flight = false;
        let state_changed = match result {
            Ok(changes) => {
                let unchanged = matches!(
                    &self.file_explorer.changes,
                    RepositoryChangesState::Ready(current)
                        if current.as_ref() == changes.as_slice()
                );
                if !unchanged {
                    self.file_explorer.changes = RepositoryChangesState::Ready(changes.into());
                }
                !unchanged
            }
            Err(error) => {
                let message = format!("{error:#}");
                let unchanged = matches!(
                    &self.file_explorer.changes,
                    RepositoryChangesState::Error(current) if current == &message
                );
                if !unchanged {
                    self.file_explorer.changes = RepositoryChangesState::Error(message);
                }
                !unchanged
            }
        };
        let refresh_again = std::mem::take(&mut self.file_explorer.changes_refresh_pending);
        if state_changed {
            self.publish_workbench_surface(cx);
            cx.notify();
        }
        if refresh_again && self.file_explorer.mode == FileExplorerMode::Changes {
            self.request_repository_changes(cx);
        }
    }

    fn open_repository_diff(&mut self, change: RepositoryChange, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else {
            return;
        };
        if let Some(active_diff) = self.active_diff.as_mut().filter(|active_diff| {
            active_diff.root == root
                && active_diff.change.path == change.path
                && active_diff.request_in_flight
        }) {
            active_diff.change = change;
            active_diff.refresh_pending = true;
            return;
        }
        self.next_file_diff_request_id = self.next_file_diff_request_id.wrapping_add(1);
        let request_id = self.next_file_diff_request_id;
        self.file_explorer.selected = Some(change.path.clone());
        let refreshing_current = self.active_diff.as_ref().is_some_and(|active_diff| {
            active_diff.root == root && active_diff.change.path == change.path
        });
        if refreshing_current {
            if let Some(active_diff) = self.active_diff.as_mut() {
                active_diff.change = change.clone();
                active_diff.request_id = request_id;
                active_diff.request_in_flight = true;
            }
        } else {
            self.active_diff = Some(FileDiffHandle {
                root: root.clone(),
                change: change.clone(),
                load_state: FileDiffLoadState::Loading,
                request_id,
                request_in_flight: true,
                refresh_pending: false,
            });
        }
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;

        let read_root = root.clone();
        let background = cx
            .background_executor()
            .spawn(async move { repository_file_diff(&read_root, &change) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_repository_diff_request(root, request_id, result, cx)
            });
        })
        .detach();
        if !refreshing_current {
            self.publish_workbench_surface(cx);
            cx.notify();
        }
    }

    fn finish_repository_diff_request(
        &mut self,
        root: PathBuf,
        request_id: u64,
        result: Result<RepositoryFileDiff>,
        cx: &mut Context<Self>,
    ) {
        let Some(diff) = self.active_diff.as_mut() else {
            return;
        };
        if diff.root != root || diff.request_id != request_id {
            return;
        }
        diff.request_in_flight = false;
        let state_changed = match result {
            Ok(result) => {
                let unchanged = matches!(
                    &diff.load_state,
                    FileDiffLoadState::Ready(current) if current == &result
                );
                if !unchanged {
                    diff.load_state = FileDiffLoadState::Ready(result);
                }
                !unchanged
            }
            Err(error) => {
                let message = format!("{error:#}");
                let unchanged = matches!(
                    &diff.load_state,
                    FileDiffLoadState::Error(current) if current == &message
                );
                if !unchanged {
                    diff.load_state = FileDiffLoadState::Error(message);
                }
                !unchanged
            }
        };
        let refresh_again = std::mem::take(&mut diff.refresh_pending);
        if state_changed {
            self.publish_workbench_surface(cx);
            cx.notify();
        }
        if refresh_again {
            self.refresh_active_repository_diff(cx);
        }
    }

    fn refresh_active_repository_diff(&mut self, cx: &mut Context<Self>) {
        let change = self.active_diff.as_ref().map(|diff| diff.change.clone());
        if let Some(change) = change {
            self.open_repository_diff(change, cx);
        }
    }

    fn close_repository_diff(&mut self, cx: &mut Context<Self>) {
        self.active_diff = None;
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn install_file_watcher(&mut self, root: &PathBuf, cx: &mut Context<Self>) -> Result<()> {
        self.file_watcher = None;
        let (sender, receiver) = flume::bounded::<()>(1);
        let changed_paths = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        let callback_paths = changed_paths.clone();
        let callback_root = root.clone();
        let callback_git_directory = callback_root.join(".git");
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if !matches!(
                    event.kind,
                    EventKind::Create(_)
                        | EventKind::Remove(_)
                        | EventKind::Modify(ModifyKind::Data(_))
                        | EventKind::Modify(ModifyKind::Name(_))
                        | EventKind::Any
                        | EventKind::Other
                ) {
                    return;
                }

                let mut paths = callback_paths.lock();
                let mut relevant_change = false;
                for path in event.paths {
                    if path == callback_git_directory || path.starts_with(&callback_git_directory) {
                        continue;
                    }
                    relevant_change |= paths.insert(path);
                    if paths.len() >= FILE_WATCH_BATCH_PATH_LIMIT {
                        paths.clear();
                        paths.insert(callback_root.clone());
                        relevant_change = true;
                        break;
                    }
                }
                drop(paths);
                if !relevant_change {
                    return;
                }
                // A capacity-one signal coalesces write bursts from agents and build tools.
                let _ = sender.try_send(());
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        self.file_watcher = Some(watcher);

        let watched_root = root.clone();
        cx.spawn(async move |this, cx| {
            while receiver.recv_async().await.is_ok() {
                Timer::after(Duration::from_millis(350)).await;
                while receiver.try_recv().is_ok() {}
                let paths = changed_paths.lock().drain().collect::<Vec<PathBuf>>();
                if this
                    .update(cx, |app, cx| {
                        app.refresh_changed_file_directories(&watched_root, paths, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }

    fn refresh_changed_file_directories(
        &mut self,
        root: &PathBuf,
        changed_paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if !self.file_explorer.open || self.file_explorer.root.as_ref() != Some(root) {
            return;
        }

        if self.file_explorer.mode == FileExplorerMode::Changes {
            self.request_repository_changes(cx);
            let active_diff_changed = self.active_diff.as_ref().is_some_and(|diff| {
                changed_paths
                    .iter()
                    .any(|changed_path| changed_path == &diff.change.path || changed_path == root)
            });
            if active_diff_changed {
                self.refresh_active_repository_diff(cx);
            }
            return;
        }

        let mut refresh = changed_paths
            .into_iter()
            .flat_map(|path| {
                let mut directories = Vec::with_capacity(2);
                if path == *root {
                    directories.push(root.clone());
                }
                if let Some(parent) = path.parent() {
                    directories.push(parent.to_path_buf());
                }
                directories
            })
            .filter(|directory| {
                directory == root
                    || (self.file_explorer.expanded.contains(directory)
                        && self.file_explorer.directories.contains_key(directory))
            })
            .collect::<HashSet<_>>();
        if refresh.remove(root) {
            self.request_directory(root.clone(), cx);
        }
        for directory in refresh {
            self.request_directory(directory, cx);
        }
    }

    fn request_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }

        self.file_explorer.next_request_id = self.file_explorer.next_request_id.wrapping_add(1);
        let request_id = self.file_explorer.next_request_id;
        self.file_explorer.requests.insert(path.clone(), request_id);
        if !matches!(
            self.file_explorer.directories.get(&path),
            Some(DirectoryListing::Loaded(_))
        ) {
            self.file_explorer
                .directories
                .insert(path.clone(), DirectoryListing::Loading);
        }

        let path_to_read = path.clone();
        let background = cx
            .background_executor()
            .spawn(async move { read_directory(&path_to_read) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_directory_request(root, path, request_id, result, cx)
            });
        })
        .detach();
        self.publish_workbench_surface(cx);
    }

    fn finish_directory_request(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        request_id: u64,
        result: Result<Vec<FileEntry>>,
        cx: &mut Context<Self>,
    ) {
        if self.file_explorer.root.as_ref() != Some(&root)
            || self.file_explorer.requests.get(&path) != Some(&request_id)
        {
            return;
        }

        self.file_explorer.requests.remove(&path);
        let listing = match result {
            Ok(entries) => DirectoryListing::Loaded(entries),
            Err(error) => DirectoryListing::Error(format!("{error:#}")),
        };
        self.file_explorer.directories.insert(path, listing);
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn refresh_file_explorer_if_root(&mut self, root: &PathBuf, cx: &mut Context<Self>) {
        if !self.file_explorer.open || self.file_explorer.root.as_ref() != Some(root) {
            return;
        }

        let mut directories = Vec::with_capacity(self.file_explorer.expanded.len() + 1);
        directories.push(root.clone());
        directories.extend(
            self.file_explorer
                .expanded
                .iter()
                .filter(|path| path.starts_with(root))
                .cloned(),
        );
        for directory in directories {
            self.request_directory(directory, cx);
        }
    }

    fn refresh_file_explorer(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else {
            return;
        };
        if self.file_explorer.mode == FileExplorerMode::Files {
            self.refresh_file_explorer_if_root(&root, cx);
        } else {
            self.request_repository_changes(cx);
            self.refresh_active_repository_diff(cx);
        }
        self.request_repository_git_summaries([root], cx);
    }

    fn activate_file_tree_row(
        &mut self,
        path: PathBuf,
        kind: FileEntryKind,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        self.file_explorer.selected = Some(path.clone());
        if kind.is_directory() {
            // The second event in a double-click must not immediately undo the first toggle.
            if click_count > 1 {
                self.publish_workbench_surface(cx);
                cx.notify();
                return;
            }
            if self.file_explorer.expanded.remove(&path) {
                self.publish_workbench_surface(cx);
                cx.notify();
                return;
            }
            self.file_explorer.expanded.insert(path.clone());
            if !matches!(
                self.file_explorer.directories.get(&path),
                Some(DirectoryListing::Loaded(_)) | Some(DirectoryListing::Loading)
            ) {
                self.request_directory(path, cx);
            }
            self.publish_workbench_surface(cx);
            cx.notify();
            return;
        }

        self.open_file_in_editor(path, cx);
    }

    fn open_project_settings(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };

        let preparation = ProjectInstructionsService::ensure(&workspace)
            .and_then(|_| ProjectTaskInstructionsService::ensure(&workspace).map(|_| ()));
        if let Err(error) = preparation {
            self.set_status(
                format!("Could not prepare the project settings: {error:#}"),
                true,
                cx,
            );
        }

        self.flush_active_file(cx);
        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = None;
        self.session.selected_repository_id = None;
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = Some(workspace_id);
        self.active_file = None;
        self.active_diff = None;
        self.quick_open = None;
        self.close_file_explorer(cx);
        self.persist_session();
        self.hydrate_project_settings_surface(workspace_id, cx);
        cx.notify();
    }

    fn open_project_instructions(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        let path = match ProjectInstructionsService::ensure(&workspace) {
            Ok(path) => path,
            Err(error) => {
                self.set_status(format!("Could not prepare CLAUDE.md: {error:#}"), true, cx);
                return;
            }
        };
        let Some(root) = path.parent().map(PathBuf::from) else {
            self.set_status("The project does not have a valid root directory", true, cx);
            return;
        };

        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = None;
        self.session.selected_repository_id = None;
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();

        if self.active_file.as_ref().is_some_and(|document| {
            document.source == FileDocumentSource::ProjectInstructions(workspace_id)
        }) {
            cx.notify();
            return;
        }

        self.flush_active_file(cx);
        self.next_file_request_id = self.next_file_request_id.wrapping_add(1);
        let request_id = self.next_file_request_id;
        self.active_file = Some(FileDocumentHandle {
            root: root.clone(),
            path: path.clone(),
            source: FileDocumentSource::ProjectInstructions(workspace_id),
            language: "markdown".into(),
            editor: None,
            load_state: FileDocumentLoadState::Loading,
            revision: 0,
            dirty: false,
            save_state: NoteSaveState::Saved,
            request_id,
        });

        let background = cx
            .background_executor()
            .spawn(async move { ProjectInstructionsService::read(&workspace) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_request(root, path, request_id, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn open_project_task_instructions(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        let path = match ProjectTaskInstructionsService::ensure(&workspace) {
            Ok(path) => path,
            Err(error) => {
                self.set_status(
                    format!("Could not prepare the shared task CLAUDE.md: {error:#}"),
                    true,
                    cx,
                );
                return;
            }
        };
        let Some(root) = path.parent().map(PathBuf::from) else {
            self.set_status("The project does not have a valid root directory", true, cx);
            return;
        };

        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = None;
        self.session.selected_repository_id = None;
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();

        if self.active_file.as_ref().is_some_and(|document| {
            document.source == FileDocumentSource::ProjectTaskInstructions(workspace_id)
        }) {
            cx.notify();
            return;
        }

        self.flush_active_file(cx);
        self.next_file_request_id = self.next_file_request_id.wrapping_add(1);
        let request_id = self.next_file_request_id;
        self.active_file = Some(FileDocumentHandle {
            root: root.clone(),
            path: path.clone(),
            source: FileDocumentSource::ProjectTaskInstructions(workspace_id),
            language: "markdown".into(),
            editor: None,
            load_state: FileDocumentLoadState::Loading,
            revision: 0,
            dirty: false,
            save_state: NoteSaveState::Saved,
            request_id,
        });

        let background = cx
            .background_executor()
            .spawn(async move { ProjectTaskInstructionsService::read(&workspace) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_request(root, path, request_id, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn open_file_in_editor(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(root) = self.file_explorer.root.clone() else {
            return;
        };
        self.active_diff = None;
        if self
            .active_file
            .as_ref()
            .is_some_and(|document| document.path == path)
        {
            self.show_project_note = false;
            self.show_task_note = false;
            self.show_terminal = false;
            self.show_settings = false;
            self.project_settings_workspace_id = None;
            self.publish_workbench_surface(cx);
            cx.notify();
            return;
        }

        self.flush_active_file(cx);
        self.next_file_request_id = self.next_file_request_id.wrapping_add(1);
        let request_id = self.next_file_request_id;
        self.active_file = Some(FileDocumentHandle {
            root: root.clone(),
            language: file_editor_language(&path).into(),
            path: path.clone(),
            source: FileDocumentSource::Repository,
            editor: None,
            load_state: FileDocumentLoadState::Loading,
            revision: 0,
            dirty: false,
            save_state: NoteSaveState::Saved,
            request_id,
        });
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;

        let read_root = root.clone();
        let read_path = path.clone();
        let background = cx
            .background_executor()
            .spawn(async move { read_text_file(&read_root, &read_path) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_request(root, path, request_id, result, cx)
            });
        })
        .detach();
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn finish_file_request(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        request_id: u64,
        result: Result<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.active_file.as_mut() else {
            return;
        };
        if document.root != root || document.path != path || document.request_id != request_id {
            return;
        }
        document.load_state = match result {
            Ok(content) => FileDocumentLoadState::Ready(content),
            Err(error) => FileDocumentLoadState::Error(format!("{error:#}")),
        };
        self.publish_workbench_surface(cx);
        cx.notify();
    }

    fn ensure_file_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.active_file.as_mut() else {
            return;
        };
        if document.editor.is_some() {
            return;
        }
        let content = match std::mem::replace(&mut document.load_state, FileDocumentLoadState::Open)
        {
            FileDocumentLoadState::Ready(content) => content,
            state => {
                document.load_state = state;
                return;
            }
        };
        let path = document.path.clone();
        let language = document.language.clone();
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(language)
                .tab_size(TabSize {
                    tab_size: FILE_EDITOR_TAB_SIZE,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .default_value(content)
        });
        let subscribed_path = path.clone();
        cx.subscribe(&editor, move |app, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                app.queue_file_save(subscribed_path.clone(), Duration::from_millis(650), cx)
            }
            InputEvent::Blur => app.queue_file_save(subscribed_path.clone(), Duration::ZERO, cx),
            InputEvent::Focus | InputEvent::PressEnter { .. } => {}
        })
        .detach();
        if let Some(document) = self.active_file.as_mut()
            && document.path == path
        {
            document.editor = Some(editor.clone());
        }
        editor.update(cx, |input, cx| input.focus(window, cx));
    }

    fn queue_file_save(&mut self, path: PathBuf, delay: Duration, cx: &mut Context<Self>) {
        let Some(document) = self
            .active_file
            .as_mut()
            .filter(|document| document.path == path && document.editor.is_some())
        else {
            return;
        };
        document.revision = document.revision.saturating_add(1);
        document.dirty = true;
        document.save_state = NoteSaveState::Saving;
        let revision = document.revision;
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            Timer::after(delay).await;
            let snapshot = weak
                .update(cx, |app, cx| {
                    let (root, source, content) = {
                        let document = app.active_file.as_ref()?;
                        if document.path != path || document.revision != revision {
                            return None;
                        }
                        let editor = document.editor.as_ref()?;
                        (
                            document.root.clone(),
                            document.source,
                            editor.read(cx).value().to_string(),
                        )
                    };
                    Some((
                        root.clone(),
                        app.file_save_operation(source, root, path.clone(), content),
                    ))
                })
                .ok()
                .flatten();
            let Some((repository_root, operation)) = snapshot else {
                return;
            };
            let result = match operation {
                Ok(operation) => {
                    cx.background_executor()
                        .spawn(async move { operation.execute() })
                        .await
                }
                Err(error) => Err(error),
            };
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_save(&repository_root, &path, revision, result, false, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn flush_active_file(&mut self, cx: &mut Context<Self>) {
        let Some((root, path, source, revision, editor)) =
            self.active_file.as_mut().and_then(|document| {
                if !document.dirty {
                    return None;
                }
                let editor = document.editor.clone()?;
                document.revision = document.revision.saturating_add(1);
                document.save_state = NoteSaveState::Saving;
                Some((
                    document.root.clone(),
                    document.path.clone(),
                    document.source,
                    document.revision,
                    editor,
                ))
            })
        else {
            return;
        };
        let content = editor.read(cx).value().to_string();
        let repository_root = root.clone();
        let operation = self.file_save_operation(source, root, path.clone(), content);
        let background = cx.background_executor().spawn(async move {
            match operation {
                Ok(operation) => operation.execute(),
                Err(error) => Err(error),
            }
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.finish_file_save(&repository_root, &path, revision, result, true, cx)
            });
        })
        .detach();
    }

    fn file_save_operation(
        &self,
        source: FileDocumentSource,
        root: PathBuf,
        path: PathBuf,
        content: String,
    ) -> Result<FileSaveOperation> {
        match source {
            FileDocumentSource::Repository => Ok(FileSaveOperation::Repository {
                root,
                path,
                content,
            }),
            FileDocumentSource::ProjectInstructions(workspace_id) => {
                let workspace = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .cloned()
                    .context("The project is no longer available")?;
                Ok(FileSaveOperation::ProjectInstructions { workspace, content })
            }
            FileDocumentSource::ProjectTaskInstructions(workspace_id) => {
                let workspace = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .cloned()
                    .context("The project is no longer available")?;
                let tasks = self
                    .tasks
                    .iter()
                    .filter(|task| {
                        task.workspace_id == workspace_id
                            && task
                                .worktree_root_path
                                .starts_with(&self.paths.task_workspaces)
                            && task.worktree_root_path.is_dir()
                    })
                    .cloned()
                    .collect();
                Ok(FileSaveOperation::ProjectTaskInstructions {
                    paths: self.paths.clone(),
                    workspace,
                    tasks,
                    content,
                })
            }
        }
    }

    fn finish_file_save(
        &mut self,
        repository_root: &PathBuf,
        path: &PathBuf,
        revision: u64,
        result: Result<()>,
        report_when_inactive: bool,
        cx: &mut Context<Self>,
    ) {
        let saved = result.is_ok();
        let current = self
            .active_file
            .as_ref()
            .is_some_and(|document| document.path == *path && document.revision == revision);
        if current {
            if let Some(document) = self.active_file.as_mut() {
                document.save_state = if result.is_ok() {
                    NoteSaveState::Saved
                } else {
                    NoteSaveState::Error
                };
                if result.is_ok() {
                    document.dirty = false;
                }
            }
        }
        if saved {
            self.refresh_saved_repository_git_summary(repository_root.clone(), cx);
            if self.file_explorer.root.as_ref() == Some(repository_root)
                && self.file_explorer.mode == FileExplorerMode::Changes
            {
                self.request_repository_changes(cx);
                if self
                    .active_diff
                    .as_ref()
                    .is_some_and(|diff| diff.change.path.as_path() == path.as_path())
                {
                    self.refresh_active_repository_diff(cx);
                }
            }
        }
        if let Err(error) = result
            && (current || report_when_inactive)
        {
            self.status = Some((
                format!("Could not save {}: {error:#}", path.display()),
                true,
            ));
        }
        cx.notify();
    }

    fn close_file_editor(&mut self, cx: &mut Context<Self>) {
        self.flush_active_file(cx);
        self.active_file = None;
        self.show_terminal = false;
        cx.notify();
    }

    fn file_tree_rows(&self) -> Vec<FileTreeRow> {
        let Some(root) = self.file_explorer.root.as_ref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        self.append_directory_rows(root, 0, &mut rows);
        rows
    }

    fn append_directory_rows(
        &self,
        directory: &PathBuf,
        depth: usize,
        rows: &mut Vec<FileTreeRow>,
    ) {
        match self.file_explorer.directories.get(directory) {
            Some(DirectoryListing::Loaded(entries)) => {
                for entry in entries {
                    rows.push(FileTreeRow {
                        path: entry.path.clone(),
                        label: entry.name.clone(),
                        depth,
                        hidden: entry.hidden,
                        expanded: self.file_explorer.expanded.contains(&entry.path),
                        kind: FileTreeRowKind::Entry(entry.kind),
                    });
                    if entry.kind.is_directory()
                        && self.file_explorer.expanded.contains(&entry.path)
                    {
                        self.append_directory_rows(&entry.path, depth + 1, rows);
                    }
                }
            }
            Some(DirectoryListing::Error(error)) => rows.push(FileTreeRow {
                path: directory.clone(),
                label: error.clone(),
                depth,
                hidden: false,
                expanded: false,
                kind: FileTreeRowKind::Error,
            }),
            Some(DirectoryListing::Loading) | None => rows.push(FileTreeRow {
                path: directory.clone(),
                label: self.tr("Loading…", "Cargando…").to_string(),
                depth,
                hidden: false,
                expanded: false,
                kind: FileTreeRowKind::Loading,
            }),
        }
    }

    fn tr<'a>(&self, english: &'a str, spanish: &'a str) -> &'a str {
        match self.session.language {
            Language::English => english,
            Language::Spanish => spanish,
        }
    }

    fn handle_bridge_event(&mut self, message: &str, cx: &mut Context<Self>) {
        let message = message.trim();
        if let Some(payload) = message.strip_prefix("claude-session:") {
            let Ok(payload) = serde_json::from_str::<ClaudeSessionBridgePayload>(payload) else {
                return;
            };
            let session = ClaudeSession {
                id: payload.session_id,
                profile: payload.profile,
            };
            let Some(terminal) = self
                .session
                .terminals
                .iter_mut()
                .find(|terminal| terminal.id == payload.terminal_id)
            else {
                return;
            };
            if terminal.claude_session.as_ref() == Some(&session)
                && terminal.agent == AgentKind::Claude
                && terminal.state == SessionState::Working
            {
                return;
            }
            terminal.claude_session = Some(session);
            self.set_terminal_agent(payload.terminal_id, AgentKind::Claude);
            self.update_terminal_state(payload.terminal_id, SessionState::Working);
            cx.notify();
            return;
        }
        if let Some(payload) = message.strip_prefix("codex-session:") {
            let Ok(payload) = serde_json::from_str::<CodexSessionBridgePayload>(payload) else {
                return;
            };
            let Some(terminal) = self
                .session
                .terminals
                .iter_mut()
                .find(|terminal| terminal.id == payload.terminal_id)
            else {
                return;
            };
            terminal.codex_session = Some(CodexSession {
                id: payload.session_id,
                profile: payload.profile,
            });
            self.set_terminal_agent(payload.terminal_id, AgentKind::Codex);
            self.update_terminal_state(payload.terminal_id, SessionState::Working);
            cx.notify();
            return;
        }
        if let Some(task_id) = message.strip_prefix("open-task:") {
            let Ok(task_id) = Uuid::parse_str(task_id) else {
                return;
            };
            let task = self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .cloned()
                .or_else(|| {
                    self.reload_external_data(cx);
                    self.tasks.iter().find(|task| task.id == task_id).cloned()
                });
            if let Some(task) = task {
                let repository_id = task
                    .repositories
                    .first()
                    .map(|repository| repository.repository_id);
                self.select_target(task.workspace_id, Some(task.id), repository_id, cx);
            }
            return;
        }
        if let Some(payload) = message.strip_prefix("agent-handoff:") {
            if let Ok(payload) = serde_json::from_str::<AgentHandoffPayload>(payload) {
                if let Err(error) = self.handle_agent_handoff(payload, cx) {
                    self.set_status(format!("Could not delegate: {error:#}"), true, cx);
                }
            }
            return;
        }
        if let Some(payload) = message.strip_prefix("navigation-link:") {
            if let Ok(payload) = serde_json::from_str::<NavigationLinkPayload>(payload) {
                self.handle_navigation_link(payload, cx);
            }
            return;
        }
        if let Some(payload) = message.strip_prefix("task-ready:") {
            let Ok(payload) = serde_json::from_str::<TaskReadyPayload>(payload) else {
                return;
            };
            if let Some((_, turn)) = self
                .orchestrator_turns
                .iter_mut()
                .find(|(scope, _)| scope.task_id() == Some(payload.task_id))
            {
                turn.notification_sent = true;
            }
            if let Some((title, message)) =
                self.announce_ready_task(payload.task_id, payload.title, payload.message, cx)
            {
                play_agent_attention_sound();
                cx.background_executor()
                    .spawn(async move {
                        show_native_agent_notification(&title, &message);
                    })
                    .detach();
            }
            return;
        }
        if let Some(task_id) = message.strip_prefix("note-updated:") {
            if let Ok(task_id) = Uuid::parse_str(task_id) {
                self.task_notes.remove(&task_id);
            }
            cx.notify();
            return;
        }
        if let Some(workspace_id) = message.strip_prefix("project-note-updated:") {
            if let Ok(workspace_id) = Uuid::parse_str(workspace_id) {
                self.project_notes.remove(&workspace_id);
            }
            cx.notify();
            return;
        }
        self.reload_external_data(cx);
        cx.notify();
    }

    fn reload_external_data(&mut self, cx: &mut Context<Self>) {
        if let Ok(workspaces) = self.database.workspaces() {
            self.workspaces = workspaces;
        }
        if let Ok(tasks) = self.database.all_tasks() {
            let current_task_ids = self
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<HashSet<_>>();
            let mut new_task_ids = tasks
                .iter()
                .filter(|task| !current_task_ids.contains(&task.id))
                .map(|task| (task.created_at, task.id))
                .collect::<Vec<_>>();
            new_task_ids.sort_by(|(left, _), (right, _)| left.cmp(right));
            self.tasks = tasks;
            let loaded_task_ids = self
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<HashSet<_>>();
            let unseen_count = self.session.unseen_task_ids.len();
            self.session
                .unseen_task_ids
                .retain(|task_id| loaded_task_ids.contains(task_id));
            self.app_toasts.retain(|toast| {
                toast
                    .target
                    .task_id()
                    .is_none_or(|task_id| loaded_task_ids.contains(&task_id))
            });
            if self.session.unseen_task_ids.len() != unseen_count {
                self.persist_session();
            }
            for (_, task_id) in new_task_ids {
                // Tasks discovered here were created outside the app, so they
                // must not pull the user away from what they are doing.
                self.reveal_new_task(task_id, false, cx);
            }
        }
        self.refresh_expanded_repository_git_summaries(cx);
    }

    /// Register a task that just appeared.
    ///
    /// `focus` is true only when the user created the task from the app and is
    /// waiting on it. A task created by an agent over MCP must never take over
    /// the view: the user may be working somewhere else entirely. Such a task
    /// only gets its unseen marker in the sidebar, and the agent announces it
    /// when the work is done through `notify_task_ready`.
    fn reveal_new_task(&mut self, task_id: Uuid, focus: bool, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let is_new = !self.session.unseen_task_ids.contains(&task_id);
        if is_new {
            insert_unique(&mut self.session.unseen_task_ids, task_id);
            self.persist_session();
        }
        self.request_task_git_summaries(task_id, cx);
        if !focus {
            cx.notify();
            return;
        }
        self.select_target(task.workspace_id, Some(task_id), None, cx);
        if let Some(row_index) = self.sidebar_task_row_index(task_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
        if is_new {
            self.show_task_created_toast(&task, cx);
        }
    }

    /// Show the toast an agent asked for once it finished a task.
    ///
    /// Returns the strings to repeat as a desktop notification, or `None` when
    /// there is nothing to announce. The selected task is deliberately left
    /// untouched: the user reaches the task by clicking the toast.
    fn announce_ready_task(
        &mut self,
        task_id: Uuid,
        title: Option<String>,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<(String, String)> {
        let task = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .cloned()
            .or_else(|| {
                self.reload_external_data(cx);
                self.tasks.iter().find(|task| task.id == task_id).cloned()
            })?;
        let (default_title, default_message) =
            self.task_toast_text(&task, "Task ready", "Tarea lista");
        // A later announcement about the same task replaces the earlier one, so
        // the toast on screen always carries the most recent summary.
        self.app_toasts
            .retain(|toast| toast.target.task_id() != Some(task_id));
        if !self.session.unseen_task_ids.contains(&task_id) {
            insert_unique(&mut self.session.unseen_task_ids, task_id);
            self.persist_session();
        }
        let title = title.unwrap_or(default_title);
        let message = message.unwrap_or(default_message);
        self.app_toasts.push(AppToast {
            target: AppToastTarget::Task { task_id },
            title: title.clone(),
            message: message.clone(),
        });
        cx.notify();
        Some((title, message))
    }

    fn task_toast_text(
        &self,
        task: &ProjectTask,
        english_heading: &str,
        spanish_heading: &str,
    ) -> (String, String) {
        let project = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == task.workspace_id)
            .map(|workspace| workspace.label().to_string());
        match self.session.language {
            Language::English => (
                english_heading.to_string(),
                project
                    .map(|project| format!("{} is ready in {project}", task.title))
                    .unwrap_or_else(|| format!("{} is ready", task.title)),
            ),
            Language::Spanish => (
                spanish_heading.to_string(),
                project
                    .map(|project| format!("{} está lista en {project}", task.title))
                    .unwrap_or_else(|| format!("{} está lista", task.title)),
            ),
        }
    }

    fn show_task_created_toast(&mut self, task: &ProjectTask, cx: &mut Context<Self>) {
        let target = AppToastTarget::Task { task_id: task.id };
        if self.app_toasts.iter().any(|toast| toast.target == target) {
            return;
        }
        let (title, message) = self.task_toast_text(task, "New task created", "Nueva tarea creada");
        self.app_toasts.push(AppToast {
            target,
            title,
            message,
        });
        cx.notify();
    }

    fn mark_task_seen(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        let unseen_count = self.session.unseen_task_ids.len();
        self.session
            .unseen_task_ids
            .retain(|current| *current != task_id);
        let toast_count = self.app_toasts.len();
        self.app_toasts
            .retain(|toast| toast.target.task_id() != Some(task_id));
        let task_was_unseen = self.session.unseen_task_ids.len() != unseen_count;
        let toast_was_visible = self.app_toasts.len() != toast_count;
        if task_was_unseen {
            self.persist_session();
        }
        if task_was_unseen || toast_was_visible {
            cx.notify();
        }
    }

    fn open_task_from_toast(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            self.dismiss_app_toast(AppToastTarget::Task { task_id }, cx);
            return;
        };
        self.show_task_notes_for(task.workspace_id, task_id, cx);
        if let Some(row_index) = self.sidebar_task_row_index(task_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
    }

    fn open_project_from_chat(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            self.reload_external_data(cx);
        }
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return;
        }
        self.show_project_notes(workspace_id, cx);
        if let Some(row_index) = self.sidebar_workspace_row_index(workspace_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
    }

    fn open_task_from_chat(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        if !self.tasks.iter().any(|task| task.id == task_id) {
            self.reload_external_data(cx);
        }
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        self.show_task_notes_for(task.workspace_id, task_id, cx);
        self.hydrate_navigation(cx);
        self.dispatch_navigation_event(serde_json::json!({
            "type": "reveal_target", "workspace_id": task.workspace_id, "task_id": task_id,
        }), cx);
        if let Some(row_index) = self.sidebar_task_row_index(task_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
    }

    fn sidebar_workspace_row_index(&self, target_workspace_id: Uuid) -> Option<usize> {
        let mut row_index = 0;
        for workspace in &self.workspaces {
            let workspace_id = workspace.id;
            if workspace_id == target_workspace_id {
                return Some(row_index);
            }
            row_index += 1;
            if !self.session.expanded_workspace_ids.contains(&workspace_id) {
                continue;
            }

            row_index += 1;
            row_index += self
                .session
                .terminals
                .iter()
                .filter(|terminal| {
                    terminal.workspace_id == workspace_id
                        && terminal.task_id.is_none()
                        && terminal.repository_id.is_none()
                })
                .count();
            for repository in &workspace.repositories {
                row_index += 1;
                row_index += self
                    .session
                    .terminals
                    .iter()
                    .filter(|terminal| {
                        terminal.workspace_id == workspace_id
                            && terminal.task_id.is_none()
                            && terminal.repository_id == Some(repository.id)
                    })
                    .count();
            }

            let workspace_tasks = self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace_id)
                .collect::<Vec<_>>();
            if !workspace_tasks.is_empty() {
                row_index += 1;
            }
            for task in workspace_tasks {
                row_index += 1;
                if !self.session.expanded_task_ids.contains(&task.id) {
                    continue;
                }

                row_index += 1;
                row_index += self
                    .session
                    .terminals
                    .iter()
                    .filter(|terminal| {
                        terminal.workspace_id == workspace_id
                            && terminal.task_id == Some(task.id)
                            && terminal.repository_id.is_none()
                    })
                    .count();
                for repository in &task.repositories {
                    row_index += 1;
                    row_index += self
                        .session
                        .terminals
                        .iter()
                        .filter(|terminal| {
                            terminal.workspace_id == workspace_id
                                && terminal.task_id == Some(task.id)
                                && terminal.repository_id == Some(repository.repository_id)
                        })
                        .count();
                }
            }
        }
        None
    }

    fn sidebar_task_row_index(&self, task_id: Uuid) -> Option<usize> {
        let mut row_index = 0;
        for workspace in &self.workspaces {
            let workspace_id = workspace.id;
            row_index += 1;
            if !self.session.expanded_workspace_ids.contains(&workspace_id) {
                continue;
            }

            row_index += 1;
            row_index += self
                .session
                .terminals
                .iter()
                .filter(|terminal| {
                    terminal.workspace_id == workspace_id
                        && terminal.task_id.is_none()
                        && terminal.repository_id.is_none()
                })
                .count();
            for repository in &workspace.repositories {
                row_index += 1;
                row_index += self
                    .session
                    .terminals
                    .iter()
                    .filter(|terminal| {
                        terminal.workspace_id == workspace_id
                            && terminal.task_id.is_none()
                            && terminal.repository_id == Some(repository.id)
                    })
                    .count();
            }

            let workspace_tasks = self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace_id)
                .collect::<Vec<_>>();
            if !workspace_tasks.is_empty() {
                row_index += 1;
            }
            for task in workspace_tasks {
                if task.id == task_id {
                    return Some(row_index);
                }
                row_index += 1;
                if !self.session.expanded_task_ids.contains(&task.id) {
                    continue;
                }

                row_index += 1;
                row_index += self
                    .session
                    .terminals
                    .iter()
                    .filter(|terminal| {
                        terminal.workspace_id == workspace_id
                            && terminal.task_id == Some(task.id)
                            && terminal.repository_id.is_none()
                    })
                    .count();
                for repository in &task.repositories {
                    row_index += 1;
                    row_index += self
                        .session
                        .terminals
                        .iter()
                        .filter(|terminal| {
                            terminal.workspace_id == workspace_id
                                && terminal.task_id == Some(task.id)
                                && terminal.repository_id == Some(repository.repository_id)
                        })
                        .count();
                }
            }
        }
        None
    }

    fn selected_task(&self) -> Option<&ProjectTask> {
        let id = self.session.selected_task_id?;
        self.tasks.iter().find(|task| task.id == id)
    }

    fn selected_dock_key(&self) -> Option<String> {
        Some(dock_key(
            self.session.selected_workspace_id?,
            self.session.selected_task_id,
            self.session.selected_repository_id,
        ))
    }

    fn selected_terminal_id(&self) -> Option<Uuid> {
        let dock = self.session.docks.get(&self.selected_dock_key()?)?;
        let active_tab_id = dock
            .active_tab_id
            .or_else(|| dock.tabs.last().map(|tab| tab.id))?;
        dock.tabs
            .iter()
            .find(|tab| tab.id == active_tab_id)
            .map(|tab| tab.active_terminal_id)
    }

    fn projects_root(&self) -> PathBuf {
        self.database
            .setting("projects-root-path")
            .ok()
            .flatten()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.paths.default_projects.clone())
    }

    fn agents_full_access(&self) -> bool {
        self.database
            .setting("agents-full-access")
            .ok()
            .flatten()
            .is_none_or(|value| value != "false")
    }

    fn agent_skills(&self) -> Vec<AgentSkill> {
        AgentSkillService::list(&self.paths.agent_skills_plugin).unwrap_or_default()
    }

    fn agent_mcp_servers(&self, workspace_id: Option<Uuid>) -> Vec<AgentMcpServer> {
        let provider = self.agent_provider();
        let mut servers = AgentMcpService::list(
            &self.paths,
            provider,
            self.agent_auth_mode(provider),
            &self.workspaces,
            workspace_id,
        );
        let Some(workspace_id) = workspace_id else {
            return servers;
        };
        if !AgentMcpService::supports_external_servers(provider) {
            return servers;
        }

        let mut names = servers
            .iter()
            .map(|server| server.name.clone())
            .collect::<HashSet<_>>();
        for config in self.installed_project_agent_mcps(workspace_id) {
            if !names.insert(config.name().to_string()) {
                continue;
            }
            servers.push(AgentMcpServer {
                name: config.name().to_string(),
                source: format!("Blackholes · project · {}", config.transport_label()),
                required: false,
                managed: true,
                config: Some(config),
            });
        }
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        servers
    }

    fn installed_project_agent_mcps(&self, workspace_id: Uuid) -> Vec<AgentMcpServerConfig> {
        self.database
            .setting(&format!("project-installed-mcp-servers-{workspace_id}"))
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default()
    }

    fn save_installed_project_agent_mcps(
        &self,
        workspace_id: Uuid,
        servers: &[AgentMcpServerConfig],
    ) -> Result<()> {
        self.database.set_setting(
            &format!("project-installed-mcp-servers-{workspace_id}"),
            &serde_json::to_string(servers)?,
        )
    }

    fn project_mcp_authentication_key(&self, workspace_id: Uuid, name: &str) -> String {
        let provider = self.agent_provider();
        format!(
            "project-mcp-auth-{workspace_id}-{}-{}-{name}",
            provider.id(),
            self.agent_auth_mode(provider).id()
        )
    }

    fn project_mcp_authentication_display(
        &self,
        workspace_id: Uuid,
        mcp: &AgentMcpServer,
    ) -> (Option<&'static str>, Option<String>) {
        if !mcp.managed || !matches!(mcp.config, Some(AgentMcpServerConfig::Http { .. })) {
            return (None, None);
        }
        let key = self.project_mcp_authentication_key(workspace_id, &mcp.name);
        if let Some(authentication) = self.project_mcp_authentications.get(&key) {
            return (
                Some(match authentication.status {
                    ProjectMcpAuthStatus::Connecting => "connecting",
                    ProjectMcpAuthStatus::Connected => "connected",
                    ProjectMcpAuthStatus::Error => "error",
                }),
                Some(authentication.detail.clone()),
            );
        }
        let connected = self
            .database
            .setting(&key)
            .ok()
            .flatten()
            .is_some_and(|value| value == "connected");
        if connected {
            (
                Some("connected"),
                Some(
                    self.tr(
                        "Authorization saved for the selected agent profile.",
                        "Autorización guardada para el perfil de agente seleccionado.",
                    )
                    .to_string(),
                ),
            )
        } else {
            (
                Some("needs-auth"),
                Some(
                    self.tr(
                        "Authorize this MCP before an agent uses it.",
                        "Autoriza este MCP antes de que lo use un agente.",
                    )
                    .to_string(),
                ),
            )
        }
    }

    fn agent_mcp_setting_key(&self, project_id: Option<Uuid>) -> String {
        let provider = self.agent_provider();
        let auth_mode = self.agent_auth_mode(provider);
        match project_id {
            Some(project_id) => format!(
                "project-enabled-mcp-servers-{project_id}-{}-{}",
                provider.id(),
                auth_mode.id()
            ),
            None => format!(
                "agent-enabled-mcp-servers-{}-{}",
                provider.id(),
                auth_mode.id()
            ),
        }
    }

    fn enabled_agent_mcp_names(&self) -> HashSet<String> {
        let available = self
            .agent_mcp_servers(None)
            .into_iter()
            .map(|mcp| mcp.name)
            .collect::<HashSet<_>>();
        let mut enabled = self
            .database
            .setting(&self.agent_mcp_setting_key(None))
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .map(|names| {
                names
                    .into_iter()
                    .filter(|name| available.contains(name))
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| available.clone());
        enabled.insert("blackholes".to_string());
        enabled
    }

    fn project_enabled_agent_mcp_names(&self, workspace_id: Uuid) -> HashSet<String> {
        let available = self
            .agent_mcp_servers(Some(workspace_id))
            .into_iter()
            .map(|mcp| mcp.name)
            .collect::<HashSet<_>>();
        let globally_configured = self
            .agent_mcp_servers(None)
            .into_iter()
            .map(|mcp| mcp.name)
            .collect::<HashSet<_>>();
        let globally_enabled = self.enabled_agent_mcp_names();
        let eligible = available
            .into_iter()
            .filter(|name| !globally_configured.contains(name) || globally_enabled.contains(name))
            .collect::<HashSet<_>>();
        let mut enabled = self
            .database
            .setting(&self.agent_mcp_setting_key(Some(workspace_id)))
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .map(|names| {
                names
                    .into_iter()
                    .filter(|name| eligible.contains(name))
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| eligible.clone());
        enabled.insert("blackholes".to_string());
        enabled
    }

    fn available_agent_mcp_names_for_scope(&self, scope: OrchestratorChatScope) -> Vec<String> {
        let workspace_id = self.orchestrator_scope_workspace_id(scope);
        self.agent_mcp_servers(workspace_id)
            .into_iter()
            .map(|mcp| mcp.name)
            .collect()
    }

    fn enabled_agent_mcp_names_for_scope(&self, scope: OrchestratorChatScope) -> Vec<String> {
        let mut names = self
            .orchestrator_scope_workspace_id(scope)
            .map_or_else(
                || self.enabled_agent_mcp_names(),
                |workspace_id| self.project_enabled_agent_mcp_names(workspace_id),
            )
            .into_iter()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn configured_agent_mcps_for_scope(
        &self,
        scope: OrchestratorChatScope,
    ) -> Vec<AgentMcpServerConfig> {
        let Some(workspace_id) = self.orchestrator_scope_workspace_id(scope) else {
            return Vec::new();
        };
        let enabled = self.project_enabled_agent_mcp_names(workspace_id);
        self.installed_project_agent_mcps(workspace_id)
            .into_iter()
            .filter(|server| enabled.contains(server.name()))
            .collect()
    }

    fn orchestrator_scope_workspace_id(&self, scope: OrchestratorChatScope) -> Option<Uuid> {
        match scope {
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => Some(workspace_id),
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| task.workspace_id),
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => None,
        }
    }

    fn save_agent_mcp_names(
        &self,
        project_id: Option<Uuid>,
        enabled: &HashSet<String>,
    ) -> Result<()> {
        let mut enabled = enabled.iter().cloned().collect::<Vec<_>>();
        enabled.sort();
        self.database.set_setting(
            &self.agent_mcp_setting_key(project_id),
            &serde_json::to_string(&enabled)?,
        )
    }

    fn set_agent_mcp_enabled(&mut self, name: String, enabled: bool, cx: &mut Context<Self>) {
        let available = self.agent_mcp_servers(None);
        if !available
            .iter()
            .any(|mcp| mcp.name == name && !mcp.required)
        {
            return;
        }
        let mut enabled_names = self.enabled_agent_mcp_names();
        if enabled {
            enabled_names.insert(name);
        } else {
            enabled_names.remove(&name);
        }
        if let Err(error) = self.save_agent_mcp_names(None, &enabled_names) {
            self.set_status(
                format!("Could not save the MCP configuration: {error:#}"),
                true,
                cx,
            );
        }
        cx.notify();
    }

    fn set_project_agent_mcp_enabled(
        &mut self,
        workspace_id: Uuid,
        name: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let available = self.agent_mcp_servers(Some(workspace_id));
        let globally_configured = self
            .agent_mcp_servers(None)
            .into_iter()
            .any(|mcp| mcp.name == name);
        if !available
            .iter()
            .any(|mcp| mcp.name == name && !mcp.required)
            || (globally_configured && !self.enabled_agent_mcp_names().contains(&name))
        {
            return;
        }
        let mut enabled_names = self.project_enabled_agent_mcp_names(workspace_id);
        if enabled {
            enabled_names.insert(name);
        } else {
            enabled_names.remove(&name);
        }
        if let Err(error) = self.save_agent_mcp_names(Some(workspace_id), &enabled_names) {
            self.set_status(
                format!("Could not save the project MCP configuration: {error:#}"),
                true,
                cx,
            );
        }
        cx.notify();
    }

    fn authenticate_project_agent_mcp(
        &mut self,
        workspace_id: Uuid,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self
            .installed_project_agent_mcps(workspace_id)
            .into_iter()
            .find(|server| server.name() == name)
        else {
            return;
        };
        if !matches!(server, AgentMcpServerConfig::Http { .. }) {
            self.set_status(
                self.tr(
                    "Local MCP servers do not use browser authentication.",
                    "Los servidores MCP locales no usan autenticación en el navegador.",
                ),
                true,
                cx,
            );
            return;
        }

        let provider = self.agent_provider();
        if !AgentMcpService::supports_external_servers(provider) {
            self.set_status(
                self.tr(
                    "The selected agent adapter cannot authenticate external MCP servers.",
                    "El adaptador seleccionado no puede autenticar servidores MCP externos.",
                ),
                true,
                cx,
            );
            return;
        }
        let auth_mode = self.agent_auth_mode(provider);
        let key = self.project_mcp_authentication_key(workspace_id, &name);
        if self
            .project_mcp_authentications
            .get(&key)
            .is_some_and(|state| state.status == ProjectMcpAuthStatus::Connecting)
        {
            return;
        }

        let mut enabled = self.project_enabled_agent_mcp_names(workspace_id);
        enabled.insert(name.clone());
        if let Err(error) = self.save_agent_mcp_names(Some(workspace_id), &enabled) {
            self.set_status(
                format!("Could not enable the project MCP server: {error:#}"),
                true,
                cx,
            );
            return;
        }
        let attempt_id = Uuid::new_v4();
        let (cancel, cancel_receiver) = flume::bounded(1);
        self.project_mcp_authentications.insert(
            key.clone(),
            ProjectMcpAuthentication {
                attempt_id,
                status: ProjectMcpAuthStatus::Connecting,
                detail: self
                    .tr(
                        "Complete authorization in the browser window that is opening…",
                        "Completa la autorización en la ventana del navegador que se está abriendo…",
                    )
                    .to_string(),
                cancel: Some(cancel),
            },
        );
        self.hydrate_project_settings_surface(workspace_id, cx);
        cx.notify();

        let profiles_root = self.paths.agent_profiles.clone();
        let background = cx.background_executor().spawn(async move {
            authenticate_agent_mcp(
                provider,
                auth_mode,
                &profiles_root,
                workspace_id,
                &server,
                cancel_receiver,
            )
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                let attempt_is_current = app
                    .project_mcp_authentications
                    .get(&key)
                    .is_some_and(|authentication| authentication.attempt_id == attempt_id);
                if !attempt_is_current {
                    return;
                }
                match result {
                    Ok(_) => {
                        let persist_result = app.database.set_setting(&key, "connected");
                        let detail = match persist_result {
                            Ok(()) => app
                                .tr(
                                    "Connected. Project agents can now use this MCP.",
                                    "Conectado. Los agentes del proyecto ya pueden usar este MCP.",
                                )
                                .to_string(),
                            Err(error) => {
                                format!("Connected, but the state could not be saved: {error:#}")
                            }
                        };
                        app.project_mcp_authentications.insert(
                            key.clone(),
                            ProjectMcpAuthentication {
                                attempt_id,
                                status: ProjectMcpAuthStatus::Connected,
                                detail,
                                cancel: None,
                            },
                        );
                        app.set_status(
                            match app.session.language {
                                Language::English => format!("MCP {name} connected"),
                                Language::Spanish => format!("MCP {name} conectado"),
                            },
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        app.project_mcp_authentications.insert(
                            key.clone(),
                            ProjectMcpAuthentication {
                                attempt_id,
                                status: ProjectMcpAuthStatus::Error,
                                detail: format!("{error:#}"),
                                cancel: None,
                            },
                        );
                        app.set_status(
                            match app.session.language {
                                Language::English => format!("Could not connect MCP {name}"),
                                Language::Spanish => format!("No se pudo conectar el MCP {name}"),
                            },
                            true,
                            cx,
                        );
                    }
                }
                if app.project_settings_workspace_id == Some(workspace_id) {
                    app.hydrate_project_settings_surface(workspace_id, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_project_agent_mcp_authentication(
        &mut self,
        workspace_id: Uuid,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let key = self.project_mcp_authentication_key(workspace_id, &name);
        let Some(authentication) = self.project_mcp_authentications.remove(&key) else {
            return;
        };
        if authentication.status != ProjectMcpAuthStatus::Connecting {
            self.project_mcp_authentications.insert(key, authentication);
            return;
        }
        drop(authentication);
        let _ = self.database.set_setting(&key, "needs-auth");
        self.set_status(
            match self.session.language {
                Language::English => format!("MCP {name} connection cancelled"),
                Language::Spanish => format!("Conexión del MCP {name} cancelada"),
            },
            false,
            cx,
        );
        self.hydrate_project_settings_surface(workspace_id, cx);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn install_project_agent_mcp(
        &mut self,
        workspace_id: Uuid,
        name: String,
        transport: String,
        url: Option<String>,
        oauth_client_id: Option<String>,
        oauth_callback_port: Option<u16>,
        command: Option<String>,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return;
        }
        if !AgentMcpService::supports_external_servers(self.agent_provider()) {
            self.set_status(
                self.tr(
                    "The selected agent adapter cannot install external MCP servers.",
                    "El adaptador de agente seleccionado no permite instalar servidores MCP externos.",
                ),
                true,
                cx,
            );
            return;
        }

        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || name.len() > 64
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            || name == "blackholes"
        {
            self.set_status(
                self.tr(
                    "Use a unique MCP name with only letters, numbers, hyphens, or underscores.",
                    "Usa un nombre MCP único con letras, números, guiones o guiones bajos.",
                ),
                true,
                cx,
            );
            return;
        }

        let already_managed = self
            .installed_project_agent_mcps(workspace_id)
            .iter()
            .any(|server| server.name() == name);
        if !already_managed
            && self
                .agent_mcp_servers(None)
                .iter()
                .any(|server| server.name == name)
        {
            self.set_status(
                self.tr(
                    "An MCP with that name already exists in the agent profile.",
                    "Ya existe un MCP con ese nombre en el perfil del agente.",
                ),
                true,
                cx,
            );
            return;
        }

        let config = match transport.as_str() {
            "http" => {
                let url = url.unwrap_or_default().trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    self.set_status(
                        self.tr(
                            "Enter a valid HTTP or HTTPS MCP URL.",
                            "Ingresa una URL MCP HTTP o HTTPS válida.",
                        ),
                        true,
                        cx,
                    );
                    return;
                }
                let oauth_client_id = oauth_client_id
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());
                AgentMcpServerConfig::Http {
                    name: name.clone(),
                    url,
                    oauth_callback_port: oauth_client_id
                        .as_ref()
                        .and(oauth_callback_port.filter(|port| *port > 0)),
                    oauth_client_id,
                }
            }
            "stdio" => {
                let command = command.unwrap_or_default().trim().to_string();
                if command.is_empty() {
                    self.set_status(
                        self.tr(
                            "Enter the command that starts the MCP server.",
                            "Ingresa el comando que inicia el servidor MCP.",
                        ),
                        true,
                        cx,
                    );
                    return;
                }
                if env.keys().any(|key| {
                    key.is_empty()
                        || key
                            .chars()
                            .next()
                            .is_some_and(|character| character.is_ascii_digit())
                        || !key
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                }) {
                    self.set_status(
                        self.tr(
                            "One of the environment variable names is invalid.",
                            "Uno de los nombres de variables de entorno no es válido.",
                        ),
                        true,
                        cx,
                    );
                    return;
                }
                AgentMcpServerConfig::Stdio {
                    name: name.clone(),
                    command,
                    args: args
                        .into_iter()
                        .map(|argument| argument.trim().to_string())
                        .filter(|argument| !argument.is_empty())
                        .collect(),
                    env,
                }
            }
            _ => return,
        };

        let mut installed = self.installed_project_agent_mcps(workspace_id);
        if let Some(existing) = installed.iter_mut().find(|server| server.name() == name) {
            *existing = config;
        } else {
            installed.push(config);
        }
        installed.sort_by(|left, right| left.name().cmp(right.name()));
        if let Err(error) = self.save_installed_project_agent_mcps(workspace_id, &installed) {
            self.set_status(
                format!("Could not install the project MCP server: {error:#}"),
                true,
                cx,
            );
            return;
        }

        let authentication_key = self.project_mcp_authentication_key(workspace_id, &name);
        self.project_mcp_authentications.remove(&authentication_key);
        let _ = self.database.set_setting(&authentication_key, "needs-auth");

        let mut enabled = self.project_enabled_agent_mcp_names(workspace_id);
        enabled.insert(name.clone());
        if let Err(error) = self.save_agent_mcp_names(Some(workspace_id), &enabled) {
            self.set_status(
                format!("Could not enable the project MCP server: {error:#}"),
                true,
                cx,
            );
            return;
        }
        self.set_status(
            match self.session.language {
                Language::English => format!("MCP {name} installed for this project"),
                Language::Spanish => format!("MCP {name} instalado para este proyecto"),
            },
            false,
            cx,
        );
        self.hydrate_project_settings_surface(workspace_id, cx);
        cx.notify();
    }

    fn remove_project_agent_mcp(
        &mut self,
        workspace_id: Uuid,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let mut installed = self.installed_project_agent_mcps(workspace_id);
        let previous_len = installed.len();
        installed.retain(|server| server.name() != name);
        if installed.len() == previous_len {
            return;
        }
        if let Err(error) = self.save_installed_project_agent_mcps(workspace_id, &installed) {
            self.set_status(
                format!("Could not remove the project MCP server: {error:#}"),
                true,
                cx,
            );
            return;
        }
        let authentication_key = self.project_mcp_authentication_key(workspace_id, &name);
        self.project_mcp_authentications.remove(&authentication_key);
        let _ = self.database.set_setting(&authentication_key, "removed");
        let mut enabled = self.project_enabled_agent_mcp_names(workspace_id);
        enabled.remove(&name);
        if let Err(error) = self.save_agent_mcp_names(Some(workspace_id), &enabled) {
            self.set_status(
                format!("Could not update the project MCP configuration: {error:#}"),
                true,
                cx,
            );
            return;
        }
        self.set_status(
            match self.session.language {
                Language::English => format!("MCP {name} removed from this project"),
                Language::Spanish => format!("MCP {name} eliminado de este proyecto"),
            },
            false,
            cx,
        );
        self.hydrate_project_settings_surface(workspace_id, cx);
        cx.notify();
    }

    fn enabled_agent_skill_names(&self) -> HashSet<String> {
        if let Some(value) = self.database.setting("agent-enabled-skills").ok().flatten() {
            return serde_json::from_str::<Vec<String>>(&value)
                .unwrap_or_default()
                .into_iter()
                .collect();
        }
        self.agent_skills()
            .into_iter()
            .map(|skill| skill.name)
            .collect()
    }

    fn enabled_agent_skills(&self) -> Vec<String> {
        self.agent_skills_from_names(&self.enabled_agent_skill_names())
    }

    fn agent_skills_from_names(&self, enabled: &HashSet<String>) -> Vec<String> {
        let mut skills = self
            .agent_skills()
            .into_iter()
            .filter(|skill| enabled.contains(&skill.name))
            .map(|skill| format!("{BLACKHOLES_SKILLS_PLUGIN_NAME}:{}", skill.name))
            .collect::<Vec<_>>();
        skills.sort();
        skills
    }

    fn project_enabled_agent_skill_names(&self, workspace_id: Uuid) -> HashSet<String> {
        let globally_enabled = self.enabled_agent_skill_names();
        let key = format!("project-enabled-skills-{workspace_id}");
        let Some(value) = self.database.setting(&key).ok().flatten() else {
            return globally_enabled;
        };
        serde_json::from_str::<Vec<String>>(&value)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| globally_enabled.contains(name))
            .collect()
    }

    fn enabled_agent_skills_for_scope(&self, scope: OrchestratorChatScope) -> Vec<String> {
        let workspace_id = match scope {
            OrchestratorChatScope::Project(workspace_id)
            | OrchestratorChatScope::ProjectAgent {
                project_id: workspace_id,
                ..
            } => Some(workspace_id),
            OrchestratorChatScope::Task(task_id)
            | OrchestratorChatScope::TaskAgent { task_id, .. } => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| task.workspace_id),
            OrchestratorChatScope::Global | OrchestratorChatScope::GlobalAgent(_) => None,
        };
        workspace_id.map_or_else(
            || self.enabled_agent_skills(),
            |workspace_id| {
                self.agent_skills_from_names(&self.project_enabled_agent_skill_names(workspace_id))
            },
        )
    }

    fn save_enabled_agent_skills(&self, enabled: &HashSet<String>) -> Result<()> {
        let mut enabled = enabled.iter().cloned().collect::<Vec<_>>();
        enabled.sort();
        self.database
            .set_setting("agent-enabled-skills", &serde_json::to_string(&enabled)?)
    }

    fn save_project_enabled_agent_skills(
        &self,
        workspace_id: Uuid,
        enabled: &HashSet<String>,
    ) -> Result<()> {
        let mut enabled = enabled.iter().cloned().collect::<Vec<_>>();
        enabled.sort();
        self.database.set_setting(
            &format!("project-enabled-skills-{workspace_id}"),
            &serde_json::to_string(&enabled)?,
        )
    }

    fn set_agent_skill_enabled(&mut self, name: String, enabled: bool, cx: &mut Context<Self>) {
        if !self.agent_skills().iter().any(|skill| skill.name == name) {
            return;
        }
        let mut enabled_names = self.enabled_agent_skill_names();
        if enabled {
            enabled_names.insert(name.clone());
        } else {
            enabled_names.remove(&name);
        }
        match self.save_enabled_agent_skills(&enabled_names) {
            Ok(()) => self.set_status(
                if enabled {
                    self.tr("Skill enabled", "Skill activada")
                } else {
                    self.tr("Skill disabled", "Skill desactivada")
                },
                false,
                cx,
            ),
            Err(error) => self.set_status(
                format!("Could not save the skill configuration: {error:#}"),
                true,
                cx,
            ),
        }
        cx.notify();
    }

    fn set_project_agent_skill_enabled(
        &mut self,
        workspace_id: Uuid,
        name: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
            || !self.agent_skills().iter().any(|skill| skill.name == name)
            || !self.enabled_agent_skill_names().contains(&name)
        {
            return;
        }
        let mut enabled_names = self.project_enabled_agent_skill_names(workspace_id);
        if enabled {
            enabled_names.insert(name);
        } else {
            enabled_names.remove(&name);
        }
        if let Err(error) = self.save_project_enabled_agent_skills(workspace_id, &enabled_names) {
            self.set_status(
                format!("Could not save the project skill configuration: {error:#}"),
                true,
                cx,
            );
        }
        cx.notify();
    }

    fn update_project_instructions(
        &mut self,
        workspace_id: Uuid,
        content: String,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        if let Err(error) = ProjectInstructionsService::write(&workspace, &content) {
            self.set_status(
                format!("Could not save the project CLAUDE.md: {error:#}"),
                true,
                cx,
            );
        }
        cx.notify();
    }

    fn update_project_task_instructions(
        &mut self,
        workspace_id: Uuid,
        content: String,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        if let Err(error) = ProjectTaskInstructionsService::write(&workspace, &content) {
            self.set_status(
                format!("Could not save the shared task CLAUDE.md: {error:#}"),
                true,
                cx,
            );
            return;
        }
        cx.notify();

        let tasks = self
            .tasks
            .iter()
            .filter(|task| {
                task.workspace_id == workspace_id
                    && task
                        .worktree_root_path
                        .starts_with(&self.paths.task_workspaces)
                    && task.worktree_root_path.is_dir()
            })
            .cloned()
            .collect::<Vec<_>>();
        let paths = self.paths.clone();
        let background = cx.background_executor().spawn(async move {
            let task_service = TaskService::new(&paths);
            for task in tasks {
                task_service
                    .repair_task_files(&workspace, &task)
                    .with_context(|| {
                        format!(
                            "Could not update the shared instructions for task '{}'",
                            task.title
                        )
                    })?;
            }
            Ok::<(), anyhow::Error>(())
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                if let Err(error) = result {
                    app.set_status(
                        format!("Could not update every task CLAUDE.md: {error:#}"),
                        true,
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn import_agent_skills(&mut self, source: PathBuf, cx: &mut Context<Self>) {
        match AgentSkillService::import(&source, &self.paths.agent_skills_plugin) {
            Ok(report) => {
                let mut enabled = self.enabled_agent_skill_names();
                for skill in &report.imported {
                    enabled.insert(skill.name.clone());
                }
                if let Err(error) = self.save_enabled_agent_skills(&enabled) {
                    self.set_status(
                        format!("Could not save the imported skills: {error:#}"),
                        true,
                        cx,
                    );
                    return;
                }
                let message = match (report.imported.len(), report.errors.len()) {
                    (0, errors) => format!(
                        "{}: {}",
                        self.tr("No skills were imported", "No se importaron skills"),
                        report
                            .errors
                            .first()
                            .cloned()
                            .unwrap_or_else(|| format!("{errors} errors"))
                    ),
                    (imported, 0) => match self.session.language {
                        Language::English => format!("Imported and enabled {imported} skills"),
                        Language::Spanish => format!("Se importaron y activaron {imported} skills"),
                    },
                    (imported, errors) => match self.session.language {
                        Language::English => {
                            format!("Imported {imported} skills; {errors} could not be imported")
                        }
                        Language::Spanish => {
                            format!(
                                "Se importaron {imported} skills; {errors} no se pudieron importar"
                            )
                        }
                    },
                };
                self.set_status(message, report.imported.is_empty(), cx);
            }
            Err(error) => self.set_status(format!("Could not import skills: {error:#}"), true, cx),
        }
        cx.notify();
    }

    fn reveal_agent_skills(&mut self, cx: &mut Context<Self>) {
        let path = self.paths.agent_skills_plugin.join("skills");
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("open").arg(&path).spawn();
        #[cfg(target_os = "linux")]
        let result = std::process::Command::new("xdg-open").arg(&path).spawn();
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("explorer").arg(&path).spawn();

        if let Err(error) = result {
            self.set_status(
                format!("Could not reveal {}: {error}", path.display()),
                true,
                cx,
            );
        }
    }

    fn agent_provider(&self) -> AgentProvider {
        AgentProvider::from_setting(self.database.setting("agent-provider").ok().flatten())
    }

    fn set_agent_provider(&mut self, provider: AgentProvider, cx: &mut Context<Self>) {
        match self.database.set_setting("agent-provider", provider.id()) {
            Ok(()) => self.set_status(
                format!(
                    "{}: {}",
                    self.tr("Agent provider updated", "Proveedor de agentes actualizado"),
                    provider.display_name()
                ),
                false,
                cx,
            ),
            Err(error) => self.set_status(
                format!("Could not save the agent provider: {error:#}"),
                true,
                cx,
            ),
        }
        self.invalidate_model_catalog(cx);
        self.invalidate_plan_usage(cx);
        cx.notify();
    }

    fn agent_auth_mode(&self, provider: AgentProvider) -> AgentAuthMode {
        AgentAuthMode::from_setting(
            self.database
                .setting(&format!("agent-auth-{}", provider.id()))
                .ok()
                .flatten(),
        )
    }

    fn set_agent_auth_mode(
        &mut self,
        provider: AgentProvider,
        auth_mode: AgentAuthMode,
        cx: &mut Context<Self>,
    ) {
        match self
            .database
            .set_setting(&format!("agent-auth-{}", provider.id()), auth_mode.id())
        {
            Ok(()) => self.set_status(
                self.tr(
                    "Authentication profile updated for upcoming responses",
                    "Perfil de autenticación actualizado para las próximas respuestas",
                ),
                false,
                cx,
            ),
            Err(error) => self.set_status(
                format!("Could not save the authentication profile: {error:#}"),
                true,
                cx,
            ),
        }
        self.invalidate_model_catalog(cx);
        self.invalidate_plan_usage(cx);
        cx.notify();
    }

    fn authenticate_agent_provider(
        &mut self,
        provider: AgentProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Dropping a previous flow also stops its provider process. This keeps
        // account changes isolated and prevents two OAuth attempts competing.
        self.agent_authentication = None;
        let stream = match start_agent_authentication(provider, &self.paths.agent_profiles) {
            Ok(stream) => stream,
            Err(error) => {
                self.set_status(
                    format!(
                        "{}: {error:#}",
                        self.tr(
                            "Could not start authentication",
                            "No se pudo iniciar la autenticación"
                        )
                    ),
                    true,
                    cx,
                );
                return;
            }
        };

        let authentication_id = Uuid::new_v4();
        let placeholder = self
            .tr(
                "Paste the authorization code",
                "Pega el código de autorización",
            )
            .to_string();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let events = stream.events;
        self.agent_authentication = Some(AgentAuthentication {
            id: authentication_id,
            provider,
            status: AgentAuthStatus::Connecting,
            detail: self
                .tr(
                    "Preparing secure sign-in…",
                    "Preparando el inicio de sesión seguro…",
                )
                .to_string(),
            output: String::new(),
            opened_url: None,
            input,
            input_sender: stream.input,
            cancel: Some(stream.cancel),
        });

        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let terminal = matches!(
                    &event,
                    AgentAuthEvent::Completed | AgentAuthEvent::Error { .. }
                );
                if this
                    .update(cx, |app, cx| {
                        app.handle_agent_auth_event(authentication_id, provider, event, cx)
                    })
                    .is_err()
                {
                    break;
                }
                if terminal {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn handle_agent_auth_event(
        &mut self,
        authentication_id: Uuid,
        provider: AgentProvider,
        event: AgentAuthEvent,
        cx: &mut Context<Self>,
    ) {
        let spanish = self.session.language == Language::Spanish;
        let Some(authentication) = self.agent_authentication.as_mut().filter(|authentication| {
            authentication.id == authentication_id && authentication.provider == provider
        }) else {
            return;
        };

        let completed = matches!(&event, AgentAuthEvent::Completed);
        match event {
            AgentAuthEvent::Output { text } => {
                authentication.output.push_str(&text);
                if authentication.output.len() > 65_536 {
                    let mut keep_from = authentication.output.len() - 32_768;
                    while !authentication.output.is_char_boundary(keep_from) {
                        keep_from += 1;
                    }
                    authentication.output.drain(..keep_from);
                }
                let normalized = text.to_ascii_lowercase();
                let asks_for_code = (normalized.contains("paste")
                    || normalized.contains("enter")
                    || normalized.contains("introduce")
                    || normalized.contains("pega"))
                    && (normalized.contains("code")
                        || normalized.contains("token")
                        || normalized.contains("código"));
                if asks_for_code {
                    authentication.status = AgentAuthStatus::NeedsInput;
                    authentication.detail = if spanish {
                        "El proveedor solicita un código. Pégalo aquí para continuar."
                    } else {
                        "The provider requested a code. Paste it here to continue."
                    }
                    .to_string();
                }
            }
            AgentAuthEvent::OpenUrl { url } => {
                if authentication.opened_url.as_deref() != Some(url.as_str()) {
                    authentication.opened_url = Some(url.clone());
                    authentication.status = AgentAuthStatus::Connecting;
                    authentication.detail = if spanish {
                        "Continúa el inicio de sesión en el navegador y vuelve a Blackholes. Si no se abrió, usa el botón de abajo."
                    } else {
                        "Continue signing in through your browser, then return to Blackholes. If it did not open, use the button below."
                    }
                    .to_string();
                }
            }
            AgentAuthEvent::Completed => {
                authentication.status = AgentAuthStatus::Connected;
                authentication.cancel = None;
                authentication.detail = if spanish {
                    format!("{} quedó conectado a Blackholes.", provider.display_name())
                } else {
                    format!(
                        "{} is now connected to Blackholes.",
                        provider.display_name()
                    )
                };
                if let Err(error) = self.database.set_setting(
                    &format!("agent-auth-{}", provider.id()),
                    AgentAuthMode::Isolated.id(),
                ) {
                    authentication.status = AgentAuthStatus::Error;
                    authentication.detail = format!(
                        "{}: {error:#}",
                        if spanish {
                            "La cuenta se autenticó, pero no se pudo guardar la selección"
                        } else {
                            "The account was authenticated, but the selection could not be saved"
                        }
                    );
                }
            }
            AgentAuthEvent::Error { message } => {
                authentication.status = AgentAuthStatus::Error;
                authentication.cancel = None;
                let useful_output = authentication
                    .output
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|line| {
                        !line.is_empty() && !line.contains("https://") && !line.contains("http://")
                    })
                    .map(|line| line.chars().take(280).collect::<String>());
                authentication.detail = match useful_output {
                    Some(output) if output != message => format!("{message}. {output}"),
                    _ => message,
                };
            }
        }
        if completed {
            // A newly authenticated identity must not inherit the previous account's selection.
            let _ = self.database.set_setting(&format!("agent-model-{}-isolated", provider.id()), "automatic");
            let _ = self.database.set_setting(&format!("agent-effort-{}-isolated", provider.id()), "automatic");
            self.invalidate_model_catalog(cx);
            self.invalidate_plan_usage(cx);
        }
        cx.notify();
    }

    fn submit_agent_auth_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(authentication) = self.agent_authentication.as_ref() else {
            return;
        };
        let value = authentication.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        let input = authentication.input.clone();
        let sender = authentication.input_sender.clone();
        if sender.send(value).is_err() {
            let spanish = self.session.language == Language::Spanish;
            if let Some(authentication) = self.agent_authentication.as_mut() {
                authentication.status = AgentAuthStatus::Error;
                authentication.detail = if spanish {
                    "El proceso de autenticación ya no está disponible."
                } else {
                    "The authentication process is no longer available."
                }
                .to_string();
            }
        } else {
            input.update(cx, |input, cx| input.set_value("", window, cx));
            let spanish = self.session.language == Language::Spanish;
            if let Some(authentication) = self.agent_authentication.as_mut() {
                authentication.status = AgentAuthStatus::Connecting;
                authentication.detail = if spanish {
                    "Verificando el código…"
                } else {
                    "Verifying the code…"
                }
                .to_string();
            }
        }
        cx.notify();
    }

    fn submit_agent_auth_value(&mut self, value: String, cx: &mut Context<Self>) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let spanish = self.session.language == Language::Spanish;
        let Some(authentication) = self.agent_authentication.as_mut() else {
            return;
        };
        if authentication.input_sender.send(value.to_string()).is_err() {
            authentication.status = AgentAuthStatus::Error;
            authentication.detail = if spanish {
                "El proceso de autenticación ya no está disponible."
            } else {
                "The authentication process is no longer available."
            }
            .to_string();
        } else {
            authentication.status = AgentAuthStatus::Connecting;
            authentication.detail = if spanish {
                "Verificando el código…"
            } else {
                "Verifying the code…"
            }
            .to_string();
        }
        cx.notify();
    }

    fn cancel_agent_authentication(&mut self, cx: &mut Context<Self>) {
        self.agent_authentication = None;
        cx.notify();
    }

    fn model_catalog_context(&self, provider: AgentProvider) -> (String, PathBuf) {
        // Account catalogs should not disappear when navigating between chats.
        // OpenCode additionally resolves project-local provider configuration.
        let cwd = if provider == AgentProvider::OpenCode {
            self.orchestrator_runtime(self.active_orchestrator_scope)
                .map(|(cwd, _, _)| cwd).unwrap_or_else(|| self.projects_root())
        } else {
            self.projects_root()
        };
        (format!("{}:{}:{}", provider.id(), self.agent_auth_mode(provider).id(), cwd.display()), cwd)
    }

    fn current_model_catalog(&self, provider: AgentProvider) -> Option<&AgentModelCatalog> {
        let (key, _) = self.model_catalog_context(provider);
        (key == self.model_catalog_key)
            .then_some(self.model_catalog.as_ref()).flatten()
    }

    fn refresh_model_catalog(&mut self, force: bool, cx: &mut Context<Self>) {
        let provider = self.agent_provider();
        self.migrate_agent_preferences(provider);
        let auth_mode = self.agent_auth_mode(provider);
        let (key, cwd) = self.model_catalog_context(provider);
        if key == self.model_catalog_key {
            if self.model_catalog_loading { return; }
            if !force && self.model_catalog_checked.is_some() { return; }
        }
        if let Some(previous) = self.model_catalog_cancel.take() {
            previous.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.model_catalog_cancel = Some(cancel.clone());
        self.model_catalog_generation = self.model_catalog_generation.wrapping_add(1);
        let generation = self.model_catalog_generation;
        if self.model_catalog_key != key {
            self.model_catalog = None;
        }
        self.model_catalog_key = key.clone();
        self.model_catalog_loading = true;
        self.model_catalog_error = false;
        self.model_catalog_checked = None;
        let profile = self.paths.agent_profiles.join(provider.id());
        let background = cx.background_executor().spawn(async move {
            refresh_agent_models(provider, auth_mode, profile, cwd, cancel)
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                // Generation protects against account A → B → A and reauthentication.
                if app.model_catalog_generation != generation || app.model_catalog_key != key { return; }
                app.model_catalog_loading = false;
                app.model_catalog_cancel = None;
                app.model_catalog_checked = Some(std::time::Instant::now());
                match result {
                    Ok(catalog) => app.model_catalog = Some(catalog),
                    Err(_) => { app.model_catalog_error = true; }
                }
                app.dispatch_agent_model_state(app.agent_provider(), cx);
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
            });
        }).detach();
        self.dispatch_agent_model_state(provider, cx);
        self.hydrate_active_workspace_surface(cx);
        cx.notify();
    }

    fn invalidate_model_catalog(&mut self, cx: &mut Context<Self>) {
        self.model_catalog_generation = self.model_catalog_generation.wrapping_add(1);
        self.model_catalog_loading = false;
        self.model_catalog_checked = None;
        self.model_catalog = None;
        self.refresh_model_catalog(true, cx);
    }

    fn agent_preference_key(&self, provider: AgentProvider, kind: &str) -> String {
        format!("agent-{kind}-{}-{}", provider.id(), self.agent_auth_mode(provider).id())
    }

    fn migrate_agent_preferences(&self, provider: AgentProvider) {
        // Preserve existing choices once, in the profile that was active during upgrade.
        // Other accounts begin at their own provider default instead of inheriting them.
        let marker = format!("model-catalog-migrated-{}", provider.id());
        if self.database.setting(&marker).ok().flatten().as_deref() == Some("1") { return; }
        for kind in ["model", "effort"] {
            let key = self.agent_preference_key(provider, kind);
            if self.database.setting(&key).ok().flatten().is_some() { continue; }
            let legacy = if provider == AgentProvider::Claude && kind == "model" {
                "claude-agent-model".to_string()
            } else { format!("agent-{kind}-{}", provider.id()) };
            let value = self.database.setting(&legacy).ok().flatten().unwrap_or_else(|| "automatic".to_string());
            if self.database.set_setting(&key, &value).is_err() { return; }
        }
        let _ = self.database.set_setting(&marker, "1");
    }

    fn agent_model(&self, provider: AgentProvider) -> Option<String> {
        self.database.setting(&self.agent_preference_key(provider, "model")).ok().flatten()
            .filter(|model| model != "automatic" && !model.trim().is_empty())
    }

    fn model_is_available(&self, provider: AgentProvider, model: &str) -> bool {
        model == "automatic" || self.current_model_catalog(provider).is_some_and(|catalog|
            catalog.models.iter().any(|entry| entry.id == model || entry.aliases.iter().any(|alias| alias == model)))
    }

    fn agent_model_options(&self, provider: AgentProvider) -> Vec<(String, String)> {
        let mut options = vec![("automatic".to_string(), self.tr("Provider default", "Predeterminado del proveedor").to_string())];
        if let Some(catalog) = self.current_model_catalog(provider) {
            options.extend(catalog.models.iter().map(|model| (model.id.clone(), model.label.clone())));
        }
        if let Some(selected) = self.agent_model(provider) {
            if !options.iter().any(|(id, _)| id == &selected) {
                let label = self.current_model_catalog(provider).and_then(|catalog|
                    catalog.models.iter().find(|model| model.aliases.contains(&selected)).map(|model| model.label.clone()))
                    .unwrap_or_else(|| format!("{} · {}", selected, self.tr("Unavailable / unverified", "No disponible / sin verificar")));
                options.push((selected, label));
            }
        }
        options
    }

    fn selected_model_info(&self, provider: AgentProvider) -> Option<&AgentModelInfo> {
        let catalog = self.current_model_catalog(provider)?;
        let selected = self.agent_model(provider).or_else(|| catalog.default_model.clone())?;
        catalog.models.iter().find(|model| model.id == selected || model.aliases.contains(&selected))
    }

    fn agent_model_choices(&self, provider: AgentProvider) -> Vec<serde_json::Value> {
        let mut available = HashSet::from(["automatic"]);
        if let Some(catalog) = self.current_model_catalog(provider) {
            for model in &catalog.models {
                available.insert(model.id.as_str());
                available.extend(model.aliases.iter().map(String::as_str));
            }
        }
        self.agent_model_options(provider).into_iter().map(|(value, label)|
            serde_json::json!({ "disabled": !available.contains(value.as_str()), "value": value, "label": label })).collect()
    }

    fn agent_effort_options(&self, provider: AgentProvider) -> Vec<(String, String)> {
        let Some(model) = self.selected_model_info(provider) else { return Vec::new(); };
        if model.efforts.is_empty() { return Vec::new(); }
        let mut options = vec![("automatic".to_string(), self.tr("Automatic", "Automático").to_string())];
        options.extend(model.efforts.iter().map(|effort| (effort.clone(), match effort.as_str() {
            "low" => self.tr("Low", "Bajo").to_string(),
            "medium" => self.tr("Medium", "Medio").to_string(),
            "high" => self.tr("High", "Alto").to_string(),
            "xhigh" => self.tr("Extra high", "Extra alto").to_string(),
            "max" => self.tr("Maximum", "Máximo").to_string(),
            _ => effort.clone(),
        })));
        options
    }

    fn selected_agent_model(&self, provider: AgentProvider) -> (String, String) {
        let selected = self
            .agent_model(provider)
            .unwrap_or_else(|| "automatic".to_string());
        let label = self
            .agent_model_options(provider)
            .into_iter()
            .find_map(|(value, label)| (value == selected).then_some(label))
            .unwrap_or_else(|| selected.clone());
        (selected, label)
    }

    fn dispatch_agent_model_state(&self, provider: AgentProvider, cx: &mut Context<Self>) {
        let (model, model_label) = self.selected_agent_model(provider);
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "model_changed",
                "provider_label": provider.model_brand_name(),
                "model": model,
                "model_label": model_label,
                "model_options": self.agent_model_choices(provider),
                "model_catalog_loading": self.model_catalog_loading,
                "model_catalog_error": self.model_catalog_error,
                "model_control_supported": provider.supports_model_selection(),
            }),
            cx,
        );
    }

    fn set_agent_model(&mut self, provider: AgentProvider, model: &str, cx: &mut Context<Self>) {
        if !self.model_is_available(provider, model) {
            return;
        }
        match self
            .database
            .set_setting(&self.agent_preference_key(provider, "model"), model)
        {
            Ok(()) => {
                self.set_status(
                    self.tr(
                        "Agent model updated for upcoming responses",
                        "Modelo actualizado para las próximas respuestas",
                    ),
                    false,
                    cx,
                );
                self.dispatch_agent_model_state(provider, cx);
            }
            Err(error) => self.set_status(
                format!("Could not save the agent model: {error:#}"),
                true,
                cx,
            ),
        }
        self.hydrate_active_workspace_surface(cx);
        cx.notify();
    }

    fn agent_effort(&self, provider: AgentProvider) -> Option<String> {
        self.database
            .setting(&self.agent_preference_key(provider, "effort"))
            .ok()
            .flatten()
            .filter(|effort| effort != "automatic" && self.agent_effort_options(provider).iter().any(|(value, _)| value == effort))
    }

    fn set_agent_effort(&mut self, provider: AgentProvider, effort: &str, cx: &mut Context<Self>) {
        if effort != "automatic" && !self.agent_effort_options(provider).iter().any(|(value, _)| value == effort) {
            return;
        }
        match self
            .database
            .set_setting(&self.agent_preference_key(provider, "effort"), effort)
        {
            Ok(()) => self.set_status(
                self.tr(
                    "Reasoning effort updated for upcoming responses",
                    "Esfuerzo de razonamiento actualizado para las próximas respuestas",
                ),
                false,
                cx,
            ),
            Err(error) => self.set_status(
                format!("Could not save the reasoning effort: {error:#}"),
                true,
                cx,
            ),
        }
        self.hydrate_active_workspace_surface(cx);
        cx.notify();
    }

    fn set_agents_full_access(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match self
            .database
            .set_setting("agents-full-access", if enabled { "true" } else { "false" })
        {
            Ok(()) => {
                self.set_status(
                    if enabled {
                        self.tr(
                            "Full agent access enabled",
                            "Acceso total de agentes activado",
                        )
                    } else {
                        self.tr(
                            "Standard agent permissions enabled",
                            "Permisos estándar de agentes activados",
                        )
                    },
                    false,
                    cx,
                );
                self.dispatch_orchestrator_event(
                    serde_json::json!({
                        "type": "permissions_changed",
                        "full_access": enabled,
                        "permission_control_supported": self.agent_provider().supports_permission_mode(),
                    }),
                    cx,
                );
            }
            Err(error) => self.set_status(
                format!("Could not save agent permissions: {error:#}"),
                true,
                cx,
            ),
        }
        cx.notify();
    }

    fn set_projects_root(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !path.is_dir() {
            self.set_status(self.tr("Choose a directory", "Elige una carpeta"), true, cx);
            return;
        }
        match self
            .database
            .set_setting("projects-root-path", &path.to_string_lossy())
        {
            Ok(()) => self.set_status(
                self.tr(
                    "Projects folder updated",
                    "Carpeta de proyectos actualizada",
                ),
                false,
                cx,
            ),
            Err(error) => self.set_status(
                format!("Could not save the projects folder: {error:#}"),
                true,
                cx,
            ),
        }
    }

    fn persist_session(&mut self) {
        if let Err(error) = self.database.save_session(&self.session) {
            self.status = Some((format!("Could not save the session: {error:#}"), true));
        }
    }

    fn set_status(&mut self, message: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.status = Some((message.into(), error));
        self.status_revision = self.status_revision.wrapping_add(1);
        let revision = self.status_revision;
        let expected_status = self.status.clone();
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            Timer::after(Duration::from_secs(4)).await;
            let _ = weak.update(cx, |app, cx| {
                if app.status_revision != revision || app.status != expected_status {
                    return;
                }
                app.status = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_target(
        &mut self,
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        repository_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        self.select_target_surface(workspace_id, task_id, repository_id, false, cx);
    }

    fn select_repository_target(
        &mut self,
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        repository_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        self.select_target_surface(workspace_id, task_id, Some(repository_id), true, cx);
        self.publish_workbench_surface(cx);
        if let Some((path, _)) = self.selected_repository_target() {
            self.request_repository_git_summaries([path], cx);
        }
    }

    fn select_target_surface(
        &mut self,
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        repository_id: Option<Uuid>,
        show_file_explorer: bool,
        cx: &mut Context<Self>,
    ) {
        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = task_id;
        self.session.selected_repository_id = repository_id;
        if let Some(task_id) = task_id {
            insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
            insert_unique(&mut self.session.expanded_task_ids, task_id);
        }
        self.show_project_note = false;
        self.show_task_note = task_id.is_some() && repository_id.is_none();
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        if task_id.is_none() && repository_id.is_none() && !show_file_explorer {
            let project_scope = OrchestratorChatScope::Project(workspace_id);
            self.flush_active_file(cx);
            self.active_file = None;
            self.active_diff = None;
            self.quick_open = None;
            self.active_orchestrator_scope = if self.orchestrator_chats.has_agent(project_scope) {
                project_scope
            } else {
                self.orchestrator_chats.project_agent_ids(workspace_id)
                    .iter().copied()
                    .map(|agent_id| OrchestratorChatScope::ProjectAgent {
                        project_id: workspace_id,
                        agent_id,
                    })
                    .find(|scope| self.orchestrator_chats.has_agent(*scope))
                    .unwrap_or(project_scope)
            };
            self.hydrate_orchestrator_chat(cx);
        }
        if show_file_explorer {
            self.sync_file_explorer_to_selection(cx);
        } else {
            self.close_file_explorer(cx);
        }
        self.persist_session();
        cx.notify();
    }

    fn toggle_workspace_expanded(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        toggle_id(&mut self.session.expanded_workspace_ids, workspace_id);
        self.session.navigation_expansion_initialized = true;
        if self.session.expanded_workspace_ids.contains(&workspace_id) {
            self.request_workspace_git_summaries(workspace_id, cx);
        }
        self.persist_session();
        cx.notify();
    }

    fn toggle_task_expanded(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        toggle_id(&mut self.session.expanded_task_ids, task_id);
        self.session.navigation_expansion_initialized = true;
        if self.session.expanded_task_ids.contains(&task_id) {
            self.request_task_git_summaries(task_id, cx);
        }
        self.persist_session();
        cx.notify();
    }

    fn collapse_all_navigation(&mut self, cx: &mut Context<Self>) {
        self.session.expanded_workspace_ids.clear();
        self.session.expanded_task_ids.clear();
        self.session.navigation_expansion_initialized = true;
        self.persist_session();
        cx.notify();
    }

    fn create_empty_project(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        match ProjectService::create_git_repository(&self.projects_root(), name) {
            Ok(workspace) => {
                if let Err(error) = ProjectNoteService::ensure(&workspace, "") {
                    self.set_status(
                        format!("Could not create the project note: {error:#}"),
                        true,
                        cx,
                    );
                    return false;
                }
                let index = self.workspaces.len();
                if let Err(error) = self.database.upsert_workspace(&workspace, index) {
                    self.set_status(format!("Could not save the project: {error:#}"), true, cx);
                    return false;
                }
                let id = workspace.id;
                insert_unique(&mut self.session.expanded_workspace_ids, id);
                self.workspaces.push(workspace);
                self.request_workspace_git_summaries(id, cx);
                self.select_target(id, None, None, cx);
                self.set_status("Project created", false, cx);
                true
            }
            Err(error) => {
                self.set_status(format!("Could not create the project: {error:#}"), true, cx);
                false
            }
        }
    }

    fn finish_background_workspace(&mut self, result: Result<Workspace>, cx: &mut Context<Self>) {
        self.busy = None;
        match result {
            Ok(workspace) => {
                if self
                    .workspaces
                    .iter()
                    .any(|current| current.root_path == workspace.root_path)
                {
                    self.set_status("That project is already in Blackholes", true, cx);
                    return;
                }
                if let Err(error) = ProjectNoteService::ensure(&workspace, "") {
                    self.set_status(
                        format!("Could not create the project note: {error:#}"),
                        true,
                        cx,
                    );
                    return;
                }
                let index = self.workspaces.len();
                if let Err(error) = self.database.upsert_workspace(&workspace, index) {
                    self.set_status(format!("Could not save the project: {error:#}"), true, cx);
                    return;
                }
                let id = workspace.id;
                insert_unique(&mut self.session.expanded_workspace_ids, id);
                self.workspaces.push(workspace);
                self.request_workspace_git_summaries(id, cx);
                self.select_target(id, None, None, cx);
                self.set_status("Project ready", false, cx);
            }
            Err(error) => self.set_status(format!("Project operation failed: {error:#}"), true, cx),
        }
    }

    fn finish_background_task(&mut self, result: Result<ProjectTask>, cx: &mut Context<Self>) {
        self.busy = None;
        match result {
            Ok(mut task) => {
                task.sort_order = self
                    .tasks
                    .iter()
                    .filter(|current| current.workspace_id == task.workspace_id)
                    .count() as i64;
                if let Err(error) = self.database.upsert_task(&task) {
                    self.set_status(format!("Could not save the task: {error:#}"), true, cx);
                    return;
                }
                let task_id = task.id;
                self.tasks.push(task);
                // The user created this task from the app and is waiting on it.
                self.reveal_new_task(task_id, true, cx);
                self.set_status("Task workspace ready", false, cx);
            }
            Err(error) => {
                self.set_status(format!("Could not create the task: {error:#}"), true, cx)
            }
        }
    }

    fn finish_updated_task(&mut self, result: Result<ProjectTask>, cx: &mut Context<Self>) {
        self.busy = None;
        match result {
            Ok(task) => {
                let task_id = task.id;
                if let Err(error) = self.database.upsert_task(&task) {
                    self.set_status(format!("Could not save the task: {error:#}"), true, cx);
                    return;
                }
                if let Some(current) = self.tasks.iter_mut().find(|current| current.id == task.id) {
                    *current = task;
                }
                self.request_task_git_summaries(task_id, cx);
                self.set_status("Task updated", false, cx);
            }
            Err(error) => {
                self.set_status(format!("Could not update the task: {error:#}"), true, cx)
            }
        }
    }

    fn finish_detached_task_repositories(
        &mut self,
        result: Result<(ProjectTask, Vec<RemovedTaskRepository>)>,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        let (task, removed) = match result {
            Ok(value) => value,
            Err(error) => {
                return self.set_status(
                    format!("Could not remove the repositories: {error:#}"),
                    true,
                    cx,
                );
            }
        };
        if let Err(error) = self.database.upsert_task(&task) {
            self.set_status(format!("Could not save the task: {error:#}"), true, cx);
            return;
        }
        let task_id = task.id;
        let detached = removed
            .iter()
            .map(|repository| repository.repository_id)
            .collect::<HashSet<_>>();
        for repository in &removed {
            self.repository_git_summaries
                .remove(&repository.worktree_path);
            self.repository_git_requests
                .remove(&repository.worktree_path);
            self.repository_git_refresh_pending
                .remove(&repository.worktree_path);
            self.repository_git_save_requests
                .remove(&repository.worktree_path);
        }
        if let Some(current) = self.tasks.iter_mut().find(|current| current.id == task_id) {
            *current = task;
        }
        // A terminal rooted in a worktree that is gone can never be revived.
        let orphaned = self
            .session
            .terminals
            .iter()
            .filter(|terminal| {
                terminal.task_id == Some(task_id)
                    && terminal
                        .repository_id
                        .is_some_and(|repository_id| detached.contains(&repository_id))
            })
            .map(|terminal| terminal.id)
            .collect::<Vec<_>>();
        for terminal_id in orphaned {
            self.close_terminal(terminal_id, cx);
        }
        if self.session.selected_task_id == Some(task_id)
            && self
                .session
                .selected_repository_id
                .is_some_and(|repository_id| detached.contains(&repository_id))
        {
            self.session.selected_repository_id = None;
        }
        self.persist_session();
        let deleted_branches = removed
            .iter()
            .filter(|repository| repository.branch_deleted)
            .count();
        let worktrees = removed.len();
        self.set_status(
            match deleted_branches {
                0 => format!("{worktrees} worktree(s) removed; Git branches were preserved"),
                deleted => format!("{worktrees} worktree(s) removed; {deleted} branch(es) deleted"),
            },
            false,
            cx,
        );
        self.request_task_git_summaries(task_id, cx);
        cx.notify();
    }

    fn finish_removed_task(&mut self, task_id: Uuid, result: Result<()>, cx: &mut Context<Self>) {
        self.busy = None;
        match result {
            Ok(()) => {
                let removed_task = self.tasks.iter().find(|task| task.id == task_id).cloned();
                if let Err(error) = self.database.remove_task(task_id) {
                    self.set_status(
                        format!("Could not remove the task record: {error:#}"),
                        true,
                        cx,
                    );
                    return;
                }
                let terminal_ids = self
                    .session
                    .terminals
                    .iter()
                    .filter(|terminal| terminal.task_id == Some(task_id))
                    .map(|terminal| terminal.id)
                    .collect::<HashSet<_>>();
                for terminal_id in &terminal_ids {
                    if let Some(handle) = self.terminals.remove(terminal_id) {
                        let _ = handle.child.lock().kill();
                    }
                }
                self.app_toasts.retain(|toast| {
                    toast
                        .target
                        .terminal_id()
                        .is_none_or(|terminal_id| !terminal_ids.contains(&terminal_id))
                });
                if let Some(task) = removed_task.as_ref() {
                    for repository in &task.repositories {
                        self.repository_git_summaries
                            .remove(&repository.worktree_path);
                        self.repository_git_requests
                            .remove(&repository.worktree_path);
                        self.repository_git_refresh_pending
                            .remove(&repository.worktree_path);
                        self.repository_git_save_requests
                            .remove(&repository.worktree_path);
                    }
                }
                self.tasks.retain(|task| task.id != task_id);
                self.orchestrator_chats.remove_task(task_id);
                if self.active_orchestrator_scope.task_id() == Some(task_id) {
                    self.active_orchestrator_scope = OrchestratorChatScope::Global;
                }
                self.persist_orchestrator_chats();
                self.session
                    .unseen_task_ids
                    .retain(|current| *current != task_id);
                self.app_toasts
                    .retain(|toast| toast.target.task_id() != Some(task_id));
                self.task_notes.remove(&task_id);
                self.session
                    .expanded_task_ids
                    .retain(|current| *current != task_id);
                self.session
                    .terminals
                    .retain(|terminal| terminal.task_id != Some(task_id));
                self.session
                    .docks
                    .retain(|key, _| !key.contains(&task_id.to_string()));
                if self.session.selected_task_id == Some(task_id) {
                    self.session.selected_task_id = None;
                    self.session.selected_repository_id = None;
                    self.close_file_explorer(cx);
                }
                self.persist_session();
                self.set_status(
                    "Task, terminals, and worktrees removed; Git branches were preserved",
                    false,
                    cx,
                );
            }
            Err(error) => {
                self.set_status(format!("Could not remove the task: {error:#}"), true, cx)
            }
        }
    }

    fn update_project_name(
        &mut self,
        workspace_id: Uuid,
        name: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let icon = self.workspaces[index].icon.clone();
        let color = self.workspaces[index].color;
        self.update_project_presentation(workspace_id, name, icon, color, cx)
    }

    fn refresh_project_repositories(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let Some(root_path) = self.workspaces[index].root_path.clone() else {
            self.set_status(
                self.tr(
                    "This project does not have a root folder",
                    "Este proyecto no tiene una carpeta raíz",
                ),
                true,
                cx,
            );
            return;
        };
        let discovered = match discover_repositories(&root_path) {
            Ok(repositories) => repositories,
            Err(error) => {
                self.set_status(
                    format!(
                        "{}: {error:#}",
                        self.tr(
                            "Could not scan project repositories",
                            "No se pudieron buscar repositorios del proyecto",
                        )
                    ),
                    true,
                    cx,
                );
                return;
            }
        };
        let existing_paths = self.workspaces[index]
            .repositories
            .iter()
            .map(|repository| repository.path.clone())
            .collect::<HashSet<_>>();
        let ignored_paths = self.workspaces[index]
            .ignored_repository_paths
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let new_repositories = discovered
            .into_iter()
            .filter(|repository| {
                !existing_paths.contains(&repository.path)
                    && !ignored_paths.contains(&repository.path)
            })
            .collect::<Vec<_>>();
        if new_repositories.is_empty() {
            self.set_status(
                self.tr(
                    "No new repositories were found",
                    "No se encontraron repositorios nuevos",
                ),
                false,
                cx,
            );
            return;
        }

        let added_count = new_repositories.len();
        let new_paths = new_repositories
            .iter()
            .map(|repository| repository.path.clone())
            .collect::<Vec<_>>();
        self.workspaces[index].repositories.extend(new_repositories);
        self.workspaces[index]
            .repositories
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.workspaces[index].layout = if self.workspaces[index].repositories.len() == 1
            && self.workspaces[index].repositories[0].path == root_path
        {
            WorkspaceLayout::SingleRepository
        } else {
            WorkspaceLayout::MultiRepository
        };
        self.workspaces[index].updated_at = Utc::now();
        if let Err(error) = self
            .database
            .upsert_workspace(&self.workspaces[index], index)
        {
            self.reload_external_data(cx);
            self.set_status(
                format!(
                    "{}: {error:#}",
                    self.tr(
                        "Could not save discovered repositories",
                        "No se pudieron guardar los repositorios encontrados",
                    )
                ),
                true,
                cx,
            );
            return;
        }
        self.request_repository_git_summaries(new_paths, cx);
        let message = match (self.session.language, added_count) {
            (Language::English, 1) => "Repository added".to_string(),
            (Language::English, count) => format!("{count} repositories added"),
            (Language::Spanish, 1) => "Repositorio agregado".to_string(),
            (Language::Spanish, count) => format!("{count} repositorios agregados"),
        };
        self.set_status(message, false, cx);
    }

    fn update_project_presentation(
        &mut self,
        workspace_id: Uuid,
        name: String,
        icon: String,
        color: WorkspaceColor,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let result =
            ProjectService::update_presentation(&mut self.workspaces[index], name, icon, color)
                .and_then(|_| {
                    self.database
                        .upsert_workspace(&self.workspaces[index], index)
                });
        match result {
            Ok(()) => {
                self.status = None;
                cx.notify();
                true
            }
            Err(error) => {
                self.set_status(format!("Could not update the project: {error:#}"), true, cx);
                false
            }
        }
    }

    fn open_edit_project(
        &mut self,
        workspace_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        let language = self.session.language;
        let name =
            cx.new(|cx| InputState::new(window, cx).default_value(workspace.label().to_string()));
        let editor = cx.new(|_| ProjectAppearanceEditor {
            name,
            icon: workspace.icon.clone(),
            color: workspace.color,
            language,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let editor_submit = editor.clone();
            let weak_submit = weak.clone();
            let description = match language {
                Language::English => {
                    "Change how this project appears in Blackholes. Its folder and repositories will not be renamed."
                }
                Language::Spanish => {
                    "Cambia cómo aparece este proyecto en Blackholes. Su carpeta y repositorios no serán renombrados."
                }
            };
            dialog
                .title(match language {
                    Language::English => "Edit project",
                    Language::Spanish => "Editar proyecto",
                })
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(
                            div()
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_color(rgb(0x8e97aa))
                                .child(description),
                        )
                        .child(editor.clone()),
                )
                .button_props(DialogButtonProps::default().ok_text(match language {
                    Language::English => "Save changes",
                    Language::Spanish => "Guardar cambios",
                }))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let editor = editor_submit.read(cx);
                    let name = editor.name.read(cx).value().to_string();
                    let icon = editor.icon.clone();
                    let color = editor.color;
                    weak_submit
                        .update(cx, |app, cx| {
                            app.update_project_presentation(
                                workspace_id,
                                name,
                                icon,
                                color,
                                cx,
                            )
                        })
                        .unwrap_or(false)
                })
        });
    }

    fn update_task_details(
        &mut self,
        task_id: Uuid,
        title: String,
        description: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(workspace_id) = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.workspace_id)
        else {
            return false;
        };
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return false;
        };
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) else {
            return false;
        };
        let color = task.color;
        match TaskService::new(&self.paths)
            .update(&workspace, task, title, Some(description), color)
            .and_then(|_| self.database.upsert_task(task))
        {
            Ok(()) => {
                self.set_status("Task updated", false, cx);
                true
            }
            Err(error) => {
                self.set_status(format!("Could not update the task: {error:#}"), true, cx);
                false
            }
        }
    }

    fn remove_project_reference(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) -> bool {
        let task_ids = self
            .tasks
            .iter()
            .filter(|task| task.workspace_id == workspace_id)
            .map(|task| task.id)
            .collect::<HashSet<_>>();
        let agent_is_working = self.orchestrator_turns.keys().any(|scope| {
            scope.project_id() == Some(workspace_id)
                || scope.task_id().is_some_and(|id| task_ids.contains(&id))
        });
        if agent_is_working {
            self.set_status(
                self.tr(
                    "Wait for the project agent to finish before removing the project.",
                    "Espera a que termine el agente del proyecto antes de eliminarlo.",
                ),
                true,
                cx,
            );
            return false;
        }
        let terminal_ids = self
            .session
            .terminals
            .iter()
            .filter(|terminal| terminal.workspace_id == workspace_id)
            .map(|terminal| terminal.id)
            .collect::<Vec<_>>();
        for terminal_id in terminal_ids {
            self.close_terminal(terminal_id, cx);
        }
        if let Err(error) = self.database.remove_workspace(workspace_id) {
            self.set_status(
                format!("Could not remove the project reference: {error:#}"),
                true,
                cx,
            );
            return false;
        }
        self.workspaces
            .retain(|workspace| workspace.id != workspace_id);
        self.tasks.retain(|task| task.workspace_id != workspace_id);
        self.orchestrator_chats.remove_project(workspace_id);
        for task_id in &task_ids {
            self.orchestrator_chats.remove_task(*task_id);
        }
        self.active_orchestrator_scope = OrchestratorChatScope::Global;
        self.persist_orchestrator_chats();
        self.project_notes.remove(&workspace_id);
        self.task_notes
            .retain(|task_id, _| !task_ids.contains(task_id));
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        if self.project_settings_workspace_id == Some(workspace_id) {
            self.project_settings_workspace_id = None;
        }
        self.session
            .expanded_workspace_ids
            .retain(|current| *current != workspace_id);
        self.session
            .expanded_task_ids
            .retain(|current| !task_ids.contains(current));
        self.session
            .docks
            .retain(|key, _| !key.starts_with(&workspace_id.to_string()));
        self.session.selected_workspace_id = self.workspaces.first().map(|workspace| workspace.id);
        self.session.selected_task_id = None;
        self.session.selected_repository_id = None;
        self.flush_active_file(cx);
        self.active_file = None;
        self.close_file_explorer(cx);
        self.persist_session();
        self.set_status(
            "Project reference removed; files on disk were not touched",
            false,
            cx,
        );
        true
    }

    fn open_remove_project_confirmation(
        &mut self,
        workspace_id: Uuid,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project_name) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.label().to_string())
        else {
            return;
        };
        let title = self.tr(
            "Remove project from Blackholes?",
            "¿Eliminar proyecto de Blackholes?",
        );
        let description = self.tr(
            "The project, its tasks, and its terminals will be removed only from this application. Folders and files on disk will not be deleted.",
            "El proyecto, sus tareas y sus terminales se quitarán únicamente de esta aplicación. Las carpetas y los archivos del disco no se eliminarán.",
        );
        let remove_label = self.tr("Remove project", "Eliminar proyecto");
        if self.show_terminal {
            let weak = cx.weak_entity();
            _window.open_dialog(cx, move |dialog, _, _| {
                let weak_submit = weak.clone();
                dialog
                    .title(title)
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(project_name.clone()),
                            )
                            .child(description),
                    )
                    .button_props(DialogButtonProps::default().ok_text(remove_label))
                    .confirm()
                    .on_ok(move |_, _, cx| {
                        weak_submit
                            .update(cx, |app, cx| app.remove_project_reference(workspace_id, cx))
                            .unwrap_or(false)
                    })
            });
            return;
        }
        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "remove_project",
                    "workspace_id": workspace_id,
                    "title": title,
                    "name": project_name,
                    "description": description,
                    "confirm_label": remove_label,
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0),
                }
            }),
            cx,
        );
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": true }),
            cx,
        );
    }

    fn dismiss_app_modal(&mut self, cx: &mut Context<Self>) {
        self.project_modal_request = None;
        self.dispatch_orchestrator_event(
            serde_json::json!({ "type": "app_modal", "modal": null }),
            cx,
        );
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": false }),
            cx,
        );
    }

    fn open_manage_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.selected_task().cloned() {
            self.open_manage_task(task, window, cx);
        } else if let Some(workspace) = self.selected_workspace().cloned() {
            self.open_manage_project(workspace, window, cx);
        }
    }

    fn open_manage_project(
        &mut self,
        workspace: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name =
            cx.new(|cx| InputState::new(window, cx).default_value(workspace.label().to_string()));
        let weak = cx.weak_entity();
        let projects_root = self.projects_root();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let weak_duplicate = weak.clone();
            let weak_remove = weak.clone();
            let workspace_duplicate = workspace.clone();
            let workspace_remove = workspace.clone();
            let root_duplicate = projects_root.clone();
            let name_duplicate = name.clone();
            let content = v_flex()
                .gap_3()
                .child(Input::new(&name))
                .child(compact_button("duplicate-project", "Duplicate project on disk", move |_, window, cx| {
                    let duplicate_name = format!("{} copy", name_duplicate.read(cx).value());
                    let workspace = workspace_duplicate.clone();
                    let root = root_duplicate.clone();
                    let background = cx.background_executor().spawn(async move {
                        ProjectService::duplicate(&workspace, &root, &duplicate_name)
                    });
                    let weak = weak_duplicate.clone();
                    cx.spawn(async move |cx| {
                        let result = background.await;
                        let _ = weak.update(cx, |app, cx| app.finish_background_workspace(result, cx));
                    }).detach();
                    let _ = weak_duplicate.update(cx, |app, cx| {
                        app.busy = Some("Duplicating project…".into());
                        cx.notify();
                    });
                    window.close_dialog(cx);
                }))
                .child(compact_button("remove-project", "Remove project reference…", move |_, window, cx| {
                    let weak_confirm = weak_remove.clone();
                    let workspace_id = workspace_remove.id;
                    window.open_dialog(cx, move |dialog, _, _| {
                        let weak_submit = weak_confirm.clone();
                        dialog
                            .title("Remove project from Blackholes?")
                            .child("Files on disk are preserved. Projects with task workspaces cannot be removed yet.")
                            .button_props(DialogButtonProps::default().ok_text("Remove reference"))
                            .confirm()
                            .on_ok(move |_, _, cx| {
                                weak_submit.update(cx, |app, cx| app.remove_project_reference(workspace_id, cx)).unwrap_or(false)
                            })
                    });
                }));
            let name_submit = name.clone();
            let weak_submit = weak.clone();
            let workspace_id = workspace.id;
            dialog
                .title("Manage project")
                .child(content)
                .button_props(DialogButtonProps::default().ok_text("Save"))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let name = name_submit.read(cx).value().to_string();
                    weak_submit.update(cx, |app, cx| app.update_project_name(workspace_id, name, cx)).unwrap_or(false)
                })
        });
    }

    fn open_manage_task(&mut self, task: ProjectTask, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.session.language;
        let title = cx.new(|cx| InputState::new(window, cx).default_value(task.title.clone()));
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(task.description.clone().unwrap_or_default())
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let task_id = task.id;
            let weak_add = weak.clone();
            let weak_detach = weak.clone();
            let content = v_flex()
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(match language {
                            Language::English => "Title",
                            Language::Spanish => "Título",
                        })
                        .child(Input::new(&title)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(match language {
                            Language::English => "Description",
                            Language::Spanish => "Descripción",
                        })
                        .child(Input::new(&description).h(px(100.))),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(compact_button(
                            "add-task-repositories",
                            match language {
                                Language::English => "Add repositories…",
                                Language::Spanish => "Agregar repositorios…",
                            },
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = weak_add.update(cx, |app, cx| {
                                    app.open_add_task_repositories(task_id, window, cx)
                                });
                            },
                        ))
                        .child(compact_button(
                            "remove-task-repositories",
                            match language {
                                Language::English => "Remove repositories…",
                                Language::Spanish => "Quitar repositorios…",
                            },
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = weak_detach.update(cx, |app, cx| {
                                    app.open_remove_task_repositories(task_id, window, cx)
                                });
                            },
                        )),
                );
            let title_submit = title.clone();
            let description_submit = description.clone();
            let weak_submit = weak.clone();
            dialog
                .title(match language {
                    Language::English => "Edit task",
                    Language::Spanish => "Editar tarea",
                })
                .w(px(480.))
                .child(content)
                .button_props(DialogButtonProps::default().ok_text(match language {
                    Language::English => "Save changes",
                    Language::Spanish => "Guardar cambios",
                }))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let title = title_submit.read(cx).value().to_string();
                    let description = description_submit.read(cx).value().to_string();
                    weak_submit
                        .update(cx, |app, cx| {
                            app.update_task_details(task_id, title, description, cx)
                        })
                        .unwrap_or(false)
                })
        });
    }

    fn open_remove_task_confirmation(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        let language = self.session.language;
        let task_title = task.title.clone();
        let worktree_path = task.worktree_root_path.display().to_string();
        if !self.show_terminal && self.orchestrator_webview.is_some() {
            self.dispatch_orchestrator_event(serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "remove_task", "task_id": task_id,
                    "title": self.tr("Delete task and worktrees?", "¿Eliminar tarea y worktrees?"),
                    "name": task_title, "context": worktree_path,
                    "description": self.tr(
                        "All task terminals will be stopped. The worktree folder and any uncommitted changes inside it will be permanently deleted. Git branches are preserved.",
                        "Se detendrán todas las terminales de la tarea. La carpeta del worktree y cualquier cambio sin confirmar dentro de ella se eliminarán permanentemente. Las ramas Git se conservarán.",
                    ),
                    "confirm_label": self.tr("Delete task", "Eliminar tarea"),
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": if self.show_settings { 0.0 } else {
                        -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0)
                    },
                }
            }), cx);
            self.dispatch_navigation_event(serde_json::json!({
                "type": "modal_visibility", "visible": true,
            }), cx);
            if let Some(webview) = &self.orchestrator_webview {
                let _ = webview.read(cx).raw().focus();
            }
            return;
        }
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_submit = weak.clone();
            dialog
                .title(match language {
                    Language::English => "Delete task and worktrees?",
                    Language::Spanish => "¿Eliminar tarea y worktrees?",
                })
                .w(px(520.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(task_title.clone()),
                        )
                        .child(match language {
                            Language::English => "All task terminals will be stopped. The worktree folder and any uncommitted changes inside it will be permanently deleted. Git branches are preserved.",
                            Language::Spanish => "Se detendrán todas las terminales de la tarea. La carpeta del worktree y cualquier cambio sin confirmar dentro de ella se eliminarán permanentemente. Las ramas Git se conservarán.",
                        })
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0x8e97aa))
                                .child(worktree_path.clone()),
                        ),
                )
                .button_props(DialogButtonProps::default().ok_text(match language {
                    Language::English => "Delete task",
                    Language::Spanish => "Eliminar tarea",
                }))
                .confirm()
                .on_ok(move |_, _, cx| {
                    weak_submit
                        .update(cx, |app, cx| app.start_remove_task(task_id, cx))
                        .unwrap_or(false)
                })
        });
    }

    fn start_remove_task(&mut self, task_id: Uuid, cx: &mut Context<Self>) -> bool {
        if self
            .orchestrator_turns
            .keys()
            .any(|scope| scope.task_id() == Some(task_id))
        {
            self.set_status(
                self.tr(
                    "Wait for the task agent to finish before deleting the task.",
                    "Espera a que termine el agente de la tarea antes de eliminarla.",
                ),
                true,
                cx,
            );
            return false;
        }
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return false;
        };

        let terminal_ids = self
            .session
            .terminals
            .iter()
            .filter(|terminal| terminal.task_id == Some(task_id))
            .map(|terminal| terminal.id)
            .collect::<Vec<_>>();
        for terminal_id in terminal_ids {
            self.close_terminal_internal(terminal_id, false, cx);
        }

        if self
            .active_file
            .as_ref()
            .is_some_and(|document| document.root.starts_with(&task.worktree_root_path))
        {
            self.active_file = None;
        }
        if self
            .file_explorer
            .root
            .as_ref()
            .is_some_and(|root| root.starts_with(&task.worktree_root_path))
        {
            self.close_file_explorer(cx);
        }

        let task = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .cloned()
            .unwrap_or(task);
        let paths = self.paths.clone();
        let background = cx
            .background_executor()
            .spawn(async move { TaskService::new(&paths).remove_permanently(&task) });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| app.finish_removed_task(task_id, result, cx));
        })
        .detach();
        self.busy = Some(match self.session.language {
            Language::English => "Deleting task worktrees…".into(),
            Language::Spanish => "Eliminando worktrees de la tarea…".into(),
        });
        cx.notify();
        true
    }

    fn open_add_task_repositories(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == task.workspace_id)
            .cloned()
        else {
            return;
        };
        let current = task
            .repositories
            .iter()
            .map(|repository| repository.repository_id)
            .collect::<HashSet<_>>();
        let available = workspace
            .repositories
            .iter()
            .filter(|repository| !current.contains(&repository.id))
            .cloned()
            .collect::<Vec<_>>();
        if available.is_empty() {
            self.set_status(
                "Every project repository is already attached to this task",
                false,
                cx,
            );
            return;
        }
        let branch_name = task.branch().unwrap_or_default().to_string();
        if branch_name.is_empty() {
            self.set_status("This task does not have a branch to reuse", true, cx);
            return;
        }
        let current_source_label = self.tr("Current HEAD", "HEAD actual");
        let local_source_label = self.tr("Local branch", "Rama local");
        let remote_source_label = self.tr("Remote branch", "Rama remota");
        let base_label = self.tr("Base branch (optional)", "Rama base (opcional)");
        let base_placeholder = self.tr(
            "master · used only where the branch must be created",
            "master · solo donde haya que crear la rama",
        );
        let copy_changes_label = self.tr(
            "Copy current local changes",
            "Copiar cambios locales actuales",
        );
        let copy_env_label = self.tr("Copy .env files", "Copiar archivos .env");
        let create_missing_label = self.tr(
            "Create the branch when it is missing",
            "Crear la rama cuando no exista",
        );
        let replace_divergent_label = self.tr(
            "Replace divergent local branches (a backup is kept)",
            "Reemplazar ramas locales divergentes (se conserva un respaldo)",
        );
        let options = Rc::new(Mutex::new(TaskDraftOptions::default()));
        let base = cx.new(|cx| InputState::new(window, cx).placeholder(base_placeholder));
        let setup_commands = available
            .iter()
            .map(|repository| {
                let repository_name = repository.name.clone();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(format!("Setup command for {repository_name} (optional)"))
                });
                (repository.id, input)
            })
            .collect::<HashMap<_, _>>();
        let weak = cx.weak_entity();
        let paths = self.paths.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let draft = options.lock();
            let branch_source = draft.branch_source;
            let existing_action = draft.existing_branch_action;
            let create_missing = draft.create_missing_branch;
            let replace_divergent = draft.replace_divergent_local_branches;
            let availability = draft.availability.clone();
            drop(draft);
            let mut content = v_flex().gap_3().child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x8e97aa))
                            .child("Task branch"),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(branch_name.clone()),
                    ),
            );

            let mut sources = h_flex().gap_2();
            for (id, label, source) in [
                (
                    "add-branch-current",
                    current_source_label,
                    TaskBranchSource::Current,
                ),
                (
                    "add-branch-local",
                    local_source_label,
                    TaskBranchSource::Local,
                ),
                (
                    "add-branch-remote",
                    remote_source_label,
                    TaskBranchSource::Remote,
                ),
            ] {
                let options_source = options.clone();
                let weak_source = weak.clone();
                sources = sources.child(choice_button(
                    id,
                    label,
                    branch_source == source,
                    move |_, _, cx| {
                        let mut draft = options_source.lock();
                        draft.branch_source = source;
                        draft.availability = None;
                        drop(draft);
                        let _ = weak_source.update(cx, |_, cx| cx.notify());
                    },
                ));
            }
            content = content
                .child(sources)
                .child(form_field(base_label, Input::new(&base)));

            if branch_source == TaskBranchSource::Current {
                let options_reuse = options.clone();
                let weak_reuse = weak.clone();
                let options_recreate = options.clone();
                let weak_recreate = weak.clone();
                content = content.child(
                    h_flex()
                        .gap_2()
                        .child(choice_button(
                            "add-reuse-existing",
                            "Reuse existing branch",
                            existing_action == ExistingBranchAction::Reuse,
                            move |_, _, cx| {
                                options_reuse.lock().existing_branch_action =
                                    ExistingBranchAction::Reuse;
                                let _ = weak_reuse.update(cx, |_, cx| cx.notify());
                            },
                        ))
                        .child(choice_button(
                            "add-recreate-existing",
                            "Recreate from current HEAD",
                            existing_action == ExistingBranchAction::Recreate,
                            move |_, _, cx| {
                                options_recreate.lock().existing_branch_action =
                                    ExistingBranchAction::Recreate;
                                let _ = weak_recreate.update(cx, |_, cx| cx.notify());
                            },
                        )),
                );
            } else {
                let options_missing = options.clone();
                let weak_missing = weak.clone();
                content = content.child(row_button(
                    "add-create-missing".into(),
                    format!(
                        "{} {create_missing_label}",
                        if create_missing { "●" } else { "○" }
                    ),
                    create_missing,
                    move |_, _, cx| {
                        let mut draft = options_missing.lock();
                        draft.create_missing_branch = !draft.create_missing_branch;
                        drop(draft);
                        let _ = weak_missing.update(cx, |_, cx| cx.notify());
                    },
                ));
                if branch_source == TaskBranchSource::Remote {
                    let options_replace = options.clone();
                    let weak_replace = weak.clone();
                    content = content.child(row_button(
                        "add-replace-divergent".into(),
                        format!(
                            "{} {replace_divergent_label}",
                            if replace_divergent { "●" } else { "○" }
                        ),
                        replace_divergent,
                        move |_, _, cx| {
                            let mut draft = options_replace.lock();
                            draft.replace_divergent_local_branches =
                                !draft.replace_divergent_local_branches;
                            drop(draft);
                            let _ = weak_replace.update(cx, |_, cx| cx.notify());
                        },
                    ));
                }

                let workspace_check = workspace.clone();
                let options_check = options.clone();
                let weak_check = weak.clone();
                let branch_check = branch_name.clone();
                let base_check = base.clone();
                content = content.child(compact_button(
                    "check-add-repository-branch",
                    "Check branch",
                    move |_, _, cx| {
                        let base_name = non_empty_value(&base_check, cx);
                        let repository_ids = options_check
                            .lock()
                            .selected_repositories
                            .iter()
                            .copied()
                            .collect::<Vec<_>>();
                        if repository_ids.is_empty() {
                            return;
                        }
                        let workspace = workspace_check.clone();
                        let branch = branch_check.clone();
                        let source = branch_source;
                        let background = cx.background_executor().spawn(async move {
                            TaskService::branch_availability(
                                &workspace,
                                &repository_ids,
                                &branch,
                                source,
                                base_name.as_deref(),
                            )
                        });
                        let options = options_check.clone();
                        let weak = weak_check.clone();
                        cx.spawn(async move |cx| match background.await {
                            Ok(result) => {
                                options.lock().availability = Some(result);
                                let _ = weak.update(cx, |_, cx| cx.notify());
                            }
                            Err(error) => {
                                let _ = weak.update(cx, |app, cx| {
                                    app.set_status(
                                        format!("Could not check the branch: {error:#}"),
                                        true,
                                        cx,
                                    )
                                });
                            }
                        })
                        .detach();
                    },
                ));
            }

            if let Some(availability) = availability {
                let mut results = v_flex().gap_1().p_2().rounded(px(6.)).bg(rgb(0x111318));
                for result in availability {
                    let exists = branch_exists(&result, branch_source);
                    let state = if result.local_checked_out {
                        "already checked out"
                    } else if exists {
                        "found"
                    } else {
                        "missing"
                    };
                    let base = match result.base.as_ref() {
                        Some(base) if !exists => format!(" · from {}", base.label),
                        _ => String::new(),
                    };
                    results = results.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0xaeb7c7))
                            .child(format!("{}: {state}{base}", result.repository_name)),
                    );
                }
                content = content.child(results);
            }

            for repository in &available {
                let repository_id = repository.id;
                let is_selected = options
                    .lock()
                    .selected_repositories
                    .contains(&repository_id);
                let selected_click = options.clone();
                let weak_click = weak.clone();
                content = content.child(row_button(
                    format!("add-task-repository-{repository_id}"),
                    format!(
                        "{}  {}",
                        if is_selected { "●" } else { "○" },
                        repository.name
                    ),
                    is_selected,
                    move |_, _, cx| {
                        let mut draft = selected_click.lock();
                        if !draft.selected_repositories.remove(&repository_id) {
                            draft.selected_repositories.insert(repository_id);
                            draft.repository_options.entry(repository_id).or_default();
                        }
                        draft.availability = None;
                        drop(draft);
                        let _ = weak_click.update(cx, |_, cx| cx.notify());
                    },
                ));
                if is_selected {
                    let repository_options = options
                        .lock()
                        .repository_options
                        .get(&repository_id)
                        .cloned()
                        .unwrap_or_default();
                    let options_changes = options.clone();
                    let weak_changes = weak.clone();
                    let options_env = options.clone();
                    let weak_env = weak.clone();
                    content = content.child(
                        v_flex()
                            .ml_4()
                            .gap_1()
                            .child(row_button(
                                format!("add-copy-changes-{repository_id}"),
                                format!(
                                    "{} {copy_changes_label}",
                                    if repository_options.copy_local_changes {
                                        "●"
                                    } else {
                                        "○"
                                    }
                                ),
                                repository_options.copy_local_changes,
                                move |_, _, cx| {
                                    let mut draft = options_changes.lock();
                                    let value =
                                        draft.repository_options.entry(repository_id).or_default();
                                    value.copy_local_changes = !value.copy_local_changes;
                                    drop(draft);
                                    let _ = weak_changes.update(cx, |_, cx| cx.notify());
                                },
                            ))
                            .child(row_button(
                                format!("add-copy-env-{repository_id}"),
                                format!(
                                    "{} {copy_env_label}",
                                    if repository_options.copy_environment_files {
                                        "●"
                                    } else {
                                        "○"
                                    }
                                ),
                                repository_options.copy_environment_files,
                                move |_, _, cx| {
                                    let mut draft = options_env.lock();
                                    let value =
                                        draft.repository_options.entry(repository_id).or_default();
                                    value.copy_environment_files = !value.copy_environment_files;
                                    drop(draft);
                                    let _ = weak_env.update(cx, |_, cx| cx.notify());
                                },
                            ))
                            .child(Input::new(
                                setup_commands
                                    .get(&repository_id)
                                    .expect("setup input must exist"),
                            )),
                    );
                }
            }
            let options_submit = options.clone();
            let base_submit = base.clone();
            let setup_submit = setup_commands.clone();
            let weak_submit = weak.clone();
            let task_submit = task.clone();
            let workspace_submit = workspace.clone();
            let paths_submit = paths.clone();
            dialog
                .title("Add repositories to task")
                .w(px(680.))
                .child(content.max_h(px(650.)).overflow_y_scrollbar().pr_2())
                .button_props(DialogButtonProps::default().ok_text("Add"))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let draft = options_submit.lock();
                    let repository_ids = draft
                        .selected_repositories
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    if repository_ids.is_empty() {
                        return false;
                    }
                    let preparations = repository_ids
                        .iter()
                        .map(|repository_id| {
                            let repository_options = draft
                                .repository_options
                                .get(repository_id)
                                .cloned()
                                .unwrap_or_default();
                            let setup_command = setup_submit
                                .get(repository_id)
                                .map(|input| input.read(cx).value().to_string())
                                .unwrap_or_default();
                            (
                                *repository_id,
                                RepositoryPreparation {
                                    copy_local_changes: repository_options.copy_local_changes,
                                    copy_environment_files: repository_options
                                        .copy_environment_files,
                                    setup_command: (!setup_command.trim().is_empty())
                                        .then_some(setup_command),
                                },
                            )
                        })
                        .collect();
                    let request = AddTaskRepositoriesRequest {
                        repository_ids,
                        branch_source: draft.branch_source,
                        base_ref: non_empty_value(&base_submit, cx),
                        create_missing_branch: draft.create_missing_branch,
                        replace_divergent_local_branches: draft.replace_divergent_local_branches,
                        existing_branch_action: draft.existing_branch_action,
                        preparations,
                    };
                    drop(draft);
                    let mut task = task_submit.clone();
                    let workspace = workspace_submit.clone();
                    let paths = paths_submit.clone();
                    let background = cx.background_executor().spawn(async move {
                        TaskService::new(&paths)
                            .add_repositories(&workspace, &mut task, request)?;
                        Ok(task)
                    });
                    let weak = weak_submit.clone();
                    cx.spawn(async move |cx| {
                        let result = background.await;
                        let _ = weak.update(cx, |app, cx| app.finish_updated_task(result, cx));
                    })
                    .detach();
                    let _ = weak_submit.update(cx, |app, cx| {
                        app.busy = Some("Adding isolated worktrees…".into());
                        cx.notify();
                    });
                    true
                })
        });
    }

    /// Detach repositories that were added to a task by mistake, so they can be
    /// added again — with another base branch, for instance.
    fn open_remove_task_repositories(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == task.workspace_id)
            .cloned()
        else {
            return;
        };
        if task.repositories.len() < 2 {
            self.set_status(
                self.tr(
                    "A task keeps at least one repository; delete the whole task instead",
                    "Una tarea conserva al menos un repositorio; elimina la tarea completa",
                ),
                true,
                cx,
            );
            return;
        }
        let attached = task
            .repositories
            .iter()
            .map(|repository| {
                let name = workspace
                    .repositories
                    .iter()
                    .find(|candidate| candidate.id == repository.repository_id)
                    .map(|candidate| candidate.name.clone())
                    .unwrap_or_else(|| repository.branch.clone());
                (repository.repository_id, name, repository.branch.clone())
            })
            .collect::<Vec<_>>();
        let dialog_title = self.tr(
            "Remove repositories from task",
            "Quitar repositorios de la tarea",
        );
        let submit_label = self.tr("Remove", "Quitar");
        let delete_branch_label = self.tr(
            "Delete the local branch too (needed to add it again on another base)",
            "Eliminar también la rama local (necesario para volver a agregarlo con otra base)",
        );
        let discard_label = self.tr(
            "Discard uncommitted changes in the worktree",
            "Descartar cambios sin confirmar en el worktree",
        );
        let busy_label = self.tr("Removing worktrees…", "Quitando worktrees…");
        let options = Rc::new(Mutex::new(DetachRepositoriesDraft::default()));
        let weak = cx.weak_entity();
        let paths = self.paths.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let draft = options.lock();
            let delete_branch = draft.delete_branch;
            let discard = draft.discard_uncommitted_changes;
            drop(draft);
            let mut content = v_flex().gap_3();
            for (repository_id, name, branch) in &attached {
                let repository_id = *repository_id;
                let is_selected = options
                    .lock()
                    .selected_repositories
                    .contains(&repository_id);
                let options_click = options.clone();
                let weak_click = weak.clone();
                content = content.child(row_button(
                    format!("remove-task-repository-{repository_id}"),
                    format!(
                        "{}  {name}  ·  {branch}",
                        if is_selected { "●" } else { "○" }
                    ),
                    is_selected,
                    move |_, _, cx| {
                        let mut draft = options_click.lock();
                        if !draft.selected_repositories.remove(&repository_id) {
                            draft.selected_repositories.insert(repository_id);
                        }
                        drop(draft);
                        let _ = weak_click.update(cx, |_, cx| cx.notify());
                    },
                ));
            }
            let options_branch = options.clone();
            let weak_branch = weak.clone();
            let options_discard = options.clone();
            let weak_discard = weak.clone();
            content = content
                .child(row_button(
                    "remove-task-repository-delete-branch".into(),
                    format!(
                        "{} {delete_branch_label}",
                        if delete_branch { "●" } else { "○" }
                    ),
                    delete_branch,
                    move |_, _, cx| {
                        let mut draft = options_branch.lock();
                        draft.delete_branch = !draft.delete_branch;
                        drop(draft);
                        let _ = weak_branch.update(cx, |_, cx| cx.notify());
                    },
                ))
                .child(row_button(
                    "remove-task-repository-discard".into(),
                    format!("{} {discard_label}", if discard { "●" } else { "○" }),
                    discard,
                    move |_, _, cx| {
                        let mut draft = options_discard.lock();
                        draft.discard_uncommitted_changes = !draft.discard_uncommitted_changes;
                        drop(draft);
                        let _ = weak_discard.update(cx, |_, cx| cx.notify());
                    },
                ));

            let options_submit = options.clone();
            let weak_submit = weak.clone();
            let task_submit = task.clone();
            let workspace_submit = workspace.clone();
            let paths_submit = paths.clone();
            dialog
                .title(dialog_title)
                .w(px(620.))
                .child(content.max_h(px(560.)).overflow_y_scrollbar().pr_2())
                .button_props(DialogButtonProps::default().ok_text(submit_label))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let draft = options_submit.lock();
                    let repository_ids = draft
                        .selected_repositories
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    if repository_ids.is_empty() {
                        return false;
                    }
                    let request = RemoveTaskRepositoriesRequest {
                        repository_ids,
                        discard_uncommitted_changes: draft.discard_uncommitted_changes,
                        delete_branch: draft.delete_branch,
                    };
                    drop(draft);
                    let mut task = task_submit.clone();
                    let workspace = workspace_submit.clone();
                    let paths = paths_submit.clone();
                    let background = cx.background_executor().spawn(async move {
                        let removed = TaskService::new(&paths)
                            .remove_repositories(&workspace, &mut task, request)?;
                        Ok((task, removed))
                    });
                    let weak = weak_submit.clone();
                    cx.spawn(async move |cx| {
                        let result = background.await;
                        let _ = weak.update(cx, |app, cx| {
                            app.finish_detached_task_repositories(result, cx)
                        });
                    })
                    .detach();
                    let _ = weak_submit.update(cx, |app, cx| {
                        app.busy = Some(busy_label.into());
                        cx.notify();
                    });
                    true
                })
        });
    }

    fn open_add_project_repository(&mut self, workspace_id: Uuid, github: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() { return; }
        if !github {
            if let Some(path) = rfd::FileDialog::new().set_title(self.tr("Clone a local Git repository", "Clonar un repositorio Git local")).pick_folder() {
                self.clone_project_repository(workspace_id, path.to_string_lossy().into_owned(), false, cx);
            }
            return;
        }
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/owner/repository"));
        let title = self.tr("Clone repository into project", "Clonar repositorio dentro del proyecto");
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let input = url.clone();
            let weak = weak.clone();
            dialog.title(title).child(Input::new(&url)).confirm().on_ok(move |_, _, cx| {
                let value = input.read(cx).value().trim().to_string();
                if value.is_empty() { return false; }
                let _ = weak.update(cx, |app, cx| app.clone_project_repository(workspace_id, value, true, cx));
                true
            })
        });
    }

    fn clone_project_repository(&mut self, workspace_id: Uuid, source: String, github: bool, cx: &mut Context<Self>) {
        if self.busy.is_some() { return; }
        let Some(mut workspace) = self.workspaces.iter().find(|workspace| workspace.id == workspace_id).cloned() else { return; };
        self.busy = Some(self.tr("Cloning repository…", "Clonando repositorio…").into());
        let background = cx.background_executor().spawn(async move {
            if github { ProjectService::add_github_repository(&mut workspace, &source)?; }
            else { ProjectService::add_existing_repository(&mut workspace, Path::new(&source))?; }
            workspace.repositories.last().cloned().context("Cloned repository missing")
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.busy = None;
                match result {
                    Ok(repository) => {
                        if let Some(index) = app.workspaces.iter().position(|workspace| workspace.id == workspace_id) {
                            let mut workspace = app.workspaces[index].clone();
                            if !workspace.repositories.iter().any(|existing| existing.path == repository.path) {
                                workspace.repositories.push(repository);
                            }
                            workspace.layout = WorkspaceLayout::MultiRepository;
                            workspace.updated_at = Utc::now();
                            match app.database.upsert_workspace(&workspace, index) {
                                Ok(()) => { app.workspaces[index] = workspace; app.set_status("Repository cloned into the project", false, cx); }
                                Err(error) => app.set_status(format!("Clone saved on disk, but registration failed: {error:#}"), true, cx),
                            }
                        }
                    }
                    Err(error) => app.set_status(format!("Could not clone repository: {error:#}"), true, cx),
                }
                app.hydrate_navigation(cx);
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn open_create_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_modal_submitting {
            return;
        }
        if !self.show_terminal && self.orchestrator_webview.is_some() {
            let request_id = Uuid::new_v4();
            self.project_modal_request = Some(request_id);
            self.dispatch_orchestrator_event(serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "create_project",
                    "request_id": request_id,
                    "title": self.tr("Create project", "Crear proyecto"),
                    "name": "", "description": "",
                    "projects_root": self.projects_root().display().to_string(),
                    "confirm_label": self.tr("Create", "Crear"),
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": if self.show_settings { 0.0 } else {
                        -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0)
                    },
                }
            }), cx);
            self.dispatch_navigation_event(serde_json::json!({
                "type": "modal_visibility", "visible": true,
            }), cx);
            if let Some(webview) = &self.orchestrator_webview {
                let _ = webview.read(cx).raw().focus();
            }
            return;
        }
        let name_placeholder = self.tr("Project name", "Nombre del proyecto");
        let url_placeholder = "https://github.com/owner/repository";
        let dialog_title = self.tr("Create project", "Crear proyecto");
        let submit_label = self.tr("Create", "Crear");
        let empty_label = self.tr("Empty", "Vacío");
        let existing_label = self.tr("Clone local repositories", "Clonar repositorios locales");
        let github_label = self.tr("GitHub", "GitHub");
        let choose_label = self.tr("Choose folder…", "Elegir carpeta…");
        let destination_label = self.tr("Destination", "Destino");
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(name_placeholder));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder(url_placeholder));
        let options = Rc::new(Mutex::new(ProjectDraftOptions::default()));
        let projects_root = self.projects_root();
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let draft_mode = options.lock().mode;
            let mut content = v_flex().gap_3();
            let mut modes = h_flex().gap_2();
            for (id, label, mode) in [
                ("project-empty", empty_label, ProjectDraftMode::Empty),
                (
                    "project-existing",
                    existing_label,
                    ProjectDraftMode::Existing,
                ),
                ("project-github", github_label, ProjectDraftMode::Github),
            ] {
                let options_click = options.clone();
                let weak_click = weak.clone();
                modes = modes.child(choice_button(
                    id,
                    label,
                    draft_mode == mode,
                    move |_, _, cx| {
                        options_click.lock().mode = mode;
                        let _ = weak_click.update(cx, |_, cx| cx.notify());
                    },
                ));
            }
            content = content.child(modes);

            match draft_mode {
                ProjectDraftMode::Empty => {
                    content = content.child(Input::new(&name));
                }
                ProjectDraftMode::Existing => {
                    let selected_path = options.lock().existing_path.clone();
                    let options_choose = options.clone();
                    let weak_choose = weak.clone();
                    content = content
                        .child(compact_button(
                            "choose-existing-project",
                            choose_label,
                            move |_, _, cx| {
                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                    options_choose.lock().existing_path = Some(path);
                                    let _ = weak_choose.update(cx, |_, cx| cx.notify());
                                }
                            },
                        ))
                        .child(
                            div().text_size(px(12.)).text_color(rgb(0x9ba3b4)).child(
                                selected_path
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "No folder selected".into()),
                            ),
                        )
                        .child(Input::new(&name));
                }
                ProjectDraftMode::Github => {
                    content = content.child(Input::new(&url)).child(Input::new(&name));
                }
            }
            content = content.child(
                v_flex()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(rgb(0x8e97aa))
                    .child(destination_label)
                    .child(projects_root.display().to_string()),
            );

            let name_for_submit = name.clone();
            let url_for_submit = url.clone();
            let options_for_submit = options.clone();
            let root_for_submit = projects_root.clone();
            let weak_for_submit = weak.clone();
            dialog
                .title(dialog_title)
                .w(px(620.))
                .child(content)
                .button_props(DialogButtonProps::default().ok_text(submit_label))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let draft = options_for_submit.lock();
                    let mode = draft.mode;
                    let existing_path = draft.existing_path.clone();
                    drop(draft);
                    let requested_name = name_for_submit.read(cx).value().to_string();
                    match mode {
                        ProjectDraftMode::Empty => weak_for_submit
                            .update(cx, |app, cx| app.create_empty_project(&requested_name, cx))
                            .unwrap_or(false),
                        ProjectDraftMode::Existing => {
                            let Some(path) = existing_path else {
                                return false;
                            };
                            let name = requested_name.clone();
                            let root = root_for_submit.clone();
                            let background = cx.background_executor().spawn(async move {
                                ProjectService::import_existing(
                                    &root,
                                    &path,
                                    (!name.trim().is_empty()).then_some(name.as_str()),
                                )
                            });
                            let weak = weak_for_submit.clone();
                            cx.spawn(async move |cx| {
                                let result = background.await;
                                let _ = weak.update(cx, |app, cx| {
                                    app.finish_background_workspace(result, cx)
                                });
                            })
                            .detach();
                            let _ = weak_for_submit.update(cx, |app, cx| {
                                app.busy = Some("Importing project…".into());
                                cx.notify();
                            });
                            true
                        }
                        ProjectDraftMode::Github => {
                            let github_url = url_for_submit.read(cx).value().to_string();
                            if github_url.trim().is_empty() {
                                return false;
                            }
                            let root = root_for_submit.clone();
                            let name = requested_name.clone();
                            let background = cx.background_executor().spawn(async move {
                                ProjectService::clone_github_named(
                                    &root,
                                    &github_url,
                                    (!name.trim().is_empty()).then_some(name.as_str()),
                                )
                            });
                            let weak = weak_for_submit.clone();
                            cx.spawn(async move |cx| {
                                let result = background.await;
                                let _ = weak.update(cx, |app, cx| {
                                    app.finish_background_workspace(result, cx)
                                });
                            })
                            .detach();
                            let _ = weak_for_submit.update(cx, |app, cx| {
                                app.busy = Some("Cloning project…".into());
                                cx.notify();
                            });
                            true
                        }
                    }
                })
        });
    }

    fn submit_create_project_modal(
        &mut self,
        request_id: Uuid,
        mode: String,
        name: String,
        url: String,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if self.project_modal_request != Some(request_id) || self.project_modal_submitting {
            return;
        }
        let validation = match mode.as_str() {
            "empty" if name.trim().is_empty() => Some(self.tr("Enter a project name.", "Escribe un nombre para el proyecto.")),
            "existing" if path.is_empty() => Some(self.tr("Choose a folder.", "Elige una carpeta.")),
            "github" if url.trim().is_empty() => Some(self.tr("Enter a repository URL.", "Escribe la URL del repositorio.")),
            "empty" | "existing" | "github" => None,
            _ => Some(self.tr("Invalid project source.", "Origen del proyecto inválido.")),
        };
        if let Some(error) = validation {
            self.dispatch_orchestrator_event(serde_json::json!({
                "type": "app_modal_feedback", "request_id": request_id,
                "feedback": { "error": error },
            }), cx);
            return;
        }
        self.project_modal_submitting = true;
        let root = self.projects_root();
        let background = cx.background_executor().spawn(async move {
            let requested_name = (!name.trim().is_empty()).then_some(name.trim());
            match mode.as_str() {
                "empty" => ProjectService::create_git_repository(&root, name.trim()),
                "existing" => ProjectService::import_existing(&root, &PathBuf::from(path), requested_name),
                "github" => ProjectService::clone_github_named(&root, url.trim(), requested_name),
                _ => unreachable!("validated project source"),
            }
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                app.project_modal_submitting = false;
                app.status = None;
                app.finish_background_workspace(result, cx);
                if app.project_modal_request != Some(request_id) {
                    return;
                }
                if let Some((message, true)) = &app.status {
                    app.dispatch_orchestrator_event(serde_json::json!({
                        "type": "app_modal_feedback", "request_id": request_id,
                        "feedback": { "error": message },
                    }), cx);
                } else {
                    app.dismiss_app_modal(cx);
                }
            });
        }).detach();
    }

    fn open_create_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.selected_workspace().cloned() else {
            return;
        };
        if workspace.repositories.is_empty() {
            self.set_status("Add a Git repository before creating a task", true, cx);
            return;
        }

        let title_label = self.tr("Task title", "Título de la tarea");
        let title_placeholder = self.tr("e.g. Fix the login flow", "p. ej. Corregir el login");
        let branch_label = self.tr("Branch name (optional)", "Nombre de rama (opcional)");
        let branch_placeholder = self.tr(
            "feature/new-branch · empty uses the current HEAD",
            "feature/nueva-rama · vacío usa el HEAD actual",
        );
        let description_label = self.tr("Description (optional)", "Descripción (opcional)");
        let description_placeholder = self.tr(
            "Short context for this task…",
            "Contexto breve para esta tarea…",
        );
        let repositories_label = self.tr("Repositories", "Repositorios");
        let setup_label = self.tr(
            "Setup command (optional)",
            "Comando de preparación (opcional)",
        );
        let setup_placeholder = self.tr("e.g. npm install", "p. ej. npm install");
        let base_label = self.tr("Base branch (optional)", "Rama base (opcional)");
        let base_placeholder = self.tr(
            "master · empty branches from the current HEAD",
            "master · vacío parte del HEAD actual",
        );
        let branch_source_label = self.tr("Branch source", "Origen de la rama");
        let current_source_label = self.tr("Current HEAD", "HEAD actual");
        let local_source_label = self.tr("Local branch", "Rama local");
        let remote_source_label = self.tr("Remote branch", "Rama remota");
        let check_branch_label = self.tr("Check branch", "Comprobar rama");
        let create_missing_label = self.tr(
            "Create the branch when it is missing",
            "Crear la rama cuando no exista",
        );
        let replace_divergent_label = self.tr(
            "Replace divergent local branches (a backup is kept)",
            "Reemplazar ramas locales divergentes (se conserva un respaldo)",
        );
        let reuse_label = self.tr("Reuse existing branch", "Reutilizar rama existente");
        let recreate_label = self.tr("Recreate from current HEAD", "Recrear desde el HEAD actual");
        let dialog_title = self.tr(
            "Create isolated task workspace",
            "Crear espacio aislado para la tarea",
        );
        let submit_label = self.tr("Create task", "Crear tarea");
        let copy_changes_label = self.tr(
            "Copy current local changes",
            "Copiar cambios locales actuales",
        );
        let copy_env_label = self.tr("Copy .env files", "Copiar archivos .env");
        let preparing_label = self.tr("Preparing Git worktrees…", "Preparando worktrees Git…");
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(title_placeholder));
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder(branch_placeholder));
        let base = cx.new(|cx| InputState::new(window, cx).placeholder(base_placeholder));
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(description_placeholder)
        });
        let setup_commands = workspace
            .repositories
            .iter()
            .map(|repository| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(setup_placeholder));
                (repository.id, input)
            })
            .collect::<HashMap<_, _>>();
        let options = Rc::new(Mutex::new(TaskDraftOptions::default()));
        let weak = cx.weak_entity();
        let paths = self.paths.clone();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let draft = options.lock();
            let branch_source = draft.branch_source;
            let existing_action = draft.existing_branch_action;
            let create_missing = draft.create_missing_branch;
            let replace_divergent = draft.replace_divergent_local_branches;
            let availability = draft.availability.clone();
            drop(draft);
            let mut content = v_flex().gap_4();
            content = content
                .child(form_field(title_label, Input::new(&title)))
                .child(form_field(branch_label, Input::new(&branch)))
                .child(form_field(
                    description_label,
                    Input::new(&description).h(px(92.)),
                ))
                .child(form_divider());

            let mut branch_section = v_flex().gap_2().child(section_label(branch_source_label));

            let mut branch_sources = segmented_group();
            for (id, label, source) in [
                (
                    "task-branch-current",
                    current_source_label,
                    TaskBranchSource::Current,
                ),
                (
                    "task-branch-local",
                    local_source_label,
                    TaskBranchSource::Local,
                ),
                (
                    "task-branch-remote",
                    remote_source_label,
                    TaskBranchSource::Remote,
                ),
            ] {
                let options_source = options.clone();
                let weak_source = weak.clone();
                branch_sources = branch_sources.child(segmented_item(
                    id,
                    label,
                    branch_source == source,
                    move |_, _, cx| {
                        let mut draft = options_source.lock();
                        draft.branch_source = source;
                        draft.availability = None;
                        drop(draft);
                        let _ = weak_source.update(cx, |_, cx| cx.notify());
                    },
                ));
            }
            branch_section = branch_section
                .child(branch_sources)
                .child(form_field(base_label, Input::new(&base)));

            if branch_source == TaskBranchSource::Current {
                let options_reuse = options.clone();
                let weak_reuse = weak.clone();
                let options_recreate = options.clone();
                let weak_recreate = weak.clone();
                branch_section = branch_section.child(
                    segmented_group()
                        .child(segmented_item(
                            "reuse-existing-branch",
                            reuse_label,
                            existing_action == ExistingBranchAction::Reuse,
                            move |_, _, cx| {
                                options_reuse.lock().existing_branch_action =
                                    ExistingBranchAction::Reuse;
                                let _ = weak_reuse.update(cx, |_, cx| cx.notify());
                            },
                        ))
                        .child(segmented_item(
                            "recreate-existing-branch",
                            recreate_label,
                            existing_action == ExistingBranchAction::Recreate,
                            move |_, _, cx| {
                                options_recreate.lock().existing_branch_action =
                                    ExistingBranchAction::Recreate;
                                let _ = weak_recreate.update(cx, |_, cx| cx.notify());
                            },
                        )),
                );
            } else {
                let options_missing = options.clone();
                let weak_missing = weak.clone();
                branch_section = branch_section.child(option_row(
                    "create-missing-branch".into(),
                    create_missing_label,
                    create_missing,
                    false,
                    move |_, _, cx| {
                        let mut draft = options_missing.lock();
                        draft.create_missing_branch = !draft.create_missing_branch;
                        drop(draft);
                        let _ = weak_missing.update(cx, |_, cx| cx.notify());
                    },
                ));
                if branch_source == TaskBranchSource::Remote {
                    let options_replace = options.clone();
                    let weak_replace = weak.clone();
                    branch_section = branch_section.child(option_row(
                        "replace-divergent-branches".into(),
                        replace_divergent_label,
                        replace_divergent,
                        false,
                        move |_, _, cx| {
                            let mut draft = options_replace.lock();
                            draft.replace_divergent_local_branches =
                                !draft.replace_divergent_local_branches;
                            drop(draft);
                            let _ = weak_replace.update(cx, |_, cx| cx.notify());
                        },
                    ));
                }
            }

            {
                let workspace_check = workspace.clone();
                let branch_check = branch.clone();
                let base_check = base.clone();
                let options_check = options.clone();
                let weak_check = weak.clone();
                branch_section = branch_section.child(h_flex().child(compact_button(
                    "check-task-branch",
                    check_branch_label,
                    move |_, _, cx| {
                        let branch_name = branch_check.read(cx).value().to_string();
                        let base_name = non_empty_value(&base_check, cx);
                        let repository_ids = options_check
                            .lock()
                            .selected_repositories
                            .iter()
                            .copied()
                            .collect::<Vec<_>>();
                        if branch_name.trim().is_empty() || repository_ids.is_empty() {
                            return;
                        }
                        let workspace = workspace_check.clone();
                        let source = branch_source;
                        let background = cx.background_executor().spawn(async move {
                            TaskService::branch_availability(
                                &workspace,
                                &repository_ids,
                                &branch_name,
                                source,
                                base_name.as_deref(),
                            )
                        });
                        let options = options_check.clone();
                        let weak = weak_check.clone();
                        cx.spawn(async move |cx| match background.await {
                            Ok(result) => {
                                options.lock().availability = Some(result);
                                let _ = weak.update(cx, |app, cx| {
                                    app.status = None;
                                    cx.notify();
                                });
                            }
                            Err(error) => {
                                let _ = weak.update(cx, |app, cx| {
                                    app.set_status(
                                        format!("Could not check the branch: {error:#}"),
                                        true,
                                        cx,
                                    )
                                });
                            }
                        })
                        .detach();
                    },
                )));
            }

            if let Some(availability) = availability {
                let mut results = v_flex()
                    .gap_1()
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(rgb(0x0e1015))
                    .border_1()
                    .border_color(rgb(0x232935));
                for result in availability {
                    let state = match branch_source {
                        TaskBranchSource::Current if result.local_revision.is_some() => {
                            "existing local branch"
                        }
                        TaskBranchSource::Current => "new branch from current HEAD",
                        TaskBranchSource::Local if result.local_revision.is_some() => "local",
                        TaskBranchSource::Remote if result.remote_revision.is_some() => "origin",
                        _ => "missing",
                    };
                    let checked_out = if result.local_checked_out {
                        " · already checked out"
                    } else {
                        ""
                    };
                    // The base only matters where the branch has to be created.
                    let base = match result.base {
                        Some(base) if !branch_exists(&result, branch_source) => {
                            format!(" · from {}", base.label)
                        }
                        _ => String::new(),
                    };
                    results =
                        results.child(div().text_size(px(11.)).text_color(rgb(0xaeb7c7)).child(
                            format!("{}: {state}{checked_out}{base}", result.repository_name),
                        ));
                }
                branch_section = branch_section.child(results);
            }

            content = content.child(branch_section).child(form_divider());

            let mut repositories_section =
                v_flex().gap_2().child(section_label(repositories_label));

            for repository in &workspace.repositories {
                let repository_id = repository.id;
                let selected = options
                    .lock()
                    .selected_repositories
                    .contains(&repository_id);
                let options_for_click = options.clone();
                let weak_for_click = weak.clone();
                let mut repository_card = v_flex().gap(px(6.)).child(option_row(
                    format!("task-repository-{repository_id}"),
                    repository.name.clone(),
                    selected,
                    true,
                    move |_, _, cx| {
                        let mut draft = options_for_click.lock();
                        if !draft.selected_repositories.remove(&repository_id) {
                            draft.selected_repositories.insert(repository_id);
                            draft.repository_options.entry(repository_id).or_default();
                        }
                        draft.availability = None;
                        drop(draft);
                        let _ = weak_for_click.update(cx, |_, cx| cx.notify());
                    },
                ));
                if selected {
                    let repository_options = options
                        .lock()
                        .repository_options
                        .get(&repository_id)
                        .cloned()
                        .unwrap_or_default();
                    let options_changes = options.clone();
                    let weak_changes = weak.clone();
                    let options_env = options.clone();
                    let weak_env = weak.clone();
                    repository_card = repository_card.child(
                        v_flex()
                            .ml(px(7.))
                            .pl(px(12.))
                            .gap(px(6.))
                            .border_l_1()
                            .border_color(rgb(0x262c38))
                            .child(option_row(
                                format!("copy-local-changes-{repository_id}"),
                                copy_changes_label,
                                repository_options.copy_local_changes,
                                false,
                                move |_, _, cx| {
                                    let mut draft = options_changes.lock();
                                    let value =
                                        draft.repository_options.entry(repository_id).or_default();
                                    value.copy_local_changes = !value.copy_local_changes;
                                    drop(draft);
                                    let _ = weak_changes.update(cx, |_, cx| cx.notify());
                                },
                            ))
                            .child(option_row(
                                format!("copy-env-files-{repository_id}"),
                                copy_env_label,
                                repository_options.copy_environment_files,
                                false,
                                move |_, _, cx| {
                                    let mut draft = options_env.lock();
                                    let value =
                                        draft.repository_options.entry(repository_id).or_default();
                                    value.copy_environment_files = !value.copy_environment_files;
                                    drop(draft);
                                    let _ = weak_env.update(cx, |_, cx| cx.notify());
                                },
                            ))
                            .child(form_field(
                                setup_label,
                                Input::new(
                                    setup_commands
                                        .get(&repository_id)
                                        .expect("setup input must exist"),
                                ),
                            )),
                    );
                }
                repositories_section = repositories_section.child(repository_card);
            }

            content = content.child(repositories_section);

            let workspace_for_submit = workspace.clone();
            let title_for_submit = title.clone();
            let branch_for_submit = branch.clone();
            let base_for_submit = base.clone();
            let description_for_submit = description.clone();
            let setup_for_submit = setup_commands.clone();
            let options_for_submit = options.clone();
            let weak_for_submit = weak.clone();
            let paths_for_submit = paths.clone();
            dialog
                .title(dialog_title)
                .w(px(680.))
                .child(content.max_h(px(650.)).overflow_y_scrollbar().pr_2())
                .button_props(DialogButtonProps::default().ok_text(submit_label))
                .confirm()
                .on_ok(move |_, _, cx| {
                    let task_title = title_for_submit.read(cx).value().to_string();
                    if task_title.trim().is_empty() {
                        return false;
                    }
                    let branch_name = branch_for_submit.read(cx).value().to_string();
                    let base_ref = non_empty_value(&base_for_submit, cx);
                    let task_description = description_for_submit.read(cx).value().to_string();
                    let draft = options_for_submit.lock();
                    let branch_source = draft.branch_source;
                    let create_missing_branch = draft.create_missing_branch;
                    let replace_divergent_local_branches = draft.replace_divergent_local_branches;
                    let existing_branch_action = draft.existing_branch_action;
                    let repository_ids = workspace_for_submit
                        .repositories
                        .iter()
                        .filter(|repository| draft.selected_repositories.contains(&repository.id))
                        .map(|repository| repository.id)
                        .collect::<Vec<_>>();
                    if repository_ids.is_empty() {
                        return false;
                    }
                    let preparations = repository_ids
                        .iter()
                        .map(|repository_id| {
                            let repository_options = draft
                                .repository_options
                                .get(repository_id)
                                .cloned()
                                .unwrap_or_default();
                            let setup_command = setup_for_submit
                                .get(repository_id)
                                .map(|input| input.read(cx).value().to_string())
                                .unwrap_or_default();
                            (
                                *repository_id,
                                RepositoryPreparation {
                                    copy_local_changes: repository_options.copy_local_changes,
                                    copy_environment_files: repository_options
                                        .copy_environment_files,
                                    setup_command: (!setup_command.trim().is_empty())
                                        .then(|| setup_command.clone()),
                                },
                            )
                        })
                        .collect();
                    drop(draft);
                    let request = CreateTaskRequest {
                        title: task_title,
                        description: Some(task_description),
                        branch_name: Some(branch_name),
                        branch_source,
                        base_ref,
                        create_missing_branch,
                        replace_divergent_local_branches,
                        existing_branch_action,
                        repository_ids,
                        preparations,
                    };
                    let workspace = workspace_for_submit.clone();
                    let paths = paths_for_submit.clone();
                    let background = cx
                        .background_executor()
                        .spawn(async move { TaskService::new(&paths).create(&workspace, request) });
                    let weak = weak_for_submit.clone();
                    cx.spawn(async move |cx| {
                        let result = background.await;
                        let _ = weak.update(cx, |app, cx| app.finish_background_task(result, cx));
                    })
                    .detach();
                    let _ = weak_for_submit.update(cx, |app, cx| {
                        app.busy = Some(preparing_label.into());
                        cx.notify();
                    });
                    true
                })
        });
    }

    fn terminal_cwd(&self) -> Result<PathBuf> {
        let workspace = self.selected_workspace().context("Select a project")?;
        let cwd = if let Some(task) = self.selected_task() {
            task.repository_path(self.session.selected_repository_id)
        } else {
            workspace.terminal_root(self.session.selected_repository_id)
        };
        cwd.context("The selected target does not have a terminal directory")
    }

    fn note_handle(&self, owner: NoteOwner) -> Option<&NoteHandle> {
        match owner {
            NoteOwner::Project(id) => self.project_notes.get(&id),
            NoteOwner::Task(id) => self.task_notes.get(&id),
        }
    }

    fn note_handle_mut(&mut self, owner: NoteOwner) -> Option<&mut NoteHandle> {
        match owner {
            NoteOwner::Project(id) => self.project_notes.get_mut(&id),
            NoteOwner::Task(id) => self.task_notes.get_mut(&id),
        }
    }

    fn create_note_editor(
        owner: NoteOwner,
        content: String,
        placeholder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(16, 80)
                .placeholder(placeholder)
                .default_value(content)
        });
        editor
    }

    fn note_content(&self, owner: NoteOwner, cx: &App) -> String {
        self.note_handle(owner)
            .map(|handle| handle.editor.read(cx).value().to_string())
            .unwrap_or_default()
    }

    fn queue_note_save(
        &mut self,
        owner: NoteOwner,
        content: String,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.note_handle_mut(owner) else {
            return;
        };
        handle.revision = handle.revision.saturating_add(1);
        handle.save_state = NoteSaveState::Saving;
        let revision = handle.revision;
        let blocks = handle.blocks.clone();
        let target = match owner {
            NoteOwner::Project(workspace_id) => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .cloned()
                .map(NoteSaveTarget::Project),
            NoteOwner::Task(task_id) => self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .cloned()
                .map(NoteSaveTarget::Task),
        };
        let Some(target) = target else {
            return;
        };
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            Timer::after(delay).await;
            let is_current = weak
                .update(cx, |app, _| {
                    app.note_handle(owner)
                        .is_some_and(|handle| handle.revision == revision)
                })
                .unwrap_or(false);
            if !is_current {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    match target {
                        NoteSaveTarget::Project(workspace) => match &blocks {
                            Some(blocks) => {
                                ProjectNoteService::write_document(&workspace, &content, blocks)
                            }
                            None => ProjectNoteService::write(&workspace, &content),
                        },
                        NoteSaveTarget::Task(task) => match &blocks {
                            Some(blocks) => {
                                TaskNoteService::write_document(&task, &content, blocks)
                            }
                            None => TaskNoteService::write(&task, &content),
                        },
                    }
                })
                .await;
            let _ = weak.update(cx, |app, cx| {
                let is_current = app
                    .note_handle(owner)
                    .is_some_and(|handle| handle.revision == revision);
                if !is_current {
                    return;
                }
                if let Some(handle) = app.note_handle_mut(owner) {
                    handle.save_state = if result.is_ok() {
                        NoteSaveState::Saved
                    } else {
                        NoteSaveState::Error
                    };
                }
                if let Err(error) = result {
                    let kind = match owner {
                        NoteOwner::Project(_) => "project",
                        NoteOwner::Task(_) => "task",
                    };
                    app.status = Some((format!("Could not save the {kind} note: {error:#}"), true));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn edit_note(&mut self, owner: NoteOwner, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.note_handle_mut(owner).map(|handle| {
            handle.preview = false;
            handle.editor.clone()
        });
        if let Some(editor) = editor {
            editor.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn toggle_note_preview(
        &mut self,
        owner: NoteOwner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((preview, editor)) = self.note_handle_mut(owner).map(|handle| {
            handle.preview = !handle.preview;
            (handle.preview, handle.editor.clone())
        }) else {
            return;
        };
        if preview {
            let content = self.note_content(owner, cx);
            self.queue_note_save(owner, content, Duration::ZERO, cx);
        } else {
            editor.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn reload_note(&mut self, owner: NoteOwner, cx: &mut Context<Self>) {
        if self
            .note_handle(owner)
            .is_some_and(|note| note.save_state == NoteSaveState::Saving)
        {
            self.set_status("Wait for the note to finish saving", true, cx);
            return;
        }
        match owner {
            NoteOwner::Project(id) => {
                self.project_notes.remove(&id);
                self.show_project_note = true;
            }
            NoteOwner::Task(id) => {
                self.task_notes.remove(&id);
                self.show_task_note = true;
            }
        }
        cx.notify();
    }

    fn update_note_appearance(
        &mut self,
        owner: NoteOwner,
        icon: Option<String>,
        color: Option<WorkspaceColor>,
        cx: &mut Context<Self>,
    ) {
        match owner {
            NoteOwner::Project(workspace_id) => {
                let Some(workspace) = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .cloned()
                else {
                    return;
                };
                self.update_project_presentation(
                    workspace_id,
                    workspace.label().to_string(),
                    icon.unwrap_or(workspace.icon),
                    color.unwrap_or(workspace.color),
                    cx,
                );
            }
            NoteOwner::Task(task_id) => {
                let Some(index) = self.tasks.iter().position(|task| task.id == task_id) else {
                    return;
                };
                let mut updated = self.tasks[index].clone();
                if let Some(icon) = icon {
                    updated.icon = icon;
                }
                if let Some(color) = color {
                    updated.color = color;
                }
                updated.updated_at = Utc::now();
                match self.database.upsert_task(&updated) {
                    Ok(()) => {
                        self.tasks[index] = updated;
                        self.status = None;
                        cx.notify();
                    }
                    Err(error) => self.set_status(
                        format!("Could not update the task appearance: {error:#}"),
                        true,
                        cx,
                    ),
                }
            }
        }
    }

    fn render_note_icon_picker(
        &self,
        owner: NoteOwner,
        icon: &str,
        color: WorkspaceColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner_key = match owner {
            NoteOwner::Project(id) => format!("project-{id}"),
            NoteOwner::Task(id) => format!("task-{id}"),
        };
        let weak = cx.weak_entity();
        let selected_icon = icon.to_string();
        let language = self.session.language;
        let icon_label = self.tr("Icon", "Icono").to_string();
        let color_label = self.tr("Color", "Color").to_string();
        let tooltip = self
            .tr("Change icon and color", "Cambiar icono y color")
            .to_string();
        let accent = workspace_color(color);
        let trigger = Button::new(SharedString::from(format!("note-icon-{owner_key}")))
            .icon(project_icon_kind(icon))
            .with_size(px(72.))
            .rounded(px(16.))
            .ghost()
            .border_1()
            .border_color(with_alpha(accent, 0.28))
            .bg(with_alpha(accent, 0.16))
            .text_color(accent)
            .tooltip(tooltip);

        Popover::new(SharedString::from(format!("note-icon-popover-{owner_key}")))
            .anchor(Corner::TopLeft)
            .trigger(trigger)
            .content(move |_, _, _| {
                let mut icons = h_flex().w_full().gap_2().flex_wrap();
                for (value, _, icon) in project_icon_options(language) {
                    let weak = weak.clone();
                    let selected = value == selected_icon;
                    let value = value.to_string();
                    icons = icons.child(
                        div()
                            .id(SharedString::from(format!(
                                "note-icon-option-{owner_key}-{value}"
                            )))
                            .size(px(42.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(9.))
                            .border_1()
                            .border_color(if selected {
                                workspace_color(color)
                            } else {
                                rgb(0x252a33)
                            })
                            .bg(if selected {
                                with_alpha(workspace_color(color), 0.18)
                            } else {
                                rgb(0x15181e)
                            })
                            .text_color(if selected {
                                workspace_color(color)
                            } else {
                                rgb(0x8e97aa)
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242a35)).text_color(rgb(0xe5e9f0)))
                            .on_click(move |_, _, cx| {
                                let value = value.clone();
                                let _ = weak.update(cx, |app, cx| {
                                    app.update_note_appearance(owner, Some(value), None, cx)
                                });
                            })
                            .child(Icon::new(icon).with_size(px(20.))),
                    );
                }

                let mut colors = h_flex().w_full().gap_2().flex_wrap();
                for option in project_colors() {
                    let weak = weak.clone();
                    colors = colors.child(project_color_button(
                        format!("note-color-option-{owner_key}-{option:?}"),
                        option,
                        option == color,
                        move |_, _, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.update_note_appearance(owner, None, Some(option), cx)
                            });
                        },
                    ));
                }

                v_flex()
                    .w(px(390.))
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0x8e97aa))
                            .child(icon_label.clone()),
                    )
                    .child(icons)
                    .child(div().w_full().border_t_1().border_color(rgb(0x252a33)))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0x8e97aa))
                            .child(color_label.clone()),
                    )
                    .child(colors)
            })
            .into_any_element()
    }

    fn render_note_body(
        &self,
        owner: NoteOwner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(handle) = self.note_handle(owner) else {
            return div().into_any_element();
        };
        let owner_key = match owner {
            NoteOwner::Project(id) => format!("project-{id}"),
            NoteOwner::Task(id) => format!("task-{id}"),
        };
        let markdown = handle.editor.read(cx).value().to_string();
        if handle.preview {
            if markdown.trim().is_empty() {
                let weak = cx.weak_entity();
                return div()
                    .id(SharedString::from(format!("empty-note-{owner_key}")))
                    .w_full()
                    .py_4()
                    .text_size(px(15.))
                    .text_color(rgb(0x697386))
                    .cursor_text()
                    .hover(|style| style.text_color(rgb(0xaeb7c7)))
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |app, cx| app.edit_note(owner, window, cx));
                    })
                    .child(self.tr("Click to add note", "Click para agregar nota"))
                    .into_any_element();
            }
            return TextView::markdown(
                SharedString::from(format!("note-preview-{owner_key}")),
                markdown,
                window,
                cx,
            )
            .w_full()
            .min_w_0()
            .whitespace_normal()
            .text_size(px(15.))
            .line_height(relative(1.5))
            .selectable(true)
            .into_any_element();
        }

        div()
            .id(SharedString::from(format!("note-editor-{owner_key}")))
            .w_full()
            .min_w_0()
            .min_h(px(420.))
            .child(
                Input::new(&handle.editor)
                    .w_full()
                    .min_w_0()
                    .min_h(px(420.))
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false),
            )
            .into_any_element()
    }

    fn render_note_page(
        &self,
        owner: NoteOwner,
        title: String,
        icon: String,
        color: WorkspaceColor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(note) = self.note_handle(owner) else {
            return self.render_empty_state(cx);
        };
        let owner_key = match owner {
            NoteOwner::Project(id) => format!("project-{id}"),
            NoteOwner::Task(id) => format!("task-{id}"),
        };
        let preview = note.preview;
        let save_label = note_save_label(note.save_state, self.session.language);
        let preview_label = if preview {
            self.tr("Editor", "Editor")
        } else {
            self.tr("Preview", "Vista previa")
        };
        let selectable_title = title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let reload_label = self.tr("Reload", "Recargar");
        let weak_toggle = cx.weak_entity();
        let weak_reload = cx.weak_entity();
        let toolbar = h_flex()
            .absolute()
            .top_4()
            .right_5()
            .gap_2()
            .items_center()
            .child(note_save_status(note.save_state, save_label))
            .child(
                Button::new(SharedString::from(format!("note-reload-{owner_key}")))
                    .icon(AppIcon::RefreshCw)
                    .ghost()
                    .small()
                    .tooltip(reload_label)
                    .on_click(move |_, _, cx| {
                        let _ = weak_reload.update(cx, |app, cx| app.reload_note(owner, cx));
                    }),
            )
            .child(note_preview_button(
                SharedString::from(format!("note-preview-toggle-{owner_key}")),
                preview_label,
                move |_, window, cx| {
                    let _ = weak_toggle
                        .update(cx, |app, cx| app.toggle_note_preview(owner, window, cx));
                },
            ));

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .overflow_x_hidden()
                    .w_full()
                    .child(
                        v_flex()
                            // Warp keeps the rich-text viewport at 640px. These
                            // 80px of horizontal gutters leave the same usable width.
                            // A definite width is required here because GPUI's
                            // scroll container measures children with an
                            // unconstrained horizontal axis.
                            .w(px(720.))
                            .max_w_full()
                            .flex_none()
                            .min_w_0()
                            .mx_auto()
                            .px_10()
                            .pt(px(72.))
                            .pb_20()
                            .child(self.render_note_icon_picker(owner, &icon, color, cx))
                            .child(
                                TextView::html(
                                    SharedString::from(format!("note-title-{owner_key}")),
                                    selectable_title,
                                    window,
                                    cx,
                                )
                                .w_full()
                                .min_w_0()
                                .mt_6()
                                .mb_8()
                                .whitespace_normal()
                                .text_size(px(40.))
                                .line_height(px(48.))
                                .font_weight(gpui::FontWeight::BOLD)
                                .selectable(true),
                            )
                            .child(self.render_note_body(owner, window, cx)),
                    ),
            )
            .into_any_element()
    }

    fn ensure_project_note_editor(
        &mut self,
        workspace_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_notes.contains_key(&workspace_id) {
            return;
        }
        let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .cloned()
        else {
            return;
        };
        let (document, save_state) = match ProjectNoteService::ensure(&workspace, "")
            .and_then(|_| ProjectNoteService::read_document(&workspace))
        {
            Ok(document) => (document, NoteSaveState::Saved),
            Err(error) => {
                self.status = Some((format!("Could not read the project note: {error:#}"), true));
                (
                    RichNoteDocument {
                        markdown: String::new(),
                        blocks: None,
                    },
                    NoteSaveState::Error,
                )
            }
        };
        let owner = NoteOwner::Project(workspace_id);
        let placeholder = self
            .tr(
                "Write your note in Markdown…",
                "Escribe tu nota en Markdown…",
            )
            .to_string();
        let editor = Self::create_note_editor(owner, document.markdown, placeholder, window, cx);
        self.project_notes.insert(
            workspace_id,
            NoteHandle {
                document_id: Uuid::new_v4(),
                editor,
                blocks: document.blocks,
                preview: true,
                revision: 0,
                save_state,
            },
        );
    }

    fn show_project_notes(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return;
        }
        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = None;
        self.session.selected_repository_id = None;
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        self.request_workspace_git_summaries(workspace_id, cx);
        self.show_task_note = false;
        self.show_project_note = true;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();
        cx.notify();
    }

    fn render_project_note(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(workspace) = self.selected_workspace() else {
            return self.render_empty_state(cx);
        };
        if !self.project_notes.contains_key(&workspace.id) {
            return self.render_empty_state(cx);
        }
        self.render_note_page(
            NoteOwner::Project(workspace.id),
            workspace.label().to_string(),
            workspace.icon.clone(),
            workspace.color,
            window,
            cx,
        )
    }

    fn ensure_task_note_editor(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.task_notes.contains_key(&task_id) {
            return;
        }
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let (document, save_state) = match TaskNoteService::read_document(&task) {
            Ok(document) => (document, NoteSaveState::Saved),
            Err(error) => {
                self.status = Some((format!("Could not read the task note: {error:#}"), true));
                (
                    RichNoteDocument {
                        markdown: String::new(),
                        blocks: None,
                    },
                    NoteSaveState::Error,
                )
            }
        };
        let owner = NoteOwner::Task(task_id);
        let placeholder = self
            .tr(
                "Write your note in Markdown…",
                "Escribe tu nota en Markdown…",
            )
            .to_string();
        let editor = Self::create_note_editor(owner, document.markdown, placeholder, window, cx);
        self.task_notes.insert(
            task_id,
            NoteHandle {
                document_id: Uuid::new_v4(),
                editor,
                blocks: document.blocks,
                preview: true,
                revision: 0,
                save_state,
            },
        );
    }

    fn show_task_notes(&mut self, cx: &mut Context<Self>) {
        let Some(task_id) = self.session.selected_task_id else {
            return;
        };
        let Some(workspace_id) = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.workspace_id)
        else {
            return;
        };
        self.show_task_notes_for(workspace_id, task_id, cx);
    }

    fn show_task_notes_for(&mut self, workspace_id: Uuid, task_id: Uuid, cx: &mut Context<Self>) {
        if !self
            .tasks
            .iter()
            .any(|task| task.id == task_id && task.workspace_id == workspace_id)
        {
            return;
        }
        self.mark_task_seen(task_id, cx);
        self.session.selected_workspace_id = Some(workspace_id);
        self.session.selected_task_id = Some(task_id);
        self.session.selected_repository_id = None;
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        insert_unique(&mut self.session.expanded_task_ids, task_id);
        self.request_workspace_git_summaries(workspace_id, cx);
        self.request_task_git_summaries(task_id, cx);
        self.show_project_note = false;
        self.show_task_note = true;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();
        cx.notify();
    }

    fn render_task_note(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(task) = self.selected_task() else {
            return self.render_empty_state(cx);
        };
        if !self.task_notes.contains_key(&task.id) {
            return self.render_empty_state(cx);
        }
        self.render_note_page(
            NoteOwner::Task(task.id),
            task.title.clone(),
            task.icon.clone(),
            task.color,
            window,
            cx,
        )
    }

    fn new_terminal(&mut self, agent: AgentKind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = true;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        let Some(workspace_id) = self.session.selected_workspace_id else {
            self.set_status("Select a project first", true, cx);
            return;
        };
        let cwd = match self.terminal_cwd() {
            Ok(cwd) => cwd,
            Err(error) => {
                self.set_status(error.to_string(), true, cx);
                return;
            }
        };
        let now = Utc::now();
        let descriptor = TerminalDescriptor {
            id: Uuid::new_v4(),
            workspace_id,
            task_id: self.session.selected_task_id,
            repository_id: self.session.selected_repository_id,
            agent,
            label: agent.label().into(),
            cwd,
            state: if agent == AgentKind::Shell {
                SessionState::Idle
            } else {
                SessionState::Working
            },
            codex_session: None,
            claude_session: None,
            created_at: now,
        };

        if let Err(error) = self.spawn_terminal_view(&descriptor, window, cx) {
            self.set_status(format!("Could not open terminal: {error:#}"), true, cx);
            return;
        }

        let key = dock_key(workspace_id, descriptor.task_id, descriptor.repository_id);
        let dock = self.session.docks.entry(key).or_default();
        let tab_id = Uuid::new_v4();
        dock.tabs.push(DockTab {
            id: tab_id,
            title: descriptor.label.clone(),
            root: DockNode::Panel {
                terminal_id: descriptor.id,
            },
            active_terminal_id: descriptor.id,
        });
        dock.active_tab_id = Some(tab_id);
        self.session.terminals.push(descriptor.clone());
        insert_unique(&mut self.session.expanded_workspace_ids, workspace_id);
        if let Some(task_id) = descriptor.task_id {
            insert_unique(&mut self.session.expanded_task_ids, task_id);
        }
        self.add_task_session(&descriptor);
        self.persist_session();
        if let Some(handle) = self.terminals.get(&descriptor.id) {
            handle.view.read(cx).focus_handle().focus(window);
        }
        cx.notify();
    }

    fn spawn_terminal_view(
        &mut self,
        descriptor: &TerminalDescriptor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if self.terminals.contains_key(&descriptor.id) {
            return Ok(());
        }
        let spawned = TerminalService.spawn(descriptor)?;
        let master = spawned.master.clone();
        let master_for_resize = spawned.master.clone();
        let terminal_id = descriptor.id;
        let weak_for_bell = cx.weak_entity();
        let weak_for_exit = cx.weak_entity();
        let weak_for_title = cx.weak_entity();
        let weak_for_agent = cx.weak_entity();
        let weak_for_screen = cx.weak_entity();
        let config = terminal_config(self.session.theme);
        let view = cx.new(|cx| {
            FastTerminalView::new(spawned.writer, spawned.reader, config, window, cx)
                .with_resize_callback(move |cols, rows| {
                    let _ = master_for_resize.lock().resize(PtySize {
                        cols: cols.min(u16::MAX as usize) as u16,
                        rows: rows.min(u16::MAX as usize) as u16,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                })
                .with_bell_callback(move |cx| {
                    let notification = weak_for_bell
                        .update(cx, |app, cx| app.handle_agent_attention(terminal_id, cx))
                        .ok()
                        .flatten();
                    if let Some((title, message)) = notification {
                        play_agent_attention_sound();
                        cx.background_executor()
                            .spawn(async move {
                                show_native_agent_notification(&title, &message);
                            })
                            .detach();
                    }
                })
                .with_title_callback(move |cx, title| {
                    let title = title.trim();
                    if !title.is_empty() {
                        let _ = weak_for_title.update(cx, |app, cx| {
                            app.update_terminal_title(terminal_id, title);
                            cx.notify();
                        });
                    }
                })
                .with_agent_callback(move |signal, cx| {
                    let notification = weak_for_agent
                        .update(cx, |app, cx| {
                            app.handle_agent_signal(terminal_id, signal, cx)
                        })
                        .ok()
                        .flatten();
                    if let Some((title, message)) = notification {
                        play_agent_attention_sound();
                        cx.background_executor()
                            .spawn(async move {
                                show_native_agent_notification(&title, &message);
                            })
                            .detach();
                    }
                })
                .with_screen_mode_callback(move |alternate, cx| {
                    if !alternate {
                        let _ = weak_for_screen.update(cx, |app, cx| {
                            app.reset_terminal_agent(terminal_id, cx);
                            cx.notify();
                        });
                    }
                })
                .with_clipboard_store_callback(|cx, text| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                })
                .with_exit_callback(move |cx| {
                    let _ = weak_for_exit.update(cx, |app, cx| {
                        app.refresh_terminal_repository_git_summary(terminal_id, cx);
                        app.update_terminal_state(terminal_id, SessionState::Exited);
                        cx.notify();
                    });
                })
        });

        self.terminals.insert(
            terminal_id,
            TerminalHandle {
                view,
                master,
                child: spawned.child,
                process_id: spawned.process_id,
            },
        );
        Ok(())
    }

    fn restart_terminal(&mut self, terminal_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut descriptor) = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()
        else {
            return;
        };
        descriptor.state = if descriptor.agent == AgentKind::Shell {
            SessionState::Idle
        } else {
            SessionState::Working
        };
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = true;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        match self.spawn_terminal_view(&descriptor, window, cx) {
            Ok(()) => {
                self.update_terminal_state(terminal_id, descriptor.state);
                if let Some(handle) = self.terminals.get(&terminal_id) {
                    handle.view.read(cx).focus_handle().focus(window);
                }
                self.persist_session();
                cx.notify();
            }
            Err(error) => {
                self.set_status(format!("Could not restore terminal: {error:#}"), true, cx)
            }
        }
    }

    fn add_task_session(&mut self, descriptor: &TerminalDescriptor) {
        let Some(task_id) = descriptor.task_id else {
            return;
        };
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) else {
            return;
        };
        let now = Utc::now();
        task.sessions.push(TaskSession {
            id: Uuid::new_v4(),
            repository_id: descriptor.repository_id,
            terminal_local_id: descriptor.id,
            agent: descriptor.agent,
            label: descriptor.label.clone(),
            state: descriptor.state,
            created_at: now,
            updated_at: now,
            exited_at: None,
        });
        task.updated_at = now;
        let _ = self.database.upsert_task(task);
    }

    fn update_terminal_state(&mut self, terminal_id: Uuid, state: SessionState) {
        if state != SessionState::Attention {
            self.app_toasts
                .retain(|toast| toast.target.terminal_id() != Some(terminal_id));
        }
        if let Some(descriptor) = self
            .session
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == terminal_id)
        {
            descriptor.state = state;
        }
        let now = Utc::now();
        for task in &mut self.tasks {
            if let Some(session) = task
                .sessions
                .iter_mut()
                .find(|session| session.terminal_local_id == terminal_id)
            {
                session.state = state;
                session.updated_at = now;
                if state == SessionState::Exited {
                    session.exited_at = Some(now);
                }
                task.updated_at = now;
                let _ = self.database.upsert_task(task);
                break;
            }
        }
        self.persist_session();
    }

    fn set_terminal_agent(&mut self, terminal_id: Uuid, agent: AgentKind) {
        if let Some(descriptor) = self
            .session
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == terminal_id)
        {
            descriptor.agent = agent;
            match agent {
                AgentKind::Codex => descriptor.claude_session = None,
                AgentKind::Claude => descriptor.codex_session = None,
                AgentKind::Shell | AgentKind::Gemini => {
                    descriptor.codex_session = None;
                    descriptor.claude_session = None;
                }
            }
        }
        for task in &mut self.tasks {
            if let Some(session) = task
                .sessions
                .iter_mut()
                .find(|session| session.terminal_local_id == terminal_id)
            {
                session.agent = agent;
                break;
            }
        }
    }

    fn handle_agent_signal(
        &mut self,
        terminal_id: Uuid,
        signal: AgentTerminalSignal,
        cx: &mut Context<Self>,
    ) -> Option<(String, String)> {
        if let Some(agent) = signal.agent {
            self.set_terminal_agent(terminal_id, agent);
        }
        let agent = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .map(|terminal| terminal.agent)?;
        if agent == AgentKind::Shell {
            return None;
        }

        match signal.kind {
            AgentTerminalSignalKind::Started | AgentTerminalSignalKind::Working => {
                self.update_terminal_state(terminal_id, SessionState::Working);
                cx.notify();
                None
            }
            AgentTerminalSignalKind::Attention => self.handle_agent_attention(terminal_id, cx),
        }
    }

    fn reset_terminal_agent(&mut self, terminal_id: Uuid, cx: &mut Context<Self>) {
        // Leaving the alternate screen is not enough to prove that a coding agent exited.
        // Codex can change screen modes between turns while its process still owns the PTY.
        // Querying the foreground process group here is event-driven (no polling): the login
        // shell only regains ownership after the agent process actually finishes.
        if !self.terminal_shell_owns_foreground(terminal_id) {
            return;
        }
        let should_reset = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .is_some_and(|terminal| {
                terminal.agent != AgentKind::Shell && terminal.state != SessionState::Exited
            });
        if should_reset {
            self.refresh_terminal_repository_git_summary(terminal_id, cx);
            self.set_terminal_agent(terminal_id, AgentKind::Shell);
            self.update_terminal_state(terminal_id, SessionState::Idle);
        }
    }

    fn terminal_shell_owns_foreground(&self, terminal_id: Uuid) -> bool {
        let Some(handle) = self.terminals.get(&terminal_id) else {
            return false;
        };
        let Some(shell_process_id) = handle.process_id.and_then(|id| i32::try_from(id).ok()) else {
            return false;
        };

        #[cfg(unix)]
        {
            handle.master.lock().process_group_leader() == Some(shell_process_id)
        }

        #[cfg(not(unix))]
        {
            let _ = shell_process_id;
            false
        }
    }

    fn handle_agent_attention(
        &mut self,
        terminal_id: Uuid,
        cx: &mut Context<Self>,
    ) -> Option<(String, String)> {
        let descriptor = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()?;

        if descriptor.agent == AgentKind::Shell {
            return None;
        }

        self.refresh_terminal_repository_git_summary(terminal_id, cx);
        self.update_terminal_state(terminal_id, SessionState::Attention);
        let target = AppToastTarget::Terminal {
            terminal_id,
            agent: descriptor.agent,
        };
        if self
            .app_toasts
            .iter()
            .any(|toast| toast.target.terminal_id() == Some(terminal_id))
        {
            cx.notify();
            return None;
        }

        let context = descriptor
            .task_id
            .and_then(|task_id| {
                self.tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .map(|task| task.title.clone())
            })
            .or_else(|| {
                self.workspaces
                    .iter()
                    .find(|workspace| workspace.id == descriptor.workspace_id)
                    .map(|workspace| workspace.label().to_string())
            })
            .unwrap_or_else(|| descriptor.label.clone());
        let (title, message) = match self.session.language {
            Language::English => (
                format!("{} needs your attention", descriptor.agent.label()),
                format!("Waiting for your response in {context}"),
            ),
            Language::Spanish => (
                format!("{} necesita tu atención", descriptor.agent.label()),
                format!("Esperando tu respuesta en {context}"),
            ),
        };

        self.app_toasts.push(AppToast {
            target,
            title: title.clone(),
            message: message.clone(),
        });
        cx.notify();
        Some((title, message))
    }

    fn dismiss_app_toast(&mut self, target: AppToastTarget, cx: &mut Context<Self>) {
        self.app_toasts.retain(|toast| toast.target != target);
        cx.notify();
    }

    fn update_terminal_title(&mut self, terminal_id: Uuid, title: &str) {
        let display_title = title;
        // An unrecognized title is not an exit signal: Codex restores the project title after
        // every response while remaining open. Only classify the session as a shell again when
        // the login shell has actually regained the PTY's foreground process group.
        let detected_agent = agent_from_terminal_title(title).or_else(|| {
            self.terminal_shell_owns_foreground(terminal_id)
                .then_some(AgentKind::Shell)
        });
        let mut agent_changed = None;
        if let Some(descriptor) = self
            .session
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == terminal_id)
        {
            descriptor.label = display_title.chars().take(72).collect();
            if let Some(agent) = detected_agent
                && descriptor.agent != agent
            {
                descriptor.agent = agent;
                descriptor.state = if agent == AgentKind::Shell {
                    SessionState::Idle
                } else {
                    SessionState::Working
                };
                agent_changed = Some((agent, descriptor.state));
            }
        }
        for dock in self.session.docks.values_mut() {
            for tab in &mut dock.tabs {
                let mut ids = Vec::new();
                tab.root.terminal_ids(&mut ids);
                if ids.contains(&terminal_id) && ids.len() == 1 {
                    tab.title = display_title.chars().take(72).collect();
                }
            }
        }
        if let Some((agent, state)) = agent_changed {
            if agent == AgentKind::Shell {
                self.app_toasts
                    .retain(|toast| toast.target.terminal_id() != Some(terminal_id));
            }
            let now = Utc::now();
            for task in &mut self.tasks {
                if let Some(session) = task
                    .sessions
                    .iter_mut()
                    .find(|session| session.terminal_local_id == terminal_id)
                {
                    session.agent = agent;
                    session.label = display_title.chars().take(72).collect();
                    session.state = state;
                    session.updated_at = now;
                    task.updated_at = now;
                    let _ = self.database.upsert_task(task);
                    break;
                }
            }
            self.persist_session();
        }
    }

    fn focus_terminal(&mut self, terminal_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(descriptor) = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()
        else {
            return;
        };

        self.app_toasts
            .retain(|toast| toast.target.terminal_id() != Some(terminal_id));

        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = true;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.session.selected_workspace_id = Some(descriptor.workspace_id);
        self.session.selected_task_id = descriptor.task_id;
        self.session.selected_repository_id = descriptor.repository_id;
        insert_unique(
            &mut self.session.expanded_workspace_ids,
            descriptor.workspace_id,
        );
        if let Some(task_id) = descriptor.task_id {
            insert_unique(&mut self.session.expanded_task_ids, task_id);
        }
        let key = dock_key(
            descriptor.workspace_id,
            descriptor.task_id,
            descriptor.repository_id,
        );
        if let Some(dock) = self.session.docks.get_mut(&key)
            && let Some(tab) = dock.tabs.iter_mut().find(|tab| {
                let mut ids = Vec::new();
                tab.root.terminal_ids(&mut ids);
                ids.contains(&terminal_id)
            })
        {
            tab.active_terminal_id = terminal_id;
            dock.active_tab_id = Some(tab.id);
        }

        if !self.terminals.contains_key(&terminal_id) {
            self.restart_terminal(terminal_id, window, cx);
            return;
        }

        if let Some(handle) = self.terminals.get(&terminal_id) {
            handle.view.read(cx).focus_handle().focus(window);
            if descriptor.state == SessionState::Attention {
                let resumed_state = if descriptor.agent == AgentKind::Shell {
                    SessionState::Idle
                } else {
                    SessionState::Working
                };
                self.update_terminal_state(terminal_id, resumed_state);
                cx.notify();
                return;
            }
        }

        self.persist_session();
        cx.notify();
    }

    fn close_terminal(&mut self, terminal_id: Uuid, cx: &mut Context<Self>) {
        self.close_terminal_internal(terminal_id, true, cx);
    }

    fn close_terminal_internal(
        &mut self,
        terminal_id: Uuid,
        refresh_git: bool,
        cx: &mut Context<Self>,
    ) {
        if refresh_git {
            self.refresh_terminal_repository_git_summary(terminal_id, cx);
        }
        if let Some(handle) = self.terminals.remove(&terminal_id) {
            let _ = handle.child.lock().kill();
        }
        self.session
            .terminals
            .retain(|terminal| terminal.id != terminal_id);
        for dock in self.session.docks.values_mut() {
            let mut empty_tabs = Vec::new();
            for tab in &mut dock.tabs {
                if let Some(root) = remove_terminal_node(tab.root.clone(), terminal_id) {
                    tab.root = root;
                    let mut ids = Vec::new();
                    tab.root.terminal_ids(&mut ids);
                    if !ids.contains(&tab.active_terminal_id) {
                        tab.active_terminal_id = ids[0];
                    }
                } else {
                    empty_tabs.push(tab.id);
                }
            }
            dock.tabs.retain(|tab| !empty_tabs.contains(&tab.id));
            if dock
                .active_tab_id
                .is_some_and(|id| empty_tabs.contains(&id))
            {
                dock.active_tab_id = dock.tabs.last().map(|tab| tab.id);
            }
        }
        self.update_terminal_state(terminal_id, SessionState::Exited);
        self.persist_session();
        cx.notify();
    }

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if self.session.language == language {
            return;
        }
        self.session.language = language;
        self.persist_session();
        self.hydrate_orchestrator_chat(cx);
        self.hydrate_quick_open_overlay(cx);
        cx.notify();
    }

    fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        if self.session.theme == theme {
            return;
        }
        self.session.theme = theme;
        self.persist_session();
        apply_native_theme(theme, None, cx);

        let terminal_views = self
            .terminals
            .values()
            .map(|terminal| terminal.view.clone())
            .collect::<Vec<_>>();
        for terminal in terminal_views {
            terminal.update(cx, |terminal, cx| {
                terminal.update_config(terminal_config(theme), cx)
            });
        }

        self.dispatch_orchestrator_event(
            serde_json::json!({
                "type": "theme_changed",
                "theme": app_theme_id(theme),
            }),
            cx,
        );
        self.hydrate_navigation(cx);
        self.hydrate_active_workspace_surface(cx);
        self.hydrate_quick_open_overlay(cx);
        cx.notify();
    }

    fn set_sidebar_width(&mut self, width: f32, commit: bool, cx: &mut Context<Self>) {
        if !width.is_finite() { return; }
        let width = width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        if (self.session.sidebar_width - width).abs() >= 0.5 {
            self.session.sidebar_width = width;
            cx.notify();
        }
        // Drag frames resize in memory; save once on release or keyboard adjustment.
        if commit { self.persist_session(); }
    }

    fn show_settings(&mut self, cx: &mut Context<Self>) {
        self.flush_active_file(cx);
        if !self.show_settings {
            self.settings_return_view = Some((self.show_terminal, self.show_task_note, self.show_project_note, self.project_settings_workspace_id));
        }
        self.show_project_note = false;
        self.show_task_note = false;
        self.show_terminal = false;
        self.show_settings = true;
        self.project_settings_workspace_id = None;
        self.refresh_model_catalog(false, cx);
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.show_settings { return; }
        self.show_settings = false;
        if let Some((terminal, task_note, project_note, project_settings)) = self.settings_return_view.take() {
            self.show_terminal = terminal;
            self.show_task_note = task_note;
            self.show_project_note = project_note;
            self.project_settings_workspace_id = project_settings;
        }
        self.hydrate_navigation(cx);
        self.hydrate_orchestrator_chat(cx);
        self.hydrate_active_workspace_surface(cx);
        cx.notify();
    }

    fn invalidate_plan_usage(&mut self, cx: &mut Context<Self>) {
        self.plan_usage_generation = self.plan_usage_generation.wrapping_add(1);
        self.active_plan_usage = None;
        self.plan_usage_updated_at = None;
        self.plan_usage_refreshing = false;
        self.plan_usage_refresh_error = false;
        self.refresh_plan_usage(cx);
    }

    fn refresh_plan_usage(&mut self, cx: &mut Context<Self>) {
        if !self.show_settings || self.plan_usage_refreshing { return; }
        self.plan_usage_refreshing = true;
        self.plan_usage_refresh_error = false;
        let provider = self.agent_provider();
        let generation = self.plan_usage_generation;
        let auth_mode = self.agent_auth_mode(provider);
        let profile = self.paths.agent_profiles.join(provider.id());
        let background = cx.background_executor().spawn(async move {
            refresh_agent_plan_usage(provider, auth_mode, profile)
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = background.await;
            let _ = weak.update(cx, |app, cx| {
                if app.plan_usage_generation != generation { return; }
                app.plan_usage_refreshing = false;
                // An account change must not publish the previous account's limits.
                if app.agent_provider() != provider || app.agent_auth_mode(provider) != auth_mode {
                    app.plan_usage_refresh_error = true;
                } else {
                    match result {
                        Ok(usage) => {
                            app.active_plan_usage = Some(usage);
                            app.plan_usage_updated_at = Some(Utc::now());
                        }
                        Err(_) => app.plan_usage_refresh_error = true,
                    }
                }
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
            });
        }).detach();
        self.hydrate_active_workspace_surface(cx);
        cx.notify();
    }

    fn reveal_projects_root(&mut self, cx: &mut Context<Self>) {
        let path = self.projects_root();
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("open").arg(&path).spawn();
        #[cfg(target_os = "linux")]
        let result = std::process::Command::new("xdg-open").arg(&path).spawn();
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("explorer").arg(&path).spawn();

        if let Err(error) = result {
            self.set_status(
                format!("Could not reveal {}: {error}", path.display()),
                true,
                cx,
            );
        }
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let weak_reveal = weak.clone();
        let weak_change = weak.clone();
        let weak_english = weak.clone();
        let weak_spanish = weak.clone();
        let weak_full_access = weak.clone();
        let weak_standard_access = weak.clone();
        let weak_import_skills = weak.clone();
        let weak_reveal_skills = weak.clone();
        let weak_provider_claude = weak.clone();
        let weak_provider_codex = weak.clone();
        let weak_provider_gemini = weak.clone();
        let weak_provider_opencode = weak.clone();
        let weak_auth_system = weak.clone();
        let weak_auth_isolated = weak.clone();
        let weak_authenticate = weak.clone();
        let weak_auth_submit = weak.clone();
        let weak_auth_cancel = weak.clone();
        let projects_root = self.projects_root();
        let projects_label = self.tr("Projects", "Proyectos");
        let folder_label = self.tr("Projects folder", "Carpeta de proyectos");
        let folder_description = self.tr(
            "New projects and repositories cloned from GitHub are created here. Existing projects are not moved.",
            "Los proyectos nuevos y los repositorios clonados desde GitHub se crean aquí. Los proyectos existentes no se mueven.",
        );
        let reveal_label = self.tr("Reveal in Finder", "Mostrar en Finder");
        let change_label = self.tr("Change folder", "Cambiar carpeta");
        let claude_label = self.tr("Claude usage", "Uso de Claude");
        let claude_description = self.tr(
            "Plan limits and estimated consumption reported by the Claude Agent SDK. This snapshot refreshes after every agent response.",
            "Límites del plan y consumo estimado reportados por Claude Agent SDK. Esta información se actualiza después de cada respuesta de un agente.",
        );
        let claude_usage_link = self.tr("Open Claude usage", "Abrir uso en Claude");
        let provider_label = self.tr("Agent runtime", "Motor de agentes");
        let provider_description = self.tr(
            "Choose the native agent loop used by every Black Bot. Each provider keeps its own session and authentication profile.",
            "Elige el loop de agente nativo que usará cada Black Bot. Cada proveedor mantiene su propia sesión y perfil de autenticación.",
        );
        let selected_provider = self.agent_provider();
        let selected_auth_mode = self.agent_auth_mode(selected_provider);
        let mut authentication_feedback = Vec::new();
        if let Some(authentication) = self
            .agent_authentication
            .as_ref()
            .filter(|authentication| authentication.provider == selected_provider)
        {
            let status = authentication.status;
            let (status_title, status_color) = match status {
                AgentAuthStatus::Connecting => (
                    self.tr("Connecting account…", "Conectando cuenta…"),
                    rgb(0xa997ef),
                ),
                AgentAuthStatus::NeedsInput => (
                    self.tr("Authorization required", "Autorización requerida"),
                    rgb(0xe3b76f),
                ),
                AgentAuthStatus::Connected => (
                    self.tr("Account connected", "Cuenta conectada"),
                    rgb(0x66ca91),
                ),
                AgentAuthStatus::Error => (
                    self.tr("Could not connect", "No se pudo conectar"),
                    rgb(0xe07878),
                ),
            };
            let mut status_row = h_flex()
                .items_center()
                .gap_2()
                .child(div().size(px(8.)).rounded_full().bg(status_color))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(status_color)
                        .child(status_title),
                );
            if status == AgentAuthStatus::Connecting {
                status_row = status_row.child(agent_working_dots());
            }

            let mut card = v_flex()
                .w_full()
                .gap_3()
                .p_4()
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(0x303642))
                .bg(rgb(0x111318))
                .child(status_row)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xaab2c0))
                        .child(authentication.detail.clone()),
                );

            if status == AgentAuthStatus::NeedsInput {
                let input = authentication.input.clone();
                card = card.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .child(Input::new(&input).w_full())
                        .child(compact_button(
                            format!("settings-auth-submit-{}", selected_provider.id()),
                            self.tr("Continue", "Continuar"),
                            move |_, window, cx| {
                                let _ = weak_auth_submit
                                    .update(cx, |app, cx| app.submit_agent_auth_input(window, cx));
                            },
                        )),
                );
            }

            let mut actions = h_flex().items_center().gap_2().flex_wrap();
            if let Some(url) = authentication.opened_url.clone() {
                actions = actions.child(compact_button(
                    format!("settings-auth-browser-{}", selected_provider.id()),
                    self.tr("Open browser again", "Abrir navegador de nuevo"),
                    move |_, _, cx| cx.open_url(&url),
                ));
            }
            actions = actions.child(compact_button(
                format!("settings-auth-close-{}", selected_provider.id()),
                if matches!(
                    status,
                    AgentAuthStatus::Connecting | AgentAuthStatus::NeedsInput
                ) {
                    self.tr("Cancel", "Cancelar")
                } else {
                    self.tr("Close", "Cerrar")
                },
                move |_, _, cx| {
                    let _ =
                        weak_auth_cancel.update(cx, |app, cx| app.cancel_agent_authentication(cx));
                },
            ));
            authentication_feedback.push(card.child(actions).into_any_element());
        }
        let model_label = format!(
            "{} · {}",
            self.tr("Agent model", "Modelo del agente"),
            selected_provider.display_name()
        );
        let model_description = self.tr(
            "Choose the model used for upcoming global, project, and task responses.",
            "Elige el modelo usado en las próximas respuestas globales, de proyecto y de tarea.",
        );
        let selected_model = self
            .agent_model(selected_provider)
            .unwrap_or_else(|| "automatic".to_string());
        let model_options = self.agent_model_options(selected_provider);
        let mut model_buttons = Vec::with_capacity(model_options.len());
        for (value, label) in model_options {
            let weak_model = weak.clone();
            let model_value = value.to_string();
            let selected = selected_model == value;
            model_buttons.push(choice_button(
                format!("settings-model-{}-{value}", selected_provider.id()),
                &label,
                selected,
                move |_, _, cx| {
                    let _ = weak_model.update(cx, |app, cx| {
                        app.set_agent_model(selected_provider, &model_value, cx)
                    });
                },
            ));
        }
        let mut effort_controls = Vec::new();
        if !self.agent_effort_options(selected_provider).is_empty() {
            let selected_effort = self
                .agent_effort(selected_provider)
                .unwrap_or_else(|| "automatic".to_string());
            let effort_options = self.agent_effort_options(selected_provider);
            let mut effort_buttons = Vec::with_capacity(effort_options.len());
            for (value, label) in effort_options {
                let weak_effort = weak.clone();
                let effort_value = value.to_string();
                effort_buttons.push(choice_button(
                    format!("settings-effort-{}-{value}", selected_provider.id()),
                    &label,
                    selected_effort == value,
                    move |_, _, cx| {
                        let _ = weak_effort.update(cx, |app, cx| {
                            app.set_agent_effort(selected_provider, &effort_value, cx)
                        });
                    },
                ));
            }
            effort_controls.push(
                div()
                    .mt_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(0x252a33))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x8e97aa))
                    .child(self.tr("REASONING EFFORT", "ESFUERZO DE RAZONAMIENTO"))
                    .into_any_element(),
            );
            effort_controls.push(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0x8e97aa))
                    .child(self.tr(
                        "Controls how much reasoning the selected model uses. Available levels can vary by model.",
                        "Controla cuánto razonamiento usa el modelo seleccionado. Los niveles disponibles pueden variar según el modelo.",
                    ))
                    .into_any_element(),
            );
            effort_controls.push(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(effort_buttons)
                    .into_any_element(),
            );
        }
        let skills_label = self.tr("Agent skills", "Skills de agentes");
        let skills_description = self.tr(
            "Only skills explicitly imported and enabled here are available to Black Bots. Personal skills from ~/.claude or ~/.codex are not loaded.",
            "Solo las skills importadas explícitamente y activadas aquí estarán disponibles para los Black Bots. No se cargan las skills personales de ~/.claude ni ~/.codex.",
        );
        let import_skills_label = self.tr("Import skills…", "Importar skills…");
        let reveal_skills_label = self.tr("Reveal folder", "Mostrar carpeta");
        let import_dialog_title = self
            .tr(
                "Choose a skill folder or a folder containing skills",
                "Elige una skill o una carpeta que contenga skills",
            )
            .to_string();
        let enabled_skill_names = self.enabled_agent_skill_names();
        let available_skills = self.agent_skills();
        let skill_count = available_skills.len();
        let mut skill_rows = Vec::with_capacity(skill_count.max(1));
        for skill in available_skills {
            let enabled = enabled_skill_names.contains(&skill.name);
            let skill_name = skill.name.clone();
            let weak_toggle = weak.clone();
            skill_rows.push(settings_agent_skill_row(
                skill,
                enabled,
                if enabled {
                    self.tr("Enabled", "Activada")
                } else {
                    self.tr("Disabled", "Desactivada")
                },
                move |_, _, cx| {
                    let skill_name = skill_name.clone();
                    let _ = weak_toggle.update(cx, |app, cx| {
                        app.set_agent_skill_enabled(skill_name, !enabled, cx)
                    });
                },
            ));
        }
        if skill_rows.is_empty() {
            skill_rows.push(
                div()
                    .w_full()
                    .p_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(0x2b303a))
                    .bg(rgb(0x111318))
                    .text_size(px(12.))
                    .text_color(rgb(0x8e97aa))
                    .child(self.tr(
                        "No skills imported yet. Choose either one skill folder containing SKILL.md or a collection whose direct child folders contain SKILL.md.",
                        "Todavía no importaste skills. Elige una carpeta de skill que contenga SKILL.md o una colección cuyas carpetas hijas directas contengan SKILL.md.",
                    ))
                    .into_any_element(),
            );
        }
        let permissions_label = self.tr("Agent permissions", "Permisos de agentes");
        let permissions_description = self.tr(
            "Controls permissions for the selected native runtime. Full access bypasses provider prompts and allows Bash, filesystem, containers, network, and authenticated Git operations.",
            "Controla los permisos del runtime nativo seleccionado. Acceso total omite las confirmaciones del proveedor y permite Bash, archivos, contenedores, red y operaciones Git autenticadas.",
        );
        let full_access = self.agents_full_access();
        let plan_usage = self.orchestrator_chats.latest_plan_usage();
        let usage_totals = self.orchestrator_chats.usage_totals();
        let plan_name = claude_plan_name(plan_usage, self.session.language);
        let plan_detail = claude_plan_detail(plan_usage, self.session.language);
        let rate_limits = plan_usage.and_then(|usage| usage.rate_limits.as_ref());
        let (five_hour_value, five_hour_detail, five_hour_used) = claude_limit_display(
            rate_limits.and_then(|limits| limits.five_hour.as_ref()),
            self.session.language,
        );
        let (seven_day_value, seven_day_detail, seven_day_used) = claude_limit_display(
            rate_limits.and_then(|limits| limits.seven_day.as_ref()),
            self.session.language,
        );
        let cost_value = format!("${:.4}", usage_totals.cost_usd);
        let cost_detail = format!(
            "{} · {}",
            match self.session.language {
                Language::English => format!("{} requests", usage_totals.requests),
                Language::Spanish => format!("{} solicitudes", usage_totals.requests),
            },
            match self.session.language {
                Language::English => format!("{} agent turns", usage_totals.num_turns),
                Language::Spanish => format!("{} turnos de agente", usage_totals.num_turns),
            },
        );
        let token_detail = format!(
            "{}: {}  ·  {}: {}  ·  {}: {}  ·  {}: {}",
            self.tr("Input", "Entrada"),
            format_token_count(usage_totals.input_tokens),
            self.tr("Output", "Salida"),
            format_token_count(usage_totals.output_tokens),
            self.tr("Cache read", "Caché leída"),
            format_token_count(usage_totals.cache_read_input_tokens),
            self.tr("Cache written", "Caché escrita"),
            format_token_count(usage_totals.cache_creation_input_tokens),
        );
        let updated_detail = self
            .orchestrator_chats
            .usage_updated_at()
            .map(|timestamp| {
                let timestamp = timestamp.with_timezone(&chrono::Local);
                match self.session.language {
                    Language::English => {
                        format!("Last updated {}", timestamp.format("%b %-d, %H:%M"))
                    }
                    Language::Spanish => {
                        format!("Actualizado el {}", timestamp.format("%-d/%m, %H:%M"))
                    }
                }
            })
            .unwrap_or_else(|| {
                self.tr(
                    "Usage will appear after the next agent response.",
                    "El consumo aparecerá después de la próxima respuesta de un agente.",
                )
                .to_string()
            });
        let language_label = self.tr("Language", "Idioma");
        let language_description = self.tr(
            "Choose the language used by the Blackholes interface.",
            "Elige el idioma de la interfaz de Blackholes.",
        );

        v_flex()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .px_8()
            .py_6()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(920.))
                    .gap_6()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(38.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .bg(rgb(0x2a1d19))
                                    .text_color(rgb(0xe39a78))
                                    .child(Icon::new(AppIcon::Settings).with_size(px(20.))),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(20.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(self.tr("Settings", "Configuración")),
                                    )
                                    .child(
                                        div().text_size(px(11.)).text_color(rgb(0x8e97aa)).child(
                                            self.tr(
                                                "Manage projects, appearance, and language.",
                                                "Administra proyectos, apariencia e idioma.",
                                            ),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8e97aa))
                                    .child(provider_label.to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(provider_description),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(choice_button(
                                        "settings-provider-claude",
                                        "Claude",
                                        selected_provider == AgentProvider::Claude,
                                        move |_, _, cx| {
                                            let _ = weak_provider_claude.update(cx, |app, cx| {
                                                app.set_agent_provider(AgentProvider::Claude, cx)
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        "settings-provider-codex",
                                        "Codex",
                                        selected_provider == AgentProvider::Codex,
                                        move |_, _, cx| {
                                            let _ = weak_provider_codex.update(cx, |app, cx| {
                                                app.set_agent_provider(AgentProvider::Codex, cx)
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        "settings-provider-gemini",
                                        "Gemini",
                                        selected_provider == AgentProvider::Gemini,
                                        move |_, _, cx| {
                                            let _ = weak_provider_gemini.update(cx, |app, cx| {
                                                app.set_agent_provider(AgentProvider::Gemini, cx)
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        "settings-provider-opencode",
                                        "OpenCode · Generic",
                                        selected_provider == AgentProvider::OpenCode,
                                        move |_, _, cx| {
                                            let _ = weak_provider_opencode.update(cx, |app, cx| {
                                                app.set_agent_provider(AgentProvider::OpenCode, cx)
                                            });
                                        },
                                    )),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(choice_button(
                                        format!("settings-auth-system-{}", selected_provider.id()),
                                        self.tr("Computer account", "Cuenta de la computadora"),
                                        selected_auth_mode == AgentAuthMode::System,
                                        move |_, _, cx| {
                                            let _ = weak_auth_system.update(cx, |app, cx| {
                                                app.set_agent_auth_mode(
                                                    selected_provider,
                                                    AgentAuthMode::System,
                                                    cx,
                                                )
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        format!(
                                            "settings-auth-isolated-{}",
                                            selected_provider.id()
                                        ),
                                        self.tr("Blackholes account", "Cuenta de Blackholes"),
                                        selected_auth_mode == AgentAuthMode::Isolated,
                                        move |_, _, cx| {
                                            let _ = weak_auth_isolated.update(cx, |app, cx| {
                                                app.set_agent_auth_mode(
                                                    selected_provider,
                                                    AgentAuthMode::Isolated,
                                                    cx,
                                                )
                                            });
                                        },
                                    ))
                                    .child(compact_button(
                                        format!(
                                            "settings-authenticate-{}",
                                            selected_provider.id()
                                        ),
                                        self.tr(
                                            "Authenticate / change account…",
                                            "Autenticar / cambiar cuenta…",
                                        ),
                                        move |_, window, cx| {
                                            let _ = weak_authenticate.update(cx, |app, cx| {
                                                app.authenticate_agent_provider(
                                                    selected_provider,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    )),
                            )
                            .children(authentication_feedback)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(self.tr(
                                        "Computer account uses the provider login already installed on this Mac. Blackholes account stores an independent provider profile under Application Support; credentials are never copied into the database.",
                                        "Cuenta de la computadora usa el inicio de sesión del proveedor ya instalado en esta Mac. Cuenta de Blackholes guarda un perfil independiente en Application Support; las credenciales nunca se copian a la base de datos.",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_4()
                            .child(
                                div()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8e97aa))
                                    .child(projects_label.to_uppercase()),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_5()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(folder_label),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(0x8e97aa))
                                                    .child(folder_description),
                                            )
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(0xaab2c0))
                                                    .child(projects_root.display().to_string()),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .gap_2()
                                            .child(compact_button(
                                                "settings-reveal-projects-folder",
                                                reveal_label,
                                                move |_, _, cx| {
                                                    let _ = weak_reveal.update(cx, |app, cx| {
                                                        app.reveal_projects_root(cx)
                                                    });
                                                },
                                            ))
                                            .child(compact_button(
                                                "settings-change-projects-folder",
                                                change_label,
                                                move |_, _, cx| {
                                                    if let Some(path) =
                                                        rfd::FileDialog::new().pick_folder()
                                                    {
                                                        let _ =
                                                            weak_change.update(cx, |app, cx| {
                                                                app.set_projects_root(path, cx)
                                                            });
                                                    }
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                h_flex()
                                    .w_full()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(11.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(0x8e97aa))
                                            .child(claude_label.to_uppercase()),
                                    )
                                    .child(compact_button(
                                        "settings-open-claude-usage",
                                        claude_usage_link,
                                        move |_, _, cx| {
                                            cx.open_url("https://claude.ai/settings/usage");
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(claude_description),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .flex_wrap()
                                    .children([
                                        settings_claude_usage_card(
                                            self.tr("Plan", "Plan"),
                                            plan_name,
                                            plan_detail,
                                            None,
                                        ),
                                        settings_claude_usage_card(
                                            self.tr("5-hour limit", "Límite de 5 horas"),
                                            five_hour_value,
                                            five_hour_detail,
                                            five_hour_used,
                                        ),
                                        settings_claude_usage_card(
                                            self.tr("Weekly limit", "Límite semanal"),
                                            seven_day_value,
                                            seven_day_detail,
                                            seven_day_used,
                                        ),
                                        settings_claude_usage_card(
                                            self.tr("Estimated API cost", "Costo API estimado"),
                                            cost_value,
                                            cost_detail,
                                            None,
                                        ),
                                    ]),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .px_1()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(token_detail)
                                    .child(updated_detail)
                                    .child(self.tr(
                                        "Plan percentages come from an experimental Anthropic SDK endpoint; cost is an estimate, not an invoice.",
                                        "Los porcentajes del plan vienen de una función experimental del SDK de Anthropic; el costo es una estimación, no una factura.",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                h_flex()
                                    .w_full()
                                    .pb_3()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(11.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(0x8e97aa))
                                            .child(format!(
                                                "{} · {skill_count}",
                                                skills_label.to_uppercase()
                                            )),
                                    )
                                    .child(compact_button(
                                        "settings-reveal-agent-skills",
                                        reveal_skills_label,
                                        move |_, _, cx| {
                                            let _ = weak_reveal_skills.update(cx, |app, cx| {
                                                app.reveal_agent_skills(cx)
                                            });
                                        },
                                    ))
                                    .child(compact_button(
                                        "settings-import-agent-skills",
                                        import_skills_label,
                                        move |_, _, cx| {
                                            if let Some(path) = rfd::FileDialog::new()
                                                .set_title(&import_dialog_title)
                                                .pick_folder()
                                            {
                                                let _ = weak_import_skills.update(cx, |app, cx| {
                                                    app.import_agent_skills(path, cx)
                                                });
                                            }
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(skills_description),
                            )
                            .child(v_flex().w_full().gap_2().children(skill_rows))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(self.tr(
                                        "Changes apply to upcoming responses. Imported folders are copied into Blackholes, so the originals are never modified.",
                                        "Los cambios se aplican a las próximas respuestas. Las carpetas importadas se copian dentro de Blackholes, por lo que los originales nunca se modifican.",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8e97aa))
                                    .child(model_label.to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(model_description),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .children(model_buttons),
                            )
                            .children(effort_controls)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(self.tr(
                                        "Automatic follows the selected provider's default. OpenCode model selection is managed by its own provider configuration.",
                                        "Automático usa el modelo predeterminado del proveedor seleccionado. La selección de modelo de OpenCode se administra desde su propia configuración de proveedores.",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8e97aa))
                                    .child(permissions_label.to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(permissions_description),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(choice_button(
                                        "settings-agents-full-access",
                                        self.tr("Full access", "Acceso total"),
                                        full_access,
                                        move |_, _, cx| {
                                            let _ = weak_full_access.update(cx, |app, cx| {
                                                app.set_agents_full_access(true, cx)
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        "settings-agents-standard-access",
                                        self.tr("Standard", "Estándar"),
                                        !full_access,
                                        move |_, _, cx| {
                                            let _ = weak_standard_access.update(cx, |app, cx| {
                                                app.set_agents_full_access(false, cx)
                                            });
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0xd0a16d))
                                    .child(self.tr(
                                        "Full access is equivalent to --dangerously-skip-permissions. Remote actions still require an explicit instruction in the conversation.",
                                        "Acceso total equivale a --dangerously-skip-permissions. Las acciones remotas aún requieren una instrucción explícita en la conversación.",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .pb_3()
                                    .border_b_1()
                                    .border_color(rgb(0x252a33))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8e97aa))
                                    .child(language_label.to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x8e97aa))
                                    .child(language_description),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(choice_button(
                                        "settings-language-english",
                                        "English",
                                        self.session.language == Language::English,
                                        move |_, _, cx| {
                                            let _ = weak_english.update(cx, |app, cx| {
                                                app.set_language(Language::English, cx)
                                            });
                                        },
                                    ))
                                    .child(choice_button(
                                        "settings-language-spanish",
                                        "Español",
                                        self.session.language == Language::Spanish,
                                        move |_, _, cx| {
                                            let _ = weak_spanish.update(cx, |app, cx| {
                                                app.set_language(Language::Spanish, cx)
                                            });
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_file_explorer(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let weak_refresh = weak.clone();
        let weak_close = weak.clone();
        let weak_files = weak.clone();
        let weak_changes = weak.clone();
        let mode = self.file_explorer.mode;
        let rows = if mode == FileExplorerMode::Files {
            Rc::new(self.file_tree_rows())
        } else {
            Rc::new(Vec::new())
        };
        let row_count = rows.len();
        let selected_path = self.file_explorer.selected.clone();
        let root_label = self.file_explorer.root_label.clone();
        let root_path = self
            .file_explorer
            .root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let change_count = match &self.file_explorer.changes {
            RepositoryChangesState::Ready(changes) => changes.len(),
            _ => 0,
        };
        let changes_label = if change_count == 0 {
            self.tr("Changes", "Cambios").to_string()
        } else {
            format!("{} {change_count}", self.tr("Changes", "Cambios"))
        };
        let open_hint = if mode == FileExplorerMode::Files {
            self.tr(
                "Click a file to edit it",
                "Haz clic en un archivo para editarlo",
            )
        } else {
            self.tr(
                "Click a changed file to compare it",
                "Haz clic en un archivo modificado para compararlo",
            )
        }
        .to_string();

        v_flex()
            .size_full()
            .min_w_0()
            .bg(rgb(0x111318))
            .border_r_1()
            .border_color(rgb(0x252a33))
            .child(
                h_flex()
                    .h(px(40.))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(0x252a33))
                    .text_color(rgb(0xb6bdca))
                    .child(Icon::new(AppIcon::FolderOpen).with_size(px(15.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_ellipsis()
                            .child(self.tr("EXPLORER", "EXPLORADOR")),
                    )
                    .child(sidebar_icon_button(
                        "refresh-file-explorer",
                        AppIcon::RefreshCw,
                        move |_, _, cx| {
                            let _ =
                                weak_refresh.update(cx, |app, cx| app.refresh_file_explorer(cx));
                        },
                    ))
                    .child(sidebar_icon_button(
                        "close-file-explorer",
                        AppIcon::X,
                        move |_, _, cx| {
                            let _ = weak_close.update(cx, |app, cx| app.close_file_explorer(cx));
                        },
                    )),
            )
            .child(
                h_flex()
                    .h(px(38.))
                    .flex_none()
                    .mx_2()
                    .gap_1()
                    .items_center()
                    .child(explorer_mode_button(
                        "file-explorer-mode-files",
                        self.tr("Files", "Archivos"),
                        mode == FileExplorerMode::Files,
                        move |_, _, cx| {
                            let _ = weak_files.update(cx, |app, cx| {
                                app.set_file_explorer_mode(FileExplorerMode::Files, cx)
                            });
                        },
                    ))
                    .child(explorer_mode_button(
                        "file-explorer-mode-changes",
                        changes_label,
                        mode == FileExplorerMode::Changes,
                        move |_, _, cx| {
                            let _ = weak_changes.update(cx, |app, cx| {
                                app.set_file_explorer_mode(FileExplorerMode::Changes, cx)
                            });
                        },
                    )),
            )
            .child(
                v_flex()
                    .flex_none()
                    .min_w_0()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(0x252a33))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_ellipsis()
                            .child(root_label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(10.))
                            .text_color(rgb(0x788294))
                            .text_ellipsis()
                            .child(root_path),
                    ),
            )
            .when(mode == FileExplorerMode::Files, |this| {
                this.child(
                    uniform_list(
                        "file-explorer-rows",
                        row_count,
                        move |range, _window, _cx| {
                            let mut elements = Vec::with_capacity(range.len());
                            for index in range {
                                let Some(row) = rows.get(index).cloned() else {
                                    continue;
                                };
                                let row_id = SharedString::from(format!(
                                    "file-explorer-row-{}",
                                    row.path.to_string_lossy()
                                ));
                                let selected = selected_path.as_ref() == Some(&row.path);
                                let indentation = 8. + row.depth as f32 * 16.;
                                let base = h_flex()
                                    .id(row_id)
                                    .w_full()
                                    .h(px(27.))
                                    .min_w_0()
                                    .pl(px(indentation))
                                    .pr_2()
                                    .gap_1()
                                    .text_size(px(12.))
                                    .bg(if selected {
                                        rgb(0x29364f)
                                    } else {
                                        rgb(0x111318)
                                    });

                                match row.kind {
                                    FileTreeRowKind::Entry(kind) => {
                                        let path = row.path.clone();
                                        let weak_row = weak.clone();
                                        let (icon, icon_color) =
                                            file_tree_icon(kind, &row.path, row.expanded);
                                        elements.push(
                                            base.cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x242a35)))
                                                .on_click(move |event, _, cx| {
                                                    let click_count = event.click_count();
                                                    let path = path.clone();
                                                    let _ = weak_row.update(cx, |app, cx| {
                                                        app.activate_file_tree_row(
                                                            path,
                                                            kind,
                                                            click_count,
                                                            cx,
                                                        )
                                                    });
                                                })
                                                .child(
                                                    div()
                                                        .w(px(15.))
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_color(rgb(0x8993a5))
                                                        .when(kind.is_directory(), |this| {
                                                            this.child(
                                                                Icon::new(if row.expanded {
                                                                    AppIcon::ChevronDown
                                                                } else {
                                                                    AppIcon::ChevronRight
                                                                })
                                                                .with_size(px(12.)),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .size(px(17.))
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_color(icon_color)
                                                        .child(Icon::new(icon).with_size(px(14.))),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .text_color(if row.hidden {
                                                            rgb(0x7d8798)
                                                        } else if selected {
                                                            rgb(0xe5e9f0)
                                                        } else {
                                                            rgb(0xb6bdca)
                                                        })
                                                        .child(row.label),
                                                ),
                                        );
                                    }
                                    FileTreeRowKind::Loading => elements.push(
                                        base.text_color(rgb(0x788294))
                                            .child(div().w(px(15.)).flex_none())
                                            .child(Icon::new(AppIcon::RefreshCw).with_size(px(13.)))
                                            .child(row.label),
                                    ),
                                    FileTreeRowKind::Error => elements.push(
                                        base.text_color(rgb(0xff7b72))
                                            .child(div().w(px(15.)).flex_none())
                                            .child(Icon::new(AppIcon::X).with_size(px(13.)))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_ellipsis()
                                                    .child(row.label),
                                            ),
                                    ),
                                }
                            }
                            elements
                        },
                    )
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .with_sizing_behavior(ListSizingBehavior::Auto),
                )
            })
            .when(mode == FileExplorerMode::Changes, |this| {
                this.child(self.render_repository_changes_list(cx))
            })
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(rgb(0x252a33))
                    .text_size(px(10.))
                    .text_color(rgb(0x788294))
                    .child(open_hint),
            )
            .into_any_element()
    }

    fn render_repository_changes_list(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.file_explorer.changes {
            RepositoryChangesState::Idle | RepositoryChangesState::Loading => v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::RefreshCw).with_size(px(16.)))
                .child(self.tr("Loading changes…", "Cargando cambios…"))
                .into_any_element(),
            RepositoryChangesState::Error(error) => v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .px_4()
                .text_size(px(12.))
                .text_color(rgb(0xff7b72))
                .child(self.tr(
                    "Git changes could not be loaded",
                    "No se pudieron cargar los cambios Git",
                ))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(0x8e97aa))
                        .child(error.clone()),
                )
                .into_any_element(),
            RepositoryChangesState::Ready(changes) if changes.is_empty() => v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::GitBranch).with_size(px(17.)))
                .child(self.tr("No local changes", "No hay cambios locales"))
                .into_any_element(),
            RepositoryChangesState::Ready(changes) => {
                let weak = cx.weak_entity();
                let changes = changes.clone();
                let change_count = changes.len();
                let selected_path = self.file_explorer.selected.clone();
                uniform_list(
                    "repository-change-rows",
                    change_count,
                    move |range, _window, _cx| {
                        let mut elements = Vec::with_capacity(range.len());
                        for index in range {
                            let Some(change) = changes.get(index).cloned() else {
                                continue;
                            };
                            let selected = selected_path.as_ref() == Some(&change.path);
                            let (status, status_color) = repository_change_style(change.kind);
                            let (icon, icon_color) =
                                file_tree_icon(FileEntryKind::File, &change.path, false);
                            let weak_change = weak.clone();
                            let change_for_click = change.clone();
                            let previous_path = change.previous_relative_path.clone();
                            elements.push(
                                h_flex()
                                    .id(SharedString::from(format!(
                                        "repository-change-{}",
                                        change.relative_path
                                    )))
                                    .w_full()
                                    .h(px(38.))
                                    .min_w_0()
                                    .px_2()
                                    .gap_2()
                                    .bg(if selected {
                                        rgb(0x29364f)
                                    } else {
                                        rgb(0x111318)
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x242a35)))
                                    .on_click(move |_, _, cx| {
                                        let change = change_for_click.clone();
                                        let _ = weak_change.update(cx, |app, cx| {
                                            app.open_repository_diff(change, cx)
                                        });
                                    })
                                    .child(
                                        div()
                                            .w(px(16.))
                                            .flex_none()
                                            .text_size(px(10.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(status_color)
                                            .child(status),
                                    )
                                    .child(
                                        div()
                                            .size(px(17.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(icon_color)
                                            .child(Icon::new(icon).with_size(px(14.))),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .w_full()
                                                    .truncate()
                                                    .text_size(px(11.))
                                                    .text_color(if selected {
                                                        rgb(0xe5e9f0)
                                                    } else {
                                                        rgb(0xb6bdca)
                                                    })
                                                    .child(change.relative_path),
                                            )
                                            .when_some(previous_path, |this, previous| {
                                                this.child(
                                                    div()
                                                        .w_full()
                                                        .truncate()
                                                        .text_size(px(9.))
                                                        .text_color(rgb(0x788294))
                                                        .child(format!("← {previous}")),
                                                )
                                            }),
                                    ),
                            );
                        }
                        elements
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full()
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .into_any_element()
            }
        }
    }

    fn render_file_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(document) = self.active_file.as_ref() else {
            return self.render_empty_state(cx);
        };
        let weak = cx.weak_entity();
        let weak_save = weak.clone();
        let weak_project_instructions = weak.clone();
        let weak_task_instructions = weak.clone();
        let weak_close = weak;
        let file_name = match document.source {
            FileDocumentSource::ProjectTaskInstructions(_) => self
                .tr("CLAUDE.md for tasks", "CLAUDE.md de tareas")
                .to_string(),
            _ => document
                .path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("file")
                .to_string(),
        };
        let relative_path = match document.source {
            FileDocumentSource::ProjectInstructions(_) => self
                .tr(
                    "General project instructions · AGENTS.md links to this file",
                    "Instrucciones generales del proyecto · AGENTS.md enlaza este archivo",
                )
                .to_string(),
            FileDocumentSource::ProjectTaskInstructions(_) => self
                .tr(
                    "Shared body copied after each task's generated header",
                    "Cuerpo compartido copiado después del encabezado generado de cada tarea",
                )
                .to_string(),
            FileDocumentSource::Repository => document
                .path
                .strip_prefix(&document.root)
                .unwrap_or(&document.path)
                .to_string_lossy()
                .into_owned(),
        };
        let save_label = self.tr("Save", "Guardar");
        let save_status_label = note_save_label(document.save_state, self.session.language);
        let project_settings_tabs = match document.source {
            FileDocumentSource::ProjectInstructions(workspace_id)
            | FileDocumentSource::ProjectTaskInstructions(workspace_id) => {
                let project_selected =
                    matches!(document.source, FileDocumentSource::ProjectInstructions(_));
                Some(
                    h_flex()
                        .w_full()
                        .h(px(42.))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .border_b_1()
                        .border_color(rgb(0x252a33))
                        .bg(rgb(0x0f1217))
                        .child(
                            h_flex()
                                .w(px(430.))
                                .gap_1()
                                .child(explorer_mode_button(
                                    format!("project-settings-general-{workspace_id}"),
                                    self.tr("Project CLAUDE.md", "CLAUDE.md del proyecto"),
                                    project_selected,
                                    move |_, _, cx| {
                                        let _ = weak_project_instructions.update(cx, |app, cx| {
                                            app.open_project_instructions(workspace_id, cx)
                                        });
                                    },
                                ))
                                .child(explorer_mode_button(
                                    format!("project-settings-tasks-{workspace_id}"),
                                    self.tr("Task CLAUDE.md", "CLAUDE.md de tareas"),
                                    !project_selected,
                                    move |_, _, cx| {
                                        let _ = weak_task_instructions.update(cx, |app, cx| {
                                            app.open_project_task_instructions(workspace_id, cx)
                                        });
                                    },
                                )),
                        )
                        .into_any_element(),
                )
            }
            FileDocumentSource::Repository => None,
        };

        let content = match (&document.load_state, document.editor.as_ref()) {
            (FileDocumentLoadState::Loading, _) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::RefreshCw).with_size(px(18.)))
                .child(self.tr("Opening file…", "Abriendo archivo…"))
                .into_any_element(),
            (FileDocumentLoadState::Error(error), _) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .px_8()
                .text_color(rgb(0xff7b72))
                .child(self.tr(
                    "This file cannot be edited here",
                    "Este archivo no se puede editar aquí",
                ))
                .child(
                    div()
                        .max_w(px(620.))
                        .text_size(px(12.))
                        .text_color(rgb(0x9ba3b4))
                        .child(error.clone()),
                )
                .into_any_element(),
            (_, Some(editor)) => div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .bg(rgb(0x0c0e12))
                .font_family(CODE_FONT_FAMILY)
                .text_size(px(CODE_FONT_SIZE))
                .child(
                    Input::new(editor)
                        .w_full()
                        .h_full()
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false),
                )
                .into_any_element(),
            _ => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(0x8e97aa))
                .child(self.tr("Preparing editor…", "Preparando editor…"))
                .into_any_element(),
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .children(project_settings_tabs)
            .child(
                h_flex()
                    .w_full()
                    .h(px(46.))
                    .flex_none()
                    .items_center()
                    .px_3()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(0x252a33))
                    .bg(rgb(0x111318))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(0x8db3cf))
                            .child(Icon::new(AppIcon::File).with_size(px(15.))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .pr_2()
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .child(file_name),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x788294))
                                    .text_ellipsis()
                                    .child(relative_path),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(note_save_status(document.save_state, save_status_label))
                            .child(compact_button(
                                format!("save-file-{}", document.request_id),
                                save_label,
                                move |_, _, cx| {
                                    let _ =
                                        weak_save.update(cx, |app, cx| app.flush_active_file(cx));
                                },
                            ))
                            .child(sidebar_icon_button(
                                format!("close-file-{}", document.request_id),
                                AppIcon::X,
                                move |_, _, cx| {
                                    let _ =
                                        weak_close.update(cx, |app, cx| app.close_file_editor(cx));
                                },
                            )),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    fn render_repository_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(document) = self.active_diff.as_ref() else {
            return self.render_empty_state(cx);
        };
        let weak_close = cx.weak_entity();
        let file_name = document
            .change
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("file")
            .to_string();
        let relative_path = document.change.relative_path.clone();
        let (status, status_color) = repository_change_style(document.change.kind);
        let status_label = match (document.change.kind, self.session.language) {
            (RepositoryChangeKind::Added, Language::English) => "Added",
            (RepositoryChangeKind::Added, Language::Spanish) => "Agregado",
            (RepositoryChangeKind::Deleted, Language::English) => "Deleted",
            (RepositoryChangeKind::Deleted, Language::Spanish) => "Eliminado",
            (RepositoryChangeKind::Modified, Language::English) => "Modified",
            (RepositoryChangeKind::Modified, Language::Spanish) => "Modificado",
            (RepositoryChangeKind::Renamed, Language::English) => "Renamed",
            (RepositoryChangeKind::Renamed, Language::Spanish) => "Renombrado",
            (RepositoryChangeKind::Untracked, Language::English) => "Untracked",
            (RepositoryChangeKind::Untracked, Language::Spanish) => "Nuevo",
            (RepositoryChangeKind::Conflicted, Language::English) => "Conflict",
            (RepositoryChangeKind::Conflicted, Language::Spanish) => "Conflicto",
        };

        let content = match &document.load_state {
            FileDiffLoadState::Loading => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::RefreshCw).with_size(px(18.)))
                .child(self.tr("Loading comparison…", "Cargando comparación…"))
                .into_any_element(),
            FileDiffLoadState::Error(error) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .px_8()
                .text_color(rgb(0xff7b72))
                .child(self.tr(
                    "This comparison cannot be displayed",
                    "Esta comparación no se puede mostrar",
                ))
                .child(
                    div()
                        .max_w(px(620.))
                        .text_size(px(12.))
                        .text_color(rgb(0x9ba3b4))
                        .child(error.clone()),
                )
                .into_any_element(),
            FileDiffLoadState::Ready(diff) if diff.binary => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::File).with_size(px(20.)))
                .child(self.tr(
                    "Binary files cannot be compared here",
                    "Los archivos binarios no se pueden comparar aquí",
                ))
                .into_any_element(),
            FileDiffLoadState::Ready(diff) if diff.rows.is_empty() => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(rgb(0x8e97aa))
                .child(Icon::new(AppIcon::GitBranch).with_size(px(20.)))
                .child(self.tr(
                    "No textual changes to display",
                    "No hay cambios de texto para mostrar",
                ))
                .into_any_element(),
            FileDiffLoadState::Ready(diff) => {
                let rows = diff.rows.clone();
                let row_count = rows.len();
                let truncated = diff.truncated;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .bg(rgb(0x20262e))
                    .child(
                        uniform_list(
                            SharedString::from(format!(
                                "repository-diff-{}",
                                document.change.relative_path
                            )),
                            row_count,
                            move |range, _window, _cx| {
                                range
                                    .filter_map(|index| rows.get(index).cloned())
                                    .map(render_repository_diff_row)
                                    .collect::<Vec<_>>()
                            },
                        )
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .with_sizing_behavior(ListSizingBehavior::Auto),
                    )
                    .when(truncated, |this| {
                        this.child(
                            div()
                                .h(px(28.))
                                .flex_none()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(rgb(0x3d4654))
                                .bg(rgb(0x2b313b))
                                .text_size(px(10.))
                                .text_color(rgb(0xd1b46f))
                                .child(self.tr(
                                    "Large diff truncated at 20,000 rows",
                                    "Diff grande truncado a 20 000 filas",
                                )),
                        )
                    })
                    .into_any_element()
            }
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(
                h_flex()
                    .w_full()
                    .h(px(46.))
                    .flex_none()
                    .items_center()
                    .px_3()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(0x252a33))
                    .bg(rgb(0x111318))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(0x8db3cf))
                            .child(Icon::new(AppIcon::Code2).with_size(px(15.))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .pr_2()
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(file_name),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(10.))
                                    .text_color(rgb(0x788294))
                                    .truncate()
                                    .child(relative_path),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .bg(with_alpha(status_color, 0.14))
                                    .text_size(px(10.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(status_color)
                                    .child(status)
                                    .child(status_label),
                            )
                            .child(sidebar_icon_button(
                                format!("close-diff-{}", document.request_id),
                                AppIcon::X,
                                move |_, _, cx| {
                                    let _ = weak_close
                                        .update(cx, |app, cx| app.close_repository_diff(cx));
                                },
                            )),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.))
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(0x343b47))
                    .bg(rgb(0x181d24))
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0x8e97aa))
                    .child(div().flex_1().min_w_0().px_3().child("HEAD"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .border_l_1()
                            .border_color(rgb(0x343b47))
                            .child(self.tr("WORKING TREE", "CAMBIOS LOCALES")),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let background = rgb(0x111318);
        let border = rgb(0x252a33);
        let muted = rgb(0x8e97aa);
        let projects_label = self.tr("Projects", "Proyectos");
        let active_terminal_id = if self.show_terminal {
            self.selected_terminal_id()
        } else {
            None
        };
        let sidebar_width = self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let projects_width = (sidebar_width - 16.).max(0.);
        let add_bot_label = self.tr("Add bot", "Agregar bot").to_string();
        let terminal_label = self.tr("Terminal", "Terminal").to_string();
        let launch_tooltip = self.tr("Add", "Agregar").to_string();
        let mut projects = v_flex().gap_1().w(px(projects_width)).min_w_0();
        for workspace in &self.workspaces {
            let workspace_id = workspace.id;
            let workspace_expanded = self.session.expanded_workspace_ids.contains(&workspace_id);
            let selected_project = self.session.selected_workspace_id == Some(workspace_id);
            let weak_workspace_toggle = weak.clone();
            let weak_project = weak.clone();
            let weak_new_terminal = weak.clone();
            let weak_add_task = weak.clone();
            let weak_assign_project_agent = weak.clone();
            let weak_refresh_project = weak.clone();
            let weak_edit_project = weak.clone();
            let weak_project_instructions = weak.clone();
            let weak_remove_project = weak.clone();
            let new_terminal_label = self.tr("New terminal", "Nueva terminal").to_string();
            let add_task_label = self.tr("Add task", "Agregar tarea").to_string();
            let add_project_bot_label = add_bot_label.clone();
            let project_scope = OrchestratorChatScope::Project(workspace_id);
            let project_agent_assigned = self.orchestrator_chats.has_agent(project_scope);
            let edit_project_label = self.tr("Edit project", "Editar proyecto").to_string();
            let project_settings_label = self
                .tr("Project settings", "Configuración del proyecto")
                .to_string();
            let remove_project_label = self.tr("Remove project", "Eliminar proyecto").to_string();
            let add_project_item_label =
                self.tr("Add to project", "Agregar al proyecto").to_string();
            let refresh_project_label = self
                .tr("Find new repositories", "Buscar repositorios nuevos")
                .to_string();
            let project_menu_label = self
                .tr("Project options", "Opciones del proyecto")
                .to_string();
            let project_add_menu = Button::new(SharedString::from(format!(
                "project-add-menu-{workspace_id}"
            )))
            .icon(AppIcon::Plus)
            .ghost()
            .xsmall()
            .tooltip(add_project_item_label)
            .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                let weak_terminal = weak_new_terminal.clone();
                let weak_task = weak_add_task.clone();
                let weak_agent = weak_assign_project_agent.clone();
                menu.min_w(px(190.))
                    .item(
                        PopupMenuItem::new(add_project_bot_label.clone())
                            .icon(AppIcon::Plus)
                            .on_click(move |_, window, cx| {
                                let weak = weak_agent.clone();
                                window.defer(cx, move |_, cx| {
                                    let _ = weak.update(cx, |app, cx| {
                                        app.create_scoped_orchestrator_agent(workspace_id, None, cx)
                                    });
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new(new_terminal_label.clone())
                            .icon(AppIcon::SquareTerminal)
                            .on_click(move |_, window, cx| {
                                let weak = weak_terminal.clone();
                                window.defer(cx, move |window, cx| {
                                    let _ = weak.update(cx, |app, cx| {
                                        app.select_target(workspace_id, None, None, cx);
                                        app.new_terminal(AgentKind::Shell, window, cx);
                                    });
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new(add_task_label.clone())
                            .icon(AppIcon::ListTodo)
                            .on_click(move |_, window, cx| {
                                let weak = weak_task.clone();
                                window.defer(cx, move |window, cx| {
                                    let _ = weak.update(cx, |app, cx| {
                                        app.select_target(workspace_id, None, None, cx);
                                        app.open_create_task(window, cx);
                                    });
                                });
                            }),
                    )
            });
            let project_menu =
                Button::new(SharedString::from(format!("project-menu-{workspace_id}")))
                    .icon(AppIcon::EllipsisVertical)
                    .ghost()
                    .xsmall()
                    .tooltip(project_menu_label)
                    .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                        let weak_edit = weak_edit_project.clone();
                        let weak_instructions = weak_project_instructions.clone();
                        let weak_remove = weak_remove_project.clone();
                        menu.min_w(px(210.))
                            .item(
                                PopupMenuItem::new(edit_project_label.clone())
                                    .icon(AppIcon::Pencil)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_edit.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.open_edit_project(workspace_id, window, cx);
                                            });
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(project_settings_label.clone())
                                    .icon(AppIcon::Settings)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_instructions.clone();
                                        window.defer(cx, move |_, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.open_project_instructions(workspace_id, cx);
                                            });
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(remove_project_label.clone())
                                    .icon(AppIcon::X)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_remove.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.open_remove_project_confirmation(
                                                    workspace_id,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        });
                                    }),
                            )
                    });
            let refresh_project = Button::new(SharedString::from(format!(
                "refresh-project-repositories-{workspace_id}"
            )))
            .icon(AppIcon::RefreshCw)
            .ghost()
            .xsmall()
            .tooltip(refresh_project_label)
            .on_click(move |_, _, cx| {
                let _ = weak_refresh_project.update(cx, |app, cx| {
                    app.refresh_project_repositories(workspace_id, cx)
                });
            });
            let project_actions = h_flex()
                .gap_0()
                .child(refresh_project)
                .child(project_add_menu)
                .child(project_menu)
                .into_any_element();
            projects = projects.child(collapsible_tree_row(
                format!("toggle-workspace-{workspace_id}"),
                format!("workspace-{workspace_id}"),
                Icon::new(project_icon_kind(&workspace.icon))
                    .small()
                    .into_any_element(),
                workspace.label().to_string(),
                None,
                selected_project,
                workspace_expanded,
                0.,
                Some(workspace_color(workspace.color)),
                Some(project_actions),
                move |_, _, cx| {
                    let _ = weak_workspace_toggle.update(cx, |app, cx| {
                        app.select_target(workspace_id, None, None, cx);
                        app.toggle_workspace_expanded(workspace_id, cx);
                    });
                },
                move |_, _, cx| {
                    let _ = weak_project.update(cx, |app, cx| {
                        app.select_target(workspace_id, None, None, cx);
                        app.toggle_workspace_expanded(workspace_id, cx);
                    });
                },
            ));

            if !workspace_expanded {
                continue;
            }

            if project_agent_assigned {
                let project_agent_selected = self.orchestrator_surface_visible()
                    && self.active_orchestrator_scope == project_scope;
                let project_agent_busy = self.orchestrator_turns.contains_key(&project_scope);
                let weak_project_agent = weak.clone();
                let weak_remove_project_agent = weak.clone();
                projects = projects.child(
                    div().w_full().min_w_0().pl_6().child(agent_chat_tree_row(
                        format!("project-agent-{workspace_id}"),
                        self.orchestrator_chats
                            .avatar_color(project_scope)
                            .display_name()
                            .into(),
                        project_agent_selected,
                        project_agent_busy,
                        self.orchestrator_chats.avatar_color(project_scope),
                        move |_, _, cx| {
                            let _ = weak_project_agent.update(cx, |app, cx| {
                                app.show_orchestrator_chat(project_scope, cx)
                            });
                        },
                        move |_, _, cx| {
                            let _ = weak_remove_project_agent.update(cx, |app, cx| {
                                app.remove_orchestrator_agent(project_scope, cx)
                            });
                        },
                    )),
                );
            }
            for agent_id in self
                .orchestrator_chats
                .project_agent_ids(workspace_id)
                .iter()
                .copied()
            {
                let scope = OrchestratorChatScope::ProjectAgent {
                    project_id: workspace_id,
                    agent_id,
                };
                let selected =
                    self.orchestrator_surface_visible() && self.active_orchestrator_scope == scope;
                let busy = self.orchestrator_turns.contains_key(&scope);
                let weak_select = weak.clone();
                let weak_remove = weak.clone();
                projects = projects.child(
                    div().w_full().min_w_0().pl_6().child(agent_chat_tree_row(
                        format!("project-agent-{workspace_id}-{agent_id}"),
                        self.orchestrator_chats
                            .avatar_color(scope)
                            .display_name()
                            .into(),
                        selected,
                        busy,
                        self.orchestrator_chats.avatar_color(scope),
                        move |_, _, cx| {
                            let _ = weak_select
                                .update(cx, |app, cx| app.show_orchestrator_chat(scope, cx));
                        },
                        move |_, _, cx| {
                            let _ = weak_remove
                                .update(cx, |app, cx| app.remove_orchestrator_agent(scope, cx));
                        },
                    )),
                );
            }

            let project_notes_selected = self.show_project_note
                && self.session.selected_workspace_id == Some(workspace_id)
                && self.session.selected_task_id.is_none()
                && self.session.selected_repository_id.is_none();
            let weak_project_notes = weak.clone();
            projects = projects.child(div().w_full().min_w_0().pl_6().child(tree_row_button(
                format!("project-notes-{workspace_id}"),
                Icon::new(AppIcon::Pencil).small().into_any_element(),
                self.tr("Notes", "Notas").to_string(),
                project_notes_selected,
                move |_, _, cx| {
                    let _ = weak_project_notes
                        .update(cx, |app, cx| app.show_project_notes(workspace_id, cx));
                },
            )));

            for terminal in self.session.terminals.iter().filter(|terminal| {
                terminal.workspace_id == workspace_id
                    && terminal.task_id.is_none()
                    && terminal.repository_id.is_none()
            }) {
                let terminal_id = terminal.id;
                let weak_terminal = weak.clone();
                let weak_close_terminal = weak.clone();
                projects =
                    projects.child(div().w_full().min_w_0().pl_6().child(terminal_tree_row(
                        terminal_id,
                        terminal.label.clone(),
                        terminal.agent,
                        terminal.state,
                        active_terminal_id == Some(terminal_id),
                        move |_, window, cx| {
                            let _ = weak_terminal
                                .update(cx, |app, cx| app.focus_terminal(terminal_id, window, cx));
                        },
                        move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = weak_close_terminal
                                .update(cx, |app, cx| app.close_terminal(terminal_id, cx));
                        },
                    )));
            }

            for repository in &workspace.repositories {
                let repository_id = repository.id;
                let selected = self.session.selected_workspace_id == Some(workspace_id)
                    && self.session.selected_task_id.is_none()
                    && self.session.selected_repository_id == Some(repository_id);
                let (branch, additions, deletions, git_loading) =
                    self.repository_git_details(&repository.path, repository.branch.as_deref());
                let weak_repository = weak.clone();
                projects =
                    projects.child(div().w_full().min_w_0().pl_6().child(repository_tree_row(
                        format!("repository-{repository_id}"),
                        repository.name.clone(),
                        branch,
                        additions,
                        deletions,
                        git_loading,
                        selected,
                        Some(agent_launch_menu_button(
                            format!("repository-add-terminal-{repository_id}"),
                            launch_tooltip.clone(),
                            weak.clone(),
                            workspace_id,
                            None,
                            Some(repository_id),
                            add_bot_label.clone(),
                            terminal_label.clone(),
                        )),
                        move |_, _, cx| {
                            let _ = weak_repository.update(cx, |app, cx| {
                                app.select_repository_target(workspace_id, None, repository_id, cx)
                            });
                        },
                    )));
                for terminal in self.session.terminals.iter().filter(|terminal| {
                    terminal.workspace_id == workspace_id
                        && terminal.task_id.is_none()
                        && terminal.repository_id == Some(repository_id)
                }) {
                    let terminal_id = terminal.id;
                    let weak_terminal = weak.clone();
                    let weak_close_terminal = weak.clone();
                    projects = projects.child(div().w_full().min_w_0().pl(px(40.)).child(
                        terminal_tree_row(
                            terminal_id,
                            terminal.label.clone(),
                            terminal.agent,
                            terminal.state,
                            active_terminal_id == Some(terminal_id),
                            move |_, window, cx| {
                                let _ = weak_terminal.update(cx, |app, cx| {
                                    app.focus_terminal(terminal_id, window, cx)
                                });
                            },
                            move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = weak_close_terminal
                                    .update(cx, |app, cx| app.close_terminal(terminal_id, cx));
                            },
                        ),
                    ));
                }
            }

            if self
                .tasks
                .iter()
                .any(|task| task.workspace_id == workspace_id)
            {
                projects = projects.child(
                    div()
                        .h(px(24.))
                        .flex()
                        .items_end()
                        .pl(px(24.))
                        .pb_1()
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(0x737d90))
                        .child(self.tr("TASKS", "TAREAS")),
                );
            }

            for task in self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace_id)
            {
                let task_id = task.id;
                let selected = self.session.selected_task_id == Some(task_id);
                let task_expanded = self.session.expanded_task_ids.contains(&task_id);
                let weak_task_toggle = weak.clone();
                let weak_task_row_toggle = weak.clone();
                let weak_edit_task = weak.clone();
                let weak_remove_task = weak.clone();
                let weak_assign_task_agent = weak.clone();
                let task_to_edit = task.clone();
                let task_scope = OrchestratorChatScope::Task(task_id);
                let task_agent_assigned = self.orchestrator_chats.has_agent(task_scope);
                let edit_task_label = self.tr("Edit task", "Editar tarea").to_string();
                let remove_task_label = self.tr("Delete task", "Eliminar tarea").to_string();
                let assign_task_agent_label =
                    self.tr("Assign Black Bot", "Asignar Black Bot").to_string();
                let task_menu_label = self.tr("Task options", "Opciones de la tarea").to_string();
                let new_task_badge = self
                    .session
                    .unseen_task_ids
                    .contains(&task_id)
                    .then(|| new_task_chip(self.tr("New", "Nuevo")));
                let task_menu = Button::new(SharedString::from(format!("task-menu-{task_id}")))
                    .icon(AppIcon::EllipsisVertical)
                    .ghost()
                    .xsmall()
                    .tooltip(task_menu_label)
                    .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                        let weak_edit = weak_edit_task.clone();
                        let weak_remove = weak_remove_task.clone();
                        let weak_agent = weak_assign_task_agent.clone();
                        let task = task_to_edit.clone();
                        let mut menu = menu
                            .min_w(px(190.))
                            .item(
                                PopupMenuItem::new(edit_task_label.clone())
                                    .icon(AppIcon::Pencil)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_edit.clone();
                                        let task = task.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.open_manage_task(task, window, cx);
                                            });
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(remove_task_label.clone())
                                    .icon(AppIcon::X)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_remove.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.open_remove_task_confirmation(
                                                    task_id, window, cx,
                                                );
                                            });
                                        });
                                    }),
                            );
                        if !task_agent_assigned {
                            menu = menu.item(
                                PopupMenuItem::new(assign_task_agent_label.clone())
                                    .icon(AppIcon::Plus)
                                    .on_click(move |_, window, cx| {
                                        let weak = weak_agent.clone();
                                        window.defer(cx, move |_, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                app.assign_orchestrator_agent(task_scope, cx)
                                            });
                                        });
                                    }),
                            );
                        }
                        menu
                    });
                projects = projects.child(collapsible_tree_row(
                    format!("toggle-task-{task_id}"),
                    format!("task-{task_id}"),
                    task_navigation_icon(task),
                    task.title.clone(),
                    new_task_badge,
                    selected,
                    task_expanded,
                    16.,
                    Some(workspace_color(task.color)),
                    Some(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(agent_launch_menu_button(
                                format!("task-add-terminal-{task_id}"),
                                launch_tooltip.clone(),
                                weak.clone(),
                                workspace_id,
                                Some(task_id),
                                None,
                                add_bot_label.clone(),
                                terminal_label.clone(),
                            ))
                            .child(task_menu)
                            .into_any_element(),
                    ),
                    move |_, _, cx| {
                        let _ = weak_task_toggle.update(cx, |app, cx| {
                            app.mark_task_seen(task_id, cx);
                            app.toggle_task_expanded(task_id, cx);
                        });
                    },
                    move |_, _, cx| {
                        let _ = weak_task_row_toggle.update(cx, |app, cx| {
                            app.mark_task_seen(task_id, cx);
                            app.toggle_task_expanded(task_id, cx);
                        });
                    },
                ));
                if !task_expanded {
                    continue;
                }
                if task_agent_assigned {
                    let task_agent_selected = self.orchestrator_surface_visible()
                        && self.active_orchestrator_scope == task_scope;
                    let task_agent_busy = self.orchestrator_turns.contains_key(&task_scope);
                    let weak_task_agent = weak.clone();
                    let weak_remove_task_agent = weak.clone();
                    projects = projects.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .pl(px(40.))
                            .child(agent_chat_tree_row(
                                format!("task-agent-{task_id}"),
                                self.orchestrator_chats
                                    .avatar_color(task_scope)
                                    .display_name()
                                    .into(),
                                task_agent_selected,
                                task_agent_busy,
                                self.orchestrator_chats.avatar_color(task_scope),
                                move |_, _, cx| {
                                    let _ = weak_task_agent.update(cx, |app, cx| {
                                        app.show_orchestrator_chat(task_scope, cx)
                                    });
                                },
                                move |_, _, cx| {
                                    let _ = weak_remove_task_agent.update(cx, |app, cx| {
                                        app.remove_orchestrator_agent(task_scope, cx)
                                    });
                                },
                            )),
                    );
                }
                for agent_id in self
                    .orchestrator_chats
                    .task_agent_ids(task_id)
                    .iter()
                    .copied()
                {
                    let scope = OrchestratorChatScope::TaskAgent { task_id, agent_id };
                    let selected = self.orchestrator_surface_visible()
                        && self.active_orchestrator_scope == scope;
                    let busy = self.orchestrator_turns.contains_key(&scope);
                    let weak_select = weak.clone();
                    let weak_remove = weak.clone();
                    projects = projects.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .pl(px(40.))
                            .child(agent_chat_tree_row(
                                format!("task-agent-{task_id}-{agent_id}"),
                                self.orchestrator_chats
                                    .avatar_color(scope)
                                    .display_name()
                                    .into(),
                                selected,
                                busy,
                                self.orchestrator_chats.avatar_color(scope),
                                move |_, _, cx| {
                                    let _ = weak_select.update(cx, |app, cx| {
                                        app.show_orchestrator_chat(scope, cx)
                                    });
                                },
                                move |_, _, cx| {
                                    let _ = weak_remove.update(cx, |app, cx| {
                                        app.remove_orchestrator_agent(scope, cx)
                                    });
                                },
                            )),
                    );
                }
                let task_notes_selected = self.show_task_note
                    && self.session.selected_task_id == Some(task_id)
                    && self.session.selected_repository_id.is_none();
                let weak_task_notes = weak.clone();
                projects =
                    projects.child(div().w_full().min_w_0().pl(px(40.)).child(tree_row_button(
                        format!("task-notes-{task_id}"),
                        Icon::new(AppIcon::Pencil).small().into_any_element(),
                        self.tr("Notes", "Notas").to_string(),
                        task_notes_selected,
                        move |_, _, cx| {
                            let _ = weak_task_notes.update(cx, |app, cx| {
                                app.show_task_notes_for(workspace_id, task_id, cx)
                            });
                        },
                    )));
                for terminal in self.session.terminals.iter().filter(|terminal| {
                    terminal.workspace_id == workspace_id
                        && terminal.task_id == Some(task_id)
                        && terminal.repository_id.is_none()
                }) {
                    let terminal_id = terminal.id;
                    let weak_terminal = weak.clone();
                    let weak_close_terminal = weak.clone();
                    projects = projects.child(div().w_full().min_w_0().pl(px(40.)).child(
                        terminal_tree_row(
                            terminal_id,
                            terminal.label.clone(),
                            terminal.agent,
                            terminal.state,
                            active_terminal_id == Some(terminal_id),
                            move |_, window, cx| {
                                let _ = weak_terminal.update(cx, |app, cx| {
                                    app.focus_terminal(terminal_id, window, cx)
                                });
                            },
                            move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = weak_close_terminal
                                    .update(cx, |app, cx| app.close_terminal(terminal_id, cx));
                            },
                        ),
                    ));
                }
                for task_repository in &task.repositories {
                    let repository_id = task_repository.repository_id;
                    let name = workspace
                        .repositories
                        .iter()
                        .find(|repository| repository.id == repository_id)
                        .map(|repository| repository.name.as_str())
                        .unwrap_or("repository");
                    let selected = self.session.selected_task_id == Some(task_id)
                        && self.session.selected_repository_id == Some(repository_id);
                    let (branch, additions, deletions, git_loading) = self.repository_git_details(
                        &task_repository.worktree_path,
                        Some(task_repository.branch.as_str()),
                    );
                    let weak_task_repository = weak.clone();
                    projects = projects.child(div().w_full().min_w_0().pl(px(40.)).child(
                        repository_tree_row(
                            format!("task-repository-{task_id}-{repository_id}"),
                            name.to_string(),
                            branch,
                            additions,
                            deletions,
                            git_loading,
                            selected,
                            Some(agent_launch_menu_button(
                                format!("task-repository-add-terminal-{task_id}-{repository_id}"),
                                launch_tooltip.clone(),
                                weak.clone(),
                                workspace_id,
                                Some(task_id),
                                Some(repository_id),
                                add_bot_label.clone(),
                                terminal_label.clone(),
                            )),
                            move |_, _, cx| {
                                let _ = weak_task_repository.update(cx, |app, cx| {
                                    app.mark_task_seen(task_id, cx);
                                    app.select_repository_target(
                                        workspace_id,
                                        Some(task_id),
                                        repository_id,
                                        cx,
                                    )
                                });
                            },
                        ),
                    ));
                    for terminal in self.session.terminals.iter().filter(|terminal| {
                        terminal.workspace_id == workspace_id
                            && terminal.task_id == Some(task_id)
                            && terminal.repository_id == Some(repository_id)
                    }) {
                        let terminal_id = terminal.id;
                        let weak_terminal = weak.clone();
                        let weak_close_terminal = weak.clone();
                        projects = projects.child(div().w_full().min_w_0().pl(px(56.)).child(
                            terminal_tree_row(
                                terminal_id,
                                terminal.label.clone(),
                                terminal.agent,
                                terminal.state,
                                active_terminal_id == Some(terminal_id),
                                move |_, window, cx| {
                                    let _ = weak_terminal.update(cx, |app, cx| {
                                        app.focus_terminal(terminal_id, window, cx)
                                    });
                                },
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    let _ = weak_close_terminal
                                        .update(cx, |app, cx| app.close_terminal(terminal_id, cx));
                                },
                            ),
                        ));
                    }
                }
            }
        }

        let weak_new = weak.clone();
        let weak_collapse_all = weak.clone();
        let weak_brand = weak.clone();
        let weak_global_agent = weak.clone();
        let weak_remove_global_agent = weak.clone();
        let weak_new_global_agent = weak.clone();
        let weak_settings = weak;
        let global_scope = OrchestratorChatScope::Global;
        let global_agent_selected =
            self.orchestrator_surface_visible() && self.active_orchestrator_scope == global_scope;
        let global_agent_busy = self.orchestrator_turns.contains_key(&global_scope);
        let global_agent_preview = self.orchestrator_chat_preview(global_scope);
        let global_agent_color = self.orchestrator_chats.avatar_color(global_scope);
        let mut global_agent_cards = Vec::new();
        if self.orchestrator_chats.has_agent(global_scope) {
            global_agent_cards.push(global_agent_card(
                "global-agent",
                global_agent_color.display_name().into(),
                global_agent_preview,
                global_agent_selected,
                global_agent_busy,
                global_agent_color,
                true,
                move |_, _, cx| {
                    let _ = weak_global_agent.update(cx, |app, cx| {
                        app.show_orchestrator_chat(OrchestratorChatScope::Global, cx)
                    });
                },
                move |_, _, cx| {
                    let _ = weak_remove_global_agent.update(cx, |app, cx| {
                        app.remove_orchestrator_agent(OrchestratorChatScope::Global, cx)
                    });
                },
            ));
        }
        for agent_id in self.orchestrator_chats.global_agent_ids().iter().copied() {
            let scope = OrchestratorChatScope::GlobalAgent(agent_id);
            let selected =
                self.orchestrator_surface_visible() && self.active_orchestrator_scope == scope;
            let busy = self.orchestrator_turns.contains_key(&scope);
            let preview = self.orchestrator_chat_preview(scope);
            let avatar_color = self.orchestrator_chats.avatar_color(scope);
            let weak_select = weak_brand.clone();
            let weak_remove = weak_brand.clone();
            global_agent_cards.push(global_agent_card(
                format!("global-agent-{agent_id}"),
                avatar_color.display_name().into(),
                preview,
                selected,
                busy,
                avatar_color,
                true,
                move |_, _, cx| {
                    let _ = weak_select.update(cx, |app, cx| app.show_orchestrator_chat(scope, cx));
                },
                move |_, _, cx| {
                    let _ =
                        weak_remove.update(cx, |app, cx| app.remove_orchestrator_agent(scope, cx));
                },
            ));
        }
        let global_agent_count = global_agent_cards.len();
        let global_agent_list = v_flex()
            .id("global-agent-list")
            .w_full()
            .flex_none()
            .p_2()
            .gap_1()
            .border_b_1()
            .border_color(border)
            .children(global_agent_cards);
        let global_agent_list = if global_agent_count > 3 {
            global_agent_list
                .max_h(px(188.))
                .overflow_y_scrollbar()
                .into_any_element()
        } else {
            global_agent_list.into_any_element()
        };

        v_flex()
            .w_full()
            .h_full()
            .flex_none()
            .bg(background)
            .border_r_1()
            .border_color(border)
            .child(
                h_flex()
                    .id("sidebar-home")
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x151820)))
                    .on_click(move |_, _, cx| {
                        let _ = weak_brand.update(cx, |app, cx| {
                            app.show_orchestrator_chat(OrchestratorChatScope::Global, cx)
                        });
                    })
                    .child(
                        div()
                            .flex_1()
                            .child(app_name_label(SIDEBAR_APP_NAME_FONT_SIZE)),
                    )
                    .child(sidebar_icon_button(
                        "new-global-agent",
                        AppIcon::Plus,
                        move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = weak_new_global_agent
                                .update(cx, |app, cx| app.create_global_orchestrator_agent(cx));
                        },
                    )),
            )
            .child(global_agent_list)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .h(px(40.))
                            .flex_none()
                            .px_3()
                            .gap_1()
                            .border_b_1()
                            .border_color(border)
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child(projects_label),
                            )
                            .child(sidebar_icon_button(
                                "collapse-all-projects",
                                AppIcon::ChevronsUp,
                                move |_, _, cx| {
                                    let _ = weak_collapse_all
                                        .update(cx, |app, cx| app.collapse_all_navigation(cx));
                                },
                            ))
                            .child(sidebar_icon_button(
                                "new-project",
                                AppIcon::Plus,
                                move |_, window, cx| {
                                    let _ = weak_new
                                        .update(cx, |app, cx| app.open_create_project(window, cx));
                                },
                            )),
                    )
                    .child(
                        projects
                            .id("sidebar-projects-scroll")
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .p_2()
                            .overflow_y_scroll()
                            .track_scroll(&self.sidebar_scroll)
                            .vertical_scrollbar(&self.sidebar_scroll),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .border_t_1()
                    .border_color(border)
                    .text_color(muted)
                    .child(
                        h_flex()
                            .id("sidebar-settings")
                            .w_full()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded(px(6.))
                            .bg(if self.show_settings {
                                rgb(0x29364f)
                            } else {
                                rgb(0x111318)
                            })
                            .text_color(if self.show_settings {
                                rgb(0xdde8ff)
                            } else {
                                rgb(0xb6bdca)
                            })
                            .text_size(px(12.))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242a35)))
                            .on_click(move |_, _, cx| {
                                let _ = weak_settings.update(cx, |app, cx| app.show_settings(cx));
                            })
                            .child(Icon::new(AppIcon::Settings).small())
                            .child(self.tr("Settings", "Configuración")),
                    ),
            )
            .into_any_element()
    }

    fn render_dock(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(terminal_id) = self.selected_terminal_id() else {
            return self.render_empty_state(cx);
        };
        self.render_terminal_panel(terminal_id, terminal_id, cx)
    }

    fn render_terminal_panel(
        &self,
        terminal_id: Uuid,
        active_terminal_id: Uuid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let descriptor = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == terminal_id);
        let active = terminal_id == active_terminal_id;
        let terminal_working =
            descriptor.is_some_and(|terminal| terminal.state == SessionState::Working);
        let closed_terminal_label = self.tr("Closed terminal", "Terminal cerrada");
        let (panel_border, panel_header, panel_muted) = match self.session.theme {
            AppTheme::Dark => (rgb(0x252a33), rgb(0x15181e), rgb(0x9ba3b4)),
            AppTheme::Light => (rgb(0xd9dee8), rgb(0xf0f2f6), rgb(0x657084)),
        };
        let weak_focus = weak.clone();
        let weak_close = weak.clone();
        let mut panel = v_flex()
            .id(SharedString::from(format!("terminal-panel-{terminal_id}")))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_1()
            .border_color(if active { rgb(0x5c7cfa) } else { panel_border })
            .on_click(move |_, window, cx| {
                let _ =
                    weak_focus.update(cx, |app, cx| app.focus_terminal(terminal_id, window, cx));
            });

        panel = panel.child(
            h_flex()
                .h(px(28.))
                .flex_none()
                .px_2()
                .bg(panel_header)
                .text_size(px(11.))
                .text_color(panel_muted)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div().text_ellipsis().child(
                                descriptor
                                    .map(|terminal| terminal.label.clone())
                                    .unwrap_or_else(|| closed_terminal_label.into()),
                            ),
                        )
                        .when(terminal_working, |this| this.child(agent_working_dots())),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("close-terminal-{terminal_id}")))
                        .px_2()
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(0xff7b72)))
                        .on_click(move |_, _, cx| {
                            let _ = weak_close
                                .update(cx, |app, cx| app.close_terminal(terminal_id, cx));
                        })
                        .child(Icon::new(AppIcon::X).with_size(px(12.))),
                ),
        );

        if let Some(handle) = self.terminals.get(&terminal_id) {
            panel
                .child(div().flex_1().min_h_0().child(handle.view.clone()))
                .into_any_element()
        } else {
            panel
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_color(panel_muted)
                        .child(self.tr("Starting terminal…", "Iniciando terminal…")),
                )
                .into_any_element()
        }
    }

    fn render_app_toasts(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let mut stack = v_flex()
            .absolute()
            .top(px(56.))
            .right_4()
            .w(px(360.))
            .gap_2();

        for toast in self.app_toasts.iter().rev() {
            let target = toast.target;
            let element_key = target.element_key();
            let (
                icon,
                icon_background,
                border_color,
                background,
                hover_background,
                hover_border_color,
                title_color,
                message_color,
                close_color,
                close_hover_background,
            ) = match target {
                AppToastTarget::Terminal { agent, .. } => (
                    agent_icon(agent),
                    if agent == AgentKind::Codex {
                        rgb(0x121916)
                    } else {
                        rgb(0xf1f7f4)
                    },
                    rgb(0x42695c),
                    rgb(0x203b32),
                    rgb(0x27483c),
                    rgb(0x5a8b78),
                    rgb(0xf2f7f5),
                    rgb(0xb8ccc4),
                    rgb(0xa9c0b7),
                    rgb(0x355a4c),
                ),
                AppToastTarget::Task { .. } => (
                    div()
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x8bb9ff))
                        .child(Icon::new(AppIcon::ListTodo).with_size(px(15.)))
                        .into_any_element(),
                    rgb(0x17253b),
                    rgb(0x466fa8),
                    rgb(0x1d2e49),
                    rgb(0x263c5e),
                    rgb(0x628bc2),
                    rgb(0xf2f6fd),
                    rgb(0xb8c8df),
                    rgb(0xa9bad2),
                    rgb(0x314b72),
                ),
                AppToastTarget::Agent { scope } => (
                    black_bot_avatar(20., self.orchestrator_chats.avatar_color(scope)),
                    rgb(0x3b2a13),
                    rgb(0x9b6b2b),
                    rgb(0x3a2a17),
                    rgb(0x49351d),
                    rgb(0xb68139),
                    rgb(0xfff7ea),
                    rgb(0xe4cfad),
                    rgb(0xd8bc8e),
                    rgb(0x5b4225),
                ),
            };
            let weak_open = weak.clone();
            let weak_dismiss = weak.clone();
            stack = stack.child(
                h_flex()
                    .id(SharedString::from(format!("app-toast-{element_key}")))
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_3()
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(border_color)
                    .bg(background)
                    .shadow_lg()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_background).border_color(hover_border_color))
                    // Toasts float over the active panel, so the panel below must not
                    // reclaim the click before the notification target is opened.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = weak_open.update(cx, |app, cx| match target {
                            AppToastTarget::Terminal { terminal_id, .. } => {
                                app.focus_terminal(terminal_id, window, cx)
                            }
                            AppToastTarget::Task { task_id } => {
                                app.open_task_from_toast(task_id, cx)
                            }
                            AppToastTarget::Agent { scope } => app.open_agent_from_toast(scope, cx),
                        });
                    })
                    .child(
                        div()
                            .size(px(36.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(icon_background)
                            .child(icon),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.))
                                    .text_color(title_color)
                                    .child(toast.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(message_color)
                                    .child(toast.message.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "dismiss-app-toast-{element_key}"
                            )))
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .text_color(close_color)
                            .hover(move |style| {
                                style.bg(close_hover_background).text_color(rgb(0xffffff))
                            })
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = weak_dismiss
                                    .update(cx, |app, cx| app.dismiss_app_toast(target, cx));
                            })
                            .child(Icon::new(AppIcon::X).with_size(px(13.))),
                    ),
            );
        }

        stack.into_any_element()
    }

    fn render_quick_open(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.quick_open.as_ref()?;
        let mode = state.mode;
        let query = state.query.clone();
        let selected = state.selected;
        let results = self.quick_open_results(cx);
        let status_message = match &state.entries {
            QuickOpenEntries::Loading => Some(
                self.tr(
                    "Indexing repository files…",
                    "Indexando archivos del repositorio…",
                )
                .to_string(),
            ),
            QuickOpenEntries::Error(error) => Some(error.clone()),
            QuickOpenEntries::Ready(_) if results.is_empty() => Some(
                self.tr("No matching results", "No hay resultados")
                    .to_string(),
            ),
            QuickOpenEntries::Ready(_) => None,
        };
        let shortcut = match mode {
            QuickOpenMode::Navigation => "⌘O",
            QuickOpenMode::Files => "⌘P",
        };
        let footer_label = match mode {
            QuickOpenMode::Navigation => self.tr(
                "Projects and tasks · Notes open directly",
                "Proyectos y tareas · Las notas se abren directamente",
            ),
            QuickOpenMode::Files => self.tr(
                "Files from the selected repository",
                "Archivos del repositorio seleccionado",
            ),
        };

        let weak = cx.weak_entity();
        let weak_close = weak.clone();
        let mut result_list = v_flex().w_full().min_h_0();
        for (index, item) in results.into_iter().enumerate() {
            let is_selected = index == selected;
            let target = item.target.clone();
            let weak_activate = weak.clone();
            result_list = result_list.child(
                h_flex()
                    .id(SharedString::from(format!("quick-open-result-{index}")))
                    .w_full()
                    .min_w_0()
                    .h(px(48.))
                    .px_4()
                    .gap_3()
                    .items_center()
                    .bg(if is_selected {
                        rgb(0x353840)
                    } else {
                        rgb(0x15171b)
                    })
                    .text_color(rgb(0xe5e9f0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x292c33)))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let target = target.clone();
                        let _ = weak_activate
                            .update(cx, |app, cx| app.activate_quick_open_target(target, cx));
                    })
                    .child(
                        div()
                            .size(px(26.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(item.color)
                            .child(Icon::new(item.icon).with_size(px(17.))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(13.))
                            .when(mode == QuickOpenMode::Navigation, |this| {
                                this.w(px(500.)).flex_none()
                            })
                            .when(mode == QuickOpenMode::Files, |this| {
                                this.w(px(200.)).flex_none()
                            })
                            .child(item.title),
                    )
                    .when(mode == QuickOpenMode::Files, |this| {
                        this.child(
                            div()
                                .w(px(300.))
                                .flex_none()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(rgb(0x8e97aa))
                                .child(item.subtitle),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(rgb(0x777f8e))
                            .child(item.kind_label),
                    ),
            );
        }

        let results = if let Some(message) = status_message {
            v_flex()
                .w_full()
                .h(px(160.))
                .items_center()
                .justify_center()
                .px_6()
                .text_size(px(12.))
                .text_color(match &state.entries {
                    QuickOpenEntries::Error(_) => rgb(0xff7b72),
                    _ => rgb(0x8e97aa),
                })
                .child(message)
                .into_any_element()
        } else {
            result_list
                .max_h(px(384.))
                .overflow_y_scrollbar()
                .into_any_element()
        };

        Some(
            div()
                .id("quick-open-overlay")
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(48.))
                .px(px(40.))
                .bg(rgba(0x000000a6))
                .on_click(move |_, _, cx| {
                    let _ = weak_close.update(cx, |app, cx| {
                        app.quick_open = None;
                        cx.notify();
                    });
                })
                .child(
                    v_flex()
                        .id("quick-open-panel")
                        .w(px(700.))
                        .max_w_full()
                        .max_h(px(480.))
                        .overflow_hidden()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgb(0x4a4f58))
                        .bg(rgb(0x15171b))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .on_key_down(cx.listener(Self::handle_quick_open_key_down))
                        .child(
                            h_flex()
                                .w_full()
                                .h(px(58.))
                                .px_4()
                                .gap_3()
                                .items_center()
                                .border_b_1()
                                .border_color(rgb(0x292d35))
                                .child(
                                    Icon::new(AppIcon::Search)
                                        .with_size(px(19.))
                                        .text_color(rgb(0x9aa3b2)),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&query)
                                            .appearance(false)
                                            .bordered(false)
                                            .focus_bordered(false),
                                    ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .px_2()
                                        .py_1()
                                        .rounded(px(5.))
                                        .border_1()
                                        .border_color(rgb(0x454a54))
                                        .text_size(px(10.))
                                        .text_color(rgb(0x9aa3b2))
                                        .child(shortcut),
                                ),
                        )
                        .child(results)
                        .child(
                            h_flex()
                                .w_full()
                                .h(px(30.))
                                .px_3()
                                .items_center()
                                .justify_between()
                                .border_t_1()
                                .border_color(rgb(0x292d35))
                                .text_size(px(10.))
                                .text_color(rgb(0x737b8a))
                                .child("↑↓ Navegar   ↵ Abrir   esc Cerrar")
                                .child(footer_label),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_empty_state(&self, _cx: &mut Context<Self>) -> AnyElement {
        if let Some(webview) = &self.orchestrator_webview {
            return div()
                .size_full()
                .min_w_0()
                .child(webview.clone())
                .into_any_element();
        }

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                img(self.app_logo.clone())
                    .size(px(96.))
                    .flex_none()
                    .rounded(px(22.)),
            )
            .child(app_name_label(APP_NAME_FONT_SIZE))
            .child(div().mt_2().text_color(rgb(0x8e97aa)).child(self.tr(
                "The orchestrator WebView could not be loaded.",
                "No se pudo cargar el WebView del orquestador.",
            )))
            .into_any_element()
    }
}

impl Render for BlackholesApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_update_guard();
        if !self.show_settings
            && self.project_settings_workspace_id.is_none()
            && !self.show_project_note
            && !self.show_task_note
            && !self.show_terminal
            && self.file_explorer.mode == FileExplorerMode::Files
        {
            self.ensure_file_editor(window, cx);
        }
        if self.show_project_note {
            if let Some(workspace_id) = self.session.selected_workspace_id {
                self.ensure_project_note_editor(workspace_id, window, cx);
            }
        } else if self.show_task_note {
            if let Some(task_id) = self.session.selected_task_id {
                self.ensure_task_note_editor(task_id, window, cx);
            }
        }
        self.hydrate_active_workspace_surface(cx);
        if !self.show_terminal {
            self.hydrate_workspace_status(cx);
        }

        // The central React surface owns every visual workspace except the
        // high-throughput native terminal. Quick-open lives inside that same
        // surface so macOS can preserve the real content beneath its backdrop.
        let orchestrator_visible =
            (!self.show_terminal || self.quick_open.is_some()) && !window.has_active_dialog(cx);
        if let Some(webview) = &self.orchestrator_webview {
            orchestrator_chat::set_visible(webview, orchestrator_visible, cx);
        }
        let navigation_visible = !self.show_settings && !window.has_active_dialog(cx);
        if let Some(webview) = &self.navigation_webview {
            navigation_webview::set_visible(webview, navigation_visible, cx);
            self.hydrate_navigation(cx);
        }
        let (background, foreground, chrome_border) = match self.session.theme {
            AppTheme::Dark => (rgb(0x0c0e12), rgb(0xe5e9f0), rgb(0x252a33)),
            AppTheme::Light => (rgb(0xf6f7fa), rgb(0x18212f), rgb(0xd9dee8)),
        };
        let weak_status = cx.weak_entity();
        let body = if self.show_terminal {
            self.render_dock(cx)
        } else {
            self.render_empty_state(cx)
        };

        let mut main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .text_color(foreground)
            .child(body);

        if let Some(busy) = &self.busy {
            main = main.child(
                div()
                    .absolute()
                    .right_4()
                    .bottom_4()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .bg(rgb(0x2d6cdf))
                    .text_color(rgb(0xffffff))
                    .child(busy.clone()),
            );
        } else if let Some((message, error)) = &self.status {
            let background = if *error { rgb(0x7a263a) } else { rgb(0x245b46) };
            main = main.child(
                div()
                    .id("status-message")
                    .absolute()
                    .right_4()
                    .bottom_4()
                    .max_w(px(520.))
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .bg(background)
                    .text_color(rgb(0xffffff))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let _ = weak_status.update(cx, |app, cx| {
                            app.status = None;
                            cx.notify();
                        });
                    })
                    .child(message.clone()),
            );
        }

        let workspace_content = main.into_any_element();

        let sidebar_width = self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let sidebar = self
            .navigation_webview
            .as_ref()
            .map(|webview| {
                div()
                    .size_full()
                    .min_w_0()
                    .child(webview.clone())
                    .into_any_element()
            })
            .unwrap_or_else(|| self.render_sidebar(cx));
        let workspace_layout = if self.show_settings {
            workspace_content
        } else {
            h_flex().size_full()
                .child(div().w(px(sidebar_width)).h_full().flex_none().child(sidebar))
                .child(div().flex_1().min_w_0().h_full().child(workspace_content))
                .into_any_element()
        };

        div()
            .relative()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .on_action(cx.listener(|app, _: &OpenNavigationPalette, window, cx| {
                app.open_navigation_palette(window, cx)
            }))
            .on_action(
                cx.listener(|app, _: &OpenFilePalette, window, cx| {
                    app.open_file_palette(window, cx)
                }),
            )
            .child(
                v_flex()
                    .size_full()
                    .child(TitleBar::new().bg(background).border_color(chrome_border).child(
                        h_flex().w_full().justify_end().gap_2().pr_3()
                            .child(div().text_xs().text_color(rgb(0x8791a2)).child(concat!("v", env!("CARGO_PKG_VERSION"))))
                            .child(Button::new("app-update")
                                .xsmall()
                                .rounded_full()
                                .label(if self.update_state.restart {
                                    self.tr("Restart to update", "Reiniciar para actualizar")
                                } else if self.update_state.busy {
                                    self.tr("Updating…", "Actualizando…")
                                } else if !self.update_state.available.is_empty() {
                                    self.tr("Update", "Actualizar")
                                } else {
                                    self.tr("Check for updates", "Buscar actualizaciones")
                                })
                                .when(!self.update_state.available.is_empty() || self.update_state.restart, |button| button.primary())
                                .tooltip(if self.update_state.available.is_empty() {
                                    self.tr("Check GitHub Releases for a new version", "Buscar una nueva versión en GitHub Releases").to_string()
                                } else { format!("Blackholes {}", self.update_state.available) })
                                .on_click(cx.listener(|app, _, _, cx| app.check_app_update(cx))))
                    ))
                    .child(div().flex_1().min_h_0().w_full().child(workspace_layout)),
            )
            .child(self.render_app_toasts(cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

fn quick_open_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let file_name_start = candidate.rfind('/').map_or(0, |index| index + 1);
    let file_name = &candidate[file_name_start..];

    if candidate == query {
        return Some(140_000);
    }
    if file_name == query {
        return Some(135_000 - candidate.len() as i64);
    }
    if file_name.starts_with(query) {
        return Some(125_000 - file_name.len() as i64 - file_name_start as i64);
    }
    if candidate.starts_with(query) {
        return Some(118_000 - candidate.len() as i64);
    }
    if let Some(index) = candidate.find(&query) {
        let file_name_bonus = if index >= file_name_start { 12_000 } else { 0 };
        let boundary_bonus = if index == 0
            || candidate
                .as_bytes()
                .get(index.saturating_sub(1))
                .is_some_and(|byte| matches!(byte, b'/' | b'_' | b'-' | b'.' | b' '))
        {
            4_000
        } else {
            0
        };
        return Some(
            100_000 + file_name_bonus + boundary_bonus - index as i64 * 12 - candidate.len() as i64,
        );
    }

    // Space-separated terms can match independent filename/path portions, e.g.
    // `chat quick` finds `frontend/src/chat/QuickOpen.tsx`.
    let mut score = 30_000_i64;
    let mut term_count = 0_i64;
    for term in query.split_whitespace() {
        term_count += 1;
        if let Some(index) = candidate.find(term) {
            score += 9_000;
            if index >= file_name_start {
                score += 3_000;
            }
            if index == 0
                || candidate
                    .as_bytes()
                    .get(index.saturating_sub(1))
                    .is_some_and(|byte| matches!(byte, b'/' | b'_' | b'-' | b'.' | b' '))
            {
                score += 1_500;
            }
            score -= index as i64 * 4;
        } else {
            score += quick_open_fuzzy_term_score(term, &candidate, file_name_start)?;
        }
    }

    Some(score + term_count * 500 - candidate.len() as i64)
}

fn quick_open_fuzzy_term_score(term: &str, candidate: &str, file_name_start: usize) -> Option<i64> {
    let mut score = 0_i64;
    let mut search_from = 0_usize;
    let mut previous_end = None;
    let mut first_match = None;
    for character in term.chars() {
        let offset = candidate[search_from..].find(character)?;
        let index = search_from + offset;
        first_match.get_or_insert(index);
        score += 700;
        if previous_end == Some(index) {
            score += 850;
        } else if let Some(previous_end) = previous_end {
            score -= (index.saturating_sub(previous_end) as i64).min(80) * 12;
        }
        if index == 0
            || candidate
                .as_bytes()
                .get(index.saturating_sub(1))
                .is_some_and(|byte| matches!(byte, b'/' | b'_' | b'-' | b'.' | b' '))
        {
            score += 500;
        }
        if index >= file_name_start {
            score += 180;
        }
        let next = index + character.len_utf8();
        previous_end = Some(next);
        search_from = next;
    }
    Some(score - first_match.unwrap_or_default() as i64 * 8)
}

fn compact_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .px_2()
        .py_1()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(0x303642))
        .bg(rgb(0x1a1e26))
        .text_color(rgb(0xd8deea))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x29303c)).border_color(rgb(0x536178)))
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn settings_agent_skill_row(
    skill: AgentSkill,
    enabled: bool,
    state_label: impl Into<SharedString>,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    h_flex()
        .id(SharedString::from(format!(
            "settings-agent-skill-{}",
            skill.name
        )))
        .w_full()
        .min_w_0()
        .items_center()
        .gap_3()
        .p_3()
        .rounded(px(8.))
        .border_1()
        .border_color(if enabled {
            rgb(0x394a6d)
        } else {
            rgb(0x2b303a)
        })
        .bg(if enabled {
            rgb(0x171d2a)
        } else {
            rgb(0x111318)
        })
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .bg(if enabled {
                    rgb(0x29364f)
                } else {
                    rgb(0x1c2027)
                })
                .text_color(if enabled {
                    rgb(0x9ab6ff)
                } else {
                    rgb(0x747d8e)
                })
                .child(Icon::new(AppIcon::Code2).with_size(px(15.))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(0xd8deea))
                        .child(skill.name),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(rgb(0x8e97aa))
                        .child(skill.description),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(rgb(0x626a78))
                        .child(skill.path.display().to_string()),
                ),
        )
        .child(choice_button(
            SharedString::from(format!("toggle-agent-skill-{}", skill.path.display())),
            state_label,
            enabled,
            on_toggle,
        ))
        .into_any_element()
}

fn provider_plan_name(provider: AgentProvider, usage: Option<&ClaudePlanUsage>, language: Language) -> String {
    if provider == AgentProvider::Claude { return claude_plan_name(usage, language); }
    match usage.and_then(|usage| usage.subscription_type.as_deref()) {
        Some(plan) => format!("{} · {plan}", provider.display_name()),
        None => match language {
            Language::English => "Not reported".into(),
            Language::Spanish => "No reportado".into(),
        },
    }
}

fn provider_plan_detail(usage: Option<&ClaudePlanUsage>, language: Language) -> String {
    match (usage, language) {
        (Some(usage), Language::English) if usage.rate_limits_available => "Limits reported by the selected account".into(),
        (Some(usage), Language::Spanish) if usage.rate_limits_available => "Límites reportados por la cuenta seleccionada".into(),
        (Some(_), Language::English) => "This account or provider did not report plan limits".into(),
        (Some(_), Language::Spanish) => "Esta cuenta o proveedor no reportó límites del plan".into(),
        (None, Language::English) => "Refresh to query the selected account".into(),
        (None, Language::Spanish) => "Actualiza para consultar la cuenta seleccionada".into(),
    }
}

fn claude_plan_name(usage: Option<&ClaudePlanUsage>, language: Language) -> String {
    match usage {
        Some(usage) if usage.subscription_type.is_some() => {
            let subscription = usage.subscription_type.as_deref().unwrap_or_default();
            match subscription.to_ascii_lowercase().as_str() {
                "pro" => "Claude Pro".to_string(),
                "max" => "Claude Max".to_string(),
                "team" => "Claude Team".to_string(),
                "enterprise" => "Claude Enterprise".to_string(),
                _ => subscription.to_string(),
            }
        }
        Some(_) => match language {
            Language::English => "API or other provider".to_string(),
            Language::Spanish => "API u otro proveedor".to_string(),
        },
        None => match language {
            Language::English => "Not detected".to_string(),
            Language::Spanish => "No detectado".to_string(),
        },
    }
}

fn claude_plan_detail(usage: Option<&ClaudePlanUsage>, language: Language) -> String {
    match usage {
        Some(usage) if usage.rate_limits_available => match language {
            Language::English => "Live plan limits available".to_string(),
            Language::Spanish => "Límites del plan disponibles".to_string(),
        },
        Some(_) => match language {
            Language::English => "Plan limits are unavailable with API-key billing".to_string(),
            Language::Spanish => {
                "Los límites del plan no están disponibles con facturación por API key".to_string()
            }
        },
        None => match language {
            Language::English => "Waiting for the next agent response".to_string(),
            Language::Spanish => "Esperando la próxima respuesta de un agente".to_string(),
        },
    }
}

fn claude_limit_display(
    window: Option<&ClaudeRateLimitWindow>,
    language: Language,
) -> (String, String, Option<f32>) {
    let Some(window) = window else {
        return match language {
            Language::English => (
                "Unavailable".to_string(),
                "Claude did not report this window".to_string(),
                None,
            ),
            Language::Spanish => (
                "No disponible".to_string(),
                "Claude no reportó esta ventana".to_string(),
                None,
            ),
        };
    };
    let Some(utilization) = window.utilization else {
        return match language {
            Language::English => (
                "Unavailable".to_string(),
                "No utilization value reported".to_string(),
                None,
            ),
            Language::Spanish => (
                "No disponible".to_string(),
                "No se reportó un porcentaje".to_string(),
                None,
            ),
        };
    };

    let utilization = utilization.clamp(0.0, 100.0);
    let remaining = 100.0 - utilization;
    let reset = window
        .resets_at
        .as_deref()
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Local)
                .format("%-d/%m %H:%M")
                .to_string()
        });
    let value = match language {
        Language::English => format!("{remaining:.0}% available"),
        Language::Spanish => format!("{remaining:.0}% disponible"),
    };
    let detail = match (language, reset) {
        (Language::English, Some(reset)) => {
            format!("{utilization:.0}% used · resets {reset}")
        }
        (Language::Spanish, Some(reset)) => {
            format!("{utilization:.0}% usado · reinicia {reset}")
        }
        (Language::English, None) => format!("{utilization:.0}% used"),
        (Language::Spanish, None) => format!("{utilization:.0}% usado"),
    };
    (value, detail, Some(utilization as f32))
}

fn settings_claude_usage_card(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    utilization: Option<f32>,
) -> AnyElement {
    v_flex()
        .w(px(205.))
        .min_h(px(104.))
        .flex_none()
        .gap_1()
        .p_3()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(0x2b313b))
        .bg(rgb(0x12151a))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(0x8e97aa))
                .child(label.into()),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(0xe7ebf3))
                .child(value.into()),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(10.))
                .text_color(rgb(0x8e97aa))
                .child(detail.into()),
        )
        .when_some(utilization, |this, utilization| {
            this.child(
                div()
                    .w_full()
                    .h(px(4.))
                    .overflow_hidden()
                    .rounded(px(2.))
                    .bg(rgb(0x252a33))
                    .child(
                        div()
                            .h_full()
                            .w(relative((utilization / 100.0).clamp(0.0, 1.0)))
                            .rounded(px(2.))
                            .bg(if utilization >= 90.0 {
                                rgb(0xe26d6d)
                            } else if utilization >= 70.0 {
                                rgb(0xe2aa5f)
                            } else {
                                rgb(0x6fcf97)
                            }),
                    ),
            )
        })
        .into_any_element()
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn content_revision(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn note_owner_from_command(owner: &str, id: Uuid) -> Option<NoteOwner> {
    match owner {
        "project" => Some(NoteOwner::Project(id)),
        "task" => Some(NoteOwner::Task(id)),
        _ => None,
    }
}

fn note_save_state_id(state: NoteSaveState) -> &'static str {
    match state {
        NoteSaveState::Saved => "saved",
        NoteSaveState::Saving => "saving",
        NoteSaveState::Error => "error",
    }
}

fn workspace_color_id(color: WorkspaceColor) -> &'static str {
    match color {
        WorkspaceColor::Slate => "slate",
        WorkspaceColor::Coral => "coral",
        WorkspaceColor::Peach => "peach",
        WorkspaceColor::Amber => "amber",
        WorkspaceColor::Sage => "sage",
        WorkspaceColor::Mint => "mint",
        WorkspaceColor::Sky => "sky",
        WorkspaceColor::Lavender => "lavender",
        WorkspaceColor::Rose => "rose",
    }
}

fn repository_change_kind_id(kind: RepositoryChangeKind) -> &'static str {
    match kind {
        RepositoryChangeKind::Added => "added",
        RepositoryChangeKind::Deleted => "deleted",
        RepositoryChangeKind::Modified => "modified",
        RepositoryChangeKind::Renamed => "renamed",
        RepositoryChangeKind::Untracked => "untracked",
        RepositoryChangeKind::Conflicted => "conflicted",
    }
}

fn repository_diff_row_json(row: &RepositoryDiffRow) -> serde_json::Value {
    match row {
        RepositoryDiffRow::Hunk {
            old_start,
            new_start,
            header,
        } => serde_json::json!({
            "row_type": "hunk",
            "old_start": old_start,
            "new_start": new_start,
            "header": header,
        }),
        RepositoryDiffRow::Line {
            old_number,
            new_number,
            old_text,
            new_text,
            kind,
        } => serde_json::json!({
            "row_type": "line",
            "kind": match kind {
                RepositoryDiffLineKind::Context => "context",
                RepositoryDiffLineKind::Changed => "changed",
                RepositoryDiffLineKind::Added => "added",
                RepositoryDiffLineKind::Deleted => "deleted",
            },
            "old_number": old_number,
            "new_number": new_number,
            "old_text": old_text,
            "new_text": new_text,
        }),
    }
}

fn explorer_mode_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .h(px(28.))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .rounded(px(6.))
        .border_1()
        .border_color(if selected {
            rgb(0x40577f)
        } else {
            rgb(0x252a33)
        })
        .bg(if selected {
            rgb(0x24304a)
        } else {
            rgb(0x15181e)
        })
        .text_size(px(11.))
        .font_weight(if selected {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0x8e97aa)
        })
        .truncate()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x29303c)).text_color(rgb(0xe7ebf3)))
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn repository_change_style(kind: RepositoryChangeKind) -> (&'static str, gpui::Rgba) {
    match kind {
        RepositoryChangeKind::Added => ("A", rgb(0x56b881)),
        RepositoryChangeKind::Deleted => ("D", rgb(0xd7656f)),
        RepositoryChangeKind::Modified => ("M", rgb(0xd1b46f)),
        RepositoryChangeKind::Renamed => ("R", rgb(0x8db3cf)),
        RepositoryChangeKind::Untracked => ("U", rgb(0x56b881)),
        RepositoryChangeKind::Conflicted => ("!", rgb(0xff7b72)),
    }
}

fn render_repository_diff_row(row: RepositoryDiffRow) -> AnyElement {
    match row {
        RepositoryDiffRow::Hunk {
            old_start,
            new_start,
            header,
        } => {
            let right_header = header.clone();
            h_flex()
                .w_full()
                .h(px(24.))
                .min_w_0()
                .font_family(CODE_FONT_FAMILY)
                .text_size(px(10.))
                .text_color(rgb(0x9fb8e8))
                .child(repository_diff_hunk_side(old_start, header, true))
                .child(repository_diff_hunk_side(new_start, right_header, false))
                .into_any_element()
        }
        RepositoryDiffRow::Line {
            old_number,
            new_number,
            old_text,
            new_text,
            kind,
        } => {
            let (old_background, old_number_background, new_background, new_number_background) =
                match kind {
                    RepositoryDiffLineKind::Context => {
                        (rgb(0x20262e), rgb(0x252c35), rgb(0x20262e), rgb(0x252c35))
                    }
                    RepositoryDiffLineKind::Changed => {
                        (rgb(0x3a2b30), rgb(0x59343a), rgb(0x273a31), rgb(0x345542))
                    }
                    RepositoryDiffLineKind::Deleted => {
                        (rgb(0x3a2b30), rgb(0x59343a), rgb(0x20262e), rgb(0x252c35))
                    }
                    RepositoryDiffLineKind::Added => {
                        (rgb(0x20262e), rgb(0x252c35), rgb(0x273a31), rgb(0x345542))
                    }
                };
            let old_prefix = if matches!(
                kind,
                RepositoryDiffLineKind::Changed | RepositoryDiffLineKind::Deleted
            ) {
                "−"
            } else {
                " "
            };
            let new_prefix = if matches!(
                kind,
                RepositoryDiffLineKind::Changed | RepositoryDiffLineKind::Added
            ) {
                "+"
            } else {
                " "
            };
            h_flex()
                .w_full()
                .h(px(24.))
                .min_w_0()
                .font_family(CODE_FONT_FAMILY)
                .text_size(px(11.))
                .child(repository_diff_line_side(
                    old_number,
                    old_text,
                    old_prefix,
                    old_background,
                    old_number_background,
                    true,
                ))
                .child(repository_diff_line_side(
                    new_number,
                    new_text,
                    new_prefix,
                    new_background,
                    new_number_background,
                    false,
                ))
                .into_any_element()
        }
    }
}

fn repository_diff_hunk_side(line_number: usize, header: String, with_divider: bool) -> AnyElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(rgb(0x293a59))
        .when(with_divider, |this| {
            this.border_r_1().border_color(rgb(0x3c4f72))
        })
        .child(
            div()
                .w(px(48.))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .pr_2()
                .bg(rgb(0x304568))
                .text_color(rgb(0xa8b8d5))
                .child(line_number.to_string()),
        )
        .child(div().flex_1().min_w_0().px_2().truncate().child(header))
        .into_any_element()
}

fn repository_diff_line_side(
    line_number: Option<usize>,
    text: String,
    prefix: &'static str,
    background: gpui::Rgba,
    number_background: gpui::Rgba,
    with_divider: bool,
) -> AnyElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(background)
        .when(with_divider, |this| {
            this.border_r_1().border_color(rgb(0x38404b))
        })
        .child(
            div()
                .w(px(48.))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .pr_2()
                .bg(number_background)
                .text_color(rgb(0x9aa4b3))
                .child(
                    line_number
                        .map(|number| number.to_string())
                        .unwrap_or_default(),
                ),
        )
        .child(
            div()
                .w(px(17.))
                .flex_none()
                .text_color(if prefix == "+" {
                    rgb(0x8bd5a8)
                } else if prefix == "−" {
                    rgb(0xee9a9a)
                } else {
                    rgb(0x788294)
                })
                .child(prefix),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr_2()
                .truncate()
                .text_color(rgb(0xd8deea))
                .child(text),
        )
        .into_any_element()
}

fn new_task_chip(label: impl Into<SharedString>) -> AnyElement {
    div()
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .px_1()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(0x3f7f68))
        .bg(rgb(0x19382f))
        .text_color(rgb(0x8ce0bd))
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(label.into())
        .into_any_element()
}

fn note_preview_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .px_3()
        .py_1()
        .rounded(px(18.))
        .border_1()
        .border_color(rgb(0x4778bd))
        .bg(rgb(0x203554))
        .text_color(rgb(0x8bb9ff))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x293a53)).border_color(rgb(0x536d91)))
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn note_save_label(state: NoteSaveState, language: Language) -> &'static str {
    match (state, language) {
        (NoteSaveState::Saved, Language::English) => "Saved",
        (NoteSaveState::Saved, Language::Spanish) => "Guardado",
        (NoteSaveState::Saving, Language::English) => "Saving…",
        (NoteSaveState::Saving, Language::Spanish) => "Guardando…",
        (NoteSaveState::Error, Language::English) => "Save failed",
        (NoteSaveState::Error, Language::Spanish) => "Error al guardar",
    }
}

fn note_save_status(state: NoteSaveState, label: &'static str) -> AnyElement {
    div()
        .text_size(px(11.))
        .text_color(match state {
            NoteSaveState::Error => rgb(0xff7b72),
            NoteSaveState::Saving => rgb(0xe3b341),
            NoteSaveState::Saved => rgb(0x8e97aa),
        })
        .child(label)
        .into_any_element()
}

fn sidebar_icon_button(
    id: impl Into<SharedString>,
    icon: AppIcon,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(rgb(0x9da7b8))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x29303c)).text_color(rgb(0xe7ebf3)))
        .on_click(on_click)
        .child(Icon::new(icon).small())
        .into_any_element()
}

fn row_button(
    id: String,
    label: String,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .w_full()
        .px_2()
        .py_1()
        .rounded(px(6.))
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

fn tree_row_button(
    id: String,
    icon: AnyElement,
    label: String,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .min_w_0()
        .px_2()
        .py_1()
        .gap_2()
        .rounded(px(6.))
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_click)
        .child(icon)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label),
        )
        .into_any_element()
}

fn repository_tree_row(
    id: String,
    name: String,
    branch: Option<String>,
    additions: u64,
    deletions: u64,
    loading: bool,
    selected: bool,
    trailing: Option<AnyElement>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let trailing_id = SharedString::from(format!("{id}-trailing"));
    let branch = branch.filter(|branch| !branch.is_empty());
    let has_changes = additions > 0 || deletions > 0;
    let changes = h_flex()
        .flex_none()
        .gap_1()
        .text_size(px(10.))
        .when(additions > 0, |this| {
            this.child(
                div()
                    .text_color(rgb(0x56b881))
                    .child(format!("+{additions}")),
            )
        })
        .when(deletions > 0, |this| {
            this.child(
                div()
                    .text_color(rgb(0xd7656f))
                    .child(format!("−{deletions}")),
            )
        });
    let loading_placeholder = h_flex()
        .flex_none()
        .gap_1()
        .child(
            Skeleton::new()
                .w(px(24.))
                .h(px(7.))
                .rounded(px(2.))
                .bg(rgb(0x626d80)),
        )
        .child(
            Skeleton::new()
                .secondary()
                .w(px(18.))
                .h(px(7.))
                .rounded(px(2.))
                .bg(rgb(0x4d586a)),
        );

    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .h(px(28.))
        .min_w_0()
        .px_2()
        .gap_2()
        .justify_between()
        .rounded(px(6.))
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_click)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(Icon::new(AppIcon::GitBranch).small())
                .child(
                    div()
                        .min_w_0()
                        .max_w(px(150.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(name),
                )
                .when_some(branch, |this, branch| {
                    this.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(rgb(0x788294))
                            .child(branch),
                    )
                }),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .when(loading, |this| this.child(loading_placeholder))
                .when(!loading && has_changes, |this| this.child(changes))
                .when_some(trailing, |this, trailing| {
                    this.child(
                        div()
                            .id(trailing_id)
                            .flex_none()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(trailing),
                    )
                }),
        )
        .into_any_element()
}

fn orchestrator_tool_activity(
    name: &str,
    agent: Option<&str>,
    input: Option<&serde_json::Value>,
    fallback_agent: &str,
) -> OrchestratorChatActivity {
    let tool = name
        .strip_prefix("mcp__blackholes__")
        .unwrap_or(name)
        .replace('_', " ");
    let detail = input.and_then(|input| {
        let preferred = [
            "command",
            "file_path",
            "path",
            "pattern",
            "title",
            "description",
            "prompt",
            "taskId",
            "projectId",
        ]
        .iter()
        .find_map(|key| input.get(key))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| Some(value.to_string()))
        })
        .or_else(|| (!input.is_null()).then(|| input.to_string()))?;
        let preferred = preferred.replace(['\r', '\n'], " ");
        let truncated = preferred.chars().take(280).collect::<String>();
        Some(if preferred.chars().count() > 280 {
            format!("{truncated}…")
        } else {
            truncated
        })
    });
    OrchestratorChatActivity {
        agent: match agent {
            Some(agent) if agent != "black-bot" => agent,
            _ => fallback_agent,
        }
        .to_string(),
        tool,
        detail,
        created_at: Utc::now(),
        task_id: None,
        status: None,
        summary: None,
        background: false,
    }
}

fn black_bot_avatar(size: f32, color: AgentAvatarColor) -> AnyElement {
    let eye_width = (size * 0.09).max(2.0);
    let eye_height = (size * 0.22).max(4.0);
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size / 2.0))
        .bg(agent_avatar_color(color))
        .child(
            h_flex()
                .gap(px(size * 0.22))
                .child(
                    div()
                        .w(px(eye_width))
                        .h(px(eye_height))
                        .rounded(px(eye_width / 2.0))
                        .bg(rgb(0x17120a)),
                )
                .child(
                    div()
                        .w(px(eye_width))
                        .h(px(eye_height))
                        .rounded(px(eye_width / 2.0))
                        .bg(rgb(0x17120a)),
                ),
        )
        .into_any_element()
}

fn black_bot_avatar_with_status(size: f32, color: AgentAvatarColor, busy: bool) -> AnyElement {
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(black_bot_avatar(size, color))
        .when(busy, |this| {
            this.child(
                div()
                    .absolute()
                    .right(px(-1.))
                    .bottom(px(-1.))
                    .size(px((size * 0.25).clamp(6., 8.)))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(0x111318))
                    .bg(rgb(0x66ca91)),
            )
        })
        .into_any_element()
}

fn agent_working_dots() -> AnyElement {
    div()
        .flex_none()
        .text_size(px(10.))
        .text_color(rgb(0xa997ef))
        .child("•••")
        .into_any_element()
}

fn global_agent_card(
    id: impl Into<SharedString>,
    name: String,
    preview: String,
    selected: bool,
    busy: bool,
    avatar_color: AgentAvatarColor,
    removable: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_remove: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let remove_id = SharedString::from(format!("remove-{}", id.as_ref()));
    h_flex()
        .id(id)
        .w_full()
        .max_w_full()
        .h(px(52.))
        .flex_none()
        .min_w_0()
        .px_2()
        .gap_2()
        .rounded(px(8.))
        .border_1()
        .border_color(if selected {
            rgb(0x4a5364)
        } else {
            rgb(0x292e37)
        })
        .bg(if selected {
            rgb(0x2d3139)
        } else {
            rgb(0x1c1f25)
        })
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x292d35)).border_color(rgb(0x3c4350)))
        .on_click(on_click)
        .child(black_bot_avatar_with_status(32., avatar_color, busy))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(0xf1f2f5))
                                .child(name),
                        )
                        .when(busy, |this| this.child(agent_working_dots())),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(rgb(0x9da1aa))
                        .child(preview),
                ),
        )
        .child(h_flex().flex_none().gap_1().when(removable, |this| {
            this.child(
                div()
                    .id(remove_id)
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .text_color(rgb(0x8e97aa))
                    .hover(|style| style.bg(rgb(0x3a252b)).text_color(rgb(0xff7b72)))
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        on_remove(event, window, cx);
                    })
                    .child(Icon::new(AppIcon::X).with_size(px(12.))),
            )
        }))
        .into_any_element()
}

fn agent_chat_tree_row(
    id: String,
    name: String,
    selected: bool,
    busy: bool,
    avatar_color: AgentAvatarColor,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_remove: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let remove_id = SharedString::from(format!("remove-{id}"));
    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .h(px(28.))
        .min_w_0()
        .px_2()
        .gap_2()
        .rounded(px(6.))
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_click)
        .child(black_bot_avatar_with_status(18., avatar_color, busy))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().min_w_0().truncate().child(name))
                .when(busy, |this| this.child(agent_working_dots())),
        )
        .child(
            div()
                .id(remove_id)
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_color(rgb(0x8e97aa))
                .hover(|style| style.bg(rgb(0x3a252b)).text_color(rgb(0xff7b72)))
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_remove(event, window, cx);
                })
                .child(Icon::new(AppIcon::X).with_size(px(12.))),
        )
        .into_any_element()
}

fn terminal_tree_row(
    terminal_id: Uuid,
    label: String,
    agent: AgentKind,
    state: SessionState,
    selected: bool,
    on_select: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    h_flex()
        .id(SharedString::from(format!(
            "sidebar-terminal-{terminal_id}"
        )))
        .w_full()
        .min_w_0()
        .h(px(28.))
        .px_2()
        .gap_2()
        .justify_between()
        .rounded(px(6.))
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_select)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(
                    div()
                        .relative()
                        .size(px(16.))
                        .flex_none()
                        .child(agent_icon(agent))
                        .when(state == SessionState::Working, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .right(px(-1.))
                                    .bottom(px(-1.))
                                    .size(px(6.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(rgb(0x111318))
                                    .bg(rgb(0x66ca91)),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().min_w_0().truncate().child(label))
                        .when(state == SessionState::Working, |this| {
                            this.child(agent_working_dots())
                        }),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!(
                    "close-sidebar-terminal-{terminal_id}"
                )))
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_color(rgb(0x8e97aa))
                .hover(|style| style.bg(rgb(0x3a252b)).text_color(rgb(0xff7b72)))
                .on_click(on_close)
                .child(Icon::new(AppIcon::X).with_size(px(12.))),
        )
        .into_any_element()
}

/// Renders the `+` affordance that launches a coding agent in a specific
/// terminal target (project root, task worktree root, or one repository
/// worktree inside a task).
fn agent_launch_menu_button(
    id: String,
    tooltip: String,
    weak: WeakEntity<BlackholesApp>,
    workspace_id: Uuid,
    task_id: Option<Uuid>,
    repository_id: Option<Uuid>,
    add_bot_label: String,
    terminal_label: String,
) -> AnyElement {
    Button::new(SharedString::from(id))
        .icon(AppIcon::Plus)
        .ghost()
        .xsmall()
        .tooltip(tooltip)
        .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
            let weak_agent = weak.clone();
            let weak_terminal = weak.clone();
            menu.min_w(px(190.))
                .item(
                    PopupMenuItem::new(add_bot_label.clone())
                        .icon(AppIcon::Plus)
                        .on_click(move |_, window, cx| {
                            let weak = weak_agent.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    app.create_scoped_orchestrator_agent(workspace_id, task_id, cx)
                                });
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new(terminal_label.clone())
                        .icon(AppIcon::SquareTerminal)
                        .on_click(move |_, window, cx| {
                            let weak = weak_terminal.clone();
                            window.defer(cx, move |window, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    app.select_target(workspace_id, task_id, repository_id, cx);
                                    app.new_terminal(AgentKind::Shell, window, cx);
                                });
                            });
                        }),
                )
        })
        .into_any_element()
}

fn agent_icon(agent: AgentKind) -> AnyElement {
    let (icon, color) = match agent {
        AgentKind::Shell => (AppIcon::SquareTerminal, rgb(0xb6bdca)),
        AgentKind::Claude => (AppIcon::ClaudeCode, rgb(0xd97757)),
        AgentKind::Codex => (AppIcon::Codex, rgb(0xe7ecea)),
        AgentKind::Gemini => (AppIcon::Code2, rgb(0x5b8def)),
    };

    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .child(Icon::new(icon).with_size(px(14.)))
        .into_any_element()
}

fn agent_from_terminal_title(title: &str) -> Option<AgentKind> {
    let normalized = title.to_ascii_lowercase();
    let has_word = |needle: &str| {
        normalized
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == needle)
    };

    // Codex's default terminal title is the project name prefixed by one of these animated
    // Braille frames while it is working. It intentionally does not include the word "codex".
    let codex_activity_title = title.trim_start().chars().next().is_some_and(|character| {
        matches!(
            character,
            '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏'
        )
    }) || normalized.starts_with("[ ! ] action required")
        || normalized.starts_with("[ . ] action required");
    if has_word("codex") || codex_activity_title {
        return Some(AgentKind::Codex);
    }

    // Claude normally prefixes its title with a decorative glyph. Restrict detection to the
    // leading application name so a Codex thread mentioning "Claude Code" is not reclassified.
    let leading_title =
        normalized.trim_start_matches(|character: char| !character.is_ascii_alphanumeric());
    if leading_title.starts_with("claude code") {
        return Some(AgentKind::Claude);
    }
    if has_word("gemini") {
        return Some(AgentKind::Gemini);
    }
    None
}

fn show_native_agent_notification(title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let is_bundled_app = std::env::current_exe()
            .ok()
            .is_some_and(|path| path.to_string_lossy().contains(".app/Contents/MacOS/"));
        let bundle_identifier = if is_bundled_app {
            "dev.blackholes.rust"
        } else {
            // The legacy macOS backend requires a bundle registered with Launch
            // Services. Development binaries have no bundle, so use Terminal's
            // identity instead of letting the dependency look up `use_default`.
            "com.apple.Terminal"
        };
        #[allow(deprecated)]
        let _ = notify_rust::set_application(bundle_identifier);
    }

    let _ = notify_rust::Notification::new()
        .appname("Blackholes Rust")
        .summary(title)
        .body(message)
        .show();
}

fn play_agent_attention_sound() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .spawn();
    }
}

fn collapsible_tree_row(
    toggle_id: impl Into<SharedString>,
    row_id: String,
    icon: AnyElement,
    label: String,
    badge: Option<AnyElement>,
    selected: bool,
    expanded: bool,
    indentation: f32,
    label_color: Option<gpui::Rgba>,
    trailing: Option<AnyElement>,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_select: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let trailing_id = SharedString::from(format!("{row_id}-trailing"));
    div()
        .w_full()
        .pl(px(indentation))
        .child(
            h_flex()
                .id(SharedString::from(row_id))
                .w_full()
                .h(px(28.))
                .min_w_0()
                .px_1()
                .gap_1()
                .rounded(px(6.))
                .bg(if selected {
                    rgb(0x29364f)
                } else {
                    rgb(0x111318)
                })
                .text_color(label_color.unwrap_or(if selected {
                    rgb(0xdde8ff)
                } else {
                    rgb(0xb6bdca)
                }))
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(0x242a35)))
                .on_click(on_select)
                .child(
                    div()
                        .id(toggle_id.into())
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x9da7b8))
                        .on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            on_toggle(event, window, cx);
                        })
                        .child(
                            Icon::new(if expanded {
                                AppIcon::ChevronDown
                            } else {
                                AppIcon::ChevronRight
                            })
                            .with_size(px(13.)),
                        ),
                )
                .child(icon)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .when_some(badge, |this, badge| this.child(badge)),
                )
                .when_some(trailing, |this, trailing| {
                    this.child(
                        div()
                            .id(trailing_id)
                            .flex_none()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(trailing),
                    )
                }),
        )
        .into_any_element()
}

fn project_icon_kind(icon: &str) -> AppIcon {
    match icon {
        "folder" => AppIcon::Folder,
        "code" | "code-2" => AppIcon::Code2,
        "terminal" | "square-terminal" => AppIcon::SquareTerminal,
        "rocket" => AppIcon::Rocket,
        "database" => AppIcon::Database,
        "globe" => AppIcon::Globe,
        "list-todo" => AppIcon::ListTodo,
        "git-branch" => AppIcon::GitBranch,
        _ => AppIcon::Layers3,
    }
}

fn quick_open_icon_id(icon: AppIcon) -> &'static str {
    match icon {
        AppIcon::Code2 => "code",
        AppIcon::Database => "database",
        AppIcon::File => "file",
        AppIcon::Folder | AppIcon::FolderOpen => "folder",
        AppIcon::GitBranch => "branch",
        AppIcon::Globe => "globe",
        AppIcon::ListTodo => "list",
        AppIcon::Rocket => "rocket",
        AppIcon::SquareTerminal => "terminal",
        _ => "layers",
    }
}

fn quick_open_css_color(color: gpui::Rgba) -> String {
    format!(
        "rgba({}, {}, {}, {:.3})",
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
        color.a,
    )
}

fn file_tree_icon(
    kind: FileEntryKind,
    path: &std::path::Path,
    expanded: bool,
) -> (AppIcon, gpui::Rgba) {
    match kind {
        FileEntryKind::Directory => (
            if expanded {
                AppIcon::FolderOpen
            } else {
                AppIcon::Folder
            },
            rgb(0xd1b46f),
        ),
        FileEntryKind::Symlink => (AppIcon::File, rgb(0x8db3cf)),
        FileEntryKind::File => {
            let extension = path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            match extension.as_str() {
                "rs" => (AppIcon::Code2, rgb(0xd99084)),
                "js" | "jsx" | "ts" | "tsx" => (AppIcon::Code2, rgb(0xd1b46f)),
                "html" | "css" | "scss" | "vue" | "svelte" => (AppIcon::Code2, rgb(0x8db3cf)),
                "go" | "py" | "rb" | "java" | "kt" | "swift" | "c" | "cc" | "cpp" | "h" | "hpp"
                | "sh" => (AppIcon::Code2, rgb(0x96b39a)),
                "json" | "toml" | "yaml" | "yml" | "xml" => (AppIcon::File, rgb(0xa597c8)),
                "md" | "mdx" | "txt" => (AppIcon::File, rgb(0x9aa3ae)),
                _ => (AppIcon::File, rgb(0x8e97aa)),
            }
        }
    }
}

fn file_editor_language(path: &std::path::Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "bash" | "sh" | "zsh" => "bash",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "css" | "scss" => "css",
        "diff" | "patch" => "diff",
        "ex" | "exs" => "elixir",
        "go" => "go",
        "graphql" | "gql" => "graphql",
        "html" | "htm" | "vue" | "svelte" => "html",
        "java" | "kt" | "kts" => "java",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "json" | "jsonc" => "json",
        "md" | "mdx" => "markdown",
        "proto" => "proto",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "scala" => "scala",
        "sql" => "sql",
        "swift" => "swift",
        "toml" => "toml",
        "ts" | "dart" => "typescript",
        "tsx" => "tsx",
        "yaml" | "yml" => "yaml",
        "zig" => "zig",
        _ => match path.file_name().and_then(std::ffi::OsStr::to_str) {
            Some("CMakeLists.txt") => "cmake",
            Some("Dockerfile" | "Makefile") => "make",
            _ => "text",
        },
    }
}

fn project_icon_options(language: Language) -> Vec<(&'static str, &'static str, AppIcon)> {
    match language {
        Language::English => vec![
            ("layers", "Layers", AppIcon::Layers3),
            ("folder", "Folder", AppIcon::Folder),
            ("code-2", "Code", AppIcon::Code2),
            ("square-terminal", "Terminal", AppIcon::SquareTerminal),
            ("rocket", "Rocket", AppIcon::Rocket),
            ("database", "Database", AppIcon::Database),
            ("globe", "Globe", AppIcon::Globe),
            ("list-todo", "Tasks", AppIcon::ListTodo),
            ("git-branch", "Branch", AppIcon::GitBranch),
        ],
        Language::Spanish => vec![
            ("layers", "Capas", AppIcon::Layers3),
            ("folder", "Carpeta", AppIcon::Folder),
            ("code-2", "Código", AppIcon::Code2),
            ("square-terminal", "Terminal", AppIcon::SquareTerminal),
            ("rocket", "Cohete", AppIcon::Rocket),
            ("database", "Base de datos", AppIcon::Database),
            ("globe", "Globo", AppIcon::Globe),
            ("list-todo", "Tareas", AppIcon::ListTodo),
            ("git-branch", "Rama", AppIcon::GitBranch),
        ],
    }
}

fn project_colors() -> [WorkspaceColor; 9] {
    [
        WorkspaceColor::Slate,
        WorkspaceColor::Coral,
        WorkspaceColor::Peach,
        WorkspaceColor::Amber,
        WorkspaceColor::Sage,
        WorkspaceColor::Mint,
        WorkspaceColor::Sky,
        WorkspaceColor::Lavender,
        WorkspaceColor::Rose,
    ]
}

fn workspace_color(color: WorkspaceColor) -> gpui::Rgba {
    match color {
        WorkspaceColor::Slate => rgb(0x9aa3ae),
        WorkspaceColor::Coral => rgb(0xd99084),
        WorkspaceColor::Peach => rgb(0xdda57e),
        WorkspaceColor::Amber => rgb(0xd1b46f),
        WorkspaceColor::Sage => rgb(0x96b39a),
        WorkspaceColor::Mint => rgb(0x8db9ab),
        WorkspaceColor::Sky => rgb(0x8db3cf),
        WorkspaceColor::Lavender => rgb(0xa597c8),
        WorkspaceColor::Rose => rgb(0xc796aa),
    }
}

fn workspace_color_css(color: WorkspaceColor) -> &'static str {
    match color {
        WorkspaceColor::Slate => "#9aa3ae",
        WorkspaceColor::Coral => "#d99084",
        WorkspaceColor::Peach => "#dda57e",
        WorkspaceColor::Amber => "#d1b46f",
        WorkspaceColor::Sage => "#96b39a",
        WorkspaceColor::Mint => "#8db9ab",
        WorkspaceColor::Sky => "#8db3cf",
        WorkspaceColor::Lavender => "#a597c8",
        WorkspaceColor::Rose => "#c796aa",
    }
}

fn agent_avatar_color(color: AgentAvatarColor) -> gpui::Rgba {
    match color {
        AgentAvatarColor::Mercury => rgb(0xa5a4ab),
        AgentAvatarColor::Earthy => rgb(0x398ff4),
        AgentAvatarColor::Saturny => rgb(0xe1b36f),
    }
}

fn navigation_scope_id(scope: OrchestratorChatScope) -> String {
    match scope {
        OrchestratorChatScope::Global => "global".into(),
        OrchestratorChatScope::GlobalAgent(agent_id) => format!("global:{agent_id}"),
        OrchestratorChatScope::Project(workspace_id) => format!("project:{workspace_id}"),
        OrchestratorChatScope::ProjectAgent {
            project_id,
            agent_id,
        } => format!("project-agent:{project_id}:{agent_id}"),
        OrchestratorChatScope::Task(task_id) => format!("task:{task_id}"),
        OrchestratorChatScope::TaskAgent { task_id, agent_id } => {
            format!("task-agent:{task_id}:{agent_id}")
        }
    }
}

fn parse_navigation_scope(value: &str) -> Option<OrchestratorChatScope> {
    if value == "global" {
        return Some(OrchestratorChatScope::Global);
    }
    let mut parts = value.split(':');
    let kind = parts.next()?;
    let scope_id = Uuid::parse_str(parts.next()?).ok()?;
    match kind {
        "global" => Some(OrchestratorChatScope::GlobalAgent(scope_id)),
        "project" => Some(OrchestratorChatScope::Project(scope_id)),
        "task" => Some(OrchestratorChatScope::Task(scope_id)),
        "project-agent" => Some(OrchestratorChatScope::ProjectAgent {
            project_id: scope_id,
            agent_id: Uuid::parse_str(parts.next()?).ok()?,
        }),
        "task-agent" => Some(OrchestratorChatScope::TaskAgent {
            task_id: scope_id,
            agent_id: Uuid::parse_str(parts.next()?).ok()?,
        }),
        _ => None,
    }
}

fn with_alpha(mut color: gpui::Rgba, alpha: f32) -> gpui::Rgba {
    color.a = alpha;
    color
}

fn project_color_button(
    id: impl Into<SharedString>,
    color: WorkspaceColor,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(17.))
        .cursor_pointer()
        .when(selected, |style| {
            style
                .border_1()
                .border_color(rgb(0x697386))
                .bg(rgb(0x30343c))
        })
        .hover(|style| style.bg(rgb(0x30343c)))
        .on_click(on_click)
        .child(
            div()
                .size(px(22.))
                .rounded(px(11.))
                .bg(workspace_color(color))
                .when(selected, |style| {
                    style.border_1().border_color(rgb(0xc4cad4))
                }),
        )
        .into_any_element()
}

/// Renders the app name with tracking, which gpui text styles cannot express.
fn app_name_label(font_size: f32) -> AnyElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(font_size * APP_NAME_LETTER_SPACING_RATIO))
        .font_family(APP_NAME_FONT_FAMILY)
        .text_size(px(font_size))
        .font_weight(gpui::FontWeight(APP_NAME_FONT_WEIGHT))
        .children(
            APP_NAME
                .chars()
                .map(|letter| div().child(letter.to_string())),
        )
        .into_any_element()
}

fn field_label(label: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(px(11.))
        .text_color(rgb(0x8a93a6))
        .child(label.into())
        .into_any_element()
}

fn section_label(label: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(0x9aa4b6))
        .child(label.into())
        .into_any_element()
}

/// Read a text input, treating blank as absent.
fn non_empty_value(input: &Entity<InputState>, cx: &App) -> Option<String> {
    let value = input.read(cx).value().trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Whether the branch the check looked for already exists on the side that owns
/// the name for this source, which is what decides if a base is used at all.
fn branch_exists(result: &BranchAvailability, source: TaskBranchSource) -> bool {
    match source {
        TaskBranchSource::Remote => result.remote_revision.is_some(),
        _ => result.local_revision.is_some(),
    }
}

fn form_field(label: impl Into<SharedString>, control: impl IntoElement) -> AnyElement {
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(field_label(label))
        .child(control)
        .into_any_element()
}

fn form_divider() -> AnyElement {
    div()
        .w_full()
        .h(px(1.))
        .bg(rgb(0x222834))
        .into_any_element()
}

/// Rounded container that turns a set of `segmented_item`s into a tab-like control.
fn segmented_group() -> gpui::Div {
    h_flex()
        .w_full()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(9.))
        .bg(rgb(0x0f1116))
        .border_1()
        .border_color(rgb(0x232935))
}

fn segmented_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .py(px(6.))
        .rounded(px(6.))
        .border_1()
        .border_color(if selected {
            rgb(0x3d5480)
        } else {
            rgb(0x0f1116)
        })
        .bg(if selected {
            rgb(0x24304a)
        } else {
            rgb(0x0f1116)
        })
        .text_size(px(12.))
        .text_color(if selected {
            rgb(0xdfe8fb)
        } else {
            rgb(0x98a1b2)
        })
        .font_weight(if selected {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_ellipsis()
        .cursor_pointer()
        .hover(|style| {
            if selected {
                style
            } else {
                style.bg(rgb(0x181d26)).text_color(rgb(0xc4ccda))
            }
        })
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn check_indicator(checked: bool) -> AnyElement {
    div()
        .size(px(15.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if checked {
            rgb(0x5c7cfa)
        } else {
            rgb(0x39414f)
        })
        .bg(if checked {
            rgb(0x4c6ef5)
        } else {
            rgb(0x14171d)
        })
        .text_size(px(9.))
        .text_color(rgb(0xffffff))
        .child(if checked { "✓" } else { "" })
        .into_any_element()
}

fn option_row(
    id: String,
    label: impl Into<SharedString>,
    checked: bool,
    emphasized: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .items_center()
        .gap_2()
        .px(px(10.))
        .py(px(7.))
        .rounded(px(7.))
        .border_1()
        .border_color(if checked {
            rgb(0x33405c)
        } else {
            rgb(0x21262f)
        })
        .bg(if checked {
            rgb(0x171e2b)
        } else {
            rgb(0x101218)
        })
        .text_size(if emphasized { px(13.) } else { px(12.) })
        .text_color(if checked {
            rgb(0xdbe4f5)
        } else {
            rgb(0xa8b1c0)
        })
        .font_weight(if emphasized && checked {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x1a1f29)).border_color(rgb(0x3a4356)))
        .on_click(on_click)
        .child(check_indicator(checked))
        .child(div().flex_1().min_w_0().text_ellipsis().child(label.into()))
        .into_any_element()
}

fn choice_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .px_3()
        .py_2()
        .rounded(px(6.))
        .border_1()
        .border_color(if selected {
            rgb(0x5c7cfa)
        } else {
            rgb(0x303642)
        })
        .bg(if selected {
            rgb(0x29364f)
        } else {
            rgb(0x111318)
        })
        .text_color(if selected {
            rgb(0xdde8ff)
        } else {
            rgb(0xb6bdca)
        })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242a35)))
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn task_navigation_icon(task: &ProjectTask) -> AnyElement {
    Icon::new(project_icon_kind(&task.icon))
        .small()
        .into_any_element()
}

fn remove_terminal_node(node: DockNode, terminal_id: Uuid) -> Option<DockNode> {
    match node {
        DockNode::Panel {
            terminal_id: current,
        } => (current != terminal_id).then_some(DockNode::Panel {
            terminal_id: current,
        }),
        DockNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let first = remove_terminal_node(*first, terminal_id);
            let second = remove_terminal_node(*second, terminal_id);
            match (first, second) {
                (Some(first), Some(second)) => Some(DockNode::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            }
        }
    }
}

fn app_theme_id(theme: AppTheme) -> &'static str {
    match theme {
        AppTheme::Light => "light",
        AppTheme::Dark => "dark",
    }
}

fn terminal_config(theme: AppTheme) -> TerminalConfig {
    let colors = match theme {
        AppTheme::Dark => ColorPalette::builder()
            .background(0x0c, 0x0e, 0x12)
            .foreground(0xd8, 0xde, 0xea)
            .cursor(0xe5, 0xe9, 0xf0)
            .black(0x16, 0x18, 0x1d)
            .red(0xff, 0x7b, 0x72)
            .green(0x56, 0xd3, 0x64)
            .yellow(0xe3, 0xb3, 0x41)
            .blue(0x79, 0xc0, 0xff)
            .magenta(0xd2, 0xa8, 0xff)
            .cyan(0x56, 0xd4, 0xdd)
            .white(0xc9, 0xd1, 0xd9)
            .bright_black(0x6e, 0x76, 0x81)
            .bright_red(0xff, 0xa1, 0x98)
            .bright_green(0x7e, 0xe7, 0x87)
            .bright_yellow(0xf2, 0xcc, 0x60)
            .bright_blue(0xa5, 0xd6, 0xff)
            .bright_magenta(0xe2, 0xc5, 0xff)
            .bright_cyan(0x80, 0xe8, 0xee)
            .bright_white(0xf0, 0xf6, 0xfc)
            .build(),
        AppTheme::Light => ColorPalette::builder()
            .background(0xf8, 0xf9, 0xfb)
            .foreground(0x24, 0x2b, 0x38)
            .cursor(0x18, 0x21, 0x2f)
            .black(0x24, 0x2b, 0x38)
            .red(0xc7, 0x3e, 0x4e)
            .green(0x1f, 0x7a, 0x4f)
            .yellow(0x9a, 0x68, 0x10)
            .blue(0x2e, 0x63, 0xb8)
            .magenta(0x75, 0x4b, 0xa7)
            .cyan(0x1e, 0x76, 0x7b)
            .white(0xdf, 0xe3, 0xea)
            .bright_black(0x78, 0x82, 0x91)
            .bright_red(0xdd, 0x4f, 0x5f)
            .bright_green(0x2f, 0x91, 0x62)
            .bright_yellow(0xb3, 0x7d, 0x1b)
            .bright_blue(0x3f, 0x75, 0xcb)
            .bright_magenta(0x8b, 0x5d, 0xbb)
            .bright_cyan(0x2a, 0x8b, 0x90)
            .bright_white(0xff, 0xff, 0xff)
            .build(),
    };
    TerminalConfig {
        font_family: "Menlo".into(),
        font_size: px(13.0),
        cols: 120,
        rows: 30,
        scrollback: 20_000,
        line_height_multiplier: 1.12,
        padding: Edges::all(px(8.)),
        colors,
    }
}

fn insert_unique(ids: &mut Vec<Uuid>, id: Uuid) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}

fn toggle_id(ids: &mut Vec<Uuid>, id: Uuid) {
    if let Some(index) = ids.iter().position(|current| *current == id) {
        ids.remove(index);
    } else {
        ids.push(id);
    }
}

fn install_agent_command_bridge(paths: &AppPaths, cx: &mut Context<BlackholesApp>) -> Result<()> {
    let receiver = crate::services::agent_commands::listen(paths)?;
    cx.spawn(async move |this, cx| {
        while let Ok(command) = receiver.recv_async().await {
            let response = this.update(cx, |app, cx| -> Result<bool> {
                let payload = command.message.strip_prefix("agent-handoff:")
                    .ok_or_else(|| anyhow::anyhow!("Unsupported agent command"))?;
                let payload = serde_json::from_str::<AgentHandoffPayload>(payload)?;
                app.handle_agent_handoff(payload, cx)
            });
            let response = match response {
                Ok(Ok(started)) => serde_json::json!({
                    "accepted": true, "started": started, "queued": !started,
                }),
                Ok(Err(error)) => serde_json::json!({ "accepted": false, "error": error.to_string() }),
                Err(_) => serde_json::json!({ "accepted": false, "error": "The app closed before handling the command" }),
            };
            let _ = command.reply.send(response);
        }
    }).detach();
    Ok(())
}

fn install_event_bridge(paths: &AppPaths, cx: &mut Context<BlackholesApp>) -> Result<()> {
    if paths.events_socket.exists() {
        fs::remove_file(&paths.events_socket).with_context(|| {
            format!(
                "could not replace stale socket {}",
                paths.events_socket.display()
            )
        })?;
    }
    let socket = UnixDatagram::bind(&paths.events_socket)
        .with_context(|| format!("could not bind {}", paths.events_socket.display()))?;
    fs::set_permissions(&paths.events_socket, fs::Permissions::from_mode(0o600))?;
    let (sender, receiver) = flume::unbounded::<String>();
    std::thread::Builder::new()
        .name("blackholes-ai-bridge".into())
        .spawn(move || {
            // Agent handoffs carry the receiving agent's self-contained prompt.
            // An oversized datagram is truncated rather than split, so leave
            // room above the MCP's 16 KiB handoff limit.
            let mut buffer = [0_u8; 65_536];
            while let Ok(length) = socket.recv(&mut buffer) {
                if sender
                    .send(String::from_utf8_lossy(&buffer[..length]).into_owned())
                    .is_err()
                {
                    break;
                }
            }
        })?;

    cx.spawn(async move |this, cx| {
        while let Ok(message) = receiver.recv_async().await {
            if this
                .update(cx, |app, cx| app.handle_bridge_event(&message, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    Ok(())
}
