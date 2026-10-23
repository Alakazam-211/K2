//! Event names the thin client raises on its own windows that k2-core's
//! `agent_hooks::HookEvent` also names (one string, two emitters).

/// Pinned-chat refresh broadcast across this app's windows
/// (`HookEvent::ChatRefreshed`; the desktop `k2so_chat_refresh_broadcast`).
pub const CHAT_REFRESHED: &str = "chat:refreshed";
