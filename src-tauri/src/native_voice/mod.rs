//! Native voice calls in the desktop app (prd-k2-voice-rooms-v1 §14, N1): Sew's
//! Tauri plugin, registered with K2's sign-in. Built only with the
//! `native-voice` feature.
//!
//! Connection and OS integration only. The daemon owns the rooms; the plugin
//! owns the media and devices of this person's one call; the renderer draws what
//! both say. Tokens and the call credential stay in Rust memory.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_sew_voice::{Builder, VoiceService};

pub mod device_keys;
pub mod provider;

use provider::{K2Provider, SavedLogins};

/// The most quitting waits for Sew's leave (vs-live V36).
pub const QUIT_LEAVE_BOUND: Duration = Duration::from_secs(1);

fn k2_dir() -> PathBuf {
    k2_client::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2")
}

/// The engine's data folder. With K2's store it holds only the server's pins.
fn device_dir() -> PathBuf {
    k2_dir().join("voice-device")
}

/// K2's device key store's folder ([`device_keys`]): the public records, and on
/// Linux with no Secret Service the key too.
fn device_keys_dir() -> PathBuf {
    k2_dir().join("voice-device-keys")
}

/// The plugin. Nothing runs until someone uses it: Sew's engine lists no audio
/// devices at launch (the first `devices`, `set_device` or `join` does).
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    // reqwest's client only fails to build when TLS can't initialise, which
    // already stops every other request this app makes.
    let provider = K2Provider::new(SavedLogins).expect("native voice: HTTP client");
    Builder::new(provider)
        .data_dir(device_dir())
        .device_key_store(Arc::new(device_keys::K2DeviceKeys::for_app(device_keys_dir())))
        .build()
}

/// Quit: tell Sew now, so the seat frees at once, but never hold the app up.
/// The app `_exit`s right after, so nothing later would run (V36).
pub fn leave_on_quit<R: Runtime>(app: &AppHandle<R>) {
    let Some(service) = app.try_state::<VoiceService>() else { return };
    if !service.snapshot().state.is_live() {
        return;
    }
    let service = service.inner().clone();
    let done = tauri::async_runtime::block_on(async move { tokio::time::timeout(QUIT_LEAVE_BOUND, service.leave()).await });
    if done.is_err() {
        log_debug!("[native-voice] leave on quit passed {}s; Sew's timeout frees the seat", QUIT_LEAVE_BOUND.as_secs());
    }
}

#[cfg(test)]
mod tests;
