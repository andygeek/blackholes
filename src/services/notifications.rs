// SPDX-License-Identifier: MPL-2.0
//! macOS owns notification identity and persistence; GPUI handles navigation.
use std::sync::OnceLock;

// Keep clicks received during launch until the main window subscribes. Only
// clicks cross this channel; sending or dismissing a notification never focuses
// the app. No worker needs to block waiting for a user response.
static ACTIVATIONS: OnceLock<(flume::Sender<String>, flume::Receiver<String>)> = OnceLock::new();

fn activations() -> &'static (flume::Sender<String>, flume::Receiver<String>) {
    ACTIVATIONS.get_or_init(flume::unbounded)
}

pub fn subscribe() -> flume::Receiver<String> {
    activations().1.clone()
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn bh_notifications_init(on_click: extern "C" fn(*const std::ffi::c_char)) -> bool;
    fn bh_notifications_activate();
    fn bh_notifications_show(
        identifier: *const std::ffi::c_char,
        title: *const std::ffi::c_char,
        message: *const std::ffi::c_char,
        target: *const std::ffi::c_char,
    );
}

#[cfg(target_os = "macos")]
extern "C" fn on_click(target: *const std::ffi::c_char) {
    if target.is_null() {
        return;
    }
    // SAFETY: the delegate supplies a live NSString's NUL-terminated UTF-8
    // bytes for this call. Copy them before returning to Objective-C.
    let target = unsafe { std::ffi::CStr::from_ptr(target) }
        .to_string_lossy()
        .into_owned();
    let _ = activations().0.send(target);
}

pub fn initialize() {
    let _ = activations();
    #[cfg(target_os = "macos")]
    {
        assert!(
            objc2::MainThreadMarker::new().is_some(),
            "Initialize notifications on the main thread"
        );
        // SAFETY: main thread; the bridge retains its delegate for app lifetime.
        if !unsafe { bh_notifications_init(on_click) } {
            tracing::info!(
                "Desktop notifications require the packaged Blackholes.app; in-app notices remain available"
            );
        }
    }
}

/// Called only for a user's click, after GPUI has created the main window.
pub fn activate() {
    #[cfg(target_os = "macos")]
    // SAFETY: the bridge schedules AppKit activation on the main queue.
    unsafe {
        bh_notifications_activate()
    };
}

/// Submit without waiting for permission or delivery on the UI thread.
pub fn show(identifier: &str, title: &str, message: &str, target: &str) {
    #[cfg(target_os = "macos")]
    {
        let values = [identifier, title, message, target].map(std::ffi::CString::new);
        let [Ok(identifier), Ok(title), Ok(message), Ok(target)] = values else {
            tracing::warn!("Could not send a notification containing a NUL character");
            return;
        };
        // SAFETY: all pointers are live C strings. The bridge copies them
        // synchronously before dispatching asynchronous AppKit work.
        unsafe {
            bh_notifications_show(
                identifier.as_ptr(),
                title.as_ptr(),
                message.as_ptr(),
                target.as_ptr(),
            )
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (identifier, target);
        if let Err(error) = notify_rust::Notification::new()
            .appname("Blackholes")
            .summary(title)
            .body(message)
            .show()
        {
            tracing::warn!(?error, "Could not send a desktop notification");
        }
    }
}
