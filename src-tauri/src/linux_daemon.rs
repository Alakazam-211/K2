//! Linux: bring the LOCAL `k2-daemon` user service onto the app's version.
//!
//! The Linux twin of macOS's `launchctl kickstart` in
//! `check_daemon_version_and_restart` (lib.rs). On Arch the package ships
//! `/usr/bin/k2-daemon` plus the systemd USER unit `k2-daemon.service`;
//! pacman runs as root and cannot restart a user unit, so after
//! `pacman -U` the old daemon keeps running and the new app waits forever
//! on the exact-version gate (ConnectionGate `localPairedPolicy`). Field
//! report: z13flow, 0.46.0, 2026-10-18.
//!
//! What this does, once per call:
//!   1. Only for the local daemon: loopback base URL, a process owned by
//!      this user, and the process systemd reports as the unit's MainPID.
//!      Anything else gets an explanation, never a restart. A remote
//!      daemon is never touched (callers only pass the local client).
//!   2. `systemctl --user daemon-reload`, then
//!      `systemctl --user restart k2-daemon.service` (absolute systemctl).
//!   3. Wait for `/status` to report the target version.
//!   4. Still wrong: read `systemctl --user show` and say why. An
//!      `ExecStart` other than `/usr/bin/k2-daemon` names the drop-in (or
//!      user unit file) that overrides it, with the fix. Drop-ins are
//!      never edited.
//!
//! The renderer reads [`current_state`] (Tauri `local_daemon_update_state`)
//! so the gate says "Updating K2 daemon…" instead of a dead "Connecting…".
//!
//! Everything except the `Command` calls is pure and unit-tested on any
//! platform; call sites are Linux-only.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DAEMON_UNIT: &str = "k2-daemon.service";
/// What the Arch package (and the k2-daemon deb) install and the unit runs.
pub const PACKAGED_DAEMON_EXEC: &str = "/usr/bin/k2-daemon";
const SYSTEMCTL_CANDIDATES: [&str; 2] = ["/usr/bin/systemctl", "/bin/systemctl"];
/// How long to wait for the restarted daemon to answer with the version.
pub const RESTART_WAIT: Duration = Duration::from_secs(45);

/// First existing absolute systemctl path. Never PATH lookup.
pub fn systemctl_path() -> Option<&'static str> {
    SYSTEMCTL_CANDIDATES
        .iter()
        .copied()
        .find(|p| std::path::Path::new(p).is_file())
}

fn argv(systemctl: &str, rest: &[&str]) -> Vec<String> {
    std::iter::once(systemctl)
        .chain(rest.iter().copied())
        .map(str::to_string)
        .collect()
}

pub fn daemon_reload_argv(systemctl: &str) -> Vec<String> {
    argv(systemctl, &["--user", "daemon-reload"])
}

pub fn restart_argv(systemctl: &str) -> Vec<String> {
    argv(systemctl, &["--user", "restart", DAEMON_UNIT])
}

pub fn show_argv(systemctl: &str) -> Vec<String> {
    argv(
        systemctl,
        &[
            "--user",
            "show",
            DAEMON_UNIT,
            "--property=LoadState,ActiveState,MainPID,ExecStart,DropInPaths,FragmentPath",
        ],
    )
}

/// The fields of `systemctl --user show k2-daemon.service` we use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitInfo {
    pub load_state: String,
    pub active_state: String,
    pub main_pid: Option<u32>,
    /// Every `path=` in ExecStart (normally one).
    pub exec_paths: Vec<String>,
    pub drop_in_paths: Vec<String>,
    pub fragment_path: Option<String>,
}

/// Parse `Key=Value` lines from `systemctl show`. ExecStart looks like
/// `{ path=/usr/bin/k2-daemon ; argv[]=/usr/bin/k2-daemon ; ... }`.
pub fn parse_show(text: &str) -> UnitInfo {
    let mut info = UnitInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "LoadState" => info.load_state = value.to_string(),
            "ActiveState" => info.active_state = value.to_string(),
            "MainPID" => info.main_pid = value.parse::<u32>().ok().filter(|p| *p != 0),
            "ExecStart" => {
                for field in value.split(';') {
                    let field = field.trim().trim_start_matches('{').trim();
                    if let Some(p) = field.strip_prefix("path=") {
                        let p = p.trim();
                        if !p.is_empty() {
                            info.exec_paths.push(p.to_string());
                        }
                    }
                }
            }
            "DropInPaths" => {
                info.drop_in_paths = value.split_whitespace().map(str::to_string).collect()
            }
            "FragmentPath" => {
                if !value.is_empty() {
                    info.fragment_path = Some(value.to_string())
                }
            }
            _ => {}
        }
    }
    info
}

fn norm_version(v: &str) -> &str {
    v.trim().trim_start_matches('v')
}

/// The app found a daemon that is not on `target`.
pub fn is_version_mismatch(target: &str, daemon: &str) -> bool {
    norm_version(target) != norm_version(daemon)
}

/// Only `http://127.0.0.0/8:<port>` or `http://[::1]:<port>` counts as the
/// local daemon. Hostnames (even `localhost`) and remote URLs do not.
pub fn is_loopback_base(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

/// What the running daemon told us, plus where we reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub base: String,
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    /// uid owning `/proc/<pid>`; None when unreadable.
    pub pid_uid: Option<u32>,
    pub my_uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPlan {
    /// Versions agree, or a debug build: leave the daemon alone.
    Nothing,
    /// Not the local daemon: never act, not even to explain.
    NotLocal,
    Restart,
    /// Restarting the unit would not help; show this.
    Explain(String),
}

/// Was the unit's ExecStart binary replaced after the daemon started?
/// Uses ctime, not mtime: pacman and dpkg restore the package's mtime
/// (build time) but cannot back-date ctime. 2 s of slack for clock grain.
pub fn binary_changed_since_start(binary_ctime_secs: i64, uptime_secs: u64, now_secs: i64) -> bool {
    let started = now_secs.saturating_sub(uptime_secs.min(i64::MAX as u64) as i64);
    binary_ctime_secs > started.saturating_add(2)
}

/// Pure decision before touching systemd. `binary_changed`: see
/// [`binary_changed_since_start`]; `Some(false)` means a restart would load
/// the very binary that is running, so explain instead of killing sessions
/// for nothing. `None` (unknown) restarts.
pub fn plan_restart(
    target: &str,
    seen: &Observed,
    unit: &UnitInfo,
    binary_changed: Option<bool>,
    debug_build: bool,
) -> RestartPlan {
    if debug_build || !is_version_mismatch(target, &seen.version) {
        return RestartPlan::Nothing;
    }
    if !is_loopback_base(&seen.base) {
        return RestartPlan::NotLocal;
    }
    if seen.pid_uid != Some(seen.my_uid) {
        return RestartPlan::Explain(format!(
            "The K2 daemon answering on this computer (pid {}, v{}) is not running as your user, so K2 will not restart it. The app is v{}.",
            seen.pid,
            norm_version(&seen.version),
            norm_version(target)
        ));
    }
    if unit.load_state != "loaded" {
        return RestartPlan::Explain(format!(
            "The K2 daemon on this computer is v{} and the app is v{}, and there is no {DAEMON_UNIT} user service to restart. Install the K2 package that ships the daemon, then run:\n  systemctl --user daemon-reload && systemctl --user enable --now k2-daemon",
            norm_version(&seen.version),
            norm_version(target)
        ));
    }
    if unit.main_pid != Some(seen.pid) {
        return RestartPlan::Explain(format!(
            "The K2 daemon answering on this computer (pid {}, v{}) is not the {DAEMON_UNIT} user service (its pid is {}). Stop that process, then run:\n  systemctl --user restart k2-daemon",
            seen.pid,
            norm_version(&seen.version),
            unit.main_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "none: the service is not running".to_string())
        ));
    }
    if binary_changed == Some(false) {
        return RestartPlan::Explain(diagnose_after_restart(target, Some(&seen.version), unit));
    }
    RestartPlan::Restart
}

/// Why the daemon is still not on `target` after a restart.
/// `now_version` is None when it never answered.
pub fn diagnose_after_restart(target: &str, now_version: Option<&str>, unit: &UnitInfo) -> String {
    let target = norm_version(target);
    let overridden: Vec<&String> = unit
        .exec_paths
        .iter()
        .filter(|p| p.as_str() != PACKAGED_DAEMON_EXEC)
        .collect();
    if !overridden.is_empty() {
        let runs = overridden
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let culprit = if !unit.drop_in_paths.is_empty() {
            format!(
                "A drop-in overrides ExecStart: {}",
                unit.drop_in_paths.join(", ")
            )
        } else if let Some(frag) = unit
            .fragment_path
            .as_deref()
            .filter(|f| *f != "/usr/lib/systemd/user/k2-daemon.service")
        {
            format!("The unit file in use is {frag}, not the packaged one")
        } else {
            "Its ExecStart is overridden".to_string()
        };
        return format!(
            "{DAEMON_UNIT} starts {runs}, not {PACKAGED_DAEMON_EXEC}, so restarting it cannot load the v{target} daemon. {culprit}. K2 does not edit your systemd files. To use the packaged daemon, move that file out of the way, then run:\n  systemctl --user daemon-reload && systemctl --user restart k2-daemon"
        );
    }
    match now_version {
        None => format!(
            "{DAEMON_UNIT} was restarted but did not answer within {} seconds. Check its log:\n  journalctl --user -u k2-daemon -n 50",
            RESTART_WAIT.as_secs()
        ),
        Some(v) => format!(
            "{PACKAGED_DAEMON_EXEC} is v{} but the app is v{target}. Update the daemon to v{target} (on Arch it comes in the k2 package; with the deb or rpm app it is installed separately), then run:\n  systemctl --user restart k2-daemon",
            norm_version(v)
        ),
    }
}

/// What the gate shows. `phase`: idle | restarting | done | failed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalDaemonUpdateState {
    pub phase: &'static str,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub message: Option<String>,
}

impl LocalDaemonUpdateState {
    const fn idle() -> Self {
        Self {
            phase: "idle",
            from_version: None,
            to_version: None,
            message: None,
        }
    }
}

static STATE: Mutex<LocalDaemonUpdateState> = Mutex::new(LocalDaemonUpdateState::idle());

fn set_state(s: LocalDaemonUpdateState) {
    if let Ok(mut g) = STATE.lock() {
        *g = s;
    }
}

pub fn current_state() -> LocalDaemonUpdateState {
    STATE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| LocalDaemonUpdateState::idle())
}

/// Result of [`reconcile`], for logs and the package installer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReconcileReport {
    /// `unchanged` (already on target / not local / no daemon),
    /// `restarted` (now on target), `failed` (message says why).
    pub outcome: &'static str,
    pub message: Option<String>,
}

#[cfg(unix)]
fn run(argv: &[String]) -> Result<std::process::Output, String> {
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", argv.join(" ")))
}

#[cfg(unix)]
fn read_unit(systemctl: &str) -> UnitInfo {
    match run(&show_argv(systemctl)) {
        Ok(out) => parse_show(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => {
            log_debug!("[linux-daemon] {e}");
            UnitInfo::default()
        }
    }
}

#[cfg(unix)]
fn proc_uid(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}")).ok().map(|m| m.uid())
}

#[cfg(unix)]
fn binary_changed(unit: &UnitInfo, uptime_secs: u64) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let exec = unit.exec_paths.first()?;
    let ctime = std::fs::metadata(exec).ok()?.ctime();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(binary_changed_since_start(ctime, uptime_secs, now))
}

#[cfg(unix)]
fn observe() -> Option<Observed> {
    let client = crate::daemon_client::DaemonClient::try_connect().ok()?;
    let status = client.status().ok()?;
    Some(Observed {
        base: client.base().to_string(),
        version: status.version,
        pid: status.pid,
        uptime_secs: status.uptime_secs,
        pid_uid: proc_uid(status.pid),
        // SAFETY: getuid has no preconditions and cannot fail.
        my_uid: unsafe { libc::getuid() },
    })
}

#[cfg(unix)]
fn fail(from: &str, target: &str, message: String) -> ReconcileReport {
    log_debug!("[linux-daemon] {message}");
    set_state(LocalDaemonUpdateState {
        phase: "failed",
        from_version: Some(norm_version(from).to_string()),
        to_version: Some(norm_version(target).to_string()),
        message: Some(message.clone()),
    });
    ReconcileReport {
        outcome: "failed",
        message: Some(message),
    }
}

/// Bring the local daemon onto `target` (blocking; call off the UI thread).
#[cfg(unix)]
pub fn reconcile(target: &str) -> ReconcileReport {
    let unchanged = ReconcileReport {
        outcome: "unchanged",
        message: None,
    };
    let Some(seen) = observe() else {
        log_debug!("[linux-daemon] local daemon unreachable; nothing to restart");
        return unchanged;
    };
    let Some(systemctl) = systemctl_path() else {
        if is_version_mismatch(target, &seen.version) && is_loopback_base(&seen.base) {
            return fail(
                &seen.version,
                target,
                format!(
                    "The K2 daemon is v{} and the app is v{}, and systemctl was not found. Restart the daemon by hand.",
                    norm_version(&seen.version),
                    norm_version(target)
                ),
            );
        }
        return unchanged;
    };
    let unit = read_unit(systemctl);
    let changed = binary_changed(&unit, seen.uptime_secs);
    match plan_restart(target, &seen, &unit, changed, cfg!(debug_assertions)) {
        RestartPlan::Nothing | RestartPlan::NotLocal => return unchanged,
        RestartPlan::Explain(msg) => return fail(&seen.version, target, msg),
        RestartPlan::Restart => {}
    }
    set_state(LocalDaemonUpdateState {
        phase: "restarting",
        from_version: Some(norm_version(&seen.version).to_string()),
        to_version: Some(norm_version(target).to_string()),
        message: None,
    });
    log_debug!(
        "[linux-daemon] daemon v{} != v{}; restarting {DAEMON_UNIT}",
        seen.version,
        target
    );
    // daemon-reload first: a package upgrade may have changed the unit.
    if let Err(e) = run(&daemon_reload_argv(systemctl)) {
        log_debug!("[linux-daemon] {e}");
    }
    match run(&restart_argv(systemctl)) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return fail(
                &seen.version,
                target,
                format!(
                    "systemctl --user restart k2-daemon failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            )
        }
        Err(e) => return fail(&seen.version, target, e),
    }
    let deadline = Instant::now() + RESTART_WAIT;
    let mut last: Option<String> = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(now) = observe() {
            if !is_version_mismatch(target, &now.version) {
                set_state(LocalDaemonUpdateState {
                    phase: "done",
                    from_version: Some(norm_version(&seen.version).to_string()),
                    to_version: Some(norm_version(target).to_string()),
                    message: None,
                });
                log_debug!("[linux-daemon] daemon now v{}", now.version);
                return ReconcileReport {
                    outcome: "restarted",
                    message: None,
                };
            }
            last = Some(now.version);
        }
    }
    let unit = read_unit(systemctl);
    let msg = diagnose_after_restart(target, last.as_deref(), &unit);
    fail(&seen.version, target, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW_PACKAGED: &str = "LoadState=loaded\nActiveState=active\nMainPID=4242\nExecStart={ path=/usr/bin/k2-daemon ; argv[]=/usr/bin/k2-daemon ; ignore_errors=no ; start_time=[Sat 2026-10-18 09:00:00 CDT] ; stop_time=[n/a] ; pid=4242 ; code=(null) ; status=0/0 }\nDropInPaths=\nFragmentPath=/usr/lib/systemd/user/k2-daemon.service\n";

    const SHOW_DEV_HOP: &str = "LoadState=loaded\nActiveState=active\nMainPID=4242\nExecStart={ path=/home/rosson/dev/K2/target/release/k2-daemon ; argv[]=/home/rosson/dev/K2/target/release/k2-daemon ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=4242 ; code=(null) ; status=0/0 }\nDropInPaths=/home/rosson/.config/systemd/user/k2-daemon.service.d/dev-hop.conf\nFragmentPath=/usr/lib/systemd/user/k2-daemon.service\n";

    fn seen(version: &str) -> Observed {
        Observed {
            base: "http://127.0.0.1:58211".into(),
            version: version.into(),
            pid: 4242,
            uptime_secs: 600,
            pid_uid: Some(1000),
            my_uid: 1000,
        }
    }

    #[test]
    fn systemctl_argv_is_exact() {
        assert_eq!(
            restart_argv("/usr/bin/systemctl"),
            ["/usr/bin/systemctl", "--user", "restart", "k2-daemon.service"]
        );
        assert_eq!(
            daemon_reload_argv("/usr/bin/systemctl"),
            ["/usr/bin/systemctl", "--user", "daemon-reload"]
        );
        assert_eq!(
            show_argv("/usr/bin/systemctl"),
            [
                "/usr/bin/systemctl",
                "--user",
                "show",
                "k2-daemon.service",
                "--property=LoadState,ActiveState,MainPID,ExecStart,DropInPaths,FragmentPath"
            ]
        );
        for p in SYSTEMCTL_CANDIDATES {
            assert!(p.starts_with('/'), "{p} must be absolute");
        }
    }

    #[test]
    fn parse_show_packaged_unit() {
        let u = parse_show(SHOW_PACKAGED);
        assert_eq!(u.load_state, "loaded");
        assert_eq!(u.active_state, "active");
        assert_eq!(u.main_pid, Some(4242));
        assert_eq!(u.exec_paths, ["/usr/bin/k2-daemon"]);
        assert!(u.drop_in_paths.is_empty());
        assert_eq!(
            u.fragment_path.as_deref(),
            Some("/usr/lib/systemd/user/k2-daemon.service")
        );
    }

    #[test]
    fn parse_show_not_found_unit() {
        let u = parse_show("LoadState=not-found\nActiveState=inactive\nMainPID=0\nExecStart=\nDropInPaths=\nFragmentPath=\n");
        assert_eq!(u.load_state, "not-found");
        assert_eq!(u.main_pid, None);
        assert!(u.exec_paths.is_empty());
        assert_eq!(u.fragment_path, None);
    }

    #[test]
    fn mismatch_detection() {
        assert!(is_version_mismatch("0.46.1", "0.46.0"));
        assert!(is_version_mismatch("0.46.1", "0.46.1-dev"));
        assert!(!is_version_mismatch("0.46.1", "0.46.1"));
        assert!(!is_version_mismatch("v0.46.1", " 0.46.1 "));
    }

    #[test]
    fn same_version_or_debug_does_nothing() {
        let u = parse_show(SHOW_PACKAGED);
        assert_eq!(plan_restart("0.46.1", &seen("0.46.1"), &u, None, false), RestartPlan::Nothing);
        assert_eq!(plan_restart("0.46.1", &seen("0.46.0"), &u, None, true), RestartPlan::Nothing);
    }

    #[test]
    fn stale_packaged_daemon_is_restarted() {
        let u = parse_show(SHOW_PACKAGED);
        assert_eq!(plan_restart("0.46.1", &seen("0.46.0"), &u, None, false), RestartPlan::Restart);
        assert_eq!(
            plan_restart("0.46.1", &seen("0.46.0"), &u, Some(true), false),
            RestartPlan::Restart
        );
    }

    #[test]
    fn unchanged_binary_is_explained_without_a_restart() {
        // deb/rpm app update: the daemon is a separate install and its
        // binary did not change, so a restart would only kill sessions.
        let u = parse_show(SHOW_PACKAGED);
        match plan_restart("0.46.1", &seen("0.46.0"), &u, Some(false), false) {
            RestartPlan::Explain(m) => {
                assert!(m.contains("/usr/bin/k2-daemon is v0.46.0 but the app is v0.46.1"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        // z13flow: a dev-hop drop-in runs an old dev build; pacman replaced
        // /usr/bin/k2-daemon but not that file. Name the drop-in at once.
        let u = parse_show(SHOW_DEV_HOP);
        match plan_restart("0.46.0", &seen("0.45.7"), &u, Some(false), false) {
            RestartPlan::Explain(m) => assert!(m.contains("dev-hop.conf"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn binary_change_uses_process_start() {
        let now = 1_800_000_000;
        // Daemon up 600 s; binary replaced 60 s ago.
        assert!(binary_changed_since_start(now - 60, 600, now));
        // Binary older than the process.
        assert!(!binary_changed_since_start(now - 900, 600, now));
        // Within the 2 s grain: not a change.
        assert!(!binary_changed_since_start(now - 599, 600, now));
        // Absurd uptime does not overflow.
        assert!(binary_changed_since_start(now, u64::MAX, now));
    }

    #[test]
    fn loopback_only() {
        for base in ["http://127.0.0.1:58211", "http://127.1.2.3:1", "http://[::1]:58211"] {
            assert!(is_loopback_base(base), "{base}");
        }
        for base in [
            "https://rosson.k2.dev",
            "http://localhost:58211",
            "http://192.168.1.20:58211",
            "https://127.0.0.1:58211",
            "http://0.0.0.0:58211",
            "not a url",
            "",
        ] {
            assert!(!is_loopback_base(base), "{base}");
        }
        let u = parse_show(SHOW_PACKAGED);
        let mut remote = seen("0.46.0");
        remote.base = "https://rosson.k2.dev".into();
        assert_eq!(plan_restart("0.46.1", &remote, &u, None, false), RestartPlan::NotLocal);
    }

    #[test]
    fn another_users_daemon_is_explained_not_restarted() {
        let u = parse_show(SHOW_PACKAGED);
        let mut s = seen("0.46.0");
        s.pid_uid = Some(0);
        match plan_restart("0.46.1", &s, &u, None, false) {
            RestartPlan::Explain(m) => assert!(m.contains("not running as your user"), "{m}"),
            other => panic!("{other:?}"),
        }
        s.pid_uid = None;
        assert!(matches!(plan_restart("0.46.1", &s, &u, None, false), RestartPlan::Explain(_)));
    }

    #[test]
    fn missing_unit_is_explained() {
        let u = parse_show("LoadState=not-found\nMainPID=0\n");
        match plan_restart("0.46.1", &seen("0.46.0"), &u, None, false) {
            RestartPlan::Explain(m) => {
                assert!(m.contains("no k2-daemon.service user service"), "{m}");
                assert!(m.contains("systemctl --user daemon-reload && systemctl --user enable --now k2-daemon"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn daemon_outside_the_unit_is_explained() {
        let mut u = parse_show(SHOW_PACKAGED);
        u.main_pid = Some(1);
        match plan_restart("0.46.1", &seen("0.46.0"), &u, None, false) {
            RestartPlan::Explain(m) => assert!(m.contains("(pid 4242, v0.46.0) is not the k2-daemon.service"), "{m}"),
            other => panic!("{other:?}"),
        }
        u.main_pid = None;
        match plan_restart("0.46.1", &seen("0.46.0"), &u, None, false) {
            RestartPlan::Explain(m) => assert!(m.contains("the service is not running"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dev_hop_drop_in_is_named_with_the_fix() {
        let u = parse_show(SHOW_DEV_HOP);
        assert_eq!(u.exec_paths, ["/home/rosson/dev/K2/target/release/k2-daemon"]);
        let m = diagnose_after_restart("0.46.1", Some("0.46.1-dev"), &u);
        assert!(m.contains("starts /home/rosson/dev/K2/target/release/k2-daemon, not /usr/bin/k2-daemon"), "{m}");
        assert!(m.contains("A drop-in overrides ExecStart: /home/rosson/.config/systemd/user/k2-daemon.service.d/dev-hop.conf"), "{m}");
        assert!(m.contains("does not edit your systemd files"), "{m}");
        assert!(m.contains("systemctl --user daemon-reload && systemctl --user restart k2-daemon"), "{m}");
    }

    #[test]
    fn user_unit_file_override_is_named() {
        let mut u = parse_show(SHOW_DEV_HOP);
        u.drop_in_paths.clear();
        u.fragment_path = Some("/home/rosson/.config/systemd/user/k2-daemon.service".into());
        let m = diagnose_after_restart("0.46.1", None, &u);
        assert!(m.contains("The unit file in use is /home/rosson/.config/systemd/user/k2-daemon.service"), "{m}");
    }

    #[test]
    fn packaged_exec_still_old_says_install_the_package() {
        let u = parse_show(SHOW_PACKAGED);
        let m = diagnose_after_restart("0.46.1", Some("0.46.0"), &u);
        assert!(m.contains("/usr/bin/k2-daemon is v0.46.0 but the app is v0.46.1"), "{m}");
        let m = diagnose_after_restart("0.46.1", None, &u);
        assert!(m.contains("did not answer within 45 seconds"), "{m}");
        assert!(m.contains("journalctl --user -u k2-daemon"), "{m}");
    }
}
