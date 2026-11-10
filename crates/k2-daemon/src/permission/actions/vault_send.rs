//! `vault.send secret=A[,B…] to=agent::host [note=…]` (0.46.11, a user's
//! report: an agent's `k2 vault send NAME --to agent::host` of a secret a
//! person gave was refused `held`, with permission cards on, and the only
//! way forward was a person typing the command in a terminal).
//!
//! **The rule stays** (R3, prd-agent-secret-handoff-v1 §7.6): a secret a
//! person gave leaves this server only on a person's say, with the vault
//! confirm. What changed is how the agent asks and how the person says yes.
//!
//! - **The wall:** `POST /cli/vault/send`'s `held` refusal names the secret,
//!   the target and the send's note (`federation_vault::held_response`); with
//!   cards on, it ends `Ask with: k2 permission request vault.send secret=…
//!   to=… --why "…"`. Several secrets for one agent go on one card
//!   (`secret=A,B,C`).
//! - **Who answers:** the owner token, an Owner or an Admin login: who may
//!   send a person-given secret to another server today. A Member sees the
//!   card and can't answer it.
//! - **Proof:** the vault confirm, bound to this request (the
//!   `vault-share-service` confirm action, as `vault.use` does); it stands in
//!   for the `handoff-send` confirm the route takes, minted per secret from
//!   the confirm just taken, so the route still decides for itself.
//! - **Allow** sends now ([`apply`]): `POST /cli/vault/send`'s own entry, as
//!   the person who clicked, by the secret's id. The out row, the Access
//!   Audit line and the vault audit name that person as the sender. The agent
//!   never gets the value.
//! - **Allow, and let it send this itself from now on** ([`label::ALWAYS_VAULT_SEND`])
//!   also writes a standing send grant for exactly those secrets and that
//!   agent ([`turn_on_always`]): later sends of them there go through without
//!   a card, logged as sent on that person's grant. Listed, added and removed
//!   in Settings → Workspaces → <agent> → Permissions (`/cli/vault/send-allow`).
//! - **Deny** sends nothing; the agent's `[permission:]` turn says so.

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, ConfirmAction, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::label;
use k2_core::permission::request::PermissionRequest;
use k2_vault::{Actor, HumanProof, Namespace, Scope, SecretStatus, SecretStore, Selector};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::vault::{caller_of, SHARE_CONFIRM};
use super::{Answerer, Applied, Check, Ctx, Registered};

/// The card's id.
pub(crate) const ACTION: &str = "vault.send";
/// `k2 vault send NAME --to agent::host`.
pub(crate) const ROUTE: &str = crate::vault_handoff_routes::SEND;
/// The label of the "from now on" answer.
pub(crate) const ALWAYS_LABEL: &str = label::ALWAYS_VAULT_SEND;
/// The ingress the card's sends run with (the Access Audit's `via`).
const INGRESS: &str = "permission-card";

pub(crate) static SEND: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::Secret, required: true, list: true, choices: &[] },
        ParamSpec { key: "to", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "note", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::VaultShareService },
    expiry: Expiry::STANDARD,
    // `{secrets}`, `{them}`, `{path}` and `{noted}` are K2's own words
    // ([`with_computed`]); never a value.
    words: Words {
        setting: "Send {secrets} to {to}",
        what_changes: "Send {secrets} from {ws}'s vault to {to}, an agent on another server",
        consequences: "A person gave {ws} {secrets}, so {them} leave this server only when you say so. K2 sends {them} as you, sealed to that server's vault: nobody sees the value on the way, and it waits there until a person on that server accepts it (unless {to} already accepts from {ws}). \"From now on\" also lets {ws} send {secrets} to {to} itself, without a card, until you remove it in {path}.{noted}",
        scope: "{secrets} to {to}",
        duration: "once, unless you choose from now on",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{ws}/permissions", path: "Settings → Workspaces → {ws} → Permissions" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "held" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("secret", ParamFrom::Response("/error/secret")), ("to", ParamFrom::Response("/error/to")), ("note", ParamFrom::Response("/error/note"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The secrets the request names, in order, without repeats.
fn secrets_of(ctx: &Ctx) -> Vec<String> {
    let raw: Vec<String> = match ctx.params.get("secret") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).collect(),
        Some(Value::String(s)) => vec![s.trim().to_string()],
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for s in raw.into_iter().filter(|s| !s.is_empty()) {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// `agent::host`, lowercased (what the grant stores).
fn to_of(ctx: &Ctx) -> Result<String, String> {
    let t = text_param(ctx, "to").ok_or("name who gets it: to=agent::host")?;
    k2_vault::handoff::normalise_remote_agent(t).map_err(|e| e.hint)
}

/// "A", "A and B", "A, B and C".
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The card's computed words. They never read the vault, so the text hash
/// is the same at the ask and the click.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let names = secrets_of(ctx);
    out.params.insert("secrets".into(), json!(and_list(&names)));
    out.params.insert("them".into(), json!(if names.len() == 1 { "it" } else { "they" }));
    out.params.insert("path".into(), json!(crate::federation_vault::send_allow_settings_path(&ctx.workspace_handle)));
    let noted = match text_param(ctx, "note") {
        Some(n) => format!(" The note that goes with it: \"{n}\""),
        None => String::new(),
    };
    out.params.insert("noted".into(), json!(noted));
    Some(out)
}

/// The macOS sheet's words for this card's confirm.
pub(crate) fn confirm_words(ctx: &Ctx) -> String {
    let to = text_param(ctx, "to").unwrap_or("");
    format!("send {} from {} to {to} on another server", and_list(&secrets_of(ctx)), ctx.workspace_handle)
}

fn selector(ctx: &Ctx, name: &str) -> Selector {
    Selector::Name { namespace: Namespace::User, scope: Scope::workspace(ctx.workspace_id.clone()), name: name.to_string() }
}

/// Still wanted, and something a card can do: every secret is the asking
/// workspace's own, active, sendable and one a person gave (an agent sends
/// its own without asking), and not all of them are already allowed there.
fn check(ctx: &Ctx) -> Check {
    let names = secrets_of(ctx);
    if names.is_empty() {
        return Check::Moved("name the secret: secret=NAME (or secret=A,B for several)".into());
    }
    if names.len() > 8 {
        return Check::Moved("at most 8 secrets on one card".into());
    }
    let to = match to_of(ctx) {
        Ok(t) => t,
        Err(e) => return Check::Moved(e),
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    let system = Actor::System("permission-card".into());
    let mut allowed = 0;
    for name in &names {
        let m = match vault.meta(&selector(ctx, name), &system) {
            Ok(m) => m,
            Err(_) => return Check::Moved(format!("{} has no secret named {name}", ctx.workspace_handle)),
        };
        if m.status != SecretStatus::Active {
            return Check::Moved(format!("{name} is disabled"));
        }
        if let Err(e) = k2_vault::handoff::check_sendable(&m) {
            return Check::Moved(format!("{name}: {}", e.hint));
        }
        if !m.human_given {
            return Check::Moved(format!("{name} isn't one a person gave, so you send it yourself: k2 vault send {name} --to {to}"));
        }
        if vault.send_allowed(&ctx.workspace_id, &m.id, std::slice::from_ref(&to)).is_some() {
            allowed += 1;
        }
    }
    if allowed == names.len() {
        return Check::Moved(format!("a person already lets you send {} to {to}: run k2 vault send again", and_list(&names)));
    }
    Check::Needed
}

/// One secret's send through the route's own entry, as the person.
fn send_one(vault: &std::sync::Arc<k2_vault::LocalVault>, who: &Answerer, m: &k2_vault::SecretMeta, to: &str, note: Option<&str>, method: &str) -> Result<Value, (String, String)> {
    let stamp = who.stamp();
    let mut body = json!({
        "name": m.name,
        "id": m.id,
        "to": to,
        "confirmToken": vault.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(&m.id), &stamp, method),
    });
    if let Some(n) = note {
        body["note"] = json!(n);
    }
    let r = crate::vault_handoff_routes::handle(vault, ROUTE, &caller_of(who), body.to_string().as_bytes(), &HashMap::new(), INGRESS);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if r.status.starts_with("200") {
        Ok(v)
    } else {
        Err(error_of(&v, "send_refused"))
    }
}

/// Through `/cli/vault/send`'s own entry, as the person who clicked, one
/// call per secret. The card's confirm is taken first, bound to this
/// request; each send gets its own `handoff-send` confirm minted from it.
fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let names = secrets_of(ctx);
    let to = match to_of(ctx) {
        Ok(t) => t,
        Err(e) => return refused("bad_params", e),
    };
    if let Err(r) = route_floor(ROUTE, who) {
        return r;
    }
    let Some(token) = token else {
        return refused("vault_confirm_required", "confirm it's you first".into());
    };
    let Some(request) = super::super::answer::current_request() else {
        return refused("failed", "no request is being answered".into());
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return refused(e.code, e.hint),
    };
    let stamp = who.stamp();
    let method = match vault.confirm.take(token, SHARE_CONFIRM, Some(&request), &stamp) {
        Ok(HumanProof::Confirmed { method }) => method,
        Ok(HumanProof::None) => return refused("vault_confirm_required", "confirm again".into()),
        Err(e) => return refused(e.code, e.hint),
    };
    let system = Actor::System("permission-card".into());
    let note = text_param(ctx, "note");
    let (mut sent, mut failed): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut pending = false;
    for name in &names {
        let m = match vault.meta(&selector(ctx, name), &system) {
            Ok(m) => m,
            Err(_) => {
                failed.push(format!("{name} (no longer in {})", ctx.workspace_handle));
                continue;
            }
        };
        match send_one(&vault, who, &m, &to, note, &method) {
            Ok(v) => {
                pending |= v["status"] != "landed";
                sent.push(name.clone());
            }
            Err((code, hint)) => failed.push(format!("{name} ({code}: {hint})")),
        }
    }
    if sent.is_empty() {
        return refused("send_refused", format!("Nothing was sent: {}.", failed.join("; ")));
    }
    let there = if pending { " It waits there until a person on that server accepts it." } else { " It's in their vault now." };
    let mut msg = format!("K2 sent {} to {to} as {}.{there} Don't send {} again.", and_list(&sent), who_words(who), if sent.len() == 1 { "it" } else { "them" });
    if !failed.is_empty() {
        msg.push_str(&format!(" Not sent: {}.", failed.join("; ")));
    }
    Applied::Done { message: msg }
}

fn who_words(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "the owner".to_string(),
        Answerer::Login { user, .. } => user.clone(),
    }
}

/// The card offers "from now on" while some secret isn't allowed there yet.
pub(crate) fn offers_always(req: &PermissionRequest) -> bool {
    if req.action != ACTION {
        return false;
    }
    let ctx = crate::permission::present::ctx_of(req);
    let (Ok(to), Ok(vault)) = (to_of(&ctx), crate::vault::get()) else { return false };
    let system = Actor::System("permission-card".into());
    secrets_of(&ctx).iter().any(|n| match vault.meta(&selector(&ctx, n), &system) {
        Ok(m) => vault.send_allowed(&ctx.workspace_id, &m.id, std::slice::from_ref(&to)).is_none(),
        Err(_) => false,
    })
}

/// [`offers_always`] for a card's view (the request by its display id).
pub(crate) fn offers_always_for(action: &str, request_id: &str) -> bool {
    if action != ACTION {
        return false;
    }
    matches!(crate::permission::store::get_by_display_id(request_id), Ok(Some(req)) if offers_always(&req))
}

/// After the send: the standing send grant for exactly these secrets and
/// this agent, as the person who clicked. Reached only after [`apply`]
/// took this request's vault confirm and sent at least one (the answer path
/// calls it on `Done` only). `Ok(line)` for the card.
pub(crate) fn turn_on_always(ctx: &Ctx, who: &Answerer) -> Result<String, String> {
    let to = to_of(ctx)?;
    let to = crate::federation_vault::canonical_remote_agent(&to);
    let vault = crate::vault::get().map_err(|e| e.hint)?;
    let actor = caller_of(who).actor;
    let system = Actor::System("permission-card".into());
    let mut added = Vec::new();
    for name in secrets_of(ctx) {
        let m = vault.meta(&selector(ctx, &name), &system).map_err(|e| format!("{name}: {}", e.hint))?;
        vault.send_allow_add(&ctx.workspace_id, &m.id, &to, &actor).map_err(|e| format!("{name}: {}", e.hint))?;
        crate::vault_handoff_routes::audit(&who.stamp(), &format!("ok send-allow add {name} for {} to {to} (permission card)", ctx.workspace_handle), INGRESS);
        added.push(name);
    }
    Ok(format!(
        "{} may send {} to {to} itself from now on, without a card (remove it in {}).",
        ctx.workspace_handle,
        and_list(&added),
        crate::federation_vault::send_allow_settings_path(&ctx.workspace_handle)
    ))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &SEND, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_send_tests.rs"]
mod tests;
