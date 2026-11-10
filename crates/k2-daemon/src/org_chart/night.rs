//! The night pass (prd-org-chart-v1 §5; vs-live A1, A2, A7): its own pass
//! on the heartbeat monitor's 60 s tick. Not a heartbeat kind (the
//! `workspace_heartbeats.kind` CHECK allows only three) and not a reminder
//! (a daemon-made reminder posts "Reminder fired" to the Thread nightly).
//!
//! Each tick, while the Org Chart is on, for every org with a head and at
//! least one other member, on tonight's night (owner's zone):
//!
//! 1. **Run** (`runAt`, 01:00): the head gets one line, once a night (the
//!    `org_chart_nights` primary key). A box that slept fires late, still
//!    once, and only before the day starts; during the day nothing fires.
//! 2. **Last call** (`lastCallAt`, 04:30): one more line, only if the head
//!    hasn't written its bulletin since the run.
//!
//! Every wake checks the hourly wake budget first; a refused wake is
//! recorded as `agent_cap` and tried again on the next tick, until the day
//! starts. The bulletin's own text is never put in a line: the lines point
//! at it (§10.1).
//!
//! The same pass runs [`super::reach::tell_pass`] and prunes old wake rows.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use k2_core::org_chart::{self as oc, day, store};

/// What one pass did (tests, the debug log).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub runs: usize,
    pub last_calls: usize,
    pub capped: usize,
    pub failed: usize,
    pub reach: super::reach::TellReport,
}

/// Run every due night step at `now`.
pub fn run_pass(w: &dyn super::Waker, now: DateTime<Utc>) -> Report {
    let mut rep = Report::default();
    if !oc::enabled() {
        return rep;
    }
    let ts = now.timestamp();
    let cfg = oc::config();
    let local = day::local(cfg.zone(), now);
    if let Some(night) = day::current_night_at(&cfg.night, local) {
        let key = day::day_key(night);
        let run_due = local >= day::on_night(night, cfg.night.run_at);
        let last_due = local >= day::on_night(night, cfg.night.last_call_at);
        if run_due {
            let orgs = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::orgs(&conn).unwrap_or_default()
            };
            for org in orgs {
                night_for_org(w, &org, &key, night, last_due, ts, &mut rep);
            }
        }
    }
    rep.reach = super::reach::tell_pass(w, ts);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = store::prune_wakes(&conn, ts - 2 * 86_400);
    rep
}

fn tally(rep: &mut Report, o: &super::WakeOutcome) {
    match o {
        super::WakeOutcome::Capped => rep.capped += 1,
        super::WakeOutcome::Failed(_) => rep.failed += 1,
        super::WakeOutcome::Delivered => {}
    }
}

fn night_for_org(
    w: &dyn super::Waker,
    org: &store::Org,
    key: &str,
    night: chrono::NaiveDate,
    last_due: bool,
    ts: i64,
    rep: &mut Report,
) {
    let Some(head) = org.head.clone() else { return };
    let (others, row, bulletin) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let others = store::members(&conn, &org.id).unwrap_or_default().into_iter().filter(|m| *m != head).count();
        (others, store::night_row(&conn, &org.id, key).ok().flatten(), store::bulletin(&conn, &org.id).ok().flatten())
    };
    if others == 0 {
        return;
    }
    let from = format!("org:{}", org.name);
    let row = row.unwrap_or_default();
    match row.run_at {
        None => {
            // The run (first time tonight, or again after agent_cap).
            let since = ts - 86_400;
            let updates = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::count_updates_since(&conn, &org.id, since).unwrap_or(0)
            };
            let lines = bulletin.as_ref().map_or(0, |b| b.lines.len());
            let text = format!(
                "Night run {}: you are {}'s head. Your bulletin has {lines} of {} lines; {updates} update{} from members since last night (k2 organization updates). \
                 Ask members for news if you need it (k2 organization ask \"What changed today?\"), then write the bulletin, at most {} lines of {} characters: \
                 k2 organization bulletin set --file <path>. It shows to every agent at once.",
                day::short_date(night + chrono::Duration::days(1)),
                org.name,
                k2_core::org_chart::text::BULLETIN_MAX_LINES,
                if updates == 1 { "" } else { "s" },
                k2_core::org_chart::text::BULLETIN_MAX_LINES,
                k2_core::org_chart::text::BULLETIN_MAX_CHARS,
            );
            let out = super::wake(w, &head, store::WAKE_NIGHT_RUN, &from, &text, ts);
            tally(rep, &out);
            let db = k2_core::db::shared();
            let conn = db.lock();
            let at = match out {
                super::WakeOutcome::Capped => None,
                _ => Some(ts),
            };
            if matches!(out, super::WakeOutcome::Delivered) {
                rep.runs += 1;
            }
            let _ = store::record_run(&conn, &org.id, key, at, &out.word());
        }
        Some(run_at) => {
            if !last_due || row.last_call_at.is_some() {
                return;
            }
            let wrote = bulletin.as_ref().is_some_and(|b| b.updated_at >= run_at);
            if wrote {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::record_last_call(&conn, &org.id, key, Some(ts), "not_needed");
                return;
            }
            let text = format!(
                "{}'s bulletin wasn't updated tonight. Write it now (k2 organization bulletin set --file <path>, at most {} lines), or it stays as it was.",
                org.name,
                k2_core::org_chart::text::BULLETIN_MAX_LINES
            );
            let out = super::wake(w, &head, store::WAKE_LAST_CALL, &from, &text, ts);
            tally(rep, &out);
            if matches!(out, super::WakeOutcome::Delivered) {
                rep.last_calls += 1;
            }
            let at = match out {
                super::WakeOutcome::Capped => None,
                _ => Some(ts),
            };
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::record_last_call(&conn, &org.id, key, at, &out.word());
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The heartbeat monitor's hook: one pass on the blocking pool, never two
/// at once (a slow wake keeps its pass; the next tick skips).
pub fn spawn_pass(reason: &'static str) {
    if !oc::enabled() {
        return;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        k2_core::log_debug!("[org-chart] night pass still running — {reason} tick skipped");
        return;
    }
    tokio::task::spawn_blocking(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_pass(&super::RealWaker, oc::now())));
        match r {
            Ok(rep) if rep != Report::default() => k2_core::log_debug!("[org-chart] night pass ({reason}): {rep:?}"),
            Ok(_) => {}
            Err(_) => k2_core::log_debug!("[org-chart] night pass ({reason}) panicked"),
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}
