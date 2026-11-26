use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::webview::WebView;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wry::WebViewBuilder;

use crate::model::AgentKind;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSection {
    Agents,
    Projects,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NavigationCommand {
    Ready,
    ShowHome,
    SetSidebarWidth {
        width: f32,
        #[serde(default)]
        commit: bool,
    },
    SetSidebarAgentsFraction {
        fraction: f32,
        #[serde(default)]
        commit: bool,
    },
    ReorderAgents {
        ids: Vec<String>,
    },
    SetTerminalPinned {
        terminal_id: Uuid,
        pinned: bool,
    },
    ReorderPinned {
        ids: Vec<Uuid>,
    },
    RenameTerminal {
        terminal_id: Uuid,
    },
    /// Opens the name modal for a new section, inside `parent_id` when given; the session,
    /// if given, moves into it.
    NewAgentSection {
        #[serde(default)]
        terminal_id: Option<Uuid>,
        #[serde(default)]
        parent_id: Option<Uuid>,
    },
    /// Opens the modal to rename the section and edit its description.
    RenameAgentSection {
        section_id: Uuid,
    },
    DeleteAgentSection {
        section_id: Uuid,
    },
    /// Dropped by dragging: inside `parent_id` (or at the top), before `before_id` or last.
    PlaceAgentSection {
        section_id: Uuid,
        #[serde(default)]
        parent_id: Option<Uuid>,
        #[serde(default)]
        before_id: Option<Uuid>,
    },
    SetAgentSectionCollapsed {
        section_id: Uuid,
        collapsed: bool,
    },
    SetAllAgentSectionsCollapsed {
        collapsed: bool,
    },
    SetTerminalSection {
        terminal_id: Uuid,
        #[serde(default)]
        section_id: Option<Uuid>,
    },
    /// Dropped by dragging into a section (or out of every section with `None`); `order`
    /// lists the sessions of the block it landed in, in their new order.
    MoveAgentSession {
        terminal_id: Uuid,
        #[serde(default)]
        section_id: Option<Uuid>,
        #[serde(default)]
        order: Vec<Uuid>,
    },
    SetSidebarSectionCollapsed {
        section: SidebarSection,
        collapsed: bool,
    },
    CollapseAll,
    NewProject,
    AddProjectRepository {
        workspace_id: Uuid,
    },
    RemoveRepository {
        workspace_id: Uuid,
        repository_id: Uuid,
    },
    SelectProject {
        workspace_id: Uuid,
    },
    ToggleProject {
        workspace_id: Uuid,
    },
    RefreshProject {
        workspace_id: Uuid,
    },
    EditProject {
        workspace_id: Uuid,
    },
    ProjectSettings {
        workspace_id: Uuid,
    },
    RemoveProject {
        workspace_id: Uuid,
    },
    NewTask {
        workspace_id: Uuid,
    },
    SelectTask {
        workspace_id: Uuid,
        task_id: Uuid,
    },
    ToggleTask {
        task_id: Uuid,
    },
    EditTask {
        task_id: Uuid,
    },
    RemoveTask {
        task_id: Uuid,
    },
    SelectRepository {
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        repository_id: Uuid,
    },
    NewTerminal {
        workspace_id: Uuid,
        task_id: Option<Uuid>,
        repository_id: Option<Uuid>,
        agent: AgentKind,
    },
    FocusTerminal {
        terminal_id: Uuid,
    },
    CloseTerminal {
        terminal_id: Uuid,
    },
    ShowSettings,
}

pub fn create(
    window: &mut Window,
    cx: &mut App,
    command_sender: flume::Sender<String>,
) -> Result<Entity<WebView>> {
    let raw = WebViewBuilder::new()
        .with_html(navigation_html())
        .with_accept_first_mouse(true)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |request| {
            let _ = command_sender.send(request.body().clone());
        })
        .build_as_child(window)
        .context("Unable to create the navigation WebView")?;
    Ok(cx.new(|cx| WebView::new(raw, window, cx)))
}

pub fn dispatch(webview: &Entity<WebView>, event: Value, cx: &mut App) -> Result<()> {
    let payload = serde_json::to_string(&event)?;
    webview.update(cx, |webview, _| {
        webview
            .raw()
            .evaluate_script(&format!("window.blackholesNavigation?.receive({payload});"))
            .context("Unable to dispatch an event to the navigation WebView")
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

fn navigation_html() -> String {
    include_str!("../../assets/navigation/index.html")
        .replace("{{APP_THEME_STYLES}}", include_str!("../../assets/theme.css"))
        .replace(
            "{{NAVIGATION_STYLES}}",
            include_str!("../../assets/navigation/styles.css"),
        )
        .replace(
            "{{NAVIGATION_REACT_BUNDLE}}",
            include_str!("../../assets/generated/navigation.js"),
        )
}
