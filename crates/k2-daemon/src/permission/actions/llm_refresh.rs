//! `llm.tokens-refresh [token=<id>]` (0.46.17, field report 2026-11-16): an
//! agent asks a person to recheck the server's LLM tokens (one, or all), the
//! way `k2 llm tokens refresh` does for a person. The route stays people
//! only: an agent's `POST /cli/llm/accounts/refresh` is refused `owner_only`,
//! and with cards on that refusal ends with this card's line instead of a
//! dead end.
//!
//! **What a recheck is.** It re-reads each token's sign-in from where it is
//! kept (the server default from the tool's own store) and records the state,
//! the access token's expiry and the account labels; it never signs in,
//! switches, removes or shows a token, and it never refreshes the server
//! default (its CLIs do, LT-A10). An access token that has expired on a good
//! sign-in needs no card at all: the list says `access_expired` and who
//! renews it. A token that is signed out needs a person to sign it in again
//! (Settings → Language Models → Subs / Tokens; the owners get the sign-in
//! warnings): the recheck's answer says so.
//!
//! - **Who answers:** the owner token, an Owner or an Admin login (a card's
//!   lowest floor; the route's own Member floor runs again at the click).
//!   Proof: a click.
//! - **On Allow:** the route's own entry ([`crate::llm_accounts_routes::handle`])
//!   as the person who clicked, once, with the token the refused request
//!   named (or every token). The agent's turn says what the recheck found
//!   for each token: labels and states only, never a value.

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::{cli_name, product_name};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::llm_accounts_routes::REFRESH as ROUTE;

pub(crate) const ACTION: &str = "llm.tokens-refresh";

pub(crate) static REFRESH: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Misc,
    params: &[ParamSpec { key: "token", kind: ParamKind::Text, required: false, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Recheck LLM tokens",
        what_changes: "Recheck {what} on this server",
        consequences: concat!(
            product_name!(),
            " re-reads {what} from where each is kept and records what it finds (signed in or out, when the access token expires), as ",
            cli_name!(),
            " llm tokens refresh does. Nothing is signed in, switched or removed, no token is shown, and the server default is never refreshed (its CLIs renew it). {ws} is told each token's state."
        ),
        scope: "{what}",
        duration: "once",
        retry: concat!("Read the states with ", cli_name!(), " llm tokens list."),
    },
    undo: Undo::None,
    settings: SettingsRef { page: "llm-tokens", path: "Settings → Language Models → Subs / Tokens" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("token", ParamFrom::Body("/id"))],
    }],
    coverage: WallCoverage::PrefixOnly,
    form: None,
    machine_hints: &[],
};

fn token_of(ctx: &Ctx) -> Option<&str> {
    text_param(ctx, "token")
}

/// The named token, when the card names one: `Ok(None)` for every token.
fn named(ctx: &Ctx) -> Result<Option<k2_core::llm_accounts::Entry>, String> {
    let Some(id) = token_of(ctx) else { return Ok(None) };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::llm_accounts::lookup(&conn, None, id).map(Some).map_err(|e| e.to_string())
}

/// `{what}`: "the Claude token \"work\"" or "every LLM token", from the
/// wallet's own rows, never the agent's words.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let what = match named(ctx) {
        Ok(Some(e)) => format!("the {} token \"{}\"", e.tool().map_or(e.tool.as_str(), |t| t.display()), e.label),
        Ok(None) => "every LLM token".to_string(),
        Err(_) => format!("the token {}", token_of(ctx).unwrap_or("?")),
    };
    out.params.insert("what".into(), json!(what));
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    match named(ctx) {
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

/// One token as the recheck left it: `Claude "work" (server default):
/// signed in; its access token expired on ...`.
fn state_words(a: &Value) -> String {
    let label = a["label"].as_str().unwrap_or("?");
    let tool = a["tool"].as_str().and_then(k2_core::llm_accounts::Tool::parse).map_or("?", |t| t.display());
    let default = if a["active"] == true { " (server default)" } else { "" };
    let detail = a["detail"].as_str().filter(|d| !d.trim().is_empty() && !k2_core::llm_accounts::looks_like_secret(d));
    let state = match (a["access"].as_str(), a["state"].as_str()) {
        (Some("expired"), _) => format!("signed in. {}", a["accessNote"].as_str().unwrap_or("Its access token has expired.")),
        (Some("fine"), _) => "signed in".to_string(),
        (Some("signed_out"), _) => match detail {
            Some(d) => format!("signed out: {d} A person signs it in again on Settings → Language Models → Subs / Tokens."),
            None => "signed out: a person signs it in again on Settings → Language Models → Subs / Tokens.".to_string(),
        },
        (_, Some(st)) => st.to_string(),
        _ => "unknown".to_string(),
    };
    let line = format!("{tool} \"{label}\"{default}: {state}");
    if line.ends_with('.') {
        line
    } else {
        format!("{line}.")
    }
}

/// Through the route's own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let caller = match who {
        Answerer::OwnerToken => crate::sidecar_routes::Caller::Owner,
        Answerer::Login { user, .. } => crate::sidecar_routes::Caller::Login { username: user.clone() },
    };
    let body = match token_of(ctx) {
        Some(id) => json!({ "id": id }),
        None => json!({}),
    };
    let r = crate::llm_accounts_routes::handle(ROUTE, caller, body.to_string().as_bytes(), &HashMap::new(), "permission-card");
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "llm_refresh_refused");
        return Applied::Refused { code, message };
    }
    let lines: Vec<String> = v["accounts"].as_array().map(|a| a.iter().map(state_words).collect()).unwrap_or_default();
    let found = if lines.is_empty() { "No tokens on this server.".to_string() } else { lines.join(" ") };
    Applied::Done { message: format!("Rechecked. {found}") }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &REFRESH, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "llm_refresh_tests.rs"]
mod tests;
