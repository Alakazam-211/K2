//! One refresher for the single-record path (prd-usage-single-source-v1 §4.4).
//!
//! Every trigger (the 15-minute tick, the menu, Settings, the phone, after
//! a switch, a window past its reset) asks here. Per token: one vendor
//! call at a time, at most once a minute whoever asks, back-off per token.
//!
//! **The freeze rules (0.46.10):** no tool lock and no database lock across
//! a vendor call. This file takes neither lock directly; `accept` holds the
//! database for one read-merge-write, and the vendor call runs on its own
//! named worker thread with no lock held. `store::db_held_here` is checked
//! on entry (held: `Off("busy")`, never a wait).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use k2_core::llm_accounts::{self as wallet, state, store, Entry, Tool};
use serde_json::Value;

use crate::subscription_usage::{self as su, HarnessUsage};
use crate::usage_store::{accept, Outcome as AcceptOutcome, Reading};

/// At most once a minute per token, counted from the last admitted request.
pub const EVERY: Duration = Duration::from_secs(60);
/// A person's request waits this long for the in-flight run. Under the
/// phone's 15 s client timeout; a late result arrives by the event.
pub const PERSON_WAIT: Duration = Duration::from_secs(12);
/// The tick re-checks a token not read by the vendor in this long. 14, so a
/// 15-minute tick does not just miss.
pub const TICK_MAX_AGE: Duration = Duration::from_secs(14 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Honours every back-off.
    Background,
    /// Ignores failure back-off but waits out a 429.
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    None,
    For(Duration),
    UntilDone,
}

/// How a run went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Ok,
    Failed,
    /// The caller stopped waiting (no result yet; it arrives by the event).
    Pending,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// This request started the run.
    Ran(Check),
    /// A run was already going; this request joined it.
    Joined(Check),
    /// Asked again within a minute. `retryAfterSecs`.
    TooSoon(u64),
    /// Backing off until then.
    BackingOff(DateTime<Utc>),
    /// Not read: `airgap`, `unavailable`, `no_usage`, `not_signed_in`,
    /// `probes_off`, `busy`. The first five are today's words on `/for`.
    Off(&'static str),
}

// ── The vendor seam (U22) ───────────────────────────────────────────

/// Who reads the vendor. The real one runs the existing probes; tests pass
/// a fake (consulted before `vendor_calls_allowed`).
pub trait Vendor: Send + Sync {
    /// The live store's reading for `tool`, with no lock held.
    fn live(&self, tool: Tool) -> HarnessUsage;
    /// One token's slot reading. `None` = it can't be checked now (the slot
    /// is busy, or the token went live and its slot is stale).
    fn slot(&self, entry: &Entry) -> Option<HarnessUsage>;
}

struct RealVendor;

impl Vendor for RealVendor {
    fn live(&self, tool: Tool) -> HarnessUsage {
        debug_assert!(!store::db_held_here() && !store::tool_lock_held_here(tool), "no lock across a vendor call");
        su::probe_live_row(tool.as_str())
    }

    fn slot(&self, entry: &Entry) -> Option<HarnessUsage> {
        let tool = entry.tool()?;
        debug_assert!(!store::db_held_here() && !store::tool_lock_held_here(tool), "no lock across a vendor call");
        crate::llm_accounts_runtime::slot_usage_check(entry, &|t| su::claude_usage_get(t))
    }
}

#[cfg(test)]
fn test_vendor_slot() -> &'static Mutex<Option<Arc<dyn Vendor>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn Vendor>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn vendor_for_run() -> Result<Arc<dyn Vendor>, &'static str> {
    #[cfg(test)]
    if let Some(v) = test_vendor_slot().lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return Ok(v);
    }
    if crate::llm_accounts_runtime::vendor_calls_allowed() {
        Ok(Arc::new(RealVendor))
    } else {
        Err("probes_off")
    }
}

// ── Single flight and the once-a-minute gate ────────────────────────

struct Flight {
    done: Mutex<Option<Check>>,
    finished: Condvar,
}

impl Flight {
    fn new() -> Arc<Flight> {
        Arc::new(Flight { done: Mutex::new(None), finished: Condvar::new() })
    }

    fn finish(&self, check: Check) {
        *self.done.lock().unwrap_or_else(|p| p.into_inner()) = Some(check);
        self.finished.notify_all();
    }

    /// The run's result, or `Pending` when the wait ends first.
    fn wait(&self, wait: Wait) -> Check {
        let deadline = match wait {
            Wait::None => return Check::Pending,
            Wait::For(d) => Some(Instant::now() + d),
            Wait::UntilDone => None,
        };
        let mut guard = self.done.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(check) = *guard {
                return check;
            }
            match deadline {
                None => guard = self.finished.wait(guard).unwrap_or_else(|p| p.into_inner()),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Check::Pending;
                    }
                    guard = self.finished.wait_timeout(guard, left).unwrap_or_else(|p| p.into_inner()).0;
                }
            }
        }
    }
}

fn flights() -> &'static Mutex<HashMap<String, Arc<Flight>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<Flight>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn admitted() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn backoff_key(id: &str) -> String {
    format!("acct:{id}")
}

/// Why a token isn't checked at all (before any gate).
fn not_checkable(entry: Option<&Entry>, id: &str) -> Result<Tool, &'static str> {
    let Some(e) = entry else { return Err("unavailable") };
    if e.removed_at.is_some() {
        return Err("unavailable");
    }
    let Some(tool) = e.tool() else { return Err("no_usage") };
    if e.is_api_key() || !tool.subscription_supported() {
        return Err("no_usage");
    }
    if !is_live_owner(tool, id) && e.state != state::SIGNED_IN {
        return Err("not_signed_in");
    }
    Ok(tool)
}

fn entry_of(id: &str) -> Option<Entry> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    wallet::get(&conn, id).ok().flatten()
}

fn is_live_owner(tool: Tool, id: &str) -> bool {
    let Some(db) = k2_core::db::try_shared() else { return false };
    let conn = db.lock();
    wallet::live_owner(&conn, tool).ok().flatten().as_deref() == Some(id)
}

/// Who is live for a tool and since when: unchanged across a live-store
/// check means no switch ran during it.
fn generation(tool: Tool) -> Option<(String, i64)> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    let owner = wallet::live_owner(&conn, tool).ok()??;
    let info = wallet::active_info(&conn, tool).ok()??;
    Some((owner, info.switched_at))
}

enum Begin {
    Run { flight: Arc<Flight>, started: bool },
    Done(Outcome),
}

/// Admit a request: join a run, or start one, or say why not.
fn begin(id: &str, why: Why) -> Begin {
    if store::db_held_here() {
        k2_core::log_debug!("[usage] {id}: refresh refused, the database is held here");
        return Begin::Done(Outcome::Off("busy"));
    }
    if su::airgap_on() {
        return Begin::Done(Outcome::Off("airgap"));
    }
    let entry = entry_of(id);
    let tool = match not_checkable(entry.as_ref(), id) {
        Ok(t) => t,
        Err(why_not) => return Begin::Done(Outcome::Off(why_not)),
    };
    let entry = entry.expect("checked above");
    let now_i = Instant::now();
    let now = Utc::now();
    let flight = {
        let mut map = flights().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(f) = map.get(id) {
            return Begin::Run { flight: f.clone(), started: false };
        }
        // Stamped before the vendor check, so "probes off" also gates (U13).
        let mut admits = admitted().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(prev) = admits.get(id) {
            let since = now_i.saturating_duration_since(*prev);
            if since < EVERY {
                let secs = (EVERY - since).as_secs().max(1);
                return Begin::Done(Outcome::TooSoon(secs));
            }
        }
        if let Some((until, rate_limited)) = su::backing_off(&backoff_key(id), now) {
            if why == Why::Background || rate_limited {
                return Begin::Done(Outcome::BackingOff(until));
            }
        }
        admits.insert(id.to_string(), now_i);
        drop(admits);
        match vendor_for_run() {
            Err(reason) => return Begin::Done(Outcome::Off(reason)),
            Ok(vendor) => {
                let flight = Flight::new();
                map.insert(id.to_string(), flight.clone());
                let run = RunArgs { id: id.to_string(), tool, entry, vendor, flight: flight.clone() };
                spawn_run(run);
                flight
            }
        }
    };
    Begin::Run { flight, started: true }
}

struct RunArgs {
    id: String,
    tool: Tool,
    entry: Entry,
    vendor: Arc<dyn Vendor>,
    flight: Arc<Flight>,
}

/// Each run is one named worker thread; every caller waits on it with its
/// own timeout (U10).
fn spawn_run(run: RunArgs) {
    #[cfg(test)]
    let (pin, scoped) = (crate::usage_store::current_pin(), k2_core::db::scoped_handle_for_test());
    let id = run.id.clone();
    let flight = run.flight.clone();
    let spawned = std::thread::Builder::new().name(format!("usage-check-{id}")).spawn(move || {
        #[cfg(test)]
        let _pin = pin.map(crate::usage_store::pin);
        #[cfg(test)]
        if let Some(handle) = scoped {
            return k2_core::db::with_scoped_handle(handle, || finish_run(run));
        }
        finish_run(run);
    });
    if spawned.is_err() {
        k2_core::log_debug!("[usage] {id}: could not start a check");
        flights().lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
        flight.finish(Check::Failed);
    }
}

fn finish_run(run: RunArgs) {
    let check = check_once(&run.entry, run.tool, run.vendor.as_ref());
    flights().lock().unwrap_or_else(|p| p.into_inner()).remove(&run.id);
    run.flight.finish(check);
}

/// One vendor check and its write. No lock held across the vendor call.
fn check_once(entry: &Entry, tool: Tool, vendor: &dyn Vendor) -> Check {
    let id = entry.id.as_str();
    let before = generation(tool);
    let is_live = before.as_ref().is_some_and(|g| g.0 == id);
    let measured = Utc::now();
    let row = if is_live {
        vendor.live(tool)
    } else {
        match vendor.slot(entry) {
            Some(row) => row,
            None => return Check::Ok,
        }
    };
    let failed = row.error.is_some();
    let after = generation(tool);
    if is_live {
        // A live-store reading is only this token's if no switch ran.
        if before != after {
            k2_core::log_debug!("[usage] {id}: a switch ran during the check; reading dropped");
            return if failed { Check::Failed } else { Check::Ok };
        }
        if wallet::wallet::live_is_other_account(tool) {
            k2_core::log_debug!("[usage] {id}: the live sign-in is another account; reading dropped");
            return if failed { Check::Failed } else { Check::Ok };
        }
    } else if failed && before != after {
        // U8: a failure of a stale slot copy must not mark or back off a
        // token that went live meanwhile.
        k2_core::log_debug!("[usage] {id}: check failed while the live token changed; dropped");
        return Check::Failed;
    }
    match accept(id, Reading::from_vendor_row(&row, measured)) {
        AcceptOutcome::Accepted { .. } => {
            su::note_outcome(&backoff_key(id), &row, Utc::now());
        }
        AcceptOutcome::Dropped(why) => {
            k2_core::log_debug!("[usage] {id}: reading dropped ({why})");
        }
    }
    if failed {
        Check::Failed
    } else {
        Check::Ok
    }
}

/// Ask for one token. See the module doc.
pub fn request(id: &str, why: Why, wait: Wait) -> Outcome {
    match begin(id, why) {
        Begin::Done(outcome) => outcome,
        Begin::Run { flight, started } => {
            let check = flight.wait(wait);
            if started {
                Outcome::Ran(check)
            } else {
                Outcome::Joined(check)
            }
        }
    }
}

/// Ask for several tokens at once and wait up to `overall` for all of them
/// together (U10). A slow token can't make the route wait token by token.
pub fn request_many(ids: &[String], why: Why, overall: Duration) -> Vec<(String, Outcome)> {
    let begun: Vec<(String, Begin)> = ids.iter().map(|id| (id.clone(), begin(id, why))).collect();
    let deadline = Instant::now() + overall;
    begun
        .into_iter()
        .map(|(id, b)| {
            let outcome = match b {
                Begin::Done(o) => o,
                Begin::Run { flight, started } => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let check = flight.wait(Wait::For(left));
                    if started {
                        Outcome::Ran(check)
                    } else {
                        Outcome::Joined(check)
                    }
                }
            };
            (id, outcome)
        })
        .collect()
}

/// A served record with a window past its reset: one background re-check
/// of that token, not waited on. Each token's 60 s gate keeps this cheap.
pub fn kick_if_reset_passed(id: &str, record: &Value) {
    let passed = record["windows"].as_array().is_some_and(|ws| ws.iter().any(|w| w["resetPassed"].as_bool() == Some(true)));
    if passed {
        let _ = request(id, Why::Background, Wait::None);
    }
}

// ── The tick (U9) ───────────────────────────────────────────────────

/// When the vendor last answered for this row: `vendorCheckedAt`, or the
/// row's `checkedAt` for an old row that came from a probe.
fn vendor_time(e: &Entry) -> Option<DateTime<Utc>> {
    let row: HarnessUsage = serde_json::from_str(e.usage_json.as_deref()?).ok()?;
    let at = row.vendor_checked_at.clone().or_else(|| (row.source == "probe").then(|| row.checked_at.clone()))?;
    DateTime::parse_from_rfc3339(&at).ok().map(|t| t.with_timezone(&Utc))
}

fn vendor_fresh(e: &Entry, now: DateTime<Utc>) -> bool {
    let max_age = chrono::Duration::from_std(TICK_MAX_AGE).unwrap_or_default();
    vendor_time(e).is_some_and(|t| now.signed_duration_since(t) < max_age)
}

/// The tokens a tick asks about: every live owner whatever its state, and
/// every other signed-in subscription token, not read by the vendor lately.
fn tick_tokens(now: DateTime<Utc>) -> Vec<String> {
    let Some(db) = k2_core::db::try_shared() else { return Vec::new() };
    let conn = db.lock();
    let owners: Vec<String> = Tool::SUBSCRIPTION.iter().filter_map(|t| wallet::live_owner(&conn, *t).ok().flatten()).collect();
    let entries = wallet::list(&conn).unwrap_or_default();
    entries
        .into_iter()
        .filter(|e| {
            let eligible = e.tool().is_some_and(|t| t.subscription_supported()) && !e.is_api_key() && e.removed_at.is_none();
            eligible && (owners.contains(&e.id) || e.state == state::SIGNED_IN) && !vendor_fresh(e, now)
        })
        .map(|e| e.id)
        .collect()
}

/// One 15-minute pass: adopt any login with no wallet entry, then ask for
/// each token that is due. Runs off the request; waits for its own runs.
pub fn tick() {
    if su::airgap_on() {
        return;
    }
    crate::llm_accounts_runtime::spawn_adopt_live_logins();
    for id in tick_tokens(Utc::now()) {
        let outcome = request(&id, Why::Background, Wait::UntilDone);
        k2_core::log_debug!("[usage] tick {id}: {outcome:?}");
    }
}

#[cfg(test)]
pub(crate) fn clear_for_tests() {
    flights().lock().unwrap_or_else(|p| p.into_inner()).clear();
    admitted().lock().unwrap_or_else(|p| p.into_inner()).clear();
    *test_vendor_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
#[path = "usage_refresh_tests.rs"]
mod tests;
