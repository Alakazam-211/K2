//! E2 — the important held requests (PRD rev 3.1 §5.2, Appendix A2):
//!
//! | Action | Floor | Proof | Route entry `apply` calls |
//! |---|---|---|---|
//! | `apps.approve request=` | Owner | `app-approve` | `apps::approvals::handle("/cli/app/approve")` |
//! | `mail.approve message=` | Admin | click | `mail::routes_send::handle_approvals_approve` |
//! | `sources.approve source= spec=` (plain sources; a list is one batch) | Admin | click | `sources::approve_routes::handle("/cli/sources/{approve,batch-approve}")` |
//! | `garden.proposal.approve proposal=` | GardenOwner | click | `zen_agent_routes::approve_entry` (E1's `garden_owner_may`) |
//!
//! `compute.admin-approve` lives in [`super::compute`].
//!
//! **Call, don't edit.** Every `apply` runs the Settings route's own
//! entry with the answerer as the caller (§6.4, C15), so the route's own
//! checks (owner-only, the confirm bound to the request, "already
//! decided", the spec the person reviewed) run again at the click. Where a
//! route's floor lives only in `route_policy` (mail and sources approve,
//! both Admin), [`route_floor`] applies that same table row first, so a
//! below-floor login is refused through both doors.
//!
//! **The held id.** `apps.approve` keeps the App request's own id
//! (`ParamKind::HeldRequest` → the request's `held_ref`), so the card and
//! the agent's turn say `[permission:w_…]` (§3.2: "approving either one
//! approves both"). The asked-for values must be the asking workspace's
//! own: another workspace's held item reads as "no such item of yours".
//!
//! **Current state (§6.2, Rosson Q1).** Approved elsewhere (in Settings,
//! on the CLI) is `AlreadyDone`; denied, withdrawn, expired, failed or
//! changed since the ask is `Moved` (`bad_params` at the ask, `stale` at
//! the click).
//!
//! **Sources.** Only a *plain* source is card-able: no secret to bind, no
//! LAN or plain-http address (`approval_kind == "one_click"`). The card is
//! bound to the spec the agent asked about (`spec=`, the `specHash` from
//! `k2 sources pending`; its first [`SPEC_PREFIX_MIN`] characters are taken
//! too, a fallback from before the value check let 64-hex content hashes
//! through): a source edited after the ask is `stale`, and the route's own
//! `spec_changed` check backs that up (the click sends the full hash it
//! matched).

use std::collections::HashMap;

use k2_core::app_grace::requests as app_reqs;
use k2_core::connect_users::Role;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardable, NotCardableSpec, ParamFrom, ParamKind,
    ParamSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::sources::spec::SourceStatus;
use serde_json::{json, Value};

use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::compute_routes::Who;

// ── shared with compute.rs ──────────────────────────────────────────

/// The answerer as the route's own caller: the owner token, or the login
/// with its role (an unknown role reads as Member, the lowest).
pub(crate) fn who_of(a: &Answerer) -> Who {
    match a {
        Answerer::OwnerToken => Who::Owner,
        Answerer::Login { user, role } => {
            Who::Login { username: user.clone(), role: Role::parse_wire(role).unwrap_or(Role::Member) }
        }
    }
}

/// The name a route records as the decider (`owner`, or the login).
fn decider(a: &Answerer) -> String {
    match a {
        Answerer::OwnerToken => "owner".to_string(),
        Answerer::Login { user, .. } => user.clone(),
    }
}

/// The route's own POST floor from `route_policy` (the table `role_gate`
/// reads), for routes whose handler takes no caller. The owner token
/// passes, as it does at the dispatcher. A route with no POST row refuses
/// (fail closed).
pub(crate) fn route_floor(path: &str, a: &Answerer) -> Result<(), Applied> {
    let Answerer::Login { role, .. } = a else { return Ok(()) };
    let role = Role::parse_wire(role).unwrap_or(Role::Member);
    let Some(floor) = crate::routes::route_policy::lookup(path).and_then(|r| r.post) else {
        return Err(Applied::Refused { code: "route_unclassified".into(), message: format!("{path} has no POST row in the route policy") });
    };
    if floor.admits(role) {
        Ok(())
    } else {
        Err(Applied::Refused {
            code: "role_required".into(),
            message: format!("this needs an {} login on this server; you are signed in as {}", floor.as_wire(), role.as_wire()),
        })
    }
}

/// One text value of the request (`None` when missing or not a string).
pub(crate) fn text_param<'a>(ctx: &'a Ctx, key: &str) -> Option<&'a str> {
    ctx.params.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A list value (`["a","b"]`, or one string as a one-item list).
fn list_param(ctx: &Ctx, key: &str) -> Vec<String> {
    match ctx.params.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => Vec::new(),
    }
}

/// `{"error":{"code","hint"}}` or `{"error":"code","message"}` → (code, hint).
pub(crate) fn error_of(v: &Value, fallback: &str) -> (String, String) {
    let code = v
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| v.get("error").and_then(Value::as_str))
        .unwrap_or(fallback)
        .to_string();
    let hint = v
        .pointer("/error/hint")
        .and_then(Value::as_str)
        .or_else(|| v.get("message").and_then(Value::as_str))
        .or_else(|| v.get("hint").and_then(Value::as_str))
        .unwrap_or("the route refused it")
        .to_string();
    (code, hint)
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn)
}

fn now() -> i64 {
    k2_core::overlay::now_secs()
}

const ONE_TIME: &str = "once";

const fn one(key: &'static str, kind: ParamKind) -> ParamSpec {
    ParamSpec { key, kind, required: true, list: false, choices: &[] }
}

// ── apps.approve ────────────────────────────────────────────────────

pub(crate) static APPS_APPROVE: ActionSpec = ActionSpec {
    id: "apps.approve",
    area: Area::Held,
    params: &[one("request", ParamKind::HeldRequest)],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::AppApprove },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "App Approvals",
        what_changes: "Approve {ws}'s held App request {request}",
        consequences: "The change K2 held for the owner runs now, exactly as listed in Settings → App Approvals; open it there to see what it does.",
        scope: "request {request}",
        duration: ONE_TIME,
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "app-approvals", path: "Settings → App Approvals" },
    walls: &[
        // Every App hold (switch 2 off, a missing core, new caps, a --cmd
        // service, egress, a box service, budget, clean-mode off).
        Wall {
            matches: WallMatch::Held { status: 202, field: "status", value: "held" },
            route_prefix: "/cli/app/",
            method: WallMethod::Any,
            params: &[("request", ParamFrom::Response("/request/id"))],
        },
        // An agent trying to approve its own held request.
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: "/cli/app/approve",
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Body("/request"))],
        },
    ],
    coverage: WallCoverage::Held,
    machine_hints: &[],
};

fn app_request(id: &str) -> Option<app_reqs::AppRequest> {
    db(|c| app_reqs::get(c, id)).ok().flatten()
}

fn check_app(ctx: &Ctx) -> Check {
    let Some(id) = text_param(ctx, "request") else {
        return Check::Moved("name the held App request: request=<its w_… id> (k2 app approvals)".into());
    };
    let r = match app_request(id) {
        Some(r) if r.requester_workspace.as_deref() == Some(ctx.workspace_id.as_str()) => r,
        _ => return Check::Moved(format!("no held App request {id} of {}'s (k2 app approvals shows the full w_… ids)", ctx.workspace_handle)),
    };
    match r.status.as_str() {
        app_reqs::STATUS_PENDING if r.expires_at > now() => Check::Needed,
        app_reqs::STATUS_PENDING | app_reqs::STATUS_EXPIRED => Check::Moved(format!("App request {id} expired unanswered (7 days)")),
        app_reqs::STATUS_APPROVED => Check::AlreadyDone,
        app_reqs::STATUS_FAILED => Check::Moved(format!("App request {id} was approved already, but applying it failed (k2 app approvals)")),
        other => Check::Moved(format!("App request {id} was {other} meanwhile")),
    }
}

fn apply_app(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let Some(id) = text_param(ctx, "request") else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no held App request".into() };
    };
    let mut body = json!({ "request": id });
    if let Some(t) = token {
        body["confirmToken"] = json!(t);
    }
    match crate::apps::approvals::handle("/cli/app/approve", &who_of(who), body.to_string().as_bytes(), &HashMap::new()) {
        Ok(v) if v["status"] == app_reqs::STATUS_APPROVED => Applied::Done { message: "It applies now.".into() },
        Ok(v) => {
            let (code, hint) = error_of(&v, "apply_failed");
            Applied::Refused { code, message: format!("App request {id} was approved, but applying it failed: {hint}") }
        }
        Err(r) => Applied::Refused { code: r.code.to_string(), message: r.hint },
    }
}

// ── mail.approve ────────────────────────────────────────────────────

const MAIL_APPROVE_ROUTE: &str = "/cli/mail/approvals/approve";

pub(crate) static MAIL_APPROVE: ActionSpec = ActionSpec {
    id: "mail.approve",
    area: Area::Held,
    params: &[one("message", ParamKind::Message)],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Email approvals",
        what_changes: "Send {ws}'s held email {message}",
        consequences: "The message leaves exactly as it was queued: the same sender, recipients and text. Read it first in Settings → Email Hosting → Approvals.",
        scope: "message {message}",
        duration: ONE_TIME,
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "email-hosting", path: "Settings → Email Hosting → Approvals" },
    walls: &[
        // A send held for approval (`agentSend: approval`).
        Wall {
            matches: WallMatch::Held { status: 200, field: "status", value: "pending_approval" },
            route_prefix: "/cli/mail/",
            method: WallMethod::Any,
            params: &[("message", ParamFrom::Response("/id"))],
        },
        // An agent trying to approve its own queued send.
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: MAIL_APPROVE_ROUTE,
            method: WallMethod::Post,
            params: &[("message", ParamFrom::Body("/id"))],
        },
    ],
    coverage: WallCoverage::Held,
    machine_hints: &[],
};

/// `(owner_project_id, status, created_at)` of a queued send.
fn outbound(id: &str) -> Option<(String, String, i64)> {
    db(|c| {
        c.query_row(
            "SELECT owner_project_id, status, created_at FROM mail_outbound WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    })
}

fn check_mail(ctx: &Ctx) -> Check {
    let Some(id) = text_param(ctx, "message") else {
        return Check::Moved("name the held email: message=<its out_… id> (k2 mail outbox)".into());
    };
    let (status, created) = match outbound(id) {
        Some((owner, status, created)) if owner == ctx.workspace_id => (status, created),
        _ => return Check::Moved(format!("no held email {id} of {}'s (k2 mail outbox)", ctx.workspace_handle)),
    };
    match status.as_str() {
        "pending" if created + crate::mail::send::APPROVAL_EXPIRE_SECS > now() => Check::Needed,
        "pending" => Check::Moved(format!("email {id} waited more than 7 days and was rejected")),
        "approved" | "sent" | "scheduled" => Check::AlreadyDone,
        "denied" => Check::Moved(format!("email {id} was rejected meanwhile")),
        other => Check::Moved(format!("email {id} is {} now", crate::mail::send::wire_status(other))),
    }
}

fn apply_mail(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let Some(id) = text_param(ctx, "message") else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no held email".into() };
    };
    if let Err(refused) = route_floor(MAIL_APPROVE_ROUTE, who) {
        return refused;
    }
    let body = json!({ "id": id, "decidedBy": decider(who) });
    let resp = crate::mail::routes_send::handle_approvals_approve(body.to_string().as_bytes());
    let v: Value = serde_json::from_str(&resp.body).unwrap_or(Value::Null);
    if !resp.status.starts_with("200") {
        let (code, hint) = error_of(&v, "mail_refused");
        return Applied::Refused { code, message: hint };
    }
    match v["status"].as_str() {
        Some("scheduled") => Applied::Done {
            message: format!("It leaves after {}.", v["sendAfterRfc3339"].as_str().unwrap_or("its scheduled time")),
        },
        _ => Applied::Done { message: "It is handed to the mail server now.".into() },
    }
}

// ── sources.approve ─────────────────────────────────────────────────

const SOURCES_APPROVE_ROUTE: &str = "/cli/sources/approve";
const SOURCES_BATCH_ROUTE: &str = "/cli/sources/batch-approve";
const NOT_PLAIN: &str = "needs a secret or a LAN / plain-http address, so a person approves it in Settings → Gardens → External Data. It can't be requested.";

pub(crate) static SOURCES_APPROVE: ActionSpec = ActionSpec {
    id: "sources.approve",
    area: Area::Held,
    params: &[
        ParamSpec { key: "source", kind: ParamKind::Source, required: true, list: true, choices: &[] },
        // The specHash of each source (or its first 32 characters), in the
        // same order (k2 sources pending).
        ParamSpec { key: "spec", kind: ParamKind::Version, required: true, list: true, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "External Data",
        what_changes: "Approve {ws}'s data source {source}",
        consequences: "K2 starts fetching it on its schedule, from the public addresses it was proposed with, as proposed. No secret is bound. Review it first in Settings → Gardens → External Data.",
        scope: "data source {source}",
        duration: ONE_TIME,
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "data-sources", path: "Settings → Gardens → External Data" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "person_only" },
            route_prefix: SOURCES_APPROVE_ROUTE,
            method: WallMethod::Post,
            params: &[("source", ParamFrom::Body("/name")), ("spec", ParamFrom::Body("/specHash"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: "person_only" },
            route_prefix: SOURCES_BATCH_ROUTE,
            method: WallMethod::Post,
            params: &[],
        },
    ],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

/// The shortest `spec=` taken: 32 hex characters (128 bits) of the
/// sha-256 `specHash`.
pub(crate) const SPEC_PREFIX_MIN: usize = 32;

/// `spec` names this spec hash: the full hash, or a prefix of at least
/// [`SPEC_PREFIX_MIN`] characters.
fn spec_matches(hash: &str, spec: &str) -> bool {
    spec.len() >= SPEC_PREFIX_MIN && hash.starts_with(spec)
}

/// The asked-for `(name, specHash)` pairs, or why they don't pair up.
fn source_pairs(ctx: &Ctx) -> Result<Vec<(String, String)>, String> {
    let names = list_param(ctx, "source");
    let specs = list_param(ctx, "spec");
    if names.is_empty() {
        return Err("name the source: source=<name> (k2 sources pending)".into());
    }
    if names.len() != specs.len() {
        return Err(format!(
            "give one spec= per source=, in the same order: the specHash k2 sources pending shows ({} source(s), {} spec(s))",
            names.len(),
            specs.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for n in &names {
        if !seen.insert(n.as_str()) {
            return Err(format!("source {n} is named twice"));
        }
    }
    Ok(names.into_iter().zip(specs).collect())
}

fn check_sources(ctx: &Ctx) -> Check {
    let pairs = match source_pairs(ctx) {
        Ok(p) => p,
        Err(e) => return Check::Moved(e),
    };
    let mut done: Vec<&str> = Vec::new();
    for (name, spec) in &pairs {
        let row = db(|c| k2_core::sources::store::get_by_name(c, name)).ok().flatten();
        let Some(row) = row.filter(|r| r.made_by_project.as_deref() == Some(ctx.workspace_id.as_str())) else {
            return Check::Moved(format!("no source {name} of {}'s (k2 sources pending)", ctx.workspace_handle));
        };
        match row.status {
            SourceStatus::Active if row.approved_spec_hash.as_deref().is_some_and(|h| spec_matches(h, spec)) => {
                done.push(name);
                continue;
            }
            SourceStatus::Proposed => {}
            _ => return Check::Moved(format!("source {name} isn't waiting for approval any more")),
        }
        if !spec_matches(&row.spec_hash, spec) {
            return Check::Moved(format!("source {name} changed since it was asked (or spec= isn't its specHash); ask again with its current specHash"));
        }
        if crate::sources::approve_routes::util::approval_kind(&row) != "one_click" {
            return Check::Moved(format!("source {name} {NOT_PLAIN}"));
        }
    }
    if done.len() == pairs.len() {
        Check::AlreadyDone
    } else if !done.is_empty() {
        Check::Moved(format!("{} {} approved meanwhile; ask again for the rest", done.join(", "), if done.len() == 1 { "was" } else { "were" }))
    } else {
        Check::Needed
    }
}

fn apply_sources(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let pairs = match source_pairs(ctx) {
        Ok(p) => p,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    // The full hash each source has now, when it is still the one asked
    // about; the route refuses it if the source changes after this read.
    let mut full: Vec<(String, String)> = Vec::with_capacity(pairs.len());
    for (name, spec) in &pairs {
        let row = db(|c| k2_core::sources::store::get_by_name(c, name)).ok().flatten();
        match row {
            Some(r) if spec_matches(&r.spec_hash, spec) => full.push((name.clone(), r.spec_hash)),
            _ => {
                return Applied::Refused { code: "spec_changed".into(), message: format!("source {name} changed since it was asked") }
            }
        }
    }
    let (path, body) = match full.as_slice() {
        [(name, spec)] => (SOURCES_APPROVE_ROUTE, json!({ "name": name, "specHash": spec })),
        many => (
            SOURCES_BATCH_ROUTE,
            json!({ "items": many.iter().map(|(n, s)| json!({ "name": n, "specHash": s })).collect::<Vec<_>>() }),
        ),
    };
    if let Err(refused) = route_floor(path, who) {
        return refused;
    }
    match crate::sources::approve_routes::handle(path, &who_of(who), body.to_string().as_bytes(), &HashMap::new()) {
        Ok(_) if pairs.len() == 1 => Applied::Done { message: "K2 starts fetching it now.".into() },
        Ok(_) => Applied::Done { message: format!("K2 starts fetching all {} now.", pairs.len()) },
        Err(e) => Applied::Refused { code: e.code.to_string(), message: e.hint },
    }
}

// ── garden.proposal.approve ─────────────────────────────────────────

/// The Garden's owner from any signed-in surface (§5.2.1): the owner token
/// or an Owner login, through E1's `zen_routes::garden_owner_may`, the one
/// resolver Settings calls too.
pub(crate) static GARDEN_PROPOSAL_APPROVE: ActionSpec = ActionSpec {
    id: "garden.proposal.approve",
    area: Area::Held,
    params: &[one("proposal", ParamKind::Proposal)],
    floor: Floor::GardenOwner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Gardens",
        what_changes: "Apply {ws}'s Garden proposal {proposal}",
        consequences: "The proposed change is written to the Garden or widget now, exactly as proposed; a widget runs the new code. Read it first in Settings → Gardens.",
        scope: "proposal {proposal}",
        duration: ONE_TIME,
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "zen-gardens", path: "Settings → Gardens" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "person_only" },
        route_prefix: "/cli/zen/proposals/approve",
        method: WallMethod::Post,
        params: &[("proposal", ParamFrom::Body("/id"))],
    }],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

fn check_proposal(ctx: &Ctx) -> Check {
    let Some(id) = text_param(ctx, "proposal") else {
        return Check::Moved("name the Garden proposal: proposal=<its zp-… id> (k2 zen proposals)".into());
    };
    let p = match db(|c| k2_core::zen::agent_edit::get(c, id)).ok().flatten() {
        Some(p) if p.made_by_project == ctx.workspace_id => p,
        _ => return Check::Moved(format!("no Garden proposal {id} of {}'s (k2 zen proposals)", ctx.workspace_handle)),
    };
    match p.status.as_str() {
        "pending" => Check::Needed,
        "approved" => Check::AlreadyDone,
        other => Check::Moved(format!("Garden proposal {id} was {other} meanwhile")),
    }
}

fn apply_proposal(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let Some(id) = text_param(ctx, "proposal") else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no Garden proposal".into() };
    };
    let r = crate::zen_agent_routes::approve_entry(&super::switches::as_caller(who), id);
    super::switches::applied(&r, "It is written now.")
}

// ── the registry ────────────────────────────────────────────────────

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &APPS_APPROVE, check: check_app, apply: apply_app },
        Registered { spec: &MAIL_APPROVE, check: check_mail, apply: apply_mail },
        Registered { spec: &SOURCES_APPROVE, check: check_sources, apply: apply_sources },
        Registered { spec: &GARDEN_PROPOSAL_APPROVE, check: check_proposal, apply: apply_proposal },
    ]
}

static VAULT_INCOMING: NotCardableSpec = NotCardableSpec {
    id: "vault.incoming.accept",
    area: Area::Held,
    why: NotCardable::Dropped,
    r#where: "the secret handoff card already on the Thread (a person answers it)",
};

/// This area's rows that aren't card-able in v1 (§5.4). `clone.approve` and
/// the `sources.*` rows are E1's.
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    vec![&VAULT_INCOMING]
}

#[cfg(test)]
#[path = "held_tests.rs"]
pub(crate) mod tests;
