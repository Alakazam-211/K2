//! K2 App runtime, stage 1: deploys (`prd-k2-app-runtime-v1` §7, slices
//! S6–S9). Naming C12: this module is `deploy`, never `release`.
//!
//! What lives here (pure and SQLite parts, shared with Garden widget
//! versions):
//! - [`seed`]: the Seed store, `~/.k2/seeds/<hash>/` (immutable,
//!   content-addressed, `createdWith`, data revision, sweep).
//! - [`manifest`] + [`validate`]: the App manifest (`manifest.json`, schema
//!   1, `kind: "app"`) and `k2 app validate` (`--types` blocks on errors).
//! - [`store`]: `seeds`, `apps`, `deployments` rows (migration 0165).
//! - [`audit`]: the hash-chained `deploy_audit` (Apps and Garden widgets).
//! - [`egress`]: per-App egress policy, host list and held widening
//!   requests (migration 0168, S15).
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
//! Dark while the Settings switch "Apps (beta)" (`apps`, default ON since
//! 0.46.3) is off and `K2_APPS` isn't set (AR28): every `/cli/app/*`
//! runtime route 404s. The unwalled process engine is a second, separate
//! owner opt-in ([`process_engine_enabled`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod audit;
pub mod backoff;
pub mod box_service;
pub mod build;
pub mod egress;
pub mod engine;
pub mod framing;
pub mod handover;
pub mod health;
pub mod manifest;
/// Re-host on a K2 update, the pure parts (update continuity S2, 0.46.4).
pub mod rehost;
pub mod seed;
/// Static mode's rules (0.46.7, prd-tauri-core-0467-v1 TC-1/TC-2).
pub mod static_site;
pub mod store;
pub mod validate;
pub mod widget_events;

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);
static SETTING_PROCESS_ENGINE: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `apps` setting into this process (boot and every
/// settings update/reset).
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn setting_raw() -> bool {
    SETTING_ENABLED.load(Ordering::Relaxed)
}

/// Sync the persisted `appsProcessEngine` setting into this process.
pub fn set_process_engine(on: bool) {
    SETTING_PROCESS_ENGINE.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn process_engine_setting_raw() -> bool {
    SETTING_PROCESS_ENGINE.load(Ordering::Relaxed)
}

/// The env force-on for the unwalled process engine.
pub const PROCESS_ENGINE_ENV: &str = "K2_APPS_PROCESS";

fn env_on(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// True when Apps (beta) are on: `K2_APPS` or the persisted `apps`
/// switch (default ON). This opens the guarded container path, the
/// owner-approved process exception and the read/setup routes; it never
/// runs App code unwalled on its own (see [`process_engine_enabled`]).
pub fn enabled() -> bool {
    env_on("K2_APPS") || SETTING_ENABLED.load(Ordering::Relaxed)
}

/// True when the owner opted into the stage-1 **process engine**: App code
/// as the daemon's user with no wall (§16 C9). `K2_APPS_PROCESS=1` or the
/// owner-only setting `appsProcessEngine`. Default OFF: with it off no App
/// runs on the process engine on any OS (the deploy refuses, the
/// supervisor stops what was running) and new Apps start out guarded.
pub fn process_engine_enabled() -> bool {
    env_on(PROCESS_ENGINE_ENV) || SETTING_PROCESS_ENGINE.load(Ordering::Relaxed)
}

/// The default of the owner-only `appsAutoPrepare` setting: K2 prepares a
/// Linux box for walled Apps by itself (through the owner's root helper)
/// after an update. **OFF until the design review of
/// `design-apps-auto-prepare-0463` signs it off**; flipping it is this
/// constant (and its test).
pub const AUTO_PREPARE_DEFAULT: bool = false;

static SETTING_AUTO_PREPARE: AtomicBool = AtomicBool::new(AUTO_PREPARE_DEFAULT);

/// The env override for the automatic box prep: `0`/`off` forces it off,
/// `1`/`on` forces it on (headless boxes, tests).
pub const AUTO_PREPARE_ENV: &str = "K2_APPS_AUTO_PREPARE";

/// Sync the persisted `appsAutoPrepare` setting into this process.
pub fn set_auto_prepare(on: bool) {
    SETTING_AUTO_PREPARE.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn auto_prepare_setting_raw() -> bool {
    SETTING_AUTO_PREPARE.load(Ordering::Relaxed)
}

/// May K2 prepare this box for Apps by itself? The env override wins,
/// else the owner's `appsAutoPrepare` setting.
pub fn auto_prepare_enabled() -> bool {
    match std::env::var(AUTO_PREPARE_ENV).ok().map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "0" | "off" | "false" | "no") => false,
        Some(v) if matches!(v.as_str(), "1" | "on" | "true" | "yes") => true,
        _ => SETTING_AUTO_PREPARE.load(Ordering::Relaxed),
    }
}

/// The owner-only `appsEngineHeal` setting (prd-app-runtime-update-continuity-v1
/// R5, default ON): at every daemon boot K2 installs the `k2-app-engine` that
/// matches its own version, through the root helper, when an engine is
/// installed. `K2_APP_ENGINE_HEAL=off` (or `0`) forces it off.
static SETTING_ENGINE_HEAL: AtomicBool = AtomicBool::new(true);

/// The env opt-out for the engine heal.
pub const ENGINE_HEAL_ENV: &str = "K2_APP_ENGINE_HEAL";

/// Sync the persisted `appsEngineHeal` setting into this process.
pub fn set_engine_heal(on: bool) {
    SETTING_ENGINE_HEAL.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn engine_heal_setting_raw() -> bool {
    SETTING_ENGINE_HEAL.load(Ordering::Relaxed)
}

/// May K2 heal the App engine at boot? The env opt-out wins, else the
/// owner's `appsEngineHeal` setting (default on).
pub fn engine_heal_enabled() -> bool {
    match std::env::var(ENGINE_HEAL_ENV).ok().map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "0" | "off" | "false" | "no") => false,
        _ => SETTING_ENGINE_HEAL.load(Ordering::Relaxed),
    }
}

/// What a refusal says when an App needs the process engine and it is off.
pub const PROCESS_ENGINE_OFF_HINT: &str = "the unwalled process engine is off on this box (it would run App code as \
     the daemon's user): deploy guarded (k2 app deploy <app> <folder|seed> --engine guarded, on Linux with the App \
     engine installed), or the owner turns it on for a dev machine with K2_APPS_PROCESS=1 or the owner-only setting \
     appsProcessEngine";

/// `isolation = none` (a plain process as the developer's user) is only
/// for dev loops: off Linux, or with `K2_APPS_DEV=1` (AR28). Stage 1's
/// process engine (owner opt-in) is the one recorded exception (§16 C9).
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
    /// 0.46.7 (TC-3): `--manifest <file>` given, and the folder has its own
    /// `manifest.json` that differs.
    pub const MANIFEST_CONFLICT: &str = "manifest_conflict";
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
    /// S13: a change that widens the App goes through config, held.
    pub const WIDENING_HELD: &str = "widening_held";
    /// 0.46.3: the Seed's `requires.sew` / `requires.voice` isn't met by
    /// this box's Sew (install and deploy refuse; the hint names the fix).
    pub const REQUIRES_UNMET: &str = "requires_unmet";
    /// prd-app-runtime-update-continuity-v1 A4: the App engine's socket is
    /// missing or nothing answered on it (K2's fault, not the App's).
    pub const ENGINE_NOT_ANSWERING: &str = "engine_not_answering";
    /// A4/R34: the running App engine speaks another protocol than this
    /// daemon (the 2026-10-23 Hearth outage: protocol 2 vs 3).
    pub const ENGINE_PROTOCOL_MISMATCH: &str = "engine_protocol_mismatch";
    /// A4: the engine refused this daemon as its caller (`caller_not_daemon`,
    /// e.g. before a hand-off's MAINPID move).
    pub const ENGINE_CALLER_REFUSED: &str = "engine_caller_refused";
    /// A4: the root helper is missing or too old for a step K2 needs.
    pub const HELPER_NEEDED: &str = "helper_needed";

    /// A4/R6: a failure caused by K2 itself (its engine, its helper), never
    /// by the App. Such a failure never feeds the crash backoff or the
    /// breaker: the App waits for the engine (`waiting_engine`) instead.
    pub fn is_k2_side(code: &str) -> bool {
        matches!(code, ENGINE_NOT_ANSWERING | ENGINE_PROTOCOL_MISMATCH | ENGINE_CALLER_REFUSED | HELPER_NEEDED)
    }
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
    "token", "tokens", "hydra", "front-door", "help", "verbs", "box", "deployment", "status", "engine", "link",
    // 0.46.3: `k2 app resources` (box usage).
    "resources",
    // 0.46.7 (prd-tauri-core-0467-v1, vs-live §8): `k2 app links`.
    "links",
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

/// A per-App memory limit (`k2 app config --memory 8G|512M|none`). `none`
/// = no per-App limit, the owner's explicit choice (Rosson 2026-10-10). An
/// install that names no memory gets 512 MiB (0.46.3, F2), and the box's
/// pool admits budgets (the daemon's `apps::admission`).
pub fn parse_memory(raw: &str) -> Result<Option<i64>, String> {
    let s = raw.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let (num, mult) = match s.char_indices().last() {
        Some((i, 'G' | 'g')) => (&s[..i], 1i64 << 30),
        Some((i, 'M' | 'm')) => (&s[..i], 1i64 << 20),
        _ => (s, 1i64),
    };
    let n: i64 = num.trim().parse().map_err(|_| format!("'{raw}' is not a size; use 8G, 512M or none"))?;
    let bytes = n.checked_mul(mult).ok_or_else(|| format!("'{raw}' is too large"))?;
    if bytes < 32 << 20 {
        return Err(format!("'{raw}' is below 32M; no App runs in that"));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvVar;

    #[test]
    fn memory_limit_parse_defaults_to_none() {
        assert_eq!(parse_memory("none"), Ok(None));
        assert_eq!(parse_memory(""), Ok(None));
        assert_eq!(parse_memory("8G"), Ok(Some(8 << 30)));
        assert_eq!(parse_memory("512m"), Ok(Some(512 << 20)));
        assert!(parse_memory("1M").is_err());
        assert!(parse_memory("lots").is_err());
    }

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
        assert!(enabled(), "the apps setting turns it on");
        set_enabled(false);
        assert!(!enabled());
    }

    /// 0.46.3 safety rule: the Apps switch (and K2_APPS) never turns on the
    /// unwalled process engine; only its own opt-in does.
    #[test]
    fn the_apps_switch_never_opens_the_process_engine() {
        let _g = EnvVar::remove("K2_APPS");
        let _p = EnvVar::remove(PROCESS_ENGINE_ENV);
        set_process_engine(false);
        set_enabled(true);
        assert!(enabled());
        assert!(!process_engine_enabled(), "apps on alone: no process engine");
        {
            let _on = EnvVar::set("K2_APPS", "1");
            assert!(!process_engine_enabled(), "K2_APPS=1 alone: no process engine");
        }
        {
            let _on = EnvVar::set(PROCESS_ENGINE_ENV, "1");
            assert!(process_engine_enabled(), "K2_APPS_PROCESS=1 opts in");
        }
        set_process_engine(true);
        assert!(process_engine_enabled(), "appsProcessEngine opts in");
        set_process_engine(false);
        assert!(!process_engine_enabled());
        set_enabled(false);
    }

    #[test]
    fn reserved_and_invalid_app_ids() {
        assert_eq!(app_id_problem("jim"), None);
        assert_eq!(app_id_problem("client-status"), None);
        for r in ["host", "skin", "deploy", "front-door", "tokens", "zen", "publish", "meet", "chat"] {
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
