pub mod app;
pub mod navigation_webview;
pub mod orchestrator_chat;
pub mod quick_open_webview;
pub mod terminal;

pub use app::BlackholesApp;

use crate::model::AppTheme;

pub fn apply_native_theme(theme: AppTheme, window: Option<&mut gpui::Window>, cx: &mut gpui::App) {
    let mode = match theme {
        AppTheme::Light => gpui_component::ThemeMode::Light,
        AppTheme::Dark => gpui_component::ThemeMode::Dark,
    };
    gpui_component::Theme::change(mode, window, cx);
    apply_macos_appearance(theme);
}

#[cfg(target_os = "macos")]
fn apply_macos_appearance(theme: AppTheme) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: AppKit exports both process-lifetime appearance-name constants.
    let appearance_name = unsafe {
        match theme {
            AppTheme::Light => NSAppearanceNameAqua,
            AppTheme::Dark => NSAppearanceNameDarkAqua,
        }
    };
    if let Some(appearance) = NSAppearance::appearanceNamed(appearance_name) {
        application.setAppearance(Some(&appearance));
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_macos_appearance(_theme: AppTheme) {}
