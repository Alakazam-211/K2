//! `vault.delivery secret= delivery=file|env` (0.46.6, Rosson 2026-10-28:
//! "a person changing a setting gets a one-click card, not 'go run a
//! command'").
//!
//! An agent that needs to READ one of its own workspace's secrets (a file
//! or an environment variable) but may not widen it itself (a person gave
//! it, or it was handed over by another agent: `created_by` isn't its
//! workspace) gets `vault_owner_confirm_required` from `k2 vault delivery
//! NAME file|env` (`POST /cli/vault/policy`). That refusal is this card's
//! wall: with permission cards on, the agent's error ends with
//! `Ask with: k2 permission request vault.delivery secret=NAME delivery=env
//! --why "…"`, and the owner sees "Let <agent> read NAME as env?".
//!
//! - **Who answers:** the owner token, an Owner or an Admin login: exactly
//!   who `k2 vault delivery` lets widen a secret (`policy::can_loosen`).
//! - **Proof:** a click, the same as Settings → Vault and `k2 vault
//!   delivery` take today (no confirm on that route; a reveal still needs
//!   one).
//! - **On Allow:** `apply` runs the `/cli/vault/policy` route entry
//!   ([`crate::vault_routes::handle_on`]) as the person who clicked, with
//!   the delivery asked for (and the ceiling raised to it when it was
//!   lower), so the route's own checks run again at the click.
//! - Only `file` and `env`: `api` (print it into a transcript) stays a
//!   Settings act, and fetch never needs widening.
//! - **Values:** the secret's NAME only; never a value. The workspace
//!   comes from the passport.

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_vault::{Actor, Delivery, Namespace, Scope, SecretStore, Selector};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::vault::actor::{Channel, VaultCaller};

/// The card's id.
pub(crate) const ACTION: &str = "vault.delivery";
/// `k2 vault delivery` / Settings → Vault's delivery control.
pub(crate) const ROUTE: &str = crate::vault_routes::POLICY;
/// The deliveries a card may widen to.
pub(crate) const CHOICES: [&str; 2] = ["file", "env"];

pub(crate) static DELIVERY: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::Secret, required: true, list: false, choices: &[] },
        ParamSpec { key: "delivery", kind: ParamKind::Choice, required: true, list: false, choices: &CHOICES },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{secret} delivered as {delivery}",
        what_changes: "Let {ws} read {secret} as {delivery} (file: a 0600 file it reads; env: an environment variable)",
        consequences: "{ws} can read {secret}'s value from then on (and so can anything it runs); it can still never print it with k2 vault get. Lower it again in Settings → Vault, or with k2 vault delivery {secret} fetch.",
        scope: "{secret} in {ws}",
        duration: "until someone changes it",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "vault_owner_confirm_required" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("secret", ParamFrom::Response("/error/secret")), ("delivery", ParamFrom::Response("/error/delivery"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

fn delivery_of(ctx: &Ctx) -> Result<Delivery, String> {
    let d = text_param(ctx, "delivery").ok_or("name the delivery: delivery=file or delivery=env")?;
    if !CHOICES.contains(&d) {
        return Err(format!("delivery={d}: a card widens to file or env only"));
    }
    Delivery::parse(d).map_err(|e| e.hint)
}

fn secret_of(ctx: &Ctx) -> Result<&str, String> {
    text_param(ctx, "secret").ok_or_else(|| "name the secret: secret=NAME".to_string())
}

fn selector(ctx: &Ctx, name: &str) -> Selector {
    Selector::Name { namespace: Namespace::User, scope: Scope::workspace(ctx.workspace_id.clone()), name: name.to_string() }
}

/// Still wanted: the asking workspace has that secret, and it doesn't
/// already reach it at least that way.
fn check(ctx: &Ctx) -> Check {
    let (name, want) = match (secret_of(ctx), delivery_of(ctx)) {
        (Ok(n), Ok(d)) => (n, d),
        (Err(e), _) | (_, Err(e)) => return Check::Moved(e),
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    match vault.meta(&selector(ctx, name), &Actor::System("permission-card".into())) {
        Ok(m) if m.default_delivery.level() >= want.level() => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(_) => Check::Moved(format!("{} has no secret named {name}", ctx.workspace_handle)),
    }
}

fn caller_of(who: &Answerer) -> VaultCaller {
    match who {
        Answerer::OwnerToken => VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None },
        Answerer::Login { user, role } => VaultCaller {
            actor: Actor::Login {
                username: user.clone(),
                role: match role.as_str() {
                    "owner" => k2_vault::Role::Owner,
                    "admin" => k2_vault::Role::Admin,
                    _ => k2_vault::Role::Member,
                },
            },
            channel: Channel::Login,
            session_id: None,
        },
    }
}

/// Through `/cli/vault/policy`'s own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (name, want) = match (secret_of(ctx), delivery_of(ctx)) {
        (Ok(n), Ok(d)) => (n.to_string(), d),
        (Err(e), _) | (_, Err(e)) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    // The ceiling rises with it when it was lower (a delivery above the
    // ceiling is refused).
    let m = match vault.meta(&selector(ctx, &name), &Actor::System("permission-card".into())) {
        Ok(m) => m,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    // By id: exactly the asking workspace's secret of that name.
    let mut body = json!({ "id": m.id, "delivery": want.as_str() });
    if m.delivery_ceiling.level() < want.level() {
        body["ceiling"] = json!(want.as_str());
    }
    let r = crate::vault_routes::handle_on(&vault, ROUTE, Ok(caller_of(who)), body.to_string().as_bytes(), &HashMap::new(), "permission-card", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    let how = match want {
        Delivery::File => format!("k2 vault file --secret {name} (a short-lived 0600 file)"),
        _ => format!("k2 vault env-file --secret {name} or k2 vault file --secret {name}; ${name} in new sessions"),
    };
    Applied::Done { message: format!("{name} is delivered to {} as {} now: {how}.", ctx.workspace_handle, want.as_str()) }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &DELIVERY, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
