//! k2-ptyd S3: adopting agent sessions after a daemon restart (PRD
//! prd-k2-ptyd-v1 §7.9, P29–P39, P43, P44, P79, P80, P89).
//!
//! A restart (update, `kickstart -k`, `systemctl restart`, a crash) leaves
//! every ptyd-hosted agent running in k2-ptyd. Boot takes them back:
//!
//! - **B0** ([`b0`]) right after the DB opens, before the first boot
//!   sweep: connect to the ptyd that is already running (never launch
//!   one), take ownership, list. The boot sweeps that assume "no terminal
//!   survives a restart" keep these sessions' rows, stamps and secrets dirs
//!   (`k2_core::terminal::ptyd::adopt::running_sids`).
//! - **B1** ([`b1`]) just before `set_ready`: rebuild each session from the
//!   ring (replay with query replies muted), register it under its
//!   original key and `SessionId`, start its exit observer, re-mark its
//!   LLM slot, drain pending signals, and clean up sessions that exited
//!   while the daemon was down. Bounded to 10 s; the rest are retried in
//!   the background. B1 finishes before `/boot-status` says ready, so the
//!   heartbeat monitor's boot overdue fire and the renderer's reattach see
//!   the adopted sessions (P29, P43).
//! - After ready ([`serve_adopted_cells`]): the per-cell `K2_HOOK_SOCK`
//!   listeners ptyd held across the restart start accepting (P44: never
//!   before ready; connections made meanwhile wait in the backlog).
//!
//! With no ptyd running, B0 is one `stat` and everything here is a no-op:
//! boot is unchanged. Adoption runs whenever a ptyd holds sessions, even
//! with the switch off (P68: the switch affects only new spawns).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::log_debug;
use serde_json::{Map, Value};

/// Build the ptyd meta a spawn site records (PRD §7.5): `base` (workspace
/// uuid, secrets dir) plus the session key and the rest of the identity a
/// restarted daemon needs. Identity only (P72): never env values, tokens
/// or keys.
pub fn spawn_meta(
    mut base: Map<String, Value>,
    key: &str,
    site: &str,
    project_id: Option<&str>,
    program: Option<&str>,
    args: &[String],
    llm: Option<Value>,
) -> Map<String, Value> {
    base.insert("key".into(), key.into());
    base.insert("spawn_site".into(), site.into());
    let canonical = project_id.is_some_and(|p| !p.is_empty() && p == key);
    base.insert("canonical".into(), canonical.into());
    base.insert("hb_terminal".into(), key.contains(":hb:").into());
    if let Some(p) = project_id.filter(|p| !p.is_empty()) {
        base.entry("workspace_uuid").or_insert_with(|| p.into());
    }
    if let Some(conv) = k2_core::workspace::provider_resume::session_id_from_spawn_argv(program.unwrap_or(""), args) {
        base.insert("conversation_id".into(), conv.into());
    }
    base.insert("llm_slot".into(), llm.unwrap_or(Value::Null));
    base
}

/// What boot did, for the log and `/boot-status` (S6 reads [`report`]).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Report {
    pub found_running: usize,
    pub found_exited: usize,
    pub adopted: usize,
    pub ended: usize,
    pub retried_in_background: usize,
    pub skipped: Option<String>,
}

static REPORT: Mutex<Option<Report>> = Mutex::new(None);

/// The last boot's adoption report (`None` before B0).
pub fn report() -> Option<Report> {
    REPORT.lock().ok().and_then(|g| g.clone())
}

fn update_report(f: impl FnOnce(&mut Report)) {
    if let Ok(mut g) = REPORT.lock() {
        f(g.get_or_insert_with(Report::default));
    }
}

/// B0 (§7.9): the adoptable set. Call once, after the DB opens and before
/// the vault boot / heartbeat / activity sweeps.
pub fn b0() {
    #[cfg(unix)]
    {
        let scan = k2_core::terminal::ptyd::adopt::scan();
        update_report(|r| {
            r.found_running = scan.running.len();
            r.found_exited = scan.exited.len();
            r.skipped = scan.skipped.clone();
        });
        match scan.skipped.as_deref() {
            // The common case: no ptyd → today's boot, and nothing logged.
            Some("no_ptyd") => {}
            Some(why) => log_debug!("[ptyd] B0: no adoption ({why})"),
            None => log_debug!(
                "[ptyd] B0: {} running and {} exited session(s) in ptyd",
                scan.running.len(),
                scan.exited.len()
            ),
        }
    }
}

/// P50: the main TCP listener the previous daemon left with ptyd (only
/// when B0 connected to one). Checked against this boot's bind address: a
/// changed LAN setting binds fresh instead.
pub fn take_main_listener() -> Option<k2_core::port_claim::ClaimedPort> {
    #[cfg(unix)]
    {
        use k2_core::terminal::ptyd::{client, MAIN_LISTENER_NAME};
        let owner = client::global().current_owner()?;
        let fd = match owner.take_fd(MAIN_LISTENER_NAME) {
            Ok(Some(fd)) => fd,
            Ok(None) => return None,
            Err(e) => {
                log_debug!("[ptyd] take main listener: {e}");
                return None;
            }
        };
        let listener = std::net::TcpListener::from(fd);
        let addr = listener.local_addr().ok()?;
        if addr.ip() != std::net::IpAddr::V4(k2_core::listen::bind_ip()) {
            log_debug!("[ptyd] held main listener is on {addr}, this boot binds {}; binding fresh", k2_core::listen::bind_ip());
            owner.release_fd(MAIN_LISTENER_NAME);
            return None;
        }
        log_debug!("[ptyd] took the main listener back from ptyd (port {})", addr.port());
        Some(k2_core::port_claim::ClaimedPort { port: addr.port(), listener, reused: true })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// P50: give ptyd a copy of the main listener so a restart never closes
/// the port — when this daemon already talks to a ptyd (B0), or the switch
/// is on (ptyd will host the agents; this launches it). Switch off and no
/// ptyd: nothing.
pub fn hold_main_listener(listener: &std::net::TcpListener) {
    #[cfg(unix)]
    {
        use k2_core::terminal::ptyd::{client, Switch, MAIN_LISTENER_NAME};
        let ptyd = client::global();
        if Switch::resolve() == Switch::Off {
            // Adopted sessions (if any) keep running, but with the switch
            // off ptyd is on its way out: it must not keep the port.
            if let Some(o) = ptyd.current_owner() {
                o.release_fd(MAIN_LISTENER_NAME);
            }
            return;
        }
        let owner = match ptyd.current_owner() {
            Some(o) => o,
            None => match ptyd.owner() {
                Ok(o) => o,
                Err(e) => {
                    log_debug!("[ptyd] main listener not held: {e}");
                    return;
                }
            },
        };
        if let Err(e) = owner.hold_fd(MAIN_LISTENER_NAME, listener) {
            log_debug!("[ptyd] main listener not held: {e}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = listener;
    }
}

/// The session ids B0 found running in ptyd (the boot sweeps' keep set).
/// Empty with no ptyd.
pub fn running_sids() -> std::collections::HashSet<String> {
    #[cfg(unix)]
    {
        k2_core::terminal::ptyd::adopt::running_sids()
    }
    #[cfg(not(unix))]
    {
        std::collections::HashSet::new()
    }
}

/// B1 time bound (P29).
const B1_BOUND: Duration = Duration::from_secs(10);
/// Per-session replay wait inside B1.
const REPLAY_WAIT: Duration = Duration::from_secs(2);

/// Cell listeners taken back from ptyd, served once the daemon is ready.
#[cfg(unix)]
static HELD_CELLS: Mutex<Vec<(k2_core::session::SessionId, std::os::unix::net::UnixListener)>> = Mutex::new(Vec::new());
/// Adopted sessions whose cell socket must be rebound after ready (ptyd
/// held none for them).
#[cfg(unix)]
static REBIND_CELLS: Mutex<Vec<k2_core::session::SessionId>> = Mutex::new(Vec::new());

/// B1 (§7.9): adopt every running B0 session. Runs on the boot thread
/// inside the tokio runtime (the exit observers are tokio tasks), before
/// `set_ready`.
pub fn b1() {
    #[cfg(unix)]
    {
        use k2_core::terminal::ptyd::adopt;
        let running = adopt::running();
        let exited = adopt::exited();
        let Some(owner) = k2_core::terminal::ptyd::client::global().current_owner() else {
            if !running.is_empty() {
                log_debug!("[ptyd] B1: the owner connection from B0 is gone; nothing adopted");
            }
            return;
        };
        cleanup_exited_while_down(&owner, &exited);
        if running.is_empty() {
            // Nothing survived. With the switch off this ptyd has no more
            // use: let it exit (it holds no session).
            if k2_core::terminal::ptyd::Switch::resolve() == k2_core::terminal::ptyd::Switch::Off {
                let _ = owner.request(k2_ptyd::proto::Request::Shutdown { end_sessions: false });
            }
            return;
        }
        let started = Instant::now();
        let mut left = Vec::new();
        for info in running {
            if started.elapsed() >= B1_BOUND {
                left.push(info);
                continue;
            }
            let wait = REPLAY_WAIT.min(B1_BOUND.saturating_sub(started.elapsed()));
            adopt_one(&owner, &info, wait, false);
        }
        if !left.is_empty() {
            update_report(|r| r.retried_in_background = left.len());
            log_debug!("[ptyd] B1: {} session(s) left for the background after {:?}", left.len(), B1_BOUND);
            let handle = tokio::runtime::Handle::current();
            let _ = std::thread::Builder::new().name("ptyd-adopt-late".into()).spawn(move || {
                let _rt = handle.enter();
                for info in left {
                    adopt_one(&owner, &info, REPLAY_WAIT, true);
                }
            });
        }
        if let Some(r) = report() {
            log_debug!(
                "[ptyd] B1: adopted {} session(s), ended {} in {:?}",
                r.adopted,
                r.ended,
                started.elapsed()
            );
        }
    }
}

/// Adopt one running session. `late`: B1's bound has passed and the
/// daemon may already be ready, so its cell listener is served at once.
#[cfg(unix)]
fn adopt_one(owner: &std::sync::Arc<k2_core::terminal::ptyd::client::Owner>, info: &k2_ptyd::proto::SessionInfo, wait: Duration, late: bool) {
    use k2_core::terminal::ptyd::{adopt, client};
    use k2_core::terminal::DaemonPtySession;
    let sid = info.sid.clone();
    let end = |why: &str| {
        log_debug!("[ptyd] B1: ending session={sid} ({why})");
        client::end_sessions(owner, std::slice::from_ref(&sid));
        owner.release_fd(&k2_core::terminal::ptyd::cell_listener_name(&sid));
        if let Some(id) = k2_core::session::SessionId::parse(&sid) {
            crate::session_token::revoke_session(&id);
        }
        adopt::mark_settled(&sid);
        update_report(|r| r.ended += 1);
    };
    let meta = &info.meta;
    let Some(key) = meta.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).map(str::to_string) else {
        // Spawned by a daemon without adoption (0.46.3): nothing to adopt it
        // under. Today's restart semantics for it: end it.
        return end("no session key in its meta");
    };
    // P89: adoption is not a revive — but never two sessions under one key.
    if let Some(existing) = crate::v2_session_map::lookup_by_agent_name(&key) {
        if existing.session_id.to_string() != sid {
            return end(&format!("key {key} already has a live session {}", existing.session_id));
        }
    }
    let session = match DaemonPtySession::adopt(info, std::sync::Arc::clone(owner), wait) {
        Ok(s) => s,
        Err(e) => return end(&format!("rebuild failed: {e}")),
    };
    let session_id = session.session_id;
    // P33: this conversation is running; no path may resume it again.
    let conversation = meta
        .get("conversation_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| k2_core::workspace::provider_resume::session_id_from_spawn_argv(session.program.as_deref().unwrap_or(""), &session.args));
    if let Some(c) = conversation.as_deref() {
        adopt::hold_conversation(c, &sid);
    }
    // P34: register under the original key and SessionId.
    crate::v2_session_map::register(key.clone(), std::sync::Arc::clone(&session));
    crate::v2_spawn::spawn_child_exit_observer(key.clone(), std::sync::Arc::clone(&session), None, None, None, None);
    // P80: the slot is in use by this (same) agent process.
    if let Some(llm) = meta.get("llm_slot").filter(|v| !v.is_null()) {
        crate::llm_accounts_runtime::reseed_after_adoption(llm, info.pid);
    }
    // P44: the cell listener ptyd held across the restart.
    if crate::session_token::scoped_hooks_enabled() {
        match crate::cell_uds::take_held_cell_listener(owner, &session_id) {
            Ok(Some(l)) if late => crate::cell_server::serve_cell(session_id, l, None),
            Ok(Some(l)) => HELD_CELLS.lock().unwrap_or_else(|p| p.into_inner()).push((session_id, l)),
            Ok(None) | Err(_) if late => crate::session_token::activate_cell_uds_for(&session),
            Ok(None) | Err(_) => REBIND_CELLS.lock().unwrap_or_else(|p| p.into_inner()).push(session_id),
        }
    }
    // P39: signals queued for this key while it had no session.
    let pending = crate::pending_live::drain_for_agent(&key);
    for signal in &pending {
        session.write(crate::signal_format::inject_bytes(signal).into_bytes());
        std::thread::sleep(Duration::from_millis(150));
        session.write(b"\r".to_vec());
    }
    adopt::mark_settled(&sid);
    update_report(|r| r.adopted += 1);
    log_debug!(
        "[ptyd] B1: adopted session={} key={} pid={} conversation={} pending_drained={}",
        sid,
        key,
        info.pid,
        conversation.as_deref().unwrap_or("-"),
        pending.len()
    );
}

/// P32: a session that exited while no daemon was attached — revoke its
/// token, drop its cell listener, remove its secrets dir (boot's vault
/// sweep already did: only RUNNING sessions' dirs are kept) and its ptyd
/// record.
#[cfg(unix)]
fn cleanup_exited_while_down(owner: &k2_core::terminal::ptyd::client::Owner, exited: &[k2_ptyd::proto::SessionInfo]) {
    for info in exited {
        if let Some(id) = k2_core::session::SessionId::parse(&info.sid) {
            crate::session_token::revoke_session(&id);
            let _ = std::fs::remove_file(crate::cell_uds::cell_socket_path(&id));
        }
        owner.release_fd(&k2_core::terminal::ptyd::cell_listener_name(&info.sid));
        let _ = owner.request(k2_ptyd::proto::Request::AckExit { sid: info.sid.clone() });
        log_debug!("[ptyd] B1: session={} exited while the daemon was down ({:?})", info.sid, info.state);
    }
}

/// P44: after `set_ready`, serve the cell listeners B1 took back (and
/// rebind the ones ptyd didn't hold).
pub fn serve_adopted_cells() {
    #[cfg(unix)]
    {
        let held: Vec<_> = std::mem::take(&mut *HELD_CELLS.lock().unwrap_or_else(|p| p.into_inner()));
        for (id, l) in held {
            crate::cell_server::serve_cell(id, l, None);
        }
        let rebind: Vec<_> = std::mem::take(&mut *REBIND_CELLS.lock().unwrap_or_else(|p| p.into_inner()));
        for id in rebind {
            if let Some(s) = crate::v2_session_map::lookup_by_session_id(&id) {
                crate::session_token::activate_cell_uds_for(&s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_meta_records_identity_only() {
        let mut base = Map::new();
        base.insert("secrets_dir".into(), "/run/k2/secrets/abc-1234".into());
        let pid = "4f6e9a1c-1111-4222-8333-944455556666";
        let conv = "0b4d1f7e-4f8f-4c66-9d11-3a3e8f6a7c21";
        let m = spawn_meta(
            base,
            pid,
            "spawn",
            Some(pid),
            Some("claude"),
            &["--resume".into(), conv.into()],
            Some(serde_json::json!({"slot": "acct-1", "person": null})),
        );
        assert_eq!(m["key"], pid);
        assert_eq!(m["canonical"], true);
        assert_eq!(m["hb_terminal"], false);
        assert_eq!(m["workspace_uuid"], pid);
        assert_eq!(m["conversation_id"], conv);
        assert_eq!(m["spawn_site"], "spawn");
        assert_eq!(m["llm_slot"]["slot"], "acct-1");
        assert_eq!(m["secrets_dir"], "/run/k2/secrets/abc-1234");

        let hb = spawn_meta(Map::new(), &format!("{pid}:hb:nightly"), "spawn", Some(pid), Some("claude"), &[], None);
        assert_eq!(hb["canonical"], false);
        assert_eq!(hb["hb_terminal"], true);
        assert!(hb.get("conversation_id").is_none());
        assert_eq!(hb["llm_slot"], Value::Null);
    }
}
