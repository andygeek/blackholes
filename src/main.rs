use anyhow::Result;
use blackholes_rust::{
    assets::AppAssets,
    paths::AppPaths,
    services::{
        claude::run_claude_session_hook, codex::run_codex_session_hook, database::Database,
        orchestrator::terminate_all_agent_processes,
    },
    ui::{BlackholesApp, apply_native_theme},
};
use gpui::{
    AppContext as _, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, point, px, size,
};
use std::borrow::Cow;

#[path = "bin/blackholes-mcp.rs"]
mod blackholes_mcp;

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("claude-session-hook") {
        return run_claude_session_hook();
    }
    if std::env::args().nth(1).as_deref() == Some("codex-session-hook") {
        return run_codex_session_hook();
    }
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        return blackholes_mcp::run();
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blackholes_rust=info".into()),
        )
        .compact()
        .init();

    let paths = AppPaths::discover()?;
    let database = Database::open(&paths)?;
    let initial_theme = database.load_session().theme;
    let application = Application::new().with_assets(AppAssets);
    application.run(move |cx| {
        configure_macos_application();
        blackholes_rust::services::updater::initialize();
        if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/GeistMono-ExtraBold.ttf").as_slice(),
        )]) {
            tracing::warn!(?error, "failed to register Geist Mono");
        }
        gpui_component::init(cx);
        apply_native_theme(initial_theme, None, cx);
        cx.on_app_quit(|_| async {
            terminate_all_agent_processes();
        })
        .detach();
        BlackholesApp::init(cx);
        let paths = paths.clone();
        let database = database.clone();
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(100.), px(80.)),
                        size(px(1440.), px(900.)),
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Blackholes".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.), px(14.))),
                    }),
                    window_background: WindowBackgroundAppearance::Blurred,
                    ..Default::default()
                },
                move |window, cx| {
                    let app = cx.new(|cx| BlackholesApp::new(paths, database, window, cx));
                    BlackholesApp::register_global_actions(&app, cx);
                    cx.new(|cx| BlackholesApp::wrap_root(app, window, cx))
                },
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_macos_application() {
    use objc2::{AnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);

    let data = NSData::with_bytes(include_bytes!("../assets/app-icon.png"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    // SAFETY: AppKit retains a valid NSImage and this setter runs on the main thread.
    unsafe { application.setApplicationIconImage(Some(&icon)) };
}

#[cfg(not(target_os = "macos"))]
fn configure_macos_application() {}
