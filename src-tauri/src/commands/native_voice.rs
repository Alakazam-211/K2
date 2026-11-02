//! What the renderer asks about native voice. Compiled with or without the
//! `native-voice` feature, so a build without it answers "not here" instead of
//! the renderer having to guess from a failed plugin call.

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NativeVoiceCall {
    /// The saved host's id, as `join` took it.
    pub target: String,
    /// The room's slug.
    pub room: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NativeVoiceStatus {
    /// This build has the native engine.
    pub available: bool,
    /// The live call's server and room, if there is one.
    pub call: Option<NativeVoiceCall>,
}

#[cfg(feature = "native-voice")]
#[tauri::command]
pub fn voice_native_status(app: tauri::AppHandle) -> NativeVoiceStatus {
    NativeVoiceStatus {
        available: true,
        call: crate::native_voice::live_call(&app).map(|c| NativeVoiceCall { target: c.target, room: c.room }),
    }
}

#[cfg(not(feature = "native-voice"))]
#[tauri::command]
pub fn voice_native_status() -> NativeVoiceStatus {
    NativeVoiceStatus { available: false, call: None }
}

/// Quit with a call up: leave it, bounded. Nothing to do without the feature.
#[cfg(feature = "native-voice")]
pub fn leave_on_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    crate::native_voice::leave_on_quit(app);
}

#[cfg(not(feature = "native-voice"))]
pub fn leave_on_quit<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}
