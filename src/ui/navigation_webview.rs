use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::webview::WebView;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wry::WebViewBuilder;

use crate::model::AgentKind;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NavigationCommand {
    Ready,
    SetSidebarWidth {
        width: f32,
        #[serde(default)]
        commit: bool,
    },
    CreateGlobalAgent,
    CreateScopedAgent {
        workspace_id: Uuid,
        task_id: Option<Uuid>,
    },
    OpenAgent {
        scope: String,
    },
    RemoveAgent {
        scope: String,
    },
    CollapseAll,
    NewProject,
    AddProjectRepository {
        workspace_id: Uuid,
        #[serde(default)]
        github: bool,
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
    AssignProjectAgent {
        workspace_id: Uuid,
    },
    ProjectNotes {
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
    AssignTaskAgent {
        task_id: Uuid,
    },
    TaskNotes {
        workspace_id: Uuid,
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
        .replace(
            "{{AGENT_AVATAR_STYLES}}",
            include_str!("../../assets/agent-avatar.css"),
        )
        .replace(
            "{{NAVIGATION_STYLES}}",
            include_str!("../../assets/navigation/styles.css"),
        )
        .replace(
            "{{NAVIGATION_REACT_BUNDLE}}",
            include_str!("../../assets/generated/navigation.js"),
        )
}
