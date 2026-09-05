use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::webview::WebView;
use serde::Deserialize;
use serde_json::Value;
use wry::WebViewBuilder;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuickOpenCommand {
    Ready,
    QueryChanged { open_id: u64, query: String },
    Activate { open_id: u64, result_index: usize },
    Dismiss { open_id: u64 },
}

pub fn create(
    window: &mut Window,
    cx: &mut App,
    command_sender: flume::Sender<String>,
) -> Result<Entity<WebView>> {
    let raw = WebViewBuilder::new()
        .with_html(quick_open_html())
        .with_transparent(true)
        .with_accept_first_mouse(true)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |request| {
            let _ = command_sender.send(request.body().clone());
        })
        .build_as_child(window)
        .context("Unable to create the quick-open WebView")?;
    Ok(cx.new(|cx| WebView::new(raw, window, cx)))
}

pub fn dispatch(webview: &Entity<WebView>, event: Value, cx: &mut App) -> Result<()> {
    let payload = serde_json::to_string(&event)?;
    webview.update(cx, |webview, _| {
        webview
            .raw()
            .evaluate_script(&format!("window.blackholesQuickOpen?.receive({payload});"))
            .context("Unable to dispatch an event to the quick-open WebView")
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

pub fn focus(webview: &Entity<WebView>, cx: &mut App) {
    webview.update(cx, |webview, _| {
        let _ = webview.raw().focus();
    });
}

fn quick_open_html() -> String {
    include_str!("../../assets/quick-open/index.html")
        .replace(
            "{{QUICK_OPEN_STYLES}}",
            include_str!("../../assets/quick-open/styles.css"),
        )
        .replace(
            "{{QUICK_OPEN_REACT_BUNDLE}}",
            include_str!("../../assets/generated/quick-open.js"),
        )
}
