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
//! `not_found`, more than 1 → 400 `usage` listing them. Event ids are
//! opaque `ev_…` tokens carrying their inbox address, exactly like mail's
//! `m_…` message ids (capped at 1 KiB before decoding, MCS-A23).
//!
//! **Linked inboxes (MCS3/MCS4, prd-app-mail-calendar-surfaces-v1
//! §3.1/§3.4):** [`resolve_target`] routes a LINKED inbox — only AFTER the
//! access gate passed, so a linked inbox the caller can't see still
//! answers the masked `not_found` (CAL19) — to its calendar provider
//! behind [`crate::mail::calendar_provider::CalendarProvider`]: Microsoft
//! 365 → Microsoft Graph ([`crate::mail::calendar_graph`]), Google →
//! the Google Calendar API ([`crate::mail::calendar_google`]); same verbs,
//! same wire shapes plus `source`. Anything a linked inbox can't do
//! answers 409 `capability_missing` `{cap, reason, fix}`: an app-password
//! IMAP inbox (`backend_unsupported`), a link consented for mail only
//! (`scope_not_granted`, fix `k2 mail link upgrade … --features
//! calendar-read`), a write on a read-only link (`read_only_link`,
//! calendar S1), or the owner's `linkedCalendars` switch off
//! (`owner_disabled`). The hosted path below is unchanged (MCS-A16:
//! [`CalendarApi`] stays the Stalwart internals).
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
//! **Shared calendars (S4):** a grant on inbox A also covers the calendars
//! other hosted mailboxes shared TO A — directly or through a group A is
//! in — so an assistant with a grant on a person's inbox sees what that
//! person sees. K2 enforces the rights itself (the admin key bypasses
//! Stalwart's ACLs): [`shared_calendars`] reads each peer account's
//! `shareWith`, matches A's account id and `memberGroupIds`, and takes
//! the share ∩ the grant level — read = `mayReadItems`, free/busy =
//! `mayReadFreeBusy` (only in `freebusy`), write = `mayWriteAll` + draft,
//! delete = `mayDelete` (editor) + draft. Shared event ids are
//! `ev_<b64(A \n jmap-id \n owner address)>`; the owner's account comes
//! from K2's address rows and every use re-checks the live share. The
//! draft guard applies against the calendar's owner.
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
use crate::mail::calendar_share::{HostedAddr, ShareRights};
use crate::mail::jmap::StalwartClient;
use crate::mail::messages::ReadError;

// ── Limits ──────────────────────────────────────────────────────────────

/// Default `events`/`freebusy` window when `end` is absent.
const DEFAULT_WINDOW_DAYS: i64 = 7;
/// Longest window one call may ask for (recurrence expansion is bounded
/// by the window, so this also bounds the server's work).
const MAX_WINDOW_DAYS: i64 = 366;
/// `events` page size: default and hard cap.
pub(crate) const DEFAULT_EVENTS: usize = 100;
pub(crate) const MAX_EVENTS: usize = 500;
/// `freebusy` reads at most this many (expanded) events per window.
const FREEBUSY_MAX_EVENTS: usize = 2000;
/// `CalendarEvent/get` batch size.
const GET_BATCH: usize = 100;
/// `wait` polls `CalendarEvent/changes` every this many seconds.
pub(crate) const WAIT_POLL_SECS: u64 = 2;
/// `wait` timeout default / max (mirrors `k2 mail wait`).
const WAIT_DEFAULT_TIMEOUT_SECS: u64 = 300;
const WAIT_MAX_TIMEOUT_SECS: u64 = 900;
/// Field caps for draft writes.
pub(crate) const MAX_TITLE: usize = 500;
pub(crate) const MAX_LOCATION: usize = 500;
pub(crate) const MAX_NOTES: usize = 20_000;
/// The time zone stored when the caller names none.
pub(crate) const DEFAULT_TZ: &str = "Etc/UTC";

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

pub(crate) fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

pub(crate) fn usage(hint: impl AsRef<str>) -> CliResponse {
    error_response("400 Bad Request", "usage", hint.as_ref())
}

pub(crate) fn engine_err(hint: impl AsRef<str>) -> CliResponse {
    error_response("502 Bad Gateway", "engine", hint.as_ref())
}

pub(crate) fn ok_json(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

pub(crate) fn invites_need_send(what: &str) -> CliResponse {
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
    /// S4: `Calendar/get` WITH `shareWith` on another hosted account —
    /// how K2 finds the calendars shared to an inbox.
    fn calendars_shared(&self, account: &str) -> Result<Vec<Value>, String>;
    /// S4: the inbox account's groups (`memberGroupIds`).
    fn member_groups(&self, account: &str) -> Result<Vec<String>, String>;
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
    fn calendars_shared(&self, account: &str) -> Result<Vec<Value>, String> {
        self.calendar_list_shares(account)
    }
    fn member_groups(&self, account: &str) -> Result<Vec<String>, String> {
        self.account_member_groups(account)
    }
}

/// The production engine (503 `not_ready` when the server isn't up).
pub(crate) fn engine() -> Result<StalwartClient, CliResponse> {
    crate::mail::domains::engine_from_db()
        .map(|(client, _host)| client)
        .map_err(|hint| error_response("503 Service Unavailable", "not_ready", &hint))
}

// ── Owner switch ────────────────────────────────────────────────────────

/// 409 `calendars_disabled` when the owner turned calendars off for
/// hosted addresses (S2 DAV policy). NULL policy = Stalwart default (on).
pub(crate) fn calendars_enabled_gate() -> Result<(), CliResponse> {
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
pub(crate) fn caller_from_params(params: &HashMap<String, String>) -> Result<String, CliResponse> {
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
pub(crate) fn caller_from_body(project: &str) -> Result<String, CliResponse> {
    crate::mail::identity::resolve_caller(project).map(|(_path, id)| id)
}

// ── Inbox + id resolution (CAL16, CAL19) ────────────────────────────────

/// A hosted inbox the caller may use at the requested level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalInbox {
    pub address: String,
    /// The Stalwart account id — from the address row, never the client.
    pub account_id: String,
    /// The caller's `k2 mail access` level on this inbox (S4: it caps
    /// what a share allows — write/delete need draft).
    pub level: Level,
    /// S4: every OTHER active hosted mailbox (K2's rows) — the owners
    /// whose calendars may be shared to this inbox. Never from the client.
    pub peers: Vec<HostedAddr>,
}

pub(crate) fn gate_error(err: ReadError) -> CliResponse {
    match err {
        ReadError::Usage(h) => usage(h),
        ReadError::NotFound(h) => error_response("404 Not Found", "not_found", &h),
        ReadError::Engine(h) => engine_err(h),
    }
}

/// Where a calendar verb runs: the hosted Stalwart account, or a linked
/// account's calendar provider (Microsoft Graph since MCS3, Google
/// Calendar since MCS4 — [`crate::mail::calendar_provider`]).
pub enum CalTarget {
    Hosted(CalInbox),
    Linked(crate::mail::calendar_provider::LinkedCalInbox),
}

/// Resolve the inbox for a calendar verb: explicit `address`, else the
/// single hosted inbox the caller owns. Masked like the mail gate. A
/// LINKED inbox is routed to its provider, or refused with
/// `capability_missing` — only after the access gate passed.
pub fn resolve_target(
    project_id: &str,
    explicit: Option<&str>,
    need: Level,
) -> Result<CalTarget, CliResponse> {
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
                         ('k2 mail inboxes'), or mint one with 'k2 hostmail create'",
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
        return crate::mail::calendar_provider::linked_target(inbox, need, project_id).map(CalTarget::Linked);
    }
    let Some(account_id) = inbox.account_id.filter(|s| !s.trim().is_empty()) else {
        return Err(engine_err(format!(
            "address '{}' has no mailbox on the mail server",
            inbox.address
        )));
    };
    let peers = crate::mail::calendar_share::load_hosted()
        .map_err(engine_err)?
        .into_iter()
        .filter(|h| h.account_id != account_id)
        .collect();
    Ok(CalTarget::Hosted(CalInbox { address: inbox.address, account_id, level: inbox.your_level, peers }))
}

/// [`resolve_target`] for the HOSTED-only callers (S4 share code and the
/// S3 tests): a routable linked inbox answers 409 `hosted_only` here.
pub fn resolve_inbox(
    project_id: &str,
    explicit: Option<&str>,
    need: Level,
) -> Result<CalInbox, CliResponse> {
    match resolve_target(project_id, explicit, need)? {
        CalTarget::Hosted(i) => Ok(i),
        CalTarget::Linked(g) => Err(error_response(
            "409 Conflict",
            "hosted_only",
            &format!("'{}' is a linked inbox — this verb is hosted-only", g.address),
        )),
    }
}

/// The longest `ev_…` token decoded (MCS-A23: Graph ids are long, but a
/// token is never allowed to grow unbounded).
const MAX_EVENT_TOKEN: usize = 1024;

pub fn encode_event_id(address: &str, jmap_id: &str) -> String {
    format!("{EVENT_ID_PREFIX}{}", B64URL.encode(format!("{address}\n{jmap_id}")))
}

pub fn decode_event_id(token: &str) -> Option<(String, String)> {
    if token.trim().len() > MAX_EVENT_TOKEN {
        return None;
    }
    let raw = token.trim().strip_prefix(EVENT_ID_PREFIX)?;
    let bytes = B64URL.decode(raw).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let (address, id) = s.split_once('\n')?;
    if address.is_empty() || id.is_empty() || id.contains('\n') {
        return None;
    }
    Some((address.to_string(), id.to_string()))
}

/// S4: an event on a calendar SHARED to `viewer`, held by `owner`'s
/// account: `ev_<b64(viewer \n jmap-id \n owner)>`. The owner is a hosted
/// ADDRESS, resolved against K2's own rows — never an account id.
pub fn encode_shared_event_id(viewer: &str, jmap_id: &str, owner: &str) -> String {
    format!("{EVENT_ID_PREFIX}{}", B64URL.encode(format!("{viewer}\n{jmap_id}\n{owner}")))
}

/// Any event id → (inbox address, jmap id, owner address when shared).
pub fn decode_event_ref(token: &str) -> Option<(String, String, Option<String>)> {
    if token.trim().len() > MAX_EVENT_TOKEN {
        return None;
    }
    if let Some((a, id)) = decode_event_id(token) {
        return Some((a, id, None));
    }
    let raw = token.trim().strip_prefix(EVENT_ID_PREFIX)?;
    let s = String::from_utf8(B64URL.decode(raw).ok()?).ok()?;
    match s.split('\n').collect::<Vec<_>>().as_slice() {
        [v, id, o] if !v.is_empty() && !id.is_empty() && !o.is_empty() => {
            let owner = (!v.eq_ignore_ascii_case(o)).then(|| o.to_string());
            Some((v.to_string(), id.to_string(), owner))
        }
        _ => None,
    }
}

/// An event id resolved to where it lives (MCS3).
pub(crate) enum EventTarget {
    Hosted(CalInbox, String, Option<HostedAddr>),
    Linked(crate::mail::calendar_provider::LinkedCalInbox, String),
}

/// [`resolve_event`] across providers: a hosted event (own or shared),
/// or an event on a linked Microsoft 365 / Google calendar.
pub(crate) fn resolve_event_target(
    project_id: &str,
    token: &str,
    need: Level,
) -> Result<EventTarget, CliResponse> {
    let Some((address, id, owner)) = decode_event_ref(token) else {
        return Err(usage("invalid event id — use an id from 'k2 calendar events'"));
    };
    match resolve_target(project_id, Some(&address), need) {
        Ok(CalTarget::Linked(g)) => {
            if owner.is_some() {
                return Err(event_not_found(token));
            }
            Ok(EventTarget::Linked(g, id))
        }
        Ok(CalTarget::Hosted(_)) | Err(_) => {
            resolve_event(project_id, token, need).map(|(i, j, o)| EventTarget::Hosted(i, j, o))
        }
    }
}

/// Decode + gate an event id. Every gate failure is the same masked
/// event-level `not_found` (the token's address is never confirmed).
/// A shared event's owner must be an active hosted mailbox on this
/// server; whether the inbox may see it is checked against the live
/// shares by the verb ([`shared_event`]).
fn resolve_event(
    project_id: &str,
    token: &str,
    need: Level,
) -> Result<(CalInbox, String, Option<HostedAddr>), CliResponse> {
    let Some((address, jmap_id, owner)) = decode_event_ref(token) else {
        return Err(usage("invalid event id — use an id from 'k2 calendar events'"));
    };
    let inbox = match resolve_target(project_id, Some(&address), need) {
        Ok(CalTarget::Hosted(inbox)) => inbox,
        Ok(CalTarget::Linked(_)) => return Err(event_not_found(token)),
        Err(resp) if resp.status.starts_with("404") => return Err(event_not_found(token)),
        Err(resp) => return Err(resp),
    };
    let owner = match owner {
        None => None,
        Some(o) => match inbox.peers.iter().find(|p| p.address.eq_ignore_ascii_case(&o)) {
            Some(p) => Some(p.clone()),
            None => return Err(event_not_found(token)),
        },
    };
    Ok((inbox, jmap_id, owner))
}

pub(crate) fn event_not_found(token: &str) -> CliResponse {
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
pub(crate) fn parse_event_instant(raw: &str, field: &str) -> Result<DateTime<Utc>, String> {
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
pub(crate) fn window(
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
pub(crate) fn strip_mailto(s: &str) -> String {
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

pub(crate) fn email_alert_refused() -> CliResponse {
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

// ── Calendars shared TO the inbox (S4) ──────────────────────────────────

/// A calendar the inbox can use: its own, or one another hosted mailbox
/// shared to it (directly or through a group the inbox is in).
#[derive(Debug, Clone)]
pub struct VisCal {
    /// The mailbox whose account holds the calendar (from K2's rows).
    pub owner: HostedAddr,
    /// The calendar id on the owner's account.
    pub id: String,
    pub shared: bool,
    /// Effective rights: the share's rights ∩ the caller's grant level.
    pub rights: ShareRights,
    /// The `Calendar/get` row (name, color, …).
    pub raw: Value,
}

impl VisCal {
    /// The id the CLI shows and takes: a shared calendar is
    /// `<owner address>/<calendar id>` (ids are per account, so they can
    /// repeat across owners).
    pub fn display_id(&self) -> String {
        if self.shared {
            format!("{}/{}", self.owner.address, self.id)
        } else {
            self.id.clone()
        }
    }
}

fn can_write(level: Level) -> bool {
    level >= Level::Draft
}

/// The inbox's own calendars: what the grant level allows.
fn own_rights(level: Level) -> ShareRights {
    ShareRights { free_busy: true, read: true, write: can_write(level), delete: can_write(level) }
}

/// share ∩ grant: writing and deleting also need the draft level.
pub fn effective_rights(share: ShareRights, level: Level) -> ShareRights {
    ShareRights {
        write: share.write && can_write(level),
        delete: share.delete && can_write(level),
        ..share
    }
}

fn rights_json(r: ShareRights) -> Value {
    json!({ "freeBusy": r.free_busy, "read": r.read, "write": r.write, "delete": r.delete })
}

/// Calendars on OTHER hosted accounts whose `shareWith` names the inbox's
/// account or one of its groups, with effective rights (only those that
/// grant anything). K2 enforces these itself: the admin key bypasses
/// Stalwart's ACLs, so every shared read/write below is checked against
/// this list first.
pub fn shared_calendars(api: &dyn CalendarApi, inbox: &CalInbox) -> Result<Vec<VisCal>, CliResponse> {
    if inbox.peers.is_empty() {
        return Ok(Vec::new());
    }
    let mut grantees = vec![inbox.account_id.clone()];
    grantees.extend(api.member_groups(&inbox.account_id).map_err(engine_err)?);
    let mut out = Vec::new();
    for peer in &inbox.peers {
        if peer.account_id == inbox.account_id {
            continue;
        }
        let cals = api
            .calendars_shared(&peer.account_id)
            .map_err(|e| engine_err(format!("calendars of {}: {e}", peer.address)))?;
        for c in cals {
            let Some(map) = c.get("shareWith").and_then(Value::as_object) else {
                continue;
            };
            let mut share = ShareRights::default();
            for g in &grantees {
                if let Some(v) = map.get(g).filter(|v| !v.is_null()) {
                    share = share.union(ShareRights::from_json(v));
                }
            }
            let rights = effective_rights(share, inbox.level);
            let Some(id) = c.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            if rights.any() {
                out.push(VisCal { owner: peer.clone(), id, shared: true, rights, raw: c });
            }
        }
    }
    Ok(out)
}

fn share_rights_refused(what: &str) -> CliResponse {
    error_response(
        "403 Forbidden",
        "share_rights",
        &format!(
            "{what} — this inbox's share of that calendar (or your `k2 mail access` level) \
             doesn't allow it; ask your human (k2 hostmail calendar share … --write|--editor)"
        ),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Read,
    Write,
}

#[derive(Debug)]
enum Picked {
    Own(String),
    Shared(VisCal),
}

/// `--calendar <id|name|owner/id>` → one of the inbox's own calendars, or
/// a calendar shared to it that has the needed right. An unknown value is
/// a 400 listing what the inbox can see (nothing else leaks).
fn pick_calendar(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    shared: &[VisCal],
    raw: &str,
    need: Need,
) -> Result<Picked, CliResponse> {
    let own = api.calendars(&inbox.account_id).map_err(engine_err)?;
    let raw = raw.trim();
    if let Some(c) = own.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(raw)) {
        return Ok(Picked::Own(c["id"].as_str().unwrap_or_default().to_string()));
    }
    let usable = |v: &VisCal| match need {
        Need::Read => v.rights.read,
        Need::Write => v.rights.write,
    };
    let visible: Vec<&VisCal> = shared.iter().filter(|v| v.rights.read).collect();
    let refuse = |v: &VisCal| {
        share_rights_refused(&format!(
            "calendar '{}' is shared with {} read-only",
            v.display_id(),
            inbox.address
        ))
    };
    if let Some((o, id)) = raw.split_once('/') {
        if let Some(v) =
            visible.iter().find(|v| v.id == id && v.owner.address.eq_ignore_ascii_case(o))
        {
            return if usable(v) { Ok(Picked::Shared((*v).clone())) } else { Err(refuse(v)) };
        }
    }
    let name_of = |c: &Value| c.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut named: Vec<Picked> = own
        .iter()
        .filter(|c| name_of(c).eq_ignore_ascii_case(raw))
        .map(|c| Picked::Own(c["id"].as_str().unwrap_or_default().to_string()))
        .collect();
    named.extend(
        visible
            .iter()
            .filter(|v| name_of(&v.raw).eq_ignore_ascii_case(raw))
            .map(|v| Picked::Shared((*v).clone())),
    );
    if named.len() == 1 {
        return match named.pop() {
            Some(Picked::Shared(v)) if !usable(&v) => Err(refuse(&v)),
            Some(p) => Ok(p),
            None => Err(engine_err("calendar pick vanished")),
        };
    }
    let mut list: Vec<String> = own
        .iter()
        .map(|c| {
            format!(
                "{} ({})",
                c.get("name").and_then(Value::as_str).unwrap_or("?"),
                c.get("id").and_then(Value::as_str).unwrap_or("?")
            )
        })
        .collect();
    list.extend(visible.iter().map(|v| format!("{} ({})", name_of(&v.raw), v.display_id())));
    Err(usage(format!(
        "no single calendar '{raw}' on this inbox — available: {}",
        if list.is_empty() { "(none)".to_string() } else { list.join(", ") }
    )))
}

/// An event on a calendar SHARED to the inbox, shaped like the inbox's
/// own (opaque 3-part id, calendar ids as the CLI shows them).
fn shape_shared(ev: &Value, inbox: &CalInbox, owner: &HostedAddr, shared: &[VisCal], full: bool) -> Value {
    let mut out = if full { shape_full(ev, &inbox.address) } else { shape_summary(ev, &inbox.address) };
    out["id"] = json!(encode_shared_event_id(&inbox.address, stable_id(ev), &owner.address));
    let ids: Vec<String> = calendar_ids(ev)
        .into_iter()
        .filter_map(|c| {
            shared
                .iter()
                .find(|v| v.owner.account_id == owner.account_id && v.id == c && v.rights.read)
                .map(VisCal::display_id)
        })
        .collect();
    out["calendarIds"] = json!(ids);
    out["owner"] = json!(owner.address);
    out["shared"] = json!(true);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Read,
    Write,
    Delete,
}

/// A STORED event on `owner`'s account that the inbox may `act` on
/// through a share. Not on a calendar the inbox can read → the masked
/// event-level 404; readable but without the right → 403 `share_rights`.
/// Write/delete need the right on EVERY calendar the event is in.
fn shared_event(
    api: &dyn CalendarApi,
    shared: &[VisCal],
    owner: &HostedAddr,
    jmap_id: &str,
    token: &str,
    act: Act,
) -> Result<Value, CliResponse> {
    let mine: Vec<&VisCal> =
        shared.iter().filter(|v| v.owner.account_id == owner.account_id && v.rights.read).collect();
    if mine.is_empty() {
        return Err(event_not_found(token));
    }
    let ev = match get_one(api, &owner.account_id, jmap_id, FULL_PROPS) {
        Ok(Some(e)) => e,
        Ok(None) => return Err(event_not_found(token)),
        Err(e) => return Err(engine_err(e)),
    };
    let cals = calendar_ids(&ev);
    let find = |c: &String| mine.iter().find(|v| &v.id == c);
    if cals.is_empty() || !cals.iter().any(|c| find(c).is_some()) {
        return Err(event_not_found(token));
    }
    let ok = match act {
        Act::Read => true,
        Act::Write => cals.iter().all(|c| find(c).is_some_and(|v| v.rights.write)),
        Act::Delete => cals.iter().all(|c| find(c).is_some_and(|v| v.rights.delete)),
    };
    if !ok {
        return Err(share_rights_refused(match act {
            Act::Delete => "deleting an event on a shared calendar needs the editor share (mayDelete) and draft level",
            _ => "changing an event on a shared calendar needs a write share (mayWriteAll) and draft level",
        }));
    }
    Ok(ev)
}

// ── wait state across accounts (S4) ─────────────────────────────────────

/// A `wait` state covering several accounts: `k2s_<b64(json {account:
/// state})>`. A plain state (S3) is the inbox's own account only.
const STATES_PREFIX: &str = "k2s_";

fn encode_states(states: &std::collections::BTreeMap<String, String>) -> String {
    format!("{STATES_PREFIX}{}", B64URL.encode(json!(states).to_string()))
}

fn decode_states(raw: &str) -> Option<HashMap<String, String>> {
    let b = B64URL.decode(raw.trim().strip_prefix(STATES_PREFIX)?).ok()?;
    serde_json::from_slice::<HashMap<String, String>>(&b).ok()
}

/// Of `ids` on `owner`'s account, those on a calendar shared to the inbox
/// with read.
fn visible_ids(
    api: &dyn CalendarApi,
    shared: &[VisCal],
    owner: &HostedAddr,
    ids: &[String],
) -> Result<Vec<String>, CliResponse> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let readable: Vec<&str> = shared
        .iter()
        .filter(|v| v.owner.account_id == owner.account_id && v.rights.read)
        .map(|v| v.id.as_str())
        .collect();
    let evs = get_events(api, &owner.account_id, ids, &["id", "calendarIds"]).map_err(engine_err)?;
    Ok(evs
        .iter()
        .filter(|e| calendar_ids(e).iter().any(|c| readable.contains(&c.as_str())))
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_string))
        .collect())
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
    let inbox =
        match resolve_target(&project_id, params.get("address").map(String::as_str), Level::Read) {
            Ok(CalTarget::Hosted(i)) => i,
            Ok(CalTarget::Linked(g)) => return crate::mail::calendar_provider::handle_list(&g),
            Err(r) => return r,
        };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    match engine() {
        Ok(api) => list_with(&api, &inbox),
        Err(r) => r,
    }
}

/// The inbox's own calendars plus the calendars shared to it (directly
/// or through a group) that it may read, each with its owner and the
/// effective rights (share ∩ grant). Free/busy-only shares are not
/// listed (they show up in `freebusy` only).
pub fn list_with(api: &dyn CalendarApi, inbox: &CalInbox) -> CliResponse {
    let own = match api.calendars(&inbox.account_id) {
        Ok(c) => c,
        Err(e) => return engine_err(e),
    };
    let shared = match shared_calendars(api, inbox) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut out: Vec<Value> = own
        .iter()
        .map(|c| {
            let mut v = shape_calendar(c);
            v["owner"] = json!(inbox.address);
            v["shared"] = json!(false);
            v["rights"] = rights_json(own_rights(inbox.level));
            v
        })
        .collect();
    for s in shared.iter().filter(|s| s.rights.read) {
        let mut v = shape_calendar(&s.raw);
        v["id"] = json!(s.display_id());
        v["isDefault"] = json!(false);
        v["owner"] = json!(s.owner.address);
        v["shared"] = json!(true);
        v["rights"] = rights_json(s.rights);
        out.push(v);
    }
    ok_json(json!({ "ok": true, "address": inbox.address, "calendars": out }))
}

// ── GET /cli/mail/calendar/events ───────────────────────────────────────

pub fn handle_events(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let inbox =
        match resolve_target(&project_id, params.get("address").map(String::as_str), Level::Read) {
            Ok(CalTarget::Hosted(i)) => i,
            Ok(CalTarget::Linked(g)) => {
                return crate::mail::calendar_provider::handle_events(&g, params, Utc::now())
            }
            Err(r) => return r,
        };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
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
    let shared = match shared_calendars(api, inbox) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // Sources: the inbox's own account (all its calendars, or one), and
    // each calendar shared to it with read (S4).
    enum Src {
        Own(Option<String>),
        Shared(VisCal),
    }
    let (sources, calendar) = match crate::cli::opt_param(params, "calendar") {
        Some(raw) => match pick_calendar(api, inbox, &shared, &raw, Need::Read) {
            Ok(Picked::Own(id)) => (vec![Src::Own(Some(id.clone()))], Some(id)),
            Ok(Picked::Shared(v)) => {
                let shown = v.display_id();
                (vec![Src::Shared(v)], Some(shown))
            }
            Err(r) => return r,
        },
        None => {
            let mut s = vec![Src::Own(None)];
            s.extend(shared.iter().filter(|v| v.rights.read).cloned().map(Src::Shared));
            (s, None)
        }
    };
    let several = sources.len() > 1;
    let mut rows: Vec<(Option<DateTime<Utc>>, Value)> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, String)> = Default::default();
    let mut truncated = false;
    for src in &sources {
        let (account, cal) = match src {
            Src::Own(c) => (inbox.account_id.as_str(), c.as_deref()),
            Src::Shared(v) => (v.owner.account_id.as_str(), Some(v.id.as_str())),
        };
        // Ask for one more than the page so `truncated` is honest.
        let ids = match api.query(account, window_query(&start, &end, cal, limit + 1)) {
            Ok(ids) => ids,
            Err(e) => return query_err(e),
        };
        truncated |= ids.len() > limit;
        let ids: Vec<String> = ids.into_iter().take(limit).collect();
        let events = match get_events(api, account, &ids, SUMMARY_PROPS) {
            Ok(evs) => evs,
            Err(e) => return engine_err(e),
        };
        for ev in &events {
            let span = event_utc_span(ev).0;
            if let Src::Shared(v) = src {
                // An event in two calendars shared to the inbox shows once.
                let key = (
                    v.owner.account_id.clone(),
                    stable_id(ev).to_string(),
                    span.map(|d| rfc3339_utc(&d)).unwrap_or_default(),
                );
                if !seen.insert(key) {
                    continue;
                }
            }
            let shaped = match src {
                Src::Own(_) => {
                    let mut e = shape_summary(ev, &inbox.address);
                    if several {
                        e["owner"] = json!(inbox.address);
                        e["shared"] = json!(false);
                    }
                    e
                }
                Src::Shared(v) => shape_shared(ev, inbox, &v.owner, &shared, false),
            };
            rows.push((span, shaped));
        }
    }
    if several {
        rows.sort_by_key(|(s, _)| *s);
    }
    truncated |= rows.len() > limit;
    rows.truncate(limit);
    ok_json(json!({
        "ok": true,
        "address": inbox.address,
        "start": rfc3339_utc(&start),
        "end": rfc3339_utc(&end),
        "calendar": calendar,
        "events": rows.into_iter().map(|(_, e)| e).collect::<Vec<_>>(),
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
    let (inbox, jmap_id, owner) = match resolve_event_target(&project_id, &token, Level::Read) {
        Ok(EventTarget::Hosted(i, j, o)) => (i, j, o),
        Ok(EventTarget::Linked(g, id)) => {
            return crate::mail::calendar_provider::handle_show(&g, &id, &token)
        }
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    match engine() {
        Ok(api) => match &owner {
            None => show_with(&api, &inbox, &jmap_id, &token),
            Some(o) => show_shared_with(&api, &inbox, o, &jmap_id, &token),
        },
        Err(r) => r,
    }
}

/// `show` of an event on a calendar shared to the inbox (S4): only when
/// the event is on a calendar the inbox may read right now.
pub fn show_shared_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    owner: &HostedAddr,
    jmap_id: &str,
    token: &str,
) -> CliResponse {
    let shared = match shared_calendars(api, inbox) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match shared_event(api, &shared, owner, jmap_id, token, Act::Read) {
        Ok(ev) => ok_json(json!({
            "ok": true,
            "address": inbox.address,
            "event": shape_shared(&ev, inbox, owner, &shared, true),
        })),
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
    let inbox =
        match resolve_target(&project_id, params.get("address").map(String::as_str), Level::Read) {
            Ok(CalTarget::Hosted(i)) => i,
            Ok(CalTarget::Linked(g)) => {
                return crate::mail::calendar_provider::handle_freebusy(&g, params, Utc::now())
            }
            Err(r) => return r,
        };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
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
    pub(crate) fn as_str(self) -> &'static str {
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
    let (mut spans, source, mut truncated) =
        match api.availability(&inbox.account_id, &rfc3339_utc(&start), &rfc3339_utc(&end)) {
            Ok(list) => (availability_spans(&list, start, end), "availability", false),
            Err(_) => match busy_from_events(api, &inbox.account_id, None, start, end) {
                Ok((spans, truncated)) => (spans, "events", truncated),
                Err(r) => return r,
            },
        };
    // S4: plus every calendar shared to the inbox with at least
    // free/busy, from its events on the owner's account (busy times only).
    let shared = match shared_calendars(api, inbox) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let fb: Vec<&VisCal> = shared.iter().filter(|v| v.rights.free_busy).collect();
    for v in &fb {
        match busy_from_events(api, &v.owner.account_id, Some(&v.id), start, end) {
            Ok((s, t)) => {
                spans.extend(s);
                truncated |= t;
            }
            Err(r) => return r,
        }
    }
    let source = if fb.is_empty() { source.to_string() } else { format!("{source}+shared") };
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
        "sharedCalendars": fb.len(),
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
    account: &str,
    calendar: Option<&str>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(Vec<BusySpan>, bool), CliResponse> {
    let ids = api
        .query(account, window_query(&start, &end, calendar, FREEBUSY_MAX_EVENTS + 1))
        .map_err(query_err)?;
    let truncated = ids.len() > FREEBUSY_MAX_EVENTS;
    let ids: Vec<String> = ids.into_iter().take(FREEBUSY_MAX_EVENTS).collect();
    let events = get_events(
        api,
        account,
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
pub(crate) fn query_err(e: String) -> CliResponse {
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
    let inbox =
        match resolve_target(&project_id, params.get("address").map(String::as_str), Level::Read) {
            Ok(CalTarget::Hosted(i)) => i,
            Ok(CalTarget::Linked(g)) => {
                return crate::mail::calendar_provider::handle_wait(
                    &g,
                    &project_id,
                    params,
                    timeout_secs,
                )
            }
            Err(r) => return r,
        };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
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

pub(crate) fn parse_wait_timeout(params: &HashMap<String, String>) -> Result<u64, CliResponse> {
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
///
/// S4: when calendars are shared to the inbox, each owner account with a
/// readable shared calendar is watched too (created/updated ids filtered
/// to those calendars; destroyed ids cannot be checked after deletion and
/// are passed on as opaque ids), and the state becomes one `k2s_…` token
/// covering every account.
pub fn wait_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    since: Option<&str>,
    timeout_secs: u64,
    elapsed: &mut dyn FnMut() -> u64,
    sleep: &mut dyn FnMut(std::time::Duration),
) -> CliResponse {
    let shared = match shared_calendars(api, inbox) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut owners: Vec<HostedAddr> = Vec::new();
    for v in shared.iter().filter(|v| v.rights.read) {
        if !owners.iter().any(|o| o.account_id == v.owner.account_id) {
            owners.push(v.owner.clone());
        }
    }
    let since = since.map(str::trim).filter(|s| !s.is_empty());
    let given: Option<HashMap<String, String>> = match since {
        Some(s) if s.starts_with(STATES_PREFIX) => match decode_states(s) {
            Some(m) => Some(m),
            None => return usage("invalid --since-state — use the state a wait printed"),
        },
        _ => None,
    };
    if owners.is_empty() {
        let own = match &given {
            Some(m) => m.get(&inbox.account_id).cloned(),
            None => since.map(str::to_string),
        };
        return wait_own(api, inbox, own.as_deref(), timeout_secs, elapsed, sleep);
    }
    let given = given.unwrap_or_else(|| {
        since
            .map(|s| HashMap::from([(inbox.account_id.clone(), s.to_string())]))
            .unwrap_or_default()
    });
    let mut accounts: Vec<(String, Option<HostedAddr>)> = vec![(inbox.account_id.clone(), None)];
    accounts.extend(owners.iter().map(|o| (o.account_id.clone(), Some(o.clone()))));
    let mut states: std::collections::BTreeMap<String, String> = Default::default();
    for (acct, _) in &accounts {
        let st = match given.get(acct) {
            Some(s) => s.clone(),
            None => match api.get(acct, &[], None) {
                Ok(reply) => match reply.get("state").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => return engine_err("CalendarEvent/get: reply has no state"),
                },
                Err(e) => return engine_err(e),
            },
        };
        states.insert(acct.clone(), st);
    }
    let (mut created, mut updated, mut destroyed) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        for (acct, owner) in &accounts {
            let mut state = states.get(acct).cloned().unwrap_or_default();
            let (mut c, mut u, mut d) = (Vec::new(), Vec::new(), Vec::new());
            loop {
                let reply = match api.changes(acct, &state) {
                    Ok(r) => r,
                    Err(e) if e.contains("cannotCalculateChanges") => {
                        return error_response(
                            "409 Conflict",
                            "state_expired",
                            "the server can no longer list changes since that state — re-read \
                             with 'k2 calendar events' and wait again without --since-state",
                        )
                    }
                    Err(e) => return engine_err(e),
                };
                c.extend(id_list(reply.get("created")));
                u.extend(id_list(reply.get("updated")));
                d.extend(id_list(reply.get("destroyed")));
                if let Some(ns) = reply.get("newState").and_then(Value::as_str) {
                    state = ns.to_string();
                }
                if !reply.get("hasMoreChanges").and_then(Value::as_bool).unwrap_or(false) {
                    break;
                }
            }
            states.insert(acct.clone(), state);
            match owner {
                None => {
                    let enc = |i: &String| encode_event_id(&inbox.address, i);
                    created.extend(c.iter().map(enc));
                    updated.extend(u.iter().map(enc));
                    destroyed.extend(d.iter().map(enc));
                }
                Some(o) => {
                    let enc = |i: &String| encode_shared_event_id(&inbox.address, i, &o.address);
                    let c = match visible_ids(api, &shared, o, &c) {
                        Ok(v) => v,
                        Err(r) => return r,
                    };
                    let u = match visible_ids(api, &shared, o, &u) {
                        Ok(v) => v,
                        Err(r) => return r,
                    };
                    created.extend(c.iter().map(enc));
                    updated.extend(u.iter().map(enc));
                    destroyed.extend(d.iter().map(enc));
                }
            }
        }
        if !(created.is_empty() && updated.is_empty() && destroyed.is_empty()) {
            return ok_json(json!({
                "ok": true,
                "timedOut": false,
                "address": inbox.address,
                "state": encode_states(&states),
                "created": created,
                "updated": updated,
                "destroyed": destroyed,
                "sharedAccounts": owners.len(),
            }));
        }
        if elapsed() + WAIT_POLL_SECS > timeout_secs {
            return ok_json(json!({
                "ok": true,
                "timedOut": true,
                "address": inbox.address,
                "state": encode_states(&states),
            }));
        }
        sleep(std::time::Duration::from_secs(WAIT_POLL_SECS));
    }
}

/// The S3 long-poll on the inbox's own account (plain state).
fn wait_own(
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

pub(crate) fn parse_body(body: &[u8]) -> Result<WriteBody, CliResponse> {
    serde_json::from_slice(body).map_err(|e| usage(format!("invalid JSON body: {e}")))
}

/// IANA-ish zone name check (no tz database here; Stalwart validates
/// the name itself and rejects unknown zones in the set).
pub(crate) fn valid_tz(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && tz
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

pub(crate) fn check_len(v: &Option<String>, field: &str, max: usize) -> Result<(), CliResponse> {
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
    let inbox = match resolve_target(&project_id, b.address.as_deref(), Level::Draft) {
        Ok(CalTarget::Hosted(i)) => i,
        Ok(CalTarget::Linked(g)) => return crate::mail::calendar_provider::handle_create(&g, &b),
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
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
    // S4: `--calendar` may name a calendar shared to the inbox with
    // write; the event then lives on the OWNER's account.
    let mut target: Option<VisCal> = None;
    let calendar_id = match b.calendar.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let shared = match shared_calendars(api, inbox) {
                Ok(s) => s,
                Err(r) => return r,
            };
            match pick_calendar(api, inbox, &shared, raw, Need::Write) {
                Ok(Picked::Own(id)) => id,
                Ok(Picked::Shared(v)) => {
                    // On someone else's calendar the only participant a
                    // draft event may name is that calendar's owner.
                    let others = non_owner_participants(b.participants.as_ref(), &v.owner.address);
                    if !others.is_empty() {
                        return invites_need_send(&format!(
                            "this event names {} participant(s) besides {}",
                            others.len(),
                            v.owner.address
                        ));
                    }
                    let id = v.id.clone();
                    target = Some(v);
                    id
                }
                Err(r) => return r,
            }
        }
        None => match api.calendars(&inbox.account_id).map(|cals| default_calendar(&cals)) {
            Err(e) => return engine_err(e),
            Ok(Some(id)) => id,
            Ok(None) => {
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
    let account = target
        .as_ref()
        .map(|v| v.owner.account_id.clone())
        .unwrap_or_else(|| inbox.account_id.clone());
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
        &account,
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
    if let Some(v) = &target {
        let event = get_one(api, &account, new_id, SUMMARY_PROPS)
            .ok()
            .flatten()
            .map(|e| shape_shared(&e, inbox, &v.owner, std::slice::from_ref(v), false));
        return ok_json(json!({
            "ok": true,
            "address": inbox.address,
            "owner": v.owner.address,
            "id": encode_shared_event_id(&inbox.address, new_id, &v.owner.address),
            "calendarId": v.display_id(),
            "event": event,
            "emailSent": false,
        }));
    }
    let event = get_one(api, &account, new_id, SUMMARY_PROPS)
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
    let (inbox, jmap_id, owner) = match resolve_event_target(&project_id, &token, Level::Draft) {
        Ok(EventTarget::Hosted(i, j, o)) => (i, j, o),
        Ok(EventTarget::Linked(g, id)) => {
            return crate::mail::calendar_provider::handle_update(&g, &id, &token, &b)
        }
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    match engine() {
        Ok(api) => update_at(&api, &inbox, owner.as_ref(), &jmap_id, &token, &b),
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
    participants_guard(&stored, &inbox.address, what)?;
    Ok(stored)
}

/// CAL20 on a STORED event: participants other than `owner` (the
/// calendar's owner mailbox), or an organizer who isn't `owner`, need
/// the send level.
fn participants_guard(stored: &Value, owner: &str, what: &str) -> Result<(), CliResponse> {
    let mut others = non_owner_participants(stored.get("participants"), owner);
    // An invitation someone else organized counts too, even when its
    // participant list was reduced to the owner.
    if let Some(org) = stored.get("organizerCalendarAddress").and_then(Value::as_str) {
        let org = strip_mailto(org);
        if !org.is_empty() && org != owner.to_ascii_lowercase() && !others.contains(&org) {
            others.push(org);
        }
    }
    if !others.is_empty() {
        return Err(invites_need_send(&format!(
            "{what}: this event has {} participant(s) besides {}",
            others.len(),
            owner
        )));
    }
    Ok(())
}

/// A shared event the inbox may `act` on, then CAL20 against the
/// calendar's OWNER. Returns the stored event and the shared view.
fn shared_write_guard(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    owner: &HostedAddr,
    jmap_id: &str,
    token: &str,
    act: Act,
    what: &str,
) -> Result<(Value, Vec<VisCal>), CliResponse> {
    let shared = shared_calendars(api, inbox)?;
    let stored = shared_event(api, &shared, owner, jmap_id, token, act)?;
    participants_guard(&stored, &owner.address, what)?;
    Ok((stored, shared))
}

/// The inbox's own account (S3 entry point; tests drive it).
#[cfg(test)]
pub fn update_with(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    jmap_id: &str,
    token: &str,
    b: &WriteBody,
) -> CliResponse {
    update_at(api, inbox, None, jmap_id, token, b)
}

/// `update` of an event on the inbox's own account (`owner` None) or on
/// a calendar shared to it (S4: the share needs write, the grant draft;
/// the write goes to the OWNER's account).
pub fn update_at(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    owner: Option<&HostedAddr>,
    jmap_id: &str,
    token: &str,
    b: &WriteBody,
) -> CliResponse {
    let owner_addr = owner.map(|o| o.address.as_str()).unwrap_or(&inbox.address);
    let account = owner.map(|o| o.account_id.as_str()).unwrap_or(&inbox.account_id);
    let others = non_owner_participants(b.participants.as_ref(), owner_addr);
    if !others.is_empty() {
        return invites_need_send(&format!(
            "this update names {} participant(s) besides {}",
            others.len(),
            owner_addr
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
    let (stored, shared) = match owner {
        None => match stored_event_guard(api, inbox, jmap_id, token, "update refused") {
            Ok(ev) => (ev, Vec::new()),
            Err(r) => return r,
        },
        Some(o) => {
            match shared_write_guard(api, inbox, o, jmap_id, token, Act::Write, "update refused") {
                Ok(v) => v,
                Err(r) => return r,
            }
        }
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
        account,
        json!({ "update": { jmap_id: Value::Object(patch) }, "sendSchedulingMessages": false }),
    ) {
        Ok(r) => r,
        Err(e) => return engine_err(e),
    };
    if let Some(msg) = set_error("CalendarEvent/set update", &reply, "notUpdated") {
        return set_failure(msg);
    }
    let event = get_one(api, account, jmap_id, SUMMARY_PROPS).ok().flatten().map(|e| match owner {
        None => shape_summary(&e, &inbox.address),
        Some(o) => shape_shared(&e, inbox, o, &shared, false),
    });
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
    let (inbox, jmap_id, owner) = match resolve_event_target(&project_id, &token, Level::Draft) {
        Ok(EventTarget::Hosted(i, j, o)) => (i, j, o),
        Ok(EventTarget::Linked(g, id)) => {
            return crate::mail::calendar_provider::handle_delete(&g, &id, &token)
        }
        Err(r) => return r,
    };
    if let Err(r) = calendars_enabled_gate() {
        return r;
    }
    match engine() {
        Ok(api) => delete_at(&api, &inbox, owner.as_ref(), &jmap_id, &token),
        Err(r) => r,
    }
}

/// The inbox's own account (S3 entry point; tests drive it).
#[cfg(test)]
pub fn delete_with(api: &dyn CalendarApi, inbox: &CalInbox, jmap_id: &str, token: &str) -> CliResponse {
    delete_at(api, inbox, None, jmap_id, token)
}

/// `delete` on the inbox's own account, or on a calendar shared to it
/// (S4: the share needs mayDelete — the editor level — and the grant
/// draft; IT10).
pub fn delete_at(
    api: &dyn CalendarApi,
    inbox: &CalInbox,
    owner: Option<&HostedAddr>,
    jmap_id: &str,
    token: &str,
) -> CliResponse {
    // CAL20: a draft delete of a participant event would leave attendees
    // without a CANCEL — refuse on the STORED event.
    let guard = match owner {
        None => stored_event_guard(api, inbox, jmap_id, token, "delete refused").map(|_| ()),
        Some(o) => shared_write_guard(api, inbox, o, jmap_id, token, Act::Delete, "delete refused")
            .map(|_| ()),
    };
    if let Err(r) = guard {
        return r;
    }
    let account = owner.map(|o| o.account_id.as_str()).unwrap_or(&inbox.account_id);
    let reply = match api.set_no_scheduling(
        account,
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
#[path = "calendar_tests.rs"]
mod tests;
