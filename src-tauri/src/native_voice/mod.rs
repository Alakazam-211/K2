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
use tauri::{AppHandle, Listener, Manager, Runtime};
use tauri_plugin_sew_voice::wire::{NoiseModel, NoiseRemovalState, EVENT_PREFIX, NOISE_EVENTS};
use tauri_plugin_sew_voice::{Builder, VoiceService};

pub mod device_keys;
pub mod provider;

use provider::{K2Provider, SavedLogins};

/// The most quitting waits for Sew's leave (vs-live V36).
pub const QUIT_LEAVE_BOUND: Duration = Duration::from_secs(1);

fn k2_dir() -> PathBuf {
    k2_client::paths::k2_home()
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

/// The app's copy of the plugin's provider: the leave of K2's own call record
/// ([`K2Provider::session_leave`]), managed as app state.
pub type AppProvider = K2Provider<SavedLogins>;

/// The plugin, and the app's copy of its provider. Nothing runs until someone
/// uses it: Sew's engine lists no audio devices at launch (the first
/// `devices`, `set_device` or `join` does).
pub fn plugin<R: Runtime>() -> (TauriPlugin<R>, AppProvider) {
    // reqwest's client only fails to build when TLS can't initialise, which
    // already stops every other request this app makes.
    let provider = K2Provider::new(SavedLogins).expect("native voice: HTTP client");
    let app_copy = provider.clone();
    let plugin = Builder::new(provider)
        .data_dir(device_dir())
        .device_key_store(Arc::new(device_keys::K2DeviceKeys::for_app(device_keys_dir())))
        .build();
    (plugin, app_copy)
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

/// Log each noise removal change (the plugin's `sew-voice://noise_removal`
/// event, sent when the mode, what runs, the reason or the grace window changes,
/// never per block) as one line in the app's log (stderr), so a call's log says
/// whether it ran DeepFilterNet3 and, if not, why. Settings and counts only: no
/// names, rooms or tokens.
pub fn log_noise_removal<R: Runtime>(app: &AppHandle<R>) {
    app.listen_any(format!("{EVENT_PREFIX}{}", NOISE_EVENTS[0]), |event| match noise_removal_line(event.payload()) {
        Some(line) => log_debug!("[native-voice] {line}"),
        None => log_debug!("[native-voice] noise removal: unreadable event ({} bytes)", event.payload().len()),
    });
}

/// The log line for one `noise_removal` payload, or `None` if it isn't one:
/// `noise removal: mode standard, active standard, model dfn3, reason none,
/// step_downs 0, in_grace false`. `model` is `none` where no model runs (Off, or
/// DeepFilterNet3 not in play). `reason` says why the model isn't DeepFilterNet3
/// (`model_reason`, else the latest `step_down_reason`), else why less runs than
/// was asked (`reason`), else `none`.
pub fn noise_removal_line(payload: &str) -> Option<String> {
    let s: NoiseRemovalState = serde_json::from_str(payload).ok()?;
    let model = s.model.map_or("none", NoiseModel::as_str);
    let model_why = if s.model == Some(NoiseModel::Dfn3) {
        None
    } else {
        s.model_reason.map(|r| r.as_str()).or(s.step_down_reason.map(|r| r.as_str()))
    };
    let reason = model_why.or(s.reason.map(|r| r.as_str())).unwrap_or("none");
    Some(format!(
        "noise removal: mode {}, active {}, model {model}, reason {reason}, step_downs {}, in_grace {}",
        s.mode.as_str(),
        s.active.as_str(),
        s.step_downs,
        s.in_grace
    ))
}

#[cfg(test)]
mod tests;
