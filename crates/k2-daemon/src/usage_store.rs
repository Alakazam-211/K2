//! One record per token for the usage percent (prd-usage-single-source-v1).
//!
//! The per-token row (`llm_accounts.usage_json` + `usage_checked_at`) is the
//! only store on this path. [`accept`] is the only code that writes it;
//! [`merge`] is the pure decision behind it, so the rules are table-tested
//! without a database. Every writer (vendor checks, Claude's statusLine,
//! Codex's session log) comes through here.
//!
//! All of this sits behind the `usageSingleRecord` setting. [`single_record_on`]
//! is the one gate; off = today's behaviour (the shared file plus the
//! per-token row). S7 retires the old path on Rosson's go.
//!
//! **Lock rule (0.46.10 freeze):** [`accept`] holds the database for one
//! read-merge-write and nothing else. No tool lock, no vendor call, and the
//! event goes out after the database is released.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};

use k2_core::llm_accounts::{Entry, Tool};
use serde_json::{json, Value};

use crate::subscription_usage::{self as su, HarnessUsage, UsageWindow};

/// Two `resetsAt` values this close are the same window. Exact equality
/// doesn't work: the vendor's values jitter and Codex's are computed.
pub const SAME_WINDOW_MINUTES: i64 = 10;
/// A claimed measured time this far past arrival is not believed (U2).
pub const FUTURE_LIMIT_MINUTES: i64 = 5;
/// A token with no other change still announces this often, so "as of" moves.
pub const EVENT_EVERY_SECS: u64 = 60;

// ── The switch (U3, U18) ────────────────────────────────────────────

static SETTING: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static PIN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Sync the persisted `usageSingleRecord` setting (boot, settings update).
pub fn set_single_record(on: bool) {
    SETTING.store(on, Ordering::Relaxed);
}

/// The one gate. On = the single-record path everywhere; off = today's.
pub fn single_record_on() -> bool {
    #[cfg(test)]
    if let Some(on) = PIN.with(|p| p.get()) {
        return on;
    }
    SETTING.load(Ordering::Relaxed)
}

/// Test pin for this thread: the single-record switch reads `on` until
/// the guard drops. Worker threads inherit it (see `usage_refresh`).
#[cfg(test)]
pub(crate) fn pin(on: bool) -> PinGuard {
    let prev = PIN.with(|p| p.replace(Some(on)));
    PinGuard { prev, _not_send: std::marker::PhantomData }
}

#[cfg(test)]
pub(crate) fn current_pin() -> Option<bool> {
    PIN.with(|p| p.get())
}

#[cfg(test)]
pub(crate) struct PinGuard {
    prev: Option<bool>,
    _not_send: std::marker::PhantomData<*const ()>,
}

#[cfg(test)]
impl Drop for PinGuard {
    fn drop(&mut self) {
        PIN.with(|p| p.set(self.prev));
    }
}

// ── The plan name ───────────────────────────────────────────────────

/// One name for a plan, used at write and at serve. Claude goes through
/// the tier logic (`max_20x` is "Ultra"); other tools get the first letter
/// upper-cased.
pub fn plan_name(tool: &str, tier: &str, subscription: &str) -> String {
    if tool == "claude" {
        return su::plan_label(tier, subscription);
    }
    let raw = if subscription.is_empty() { tier } else { subscription };
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// A stored or raw plan string, normalised when served (old rows, U27).
pub fn normalize_plan(tool: &str, stored: &str) -> String {
    plan_name(tool, stored, stored)
}

// ── A reading ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Vendor,
    Feed,
}

/// What a reading says.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Numbers. A vendor check is a full reading; a feed carries only the
    /// windows it knows.
    Windows(Vec<UsageWindow>),
    /// A status-only result (U5): `Not signed in` / `Sign-in expired`, with
    /// or without an error. Keeps the windows and each `observedAt`.
    Status { status: String, error: Option<String> },
    /// A failed check: touches only `error` and `attemptedAt`.
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub kind: Kind,
    pub measured: DateTime<Utc>,
    pub source: String,
    pub plan: String,
    pub body: Body,
}

impl Reading {
    /// A live feed's reading with its measured time checked against
    /// arrival (U2): a claim more than five minutes ahead is refused, and
    /// a smaller skew reads as the arrival time.
    pub fn feed(
        claimed: DateTime<Utc>,
        arrival: DateTime<Utc>,
        source: &str,
        plan: &str,
        windows: Vec<UsageWindow>,
    ) -> Result<Reading, String> {
        if claimed > arrival + Duration::minutes(FUTURE_LIMIT_MINUTES) {
            return Err(format!("measured time {} is ahead of arrival", claimed.to_rfc3339()));
        }
        Ok(Reading {
            kind: Kind::Feed,
            measured: claimed.min(arrival),
            source: source.to_string(),
            plan: plan.to_string(),
            body: Body::Windows(windows),
        })
    }

    /// A probe's row as a vendor reading measured at `measured` (the start
    /// of this token's own call).
    pub fn from_vendor_row(row: &HarnessUsage, measured: DateTime<Utc>) -> Reading {
        let body = if row.status == su::STATUS_NOT_SIGNED_IN || row.status == su::STATUS_SIGN_IN_EXPIRED {
            Body::Status { status: row.status.clone(), error: row.error.clone() }
        } else if let Some(error) = &row.error {
            Body::Failed { error: error.clone() }
        } else {
            Body::Windows(row.windows.clone())
        };
        Reading { kind: Kind::Vendor, measured, source: "probe".to_string(), plan: row.plan.clone(), body }
    }
}

/// What a merge decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Store `row`. `changed`: a number, a reset time or a state moved.
    Keep { row: HarnessUsage, changed: bool },
    Drop(String),
}

// ── The merge (pure) ────────────────────────────────────────────────

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// The window key: same `id` when both have one, else same `label` (U25).
fn same_window_key(a: &UsageWindow, b: &UsageWindow) -> bool {
    if !a.id.is_empty() && !b.id.is_empty() {
        a.id == b.id
    } else {
        a.label == b.label
    }
}

/// An old row's windows read as the row's `checkedAt` and `source`. Stamp
/// them before anything moves `checkedAt`.
fn materialise(prev: &HarnessUsage) -> HarnessUsage {
    let mut out = prev.clone();
    let source = if prev.source.is_empty() { "probe".to_string() } else { prev.source.clone() };
    for w in &mut out.windows {
        if w.observed_at.is_none() && !prev.checked_at.is_empty() {
            w.observed_at = Some(prev.checked_at.clone());
        }
        if w.source.is_none() {
            w.source = Some(source.clone());
        }
    }
    out
}

fn observed(w: &UsageWindow) -> Option<DateTime<Utc>> {
    w.observed_at.as_deref().and_then(parse_time)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reset {
    Same,
    Later,
    Earlier,
    /// Either side empty or unparseable (U4).
    Unknown,
}

fn reset_relation(new: &UsageWindow, stored: &UsageWindow) -> Reset {
    let (Some(n), Some(s)) = (parse_time(&new.resets_at), parse_time(&stored.resets_at)) else {
        return Reset::Unknown;
    };
    let gap = n.signed_duration_since(s);
    if gap.num_seconds().abs() <= SAME_WINDOW_MINUTES * 60 {
        Reset::Same
    } else if gap > Duration::zero() {
        Reset::Later
    } else {
        Reset::Earlier
    }
}

/// Why a feed can't take this stored window, or `Ok`.
fn feed_takes(stored: &UsageWindow, new: &UsageWindow, measured: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), &'static str> {
    if let Some(t) = observed(stored) {
        // A stored time in the future counts as now (U2).
        if measured <= t.min(now) {
            return Err("measured at or before the stored reading");
        }
    }
    if parse_time(&new.resets_at).is_some_and(|r| r <= measured) {
        return Err("the window already closed when it was measured");
    }
    match reset_relation(new, stored) {
        Reset::Later => Ok(()),
        Reset::Earlier => Err("an earlier window than the stored one"),
        Reset::Same | Reset::Unknown if new.used >= stored.used => Ok(()),
        Reset::Same | Reset::Unknown => Err("lower within the same window"),
    }
}

/// A vendor reading only loses to a stored window measured later than it
/// (U1, U7). A stored time in the future blocks nothing (U2).
fn vendor_takes(stored: &UsageWindow, measured: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match observed(stored) {
        None => true,
        Some(t) if t > now => true,
        Some(t) => measured >= t,
    }
}

fn stamp(new: &UsageWindow, reading: &Reading, stored: Option<&UsageWindow>) -> UsageWindow {
    let mut w = new.clone();
    w.observed_at = Some(reading.measured.to_rfc3339());
    w.source = Some(reading.source.clone());
    // Served-only fields never reach the store.
    w.used_percent = 0.0;
    w.window_start = None;
    w.reset_passed = false;
    if w.window_secs.is_none() {
        w.window_secs = stored.and_then(|s| s.window_secs);
    }
    w
}

/// A number, a reset time, or a window appearing or leaving.
fn windows_moved(prev: &[UsageWindow], next: &[UsageWindow]) -> bool {
    prev.len() != next.len()
        || next.iter().any(|n| match prev.iter().find(|p| same_window_key(p, n)) {
            Some(p) => (p.used - n.used).abs() > f64::EPSILON || p.resets_at != n.resets_at,
            None => true,
        })
}

/// `checkedAt` never moves backwards: the later of the stored time and
/// this reading's.
fn later_of(a: &str, b: DateTime<Utc>) -> String {
    match parse_time(a) {
        Some(t) if t > b => a.to_string(),
        _ => b.to_rfc3339(),
    }
}

/// `prev` with the reading applied, or why it was dropped.
pub fn merge(prev: Option<&HarnessUsage>, reading: &Reading, harness: &str, account_id: &str, now: DateTime<Utc>) -> Decision {
    let stored = prev.map(materialise).unwrap_or_else(|| HarnessUsage { harness: harness.to_string(), ..Default::default() });
    let plan = if reading.plan.is_empty() { stored.plan.clone() } else { normalize_plan(harness, &reading.plan) };
    let mut row = stored.clone();
    row.harness = harness.to_string();
    row.account_id = Some(account_id.to_string());
    row.plan = plan;
    row.state.clear();
    row.next_probe_at = None;
    match (&reading.body, reading.kind) {
        (Body::Windows(new), Kind::Feed) => {
            let mut windows = stored.windows.clone();
            let mut taken = 0usize;
            let mut last_why = "no window it carried could be taken";
            for n in new {
                match windows.iter().position(|w| same_window_key(w, n)) {
                    Some(i) => match feed_takes(&windows[i], n, reading.measured, now) {
                        Ok(()) => {
                            windows[i] = stamp(n, reading, Some(&windows[i]));
                            taken += 1;
                        }
                        Err(why) => last_why = why,
                    },
                    None if parse_time(&n.resets_at).is_some_and(|r| r <= reading.measured) => {
                        last_why = "the window already closed when it was measured";
                    }
                    None => {
                        windows.push(stamp(n, reading, None));
                        taken += 1;
                    }
                }
            }
            if taken == 0 {
                return Decision::Drop(last_why.to_string());
            }
            row.windows = windows;
            row.status.clear();
            row.error = None;
            row.attempted_at = None;
            row.checked_at = later_of(&stored.checked_at, reading.measured);
            row.source = reading.source.clone();
            let changed = windows_moved(&stored.windows, &row.windows) || stored.status != row.status || stored.error.is_some();
            Decision::Keep { row, changed }
        }
        (Body::Windows(new), Kind::Vendor) => {
            let mut windows: Vec<UsageWindow> = Vec::with_capacity(new.len().max(stored.windows.len()));
            let (mut taken, mut removed) = (0usize, 0usize);
            for s in &stored.windows {
                match new.iter().find(|n| same_window_key(s, n)) {
                    Some(n) if vendor_takes(s, reading.measured, now) => {
                        windows.push(stamp(n, reading, Some(s)));
                        taken += 1;
                    }
                    Some(_) => windows.push(s.clone()),
                    // Gone from a full reading: removed, unless a feed
                    // measured it after this check began.
                    None if observed(s).is_some_and(|t| t <= now && t > reading.measured) => windows.push(s.clone()),
                    None => removed += 1,
                }
            }
            for n in new {
                if !stored.windows.iter().any(|s| same_window_key(s, n)) {
                    windows.push(stamp(n, reading, None));
                    taken += 1;
                }
            }
            if taken == 0 && removed == 0 && !stored.windows.is_empty() {
                return Decision::Drop("measured earlier than every stored window".to_string());
            }
            row.status = if windows.is_empty() { su::STATUS_NO_WINDOW.to_string() } else { String::new() };
            row.windows = windows;
            row.error = None;
            row.attempted_at = None;
            row.checked_at = later_of(&stored.checked_at, reading.measured);
            row.source = reading.source.clone();
            row.vendor_checked_at = Some(reading.measured.to_rfc3339());
            let changed = windows_moved(&stored.windows, &row.windows) || stored.status != row.status || stored.error.is_some();
            Decision::Keep { row, changed }
        }
        (Body::Status { status, error }, _) => {
            if error.is_some() && newest_stored(&stored, now).is_some_and(|t| t > reading.measured) {
                return Decision::Drop("an accepted reading was measured after this check began".to_string());
            }
            row.status = status.clone();
            row.error = error.clone();
            if error.is_none() {
                row.checked_at = later_of(&stored.checked_at, reading.measured);
                row.attempted_at = None;
                row.vendor_checked_at = Some(reading.measured.to_rfc3339());
            } else {
                row.attempted_at = Some(reading.measured.to_rfc3339());
            }
            let changed = stored.status != row.status || stored.error != row.error;
            Decision::Keep { row, changed }
        }
        (Body::Failed { error }, _) => {
            if newest_stored(&stored, now).is_some_and(|t| t > reading.measured) {
                return Decision::Drop("an accepted reading was measured after this check began".to_string());
            }
            row.error = Some(error.clone());
            row.attempted_at = Some(reading.measured.to_rfc3339());
            let changed = stored.error != row.error;
            Decision::Keep { row, changed }
        }
    }
}

/// The newest time any accepted reading was measured (never in the
/// future, so a bad stored time can't suppress a failure).
fn newest_stored(row: &HarnessUsage, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    row.windows
        .iter()
        .filter_map(observed)
        .chain(parse_time(&row.checked_at))
        .filter(|t| *t <= now)
        .max()
}

// ── The one write ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Accepted { changed: bool },
    Dropped(String),
}

fn last_events() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Announce a write: on any change, or at most once a minute so "as of"
/// moves. Never under the database lock.
fn announce(tool: &str, account_id: &str, source: &str, observed_at: &str, changed: bool) {
    let due = {
        let mut map = last_events().lock().unwrap_or_else(|p| p.into_inner());
        let due = changed || map.get(account_id).is_none_or(|t| t.elapsed().as_secs() >= EVENT_EVERY_SECS);
        if due {
            map.insert(account_id.to_string(), Instant::now());
        }
        due
    };
    if due {
        crate::session_events::emit_subscription_usage_changed_at(Some(tool), Some(account_id), source, Some(observed_at));
    }
}

/// The only code that writes `usage_json`: one read-merge-write under the
/// database lock, then the event.
pub fn accept(account_id: &str, reading: Reading) -> Outcome {
    let now = Utc::now();
    let Some(db) = k2_core::db::try_shared() else { return Outcome::Dropped("no database".to_string()) };
    let (tool, changed) = {
        let conn = db.lock();
        let entry = match k2_core::llm_accounts::get(&conn, account_id) {
            Ok(Some(e)) if e.removed_at.is_none() => e,
            _ => return Outcome::Dropped("unknown or removed token".to_string()),
        };
        let prev: Option<HarnessUsage> = entry.usage_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
        let (row, changed) = match merge(prev.as_ref(), &reading, &entry.tool, account_id, now) {
            Decision::Keep { row, changed } => (row, changed),
            Decision::Drop(why) => return Outcome::Dropped(why),
        };
        let text = match serde_json::to_string(&row) {
            Ok(t) => t,
            Err(e) => return Outcome::Dropped(format!("serialize: {e}")),
        };
        let checked = if row.error.is_none() { parse_time(&row.checked_at).map(|t| t.timestamp()) } else { None };
        let res = k2_core::llm_accounts::update_meta(
            &conn,
            account_id,
            &k2_core::llm_accounts::MetaUpdate { usage_json: Some(text), usage_checked_at: checked, ..Default::default() },
        );
        if let Err(e) = res {
            return Outcome::Dropped(format!("write: {e}"));
        }
        (entry.tool.clone(), changed)
    };
    announce(&tool, account_id, &reading.source, &reading.measured.to_rfc3339(), changed);
    view_after_accept(&tool, account_id);
    Outcome::Accepted { changed }
}

// ── The file, as a view (D-1) ───────────────────────────────────────

/// `subscriptions.json` stays for one release as a view of the live token's
/// record: nothing reads it for truth, and it is the way back if a daemon
/// is rolled back. Rebuilt from the current record (U29), outside the
/// database lock, under `cache_lock` (inside `update_cache_row`).
fn view_after_accept(tool: &str, account_id: &str) {
    let Some(t) = k2_core::llm_accounts::Tool::parse(tool) else { return };
    let live = {
        let Some(db) = k2_core::db::try_shared() else { return };
        let conn = db.lock();
        k2_core::llm_accounts::live_owner(&conn, t).ok().flatten().as_deref() == Some(account_id)
    };
    if live {
        su::update_cache_row(tool, |_| live_record(t));
    }
}

/// The live token's stored record, as the view holds it.
fn live_record(tool: k2_core::llm_accounts::Tool) -> Option<HarnessUsage> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    let owner = k2_core::llm_accounts::live_owner(&conn, tool).ok()??;
    let e = k2_core::llm_accounts::get(&conn, &owner).ok()??;
    serde_json::from_str(e.usage_json.as_deref()?).ok()
}

// ── The one served shape (§4.5) ─────────────────────────────────────

fn chip_window(row: &HarnessUsage, now: DateTime<Utc>) -> Option<&UsageWindow> {
    su::chip_percent(row, now)?;
    let mut best: Option<(&UsageWindow, i64, Option<DateTime<Utc>>)> = None;
    for w in row.windows.iter().filter(|w| w.used.is_finite() && su::chip_window(&row.harness, w)) {
        let (percent, time) = (su::window_percent(w, now), observed(w));
        let better = match &best {
            None => true,
            // Highest percent, then the most recently measured, then the
            // first listed.
            Some((_, p, t)) => percent > *p || (percent == *p && time > *t),
        };
        if better {
            best = Some((w, percent, time));
        }
    }
    best.map(|(w, _, _)| w)
}

/// The record every route serves for one row. `account_id` is the token it
/// belongs to (`None` = no token, no reading).
pub fn record_value(mut row: HarnessUsage, account_id: Option<&str>, now: DateTime<Utc>) -> Value {
    let key = account_id.map(|id| format!("acct:{id}")).unwrap_or_default();
    row.account_id = account_id.map(str::to_string);
    let harness = row.harness.clone();
    let plan = normalize_plan(&harness, &row.plan);
    let row = su::decorate_row(materialise_served(row), now, &key);
    let row = HarnessUsage { plan, ..row };
    let chip = chip_window(&row, now).cloned();
    let newest_window = row.windows.iter().filter_map(observed).max();
    let as_of = match (&chip, newest_window) {
        (Some(w), _) => w.observed_at.clone(),
        (None, Some(t)) => Some(t.to_rfc3339()),
        (None, None) if !row.checked_at.is_empty() => Some(row.checked_at.clone()),
        (None, None) => None,
    };
    let source = match &chip {
        Some(w) => w.source.clone().unwrap_or_else(|| row.source.clone()),
        None => row.source.clone(),
    };
    let mut v = su::served_row_json(&row, now);
    let o = v.as_object_mut().expect("served row is an object");
    o.insert("accountId".into(), json!(account_id));
    o.insert("harness".into(), json!(harness));
    o.insert("asOf".into(), json!(as_of));
    o.insert("source".into(), json!(source));
    o.insert("error".into(), json!(row.error));
    o.insert("attemptedAt".into(), json!(row.attempted_at));
    o.insert("nextProbeAt".into(), json!(row.next_probe_at));
    o.insert(
        "chipWindow".into(),
        match &chip {
            Some(w) => json!({
                "id": w.id,
                "label": w.label,
                "percent": su::window_percent(w, now),
                "observedAt": w.observed_at,
            }),
            None => Value::Null,
        },
    );
    v
}

/// An old row's windows read as the row's time and source, on every route.
fn materialise_served(row: HarnessUsage) -> HarnessUsage {
    materialise(&row)
}

/// One token's record, or `None` (no row or an unparseable row, or an API
/// token: no reading). Under air-gap the record has no windows and says so.
pub fn served(e: &Entry, now: DateTime<Utc>) -> Option<Value> {
    if e.is_api_key() {
        return None;
    }
    let row: HarnessUsage = serde_json::from_str(e.usage_json.as_deref()?).ok()?;
    Some(record_for(row, e, now))
}

fn record_for(mut row: HarnessUsage, e: &Entry, now: DateTime<Utc>) -> Value {
    if row.harness.is_empty() {
        row.harness = e.tool.clone();
    }
    if su::airgap_on() {
        row.windows.clear();
        row.status = su::STATUS_AIRGAP.to_string();
        row.error = None;
    }
    record_value(row, Some(&e.id), now)
}

/// The record for a token with no readable row: no numbers.
fn blank_record(e: &Entry, now: DateTime<Utc>) -> Value {
    let mut row = HarnessUsage { harness: e.tool.clone(), ..Default::default() };
    if su::airgap_on() {
        row.status = su::STATUS_AIRGAP.to_string();
    }
    record_value(row, Some(&e.id), now)
}

/// `GET /cli/usage/subscriptions`, the single-record answer: the record of
/// each tool's live token, in the tools' order. A tool with no live token
/// answers `Not signed in`. Air-gap stays the route's own check.
pub fn subscriptions_doc(now: DateTime<Utc>) -> Value {
    let db = k2_core::db::try_shared();
    let harnesses: Vec<Value> = su::PROBED_NAMES
        .iter()
        .map(|name| {
            let Some(tool) = Tool::parse(name) else { return not_signed_in(name, now) };
            let entry = db.as_ref().and_then(|db| {
                let conn = db.lock();
                let owner = k2_core::llm_accounts::live_owner(&conn, tool).ok().flatten()?;
                k2_core::llm_accounts::get(&conn, &owner).ok().flatten()
            });
            match entry {
                Some(e) => served(&e, now).unwrap_or_else(|| blank_record(&e, now)),
                None => not_signed_in(name, now),
            }
        })
        .collect();
    json!({ "harnesses": harnesses })
}

fn not_signed_in(name: &str, now: DateTime<Utc>) -> Value {
    record_value(
        HarnessUsage { harness: name.to_string(), status: su::STATUS_NOT_SIGNED_IN.to_string(), ..Default::default() },
        None,
        now,
    )
}

/// `GET /cli/usage/limits`, one login's record plus the row's own fields.
/// API tokens and tools with no limits API read as `Not supported`.
pub fn login_record(e: &Entry, now: DateTime<Utc>, supported: bool) -> Value {
    let mut rec = match served(e, now) {
        Some(v) if !e.is_api_key() && supported => v,
        _ => {
            let row = HarnessUsage { harness: e.tool.clone(), status: su::STATUS_NOT_SUPPORTED.to_string(), ..Default::default() };
            if e.is_api_key() || !supported {
                record_for(row, e, now)
            } else {
                blank_record(e, now)
            }
        }
    };
    let o = rec.as_object_mut().expect("record is an object");
    if o.get("plan").and_then(Value::as_str).unwrap_or("").is_empty() {
        o.insert("plan".into(), json!(normalize_plan(&e.tool, e.plan.as_deref().unwrap_or(""))));
    }
    rec
}

// ── Claude's statusLine: when a reading was measured ─────────────────

/// How far back from the end of a transcript the reader looks (64 KB
/// steps, up to 1 MB). The newest assistant line is near the end; a file
/// that hides it deeper drops the reading and the probe covers the token.
const TRANSCRIPT_STEP: u64 = 64 * 1024;
const TRANSCRIPT_MAX: u64 = 1024 * 1024;

/// `transcript_path` when it is a `.jsonl` under `<config_dir>/projects`,
/// canonicalised (U20): a `..` prefix doesn't pass a prefix check, and the
/// session supplies this path. Only its last assistant timestamp is read.
pub fn transcript_under_config(raw: &str, config_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(raw.trim());
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }
    let canon = dunce::canonicalize(path).ok()?;
    let projects = dunce::canonicalize(config_dir.join("projects")).ok()?;
    if canon.starts_with(&projects) {
        Some(canon)
    } else {
        None
    }
}

/// The timestamp of the last real `assistant` line in `path`, or `None`
/// (the reading is then dropped). Read backwards in 64 KB steps, no lock
/// held; each step's first line may be partial and is discarded (U20).
pub fn transcript_measured_at(path: &std::path::Path) -> Option<DateTime<Utc>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let limit = len.min(TRANSCRIPT_MAX);
    let mut read_to = len;
    let mut buf: Vec<u8> = Vec::new();
    while len - read_to < limit {
        let start = read_to.saturating_sub(TRANSCRIPT_STEP).max(len - limit);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut chunk = vec![0u8; (read_to - start) as usize];
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
        read_to = start;
        let body = if start == 0 {
            buf.as_slice()
        } else {
            // A partial first line (a real line can be over 1 MB).
            let nl = buf.iter().position(|b| *b == b'\n')?;
            &buf[nl + 1..]
        };
        for line in body.rsplit(|b| *b == b'\n') {
            if let Some(t) = assistant_time(line) {
                return Some(t);
            }
        }
        if start == 0 {
            return None;
        }
    }
    None
}

/// A line's timestamp when it is a real assistant message: not an API
/// error stub and not K2's or Claude's `<synthetic>` line (U20).
fn assistant_time(line: &[u8]) -> Option<DateTime<Utc>> {
    if line.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return None;
    }
    if v.get("isApiErrorMessage").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    if v.pointer("/message/model").and_then(|m| m.as_str()) == Some("<synthetic>") {
        return None;
    }
    let ts = v.get("timestamp").and_then(|t| t.as_str())?;
    parse_time(ts)
}

#[cfg(test)]
#[path = "usage_store_tests.rs"]
mod tests;
