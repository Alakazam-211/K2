//! The permissions step of a template hire (prd-agent-templates-v1 §3, §9;
//! slice T3): the daemon half of `k2_core::hire::template::permissions`,
//! installed at start ([`install`]).
//!
//! **A template suggests; the hirer's reach decides.** Each suggestion
//! (and "Can ask for permission cards" when the template has `cards = true`)
//! is, in order:
//! 1. `declined` when the person unticked it on the Review step;
//! 2. `skip` when it is already on for the new agent;
//! 3. `grant` / `done` when the hirer may give it (below);
//! 4. else `card` while the server's permission cards (Beta) are on: a
//!    permission card for a person, filed on the agent's Thread, or on its
//!    first start when it has no Thread yet ([`file_pending_cards`]);
//! 5. else `refused`, with a hint that names who can turn it on and how.
//!    A refusal never fails the hire.
//!
//! **Reach** (who may give what):
//! - **the owner token, an Owner login:** any standing switch. **An Admin
//!   login:** Admin-floor ones; an Owner-floor switch (`apps.build`,
//!   `organization.manage`, `compute.manage`, …) goes to a card. Each grant
//!   runs the Settings route's own entry with that person as the caller (the
//!   action's `apply`, exactly what a card's Allow runs), so the route's
//!   floor and its `agent-access-set` audit line are Settings'.
//! - **A Member login:** may hire, grants nothing.
//! - **An agent passport** (its main session, with "Let this agent hire
//!   agents" on): only a switch it holds on its own workspace, from the
//!   starter-set switches K2 writes in-process (`starter_permissions::
//!   standing_switch`), never an Owner-floor one, never database access or
//!   Manage Chat. "Can ask for permission cards" only while its own is on.
//! - **An API key:** grants nothing.
//!
//! "Can ask for permission cards" counts only while the server's Beta switch
//! is on; with it off the row is `skip`, and every switch the hirer can't
//! give is `refused` (no card can be filed).
//!
//! **Audit:** every grant, card, refusal and decline writes one
//! `auth-audit.jsonl` line (`agent-hire-permission`, the hirer as actor) and
//! one `hire_events` row (same action). A grant also writes the switch's own
//! `agent-access-set` line (the route's, or this module's for an agent
//! hirer), and a filed card its `permission-request` line. A dry run writes
//! nothing.

use k2_core::hire::starter_permissions::{owner_floor, standing_switch};
use k2_core::hire::template::permissions::{targets, Decision, Refusal, StepIn, StepOut, CARDS_ROW};
use k2_core::hire::HireCaller;
use k2_core::permission::catalog::{Floor, Grant};
use serde_json::json;

use crate::permission::actions::{switches, Answerer, Applied, Check, Ctx};

/// The audit event (and `hire_events` action) for every decision.
pub(crate) const AUDIT_EVENT: &str = "agent-hire-permission";

/// Install the step into the template apply (daemon start).
pub fn install() {
    k2_core::hire::template::permissions::install(step);
}

/// The installed step: the server's own Beta switch decides whether cards
/// can be filed.
fn step(inp: &StepIn<'_>) -> StepOut {
    step_with(inp, k2_core::permission::gate::server_on())
}

/// Who hires, as far as reach goes.
#[derive(Debug, Clone)]
enum Hirer {
    Person { who: Answerer, owner: bool, admin: bool },
    Agent { workspace_id: String },
    ApiKey,
}

fn hirer_of(caller: &HireCaller) -> Hirer {
    use k2_core::connect_users::Role;
    match caller {
        HireCaller::Owner => Hirer::Person { who: Answerer::OwnerToken, owner: true, admin: true },
        HireCaller::Login { username } => {
            let role = k2_core::connect_users::role_for_user(username);
            Hirer::Person {
                who: Answerer::Login { user: username.clone(), role: role.map(|r| r.as_wire().to_string()).unwrap_or_default() },
                owner: role == Some(Role::Owner),
                admin: matches!(role, Some(Role::Owner | Role::Admin)),
            }
        }
        HireCaller::Agent { workspace_id } => Hirer::Agent { workspace_id: workspace_id.clone() },
        HireCaller::ApiKey { .. } => Hirer::ApiKey,
    }
}

/// The new agent, once it is registered.
#[derive(Debug, Clone)]
struct Target {
    id: String,
    path: String,
    handle: String,
}

fn target_of(path: &str) -> Option<Target> {
    let (registered, row) = k2_core::hire::converge::lookup_project_row(path)?;
    let handle = k2_core::workspace_session_handles::workspace_address_name_shared(&row.id)
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or(row.handle.clone())
        .unwrap_or_else(|| row.name.clone());
    Some(Target { id: row.id, path: registered, handle })
}

/// The handle a hint names: the agent's, or (a dry run of a new folder)
/// the folder's name, which is what the handle will be.
fn handle_for(path: &str, t: Option<&Target>) -> String {
    match t {
        Some(t) => t.handle.clone(),
        None => std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// Why the hirer couldn't give a switch, in words (`code` for the audit).
#[derive(Debug, Clone)]
struct Cant {
    code: String,
    why: String,
}

fn cant(code: &str, why: impl Into<String>) -> Cant {
    Cant { code: code.to_string(), why: why.into() }
}

/// Who may turn `floor` on, in words.
fn who_turns_on(floor: Floor) -> &'static str {
    match floor {
        Floor::Owner | Floor::GardenOwner => "The server's owner",
        Floor::Admin => "An owner or admin",
    }
}

/// The `/cli/agent-access/set` toggle an action writes, if that is its writer.
fn access_toggle(action: &str) -> Option<&'static str> {
    switches::SWITCHES.iter().find(|s| s.on.id == action).and_then(|s| match s.writer {
        switches::Writer::Access(t) => Some(t),
        _ => None,
    })
}

/// The refused row's hint: who can turn it on, and how (Settings, plus the
/// command when one exists).
fn refused_hint(action: &str, setting: &str, settings_path: &str, floor: Floor, handle: &str) -> String {
    let path = settings_path.replace("{ws}", handle);
    let cmd = match action {
        CARDS_ROW => Some(format!("k2 agent access set {handle} permission_cards on")),
        _ => access_toggle(action).map(|t| format!("k2 agent access set {handle} {t} on")),
    };
    match cmd {
        Some(c) => format!(
            "{} can turn on \"{setting}\" for {handle}: {path}, or run `{c}` in their own terminal (an agent can't).",
            who_turns_on(floor)
        ),
        None => format!("{} can turn on \"{setting}\" for {handle}: {path}.", who_turns_on(floor)),
    }
}

/// Can this hirer give `action` (a standing switch with floor `floor`)?
fn may_give(h: &Hirer, action: &str, floor: Floor) -> Result<(), Cant> {
    match h {
        Hirer::Person { owner, admin, .. } => match floor {
            Floor::Owner | Floor::GardenOwner if !owner => Err(cant("owner_floor", "only the server's owner can turn it on")),
            Floor::Admin if !admin => Err(cant("role", "your role can't turn it on (an owner or admin can)")),
            _ => Ok(()),
        },
        Hirer::Agent { workspace_id } => {
            let Some(switch) = standing_switch(action).filter(|_| !owner_floor(action) && floor == Floor::Admin) else {
                return Err(cant("not_passable", "an agent can't pass this one on"));
            };
            if crate::first_run_grants::switch_on(workspace_id, switch) {
                Ok(())
            } else {
                Err(cant("hirer_lacks", "the hiring agent doesn't have it itself"))
            }
        }
        Hirer::ApiKey => Err(cant("api_key", "an API key turns on no switches")),
    }
}

/// The step with the server's Beta switch given (unit tests pass it; lib
/// tests never flip the process switch).
pub(crate) fn step_with(inp: &StepIn<'_>, cards_on: bool) -> StepOut {
    let hirer = hirer_of(inp.caller);
    let target = target_of(inp.path);
    let handle = handle_for(inp.path, target.as_ref());
    let mut out = StepOut::default();
    for want in targets(inp.cards, inp.suggested) {
        if inp.declined.iter().any(|d| *d == want) {
            out.rows.push(Decision::new(want.clone(), "declined", Some("left off at hire; a re-run leaves it off too".into()), None));
            record(inp, &target, &want, "declined", None);
            continue;
        }
        if want == CARDS_ROW {
            cards_row(inp, &hirer, target.as_ref(), &handle, cards_on, &mut out);
        } else {
            switch_row(inp, &hirer, target.as_ref(), &handle, &want, cards_on, &mut out);
        }
    }
    out
}

/// "Can ask for permission cards" for the new agent (`cards = true`).
fn cards_row(inp: &StepIn<'_>, h: &Hirer, t: Option<&Target>, handle: &str, cards_on: bool, out: &mut StepOut) {
    const SETTING: &str = "Can ask for permission cards";
    if !cards_on {
        out.rows.push(Decision::new(CARDS_ROW, "skip", Some("permission cards are off on this server".into()), None));
        return;
    }
    if t.is_some_and(|t| k2_core::permission::gate::agent_switch_on(&t.id)) {
        out.rows.push(Decision::new(CARDS_ROW, "skip", Some("already on".into()), None));
        return;
    }
    let reach = match h {
        Hirer::Person { admin: true, .. } => Ok(()),
        Hirer::Person { .. } => Err(cant("role", "your role can't turn it on (an owner or admin can)")),
        Hirer::Agent { workspace_id } if k2_core::permission::gate::agent_switch_on(workspace_id) => Ok(()),
        Hirer::Agent { .. } => Err(cant("hirer_lacks", "the hiring agent doesn't have it itself, and it is never asked on a card")),
        Hirer::ApiKey => Err(cant("api_key", "an API key turns on no switches")),
    };
    let refuse = |out: &mut StepOut, c: Cant| {
        let hint = refused_hint(CARDS_ROW, SETTING, switches::AGENT_TAB.path, Floor::Admin, handle);
        out.rows.push(Decision::new(CARDS_ROW, "refused", Some(c.why.clone()), Some(hint.clone())));
        out.refused.push(Refusal { action: CARDS_ROW.into(), code: c.code.clone(), hint });
        record(inp, &t.cloned(), CARDS_ROW, &format!("refused:{}", c.code), None);
    };
    if let Err(c) = reach {
        return refuse(out, c);
    }
    if inp.dry_run {
        out.rows.push(Decision::new(CARDS_ROW, "grant", None, None));
        out.granted.push(CARDS_ROW.into());
        return;
    }
    let Some(t) = t else {
        return refuse(out, cant("not_registered", "the agent isn't registered"));
    };
    let r = match h {
        Hirer::Person { who, .. } => {
            let r = crate::sidecar_routes::access_entry(who, &json!({ "toggle": "permission_cards", "workspace": t.id, "value": true }));
            if r.status.starts_with('2') {
                Ok(())
            } else {
                Err(cant(&route_code(&r), "the setting refused it"))
            }
        }
        Hirer::Agent { .. } => agent_write(inp, t, "permission_cards"),
        Hirer::ApiKey => Err(cant("api_key", "an API key turns on no switches")),
    };
    match r {
        Ok(()) => {
            out.rows.push(Decision::new(CARDS_ROW, "done", None, None));
            out.granted.push(CARDS_ROW.into());
            record(inp, &Some(t.clone()), CARDS_ROW, "granted", None);
        }
        Err(c) => refuse(out, c),
    }
}

/// One suggested standing switch.
fn switch_row(inp: &StepIn<'_>, h: &Hirer, t: Option<&Target>, handle: &str, action: &str, cards_on: bool, out: &mut StepOut) {
    let Some(reg) = crate::permission::actions::find(action) else {
        let hint = format!("K2 doesn't know the permission {action}; check the template (k2 agent template show <id>).");
        out.rows.push(Decision::new(action, "refused", Some("unknown permission".into()), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: "unknown".into(), hint });
        record(inp, &t.cloned(), action, "refused:unknown", None);
        return;
    };
    let spec = reg.spec;
    let ctx = t.map(|t| Ctx { workspace_id: t.id.clone(), workspace_handle: t.handle.clone(), params: Default::default() });
    let hint = refused_hint(action, spec.words.setting, spec.settings.path, spec.floor, handle);
    let refuse = |out: &mut StepOut, code: &str, why: String| {
        out.rows.push(Decision::new(action, "refused", Some(why), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: code.into(), hint: hint.clone() });
        record(inp, &t.cloned(), action, &format!("refused:{code}"), None);
    };
    if spec.grant != Grant::Standing || spec.params.iter().any(|p| p.required) {
        return refuse(out, "needs_person", "it needs a person to choose its value".into());
    }
    if let Some(ctx) = ctx.as_ref() {
        match (reg.check)(ctx) {
            Check::AlreadyDone => {
                out.rows.push(Decision::new(action, "skip", Some("already on".into()), None));
                return;
            }
            Check::Needed => {}
            Check::Moved(why) => return refuse(out, "moved", why),
            Check::Unavailable { code, why } => return refuse(out, code, why),
        }
    }
    let why_not = match may_give(h, action, spec.floor) {
        Ok(()) if inp.dry_run => {
            out.rows.push(Decision::new(action, "grant", None, None));
            out.granted.push(action.into());
            return;
        }
        Ok(()) => match (h, t, ctx.as_ref()) {
            (Hirer::Person { who, .. }, Some(_), Some(ctx)) => match (reg.apply)(ctx, who, None) {
                Applied::Done { .. } => Ok(()),
                Applied::Refused { code, message } => Err(cant(&code, message)),
            },
            (Hirer::Agent { .. }, Some(t), _) => match standing_switch(action) {
                Some(sw) => agent_write(inp, t, sw),
                None => Err(cant("not_passable", "an agent can't pass this one on")),
            },
            _ => Err(cant("not_registered", "the agent isn't registered")),
        },
        Err(c) => Err(c),
    };
    match why_not {
        Ok(()) => {
            out.rows.push(Decision::new(action, "done", None, None));
            out.granted.push(action.into());
            record(inp, &t.cloned(), action, "granted", None);
        }
        // Asked by an earlier apply: never twice.
        Err(_) if inp.asked_before.iter().any(|a| a == action) => {
            out.rows.push(Decision::new(action, "card", Some("asked on a card already".into()), None));
        }
        Err(c) if cards_on => card(inp, t, action, c, out),
        Err(c) => refuse(out, &c.code, format!("{}; permission cards are off on this server", c.why)),
    }
}

/// Ask a person on a card: now when the agent has a Thread, else on its
/// first start. A dry run only says so.
fn card(inp: &StepIn<'_>, t: Option<&Target>, action: &str, c: Cant, out: &mut StepOut) {
    if inp.dry_run {
        out.rows.push(Decision::new(action, "card", Some(format!("{}; a person is asked on a card", c.why)), None));
        out.carded.push(action.into());
        return;
    }
    let Some(t) = t else { return };
    if crate::overlay_routes::pinned_chat_of(&t.id).is_some() {
        let why = card_why(inp.template_name);
        let (status, v) = crate::permission::routes::raise_hire_card(&t.id, action, &why);
        if status == 200 {
            out.rows.push(Decision::new(action, "card", Some(format!("{}; a person is asked on its Thread", c.why)), None));
            out.carded.push(action.into());
            record(inp, &Some(t.clone()), action, &format!("carded:{}", c.code), v["id"].as_str());
            return;
        }
        let code = v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("card_refused").to_string();
        let hint = v.pointer("/error/hint").and_then(|x| x.as_str()).unwrap_or("the card could not be filed").to_string();
        out.rows.push(Decision::new(action, "refused", Some(format!("{}; the card could not be filed", c.why)), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: code.clone(), hint });
        record(inp, &Some(t.clone()), action, &format!("refused:{code}"), None);
        return;
    }
    out.rows.push(Decision::new(action, "card", Some(format!("{}; a person is asked on its Thread when it first starts", c.why)), None));
    out.carded.push(action.into());
    out.cards_pending.push(action.into());
    record(inp, &Some(t.clone()), action, &format!("carded:{} pending-first-start", c.code), None);
}

/// The card's one-line why (the agent is the asker).
fn card_why(template_name: &str) -> String {
    let w = format!("Suggested by my {template_name} template when I was hired; my hirer couldn't turn it on.");
    w.chars().take(200).collect()
}

/// An agent hirer passes a switch on in-process (the same single writer
/// Settings uses), with its own `agent-access-set` line.
fn agent_write(inp: &StepIn<'_>, t: &Target, switch: &str) -> Result<(), Cant> {
    let actor = actor_label(inp.caller);
    let ft = crate::first_run_grants::Target { id: t.id.clone(), path: t.path.clone(), handle: t.handle.clone() };
    crate::first_run_grants::write_on(&ft, switch, &actor).map_err(|e| cant("write_failed", e))?;
    access_audit(&actor, &format!("ok {} {switch}=true (template hire)", t.handle));
    Ok(())
}

/// The actor in audit lines: `owner`, `login:<user>`, `agent:<handle>`,
/// `api-key:<id>`.
fn actor_label(caller: &HireCaller) -> String {
    match caller {
        HireCaller::Owner => "owner".into(),
        HireCaller::Login { username } => format!("login:{username}"),
        HireCaller::Agent { workspace_id } => format!(
            "agent:{}",
            k2_core::workspace_session_handles::workspace_address_name_shared(workspace_id).unwrap_or_else(|_| workspace_id.clone())
        ),
        HireCaller::ApiKey { id, .. } => format!("api-key:{id}"),
    }
}

fn route_code(r: &crate::cli_response::CliResponse) -> String {
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    v.pointer("/error/code")
        .or_else(|| v.get("error").filter(|e| e.is_string()))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("refused")
        .to_string()
}

fn access_audit(actor: &str, outcome: &str) {
    #[cfg(test)]
    {
        crate::hire_routes::AUDIT_SINK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(("agent-access-set".into(), actor.into(), outcome.into()));
    }
    #[cfg(not(test))]
    {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("agent-access-set", actor, outcome.to_string(), "hire", "-", "cli"));
    }
}

/// One decision into the audit and the hire ledger (never in a dry run).
fn record(inp: &StepIn<'_>, t: &Option<Target>, action: &str, result: &str, card: Option<&str>) {
    if inp.dry_run {
        return;
    }
    let handle = t.as_ref().map(|t| t.handle.clone()).unwrap_or_default();
    let mut outcome = format!("{result} {action} {handle}");
    if let Some(c) = card {
        outcome.push_str(&format!(" card={c}"));
    }
    crate::hire_routes::audit(AUDIT_EVENT, &inp.caller.actor(), &outcome);
    let detail = json!({ "workspaceId": t.as_ref().map(|t| t.id.clone()), "handle": handle, "template": inp.template_name, "card": card });
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::hire::ledger::event(&conn, AUDIT_EVENT, inp.caller, None, None, None, result, Some(action), Some(&detail));
}

// ── cards waiting for the first start ───────────────────────────────

/// File the cards a hire left for this workspace's first start, once its
/// Thread exists. `false`: nothing to file yet (no pending cards, no Thread,
/// or the server's permission cards are off: they stay in the lock for a
/// later start).
pub(crate) fn file_pending_cards(workspace_id: &str) -> bool {
    file_pending_cards_with(workspace_id, k2_core::permission::gate::server_on())
}

/// [`file_pending_cards`] with the server's switch given (lib tests).
pub(crate) fn file_pending_cards_with(workspace_id: &str, cards_on: bool) -> bool {
    let Some(t) = crate::sidecar_routes::workspace_by_id(workspace_id) else { return false };
    if !cards_on || !k2_core::hire::template::lock::has_pending_cards(&t.path) || crate::overlay_routes::pinned_chat_of(&t.id).is_none() {
        return false;
    }
    let taken = match k2_core::hire::template::lock::take_pending_cards(&t.path) {
        Ok(v) => v,
        Err(e) => {
            k2_core::log_debug!("[hire-permissions] {}: {}", t.handle, e.hint);
            return false;
        }
    };
    let name = k2_core::hire::template::lock::read(&t.path)
        .ok()
        .flatten()
        .and_then(|l| k2_core::hire::template::resolve(&l.id).ok().map(|tpl| tpl.meta.name.clone()))
        .unwrap_or_else(|| "agent".into());
    for action in taken {
        let (status, v) = crate::permission::routes::raise_hire_card(&t.id, &action, &card_why(&name));
        let result = if status == 200 {
            format!("card-filed {action} {} card={}", t.handle, v["id"].as_str().unwrap_or(""))
        } else {
            format!("card-refused:{} {action} {}", v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("refused"), t.handle)
        };
        crate::hire_routes::audit(AUDIT_EVENT, &format!("agent:{}", t.handle), &result);
    }
    true
}

/// The first-start hook's body: wait (off the caller's thread) for the
/// agent's Thread, then file its pending cards. A harness whose
/// conversation id appears only after its first turn gets a few minutes.
pub(crate) fn file_pending_cards_when_ready(workspace_id: &str) {
    let path = match crate::sidecar_routes::workspace_by_id(workspace_id) {
        Some(w) => w.path,
        None => return,
    };
    if !k2_core::permission::gate::server_on() || !k2_core::hire::template::lock::has_pending_cards(&path) {
        return;
    }
    for _ in 0..150 {
        if file_pending_cards(workspace_id) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    k2_core::log_debug!("[hire-permissions] {workspace_id}: no Thread yet; its cards wait for the next start");
}

#[cfg(test)]
#[path = "hire_permissions_tests.rs"]
mod tests;
