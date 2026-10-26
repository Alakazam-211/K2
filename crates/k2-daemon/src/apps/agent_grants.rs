//! An agent's grants on its OWN workspace's Apps (audit-app-agent-access-0464
//! §4, Rosson 2026-10-26: "agents must be able to make test users").
//!
//! | step | with "Let this agent manage its Apps' users" (`apps.manage-users`, 0225) | without |
//! |---|---|---|
//! | test guests (`k2 app user add <app> <name> --test`): add, its own rooms, the App's own roles; then password and the rest through `k2 app user …` | the agent, at once | one held request + card ([`hold`]); "Always allow" turns the switch on |
//! | remove its own test guests | the agent | the agent with "Build and run Apps" (removing narrows) |
//! | the guests-rooms grant (`k2 app grant <app> --kind guests-rooms`) in its own room | the agent, at once | held + card (Always allow) |
//! | the guests-rooms grant naming another workspace's room | held + card | held + card |
//! | copy box logins onto its App (`k2 app grant copy-host <app>`) | held + card: it copies the box's people, other workspaces' and the owner's | held + card |
//! | a label another workspace holds, or an unattributed hostname it has no record of (`label take/return`, `publish subdomain claim`) | held `label_transfer` + card (any switch) | held + card |
//!
//! App users themselves (`k2 app user add/remove/password/… --app`) are the
//! same switch, through the skin-manage door
//! (`skin_routes::agent_may_manage_app_users`).
//!
//! Never a box (host) login, never another workspace's App, guest or room.
//! A test guest an agent makes (or one an approval made for it) is stamped
//! and tied to its workspace (`skin::creators`): it is never "a person"
//! for person-only cards.
//!
//! **Held.** Each owner step is one held `manage` request (`app_widenings`,
//! the Approvals flow: Settings → App Approvals, `k2 app approve` outside
//! K2), with the values fixed at request time, and K2 presents it as an
//! `apps.approve` card on the agent's Thread, permission-cards Beta or not
//! (`super::held_cards`). The approval runs exactly what it showed:
//! - copy-host: the box logins it would copy now must be exactly the ones
//!   listed (compared and written in one transaction), else nothing;
//! - the guests-rooms grant: the App (by service id, still the
//!   workspace's) and exactly those rooms;
//! - a test guest: the agent's own call replayed as the agent (every
//!   test-guest rule runs again); its password is never stored with the
//!   request, so the agent sets one (`k2 app user password <name>
//!   --password-stdin`);
//! - a label transfer: only while the label's holder is still the one the
//!   request named; the move is the owner's audited transfer.

use std::collections::HashMap;

use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::{codes, store};
use serde_json::{json, Value as J};

use super::grace::{target, Target};
use super::routes::{actor, agent_ws, json_body, need_credential, no, s, ws_handle, Who, R};

/// `k2 app grant copy-host <app>`, the passport-aware door (the owner's
/// older `POST /cli/skin/grants/copy-host` stays as it was).
pub const COPY_HOST: &str = "/cli/app/grant/copy-host";

/// The routes a held `manage` request carries for these steps (never HTTP
/// routes: the approval runs [`apply`], as the owner who approved).
pub const COPY_HOST_APPLY: &str = "/cli/app/grant/copy-host/apply";
pub const GUEST_ROOMS_APPLY: &str = "/cli/app/grant/apply";
pub const LABEL_TRANSFER_APPLY: &str = "/cli/app/label/transfer/apply";
/// Replayed as the requesting agent on approval (`approvals::apply_door`).
pub const TEST_GUEST_ADD: &str = "/cli/app/test-guest/add";

/// The held routes K2 presents as Thread cards (`super::held_cards`).
pub const CARD_ROUTES: &[&str] = &[COPY_HOST_APPLY, GUEST_ROOMS_APPLY, LABEL_TRANSFER_APPLY, TEST_GUEST_ADD];

/// The switch's name in hints.
pub const SWITCH_NAME: &str = "Let this agent manage its Apps' users";

/// 403 `app_users_off`: the App-user switch is off for this agent.
pub(crate) fn users_off(what: &str) -> Refusal {
    no(
        crate::skin_routes::APP_USERS_OFF,
        403,
        format!(
            "{what} needs \"{SWITCH_NAME}\" for this agent (off): ask your owner to turn it on (Settings → Workspaces → \
             <this agent> → Agent → Apps, or k2 app access <ws> --users on from a terminal outside K2)"
        ),
    )
}

/// A roster-door refusal (`CliResponse`) as an App-route refusal.
pub(crate) fn refusal_of(r: &crate::cli_response::CliResponse) -> Refusal {
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let code = v.pointer("/error/code").and_then(J::as_str).or_else(|| v.get("error").and_then(J::as_str)).unwrap_or("forbidden");
    let hint = v.pointer("/error/hint").and_then(J::as_str).or_else(|| v.get("hint").and_then(J::as_str)).unwrap_or("refused").to_string();
    let status = r.status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(403);
    let code: &'static str = match code {
        "owner_only" => codes::OWNER_ONLY,
        "app_users_off" => crate::skin_routes::APP_USERS_OFF,
        _ => codes::FORBIDDEN,
    };
    no(code, status, hint)
}

/// May this agent manage its own Apps' users itself: "Let this agent
/// manage its Apps' users" on for its workspace, or the replay of a
/// `manage` request the owner approved for this call.
pub(crate) fn may_self(who: &Who) -> bool {
    match agent_ws(who) {
        Some(ws) => k2_core::workspace::settings::agents_can_manage_app_users(ws) || super::approvals::manage_approved_now(),
        None => false,
    }
}

/// Room ids from handles or ids; unknown ones are a 400.
pub(crate) fn resolve_rooms(tokens: &[String]) -> R<Vec<String>> {
    let (ids, unknown) = k2_core::skin::guest_rooms::resolve_rooms(tokens);
    if !unknown.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, format!("unknown room: {}", unknown.join(", "))));
    }
    Ok(ids)
}

/// Are all `ids` the workspace's own room?
pub(crate) fn own_rooms(ws: &str, ids: &[String]) -> bool {
    ids.iter().all(|id| id == ws)
}

/// An agent names rooms inside its own workspace only (a test guest's).
pub(crate) fn own_rooms_only(ws: &str, ids: &[String]) -> R<()> {
    let outside: Vec<String> = ids.iter().filter(|id| id.as_str() != ws).map(|id| ws_handle(id)).collect();
    if outside.is_empty() {
        return Ok(());
    }
    Err(no(
        codes::OWNER_ONLY,
        403,
        format!(
            "rooms outside your workspace ({}) are the owner's to hand out: an agent gives its guests its own room only",
            outside.join(", ")
        ),
    ))
}

/// A list field: an array of strings, or one comma string.
pub(crate) fn list_field(v: &J, key: &str) -> Option<Vec<String>> {
    match v.get(key)? {
        J::Array(a) => Some(a.iter().filter_map(J::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        J::String(t) => Some(t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        _ => None,
    }
}

fn app_json(t: &Target) -> J {
    json!({
        "serviceId": t.service_id,
        "name": t.name,
        "handle": format!("{}/{}", ws_handle(&t.project_id), t.name),
        "appId": t.app_id,
    })
}

/// The runtime App behind a target, if it is one (the held request's App).
fn app_row_of(t: &Target) -> Option<store::AppRow> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app_by_service(&conn, &t.service_id).ok().flatten()
}

/// What one held owner step is.
pub(crate) struct Step<'a> {
    pub who: &'a Who,
    pub app: Option<&'a store::AppRow>,
    pub label: &'a str,
    pub workspace: &'a str,
    pub route: &'static str,
    pub body: J,
    pub lines: Vec<String>,
    pub summary: String,
    pub why: String,
    pub reason: Option<&'a str>,
}

/// File one held `manage` request for the owner and present it as a card
/// on the agent's Thread. The answer is 202 `held` (exit 2 in the CLI)
/// and says where the owner sees it, and which switch would have let the
/// agent do it itself.
/// At most this many open held requests of one kind (route) per agent
/// workspace; the next is 429 `too_many_pending` (review 0.46.4).
pub const MAX_PENDING_PER_KIND: usize = 5;

/// How many held requests of `route` the workspace has open now.
fn pending_of(ws: &str, route: &str) -> R<usize> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let f = reqs::Filter { app_id: None, service_id: None, workspace: None, status: Some(reqs::STATUS_PENDING) };
    let rows = reqs::list(&conn, &f, 1000).map_err(|e| no("db_error", 500, e.to_string()))?;
    Ok(rows
        .iter()
        .filter(|r| r.requester_workspace.as_deref() == Some(ws) && r.body.get("route").and_then(J::as_str) == Some(route))
        .count())
}

pub(crate) fn hold(step: Step<'_>, switch_hint: bool) -> R<J> {
    let open = pending_of(step.workspace, step.route)?;
    if open >= MAX_PENDING_PER_KIND {
        return Err(no(
            "too_many_pending",
            429,
            format!(
                "{open} requests like this already wait for the owner (at most {MAX_PENDING_PER_KIND}): withdraw one (k2 app deny <id>) or wait for the owner"
            ),
        ));
    }
    let r = super::approvals::file(&super::approvals::Filing {
        who: step.who,
        kind: reqs::KIND_MANAGE,
        app: step.app,
        label: step.label,
        workspace: step.workspace,
        body: json!({ "route": step.route, "body": step.body }),
        diff: json!({ "lines": step.lines, "route": step.route }),
        summary: step.summary,
        reasons: vec![step.why],
        reason: step.reason,
    })?;
    let card = super::held_cards::present_for(&r);
    let mut out = super::approvals::held_answer(&r);
    let shown = match card.get("card").and_then(J::as_str) {
        Some(_) => "the owner sees it as a card on your Thread (and in Settings → App Approvals)",
        None => "the owner sees it in Settings → App Approvals",
    };
    out["card"] = card;
    out["hint"] = json!(format!(
        "held for the owner: {shown}. They approve it once, in person (or k2 app approve {} from a terminal outside K2); \
         you get a k2 msg when it's decided.{}",
        r.id,
        if switch_hint {
            format!(" With \"{SWITCH_NAME}\" on for this agent you do this yourself (Settings → Workspaces → <this agent> → Agent → Apps); the card's Always allow turns it on.")
        } else {
            String::new()
        }
    ));
    Ok(out)
}

// ── copy-host ───────────────────────────────────────────────────────

fn sorted(mut v: Vec<String>) -> Vec<String> {
    for s in v.iter_mut() {
        *s = s.trim().to_ascii_lowercase();
    }
    v.sort();
    v.dedup();
    v
}

/// Copy the box logins onto `t`, exactly `expected` when given (one
/// transaction: a changed list writes nothing, 409 `logins_changed`).
pub(crate) fn copy_to(t: &Target, by: &str, dry_run: bool, expected: Option<&[String]>) -> R<J> {
    let report = match expected {
        Some(want) if !dry_run => {
            k2_core::skin::copy_host_grants_to_app_expect(&t.service_id, Some(&t.project_id), by, want).map_err(|e| match e {
                k2_core::skin::CopyHostExpectError::Changed { now } => {
                    let mut r = no(
                        "logins_changed",
                        409,
                        format!(
                            "the box logins to copy changed since the request ({} now: {}): nothing copied; ask again so the \
                             owner sees them",
                            now.len(),
                            if now.is_empty() { "none".to_string() } else { now.join(", ") }
                        ),
                    );
                    r.extra = Some(json!({ "now": now }));
                    r
                }
                k2_core::skin::CopyHostExpectError::Store(e) => no(codes::BAD_REQUEST, 400, e),
            })?
        }
        _ => k2_core::skin::copy_host_grants_to_app_in(&t.service_id, Some(&t.project_id), by, dry_run)
            .map_err(|e| no(codes::BAD_REQUEST, 400, e))?,
    };
    let kept: Vec<J> = report.kept_existing.iter().map(|(u, enabled)| json!({ "username": u, "enabled": enabled })).collect();
    let mut out = json!({
        "ok": true,
        "app": report.app,
        "appInfo": app_json(t),
        "dryRun": report.dry_run,
        "added": report.added,
        "keptExisting": kept,
        "skippedOtherAppRole": report.skipped_other_app_role,
    });
    if !report.skipped_other_workspace_agent.is_empty() {
        out["skippedOtherWorkspaceAgent"] = json!(report.skipped_other_workspace_agent);
    }
    Ok(out)
}

/// POST `/cli/app/grant/copy-host {app, dryRun?, reason?}`: the owner
/// copies at once; an agent's (its own workspace's App only) is one held
/// request listing exactly the logins, whatever its switches.
pub(crate) fn copy_host(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let dry_run = v.get("dryRun").or_else(|| v.get("dry_run")).and_then(J::as_bool).unwrap_or(false);
    if matches!(who, Who::Login { role, .. } if *role != Role::Owner) {
        return Err(no(codes::OWNER_ONLY, 403, "only the server's owner copies box logins onto an App (never an Admin or Member login)"));
    }
    // A passport sees only its own workspace's Apps (`target`).
    let t = target(who, s(&v, "app").or_else(|| s(&v, "target")))?;
    let by = actor(who);
    let Some(ws) = agent_ws(who).map(str::to_string) else {
        return copy_to(&t, &by, dry_run, None);
    };
    let look = copy_to(&t, &by, true, None)?;
    let added = sorted(look["added"].as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default());
    if dry_run || added.is_empty() {
        // Who has a box login is the owner's to know: without "Build and
        // run Apps" a look says how many, not who (review 0.46.4).
        if super::access::level(&ws) == super::access::Level::Off {
            return Ok(json!({
                "ok": true,
                "app": look["app"],
                "appInfo": look["appInfo"],
                "dryRun": true,
                "addedCount": added.len(),
                "note": "names need \"Build and run Apps\"; the owner sees them on the card",
            }));
        }
        return Ok(look);
    }
    let handle = format!("{}/{}", ws_handle(&t.project_id), t.name);
    let app = app_row_of(&t);
    hold(
        Step {
            who,
            app: app.as_ref(),
            label: &t.name,
            workspace: &ws,
            route: COPY_HOST_APPLY,
            body: json!({ "service": t.service_id, "app": handle, "workspace": ws, "users": added }),
            lines: vec![
                format!("copy {} box login(s) onto {handle}: {}", added.len(), added.join(", ")),
                "each gets App access on it like their box login (no new box login); nothing is copied if the list changed".into(),
            ],
            summary: format!("copy {} box login(s) onto {handle}: {}", added.len(), added.join(", ")),
            why: "copying box logins onto an App is the owner's: they are the box's people, other workspaces' and yours".into(),
            reason: s(&v, "reason"),
        },
        false,
    )
}

// ── the guests-rooms grant ──────────────────────────────────────────

/// An agent's guests-rooms grant (`guest_rooms::give`): at once with
/// "Let this agent manage its Apps' users" in its own room; held otherwise. `Ok(None)` = go on (the
/// agent may do it itself).
pub(crate) fn guest_rooms_agent(who: &Who, ws: &str, t: &Target, rooms: &[String], reason: Option<&str>) -> R<Option<J>> {
    let own = own_rooms(ws, rooms);
    if own && may_self(who) {
        return Ok(None);
    }
    let handle = format!("{}/{}", ws_handle(&t.project_id), t.name);
    let room_handles: Vec<String> = rooms.iter().map(|r| ws_handle(r)).collect();
    let app = app_row_of(t);
    let shown = if room_handles.is_empty() { "its own room".to_string() } else { room_handles.join(", ") };
    hold(
        Step {
            who,
            app: app.as_ref(),
            label: &t.name,
            workspace: ws,
            route: GUEST_ROOMS_APPLY,
            body: json!({ "service": t.service_id, "app": handle, "workspace": ws, "rooms": rooms }),
            lines: vec![
                format!("let {handle} set its own guests' rooms (guests-rooms): {shown}"),
                format!(
                    "then any of {handle}'s guests, test guests {} holds included, can be given {shown}",
                    ws_handle(ws)
                ),
            ],
            summary: format!("guests-rooms for {handle}: {shown}"),
            why: if own {
                format!("\"{SWITCH_NAME}\" is off for this agent")
            } else {
                "a room outside the agent's workspace is the owner's to hand out".to_string()
            },
            reason,
        },
        own,
    )
    .map(Some)
}

// ── test guests ─────────────────────────────────────────────────────

/// A test guest an agent asks for, checked: the normalized name, the
/// App's own role (by id), its own room ids.
pub(crate) struct TestGuestAsk<'a> {
    pub username: &'a str,
    pub role_id: Option<&'a str>,
    pub rooms: &'a [String],
    pub expires: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// An agent's test guest without "Let this agent manage its Apps' users":
/// one held request replaying its own call with the checked values (the
/// App's service id, the role's id, room ids; never a password: the
/// approval's one-time password set is the agent's,
/// `test_guests::password`).
pub(crate) fn test_guest_held(who: &Who, ws: &str, app: &store::AppRow, a: &TestGuestAsk<'_>) -> R<J> {
    let mut body = json!({ "app": app.app_id, "service": app.service_id, "username": a.username, "rooms": a.rooms });
    if let Some(r) = a.role_id {
        body["role"] = json!(r);
    }
    if let Some(e) = a.expires {
        body["expires"] = json!(e);
    }
    let role_name = a.role_id.and_then(|id| k2_core::skin::list_roles().ok()?.into_iter().find(|r| r.id == id).map(|r| r.name));
    let mut what = format!("test guest {} of {}", a.username, app.app_id);
    if let Some(role) = role_name {
        what.push_str(&format!(", role {role}"));
    }
    if !a.rooms.is_empty() {
        what.push_str(&format!(", rooms {}", rooms_csv(a.rooms)));
    }
    what.push_str(&format!(", for {}", a.expires.unwrap_or("24h")));
    hold(
        Step {
            who,
            app: Some(app),
            label: &app.app_id,
            workspace: ws,
            route: TEST_GUEST_ADD,
            body,
            lines: vec![
                format!("make the {what}"),
                "it signs in only through the App's link, ends on its own and is tied to the agent".into(),
            ],
            summary: format!("make the {what}"),
            why: format!("\"{SWITCH_NAME}\" is off for this agent"),
            reason: a.reason,
        },
        true,
    )
}

/// Room handles, comma-joined.
pub(crate) fn rooms_csv(ids: &[String]) -> String {
    ids.iter().map(|id| ws_handle(id)).collect::<Vec<_>>().join(", ")
}

// ── labels ──────────────────────────────────────────────────────────

/// A held label transfer: `move` is `take`, `return` or `claim`; `holder`
/// the workspace holding the label now (`None`: nobody).
pub(crate) fn label_transfer_held(
    who: &Who,
    ws: &str,
    app: Option<&store::AppRow>,
    label: &str,
    move_: &str,
    holder: Option<&str>,
    reason: Option<&str>,
) -> R<J> {
    // A label is a DNS label: never free text on a card.
    let ok = !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err(no(codes::BAD_REQUEST, 400, "a label is 1-63 of a-z, 0-9 and -"));
    }
    let held_by = holder.map(ws_handle).unwrap_or_else(|| "nobody".to_string());
    let target = app.map(|a| a.app_id.clone()).unwrap_or_else(|| ws_handle(ws));
    // What it serves now (the control plane's map), for the owner to see.
    let serves = k2_core::tunnel::subdomains::current().targets.get(label).cloned();
    hold(
        Step {
            who,
            app,
            label: app.map(|a| a.app_id.as_str()).unwrap_or(label),
            workspace: ws,
            route: LABEL_TRANSFER_APPLY,
            body: json!({ "label": label, "move": move_, "workspace": ws, "app": app.map(|a| a.app_id.clone()), "holder": holder }),
            lines: vec![
                format!("{move_} the label {label} for {target} (held by {held_by} now)"),
                match &serves {
                    Some(s) => format!("{label} serves {s} now"),
                    None => format!("{label} serves nothing on this box now"),
                },
                format!(
                    "the label becomes {}'s (an audited transfer): its agent can then point it at its own services and Apps; nothing \
                     moves if its holder changed",
                    ws_handle(ws)
                ),
            ],
            summary: format!("label transfer: {move_} {label} for {target} (held by {held_by})"),
            why: match holder {
                Some(_) => "another workspace holds this label".to_string(),
                None => "no workspace holds this hostname and nothing of the agent's names it".to_string(),
            },
            reason,
        },
        false,
    )
}

// ── the approval ────────────────────────────────────────────────────

fn str_list(v: &J) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// The service a held request names, still `ws`'s.
fn service_of(b: &J, ws: &str) -> R<Target> {
    let svc = b["service"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable held request"))?;
    let t = target(&Who::Owner, Some(svc)).map_err(|_| no(codes::APP_NOT_FOUND, 409, "the App in this request is gone"))?;
    if t.service_id != svc || t.project_id != ws {
        return Err(no(codes::FORBIDDEN, 409, format!("{} isn't the requesting workspace's App any more: ask again", t.name)));
    }
    Ok(t)
}

/// Carry out an approved held step whose route is one of ours, as the
/// owner who approved it (`approvals::apply_door`). `None`: not ours.
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> Option<R<J>> {
    let route = r.body.get("route").and_then(J::as_str)?;
    let b = r.body.get("body").cloned().unwrap_or(J::Null);
    let ws = b["workspace"].as_str().unwrap_or_default().to_string();
    let run = || -> R<J> {
        if ws.is_empty() || r.requester_workspace.as_deref() != Some(ws.as_str()) {
            return Err(no(codes::BAD_REQUEST, 400, "unreadable held request"));
        }
        match route {
            COPY_HOST_APPLY => {
                let t = service_of(&b, &ws)?;
                let users = str_list(&b["users"]);
                copy_to(&t, by, false, Some(&users))
            }
            GUEST_ROOMS_APPLY => {
                let t = service_of(&b, &ws)?;
                let rooms = str_list(&b["rooms"]);
                let body = json!({ "app": t.service_id, "kind": k2_core::skin::guest_rooms::GRANT_KIND, "rooms": rooms }).to_string();
                let resp = super::guest_rooms::handle("/cli/app/grant", &Who::Owner, true, body.as_bytes(), &HashMap::new());
                let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
                if resp.status.starts_with('2') {
                    Ok(v)
                } else {
                    Err(no("grant_failed", 409, v.pointer("/error/hint").and_then(J::as_str).unwrap_or("the grant failed").to_string()))
                }
            }
            LABEL_TRANSFER_APPLY => super::labels::apply_transfer(&b, &ws, by),
            _ => Err(no(codes::BAD_REQUEST, 400, format!("'{route}' isn't an App grant step"))),
        }
    };
    match route {
        COPY_HOST_APPLY | GUEST_ROOMS_APPLY | LABEL_TRANSFER_APPLY => Some(run()),
        _ => None,
    }
}
