// SPDX-License-Identifier: MPL-2.0
//! Main-thread-only bridge to Sparkle's signed macOS update workflow.
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct UpdateState {
    pub enabled: bool,
    pub available: String,
    pub busy: bool,
    pub restart: bool,
    pub error: String,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn bh_updater_init();
    fn bh_updater_set_blocked(blocked: bool, spanish: bool);
    fn bh_updater_check();
    fn bh_updater_snapshot() -> *const std::ffi::c_char;
}

fn assert_main_thread() {
    #[cfg(target_os = "macos")]
    assert!(objc2::MainThreadMarker::new().is_some(), "Updater must run on the main thread");
}

pub fn initialize() {
    assert_main_thread();
    #[cfg(target_os = "macos")]
    // SAFETY: checked main thread; native bridge owns all Objective-C objects.
    unsafe { bh_updater_init() };
}

pub fn set_blocked(blocked: bool, spanish: bool) {
    assert_main_thread();
    #[cfg(target_os = "macos")]
    unsafe { bh_updater_set_blocked(blocked, spanish) };
    #[cfg(not(target_os = "macos"))]
    let _ = (blocked, spanish);
}

pub fn check() {
    assert_main_thread();
    #[cfg(target_os = "macos")]
    unsafe { bh_updater_check() };
}

pub fn state() -> UpdateState {
    assert_main_thread();
    #[cfg(target_os = "macos")]
    {
        // SAFETY: native static NSString retains the NUL-terminated bytes until
        // the next snapshot call. Copy and deserialize before returning.
        let ptr = unsafe { bh_updater_snapshot() };
        if !ptr.is_null() {
            return decode_state(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes());
        }
        return UpdateState {
            error: "The updater did not return its status. Restart Blackholes and try again.".into(),
            ..UpdateState::default()
        };
    }
    #[cfg(not(target_os = "macos"))]
    UpdateState::default()
}

fn decode_state(bytes: &[u8]) -> UpdateState {
    serde_json::from_slice(bytes).unwrap_or_else(|error| UpdateState {
        error: format!("Could not read the updater status: {error}"),
        ..UpdateState::default()
    })
}
