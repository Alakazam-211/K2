//! Shared state for the agent-lifecycle HTTP hook server (a.k.a.
//! `agent_hooks`).
//!
//! Just two atomic values — the port the hook server is listening on and
//! the 32-byte hex token required for every request. Kept as a standalone
//! module so k2so-core's terminal backend can read them (it exports the
//! values into child-process environments as `K2SO_HOOK_PORT` and
//! `K2SO_HOOK_TOKEN`) without having to depend on the full `agent_hooks`
//! HTTP server body (which still lives in src-tauri for now).
//!
//! The HTTP server in `src-tauri/src/agent_hooks.rs` is the single writer
//! — it calls [`set_port`] and [`set_token`] once at startup. Everything
//! else (terminal, CLI discovery via `~/.k2so/heartbeat.port`) is a
//! reader.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::OnceLock;

static HOOK_PORT: AtomicU16 = AtomicU16::new(0);
static HOOK_TOKEN: OnceLock<String> = OnceLock::new();

/// Port the hook server is currently bound to. Returns `0` before the
/// server is initialized — callers should check and either retry or omit
/// the `K2SO_HOOK_*` env vars.
pub fn get_port() -> u16 {
    HOOK_PORT.load(Ordering::Relaxed)
}

/// Sets the listening port. Called by the hook server after successful bind.
pub fn set_port(port: u16) {
    HOOK_PORT.store(port, Ordering::Relaxed);
}

/// 32-byte hex token all hook requests must present. Empty string if the
/// server hasn't started yet.
pub fn get_token() -> &'static str {
    HOOK_TOKEN.get().map(|s| s.as_str()).unwrap_or("")
}

/// Sets the request token. Called once at startup; further calls silently
/// no-op (OnceLock::set semantics).
pub fn set_token(token: String) {
    let _ = HOOK_TOKEN.set(token);
}
