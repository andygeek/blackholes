use super::{
    apply_native_theme,
    navigation_webview::{self, NavigationCommand},
    workspace_webview::{self, WorkspaceCommand},
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
        notes::{
            ProjectInstructionsService, ProjectNoteService, ProjectTaskInstructionsService,
            TaskNoteService,
        },
        providers::{
            AgentAuthEvent, AgentAuthMode, AgentProvider, ProviderPlanUsage, PlanUsageWindow,
            refresh_agent_plan_usage, start_agent_authentication,
        },
        projects::{
            ProjectService, ProjectRepositoryMode, ProjectRepositorySource, RepositoryRemoval, RepositoryGitSummary, discover_repositories, repository_git_summary,
        },
        tasks::{
            AddTaskRepositoriesRequest, BranchAvailability, CreateTaskRequest,
            ExistingBranchAction, RemoveTaskRepositoriesRequest, RemovedTaskRepository,
            RepositoryPreparation, TaskBranchSource, TaskService,
        },
        terminal::{SharedChild, SharedMasterPty, TerminalService},
        task_agents::{StartTaskAgentsRequest, TaskAgentRequest, implementation_prompt},
    },
};
use anyhow::{Context as _, Result};
use chrono::Utc;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Corner, Edges, Entity, Focusable as _, IntoElement, KeyBinding,
    KeyDownEvent, ListSizingBehavior, ParentElement, Render, ScrollHandle, SharedString, Timer,
    WeakEntity, Window, div, img, prelude::*, px, rgb, rgba, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Icon, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState, TabSize},
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    resizable::ResizableState,
    scroll::ScrollableElement as _,
    skeleton::Skeleton,
    v_flex,
};
use gpui_terminal::{ColorPalette, TerminalConfig};
use notify::{EventKind, RecursiveMode, Watcher as _, event::ModifyKind};
use parking_lot::Mutex;
use portable_pty::PtySize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    os::unix::{fs::PermissionsExt as _, net::UnixDatagram},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

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

impl Drop for AgentAuthentication {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
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
struct NavigationLinkPayload {
    scope: String,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AppToastTarget {
    Terminal { terminal_id: Uuid, agent: AgentKind },
    Task { task_id: Uuid },
}

impl AppToastTarget {
    fn element_key(self) -> String {
        match self {
            Self::Terminal { terminal_id, .. } => format!("terminal-{terminal_id}"),
            Self::Task { task_id } => format!("task-{task_id}"),
        }
    }

    fn terminal_id(self) -> Option<Uuid> {
        match self {
            Self::Terminal { terminal_id, .. } => Some(terminal_id),
            Self::Task { .. } => None,
        }
    }

    fn task_id(self) -> Option<Uuid> {
        match self {
            Self::Task { task_id } => Some(task_id),
            Self::Terminal { .. } => None,
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


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickOpenMode {
    Navigation,
    Files,
}

#[derive(Clone)]
enum QuickOpenTarget {
    Terminal { terminal_id: Uuid },
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

    terminal_provider: Option<AgentKind>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentRemovalTarget {
    Terminal(Uuid),
}

pub struct BlackholesApp {
    update_state: crate::services::updater::UpdateState,
    external_integrations: crate::services::external_integrations::IntegrationStatus,
    paths: AppPaths,
    database: Database,
    app_logo: Arc<gpui::RenderImage>,
    navigation_webview: Option<Entity<gpui_component::webview::WebView>>,
    workspace_webview: Option<Entity<gpui_component::webview::WebView>>,
    project_modal_request: Option<Uuid>,
    project_appearance_request: Option<(Uuid, Uuid)>,
    repository_modal_workspace: Option<Uuid>,
    repository_removal: Option<(Uuid, Uuid, Uuid, RepositoryRemoval)>,
    project_modal_submitting: bool,
    project_modal_sources: Vec<PathBuf>,
    task_modal_request: Option<(Uuid, Uuid)>,
    task_removal_confirmation: Option<Uuid>,
    agent_removal_confirmation: Option<AgentRemovalTarget>,
    task_modal_submitting: bool,
    agent_authentication: Option<AgentAuthentication>,
    workspaces: Vec<Workspace>,
    tasks: Vec<ProjectTask>,
    session: AppSession,
    terminals: HashMap<Uuid, TerminalHandle>,
    task_details_dirty: HashSet<Uuid>,
    task_legacy_notes: HashMap<Uuid, String>,
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
    show_task_details: bool,
    show_project_overview: bool,
    show_settings: bool,
    settings_return_view: Option<(bool, bool, bool, Option<Uuid>)>,
    plan_usage_refreshing: bool,
    plan_usage_refresh_error: bool,
    active_plan_usage: Option<ProviderPlanUsage>,
    plan_usage_updated_at: Option<chrono::DateTime<Utc>>,
    plan_usage_generation: u64,
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
        session.agent_order.retain(|id| id.strip_prefix("terminal:")
            .and_then(|id| Uuid::parse_str(id).ok())
            .is_some_and(|id| session.terminals.iter().any(|terminal| terminal.id == id)));
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
        if let Err(error) = install_agent_command_bridge(&paths, window, cx) {
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

        let show_task_details = false;
        let file_explorer_resize = cx.new(|_| ResizableState::default());
        let app_logo = gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../assets/app-logo-transparent.png").to_vec(),
        )
        .to_image_data(cx.svg_renderer())
        .expect("the embedded application logo must be a valid PNG");
        let (workspace_command_sender, workspace_command_receiver) = flume::unbounded();
        let workspace_webview =
            match workspace_webview::create(window, cx, workspace_command_sender) {
                Ok(webview) => Some(webview),
                Err(error) => {
                    status = Some((
                        format!("The workspace is unavailable: {error:#}"),
                        true,
                    ));
                    None
                }
            };
        let workspace_window = window.window_handle();
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
        let notification_window = window.window_handle();
        let notification_receiver = crate::services::notifications::subscribe();
        let mut app = Self {
            update_state: crate::services::updater::state(),
            external_integrations: Default::default(),
            paths,
            database,
            app_logo,
            navigation_webview,
            workspace_webview,
            project_modal_request: None,
            project_appearance_request: None,
            repository_modal_workspace: None,
            repository_removal: None,
            project_modal_submitting: false,
            project_modal_sources: Vec::new(),
            task_modal_request: None,
            task_removal_confirmation: None,
            agent_removal_confirmation: None,
            task_modal_submitting: false,
            agent_authentication: None,
            workspaces,
            tasks,
            session,
            terminals: HashMap::new(),
            task_details_dirty: HashSet::new(),
            task_legacy_notes: HashMap::new(),
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
            show_task_details,
            show_project_overview: false,
            show_settings: !crate::services::projects::git_tools_available(),
            settings_return_view: None,
            plan_usage_refreshing: false,
            plan_usage_refresh_error: false,
            active_plan_usage: None,
            plan_usage_updated_at: None,
            plan_usage_generation: 0,
            project_settings_workspace_id: None,
            app_toasts: Vec::new(),
            status,
            status_revision: 0,
            busy: None,
        };
        cx.spawn(async move |this, cx| {
            while let Ok(raw_command) = workspace_command_receiver.recv_async().await {
                if workspace_window
                    .update(cx, |_, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.handle_workspace_command(&raw_command, window, cx)
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
        cx.spawn(async move |this, cx| {
            while let Ok(payload) = notification_receiver.recv_async().await {
                if notification_window
                    .update(cx, |_, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            if let Ok(target) = serde_json::from_str::<AppToastTarget>(&payload) {
                                app.open_toast_target(target, window, cx);
                            }
                        });
                        // Even an old notice with no surviving target opens the
                        // app. This never runs when merely receiving a notice.
                        crate::services::notifications::activate();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        app.refresh_external_integrations(cx);
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

    fn refresh_external_integrations(&mut self, cx: &mut Context<Self>) {
        if self.external_integrations.running { return; }
        self.external_integrations.running = true;
        let paths = self.paths.clone();
        self.hydrate_active_workspace_surface(cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                crate::services::external_integrations::synchronize(&paths)
            }).await;
            let _ = this.update(cx, |app, cx| {
                app.external_integrations = result;
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
            });
        }).detach();
    }

    fn sync_update_guard(&self) {
        let blocked = self.busy.is_some()
            || self.task_modal_request.is_some()
            || self.task_removal_confirmation.is_some()
            || self.agent_removal_confirmation.is_some()
            || self.project_modal_request.is_some()
            || self.project_appearance_request.is_some()
            || !self.terminals.is_empty()
            || self.agent_authentication.is_some()
            || self.active_file.as_ref().is_some_and(|file| file.dirty || file.save_state != NoteSaveState::Saved)
            || !self.task_details_dirty.is_empty();
        crate::services::updater::set_blocked(blocked, self.session.language == Language::Spanish);
    }

    fn check_app_update(&mut self, cx: &mut Context<Self>) {
        self.sync_update_guard();
        if !self.update_state.enabled {
            let message = if self.update_state.error.is_empty() {
                self.tr(
                "Updates are available in the packaged release app. See GitHub Releases for signed downloads.",
                "Las actualizaciones funcionan en la app empaquetada. Consulta GitHub Releases para las descargas firmadas.",
                ).to_string()
            } else {
                format!("{}: {}", self.tr("Updater unavailable", "Actualizador no disponible"), self.update_state.error)
            };
            self.set_status(message, true, cx);
            return;
        }
        if let Err(error) = self.database.save_session(&self.session) {
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

    fn handle_workspace_command(
        &mut self,
        raw_command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = match serde_json::from_str::<WorkspaceCommand>(raw_command) {
            Ok(command) => command,
            Err(error) => {
                tracing::warn!(?error, "ignored malformed workspace command");
                return;
            }
        };
        match command {
            WorkspaceCommand::Ready => {

                self.hydrate_active_workspace_surface(cx);
                self.hydrate_workspace_status(cx);
            }
            WorkspaceCommand::CopyText { text } => {
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            WorkspaceCommand::OpenUrl { url } => {
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:")
                    || url.starts_with("tel:")
                {
                    cx.open_url(&url);
                }
            }
            WorkspaceCommand::SetLanguage { language } => self.set_language(
                if language == "es" {
                    Language::Spanish
                } else {
                    Language::English
                },
                cx,
            ),
            WorkspaceCommand::SetTheme { theme } => self.set_theme(theme, cx),
            WorkspaceCommand::SetAgentProvider { provider } => {
                self.set_agent_provider(provider, cx)
            }
            WorkspaceCommand::CloseSettings => self.close_settings(cx),
            WorkspaceCommand::SetSidebarWidth { width, commit } => self.set_sidebar_width(width, commit, cx),
            WorkspaceCommand::RefreshPlanUsage => self.refresh_plan_usage(cx),
            WorkspaceCommand::InstallGitTools => {
                #[cfg(target_os = "macos")]
                if let Err(error) = std::process::Command::new("/usr/bin/xcode-select").arg("--install").spawn() {
                    self.set_status(format!("Could not open Apple's tools installer: {error}"), true, cx);
                }
            }
            WorkspaceCommand::RefreshRuntimeStatus => self.hydrate_active_workspace_surface(cx),
            WorkspaceCommand::RefreshExternalIntegrations => self.refresh_external_integrations(cx),
            WorkspaceCommand::SetAgentAuthMode { auth_mode } => {
                self.set_agent_auth_mode(self.agent_provider(), auth_mode, cx)
            }
            WorkspaceCommand::AuthenticateAgentProvider => {
                self.authenticate_agent_provider(self.agent_provider(), window, cx)
            }
            WorkspaceCommand::SubmitAgentAuth { value } => {
                self.submit_agent_auth_value(value, cx)
            }
            WorkspaceCommand::CancelAgentAuth => self.cancel_agent_authentication(cx),
            WorkspaceCommand::DismissAppModal => {
                self.dismiss_app_modal(cx);
                if self.show_terminal && self.project_modal_request.is_none() && self.task_modal_request.is_none() {
                    if let Some(terminal_id) = self.selected_terminal_id() {
                        self.focus_terminal_input(terminal_id, window, cx);
                    }
                }
            }
            WorkspaceCommand::CreateTaskModal { request_id, workspace_id, request, check_only } => {
                self.handle_create_task_modal(request_id, workspace_id, request, check_only, cx);
            }
            WorkspaceCommand::ChooseProjectModalFolder { request_id } => {
                if self.project_modal_request != Some(request_id) || self.project_modal_submitting || self.repository_removal.is_some() {
                    return;
                }
                let path = rfd::FileDialog::new()
                    .set_title(self.tr("Choose a repository or a folder containing repositories", "Elegir un repositorio o una carpeta con repositorios"))
                    .pick_folder();
                if let Some(path) = path {
                    self.project_modal_submitting = true;
                    let scan = cx.background_executor().spawn(async move {
                        let path = fs::canonicalize(path)?;
                        discover_repositories(&path)
                    });
                    cx.spawn(async move |this, cx| {
                        let result = scan.await;
                        let _ = this.update(cx, |app, cx| {
                            app.project_modal_submitting = false;
                            if app.project_modal_request != Some(request_id) { return; }
                            let feedback = match result {
                                Ok(repositories) if !repositories.is_empty() => {
                                    for repo in &repositories {
                                        if !app.project_modal_sources.contains(&repo.path) {
                                            app.project_modal_sources.push(repo.path.clone());
                                        }
                                    }
                                    serde_json::json!({ "repositories": repositories.iter().map(|repo| serde_json::json!({
                                        "name": repo.name, "path": repo.path,
                                    })).collect::<Vec<_>>() })
                                }
                                Ok(_) => serde_json::json!({ "error": app.tr("No Git repositories found in this folder or its immediate subfolders.", "No se encontraron repositorios Git en esta carpeta ni en sus subcarpetas inmediatas.") }),
                                Err(error) => serde_json::json!({ "error": format!("{error:#}") }),
                            };
                            app.dispatch_workspace_event(serde_json::json!({
                                "type": "app_modal_feedback", "request_id": request_id, "feedback": feedback,
                            }), cx);
                            cx.notify();
                        });
                    }).detach();
                } else {
                    self.dispatch_workspace_event(serde_json::json!({
                        "type": "app_modal_feedback", "request_id": request_id, "feedback": {},
                    }), cx);
                }
            }
            WorkspaceCommand::SubmitCreateProject { request_id, name, sources, mode } => {
                self.submit_create_project_modal(request_id, name, sources, mode, cx);
            }
            WorkspaceCommand::SubmitEditProject { request_id, workspace_id, name, icon, color } => {
                if self.project_appearance_request != Some((request_id, workspace_id)) {
                    return;
                }
                if self.update_project_presentation(workspace_id, name, icon, color, cx) {
                    self.dismiss_app_modal(cx);
                    if self.show_terminal {
                        if let Some(terminal_id) = self.selected_terminal_id() {
                            self.focus_terminal_input(terminal_id, window, cx);
                        }
                    }
                } else {
                    let error = self.status.as_ref().map(|(message, _)| message.clone())
                        .unwrap_or_else(|| self.tr("Could not update the project.", "No se pudo actualizar el proyecto.").to_string());
                    self.dispatch_workspace_event(serde_json::json!({
                        "type": "app_modal_feedback", "request_id": request_id,
                        "feedback": { "error": error },
                    }), cx);
                }
            }
            WorkspaceCommand::SubmitAddRepositories { request_id, workspace_id, sources, mode } => {
                self.submit_add_repositories(request_id, workspace_id, sources, mode, cx);
            }
            WorkspaceCommand::ConfirmRemoveRepository { request_id } => {
                self.confirm_remove_repository(request_id, cx);
            }
            WorkspaceCommand::ConfirmRemoveProject { workspace_id } => {
                self.remove_project_reference(workspace_id, cx);
                self.dismiss_app_modal(cx);
            }
            WorkspaceCommand::ConfirmCloseTerminal { terminal_id } => {
                if self.agent_removal_confirmation == Some(AgentRemovalTarget::Terminal(terminal_id)) {
                    self.close_terminal(terminal_id, cx);
                    self.dismiss_app_modal(cx);
                }
            }
            WorkspaceCommand::ConfirmRemoveTask { task_id } => {
                if self.task_removal_confirmation == Some(task_id) {
                    self.start_remove_task(task_id, cx);
                    self.dismiss_app_modal(cx);
                }
            }
            WorkspaceCommand::RevealProjectsRoot => self.reveal_projects_root(cx),
            WorkspaceCommand::ChooseProjectsRoot => {
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
            WorkspaceCommand::SetProjectTerminalSkipPermissions { workspace_id, enabled } => {
                if self.workspaces.iter().any(|workspace| workspace.id == workspace_id) {
                    let result = self.database.set_setting(
                        &format!("project-terminal-skip-permissions-{workspace_id}"),
                        if enabled { "true" } else { "false" },
                    );
                    if let Err(error) = result {
                        self.set_status(format!("Could not save terminal permissions: {error:#}"), true, cx);
                    }
                    self.hydrate_project_settings_surface(workspace_id, cx);
                    cx.notify();
                }
            }
            WorkspaceCommand::UpdateProjectInstructions {
                workspace_id,
                content,
            } => self.update_project_instructions(workspace_id, content, cx),
            WorkspaceCommand::UpdateProjectTaskInstructions {
                workspace_id,
                content,
            } => self.update_project_task_instructions(workspace_id, content, cx),
            WorkspaceCommand::SaveTaskDetails { task_id, request_id, patch } => {
                let result = self.tasks.iter().find(|task| task.id == task_id)
                    .and_then(|task| self.workspaces.iter().find(|workspace| workspace.id == task.workspace_id))
                    .ok_or_else(|| anyhow::anyhow!("Task is no longer available"))
                    .and_then(|workspace| crate::services::task_details::update(&self.database, workspace, &TaskService::new(&self.paths), task_id, &patch));
                match result {
                    Ok(task) => {
                        if let Some(current) = self.tasks.iter_mut().find(|current| current.id == task_id) { *current = task.clone(); }
                        self.task_details_dirty.remove(&task_id);
                        self.dispatch_workspace_event(serde_json::json!({"type":"task_details_saved", "task_id":task_id,
                            "request_id":request_id, "details":crate::services::task_details::metadata(&task),
                            "revision":crate::services::task_details::revision(&task)}), cx);
                        self.hydrate_active_workspace_surface(cx);
                        self.hydrate_navigation(cx);
                        cx.notify();
                    }
                    Err(error) => {
                        if let Ok(tasks) = self.database.all_tasks() { self.tasks = tasks; }
                        self.dispatch_workspace_event(serde_json::json!({"type":"task_details_saved", "task_id":task_id,
                            "request_id":request_id, "error":format!("{error:#}")}), cx);
                        self.hydrate_active_workspace_surface(cx);
                    }
                }
            }
            WorkspaceCommand::TaskDetailsDirty { task_id, dirty } => {
                if dirty { self.task_details_dirty.insert(task_id); } else { self.task_details_dirty.remove(&task_id); }
                self.sync_update_guard();
            }
            WorkspaceCommand::AddTaskRepositories { task_id } => self.open_add_task_repositories(task_id, window, cx),
            WorkspaceCommand::RemoveTaskRepositories { task_id } => self.open_remove_task_repositories(task_id, window, cx),
            WorkspaceCommand::FocusTerminal { terminal_id } => self.focus_terminal(terminal_id, window, cx),
            WorkspaceCommand::OpenTaskDetails { workspace_id, task_id } => self.show_task_details_for(workspace_id, task_id, cx),
            WorkspaceCommand::OpenProjectRepository { workspace_id, repository_id } => self.select_repository_target(workspace_id, None, repository_id, cx),
            WorkspaceCommand::RefreshFileExplorer => self.refresh_file_explorer(cx),
            WorkspaceCommand::CloseFileExplorer => self.close_file_explorer(cx),
            WorkspaceCommand::SetFileExplorerMode { mode } => self.set_file_explorer_mode(
                if mode == "changes" {
                    FileExplorerMode::Changes
                } else {
                    FileExplorerMode::Files
                },
                cx,
            ),
            WorkspaceCommand::ActivateFileRow {
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
            WorkspaceCommand::OpenRepositoryDiff { relative_path } => {
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
            WorkspaceCommand::CloseRepositoryDiff => self.close_repository_diff(cx),
            WorkspaceCommand::UpdateFileContent {
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
            WorkspaceCommand::SaveActiveFile => self.flush_active_file(cx),
            WorkspaceCommand::CloseFileEditor => self.close_file_editor(cx),
            WorkspaceCommand::OpenProjectInstructions { workspace_id } => {
                self.open_project_instructions(workspace_id, cx)
            }
            WorkspaceCommand::OpenProjectTaskInstructions { workspace_id } => {
                self.open_project_task_instructions(workspace_id, cx)
            }
            WorkspaceCommand::QuickOpenPaste { open_id, request_id } => {
                if self.quick_open.as_ref().map(|state| state.id) == Some(open_id) {
                    let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
                    self.dispatch_workspace_event(serde_json::json!({
                        "type": "quick_open_paste", "open_id": open_id,
                        "request_id": request_id, "text": text,
                    }), cx);
                }
            }
            WorkspaceCommand::QuickOpenQueryChanged { open_id, query } => {
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
            WorkspaceCommand::QuickOpenActivate {
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
                self.activate_quick_open_target(target, window, cx);
            }
            WorkspaceCommand::QuickOpenDismiss { open_id } => {
                if self.quick_open.as_ref().map(|state| state.id) == Some(open_id) {
                    self.close_quick_open(cx);
                    if self.show_terminal && let Some(id) = self.selected_terminal_id() {
                        self.focus_terminal_input(id, window, cx);
                    }
                }
            }
            WorkspaceCommand::DismissStatus => {
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
            NavigationCommand::ShowHome => self.show_home(cx),
            NavigationCommand::SetSidebarWidth { width, commit } => self.set_sidebar_width(width, commit, cx),
            NavigationCommand::CollapseAll => self.collapse_all_navigation(cx),
            NavigationCommand::NewProject => self.open_create_project(window, cx),
            NavigationCommand::AddProjectRepository { workspace_id } => {
                self.open_add_project_repository(workspace_id, window, cx)
            }
            NavigationCommand::RemoveRepository { workspace_id, repository_id } => {
                self.open_remove_repository(workspace_id, repository_id, cx)
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
                self.request_close_terminal(terminal_id, window, cx)
            }
            NavigationCommand::ReorderAgents { ids } => {
                let mut seen = HashSet::new();
                self.session.agent_order = ids.into_iter().filter(|id| {
                    let exists = if let Some(id) = id.strip_prefix("terminal:").and_then(|id| Uuid::parse_str(id).ok()) {
                        self.session.terminals.iter().any(|terminal| terminal.id == id && terminal.agent != AgentKind::Shell)
                    } else { false };
                    exists && seen.insert(id.clone())
                }).collect();
                self.persist_session();
                self.hydrate_navigation(cx);
                cx.notify();
            }
            NavigationCommand::ShowSettings => self.show_settings(cx),
        }
    }

    fn dispatch_workspace_event(&self, mut event: serde_json::Value, cx: &mut Context<Self>) {
        event["language"] = serde_json::json!(if self.session.language == Language::English { "en" } else { "es" });
        if event.get("sidebar_width").is_none() {
            event["sidebar_width"] = serde_json::json!(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX));
        }
        let Some(webview) = &self.workspace_webview else {
            return;
        };
        if let Err(error) = workspace_webview::dispatch(webview, event, cx) {
            tracing::warn!(?error, "failed to update workspace");
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

    fn navigation_terminal(
        &self,
        terminal: &TerminalDescriptor,
        active_terminal_id: Option<Uuid>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": terminal.id,
            "label": terminal.label,
            "agent": terminal.agent,
            "provider_label": terminal.agent.label(),
            "context": self.workspaces.iter()
                .find(|workspace| workspace.id == terminal.workspace_id)
                .map(|workspace| {
                    let project = workspace.label();
                    terminal.task_id
                        .and_then(|id| self.tasks.iter().find(|task| task.id == id))
                        .map(|task| format!("{project}/{}", task.title))
                        .unwrap_or_else(|| project.to_string())
                })
                .unwrap_or_else(|| terminal.cwd.display().to_string()),
            "selected": active_terminal_id == Some(terminal.id),
        })
    }

    fn hydrate_navigation(&self, cx: &mut Context<Self>) {
        let active_terminal_id = self
            .show_terminal
            .then(|| self.selected_terminal_id())
            .flatten();
        let mut projects = Vec::with_capacity(self.workspaces.len());
        for workspace in &self.workspaces {
            let workspace_id = workspace.id;
            let expanded = self.session.expanded_workspace_ids.contains(&workspace_id);
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
                    "blankTerminal": self.tr("Blank Terminal", "Terminal vacía"),
                    "tasks": "Tasks",
                    "task": "Task",
                    "new": "New",
                    "toggle": "Expand or collapse",
                    "options": "Options",
                    "closeTerminal": "Close terminal",
                    "newTerminal": "New terminal",
                    "newTask": "Add task",
                    "refreshProject": "Find new repositories",
                    "addToProject": "Add to project",
                    "addRepository": "Add repository…",
                    "removeRepository": "Remove repository",
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
                    "blankTerminal": self.tr("Blank Terminal", "Terminal vacía"),
                    "tasks": "Tareas",
                    "task": "Tarea",
                    "new": "Nuevo",
                    "toggle": "Expandir o contraer",
                    "options": "Opciones",
                    "closeTerminal": "Cerrar terminal",
                    "newTerminal": "Nueva terminal",
                    "newTask": "Agregar tarea",
                    "refreshProject": "Buscar repositorios nuevos",
                    "addToProject": "Agregar al proyecto",
                    "addRepository": "Agregar repositorio…",
                    "removeRepository": "Eliminar repositorio",
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

                "agent_order": self.session.agent_order,
                "terminal_agents": self.session.terminals.iter()
                    .filter(|terminal| terminal.agent != AgentKind::Shell)
                    .map(|terminal| self.navigation_terminal(terminal, active_terminal_id))
                    .collect::<Vec<_>>(),
                "projects": projects,
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
        let mut usage_cards = vec![serde_json::json!({
            "label": self.tr("Plan", "Plan"),
            "value": provider_plan_name(provider, plan_usage, self.session.language),
            "detail": provider_plan_detail(plan_usage, self.session.language),
            "utilization": serde_json::Value::Null,
        })];
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
                let (value, detail, utilization) = plan_limit_display(window, self.session.language);
                usage_cards.insert(index + 1, serde_json::json!({
                    "label": label, "value": value, "detail": detail, "utilization": utilization,
                }));
            }
        }
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
        self.dispatch_workspace_event(
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
                    "external_integrations": self.external_integrations,
                    "usage_cards": usage_cards,
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
        self.dispatch_workspace_event(
            serde_json::json!({
                "type": "workspace_surface",
                "surface": "project-settings",
                "theme": app_theme_id(self.session.theme),
                "data": {
                    "language": match self.session.language { Language::English => "en", Language::Spanish => "es" },
                    "theme": app_theme_id(self.session.theme),
                    "workspace_id": workspace.id,
                    "title": workspace.label(),
                    "terminal_skip_permissions": self.project_terminal_skip_permissions(workspace.id),
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


    fn hydrate_task_details_surface(&self, task_id: Uuid, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else { return; };
        let workspace = self.workspaces.iter().find(|workspace| workspace.id == task.workspace_id);
        let terminals = self.session.terminals.iter().filter(|terminal| terminal.task_id == Some(task_id))
            .map(|terminal| serde_json::json!({"id":terminal.id,"label":terminal.label,"agent":terminal.agent})).collect::<Vec<_>>();
        let repositories = task.repositories.iter().map(|attached| serde_json::json!({
            "id": attached.repository_id, "branch":attached.branch,
            "name": workspace.and_then(|w| w.repositories.iter().find(|r| r.id == attached.repository_id)).map(|r| &r.name)
        })).collect::<Vec<_>>();
        self.dispatch_workspace_event(serde_json::json!({
            "type":"workspace_surface", "surface":"task-details", "theme":app_theme_id(self.session.theme),
            "data": {"language":if self.session.language == Language::English {"en"} else {"es"},
                "id":task.id, "workspace_id":task.workspace_id, "project":workspace.map(|w|w.label()),
                "details":crate::services::task_details::metadata(task), "revision":crate::services::task_details::revision(task),
                "legacy_note":self.task_legacy_notes.get(&task_id),
                "repositories":repositories, "terminals":terminals}
        }),cx);
    }

    fn hydrate_project_overview_surface(&self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspaces.iter().find(|workspace| workspace.id == workspace_id) else { return; };
        let terminals = self.session.terminals.iter().filter(|terminal| terminal.workspace_id == workspace_id)
            .map(|terminal| serde_json::json!({"id":terminal.id,"label":terminal.label,"agent":terminal.agent})).collect::<Vec<_>>();
        let tasks = self.tasks.iter().filter(|task|task.workspace_id == workspace_id)
            .map(|task|serde_json::json!({"id":task.id,"title":task.title})).collect::<Vec<_>>();
        self.dispatch_workspace_event(serde_json::json!({
            "type":"workspace_surface", "surface":"project-overview", "theme":app_theme_id(self.session.theme),
            "data":{"language":if self.session.language == Language::English {"en"} else {"es"},
                "id":workspace_id,"title":workspace.label(),"terminals":terminals,"tasks":tasks,
                "repositories":workspace.repositories.iter().map(|r|serde_json::json!({"id":r.id,"name":r.name})).collect::<Vec<_>>()}
        }),cx);
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
        self.dispatch_workspace_event(
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
        self.dispatch_workspace_event(
            serde_json::json!({
                "type": "workspace_status",
                "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
                "message": self.status.as_ref().map(|(message, _)| message),
                "error": self.status.as_ref().is_some_and(|(_, error)| *error),
            }),
            cx,
        );
    }

    fn show_home(&mut self, cx: &mut Context<Self>) {
        self.flush_active_file(cx);
        self.show_terminal = false;
        self.show_task_details = false;
        self.show_project_overview = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.active_file = None;
        self.active_diff = None;
        self.quick_open = None;
        self.file_explorer.open = false;
        self.hydrate_home_surface(cx);
        cx.notify();
    }

    fn hydrate_home_surface(&self, cx: &mut Context<Self>) {
        self.dispatch_workspace_event(serde_json::json!({
            "type": "workspace_surface", "surface": "home",
            "theme": app_theme_id(self.session.theme),
            "language": if self.session.language == Language::English { "en" } else { "es" },
            "sidebar_width": self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            "data": {
                "title": self.tr("Your agent workspace", "Tu espacio de trabajo para agentes"),
                "description": self.tr(
                    "Open a project or task and use its + menu to start a terminal or coding agent.",
                    "Abre un proyecto o una tarea y usa su menú + para iniciar una terminal o un agente de código."),
            },
        }), cx);
    }

    fn hydrate_active_workspace_surface(&self, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.project_settings_workspace_id {
            self.hydrate_project_settings_surface(workspace_id, cx);
        } else if self.show_settings {
            self.hydrate_settings_surface(cx);
        } else if self.show_project_overview {
            if let Some(workspace_id) = self.session.selected_workspace_id {
                self.hydrate_project_overview_surface(workspace_id, cx);
            }
        } else if self.show_task_details {
            if let Some(task_id) = self.session.selected_task_id {
                self.hydrate_task_details_surface(task_id, cx);
            }
        } else if !self.show_terminal
            && (self.file_explorer.open || self.active_file.is_some() || self.active_diff.is_some())
        {
            self.hydrate_workbench_surface(cx);
        } else if !self.show_terminal {
            self.hydrate_home_surface(cx);
        }
    }

    fn publish_workbench_surface(&self, cx: &mut Context<Self>) {
        if !self.show_settings
            && self.project_settings_workspace_id.is_none()
            && !self.show_project_overview
            && !self.show_task_details
            && !self.show_terminal
            && (self.file_explorer.open || self.active_file.is_some() || self.active_diff.is_some())
        {
            self.hydrate_workbench_surface(cx);
        }
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
            .tr("Search projects, tasks, agents and terminals…", "Buscar proyectos, tareas, agentes y terminales…")
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

    fn show_quick_open_overlay(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.hydrate_quick_open_overlay(cx);
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": true }),
            cx,
        );
        if let Some(webview) = &self.workspace_webview {
            workspace_webview::set_visible(webview, true, cx);
            let _ = webview.read(cx).raw().focus();
        } else if let Some(state) = &self.quick_open {
            state.query.focus_handle(cx).focus(window);
        }
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
                    "Projects · Tasks · Agents · Terminals",
                    "Proyectos · Tareas · Agentes · Terminales",
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

                    "terminal_provider": item.terminal_provider,
                })
            })
            .collect::<Vec<_>>();
        self.dispatch_workspace_event(
            serde_json::json!({
                "type": "quick_open",
                "open_id": state.id,
                "sidebar_width": if self.show_settings { 0.0 } else { self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) },
                "over_terminal": self.show_terminal,
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
        self.dispatch_workspace_event(serde_json::json!({ "type": "quick_open_close" }), cx);
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
                terminal_provider: None,
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
                terminal_provider: None,
                target: QuickOpenTarget::Task {
                    workspace_id: task.workspace_id,
                    task_id: task.id,
                },
            });
        }
        let mut sessions = Vec::new();
        for terminal in &self.session.terminals {
            let context = self.navigation_terminal(terminal, None)["context"].as_str().unwrap_or_default().to_string();
            let repository = self.workspaces.iter().find(|workspace| workspace.id == terminal.workspace_id)
                .and_then(|workspace| workspace.repositories.iter().find(|repo| Some(repo.id) == terminal.repository_id));
            let subtitle = format!("{} · {}{}", terminal.agent.label(), context,
                repository.map(|repo| format!(" · {}", repo.name)).unwrap_or_default());
            let kind_label = self.tr("Terminal", "Terminal").to_string();
            sessions.push((format!("terminal:{}", terminal.id), QuickOpenItem {
                search_key: format!("{} {subtitle} {kind_label} {} {}", terminal.label,
                    if terminal.agent == AgentKind::Antigravity { "agy" } else { "" },
                    if terminal.agent == AgentKind::Shell { "shell" } else { "agent agente" }).to_ascii_lowercase(),
                title: terminal.label.clone(), subtitle, kind_label,
                icon: AppIcon::SquareTerminal, color: rgb(0x8190d7), color_css: "#8190d7".into(),
                terminal_provider: Some(terminal.agent),
                target: QuickOpenTarget::Terminal { terminal_id: terminal.id },
            }));
        }
        sessions.sort_by_key(|(id, _)| self.session.agent_order.iter().position(|key| key == id).unwrap_or(usize::MAX));
        items.extend(sessions.into_iter().map(|(_, item)| item));
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
                            terminal_provider: None,
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
            // Navigation is a small in-memory list: do not hide agents behind
            // the file search's result cap when projects/tasks fill the list.
            .take(if state.mode == QuickOpenMode::Navigation { usize::MAX } else { QUICK_OPEN_RESULT_LIMIT })
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
        let Some(window_handle) = cx.active_window() else { return; };
        cx.spawn(async move |this, cx| {
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| app.activate_quick_open_target(target, window, cx));
            });
        }).detach();
    }

    fn activate_quick_open_target(&mut self, target: QuickOpenTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.close_quick_open(cx);
        match target {
            QuickOpenTarget::Terminal { terminal_id } => {
                self.focus_terminal(terminal_id, window, cx);
                self.hydrate_navigation(cx);
                self.dispatch_navigation_event(serde_json::json!({
                    "type": "reveal_agent", "row_id": format!("nav-terminal-{terminal_id}"),
                }), cx);
            }
            QuickOpenTarget::Project { workspace_id } => self.show_project_overview(workspace_id, cx),
            QuickOpenTarget::Task {
                workspace_id,
                task_id,
            } => {
                self.show_task_details_for(workspace_id, task_id, cx);
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
        self.show_project_overview = false;
        self.show_task_details = false;
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
        self.show_project_overview = false;
        self.show_task_details = false;
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
        self.show_project_overview = false;
        self.show_task_details = false;
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
        self.show_project_overview = false;
        self.show_task_details = false;
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
        self.show_project_overview = false;
        self.show_task_details = false;
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
            self.show_project_overview = false;
            self.show_task_details = false;
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
        self.show_project_overview = false;
        self.show_task_details = false;
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

    fn handle_navigation_link(&mut self, payload: NavigationLinkPayload, cx: &mut Context<Self>) {
        self.reload_external_data(cx);
        match payload.scope.as_str() {
            "project" => if let Some(id) = payload.project_id { self.open_project_from_navigation(id, cx); },
            "task" => if let Some(id) = payload.task_id { self.open_task_from_navigation(id, cx); },
            _ => {},
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
            {
                return;
            }
            terminal.claude_session = Some(session);
            self.set_terminal_agent(payload.terminal_id, AgentKind::Claude);
            self.persist_session();
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
            self.persist_session();
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
            if let Some(notification) =
                self.announce_ready_task(payload.task_id, payload.title, payload.message, cx)
            {
                play_agent_attention_sound();
                cx.background_executor()
                    .spawn(async move {
                        show_native_agent_notification(&notification);
                    })
                    .detach();
            }
            return;
        }
        if let Some(task_id) = message.strip_prefix("note-updated:") {
            if let Ok(task_id) = Uuid::parse_str(task_id) {
                self.task_legacy_notes.remove(&task_id);
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
            self.task_details_dirty.retain(|id| loaded_task_ids.contains(id));
            self.task_legacy_notes.retain(|id, _| loaded_task_ids.contains(id));
            if self.show_task_details && self.session.selected_task_id.is_some_and(|id| !loaded_task_ids.contains(&id)) {
                self.show_home(cx);
            }
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
    /// Returns the toast and its destination for the desktop notification, or `None` when
    /// there is nothing to announce. The selected task is deliberately left
    /// untouched: the user reaches the task by clicking the toast.
    fn announce_ready_task(
        &mut self,
        task_id: Uuid,
        title: Option<String>,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<AppToast> {
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
        let notification = AppToast {
            target: AppToastTarget::Task { task_id },
            title,
            message,
        };
        self.app_toasts.push(notification.clone());
        cx.notify();
        Some(notification)
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
        self.show_task_details_for(task.workspace_id, task_id, cx);
        if let Some(row_index) = self.sidebar_task_row_index(task_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
    }

    fn open_project_from_navigation(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
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
        self.show_project_overview(workspace_id, cx);
        if let Some(row_index) = self.sidebar_workspace_row_index(workspace_id) {
            self.sidebar_scroll.scroll_to_item(row_index);
        }
    }

    fn open_task_from_navigation(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        if !self.tasks.iter().any(|task| task.id == task_id) {
            self.reload_external_data(cx);
        }
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        self.show_task_details_for(task.workspace_id, task_id, cx);
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
                    "Authentication profile updated for new terminal sessions",
                    "Perfil de autenticación actualizado para las nuevas sesiones de terminal",
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
            self.refresh_external_integrations(cx);
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
        self.show_project_overview = false;
        self.show_task_details = task_id.is_some() && repository_id.is_none();
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        if task_id.is_none() && repository_id.is_none() && !show_file_explorer {
            self.flush_active_file(cx);
            self.active_file = None;
            self.active_diff = None;
            self.quick_open = None;
            self.show_project_overview = true;
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
                self.session
                    .unseen_task_ids
                    .retain(|current| *current != task_id);
                self.app_toasts
                    .retain(|toast| toast.target.task_id() != Some(task_id));
                self.task_legacy_notes.remove(&task_id);
                self.task_details_dirty.remove(&task_id);
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
        if self.workspace_webview.is_some() {
            let request_id = Uuid::new_v4();
            self.project_appearance_request = Some((request_id, workspace_id));
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal", "modal": {
                    "kind": "edit_project", "request_id": request_id, "workspace_id": workspace_id,
                    "over_terminal": self.show_terminal,
                    "title": self.tr("Edit project", "Editar proyecto"),
                    "name": workspace.label(), "icon": workspace.icon,
                    "color_id": workspace_color_id(workspace.color),
                    "icon_options": project_icon_options(self.session.language).into_iter()
                        .map(|(value, label, _)| serde_json::json!({ "value": value, "label": label })).collect::<Vec<_>>(),
                    "color_options": project_colors().into_iter().map(|color| serde_json::json!({
                        "value": workspace_color_id(color), "label": workspace_color_id(color), "color": workspace_color_css(color),
                    })).collect::<Vec<_>>(),
                    "description": self.tr(
                        "Change how this project appears in Blackholes. Its folder and repositories will not be renamed.",
                        "Cambia cómo aparece este proyecto en Blackholes. Su carpeta y repositorios no serán renombrados.",
                    ),
                    "confirm_label": self.tr("Save changes", "Guardar cambios"),
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": if self.show_settings { 0.0 } else {
                        -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0)
                    },
                }
            }), cx);
            self.dispatch_navigation_event(serde_json::json!({ "type": "modal_visibility", "visible": true }), cx);
            if let Some(webview) = &self.workspace_webview { let _ = webview.read(cx).raw().focus(); }
            cx.notify();
            return;
        }
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


    fn remove_project_reference(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) -> bool {
        let task_ids = self
            .tasks
            .iter()
            .filter(|task| task.workspace_id == workspace_id)
            .map(|task| task.id)
            .collect::<HashSet<_>>();
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
        self.task_legacy_notes
            .retain(|task_id, _| !task_ids.contains(task_id));
        self.show_project_overview = false;
        self.show_task_details = false;
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
        self.dispatch_workspace_event(
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
        if self.task_modal_submitting || self.project_modal_submitting {
            return;
        }
        self.task_modal_request = None;
        self.task_removal_confirmation = None;
        self.agent_removal_confirmation = None;
        self.project_modal_request = None;
        self.project_appearance_request = None;
        self.repository_modal_workspace = None;
        self.repository_removal = None;
        self.project_modal_sources.clear();
        self.dispatch_workspace_event(
            serde_json::json!({ "type": "app_modal", "modal": null }),
            cx,
        );
        self.dispatch_navigation_event(
            serde_json::json!({ "type": "modal_visibility", "visible": false }),
            cx,
        );
        cx.notify();
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


    fn open_manage_task(&mut self, task: ProjectTask, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_task_details_for(task.workspace_id, task.id, cx);
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
        // Use the shared overlay even over a native terminal. GPUI dialogs hide
        // the child WebViews to avoid native layering conflicts, blanking the
        // navigation sidebar. The web overlay preserves and dims both surfaces.
        if self.workspace_webview.is_some() {
            self.task_removal_confirmation = Some(task_id);
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "remove_task", "task_id": task_id,
                    "over_terminal": self.show_terminal,
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
            if let Some(webview) = &self.workspace_webview {
                let _ = webview.read(cx).raw().focus();
            }
            cx.notify();
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
        window.open_dialog(cx, move |dialog, _, _cx| {
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
                .child(form_field(_cx, base_label, Input::new(&base)));

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

    fn open_add_project_repository(&mut self, workspace_id: Uuid, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.project_modal_submitting || self.task_modal_submitting { return; }
        let Some(workspace) = self.workspaces.iter().find(|w| w.id == workspace_id) else { return; };
        if self.workspace_webview.is_none() { return; }
        let request_id = Uuid::new_v4();
        self.project_modal_request = Some(request_id);
        self.repository_modal_workspace = Some(workspace_id);
        self.repository_removal = None;
        self.project_modal_sources.clear();
        self.dispatch_workspace_event(serde_json::json!({
            "type": "app_modal", "modal": {
                "kind": "add_repository", "request_id": request_id, "workspace_id": workspace_id,
                "over_terminal": self.show_terminal,
                "title": self.tr("Add repository", "Agregar repositorio"),
                "name": workspace.label(), "description": "",
                "projects_root": workspace.root_path.as_ref().map(|p| p.display().to_string()),
                "confirm_label": self.tr("Add repositories", "Agregar repositorios"),
                "cancel_label": self.tr("Cancel", "Cancelar"),
                "offset_x": -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0),
            }
        }), cx);
        self.dispatch_navigation_event(serde_json::json!({ "type": "modal_visibility", "visible": true }), cx);
        if let Some(webview) = &self.workspace_webview { let _ = webview.read(cx).raw().focus(); }
        cx.notify();
    }

    fn submit_add_repositories(&mut self, request_id: Uuid, workspace_id: Uuid,
        sources: Vec<ProjectRepositorySource>, mode: ProjectRepositoryMode, cx: &mut Context<Self>) {
        if self.project_modal_request != Some(request_id) || self.repository_modal_workspace != Some(workspace_id)
            || self.project_modal_submitting || self.busy.is_some() { return; }
        if sources.is_empty() || sources.iter().any(|source| matches!(source,
            ProjectRepositorySource::Local(path) if !self.project_modal_sources.contains(path))) {
            self.repository_modal_error(request_id, self.tr("Choose repositories using the form.", "Selecciona repositorios usando el formulario.").into(), cx);
            return;
        }
        let Some(mut workspace) = self.workspaces.iter().find(|w| w.id == workspace_id).cloned() else { return; };
        let original_workspace = workspace.clone();
        self.project_modal_submitting = true;
        self.busy = Some(self.tr("Adding repositories…", "Agregando repositorios…").into());
        let background = cx.background_executor().spawn(async move {
            let mut completed = Vec::new();
            let mut error = None;
            for source in sources {
                let value = match &source {
                    ProjectRepositorySource::Local(path) => path.to_string_lossy().into_owned(),
                    ProjectRepositorySource::Github(url) => url.clone(),
                };
                let result = match source {
                    ProjectRepositorySource::Local(path) => match mode {
                        ProjectRepositoryMode::Link => ProjectService::add_existing_repository(&mut workspace, &path),
                        ProjectRepositoryMode::Copy => ProjectService::copy_existing_repository(&mut workspace, &path),
                    },
                    ProjectRepositorySource::Github(url) => ProjectService::add_github_repository(&mut workspace, &url),
                };
                match result {
                    Ok(()) => completed.push(value),
                    Err(e) => { error = Some(format!("{e:#}")); break; }
                }
            }
            (workspace, completed, error)
        });
        cx.spawn(async move |this, cx| {
            let (workspace, completed, mut error) = background.await;
            let _ = this.update(cx, |app, cx| {
                app.project_modal_submitting = false;
                app.busy = None;
                if let Some(index) = app.workspaces.iter().position(|w| w.id == workspace_id) {
                    if !completed.is_empty() {
                        // Keep successful imports visible even if a later source failed.
                        if let Err(e) = app.database.replace_workspace_if_unchanged(&original_workspace, &workspace) {
                            error = Some(format!("Repositories were added on disk, but registration could not be saved: {e:#}. Their files are preserved in the project folder. Refresh the project before continuing."));
                        } else {
                            app.workspaces[index] = workspace;
                        }
                    }
                } else {
                    error = Some(app.tr("The project was removed while adding repositories. Any copied files remain in its folder.", "El proyecto se quitó mientras se agregaban repositorios. Los archivos copiados permanecen en su carpeta.").into());
                }
                if app.project_modal_request == Some(request_id) {
                    if let Some(error) = error {
                        app.dispatch_workspace_event(serde_json::json!({
                            "type": "app_modal_feedback", "request_id": request_id,
                            "feedback": { "error": error, "completed_sources": completed },
                        }), cx);
                    } else {
                        app.dismiss_app_modal(cx);
                        app.set_status(app.tr("Repositories added", "Repositorios agregados"), false, cx);
                    }
                }
                app.hydrate_navigation(cx);
                app.hydrate_active_workspace_surface(cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn repository_modal_error(&mut self, request_id: Uuid, message: String, cx: &mut Context<Self>) {
        self.dispatch_workspace_event(serde_json::json!({
            "type": "app_modal_feedback", "request_id": request_id,
            "feedback": { "error": message },
        }), cx);
    }

    fn repository_removal_guard(&self, workspace_id: Uuid, repository_id: Uuid) -> Result<()> {
        if self.busy.is_some() {
            anyhow::bail!("{}", self.tr("Wait for active operations and agents to finish.", "Espera a que terminen las operaciones y los agentes activos."));
        }
        let workspace = self.workspaces.iter().find(|w| w.id == workspace_id).context("Project missing")?;
        if self.database.all_tasks()?.iter().any(|task| task.repositories.iter().any(|r|
            r.repository_id == repository_id)) {
            anyhow::bail!("{}", self.tr("Remove this repository from its tasks first. Their worktrees must be preserved.", "Primero quita este repositorio de sus tareas. Es necesario proteger sus worktrees."));
        }
        if self.session.terminals.iter().any(|terminal| terminal.workspace_id == workspace_id) {
            anyhow::bail!("{}", self.tr("Close this project's terminals before removing a repository.", "Cierra las terminales de este proyecto antes de eliminar un repositorio."));
        }
        if self.active_file.as_ref().is_some_and(|file| file.dirty || file.save_state != NoteSaveState::Saved) {
            anyhow::bail!("{}", self.tr("Save pending file edits first.", "Guarda primero los archivos con cambios pendientes."));
        }
        let plan = ProjectService::repository_removal(workspace, repository_id)?;
        if !plan.linked && self.database.workspaces()?.iter().any(|other| other.id != workspace_id
            && (other.root_path.as_ref().is_some_and(|root| root.starts_with(&plan.path))
                || other.repositories.iter().any(|r| r.path.starts_with(&plan.path)))) {
            anyhow::bail!("{}", self.tr("Another project uses this folder. Remove that reference first.", "Otro proyecto utiliza esta carpeta. Quita primero esa referencia."));
        }
        Ok(())
    }

    fn open_remove_repository(&mut self, workspace_id: Uuid, repository_id: Uuid, cx: &mut Context<Self>) {
        if self.project_modal_submitting || self.task_modal_submitting || self.workspace_webview.is_none() { return; }
        let Some(workspace) = self.workspaces.iter().find(|w| w.id == workspace_id) else { return; };
        let plan = match ProjectService::repository_removal(workspace, repository_id) {
            Ok(plan) => plan,
            Err(error) => { self.set_status(format!("{error:#}"), true, cx); return; }
        };
        let name = workspace.repositories.iter().find(|r| r.id == repository_id).unwrap().name.clone();
        let request_id = Uuid::new_v4();
        let description = if plan.linked {
            self.tr("Only the shortcut and this project's reference will be removed. The original repository, its files and pending changes stay untouched.",
                "Solo se eliminarán el acceso directo y la referencia de este proyecto. El repositorio original, sus archivos y cambios pendientes se conservan.")
        } else {
            self.tr("The entire repository folder will be removed from this project, including Git history, .env files, dependencies and all uncommitted changes. It will be moved to Trash; emptying Trash permanently loses all of these data. Stop external processes using this folder first.",
                "Se quitará la carpeta completa del repositorio, incluidos el historial Git, archivos .env, dependencias y todos los cambios sin commit. Irá a la Papelera; al vaciarla perderás todos estos datos definitivamente. Detén primero los procesos externos que usen esta carpeta.")
        };
        self.dispatch_workspace_event(serde_json::json!({
            "type": "app_modal", "modal": {
                "kind": "remove_repository", "request_id": request_id,
                "workspace_id": workspace_id, "repository_id": repository_id,
                "over_terminal": self.show_terminal,
                "title": if plan.linked { self.tr("Remove repository link?", "¿Eliminar enlace al repositorio?") }
                    else { self.tr("Remove repository and its files?", "¿Eliminar repositorio y sus archivos?") },
                "name": name, "context": plan.path.display().to_string(), "description": description,
                "confirm_label": if plan.linked { self.tr("Remove link", "Eliminar enlace") } else { self.tr("Move to Trash", "Mover a la Papelera") },
                "cancel_label": self.tr("Cancel", "Cancelar"),
                "offset_x": -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0),
            }
        }), cx);
        self.repository_removal = Some((request_id, workspace_id, repository_id, plan));
        self.repository_modal_workspace = None;
        self.project_modal_request = Some(request_id);
        self.dispatch_navigation_event(serde_json::json!({ "type": "modal_visibility", "visible": true }), cx);
        if let Some(webview) = &self.workspace_webview { let _ = webview.read(cx).raw().focus(); }
        cx.notify();
    }

    fn confirm_remove_repository(&mut self, request_id: Uuid, cx: &mut Context<Self>) {
        let Some((expected_id, workspace_id, repository_id, plan)) = self.repository_removal.clone() else { return; };
        if request_id != expected_id || self.project_modal_request != Some(request_id) || self.project_modal_submitting { return; }
        let result = (|| -> Result<()> {
            self.repository_removal_guard(workspace_id, repository_id)?;
            let index = self.workspaces.iter().position(|w| w.id == workspace_id).context("Project missing")?;
            let mut workspace = self.workspaces[index].clone();
            let moved = ProjectService::trash_repository(&workspace, repository_id, &plan)?;
            let repository = workspace.repositories.iter().find(|r| r.id == repository_id).context("Repository missing")?;
            if !workspace.ignored_repository_paths.contains(&repository.path) {
                workspace.ignored_repository_paths.push(repository.path.clone());
            }
            workspace.repositories.retain(|r| r.id != repository_id);
            workspace.layout = if workspace.repositories.is_empty() { WorkspaceLayout::Empty } else { WorkspaceLayout::MultiRepository };
            workspace.updated_at = Utc::now();
            if let Err(error) = self.database.replace_workspace_if_unchanged(&self.workspaces[index], &workspace) {
                if let Some(moved) = moved {
                    if fs::symlink_metadata(&plan.path).is_ok() {
                        anyhow::bail!("Could not save removal: {error:#}. Files are safe in {}", moved.display());
                    }
                    fs::rename(&moved, &plan.path).with_context(|| format!("Could not restore after database failure. Files are safe in {}", moved.display()))?;
                }
                return Err(error);
            }
            self.workspaces[index] = workspace;
            if self.session.selected_workspace_id == Some(workspace_id) {
                self.active_file = None;
                self.select_target(workspace_id, None, None, cx);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.dismiss_app_modal(cx);
                self.hydrate_navigation(cx);
                self.hydrate_active_workspace_surface(cx);
                self.set_status(self.tr("Repository removed. Any moved files are recoverable from Trash.", "Repositorio eliminado. Los archivos movidos se pueden recuperar de la Papelera."), false, cx);
            }
            Err(error) => self.repository_modal_error(request_id, format!("{error:#}"), cx),
        }
        cx.notify();
    }

    fn open_create_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.project_modal_submitting || self.task_modal_submitting {
            return;
        }
        if self.workspace_webview.is_some() {
            let request_id = Uuid::new_v4();
            self.project_modal_request = Some(request_id);
            self.repository_modal_workspace = None;
            self.repository_removal = None;
            self.project_modal_sources.clear();
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "create_project",
                    "over_terminal": self.show_terminal,
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
            if let Some(webview) = &self.workspace_webview {
                let _ = webview.read(cx).raw().focus();
            }
            cx.notify();
            return;
        }
        self.set_status(self.tr("The project form is unavailable. Restart Blackholes and try again.", "El formulario no está disponible. Reinicia Blackholes e inténtalo de nuevo."), true, cx);
    }

    fn submit_create_project_modal(
        &mut self,
        request_id: Uuid,
        name: String,
        sources: Vec<ProjectRepositorySource>,
        mode: ProjectRepositoryMode,
        cx: &mut Context<Self>,
    ) {
        if self.project_modal_request != Some(request_id) || self.project_modal_submitting
            || self.repository_modal_workspace.is_some() || self.repository_removal.is_some() {
            return;
        }
        let validation = if name.trim().is_empty() {
            Some(self.tr("Enter a project name.", "Escribe un nombre para el proyecto."))
        } else if sources.iter().any(|source| matches!(source, ProjectRepositorySource::Local(path) if !self.project_modal_sources.contains(path))) {
            Some(self.tr("Choose local repositories using the folder picker.", "Selecciona los repositorios locales con el selector de carpetas."))
        } else {
            None
        };
        if let Some(error) = validation {
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal_feedback", "request_id": request_id,
                "feedback": { "error": error },
            }), cx);
            return;
        }
        self.project_modal_submitting = true;
        let root = self.projects_root();
        let background = cx.background_executor().spawn(async move {
            ProjectService::create_with_repositories(&root, name.trim(), sources, mode)
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
                    app.dispatch_workspace_event(serde_json::json!({
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
        if self.busy.is_some() || self.task_modal_submitting || self.project_modal_submitting {
            return;
        }
        let Some(workspace) = self.selected_workspace().cloned() else {
            return;
        };
        if workspace.repositories.is_empty() {
            self.set_status("Add a Git repository before creating a task", true, cx);
            return;
        }

        // Keep the underlying WebViews visible: a native GPUI dialog must hide
        // them to avoid AppKit layering conflicts, which produces a blank backdrop.
        if self.workspace_webview.is_some() {
            let request_id = Uuid::new_v4();
            self.task_modal_request = Some((request_id, workspace.id));
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal",
                "modal": {
                    "kind": "create_task", "request_id": request_id, "over_terminal": self.show_terminal,
                    "workspace_id": workspace.id,
                    "repositories": workspace.repositories.iter().map(|repository| serde_json::json!({
                        "id": repository.id, "name": repository.name,
                    })).collect::<Vec<_>>(),
                    "title": self.tr("Create isolated task workspace", "Crear espacio aislado para la tarea"),
                    "name": "", "description": "",
                    "confirm_label": self.tr("Create task", "Crear tarea"),
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": if self.show_settings { 0.0 } else {
                        -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0)
                    },
                }
            }), cx);
            self.dispatch_navigation_event(serde_json::json!({
                "type": "modal_visibility", "visible": true,
            }), cx);
            if let Some(webview) = &self.workspace_webview {
                let _ = webview.read(cx).raw().focus();
            }
            cx.notify();
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
                .child(form_field(_cx, title_label, Input::new(&title)))
                .child(form_field(_cx, branch_label, Input::new(&branch)))
                .child(form_field(
                    _cx,
                    description_label,
                    Input::new(&description).h(px(92.)),
                ))
                .child(form_divider(_cx));

            let mut branch_section = v_flex().gap_2().child(section_label(_cx, branch_source_label));

            let mut branch_sources = segmented_group(_cx);
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
                    _cx,
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
                .child(form_field(_cx, base_label, Input::new(&base)));

            if branch_source == TaskBranchSource::Current {
                let options_reuse = options.clone();
                let weak_reuse = weak.clone();
                let options_recreate = options.clone();
                let weak_recreate = weak.clone();
                branch_section = branch_section.child(
                    segmented_group(_cx)
                        .child(segmented_item(
                            _cx,
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
                            _cx,
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
                    _cx,
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
                        _cx,
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
                branch_section = branch_section.child(h_flex().child(
                    Button::new("check-task-branch").small().label(check_branch_label).on_click(move |_, _, cx| {
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
                    }),
                ));
            }

            if let Some(availability) = availability {
                let mut results = v_flex()
                    .gap_1()
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(_cx.theme().muted)
                    .border_1()
                    .border_color(_cx.theme().border);
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
                        results.child(div().text_size(px(11.)).text_color(_cx.theme().foreground).child(
                            format!("{}: {state}{checked_out}{base}", result.repository_name),
                        ));
                }
                branch_section = branch_section.child(results);
            }

            content = content.child(branch_section).child(form_divider(_cx));

            let mut repositories_section =
                v_flex().gap_2().child(section_label(_cx, repositories_label));

            for repository in &workspace.repositories {
                let repository_id = repository.id;
                let selected = options
                    .lock()
                    .selected_repositories
                    .contains(&repository_id);
                let options_for_click = options.clone();
                let weak_for_click = weak.clone();
                let mut repository_card = v_flex().gap(px(6.)).child(option_row(
                    _cx,
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
                            .border_color(_cx.theme().border)
                            .child(option_row(
                                _cx,
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
                                _cx,
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
                                _cx,
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
                        acceptance_criteria: None,
                        pull_request_url: None,
                        external_task_url: None,
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

    fn handle_create_task_modal(
        &mut self,
        request_id: Uuid,
        workspace_id: Uuid,
        request: CreateTaskRequest,
        check_only: bool,
        cx: &mut Context<Self>,
    ) {
        if self.task_modal_request != Some((request_id, workspace_id)) || self.task_modal_submitting {
            return;
        }
        let Some(workspace) = self.workspaces.iter().find(|workspace| workspace.id == workspace_id).cloned() else {
            self.dismiss_app_modal(cx);
            return;
        };
        let invalid_repositories = request.repository_ids.is_empty()
            || request.repository_ids.iter().any(|id| !workspace.repositories.iter().any(|repo| repo.id == *id));
        let error = if invalid_repositories {
            Some(self.tr("Select at least one repository from this project.", "Selecciona al menos un repositorio de este proyecto."))
        } else if check_only && request.branch_name.as_deref().unwrap_or_default().trim().is_empty() {
            Some(self.tr("Enter a branch name to check.", "Escribe una rama para comprobarla."))
        } else if !check_only && request.title.trim().is_empty() {
            Some(self.tr("Enter a task title.", "Escribe un título para la tarea."))
        } else { None };
        if let Some(error) = error {
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal_feedback", "request_id": request_id, "feedback": { "error": error },
            }), cx);
            return;
        }
        self.task_modal_submitting = true;
        let paths = self.paths.clone();
        let background = cx.background_executor().spawn(async move {
            if check_only {
                TaskService::branch_availability(
                    &workspace, &request.repository_ids,
                    request.branch_name.as_deref().unwrap_or_default(),
                    request.branch_source, request.base_ref.as_deref(),
                ).map(|branches| (None, Some(branches)))
            } else {
                TaskService::new(&paths).create(&workspace, request).map(|task| (Some(task), None))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = background.await;
            let _ = this.update(cx, |app, cx| {
                app.task_modal_submitting = false;
                if app.task_modal_request != Some((request_id, workspace_id)) { return; }
                match result {
                    Ok((Some(task), _)) => {
                        app.dismiss_app_modal(cx);
                        app.finish_background_task(Ok(task), cx);
                    }
                    Ok((_, branches)) => app.dispatch_workspace_event(serde_json::json!({
                        "type": "app_modal_feedback", "request_id": request_id,
                        "feedback": { "branches": branches.unwrap_or_default() },
                    }), cx),
                    Err(error) => app.dispatch_workspace_event(serde_json::json!({
                        "type": "app_modal_feedback", "request_id": request_id,
                        "feedback": { "error": format!("{error:#}") },
                    }), cx),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
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














    fn show_project_overview(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
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
        self.show_task_details = false;
        self.show_project_overview = true;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();
        cx.notify();
    }



    fn show_task_details(&mut self, cx: &mut Context<Self>) {
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
        self.show_task_details_for(workspace_id, task_id, cx);
    }

    fn show_task_details_for(&mut self, workspace_id: Uuid, task_id: Uuid, cx: &mut Context<Self>) {
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
        self.show_project_overview = false;
        self.show_task_details = true;
        self.show_terminal = false;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        self.persist_session();
        cx.notify();
    }


    fn terminal_agent_profile(&self, agent: AgentKind) -> Option<PathBuf> {
        let provider = match agent {
            AgentKind::Claude => AgentProvider::Claude,
            AgentKind::Codex => AgentProvider::Codex,
            AgentKind::Gemini => AgentProvider::Gemini,
            AgentKind::OpenCode => AgentProvider::OpenCode,
            AgentKind::Shell | AgentKind::Antigravity => return None,
        };
        (self.agent_auth_mode(provider) == AgentAuthMode::Isolated)
            .then(|| self.paths.agent_profiles.join(provider.id()))
    }

    fn new_terminal(&mut self, agent: AgentKind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_project_overview = false;
        self.show_task_details = false;
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
            // A running CLI is not necessarily processing a prompt.
            state: SessionState::Idle,
            codex_session: None,
            claude_session: None,
            agent_config_dir: self.terminal_agent_profile(agent),
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
        self.focus_terminal_input(descriptor.id, window, cx);
        cx.notify();
    }

    fn project_terminal_skip_permissions(&self, workspace_id: Uuid) -> bool {
        // Local, per-project opt-in. Missing or invalid settings use normal provider permissions.
        self.database
            .setting(&format!("project-terminal-skip-permissions-{workspace_id}"))
            .ok()
            .flatten()
            .as_deref() == Some("true")
    }

    fn start_task_agents(
        &mut self,
        request: StartTaskAgentsRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value> {
        request.validate()?;
        self.reload_external_data(cx);
        let source = request.source_terminal_id.and_then(|id| {
            self.session.terminals.iter().find(|terminal| terminal.id == id).cloned()
        });
        let source_agent = request.source_agent.or_else(|| source.as_ref().map(|terminal| terminal.agent)
            .filter(|agent| *agent != AgentKind::Shell));
        let mut results = Vec::new();
        for entry in &request.tasks {
            let result = (|| -> Result<serde_json::Value> {
                let agent = entry.agent.or(request.agent).or(source_agent)
                    .filter(|agent| *agent != AgentKind::Shell)
                    .context("Could not detect the calling agent. Specify agent: codex, claude, gemini, or opencode")?;
                let config_dir = if Some(agent) == source_agent {
                    request.source_config_dir.clone()
                        .or_else(|| source.as_ref().filter(|terminal| terminal.agent == agent)
                            .and_then(|terminal| terminal.agent_config_dir.clone()))
                        .or_else(|| {
                            let terminal = source.as_ref()?;
                            let home = std::env::var_os("HOME").map(PathBuf::from)?;
                            match agent {
                                AgentKind::Codex if terminal.codex_session.as_ref().is_some_and(|session| session.profile == crate::model::CodexProfile::Work) => Some(home.join(".codex-work")),
                                AgentKind::Claude if terminal.claude_session.as_ref().is_some_and(|session| session.profile == crate::model::ClaudeProfile::Work) => Some(home.join(".claude-work")),
                                _ => None,
                            }
                        })
                } else { None };
                self.start_task_terminal(entry, agent, config_dir, request.source_terminal_id, window, cx)
            })();
            results.push(result.unwrap_or_else(|error| serde_json::json!({
                "taskId": entry.task_id, "started": false, "reused": false, "error": format!("{error:#}"),
            })));
        }
        self.persist_session();
        self.hydrate_navigation(cx);
        cx.notify();
        Ok(serde_json::json!({
            "accepted": true,
            "startedCount": results.iter().filter(|result| result["started"] == true).count(),
            "reusedCount": results.iter().filter(|result| result["reused"] == true).count(),
            "failedCount": results.iter().filter(|result| result.get("error").is_some()).count(),
            "results": results,
        }))
    }

    fn start_task_terminal(
        &mut self,
        request: &TaskAgentRequest,
        agent: AgentKind,
        config_dir: Option<PathBuf>,
        source_terminal_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value> {
        let task = self.tasks.iter().find(|task| task.id == request.task_id)
            .cloned().context("The task does not exist")?;
        if let Some(existing) = self.session.terminals.iter().find(|terminal| {
            terminal.task_id == Some(task.id) && terminal.agent == agent
                && terminal.state != SessionState::Exited && self.terminals.contains_key(&terminal.id)
        }) {
            // Retrying an acknowledged/ambiguous request must not submit the work twice.
            return Ok(serde_json::json!({
                "taskId": task.id, "terminalId": existing.id, "agent": agent,
                "started": false, "reused": true,
                "message": if source_terminal_id == Some(existing.id) {
                    "This terminal already owns the task. Implement here; do not delegate to yourself."
                } else { "A terminal for this task and agent is already open. No prompt was resubmitted." },
            }));
        }
        if task.repositories.is_empty() || task.repositories.iter().any(|repository| !repository.worktree_path.is_dir()) {
            anyhow::bail!("The task needs its attached worktrees before an agent can start");
        }
        let prompt = implementation_prompt(&task, request.prompt.as_deref())?;
        let descriptor = TerminalDescriptor {
            id: Uuid::new_v4(), workspace_id: task.workspace_id,
            task_id: Some(task.id), repository_id: None, agent,
            label: format!("{} · {}", agent.label(), task.title),
            cwd: task.worktree_root_path.clone(), state: SessionState::Idle,
            codex_session: None, claude_session: None, agent_config_dir: config_dir,
            created_at: Utc::now(),
        };
        self.spawn_terminal_view_with_prompt(&descriptor, Some(&prompt), window, cx)?;
        let tab_id = Uuid::new_v4();
        let dock = self.session.docks.entry(dock_key(task.workspace_id, Some(task.id), None)).or_default();
        dock.tabs.push(DockTab {
            id: tab_id, title: descriptor.label.clone(),
            root: DockNode::Panel { terminal_id: descriptor.id }, active_terminal_id: descriptor.id,
        });
        dock.active_tab_id = Some(tab_id);
        self.session.terminals.push(descriptor.clone());
        insert_unique(&mut self.session.expanded_workspace_ids, task.workspace_id);
        insert_unique(&mut self.session.expanded_task_ids, task.id);
        self.add_task_session(&descriptor);
        // Keep the caller's selection and terminal focused while all tasks launch.
        let persistence_warning = self.tasks.iter().find(|task| task.id == request.task_id)
            .map(|task| self.database.upsert_task(task))
            .unwrap_or(Ok(()))
            .and_then(|_| self.database.save_session(&self.session))
            .err().map(|error| format!("Terminal started but session persistence failed: {error:#}"));
        self.app_toasts.push(AppToast {
            target: AppToastTarget::Terminal { terminal_id: descriptor.id, agent },
            title: format!("{} · {}", agent.label(), task.title),
            message: self.tr("Task terminal started. Click to follow its work.", "Terminal de la tarea iniciada. Haz clic para seguir el trabajo.").into(),
        });
        Ok(serde_json::json!({
            "taskId": task.id, "terminalId": descriptor.id, "agent": agent,
            "cwd": descriptor.cwd, "started": true, "reused": false,
            "skipPermissions": self.project_terminal_skip_permissions(task.workspace_id),
            "warning": persistence_warning,
            "message": "Terminal launched with the task prompt. Provider authentication or first-run setup, if needed, is visible in that terminal.",
        }))
    }

    fn spawn_terminal_view(
        &mut self,
        descriptor: &TerminalDescriptor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        self.spawn_terminal_view_with_prompt(descriptor, None, window, cx)
    }

    fn spawn_terminal_view_with_prompt(
        &mut self,
        descriptor: &TerminalDescriptor,
        prompt: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if self.terminals.contains_key(&descriptor.id) {
            return Ok(());
        }
        let spawned = TerminalService.spawn_with_prompt(
            descriptor,
            self.project_terminal_skip_permissions(descriptor.workspace_id),
            prompt,
        )?;
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
                    if let Some(notification) = notification {
                        play_agent_attention_sound();
                        cx.background_executor()
                            .spawn(async move {
                                show_native_agent_notification(&notification);
                            })
                            .detach();
                    }
                })
                .with_title_callback(move |cx, title| {
                    let title = title.trim();
                    if !title.is_empty() {
                        let _ = weak_for_title.update(cx, |app, cx| {
                            app.update_terminal_title(terminal_id, title);
                            app.detect_foreground_terminal_agent(terminal_id, cx);
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
                    if let Some(notification) = notification {
                        play_agent_attention_sound();
                        cx.background_executor()
                            .spawn(async move {
                                show_native_agent_notification(&notification);
                            })
                            .detach();
                    }
                })
                .with_screen_mode_callback(move |alternate, cx| {
                    let _ = weak_for_screen.update(cx, |app, cx| {
                        if alternate {
                            app.detect_foreground_terminal_agent(terminal_id, cx);
                        } else {
                            app.reset_terminal_agent(terminal_id, cx);
                        }
                        cx.notify();
                    });
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
        descriptor.state = SessionState::Idle;
        self.show_project_overview = false;
        self.show_task_details = false;
        self.show_terminal = true;
        self.show_settings = false;
        self.project_settings_workspace_id = None;
        self.close_file_explorer(cx);
        match self.spawn_terminal_view(&descriptor, window, cx) {
            Ok(()) => {
                self.update_terminal_state(terminal_id, descriptor.state);
                self.focus_terminal_input(terminal_id, window, cx);
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
                AgentKind::Shell | AgentKind::Gemini | AgentKind::OpenCode | AgentKind::Antigravity => {
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
    ) -> Option<AppToast> {
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
            AgentTerminalSignalKind::Started => {
                self.update_terminal_state(terminal_id, SessionState::Idle);
                cx.notify();
                None
            }
            AgentTerminalSignalKind::Working => {
                self.update_terminal_state(terminal_id, SessionState::Working);
                cx.notify();
                None
            }
            AgentTerminalSignalKind::Attention => self.handle_agent_attention(terminal_id, cx),
        }
    }

    fn detect_foreground_terminal_agent(&self, terminal_id: Uuid, cx: &mut Context<Self>) {
        // No polling and no blocking process lookup on the UI thread. Existing provider
        // identities survive custom conversation titles until the shell regains the PTY.
        if !self.session.terminals.iter().any(|terminal| {
            terminal.id == terminal_id && terminal.agent == AgentKind::Shell
                && terminal.state != SessionState::Exited
        }) || self.terminal_shell_owns_foreground(terminal_id) {
            return;
        }
        let Some(process_id) = self.terminals.get(&terminal_id)
            .and_then(|handle| handle.master.lock().process_group_leader()) else { return; };
        cx.spawn(async move |weak, cx| {
            let agent = cx.background_executor()
                .spawn(async move { TerminalService::foreground_agent(process_id) }).await;
            if let Some(agent) = agent {
                let _ = weak.update(cx, |app, cx| {
                    let unchanged = app.terminals.get(&terminal_id)
                        .is_some_and(|handle| handle.master.lock().process_group_leader() == Some(process_id))
                        && app.session.terminals.iter().any(|terminal| {
                            terminal.id == terminal_id && terminal.agent == AgentKind::Shell
                                && terminal.state != SessionState::Exited
                        });
                    if unchanged {
                        app.set_terminal_agent(terminal_id, agent);
                        app.update_terminal_state(terminal_id, SessionState::Idle);
                        cx.notify();
                    }
                });
            }
        }).detach();
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
    ) -> Option<AppToast> {
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

        let notification = AppToast {
            target,
            title,
            message,
        };
        self.app_toasts.push(notification.clone());
        cx.notify();
        Some(notification)
    }

    fn open_toast_target(
        &mut self,
        target: AppToastTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            AppToastTarget::Terminal { terminal_id, .. } => {
                let Some(terminal) = self.session.terminals.iter()
                    .find(|terminal| terminal.id == terminal_id).cloned()
                else {
                    return;
                };
                if terminal.state != SessionState::Exited {
                    // A saved terminal is restored through the same path as
                    // clicking it in the sidebar after reopening the app.
                    self.focus_terminal(terminal_id, window, cx);
                } else {
                    // A stale notification must not restart an exited agent.
                    self.dismiss_app_toast(target, cx);
                    if let Some(task_id) = terminal.task_id {
                        self.open_task_from_navigation(task_id, cx);
                    } else {
                        self.open_project_from_navigation(terminal.workspace_id, cx);
                    }
                }
            }
            AppToastTarget::Task { task_id } => self.open_task_from_toast(task_id, cx),
        }
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
                descriptor.state = SessionState::Idle;
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

    fn focus_terminal_input(&self, terminal_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.terminals.get(&terminal_id) else {
            return;
        };
        handle.view.read(cx).focus_handle().focus(window);

        // GPUI focus and AppKit's first responder are separate. Selecting a
        // terminal in the navigation WKWebView must hand the native keyboard
        // back as well, even when the central WebView was already hidden.
        // Otherwise WebKit can consume Space while forwarding other keys.
        // Both WebViews are children of the same native GPUI content view.
        if let Some(webview) = self.navigation_webview.as_ref().or(self.workspace_webview.as_ref()) {
            if let Err(error) = webview.read(cx).raw().focus_parent() {
                tracing::warn!(?error, "could not return native keyboard focus to terminal");
            }
        }
        // Refresh even if this terminal already owns logical focus: its input
        // handler must be installed for the newly visible native surface.
        window.refresh();
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

        self.show_project_overview = false;
        self.show_task_details = false;
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

        self.focus_terminal_input(terminal_id, window, cx);
        if descriptor.state == SessionState::Attention {
            // Focusing acknowledges attention; it does not submit work.
            self.update_terminal_state(terminal_id, SessionState::Idle);
            cx.notify();
            return;
        }

        self.persist_session();
        cx.notify();
    }

    fn request_close_terminal(&mut self, terminal_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.session.terminals.iter().find(|terminal| terminal.id == terminal_id) else {
            return;
        };
        // Plain shells retain their existing close behavior. Agent sessions must
        // be confirmed, regardless of which X (Agents, project tree or pane) was used.
        if terminal.agent == AgentKind::Shell {
            self.close_terminal(terminal_id, cx);
            return;
        }
        let name = terminal.label.clone();
        let context = self.navigation_terminal(terminal, self.selected_terminal_id())["context"].clone();
        let title = self.tr("Close this terminal agent?", "¿Cerrar este agente de terminal?");
        let description = self.tr(
            "The terminal and its running agent will be stopped and removed from Blackholes' restored session. Repository files are not deleted. Conversation history saved by the provider remains available.",
            "Se detendrán la terminal y su agente, y dejarán de restaurarse al abrir Blackholes. No se eliminan archivos del repositorio. Se conserva el historial que haya guardado el proveedor.",
        );
        let confirm_label = self.tr("Close agent", "Cerrar agente");
        if self.workspace_webview.is_some() {
            self.agent_removal_confirmation = Some(AgentRemovalTarget::Terminal(terminal_id));
            self.dispatch_workspace_event(serde_json::json!({
                "type": "app_modal", "modal": {
                    "kind": "close_terminal", "terminal_id": terminal_id,
                    "over_terminal": self.show_terminal,
                    "title": title, "name": name, "context": context,
                    "description": description, "confirm_label": confirm_label,
                    "cancel_label": self.tr("Cancel", "Cancelar"),
                    "offset_x": if self.show_settings { 0.0 } else {
                        -(self.session.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX) / 2.0)
                    },
                }
            }), cx);
            self.dispatch_navigation_event(serde_json::json!({
                "type": "modal_visibility", "visible": true,
            }), cx);
            cx.notify();
            return;
        }
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_submit = weak.clone();
            dialog.title(title).w(px(480.))
                .child(v_flex().gap_3().child(name.clone()).child(description))
                .button_props(DialogButtonProps::default().ok_text(confirm_label))
                .confirm().on_ok(move |_, _, cx| {
                    weak_submit.update(cx, |app, cx| { app.close_terminal(terminal_id, cx); true }).unwrap_or(false)
                })
        });
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

        self.dispatch_workspace_event(
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
            self.settings_return_view = Some((self.show_terminal, self.show_task_details, self.show_project_overview, self.project_settings_workspace_id));
        }
        self.show_project_overview = false;
        self.show_task_details = false;
        self.show_terminal = false;
        self.show_settings = true;
        self.project_settings_workspace_id = None;

        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.show_settings { return; }
        self.show_settings = false;
        if let Some((terminal, task_note, project_note, project_settings)) = self.settings_return_view.take() {
            self.show_terminal = terminal;
            self.show_task_details = task_note;
            self.show_project_overview = project_note;
            self.project_settings_workspace_id = project_settings;
        }
        self.hydrate_navigation(cx);

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
        let terminal_label = self.tr("Blank Terminal", "Terminal vacía").to_string();
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
            let weak_refresh_project = weak.clone();
            let weak_edit_project = weak.clone();
            let weak_project_instructions = weak.clone();
            let weak_remove_project = weak.clone();
            let new_terminal_label = self.tr("New terminal", "Nueva terminal").to_string();
            let add_task_label = self.tr("Add task", "Agregar tarea").to_string();
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
                menu.min_w(px(190.))
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
                        move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = weak_close_terminal
                                .update(cx, |app, cx| app.request_close_terminal(terminal_id, window, cx));
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
                            move |_, window, cx| {
                                cx.stop_propagation();
                                let _ = weak_close_terminal
                                    .update(cx, |app, cx| app.request_close_terminal(terminal_id, window, cx));
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
                let task_to_edit = task.clone();
                let edit_task_label = self.tr("Edit task", "Editar tarea").to_string();
                let remove_task_label = self.tr("Delete task", "Eliminar tarea").to_string();
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
                        let task = task_to_edit.clone();
                        let menu = menu
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
                            move |_, window, cx| {
                                cx.stop_propagation();
                                let _ = weak_close_terminal
                                    .update(cx, |app, cx| app.request_close_terminal(terminal_id, window, cx));
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
                                move |_, window, cx| {
                                    cx.stop_propagation();
                                    let _ = weak_close_terminal
                                        .update(cx, |app, cx| app.request_close_terminal(terminal_id, window, cx));
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
        let weak_settings = weak.clone();
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
                            app.show_home(cx)
                        });
                    })
                    .child(
                        div()
                            .flex_1()
                            .child(app_name_label(SIDEBAR_APP_NAME_FONT_SIZE)),
                    ),
            )
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
                        .when_some(descriptor, |this, terminal| this.child(agent_icon_themed(terminal.agent, self.session.theme)))
                        .child(
                            div().text_ellipsis().child(
                                descriptor
                                    .map(|terminal| terminal.label.clone())
                                    .unwrap_or_else(|| closed_terminal_label.into()),
                            ),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("close-terminal-{terminal_id}")))
                        .px_2()
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(0xff7b72)))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = weak_close
                                .update(cx, |app, cx| app.request_close_terminal(terminal_id, window, cx));
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
                        let _ = weak_open.update(cx, |app, cx| app.open_toast_target(target, window, cx));
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
                    "Projects · Tasks · Agents · Terminals",
                    "Proyectos · Tareas · Agentes · Terminales",
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
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let target = target.clone();
                        let _ = weak_activate
                            .update(cx, |app, cx| app.activate_quick_open_target(target, window, cx));
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
        if let Some(webview) = &self.workspace_webview {
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
                "The workspace WebView could not be loaded.",
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
            && !self.show_project_overview
            && !self.show_task_details
            && !self.show_terminal
            && self.file_explorer.mode == FileExplorerMode::Files
        {
            self.ensure_file_editor(window, cx);
        }
        if self.show_task_details && let Some(task) = self.selected_task().cloned()
            && !self.task_legacy_notes.contains_key(&task.id) {
            match TaskNoteService::read(&task) {
                Ok(note) => { self.task_legacy_notes.insert(task.id, note); }
                Err(error) => {
                    self.task_legacy_notes.insert(task.id, String::new());
                    self.status = Some((format!("Could not read previous task notes: {error:#}"), true));
                }
            }
        }
        self.hydrate_active_workspace_surface(cx);
        if !self.show_terminal {
            self.hydrate_workspace_status(cx);
        }

        // The central React surface owns every visual workspace except the
        // high-throughput native terminal. Quick-open lives inside that same
        // surface so macOS can preserve the real content beneath its backdrop.
        let terminal_modal = self.show_terminal
            && (self.project_modal_request.is_some()
                || self.project_appearance_request.is_some()
                || self.task_modal_request.is_some()
                || self.task_removal_confirmation.is_some()
                || self.agent_removal_confirmation.is_some()
                || self.quick_open.is_some());
        let workspace_visible =
            (!self.show_terminal || terminal_modal || self.quick_open.is_some()) && !window.has_active_dialog(cx);
        if let Some(webview) = &self.workspace_webview {
            let focus_modal = terminal_modal && !webview.read(cx).visible();
            workspace_webview::set_visible(webview, workspace_visible, cx);
            if focus_modal { let _ = webview.read(cx).raw().focus(); }
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
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .text_color(foreground)
            .child(body);

        if terminal_modal {
            if let Some(webview) = &self.workspace_webview {
                main = main.child(div().absolute().inset_0().child(webview.clone()));
            }
        }

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
                                .when(!self.update_state.available.is_empty() || self.update_state.restart, |button| button.custom(
                                    ButtonCustomVariant::new(cx)
                                        .color(rgb(0x2563eb).into())
                                        .foreground(rgb(0xffffff).into())
                                        .border(rgb(0x2563eb).into())
                                        .hover(rgb(0x1d4ed8).into())
                                        .active(rgb(0x1e40af).into())
                                ))
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
    // `workspace quick` finds `frontend/src/workspace/QuickOpen.tsx`.
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

fn provider_plan_name(provider: AgentProvider, usage: Option<&ProviderPlanUsage>, language: Language) -> String {
    match usage.and_then(|usage| usage.subscription_type.as_deref()) {
        Some(plan) => format!("{} · {plan}", provider.display_name()),
        None => match language {
            Language::English => "Not reported".into(),
            Language::Spanish => "No reportado".into(),
        },
    }
}

fn provider_plan_detail(usage: Option<&ProviderPlanUsage>, language: Language) -> String {
    match (usage, language) {
        (Some(usage), Language::English) if usage.rate_limits_available => "Limits reported by the selected account".into(),
        (Some(usage), Language::Spanish) if usage.rate_limits_available => "Límites reportados por la cuenta seleccionada".into(),
        (Some(_), Language::English) => "This account or provider did not report plan limits".into(),
        (Some(_), Language::Spanish) => "Esta cuenta o proveedor no reportó límites del plan".into(),
        (None, Language::English) => "Refresh to query the selected account".into(),
        (None, Language::Spanish) => "Actualiza para consultar la cuenta seleccionada".into(),
    }
}

fn plan_limit_display(
    window: &PlanUsageWindow,
    language: Language,
) -> (String, String, Option<f32>) {
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

fn content_revision(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
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

fn terminal_tree_row(
    terminal_id: Uuid,
    label: String,
    agent: AgentKind,
    _state: SessionState,
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
                        .child(agent_icon(agent)),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().min_w_0().truncate().child(label)),
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
    terminal_label: String,
) -> AnyElement {
    Button::new(SharedString::from(id))
        .icon(AppIcon::Plus).ghost().xsmall().tooltip(tooltip)
        .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
            let mut menu = menu.min_w(px(190.));
            for (agent, label, icon) in [
                (AgentKind::Shell, terminal_label.clone(), AppIcon::SquareTerminal),
                (AgentKind::Claude, "Claude".into(), AppIcon::ClaudeCode),
                (AgentKind::Codex, "Codex".into(), AppIcon::Codex),
            ] {
                let weak = weak.clone();
                menu = menu.item(PopupMenuItem::new(label).icon(icon).on_click(move |_, window, cx| {
                    let weak = weak.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.select_target(workspace_id, task_id, repository_id, cx);
                            app.new_terminal(agent, window, cx);
                        });
                    });
                }));
            }
            menu
        }).into_any_element()
}

fn agent_icon(agent: AgentKind) -> AnyElement {
    agent_icon_themed(agent, AppTheme::Dark)
}

fn agent_icon_themed(agent: AgentKind, theme: AppTheme) -> AnyElement {
    if agent == AgentKind::Antigravity {
        return img("icons/antigravity.png").size(px(16.)).flex_none().into_any_element();
    }
    let (icon, color) = match agent {
        AgentKind::Shell => (AppIcon::SquareTerminal, rgb(0xb6bdca)),
        AgentKind::Claude => (AppIcon::ClaudeCode, rgb(0xd97757)),
        AgentKind::Codex => (AppIcon::Codex, if theme == AppTheme::Light { rgb(0x202622) } else { rgb(0xe7ecea) }),
        AgentKind::Gemini => (AppIcon::Code2, rgb(0x5b8def)),
        AgentKind::OpenCode => (AppIcon::OpenCode, if theme == AppTheme::Light { rgb(0x211e1e) } else { rgb(0xf1ecec) }),
        AgentKind::Antigravity => unreachable!("Antigravity uses its full-color image above"),
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
    let application = leading_title.split(|character: char| !character.is_ascii_alphanumeric()).next();
    match application {
        Some("opencode") => return Some(AgentKind::OpenCode),
        Some("antigravity" | "agy") => return Some(AgentKind::Antigravity),
        _ => {}
    }
    if has_word("gemini") {
        return Some(AgentKind::Gemini);
    }
    None
}

fn show_native_agent_notification(notification: &AppToast) {
    let target = match serde_json::to_string(&notification.target) {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(?error, "Could not encode the notification destination");
            return;
        }
    };
    crate::services::notifications::show(
        &notification.target.element_key(),
        &notification.title,
        &notification.message,
        &target,
    );
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

fn section_label(cx: &App, label: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
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

fn form_field(cx: &App, label: impl Into<SharedString>, control: impl IntoElement) -> AnyElement {
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(div().text_size(px(11.)).text_color(cx.theme().muted_foreground).child(label.into()))
        .child(control)
        .into_any_element()
}

fn form_divider(cx: &App) -> AnyElement {
    div()
        .w_full()
        .h(px(1.))
        .bg(cx.theme().border)
        .into_any_element()
}

/// Rounded container that turns a set of `segmented_item`s into a tab-like control.
fn segmented_group(cx: &App) -> gpui::Div {
    h_flex()
        .w_full()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(9.))
        .bg(cx.theme().muted)
        .border_1()
        .border_color(cx.theme().border)
}

fn segmented_item(
    cx: &App,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = cx.theme().colors;
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
            colors.input
        } else {
            colors.muted
        })
        .bg(if selected {
            colors.background
        } else {
            colors.muted
        })
        .text_size(px(12.))
        .text_color(if selected {
            colors.foreground
        } else {
            colors.muted_foreground
        })
        .font_weight(if selected {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_ellipsis()
        .cursor_pointer()
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(colors.secondary_hover).text_color(colors.foreground)
            }
        })
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

fn check_indicator(cx: &App, checked: bool) -> AnyElement {
    div()
        .size(px(15.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if checked {
            cx.theme().primary
        } else {
            cx.theme().input
        })
        .bg(if checked {
            cx.theme().primary
        } else {
            cx.theme().background
        })
        .text_size(px(9.))
        .text_color(cx.theme().primary_foreground)
        .child(if checked { "✓" } else { "" })
        .into_any_element()
}

fn option_row(
    cx: &App,
    id: String,
    label: impl Into<SharedString>,
    checked: bool,
    emphasized: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = cx.theme().colors;
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
            colors.input
        } else {
            colors.border
        })
        .bg(if checked {
            colors.accent
        } else {
            colors.background
        })
        .text_size(if emphasized { px(13.) } else { px(12.) })
        .text_color(if checked {
            colors.accent_foreground
        } else {
            colors.foreground
        })
        .font_weight(if emphasized && checked {
            gpui::FontWeight::MEDIUM
        } else {
            gpui::FontWeight::NORMAL
        })
        .cursor_pointer()
        .hover(move |style| style.bg(colors.secondary_hover).border_color(colors.input))
        .on_click(on_click)
        .child(check_indicator(cx, checked))
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

fn install_agent_command_bridge(paths: &AppPaths, window: &Window, cx: &mut Context<BlackholesApp>) -> Result<()> {
    let receiver = crate::services::agent_commands::listen(paths)?;
    let window_handle = window.window_handle();
    cx.spawn(async move |this, cx| {
        while let Ok(command) = receiver.recv_async().await {
            let response = if let Some(payload) = command.message.strip_prefix("start-task-agents:") {
                window_handle.update(cx, |_, window, cx| {
                    this.update(cx, |app, cx| -> Result<serde_json::Value> {
                        app.start_task_agents(serde_json::from_str(payload)?, window, cx)
                    })
                }).and_then(|result| result)
            } else {
                Ok(Err(anyhow::anyhow!("Unsupported agent command")))
            };
            let response = match response {
                Ok(Ok(response)) => response,
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
            // Leave room for navigation and task-notification event payloads.
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
