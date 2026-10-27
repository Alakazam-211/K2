//! k2-ptyd backend for `DaemonPtySession` (PRD `prd-k2-ptyd-v1`, slice S2).
//!
//! Agent PTYs can be hosted by the separate `k2-ptyd` process instead of
//! the daemon, so a restarted daemon adopts them (S3, [`adopt`]): the
//! agent keeps running and its screen is rebuilt by replay. `K2_PTYD=1`
//! makes new `survive` sessions run under ptyd.
//!
//! With the switch off, [`select_backend`] returns `InProcess` before it
//! evaluates anything else: ptyd is never contacted, launched or even
//! looked up, and `DaemonPtySession::spawn` takes its existing
//! alacritty `EventLoop` path unchanged.
//!
//! Design (S0 decision (i), `ptyd-s0-results.md` A1): a ptyd session is
//! driven by a K2-owned IO loop ([`io`]) that owns its vte `Processor`, not
//! by alacritty's `EventLoop` — so `detach()` can `stop_sync` and compute an
//! exact replay offset with the [`shadow`] scanner.

pub mod shadow;

#[cfg(unix)]
pub mod adopt;
#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod env;
#[cfg(unix)]
pub mod io;
#[cfg(unix)]
pub mod snapshot;

use std::sync::Mutex;

/// Called once a session is spawned under k2-ptyd: `(sid, pid, ptyd boot
/// id)`. The daemon binds the session's agent socket record to that ptyd
/// instance and process (prd-vault-gaps-v1 item 3), so a restarted daemon
/// re-serves the socket only to the same process.
pub type SpawnedHook = fn(&str, i32, &str);

static SPAWNED_HOOK: std::sync::OnceLock<SpawnedHook> = std::sync::OnceLock::new();

/// Install the spawn hook (the daemon, once at boot).
pub fn set_spawned_hook(h: SpawnedHook) {
    let _ = SPAWNED_HOOK.set(h);
}

/// A session was spawned under ptyd (`daemon_pty`, after `Spawned`).
pub fn note_spawned(sid: &str, pid: i32, boot_id: &str) {
    if let Some(h) = SPAWNED_HOOK.get() {
        h(sid, pid, boot_id);
    }
}

/// The name ptyd holds a session's per-cell `K2_HOOK_SOCK` listener under
/// (P44): the socket outlives the daemon, and a restarted daemon takes it
/// back instead of rebinding.
pub fn cell_listener_name(sid: &str) -> String {
    format!("cell:{sid}")
}

/// The name ptyd holds the daemon's main TCP listener under (P50).
pub const MAIN_LISTENER_NAME: &str = "main";

/// PRD §7.10 / P65: the oldest k2-ptyd this daemon leaves running. Raise it
/// for a ptyd security fix: a running ptyd below it is exec-upgraded in
/// place to the daemon's own binary on the first owner connection (B0).
/// A 0.1.x ptyd (K2 0.46.2–0.46.4) can't exec-upgrade (`unsupported`), so
/// raising this above 0.1.0 only reaches ptyds from 0.46.5 on.
pub const MIN_PTYD_VERSION: &str = "0.1.0";

/// [`MIN_PTYD_VERSION`], or (debug builds only) `K2_PTYD_TEST_MIN_VERSION`
/// so tests can make the daemon upgrade a ptyd.
pub fn min_ptyd_version() -> String {
    if cfg!(debug_assertions) {
        if let Some(v) = std::env::var("K2_PTYD_TEST_MIN_VERSION").ok().filter(|v| !v.is_empty()) {
            return v;
        }
    }
    MIN_PTYD_VERSION.to_string()
}

/// The P65 decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeDecision {
    /// Leave the running ptyd as it is (why: `current`,
    /// `running_version_unparsable`, `candidate_unknown`,
    /// `candidate_not_newer`).
    Keep(&'static str),
    /// Send `upgrade{exe}` (why: `proto` or `min_version`).
    Upgrade { reason: &'static str, to: String },
}

/// P65 (§7.10): upgrade only when the daemon's own ptyd binary is NEWER
/// than the running one (never a downgrade, never a no-op) AND either the
/// running protocol range is outside the daemon's, or `min_version` is
/// above the running version. `candidate` (the daemon binary's version, a
/// process spawn) runs only when an upgrade is otherwise warranted.
///
/// The `proto` branch is defensive: today's handshake refuses a ptyd with
/// no common protocol before this runs, and the daemon keeps N-1 support
/// (§7.2), so in practice `min_version` is the trigger.
pub fn decide_upgrade(
    running_version: &str,
    running_proto: (u32, u32),
    daemon_proto: (u32, u32),
    min_version: &str,
    candidate: impl FnOnce() -> Option<String>,
) -> UpgradeDecision {
    use k2_ptyd::proto::parse_version;
    let Some(running) = parse_version(running_version) else {
        return UpgradeDecision::Keep("running_version_unparsable");
    };
    let proto_outside = running_proto.1 < daemon_proto.0 || running_proto.0 > daemon_proto.1;
    let below_min = parse_version(min_version).is_some_and(|m| running < m);
    let reason = if proto_outside {
        "proto"
    } else if below_min {
        "min_version"
    } else {
        return UpgradeDecision::Keep("current");
    };
    let Some(to) = candidate() else {
        return UpgradeDecision::Keep("candidate_unknown");
    };
    match parse_version(&to) {
        Some(c) if c > running => UpgradeDecision::Upgrade { reason, to },
        Some(_) => UpgradeDecision::Keep("candidate_not_newer"),
        None => UpgradeDecision::Keep("candidate_unknown"),
    }
}

/// The switch (PRD P67). 0.46.5: on by default on macOS and Linux; an
/// owner turns it off with `terminal.ptyd = off` (Settings → General →
/// Server, `k2 daemon survive off`). `K2_PTYD` in the daemon's env wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    On,
    Off,
}

impl Switch {
    /// `K2_PTYD=1|on|true` → on; anything else, or unset, → off.
    pub fn from_env_value(v: Option<&str>) -> Switch {
        match v.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s == "1" || s == "on" || s == "true" => Switch::On,
            _ => Switch::Off,
        }
    }

    /// P67: `K2_PTYD` in the daemon's env wins (any value: `1|on|true` on,
    /// anything else off); otherwise the app setting `terminal.ptyd`
    /// (0.46.5: `off` → off; `on`, `auto` or unset → on, on Unix).
    pub fn resolve() -> Switch {
        match std::env::var("K2_PTYD") {
            Ok(v) => Switch::from_env_value(Some(&v)),
            Err(_) => Switch::from_setting(&crate::app_settings::load().terminal.ptyd),
        }
    }

    /// The `terminal.ptyd` app setting. 0.46.5 default flip: everything
    /// but an explicit `off` is on (`on`, `auto`, unset), on macOS and
    /// Linux. The in-process path stays the fallback: with ptyd missing,
    /// unreachable or a `KillMode` other than `process`, sessions run
    /// in-process as before (`select_backend`). Windows waits for S8.
    pub fn from_setting(v: &str) -> Switch {
        if v.trim().eq_ignore_ascii_case("off") || !cfg!(unix) {
            Switch::Off
        } else {
            Switch::On
        }
    }
}

/// Which backend a spawn uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    InProcess(Reason),
    Ptyd,
}

/// Why a spawn stayed in-process (P27: logged once per boot and per change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// `DaemonPtyConfig::survive` is false (host sessions, LLM login, tests,
    /// legacy owners — N8/N9). Never logged: it's the normal case.
    NotSurvive,
    /// The switch is off (the default).
    SwitchOff,
    /// microVM cells stay in-process (N2).
    Sandbox,
    /// ptyd couldn't be found, launched or connected.
    Unavailable(String),
    /// Linux under systemd with a `KillMode` other than `process` (P46).
    KillMode(String),
}

impl Reason {
    pub fn code(&self) -> String {
        match self {
            Reason::NotSurvive => "not_survive".into(),
            Reason::SwitchOff => "switch_off".into(),
            Reason::Sandbox => "sandbox".into(),
            Reason::Unavailable(r) => format!("unavailable:{r}"),
            Reason::KillMode(m) => format!("killmode_{m}"),
        }
    }
}

/// Backend selection (PRD §7.5). Evaluated in order, stopping at the first
/// "no": `survive`, the switch, the sandbox, then (lazily) KillMode and
/// availability. The closures only run when every earlier check passed, so
/// a switched-off daemon never touches ptyd.
pub fn select_backend(
    survive: bool,
    switch: Switch,
    passthrough: bool,
    killmode: impl FnOnce() -> Option<String>,
    available: impl FnOnce() -> Result<(), String>,
) -> Backend {
    if !survive {
        return Backend::InProcess(Reason::NotSurvive);
    }
    if switch == Switch::Off {
        return Backend::InProcess(Reason::SwitchOff);
    }
    if !passthrough {
        return Backend::InProcess(Reason::Sandbox);
    }
    if let Some(mode) = killmode() {
        if mode != "process" {
            return Backend::InProcess(Reason::KillMode(mode));
        }
    }
    match available() {
        Ok(()) => Backend::Ptyd,
        Err(r) => Backend::InProcess(Reason::Unavailable(r)),
    }
}

static LAST_REASON: Mutex<Option<String>> = Mutex::new(None);

/// Current ptyd status for `/boot-status` (P27): `on`, or `off (<reason>)`.
pub fn status_line() -> String {
    match LAST_REASON.lock().map(|g| g.clone()) {
        Ok(Some(r)) if r == "on" => "on".into(),
        Ok(Some(r)) => format!("off ({r})"),
        // No survive spawn yet this boot: "on" when this daemon is already
        // connected to a ptyd (it adopted sessions, or holds the main
        // listener there).
        #[cfg(unix)]
        _ if client::global().current_owner().is_some() => "on".into(),
        _ => match Switch::resolve() {
            Switch::Off => "off (switch_off)".into(),
            Switch::On => "off (no survive spawn yet)".into(),
        },
    }
}

/// Log a selection outcome once per boot and per change (P27).
pub fn note_selection(b: &Backend) {
    let code = match b {
        Backend::Ptyd => "on".to_string(),
        // The default (switch off) and non-survive spawns log nothing, so a
        // switched-off daemon's output is unchanged.
        Backend::InProcess(Reason::NotSurvive) | Backend::InProcess(Reason::SwitchOff) => return,
        Backend::InProcess(r) => r.code(),
    };
    let mut g = match LAST_REASON.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if g.as_deref() != Some(code.as_str()) {
        crate::log_debug!("[ptyd] backend for survive sessions: {}", if code == "on" { "ptyd".to_string() } else { format!("in-process ({code})") });
        *g = Some(code);
    }
}

/// Linux under systemd: the daemon unit's current `KillMode` (P46/P87),
/// re-read on every call (the stage-H drop-in converges after ready).
/// `None` when not under systemd, or not Linux.
pub fn current_killmode() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // systemd sets INVOCATION_ID for every service it starts.
        std::env::var_os("INVOCATION_ID")?;
        let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let unit = cg.lines().find_map(|l| l.rsplit('/').find(|seg| seg.ends_with(".service")).map(str::to_string))?;
        let user = cg.contains("/user@");
        let mut cmd = std::process::Command::new("systemctl");
        if user {
            cmd.arg("--user");
        }
        let out = cmd.args(["show", "-p", "KillMode", "--value", &unit]).output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !v.is_empty() {
            Some(v)
        } else {
            Some("unknown".into())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn switch_parses_only_explicit_on_values() {
        assert_eq!(Switch::from_env_value(None), Switch::Off);
        assert_eq!(Switch::from_env_value(Some("")), Switch::Off);
        assert_eq!(Switch::from_env_value(Some("0")), Switch::Off);
        assert_eq!(Switch::from_env_value(Some("auto")), Switch::Off, "auto is a 0.46.4 value, off in 0.46.2");
        assert_eq!(Switch::from_env_value(Some("1")), Switch::On);
        assert_eq!(Switch::from_env_value(Some(" ON ")), Switch::On);
    }

    #[test]
    fn setting_is_on_unless_explicitly_off() {
        assert_eq!(Switch::from_setting(""), Switch::On, "unset = on (the 0.46.5 default)");
        assert_eq!(Switch::from_setting("auto"), Switch::On);
        assert_eq!(Switch::from_setting("on"), Switch::On);
        assert_eq!(Switch::from_setting(" On "), Switch::On);
        assert_eq!(Switch::from_setting("off"), Switch::Off, "the owner's off always wins");
        assert_eq!(Switch::from_setting(" OFF "), Switch::Off);
    }

    #[test]
    fn switch_off_never_evaluates_killmode_or_availability() {
        let touched = Cell::new(0);
        for survive in [true, false] {
            for passthrough in [true, false] {
                let b = select_backend(
                    survive,
                    Switch::Off,
                    passthrough,
                    || {
                        touched.set(touched.get() + 1);
                        Some("process".into())
                    },
                    || {
                        touched.set(touched.get() + 1);
                        Ok(())
                    },
                );
                assert!(matches!(b, Backend::InProcess(_)), "{b:?}");
            }
        }
        assert_eq!(touched.get(), 0, "a switched-off daemon must not look for ptyd at all");
    }

    #[test]
    fn upgrade_only_when_needed_and_newer_and_never_down() {
        let d = (1, 1);
        let never = || -> Option<String> { panic!("the candidate binary must not be probed") };
        // Current and in range: no probe at all.
        assert_eq!(decide_upgrade("0.2.0", (1, 1), d, "0.1.0", never), UpgradeDecision::Keep("current"));
        assert_eq!(decide_upgrade("0.1.0", (1, 1), d, "0.1.0", never), UpgradeDecision::Keep("current"), "equal to the minimum is fine");
        // Below the minimum: upgrade to a newer binary.
        assert_eq!(
            decide_upgrade("0.1.0", (1, 1), d, "0.2.0", || Some("0.2.0".into())),
            UpgradeDecision::Upgrade { reason: "min_version", to: "0.2.0".into() }
        );
        // ...but never to the same or an older one (a downgrade).
        assert_eq!(decide_upgrade("0.1.0", (1, 1), d, "0.2.0", || Some("0.1.0".into())), UpgradeDecision::Keep("candidate_not_newer"));
        assert_eq!(decide_upgrade("0.1.5", (1, 1), d, "0.2.0", || Some("0.1.4".into())), UpgradeDecision::Keep("candidate_not_newer"));
        assert_eq!(decide_upgrade("0.1.0", (1, 1), d, "0.2.0", || None), UpgradeDecision::Keep("candidate_unknown"));
        assert_eq!(decide_upgrade("0.1.0", (1, 1), d, "0.2.0", || Some("garbage".into())), UpgradeDecision::Keep("candidate_unknown"));
        // Protocol outside the daemon's range (either side).
        assert_eq!(
            decide_upgrade("0.3.0", (2, 3), (4, 5), "0.1.0", || Some("0.4.0".into())),
            UpgradeDecision::Upgrade { reason: "proto", to: "0.4.0".into() }
        );
        assert_eq!(decide_upgrade("0.3.0", (2, 3), (3, 5), "0.1.0", never), UpgradeDecision::Keep("current"), "overlap: in range");
        assert_eq!(decide_upgrade("0.3.0", (6, 6), (4, 5), "0.1.0", || Some("0.2.0".into())), UpgradeDecision::Keep("candidate_not_newer"));
        // A ptyd that reports nonsense is left alone.
        assert_eq!(decide_upgrade("x", (1, 1), d, "9.9.9", never), UpgradeDecision::Keep("running_version_unparsable"));
        // The shipped minimum never upgrades the 0.46.3/0.46.4 ptyd (0.1.0),
        // which can't exec-upgrade anyway.
        assert_eq!(decide_upgrade("0.1.0", (1, 1), d, MIN_PTYD_VERSION, never), UpgradeDecision::Keep("current"));
    }

    #[test]
    fn selection_order_and_reasons() {
        let ok = || Ok(());
        let none = || None;
        assert_eq!(select_backend(false, Switch::On, true, none, ok), Backend::InProcess(Reason::NotSurvive));
        assert_eq!(select_backend(true, Switch::On, false, none, ok), Backend::InProcess(Reason::Sandbox));
        assert_eq!(
            select_backend(true, Switch::On, true, || Some("control-group".into()), ok),
            Backend::InProcess(Reason::KillMode("control-group".into()))
        );
        assert_eq!(
            select_backend(true, Switch::On, true, none, || Err("no_binary".into())),
            Backend::InProcess(Reason::Unavailable("no_binary".into()))
        );
        assert_eq!(select_backend(true, Switch::On, true, || Some("process".into()), ok), Backend::Ptyd);
        assert_eq!(select_backend(true, Switch::On, true, none, ok), Backend::Ptyd);
        assert_eq!(Reason::KillMode("mixed".into()).code(), "killmode_mixed");
    }
}
