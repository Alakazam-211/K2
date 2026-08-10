//! Work-completion-aware reaper for API host-sessions / sandbox cells.
//!
//! ## Model (product lock 2026-08-03 — Scout / Julie / Rosson;
//! completion lifecycle 2026-08 — `k2 done` / `mark_complete`)
//!
//! Persistent-interview cells must survive user think-time and long mid-write
//! turns. A spawn-time spend-cap (`timeout_secs` as hard wall) is incompatible
//! with that shape.
//!
//! - **Working** — inject / register / non-final `k2 respond` → **never**
//!   auto-reaped (no silence reap, no `timeout_secs` wall from spawn).
//!   Continuous productive work may run past 300s+.
//! - **Grace** — after `k2 respond --final` or `k2 done` ([`mark_complete`]),
//!   short window ([`FINAL_GRACE_SECS`] = 10s) then reap (work completed).
//! - **New activity** (inject / live-resume / non-final respond) cancels Grace
//!   and re-enters Working (resets the completion path).
//! - **`timeout_secs`** — still accepted on spawn (JWT lifetime clamp, client
//!   poll budgets); it does **not** kill a Working cell.
//! - **Spend control** — integrator **kill** + capability non-remint / caps,
//!   not mid-write wall.
//!
//! Activity stamped by spawn / message-live / resume inject (re-enters Working).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::session::SessionId;
use tokio::task::JoinHandle;

/// Fallback idle timeout when a request doesn't set `timeout_secs`.
/// (JWT / client budget default; not a Working hard wall.)
pub const DEFAULT_TIMEOUT_SECS: u64 = 180;
const MIN_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 86_400;
const TICK_SECS: u64 = 15;
/// Grace after `--final` before the cell may be reaped as idle-complete.
/// Override with `K2_HOST_SESSION_FINAL_GRACE_SECS` (including `0` for
/// immediate-next-tick reap — useful for smoke/dev; production default 10).
pub const FINAL_GRACE_SECS: u64 = 10;

/// Effective grace seconds (env override, else [`FINAL_GRACE_SECS`]).
pub fn final_grace_secs() -> u64 {
    std::env::var("K2_HOST_SESSION_FINAL_GRACE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(FINAL_GRACE_SECS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Agent is generating / mid-turn. Never auto-reaped.
    Working,
    /// After final respond / done; reap once grace elapses.
    Grace,
}

struct Entry {
    last_activity: Instant,
    /// Spawn/register instant (observability; not a kill clock).
    #[allow(dead_code)]
    registered_at: Instant,
    /// Requested `timeout_secs` (JWT/client budget; not a Working kill).
    #[allow(dead_code)]
    timeout: Duration,
    phase: Phase,
    /// When phase == Grace, reap after this instant (unless re-Working).
    grace_until: Option<Instant>,
}

static REG: LazyLock<Mutex<HashMap<SessionId, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn normalize_timeout(requested: Option<u64>) -> u64 {
    requested
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

/// Register or re-arm. Spawn / dead-resume start **Working** (agent about to run).
pub fn register(id: SessionId, timeout_secs: u64) {
    let secs = timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    let now = Instant::now();
    if let Ok(mut m) = REG.lock() {
        m.insert(
            id,
            Entry {
                last_activity: now,
                registered_at: now,
                timeout: Duration::from_secs(secs),
                phase: Phase::Working,
                grace_until: None,
            },
        );
    }
}

/// Inject / live-resume: stamp activity and mark **Working** (cancels grace).
pub fn stamp(id: &SessionId) {
    mark_working(id);
}

/// Explicit Working (same as stamp; named for call sites).
pub fn mark_working(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        if let Some(e) = m.get_mut(id) {
            e.last_activity = Instant::now();
            e.phase = Phase::Working;
            e.grace_until = None;
        }
    }
}

/// Non-final `k2 respond` — stay Working, refresh activity.
pub fn on_respond(id: &SessionId, final_: bool) {
    if final_ {
        on_respond_final(id);
    } else {
        mark_working(id);
    }
}

/// `k2 respond --final` → enter Grace; may reap after FINAL_GRACE_SECS.
pub fn on_respond_final(id: &SessionId) {
    mark_complete(id);
}

/// Arm Grace for `id` with [`final_grace_secs`] (default 10s; env override).
/// Used by `k2 respond --final` and `k2 done` / `POST /cli/session/complete`.
/// Idempotent re-arm. Does **not** touch the respond drain.
pub fn mark_complete(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        if let Some(e) = m.get_mut(id) {
            let now = Instant::now();
            e.last_activity = now;
            e.phase = Phase::Grace;
            e.grace_until = Some(now + Duration::from_secs(final_grace_secs()));
        }
    }
}

#[allow(dead_code)]
pub fn registered(id: &SessionId) -> bool {
    REG.lock().map(|m| m.contains_key(id)).unwrap_or(false)
}

/// Reaper phase label for status/observability: `"working"` | `"grace"`, or
/// `None` when the session is not registered (caller maps to `"none"`).
pub fn phase_label(id: &SessionId) -> Option<&'static str> {
    REG.lock().ok().and_then(|m| {
        m.get(id).map(|e| match e.phase {
            Phase::Working => "working",
            Phase::Grace => "grace",
        })
    })
}

/// Test/observability: is the cell in Working phase?
#[cfg(test)]
pub fn is_working(id: &SessionId) -> bool {
    phase_label(id) == Some("working")
}

/// Test helper: would the reaper reap this entry at `now`?
#[cfg(test)]
fn would_reap_at(id: &SessionId, now: Instant) -> bool {
    REG.lock()
        .ok()
        .and_then(|m| m.get(id).map(|e| should_reap(e, now)))
        .unwrap_or(false)
}

pub fn unregister(id: &SessionId) {
    if let Ok(mut m) = REG.lock() {
        m.remove(id);
    }
}

/// Resolve `api-…` agent_name from the durable tab index.
///
/// When `ws_path` is set, scoped to that workspace (kill path). Otherwise any
/// matching `api-%` row for the session id (Grace reaper has only SessionId).
/// Fail-closed: DB error / no row → `None`.
fn agent_name_from_tab_index(ws_path: Option<&str>, session_id_str: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(ws) = ws_path {
        let mut stmt = conn
            .prepare(
                "SELECT wts.agent_name FROM workspace_tab_sessions wts \
                 JOIN projects p ON p.id = wts.project_id \
                 WHERE p.path = ?1 AND wts.session_id = ?2 \
                   AND wts.agent_name LIKE 'api-%' \
                 ORDER BY wts.last_seen_at DESC \
                 LIMIT 1",
            )
            .ok()?;
        stmt.query_row(rusqlite::params![ws, session_id_str], |r| r.get::<_, String>(0))
            .ok()
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT wts.agent_name FROM workspace_tab_sessions wts \
                 WHERE wts.session_id = ?1 \
                   AND wts.agent_name LIKE 'api-%' \
                 ORDER BY wts.last_seen_at DESC \
                 LIMIT 1",
            )
            .ok()?;
        stmt.query_row(rusqlite::params![session_id_str], |r| r.get::<_, String>(0))
            .ok()
    }
}

/// Full host/sandbox cell teardown — **kill-parity minus auth and kill tombstone**.
///
/// Used by Grace expiry and (after auth) by
/// [`crate::v1_host_sessions::handle_v1_host_kill`]. Does **not** write the
/// integrator kill tombstone (that is deliberate `/kill` ownership only).
///
/// 1. Resolve `agent_name` via map reverse-lookup + tab-index fallback.  
/// 2. Name found → [`crate::v2_session_map::unregister`] (kill + map + events).  
/// 3. Else if live → bare `sess.kill()`.  
/// 4. Drop reaper REG for the daemon SessionId and, when distinct, the
///    caller-facing/adopted id.  
/// 5. Optionally clear durable `api-%` tab index (see `clear_durable_index`).  
/// 6. Idempotent when already dead / unregistered.
///
/// Quota release stays on the child-exit observer path — this must not
/// double-release (same contract as `/kill`).
///
/// ## Durable index (`clear_durable_index`)
///
/// - **`true` (Grace / work-complete reaper):** delete `api-%` tab rows so
///   GET host-sessions does not retain ended ghosts (Scout list accretion).
/// - **`false` (integrator live kill):** **keep** the durable row so
///   `{"session": id}` dead-resume still finds the conversation handle
///   (prd-v1-host-session-kill-v1 §6 acceptance: dead-resume after kill).
///   Orphan sweeps that deliberately drop the index use the kill handler's
///   not_live path + explicit `clear_durable_api_host_rows`.
pub fn force_teardown_host_session(session_id: &SessionId) {
    // Grace-class default: clear durable index (work completed / auto reap).
    force_teardown_host_session_inner(session_id, None, None, true);
}

/// Like [`force_teardown_host_session`] with workspace + caller-facing id for
/// tab-index name resolution and dual reaper-key clear (adopted ≠ daemon).
/// Clears durable index (Grace / not_live cleanup callers).
pub(crate) fn force_teardown_host_session_ctx(
    session_id: &SessionId,
    ws_path: &str,
    caller_facing_sid: &str,
) {
    force_teardown_host_session_inner(
        session_id,
        Some(ws_path),
        Some(caller_facing_sid),
        true,
    );
}

/// Integrator **live kill**: stop spend (PTY + map + reaper keys) but **keep**
/// the durable `api-%` tab row so the same `sessionId` remains resumable.
pub(crate) fn force_teardown_host_session_preserve_index(
    session_id: &SessionId,
    ws_path: &str,
    caller_facing_sid: &str,
) {
    force_teardown_host_session_inner(
        session_id,
        Some(ws_path),
        Some(caller_facing_sid),
        false,
    );
}

fn force_teardown_host_session_inner(
    session_id: &SessionId,
    ws_path: Option<&str>,
    caller_facing_sid: Option<&str>,
    clear_durable_index: bool,
) {
    let sid_for_tab = caller_facing_sid
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());

    // Prefer map reverse-lookup (O(live)); tab-index is the same fallback as
    // handle_v1_host_kill when the map key is missing but a durable row exists.
    let agent_name = crate::v2_session_map::agent_name_for_session_id(session_id).or_else(|| {
        agent_name_from_tab_index(ws_path, &sid_for_tab).or_else(|| {
            // If kill/reaper only has the daemon id but the tab row stamps the
            // adopted id (or vice versa), try the daemon id string too.
            let daemon_str = session_id.to_string();
            if daemon_str != sid_for_tab {
                agent_name_from_tab_index(ws_path, &daemon_str)
            } else {
                None
            }
        })
    });

    if let Some(ref name) = agent_name {
        // Force unregister: no subscriber guard (same as integrator kill /
        // force:true on /cli/sessions/v2/close). Kills PTY, drops map, emits
        // SessionRemoved + activity idle, clears active_terminal DB.
        let _ = crate::v2_session_map::unregister(name);
    } else if let Some(sess) = crate::v2_session_map::lookup_by_session_id(session_id) {
        // Live somehow without a map key — still kill the PTY so spend stops.
        sess.kill();
    }
    // else: already dead / never registered — idempotent no-op for process/map.

    // Reaper keys on the daemon SessionId; adopted self-mint ids may also be
    // registered (or stamped) under the caller-facing UUID.
    unregister(session_id);
    if let Some(cf) = caller_facing_sid {
        if let Some(parsed) = SessionId::parse(cf) {
            if parsed != *session_id {
                unregister(&parsed);
            }
        }
    }

    // Durable index: Grace clears (no resume after work-complete). Live kill
    // preserves so dead-resume can re-exec under the same conversation id.
    // Prefer agent_name — many host-session rows stamp NULL session_id until
    // post-hoc adoption, so DELETE-by-session_id alone left ghosts forever
    // (Scout residual tabs / GUI audit panes after TUI reaped).
    if clear_durable_index {
        clear_durable_api_tab_rows(
            ws_path,
            session_id,
            caller_facing_sid,
            agent_name.as_deref(),
        );
    }
}

/// Best-effort delete of `workspace_tab_sessions` api-* rows for this session
/// and/or agent key.
fn clear_durable_api_tab_rows(
    ws_path: Option<&str>,
    session_id: &SessionId,
    caller_facing_sid: Option<&str>,
    agent_name: Option<&str>,
) {
    let daemon_s = session_id.to_string();
    let mut ids: Vec<String> = vec![daemon_s.clone()];
    if let Some(cf) = caller_facing_sid {
        if cf != daemon_s {
            ids.push(cf.to_string());
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    for sid in ids {
        let r = if let Some(ws) = ws_path {
            conn.execute(
                "DELETE FROM workspace_tab_sessions \
                 WHERE project_id = (SELECT id FROM projects WHERE path = ?1 LIMIT 1) \
                   AND session_id = ?2 \
                   AND agent_name LIKE 'api-%'",
                rusqlite::params![ws, sid],
            )
        } else {
            conn.execute(
                "DELETE FROM workspace_tab_sessions \
                 WHERE session_id = ?1 AND agent_name LIKE 'api-%'",
                rusqlite::params![sid],
            )
        };
        if let Ok(n) = r {
            if n > 0 {
                log_debug!(
                    "[sandbox-reaper] cleared {n} durable api-* tab row(s) for session={sid}"
                );
            }
        }
    }
    // Load-bearing: stamp can leave session_id NULL (self-mint / race). Clear
    // by agent_name so work-complete never leaves a ghost row.
    if let Some(name) = agent_name {
        if name.starts_with("api-") {
            clear_durable_by_agent_name(&conn, name);
        }
    }
}

/// Delete durable `api-%` tab row(s) keyed by agent map name.
pub fn clear_durable_by_agent_name(conn: &rusqlite::Connection, agent_name: &str) {
    match conn.execute(
        "DELETE FROM workspace_tab_sessions \
         WHERE agent_name = ?1 AND agent_name LIKE 'api-%'",
        rusqlite::params![agent_name],
    ) {
        Ok(n) if n > 0 => {
            log_debug!(
                "[sandbox-reaper] cleared {n} durable api-* tab row(s) for agent={agent_name}"
            );
        }
        _ => {}
    }
}

/// True when this session is in Grace (post `--final` / `k2 done`).
pub fn is_grace(id: &SessionId) -> bool {
    phase_label(id) == Some("grace")
}

/// ChildExit chokepoint for host-minted `api-%` cells.
///
/// - Always drops **non-resumable** rows (`session_id` NULL/empty) for this
///   agent — they cannot dead-resume and were the smoke residual pile.
/// - When Grace is armed (work complete), drops **all** durable rows for
///   this agent (ChildExit often races the Grace tick; map is already
///   empty so tick-only clear-by-session_id missed).
/// - Live-kill preserve path: Grace is not armed and session_id is set →
///   durable row stays for dead-resume.
pub fn on_api_host_child_exit(session_id: &SessionId, agent_name: &str) {
    if !agent_name.starts_with("api-") {
        return;
    }
    let grace = is_grace(session_id);
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Null-sid orphans: never resumable.
    let _ = conn.execute(
        "DELETE FROM workspace_tab_sessions \
         WHERE agent_name = ?1 \
           AND agent_name LIKE 'api-%' \
           AND (session_id IS NULL OR session_id = '')",
        rusqlite::params![agent_name],
    );
    if grace {
        clear_durable_by_agent_name(&conn, agent_name);
    }
}

fn tick() -> Duration {
    let secs = std::env::var("K2_SANDBOX_REAPER_TICK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(TICK_SECS);
    Duration::from_secs(secs)
}

fn should_reap(e: &Entry, now: Instant) -> bool {
    // Working is never auto-reaped (no short hard wall mid-write; no pure
    // idle kill while a turn is open). Grace only.
    match e.phase {
        Phase::Working => false,
        Phase::Grace => e.grace_until.map(|u| now >= u).unwrap_or(false),
    }
}

pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let t = tick();
    log_debug!(
        "[sandbox-reaper] started — tick={}s (work-completion gate)",
        t.as_secs()
    );
    loop {
        tokio::time::sleep(t).await;
        // Scout 0.40.78: drop map entries whose child is already dead so
        // host-sessions list cannot report phantom live:true (restart /
        // KillMode / missed ChildExit). Map is empty right after boot; this
        // catches mid-lifetime zombies.
        crate::v2_session_map::reconcile_dead_children();
        let expired: Vec<SessionId> = {
            let Ok(m) = REG.lock() else { continue };
            let now = Instant::now();
            m.iter()
                .filter(|(_, e)| should_reap(e, now))
                .map(|(id, _)| *id)
                .collect()
        };
        for id in expired {
            // Full teardown (unregister + events), not bare sess.kill() —
            // same chokepoint as /kill minus auth/tombstone (completion PRD D3).
            force_teardown_host_session(&id);
            log_debug!(
                "[sandbox-reaper] reaped sandbox/host cell session={} (full teardown)",
                id
            );
        }
        // Sweep non-resumable host-session ghosts (NULL session_id). These
        // cannot dead-resume and accumulate when ChildExit races Grace clear.
        gc_null_session_api_tab_rows();
    }
}

/// Drop `api-%` durable rows with no session_id (not dead-resume capable).
fn gc_null_session_api_tab_rows() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match conn.execute(
        "DELETE FROM workspace_tab_sessions \
         WHERE agent_name LIKE 'api-%' \
           AND (session_id IS NULL OR session_id = '')",
        [],
    ) {
        Ok(n) if n > 0 => {
            log_debug!(
                "[sandbox-reaper] GC cleared {n} null-session_id api-* tab ghost(s)"
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_survives_idle_window() {
        let id = SessionId::new();
        register(id, 30);
        assert!(is_working(&id));
        let now = Instant::now();
        let entry = Entry {
            last_activity: now - Duration::from_secs(120),
            registered_at: now,
            timeout: Duration::from_secs(180),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(!should_reap(&entry, now), "Working must not idle-reap");
        unregister(&id);
    }

    #[test]
    fn grace_reaps_after_deadline() {
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now,
            timeout: Duration::from_secs(180),
            phase: Phase::Grace,
            grace_until: Some(now - Duration::from_secs(1)),
        };
        assert!(should_reap(&entry, now));
    }

    #[test]
    fn working_survives_past_timeout_secs_wall() {
        // Scout E-1: continuous work at timeout_secs=300 must not die
        // solely because registered_at + wall elapsed (plan b95c7409).
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now - Duration::from_secs(400),
            timeout: Duration::from_secs(300),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(
            !should_reap(&entry, now),
            "Working must not be reaped by timeout_secs hard wall"
        );
    }

    #[test]
    fn working_survives_long_mid_write_silence() {
        // E-1 shape: inject once, then write continuously for > timeout without
        // new API activity stamps — still Working → still alive.
        let now = Instant::now();
        let entry = Entry {
            last_activity: now - Duration::from_secs(400),
            registered_at: now - Duration::from_secs(400),
            timeout: Duration::from_secs(300),
            phase: Phase::Working,
            grace_until: None,
        };
        assert!(
            !should_reap(&entry, now),
            "mid-write silence must not kill Working"
        );
    }

    #[test]
    fn grace_reaps_even_if_wall_not_reached() {
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now,
            timeout: Duration::from_secs(86_400),
            phase: Phase::Grace,
            grace_until: Some(now - Duration::from_secs(1)),
        };
        assert!(should_reap(&entry, now), "Grace after --final still reaps");
    }

    #[test]
    fn final_enters_grace() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        let e = REG.lock().unwrap();
        let ent = e.get(&id).unwrap();
        assert_eq!(ent.phase, Phase::Grace);
        assert!(ent.grace_until.is_some());
        drop(e);
        unregister(&id);
    }

    #[test]
    fn inject_cancels_grace() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        stamp(&id);
        assert!(is_working(&id));
        unregister(&id);
    }

    #[test]
    fn non_final_respond_stays_working() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        on_respond(&id, false);
        assert!(is_working(&id));
        unregister(&id);
    }

    #[test]
    fn grace_not_reaped_before_deadline() {
        let now = Instant::now();
        let entry = Entry {
            last_activity: now,
            registered_at: now,
            timeout: Duration::from_secs(180),
            phase: Phase::Grace,
            grace_until: Some(now + Duration::from_secs(30)),
        };
        assert!(
            !should_reap(&entry, now),
            "Grace must not reap before grace_until"
        );
    }

    /// force_teardown is idempotent when the session is already gone (no map
    /// entry, no reaper REG) — Grace and second-kill must not panic.
    #[test]
    fn force_teardown_idempotent_when_dead() {
        let id = SessionId::new();
        // No register, no map entry.
        force_teardown_host_session(&id);
        force_teardown_host_session(&id);
        assert!(!registered(&id));
        assert!(crate::v2_session_map::lookup_by_session_id(&id).is_none());
    }

    /// force_teardown drops the reaper REG even when the map has no live entry
    /// (already-killed PTY, REG lag) so Grace expiry always clears its key.
    #[test]
    fn force_teardown_clears_reaper_reg_without_map_entry() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        assert!(registered(&id));
        force_teardown_host_session(&id);
        assert!(
            !registered(&id),
            "force_teardown must drop reaper REG even with no live map entry"
        );
    }

    /// When a live map entry exists under agent_name, force_teardown must
    /// unregister (not bare kill-only) so the map is empty afterward —
    /// the Grace-path contract that fixed sess.kill()-only lag.
    ///
    /// `v2_session_map::unregister` emits session events that need a Tokio
    /// runtime (same constraint as other map-teardown tests).
    #[test]
    fn force_teardown_unregisters_map_entry() {
        use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio test runtime");
        rt.block_on(async {
            k2_core::db::init_for_tests();
            let session = DaemonPtySession::spawn(DaemonPtyConfig {
                program: Some("/bin/sleep".to_string()),
                args: vec!["30".to_string()],
                ..Default::default()
            })
            .expect("spawn sleep PTY for force_teardown test");
            let sid = session.session_id;
            let agent = format!("api-force-teardown-{}", sid);
            crate::v2_session_map::register(agent.clone(), session);
            register(sid, 180);
            on_respond_final(&sid);

            assert!(crate::v2_session_map::lookup_by_agent_name(&agent).is_some());
            assert!(registered(&sid));

            force_teardown_host_session(&sid);

            assert!(
                crate::v2_session_map::lookup_by_agent_name(&agent).is_none(),
                "force_teardown must v2_session_map::unregister, not kill-only"
            );
            assert!(crate::v2_session_map::lookup_by_session_id(&sid).is_none());
            assert!(!registered(&sid));
            // Second call must stay idempotent.
            force_teardown_host_session(&sid);
        });
    }

    /// Dual-id: caller-facing adopted id ≠ daemon SessionId — both reaper
    /// keys must clear (kill path / completion PRD A7).
    #[test]
    fn force_teardown_ctx_clears_dual_reaper_keys() {
        let daemon = SessionId::new();
        let adopted = SessionId::new();
        assert_ne!(daemon, adopted);
        register(daemon, 180);
        register(adopted, 180);
        force_teardown_host_session_ctx(&daemon, "/tmp/k2-force-teardown-dual", &adopted.to_string());
        assert!(!registered(&daemon));
        assert!(
            !registered(&adopted),
            "caller-facing reaper key must clear when distinct from daemon id"
        );
    }

    /// Unit A: `k2 done` / `POST /cli/session/complete` arms Grace with
    /// fixed FINAL_GRACE_SECS (same as `--final`).
    #[test]
    fn mark_complete_arms_grace() {
        let id = SessionId::new();
        register(id, 180);
        mark_complete(&id);
        assert!(is_grace(&id));
        assert!(!would_reap_at(&id, Instant::now()));
        assert!(would_reap_at(
            &id,
            Instant::now() + Duration::from_secs(FINAL_GRACE_SECS + 1)
        ));
        unregister(&id);
    }

    /// Inject after mark_complete re-enters Working (cancels Grace).
    #[test]
    fn inject_cancels_grace_after_mark_complete() {
        let id = SessionId::new();
        register(id, 180);
        mark_complete(&id);
        assert!(is_grace(&id));
        stamp(&id);
        assert!(is_working(&id));
        assert!(!would_reap_at(
            &id,
            Instant::now() + Duration::from_secs(FINAL_GRACE_SECS + 1)
        ));
        unregister(&id);
    }

    /// on_respond_final and mark_complete share the same Grace arming path.
    #[test]
    fn on_respond_final_delegates_to_mark_complete() {
        let id = SessionId::new();
        register(id, 180);
        on_respond_final(&id);
        assert!(is_grace(&id));
        unregister(&id);
    }
}
