//! macOS permission status + request commands.
//!
//! Backs the Settings → Permissions page. Five permissions surface
//! to the user:
//!
//! | Permission | Why K2SO needs it | Detection |
//! |---|---|---|
//! | Microphone | Apple Dictation (and any future voice features) | `AVCaptureDevice.authorizationStatus(for:.audio)` |
//! | Full Disk Access | Workspace folders outside `~/Documents` (TCC-protected) | Try-read `~/Library/Safari/Bookmarks.plist` |
//! | Accessibility | Programmatic keystroke replay, automation tools | `AXIsProcessTrusted()` |
//! | Apple Events / Automation | AppleScript-driven app integration | (no programmatic check; user opens System Settings) |
//! | Local Network | Mobile companion device discovery on LAN | (no programmatic check; user opens System Settings) |
//!
//! Request actions either prompt directly (microphone via
//! `AVCaptureDevice.requestAccess`) or open the relevant System
//! Settings privacy pane via `x-apple.systempreferences:` URL scheme.

use serde::Serialize;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    /// `true` when the user has granted Full Disk Access. Detected by
    /// trying to read a TCC-protected file (Safari bookmarks) — no
    /// entitlement or private API needed; if the read succeeds the
    /// app has FDA, if it fails with EPERM the app does not.
    pub full_disk_access: bool,
    /// `true` when the app is in System Settings → Privacy & Security
    /// → Accessibility and toggled on.
    pub accessibility: bool,
    /// `true` when AVFoundation reports `.authorized` for audio.
    pub microphone: bool,
}

#[cfg(target_os = "macos")]
fn check_full_disk_access() -> bool {
    use std::path::PathBuf;
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let p = PathBuf::from(home).join("Library/Safari/Bookmarks.plist");
    // Successful open implies FDA granted (or running with TCC fully
    // disabled, which only happens on a clean dev machine that's
    // never seen the file). The caller does NOT need to actually
    // read the bytes — just that the OS allows the open.
    std::fs::File::open(p).is_ok()
}

#[cfg(target_os = "macos")]
fn check_accessibility() -> bool {
    // ApplicationServices.framework's AXIsProcessTrusted() returns
    // YES iff the running process is in System Settings →
    // Accessibility and the toggle is on. Linked at app bundle level
    // through Tauri/wry's existing framework wiring.
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
fn check_microphone() -> bool {
    // AVCaptureDevice.authorizationStatus(for: .audio) returns one of
    // .notDetermined (0), .restricted (1), .denied (2), .authorized (3).
    // We treat only `.authorized` as granted — the user has
    // explicitly approved mic access.
    use objc::runtime::Class;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let cls = match Class::get("AVCaptureDevice") {
            Some(c) => c,
            None => return false, // AVFoundation missing — extremely unlikely on macOS
        };
        // AVMediaTypeAudio is an NSString* extern. We can pass a
        // Cocoa NSString of "soun" (the FourCC behind it) — but
        // safer to use the actual symbol via a helper. The crate
        // doesn't expose it, so build the NSString from the literal
        // value AVMediaTypeAudio resolves to: `"soun"`.
        use cocoa::base::nil;
        use cocoa::foundation::NSString;
        let media_type: cocoa::base::id =
            NSString::alloc(nil).init_str("soun");
        let status: i64 =
            msg_send![cls, authorizationStatusForMediaType: media_type];
        // 3 == AVAuthorizationStatusAuthorized
        status == 3
    }
}

#[cfg(not(target_os = "macos"))]
fn check_full_disk_access() -> bool { true } // No TCC outside macOS — treat as granted.
#[cfg(not(target_os = "macos"))]
fn check_accessibility() -> bool { true }
#[cfg(not(target_os = "macos"))]
fn check_microphone() -> bool { true }

#[tauri::command]
pub fn permissions_get_status() -> PermissionStatus {
    PermissionStatus {
        full_disk_access: check_full_disk_access(),
        accessibility: check_accessibility(),
        microphone: check_microphone(),
    }
}

/// Open a System Settings privacy pane via the `x-apple.systempreferences:`
/// URL scheme. macOS routes the URL to System Settings and pre-selects
/// the requested category.
#[cfg(target_os = "macos")]
fn open_settings_pane(scheme_path: &str) -> Result<(), String> {
    use std::process::Command;
    Command::new("open")
        .arg(format!("x-apple.systempreferences:{}", scheme_path))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open System Settings: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn open_settings_pane(_scheme_path: &str) -> Result<(), String> {
    Err("System Settings deep links are macOS-only".into())
}

#[tauri::command]
pub fn permissions_request_full_disk_access() -> Result<(), String> {
    open_settings_pane("com.apple.preference.security?Privacy_AllFiles")
}

#[tauri::command]
pub fn permissions_request_accessibility() -> Result<(), String> {
    open_settings_pane("com.apple.preference.security?Privacy_Accessibility")
}

#[tauri::command]
pub fn permissions_request_apple_events() -> Result<(), String> {
    open_settings_pane(
        "com.apple.settings.PrivacySecurity.extension?Privacy_Automation",
    )
}

#[tauri::command]
pub fn permissions_request_local_network() -> Result<(), String> {
    open_settings_pane(
        "com.apple.settings.PrivacySecurity.extension?Privacy_LocalNetwork",
    )
}

/// Microphone is special — AVFoundation has a programmatic prompt
/// (`AVCaptureDevice.requestAccess(for:.audio)`) we can fire directly
/// the FIRST time. After the user has answered (granted OR denied),
/// the same call returns immediately with the cached answer; further
/// requests must go through System Settings. We try the programmatic
/// path first and fall back to opening Settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneRequestResult {
    pub granted: bool,
    pub opened_settings: bool,
}

#[tauri::command]
pub fn permissions_request_microphone() -> Result<MicrophoneRequestResult, String> {
    // Already granted — short-circuit, no prompt, no Settings open.
    if check_microphone() {
        return Ok(MicrophoneRequestResult { granted: true, opened_settings: false });
    }

    #[cfg(target_os = "macos")]
    {
        use objc::runtime::Class;
        use objc::{class, msg_send, sel, sel_impl};

        unsafe {
            // Check the current status. If it's `.notDetermined` (0),
            // we can fire the programmatic prompt and macOS will show
            // its sheet. Otherwise (denied/restricted), we route to
            // System Settings — AVCaptureDevice's request is a no-op
            // post-decision.
            let cls = match Class::get("AVCaptureDevice") {
                Some(c) => c,
                None => {
                    open_settings_pane(
                        "com.apple.preference.security?Privacy_Microphone",
                    )?;
                    return Ok(MicrophoneRequestResult {
                        granted: false,
                        opened_settings: true,
                    });
                }
            };
            use cocoa::base::nil;
            use cocoa::foundation::NSString;
            let media_type: cocoa::base::id =
                NSString::alloc(nil).init_str("soun");
            let status: i64 =
                msg_send![cls, authorizationStatusForMediaType: media_type];
            // 0 = not determined → safe to fire the programmatic prompt.
            // The prompt is async; the result lands when the user
            // clicks Allow/Don't Allow. We don't block on it — just
            // open the System Settings pane in case they hit Don't
            // Allow and want to revisit. Polling on the renderer
            // (refetchInterval) catches the new status when granted.
            if status == 0 {
                // Fire-and-forget prompt. We pass a no-op block.
                // objc-foundation doesn't expose blocks easily; the
                // simplest approach is to call requestAccess with
                // a NULL completion handler — AVFoundation tolerates
                // this (no callback fired, but the prompt still
                // shows and the user's answer is persisted).
                let _: () = msg_send![
                    cls,
                    requestAccessForMediaType: media_type
                    completionHandler: std::ptr::null::<std::ffi::c_void>()
                ];
                return Ok(MicrophoneRequestResult {
                    granted: false, // user hasn't answered yet
                    opened_settings: false,
                });
            }
        }

        // Status was denied/restricted — programmatic prompt is a
        // no-op. Open System Settings instead.
        open_settings_pane("com.apple.preference.security?Privacy_Microphone")?;
        Ok(MicrophoneRequestResult { granted: false, opened_settings: true })
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(MicrophoneRequestResult { granted: true, opened_settings: false })
    }
}
