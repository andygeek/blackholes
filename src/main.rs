use anyhow::Result;
use blackholes_rust::{
    assets::AppAssets,
    paths::AppPaths,
    services::{
        claude::run_claude_session_hook, codex::run_codex_session_hook, database::Database,
        providers::terminate_provider_helpers,
    },
    ui::{BlackholesApp, apply_native_theme},
};
use gpui::{
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, point, px, size,
};
use std::borrow::Cow;

#[path = "bin/blackholes-mcp.rs"]
mod blackholes_mcp;

#[cfg(target_os = "macos")]
gpui::actions!(blackholes_application, [Quit]);

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
    // Register before AppKit finishes launching so notification clicks that
    // relaunch the app are queued until its window and session are ready.
    blackholes_rust::services::notifications::initialize();
    let reopen_paths = paths.clone();
    let reopen_database = database.clone();
    application.on_reopen(move |cx| {
        if let Err(error) = show_main_window(&reopen_paths, &reopen_database, cx) {
            tracing::error!(?error, "failed to reopen the main window");
        }
    });
    application.run(move |cx| {
        blackholes_rust::services::updater::initialize();
        if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/GeistMono-ExtraBold.ttf").as_slice(),
        )]) {
            tracing::warn!(?error, "failed to register Geist Mono");
        }
        gpui_component::init(cx);
        configure_macos_application(cx);
        apply_native_theme(initial_theme, None, cx);
        cx.on_app_quit(|_| async {
            terminate_provider_helpers();
        })
        .detach();
        BlackholesApp::init(cx);
        let paths = paths.clone();
        let database = database.clone();
        cx.spawn(async move |cx| {
            cx.update(|cx| show_main_window(&paths, &database, cx))??;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}

fn show_main_window(paths: &AppPaths, database: &Database, cx: &mut App) -> Result<()> {
    // Reuse the live window, including its WebViews, agents and terminal sessions.
    // Checking synchronously also prevents repeated Dock events from creating duplicates.
    if let Some(window) = cx.windows().into_iter().find(|window| window.downcast::<gpui_component::Root>().is_some()) {
        window.update(cx, |_, window, _| window.activate_window())?;
    } else {
        let paths = paths.clone();
        let database = database.clone();
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
                // Closing the only macOS window must not drop in-memory work.
                // Hide after the close callback returns to avoid reentrant AppKit events.
                // Application termination (including Sparkle relaunch) remains separate.
                #[cfg(target_os = "macos")]
                window.on_window_should_close(cx, |_, cx| {
                    cx.defer(|cx| cx.hide());
                    false
                });
                let app = cx.new(|cx| BlackholesApp::new(paths, database, window, cx));
                BlackholesApp::register_global_actions(&app, cx);
                cx.new(|cx| BlackholesApp::wrap_root(app, window, cx))
            },
        )?;
    }
    cx.defer(|cx| cx.activate(true));
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_macos_application(cx: &mut App) {
    use gpui::{KeyBinding, Menu, MenuItem, OsAction};
    use gpui_component::input::{Copy, Cut, Paste, SelectAll};
    use objc2::{AnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

    // Register every item through GPUI so native validation AND dispatch have
    // valid entries in its menu_actions table. GPUI 0.2.2 re-locks its platform
    // mutex without releasing it when a foreign item's tag is out of range.
    // Hand-built NSMenuItems (including tag -1) can therefore deadlock the main
    // thread when AppKit inspects menus, even without the user opening a menu.
    // OsAction keeps the native selectors and nil targets, so focused WKWebView
    // editors still receive their own rich clipboard actions via NSResponder.
    // Run after gpui_component::init so its edit keybindings are already known.
    cx.set_menus(vec![
        Menu {
            name: "Blackholes".into(),
            items: vec![MenuItem::action("Quit Blackholes", Quit)],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
    ]);

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
fn configure_macos_application(_: &mut App) {}
