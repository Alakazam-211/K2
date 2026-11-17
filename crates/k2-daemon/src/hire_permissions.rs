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
use k2_core::hire::{HireCaller, HireError};
use k2_core::permission::catalog::{Floor, Grant};
use serde_json::json;

use crate::permission::actions::{switches, Answerer, Applied, Check, Ctx};
use k2_core::hire::permission_records as records;

/// The audit event (and `hire_events` action) for every decision.
pub(crate) const AUDIT_EVENT: &str = "agent-hire-permission";

/// Install the step into the template apply (daemon start).
pub fn install() {
    k2_core::hire::template::permissions::install(step);
}

/// The installed step: the server's own Beta switch decides whether cards
/// can be filed.
fn step(inp: &StepIn<'_>) -> Result<StepOut, HireError> {
    step_with(inp, server_cards_on())
}

#[cfg(test)]
thread_local! {
    /// Lib tests never flip the process switch (lane B's rule): a test sets
    /// the server's cards switch for its own thread here instead.
    pub(crate) static TEST_SERVER_CARDS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// The server's permission cards (Beta) switch.
fn server_cards_on() -> bool {
    #[cfg(test)]
    {
        if let Some(on) = TEST_SERVER_CARDS.with(|c| c.get()) {
            return on;
        }
    }
    k2_core::permission::gate::server_on()
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
            if crate::first_run_grants::AGENT_NEVER_PASSES.contains(&action) {
                return Err(cant("not_passable", "an agent can't pass on compute: its machines and limits are its own"));
            }
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

/// The hirer's rank, recorded with a decline: a decline is lifted only by
/// a role at least as high (an Owner's decline needs an Owner).
fn rank_of(h: &Hirer) -> i64 {
    match h {
        Hirer::Person { owner: true, .. } => records::RANK_OWNER,
        Hirer::Person { admin: true, .. } => records::RANK_ADMIN,
        Hirer::Person { .. } => records::RANK_MEMBER,
        Hirer::Agent { .. } | Hirer::ApiKey => records::RANK_AGENT,
    }
}

/// May this hirer turn on "Can ask for permission cards" for the hire?
fn cards_reach(h: &Hirer) -> Result<(), Cant> {
    match h {
        Hirer::Person { admin: true, .. } => Ok(()),
        Hirer::Person { .. } => Err(cant("role", "your role can't turn it on (an owner or admin can)")),
        Hirer::Agent { workspace_id } if k2_core::permission::gate::agent_switch_on(workspace_id) => Ok(()),
        Hirer::Agent { .. } => Err(cant("hirer_lacks", "the hiring agent doesn't have it itself, and it is never asked on a card")),
        Hirer::ApiKey => Err(cant("api_key", "an API key turns on no switches")),
    }
}

/// Could this hirer grant `target` itself (not on a card)? `--allow` lifts
/// a held decline only then.
fn can_grant(h: &Hirer, target: &str) -> Result<(), Cant> {
    if target == CARDS_ROW {
        return cards_reach(h);
    }
    match crate::permission::actions::find(target) {
        Some(reg) => may_give(h, target, reg.spec.floor),
        None => Err(cant("unknown", "K2 doesn't know this permission")),
    }
}

/// The record couldn't be read or written: the hire stops, loudly.
/// Deciding without it could grant what a person declined. The rows
/// before the failure may already have been turned on or carded (each
/// through its own Settings or card route, which takes the database
/// itself, so the loop can't be one transaction): the message names them.
fn records_error(handle: &str, e: String, so_far: &StepOut) -> HireError {
    HireError::new(500, "permission_records", records_error_text(handle, &e, &so_far.granted, &so_far.carded))
}

fn records_error_text(handle: &str, e: &str, granted: &[String], carded: &[String]) -> String {
    let mut done = Vec::new();
    if !granted.is_empty() {
        done.push(format!("turned on {}", granted.join(", ")));
    }
    if !carded.is_empty() {
        done.push(format!("asked on a card for {}", carded.join(", ")));
    }
    let before = if done.is_empty() {
        "nothing was granted".to_string()
    } else {
        format!("before it stopped, this hire {} (each audited); the rest was not decided", done.join(" and "))
    };
    format!(
        "K2 couldn't read or keep what was declined or asked at hires of {handle} ({e}), so the hire stopped; {before}. Try again; if it repeats, the daemon log has the cause."
    )
}

/// The step with the server's Beta switch given (unit tests pass it; lib
/// tests never flip the process switch).
pub(crate) fn step_with(inp: &StepIn<'_>, cards_on: bool) -> Result<StepOut, HireError> {
    let hirer = hirer_of(inp.caller);
    let target = target_of(inp.path);
    let handle = handle_for(inp.path, target.as_ref());
    // The daemon's record of earlier hires of this workspace (0309), never
    // the agent's own lock: what was declined, and what was asked.
    // A failed read stops the hire (never an empty list).
    let (held_declines, asked) = match target.as_ref() {
        Some(t) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let none = StepOut::default();
            let d = records::declined(&conn, &t.id).map_err(|e| records_error(&t.handle, e, &none))?;
            let a = records::asked(&conn, &t.id).map_err(|e| records_error(&t.handle, e, &none))?;
            (d, a)
        }
        None => (Vec::new(), Vec::new()),
    };
    let mut ctx = RowCtx {
        hirer: &hirer,
        t: target.as_ref(),
        handle: &handle,
        cards_on,
        // A card is filed for the new agent only while it may ask for cards
        // (the cards row below may turn that on).
        agent_asks: cards_on && target.as_ref().is_some_and(|t| k2_core::permission::gate::agent_switch_on(&t.id)),
        asked: &asked,
    };
    let mut out = StepOut::default();
    for want in targets(inp.cards, inp.suggested) {
        let named_again = inp.allowed.contains(&want);
        if inp.declined.contains(&want) {
            out.rows.push(Decision::new(want.clone(), "declined", Some("left off at hire; a re-run leaves it off too".into()), None));
            if let (false, Some(t)) = (inp.dry_run, target.as_ref()) {
                let db = k2_core::db::shared();
                let conn = db.lock();
                records::record_declined(&conn, &t.id, &want, inp.template_name, &inp.caller.actor(), rank_of(&hirer), now())
                    .map_err(|e| records_error(&t.handle, e, &out))?;
            }
            record(inp, &target, &want, "declined", None);
            continue;
        }
        if held_declines.contains(&want) {
            if !named_again {
                out.rows.push(Decision::new(want.clone(), "declined", Some("declined at an earlier hire; name it with --allow to decide it again".into()), None));
                continue;
            }
            // Reach first, the record after: --allow lifts a decline only
            // for a caller who could grant the switch itself, with a role
            // at least the decliner's. Otherwise the decline stays.
            let decliner = match target.as_ref() {
                Some(t) => {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    records::decline_rank(&conn, &t.id, &want).map_err(|e| records_error(&t.handle, e, &out))?.unwrap_or(records::RANK_AGENT)
                }
                None => records::RANK_AGENT,
            };
            let lift = can_grant(&hirer, &want).and_then(|()| {
                if rank_of(&hirer) >= decliner {
                    Ok(())
                } else {
                    Err(cant("decliner_outranks", format!("{} declined it, and your role is lower", records::rank_word(decliner))))
                }
            });
            if let Err(c) = lift {
                let who = if decliner >= records::RANK_OWNER { "the server's owner" } else { "an owner or admin who can turn it on" };
                let hint = format!("the decline stays; {who} can allow it again by re-running the hire with --allow {want}");
                out.rows.push(Decision::new(want.clone(), "refused", Some(format!("declined at an earlier hire; --allow refused: {}", c.why)), Some(hint.clone())));
                out.refused.push(Refusal { action: want.clone(), code: format!("allow_{}", c.code), hint });
                record(inp, &target, &want, &format!("refused:allow_{}", c.code), None);
                continue;
            }
            if let (false, Some(t)) = (inp.dry_run, target.as_ref()) {
                let db = k2_core::db::shared();
                let conn = db.lock();
                records::undecline(&conn, &t.id, &want).map_err(|e| records_error(&t.handle, e, &out))?;
            }
            record(inp, &target, &want, "allowed-again", None);
        }
        if want == CARDS_ROW {
            cards_row(inp, &mut ctx, &mut out);
        } else {
            switch_row(inp, &ctx, &want, &mut out);
        }
    }
    Ok(out)
}

/// What every row of one apply shares.
struct RowCtx<'a> {
    hirer: &'a Hirer,
    t: Option<&'a Target>,
    handle: &'a str,
    /// The server's permission cards (Beta).
    cards_on: bool,
    /// The new agent may ask for cards after this apply (a card is filed
    /// for it only then).
    agent_asks: bool,
    /// Switches asked on a card at an earlier hire (the daemon's record).
    asked: &'a [String],
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// "Can ask for permission cards" for the new agent (`cards = true`).
fn cards_row(inp: &StepIn<'_>, ctx: &mut RowCtx<'_>, out: &mut StepOut) {
    const SETTING: &str = "Can ask for permission cards";
    let (h, t, handle) = (ctx.hirer, ctx.t, ctx.handle);
    if !ctx.cards_on {
        out.rows.push(Decision::new(CARDS_ROW, "skip", Some("permission cards are off on this server".into()), None));
        return;
    }
    if t.is_some_and(|t| k2_core::permission::gate::agent_switch_on(&t.id)) {
        out.rows.push(Decision::new(CARDS_ROW, "skip", Some("already on".into()), None));
        return;
    }
    let reach = cards_reach(h);
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
        ctx.agent_asks = true;
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
            ctx.agent_asks = true;
            record(inp, &Some(t.clone()), CARDS_ROW, "granted", None);
        }
        Err(c) => refuse(out, c),
    }
}

/// One suggested standing switch.
fn switch_row(inp: &StepIn<'_>, ctx: &RowCtx<'_>, action: &str, out: &mut StepOut) {
    let (h, t, handle) = (ctx.hirer, ctx.t, ctx.handle);
    let Some(reg) = crate::permission::actions::find(action) else {
        let hint = format!("K2 doesn't know the permission {action}; check the template (k2 agent template show <id>).");
        out.rows.push(Decision::new(action, "refused", Some("unknown permission".into()), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: "unknown".into(), hint });
        record(inp, &t.cloned(), action, "refused:unknown", None);
        return;
    };
    let spec = reg.spec;
    let actx = t.map(|t| Ctx { workspace_id: t.id.clone(), workspace_handle: t.handle.clone(), params: Default::default() });
    let hint = refused_hint(action, spec.words.setting, spec.settings.path, spec.floor, handle);
    let refuse = |out: &mut StepOut, code: &str, why: String| {
        out.rows.push(Decision::new(action, "refused", Some(why), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: code.into(), hint: hint.clone() });
        record(inp, &t.cloned(), action, &format!("refused:{code}"), None);
    };
    if spec.grant != Grant::Standing || spec.params.iter().any(|p| p.required) {
        return refuse(out, "needs_person", "it needs a person to choose its value".into());
    }
    if let Some(actx) = actx.as_ref() {
        match (reg.check)(actx) {
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
        Ok(()) => match (h, t, actx.as_ref()) {
            (Hirer::Person { who, .. }, Some(_), Some(actx)) => match (reg.apply)(actx, who, None) {
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
        // Asked at an earlier hire (the daemon's record): never twice.
        Err(_) if ctx.asked.iter().any(|a| a == action) => {
            out.rows.push(Decision::new(action, "card", Some("asked on a card already".into()), None));
        }
        Err(c) if ctx.cards_on && ctx.agent_asks => card(inp, t, action, c, out),
        Err(c) if ctx.cards_on => refuse(out, &c.code, format!("{}; the new agent can't ask for permission cards", c.why)),
        Err(c) => refuse(out, &c.code, format!("{}; permission cards are off on this server", c.why)),
    }
}

/// Ask a person on a card: now when the agent has a Thread, else recorded
/// in the daemon (0309) and filed at its first start. A dry run only says so.
fn card(inp: &StepIn<'_>, t: Option<&Target>, action: &str, c: Cant, out: &mut StepOut) {
    if inp.dry_run {
        out.rows.push(Decision::new(action, "card", Some(format!("{}; a person is asked on a card", c.why)), None));
        out.carded.push(action.into());
        return;
    }
    let Some(t) = t else {
        // A real apply always has its workspace; say so if it doesn't.
        let hint = "the new agent isn't registered, so no card could be filed; run the hire again".to_string();
        out.rows.push(Decision::new(action, "refused", Some(format!("{}; the agent isn't registered", c.why)), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: "not_registered".into(), hint });
        record(inp, &None, action, "refused:not_registered", None);
        return;
    };
    let hirer = inp.caller.actor();
    if crate::overlay_routes::pinned_chat_of(&t.id).is_some() {
        let (status, v) = crate::permission::routes::raise_hire_card(&t.id, action, &card_why(inp.template_name), &hirer, inp.template_name);
        if status == 200 {
            let id = v["id"].as_str().map(str::to_string);
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                if let Err(e) = records::record_card(&conn, &t.id, action, false, inp.template_name, &hirer, id.as_deref(), now()) {
                    k2_core::log_debug!("[hire-permissions] {}: {e}", t.handle);
                }
            }
            out.rows.push(Decision::new(action, "card", Some(format!("{}; a person is asked on its Thread", c.why)), None));
            out.carded.push(action.into());
            record(inp, &Some(t.clone()), action, &format!("carded:{}", c.code), id.as_deref());
            return;
        }
        let code = v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("card_refused").to_string();
        let hint = v.pointer("/error/hint").and_then(|x| x.as_str()).unwrap_or("the card could not be filed").to_string();
        out.rows.push(Decision::new(action, "refused", Some(format!("{}; the card could not be filed", c.why)), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: code.clone(), hint });
        record(inp, &Some(t.clone()), action, &format!("refused:{code}"), None);
        return;
    }
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        records::record_card(&conn, &t.id, action, true, inp.template_name, &hirer, None, now())
    };
    if let Err(e) = stored {
        let hint = format!("K2 couldn't keep the card for the agent's first start ({e}); run the hire again");
        out.rows.push(Decision::new(action, "refused", Some(format!("{}; the card couldn't be kept", c.why)), Some(hint.clone())));
        out.refused.push(Refusal { action: action.into(), code: "store".into(), hint });
        record(inp, &Some(t.clone()), action, "refused:store", None);
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

/// File the cards a hire left for this workspace's first start, from the
/// daemon's record (0309; the agent's own lock is never read), once its
/// Thread exists and while it may ask for cards (the server's switch and
/// its own, as the hire left it). Each one is claimed before it is filed,
/// so it goes once. `false`: nothing filed now (none waiting, no Thread
/// yet, or cards off: they wait for a later start).
// Called from the first-start hook, which is off in unit tests.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn file_pending_cards(workspace_id: &str) -> bool {
    file_pending_cards_with(workspace_id, server_cards_on())
}

/// Any card waiting for this workspace (a cheap check for the hook).
#[cfg_attr(test, allow(dead_code))]
fn has_pending(workspace_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    !records::pending_cards(&conn, workspace_id).is_empty()
}

/// [`file_pending_cards`] with the server's switch given (lib tests).
pub(crate) fn file_pending_cards_with(workspace_id: &str, cards_on: bool) -> bool {
    let Some(t) = crate::sidecar_routes::workspace_by_id(workspace_id) else { return false };
    if !cards_on || !k2_core::permission::gate::agent_switch_on(&t.id) || crate::overlay_routes::pinned_chat_of(&t.id).is_none() {
        return false;
    }
    let pending = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        records::pending_cards(&conn, &t.id)
    };
    let mut filed = false;
    for p in pending {
        let claimed = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            records::claim(&conn, p.id, now())
        };
        if !claimed {
            continue;
        }
        let (status, v) = crate::permission::routes::raise_hire_card(&t.id, &p.action, &card_why(&p.template), &p.hirer, &p.template);
        let card = (status == 200).then(|| v["id"].as_str().unwrap_or("").to_string());
        let result = match &card {
            Some(_) => "card-filed".to_string(),
            None => format!("card-refused:{}", v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("refused")),
        };
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            if let Err(e) = records::finish(&conn, p.id, card.as_deref()) {
                k2_core::log_debug!("[hire-permissions] {}: {e}", t.handle);
            }
            let detail = json!({ "workspaceId": t.id, "handle": t.handle, "template": p.template, "card": card, "at": "first-start" });
            k2_core::hire::ledger::event_by(&conn, AUDIT_EVENT, &p.hirer, &result, Some(&p.action), Some(&detail));
        }
        let mut outcome = format!("{result} {} {}", p.action, t.handle);
        if let Some(c) = card.as_deref() {
            outcome.push_str(&format!(" card={c}"));
        }
        crate::hire_routes::audit(AUDIT_EVENT, &p.hirer, &outcome);
        filed = true;
    }
    filed
}

/// The first-start hook's body: wait (off the caller's thread) for the
/// agent's Thread, then file its pending cards. A harness whose
/// conversation id appears only after its first turn gets a few minutes.
// Called from the first-start hook, which is off in unit tests.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn file_pending_cards_when_ready(workspace_id: &str) {
    if !server_cards_on() || !has_pending(workspace_id) {
        return;
    }
    for _ in 0..150 {
        if file_pending_cards(workspace_id) || !has_pending(workspace_id) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    k2_core::log_debug!("[hire-permissions] {workspace_id}: no Thread yet; its cards wait for the next start");
}

#[cfg(test)]
#[path = "hire_permissions_tests.rs"]
mod tests;
