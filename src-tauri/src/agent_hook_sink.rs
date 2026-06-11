//! Tauri-side implementation of
//! `k2_core::agent_hooks::AgentHookEventSink`.
//!
//! Registers at startup in `lib.rs::setup()`. Routes
//! `HookEvent::AgentLifecycle`, `::AgentReply`, `::SyncProjects`,
//! `::SyncSettings`, `::CliTerminalSpawn`,
//! `::CliTerminalSpawnBackground`, and `::CliAiCommit` back onto Tauri's
//! event bus under the same wire-format event names the React frontend
//! already listens for.

use k2_core::agent_hooks::{AgentHookEventSink, HookEvent};
use tauri::{AppHandle, Emitter};

pub struct TauriAgentHookEventSink {
    app_handle: AppHandle,
}

impl TauriAgentHookEventSink {
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }
}

impl AgentHookEventSink for TauriAgentHookEventSink {
    fn emit(&self, event: HookEvent, payload: serde_json::Value) {
        let _ = self.app_handle.emit(event.event_name(), payload);
    }
}
