//! `k2 calendar` — the AGENT calendar CLI over hosted inboxes
//! (prd-hostmail-calendars-v1 S3; review CAL16–CAL20, CAL23, CAL32).
//!
//! Routes (registered in `mail_routes`, Member floor in `ROUTES`):
//!
//! | route                                   | level |
//! |-----------------------------------------|-------|
//! | GET  /cli/mail/calendar/list            | read  |
//! | GET  /cli/mail/calendar/events          | read  |
//! | GET  /cli/mail/calendar/show            | read  |
//! | GET  /cli/mail/calendar/freebusy        | read  |
//! | GET  /cli/mail/calendar/wait            | read  |
//! | POST /cli/mail/calendar/create          | draft |
//! | POST /cli/mail/calendar/update          | draft |
//! | POST /cli/mail/calendar/delete          | draft |
//!
//! **Access (CAL2/CAL32):** the existing `k2 mail access` levels — read =
//! read calendars, draft = write events WITHOUT email, send = invites
//! (S5, not built). Every existing hosted read/draft grant therefore
//! covers calendars too. Every deny is the same masked `not_found` the
//! mail read gate uses ([`access::can_read`] / [`access::can_draft`]).
//!
//! **Inbox (CAL19):** param `address`. Absent → the ONE active hosted row
//! the caller's workspace owns ([`access::primary_hosted`]): 0 → 404
//! `not_found`, more than 1 → 400 `usage` listing them. A linked inbox →
//! 400 `hosted_only`, only AFTER the access gate passed (no existence
//! leak). Event ids are opaque `ev_…` tokens carrying their inbox
//! address, exactly like mail's `m_…` message ids.
//!
//! **Account (CAL16):** the Stalwart account id comes ONLY from the
//! address row; no client `accountId` is ever read. The admin API key can
//! address any account, so every calendar and event id is validated
//! against the RESOLVED account (`Calendar/get` / `CalendarEvent/get` on
//! that account — an id from another account is `notFound` there and
//! answers the masked `not_found`).
//!
//! **Params (CAL18):** the window is `start`/`end` (RFC 3339 or a
//! `YYYY-MM-DD` date = midnight UTC). Never `from`/`to`:
//! `stamp_principal` overwrites `from` on every scoped GET (TCP and cell
//! UDS alike).
//!
//! **Draft guard (CAL4/CAL20):** every `CalendarEvent/set` goes through
//! [`StalwartClient::calendar_event_set_no_scheduling`], which forces
//! `sendSchedulingMessages: false`. Create/update with a participant other
//! than the inbox owner → 409 `invites_need_send_level`. Update and delete
//! first `CalendarEvent/get` the STORED event and refuse the same way when
//! it already has non-owner participants (a draft delete would leave
//! attendees without a CANCEL). Alerts with an email action → 409
//! `email_alerts_need_send_level`. S3 creates no calendars (events only),
//! so it is clear of the 0.16.10 per-user `Calendar/set` property bug.
//!
//! **Owner switch:** when the owner turned calendars off for hosted
//! addresses (`k2 hostmail calendar disable`, `mail::dav`), every verb
//! answers 409 `calendars_disabled`. The Stalwart permissions that switch
//! sets bind USER logins only; the admin key the daemon uses is not bound
//! by them, so this check is what keeps agents in step with the owner.

use std::collections::HashMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::access::{self, Level, Source};
use crate::mail::jmap::StalwartClient;
use crate::mail::messages::ReadError;

// ── Limits ──────────────────────────────────────────────────────────────

/// Default `events`/`freebusy` window when `end` is absent.
const DEFAULT_WINDOW_DAYS: i64 = 7;
/// Longest window one call may ask for (recurrence expansion is bounded
/// by the window, so this also bounds the server's work).
const MAX_WINDOW_DAYS: i64 = 366;
/// `events` page size: default and hard cap.
const DEFAULT_EVENTS: usize = 100;
const MAX_EVENTS: usize = 500;
/// `freebusy` reads at most this many (expanded) events per window.
const FREEBUSY_MAX_EVENTS: usize = 2000;
/// `CalendarEvent/get` batch size.
const GET_BATCH: usize = 100;
/// `wait` polls `CalendarEvent/changes` every this many seconds.
const WAIT_POLL_SECS: u64 = 2;
/// `wait` timeout default / max (mirrors `k2 mail wait`).
const WAIT_DEFAULT_TIMEOUT_SECS: u64 = 300;
const WAIT_MAX_TIMEOUT_SECS: u64 = 900;
/// Field caps for draft writes.
const MAX_TITLE: usize = 500;
const MAX_LOCATION: usize = 500;
const MAX_NOTES: usize = 20_000;
/// The time zone stored when the caller names none.
const DEFAULT_TZ: &str = "Etc/UTC";

/// Opaque event-id prefix (`ev_<b64url(address \n jmap-id)>`).
const EVENT_ID_PREFIX: &str = "ev_";

/// Properties read for summaries / full views / the draft guard.
const SUMMARY_PROPS: &[&str] = &[
    "id",
    "baseEventId",
    "calendarIds",
    "title",
    "start",
    "duration",
    "timeZone",
    "showWithoutTime",
    "utcStart",
    "utcEnd",
    "locations",
    "participants",
    "status",
    "freeBusyStatus",
    "recurrenceRule",
    "recurrenceId",
];
const FULL_PROPS: &[&str] = &[
    "id",
    "baseEventId",
    "uid",
    "calendarIds",
    "title",
    "description",
    "start",
    "duration",
    "timeZone",
    "showWithoutTime",
    "utcStart",
    "utcEnd",
    "locations",
    "participants",
    "organizerCalendarAddress",
    "alerts",
    "status",
    "freeBusyStatus",
    "privacy",
    "recurrenceRule",
    "recurrenceId",
];

// ── Response helpers ────────────────────────────────────────────────────

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn usage(hint: impl AsRef<str>) -> CliResponse {
    error_response("400 Bad Request", "usage", hint.as_ref())
}

fn engine_err(hint: impl AsRef<str>) -> CliResponse {
    error_response("502 Bad Gateway", "engine", hint.as_ref())
}

fn ok_json(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn invites_need_send(what: &str) -> CliResponse {
    error_response(
        "409 Conflict",
        "invites_need_send_level",
        &format!(
            "{what} — participants other than the inbox owner would get invitation \
             email, which needs the 'send' level (calendar invites are not available \
             yet). Draft level writes your own events only; ask your human"
        ),
    )
}

// ── The JMAP seam (fixture-faked in tests) ──────────────────────────────

/// The five JMAP-for-Calendars calls S3 makes. Production is
/// [`StalwartClient`]; tests use a recording fake. `account` is always
/// the account K2 resolved from the address row.
pub trait CalendarApi {
    /// `Calendar/get` → the `list`.
    fn calendars(&self, account: &str) -> Result<Vec<Value>, String>;
    /// `CalendarEvent/query` → ids.
    fn query(&self, account: &str, args: Value) -> Result<Vec<String>, String>;
    /// `CalendarEvent/get` → the method response (`list`/`notFound`/`state`).
    fn get(&self, account: &str, ids: &[String], props: Option<&[&str]>) -> Result<Value, String>;
    /// `CalendarEvent/changes` → the method response.
    fn changes(&self, account: &str, since_state: &str) -> Result<Value, String>;
    /// `CalendarEvent/set` with `sendSchedulingMessages: false` FORCED.
    fn set_no_scheduling(&self, account: &str, args: Value) -> Result<Value, String>;
    /// `Principal/getAvailability` for the account's own principal.
    fn availability(&self, account: &str, utc_start: &str, utc_end: &str)
        -> Result<Vec<Value>, String>;
}

impl CalendarApi for StalwartClient {
    fn availability(&self, account: &str, utc_start: &str, utc_end: &str)
        -> Result<Vec<Value>, String> {
        self.principal_availability(account, utc_start, utc_end)
    }
    fn calendars(&self, account: &str) -> Result<Vec<Value>, String> {
        self.calendar_list(account)
    }
    fn query(&self, account: &str, args: Value) -> Result<Vec<String>, String> {
        self.calendar_event_query(account, args)
    }
    fn get(&self, account: &str, ids: &[String], props: Option<&[&str]>) -> Result<Value, String> {
        self.calendar_event_get(account, ids, props)
    }
    fn changes(&self, account: &str, since_state: &str) -> Result<Value, String> {
        self.calendar_event_changes(account, since_state)
    }
    fn set_no_scheduling(&self, account: &str, args: Value) -> Result<Value, String> {
        self.calendar_event_set_no_scheduling(account, args)
    }
}

/// The production engine (503 `not_ready` when the server isn't up).
fn engine() -> Result<StalwartClient, CliResponse> {
    crate::mail::domains::engine_from_db()
        .map(|(client, _host)| client)
        .map_err(|hint| error_response("503 Service Unavailable", "not_ready", &hint))
}

// ── Owner switch ────────────────────────────────────────────────────────

/// 409 `calendars_disabled` when the owner turned calendars off for
/// hosted addresses (S2 DAV policy). NULL policy = Stalwart default (on).
fn calendars_enabled_gate() -> Result<(), CliResponse> {
    match crate::mail::dav::load_policy() {
        Ok(Some(p)) if p.calendars == crate::mail::dav::OnOff::Off => Err(error_response(
            "409 Conflict",
            "calendars_disabled",
            "the owner turned calendars off for hosted addresses on this server — \
             they can turn them back on with 'k2 hostmail calendar enable'; ask your human",
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(engine_err(format!("reading the calendar policy: {e}"))),
    }
}

// ── Identity ────────────────────────────────────────────────────────────

/// GET identity: the stamped scoped principal wins over `project=`
/// (same as `k2 mail messages`). Returns the caller's workspace uuid.
fn caller_from_params(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    match crate::mail::identity::resolve_caller_params(params) {
        Ok((_path, id)) => Ok(id),
        Err(resp) => {
            if crate::caller_workspace::principal_from_params(params).is_none()
                && crate::cli::need_project(params).is_err()
            {
                return Err(usage("missing 'project' (workspace name | path | UUID)"));
            }
            Err(resp)
        }
    }
}

/// POST identity: request-scoped principal first, else the body claim.
fn caller_from_body(project: &str) -> Result<String, CliResponse> {
    crate::mail::identity::resolve_caller(project).map(|(_path, id)| id)
}

// ── Inbox + id resolution (CAL16, CAL19) ────────────────────────────────

/// A hosted inbox the caller may use at the requested level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalInbox {
    pub address: String,
    /// The Stalwart account id — from the address row, never the client.
    pub account_id: String,
}

fn gate_error(err: ReadError) -> CliResponse {
    match err {
        ReadError::Usage(h) => usage(h),
        ReadError::NotFound(h) => error_response("404 Not Found", "not_found", &h),
        ReadError::Engine(h) => engine_err(h),
    }
}

/// Resolve the inbox for a calendar verb: explicit `address`, else the
/// single hosted inbox the caller owns. Masked like the mail gate.
pub fn resolve_inbox(
    project_id: &str,
    explicit: Option<&str>,
    need: Level,
) -> Result<CalInbox, CliResponse> {
    let address = match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(a) => a.to_string(),
        None => {
            let owned = access::primary_hosted(project_id).map_err(engine_err)?;
            match owned.len() {
                0 => {
                    return Err(error_response(
                        "404 Not Found",
                        "not_found",
                        "no hosted inbox in this workspace — name one you have access to \
                         ('k2 mail inboxes'), or mint one with 'k2 mail create'",
                    ))
                }
                1 => owned.into_iter().next().unwrap_or_default(),
                _ => {
                    return Err(usage(format!(
                        "this workspace owns {} hosted inboxes — pass one: {}",
                        owned.len(),
                        owned.join(", ")
                    )))
                }
            }
        }
    };
    let gate = match need {
        Level::Read => access::can_read(project_id, &address),
        Level::Draft => access::can_draft(project_id, &address),
        Level::Send => access::can_send(project_id, &address),
    };
    let inbox = gate.map_err(gate_error)?;
    // CAL19: only AFTER access passed, so a linked inbox the caller
    // cannot see still answers the masked not_found above.
    if inbox.source == Source::Linked {
        return Err(error_response(
            "400 Bad Request",
            "hosted_only",
            &format!(
                "'{}' is a linked inbox — calendars are available on hosted (K2-minted) \
                 inboxes only",
                inbox.address
            ),
        ));
    }
    let Some(account_id) = inbox.account_id.filter(|s| !s.trim().is_empty()) else {
        return Err(engine_err(format!(
            "address '{}' has no mailbox on the mail server",
            inbox.address
        )));
    };
    Ok(CalInbox { address: inbox.address, account_id })
}

pub fn encode_event_id(address: &str, jmap_id: &str) -> String {
    format!("{EVENT_ID_PREFIX}{}", B64URL.encode(format!("{address}\n{jmap_id}")))
}

pub fn decode_event_id(token: &str) -> Option<(String, String)> {
    let raw = token.trim().strip_prefix(EVENT_ID_PREFIX)?;
    let bytes = B64URL.decode(raw).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let (address, id) = s.split_once('\n')?;
    if address.is_empty() || id.is_empty() || id.contains('\n') {
        return None;
    }
    Some((address.to_string(), id.to_string()))
}

/// Decode + gate an event id. Every gate failure is the same masked
/// event-level `not_found` (the token's address is never confirmed).
fn resolve_event(
    project_id: &str,
    token: &str,
    need: Level,
) -> Result<(CalInbox, String), CliResponse> {
    let Some((address, jmap_id)) = decode_event_id(token) else {
        return Err(usage("invalid event id — use an id from 'k2 calendar events'"));
    };
    match resolve_inbox(project_id, Some(&address), need) {
        Ok(inbox) => Ok((inbox, jmap_id)),
        Err(resp) if resp.status.starts_with("404") => Err(event_not_found(token)),
        Err(resp) => Err(resp),
    }
}

fn event_not_found(token: &str) -> CliResponse {
    error_response(
        "404 Not Found",
        "not_found",
        &format!("no event '{token}' in this workspace"),
    )
}

// ── Times ───────────────────────────────────────────────────────────────

/// RFC 3339 instant, or `YYYY-MM-DD` = midnight UTC.
pub fn parse_instant(raw: &str, field: &str) -> Result<DateTime<Utc>, String> {
    let t = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        if let Some(ndt) = d.and_hms_opt(0, 0, 0) {
            return Ok(ndt.and_utc());
        }
    }
    Err(format!(
        "'{field}' must be RFC 3339 (e.g. 2026-10-08T09:00:00-07:00 or 2026-10-08T16:00:00Z) \
         or a date YYYY-MM-DD — got '{t}'"
    ))
}

/// RFC 3339 instant only (event start/end: a bare date is ambiguous).
fn parse_event_instant(raw: &str, field: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| {
            format!(
                "'{field}' must be an RFC 3339 time with an offset \
                 (e.g. 2026-10-08T09:00:00-07:00 or 2026-10-08T16:00:00Z) — got '{}'",
                raw.trim()
            )
        })
}

/// RFC 3339 UTC with seconds (`2026-10-08T16:00:00Z`) — also the JMAP
/// `UTCDate` wire form.
pub fn rfc3339_utc(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The `[start, end)` window of a read: defaults to now .. now + 7 days,
/// `end` must follow `start`, and the span is capped at 366 days.
fn window(
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), CliResponse> {
    let start = match crate::cli::opt_param(params, "start") {
        Some(s) => parse_instant(&s, "start").map_err(usage)?,
        None => now,
    };
    let end = match crate::cli::opt_param(params, "end") {
        Some(s) => parse_instant(&s, "end").map_err(usage)?,
        None => start + Duration::days(DEFAULT_WINDOW_DAYS),
    };
    if end <= start {
        return Err(usage("'end' must be after 'start'"));
    }
    if end - start > Duration::days(MAX_WINDOW_DAYS) {
        return Err(usage(format!(
            "the window is capped at {MAX_WINDOW_DAYS} days — narrow --from/--to"
        )));
    }
    Ok((start, end))
}

/// ISO 8601 duration (`PT1H30M`, `P1DT2H`) → seconds. Weeks (`P2W`) too.
pub fn parse_duration_secs(raw: &str) -> Option<i64> {
    let s = raw.trim();
    let body = s.strip_prefix('P').filter(|b| !b.is_empty())?;
    let (date_part, time_part) = match body.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (body, None),
    };
    let mut total: i64 = 0;
    let mut num = String::new();
    for c in date_part.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: i64 = num.parse().ok()?;
        num.clear();
        total += match c {
            'W' => n * 7 * 86_400,
            'D' => n * 86_400,
            _ => return None,
        };
    }
    if !num.is_empty() {
        return None;
    }
    if let Some(t) = time_part {
        if t.is_empty() {
            return None;
        }
        for c in t.chars() {
            if c.is_ascii_digit() {
                num.push(c);
                continue;
            }
            let n: i64 = num.parse().ok()?;
            num.clear();
            total += match c {
                'H' => n * 3600,
                'M' => n * 60,
                'S' => n,
                _ => return None,
            };
        }
        if !num.is_empty() {
            return None;
        }
    }
    Some(total)
}

/// Seconds → ISO 8601 duration (`PT1H30M`, `P1DT2H`, `PT0S`).
pub fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut out = String::from("P");
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if rem > 0 || days == 0 {
        out.push('T');
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 || (h == 0 && m == 0) {
            out.push_str(&format!("{s}S"));
        }
    }
    out
}

/// A JSCalendar `LocalDateTime` (`2026-10-08T09:00:00`).
fn local_datetime(dt: &NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// The event's UTC `[start, end)`: Stalwart's computed `utcStart`/
/// `utcEnd` when present; otherwise `start` + `duration` when the event
/// is in UTC or floating (no other zone can be resolved without a tz
/// database, and then `None`).
fn event_utc_span(ev: &Value) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let parse_utc = |k: &str| {
        ev.get(k)
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    let (mut start, mut end) = (parse_utc("utcStart"), parse_utc("utcEnd"));
    if start.is_none() {
        let tz = ev.get("timeZone").and_then(Value::as_str);
        let utc_like = matches!(tz, None | Some("Etc/UTC") | Some("UTC") | Some("Etc/GMT"));
        if utc_like {
            start = ev
                .get("start")
                .and_then(Value::as_str)
                .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
                .map(|n| n.and_utc());
        }
    }
    if end.is_none() {
        if let Some(s) = start {
            let dur = ev
                .get("duration")
                .and_then(Value::as_str)
                .and_then(parse_duration_secs)
                .unwrap_or(0);
            end = Some(s + Duration::seconds(dur));
        }
    }
    (start, end)
}

// ── Participants / alerts (CAL20) ───────────────────────────────────────

/// `mailto:x@y` / `x@y` → `x@y`, lowercased.
fn strip_mailto(s: &str) -> String {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let s = if lower.starts_with("mailto:") { &s[7..] } else { s };
    s.trim().to_ascii_lowercase()
}

/// The email of a JSCalendar participant. Stalwart 0.16.20 (calcard
/// 0.3.13) builds the ATTENDEE from `calendarAddress` (`mailto:…`); `email`
/// is only a parameter and `sendTo` is not used for the address — but a
/// raw caller may send either, so all three are read (first wins).
fn participant_email(p: &Value) -> Option<String> {
    for ptr in ["/calendarAddress", "/sendTo/imip", "/email"] {
        if let Some(s) = p.pointer(ptr).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return Some(strip_mailto(s));
            }
        }
    }
    None
}

/// Participants that are NOT the inbox owner (any participant with no
/// resolvable email counts as non-owner: fail closed).
pub fn non_owner_participants(participants: Option<&Value>, owner: &str) -> Vec<String> {
    let owner = owner.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    let iter: Vec<&Value> = match participants {
        Some(Value::Object(m)) => m.values().collect(),
        Some(Value::Array(a)) => a.iter().collect(),
        _ => Vec::new(),
    };
    for p in iter {
        let email = match p {
            Value::String(s) if !s.trim().is_empty() => Some(strip_mailto(s)),
            Value::String(_) => None,
            _ => participant_email(p),
        };
        match email {
            Some(e) if e == owner => {}
            Some(e) => out.push(e),
            None => out.push("(participant without an address)".to_string()),
        }
    }
    out
}

/// True when any alert (object map or array) has an email action.
pub fn has_email_alert(alerts: Option<&Value>) -> bool {
    let iter: Vec<&Value> = match alerts {
        Some(Value::Object(m)) => m.values().collect(),
        Some(Value::Array(a)) => a.iter().collect(),
        _ => return false,
    };
    iter.into_iter().any(|a| {
        a.get("action")
            .and_then(Value::as_str)
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("email"))
    })
}

fn email_alert_refused() -> CliResponse {
    error_response(
        "409 Conflict",
        "email_alerts_need_send_level",
        "an email alarm makes the mail server send email, which draft level never does — \
         use a display alarm, or ask your human for the 'send' level",
    )
}

// ── Shaping ─────────────────────────────────────────────────────────────

fn first_location(ev: &Value) -> Option<String> {
    let locs = ev.get("locations")?;
    let iter: Vec<&Value> = match locs {
        Value::Object(m) => m.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => return None,
    };
    iter.into_iter()
        .find_map(|l| l.get("name").and_then(Value::as_str))
        .map(str::to_string)
}

fn calendar_ids(ev: &Value) -> Vec<String> {
    match ev.get("calendarIds") {
        Some(Value::Object(m)) => m
            .iter()
            .filter(|(_, v)| v.as_bool().unwrap_or(true))
            .map(|(k, _)| k.clone())
            .collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

fn participant_count(ev: &Value) -> usize {
    match ev.get("participants") {
        Some(Value::Object(m)) => m.len(),
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    }
}

/// The STABLE id of an event: `baseEventId` when present. Stalwart
/// 0.16.20 gives EVERY result of an expanded query a synthetic,
/// index-based id (`calendar_event/query.rs:355`) — even single events —
/// and those shift when the series is edited, so they are never handed
/// out. A repeating event's id is therefore its series: `update`/`delete`
/// act on the whole series.
fn stable_id(ev: &Value) -> &str {
    ev.get("baseEventId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| ev.get("id").and_then(Value::as_str))
        .unwrap_or("")
}

fn is_recurring(ev: &Value) -> bool {
    ev.get("recurrenceRule").is_some_and(|r| !r.is_null())
        || ev.get("recurrenceRules").is_some_and(|r| !r.is_null())
        || ev.get("recurrenceId").is_some_and(|r| !r.is_null())
}

/// One event, summary shape (the `events` list and write replies).
pub fn shape_summary(ev: &Value, address: &str) -> Value {
    let id = stable_id(ev);
    let (start, end) = event_utc_span(ev);
    json!({
        "id": encode_event_id(address, id),
        "calendarIds": calendar_ids(ev),
        "title": ev.get("title").and_then(Value::as_str).unwrap_or(""),
        "start": start.map(|d| rfc3339_utc(&d)),
        "end": end.map(|d| rfc3339_utc(&d)),
        "localStart": ev.get("start").and_then(Value::as_str),
        "duration": ev.get("duration").and_then(Value::as_str),
        "timeZone": ev.get("timeZone").and_then(Value::as_str),
        "allDay": ev.get("showWithoutTime").and_then(Value::as_bool).unwrap_or(false),
        "location": first_location(ev),
        "status": ev.get("status").and_then(Value::as_str).unwrap_or("confirmed"),
        "freeBusy": ev.get("freeBusyStatus").and_then(Value::as_str).unwrap_or("busy"),
        "recurring": is_recurring(ev),
        "participants": participant_count(ev),
    })
}

/// One event, full shape (`show`).
pub fn shape_full(ev: &Value, address: &str) -> Value {
    let mut out = shape_summary(ev, address);
    out["address"] = json!(address);
    out["uid"] = ev.get("uid").cloned().unwrap_or(Value::Null);
    out["description"] = ev.get("description").cloned().unwrap_or(Value::Null);
    out["privacy"] = ev.get("privacy").cloned().unwrap_or(Value::Null);
    out["organizer"] = ev
        .get("organizerCalendarAddress")
        .and_then(Value::as_str)
        .map(|s| json!(strip_mailto(s)))
        .unwrap_or(Value::Null);
    let parts: Vec<Value> = match ev.get("participants") {
        Some(Value::Object(m)) => m.values().cloned().collect(),
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    out["participantList"] = Value::Array(
        parts
            .iter()
            .map(|p| {
                json!({
                    "name": p.get("name").and_then(Value::as_str),
                    "email": participant_email(p),
                    "roles": p.get("roles").cloned().unwrap_or(Value::Null),
                    "status": p.get("participationStatus").and_then(Value::as_str),
                })
            })
            .collect(),
    );
    let alerts: Vec<Value> = match ev.get("alerts") {
        Some(Value::Object(m)) => m.values().cloned().collect(),
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    out["alerts"] = Value::Array(
        alerts
            .iter()
            .map(|a| {
                json!({
                    "action": a.get("action").and_then(Value::as_str).unwrap_or("display"),
                    "trigger": a.get("trigger").cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    );
    out
}

fn shape_calendar(c: &Value) -> Value {
    json!({
        "id": c.get("id").and_then(Value::as_str).unwrap_or(""),
        "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
        "description": c.get("description").cloned().unwrap_or(Value::Null),
        "color": c.get("color").cloned().unwrap_or(Value::Null),
        "isDefault": c.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        "timeZone": c.get("timeZone").cloned().unwrap_or(Value::Null),
    })
}

// ── Calendar ids (validated against the resolved account) ───────────────

/// `--calendar <id|name>` → an id that belongs to `account`. An unknown
/// value is a 400 listing the account's calendars (they're the caller's
/// own, so nothing leaks).
fn resolve_calendar(
    api: &dyn CalendarApi,
    account: &str,
    raw: &str,
) -> Result<String, CliResponse> {
    let cals = api.calendars(account).map_err(engine_err)?;
    let raw = raw.trim();
    if let Some(c) = cals
        .iter()
        .find(|c| c.get("id").and_then(Value::as_str) == Some(raw))
    {
        return Ok(c["id"].as_str().unwrap_or_default().to_string());
    }
    let named: Vec<&Value> = cals
        .iter()
        .filter(|c| {
            c.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(raw))
        })
        .collect();
    if named.len() == 1 {
        return Ok(named[0]["id"].as_str().unwrap_or_default().to_string());
    }
    let list: Vec<String> = cals
        .iter()
        .map(|c| {
            format!(
                "{} ({})",
                c.get("name").and_then(Value::as_str).unwrap_or("?"),
                c.get("id").and_then(Value::as_str).unwrap_or("?")
            )
        })
        .collect();
    Err(usage(format!(
        "no single calendar '{raw}' on this inbox — available: {}",
        if list.is_empty() { "(none)".to_string() } else { list.join(", ") }
    )))
}

/// The calendar a create lands in when `--calendar` is absent: the one
/// marked `isDefault`, else the only calendar, else the first by
/// `sortOrder`. `None` = the account has no calendar yet.
pub(crate) fn default_calendar(cals: &[Value]) -> Option<String> {
    if let Some(c) = cals
        .iter()
        .find(|c| c.get("isDefault").and_then(Value::as_bool) == Some(true))
    {
        return c.get("id").and_then(Value::as_str).map(str::to_string);
    }
    let mut sorted: Vec<&Value> = cals.iter().collect();
    sorted.sort_by_key(|c| c.get("sortOrder").and_then(Value::as_u64).unwrap_or(u64::MAX));
    sorted
        .first()
        .and_then(|c| c.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

// ── Event reads ─────────────────────────────────────────────────────────

/// `CalendarEvent/get` in batches; returns the events in `ids` order.
fn get_events(
    api: &dyn CalendarApi,
    account: &str,
    ids: &[String],
    props: &[&str],
) -> Result<Vec<Value>, String> {
    let mut by_id: HashMap<String, Value> = HashMap::new();
    for chunk in ids.chunks(GET_BATCH) {
        let reply = api.get(account, chunk, Some(props))?;
        for ev in reply.get("list").and_then(Value::as_array).cloned().unwrap_or_default() {
            if let Some(id) = ev.get("id").and_then(Value::as_str) {
                by_id.insert(id.to_string(), ev);
            }
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// One STORED event on the resolved account; `None` = not on it.
fn get_one(
    api: &dyn CalendarApi,
    account: &str,
    jmap_id: &str,
    props: &[&str],
) -> Result<Option<Value>, String> {
    let reply = api.get(account, &[jmap_id.to_string()], Some(props))?;
    Ok(reply
        .get("list")
        .and_then(Value::as_array)
        .and_then(|l| {
            l.iter()
                .find(|e| e.get("id").and_then(Value::as_str) == Some(jmap_id))
                .cloned()
        }))
}

/// The `CalendarEvent/query` args for a window: events overlapping
/// `[start, end)` (Stalwart: `after` < event end, `before` > event
/// start), recurrences expanded inside it server-side.
///
/// Stalwart 0.16.20 parses `after`/`before` with `to_timestamp_local()`,
/// which DROPS any offset or `Z` and reads the wall time in the query's
/// `timeZone` (`jmap-proto/src/object/calendar_event.rs:312-333`). So the
/// window goes out as offset-free UTC wall times with `timeZone:
/// "Etc/UTC"` — never an RFC 3339 string with an offset. The filter key
/// is the singular `inCalendar`; `expandRecurrences` needs both bounds
/// (`query.rs:267`) and is capped server-side (3000 expansions →
/// `invalidArguments`, mapped to a usage error).
fn window_query(
    start: &DateTime<Utc>,
    end: &DateTime<Utc>,
    calendar: Option<&str>,
    limit: usize,
) -> Value {
    let mut filter = json!({
        "after": local_datetime(&start.naive_utc()),
        "before": local_datetime(&end.naive_utc()),
    });
    if let Some(c) = calendar {
        filter["inCalendar"] = json!(c);
    }
    json!({
        "filter": filter,
        "sort": [{ "property": "start", "isAscending": true }],
        "expandRecurrences": true,
        "timeZone": DEFAULT_TZ,
        "limit": limit,
    })
}

// ── GET /cli/mail/calendar/list ─────────────────────────────────────────

pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let inbox = match resolve_inbox(&project_id, params.get("address").map(String::as_str), Level::Read)
    {
        Ok(i) => i,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => list_with(&api, &inbox),
        Err(r) => r,
    }
}

pub fn list_with(api: &dyn CalendarApi, inbox: &CalInbox) -> CliResponse {
    match api.calendars(&inbox.account_id) {
        Ok(cals) => ok_json(json!({
            "ok": true,
            "address": inbox.address,
            "calendars": cals.iter().map(shape_calendar).collect::<Vec<_>>(),
        })),
        Err(e) => engine_err(e),
    }
}

// ── GET /cli/mail/calendar/events ───────────────────────────────────────

pub fn handle_events(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let inbox = match resolve_inbox(&project_id, params.get("address").map(String::as_str), Level::Read)
    {
        Ok(i) => i,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => events_with(&api, &inbox, params, Utc::now()),
        Err(r) => r,
    }
}

pub fn events_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    let (start, end) = match window(params, now) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let limit = match params.get("limit").map(|v| v.trim().parse::<usize>()) {
        None => DEFAULT_EVENTS,
        Some(Ok(n)) if n >= 1 => n.min(MAX_EVENTS),
        _ => return usage(format!("invalid 'limit' — a number from 1 to {MAX_EVENTS}")),
    };
    let calendar = match crate::cli::opt_param(params, "calendar") {
        Some(raw) => match resolve_calendar(api, &inbox.account_id, &raw) {
            Ok(id) => Some(id),
            Err(r) => return r,
        },
        None => None,
    };
    // Ask for one more than the page so `truncated` is honest.
    let ids = match api.query(
        &inbox.account_id,
        window_query(&start, &end, calendar.as_deref(), limit + 1),
    ) {
        Ok(ids) => ids,
        Err(e) => return query_err(e),
    };
    let truncated = ids.len() > limit;
    let ids: Vec<String> = ids.into_iter().take(limit).collect();
    let events = match get_events(api, &inbox.account_id, &ids, SUMMARY_PROPS) {
        Ok(evs) => evs,
        Err(e) => return engine_err(e),
    };
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "start": rfc3339_utc(&start),
        "end": rfc3339_utc(&end),
        "calendar": calendar,
        "events": events.iter().map(|e| shape_summary(e, &inbox.address)).collect::<Vec<_>>(),
        "truncated": truncated,
    }))
}

// ── GET /cli/mail/calendar/show ─────────────────────────────────────────

pub fn handle_show(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(token) = crate::cli::opt_param(params, "id") else {
        return usage("missing 'id' — an event id from 'k2 calendar events'");
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let (inbox, jmap_id) = match resolve_event(&project_id, &token, Level::Read) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => show_with(&api, &inbox, &jmap_id, &token),
        Err(r) => r,
    }
}

pub fn show_with(api: &dyn CalendarApi, inbox: &CalInbox, jmap_id: &str, token: &str) -> CliResponse {
    match get_one(api, &inbox.account_id, jmap_id, FULL_PROPS) {
        Ok(Some(ev)) => ok_json(json!({
            "ok": true,
            "address": inbox.address,
            "event": shape_full(&ev, &inbox.address),
        })),
        Ok(None) => event_not_found(token),
        Err(e) => engine_err(e),
    }
}

// ── GET /cli/mail/calendar/freebusy ─────────────────────────────────────

pub fn handle_freebusy(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let inbox = match resolve_inbox(&project_id, params.get("address").map(String::as_str), Level::Read)
    {
        Ok(i) => i,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => freebusy_with(&api, &inbox, params, Utc::now()),
        Err(r) => r,
    }
}

/// A busy span `[start, end)` with its label.
pub type BusySpan = (DateTime<Utc>, DateTime<Utc>, BusyKind);

/// Busy labels, weakest first (a merged span keeps the strongest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusyKind {
    Tentative,
    Unavailable,
    Busy,
}

impl BusyKind {
    fn as_str(self) -> &'static str {
        match self {
            BusyKind::Tentative => "tentative",
            BusyKind::Unavailable => "unavailable",
            BusyKind::Busy => "busy",
        }
    }
}

/// Busy windows of the inbox (titles never leave this function).
///
/// Primary: Stalwart's own `Principal/getAvailability` for the account's
/// principal (0.16.20: subscribed calendars with availability on;
/// cancelled, free and secret events skipped; recurrences expanded;
/// offsets honoured). If that call fails (e.g. a server that refuses it),
/// the same answer is computed from the inbox's expanded events —
/// `source` in the reply says which.
pub fn freebusy_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    params: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> CliResponse {
    let (start, end) = match window(params, now) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let (spans, source, truncated) =
        match api.availability(&inbox.account_id, &rfc3339_utc(&start), &rfc3339_utc(&end)) {
            Ok(list) => (availability_spans(&list, start, end), "availability", false),
            Err(_) => match busy_from_events(api, inbox, start, end) {
                Ok((spans, truncated)) => (spans, "events", truncated),
                Err(r) => return r,
            },
        };
    let busy = merge_busy(spans);
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "start": rfc3339_utc(&start),
        "end": rfc3339_utc(&end),
        "busy": busy
            .iter()
            .map(|(s, e, k)| json!({
                "start": rfc3339_utc(s),
                "end": rfc3339_utc(e),
                "status": k.as_str(),
            }))
            .collect::<Vec<_>>(),
        "source": source,
        "truncated": truncated,
    }))
}

fn clip(
    s: DateTime<Utc>,
    e: DateTime<Utc>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (s, e) = (s.max(start), e.min(end));
    (e > s).then_some((s, e))
}

/// `Principal/getAvailability` `list` → spans (`busyStatus` confirmed →
/// busy, unavailable, tentative), clipped to the window.
fn availability_spans(list: &[Value], start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<BusySpan> {
    let parse = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    list.iter()
        .filter_map(|p| {
            let (s, e) = (parse(p.get("utcStart"))?, parse(p.get("utcEnd"))?);
            let kind = match p.get("busyStatus").and_then(Value::as_str) {
                Some("tentative") => BusyKind::Tentative,
                Some("unavailable") => BusyKind::Unavailable,
                _ => BusyKind::Busy,
            };
            let (s, e) = clip(s, e, start, end)?;
            Some((s, e, kind))
        })
        .collect()
}

/// Fallback: expanded events in the window, minus cancelled, free and
/// secret ones (Stalwart's availability rules), as spans.
fn busy_from_events(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(Vec<BusySpan>, bool), CliResponse> {
    let ids = api
        .query(&inbox.account_id, window_query(&start, &end, None, FREEBUSY_MAX_EVENTS + 1))
        .map_err(query_err)?;
    let truncated = ids.len() > FREEBUSY_MAX_EVENTS;
    let ids: Vec<String> = ids.into_iter().take(FREEBUSY_MAX_EVENTS).collect();
    let events = get_events(
        api,
        &inbox.account_id,
        &ids,
        &[
            "id", "baseEventId", "start", "duration", "timeZone", "utcStart", "utcEnd",
            "status", "freeBusyStatus", "privacy",
        ],
    )
    .map_err(engine_err)?;
    let mut spans = Vec::new();
    for ev in &events {
        let s = |k: &str| ev.get(k).and_then(Value::as_str);
        if s("status") == Some("cancelled") || s("privacy") == Some("secret") {
            continue;
        }
        let kind = match s("freeBusyStatus").unwrap_or("busy") {
            "free" => continue,
            "tentative" => BusyKind::Tentative,
            _ => BusyKind::Busy,
        };
        let (Some(es), Some(ee)) = event_utc_span(ev) else {
            continue;
        };
        if let Some((cs, ce)) = clip(es, ee, start, end) {
            spans.push((cs, ce, kind));
        }
    }
    Ok((spans, truncated))
}

/// Merge overlapping/adjacent spans; a merged span keeps the strongest
/// label (busy > unavailable > tentative).
pub fn merge_busy(mut spans: Vec<BusySpan>) -> Vec<BusySpan> {
    spans.sort_by_key(|(s, e, _)| (*s, *e));
    let mut out: Vec<BusySpan> = Vec::new();
    for (s, e, k) in spans {
        if let Some(last) = out.last_mut() {
            if s <= last.1 {
                if e > last.1 {
                    last.1 = e;
                }
                last.2 = last.2.max(k);
                continue;
            }
        }
        out.push((s, e, k));
    }
    out
}

/// A query failure the caller can fix (too many expansions, bad filter)
/// is a 400; the rest are 502.
fn query_err(e: String) -> CliResponse {
    if e.contains("invalidArguments") || e.contains("unsupportedFilter") {
        usage(format!(
            "{e} — the window holds too many occurrences or an unsupported filter; \
             narrow --from/--to"
        ))
    } else {
        engine_err(e)
    }
}

// ── GET /cli/mail/calendar/wait ─────────────────────────────────────────

pub fn handle_wait(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let timeout_secs = match parse_wait_timeout(params) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let inbox = match resolve_inbox(&project_id, params.get("address").map(String::as_str), Level::Read)
    {
        Ok(i) => i,
        Err(r) => return r,
    };
    let api = match engine() {
        Ok(a) => a,
        Err(r) => return r,
    };
    // Shares the per-workspace long-poll cap with `k2 mail wait`.
    let Some(_slot) = crate::mail::routes_messages::WaitSlot::try_acquire(&project_id) else {
        return error_response(
            "429 Too Many Requests",
            "rate_limited",
            "this workspace already has 4 wait calls open — loop ONE wait, don't fan out",
        );
    };
    let since = crate::cli::opt_param(params, "since_state");
    let start = std::time::Instant::now();
    let mut elapsed = || start.elapsed().as_secs();
    let mut sleep = |d: std::time::Duration| std::thread::sleep(d);
    wait_with(&api, &inbox, since.as_deref(), timeout_secs, &mut elapsed, &mut sleep)
}

fn parse_wait_timeout(params: &HashMap<String, String>) -> Result<u64, CliResponse> {
    match params.get("timeout").map(|v| v.trim().parse::<u64>()) {
        None => Ok(WAIT_DEFAULT_TIMEOUT_SECS),
        Some(Ok(n)) if (1..=WAIT_MAX_TIMEOUT_SECS).contains(&n) => Ok(n),
        Some(Ok(n)) if n > WAIT_MAX_TIMEOUT_SECS => Err(usage(format!(
            "timeout is capped at {WAIT_MAX_TIMEOUT_SECS} s per call — loop wait calls for longer"
        ))),
        _ => Err(usage("invalid 'timeout' — seconds from 1 to 900 (default 300)")),
    }
}

fn id_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Long-poll `CalendarEvent/changes` until something changed or the
/// budget ran out. Without `since`, the current state is read first
/// (`CalendarEvent/get ids:[]`), so only changes AFTER the call count.
/// `elapsed`/`sleep` are injected (tests run instantly).
pub fn wait_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    since: Option<&str>,
    timeout_secs: u64,
    elapsed: &mut dyn FnMut() -> u64,
    sleep: &mut dyn FnMut(std::time::Duration),
) -> CliResponse {
    let mut state = match since.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => match api.get(&inbox.account_id, &[], None) {
            Ok(reply) => match reply.get("state").and_then(Value::as_str) {
                Some(s) => s.to_string(),
                None => return engine_err("CalendarEvent/get: reply has no state"),
            },
            Err(e) => return engine_err(e),
        },
    };
    let (mut created, mut updated, mut destroyed) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let reply = match api.changes(&inbox.account_id, &state) {
            Ok(r) => r,
            Err(e) if e.contains("cannotCalculateChanges") => {
                return error_response(
                    "409 Conflict",
                    "state_expired",
                    "the server can no longer list changes since that state — re-read with \
                     'k2 calendar events' and wait again without --since-state",
                )
            }
            Err(e) => return engine_err(e),
        };
        created.extend(id_list(reply.get("created")));
        updated.extend(id_list(reply.get("updated")));
        destroyed.extend(id_list(reply.get("destroyed")));
        if let Some(ns) = reply.get("newState").and_then(Value::as_str) {
            state = ns.to_string();
        }
        let more = reply.get("hasMoreChanges").and_then(Value::as_bool).unwrap_or(false);
        if more {
            continue;
        }
        if !(created.is_empty() && updated.is_empty() && destroyed.is_empty()) {
            let enc = |ids: &[String]| {
                ids.iter().map(|i| encode_event_id(&inbox.address, i)).collect::<Vec<_>>()
            };
            return ok_json(json!({
                "ok": true,
                "timedOut": false,
                "address": inbox.address,
                "state": state,
                "created": enc(&created),
                "updated": enc(&updated),
                "destroyed": enc(&destroyed),
            }));
        }
        if elapsed() + WAIT_POLL_SECS > timeout_secs {
            return ok_json(json!({
                "ok": true,
                "timedOut": true,
                "address": inbox.address,
                "state": state,
            }));
        }
        sleep(std::time::Duration::from_secs(WAIT_POLL_SECS));
    }
}

// ── Draft writes (POST) ─────────────────────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteBody {
    pub project: String,
    pub address: Option<String>,
    pub id: Option<String>,
    pub calendar: Option<String>,
    pub title: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub time_zone: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    /// Refused unless only the owner (CAL20). Accepted so a raw caller
    /// gets the teaching 409, not a silent drop.
    pub participants: Option<Value>,
    /// Display alerts only; email → 409 (CAL20).
    pub alerts: Option<Value>,
}

fn parse_body(body: &[u8]) -> Result<WriteBody, CliResponse> {
    serde_json::from_slice(body).map_err(|e| usage(format!("invalid JSON body: {e}")))
}

/// IANA-ish zone name check (no tz database here; Stalwart validates
/// the name itself and rejects unknown zones in the set).
fn valid_tz(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && tz
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

fn check_len(v: &Option<String>, field: &str, max: usize) -> Result<(), CliResponse> {
    match v {
        Some(s) if s.chars().count() > max => {
            Err(usage(format!("'{field}' is too long (max {max} characters)")))
        }
        _ => Ok(()),
    }
}

/// Display alerts → JSCalendar `alerts` (`{"a1": {...}}`). Each entry is
/// `{action?: "display", offset: "-PT15M"}` or a JSCalendar alert.
fn shape_alerts(alerts: &Value) -> Result<Value, CliResponse> {
    let items: Vec<&Value> = match alerts {
        Value::Array(a) => a.iter().collect(),
        Value::Object(m) => m.values().collect(),
        Value::Null => return Ok(Value::Null),
        _ => return Err(usage("'alerts' must be a list")),
    };
    let mut out = serde_json::Map::new();
    for (i, a) in items.into_iter().enumerate() {
        let trigger = if let Some(t) = a.get("trigger") {
            t.clone()
        } else if let Some(off) = a.get("offset").and_then(Value::as_str) {
            if parse_duration_secs(off.trim().trim_start_matches(['-', '+'])).is_none() {
                return Err(usage(format!("alert offset '{off}' is not an ISO 8601 duration")));
            }
            json!({ "@type": "OffsetTrigger", "offset": off.trim(), "relativeTo": "start" })
        } else {
            return Err(usage("each alert needs an 'offset' like -PT15M"));
        };
        out.insert(
            format!("a{}", i + 1),
            json!({ "@type": "Alert", "action": "display", "trigger": trigger }),
        );
    }
    Ok(Value::Object(out))
}

fn set_error(method: &str, reply: &Value, key: &str) -> Option<String> {
    let map = reply.get(key)?.as_object()?;
    let (_, err) = map.iter().next()?;
    let etype = err.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let desc = err.get("description").and_then(Value::as_str).unwrap_or("");
    Some(if desc.is_empty() {
        format!("{method}: {etype}")
    } else {
        format!("{method}: {etype}: {desc}")
    })
}

/// A set-level failure the agent can fix (bad zone, bad property) is a
/// 400; the rest stay 502.
fn set_failure(msg: String) -> CliResponse {
    if msg.contains("invalidProperties") || msg.contains("invalidPatch") {
        usage(msg)
    } else {
        engine_err(msg)
    }
}

// ── POST /cli/mail/calendar/create ──────────────────────────────────────

pub fn handle_create(body: &[u8]) -> CliResponse {
    let b = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match caller_from_body(&b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if b.id.is_some() {
        return usage("create takes no 'id' — use 'k2 calendar update <id>' to change an event");
    }
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let inbox = match resolve_inbox(&project_id, b.address.as_deref(), Level::Draft) {
        Ok(i) => i,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => create_with(&api, &inbox, &b),
        Err(r) => r,
    }
}

pub fn create_with(api: &dyn CalendarApi, inbox: &CalInbox, b: &WriteBody) -> CliResponse {
    // Draft guard first: no work at all for a refused write.
    let others = non_owner_participants(b.participants.as_ref(), &inbox.address);
    if !others.is_empty() {
        return invites_need_send(&format!(
            "this event names {} participant(s) besides {}",
            others.len(),
            inbox.address
        ));
    }
    if has_email_alert(b.alerts.as_ref()) {
        return email_alert_refused();
    }
    let title = b.title.as_deref().map(str::trim).unwrap_or("");
    if title.is_empty() {
        return usage("create requires a 'title'");
    }
    for (v, f, m) in [
        (&b.title, "title", MAX_TITLE),
        (&b.location, "location", MAX_LOCATION),
        (&b.notes, "notes", MAX_NOTES),
    ] {
        if let Err(r) = check_len(v, f, m) {
            return r;
        }
    }
    let (Some(start_raw), Some(end_raw)) = (b.start.as_deref(), b.end.as_deref()) else {
        return usage("create requires 'start' and 'end' (RFC 3339)");
    };
    let start = match parse_event_instant(start_raw, "start") {
        Ok(s) => s,
        Err(e) => return usage(e),
    };
    let end = match parse_event_instant(end_raw, "end") {
        Ok(s) => s,
        Err(e) => return usage(e),
    };
    if end <= start {
        return usage("'end' must be after 'start'");
    }
    let tz = b.time_zone.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(z) = tz {
        if !valid_tz(z) {
            return usage(format!("'{z}' is not an IANA time zone name (e.g. America/Los_Angeles)"));
        }
    }
    let cals = match api.calendars(&inbox.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    let calendar_id = match b.calendar.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match resolve_calendar(api, &inbox.account_id, raw) {
            Ok(id) => id,
            Err(r) => return r,
        },
        None => match default_calendar(&cals) {
            Some(id) => id,
            None => {
                return error_response(
                    "409 Conflict",
                    "no_calendar",
                    &format!(
                        "'{}' has no calendar yet — open it once in a calendar app (or ask \
                         your human), then retry; agents don't create calendars",
                        inbox.address
                    ),
                )
            }
        },
    };
    let mut ev = json!({
        "@type": "Event",
        "calendarIds": { calendar_id.clone(): true },
        "title": title,
    });
    apply_times(&mut ev, &start, &end, tz);
    if let Some(l) = b.location.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        ev["locations"] = json!({ "l1": { "@type": "Location", "name": l } });
    }
    if let Some(n) = b.notes.as_deref().filter(|s| !s.trim().is_empty()) {
        ev["description"] = json!(n);
    }
    if let Some(a) = b.alerts.as_ref() {
        match shape_alerts(a) {
            Ok(Value::Null) => {}
            Ok(v) => ev["alerts"] = v,
            Err(r) => return r,
        }
    }
    let reply = match api.set_no_scheduling(
        &inbox.account_id,
        json!({ "create": { "k2new": ev }, "sendSchedulingMessages": false }),
    ) {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(msg) = set_error("CalendarEvent/set create", &reply, "notCreated") {
        return set_failure(msg);
    }
    let Some(new_id) = reply.pointer("/created/k2new/id").and_then(Value::as_str) else {
        return engine_err("CalendarEvent/set create: no created id in reply");
    };
    let event = get_one(api, &inbox.account_id, new_id, SUMMARY_PROPS)
        .ok()
        .flatten()
        .map(|e| shape_summary(&e, &inbox.address));
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "id": encode_event_id(&inbox.address, new_id),
        "calendarId": calendar_id,
        "event": event,
        "emailSent": false,
    }))
}

/// Write the times of a UTC span. `end` is not a JSCalendar property
/// (Stalwart rejects unknown keys), so:
/// - no zone / UTC: `timeZone: "Etc/UTC"` + `start` as the offset-free
///   UTC wall time + `duration` (Stalwart drops offsets on `start`,
///   calcard `jscalendar/parser.rs:44-63`, so none is ever sent);
/// - another zone: `timeZone` + BOTH `utcStart`/`utcEnd` in `Z` form; the
///   server derives `start` + `duration` in that zone (0.16.20
///   `calendar_event/set.rs:1032-1077`) — K2 carries no tz database.
fn apply_times(ev: &mut Value, start: &DateTime<Utc>, end: &DateTime<Utc>, tz: Option<&str>) {
    let dur = format_duration((*end - *start).num_seconds());
    match tz {
        None | Some("Etc/UTC") | Some("UTC") => {
            ev["timeZone"] = json!(DEFAULT_TZ);
            ev["start"] = json!(local_datetime(&start.naive_utc()));
            ev["duration"] = json!(dur);
        }
        Some(z) => {
            ev["timeZone"] = json!(z);
            ev["utcStart"] = json!(rfc3339_utc(start));
            ev["utcEnd"] = json!(rfc3339_utc(end));
        }
    }
}

// ── POST /cli/mail/calendar/update ──────────────────────────────────────

pub fn handle_update(body: &[u8]) -> CliResponse {
    let b = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match caller_from_body(&b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(token) = b.id.clone().filter(|s| !s.trim().is_empty()) else {
        return usage("update requires 'id' — an event id from 'k2 calendar events'");
    };
    if b.address.is_some() || b.calendar.is_some() {
        return usage("update takes the inbox from the event id — no 'address' or 'calendar'");
    }
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let (inbox, jmap_id) = match resolve_event(&project_id, &token, Level::Draft) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => update_with(&api, &inbox, &jmap_id, &token, &b),
        Err(r) => r,
    }
}

/// The STORED event's draft guard (CAL20): `Ok(event)` when draft level
/// may touch it; 404 when it isn't on the resolved account; 409 when it
/// has participants other than the owner.
fn stored_event_guard(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    jmap_id: &str,
    token: &str,
    what: &str,
) -> Result<Value, CliResponse> {
    let stored = match get_one(api, &inbox.account_id, jmap_id, FULL_PROPS) {
        Ok(Some(ev)) => ev,
        Ok(None) => return Err(event_not_found(token)),
        Err(e) => return Err(engine_err(e)),
    };
    let mut others = non_owner_participants(stored.get("participants"), &inbox.address);
    // An invitation someone else organized counts too, even when its
    // participant list was reduced to the owner.
    if let Some(org) = stored.get("organizerCalendarAddress").and_then(Value::as_str) {
        let org = strip_mailto(org);
        if !org.is_empty() && org != inbox.address.to_ascii_lowercase() && !others.contains(&org) {
            others.push(org);
        }
    }
    if !others.is_empty() {
        return Err(invites_need_send(&format!(
            "{what}: this event has {} participant(s) besides {}",
            others.len(),
            inbox.address
        )));
    }
    Ok(stored)
}

pub fn update_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    jmap_id: &str,
    token: &str,
    b: &WriteBody,
) -> CliResponse {
    let others = non_owner_participants(b.participants.as_ref(), &inbox.address);
    if !others.is_empty() {
        return invites_need_send(&format!(
            "this update names {} participant(s) besides {}",
            others.len(),
            inbox.address
        ));
    }
    if has_email_alert(b.alerts.as_ref()) {
        return email_alert_refused();
    }
    for (v, f, m) in [
        (&b.title, "title", MAX_TITLE),
        (&b.location, "location", MAX_LOCATION),
        (&b.notes, "notes", MAX_NOTES),
    ] {
        if let Err(r) = check_len(v, f, m) {
            return r;
        }
    }
    if b.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return usage("'title' can't be empty");
    }
    let tz = b.time_zone.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(z) = tz {
        if !valid_tz(z) {
            return usage(format!("'{z}' is not an IANA time zone name (e.g. America/Los_Angeles)"));
        }
    }
    let new_start = match b.start.as_deref() {
        Some(s) => match parse_event_instant(s, "start") {
            Ok(v) => Some(v),
            Err(e) => return usage(e),
        },
        None => None,
    };
    let new_end = match b.end.as_deref() {
        Some(s) => match parse_event_instant(s, "end") {
            Ok(v) => Some(v),
            Err(e) => return usage(e),
        },
        None => None,
    };
    // CAL20: the STORED event decides, not the request.
    let stored = match stored_event_guard(api, inbox, jmap_id, token, "update refused") {
        Ok(ev) => ev,
        Err(r) => return r,
    };
    let mut patch = serde_json::Map::new();
    if let Some(t) = b.title.as_deref() {
        patch.insert("title".into(), json!(t.trim()));
    }
    if let Some(l) = b.location.as_deref() {
        let l = l.trim();
        patch.insert(
            "locations".into(),
            if l.is_empty() {
                Value::Null
            } else {
                json!({ "l1": { "@type": "Location", "name": l } })
            },
        );
    }
    if let Some(n) = b.notes.as_deref() {
        patch.insert(
            "description".into(),
            if n.trim().is_empty() { json!("") } else { json!(n) },
        );
    }
    if let Some(a) = b.alerts.as_ref() {
        match shape_alerts(a) {
            Ok(v) => {
                patch.insert("alerts".into(), v);
            }
            Err(r) => return r,
        }
    }
    if new_start.is_some() || new_end.is_some() || tz.is_some() {
        let (old_start, old_end) = event_utc_span(&stored);
        let Some(old_start) = old_start else {
            return engine_err("the stored event has no resolvable start time");
        };
        let old_end = old_end.unwrap_or(old_start);
        let start = new_start.unwrap_or(old_start);
        // A new start alone keeps the duration.
        let end = match (new_start, new_end) {
            (_, Some(e)) => e,
            (Some(s), None) => s + (old_end - old_start),
            (None, None) => old_end,
        };
        if end <= start {
            return usage("'end' must be after 'start'");
        }
        let zone = tz.or_else(|| stored.get("timeZone").and_then(Value::as_str));
        let mut times = json!({});
        apply_times(&mut times, &start, &end, zone);
        if let Value::Object(m) = times {
            for (k, v) in m {
                patch.insert(k, v);
            }
        }
    }
    if patch.is_empty() {
        return usage("nothing to update — pass at least one of title/start/end/timeZone/location/notes");
    }
    let reply = match api.set_no_scheduling(
        &inbox.account_id,
        json!({ "update": { jmap_id: Value::Object(patch) }, "sendSchedulingMessages": false }),
    ) {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(msg) = set_error("CalendarEvent/set update", &reply, "notUpdated") {
        return set_failure(msg);
    }
    let event = get_one(api, &inbox.account_id, jmap_id, SUMMARY_PROPS)
        .ok()
        .flatten()
        .map(|e| shape_summary(&e, &inbox.address));
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "id": token,
        "event": event,
        "emailSent": false,
    }))
}

// ── POST /cli/mail/calendar/delete ──────────────────────────────────────

pub fn handle_delete(body: &[u8]) -> CliResponse {
    let b = match parse_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match caller_from_body(&b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(token) = b.id.clone().filter(|s| !s.trim().is_empty()) else {
        return usage("delete requires 'id' — an event id from 'k2 calendar events'");
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    let (inbox, jmap_id) = match resolve_event(&project_id, &token, Level::Draft) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match engine() {
        Ok(api) => delete_with(&api, &inbox, &jmap_id, &token),
        Err(r) => r,
    }
}

pub fn delete_with(api: &dyn CalendarApi, inbox: &CalInbox, jmap_id: &str, token: &str) -> CliResponse {
    // CAL20: a draft delete of a participant event would leave attendees
    // without a CANCEL — refuse on the STORED event.
    if let Err(r) = stored_event_guard(api, inbox, jmap_id, token, "delete refused") {
        return r;
    }
    let reply = match api.set_no_scheduling(
        &inbox.account_id,
        json!({ "destroy": [jmap_id], "sendSchedulingMessages": false }),
    ) {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(msg) = set_error("CalendarEvent/set destroy", &reply, "notDestroyed") {
        if msg.contains("notFound") {
            return event_not_found(token);
        }
        return engine_err(msg);
    }
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "id": token,
        "deleted": true,
        "emailSent": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    // ── Fixture JMAP (no network) ────────────────────────────────────────

    #[derive(Default)]
    struct Fake {
        calendars: Vec<Value>,
        /// jmap id → stored event (what CalendarEvent/get returns).
        events: HashMap<String, Value>,
        query_ids: Vec<String>,
        query_err: Option<String>,
        availability: Option<Result<Vec<Value>, String>>,
        state: String,
        changes: RefCell<VecDeque<Result<Value, String>>>,
        set_reply: Option<Value>,
        /// (method, account, args)
        calls: RefCell<Vec<(String, String, Value)>>,
    }

    impl Fake {
        fn record(&self, m: &str, account: &str, args: Value) {
            self.calls.borrow_mut().push((m.to_string(), account.to_string(), args));
        }
        fn calls_of(&self, m: &str) -> Vec<(String, Value)> {
            self.calls
                .borrow()
                .iter()
                .filter(|(n, _, _)| n == m)
                .map(|(_, a, v)| (a.clone(), v.clone()))
                .collect()
        }
    }

    impl CalendarApi for Fake {
        fn calendars(&self, account: &str) -> Result<Vec<Value>, String> {
            self.record("Calendar/get", account, Value::Null);
            Ok(self.calendars.clone())
        }
        fn query(&self, account: &str, args: Value) -> Result<Vec<String>, String> {
            self.record("CalendarEvent/query", account, args);
            match &self.query_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.query_ids.clone()),
            }
        }
        fn get(&self, account: &str, ids: &[String], props: Option<&[&str]>) -> Result<Value, String> {
            self.record("CalendarEvent/get", account, json!({ "ids": ids, "properties": props }));
            let list: Vec<Value> = ids.iter().filter_map(|i| self.events.get(i).cloned()).collect();
            Ok(json!({ "list": list, "state": self.state }))
        }
        fn changes(&self, account: &str, since: &str) -> Result<Value, String> {
            self.record("CalendarEvent/changes", account, json!({ "sinceState": since }));
            self.changes
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Ok(json!({ "newState": since, "created": [], "updated": [], "destroyed": [] })))
        }
        fn set_no_scheduling(&self, account: &str, args: Value) -> Result<Value, String> {
            self.record("CalendarEvent/set", account, args);
            Ok(self.set_reply.clone().unwrap_or_else(|| json!({})))
        }
        fn availability(&self, account: &str, s: &str, e: &str) -> Result<Vec<Value>, String> {
            self.record("Principal/getAvailability", account, json!({ "utcStart": s, "utcEnd": e }));
            self.availability
                .clone()
                .unwrap_or_else(|| Err("unknownMethod".to_string()))
        }
    }

    const OWNER: &str = "cal-owner@example.com";

    fn inbox() -> CalInbox {
        CalInbox { address: OWNER.to_string(), account_id: "acct-1".to_string() }
    }

    /// `CliResponse` isn't `Debug`: unwrap with the status + body shown.
    fn must<T>(r: Result<T, CliResponse>, what: &str) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what}: {} {}", e.status, e.body),
        }
    }

    fn body_of(resp: &CliResponse) -> Value {
        serde_json::from_str(&resp.body).expect("JSON body")
    }

    fn code_of(resp: &CliResponse) -> String {
        body_of(resp)["error"]["code"].as_str().unwrap_or("").to_string()
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn now() -> DateTime<Utc> {
        parse_instant("2026-10-07T12:00:00Z", "now").expect("now")
    }

    fn write_body(v: Value) -> WriteBody {
        serde_json::from_value(v).expect("write body")
    }

    fn plain_event(id: &str) -> Value {
        json!({
            "id": id,
            "calendarIds": { "cal-1": true },
            "title": "Prep",
            "start": "2026-10-08T16:00:00",
            "duration": "PT30M",
            "timeZone": "Etc/UTC",
            "utcStart": "2026-10-08T16:00:00Z",
            "utcEnd": "2026-10-08T16:30:00Z",
        })
    }

    fn assert_no_set(fake: &Fake) {
        assert!(
            fake.calls_of("CalendarEvent/set").is_empty(),
            "a refused draft write must never reach CalendarEvent/set"
        );
    }

    // ── DB fixtures (shared test DB; unique rows per test) ──────────────

    fn insert_project(tag: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/k2-cal-{tag}-{}", &id[..8]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("cal-{tag}-{}", &id[..8]), path],
        )
        .expect("insert project");
        (id, path)
    }

    fn seed_hosted(owner: &str, address: &str, level: &str, account: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at, primary_level) \
             VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100, ?5)",
            rusqlite::params![id, address, account, owner, level],
        )
        .expect("seed hosted");
        id
    }

    fn seed_linked(owner: &str, address: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, 'draft')",
            rusqlite::params![id, owner, address],
        )
        .expect("seed linked");
        id
    }

    fn grant(source: &str, inbox_id: &str, project_id: &str, level: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
             VALUES (?1, ?2, ?3, ?4, 100)",
            rusqlite::params![source, inbox_id, project_id, level],
        )
        .expect("seed grant");
    }

    fn cleanup(project_ids: &[&str]) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for p in project_ids {
            let _ = conn.execute(
                "DELETE FROM mail_inbox_grants WHERE project_id = ?1",
                rusqlite::params![p],
            );
            let _ = conn.execute(
                "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
                rusqlite::params![p],
            );
            let _ = conn.execute(
                "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
                rusqlite::params![p],
            );
            let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![p]);
        }
    }

    fn addr(tag: &str) -> String {
        format!("{tag}-{}@cal.example.com", &uuid::Uuid::new_v4().simple().to_string()[..10])
    }

    // ── JMAP envelope (loopback mock server) ────────────────────────────

    #[test]
    fn calendars_call_envelope_pins_using_account_and_no_scheduling() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server, NORMAL_SESSION_FIXTURE};
        let set_reply = json!({
            "methodResponses": [["CalendarEvent/set", { "accountId": "acct-u1", "created": {} }, "0"]]
        })
        .to_string();
        let cal_reply = json!({
            "methodResponses": [["Calendar/get", { "accountId": "acct-u1", "list": [], "state": "s1" }, "0"]]
        })
        .to_string();
        let avail_reply = json!({
            "methodResponses": [["Principal/getAvailability", { "list": [] }, "0"]]
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            set_reply,
            cal_reply,
            avail_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "test-key");
        // A caller-built accountId and sendSchedulingMessages:true must
        // both be overwritten by the wrapper.
        client
            .calendar_event_set_no_scheduling(
                "acct-u1",
                json!({ "accountId": "acct-evil", "sendSchedulingMessages": true, "destroy": ["e1"] }),
            )
            .expect("set");
        client.calendar_list("acct-u1").expect("calendar list");
        client
            .principal_availability("acct-u1", "2026-10-08T00:00:00Z", "2026-10-09T00:00:00Z")
            .expect("availability");
        let _session = rx.recv().expect("session request");
        let set_req = body_json(&rx.recv().expect("set request"));
        assert_eq!(
            set_req["using"],
            json!([
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:calendars",
                "urn:ietf:params:jmap:principals"
            ])
        );
        let call = &set_req["methodCalls"][0];
        assert_eq!(call[0], "CalendarEvent/set");
        assert_eq!(call[1]["accountId"], "acct-u1", "never a client accountId (CAL16)");
        assert_eq!(call[1]["sendSchedulingMessages"], false, "draft never schedules (CAL4)");
        let cal_req = body_json(&rx.recv().expect("calendar request"));
        assert_eq!(cal_req["methodCalls"][0][0], "Calendar/get");
        let props = cal_req["methodCalls"][0][1]["properties"].as_array().expect("properties");
        assert!(props.contains(&json!("isDefault")), "isDefault must be asked for: {props:?}");
        let av_req = body_json(&rx.recv().expect("availability request"));
        assert_eq!(av_req["methodCalls"][0][0], "Principal/getAvailability");
        assert_eq!(av_req["methodCalls"][0][1]["id"], "acct-u1");
        assert_eq!(av_req["methodCalls"][0][1]["accountId"], "acct-u1");
        assert_eq!(av_req["methodCalls"][0][1]["utcStart"], "2026-10-08T00:00:00Z");
    }

    // ── Inbox resolution (CAL19) + gating ───────────────────────────────

    #[test]
    fn no_address_uses_the_single_owned_hosted_inbox_zero_and_many_refused() {
        let (p, _) = insert_project("prim");
        // 0 owned → 404 not_found.
        let r = resolve_inbox(&p, None, Level::Read).expect_err("none owned");
        assert!(r.status.starts_with("404"), "{}", r.status);
        assert_eq!(code_of(&r), "not_found");
        // 1 owned → it, with the account from the ROW.
        let a1 = addr("one");
        seed_hosted(&p, &a1, "send", Some("acct-row-1"));
        let got = must(resolve_inbox(&p, None, Level::Read), "one owned");
        assert_eq!(got, CalInbox { address: a1.clone(), account_id: "acct-row-1".into() });
        // >1 owned → 400 usage naming both.
        let a2 = addr("two");
        seed_hosted(&p, &a2, "send", Some("acct-row-2"));
        let r = resolve_inbox(&p, None, Level::Read).expect_err("two owned");
        assert!(r.status.starts_with("400"));
        assert_eq!(code_of(&r), "usage");
        let hint = body_of(&r)["error"]["hint"].as_str().unwrap_or("").to_string();
        assert!(hint.contains(&a1) && hint.contains(&a2), "{hint}");
        // Explicit address still works with two.
        assert_eq!(
            must(resolve_inbox(&p, Some(&a2), Level::Read), "explicit").account_id,
            "acct-row-2"
        );
        cleanup(&[&p]);
    }

    #[test]
    fn linked_inbox_is_hosted_only_but_only_after_access_passes() {
        let (owner, _) = insert_project("lnk-own");
        let (other, _) = insert_project("lnk-oth");
        let linked = addr("linked");
        seed_linked(&owner, &linked);
        let r = resolve_inbox(&owner, Some(&linked), Level::Read).expect_err("linked");
        assert!(r.status.starts_with("400"));
        assert_eq!(code_of(&r), "hosted_only");
        // A workspace with no access never learns it is linked.
        let r = resolve_inbox(&other, Some(&linked), Level::Read).expect_err("masked");
        assert!(r.status.starts_with("404"));
        assert_eq!(code_of(&r), "not_found");
        cleanup(&[&owner, &other]);
    }

    #[test]
    fn read_grant_reads_but_cannot_draft_and_no_grant_is_masked() {
        let (owner, _) = insert_project("g-own");
        let (reader, _) = insert_project("g-read");
        let (drafter, _) = insert_project("g-draft");
        let (stranger, _) = insert_project("g-none");
        let a = addr("granted");
        let row = seed_hosted(&owner, &a, "send", Some("acct-g"));
        grant("hosted", &row, &reader, "read");
        grant("hosted", &row, &drafter, "draft");
        assert!(resolve_inbox(&reader, Some(&a), Level::Read).is_ok());
        let r = resolve_inbox(&reader, Some(&a), Level::Draft).expect_err("read can't write");
        assert_eq!(code_of(&r), "not_found", "a read grant answers the masked not_found");
        assert!(resolve_inbox(&drafter, Some(&a), Level::Draft).is_ok());
        for need in [Level::Read, Level::Draft] {
            let r = resolve_inbox(&stranger, Some(&a), need).expect_err("no grant");
            assert!(r.status.starts_with("404"));
            assert_eq!(code_of(&r), "not_found");
        }
        // The owner's own primary_level applies too (read-only owner).
        let (ro_owner, _) = insert_project("g-ro");
        let ro = addr("readonly");
        seed_hosted(&ro_owner, &ro, "read", Some("acct-ro"));
        assert!(resolve_inbox(&ro_owner, None, Level::Read).is_ok());
        assert_eq!(
            code_of(&resolve_inbox(&ro_owner, None, Level::Draft).expect_err("ro owner")),
            "not_found"
        );
        cleanup(&[&owner, &reader, &drafter, &stranger, &ro_owner]);
    }

    #[test]
    fn hosted_row_without_account_fails_loudly() {
        let (p, _) = insert_project("noacct");
        let a = addr("degraded");
        seed_hosted(&p, &a, "send", None);
        let r = resolve_inbox(&p, Some(&a), Level::Read).expect_err("degraded mint");
        assert!(r.status.starts_with("502"));
        cleanup(&[&p]);
    }

    #[test]
    fn event_ids_carry_their_inbox_and_foreign_ids_are_masked() {
        let tok = encode_event_id("a@example.com", "Jx9");
        assert!(tok.starts_with("ev_"));
        assert_eq!(decode_event_id(&tok), Some(("a@example.com".into(), "Jx9".into())));
        for bad in ["", "ev_", "m_abc", "ev_!!!", &format!("ev_{}", B64URL.encode("noline"))] {
            assert_eq!(decode_event_id(bad), None, "{bad}");
        }
        let (owner, _) = insert_project("id-own");
        let (other, _) = insert_project("id-oth");
        let a = addr("ids");
        seed_hosted(&owner, &a, "send", Some("acct-ids"));
        let tok = encode_event_id(&a, "e1");
        let (inbox, jid) = must(resolve_event(&owner, &tok, Level::Read), "own");
        assert_eq!((inbox.account_id.as_str(), jid.as_str()), ("acct-ids", "e1"));
        let r = resolve_event(&other, &tok, Level::Read).expect_err("foreign");
        assert!(r.status.starts_with("404"));
        let hint = body_of(&r)["error"]["hint"].as_str().unwrap_or("").to_string();
        assert!(hint.contains("no event"), "event-level mask, not the address: {hint}");
        assert!(!hint.contains(&a), "the inbox address must not leak: {hint}");
        assert_eq!(code_of(&resolve_event(&owner, "junk", Level::Read).expect_err("bad")), "usage");
        cleanup(&[&owner, &other]);
    }

    #[test]
    fn an_event_id_not_on_the_resolved_account_is_not_found() {
        // The admin key can read ANY account; the id must exist on the
        // RESOLVED one (CAL16) — the fake account has no such event.
        let fake = Fake::default();
        let r = show_with(&fake, &inbox(), "other-acct-event", "ev_x");
        assert!(r.status.starts_with("404"), "{}", r.body);
        let gets = fake.calls_of("CalendarEvent/get");
        assert_eq!(gets[0].0, "acct-1", "looked up on the resolved account only");
        // Same for update/delete: masked, never a set.
        let b = write_body(json!({ "project": "x", "id": "ev_x", "title": "t" }));
        assert!(update_with(&fake, &inbox(), "nope", "ev_x", &b).status.starts_with("404"));
        assert!(delete_with(&fake, &inbox(), "nope", "ev_x").status.starts_with("404"));
        assert_no_set(&fake);
    }

    #[test]
    fn unknown_calendar_is_rejected_with_the_inbox_calendars() {
        let fake = Fake {
            calendars: vec![json!({ "id": "cal-1", "name": "Work", "isDefault": true })],
            ..Default::default()
        };
        let r = events_with(&fake, &inbox(), &params(&[("calendar", "cal-of-someone-else")]), now());
        assert_eq!(code_of(&r), "usage");
        assert!(r.body.contains("Work (cal-1)"), "{}", r.body);
        assert!(fake.calls_of("CalendarEvent/query").is_empty());
        // By name works and becomes the singular inCalendar filter.
        let r = events_with(&fake, &inbox(), &params(&[("calendar", "work")]), now());
        assert!(r.status.starts_with("200"), "{}", r.body);
        let q = &fake.calls_of("CalendarEvent/query")[0].1;
        assert_eq!(q["filter"]["inCalendar"], "cal-1");
    }

    // ── Draft guard (CAL4 / CAL20) ──────────────────────────────────────

    fn create_body(extra: Value) -> WriteBody {
        let mut b = json!({
            "project": "x",
            "title": "Prep call",
            "start": "2026-10-08T09:00:00-07:00",
            "end": "2026-10-08T09:30:00-07:00",
        });
        if let (Value::Object(m), Value::Object(e)) = (&mut b, extra) {
            m.extend(e);
        }
        write_body(b)
    }

    #[test]
    fn create_with_other_participants_or_email_alert_is_refused_before_any_call() {
        let fake = Fake {
            calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
            ..Default::default()
        };
        let r = create_with(
            &fake,
            &inbox(),
            &create_body(json!({ "participants": {
                "p1": { "calendarAddress": "mailto:guest@example.org" }
            }})),
        );
        assert!(r.status.starts_with("409"), "{}", r.body);
        assert_eq!(code_of(&r), "invites_need_send_level");
        let r = create_with(&fake, &inbox(), &create_body(json!({ "participants": ["guest@example.org"] })));
        assert_eq!(code_of(&r), "invites_need_send_level");
        let r = create_with(
            &fake,
            &inbox(),
            &create_body(json!({ "alerts": [{ "action": "email", "offset": "-PT15M" }] })),
        );
        assert_eq!(code_of(&r), "email_alerts_need_send_level");
        assert_no_set(&fake);
        assert!(fake.calls.borrow().is_empty(), "refused before any JMAP call");
    }

    #[test]
    fn create_owner_only_participant_is_fine_and_sets_never_schedule() {
        let fake = Fake {
            calendars: vec![
                json!({ "id": "cal-2", "name": "Other", "sortOrder": 0 }),
                json!({ "id": "cal-1", "name": "Main", "isDefault": true }),
            ],
            set_reply: Some(json!({ "created": { "k2new": { "id": "new-1" } } })),
            ..Default::default()
        };
        let r = create_with(
            &fake,
            &inbox(),
            &create_body(json!({ "participants": [format!("mailto:{}", OWNER.to_uppercase())] })),
        );
        assert!(r.status.starts_with("200"), "{}", r.body);
        let b = body_of(&r);
        assert_eq!(b["id"], json!(encode_event_id(OWNER, "new-1")));
        assert_eq!(b["emailSent"], false);
        let sets = fake.calls_of("CalendarEvent/set");
        assert_eq!(sets.len(), 1);
        let (account, args) = &sets[0];
        assert_eq!(account, "acct-1");
        assert_eq!(args["sendSchedulingMessages"], false);
        let ev = &args["create"]["k2new"];
        assert_eq!(ev["calendarIds"], json!({ "cal-1": true }), "the isDefault calendar");
        // No zone → Etc/UTC wall time + duration, never an offset or `end`.
        assert_eq!(ev["timeZone"], "Etc/UTC");
        assert_eq!(ev["start"], "2026-10-08T16:00:00");
        assert_eq!(ev["duration"], "PT30M");
        assert!(ev.get("end").is_none() && ev.get("participants").is_none());
    }

    #[test]
    fn create_with_a_zone_sends_utc_start_end_for_the_server_to_convert() {
        let fake = Fake {
            calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
            set_reply: Some(json!({ "created": { "k2new": { "id": "new-2" } } })),
            ..Default::default()
        };
        let r = create_with(
            &fake,
            &inbox(),
            &create_body(json!({ "timeZone": "America/Los_Angeles", "location": "Room 2",
                                 "notes": "agenda", "alerts": [{ "offset": "-PT10M" }] })),
        );
        assert!(r.status.starts_with("200"), "{}", r.body);
        let args = &fake.calls_of("CalendarEvent/set")[0].1;
        let ev = &args["create"]["k2new"];
        assert_eq!(ev["timeZone"], "America/Los_Angeles");
        assert_eq!(ev["utcStart"], "2026-10-08T16:00:00Z");
        assert_eq!(ev["utcEnd"], "2026-10-08T16:30:00Z");
        assert!(ev.get("start").is_none());
        assert_eq!(ev["locations"]["l1"]["name"], "Room 2");
        assert_eq!(ev["alerts"]["a1"]["action"], "display");
        // Bad inputs are usage errors with no set.
        let fake2 = Fake {
            calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
            ..Default::default()
        };
        for bad in [
            json!({ "timeZone": "Not a zone!" }),
            json!({ "start": "2026-10-08 09:00" }),
            json!({ "end": "2026-10-08T08:00:00-07:00" }),
            json!({ "title": "  " }),
        ] {
            let r = create_with(&fake2, &inbox(), &create_body(bad.clone()));
            assert_eq!(code_of(&r), "usage", "{bad}: {}", r.body);
        }
        assert_no_set(&fake2);
    }

    #[test]
    fn update_and_delete_refuse_a_stored_event_with_other_participants() {
        let mut stored = plain_event("e-inv");
        stored["participants"] = json!({
            "o": { "calendarAddress": format!("mailto:{OWNER}"), "roles": { "owner": true } },
            "g": { "calendarAddress": "mailto:guest@example.org", "roles": { "attendee": true } }
        });
        let mut organized_elsewhere = plain_event("e-org");
        organized_elsewhere["organizerCalendarAddress"] = json!("mailto:boss@example.org");
        let fake = Fake {
            events: HashMap::from([
                ("e-inv".to_string(), stored),
                ("e-org".to_string(), organized_elsewhere),
            ]),
            ..Default::default()
        };
        // The REQUEST has no participants — the stored event decides.
        let b = write_body(json!({ "project": "x", "id": "ev_t", "title": "renamed" }));
        for id in ["e-inv", "e-org"] {
            let r = update_with(&fake, &inbox(), id, "ev_t", &b);
            assert!(r.status.starts_with("409"), "{id}: {}", r.body);
            assert_eq!(code_of(&r), "invites_need_send_level");
            let r = delete_with(&fake, &inbox(), id, "ev_t");
            assert_eq!(code_of(&r), "invites_need_send_level", "{id}: delete would skip the CANCEL");
        }
        assert_no_set(&fake);
    }

    #[test]
    fn update_patches_only_given_fields_and_keeps_duration_on_a_new_start() {
        let fake = Fake {
            events: HashMap::from([("e1".to_string(), plain_event("e1"))]),
            ..Default::default()
        };
        let b = write_body(json!({ "project": "x", "id": "ev_t", "start": "2026-10-09T10:00:00Z",
                                   "location": "" }));
        let r = update_with(&fake, &inbox(), "e1", "ev_t", &b);
        assert!(r.status.starts_with("200"), "{}", r.body);
        let args = &fake.calls_of("CalendarEvent/set")[0].1;
        assert_eq!(args["sendSchedulingMessages"], false);
        let patch = &args["update"]["e1"];
        assert_eq!(patch["start"], "2026-10-09T10:00:00");
        assert_eq!(patch["duration"], "PT30M", "duration kept");
        assert_eq!(patch["locations"], Value::Null, "empty location clears");
        assert!(patch.get("title").is_none(), "untouched fields stay out of the patch");
        // An email alert in an update is refused too.
        let b = write_body(json!({ "project": "x", "id": "ev_t",
                                   "alerts": { "x": { "action": "email" } } }));
        assert_eq!(
            code_of(&update_with(&fake, &inbox(), "e1", "ev_t", &b)),
            "email_alerts_need_send_level"
        );
    }

    #[test]
    fn delete_of_an_owner_only_event_destroys_without_scheduling() {
        let fake = Fake {
            events: HashMap::from([("e1".to_string(), plain_event("e1"))]),
            set_reply: Some(json!({ "destroyed": ["e1"] })),
            ..Default::default()
        };
        let r = delete_with(&fake, &inbox(), "e1", "ev_t");
        assert!(r.status.starts_with("200"), "{}", r.body);
        let args = &fake.calls_of("CalendarEvent/set")[0].1;
        assert_eq!(args["destroy"], json!(["e1"]));
        assert_eq!(args["sendSchedulingMessages"], false);
    }

    #[test]
    fn every_write_body_rejects_an_account_id() {
        // deny_unknown_fields: there is no way to smuggle an accountId.
        let err = serde_json::from_value::<WriteBody>(json!({ "project": "x", "accountId": "acct-evil" }))
            .expect_err("accountId must be refused");
        assert!(err.to_string().contains("accountId"), "{err}");
        assert_eq!(
            code_of(&handle_create(br#"{"project":"x","accountId":"a"}"#)),
            "usage"
        );
    }

    // ── Reads ───────────────────────────────────────────────────────────

    #[test]
    fn events_query_uses_offset_free_window_expansion_and_stable_ids() {
        let mut inst = plain_event("syn-7");
        inst["baseEventId"] = json!("master-1");
        inst["recurrenceId"] = json!("2026-10-08T16:00:00");
        let fake = Fake {
            query_ids: vec!["syn-7".into(), "syn-8".into()],
            events: HashMap::from([("syn-7".to_string(), inst)]),
            ..Default::default()
        };
        let p = params(&[("start", "2026-10-08T00:00:00-07:00"), ("end", "2026-10-10"), ("limit", "1")]);
        let r = events_with(&fake, &inbox(), &p, now());
        assert!(r.status.starts_with("200"), "{}", r.body);
        let q = &fake.calls_of("CalendarEvent/query")[0].1;
        // Stalwart drops offsets in after/before: UTC wall time + timeZone.
        assert_eq!(q["filter"]["after"], "2026-10-08T07:00:00");
        assert_eq!(q["filter"]["before"], "2026-10-10T00:00:00");
        assert_eq!(q["timeZone"], "Etc/UTC");
        assert_eq!(q["expandRecurrences"], true);
        assert_eq!(q["limit"], 2, "page + 1 to detect truncation");
        let b = body_of(&r);
        assert_eq!(b["truncated"], true);
        let ev = &b["events"][0];
        assert_eq!(ev["id"], json!(encode_event_id(OWNER, "master-1")), "series id, not synthetic");
        assert_eq!(ev["start"], "2026-10-08T16:00:00Z");
        assert_eq!(ev["end"], "2026-10-08T16:30:00Z");
        assert_eq!(ev["recurring"], true);
        assert_eq!(b["start"], "2026-10-08T07:00:00Z");
    }

    #[test]
    fn window_defaults_caps_and_errors() {
        let (s, e) = must(window(&params(&[]), now()), "default");
        assert_eq!((s, e - s), (now(), Duration::days(7)));
        for (p, want) in [
            (vec![("start", "2026-10-08"), ("end", "2026-10-08")], "after"),
            (vec![("start", "2026-01-01"), ("end", "2027-06-01")], "366"),
            (vec![("start", "yesterday")], "RFC 3339"),
        ] {
            let r = window(&params(&p), now()).expect_err("bad window");
            assert_eq!(code_of(&r), "usage");
            assert!(r.body.contains(want), "{p:?}: {}", r.body);
        }
    }

    /// CAL18: the identity stamp overwrites `from` (and project) on every
    /// scoped GET — over TCP (dispatcher) and over the cell UDS (which
    /// adds `cell_session_id` then calls the same stamp). The calendar
    /// window rides `start`/`end`, so it must survive both untouched.
    #[test]
    fn window_survives_the_identity_stamp_over_tcp_and_cell_uds() {
        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: uuid::Uuid::new_v4().to_string(),
            agent_address: "agent".to_string(),
        };
        let client = params(&[
            ("start", "2026-10-08T00:00:00Z"),
            ("end", "2026-10-09T00:00:00Z"),
            ("from", "2026-01-01T00:00:00Z"),
            ("project", "/tmp/whatever"),
        ]);
        // TCP: the dispatcher restores `from` ONLY for messages/wait.
        let mut tcp = client.clone();
        crate::caller_workspace::stamp_principal(&mut tcp, &principal);
        // Cell UDS: cell_session_id first, then the same stamp.
        let mut uds = client.clone();
        uds.insert("cell_session_id".into(), "sess-1".into());
        crate::caller_workspace::stamp_principal(&mut uds, &principal);
        for (label, p) in [("tcp", &tcp), ("uds", &uds)] {
            assert_ne!(
                p.get("from").map(String::as_str),
                Some("2026-01-01T00:00:00Z"),
                "{label}: the stamp clobbers `from` — why the wire uses start/end"
            );
            let fake = Fake::default();
            let r = events_with(&fake, &inbox(), p, now());
            assert!(r.status.starts_with("200"), "{label}: {}", r.body);
            let q = &fake.calls_of("CalendarEvent/query")[0].1;
            assert_eq!(q["filter"]["after"], "2026-10-08T00:00:00", "{label}");
            assert_eq!(q["filter"]["before"], "2026-10-09T00:00:00", "{label}");
        }
    }

    #[test]
    fn list_shapes_calendars() {
        let fake = Fake {
            calendars: vec![json!({ "id": "c1", "name": "Main", "isDefault": true, "color": "#0a0" })],
            ..Default::default()
        };
        let b = body_of(&list_with(&fake, &inbox()));
        assert_eq!(b["calendars"][0], json!({
            "id": "c1", "name": "Main", "description": null, "color": "#0a0",
            "isDefault": true, "timeZone": null
        }));
        assert_eq!(fake.calls_of("Calendar/get")[0].0, "acct-1");
    }

    #[test]
    fn show_wraps_full_event_with_participants() {
        let mut ev = plain_event("e1");
        ev["description"] = json!("notes");
        ev["participants"] = json!({ "p": { "calendarAddress": "mailto:Guest@Example.org",
                                            "participationStatus": "accepted" } });
        let fake = Fake { events: HashMap::from([("e1".to_string(), ev)]), ..Default::default() };
        let b = body_of(&show_with(&fake, &inbox(), "e1", "ev_t"));
        assert_eq!(b["event"]["participantList"][0]["email"], "guest@example.org");
        assert_eq!(b["event"]["description"], "notes");
        let props = &fake.calls_of("CalendarEvent/get")[0].1["properties"];
        assert!(props.as_array().expect("props").contains(&json!("utcStart")));
    }

    #[test]
    fn freebusy_prefers_server_availability_and_merges() {
        let fake = Fake {
            availability: Some(Ok(vec![
                json!({ "utcStart": "2026-10-08T09:00:00Z", "utcEnd": "2026-10-08T10:00:00Z", "busyStatus": "tentative" }),
                json!({ "utcStart": "2026-10-08T09:30:00Z", "utcEnd": "2026-10-08T11:00:00Z", "busyStatus": "confirmed" }),
                json!({ "utcStart": "2026-10-07T23:00:00Z", "utcEnd": "2026-10-08T01:00:00Z", "busyStatus": "unavailable" }),
            ])),
            ..Default::default()
        };
        let p = params(&[("start", "2026-10-08"), ("end", "2026-10-09")]);
        let b = body_of(&freebusy_with(&fake, &inbox(), &p, now()));
        assert_eq!(b["source"], "availability");
        assert_eq!(b["busy"], json!([
            { "start": "2026-10-08T00:00:00Z", "end": "2026-10-08T01:00:00Z", "status": "unavailable" },
            { "start": "2026-10-08T09:00:00Z", "end": "2026-10-08T11:00:00Z", "status": "busy" },
        ]));
        assert!(fake.calls_of("CalendarEvent/query").is_empty());
    }

    #[test]
    fn freebusy_falls_back_to_events_skipping_free_cancelled_secret() {
        let mut free = plain_event("f");
        free["freeBusyStatus"] = json!("free");
        let mut cancelled = plain_event("c");
        cancelled["status"] = json!("cancelled");
        let mut secret = plain_event("s");
        secret["privacy"] = json!("secret");
        let fake = Fake {
            availability: Some(Err("Principal/getAvailability: JMAP error 'forbidden'".into())),
            query_ids: vec!["b".into(), "f".into(), "c".into(), "s".into()],
            events: HashMap::from([
                ("b".to_string(), plain_event("b")),
                ("f".to_string(), free),
                ("c".to_string(), cancelled),
                ("s".to_string(), secret),
            ]),
            ..Default::default()
        };
        let p = params(&[("start", "2026-10-08"), ("end", "2026-10-09")]);
        let b = body_of(&freebusy_with(&fake, &inbox(), &p, now()));
        assert_eq!(b["source"], "events");
        assert_eq!(b["busy"], json!([
            { "start": "2026-10-08T16:00:00Z", "end": "2026-10-08T16:30:00Z", "status": "busy" }
        ]));
        assert!(!b.to_string().contains("Prep"), "titles never leave freebusy");
    }

    #[test]
    fn too_many_expansions_is_a_usage_error() {
        let fake = Fake {
            query_err: Some("CalendarEvent/query: JMAP error 'invalidArguments': exceeds the server limit".into()),
            ..Default::default()
        };
        let r = events_with(&fake, &inbox(), &params(&[]), now());
        assert_eq!(code_of(&r), "usage");
    }

    // ── wait ────────────────────────────────────────────────────────────

    #[test]
    fn wait_times_out_with_the_state_and_reports_changes() {
        let fake = Fake { state: "s0".into(), ..Default::default() };
        // A fake clock the fake sleep advances.
        let clock = std::cell::Cell::new(0u64);
        let mut elapsed = || clock.get();
        let mut sleep = |d: std::time::Duration| clock.set(clock.get() + d.as_secs());
        let r = wait_with(&fake, &inbox(), None, 5, &mut elapsed, &mut sleep);
        let b = body_of(&r);
        assert_eq!(b["timedOut"], true);
        assert_eq!(b["state"], "s0", "state read first when none given");
        assert_eq!(fake.calls_of("CalendarEvent/get")[0].1["ids"], json!([]));
        assert!(clock.get() <= 5, "never sleeps past the budget: {}", clock.get());
        assert!(fake.calls_of("CalendarEvent/changes").len() >= 2, "it polled");

        let fake = Fake::default();
        fake.changes.borrow_mut().push_back(Ok(json!({
            "newState": "s1", "created": ["n1"], "updated": [], "destroyed": [], "hasMoreChanges": true
        })));
        fake.changes.borrow_mut().push_back(Ok(json!({
            "newState": "s2", "created": [], "updated": ["u1"], "destroyed": ["d1"]
        })));
        let mut elapsed = || 0u64;
        let mut sleep = |_d: std::time::Duration| {};
        let b = body_of(&wait_with(&fake, &inbox(), Some("s0"), 30, &mut elapsed, &mut sleep));
        assert_eq!(b["timedOut"], false);
        assert_eq!(b["state"], "s2");
        assert_eq!(b["created"], json!([encode_event_id(OWNER, "n1")]));
        assert_eq!(b["updated"], json!([encode_event_id(OWNER, "u1")]));
        assert_eq!(b["destroyed"], json!([encode_event_id(OWNER, "d1")]));
        assert_eq!(fake.calls_of("CalendarEvent/changes")[0].1["sinceState"], "s0");

        let fake = Fake::default();
        fake.changes.borrow_mut().push_back(Err(
            "CalendarEvent/changes: JMAP error 'cannotCalculateChanges': gone".into(),
        ));
        let r = wait_with(&fake, &inbox(), Some("old"), 30, &mut elapsed, &mut sleep);
        assert!(r.status.starts_with("409"));
        assert_eq!(code_of(&r), "state_expired");
    }

    #[test]
    fn wait_timeout_bounds() {
        assert_eq!(must(parse_wait_timeout(&params(&[])), "default"), 300);
        assert_eq!(must(parse_wait_timeout(&params(&[("timeout", "900")])), "max"), 900);
        for bad in ["0", "901", "x"] {
            let r = parse_wait_timeout(&params(&[("timeout", bad)])).expect_err(bad);
            assert_eq!(code_of(&r), "usage", "{bad}");
        }
    }

    // ── Owner switch + routing ──────────────────────────────────────────

    #[test]
    fn calendars_disabled_policy_answers_409_on_every_verb() {
        let _g = crate::mail::mail_server_test_lock();
        let (p, path) = insert_project("dis");
        let a = addr("dis");
        seed_hosted(&p, &a, "send", Some("acct-dis"));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear");
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, dav_policy_json, updated_at) \
                 VALUES (1, 'running', '0.16.20', 'mail.example.com', \
                 '{\"calendars\":\"off\",\"files\":\"off\",\"appliedAt\":1,\"backfilledAt\":1}', 1)",
                [],
            )
            .expect("seed policy");
        }
        let q = params(&[("project", &path), ("id", &encode_event_id(&a, "e1"))]);
        for (label, r) in [
            ("list", handle_list(&q)),
            ("events", handle_events(&q)),
            ("show", handle_show(&q)),
            ("freebusy", handle_freebusy(&q)),
            ("wait", handle_wait(&q)),
        ] {
            assert!(r.status.starts_with("409"), "{label}: {} {}", r.status, r.body);
            assert_eq!(code_of(&r), "calendars_disabled", "{label}");
            assert!(r.body.contains("k2 hostmail calendar enable"), "{label}");
        }
        let body = json!({ "project": path, "title": "t", "start": "2026-10-08T09:00:00Z",
                           "end": "2026-10-08T10:00:00Z" })
        .to_string();
        assert_eq!(code_of(&handle_create(body.as_bytes())), "calendars_disabled");
        let body = json!({ "project": path, "id": encode_event_id(&a, "e1") }).to_string();
        assert_eq!(code_of(&handle_delete(body.as_bytes())), "calendars_disabled");
        assert_eq!(code_of(&handle_update(body.as_bytes())), "calendars_disabled");
        // Calendars on (or never applied) → past the switch; with the
        // server row gone the engine answers 503 not_ready.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear");
        }
        let r = handle_list(&q);
        assert!(r.status.starts_with("503"), "{} {}", r.status, r.body);
        cleanup(&[&p]);
    }

    #[test]
    fn routes_are_registered_get_post_and_405() {
        let empty = HashMap::new();
        for p in ["/cli/mail/calendar/create", "/cli/mail/calendar/update", "/cli/mail/calendar/delete"] {
            let r = crate::mail_routes::dispatch(p, &empty).expect("mail family");
            assert!(r.status.starts_with("405"), "GET {p} must 405, got {}", r.status);
            assert!(crate::routes::route_policy::post_allowed(p), "{p} on the POST allowlist");
            assert!(!crate::mail_routes::is_mail_manage_surface(p), "{p} is an agent verb");
            assert!(!crate::mail_routes::is_owner_level_mutation(p), "{p} is an agent verb");
            assert!(crate::session_token::is_agent_verb(p), "{p} rides the /cli/mail/ agent prefix");
        }
        for p in [
            "/cli/mail/calendar/list",
            "/cli/mail/calendar/events",
            "/cli/mail/calendar/show",
            "/cli/mail/calendar/freebusy",
            "/cli/mail/calendar/wait",
        ] {
            let r = crate::mail_routes::dispatch(p, &empty).expect("mail family");
            // Served (not the family 404/405): no identity → usage.
            assert_eq!(code_of(&r), "usage", "GET {p} reaches its handler: {} {}", r.status, r.body);
            assert!(!crate::routes::route_policy::post_allowed(p), "{p} is GET-only");
            assert!(!crate::mail_routes::is_mail_manage_surface(p));
            assert!(!crate::mail_routes::is_owner_level_mutation(p));
            assert!(crate::session_token::is_agent_verb(p));
        }
        // POST dispatch reaches the handlers (bad JSON → usage, not 404).
        for p in ["/cli/mail/calendar/create", "/cli/mail/calendar/update", "/cli/mail/calendar/delete"] {
            let r = crate::mail_routes::dispatch_post(p, b"not json");
            assert_eq!(code_of(&r), "usage", "{p}: {}", r.body);
        }
    }

    // ── Pure helpers ────────────────────────────────────────────────────

    #[test]
    fn durations_round_trip() {
        for (s, secs) in [("PT30M", 1800), ("PT1H30M", 5400), ("P1D", 86_400), ("P1DT2H", 93_600),
                          ("P2W", 1_209_600), ("PT0S", 0), ("PT45S", 45)] {
            assert_eq!(parse_duration_secs(s), Some(secs), "{s}");
        }
        for bad in ["", "P", "PT", "1H", "PT1X", "P1H"] {
            assert_eq!(parse_duration_secs(bad), None, "{bad}");
        }
        assert_eq!(format_duration(1800), "PT30M");
        assert_eq!(format_duration(93_600), "P1DT2H");
        assert_eq!(format_duration(86_400), "P1D");
        assert_eq!(format_duration(0), "PT0S");
    }

    #[test]
    fn participant_owner_matching_is_case_and_mailto_insensitive() {
        let parts = json!({
            "a": { "calendarAddress": "MAILTO:Cal-Owner@Example.com" },
            "b": { "sendTo": { "imip": "mailto:x@example.org" } },
            "c": { "name": "no address" }
        });
        assert_eq!(
            non_owner_participants(Some(&parts), OWNER),
            vec!["x@example.org".to_string(), "(participant without an address)".to_string()]
        );
        assert!(non_owner_participants(None, OWNER).is_empty());
        assert!(has_email_alert(Some(&json!([{ "action": "EMAIL" }]))));
        assert!(!has_email_alert(Some(&json!({ "a": { "action": "display" } }))));
    }
}
