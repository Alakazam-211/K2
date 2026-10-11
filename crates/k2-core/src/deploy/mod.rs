//! K2 App runtime, stage 1: deploys (`prd-k2-app-runtime-v1` §7, slices
//! S6–S9). Naming C12: this module is `deploy`, never `release`.
//!
//! What lives here (pure and SQLite parts, shared with Garden widget
//! versions):
//! - [`seed`]: the Seed store, `~/.k2/seeds/<hash>/` (immutable,
//!   content-addressed, `createdWith`, data revision, sweep).
//! - [`manifest`] + [`validate`]: the App manifest (`manifest.json`, schema
//!   1, `kind: "app"`) and `k2 app validate` (`--types` blocks on errors).
//! - [`store`]: `seeds`, `apps`, `deployments` rows (migration 0150).
//! - [`audit`]: the hash-chained `deploy_audit` (Apps and Garden widgets).
//! - [`engine`]: the `RuntimeEngine` trait every isolation plugs into
//!   (process in stage 1; Podman/Quadlet in stage 3; krun in stage V).
//! - [`health`]: the HTTP health gate, one implementation above the trait,
//!   with the logged-in probe hook (litmus L4).
//! - [`backoff`]: the K2 supervisor's restart schedule and crash-loop
//!   breaker (pure, fake-clock tested).
//! - [`framing`]: App output framed as untrusted text for agents.
//!
//! The live parts (routes, the process engine, the supervisor, the local
//! route override and drain) are `k2-daemon`'s `apps` module.
//!
//! Dark until `K2_APPS=1` or the Settings switch "Apps (preview)"
//! (`appsPreview`) is on (AR28): every `/cli/app/*` runtime route 404s.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod audit;
pub mod build;
pub mod framing;
pub mod manifest;
pub mod seed;
pub mod store;
pub mod validate;

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `appsPreview` setting into this process (boot and
/// every settings update/reset).
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

fn env_on(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// True when the App runtime preview is on: `K2_APPS` or the persisted
/// `appsPreview` switch. Default OFF. Our own boxes only (§13 S7).
pub fn enabled() -> bool {
    env_on("K2_APPS") || SETTING_ENABLED.load(Ordering::Relaxed)
}

/// `isolation = none` (a plain process as the developer's user) is only
/// for dev loops: off Linux, or with `K2_APPS_DEV=1` (AR28). Stage 1's
/// preview process engine is the one recorded exception (§16 C9).
pub fn dev_mode() -> bool {
    !cfg!(target_os = "linux") || env_on("K2_APPS_DEV")
}

/// Unix seconds.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `~/.k2/seeds/`.
pub fn seeds_dir() -> PathBuf {
    crate::paths::k2_home().join("seeds")
}

/// `~/.k2/apps/<app-id>/` (the App folder: `app.json` mirror, `data/`,
/// `cache/`, `logs/`, and in stage 1 the process engine's `home/`).
pub fn app_dir(app_id: &str) -> PathBuf {
    crate::paths::k2_home().join("apps").join(app_id)
}

/// The per-deployment log (§7.2), capped at [`LOG_CAP_BYTES`].
pub fn deployment_log_path(app_id: &str, deployment_id: &str) -> PathBuf {
    app_dir(app_id).join("logs").join(format!("{deployment_id}.log"))
}

/// Per-deployment log cap (§6.2): 50 MiB, rotated once.
pub const LOG_CAP_BYTES: u64 = 50 << 20;

/// Stable error codes the CLI turns into exit 3 with teaching text (§8.1),
/// except a failed health gate (exit 1) and pending states (exit 2).
pub mod codes {
    pub const APPS_OFF: &str = "apps_disabled";
    pub const APP_NOT_FOUND: &str = "app_not_found";
    pub const APP_EXISTS: &str = "app_exists";
    pub const APP_ID_RESERVED: &str = "app_id_reserved";
    pub const APP_ID_INVALID: &str = "app_id_invalid";
    pub const LABEL_TAKEN: &str = "label_taken";
    pub const SEED_NOT_FOUND: &str = "seed_not_found";
    pub const SEED_CORRUPT: &str = "seed_corrupt";
    pub const SEED_INVALID: &str = "seed_invalid";
    pub const MANIFEST_INVALID: &str = "manifest_invalid";
    pub const FOLDER_OUTSIDE_WORKSPACE: &str = "folder_outside_workspace";
    pub const OWNER_ONLY: &str = "owner_only";
    pub const APPS_MANAGE_OFF: &str = "apps_manage_off";
    pub const FORBIDDEN: &str = "forbidden";
    pub const WRONG_CREDENTIAL: &str = "wrong_credential";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const DEPLOY_IN_PROGRESS: &str = "deploy_in_progress";
    pub const HEALTH_FAILED: &str = "health_failed";
    pub const ENGINE_UNAVAILABLE: &str = "engine_unavailable";
    pub const STATIC_NOT_ON_RUNTIME: &str = "static_not_on_runtime";
    pub const ROLLBACK_CROSSES_MIGRATION: &str = "rollback_crosses_migration";
    pub const NOTHING_TO_ROLL_BACK: &str = "nothing_to_roll_back";
    pub const CRASHLOOPING: &str = "crashlooping";
    pub const SUSPENDED: &str = "suspended";
    pub const TEST_GUEST_UNAVAILABLE: &str = "test_guest_unavailable";
    pub const IS_AN_APP: &str = "is_an_app";
}

/// Deploy timings (§7.2, §6.3). Defaults per the PRD; `K2_APPS_*` env
/// overrides exist for the headless tests and the Linux harness only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Health probe interval (1 s).
    pub probe_every_ms: u64,
    /// One probe's timeout (5 s).
    pub probe_timeout_ms: u64,
    /// The whole health gate (120 s; the manifest may set its own).
    pub deploy_timeout_secs: u64,
    /// Watch window after a switch (5 min).
    pub watch_secs: u64,
    /// Drain for spliced connections of a replaced instance (30 s).
    pub drain_secs: u64,
    /// SIGTERM grace before SIGKILL when an instance stops (30 s).
    pub stop_grace_secs: u64,
    /// A deploy not live after this is `stalled` and its agent is told
    /// (litmus L3; 10 min).
    pub stall_secs: u64,
    /// Periodic health probe of a live instance after its watch window
    /// (60 s; 0 = off).
    pub live_probe_secs: u64,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            probe_every_ms: 1000,
            probe_timeout_ms: 5000,
            deploy_timeout_secs: 120,
            watch_secs: 300,
            drain_secs: 30,
            stop_grace_secs: 30,
            stall_secs: 600,
            live_probe_secs: 60,
        }
    }
}

impl Timings {
    /// Defaults with any `K2_APPS_<FIELD>` override applied.
    pub fn from_env() -> Self {
        fn get(key: &str, dflt: u64) -> u64 {
            std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(dflt)
        }
        let d = Self::default();
        Self {
            probe_every_ms: get("K2_APPS_PROBE_EVERY_MS", d.probe_every_ms),
            probe_timeout_ms: get("K2_APPS_PROBE_TIMEOUT_MS", d.probe_timeout_ms),
            deploy_timeout_secs: get("K2_APPS_DEPLOY_TIMEOUT_SECS", d.deploy_timeout_secs),
            watch_secs: get("K2_APPS_WATCH_SECS", d.watch_secs),
            drain_secs: get("K2_APPS_DRAIN_SECS", d.drain_secs),
            stop_grace_secs: get("K2_APPS_STOP_GRACE_SECS", d.stop_grace_secs),
            stall_secs: get("K2_APPS_STALL_SECS", d.stall_secs),
            live_probe_secs: get("K2_APPS_LIVE_PROBE_SECS", d.live_probe_secs),
        }
    }
}

/// App ids (§2, §8.4): the App's slug on this box, unique box-wide. The
/// publish-label charset (`published_services::is_valid_name`) so the id
/// is also its URL label.
pub fn valid_app_id(id: &str) -> bool {
    crate::published_services::is_valid_name(id) && id.len() <= 40
}

/// Every `k2 app` subcommand (§8.4, AR7) plus `host` and `skin`: an App id
/// never equals one of these.
pub const RESERVED_APP_IDS: &[&str] = &[
    "host", "skin", "init", "validate", "build", "install", "deploy", "rollback", "promote", "uninstall",
    "list", "inspect", "history", "logs", "errors", "top", "audit", "stop", "start", "restart", "exec",
    "shell", "suspend", "resume", "soak", "guest", "config", "approvals", "approve", "deny", "adopt",
    "transfer", "pack", "access", "doctor", "call", "user", "users", "role", "roles", "grant", "grants",
    "token", "tokens", "hydra", "front-door", "help", "verbs", "box",
];

/// Core `k2` nouns an App id can't take either (§8.4).
pub const CORE_NOUNS: &[&str] = &[
    "app", "agent", "api", "calendar", "chat", "clone", "compute", "connections", "daemon", "db",
    "dns", "domain", "feedback", "heartbeat", "hostmail", "inbox", "mail", "msg", "project", "publish",
    "sidecar", "settings", "store", "study", "thread", "tickets", "update", "users", "vault", "wiki",
    "workspace", "zen",
];

/// Why an App id is refused, or `None` when it is free to use (charset and
/// reserved words only; box-wide uniqueness is the caller's DB check).
pub fn app_id_problem(id: &str) -> Option<(&'static str, String)> {
    if !valid_app_id(id) {
        return Some((
            codes::APP_ID_INVALID,
            format!("'{id}' is not an App id: lower-case letters, digits and -, up to 40, like a publish label"),
        ));
    }
    if RESERVED_APP_IDS.contains(&id) || CORE_NOUNS.contains(&id) || crate::skin::is_reserved_nested_label(id) {
        return Some((
            codes::APP_ID_RESERVED,
            format!("'{id}' is reserved (a k2 app subcommand or k2 noun); pick another App id with --as"),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvVar;

    #[test]
    fn preview_flag_env_and_setting() {
        let _g = EnvVar::remove("K2_APPS");
        set_enabled(false);
        assert!(!enabled(), "default off");
        {
            let _on = EnvVar::set("K2_APPS", "1");
            assert!(enabled(), "K2_APPS=1 turns it on");
        }
        set_enabled(true);
        assert!(enabled(), "appsPreview setting turns it on");
        set_enabled(false);
        assert!(!enabled());
    }

    #[test]
    fn reserved_and_invalid_app_ids() {
        assert_eq!(app_id_problem("jim"), None);
        assert_eq!(app_id_problem("client-status"), None);
        for r in ["host", "skin", "deploy", "front-door", "tokens", "zen", "publish"] {
            assert_eq!(app_id_problem(r).map(|p| p.0), Some(codes::APP_ID_RESERVED), "{r}");
        }
        for bad in ["", "Jim", "a b", "../x", "a.b"] {
            assert_eq!(app_id_problem(bad).map(|p| p.0), Some(codes::APP_ID_INVALID), "{bad:?}");
        }
    }

    #[test]
    fn timings_env_override() {
        let _w = EnvVar::set("K2_APPS_WATCH_SECS", "7");
        let t = Timings::from_env();
        assert_eq!(t.watch_secs, 7);
        assert_eq!(t.deploy_timeout_secs, 120);
    }
}
