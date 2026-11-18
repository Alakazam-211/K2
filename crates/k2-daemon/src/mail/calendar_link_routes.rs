//! Calendar S1 routes (prd-google-calendar-link-v1 §3.2/§3.4, vs-live-2
//! CV-17, CV-18):
//!
//! | route                                       | who                          |
//! |---------------------------------------------|------------------------------|
//! | GET  /cli/mail/calendar/calendars?address   | owner/Admin: every calendar with `optedIn`; anyone else with a read grant: the calendars the link exposes |
//! | POST /cli/mail/calendar/calendars           | owner/Admin (a person, never a passport): `{address, add, remove}` |
//! | POST /cli/mail/calendar/link-request        | Member, agents too: posts a "link your calendar, read-only" card; links nothing |
//!
//! The opt-in list lives in [`crate::mail::calendar_link`] (0316). A link
//! stored `calendar` (read-write, 0.46.1) exposes every calendar and has no
//! opt-ins to change. The primary is always on for a `calendar-read` link.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::access::{AccessInbox, Level, Source};
use crate::mail::calendar::{self as cal, error_response, ok_json, usage, CalTarget};
use crate::mail::calendar_link::{self as store, PRIMARY};
use crate::mail::calendar_provider::{self as linked, CalendarProvider, LinkedCalInbox};
use crate::mail::mailbox_guard;
use crate::mail::oauth::CalAccess;

fn owner_only(what: &str) -> CliResponse {
    error_response(
        "403 Forbidden",
        "owner_only",
        &format!("{what} is a person's choice (the owner or an Admin) — ask your human"),
    )
}

/// The owner or an Admin calling as themselves (not an agent passport).
fn is_owner_or_admin() -> bool {
    mailbox_guard::viewer_info().is_some_and(|v| v.opens_any)
}

/// A linked inbox for the OWNER surface: no workspace gate (the owner
/// opens every inbox), the same owner-switch and scope checks as an
/// agent's read (vs-live-2 CV-9's request-free twin of `linked_target`).
pub fn owner_target(address: &str) -> Result<LinkedCalInbox, CliResponse> {
    let Ok(addr) = crate::mail::addresses::normalize_address(address) else {
        return Err(usage(format!("'{}' is not a valid address", address.trim())));
    };
    let Some(row) = crate::mail::external::inbox_for_address(&addr) else {
        return Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("no linked calendar '{addr}' — 'k2 mail link list' shows what's connected"),
        ));
    };
    let owner = row.owner_project_id.clone();
    let inbox = AccessInbox {
        source: Source::Linked,
        address: row.email_address.clone(),
        account_id: None,
        linked: Some(row),
        your_level: Level::Read,
    };
    linked::linked_target(inbox, Level::Read, &owner)
}

/// The account's calendars as the owner sees them: each with `optedIn`
/// (`primary` and every calendar on a read-write link are always in).
fn owner_view(p: &dyn CalendarProvider, g: &LinkedCalInbox) -> Result<Vec<Value>, CliResponse> {
    let all = p.calendars(g).map_err(|e| linked::to_resp(g, e, None))?;
    Ok(all
        .into_iter()
        .map(|mut c| {
            let primary = c.get("isDefault").and_then(Value::as_bool) == Some(true);
            let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let opted = match &g.opted_in {
                None => true,
                Some(list) => primary || list.iter().any(|o| p.same_calendar(g, &id, o)),
            };
            c["optedIn"] = json!(opted);
            c["primary"] = json!(primary);
            c
        })
        .collect())
}

fn access_word(g: &LinkedCalInbox) -> &'static str {
    if g.cal == CalAccess::Read {
        "read-only"
    } else {
        "read-write"
    }
}

/// GET `/cli/mail/calendar/calendars?address=`.
pub fn handle_calendars_get(params: &HashMap<String, String>) -> CliResponse {
    let Some(address) = crate::cli::opt_param(params, "address") else {
        return usage("missing 'address' — the linked calendar's account (k2 cal calendars <address>)");
    };
    if is_owner_or_admin() {
        // The person's own read is logged like any mailbox read (OA0).
        if !mailbox_guard::check_address(None, &address) {
            return if mailbox_guard::audit_failed() {
                mailbox_guard::audit_unavailable()
            } else {
                error_response("404 Not Found", "not_found", "no such linked calendar")
            };
        }
        let g = match owner_target(&address) {
            Ok(g) => g,
            Err(r) => return r,
        };
        return calendars_get_with(linked::real(&g).as_ref(), &g, true);
    }
    let project_id = match cal::caller_from_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match cal::resolve_target(&project_id, Some(&address), Level::Read) {
        Ok(CalTarget::Linked(g)) => {
            if let Err(r) = mailbox_guard::log_agent_read(
                &g.project_id,
                Source::Linked,
                &g.row.id,
                &g.address,
                "calendar/calendars",
                None,
            ) {
                return r;
            }
            calendars_get_with(linked::real(&g).as_ref(), &g, false)
        }
        Ok(CalTarget::Hosted(_)) => usage("this is a hosted inbox — 'k2 cal list' shows its calendars"),
        Err(r) => r,
    }
}

/// The GET answer: the owner sees every calendar with `optedIn`; anyone
/// else only the calendars the link exposes.
pub fn calendars_get_with(p: &dyn CalendarProvider, g: &LinkedCalInbox, owner: bool) -> CliResponse {
    let all = match owner_view(p, g) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let cals: Vec<Value> = if owner {
        all
    } else {
        all.into_iter().filter(|c| c["optedIn"] == json!(true)).collect()
    };
    ok_json(json!({
        "ok": true,
        "address": g.address,
        "source": p.source(),
        "access": access_word(g),
        "calendars": cals,
        "hint": if g.cal == CalAccess::Read {
            "a read-only link shows its primary calendar and the ones the owner added (k2 cal calendars <address> --add <id|name>)"
        } else {
            "a read-write link (made before 0.46.16) shows every calendar on the account"
        },
    }))
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CalendarsBody {
    address: String,
    add: Value,
    remove: Value,
}

/// `"x"`, `["x", "y"]` or absent → the list.
fn words(v: &Value) -> Result<Vec<String>, CliResponse> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::String(s) => Ok(s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| usage("'add' / 'remove' take calendar ids or names")))
            .collect(),
        _ => Err(usage("'add' / 'remove' take calendar ids or names")),
    }
}

/// Find one listed calendar by id (or `primary`) or by its name (one match).
fn pick<'a>(p: &dyn CalendarProvider, g: &LinkedCalInbox, all: &'a [Value], asked: &str) -> Result<&'a Value, CliResponse> {
    let by_id: Vec<&Value> = all
        .iter()
        .filter(|c| {
            let primary = c.get("isDefault").and_then(Value::as_bool) == Some(true);
            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
            p.same_calendar(g, id, asked) || (primary && asked.eq_ignore_ascii_case(PRIMARY))
        })
        .collect();
    if let [one] = by_id.as_slice() {
        return Ok(one);
    }
    let by_name: Vec<&Value> = all
        .iter()
        .filter(|c| c.get("name").and_then(Value::as_str).is_some_and(|n| n.trim().eq_ignore_ascii_case(asked)))
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one),
        [] => Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("no calendar '{asked}' on {} — 'k2 cal calendars {}' lists them", g.address, g.address),
        )),
        many => Err(usage(format!(
            "{} calendars are named '{asked}' — pass an id: {}",
            many.len(),
            many.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// POST `/cli/mail/calendar/calendars` `{address, add, remove}`.
pub fn handle_calendars_post(body: &[u8]) -> CliResponse {
    if !is_owner_or_admin() {
        return owner_only("Which calendars a link shows");
    }
    let b: CalendarsBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let g = match owner_target(&b.address) {
        Ok(g) => g,
        Err(r) => return r,
    };
    calendars_post_with(linked::real(&g).as_ref(), &g, &b.add, &b.remove)
}

/// The POST, over any provider (tests use the fakes).
pub fn calendars_post_with(p: &dyn CalendarProvider, g: &LinkedCalInbox, add: &Value, remove: &Value) -> CliResponse {
    let (add, remove) = match (words(add), words(remove)) {
        (Ok(a), Ok(r)) => (a, r),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    if add.is_empty() && remove.is_empty() {
        return usage("nothing to change — pass add and/or remove (calendar ids or names)");
    }
    if g.cal != CalAccess::Read {
        return error_response(
            "409 Conflict",
            "usage",
            &format!(
                "'{}' is a read-write link (made before 0.46.16): it already shows every calendar. \
                 To pick calendars, unlink it and link it again read-only with 'k2 cal link'",
                g.address
            ),
        );
    }
    let all = match p.calendars(g) {
        Ok(v) => v,
        Err(e) => return linked::to_resp(g, e, None),
    };
    let (actor, _) = mailbox_guard::governance_actor();
    let mut added = Vec::new();
    for a in &add {
        let c = match pick(p, g, &all, a) {
            Ok(c) => c,
            Err(r) => return r,
        };
        if c.get("isDefault").and_then(Value::as_bool) == Some(true) {
            continue; // the primary is always on
        }
        let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let name = c.get("name").and_then(Value::as_str);
        if let Err(r) = mailbox_guard::log_governance(Source::Linked, &g.row.id, &g.address, None, "calendar_add") {
            return r;
        }
        match store::add_calendar(&g.row.id, &id, name, &actor) {
            Ok(_) => added.push(id),
            Err(e) => return cal::engine_err(e),
        }
    }
    let stored = match store::opted_in(&g.row.id) {
        Ok(v) => v,
        Err(e) => return cal::engine_err(e),
    };
    let mut removed = Vec::new();
    for r in &remove {
        if r.eq_ignore_ascii_case(PRIMARY) || r.eq_ignore_ascii_case(&g.address) {
            return usage(format!(
                "the primary calendar is always on for a read-only link — to stop it, unlink {} ('k2 mail link remove')",
                g.address
            ));
        }
        let hit = stored
            .iter()
            .find(|o| o.calendar_id == *r || p.same_calendar(g, &o.calendar_id, r) || o.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(r)))
            .map(|o| o.calendar_id.clone());
        let Some(id) = hit else {
            return error_response(
                "404 Not Found",
                "not_found",
                &format!("'{r}' isn't one of the calendars added to {} — 'k2 cal calendars {}' lists them", g.address, g.address),
            );
        };
        if let Err(resp) = mailbox_guard::log_governance(Source::Linked, &g.row.id, &g.address, None, "calendar_remove") {
            return resp;
        }
        if let Err(e) = store::remove_calendar(&g.row.id, &id) {
            return cal::engine_err(e);
        }
        removed.push(id);
    }
    let g2 = LinkedCalInbox {
        opted_in: Some(match store::opted_in(&g.row.id) {
            Ok(v) => v.into_iter().map(|o| o.calendar_id).collect(),
            Err(e) => return cal::engine_err(e),
        }),
        ..g.clone()
    };
    let view = match owner_view(p, &g2) {
        Ok(v) => v,
        Err(r) => return r,
    };
    ok_json(json!({
        "ok": true,
        "address": g.address,
        "access": access_word(g),
        "added": added,
        "removed": removed,
        "calendars": view,
    }))
}

// ── POST /cli/mail/calendar/link-request ────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LinkRequestBody {
    project: String,
    address: String,
    provider: String,
}

/// One card per address per workspace per 10 minutes (an agent retrying
/// doesn't stack cards).
const REQUEST_REPEAT: Duration = Duration::from_secs(600);

fn recent_requests() -> &'static Mutex<HashMap<(String, String), Instant>> {
    static M: OnceLock<Mutex<HashMap<(String, String), Instant>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The provider word a card and a link use: `gmail` | `microsoft`.
pub fn provider_word(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "google" | "gmail" => Some("gmail"),
        "microsoft" | "outlook" | "m365" | "office365" => Some("microsoft"),
        _ => None,
    }
}

/// The card's client action: open Settings → Email Link → Connect a
/// calendar, filled in (the owner taps; the agent never holds the link).
pub const LINK_CALENDAR_ACTION: &str = "link_calendar";

/// POST `/cli/mail/calendar/link-request` `{project, address, provider}`:
/// an agent asks its person to link a calendar read-only. Posts a card on
/// the workspace's Thread; links nothing, stores nothing but the log row.
pub fn handle_link_request(body: &[u8]) -> CliResponse {
    let b: LinkRequestBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let Some(provider) = provider_word(&b.provider) else {
        return usage("'provider' is google or microsoft");
    };
    let address = match crate::mail::addresses::normalize_address(&b.address) {
        Ok(a) => a,
        Err(_) => return usage(format!("'{}' is not a valid address", b.address.trim())),
    };
    let (path, project_id) = match crate::mail::identity::resolve_caller(&b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    {
        let mut m = recent_requests().lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        m.retain(|_, t| now.duration_since(*t) < REQUEST_REPEAT);
        if m.contains_key(&(project_id.clone(), address.clone())) {
            return ok_json(json!({
                "ok": true,
                "address": address,
                "posted": false,
                "hint": "a card for this calendar was posted in the last 10 minutes — your human has it",
            }));
        }
        m.insert((project_id.clone(), address.clone()), now);
    }
    let inbox_id = crate::mail::external::inbox_for_address(&address).map(|r| r.id).unwrap_or_default();
    if let Err(r) = mailbox_guard::log_governance(Source::Linked, &inbox_id, &address, Some(&project_id), "link_request") {
        return r;
    }
    let label = if provider == "gmail" { "Google" } else { "Microsoft 365" };
    let text = format!(
        "Your agent asks you to link the {label} calendar of {address}, read-only. Agents will \
         see its events (primary calendar only, unless you add others); they can't change, \
         accept or decline anything."
    );
    let mut notice = k2_core::overlay::SystemNotice::action("calendar.link_request");
    notice.actions.push(k2_core::overlay::NoticeAction {
        id: LINK_CALENDAR_ACTION.to_string(),
        label: "Link read-only".to_string(),
        route: None,
        body: Some(json!({ "address": address, "provider": provider, "workspace": path })),
        workspace: Some(path.clone()),
        done_at: None,
        done_by: None,
    });
    match crate::overlay_routes::post_workspace_notice(&path, &text, notice) {
        Ok(_) => ok_json(json!({
            "ok": true,
            "address": address,
            "provider": provider,
            "posted": true,
            "hint": "a 'Link your calendar, read-only' card is on your Thread; your human links it",
        })),
        Err(e) => cal::engine_err(format!("posting the card: {e}")),
    }
}
