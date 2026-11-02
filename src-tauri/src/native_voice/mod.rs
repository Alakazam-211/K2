//! Native voice calls in the desktop app (prd-k2-voice-rooms-v1 §14, N1): Sew's
//! Tauri plugin, registered with K2's sign-in. Built only with the
//! `native-voice` feature.
//!
//! Connection and OS integration only. The daemon owns the rooms; the plugin
//! owns the media and devices of this person's one call; the renderer draws what
//! both say. Tokens and the call credential stay in Rust memory.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_sew_voice::real::CallClientEngine;
use tauri_plugin_sew_voice::sew_call_client::ClientConfig;
use tauri_plugin_sew_voice::{Builder, DeviceKeyStore, VoiceEngine, VoiceService};

pub mod device_keys;
pub mod lazy;
pub mod provider;

use provider::{K2Provider, SavedLogins};

/// The most quitting waits for Sew's leave (vs-live V36).
pub const QUIT_LEAVE_BOUND: Duration = Duration::from_secs(1);

static PROVIDER: OnceLock<Arc<K2Provider<SavedLogins>>> = OnceLock::new();

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

/// The plugin. The engine is made on first use ([`lazy`]), so a person who never
/// joins a call has nothing running, and nothing here can stop the app starting.
pub fn plugin<R: Runtime>(app_identifier: &str) -> TauriPlugin<R> {
    let provider = PROVIDER.get_or_init(|| {
        // reqwest's client only fails to build when TLS can't initialise, which
        // already stops every other request this app makes.
        Arc::new(K2Provider::new(SavedLogins).expect("native voice: HTTP client"))
    });
    let app_id = app_identifier.to_string();
    let engine = lazy::LazyEngine::new(move || {
        let dir = device_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log_debug!("[native-voice] can't create {}: {e}", dir.display());
        }
        let keys: Arc<dyn DeviceKeyStore> = Arc::new(device_keys::K2DeviceKeys::for_app(device_keys_dir()));
        Arc::new(CallClientEngine::with_mls(ClientConfig::new(&app_id, dir).with_device_key_store(keys))) as Arc<dyn VoiceEngine>
    });
    Builder::with_provider(Arc::clone(provider) as Arc<dyn tauri_plugin_sew_voice::JoinProvider>).engine(Arc::new(engine)).build()
}

/// Which server and room the live call is in, for a window opened mid-call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveCall {
    pub target: String,
    pub room: String,
}

pub fn live_call<R: Runtime>(app: &AppHandle<R>) -> Option<LiveCall> {
    let service = app.try_state::<VoiceService>()?;
    if !service.snapshot().state.is_live() {
        return None;
    }
    PROVIDER.get()?.last_target().map(|t| LiveCall { target: t.target, room: t.room })
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
