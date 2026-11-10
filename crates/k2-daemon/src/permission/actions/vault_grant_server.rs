//! `vault.grant-server secret=NAME [as=fetch|agent|file|env]` (0.46.11, a
//! user's report: an Owner saved a server secret; an agent that needed it
//! ran `k2 vault grant NAME --to ws:<itself>`, got `not_found`, and had no
//! card to ask with).
//!
//! The standing version of `vault.use` for a server secret: "share NAME with
//! me", which is what `k2 vault grant NAME --scope server --to ws:<agent>`
//! does for a person.
//!
//! - **The wall:** that grant, from the agent to its own workspace, of a
//!   secret it can't see, answers `grant_out_of_reach` with the same words
//!   whether or not the secret exists (V20; `vault_routes::grant_out_of_reach`),
//!   ending `Ask with: k2 permission request vault.grant-server secret=NAME
//!   [as=…] --why "…"`.
//! - **Who asks:** any agent, for its own workspace only (the grantee is
//!   never a value). The request never reads the vault, so asking tells it
//!   nothing about what exists; only an approver is told when the secret
//!   isn't there, and then only Deny is offered ([`missing`]).
//! - **Who answers:** the owner token, an Owner or an Admin login: who may
//!   share a server secret. Never an agent.
//! - **Proof:** a click for `fetch` and `agent` (the grantee can never read
//!   the value); for `file`, `env` or the secret's own delivery, the vault
//!   confirm bound to this request, which the route's own `grant-read`
//!   confirm is minted from for the owner token (as `vault.use` does).
//! - **On Allow:** `/cli/vault/grant`'s own entry, as the person who clicked,
//!   by the secret's id, to `ws:<the asking workspace>`, with no end.
//! - **After:** env reaches new sessions only, so the agent's turn says to run
//!   `k2 agent refresh` (and how to read it in this session now).

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef,
    Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_vault::{Actor, ConsumerKind, Delivery, HumanProof, Namespace, Scope, SecretStatus, SecretStore, Selector};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::vault::{caller_of, SHARE_CONFIRM};
use super::{Answerer, Applied, Check, Ctx, Registered};

/// The card's id.
pub(crate) const ACTION: &str = "vault.grant-server";
/// `k2 vault grant NAME --scope server --to ws:<agent>`.
pub(crate) const ROUTE: &str = crate::vault_routes::GRANT;
/// How the agent may get it. `api` (print it) stays a Settings act.
pub(crate) const HOWS: [&str; 4] = ["fetch", "agent", "file", "env"];

pub(crate) static GRANT_SERVER: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::OtherSecret, required: true, list: false, choices: &[] },
        ParamSpec { key: "as", kind: ParamKind::Choice, required: false, list: false, choices: &HOWS },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    // The strict answer; `proof` lowers it to a click for a delivery the
    // grantee can't read.
    proof: Proof::SettingsConfirm { action: ConfirmAction::VaultShareService },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Server secret {secret} for {ws}",
        what_changes: "Let {ws} use the server secret {secret} {how}",
        consequences: "{effect}",
        scope: "{secret} (server), for {ws}",
        duration: "until someone removes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::vault_routes::GRANT_OUT_OF_REACH },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("secret", ParamFrom::Response("/error/secret")), ("as", ParamFrom::Response("/error/as"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

fn secret_of(ctx: &Ctx) -> Result<&str, String> {
    text_param(ctx, "secret").ok_or_else(|| "name the secret: secret=NAME".to_string())
}

/// `as=`, if the agent said; `None`: the secret's own delivery.
fn how_of(ctx: &Ctx) -> Result<Option<Delivery>, String> {
    match text_param(ctx, "as") {
        None => Ok(None),
        Some(w) if HOWS.contains(&w) => Delivery::parse(w).map(Some).map_err(|e| e.hint),
        Some(w) => Err(format!("as={w}: it is fetch, agent, file or env")),
    }
}

fn selector(name: &str) -> Selector {
    Selector::Name { namespace: Namespace::User, scope: Scope::server(), name: name.to_string() }
}

/// `Some(Click)` when the delivery asked for can never be read by the agent.
pub(crate) fn proof(action: &str, ctx: &Ctx) -> Option<Proof> {
    (action == ACTION && matches!(text_param(ctx, "as"), Some("fetch" | "agent"))).then_some(Proof::Click)
}

/// The card's computed words; never from the vault (V20), so the text hash
/// holds from the ask to the click and says the same whether or not the
/// secret exists.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let ws = &ctx.workspace_handle;
    let name = text_param(ctx, "secret").unwrap_or("");
    let how = match text_param(ctx, "as") {
        Some(w) => format!("as {w}"),
        None => "the way it is set up (its own delivery)".to_string(),
    };
    let reads = match text_param(ctx, "as") {
        Some("fetch") => format!("{ws} can't read the value: K2 adds it to {ws}'s calls."),
        Some("agent") => format!("{ws} can't read the value: K2's ssh agent signs with it."),
        _ => format!("{ws} can read the value from then on, and so can anything it runs."),
    };
    out.params.insert("how".into(), json!(how));
    out.params.insert(
        "effect".into(),
        json!(format!("{reads} It stays until someone removes it: Settings → Vault, the Share on {name}. An environment variable reaches {ws}'s new sessions only.")),
    );
    Some(out)
}

/// The macOS sheet's words for this card's confirm.
pub(crate) fn confirm_words(ctx: &Ctx) -> String {
    let ctx = with_computed(ACTION, ctx).unwrap_or_else(|| ctx.clone());
    crate::permission::present::fill("let {ws} use the server secret {secret} {how}", &ctx)
}

/// The values hold; nothing else is looked at (V20).
fn check(ctx: &Ctx) -> Check {
    if let Err(e) = secret_of(ctx).and(how_of(ctx)) {
        return Check::Moved(e);
    }
    Check::Needed
}

/// The server secret the card names, or the line an approver reads instead.
/// A vault fault isn't "missing": the click decides then.
fn target(vault: &k2_vault::LocalVault, name: &str) -> Result<k2_vault::SecretMeta, Option<String>> {
    match vault.meta(&selector(name), &Actor::System("permission-card".into())) {
        Ok(m) if m.status == SecretStatus::Active => Ok(m),
        Ok(_) => Err(Some(format!("{name} is disabled; nothing to allow."))),
        Err(e) if e.code == "not_found" => Err(Some(format!("K2 has no server secret {name}; nothing to allow."))),
        Err(_) => Err(None),
    }
}

/// For a person who can answer, and only them: the line that replaces the
/// card's consequences when the secret isn't there (only Deny is offered).
pub(crate) fn missing(action: &str, request_id: &str) -> Option<String> {
    if action != ACTION {
        return None;
    }
    let req = crate::permission::store::get_by_display_id(request_id).ok().flatten()?;
    let ctx = crate::permission::present::ctx_of(&req);
    let vault = crate::vault::get().ok()?;
    target(&vault, secret_of(&ctx).ok()?).err().flatten()
}

/// How the agent uses it, for its turn.
fn use_words(name: &str, how: Delivery) -> String {
    match how {
        Delivery::Fetch => format!("k2 vault fetch --secret {name} --scope server <url> (K2 adds it; you never see it)."),
        Delivery::Agent => "ssh and git sign with it on $SSH_AUTH_SOCK in new sessions.".to_string(),
        Delivery::File => format!("k2 vault file {name} --scope server for a short-lived 0600 file, or k2 vault sync to put it in $K2_SECRETS_DIR now."),
        _ => {
            let var = k2_vault::delivery::default_env_name(name);
            format!(
                "new sessions get ${var}. An environment variable reaches new sessions only, so for ${var} in this chat run k2 agent refresh; until then k2 vault env-file --secret {name} --scope server works now."
            )
        }
    }
}

/// Through `/cli/vault/grant`'s own entry, as the person who clicked, to the
/// asking workspace only.
fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let (name, asked) = match (secret_of(ctx), how_of(ctx)) {
        (Ok(n), Ok(h)) => (n.to_string(), h),
        (Err(e), _) | (_, Err(e)) => return refused("bad_params", e),
    };
    if let Err(r) = route_floor(ROUTE, who) {
        return r;
    }
    let Some(request) = super::super::answer::current_request() else {
        return refused("failed", "no request is being answered".into());
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return refused(e.code, e.hint),
    };
    let m = match target(&vault, &name) {
        Ok(m) => m,
        Err(line) => return refused("not_found", line.unwrap_or_else(|| "the vault can't answer right now; try again".into())),
    };
    let how = asked.unwrap_or(m.default_delivery);
    let stamp = who.stamp();
    // A readable grant (or one the card couldn't tell): the card's confirm.
    let proved = if how.readable() || asked.is_none() {
        let Some(token) = token else {
            return refused("vault_confirm_required", "confirm it's you first".into());
        };
        match vault.confirm.take(token, SHARE_CONFIRM, Some(&request), &stamp) {
            Ok(HumanProof::Confirmed { method }) => Some(method),
            Ok(HumanProof::None) => return refused("vault_confirm_required", "confirm again".into()),
            Err(e) => return refused(e.code, e.hint),
        }
    } else {
        None
    };
    if crate::vault::workspace_id(&ctx.workspace_handle).as_deref() != Some(ctx.workspace_id.as_str()) {
        return refused("not_found", format!("K2 can't find the workspace {} any more", ctx.workspace_handle));
    }
    let system = Actor::System("permission-card".into());
    let existing = match vault.grants(&Selector::Id(m.id.clone()), &system) {
        Ok(g) => g,
        Err(e) => return refused(e.code, e.hint),
    };
    if existing
        .iter()
        .any(|g| g.consumer.kind == ConsumerKind::Workspace && g.consumer.id == ctx.workspace_id && g.delivery == how && g.expires_at.is_none())
    {
        return Applied::Done { message: format!("{} already has {name} (server) as {}, so nothing changed: {}", ctx.workspace_handle, how.as_str(), use_words(&name, how)) };
    }
    let mut body = json!({ "id": m.id, "to": format!("ws:{}", ctx.workspace_handle), "as": how.as_str() });
    if let (true, Some(method), Answerer::OwnerToken) = (how.readable(), &proved, who) {
        body["confirmToken"] = json!(vault.confirm.mint(crate::vault_routes::GRANT_READ_CONFIRM_ACTION, Some(&m.id), &stamp, method));
    }
    let r = crate::vault_routes::handle_on(&vault, ROUTE, Ok(caller_of(who)), body.to_string().as_bytes(), &HashMap::new(), "permission-card", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    Applied::Done { message: format!("You can use {name} (server) as {} until someone removes it: {}", how.as_str(), use_words(&name, how)) }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &GRANT_SERVER, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_grant_server_tests.rs"]
mod tests;
