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

/// The 0.46.2 switch (PRD P67). Dev-only: only the env var is honoured; the
/// `terminal.ptyd` app setting arrives (hidden → opt-in) in 0.46.3.
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
    /// (`on` → on; empty, `off` or `auto` → off in 0.46.4).
    pub fn resolve() -> Switch {
        match std::env::var("K2_PTYD") {
            Ok(v) => Switch::from_env_value(Some(&v)),
            Err(_) => Switch::from_setting(&crate::app_settings::load().terminal.ptyd),
        }
    }

    /// The `terminal.ptyd` app setting: only an explicit `on` is on.
    pub fn from_setting(v: &str) -> Switch {
        if v.trim().eq_ignore_ascii_case("on") {
            Switch::On
        } else {
            Switch::Off
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
    fn setting_is_on_only_when_explicitly_on() {
        assert_eq!(Switch::from_setting(""), Switch::Off, "unset = off (the default)");
        assert_eq!(Switch::from_setting("off"), Switch::Off);
        assert_eq!(Switch::from_setting("auto"), Switch::Off, "auto stays off until 0.46.5");
        assert_eq!(Switch::from_setting("on"), Switch::On);
        assert_eq!(Switch::from_setting(" On "), Switch::On);
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
