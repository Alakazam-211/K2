//! What the renderer asks about native voice. Compiled with or without the
//! `native-voice` feature, so a build without it answers "not here" instead of
//! the renderer having to guess from a failed plugin call.

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NativeVoiceStatus {
    /// This build has the native engine.
    pub available: bool,
}

#[tauri::command]
pub fn voice_native_status() -> NativeVoiceStatus {
    NativeVoiceStatus { available: cfg!(feature = "native-voice") }
}

/// What `voice_native_session_leave` did: `posted` when K2's call id for the
/// target was known and the route went (`status` = the daemon's answer).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NativeSessionLeave {
    pub posted: bool,
    pub status: Option<u16>,
}

/// After the native engine left a call: end K2's own record of it,
/// `POST /cli/voice/session/leave {callId}` with K2's call id from that
/// target's join and the same login. Nothing is posted without that id.
/// Errors are the provider's fixed text (no token).
#[cfg(feature = "native-voice")]
#[tauri::command]
pub async fn voice_native_session_leave(
    target: String,
    provider: tauri::State<'_, crate::native_voice::AppProvider>,
) -> Result<NativeSessionLeave, String> {
    let left = provider.session_leave(&target).await.map_err(|e| e.to_string())?;
    Ok(NativeSessionLeave { posted: left.posted, status: left.status })
}

/// Without the feature there is no native call to leave.
#[cfg(not(feature = "native-voice"))]
#[tauri::command]
pub fn voice_native_session_leave(target: String) -> NativeSessionLeave {
    let _ = target;
    NativeSessionLeave { posted: false, status: None }
}

/// The calling window shows the call with your camera on or paused (`want`),
/// or no longer: the native self-view (native_voice/self_view.rs) shows in a
/// small window of its own beside it while some window wants it. `anchor`:
/// the call view's rect in this window (CSS pixels), for where it sits.
/// Answers whether this app draws the self-view natively (macOS with native
/// voice); false elsewhere, where the page keeps its own. Whether it shows
/// comes as `self-view:state` events.
#[cfg(feature = "native-voice")]
#[tauri::command]
pub fn voice_self_view(
    app: tauri::AppHandle,
    window: tauri::Window,
    want: bool,
    anchor: Option<crate::native_voice::self_view::CssRect>,
) -> bool {
    crate::native_voice::self_view::set_want(&app, window, want, anchor)
}

/// Without the feature there is no native self-view: the page keeps its own.
#[cfg(not(feature = "native-voice"))]
#[tauri::command]
pub fn voice_self_view(want: bool, anchor: Option<serde_json::Value>) -> bool {
    let _ = (want, anchor);
    false
}

/// Quit with a call up: leave it, bounded. Nothing to do without the feature.
#[cfg(feature = "native-voice")]
pub fn leave_on_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    crate::native_voice::leave_on_quit(app);
}

#[cfg(not(feature = "native-voice"))]
pub fn leave_on_quit<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

#[cfg(all(test, not(feature = "native-voice")))]
mod tests {
    use super::*;

    /// Without the feature the renderer stays on the seat path, and nothing else
    /// of native voice is in the build.
    #[test]
    fn a_build_without_the_feature_says_so() {
        assert_eq!(voice_native_status(), NativeVoiceStatus { available: false });
        assert!(!voice_self_view(true, None));
    }
}
