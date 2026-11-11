//! The night pass (prd-org-chart-v1 §5; vs-live A1, A2, A7): its own pass
//! on the heartbeat monitor's 60 s tick. Not a heartbeat kind (the
//! `workspace_heartbeats.kind` CHECK allows only three) and not a reminder
//! (a daemon-made reminder posts "Reminder fired" to the Thread nightly).
//!
//! Each tick, while the Org Chart is on, for every org with a head and at
//! least one other member, on tonight's night (owner's zone):
//!
//! 1. **Run**: each org's head has its own stable time between `runAt` and
//!    `lastCallAt` less a margin (01:00–04:00 by default; a hash of the
//!    org's id, `day::NightTimes::run_due`). From then it waits for a slot
//!    in the server's wake governor (`crate::wake_governor`: as many at
//!    once as the machine can take, none while it's overloaded), then gets
//!    one line, once a night (the `org_chart_nights` primary key). A head
//!    still waiting when the spread ends is skipped for the night
//!    (`skipped`): its bulletin stays, marked "not updated since …". A box
//!    that slept catches up the same way: the overdue heads queue for
//!    slots in hash order; none fire in a burst, none after the spread.
//! 2. **Last call** (`lastCallAt`, 04:30): one more line, only to a head
//!    whose run went out and who hasn't written its bulletin since; also
//!    through the governor, so it is never a fresh burst.
//! 3. **Asks**: `k2 organization ask` goes to one member at a time, about
//!    two minutes apart, each through the governor (asks go before new
//!    runs: they finish work a head already started).
//!
//! A slot ends when the head writes its bulletin (a member: sends its
//! update), when the agent goes idle, or at the time cap
//! (`day::RUN_CAP_SECS` and friends). Every wake also checks the hourly
//! wake budget; a refused wake is recorded as `agent_cap` and tried again on
//! the next tick. The bulletin's own text is never put in a line: the lines
//! point at it (§10.1).
//!
//! The same pass runs [`super::reach::tell_pass`] and prunes old wake rows.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use k2_core::org_chart::{self as oc, day, store};
use serde_json::{json, Value};

use crate::wake_governor::{Governor, Hold, Slot};

/// The governor's name for the Org Chart's wakes.
pub const SOURCE: &str = "org_chart";

/// `org_chart_nights.run_result` for a head the night ran out on.
pub const RUN_SKIPPED: &str = "skipped";
/// … for a head whose run went out.
pub const RUN_DELIVERED: &str = "delivered";

/// What one pass did (tests, the debug log).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub runs: usize,
    pub last_calls: usize,
    pub capped: usize,
    pub failed: usize,
    /// Due heads (or last calls) waiting for a governor slot.
    pub queued: usize,
    /// Heads the spread ran out on tonight (marked this pass).
    pub skipped: usize,
    /// Governor slots this pass ended.
    pub ended: usize,
    pub asks_sent: usize,
    pub asks_queued: usize,
    pub reach: super::reach::TellReport,
}

fn cap_for(kind: &str) -> i64 {
    match kind {
        store::WAKE_NIGHT_RUN => day::RUN_CAP_SECS,
        store::WAKE_LAST_CALL => day::LAST_CALL_CAP_SECS,
        _ => day::ASK_CAP_SECS,
    }
}

/// The Org Chart's rule for "done": a head wrote its bulletin since its
/// run (or last call) went out; a member sent an update since its ask.
fn slot_ended(s: &Slot) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match s.kind.as_str() {
        store::WAKE_NIGHT_RUN | store::WAKE_LAST_CALL => store::head_wrote_since(&conn, &s.project_id, s.started_at).unwrap_or(false),
        store::WAKE_ASK => store::sent_update_since(&conn, &s.project_id, s.started_at).unwrap_or(false),
        _ => false,
    }
}

/// After a restart: the org wakes still inside their cap, from the ledger.
fn adopt(ts: i64) -> Vec<Slot> {
    let kinds = [store::WAKE_NIGHT_RUN, store::WAKE_LAST_CALL, store::WAKE_ASK];
    let since = ts - day::RUN_CAP_SECS.max(day::LAST_CALL_CAP_SECS).max(day::ASK_CAP_SECS);
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::recent_wakes(&conn, &kinds, since).unwrap_or_default()
    };
    rows.into_iter()
        .filter(|(_, kind, at)| at + cap_for(kind) > ts)
        .map(|(p, kind, at)| Slot::new(SOURCE, &kind, &p, at, cap_for(&kind)))
        .collect()
}

/// One org's place in tonight's queue.
struct Due {
    org: store::Org,
    head: String,
    due: NaiveDateTime,
    row: store::NightRow,
    bulletin: Option<store::Bulletin>,
}

/// Tonight's eligible orgs (a head and at least one other member), in
/// their stable order: by due time, then id.
fn plan(times: &day::NightTimes, night: NaiveDate, key: &str) -> Vec<Due> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<Due> = store::orgs(&conn)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|org| {
            let head = org.head.clone()?;
            let others = store::members(&conn, &org.id).unwrap_or_default().into_iter().filter(|m| *m != head).count();
            if others == 0 {
                return None;
            }
            Some(Due {
                due: times.run_due(night, &org.id),
                row: store::night_row(&conn, &org.id, key).ok().flatten().unwrap_or_default(),
                bulletin: store::bulletin(&conn, &org.id).ok().flatten(),
                head,
                org,
            })
        })
        .collect();
    out.sort_by(|a, b| a.due.cmp(&b.due).then_with(|| a.org.id.cmp(&b.org.id)));
    out
}

/// Run every due night step at `now`.
pub fn run_pass(w: &dyn super::Waker, now: DateTime<Utc>) -> Report {
    let mut rep = Report::default();
    if !oc::enabled() {
        return rep;
    }
    let ts = now.timestamp();
    let gov = crate::wake_governor::current();
    gov.adopt_once(SOURCE, || adopt(ts));
    rep.ended = gov.reap(SOURCE, ts, &slot_ended).len();
    ask_pass(w, &gov, ts, &mut rep);
    let cfg = oc::config();
    let local = day::local(cfg.zone(), now);
    if let Some(night) = day::current_night_at(&cfg.night, local) {
        if local >= day::on_night(night, cfg.night.run_at) {
            let key = day::day_key(night);
            let tonight = plan(&cfg.night, night, &key);
            run_phase(w, &gov, &tonight, &cfg.night, night, &key, local, ts, &mut rep);
            if local >= day::on_night(night, cfg.night.last_call_at) {
                last_call_phase(w, &gov, &tonight, &key, ts, &mut rep);
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

/// A claim that waits for the whole machine (not just this agent) means
/// nobody after it in the queue starts this pass either.
fn machine_hold(h: &Hold) -> bool {
    !matches!(h, Hold::AgentBusy)
}

#[allow(clippy::too_many_arguments)]
fn run_phase(
    w: &dyn super::Waker,
    gov: &Governor,
    tonight: &[Due],
    times: &day::NightTimes,
    night: NaiveDate,
    key: &str,
    local: NaiveDateTime,
    ts: i64,
    rep: &mut Report,
) {
    let spread_end = times.spread_end(night);
    let mut held = false;
    for d in tonight {
        if d.row.run_at.is_some() || d.row.run_result.as_deref() == Some(RUN_SKIPPED) {
            continue;
        }
        if local >= spread_end {
            // The night ran out before this head's turn: it keeps its last
            // bulletin ("not updated since …"); nothing runs late.
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::record_run(&conn, &d.org.id, key, None, RUN_SKIPPED);
            rep.skipped += 1;
            continue;
        }
        if local < d.due {
            continue;
        }
        if held {
            rep.queued += 1;
            continue;
        }
        let slot = Slot::new(SOURCE, store::WAKE_NIGHT_RUN, &d.head, ts, day::RUN_CAP_SECS);
        let slot_key = slot.key.clone();
        if let Err(h) = gov.claim(slot) {
            held = machine_hold(&h);
            rep.queued += 1;
            continue;
        }
        let since = ts - 86_400;
        let updates = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::count_updates_since(&conn, &d.org.id, since).unwrap_or(0)
        };
        let lines = d.bulletin.as_ref().map_or(0, |b| b.lines.len());
        let text = format!(
            "Night run {}: you are {}'s head. Your bulletin has {lines} of {} lines; {updates} update{} from members since last night (k2 organization updates). \
             Ask members for news if you need it (k2 organization ask \"What changed today?\"), then write the bulletin, at most {} lines of {} characters: \
             k2 organization bulletin set --file <path>. It shows to every agent at once.",
            day::short_date(night + chrono::Duration::days(1)),
            d.org.name,
            k2_core::org_chart::text::BULLETIN_MAX_LINES,
            if updates == 1 { "" } else { "s" },
            k2_core::org_chart::text::BULLETIN_MAX_LINES,
            k2_core::org_chart::text::BULLETIN_MAX_CHARS,
        );
        let out = super::wake(w, &d.head, store::WAKE_NIGHT_RUN, &format!("org:{}", d.org.name), &text, ts);
        tally(rep, &out);
        if !matches!(out, super::WakeOutcome::Delivered) {
            gov.release(&slot_key);
        }
        let (at, word) = match &out {
            super::WakeOutcome::Delivered => {
                rep.runs += 1;
                (Some(ts), RUN_DELIVERED.to_string())
            }
            super::WakeOutcome::Capped => (None, out.word()),
            super::WakeOutcome::Failed(_) => (Some(ts), out.word()),
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::record_run(&conn, &d.org.id, key, at, &word);
    }
}

fn last_call_phase(w: &dyn super::Waker, gov: &Governor, tonight: &[Due], key: &str, ts: i64, rep: &mut Report) {
    let mut held = false;
    for d in tonight {
        // Only to heads whose run went out tonight: never a fresh start.
        let Some(run_at) = d.row.run_at else { continue };
        if d.row.run_result.as_deref() != Some(RUN_DELIVERED) || d.row.last_call_at.is_some() {
            continue;
        }
        if d.bulletin.as_ref().is_some_and(|b| b.updated_at >= run_at) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::record_last_call(&conn, &d.org.id, key, Some(ts), "not_needed");
            continue;
        }
        if held {
            rep.queued += 1;
            continue;
        }
        let slot = Slot::new(SOURCE, store::WAKE_LAST_CALL, &d.head, ts, day::LAST_CALL_CAP_SECS);
        let slot_key = slot.key.clone();
        if let Err(h) = gov.claim(slot) {
            held = machine_hold(&h);
            rep.queued += 1;
            continue;
        }
        let text = format!(
            "{}'s bulletin wasn't updated tonight. Write it now (k2 organization bulletin set --file <path>, at most {} lines), or it stays as it was.",
            d.org.name,
            k2_core::org_chart::text::BULLETIN_MAX_LINES
        );
        let out = super::wake(w, &d.head, store::WAKE_LAST_CALL, &format!("org:{}", d.org.name), &text, ts);
        tally(rep, &out);
        if matches!(out, super::WakeOutcome::Delivered) {
            rep.last_calls += 1;
        } else {
            gov.release(&slot_key);
        }
        let at = match out {
            super::WakeOutcome::Capped => None,
            _ => Some(ts),
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::record_last_call(&conn, &d.org.id, key, at, &out.word());
    }
}

/// The line a member gets for a head's ask.
pub(crate) fn ask_line(org_name: &str, head_handle: &str, question: &str) -> String {
    format!("{org_name}'s head ({head_handle}) asks: {question} Answer with: k2 organization update \"<your news>\"")
}

/// What trying one ask copy did.
pub(crate) enum AskTry {
    Sent,
    /// Waits for a governor slot.
    Held(Hold),
    /// Won't go out (budget spent, delivery failed): marked so.
    NotDelivered(String),
}

/// Try to send one member's copy of an ask now, through the governor.
pub(crate) fn send_ask(w: &dyn super::Waker, gov: &Governor, c: &store::AskCopy, ts: i64) -> AskTry {
    let slot = Slot::new(SOURCE, store::WAKE_ASK, &c.to, ts, day::ASK_CAP_SECS);
    let slot_key = slot.key.clone();
    if let Err(h) = gov.claim(slot) {
        return AskTry::Held(h);
    }
    let org_name = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_org(&conn, &c.org_id).ok().flatten().map(|o| o.name)
    };
    let Some(org_name) = org_name else {
        gov.release(&slot_key);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::set_ask_state(&conn, &c.id, store::ASK_NOT_DELIVERED);
        return AskTry::NotDelivered("the org is gone".into());
    };
    let line = ask_line(&org_name, &super::caller_handle(&c.from), &c.body);
    let out = super::wake(w, &c.to, store::WAKE_ASK, &format!("org:{org_name}"), &line, ts);
    let db = k2_core::db::shared();
    let conn = db.lock();
    match out {
        super::WakeOutcome::Delivered => {
            let _ = store::set_ask_state(&conn, &c.id, store::ASK_DELIVERED);
            AskTry::Sent
        }
        other => {
            gov.release(&slot_key);
            let _ = store::set_ask_state(&conn, &c.id, store::ASK_NOT_DELIVERED);
            AskTry::NotDelivered(other.word())
        }
    }
}

/// The ask fan-out: for each ask with members still waiting, the next
/// member in line, once the one before is done (or its slot ran out) and
/// its gap has passed, when the governor has a slot.
fn ask_pass(w: &dyn super::Waker, gov: &Governor, ts: i64, rep: &mut Report) {
    let since = ts - day::ASK_QUEUE_TTL_SECS;
    let copies = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::expire_queued_asks(&conn, since);
        store::asks_with_queued(&conn, since).unwrap_or_default()
    };
    if copies.is_empty() {
        return;
    }
    let running: HashSet<String> = gov.running().into_iter().filter(|s| s.kind == store::WAKE_ASK).map(|s| s.project_id).collect();
    let mut seen_groups: HashSet<&str> = HashSet::new();
    let mut held = false;
    for c in copies.iter().filter(|c| c.state == store::ASK_QUEUED) {
        if !seen_groups.insert(c.group.as_str()) {
            rep.asks_queued += 1;
            continue;
        }
        let group: Vec<&store::AskCopy> = copies.iter().filter(|o| o.group == c.group).collect();
        // One member at a time: the one before still in its ask holds this.
        let before = c.pos.checked_sub(1).and_then(|p| group.iter().find(|o| o.pos == p));
        if before.is_some_and(|b| b.state == store::ASK_DELIVERED && running.contains(&b.to)) {
            rep.asks_queued += 1;
            continue;
        }
        let prev_sent = before.filter(|b| b.state == store::ASK_DELIVERED).and_then(|b| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::last_wake_at(&conn, &b.to, store::WAKE_ASK, c.created_at).ok().flatten()
        });
        if ts < day::ask_due(c.created_at, c.pos, &c.id, prev_sent) || held {
            rep.asks_queued += 1;
            continue;
        }
        match send_ask(w, gov, c, ts) {
            AskTry::Sent => rep.asks_sent += 1,
            AskTry::Held(h) => {
                held = machine_hold(&h);
                rep.asks_queued += 1;
            }
            AskTry::NotDelivered(_) => {}
        }
    }
}

/// `GET /cli/organization/night`: tonight's queue (during the day, last
/// night's) and the governor, for `k2 organization night`.
pub fn status(now: DateTime<Utc>) -> Value {
    let cfg = oc::config();
    let local = day::local(cfg.zone(), now);
    let night = day::latest_night_at(&cfg.night, local);
    let key = day::day_key(night);
    let gov = crate::wake_governor::current();
    let snap = gov.snapshot();
    let in_night = day::current_night_at(&cfg.night, local).is_some();
    let spread_end = cfg.night.spread_end(night);
    let running: HashSet<(String, String)> = snap.running.iter().filter(|s| s.source == SOURCE).map(|s| (s.kind.clone(), s.project_id.clone())).collect();
    let mut counts = [0usize; 5]; // queued, running, done, skipped, failed
    let orgs: Vec<Value> = plan(&cfg.night, night, &key)
        .into_iter()
        .map(|d| {
            let wrote = match (d.row.run_at, d.bulletin.as_ref()) {
                (Some(r), Some(b)) => b.updated_at >= r,
                _ => false,
            };
            let state = match (d.row.run_at, d.row.run_result.as_deref()) {
                (None, Some(RUN_SKIPPED)) => "skipped",
                // The night ended (or ran out) with this head never run.
                (None, _) if !in_night || local >= spread_end => "skipped",
                (None, _) => "queued",
                (Some(_), Some(RUN_DELIVERED)) => {
                    if running.contains(&(store::WAKE_NIGHT_RUN.to_string(), d.head.clone()))
                        || running.contains(&(store::WAKE_LAST_CALL.to_string(), d.head.clone()))
                    {
                        "running"
                    } else {
                        "done"
                    }
                }
                (Some(_), _) => "failed",
            };
            let i = ["queued", "running", "done", "skipped", "failed"].iter().position(|s| *s == state).unwrap_or(0);
            counts[i] += 1;
            json!({
                "org": { "id": d.org.id, "name": d.org.name },
                "head": { "id": d.head, "handle": super::caller_handle(&d.head) },
                "due": day::fmt_hhmm(d.due.time()),
                "state": state,
                "ranAt": d.row.run_at,
                "wrote": wrote,
                "lastCall": d.row.last_call_result,
            })
        })
        .collect();
    json!({
        "ok": true,
        "night": key,
        "tonight": in_night,
        "zone": day::zone_name(cfg.zone()),
        "runAt": day::fmt_hhmm(cfg.night.run_at),
        "spreadEndsAt": day::fmt_hhmm(spread_end.time()),
        "spreadMinutes": cfg.night.spread_minutes(),
        "lastCallAt": day::fmt_hhmm(cfg.night.last_call_at),
        "dayStartsAt": day::fmt_hhmm(cfg.night.day_starts_at),
        "counts": { "queued": counts[0], "running": counts[1], "done": counts[2], "skipped": counts[3], "failed": counts[4] },
        "orgs": orgs,
        "governor": snap.to_json(),
    })
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
