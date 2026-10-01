//! S5 — creation traps (`.k2/prds/prd-heartbeat-firing-v1.md` HB33,
//! HB35, Rosson decision D4).
//!
//! - **Empty is not missing (HB33).** A WAKEUP.md that exists with an
//!   empty body (the Settings/CLI scaffold before anyone writes
//!   instructions) never auto-disables and never counts a failure. The
//!   row stays enabled and waits with `wait_reason = wakeup_empty`.
//!   `wakeup_missing` stays for a file that is really gone.
//! - **No surprise catch-up (D4).** Instructions added later wait for
//!   the next scheduled slot. The tick records the episode in the audit
//!   trail: one `wakeup_empty` row when a due slot finds the body
//!   empty, then one `wakeup_added` row on the first due tick that finds
//!   a body. From that row on, the row's schedule counts from the moment
//!   the instructions were seen, so slots that passed while it was empty
//!   are skipped, not replayed. The first real fire closes the episode.
//! - **Flag bad rows (HB35).** [`boot_reconcile`] flags every enabled
//!   row whose schedule can never fire, and relabels old
//!   `wakeup_missing` auto-disables whose file is actually just empty.
//!
//! `wait_reason` is not a stored column until S3 (migration 0123). Until
//! then the daemon computes it when a row is read
//! ([`annotate_wait_state`]). Same names, same vocabulary (HB20), so S3
//! only has to store what this already returns.

use std::path::Path;

use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension};

use super::cron;
use crate::db::schema::{AgentHeartbeat, HeartbeatFire};

/// HB20 `wait_reason`: the WAKEUP.md body is empty.
pub const WAIT_WAKEUP_EMPTY: &str = "wakeup_empty";
/// HB20 `wait_reason`: the schedule can never fire.
pub const WAIT_SCHEDULE_ERROR: &str = "schedule_error";
/// `wait_detail` for [`WAIT_WAKEUP_EMPTY`].
pub const WAKEUP_EMPTY_DETAIL: &str =
    "needs instructions: WAKEUP.md is empty. It fires at the next scheduled slot after you add them.";

/// Audit decision: a due slot found the body empty (opens the episode).
pub const DECISION_WAKEUP_EMPTY: &str = "wakeup_empty";
/// Audit decision: the body was seen again; wait for the next slot.
pub const DECISION_WAKEUP_ADDED: &str = "wakeup_added";

/// `disabled_reason` for an old auto-disable whose file is just empty.
pub const DISABLED_WAKEUP_EMPTY: &str = "wakeup_empty";
/// `disabled_reason` for a WAKEUP.md that does not exist.
pub const DISABLED_WAKEUP_MISSING: &str = "wakeup_missing";

/// What is on disk at a heartbeat's WAKEUP.md path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupBody {
    /// No file.
    Missing,
    /// The file exists; with frontmatter stripped, nothing is left.
    Empty,
    /// The file has instructions (or could not be read — the launcher
    /// reports an unreadable file itself, as before S5).
    Present,
}

/// Classify the WAKEUP.md at `path`. Same emptiness test the launcher's
/// composer uses (`compose_agent_wake_from_body`), so the tick and the
/// fire path can never disagree about "empty".
pub fn wakeup_body(path: &Path) -> WakeupBody {
    if !path.exists() {
        return WakeupBody::Missing;
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            if crate::workspace::wake_prompts::compose_agent_wake_from_body(Some(&raw)).is_none() {
                WakeupBody::Empty
            } else {
                WakeupBody::Present
            }
        }
        Err(_) => WakeupBody::Present,
    }
}

/// The open empty-WAKEUP episode for a heartbeat, from its audit rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupEpisode {
    /// A due slot found the body empty; nothing fired since.
    Empty,
    /// Instructions were seen at this time; nothing fired since. The
    /// schedule counts from here.
    Added(DateTime<Local>),
}

/// Latest `wakeup_empty` / `wakeup_added` audit row for this heartbeat,
/// if it is newer than `last_fired`. A fire after the marker closes the
/// episode, so the marker no longer applies.
pub fn open_episode(
    conn: &Connection,
    project_id: &str,
    name: &str,
    last_fired: Option<&str>,
) -> Option<WakeupEpisode> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT decision, fired_at FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision IN (?3, ?4) \
             ORDER BY id DESC LIMIT 1",
            params![project_id, name, DECISION_WAKEUP_EMPTY, DECISION_WAKEUP_ADDED],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (decision, at) = row?;
    let at = DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Local);
    if let Some(fired) = last_fired.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        if fired >= at {
            return None;
        }
    }
    if decision == DECISION_WAKEUP_ADDED {
        Some(WakeupEpisode::Added(at))
    } else {
        Some(WakeupEpisode::Empty)
    }
}

/// The row as the evaluator should see it while an `Added` episode is
/// open: the reference point moves to when the instructions were seen,
/// so the next occurrence is the first slot after that (D4).
pub fn with_reference(hb: &AgentHeartbeat, at: DateTime<Local>) -> AgentHeartbeat {
    let mut shifted = hb.clone();
    shifted.last_fired = Some(at.to_rfc3339());
    shifted
}

/// Why an enabled heartbeat is waiting, computed when the row is read.
/// `(None, None)` when nothing is in the way (or the row is disabled or
/// archived — `disabled_reason` already says why).
pub fn wait_state_for(project_path: &str, hb: &AgentHeartbeat) -> (Option<String>, Option<String>) {
    if !hb.enabled || hb.archived_at.is_some() {
        return (None, None);
    }
    if let cron::DueStatus::Invalid { reason } = cron::evaluate(hb) {
        return (Some(WAIT_SCHEDULE_ERROR.to_string()), Some(reason));
    }
    let abs = Path::new(project_path).join(&hb.wakeup_path);
    if wakeup_body(&abs) == WakeupBody::Empty {
        return (
            Some(WAIT_WAKEUP_EMPTY.to_string()),
            Some(WAKEUP_EMPTY_DETAIL.to_string()),
        );
    }
    (None, None)
}

/// Fill `wait_reason` / `wait_detail` on rows read from the database.
pub fn annotate_wait_state(project_path: &str, rows: &mut [AgentHeartbeat]) {
    for hb in rows.iter_mut() {
        let (reason, detail) = wait_state_for(project_path, hb);
        hb.wait_reason = reason;
        hb.wait_detail = detail;
    }
}

/// What [`boot_reconcile`] changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BootReconcile {
    /// Disabled `wakeup_missing` rows whose file exists but is empty,
    /// now `wakeup_empty`. They stay disabled.
    pub relabelled: Vec<String>,
    /// Enabled rows whose schedule can never fire, newly flagged with
    /// `schedule_error` (rows already carrying the same error are not
    /// counted and get no second audit row). Not disabled, not deleted.
    pub flagged: Vec<String>,
}

/// Daemon boot pass (HB33 relabel, HB35 flag). Reads every non-archived
/// row in every workspace — including workspaces the tick skips because
/// no agent name resolves, which is where an invalid row could sit
/// unflagged forever.
pub fn boot_reconcile() -> Result<BootReconcile, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    boot_reconcile_with(&conn)
}

/// [`boot_reconcile`] on a given connection (tests).
pub fn boot_reconcile_with(conn: &Connection) -> Result<BootReconcile, String> {
    let rows = AgentHeartbeat::list_all_active_with_project(conn)
        .map_err(|e| format!("list heartbeats: {e}"))?;
    let mut out = BootReconcile::default();
    for (hb, _project_name, project_path) in rows {
        let label = format!("{project_path}::{}", hb.name);
        if !hb.enabled && hb.disabled_reason.as_deref() == Some(DISABLED_WAKEUP_MISSING) {
            let abs = Path::new(&project_path).join(&hb.wakeup_path);
            if wakeup_body(&abs) == WakeupBody::Empty {
                AgentHeartbeat::auto_disable(conn, &hb.project_id, &hb.name, DISABLED_WAKEUP_EMPTY)
                    .map_err(|e| format!("relabel {label}: {e}"))?;
                out.relabelled.push(label.clone());
            }
        }
        if hb.enabled {
            if let cron::DueStatus::Invalid { reason } = cron::evaluate(&hb) {
                if hb.schedule_error.as_deref() != Some(reason.as_str()) {
                    AgentHeartbeat::set_schedule_error(conn, &hb.project_id, &hb.name, Some(&reason))
                        .map_err(|e| format!("flag {label}: {e}"))?;
                    HeartbeatFire::insert_with_schedule(
                        conn,
                        &hb.project_id,
                        None,
                        Some(&hb.name),
                        &hb.frequency,
                        "schedule_invalid",
                        Some(&reason),
                        None,
                        None,
                        None,
                    )
                    .map_err(|e| format!("audit {label}: {e}"))?;
                    out.flagged.push(label);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! S5 tick + boot behaviour. Rows are seeded straight into the
    //! shared test DB (no add → no scheduler install path at all).

    use super::*;
    use crate::heartbeats::k2so_agents_heartbeat_tick;

    const SCAFFOLD: &str = "---\ndescription:\n---\n\n";

    struct Ws {
        dir: std::path::PathBuf,
        path: String,
        project_id: String,
    }

    impl Drop for Ws {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A registered workspace with a configured agent (so the tick
    /// resolves an agent name and actually evaluates its rows).
    fn workspace(label: &str) -> Ws {
        crate::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "k2-hb-s5-wait-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();
        let project_id = uuid::Uuid::new_v4().to_string();
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, 's5-wait', ?2, 1)",
            rusqlite::params![project_id, path],
        )
        .unwrap();
        Ws { dir, path, project_id }
    }

    /// Insert an enabled row whose reference point is `age_secs` ago,
    /// with WAKEUP.md written as `body` (None = no file).
    fn seed(
        ws: &Ws,
        name: &str,
        frequency: &str,
        spec: &str,
        age_secs: i64,
        body: Option<&str>,
    ) -> std::path::PathBuf {
        let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
        let abs = ws.dir.join(&rel);
        if let Some(body) = body {
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
        }
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &ws.project_id,
            name,
            frequency,
            spec,
            &rel,
            true,
        )
        .unwrap();
        conn.execute(
            "UPDATE workspace_heartbeats SET created_at = unixepoch() - ?1 \
             WHERE project_id = ?2 AND name = ?3",
            rusqlite::params![age_secs, ws.project_id, name],
        )
        .unwrap();
        abs
    }

    fn row(ws: &Ws, name: &str) -> AgentHeartbeat {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::get_by_name(&conn, &ws.project_id, name)
            .unwrap()
            .expect("row exists")
    }

    fn decisions(ws: &Ws, name: &str) -> Vec<String> {
        let db = crate::db::shared();
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT decision FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 \
                 ORDER BY fired_at, id",
            )
            .unwrap();
        let out = stmt
            .query_map(rusqlite::params![ws.project_id, name], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        out
    }

    fn candidate_names(ws: &Ws) -> Vec<String> {
        k2so_agents_heartbeat_tick(&ws.path).into_iter().map(|c| c.name).collect()
    }

    fn listed(ws: &Ws, name: &str) -> AgentHeartbeat {
        crate::heartbeats::k2so_heartbeat_list(ws.path.clone())
            .expect("list")
            .into_iter()
            .find(|r| r.name == name)
            .expect("row listed")
    }

    fn assert_waiting_enabled(ws: &Ws, name: &str) {
        let hb = row(ws, name);
        assert!(hb.enabled, "{name}: an empty WAKEUP.md must not disable the row");
        assert_eq!(hb.disabled_reason, None, "{name}: no disabled_reason");
        assert_eq!(hb.consecutive_failures, 0, "{name}: empty is not a failure");
        assert_eq!(hb.next_retry_at, None, "{name}: empty sets no backoff");
    }

    /// T8 (HB33 + D4), hourly. Due + empty: waits enabled, one audit row
    /// per episode. Body written: no immediate fire. First slot after
    /// the body was seen: fires on time (not as a catch-up).
    #[test]
    fn empty_wakeup_waits_then_fires_at_the_next_slot_not_at_once() {
        let ws = workspace("hourly");
        let abs = seed(&ws, "minutely", "hourly", r#"{"every_seconds":60}"#, 630, Some(SCAFFOLD));

        assert_eq!(candidate_names(&ws), Vec::<String>::new(), "empty body must not be a candidate");
        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        assert_waiting_enabled(&ws, "minutely");
        assert_eq!(
            decisions(&ws, "minutely"),
            vec![DECISION_WAKEUP_EMPTY.to_string()],
            "exactly one wakeup_empty audit row per episode"
        );
        assert_eq!(listed(&ws, "minutely").wait_reason.as_deref(), Some(WAIT_WAKEUP_EMPTY));

        std::fs::write(&abs, format!("{SCAFFOLD}check inbox\n")).unwrap();
        assert_eq!(
            candidate_names(&ws),
            Vec::<String>::new(),
            "instructions added after the slot must not fire at once (D4)"
        );
        assert_eq!(
            candidate_names(&ws),
            Vec::<String>::new(),
            "still waiting for the first slot after the body was seen"
        );
        assert_eq!(
            decisions(&ws, "minutely"),
            vec![DECISION_WAKEUP_EMPTY.to_string(), DECISION_WAKEUP_ADDED.to_string()]
        );
        assert_waiting_enabled(&ws, "minutely");
        assert_eq!(listed(&ws, "minutely").wait_reason, None, "a body clears the empty wait");

        // Next slot: age both markers (order kept), so the body was seen
        // 61s ago and the first slot after it has just passed.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            for (decision, secs) in [(DECISION_WAKEUP_EMPTY, 121), (DECISION_WAKEUP_ADDED, 61)] {
                let back = (chrono::Local::now() - chrono::Duration::seconds(secs)).to_rfc3339();
                let n = conn
                    .execute(
                        "UPDATE heartbeat_fires SET fired_at = ?1 \
                         WHERE project_id = ?2 AND schedule_name = 'minutely' AND decision = ?3",
                        rusqlite::params![back, ws.project_id, decision],
                    )
                    .unwrap();
                assert_eq!(n, 1, "{decision}");
            }
        }
        let fired = k2so_agents_heartbeat_tick(&ws.path);
        assert_eq!(fired.len(), 1, "fires at the first slot after the body was seen");
        assert_eq!(fired[0].name, "minutely");
        assert_eq!(fired[0].catchup_of, None, "the next slot is on time, not a catch-up");

        // A fire closes the episode: the marker no longer applies.
        crate::heartbeats::stamp_heartbeat_fired(&ws.path, "minutely");
        let hb = row(&ws, "minutely");
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(open_episode(&conn, &ws.project_id, "minutely", hb.last_fired.as_deref()), None);
    }

    /// T8 (D4), daily: a slot missed while empty is never replayed.
    #[test]
    fn daily_slot_missed_while_empty_is_not_caught_up() {
        let ws = workspace("daily");
        let two_hours_ago = (chrono::Local::now() - chrono::Duration::hours(2))
            .format("%H:%M")
            .to_string();
        let spec = format!(r#"{{"time":"{two_hours_ago}"}}"#);
        let abs = seed(&ws, "morning", "daily", &spec, 3 * 86_400, Some(SCAFFOLD));

        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        assert_waiting_enabled(&ws, "morning");
        std::fs::write(&abs, "do the morning brief\n").unwrap();
        for _ in 0..3 {
            assert_eq!(
                candidate_names(&ws),
                Vec::<String>::new(),
                "today's slot passed while empty; it waits for tomorrow's"
            );
        }
        assert_eq!(
            decisions(&ws, "morning"),
            vec![DECISION_WAKEUP_EMPTY.to_string(), DECISION_WAKEUP_ADDED.to_string()]
        );
        assert_waiting_enabled(&ws, "morning");
    }

    /// A missing file keeps today's behaviour: auto-disable as
    /// `wakeup_missing`.
    #[test]
    fn missing_wakeup_still_disables_as_missing() {
        let ws = workspace("missing");
        seed(&ws, "gone", "hourly", r#"{"every_seconds":60}"#, 630, None);
        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        let hb = row(&ws, "gone");
        assert!(!hb.enabled);
        assert_eq!(hb.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_MISSING));
    }

    /// A body present from the start fires on its slot (no episode).
    #[test]
    fn body_from_the_start_fires_normally() {
        let ws = workspace("normal");
        seed(&ws, "ready", "hourly", r#"{"every_seconds":60}"#, 90, Some("check inbox\n"));
        let fired = k2so_agents_heartbeat_tick(&ws.path);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].name, "ready");
        assert_eq!(decisions(&ws, "ready"), Vec::<String>::new(), "no wait markers");
    }

    /// T-S5e (HB33) + HB35: boot relabels empty-file auto-disables
    /// (still disabled), leaves truly missing ones, and flags invalid
    /// schedules once without disabling, rewriting or deleting them.
    #[test]
    fn boot_reconcile_relabels_empty_and_flags_invalid_once() {
        let ws = workspace("boot");
        seed(&ws, "was-empty", "daily", "{}", 60, Some(SCAFFOLD));
        seed(&ws, "really-gone", "daily", "{}", 60, None);
        seed(&ws, "reggie", "list", "{}", 60, Some("do things\n"));
        {
            let db = crate::db::shared();
            let conn = db.lock();
            for name in ["was-empty", "really-gone"] {
                AgentHeartbeat::auto_disable(&conn, &ws.project_id, name, DISABLED_WAKEUP_MISSING)
                    .unwrap();
            }
        }

        // Flagged on read, before any boot pass.
        let reggie = listed(&ws, "reggie");
        assert_eq!(reggie.wait_reason.as_deref(), Some(WAIT_SCHEDULE_ERROR));
        assert_eq!(reggie.wait_detail.as_deref(), Some("unknown frequency 'list'"));

        let first = {
            let db = crate::db::shared();
            let conn = db.lock();
            boot_reconcile_with(&conn).expect("reconcile")
        };
        let label = |n: &str| format!("{}::{n}", ws.path);
        assert!(first.relabelled.contains(&label("was-empty")), "{first:?}");
        assert!(!first.relabelled.contains(&label("really-gone")), "{first:?}");
        assert!(first.flagged.contains(&label("reggie")), "{first:?}");

        let was_empty = row(&ws, "was-empty");
        assert!(!was_empty.enabled, "relabel keeps the row disabled");
        assert_eq!(was_empty.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_EMPTY));
        let gone = row(&ws, "really-gone");
        assert_eq!(gone.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_MISSING));
        let reggie = row(&ws, "reggie");
        assert!(reggie.enabled, "an invalid schedule is flagged, not disabled");
        assert_eq!(reggie.frequency, "list", "the row is not rewritten");
        assert_eq!(reggie.schedule_error.as_deref(), Some("unknown frequency 'list'"));
        assert_eq!(decisions(&ws, "reggie"), vec!["schedule_invalid".to_string()]);

        let second = {
            let db = crate::db::shared();
            let conn = db.lock();
            boot_reconcile_with(&conn).expect("reconcile again")
        };
        assert!(!second.flagged.contains(&label("reggie")), "{second:?}");
        assert!(!second.relabelled.contains(&label("was-empty")), "{second:?}");
        assert_eq!(
            decisions(&ws, "reggie"),
            vec!["schedule_invalid".to_string()],
            "one audit row per episode, not per boot"
        );
    }
}
