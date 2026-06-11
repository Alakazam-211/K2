//! Daemon-side Active reaper (task #672, PRD
//! `.k2so/prds/daemon-canonical-active.md` §4.3).
//!
//! A single long-lived tokio task that OWNS the grace-reap of dormant
//! workspace chat PTYs. It replaces the per-client renderer sweeps
//! (`sweepAgedOutWorkspaceChats*` / `scheduleWorkspaceChatReap`), which
//! the renderer agent deletes. Because the decision is driven off the
//! canonical, daemon-owned Active set (one source of truth per daemon),
//! two connected clients can no longer disagree about what's Active and
//! reap a session another client is using — the GH#22 root cause.
//!
//! ## Gates (ACTIVE-ONLY — load-bearing design decision)
//! A chat PTY is reap-eligible iff:
//!   1. `!in_active_set(projectId)` — aged out of the window AND not
//!      `manually_active`, and
//!   2. `!heartbeat_enabled(projectId)` — a heartbeat keeps it warm.
//!
//! There is **no subscriber/attach gate**. Attachment does NOT keep a
//! session alive; *Active* does. This rests on the invariant (PRD
//! §4.3.1) that opening/attaching a workspace chat IS an activation
//! (`POST /cli/projects/activate` bumps `last_interaction_at`), so a
//! client watching a workspace ⇒ it is Active ⇒ the reaper won't touch
//! it. `subscriberCount` stays as data but is never a reap input.
//!
//! ## Grace
//! When a workspace first becomes reap-eligible — either the window-tick
//! finds it aged, or an explicit `dismiss` arms it NOW — a 15s timer
//! ([`DISMISS_REAP_GRACE_MS`]) is armed, keyed by projectId. If the
//! workspace re-enters the Active set within the grace, the timer is
//! cancelled. Re-arm replaces.
//!
//! ## On fire
//! Re-check the Active gates at fire time. If still eligible: persist the
//! chat's `sessionId` so `--resume` has a target (the daemon already
//! knows the PTY's session id), then **force-close** the v2 session by
//! calling [`crate::v2_session_map::unregister`] directly — the lowest
//! daemon close chokepoint. This intentionally bypasses any route-level
//! `v2/close` close-guard (the reaper is Active-authoritative); the
//! guard, which lives in `v2_spawn::handle_v2_close`, stays in force for
//! non-reaper HTTP callers. Emit `ActiveChanged` after the close.
//!
//! ## Boot reconciliation
//! On daemon start, one pass arms timers for already-aged survivors —
//! absorbing the #663 boot-survivor gap daemon-side.
//!
//! ## Cadence
//! A low-frequency tick ([`TICK_INTERVAL_SECS`], default 30s) recomputes
//! eligibility; explicit `dismiss` deltas poke the task awake
//! immediately via a [`Notify`]. NO tight loop — this cannot reproduce
//! the #603 `set_active` storm (which was grid-pane focus in
//! `sessions_grid_ws.rs`, a different "active", left untouched).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use k2_core::log_debug;

use crate::session_events::{self, SessionEvent};
use crate::v2_session_map;

/// Grace period between a workspace becoming reap-eligible and the
/// reaper force-closing its chat PTY. Moved daemon-side from the
/// renderer's `DISMISS_REAP_GRACE_MS = 15_000` (`tabs.ts`).
pub const DISMISS_REAP_GRACE_MS: u64 = 15_000;

/// Reaper recompute cadence. Low-frequency on purpose (no tight loop).
/// Overridable via `K2SO_ACTIVE_REAPER_TICK_SECS` for tests/tuning.
const TICK_INTERVAL_SECS: u64 = 30;

/// Cross-task signal channel: routes (`dismiss`) push project ids onto a
/// shared "arm grace NOW" set and wake the reaper. Holds a `Notify` so a
/// `dismiss` doesn't wait for the next tick to start the grace clock.
struct ReaperSignal {
    /// Project ids explicitly dismissed — force reap-eligible NOW (still
    /// gated by `!heartbeat`), independent of the interaction window.
    /// Drained by the reaper on each pass; an entry persists in the
    /// armed-timer map until it fires or the workspace re-activates.
    pending_dismiss: Mutex<HashSet<String>>,
    notify: Notify,
}

static SIGNAL: OnceLock<ReaperSignal> = OnceLock::new();

fn signal() -> &'static ReaperSignal {
    SIGNAL.get_or_init(|| ReaperSignal {
        pending_dismiss: Mutex::new(HashSet::new()),
        notify: Notify::new(),
    })
}

/// Arm the grace-reap for `project_id` IMMEDIATELY (called from
/// `POST /cli/projects/dismiss`). The workspace is treated as
/// reap-eligible right away — the reaper won't wait for the interaction
/// window to expire — but the `!heartbeat` gate + the 15s grace + the
/// fire-time Active re-check still apply, and re-activating within the
/// grace cancels it. Wakes the reaper task so the grace clock starts now.
pub fn arm_dismiss_grace(project_id: &str) {
    let sig = signal();
    sig.pending_dismiss
        .lock()
        .unwrap()
        .insert(project_id.to_string());
    sig.notify.notify_one();
}

/// Test/diagnostic hook: is `project_id` currently in the pending-dismiss
/// force-eligible set?
#[cfg(test)]
pub fn is_dismiss_armed(project_id: &str) -> bool {
    signal()
        .pending_dismiss
        .lock()
        .unwrap()
        .contains(project_id)
}

fn tick_interval() -> Duration {
    let secs = std::env::var("K2SO_ACTIVE_REAPER_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(TICK_INTERVAL_SECS);
    Duration::from_secs(secs)
}

fn grace() -> Duration {
    let ms = std::env::var("K2SO_ACTIVE_REAPER_GRACE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DISMISS_REAP_GRACE_MS);
    Duration::from_millis(ms)
}

/// Recompute the canonical Active set and broadcast an `ActiveChanged`
/// (full set). Called by the routes after every membership-changing
/// write and by the reaper after a close. Best-effort: a serialize /
/// no-subscriber error is a no-op (`let _ =`).
pub fn recompute_and_broadcast_active() {
    let window = k2_core::app_settings::load().active_window_hours;
    let now_ms = unix_now_ms();
    match k2_core::projects_ops::compute_active_project_ids(now_ms, window) {
        Ok(ids) => {
            let _ = session_events::emit(SessionEvent::ActiveChanged {
                active_project_ids: ids,
                active_window_hours: window,
            });
        }
        Err(e) => {
            log_debug!("[active-reaper] recompute_and_broadcast failed: {e}");
        }
    }
}

/// Did the workspace's `last_interaction_at` advance since the timer
/// armed? A `Some(newer) > Some(armed)` or `Some(_) > None` transition
/// means a fresh activate/touch happened — re-activation within the
/// grace, which cancels a forced (dismiss) timer.
fn interaction_advanced(armed_secs: Option<i64>, current_secs: Option<i64>) -> bool {
    match (armed_secs, current_secs) {
        (None, Some(_)) => true,
        (Some(a), Some(c)) => c > a,
        _ => false,
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// State for an armed grace timer, keyed by project_id in the reaper's
/// in-memory map. Re-arm replaces.
#[derive(Debug, Clone)]
struct ArmState {
    /// Instant the grace expires → fire after this.
    deadline: Instant,
    /// `true` when armed by an explicit `dismiss` (force-eligible even
    /// if still within the interaction window). A forced timer ignores
    /// the window-Active gate; only `!heartbeat` + "no re-activation
    /// since arm" keep it alive.
    forced: bool,
    /// `last_interaction_at` (unix secs) observed when the timer armed.
    /// A later interaction (value advances) signals re-activation → the
    /// forced timer cancels. `None` when the workspace had never been
    /// interacted with at arm time.
    armed_interaction_secs: Option<i64>,
}

/// A live v2 chat PTY resolved to its owning workspace. Built per pass
/// from `v2_session_map::snapshot()` joined with the `projects` table.
struct LiveChat {
    /// `v2_session_map` key (the close target for `unregister`).
    agent_name: String,
    /// Owning project's primary-key id (Active-set membership token).
    project_id: String,
    /// The PTY's daemon-side session UUID — persisted for `--resume`.
    session_id: String,
    /// Owning project's `last_interaction_at` (unix secs) at snapshot
    /// time — used to detect re-activation that cancels a forced
    /// (dismiss) timer.
    last_interaction_secs: Option<i64>,
}

/// Spawn the Active reaper as a background tokio task. Returns the
/// JoinHandle (not currently aborted on shutdown; the task exits when
/// the runtime drops). Mirrors `watchdog::spawn`'s shape.
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let tick = tick_interval();
    let grace_dur = grace();
    log_debug!(
        "[active-reaper] started — tick={}s grace={}ms",
        tick.as_secs(),
        grace_dur.as_millis(),
    );

    // Per-project armed grace timers. Keyed by project_id; re-arm
    // replaces. Boot reconciliation seeds it (first pass below).
    let mut armed: HashMap<String, ArmState> = HashMap::new();

    loop {
        // One reconciliation pass: refresh eligibility, arm/cancel
        // timers, and fire any whose grace has elapsed.
        reconcile_pass(&mut armed, grace_dur).await;

        // Sleep until the next tick OR until a `dismiss` pokes us awake
        // (so the grace clock starts immediately on an explicit
        // dismiss instead of waiting up to `tick` seconds).
        tokio::select! {
            _ = tokio::time::sleep(tick) => {}
            _ = signal().notify.notified() => {
                log_debug!("[active-reaper] woken by dismiss signal");
            }
        }
    }
}

/// Single pass. Pure-ish orchestration; all DB reads happen up front so
/// the Active set + heartbeat flags are a consistent snapshot for both
/// arming and firing within this pass.
async fn reconcile_pass(armed: &mut HashMap<String, ArmState>, grace_dur: Duration) {
    let window = k2_core::app_settings::load().active_window_hours;
    let now_ms = unix_now_ms();

    // Snapshot: canonical Active set + heartbeat-enabled set + live
    // chat PTYs. Done on a blocking thread — DB lock + map lock.
    let snapshot = tokio::task::spawn_blocking(move || collect_snapshot(now_ms, window))
        .await
        .unwrap_or_else(|e| {
            log_debug!("[active-reaper] snapshot join error: {e}");
            Snapshot::default()
        });

    // Drain explicit dismissals → force-eligible regardless of window.
    // Once drained, a dismissed workspace stays armed across subsequent
    // passes via the `armed` map (until it fires or re-activates), so we
    // only need to consume `pending_dismiss` once.
    let dismissed: HashSet<String> = {
        let mut set = signal().pending_dismiss.lock().unwrap();
        std::mem::take(&mut *set)
    };

    let now = Instant::now();
    // (chat, forced) — `forced` = armed by an explicit dismiss, so the
    // fire-time re-check skips the window-Active gate (only `!heartbeat`
    // + no-reactivation keep it alive).
    let mut to_fire: Vec<(LiveChat, bool)> = Vec::new();

    for chat in &snapshot.live_chats {
        let pid = &chat.project_id;
        let heartbeat = snapshot.heartbeat_ids.contains(pid);
        let active = snapshot.active_ids.contains(pid);
        let dismissed_this_pass = dismissed.contains(pid);
        let existing = armed.get(pid).cloned();
        // Was this workspace already armed by a PRIOR dismiss? Such a
        // forced timer keeps counting down even while the workspace is
        // still within its interaction window.
        let forced_already = existing.as_ref().map(|s| s.forced).unwrap_or(false);

        // (1) Heartbeat-warm ⇒ never reaped. The keep-warm gate wins
        //     over everything, including a dismiss. Cancel any timer.
        if heartbeat {
            if existing.is_some() {
                log_debug!("[active-reaper] cancel grace for project={pid} (heartbeat warm)");
            }
            armed.remove(pid);
            continue;
        }

        // (2) Re-activation cancels a forced (dismiss) timer: if the
        //     workspace's interaction advanced past the value captured
        //     when the timer armed (a fresh activate/touch), OR it got
        //     pinned (active via manually_active) AFTER the dismiss, the
        //     user clearly wants it back — cancel.
        if let Some(state) = existing.as_ref() {
            if state.forced && !dismissed_this_pass {
                let reactivated = interaction_advanced(
                    state.armed_interaction_secs,
                    chat.last_interaction_secs,
                );
                if reactivated {
                    log_debug!(
                        "[active-reaper] cancel forced grace for project={pid} (re-activated within grace)"
                    );
                    armed.remove(pid);
                    continue;
                }
            }
        }

        // (3) Active (within-window or pinned) AND not under a forced
        //     timer AND not dismissed this pass ⇒ not eligible. Cancel
        //     any aged-out timer (it re-entered the window).
        if active && !forced_already && !dismissed_this_pass {
            if existing.is_some() {
                log_debug!("[active-reaper] cancel grace for project={pid} (re-entered Active set)");
            }
            armed.remove(pid);
            continue;
        }

        // Eligible: aged out (not Active, not warm), OR explicitly
        // dismissed (forced), OR a still-running forced timer.
        match armed.get(pid) {
            Some(state) if now >= state.deadline => {
                // Grace elapsed → fire (after a fresh re-check below).
                let forced = state.forced;
                to_fire.push((
                    LiveChat {
                        agent_name: chat.agent_name.clone(),
                        project_id: chat.project_id.clone(),
                        session_id: chat.session_id.clone(),
                        last_interaction_secs: chat.last_interaction_secs,
                    },
                    forced,
                ));
            }
            Some(_) => { /* still within grace — wait */ }
            None => {
                // First time eligible → arm the grace timer.
                armed.insert(
                    pid.clone(),
                    ArmState {
                        deadline: now + grace_dur,
                        forced: dismissed_this_pass,
                        armed_interaction_secs: chat.last_interaction_secs,
                    },
                );
                log_debug!(
                    "[active-reaper] armed grace for project={pid} (agent={} forced={dismissed_this_pass})",
                    chat.agent_name,
                );
            }
        }
    }

    // Prune armed entries whose chat PTY no longer exists (closed by
    // another path) so the map doesn't leak.
    let live_pids: HashSet<&String> =
        snapshot.live_chats.iter().map(|c| &c.project_id).collect();
    armed.retain(|pid, _| live_pids.contains(pid));

    if to_fire.is_empty() {
        return;
    }

    // Fire: re-check Active gates at fire time (defensive against a
    // race where the workspace re-activated between arm and fire), then
    // force-close + persist sessionId for --resume.
    let mut closed_any = false;
    for (chat, forced) in to_fire {
        // Fire-time re-check against a fresh snapshot.
        let pid = chat.project_id.clone();
        let recheck = tokio::task::spawn_blocking(move || {
            let window = k2_core::app_settings::load().active_window_hours;
            let now = unix_now_ms();
            let rows = k2_core::projects_ops::active_project_rows().unwrap_or_default();
            let active = k2_core::active::is_in_active_set(now, &rows, window, &pid);
            let heartbeat = heartbeat_enabled_ids().contains(&pid);
            (active, heartbeat)
        })
        .await
        .unwrap_or((true, true)); // fail closed: skip the reap on join error

        let (active, heartbeat) = recheck;
        // Heartbeat-warm ALWAYS spares the session (the keep-warm gate
        // wins, even over a dismiss). For a NON-forced (aged-out) timer
        // the window-Active gate also spares it — the workspace
        // re-entered the window between arm and fire. A FORCED (dismiss)
        // timer intentionally ignores the window: dismiss means "remove
        // it now" even if the last interaction is still recent. The
        // arming loop above already cancelled the forced timer if a
        // fresh interaction (re-activation) advanced past the arm-time
        // value, so reaching here forced means no re-activation happened.
        let spare = heartbeat || (active && !forced);
        if spare {
            log_debug!(
                "[active-reaper] skip fire for project={} (re-check: active={active} heartbeat={heartbeat} forced={forced})",
                chat.project_id,
            );
            armed.remove(&chat.project_id);
            continue;
        }

        // Persist sessionId so a later --resume reattaches the same
        // conversation. Best-effort; failure doesn't block the close.
        persist_session_id_for_resume(&chat.project_id, &chat.session_id);

        log_debug!(
            "[active-reaper] firing close for project={} agent={} session={}",
            chat.project_id,
            chat.agent_name,
            chat.session_id,
        );
        // Force-close via the lowest chokepoint — bypasses any
        // route-level v2/close close-guard (reaper is Active-authoritative).
        let agent = chat.agent_name.clone();
        let removed = tokio::task::spawn_blocking(move || {
            v2_session_map::unregister(&agent).is_some()
        })
        .await
        .unwrap_or(false);
        armed.remove(&chat.project_id);
        if removed {
            closed_any = true;
        }
    }

    if closed_any {
        // Membership changed (a chat went away / workspace fully
        // dormant) — re-broadcast the canonical set.
        recompute_and_broadcast_active();
    }
}

#[derive(Default)]
struct Snapshot {
    active_ids: HashSet<String>,
    heartbeat_ids: HashSet<String>,
    live_chats: Vec<LiveChat>,
}

/// Build the per-pass snapshot: Active set, heartbeat-enabled set, and
/// the live chat PTYs resolved to their owning project. Runs on a
/// blocking thread (DB lock + map lock).
fn collect_snapshot(now_ms: i64, window: u32) -> Snapshot {
    let rows = k2_core::projects_ops::active_project_rows().unwrap_or_default();
    let active_ids: HashSet<String> = k2_core::active::active_project_ids(now_ms, &rows, window)
        .into_iter()
        .collect();
    // project_id → last_interaction_at (secs) so each LiveChat carries
    // its interaction timestamp for the re-activation detector.
    let interaction_by_pid: HashMap<String, Option<i64>> = rows
        .iter()
        .map(|r| (r.id.clone(), r.last_interaction_at_secs))
        .collect();
    let heartbeat_ids = heartbeat_enabled_ids();

    // Resolve each live v2 PTY to a project via its cwd. A "chat" PTY is
    // the canonical pinned workspace chat (key == bare project_id) — but
    // we resolve ALL live PTYs by cwd so any workspace-owned session
    // counts toward keeping the workspace from being reaped while it has
    // a live session that maps to a dormant workspace. The Active gate,
    // not the session kind, is the authority.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut live_chats = Vec::new();
    let mut seen_pids: HashSet<String> = HashSet::new();
    for (agent_name, session) in v2_session_map::snapshot() {
        let cwd = match session.cwd.as_ref() {
            Some(p) => p.to_string_lossy().into_owned(),
            None => continue,
        };
        let project_id =
            match k2_core::workspace::agent_identity::resolve_project_id(&conn, &cwd) {
                Some(id) => id,
                None => continue,
            };
        // One reap-candidate per workspace: the chat keyed on the bare
        // project_id is the canonical workspace chat. If present, prefer
        // it; otherwise the first resolved session for the workspace.
        let is_canonical_chat = agent_name == project_id;
        if is_canonical_chat || !seen_pids.contains(&project_id) {
            seen_pids.insert(project_id.clone());
            // Replace any earlier non-canonical entry for this pid with
            // the canonical chat when we find it.
            live_chats.retain(|c: &LiveChat| c.project_id != project_id);
            let last_interaction_secs = interaction_by_pid
                .get(&project_id)
                .copied()
                .flatten();
            live_chats.push(LiveChat {
                agent_name,
                project_id,
                session_id: session.session_id.to_string(),
                last_interaction_secs,
            });
        }
    }
    drop(conn);

    Snapshot {
        active_ids,
        heartbeat_ids,
        live_chats,
    }
}

/// The set of project ids with at least one enabled, non-archived
/// heartbeat — the same "keep warm" predicate `Project::list` computes
/// live. Read directly so the reaper doesn't depend on the (stale)
/// legacy `projects.heartbeat_enabled` column.
fn heartbeat_enabled_ids() -> HashSet<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = HashSet::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT project_id FROM workspace_heartbeats \
         WHERE enabled = 1 AND archived_at IS NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            for id in rows.flatten() {
                out.insert(id);
            }
        }
    }
    out
}

/// Persist the chat PTY's `sessionId` onto the workspace's tab-session
/// row so a later `--resume` has a target. The daemon already stamps
/// `workspace_tab_sessions.session_id` on register when the spawn args
/// carry `--session-id`/`--resume` (see `v2_session_map::register`); this
/// is a belt-and-suspenders write keyed on the PTY's session id for the
/// dormant-reap case. Best-effort.
fn persist_session_id_for_resume(project_id: &str, session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Stamp the canonical (bare project_id) tab-session row's session_id
    // if it isn't already set. COALESCE keeps a previously-stamped value.
    let _ = conn.execute(
        "UPDATE workspace_tab_sessions \
         SET session_id = COALESCE(session_id, ?1) \
         WHERE project_id = ?2 AND pane_group_id = ?2",
        rusqlite::params![session_id, project_id],
    );
}

// ── Test driver (integration tests in `tests/*.rs`) ────────────────────
//
// The real reaper runs a private `loop { reconcile_pass }`. Integration
// tests need to drive DETERMINISTIC, single passes with a short grace so
// they don't sleep 15s. This `#[doc(hidden)]` driver owns the same
// armed-timer map a live reaper would and exposes one pass at a time —
// it exercises the exact `reconcile_pass` code path the daemon runs.
// `#[allow(dead_code)]`: consumed by the integration test
// (`tests/active_reaper_integration.rs`) via the test-facing lib, but
// the BINARY compilation never references it — same pattern as the
// `test_harness` module in `lib.rs`.
#[doc(hidden)]
#[allow(dead_code)]
pub struct ReaperTestDriver {
    armed: HashMap<String, ArmState>,
    grace: Duration,
}

#[doc(hidden)]
#[allow(dead_code)]
impl ReaperTestDriver {
    /// New driver with a custom (typically tiny) grace so tests can
    /// observe arm → fire without a real 15s wait.
    pub fn with_grace_ms(grace_ms: u64) -> Self {
        Self {
            armed: HashMap::new(),
            grace: Duration::from_millis(grace_ms),
        }
    }

    /// Run exactly one reconciliation pass (arm/cancel/fire).
    pub async fn pass(&mut self) {
        reconcile_pass(&mut self.armed, self.grace).await;
    }

    /// Is `project_id` currently armed (a grace timer running)?
    pub fn is_armed(&self, project_id: &str) -> bool {
        self.armed.contains_key(project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grace_constant_matches_renderer() {
        // The renderer's DISMISS_REAP_GRACE_MS was 15_000; the daemon
        // owns it now. Pin the value so a drift surfaces in CI.
        assert_eq!(DISMISS_REAP_GRACE_MS, 15_000);
    }

    #[test]
    fn arm_dismiss_then_observe() {
        let pid = format!("test-dismiss-{}", std::process::id());
        assert!(!is_dismiss_armed(&pid));
        arm_dismiss_grace(&pid);
        assert!(is_dismiss_armed(&pid));
    }

    #[test]
    fn tick_interval_env_override() {
        // Default when unset.
        std::env::remove_var("K2SO_ACTIVE_REAPER_TICK_SECS");
        assert_eq!(tick_interval(), Duration::from_secs(TICK_INTERVAL_SECS));
    }
}
