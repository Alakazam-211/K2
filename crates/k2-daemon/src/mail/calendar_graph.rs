//! `k2 calendar` on a LINKED Microsoft 365 / Exchange Online inbox, over
//! Microsoft Graph (prd-app-mail-calendar-surfaces-v1 §3.1/§3.4, slice
//! MCS3) — one [`CalendarProvider`] behind the MCS4 seam
//! ([`crate::mail::calendar_provider`], whose provider-neutral verbs own
//! validation, the draft guard, time math and the wait loop; this module
//! fetches, writes and shapes).
//!
//! [`crate::mail::calendar::resolve_target`] routes here only AFTER the
//! access gate passed (CAL19 masking holds), and only for a Microsoft
//! link whose grant covers `Calendars.ReadWrite` while the owner's
//! `linkedCalendars` switch is on — every other linked case answers 409
//! `capability_missing` `{cap, reason, fix}` from [`linked_target`].
//!
//! | verb      | Graph                                                     |
//! |-----------|-----------------------------------------------------------|
//! | list      | `GET /me/calendars`                                       |
//! | events    | `GET /me[/calendars/{id}]/calendarView` (occurrences)     |
//! | show      | `GET /me/events/{id}`                                     |
//! | create    | `POST /me[/calendars/{id}]/events` — never attendees      |
//! | update    | `PATCH /me/events/{id}` — guard on the STORED event       |
//! | delete    | `DELETE /me/events/{id}` — guard on the STORED event      |
//! | freebusy  | `POST /me/calendar/getSchedule` (self), else calendarView |
//! | wait      | `GET /me/calendarView/delta` (`gd_…` state tokens)        |
//!
//! **Draft guard (CAL20, PRD §3.4):** Exchange sends meeting requests the
//! moment an attendee is on an event, so a draft write NEVER carries
//! attendees: a create/update naming anyone but the mailbox → 409
//! `invites_need_send_level`, and update/delete of a stored event with
//! attendees (or one someone else organized) → the same 409. Email alarms
//! are refused; Graph keeps one popup reminder per event.
//!
//! **Times:** every read asks `Prefer: outlook.timezone="UTC"` (and plain
//! text bodies); the event's own zone (`originalStartTimeZone`) is kept in
//! `timeZone`. Writes send the wall time in the caller's IANA zone (or the
//! stored zone when it is an IANA name, else UTC).
//!
//! **Wire shape:** the same `shape_summary` / `shape_full` fields the
//! hosted path returns, plus `source: "microsoft"` and `invite: {state:
//! "none"}`. Event ids stay `ev_<b64(address \n graph-id)>`.
//!
//! **Wait state:** `gd_<b64url(json)>` wraps ONLY the Graph `$deltatoken`
//! / `$skiptoken` value — never a URL — so a client can't point the
//! daemon's bearer token anywhere but `/me/calendarView/delta`.
//!
//! ⚠ LIVE-BOX: occurrence ids from `calendarView`, `getSchedule` on
//! personal (outlook.com) accounts, and Windows zone names in
//! `originalStartTimeZone` (kept as-is; an update without `timeZone` then
//! writes UTC) are the parts to check against a real tenant.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::access::Level;
use crate::mail::calendar::{self as cal, ok_json, rfc3339_utc, usage, BusyKind, BusySpan, WriteBody};
use crate::mail::calendar_provider::{
    self as linked, CalendarProvider, Changes, DraftEvent, DraftPatch, LinkedCalInbox, StateErr,
    StoredEvent,
};
use crate::mail::graph::{self, GraphHttp, GraphMethod, GraphResponse};
use k2_core::db::schema::MailExternalInbox;

/// The `$select` for summaries and the draft guard.
const EVENT_SELECT: &str = "id,subject,start,end,isAllDay,location,showAs,isCancelled,type,\
     recurrence,attendees,organizer,isOrganizer,originalStartTimeZone,originalEndTimeZone,\
     seriesMasterId";
/// The `$select` for `show`.
const EVENT_FULL_SELECT: &str = "id,subject,start,end,isAllDay,location,showAs,isCancelled,\
     type,recurrence,attendees,organizer,isOrganizer,originalStartTimeZone,originalEndTimeZone,\
     seriesMasterId,body,iCalUId,sensitivity,isReminderOn,reminderMinutesBeforeStart";
/// Graph throttling: bounded 429 retries, capped `Retry-After`.
const MAX_THROTTLE_RETRIES: u32 = 3;
const RETRY_AFTER_CAP_SECS: u64 = 60;
/// Delta polling cadence for `wait` (Graph is a remote API — not the
/// hosted 2 s loopback cadence).
const GRAPH_WAIT_POLL_SECS: u64 = 10;
/// The window a `wait` delta watches (Graph delta needs one).
const WAIT_PAST_DAYS: i64 = 1;
const WAIT_FUTURE_DAYS: i64 = 366;
/// Delta pages: the page size asked for, and the most pages one round
/// follows (200 × 100 = 20 000 changed occurrences).
const DELTA_PAGE_SIZE: u32 = 100;
const DELTA_MAX_PAGES: usize = 200;
/// `wait` state tokens over Graph.
const STATE_PREFIX: &str = "gd_";

// ── The target (MCS4: the provider seam owns it) ────────────────────────

/// A linked Microsoft 365 inbox a calendar verb may use — the
/// provider-neutral [`LinkedCalInbox`] (MCS4 moved routing, the
/// capability errors and the verbs into [`crate::mail::calendar_provider`]).
pub type GraphCalInbox = LinkedCalInbox;
pub use crate::mail::calendar_provider::{capability_missing, linked_target, CalErr};
#[cfg(test)]
use crate::mail::calendar_provider::cap_name;

// ── The Graph client (injectable) ───────────────────────────────────────

/// Graph calendar requests for one inbox, with bounded 429 back-off and
/// the UTC / plain-text `Prefer` headers on every call.
pub struct GraphCalendar {
    http: Arc<dyn GraphHttp>,
    sleep: Box<dyn Fn(StdDuration) + Send + Sync>,
}

impl GraphCalendar {
    pub fn real(row: &MailExternalInbox) -> GraphCalendar {
        GraphCalendar {
            http: Arc::new(graph::RealGraphHttp::new(row.id.clone())),
            sleep: Box::new(std::thread::sleep),
        }
    }

    #[cfg(test)]
    pub fn with_http(http: Arc<dyn GraphHttp>) -> GraphCalendar {
        GraphCalendar { http, sleep: Box::new(|_| {}) }
    }

    fn url(path: &str) -> String {
        format!("{}{path}", crate::mail::oauth::graph_base())
    }

    fn req(&self, m: GraphMethod, url: &str, body: Option<&Value>) -> Result<GraphResponse, CalErr> {
        self.req_with(m, url, body, None)
    }

    /// [`Self::req`] plus one more `Prefer` value (delta page size).
    fn req_with(
        &self,
        m: GraphMethod,
        url: &str,
        body: Option<&Value>,
        extra_prefer: Option<String>,
    ) -> Result<GraphResponse, CalErr> {
        let mut headers = vec![
            ("Prefer", "outlook.timezone=\"UTC\"".to_string()),
            ("Prefer", "outlook.body-content-type=\"text\"".to_string()),
        ];
        if let Some(p) = extra_prefer {
            headers.push(("Prefer", p));
        }
        let mut attempt = 0;
        loop {
            let resp = self.http.send_with(m, url, body, &headers).map_err(CalErr::Engine)?;
            if resp.status == 429 && attempt < MAX_THROTTLE_RETRIES {
                let secs = resp.retry_after.unwrap_or(1).min(RETRY_AFTER_CAP_SECS);
                (self.sleep)(StdDuration::from_secs(secs));
                attempt += 1;
                continue;
            }
            return Ok(resp);
        }
    }

    /// 2xx JSON (`Null` for an empty body); 404 → `NotFound`; 400 →
    /// `Usage` (Graph rejected a value the caller sent); anything else
    /// → `Engine` with Graph's own message.
    fn json(&self, resp: GraphResponse, what: &str) -> Result<Value, CalErr> {
        match resp.status {
            404 => Err(CalErr::NotFound),
            200..=299 if resp.body.trim().is_empty() => Ok(Value::Null),
            200..=299 => serde_json::from_str(&resp.body)
                .map_err(|_| CalErr::Engine(format!("graph {what}: unparseable JSON reply"))),
            400 => Err(CalErr::Usage(format!(
                "Microsoft refused the {what}: {}",
                graph::graph_error_excerpt(&resp.body)
            ))),
            s => Err(CalErr::Engine(format!(
                "graph {what}: HTTP {s}: {}",
                graph::graph_error_excerpt(&resp.body)
            ))),
        }
    }

    pub fn calendar_list(&self) -> Result<Vec<Value>, CalErr> {
        let url = graph::with_query(
            &Self::url("/me/calendars"),
            &[
                ("$top", "100".to_string()),
                ("$select", "id,name,color,isDefaultCalendar,canEdit,owner".to_string()),
            ],
        );
        let v = self.json(self.req(GraphMethod::Get, &url, None)?, "calendar list")?;
        Ok(v.get("value").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    /// Occurrences overlapping `[start, end)` (expanded by Graph), first
    /// page only; `truncated` when there were more than `limit`.
    pub fn calendar_view(
        &self,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
        calendar: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Value>, bool), CalErr> {
        let path = match calendar {
            Some(c) => format!("/me/calendars/{}/calendarView", graph::path_encode(c)),
            None => "/me/calendarView".to_string(),
        };
        let url = graph::with_query(
            &Self::url(&path),
            &[
                ("startDateTime", rfc3339_utc(start)),
                ("endDateTime", rfc3339_utc(end)),
                ("$top", (limit + 1).to_string()),
                ("$orderby", "start/dateTime".to_string()),
                ("$select", EVENT_SELECT.to_string()),
            ],
        );
        let v = self.json(self.req(GraphMethod::Get, &url, None)?, "calendar view")?;
        let mut list = v.get("value").and_then(Value::as_array).cloned().unwrap_or_default();
        let more = v.get("@odata.nextLink").is_some();
        let truncated = more || list.len() > limit;
        list.truncate(limit);
        Ok((list, truncated))
    }

    pub fn event_raw(&self, id: &str, full: bool) -> Result<Option<Value>, CalErr> {
        let url = graph::with_query(
            &Self::url(&format!("/me/events/{}", graph::path_encode(id))),
            &[("$select", if full { EVENT_FULL_SELECT } else { EVENT_SELECT }.to_string())],
        );
        match self.json(self.req(GraphMethod::Get, &url, None)?, "event read") {
            Ok(v) => Ok(Some(v)),
            Err(CalErr::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn create_raw(&self, calendar: Option<&str>, ev: &Value) -> Result<Value, CalErr> {
        let path = match calendar {
            Some(c) => format!("/me/calendars/{}/events", graph::path_encode(c)),
            None => "/me/events".to_string(),
        };
        self.json(self.req(GraphMethod::Post, &Self::url(&path), Some(ev))?, "event create")
    }

    pub fn update_raw(&self, id: &str, patch: &Value) -> Result<Value, CalErr> {
        let url = Self::url(&format!("/me/events/{}", graph::path_encode(id)));
        self.json(self.req(GraphMethod::Patch, &url, Some(patch))?, "event update")
    }

    pub fn delete_raw(&self, id: &str) -> Result<(), CalErr> {
        let url = Self::url(&format!("/me/events/{}", graph::path_encode(id)));
        self.json(self.req(GraphMethod::Delete, &url, None)?, "event delete").map(|_| ())
    }

    /// The mailbox's own schedule (busy items only — subjects are dropped
    /// here and never leave this function).
    pub fn schedule(
        &self,
        address: &str,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
    ) -> Result<Vec<BusySpan>, CalErr> {
        let body = json!({
            "schedules": [address],
            "startTime": { "dateTime": naive_utc(start), "timeZone": "UTC" },
            "endTime": { "dateTime": naive_utc(end), "timeZone": "UTC" },
            "availabilityViewInterval": 30,
        });
        let url = Self::url("/me/calendar/getSchedule");
        let v = self.json(self.req(GraphMethod::Post, &url, Some(&body))?, "getSchedule")?;
        let Some(first) = v.get("value").and_then(Value::as_array).and_then(|a| a.first()) else {
            return Err(CalErr::Engine("getSchedule: empty reply".into()));
        };
        if first.get("error").is_some_and(|e| !e.is_null()) {
            return Err(CalErr::Engine("getSchedule: the schedule is not available".into()));
        }
        let mut out = Vec::new();
        for item in first.get("scheduleItems").and_then(Value::as_array).into_iter().flatten() {
            let kind = match item.get("status").and_then(Value::as_str).unwrap_or("busy") {
                "free" => continue,
                "tentative" => BusyKind::Tentative,
                "oof" => BusyKind::Unavailable,
                _ => BusyKind::Busy,
            };
            if let (Some(s), Some(e)) = (graph_instant(item.get("start")), graph_instant(item.get("end"))) {
                out.push((s, e, kind));
            }
        }
        Ok(out)
    }

    /// One delta round: from `token` (or a fresh window), follow
    /// `$skiptoken` pages until a `$deltatoken`. Returns the changed
    /// items and the next state. Never follows a URL — only the token.
    pub fn delta(&self, token: Option<&DeltaToken>, now: DateTime<Utc>) -> Result<(Vec<Value>, DeltaToken), CalErr> {
        let base = Self::url("/me/calendarView/delta");
        let mut url = match token {
            Some(t) => graph::with_query(&base, &[(t.kind.param(), t.value.clone())]),
            None => graph::with_query(
                &base,
                &[
                    ("startDateTime", rfc3339_utc(&(now - Duration::days(WAIT_PAST_DAYS)))),
                    ("endDateTime", rfc3339_utc(&(now + Duration::days(WAIT_FUTURE_DAYS)))),
                ],
            ),
        };
        let mut items = Vec::new();
        for _ in 0..DELTA_MAX_PAGES {
            let resp = self.req_with(
                GraphMethod::Get,
                &url,
                None,
                Some(format!("odata.maxpagesize={DELTA_PAGE_SIZE}")),
            )?;
            if resp.status == 410 {
                return Err(CalErr::Usage("state_expired".into()));
            }
            let v = self.json(resp, "calendar delta")?;
            items.extend(v.get("value").and_then(Value::as_array).cloned().unwrap_or_default());
            if let Some(next) = v.get("@odata.nextLink").and_then(Value::as_str) {
                let t = DeltaToken::from_link(next)
                    .ok_or_else(|| CalErr::Engine("calendar delta: unreadable nextLink".into()))?;
                url = graph::with_query(&base, &[(t.kind.param(), t.value)]);
                continue;
            }
            if let Some(dl) = v.get("@odata.deltaLink").and_then(Value::as_str) {
                let t = DeltaToken::from_link(dl)
                    .ok_or_else(|| CalErr::Engine("calendar delta: unreadable deltaLink".into()))?;
                return Ok((items, t));
            }
            return Err(CalErr::Engine("calendar delta: no deltaLink in the reply".into()));
        }
        Err(CalErr::Engine("calendar delta: too many pages".into()))
    }
}

/// Which delta token a state carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    Delta,
    Skip,
}

impl DeltaKind {
    fn param(self) -> &'static str {
        match self {
            DeltaKind::Delta => "$deltatoken",
            DeltaKind::Skip => "$skiptoken",
        }
    }
}

/// The token value of a Graph delta/next link (the only part kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaToken {
    pub kind: DeltaKind,
    pub value: String,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl DeltaToken {
    /// Pull `$deltatoken` / `$skiptoken` out of a Graph link.
    pub fn from_link(link: &str) -> Option<DeltaToken> {
        let q = link.split_once('?')?.1;
        for pair in q.split('&') {
            let Some((k, v)) = pair.split_once('=') else { continue };
            let k = pct_decode(k);
            let kind = match k.as_str() {
                "$deltatoken" | "deltatoken" => DeltaKind::Delta,
                "$skiptoken" | "skiptoken" => DeltaKind::Skip,
                _ => continue,
            };
            let value = pct_decode(v);
            if value.is_empty() {
                return None;
            }
            return Some(DeltaToken { kind, value });
        }
        None
    }

    pub fn encode(&self) -> String {
        let k = if self.kind == DeltaKind::Delta { "d" } else { "s" };
        format!("{STATE_PREFIX}{}", B64URL.encode(json!({ "k": k, "t": self.value }).to_string()))
    }

    pub fn decode(state: &str) -> Option<DeltaToken> {
        let raw = state.trim().strip_prefix(STATE_PREFIX)?;
        if raw.len() > 8192 {
            return None;
        }
        let v: Value = serde_json::from_slice(&B64URL.decode(raw).ok()?).ok()?;
        let kind = match v.get("k")?.as_str()? {
            "d" => DeltaKind::Delta,
            "s" => DeltaKind::Skip,
            _ => return None,
        };
        let value = v.get("t")?.as_str()?.to_string();
        (!value.is_empty()).then_some(DeltaToken { kind, value })
    }
}

// ── Shaping (the hosted wire shape + `source`) ──────────────────────────

fn naive_utc(dt: &DateTime<Utc>) -> String {
    dt.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// A Graph `dateTimeTimeZone` → UTC. Reads ask for UTC; a non-UTC zone
/// that chrono-tz knows is honoured, anything else is read as UTC.
fn graph_instant(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let v = v?;
    let raw = v.get("dateTime")?.as_str()?;
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    let tz = v.get("timeZone").and_then(Value::as_str).unwrap_or("UTC");
    if matches!(tz, "UTC" | "Etc/UTC" | "tzone://Microsoft/Utc") {
        return Some(naive.and_utc());
    }
    match tz.parse::<chrono_tz::Tz>() {
        Ok(z) => naive.and_local_timezone(z).earliest().map(|d| d.with_timezone(&Utc)),
        Err(_) => Some(naive.and_utc()),
    }
}

fn local_in(dt: &DateTime<Utc>, zone: Option<&str>) -> String {
    match zone.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        Some(z) => dt.with_timezone(&z).naive_local().format("%Y-%m-%dT%H:%M:%S").to_string(),
        None => naive_utc(dt),
    }
}

fn email_of(v: Option<&Value>) -> Option<String> {
    v?.pointer("/emailAddress/address")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

/// Attendees (and an organizer) other than the mailbox — the STORED
/// event's draft guard.
pub fn graph_others(ev: &Value, owner: &str) -> Vec<String> {
    let owner = owner.trim().to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for a in ev.get("attendees").and_then(Value::as_array).into_iter().flatten() {
        match email_of(Some(a)) {
            Some(e) if e == owner => {}
            Some(e) => out.push(e),
            None => out.push("(attendee without an address)".into()),
        }
    }
    if ev.get("isOrganizer").and_then(Value::as_bool) == Some(false) {
        if let Some(org) = email_of(ev.get("organizer")) {
            if org != owner && !out.contains(&org) {
                out.push(org);
            }
        }
    }
    out
}

/// One Graph event, summary shape.
pub fn shape_summary(ev: &Value, address: &str, calendar: Option<&str>) -> Value {
    let id = ev.get("id").and_then(Value::as_str).unwrap_or("");
    let start = graph_instant(ev.get("start"));
    let end = graph_instant(ev.get("end"));
    let zone = ev.get("originalStartTimeZone").and_then(Value::as_str);
    let duration = match (start, end) {
        (Some(s), Some(e)) => Some(cal::format_duration((e - s).num_seconds())),
        _ => None,
    };
    let free_busy = match ev.get("showAs").and_then(Value::as_str).unwrap_or("busy") {
        "free" => "free",
        "tentative" => "tentative",
        _ => "busy",
    };
    let recurring = ev.get("recurrence").is_some_and(|r| !r.is_null())
        || matches!(ev.get("type").and_then(Value::as_str), Some("occurrence" | "exception" | "seriesMaster"));
    json!({
        "id": cal::encode_event_id(address, id),
        "calendarIds": calendar.map(|c| vec![c.to_string()]).unwrap_or_default(),
        "title": ev.get("subject").and_then(Value::as_str).unwrap_or(""),
        "start": start.map(|d| rfc3339_utc(&d)),
        "end": end.map(|d| rfc3339_utc(&d)),
        "localStart": start.map(|d| local_in(&d, zone)),
        "duration": duration,
        "timeZone": zone,
        "allDay": ev.get("isAllDay").and_then(Value::as_bool).unwrap_or(false),
        "location": ev.pointer("/location/displayName").and_then(Value::as_str).filter(|s| !s.is_empty()),
        "status": if ev.get("isCancelled").and_then(Value::as_bool) == Some(true) { "cancelled" } else { "confirmed" },
        "freeBusy": free_busy,
        "recurring": recurring,
        "participants": ev.get("attendees").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
        "source": "microsoft",
        "invite": { "state": "none" },
    })
}

/// One Graph event, full shape (`show`).
pub fn shape_full(ev: &Value, address: &str) -> Value {
    let mut out = shape_summary(ev, address, None);
    out["address"] = json!(address);
    out["uid"] = ev.get("iCalUId").cloned().unwrap_or(Value::Null);
    out["description"] = ev.pointer("/body/content").cloned().unwrap_or(Value::Null);
    out["privacy"] = ev.get("sensitivity").cloned().unwrap_or(Value::Null);
    out["organizer"] = email_of(ev.get("organizer")).map(Value::from).unwrap_or(Value::Null);
    out["participantList"] = Value::Array(
        ev.get("attendees")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|a| {
                json!({
                    "name": a.pointer("/emailAddress/name").and_then(Value::as_str),
                    "email": email_of(Some(a)),
                    "roles": a.get("type").cloned().unwrap_or(Value::Null),
                    "status": a.pointer("/status/response").and_then(Value::as_str),
                })
            })
            .collect(),
    );
    out["alerts"] = if ev.get("isReminderOn").and_then(Value::as_bool) == Some(true) {
        let m = ev.get("reminderMinutesBeforeStart").and_then(Value::as_i64).unwrap_or(15);
        json!([{
            "action": "display",
            "trigger": { "@type": "OffsetTrigger", "offset": format!("-PT{m}M"), "relativeTo": "start" },
        }])
    } else {
        json!([])
    };
    out
}

fn shape_calendar(c: &Value, address: &str, level: Level) -> Value {
    let owner = email_of(c.get("owner").map(|o| json!({ "emailAddress": o })).as_ref())
        .unwrap_or_else(|| address.to_ascii_lowercase());
    let own = owner.eq_ignore_ascii_case(address);
    let can_edit = c.get("canEdit").and_then(Value::as_bool).unwrap_or(false);
    let write = own && can_edit && level >= Level::Draft;
    json!({
        "id": c.get("id").and_then(Value::as_str).unwrap_or(""),
        "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
        "description": Value::Null,
        "color": c.get("color").cloned().unwrap_or(Value::Null),
        "isDefault": c.get("isDefaultCalendar").and_then(Value::as_bool).unwrap_or(false),
        "timeZone": Value::Null,
        "owner": owner,
        "shared": !own,
        "rights": { "freeBusy": true, "read": true, "write": write, "delete": write },
        "source": "microsoft",
    })
}

// ── Write bodies (draft level: never attendees) ─────────────────────────

/// A Graph `dateTimeTimeZone` for `dt`: the wall time in `zone` (IANA)
/// or UTC.
fn graph_time(dt: &DateTime<Utc>, zone: Option<&str>) -> Result<Value, CalErr> {
    match zone {
        None | Some("UTC") | Some("Etc/UTC") => Ok(json!({ "dateTime": naive_utc(dt), "timeZone": "UTC" })),
        Some(z) => match z.parse::<chrono_tz::Tz>() {
            Ok(tz) => Ok(json!({
                "dateTime": dt.with_timezone(&tz).naive_local().format("%Y-%m-%dT%H:%M:%S").to_string(),
                "timeZone": z,
            })),
            Err(_) => Err(CalErr::Usage(format!(
                "'{z}' is not an IANA time zone name (e.g. America/Los_Angeles)"
            ))),
        },
    }
}

/// A Graph event → the provider-neutral stored event (draft guard + show).
fn stored_of(ev: &Value, address: &str, full: bool) -> StoredEvent {
    StoredEvent {
        start: graph_instant(ev.get("start")),
        end: graph_instant(ev.get("end")),
        // Keep the event's own zone only when it is an IANA name (a
        // Windows zone name writes UTC).
        zone: ev
            .get("originalStartTimeZone")
            .and_then(Value::as_str)
            .filter(|z| z.parse::<chrono_tz::Tz>().is_ok())
            .map(str::to_string),
        others: graph_others(ev, address),
        all_day: ev.get("isAllDay").and_then(Value::as_bool).unwrap_or(false),
        full: if full { shape_full(ev, address) } else { Value::Null },
    }
}

// ── The provider (MCS4 seam) ────────────────────────────────────────────

impl CalendarProvider for GraphCalendar {
    fn source(&self) -> &'static str {
        "microsoft"
    }

    fn label(&self) -> &'static str {
        "Microsoft"
    }

    fn calendars(&self, g: &LinkedCalInbox) -> Result<Vec<Value>, CalErr> {
        Ok(self.calendar_list()?.iter().map(|c| shape_calendar(c, &g.address, g.level)).collect())
    }

    fn view(
        &self,
        g: &LinkedCalInbox,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
        calendar: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Value>, bool), CalErr> {
        let (events, truncated) = self.calendar_view(start, end, calendar, limit)?;
        Ok((events.iter().map(|e| shape_summary(e, &g.address, calendar)).collect(), truncated))
    }

    fn event(&self, g: &LinkedCalInbox, id: &str, full: bool) -> Result<Option<StoredEvent>, CalErr> {
        Ok(self.event_raw(id, full)?.map(|ev| stored_of(&ev, &g.address, full)))
    }

    fn insert(
        &self,
        g: &LinkedCalInbox,
        calendar: Option<&str>,
        d: &DraftEvent,
    ) -> Result<(String, Value), CalErr> {
        let zone = d.zone.as_deref();
        let mut ev = json!({
            "subject": d.title,
            "start": graph_time(&d.start, zone)?,
            "end": graph_time(&d.end, zone)?,
            // CAL20: a draft event NEVER carries attendees (Exchange would
            // send meeting requests the moment one is on it).
            "attendees": [],
            "isReminderOn": false,
        });
        if let Some(l) = &d.location {
            ev["location"] = json!({ "displayName": l });
        }
        if let Some(n) = &d.notes {
            ev["body"] = json!({ "contentType": "text", "content": n });
        }
        if let Some(m) = d.reminders.first() {
            ev["isReminderOn"] = json!(true);
            ev["reminderMinutesBeforeStart"] = json!(m);
        }
        let created = self.create_raw(calendar, &ev)?;
        let id = created.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((id, shape_summary(&created, &g.address, calendar)))
    }

    fn patch(&self, g: &LinkedCalInbox, id: &str, _stored: &StoredEvent, p: &DraftPatch) -> Result<Value, CalErr> {
        let mut patch = serde_json::Map::new();
        if let Some(t) = &p.title {
            patch.insert("subject".into(), json!(t));
        }
        if let Some(l) = &p.location {
            patch.insert("location".into(), json!({ "displayName": l }));
        }
        if let Some(n) = &p.notes {
            patch.insert("body".into(), json!({ "contentType": "text", "content": n }));
        }
        match p.reminders.as_deref() {
            Some([m, ..]) => {
                patch.insert("isReminderOn".into(), json!(true));
                patch.insert("reminderMinutesBeforeStart".into(), json!(m));
            }
            Some([]) => {
                patch.insert("isReminderOn".into(), json!(false));
            }
            None => {}
        }
        if let Some((start, end, zone)) = &p.times {
            patch.insert("start".into(), graph_time(start, zone.as_deref())?);
            patch.insert("end".into(), graph_time(end, zone.as_deref())?);
        }
        let ev = self.update_raw(id, &Value::Object(patch))?;
        Ok(shape_summary(&ev, &g.address, None))
    }

    fn remove(&self, _g: &LinkedCalInbox, id: &str) -> Result<(), CalErr> {
        self.delete_raw(id)
    }

    /// `getSchedule` first; when it is refused (some personal accounts),
    /// the same answer from the expanded calendar view.
    fn busy(
        &self,
        g: &LinkedCalInbox,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
    ) -> Result<(Vec<BusySpan>, &'static str, bool), CalErr> {
        if let Ok(s) = self.schedule(&g.address, start, end) {
            return Ok((s, "getSchedule", false));
        }
        let (events, truncated) = self.calendar_view(start, end, None, cal::MAX_EVENTS)?;
        let spans = events
            .iter()
            .filter(|e| e.get("isCancelled").and_then(Value::as_bool) != Some(true))
            .filter_map(|e| {
                let kind = match e.get("showAs").and_then(Value::as_str).unwrap_or("busy") {
                    "free" => return None,
                    "tentative" => BusyKind::Tentative,
                    "oof" => BusyKind::Unavailable,
                    _ => BusyKind::Busy,
                };
                Some((graph_instant(e.get("start"))?, graph_instant(e.get("end"))?, kind))
            })
            .collect();
        Ok((spans, "events", truncated))
    }

    /// `calendarView/delta`: the state is a `gd_…` token holding only the
    /// Graph `$deltatoken` / `$skiptoken`.
    fn changes(&self, _g: &LinkedCalInbox, state: Option<&str>, now: DateTime<Utc>) -> Result<Changes, StateErr> {
        let token = match state {
            Some(s) => Some(DeltaToken::decode(s).ok_or(StateErr::Invalid)?),
            None => None,
        };
        let (items, next) = match self.delta(token.as_ref(), now) {
            Ok(v) => v,
            Err(CalErr::Usage(m)) if m == "state_expired" => return Err(StateErr::Expired),
            Err(e) => return Err(StateErr::Other(e)),
        };
        let mut out = Changes { state: next.encode(), ..Changes::default() };
        if token.is_none() {
            return Ok(out);
        }
        for it in &items {
            let Some(id) = it.get("id").and_then(Value::as_str) else { continue };
            if it.get("@removed").is_some() {
                out.destroyed.push(id.to_string());
            } else {
                out.updated.push(id.to_string());
            }
        }
        Ok(out)
    }

    fn wait_poll_secs(&self) -> u64 {
        GRAPH_WAIT_POLL_SECS
    }

    fn max_reminders(&self) -> usize {
        1
    }

    fn too_many_reminders(&self) -> String {
        "Microsoft 365 keeps one reminder per event — pass a single alert".to_string()
    }
}

// ── Verbs (the provider-neutral verbs over Graph) ───────────────────────

pub fn list_with(c: &GraphCalendar, g: &GraphCalInbox) -> CliResponse {
    linked::list_with(c, g)
}

pub fn events_with(
    c: &GraphCalendar,
    g: &GraphCalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    linked::events_with(c, g, params, now)
}

pub fn show_with(c: &GraphCalendar, g: &GraphCalInbox, id: &str, token: &str) -> CliResponse {
    linked::show_with(c, g, id, token)
}

pub fn create_with(c: &GraphCalendar, g: &GraphCalInbox, b: &WriteBody) -> CliResponse {
    linked::create_with(c, g, b)
}

pub fn update_with(c: &GraphCalendar, g: &GraphCalInbox, id: &str, token: &str, b: &WriteBody) -> CliResponse {
    linked::update_with(c, g, id, token, b)
}

pub fn delete_with(c: &GraphCalendar, g: &GraphCalInbox, id: &str, token: &str) -> CliResponse {
    linked::delete_with(c, g, id, token)
}

pub fn freebusy_with(
    c: &GraphCalendar,
    g: &GraphCalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    linked::freebusy_with(c, g, params, now)
}

pub fn wait_with(
    c: &GraphCalendar,
    g: &GraphCalInbox,
    since: Option<&str>,
    timeout_secs: u64,
    now: DateTime<Utc>,
    elapsed: &mut dyn FnMut() -> u64,
    sleep: &mut dyn FnMut(StdDuration),
) -> CliResponse {
    linked::wait_with(c, g, since, timeout_secs, now, elapsed, sleep)
}

// ── The owner's switch: GET/POST /cli/mail/link/calendars ───────────────

/// The `linkedCalendars` switch as JSON.
fn linked_calendars_payload(on: bool) -> Value {
    json!({
        "ok": true,
        "linkedCalendars": if on { "on" } else { "off" },
        "enabled": on,
        "scope": "every linked Google / Microsoft inbox on this server",
    })
}

/// `GET /cli/mail/link/calendars` — is the per-box switch for calendars on
/// LINKED inboxes on? Owner/admin only (dispatcher: `is_owner_level_mutation`;
/// agent passports never reach `/cli/mail/link/`).
pub fn handle_linked_calendars_get(_params: &HashMap<String, String>) -> CliResponse {
    ok_json(linked_calendars_payload(k2_core::app_settings::load().linked_calendars))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkedCalendarsBody {
    enabled: bool,
}

/// `POST /cli/mail/link/calendars` `{enabled: true|false}` — the owner turns
/// calendars on linked inboxes on or off for the whole server (`k2 mail link
/// calendars on|off`). Off answers `capability_missing` / `owner_disabled`
/// on every linked calendar verb; hosted calendars keep their own switch.
pub fn handle_linked_calendars_post(body: &[u8]) -> CliResponse {
    let b: LinkedCalendarsBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return usage(format!(
                "body must be {{\"enabled\": true|false}} ({e}) — k2 mail link calendars on|off"
            ))
        }
    };
    let was = k2_core::app_settings::load().linked_calendars;
    match k2_core::app_settings::update(json!({ "linkedCalendars": b.enabled })) {
        Ok(s) => {
            let mut out = linked_calendars_payload(s.linked_calendars);
            out["changed"] = json!(was != s.linked_calendars);
            ok_json(out)
        }
        Err(e) => cal::engine_err(format!("could not save the setting: {e}")),
    }
}

#[cfg(test)]
mod tests {
    //! Graph calendars against a scripted mock: request shapes, the draft
    //! guard (no attendees ever; stored attendees / outside organizer →
    //! 409 before any write), delta state tokens, the getSchedule
    //! fallback, and time-zone handling. No network, no token.
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const ME: &str = "rosson@contoso.com";
    const GRAPH: &str = "https://graph.microsoft.com/v1.0";

    #[derive(Debug, Clone)]
    struct Seen {
        method: GraphMethod,
        url: String,
        body: Option<Value>,
        headers: Vec<(String, String)>,
    }

    impl Seen {
        /// The URL path (no query).
        fn path(&self) -> &str {
            self.url.split_once('?').map(|(p, _)| p).unwrap_or(&self.url)
        }
        /// The decoded query pairs.
        fn query(&self) -> HashMap<String, String> {
            let Some((_, q)) = self.url.split_once('?') else { return HashMap::new() };
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (pct_decode(k), pct_decode(v)))
                .collect()
        }
        fn prefers(&self) -> Vec<String> {
            self.headers.iter().filter(|(k, _)| k == "Prefer").map(|(_, v)| v.clone()).collect()
        }
    }

    /// Scripted replies (status, JSON body; `Value::Null` = empty body),
    /// popped in order. Every request is recorded with its headers.
    struct Mock {
        replies: Mutex<VecDeque<(u16, Option<u64>, String)>>,
        seen: Mutex<Vec<Seen>>,
    }

    impl Mock {
        fn new(replies: Vec<(u16, Value)>) -> Arc<Mock> {
            Arc::new(Mock {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|(s, b)| (s, None, if b.is_null() { String::new() } else { b.to_string() }))
                        .collect(),
                ),
                seen: Mutex::new(Vec::new()),
            })
        }
        fn throttled_then(retry_after: u64, then: Vec<(u16, Value)>) -> Arc<Mock> {
            let m = Mock::new(then);
            m.replies.lock().unwrap().push_front((429, Some(retry_after), String::new()));
            m
        }
        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
        fn left(&self) -> usize {
            self.replies.lock().unwrap().len()
        }
    }

    impl GraphHttp for Mock {
        fn send(&self, _: GraphMethod, url: &str, _: Option<&Value>) -> Result<GraphResponse, String> {
            panic!("calendar calls must go through send_with (Prefer headers): {url}")
        }
        fn send_with(
            &self,
            method: GraphMethod,
            url: &str,
            body: Option<&Value>,
            headers: &[(&'static str, String)],
        ) -> Result<GraphResponse, String> {
            self.seen.lock().unwrap().push(Seen {
                method,
                url: url.to_string(),
                body: body.cloned(),
                headers: headers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            });
            let (status, retry_after, body) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("mock ran out of scripted replies at {method:?} {url}"));
            Ok(GraphResponse { status, retry_after, body })
        }
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
            panic!("calendars never fetch bytes: {url}")
        }
    }

    fn row() -> MailExternalInbox {
        MailExternalInbox {
            id: "row-cal".into(),
            owner_project_id: "p1".into(),
            email_address: ME.into(),
            display_name: None,
            kind: "graph".into(),
            host: "graph.microsoft.com".into(),
            port: 443,
            tls: "implicit-tls".into(),
            username: ME.into(),
            drafts_folder: None,
            status: "connected".into(),
            last_checked_at: None,
            last_error: None,
            created_at: 0,
            smtp_host: None,
            smtp_port: None,
            smtp_tls: None,
        }
    }

    fn inbox(level: Level) -> GraphCalInbox {
        GraphCalInbox { address: ME.into(), row: row(), level, backend: crate::mail::calendar_provider::LinkedBackend::Microsoft }
    }

    fn cal(mock: &Arc<Mock>) -> GraphCalendar {
        GraphCalendar::with_http(mock.clone())
    }

    fn body(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("non-JSON body {e}: {}", r.body))
    }

    fn code(r: &CliResponse) -> String {
        body(r)["error"]["code"].as_str().expect("error.code").to_string()
    }

    fn ok(r: &CliResponse) -> Value {
        assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
        body(r)
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).expect("rfc3339").with_timezone(&Utc)
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A stored Graph event (UTC, as `Prefer: outlook.timezone="UTC"` returns it).
    fn stored(id: &str, attendees: Value, is_organizer: bool, organizer: &str, zone: &str) -> Value {
        json!({
            "id": id,
            "subject": "Standup",
            "start": { "dateTime": "2026-10-20T16:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-10-20T17:00:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "showAs": "busy",
            "type": "singleInstance",
            "attendees": attendees,
            "isOrganizer": is_organizer,
            "organizer": { "emailAddress": { "address": organizer, "name": "Org" } },
            "originalStartTimeZone": zone,
            "originalEndTimeZone": zone,
        })
    }

    fn attendee(addr: &str) -> Value {
        json!({ "type": "required", "status": { "response": "none" },
                "emailAddress": { "address": addr, "name": addr } })
    }

    fn write(v: Value) -> WriteBody {
        serde_json::from_value(v).expect("write body")
    }

    fn no_sleep() -> impl FnMut(StdDuration) {
        |_| {}
    }

    // ── create ──────────────────────────────────────────────────────────

    #[test]
    fn create_posts_to_me_events_with_no_attendees_in_the_callers_zone() {
        let created = stored("AAMk-new", json!([]), true, ME, "America/Los_Angeles");
        let m = Mock::new(vec![(201, created)]);
        let r = create_with(
            &cal(&m),
            &inbox(Level::Draft),
            &write(json!({
                "project": "p1",
                "title": "Focus block",
                "start": "2026-10-20T16:00:00Z",
                "end": "2026-10-20T17:00:00Z",
                "timeZone": "America/Los_Angeles",
                "location": "Desk",
                "notes": "heads down",
                "alerts": [{ "action": "display", "offset": "-PT10M" }],
                // Only the mailbox itself: not an invite.
                "participants": [ME],
            })),
        );
        let out = ok(&r);
        let seen = m.seen();
        assert_eq!(seen.len(), 1, "{seen:?}");
        let s = &seen[0];
        assert_eq!(s.method, GraphMethod::Post);
        assert_eq!(s.path(), format!("{GRAPH}/me/events"));
        let b = s.body.as_ref().expect("create body");
        assert_eq!(b["attendees"], json!([]), "a draft NEVER carries attendees: {b}");
        assert_eq!(b["subject"], "Focus block");
        // 16:00Z = 09:00 PDT, written as wall time + IANA zone.
        assert_eq!(b["start"], json!({ "dateTime": "2026-10-20T09:00:00", "timeZone": "America/Los_Angeles" }));
        assert_eq!(b["end"], json!({ "dateTime": "2026-10-20T10:00:00", "timeZone": "America/Los_Angeles" }));
        assert_eq!(b["location"], json!({ "displayName": "Desk" }));
        assert_eq!(b["body"], json!({ "contentType": "text", "content": "heads down" }));
        assert_eq!(b["isReminderOn"], true);
        assert_eq!(b["reminderMinutesBeforeStart"], 10);
        assert!(
            s.prefers().contains(&"outlook.timezone=\"UTC\"".to_string()),
            "reads come back in UTC: {:?}",
            s.headers
        );
        assert!(!s.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization")));
        assert_eq!(out["id"], cal::encode_event_id(ME, "AAMk-new"));
        assert_eq!(out["emailSent"], false);
        assert_eq!(out["source"], "microsoft");
        assert_eq!(out["event"]["timeZone"], "America/Los_Angeles", "the event's own zone is kept");
        assert_eq!(out["event"]["start"], "2026-10-20T16:00:00Z");
    }

    #[test]
    fn create_naming_another_person_is_refused_before_any_request() {
        for participants in [
            json!(["colleague@contoso.com"]),
            json!([{ "email": "colleague@contoso.com" }]),
            json!({ "p1": { "calendarAddress": "mailto:outsider@example.com" } }),
        ] {
            let m = Mock::new(vec![]);
            let r = create_with(
                &cal(&m),
                &inbox(Level::Draft),
                &write(json!({
                    "project": "p1", "title": "Sync",
                    "start": "2026-10-20T16:00:00Z", "end": "2026-10-20T17:00:00Z",
                    "participants": participants,
                })),
            );
            assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
            assert_eq!(code(&r), "invites_need_send_level");
            assert!(m.seen().is_empty(), "nothing may reach Graph: {:?}", m.seen());
        }
    }

    #[test]
    fn create_with_an_email_alarm_is_refused_before_any_request() {
        let m = Mock::new(vec![]);
        let r = create_with(
            &cal(&m),
            &inbox(Level::Draft),
            &write(json!({
                "project": "p1", "title": "Sync",
                "start": "2026-10-20T16:00:00Z", "end": "2026-10-20T17:00:00Z",
                "alerts": [{ "action": "email", "offset": "-PT10M" }],
            })),
        );
        assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
        assert_eq!(code(&r), "email_alerts_need_send_level");
        assert!(m.seen().is_empty());
    }

    #[test]
    fn create_rejects_bad_times_and_unknown_zones_without_a_write() {
        for (b, want) in [
            (json!({"project":"p1","title":"x","start":"2026-10-20T17:00:00Z","end":"2026-10-20T16:00:00Z"}), "after"),
            (json!({"project":"p1","title":"x","start":"2026-10-20","end":"2026-10-21"}), "start"),
            (json!({"project":"p1","title":" ","start":"2026-10-20T16:00:00Z","end":"2026-10-20T17:00:00Z"}), "title"),
            (json!({"project":"p1","title":"x","start":"2026-10-20T16:00:00Z","end":"2026-10-20T17:00:00Z","timeZone":"Mars/Olympus_Mons"}), "IANA"),
        ] {
            let m = Mock::new(vec![]);
            let r = create_with(&cal(&m), &inbox(Level::Draft), &write(b.clone()));
            assert!(r.status.starts_with("400"), "{b}: {} {}", r.status, r.body);
            assert_eq!(code(&r), "usage");
            assert!(r.body.contains(want), "{b}: {}", r.body);
            assert!(m.seen().is_empty(), "{b}");
        }
    }

    #[test]
    fn create_in_a_named_calendar_checks_it_is_the_mailboxs_own_and_editable() {
        let cals = json!({ "value": [
            { "id": "CAL-own", "name": "Calendar", "canEdit": true, "isDefaultCalendar": true,
              "owner": { "address": ME, "name": "Me" } },
            { "id": "CAL-shared", "name": "Boss", "canEdit": true,
              "owner": { "address": "boss@contoso.com", "name": "Boss" } },
            { "id": "CAL-ro", "name": "Holidays", "canEdit": false,
              "owner": { "address": ME, "name": "Me" } },
        ]});
        // Own + editable: GET calendars, then POST into that calendar.
        let m = Mock::new(vec![(200, cals.clone()), (201, stored("E1", json!([]), true, ME, "UTC"))]);
        let r = create_with(
            &cal(&m),
            &inbox(Level::Draft),
            &write(json!({"project":"p1","title":"x","calendar":"CAL-own",
                          "start":"2026-10-20T16:00:00Z","end":"2026-10-20T17:00:00Z"})),
        );
        assert_eq!(ok(&r)["calendarId"], "CAL-own");
        let seen = m.seen();
        assert_eq!(seen[1].method, GraphMethod::Post);
        assert_eq!(seen[1].path(), format!("{GRAPH}/me/calendars/CAL-own/events"));
        assert_eq!(seen[1].body.as_ref().expect("body")["start"]["timeZone"], "UTC");
        // Shared to the mailbox, or read-only: 403, no write.
        for id in ["CAL-shared", "CAL-ro"] {
            let m = Mock::new(vec![(200, cals.clone())]);
            let r = create_with(
                &cal(&m),
                &inbox(Level::Draft),
                &write(json!({"project":"p1","title":"x","calendar":id,
                              "start":"2026-10-20T16:00:00Z","end":"2026-10-20T17:00:00Z"})),
            );
            assert!(r.status.starts_with("403"), "{id}: {} {}", r.status, r.body);
            assert_eq!(code(&r), "share_rights");
            assert_eq!(m.seen().len(), 1, "{id}: only the calendar list was read");
        }
        // Unknown calendar: 404, no write.
        let m = Mock::new(vec![(200, cals)]);
        let r = create_with(
            &cal(&m),
            &inbox(Level::Draft),
            &write(json!({"project":"p1","title":"x","calendar":"nope",
                          "start":"2026-10-20T16:00:00Z","end":"2026-10-20T17:00:00Z"})),
        );
        assert!(r.status.starts_with("404"), "{}", r.body);
        assert_eq!(m.seen().len(), 1);
    }

    // ── update / delete: the STORED event's guard ───────────────────────

    #[test]
    fn update_of_an_event_with_attendees_is_409_and_never_patched() {
        let ev = stored("E1", json!([attendee(ME), attendee("colleague@contoso.com")]), true, ME, "UTC");
        let m = Mock::new(vec![(200, ev)]);
        let r = update_with(&cal(&m), &inbox(Level::Draft), "E1", "ev_tok", &write(json!({"project":"p1","title":"new"})));
        assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
        assert_eq!(code(&r), "invites_need_send_level");
        let seen = m.seen();
        assert_eq!(seen.len(), 1, "only the stored read: {seen:?}");
        assert_eq!(seen[0].method, GraphMethod::Get);
        assert_eq!(seen[0].path(), format!("{GRAPH}/me/events/E1"));
        assert!(seen[0].query()["$select"].contains("attendees"), "the guard reads attendees");
    }

    #[test]
    fn update_and_delete_of_an_event_someone_else_organised_are_409() {
        // No attendee listed, but the mailbox isn't the organizer: changing
        // or deleting it would message the organizer.
        let ev = stored("E2", json!([]), false, "boss@contoso.com", "UTC");
        let m = Mock::new(vec![(200, ev.clone())]);
        let r = update_with(&cal(&m), &inbox(Level::Draft), "E2", "ev_tok", &write(json!({"project":"p1","title":"x"})));
        assert_eq!(code(&r), "invites_need_send_level", "{}", r.body);
        assert_eq!(m.seen().len(), 1);
        let m = Mock::new(vec![(200, ev)]);
        let r = delete_with(&cal(&m), &inbox(Level::Draft), "E2", "ev_tok");
        assert_eq!(code(&r), "invites_need_send_level", "{}", r.body);
        assert!(m.seen().iter().all(|s| s.method != GraphMethod::Delete));
    }

    #[test]
    fn update_with_participants_in_the_body_is_refused_before_any_request() {
        let m = Mock::new(vec![]);
        let r = update_with(
            &cal(&m),
            &inbox(Level::Draft),
            "E1",
            "ev_tok",
            &write(json!({"project":"p1","participants":["x@example.com"]})),
        );
        assert_eq!(code(&r), "invites_need_send_level");
        assert!(m.seen().is_empty());
    }

    #[test]
    fn update_moves_the_event_in_its_own_iana_zone_and_keeps_the_duration() {
        let ev = stored("E3", json!([attendee(ME)]), true, ME, "Europe/Berlin");
        let patched = json!({
            "id": "E3", "subject": "Standup",
            "start": { "dateTime": "2026-10-21T08:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-10-21T09:00:00.0000000", "timeZone": "UTC" },
            "originalStartTimeZone": "Europe/Berlin",
        });
        let m = Mock::new(vec![(200, ev), (200, patched)]);
        let r = update_with(
            &cal(&m),
            &inbox(Level::Draft),
            "E3",
            "ev_tok",
            &write(json!({"project":"p1","start":"2026-10-21T08:00:00Z"})),
        );
        let out = ok(&r);
        let seen = m.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].method, GraphMethod::Patch);
        assert_eq!(seen[1].path(), format!("{GRAPH}/me/events/E3"));
        let p = seen[1].body.as_ref().expect("patch");
        assert!(p.get("attendees").is_none(), "a draft patch never touches attendees: {p}");
        // 08:00Z = 10:00 CEST; the hour-long duration is kept.
        assert_eq!(p["start"], json!({ "dateTime": "2026-10-21T10:00:00", "timeZone": "Europe/Berlin" }));
        assert_eq!(p["end"], json!({ "dateTime": "2026-10-21T11:00:00", "timeZone": "Europe/Berlin" }));
        assert!(p.get("subject").is_none(), "only what changed: {p}");
        assert_eq!(out["id"], "ev_tok");
        assert_eq!(out["emailSent"], false);
        assert_eq!(out["event"]["timeZone"], "Europe/Berlin");
        assert_eq!(out["event"]["localStart"], "2026-10-21T10:00:00");
    }

    #[test]
    fn update_of_an_event_in_a_windows_zone_writes_utc() {
        // Exchange often stores Windows zone names; they're not IANA, so the
        // write is UTC (the instant is right; Outlook shows it localised).
        let ev = stored("E4", json!([]), true, ME, "Pacific Standard Time");
        let m = Mock::new(vec![(200, ev.clone()), (200, ev)]);
        let r = update_with(
            &cal(&m),
            &inbox(Level::Draft),
            "E4",
            "ev_tok",
            &write(json!({"project":"p1","end":"2026-10-20T18:30:00Z"})),
        );
        ok(&r);
        let p = m.seen()[1].body.clone().expect("patch");
        assert_eq!(p["start"], json!({ "dateTime": "2026-10-20T16:00:00", "timeZone": "UTC" }));
        assert_eq!(p["end"], json!({ "dateTime": "2026-10-20T18:30:00", "timeZone": "UTC" }));
    }

    #[test]
    fn update_rejects_end_before_start_and_empty_patches_without_writing() {
        let m = Mock::new(vec![(200, stored("E5", json!([]), true, ME, "UTC"))]);
        let r = update_with(
            &cal(&m),
            &inbox(Level::Draft),
            "E5",
            "ev_tok",
            &write(json!({"project":"p1","start":"2026-10-20T18:00:00Z","end":"2026-10-20T17:00:00Z"})),
        );
        assert_eq!(code(&r), "usage", "{}", r.body);
        assert_eq!(m.seen().len(), 1, "no PATCH");
        let m = Mock::new(vec![(200, stored("E5", json!([]), true, ME, "UTC"))]);
        let r = update_with(&cal(&m), &inbox(Level::Draft), "E5", "ev_tok", &write(json!({"project":"p1"})));
        assert_eq!(code(&r), "usage", "{}", r.body);
        assert_eq!(m.seen().len(), 1, "no PATCH");
    }

    #[test]
    fn delete_of_a_clean_event_issues_delete_and_one_with_attendees_never_does() {
        let m = Mock::new(vec![(200, stored("E6", json!([attendee(ME)]), true, ME, "UTC")), (204, Value::Null)]);
        let r = delete_with(&cal(&m), &inbox(Level::Draft), "E6", "ev_tok");
        let out = ok(&r);
        assert_eq!(out["deleted"], true);
        assert_eq!(out["emailSent"], false);
        let seen = m.seen();
        assert_eq!(seen[1].method, GraphMethod::Delete);
        assert_eq!(seen[1].path(), format!("{GRAPH}/me/events/E6"));
        assert!(seen[1].body.is_none());

        let m = Mock::new(vec![(200, stored("E7", json!([attendee("guest@example.com")]), true, ME, "UTC"))]);
        let r = delete_with(&cal(&m), &inbox(Level::Draft), "E7", "ev_tok");
        assert_eq!(code(&r), "invites_need_send_level");
        assert_eq!(m.seen().len(), 1);
        assert!(m.seen().iter().all(|s| s.method != GraphMethod::Delete));
    }

    #[test]
    fn a_missing_event_is_the_masked_not_found_with_the_callers_token() {
        let m = Mock::new(vec![(404, json!({"error":{"code":"ErrorItemNotFound","message":"not found"}}))]);
        let r = show_with(&cal(&m), &inbox(Level::Read), "E404", "ev_the_token");
        assert!(r.status.starts_with("404"));
        assert_eq!(code(&r), "not_found");
        assert!(r.body.contains("ev_the_token"), "{}", r.body);
        assert!(!r.body.contains("ErrorItemNotFound"), "Graph's text isn't echoed: {}", r.body);
        let m = Mock::new(vec![(404, json!({}))]);
        let r = delete_with(&cal(&m), &inbox(Level::Draft), "E404", "ev_the_token");
        assert_eq!(code(&r), "not_found");
    }

    // ── reads ───────────────────────────────────────────────────────────

    #[test]
    fn events_read_the_calendar_view_in_utc_and_report_truncation() {
        let page = json!({
            "value": [
                stored("O1", json!([]), true, ME, "Eastern Standard Time"),
                stored("O2", json!([attendee("x@example.com")]), true, ME, "UTC"),
                stored("O3", json!([]), true, ME, "UTC"),
            ],
        });
        let m = Mock::new(vec![(200, page)]);
        let r = events_with(
            &cal(&m),
            &inbox(Level::Read),
            &params(&[
                ("start", "2026-10-20T00:00:00Z"),
                ("end", "2026-10-27T00:00:00Z"),
                ("limit", "2"),
                ("calendar", "CAL-1"),
            ]),
            at("2026-10-17T00:00:00Z"),
        );
        let out = ok(&r);
        let s = &m.seen()[0];
        assert_eq!(s.method, GraphMethod::Get);
        assert_eq!(s.path(), format!("{GRAPH}/me/calendars/CAL-1/calendarView"));
        let q = s.query();
        assert_eq!(q["startDateTime"], "2026-10-20T00:00:00Z");
        assert_eq!(q["endDateTime"], "2026-10-27T00:00:00Z");
        assert_eq!(q["$top"], "3", "one extra row tells truncation");
        assert!(s.prefers().contains(&"outlook.timezone=\"UTC\"".to_string()));
        let evs = out["events"].as_array().expect("events");
        assert_eq!(evs.len(), 2);
        assert_eq!(out["truncated"], true);
        assert_eq!(evs[0]["id"], cal::encode_event_id(ME, "O1"));
        assert_eq!(evs[0]["timeZone"], "Eastern Standard Time", "the stored zone name is kept as-is");
        assert_eq!(evs[0]["duration"], cal::format_duration(3600));
        assert_eq!(evs[1]["participants"], 1);
        assert_eq!(evs[0]["calendarIds"], json!(["CAL-1"]));
        assert_eq!(out["source"], "microsoft");
    }

    #[test]
    fn list_maps_rights_from_ownership_edit_and_grant_level() {
        let cals = json!({ "value": [
            { "id": "C1", "name": "Calendar", "canEdit": true, "isDefaultCalendar": true,
              "owner": { "address": "Rosson@Contoso.com" } },
            { "id": "C2", "name": "Boss", "canEdit": true, "owner": { "address": "boss@contoso.com" } },
        ]});
        let m = Mock::new(vec![(200, cals.clone())]);
        let out = ok(&list_with(&cal(&m), &inbox(Level::Draft)));
        assert_eq!(m.seen()[0].path(), format!("{GRAPH}/me/calendars"));
        let c = out["calendars"].as_array().expect("calendars");
        assert_eq!(c[0]["shared"], false);
        assert_eq!(c[0]["rights"]["write"], true);
        assert_eq!(c[0]["isDefault"], true);
        assert_eq!(c[1]["shared"], true);
        assert_eq!(c[1]["owner"], "boss@contoso.com");
        assert_eq!(c[1]["rights"]["write"], false, "shared calendars are read-only at draft level");
        // A read grant never writes.
        let m = Mock::new(vec![(200, cals)]);
        let out = ok(&list_with(&cal(&m), &inbox(Level::Read)));
        assert_eq!(out["calendars"][0]["rights"]["write"], false);
    }

    #[test]
    fn throttled_reads_back_off_then_succeed() {
        let m = Mock::throttled_then(2, vec![(200, json!({ "value": [] }))]);
        let out = ok(&list_with(&cal(&m), &inbox(Level::Read)));
        assert_eq!(out["calendars"], json!([]));
        assert_eq!(m.seen().len(), 2, "one retry after the 429");
        // A Graph 5xx is a 502 engine error carrying Graph's own message.
        let m = Mock::new(vec![(503, json!({"error":{"message":"Service busy"}}))]);
        let r = list_with(&cal(&m), &inbox(Level::Read));
        assert!(r.status.starts_with("502"), "{}", r.status);
        assert!(r.body.contains("Service busy"), "{}", r.body);
    }

    // ── free/busy ───────────────────────────────────────────────────────

    #[test]
    fn freebusy_asks_getschedule_for_the_mailbox_and_never_returns_titles() {
        let sched = json!({ "value": [{
            "scheduleId": ME,
            "scheduleItems": [
                { "status": "busy", "subject": "Secret board meeting",
                  "start": { "dateTime": "2026-10-20T09:00:00.0000000", "timeZone": "UTC" },
                  "end": { "dateTime": "2026-10-20T10:00:00.0000000", "timeZone": "UTC" } },
                { "status": "free",
                  "start": { "dateTime": "2026-10-20T11:00:00.0000000", "timeZone": "UTC" },
                  "end": { "dateTime": "2026-10-20T12:00:00.0000000", "timeZone": "UTC" } },
                { "status": "tentative",
                  "start": { "dateTime": "2026-10-20T09:30:00.0000000", "timeZone": "UTC" },
                  "end": { "dateTime": "2026-10-20T10:30:00.0000000", "timeZone": "UTC" } },
                // Starts before the window: clipped to it.
                { "status": "oof",
                  "start": { "dateTime": "2026-10-19T20:00:00.0000000", "timeZone": "UTC" },
                  "end": { "dateTime": "2026-10-20T01:00:00.0000000", "timeZone": "UTC" } },
            ],
        }]});
        let m = Mock::new(vec![(200, sched)]);
        let r = freebusy_with(
            &cal(&m),
            &inbox(Level::Read),
            &params(&[("start", "2026-10-20T00:00:00Z"), ("end", "2026-10-21T00:00:00Z")]),
            at("2026-10-17T00:00:00Z"),
        );
        let out = ok(&r);
        let s = &m.seen()[0];
        assert_eq!(s.method, GraphMethod::Post);
        assert_eq!(s.path(), format!("{GRAPH}/me/calendar/getSchedule"));
        let b = s.body.as_ref().expect("getSchedule body");
        assert_eq!(b["schedules"], json!([ME]), "self only (multi-attendee is MCS9)");
        assert_eq!(b["startTime"], json!({ "dateTime": "2026-10-20T00:00:00", "timeZone": "UTC" }));
        assert_eq!(b["endTime"], json!({ "dateTime": "2026-10-21T00:00:00", "timeZone": "UTC" }));
        assert_eq!(out["source"], "getSchedule");
        assert!(!r.body.contains("Secret board meeting"), "titles never leave: {}", r.body);
        let busy = out["busy"].as_array().expect("busy");
        assert_eq!(
            busy[0],
            json!({ "start": "2026-10-20T00:00:00Z", "end": "2026-10-20T01:00:00Z", "status": "unavailable" })
        );
        // busy 9–10 + tentative 9:30–10:30 merge; free 11–12 is dropped.
        assert_eq!(busy.len(), 2, "{busy:?}");
        assert_eq!(busy[1]["start"], "2026-10-20T09:00:00Z");
        assert_eq!(busy[1]["end"], "2026-10-20T10:30:00Z");
    }

    #[test]
    fn freebusy_falls_back_to_the_calendar_view_when_getschedule_is_refused() {
        // Personal accounts can refuse getSchedule; the same answer comes
        // from the expanded view.
        let mut cancelled = stored("V2", json!([]), true, ME, "UTC");
        cancelled["isCancelled"] = json!(true);
        let mut free = stored("V3", json!([]), true, ME, "UTC");
        free["showAs"] = json!("free");
        let view = json!({ "value": [stored("V1", json!([]), true, ME, "UTC"), cancelled, free] });
        let m = Mock::new(vec![(403, json!({"error":{"message":"Access is denied."}})), (200, view)]);
        let r = freebusy_with(
            &cal(&m),
            &inbox(Level::Read),
            &params(&[("start", "2026-10-20T00:00:00Z"), ("end", "2026-10-21T00:00:00Z")]),
            at("2026-10-17T00:00:00Z"),
        );
        let out = ok(&r);
        let seen = m.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].path(), format!("{GRAPH}/me/calendarView"));
        assert_eq!(out["source"], "events");
        assert_eq!(
            out["busy"],
            json!([{ "start": "2026-10-20T16:00:00Z", "end": "2026-10-20T17:00:00Z", "status": "busy" }])
        );
        assert!(!r.body.contains("Standup"), "titles never leave: {}", r.body);
    }

    #[test]
    fn freebusy_getschedule_entry_error_also_falls_back() {
        let sched = json!({ "value": [{ "scheduleId": ME, "error": { "message": "not available" } }] });
        let m = Mock::new(vec![(200, sched), (200, json!({ "value": [] }))]);
        let out = ok(&freebusy_with(&cal(&m), &inbox(Level::Read), &params(&[]), at("2026-10-17T00:00:00Z")));
        assert_eq!(out["source"], "events");
        assert_eq!(out["busy"], json!([]));
    }

    // ── times ───────────────────────────────────────────────────────────

    #[test]
    fn graph_instants_parse_utc_with_fractions_and_honour_iana_zones() {
        let utc = json!({ "dateTime": "2026-10-20T16:00:00.0000000", "timeZone": "UTC" });
        assert_eq!(graph_instant(Some(&utc)), Some(at("2026-10-20T16:00:00Z")));
        let plain = json!({ "dateTime": "2026-10-20T16:00:00", "timeZone": "Etc/UTC" });
        assert_eq!(graph_instant(Some(&plain)), Some(at("2026-10-20T16:00:00Z")));
        let la = json!({ "dateTime": "2026-10-20T09:00:00.0000000", "timeZone": "America/Los_Angeles" });
        assert_eq!(graph_instant(Some(&la)), Some(at("2026-10-20T16:00:00Z")));
        // Unknown (Windows) names are read as UTC — reads ask for UTC anyway.
        let win = json!({ "dateTime": "2026-10-20T16:00:00", "timeZone": "Pacific Standard Time" });
        assert_eq!(graph_instant(Some(&win)), Some(at("2026-10-20T16:00:00Z")));
        assert_eq!(graph_instant(Some(&json!({ "dateTime": "not a time" }))), None);
        assert_eq!(graph_instant(None), None);
    }

    #[test]
    fn summaries_keep_the_events_zone_and_local_wall_time() {
        let ev = stored("Z1", json!([]), true, ME, "Asia/Tokyo");
        let s = shape_summary(&ev, ME, None);
        assert_eq!(s["start"], "2026-10-20T16:00:00Z");
        assert_eq!(s["timeZone"], "Asia/Tokyo");
        assert_eq!(s["localStart"], "2026-10-21T01:00:00");
        assert_eq!(s["invite"], json!({ "state": "none" }));
        let full = shape_full(
            &json!({ "id": "Z2", "body": { "content": "agenda" }, "isReminderOn": true,
                     "reminderMinutesBeforeStart": 5, "organizer": { "emailAddress": { "address": ME } },
                     "attendees": [attendee("x@example.com")] }),
            ME,
        );
        assert_eq!(full["description"], "agenda");
        assert_eq!(full["organizer"], ME);
        assert_eq!(full["participantList"][0]["email"], "x@example.com");
        assert_eq!(full["alerts"][0]["trigger"]["offset"], "-PT5M");
    }

    // ── delta / wait ────────────────────────────────────────────────────

    #[test]
    fn delta_tokens_come_from_links_and_round_trip_without_any_url() {
        let t = DeltaToken::from_link(
            "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=abc%3D%3D&x=1",
        )
        .expect("delta");
        assert_eq!(t, DeltaToken { kind: DeltaKind::Delta, value: "abc==".into() });
        let s = DeltaToken::from_link("https://x/y?flag&%24skiptoken=s-1").expect("skip");
        assert_eq!(s, DeltaToken { kind: DeltaKind::Skip, value: "s-1".into() });
        assert_eq!(DeltaToken::from_link("https://x/y?$deltatoken="), None);
        assert_eq!(DeltaToken::from_link("https://x/y?a=b"), None);
        assert_eq!(DeltaToken::from_link("no-query"), None);

        let state = t.encode();
        assert!(state.starts_with("gd_"), "{state}");
        let raw = String::from_utf8(B64URL.decode(&state[3..]).expect("b64")).expect("utf8");
        assert!(!raw.contains("http"), "the state holds only the token: {raw}");
        assert_eq!(DeltaToken::decode(&state), Some(t));
        assert_eq!(DeltaToken::decode(&s.encode()), Some(s));
        // A hosted JMAP state, garbage, a bad kind, or an oversized blob.
        assert_eq!(DeltaToken::decode("s123"), None);
        assert_eq!(DeltaToken::decode("gd_!!!"), None);
        assert_eq!(DeltaToken::decode(&format!("gd_{}", B64URL.encode(r#"{"k":"x","t":"v"}"#))), None);
        assert_eq!(DeltaToken::decode(&format!("gd_{}", "A".repeat(9000))), None);
    }

    #[test]
    fn wait_reads_a_baseline_then_reports_changes_following_only_tokens() {
        let m = Mock::new(vec![
            // Baseline round: one page with a nextLink on ANOTHER host (the
            // daemon must keep only the token), then the deltaLink.
            (200, json!({ "value": [stored("OLD", json!([]), true, ME, "UTC")],
                          "@odata.nextLink": "https://evil.example/steal?$skiptoken=s1" })),
            (200, json!({ "value": [],
                          "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=d1" })),
            // Next round: one change, one removal.
            (200, json!({ "value": [ { "id": "A", "subject": "moved" },
                                     { "id": "B", "@removed": { "reason": "deleted" } } ],
                          "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=d2" })),
        ]);
        let mut elapsed = || 0;
        let mut sleep = no_sleep();
        let r = wait_with(
            &cal(&m),
            &inbox(Level::Read),
            None,
            300,
            at("2026-10-17T00:00:00Z"),
            &mut elapsed,
            &mut sleep,
        );
        let out = ok(&r);
        assert_eq!(out["timedOut"], false);
        assert_eq!(out["updated"], json!([cal::encode_event_id(ME, "A")]));
        assert_eq!(out["destroyed"], json!([cal::encode_event_id(ME, "B")]));
        assert_eq!(out["created"], json!([]));
        assert!(!r.body.contains(&cal::encode_event_id(ME, "OLD")), "baseline items are not changes");
        assert_eq!(
            DeltaToken::decode(out["state"].as_str().expect("state")),
            Some(DeltaToken { kind: DeltaKind::Delta, value: "d2".into() })
        );
        let seen = m.seen();
        assert_eq!(seen.len(), 3);
        let base = format!("{GRAPH}/me/calendarView/delta");
        for s in &seen {
            assert_eq!(s.path(), base, "never a link's host: {}", s.url);
            assert!(
                s.prefers().contains(&format!("odata.maxpagesize={DELTA_PAGE_SIZE}")),
                "{:?}",
                s.headers
            );
        }
        let q0 = seen[0].query();
        assert_eq!(q0["startDateTime"], "2026-10-16T00:00:00Z");
        assert_eq!(q0["endDateTime"], "2027-10-18T00:00:00Z");
        assert_eq!(seen[1].query(), HashMap::from([("$skiptoken".to_string(), "s1".to_string())]));
        assert_eq!(seen[2].query(), HashMap::from([("$deltatoken".to_string(), "d1".to_string())]));
    }

    #[test]
    fn wait_from_a_state_times_out_with_the_newest_state() {
        let since = DeltaToken { kind: DeltaKind::Delta, value: "d5".into() }.encode();
        let m = Mock::new(vec![(200, json!({ "value": [], "@odata.deltaLink": "https://g/x?$deltatoken=d6" }))]);
        let mut slept = 0u64;
        let mut elapsed = || 295;
        let mut sleep = |d: StdDuration| slept += d.as_secs();
        let r = wait_with(
            &cal(&m),
            &inbox(Level::Read),
            Some(&since),
            300,
            at("2026-10-17T00:00:00Z"),
            &mut elapsed,
            &mut sleep,
        );
        let out = ok(&r);
        assert_eq!(out["timedOut"], true);
        assert_eq!(
            DeltaToken::decode(out["state"].as_str().expect("state")),
            Some(DeltaToken { kind: DeltaKind::Delta, value: "d6".into() })
        );
        assert_eq!(m.seen().len(), 1, "no baseline read when a state is given");
        assert_eq!(m.seen()[0].query()["$deltatoken"], "d5");
        assert_eq!(slept, 0);
        assert_eq!(m.left(), 0);
    }

    #[test]
    fn wait_polls_until_a_change_arrives() {
        let since = DeltaToken { kind: DeltaKind::Delta, value: "d1".into() }.encode();
        let m = Mock::new(vec![
            (200, json!({ "value": [], "@odata.deltaLink": "https://g/x?$deltatoken=d2" })),
            (200, json!({ "value": [{ "id": "N" }], "@odata.deltaLink": "https://g/x?$deltatoken=d3" })),
        ]);
        let mut slept = Vec::new();
        let mut elapsed = || 0;
        let mut sleep = |d: StdDuration| slept.push(d.as_secs());
        let out = ok(&wait_with(
            &cal(&m),
            &inbox(Level::Read),
            Some(&since),
            300,
            at("2026-10-17T00:00:00Z"),
            &mut elapsed,
            &mut sleep,
        ));
        assert_eq!(out["updated"], json!([cal::encode_event_id(ME, "N")]));
        assert_eq!(slept, vec![GRAPH_WAIT_POLL_SECS]);
        assert_eq!(m.seen()[1].query()["$deltatoken"], "d2");
    }

    #[test]
    fn wait_answers_state_expired_on_410_and_usage_on_a_foreign_state() {
        let since = DeltaToken { kind: DeltaKind::Delta, value: "old".into() }.encode();
        let m = Mock::new(vec![(410, json!({"error":{"code":"syncStateNotFound"}}))]);
        let mut elapsed = || 0;
        let mut sleep = no_sleep();
        let r = wait_with(
            &cal(&m),
            &inbox(Level::Read),
            Some(&since),
            300,
            at("2026-10-17T00:00:00Z"),
            &mut elapsed,
            &mut sleep,
        );
        assert!(r.status.starts_with("409"), "{}", r.status);
        assert_eq!(code(&r), "state_expired");
        let m = Mock::new(vec![]);
        let r = wait_with(
            &cal(&m),
            &inbox(Level::Read),
            Some("s-hosted-jmap"),
            300,
            at("2026-10-17T00:00:00Z"),
            &mut elapsed,
            &mut sleep,
        );
        assert_eq!(code(&r), "usage");
        assert!(m.seen().is_empty());
    }

    #[test]
    fn a_delta_reply_without_a_link_is_an_engine_error() {
        let m = Mock::new(vec![(200, json!({ "value": [] }))]);
        let err = cal(&m).delta(None, at("2026-10-17T00:00:00Z")).expect_err("no link");
        assert!(matches!(err, CalErr::Engine(ref h) if h.contains("deltaLink")), "{err:?}");
    }

    #[test]
    fn capability_missing_carries_cap_reason_and_fix() {
        let r = capability_missing(
            "calendar.write",
            "scope_not_granted",
            "k2 mail link upgrade a@b --features calendar (owner)",
            "hint",
        );
        assert!(r.status.starts_with("409"));
        let e = &body(&r)["error"];
        assert_eq!(e["code"], "capability_missing");
        assert_eq!(e["cap"], "calendar.write");
        assert_eq!(e["reason"], "scope_not_granted");
        assert_eq!(e["fix"], "k2 mail link upgrade a@b --features calendar (owner)");
        assert_eq!(cap_name(Level::Read), "calendar.read");
        assert_eq!(cap_name(Level::Draft), "calendar.write");
        assert_eq!(cap_name(Level::Send), "calendar.invite");
    }
}
