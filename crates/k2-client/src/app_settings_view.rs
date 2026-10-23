//! Read-only view of the three `~/.k2/settings.json` fields the thin client
//! acts on locally: `keepDaemonOnQuit` (red close button), `airgap`
//! (updater refusals) and `wakeScheduler` (forwarded to the daemon's
//! heartbeat install routes).
//!
//! The daemon owns `settings.json` (`k2_core::app_settings`); this view
//! never writes it. Field names, serde defaults and the "any parse problem
//! → defaults" rule match `k2_core::app_settings::load()` for these fields;
//! k2-core's `client_settings_view_matches_load` test pins that. One
//! difference: when the file is valid JSON but an UNRELATED field has the
//! wrong type, `load()` falls back to defaults for every field, while this
//! view still reads these three.

use serde::Deserialize;
use std::path::PathBuf;

/// `~/.k2/settings.json` (same resolution as `k2_core::app_settings`).
pub fn settings_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("settings.json")
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsView {
    #[serde(default = "default_true")]
    pub keep_daemon_on_quit: bool,
    #[serde(default)]
    pub airgap: bool,
    #[serde(default)]
    pub wake_scheduler: WakeSchedulerView,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeSchedulerView {
    #[serde(default = "default_wake_mode")]
    pub mode: String,
    #[serde(default = "default_wake_interval")]
    pub interval_minutes: u32,
    #[serde(default)]
    pub wake_system: bool,
}

fn default_true() -> bool {
    true
}
fn default_wake_mode() -> String {
    "on_demand".to_string()
}
fn default_wake_interval() -> u32 {
    1
}

impl Default for WakeSchedulerView {
    fn default() -> Self {
        Self { mode: default_wake_mode(), interval_minutes: default_wake_interval(), wake_system: false }
    }
}

impl Default for AppSettingsView {
    fn default() -> Self {
        Self { keep_daemon_on_quit: true, airgap: false, wake_scheduler: WakeSchedulerView::default() }
    }
}

/// Parse raw `settings.json` text. Invalid JSON, or a wrong type on one of
/// these fields, gives the defaults (as `load()` does).
pub fn parse(raw: &str) -> AppSettingsView {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Read `~/.k2/settings.json`. Missing or unreadable → defaults.
pub fn load() -> AppSettingsView {
    match std::fs::read_to_string(settings_file()) {
        Ok(raw) => parse(&raw),
        Err(_) => AppSettingsView::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty_or_garbage() {
        assert_eq!(parse("{}"), AppSettingsView::default());
        assert_eq!(parse("not json"), AppSettingsView::default());
        let d = AppSettingsView::default();
        assert!(d.keep_daemon_on_quit);
        assert!(!d.airgap);
        assert_eq!(d.wake_scheduler.mode, "on_demand");
        assert_eq!(d.wake_scheduler.interval_minutes, 1);
        assert!(!d.wake_scheduler.wake_system);
    }

    #[test]
    fn reads_the_three_fields_and_ignores_the_rest() {
        let v = parse(
            r#"{"keepDaemonOnQuit": false, "airgap": true, "theme": "x",
                "wakeScheduler": {"mode": "heartbeat", "intervalMinutes": 15, "wakeSystem": true}}"#,
        );
        assert!(!v.keep_daemon_on_quit);
        assert!(v.airgap);
        assert_eq!(v.wake_scheduler, WakeSchedulerView { mode: "heartbeat".into(), interval_minutes: 15, wake_system: true });
    }

    #[test]
    fn a_wrong_type_on_a_viewed_field_gives_defaults() {
        assert_eq!(parse(r#"{"keepDaemonOnQuit": "no", "airgap": true}"#), AppSettingsView::default());
        assert_eq!(parse(r#"{"wakeScheduler": {"intervalMinutes": "5"}}"#), AppSettingsView::default());
    }
}
