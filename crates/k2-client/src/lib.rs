//! K2 client-side shared crate: what the thin client (`src-tauri`) needs
//! from the K2 code base without linking `k2-core`.
//!
//! Contents are leaf helpers with no database, no daemon state and no
//! heavy dependencies: K2 home paths, process bootstrap (login-shell PATH,
//! RLIMIT_NOFILE, SIGPIPE), the launchd daemon / menu-bar plists and the
//! bundle layout, the one-time home + launchd-label migrations, the macOS
//! keychain writer and the Connect account-session item, release SHA256SUMS,
//! Design-mode note clamps, editor discovery, perf timers, the air-gap env
//! switch and a read-only view of three `settings.json` fields.
//!
//! `k2-core` depends on this crate and re-exports every module at its old
//! `k2_core::<module>` path, so daemon code is unchanged. Real logic (DB,
//! workspaces, heartbeats, agents) stays in k2-core and is reached by the
//! thin client over the daemon's `/cli/*` routes (daemon-first).

/// Safe `eprintln!` replacement that silently ignores stderr write
/// failures (a Finder-launched app has no tty; `eprintln!` would panic on
/// broken pipe). Same body as `k2_core::log_debug!`.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "{} {}",
            $crate::log_timestamp(),
            format_args!($($arg)*)
        );
    }};
}

/// RFC3339 UTC timestamp (millisecond precision, `Z`) for [`log_debug!`].
pub fn log_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub mod account_session;
pub mod airgap;
pub mod app_settings_view;
pub mod browser_notes;
pub mod daemon_lifecycle;
pub mod download_file;
pub mod editors;
pub mod events;
pub mod login_path;
#[cfg(target_os = "macos")]
pub mod macos_keychain;
pub mod migration_home;
pub mod migration_launchd;
pub mod path_env;
pub mod paths;
pub mod perf;
pub mod process;
pub mod release_sums;
pub mod wake;
#[cfg(windows)]
pub mod win_autostart;
