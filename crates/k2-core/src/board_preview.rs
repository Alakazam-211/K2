//! The Board files preview switch (prd-board-tab-v1 §5.2):
//! `board_file_preview` (an owner-only setting) or `K2_BOARD_FILE=1`, default
//! **off**, the `voice_preview` pattern.
//!
//! It gates the `/cli/board/file/*` routes and the `board-file-v1` feature, so
//! Board files can land dark on main and still run in dev mode. A `.k5board`
//! never opening as text (`docs::target`) isn't behind it.
//!
//! **Dogfood only for now: it doesn't turn on for customers until held edits
//! survive a restart** (K2 Cores' condition on keeping them in the tab's memory).
//!
//! The persisted setting is synced in with [`set_enabled`] (daemon boot and
//! every settings update/reset).

use std::sync::atomic::{AtomicBool, Ordering};

/// The env var that turns the preview on (`1`/`true`/`yes`/`on`).
pub const ENV: &str = "K2_BOARD_FILE";

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `board_file_preview` setting into this process.
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// True when the preview is on: [`ENV`] or the persisted switch.
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var(ENV) {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    SETTING_ENABLED.load(Ordering::Relaxed)
}
