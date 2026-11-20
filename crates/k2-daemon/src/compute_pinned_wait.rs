//! A pinned job that has waited long (agent feedback, 0.46.17).
//!
//! A job with a node named waits for that node however long, even while
//! other nodes sit idle, and agents pin on purpose. Once such a job has
//! waited [`PINNED_WAIT_SECS`] (on this server or in the node's own queue),
//! its session gets one `[compute]` note, and its `wait.words` carry the
//! same advice from then on: which node it is pinned to, how long it
//! waited, a node that could start it now (when one could), and the two
//! ways out: `k2 compute move <id> --auto`, or leaving the node off next
//! time. Once per job (its `ctl.pinned_wait_noted_at`), never repeated.
//! The job itself is never moved: a pin stays a pin.

use k2_core::compute::{self as compute, store};
use k2_core::log_debug;
use k2_core::{cli_name, product_name};

use crate::compute_place::{self as place, Tier, Want};

/// A pinned job is noted after waiting this long.
pub const PINNED_WAIT_SECS: i64 = 15 * 60;

/// How often the sweep looks (a note minutes late is fine).
const EVERY_SECS: i64 = 30;

static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// When the job last entered a queue (submit, retry).
fn since(j: &store::JobRow) -> i64 {
    if j.ctl.queued_at > 0 {
        j.ctl.queued_at
    } else {
        j.created_at
    }
}

/// A pinned job, still waiting, for at least [`PINNED_WAIT_SECS`], not
/// noted yet.
pub fn due(j: &store::JobRow, now: i64) -> bool {
    j.ctl.any.is_none() && (j.state == "queued" || j.node_queued) && j.ctl.pinned_wait_noted_at.is_none() && now - since(j) >= PINNED_WAIT_SECS
}

/// The advice in the job's `wait.words` (short).
pub fn words(job_id: &str, node: &str, waited_secs: i64, could_start: Option<&str>) -> String {
    let id = compute::message::short(job_id);
    let other = match could_start {
        Some(n) => format!(" while {n} could start it now"),
        None => String::new(),
    };
    format!(
        concat!(
            "pinned to {node}: waited {}{other}; ",
            cli_name!(),
            " compute move {id} --auto lets ",
            product_name!(),
            " move it (or leave the node off next time)"
        ),
        compute::message::duration(waited_secs),
        node = node,
        other = other,
        id = id
    )
}

/// The one `[compute]` note for the job's session.
pub fn note_text(job_id: &str, node: &str, waited_secs: i64, could_start: Option<&str>) -> String {
    let id = compute::message::short(job_id);
    let other = match could_start {
        Some(n) => format!("{n} could start it now"),
        None => "no other node could start it now either".to_string(),
    };
    format!(
        concat!(
            "[compute] job {id} is pinned to {node} and has waited {} there; {other}. ",
            cli_name!(),
            " compute move {id} --auto lets ",
            product_name!(),
            " place it on the first node that can start it; next time, leave the node off and ",
            product_name!(),
            " picks one. ",
            cli_name!(),
            " compute get {id}"
        ),
        compute::message::duration(waited_secs),
        node = node,
        other = other,
        id = id
    )
}

/// Another node this workspace may use that could start the job now, as
/// `k2 compute move --auto` would look (the same OS and arch as its node).
/// The first by name, so the note is stable.
fn could_start(j: &store::JobRow, now: i64) -> Option<String> {
    let plan = j.plan()?;
    let mut probe = j.clone();
    let (os, arch) = crate::compute_move::node_kind(&j.node_id);
    probe.ctl.any = Some(store::AnyFilter { os, arch, ..Default::default() });
    let w = Want::of_job(&probe, &plan);
    place::candidates(&w, std::slice::from_ref(&j.node_id), now)
        .iter()
        .filter(|c| c.tier(&w) == Some(Tier::Now))
        .map(|c| c.node.name.clone())
        .min()
}

/// Note every pinned job that has waited long, once each.
pub fn sweep(now: i64) {
    let last = LAST.load(std::sync::atomic::Ordering::Relaxed);
    if last != 0 && (0..EVERY_SECS).contains(&(now - last)) {
        return;
    }
    LAST.store(now, std::sync::atomic::Ordering::Relaxed);
    sweep_now(now);
}

/// [`sweep`] without its throttle (tests call it directly).
pub fn sweep_now(now: i64) {
    let waiting = k2_core::db::read(|conn| {
        let mut v = store::queued_jobs(conn);
        v.extend(store::node_queued_jobs(conn));
        v
    });
    for j in waiting.into_iter().filter(|j| due(j, now)) {
        let other = could_start(&j, now);
        let waited = now - since(&j);
        let (session, ws_path, node) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            // Once per job, decided under the lock on a fresh row.
            let Some(row) = store::job_by_id(&conn, &j.id) else { continue };
            if !due(&row, now) {
                continue;
            }
            let node = store::node_by_id(&conn, &row.node_id).map(|n| n.name).unwrap_or_else(|| row.node_id.clone());
            let mut ctl = row.ctl.clone();
            ctl.pinned_wait_noted_at = Some(now);
            ctl.pinned_wait_words = Some(words(&row.id, &node, waited, other.as_deref()));
            if store::set_job_ctl(&conn, &row.id, &ctl).is_err() {
                continue;
            }
            store::record_event(
                &conn,
                "controller",
                "job_pinned_wait",
                Some(&row.node_id),
                Some(&row.workspace_id),
                Some(&row.id),
                serde_json::json!({"waitedSecs": waited, "couldStart": other}),
            );
            (row.session_id.clone(), k2_core::db::schema::Project::get(&conn, &row.workspace_id).ok().map(|p| p.path), node)
        };
        let text = note_text(&j.id, &node, waited, other.as_deref());
        let title = format!("compute job {} waits on its pinned node", compute::message::short(&j.id));
        let id = j.id.clone();
        if let Err(e) = crate::rt::spawn("compute-pinned-wait", move || crate::compute_ws::deliver(&title, &id, session.as_deref(), ws_path.as_deref(), &text)) {
            log_debug!("[compute] pinned-wait note for {} not delivered: {e}", j.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, pinned: bool, node_queued: bool, queued_at: i64) -> store::JobRow {
        let mut j = store::JobRow { id: "abcd1234ffff".into(), state: state.into(), node_queued, created_at: 100, ..Default::default() };
        j.ctl.queued_at = queued_at;
        if !pinned {
            j.ctl.any = Some(store::AnyFilter::default());
        }
        j
    }

    /// Due: pinned, still waiting (on this server or in a node's queue),
    /// 15 min or more since it was queued, and not noted before.
    #[test]
    fn a_pinned_job_is_due_after_fifteen_minutes_once() {
        let t0 = 10_000;
        assert!(due(&row("queued", true, false, t0), t0 + PINNED_WAIT_SECS));
        assert!(due(&row("assigned", true, true, t0), t0 + PINNED_WAIT_SECS), "in a shared node's queue");
        assert!(!due(&row("queued", true, false, t0), t0 + PINNED_WAIT_SECS - 1), "not before 15 min");
        assert!(!due(&row("queued", false, false, t0), t0 + 3600), "an auto job moves by itself");
        assert!(!due(&row("running", true, false, t0), t0 + 3600), "started");
        assert!(!due(&row("assigned", true, false, t0), t0 + 3600), "sent to a solo node: starting");
        let mut noted = row("queued", true, false, t0);
        noted.ctl.pinned_wait_noted_at = Some(t0 + PINNED_WAIT_SECS);
        assert!(!due(&noted, t0 + 7200), "once per job");
        // No queued_at (an older row): counted from submit.
        assert!(due(&row("queued", true, false, 0), 100 + PINNED_WAIT_SECS));
    }

    #[test]
    fn the_note_names_the_node_the_wait_and_both_ways_out() {
        assert_eq!(
            note_text("abcd1234ffff", "mini-1", 960, Some("linux-4")),
            "[compute] job abcd1234 is pinned to mini-1 and has waited 16m00s there; linux-4 could start it now. k2 compute move abcd1234 --auto lets K2 place it on the first node that can start it; next time, leave the node off and K2 picks one. k2 compute get abcd1234"
        );
        assert!(note_text("abcd1234ffff", "mini-1", 960, None).contains("; no other node could start it now either. k2 compute move abcd1234 --auto"));
        assert_eq!(
            words("abcd1234ffff", "mini-1", 900, Some("linux-4")),
            "pinned to mini-1: waited 15m00s while linux-4 could start it now; k2 compute move abcd1234 --auto lets K2 move it (or leave the node off next time)"
        );
    }
}
