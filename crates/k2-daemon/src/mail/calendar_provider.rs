//! The calendar provider seam for LINKED inboxes
//! (prd-app-mail-calendar-surfaces-v1 §3.1, extracted in MCS4).
//!
//! [`crate::mail::calendar::resolve_target`] routes a linked inbox here
//! only AFTER the access gate passed (CAL19 masking holds) and only when
//! [`linked_target`] says its provider can serve the verb; every other
//! linked case answers 409 `capability_missing` `{cap, reason, fix}`.
//!
//! Two providers implement [`CalendarProvider`]:
//! - Microsoft 365 over Microsoft Graph ([`crate::mail::calendar_graph`]);
//! - Google over the Google Calendar API ([`crate::mail::calendar_google`]).
//!
//! The hosted (Stalwart) path stays on its own code in `calendar.rs`
//! (MCS-A16: `CalendarApi` stays the Stalwart internals; S4's shared
//! calendars and peers are Stalwart-only).
//!
//! The verbs here are provider-neutral: request validation, the draft
//! guard (CAL20 — a draft write NEVER carries attendees, and an
//! update/delete of a stored event with anyone besides the mailbox, or one
//! someone else organised, answers 409 `invites_need_send_level` before
//! any write), time math (an update keeps the duration and the event's own
//! IANA zone), free/busy clipping and merging, and the `wait` long-poll.
//! A provider only fetches, writes and shapes.
//!
//! **Wire shape:** the hosted `shape_summary` / `shape_full` fields plus
//! `source` (`microsoft` | `google`) and `invite: {state: "none"}`. Event
//! ids stay `ev_<b64(address \n provider-id)>` (capped at 1 KiB, MCS-A23).

use std::collections::HashMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::access::{AccessInbox, Level};
use crate::mail::calendar::{
    self as cal, error_response, event_not_found, invites_need_send, ok_json, rfc3339_utc, usage,
    BusySpan, WriteBody,
};
use crate::mail::oauth::{self, OauthProvider};
use k2_core::db::schema::MailExternalInbox;

// ── The target ──────────────────────────────────────────────────────────

/// Which provider serves a linked inbox's calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkedBackend {
    Microsoft,
    Google,
}

impl LinkedBackend {
    /// The wire `source`.
    pub fn source(self) -> &'static str {
        match self {
            LinkedBackend::Microsoft => "microsoft",
            LinkedBackend::Google => "google",
        }
    }
}

/// A linked inbox a calendar verb may use (after the access gate and the
/// capability checks).
#[derive(Debug, Clone)]
pub struct LinkedCalInbox {
    pub address: String,
    pub row: MailExternalInbox,
    /// The caller's `k2 mail access` level on the inbox.
    pub level: Level,
    pub backend: LinkedBackend,
}

/// 409 `capability_missing` with `{cap, reason, fix}` (PRD §3.1). Only
/// ever answered after the access gate passed.
pub fn capability_missing(cap: &str, reason: &str, fix: &str, hint: &str) -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": {
                "code": "capability_missing",
                "hint": hint,
                "cap": cap,
                "reason": reason,
                "fix": fix,
            },
        })
        .to_string(),
    }
}

pub(crate) fn cap_name(need: Level) -> &'static str {
    match need {
        Level::Read => "calendar.read",
        Level::Draft => "calendar.write",
        Level::Send => "calendar.invite",
    }
}

/// The owner's fix for a link consented without calendars.
pub(crate) fn upgrade_fix(address: &str) -> String {
    format!("k2 mail link upgrade {address} --features calendar (owner)")
}

/// Route a LINKED inbox (already past the access gate) to its calendar
/// provider, or say why it can't: an app-password IMAP inbox has no
/// calendar API, the owner may have turned linked calendars off, and a
/// link consented for mail only needs `k2 mail link upgrade`.
pub fn linked_target(inbox: AccessInbox, need: Level) -> Result<LinkedCalInbox, CliResponse> {
    let cap = cap_name(need);
    let Some(row) = inbox.linked.clone() else {
        return Err(cal::engine_err("linked inbox row missing (unexpected)"));
    };
    let address = inbox.address.clone();
    let fields = crate::mail::external::read_oauth_fields(&row.id).map_err(cal::engine_err)?;
    let oauth = fields.auth_kind == crate::mail::external::AUTH_OAUTH;
    let provider = fields.provider.as_deref().unwrap_or("");
    let backend = if row.kind == "graph" || (oauth && provider == "microsoft") {
        LinkedBackend::Microsoft
    } else if oauth && provider == "gmail" {
        LinkedBackend::Google
    } else {
        return Err(capability_missing(
            cap,
            "backend_unsupported",
            "link the account with OAuth (k2 mail link add <address> --provider gmail|microsoft) for calendars",
            &format!(
                "'{address}' is linked over IMAP with an app password — that has no calendar API"
            ),
        ));
    };
    if !k2_core::app_settings::load().linked_calendars {
        return Err(capability_missing(
            cap,
            "owner_disabled",
            "k2 mail link calendars on (owner)",
            "the owner turned calendars on linked inboxes off for this server — ask your human",
        ));
    }
    let meta = crate::mail::external::read_oauth_meta(&row.id).unwrap_or_default();
    let granted = meta.granted_scopes.as_deref().unwrap_or("");
    let p = match backend {
        LinkedBackend::Microsoft => OauthProvider::Microsoft,
        LinkedBackend::Google => OauthProvider::Gmail,
    };
    if !oauth::features_granted(p, granted).calendar {
        let fix = upgrade_fix(&address);
        return Err(capability_missing(
            cap,
            "scope_not_granted",
            &fix,
            &format!(
                "'{address}' was linked without calendar access — the owner adds it with \
                 '{fix}' (or Settings → Email Link → Add calendar)"
            ),
        ));
    }
    Ok(LinkedCalInbox { address, row, level: inbox.your_level, backend })
}

// ── Errors ──────────────────────────────────────────────────────────────

/// A provider calendar failure. `NotFound` is masked by the caller;
/// `Usage` is the caller's to fix; `Scope` means the provider said the
/// grant doesn't cover the call; `Engine` carries only the provider's own
/// error text (never a token).
#[derive(Debug, PartialEq, Eq)]
pub enum CalErr {
    NotFound,
    Usage(String),
    Scope(String),
    Engine(String),
}

pub(crate) fn to_resp(g: &LinkedCalInbox, e: CalErr, token: Option<&str>) -> CliResponse {
    match e {
        CalErr::NotFound => match token {
            Some(t) => event_not_found(t),
            None => error_response("404 Not Found", "not_found", "no such calendar"),
        },
        CalErr::Usage(h) => usage(h),
        CalErr::Scope(h) => capability_missing(
            "calendar.read",
            "scope_not_granted",
            &upgrade_fix(&g.address),
            &format!(
                "{h} — the account's grant doesn't cover this; the owner re-consents with '{}'",
                upgrade_fix(&g.address)
            ),
        ),
        CalErr::Engine(h) => cal::engine_err(h),
    }
}

fn no_such_calendar() -> CliResponse {
    error_response(
        "404 Not Found",
        "not_found",
        "no such calendar on this inbox — see 'k2 calendar list'",
    )
}

// ── The trait ───────────────────────────────────────────────────────────

/// A stored event, read for `show` and for the draft guard.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    /// The event's own zone when it is an IANA name (kept on a time
    /// change); `None` = write UTC.
    pub zone: Option<String>,
    /// Attendees (and an outside organiser) besides the mailbox.
    pub others: Vec<String>,
    /// The event was all-day (a time change turns it into a timed event).
    pub all_day: bool,
    /// The `show` shape.
    pub full: Value,
}

/// A draft event to create. Never carries attendees.
#[derive(Debug, Clone)]
pub struct DraftEvent {
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// An IANA zone (validated), or `None` = UTC.
    pub zone: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    /// Display reminders, minutes before start (at most
    /// [`CalendarProvider::max_reminders`]).
    pub reminders: Vec<i64>,
}

/// A draft change. `None` = leave as is. Never carries attendees.
#[derive(Debug, Clone, Default)]
pub struct DraftPatch {
    pub title: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    /// `Some(empty)` turns reminders off.
    pub reminders: Option<Vec<i64>>,
    /// New `(start, end, zone)`; zone `None` = UTC.
    pub times: Option<(DateTime<Utc>, DateTime<Utc>, Option<String>)>,
}

/// One `wait` round: provider ids that changed or went away, and the next
/// opaque state token.
#[derive(Debug, Clone, Default)]
pub struct Changes {
    pub updated: Vec<String>,
    pub destroyed: Vec<String>,
    pub state: String,
}

/// What a `wait` state token can be wrong with.
#[derive(Debug, PartialEq, Eq)]
pub enum StateErr {
    /// Not a state this provider minted.
    Invalid,
    /// The provider can no longer list changes since it.
    Expired,
    Other(CalErr),
}

impl From<CalErr> for StateErr {
    fn from(e: CalErr) -> StateErr {
        StateErr::Other(e)
    }
}

/// One calendar provider for linked inboxes (Microsoft Graph, Google
/// Calendar). Every method takes the resolved inbox; provider ids are the
/// part of an `ev_…` token after the address.
pub trait CalendarProvider {
    /// Wire `source`.
    fn source(&self) -> &'static str;
    /// Who to name in messages ("Microsoft", "Google").
    fn label(&self) -> &'static str;
    /// The calendars the account can see, in the hosted `list` shape (with
    /// `rights.write` true only for the mailbox's own editable calendars at
    /// draft level or above).
    fn calendars(&self, g: &LinkedCalInbox) -> Result<Vec<Value>, CalErr>;
    /// Does a caller's `calendar` argument name this listed calendar id?
    fn same_calendar(&self, g: &LinkedCalInbox, listed: &str, asked: &str) -> bool {
        let _ = g;
        listed == asked
    }
    /// Occurrences overlapping `[start, end)`, summary shape; `truncated`
    /// when there were more than `limit`. `NotFound` = no such calendar.
    fn view(
        &self,
        g: &LinkedCalInbox,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
        calendar: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Value>, bool), CalErr>;
    /// One stored event (`None` = gone).
    fn event(&self, g: &LinkedCalInbox, id: &str, full: bool) -> Result<Option<StoredEvent>, CalErr>;
    /// Create a draft event; returns the provider id and the summary.
    fn insert(
        &self,
        g: &LinkedCalInbox,
        calendar: Option<&str>,
        ev: &DraftEvent,
    ) -> Result<(String, Value), CalErr>;
    /// Patch a draft event (the guard already passed); returns the summary.
    fn patch(&self, g: &LinkedCalInbox, id: &str, stored: &StoredEvent, p: &DraftPatch) -> Result<Value, CalErr>;
    /// Delete a draft event (the guard already passed).
    fn remove(&self, g: &LinkedCalInbox, id: &str) -> Result<(), CalErr>;
    /// The mailbox's busy spans (never titles) and where they came from;
    /// `truncated` when a fallback read hit its cap.
    fn busy(
        &self,
        g: &LinkedCalInbox,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
    ) -> Result<(Vec<BusySpan>, &'static str, bool), CalErr>;
    /// One change round from `state` (or a fresh baseline when `None`).
    fn changes(&self, g: &LinkedCalInbox, state: Option<&str>, now: DateTime<Utc>) -> Result<Changes, StateErr>;
    /// Seconds between `wait` polls (a remote API — not the hosted 2 s).
    fn wait_poll_secs(&self) -> u64;
    /// How many display reminders one event keeps.
    fn max_reminders(&self) -> usize;
    /// The usage hint when a write names more reminders than that.
    fn too_many_reminders(&self) -> String;
}

// ── Shared parsing ──────────────────────────────────────────────────────

/// A zone argument: `None`/empty → `None`; an IANA name chrono-tz knows →
/// `Some`; anything else is a usage error. `UTC` / `Etc/UTC` → `None`.
pub(crate) fn parse_zone(raw: Option<&str>) -> Result<Option<String>, CliResponse> {
    let Some(z) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    if matches!(z, "UTC" | "Etc/UTC") {
        return Ok(None);
    }
    if cal::valid_tz(z) && z.parse::<chrono_tz::Tz>().is_ok() {
        Ok(Some(z.to_string()))
    } else {
        Err(usage(format!("'{z}' is not an IANA time zone name (e.g. America/Los_Angeles)")))
    }
}

/// Display alerts → minutes before start, capped at the provider's count.
pub(crate) fn reminders(p: &dyn CalendarProvider, alerts: &Value) -> Result<Vec<i64>, CliResponse> {
    let items: Vec<&Value> = match alerts {
        Value::Array(a) => a.iter().collect(),
        Value::Object(m) => m.values().collect(),
        Value::Null => return Ok(Vec::new()),
        _ => return Err(usage("'alerts' must be a list")),
    };
    if items.len() > p.max_reminders() {
        return Err(usage(p.too_many_reminders()));
    }
    let mut out = Vec::with_capacity(items.len());
    for a in items {
        let off = a
            .get("offset")
            .and_then(Value::as_str)
            .or_else(|| a.pointer("/trigger/offset").and_then(Value::as_str))
            .ok_or_else(|| usage("each alert needs an 'offset' like -PT15M"))?;
        let secs = cal::parse_duration_secs(off.trim().trim_start_matches(['-', '+']))
            .ok_or_else(|| usage(format!("alert offset '{off}' is not an ISO 8601 duration")))?;
        out.push(secs / 60);
    }
    Ok(out)
}

/// The checks every draft write shares (CAL20 + lengths + the zone name).
fn draft_checks(g: &LinkedCalInbox, b: &WriteBody, what: &str) -> Result<(), CliResponse> {
    let others = cal::non_owner_participants(b.participants.as_ref(), &g.address);
    if !others.is_empty() {
        return Err(invites_need_send(&format!(
            "{what} names {} participant(s) besides {}",
            others.len(),
            g.address
        )));
    }
    if cal::has_email_alert(b.alerts.as_ref()) {
        return Err(cal::email_alert_refused());
    }
    for (v, f, m) in [
        (&b.title, "title", cal::MAX_TITLE),
        (&b.location, "location", cal::MAX_LOCATION),
        (&b.notes, "notes", cal::MAX_NOTES),
    ] {
        cal::check_len(v, f, m)?;
    }
    if let Some(z) = b.time_zone.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !cal::valid_tz(z) {
            return Err(usage(format!("'{z}' is not an IANA time zone name (e.g. America/Los_Angeles)")));
        }
    }
    Ok(())
}

// ── Verbs (provider-neutral) ────────────────────────────────────────────

pub fn list_with(p: &dyn CalendarProvider, g: &LinkedCalInbox) -> CliResponse {
    match p.calendars(g) {
        Ok(cals) => ok_json(json!({
            "ok": true,
            "address": g.address,
            "source": p.source(),
            "calendars": cals,
        })),
        Err(e) => to_resp(g, e, None),
    }
}

pub fn events_with(
    p: &dyn CalendarProvider,
    g: &LinkedCalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    let (start, end) = match cal::window(params, now) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let limit = match params.get("limit").map(|v| v.trim().parse::<usize>()) {
        None => cal::DEFAULT_EVENTS,
        Some(Ok(n)) if n >= 1 => n.min(cal::MAX_EVENTS),
        _ => return usage(format!("invalid 'limit' — a number from 1 to {}", cal::MAX_EVENTS)),
    };
    let calendar = crate::cli::opt_param(params, "calendar");
    match p.view(g, &start, &end, calendar.as_deref(), limit) {
        Ok((events, truncated)) => ok_json(json!({
            "ok": true,
            "address": g.address,
            "start": rfc3339_utc(&start),
            "end": rfc3339_utc(&end),
            "calendar": calendar,
            "events": events,
            "truncated": truncated,
            "source": p.source(),
        })),
        Err(CalErr::NotFound) => no_such_calendar(),
        Err(e) => to_resp(g, e, None),
    }
}

pub fn show_with(p: &dyn CalendarProvider, g: &LinkedCalInbox, id: &str, token: &str) -> CliResponse {
    match p.event(g, id, true) {
        Ok(Some(ev)) => ok_json(json!({ "ok": true, "address": g.address, "event": ev.full })),
        Ok(None) => event_not_found(token),
        Err(e) => to_resp(g, e, Some(token)),
    }
}

pub fn create_with(p: &dyn CalendarProvider, g: &LinkedCalInbox, b: &WriteBody) -> CliResponse {
    if let Err(r) = draft_checks(g, b, "this event") {
        return r;
    }
    let title = b.title.as_deref().map(str::trim).unwrap_or("");
    if title.is_empty() {
        return usage("create requires a 'title'");
    }
    let (Some(s), Some(e)) = (b.start.as_deref(), b.end.as_deref()) else {
        return usage("create requires 'start' and 'end' (RFC 3339)");
    };
    let start = match cal::parse_event_instant(s, "start") {
        Ok(v) => v,
        Err(e) => return usage(e),
    };
    let end = match cal::parse_event_instant(e, "end") {
        Ok(v) => v,
        Err(e) => return usage(e),
    };
    if end <= start {
        return usage("'end' must be after 'start'");
    }
    let zone = match parse_zone(b.time_zone.as_deref()) {
        Ok(z) => z,
        Err(r) => return r,
    };
    let reminders = match b.alerts.as_ref() {
        Some(a) => match reminders(p, a) {
            Ok(v) => v,
            Err(r) => return r,
        },
        None => Vec::new(),
    };
    let calendar = b.calendar.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(cid) = calendar {
        // Only the mailbox's own, editable calendars take a draft write.
        let cals = match p.calendars(g) {
            Ok(v) => v,
            Err(e) => return to_resp(g, e, None),
        };
        let Some(found) = cals
            .iter()
            .find(|x| x.get("id").and_then(Value::as_str).is_some_and(|id| p.same_calendar(g, id, cid)))
        else {
            return no_such_calendar();
        };
        if found["rights"]["write"] != json!(true) {
            return error_response(
                "403 Forbidden",
                "share_rights",
                "that calendar isn't one this inbox may write (shared to it, or read-only)",
            );
        }
    }
    let draft = DraftEvent {
        title: title.to_string(),
        start,
        end,
        zone,
        location: b.location.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        notes: b.notes.as_deref().filter(|s| !s.trim().is_empty()).map(str::to_string),
        reminders,
    };
    match p.insert(g, calendar, &draft) {
        Ok((id, summary)) => {
            if id.is_empty() {
                return cal::engine_err(format!("{} event create: no id in reply", p.label()));
            }
            ok_json(json!({
                "ok": true,
                "address": g.address,
                "id": cal::encode_event_id(&g.address, &id),
                "calendarId": calendar,
                "event": summary,
                "emailSent": false,
                "source": p.source(),
            }))
        }
        Err(CalErr::NotFound) => no_such_calendar(),
        Err(e) => to_resp(g, e, None),
    }
}

/// The STORED event's draft guard: present, and no attendee / outside
/// organiser.
fn stored_guard(
    p: &dyn CalendarProvider,
    g: &LinkedCalInbox,
    id: &str,
    token: &str,
    what: &str,
) -> Result<StoredEvent, CliResponse> {
    let stored = match p.event(g, id, false) {
        Ok(Some(ev)) => ev,
        Ok(None) => return Err(event_not_found(token)),
        Err(e) => return Err(to_resp(g, e, Some(token))),
    };
    if !stored.others.is_empty() {
        return Err(invites_need_send(&format!(
            "{what}: this event has {} participant(s) besides {}",
            stored.others.len(),
            g.address
        )));
    }
    Ok(stored)
}

pub fn update_with(
    p: &dyn CalendarProvider,
    g: &LinkedCalInbox,
    id: &str,
    token: &str,
    b: &WriteBody,
) -> CliResponse {
    if let Err(r) = draft_checks(g, b, "this update") {
        return r;
    }
    if b.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return usage("'title' can't be empty");
    }
    let parse = |raw: Option<&str>, f: &str| -> Result<Option<DateTime<Utc>>, CliResponse> {
        raw.map(|s| cal::parse_event_instant(s, f).map_err(usage)).transpose()
    };
    let new_start = match parse(b.start.as_deref(), "start") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let new_end = match parse(b.end.as_deref(), "end") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let zone_arg = b.time_zone.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let zone_given = match parse_zone(zone_arg) {
        Ok(z) => z,
        Err(r) => return r,
    };
    let reminders = match b.alerts.as_ref() {
        Some(a) => match reminders(p, a) {
            Ok(v) => Some(v),
            Err(r) => return r,
        },
        None => None,
    };
    let stored = match stored_guard(p, g, id, token, "update refused") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut patch = DraftPatch {
        title: b.title.as_deref().map(|t| t.trim().to_string()),
        location: b.location.as_deref().map(|l| l.trim().to_string()),
        notes: b.notes.clone(),
        reminders,
        times: None,
    };
    if new_start.is_some() || new_end.is_some() || zone_arg.is_some() {
        let (Some(old_start), Some(old_end)) = (stored.start, stored.end) else {
            return cal::engine_err("the stored event has no resolvable start/end");
        };
        let start = new_start.unwrap_or(old_start);
        let end = match (new_start, new_end) {
            (_, Some(e)) => e,
            (Some(s), None) => s + (old_end - old_start),
            (None, None) => old_end,
        };
        if end <= start {
            return usage("'end' must be after 'start'");
        }
        // Keep the event's own zone when the caller names none.
        let zone = if zone_arg.is_some() { zone_given } else { stored.zone.clone() };
        patch.times = Some((start, end, zone));
    }
    if patch.title.is_none()
        && patch.location.is_none()
        && patch.notes.is_none()
        && patch.reminders.is_none()
        && patch.times.is_none()
    {
        return usage("nothing to update — pass at least one of title/start/end/timeZone/location/notes");
    }
    match p.patch(g, id, &stored, &patch) {
        Ok(ev) => ok_json(json!({
            "ok": true,
            "address": g.address,
            "id": token,
            "event": ev,
            "emailSent": false,
            "source": p.source(),
        })),
        Err(e) => to_resp(g, e, Some(token)),
    }
}

pub fn delete_with(p: &dyn CalendarProvider, g: &LinkedCalInbox, id: &str, token: &str) -> CliResponse {
    if let Err(r) = stored_guard(p, g, id, token, "delete refused") {
        return r;
    }
    match p.remove(g, id) {
        Ok(()) => ok_json(json!({
            "ok": true,
            "address": g.address,
            "id": token,
            "deleted": true,
            "emailSent": false,
        })),
        Err(e) => to_resp(g, e, Some(token)),
    }
}

/// Busy windows of the mailbox (titles never leave), clipped to the
/// window and merged.
pub fn freebusy_with(
    p: &dyn CalendarProvider,
    g: &LinkedCalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    let (start, end) = match cal::window(params, now) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let (spans, source, truncated) = match p.busy(g, &start, &end) {
        Ok(v) => v,
        Err(e) => return to_resp(g, e, None),
    };
    let clipped: Vec<BusySpan> = spans
        .into_iter()
        .filter_map(|(s, e, k)| {
            let (s, e) = (s.max(start), e.min(end));
            (e > s).then_some((s, e, k))
        })
        .collect();
    let busy = cal::merge_busy(clipped);
    ok_json(json!({
        "ok": true,
        "address": g.address,
        "start": rfc3339_utc(&start),
        "end": rfc3339_utc(&end),
        "busy": busy.iter().map(|(s, e, k)| json!({
            "start": rfc3339_utc(s),
            "end": rfc3339_utc(e),
            "status": k.as_str(),
        })).collect::<Vec<_>>(),
        "source": source,
        "sharedCalendars": 0,
        "truncated": truncated,
    }))
}

/// Long-poll the provider's change feed until something changed or the
/// budget ran out. Without `since`, a baseline state is read first so only
/// changes AFTER the call count. Neither provider tells created from
/// updated reliably: every changed id is `updated`; removals `destroyed`.
pub fn wait_with(
    p: &dyn CalendarProvider,
    g: &LinkedCalInbox,
    since: Option<&str>,
    timeout_secs: u64,
    now: DateTime<Utc>,
    elapsed: &mut dyn FnMut() -> u64,
    sleep: &mut dyn FnMut(StdDuration),
) -> CliResponse {
    let state_err = |e: StateErr| match e {
        StateErr::Invalid => {
            usage("invalid --since-state for this inbox — use the 'state' a previous wait returned")
        }
        StateErr::Expired => error_response(
            "409 Conflict",
            "state_expired",
            &format!(
                "{} can no longer list changes since that state — re-read with 'k2 calendar \
                 events' and wait again without --since-state",
                p.label()
            ),
        ),
        StateErr::Other(e) => to_resp(g, e, None),
    };
    let mut state = match since.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => match p.changes(g, None, now) {
            Ok(c) => c.state,
            Err(e) => return state_err(e),
        },
    };
    loop {
        let round = match p.changes(g, Some(&state), now) {
            Ok(c) => c,
            Err(e) => return state_err(e),
        };
        state = round.state;
        if !(round.updated.is_empty() && round.destroyed.is_empty()) {
            let enc = |ids: &[String]| -> Vec<String> {
                ids.iter().map(|id| cal::encode_event_id(&g.address, id)).collect()
            };
            return ok_json(json!({
                "ok": true,
                "timedOut": false,
                "address": g.address,
                "state": state,
                "created": [],
                "updated": enc(&round.updated),
                "destroyed": enc(&round.destroyed),
                "source": p.source(),
            }));
        }
        let poll = p.wait_poll_secs();
        if elapsed() + poll > timeout_secs {
            return ok_json(json!({
                "ok": true,
                "timedOut": true,
                "address": g.address,
                "state": state,
                "source": p.source(),
            }));
        }
        sleep(StdDuration::from_secs(poll));
    }
}

// ── Production entry points (called by calendar.rs after routing) ───────

/// The real provider for a routed inbox.
fn real(g: &LinkedCalInbox) -> Box<dyn CalendarProvider> {
    match g.backend {
        LinkedBackend::Microsoft => Box::new(crate::mail::calendar_graph::GraphCalendar::real(&g.row)),
        LinkedBackend::Google => Box::new(crate::mail::calendar_google::GoogleCalendar::real(&g.row)),
    }
}

pub fn handle_list(g: &LinkedCalInbox) -> CliResponse {
    list_with(real(g).as_ref(), g)
}

pub fn handle_events(g: &LinkedCalInbox, params: &HashMap<String, String>, now: DateTime<Utc>) -> CliResponse {
    events_with(real(g).as_ref(), g, params, now)
}

pub fn handle_show(g: &LinkedCalInbox, id: &str, token: &str) -> CliResponse {
    show_with(real(g).as_ref(), g, id, token)
}

pub fn handle_create(g: &LinkedCalInbox, b: &WriteBody) -> CliResponse {
    create_with(real(g).as_ref(), g, b)
}

pub fn handle_update(g: &LinkedCalInbox, id: &str, token: &str, b: &WriteBody) -> CliResponse {
    update_with(real(g).as_ref(), g, id, token, b)
}

pub fn handle_delete(g: &LinkedCalInbox, id: &str, token: &str) -> CliResponse {
    delete_with(real(g).as_ref(), g, id, token)
}

pub fn handle_freebusy(g: &LinkedCalInbox, params: &HashMap<String, String>, now: DateTime<Utc>) -> CliResponse {
    freebusy_with(real(g).as_ref(), g, params, now)
}

pub fn handle_wait(
    g: &LinkedCalInbox,
    project_id: &str,
    params: &HashMap<String, String>,
    timeout_secs: u64,
) -> CliResponse {
    let Some(_slot) = crate::mail::routes_messages::WaitSlot::try_acquire(project_id) else {
        return error_response(
            "429 Too Many Requests",
            "rate_limited",
            "this workspace already has 4 wait calls open — loop ONE wait, don't fan out",
        );
    };
    let since = crate::cli::opt_param(params, "since_state");
    let begun = std::time::Instant::now();
    let mut elapsed = || begun.elapsed().as_secs();
    let mut sleep = |d: StdDuration| std::thread::sleep(d);
    wait_with(real(g).as_ref(), g, since.as_deref(), timeout_secs, Utc::now(), &mut elapsed, &mut sleep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_missing_carries_cap_reason_and_fix() {
        let r = capability_missing("calendar.write", "scope_not_granted", "k2 mail link upgrade a@b --features calendar (owner)", "hint");
        assert!(r.status.starts_with("409"));
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "capability_missing");
        assert_eq!(v["error"]["cap"], "calendar.write");
        assert_eq!(v["error"]["reason"], "scope_not_granted");
        assert_eq!(v["error"]["fix"], "k2 mail link upgrade a@b --features calendar (owner)");
    }

    fn zone(raw: Option<&str>) -> Option<String> {
        parse_zone(raw).unwrap_or_else(|r| panic!("{raw:?}: {} {}", r.status, r.body))
    }

    #[test]
    fn zones_are_iana_or_utc() {
        assert_eq!(zone(None), None);
        assert_eq!(zone(Some("  ")), None);
        assert_eq!(zone(Some("UTC")), None);
        assert_eq!(zone(Some("Etc/UTC")), None);
        assert_eq!(zone(Some("Europe/Berlin")), Some("Europe/Berlin".into()));
        let r = parse_zone(Some("Mars/Olympus")).expect_err("unknown");
        assert!(r.status.starts_with("400"), "{}", r.body);
        let r = parse_zone(Some("Pacific Standard Time")).expect_err("windows name");
        assert!(r.body.contains("IANA"), "{}", r.body);
    }
}
