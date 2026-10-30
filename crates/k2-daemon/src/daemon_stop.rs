//! Stop vs restart (PRD prd-k2-ptyd-v1 §7.6; Rosson's L1–L3, L5).
//!
//! **Quitting the daemon ends agents. Restarting it (update, crash,
//! `kickstart -k`, `systemctl restart`) keeps them.** At shutdown the
//! daemon decides, in order:
//!
//! 1. the stop flag set by `POST /cli/daemon/stop` (or
//!    `/cli/daemon/restart?end_agents=1`) → end;
//! 2. a fresh stop intent `<K2 home>/run/stop-intent.json` (written by the
//!    menu-bar "Quit daemon", `k2 daemon stop`, the desktop red button…;
//!    honoured for 120 s, deleted on read) → end;
//! 3. Linux under systemd: the unit's queued job is a `stop` → end;
//! 4. anything else (SIGTERM with no stop intent, a crash) → keep (L5).
//!
//! **End**: every ptyd session's token is revoked and the session is
//! unregistered (its vault dir goes), then ptyd ends whatever is left and
//! exits (`shutdown{end_sessions:true}`, which also releases the listeners
//! it held). **Keep**: every ptyd session is detached (K2's IO loop stops;
//! the agent keeps running in ptyd for the next daemon to adopt).
//! In-process sessions die with the process either way, as before.
//!
//! None of this runs unless this daemon talked to a ptyd: with the switch
//! off and no ptyd, shutdown is unchanged.
//!
//! S6 hook: the update-apply paths are restarts and need nothing here; a
//! downgrade guard (P62) calls [`end_all_ptyd_sessions`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use k2_core::log_debug;

/// The stop-intent file's freshness window (S-7: same-uid processes can
/// already SIGTERM the daemon; the file is no privilege).
pub const INTENT_MAX_AGE_SECS: u64 = 120;

static STOP_FLAG: AtomicBool = AtomicBool::new(false);

/// Windows: the shutdown decision said quit.
#[cfg(windows)]
static WINDOWS_QUIT: AtomicBool = AtomicBool::new(false);

/// Windows: the daemon's exit code for the keep-alive supervisor: quit
/// (a stop, or a stop intent) or start me again.
#[cfg(windows)]
pub fn windows_exit_code() -> i32 {
    if WINDOWS_QUIT.load(Ordering::SeqCst) {
        crate::keep_alive::EXIT_QUIT
    } else {
        crate::keep_alive::EXIT_RESTART
    }
}

/// `POST /cli/daemon/stop` (and `restart?end_agents=1`): this shutdown
/// ends agents.
pub fn request_end_agents() {
    STOP_FLAG.store(true, Ordering::SeqCst);
}

/// What shutdown does with ptyd sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitPlan {
    EndAgents(String),
    KeepAgents(String),
}

/// `<K2 home>/run/stop-intent.json` (P88: from the K2 home).
pub fn intent_path() -> PathBuf {
    k2_core::paths::k2_home().join("run").join("stop-intent.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Write a stop intent (0600, temp + rename) for `by` (`cli`, `menubar`,
/// `app`, `tray`).
#[cfg(unix)]
pub fn write_intent(by: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = intent_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let nonce = k2_ptyd::auth::hex(&k2_ptyd::auth::random_bytes::<8>()?);
    let body = serde_json::json!({"by": by, "at": now_secs(), "nonce": nonce}).to_string();
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)
}

/// Read and delete the stop intent; `Some(by)` when it is fresh.
pub fn take_intent() -> Option<String> {
    let path = intent_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    intent_fresh(&raw, now_secs())
}

/// Pure part of [`take_intent`]: `Some(by)` when `raw` parses and `at` is
/// within the window (a clock a little ahead is tolerated).
pub fn intent_fresh(raw: &str, now: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let at = v.get("at")?.as_u64()?;
    let by = v.get("by").and_then(|b| b.as_str()).unwrap_or("unknown").to_string();
    if at > now + 5 || now.saturating_sub(at) > INTENT_MAX_AGE_SECS {
        return None;
    }
    Some(by)
}

/// Linux under systemd: the queued job type for this daemon's unit
/// (`stop`, `restart`, …), from `systemctl [--user] list-jobs`.
#[cfg(target_os = "linux")]
fn systemd_job_type() -> Option<String> {
    std::env::var_os("INVOCATION_ID")?;
    let (_user, unit, scope) = crate::root_helper::own_unit();
    let mut cmd = k2_core::bg_proc::command("systemctl");
    if scope == "user" {
        cmd.arg("--user");
    }
    let out = cmd.args(["list-jobs", "--no-legend", "--plain", &unit]).output().ok()?;
    job_type_from_list_jobs(&String::from_utf8_lossy(&out.stdout), &unit)
}

/// `123 k2-daemon.service stop running` → `stop`.
pub fn job_type_from_list_jobs(out: &str, unit: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let cols: Vec<&str> = l.split_whitespace().collect();
        (cols.len() >= 3 && cols[1] == unit).then(|| cols[2].to_string())
    })
}

/// The §7.6 decision, in order.
pub fn decide() -> ExitPlan {
    if STOP_FLAG.load(Ordering::SeqCst) {
        return ExitPlan::EndAgents("stop requested".into());
    }
    if let Some(by) = take_intent() {
        return ExitPlan::EndAgents(format!("stop intent ({by})"));
    }
    #[cfg(target_os = "linux")]
    if let Some(job) = systemd_job_type() {
        if job == "stop" {
            return ExitPlan::EndAgents("systemd stop job".into());
        }
        return ExitPlan::KeepAgents(format!("systemd {job} job"));
    }
    ExitPlan::KeepAgents("restart (no stop intent)".into())
}

/// Shutdown (daemon_main, after the shutdown broadcast, before the tunnel
/// stop): apply the §7.6 decision to ptyd sessions. Not after a hand-off
/// (quiesce already detached them).
pub fn on_shutdown() {
    // Windows (no ptyd yet): the same decision picks the exit code the
    // keep-alive supervisor reads: a stop quits, anything else restarts.
    #[cfg(windows)]
    {
        let plan = decide();
        log_debug!("[daemon] shutdown: {plan:?}");
        WINDOWS_QUIT.store(matches!(plan, ExitPlan::EndAgents(_)), Ordering::SeqCst);
    }
    #[cfg(unix)]
    {
        let Some(owner) = k2_core::terminal::ptyd::client::global().current_owner() else {
            return; // this daemon never used ptyd: today's exit
        };
        match decide() {
            ExitPlan::EndAgents(why) => {
                let n = end_all_ptyd_sessions(&owner);
                log_debug!("[daemon] shutdown: ended {n} ptyd agent session(s): {why}");
            }
            ExitPlan::KeepAgents(why) => {
                let n = detach_all();
                log_debug!("[daemon] shutdown: kept {n} ptyd agent session(s) running for the next daemon: {why}");
            }
        }
    }
}

/// `KeepAgents` / hand-off quiesce: detach every ptyd session. Returns how
/// many.
pub fn detach_all() -> usize {
    crate::v2_session_map::snapshot().into_iter().filter(|(_, s)| keep_for_next_daemon(s)).count()
}

/// Let go of one session for the next daemon: carry its activity row
/// (P37), then detach it. `false` (nothing done) for an in-process
/// session, which dies with this daemon.
pub fn keep_for_next_daemon(s: &std::sync::Arc<k2_core::terminal::DaemonPtySession>) -> bool {
    if s.backend_name() != "ptyd" || s.is_detached() {
        return s.is_detached();
    }
    let sid = s.session_id.to_string();
    if let Err(e) = crate::activity_store::save_for_restart(&sid) {
        log_debug!("[daemon] activity row for {sid} not carried: {e}");
    }
    s.detach()
}

/// `EndAgents`: revoke, unregister (kills through ptyd, removes the vault
/// dir), then end everything else ptyd holds and let it exit. Returns how
/// many registered sessions were ended.
#[cfg(unix)]
pub fn end_all_ptyd_sessions(owner: &k2_core::terminal::ptyd::client::Owner) -> usize {
    let mut n = 0;
    for (key, s) in crate::v2_session_map::snapshot() {
        if s.backend_name() != "ptyd" {
            continue;
        }
        crate::session_token::revoke_session(&s.session_id);
        let _ = std::fs::remove_file(crate::cell_uds::cell_socket_path(&s.session_id));
        crate::v2_session_map::unregister(&key);
        n += 1;
    }
    // Sessions B1 hadn't settled, and the held listeners: ptyd ends them
    // and exits.
    match owner.request(k2_ptyd::proto::Request::Shutdown { end_sessions: true }) {
        Ok(k2_ptyd::proto::Reply::Ok) => {}
        other => log_debug!("[daemon] ptyd shutdown: {other:?}"),
    }
    // S4 / S-5: every ptyd session just ended, so no saved screen is
    // needed (the IO loops may not see their exit before this daemon does).
    k2_core::terminal::ptyd::snapshot::sweep(&std::collections::HashSet::new());
    n
}

/// P58: the update-apply response's `agents_restart`. An update is a
/// restart, so ptyd-hosted agents keep running (the new daemon adopts
/// them; ptyd itself is never replaced by an update before S7's exec
/// upgrade). In-process sessions die with the daemon and are resumed, so
/// any of them means "agents will restart". `None` when this daemon never
/// used ptyd: the response stays as before.
pub fn agents_restart_on_update() -> Option<bool> {
    #[cfg(unix)]
    {
        k2_core::terminal::ptyd::client::global().current_owner()?;
        Some(crate::v2_session_map::snapshot().iter().any(|(_, s)| s.backend_name() != "ptyd"))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The first release whose daemon adopts k2-ptyd sessions (S3).
pub const FIRST_ADOPTING_VERSION: (u64, u64, u64) = (0, 46, 4);

/// P62: is `version` (the binary a rollback would restore) older than the
/// first adopting release? `None` = armed by a daemon too old to record
/// its version = pre-adoption. An unparseable version counts as
/// pre-adoption too (ending agents is the safe side).
pub fn is_pre_adoption(version: Option<&str>) -> bool {
    let Some(v) = version else { return true };
    let mut parts = v.trim().trim_start_matches('v').split(|c: char| c == '.' || c == '-').map(|p| p.parse::<u64>());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) => (a, b, c) < FIRST_ADOPTING_VERSION,
        _ => true,
    }
}

/// P62 (§7.11 downgrade guard): end every session a running k2-ptyd holds
/// and let it exit, before a crash-loop rollback restores a daemon that
/// can't adopt them. Runs early in boot (nothing is registered yet):
/// connects to an EXISTING ptyd only, revokes each session's hook token,
/// then `shutdown{end_sessions:true}`. Returns how many sessions ptyd
/// listed (0 with no ptyd).
pub fn end_ptyd_sessions_for_downgrade() -> usize {
    #[cfg(unix)]
    {
        let ptyd = k2_core::terminal::ptyd::client::global();
        let Ok(owner) = ptyd.connect_existing() else { return 0 };
        let scan = k2_core::terminal::ptyd::adopt::list(&owner);
        let n = scan.running.len() + scan.exited.len();
        for s in scan.running.iter().chain(scan.exited.iter()) {
            if let Some(id) = k2_core::session::SessionId::parse(&s.sid) {
                crate::session_token::revoke_session(&id);
                let _ = std::fs::remove_file(crate::cell_uds::cell_socket_path(&id));
            }
        }
        match owner.request(k2_ptyd::proto::Request::Shutdown { end_sessions: true }) {
            Ok(k2_ptyd::proto::Reply::Ok) => {}
            other => log_debug!("[daemon] downgrade guard: ptyd shutdown: {other:?}"),
        }
        k2_core::terminal::ptyd::snapshot::sweep(&std::collections::HashSet::new());
        n
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// After `POST /cli/daemon/stop` answered: stop the supervisor's job so it
/// doesn't respawn us (launchd `unload -w` from a detached helper, systemd
/// `stop --no-block`), or shut down directly with no supervisor.
pub fn stop_via_supervisor(shutdown_tx: tokio::sync::broadcast::Sender<()>) {
    if let Some(label) = launchd_label() {
        let plist = k2_core::paths::home_dir().unwrap_or_default().join("Library/LaunchAgents").join(format!("{label}.plist"));
        if plist.is_file() && spawn_detached("launchctl", &["unload", "-w", &plist.to_string_lossy()]) {
            log_debug!("[daemon] stop: launchctl unload -w {}", plist.display());
            return;
        }
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("INVOCATION_ID").is_some() {
        let (_user, unit, scope) = crate::root_helper::own_unit();
        let mut args = vec![];
        if scope == "user" {
            args.push("--user");
        }
        args.extend(["stop", "--no-block", unit.as_str()]);
        if spawn_detached("systemctl", &args) {
            log_debug!("[daemon] stop: systemctl {}", args.join(" "));
            return;
        }
    }
    let _ = shutdown_tx.send(());
}

fn launchd_label() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    std::env::var("XPC_SERVICE_NAME").ok().filter(|v| !v.is_empty() && v != "0" && v.contains("k2"))
}

/// Run `prog args` in its own session, stdio to /dev/null, not waited on.
fn spawn_detached(prog: &str, args: &[&str]) -> bool {
    let mut cmd = k2_core::bg_proc::command(prog);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid in the forked child before exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_is_honoured_only_while_fresh() {
        let raw = r#"{"by":"menubar","at":1000,"nonce":"ab"}"#;
        assert_eq!(intent_fresh(raw, 1000).as_deref(), Some("menubar"));
        assert_eq!(intent_fresh(raw, 1000 + INTENT_MAX_AGE_SECS).as_deref(), Some("menubar"));
        assert_eq!(intent_fresh(raw, 1000 + INTENT_MAX_AGE_SECS + 1), None, "stale");
        assert_eq!(intent_fresh(raw, 900), None, "from the future");
        assert_eq!(intent_fresh("not json", 1000), None);
        assert_eq!(intent_fresh(r#"{"by":"cli"}"#, 1000), None, "no time");
    }

    #[test]
    fn rollback_targets_before_0_46_4_are_pre_adoption() {
        assert!(is_pre_adoption(None), "a marker from a daemon that didn't record its version");
        assert!(is_pre_adoption(Some("0.46.3")));
        assert!(is_pre_adoption(Some("0.45.9")));
        assert!(is_pre_adoption(Some("garbage")), "unparseable: the safe side");
        assert!(!is_pre_adoption(Some("0.46.4")));
        assert!(!is_pre_adoption(Some("v0.46.5")));
        assert!(!is_pre_adoption(Some("0.47.0-rc1")));
        assert!(!is_pre_adoption(Some("1.0.0")));
    }

    #[test]
    fn list_jobs_parsing_finds_our_unit_only() {
        let out = "17 other.service start waiting\n42 k2-daemon.service stop running\n";
        assert_eq!(job_type_from_list_jobs(out, "k2-daemon.service").as_deref(), Some("stop"));
        assert_eq!(job_type_from_list_jobs("43 k2-daemon.service restart running\n", "k2-daemon.service").as_deref(), Some("restart"));
        assert_eq!(job_type_from_list_jobs("", "k2-daemon.service"), None);
        assert_eq!(job_type_from_list_jobs(out, "k2so-daemon.service"), None);
    }
}
