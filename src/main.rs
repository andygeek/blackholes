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
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance,
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
    let reopen_paths = paths.clone();
    let reopen_database = database.clone();
    application.on_reopen(move |cx| {
        if let Err(error) = show_main_window(&reopen_paths, &reopen_database, cx) {
            tracing::error!(?error, "failed to reopen the main window");
        }
    });
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
    if let Some(window) = cx.windows().first().copied() {
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
fn configure_macos_application() {
    use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly as _, sel};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSImage, NSMenu, NSMenuItem};
    use objc2_foundation::{NSData, NSString};

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);

    // Child WKWebViews hand Command-key equivalents back to the main menu.
    // Standard nil-target edit actions route to WebKit's focused responder,
    // preserving BlockNote's native clipboard events, rich text and undo history.
    // Keep any existing application menu rather than replacing its commands.
    let main_menu = application.mainMenu().unwrap_or_else(|| {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(main_thread), &NSString::from_str("Blackholes"));
        let app_menu = NSMenu::initWithTitle(NSMenu::alloc(main_thread), &NSString::from_str("Blackholes"));
        // SAFETY: Standard AppKit action with no explicit target; AppKit resolves
        // the live responder. All menu creation occurs on the main thread.
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Quit Blackholes"), Some(sel!(terminate:)), &NSString::from_str("q"),
            );
        }
        let item = NSMenuItem::new(main_thread);
        item.setSubmenu(Some(&app_menu));
        menu.addItem(&item);
        application.setMainMenu(Some(&menu));
        menu
    });
    let edit_menu = NSMenu::initWithTitle(NSMenu::alloc(main_thread), &NSString::from_str("Edit"));
    for (title, selector, key) in [
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        // SAFETY: These are standard NSResponder edit selectors. Leaving the
        // target unset lets AppKit validate and dispatch to the focused editor.
        let item = unsafe { edit_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(title), Some(selector), &NSString::from_str(key),
        ) };
        // These native-only items are not entries in GPUI's indexed action
        // table. If no native editor handles one, it must not invoke action 0.
        item.setTag(-1);
        item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    }
    let edit_item = NSMenuItem::new(main_thread);
    edit_item.setTitle(&NSString::from_str("Edit"));
    edit_item.setSubmenu(Some(&edit_menu));
    main_menu.addItem(&edit_item);

    let data = NSData::with_bytes(include_bytes!("../assets/app-icon.png"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    // SAFETY: AppKit retains a valid NSImage and this setter runs on the main thread.
    unsafe { application.setApplicationIconImage(Some(&icon)) };
}

#[cfg(not(target_os = "macos"))]
fn configure_macos_application() {}
