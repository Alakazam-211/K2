//! `vault.accept-from sender=<agent> receivers=<ws1>[,<ws2>…]` (0.46.17, a
//! user's ask): let some agents take vault sends from one agent without a
//! person accepting each.
//!
//! - **Before:** `k2 vault accept-from add ws:<sender> --workspace <receiver>`
//!   was a person's, typed in their own terminal (or Settings → Permissions),
//!   one receiver at a time, with no card (`handoff_person_only` for a
//!   passport, H19).
//! - **Who asks:** any agent, normally the sender. Asking changes nothing: a
//!   person answers. `--assign <login>` (the ask's `--to`) sends the card to
//!   that owner or admin only.
//! - **One card, several receivers:** "Let {receivers} accept vault sends
//!   from {sender}", every receiver named, with the agent's why.
//! - **Who answers:** the owner token, an Owner or an Admin login (a card's
//!   lowest floor, and the route's own: it refuses any other login). An agent never: the
//!   card is a person's, and the route refuses a passport.
//! - **Proof:** a click by a person, the same as the command today (it takes
//!   no vault confirm).
//! - **Allow** runs `POST /cli/vault/accept-from`'s own entry once per
//!   receiver, as the person who clicked ([`apply`]); the vault audit and the
//!   Access Audit name that person. Receivers that already accept from the
//!   sender are left as they are.
//! - **Where it stands:** `k2 permission status <id>`, as every card.

use std::collections::HashMap;

use k2_core::cli_name;
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    WallCoverage, Words,
};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::vault::caller_of;
use super::{Answerer, Applied, Check, Ctx, Registered};

/// The card's id.
pub(crate) const ACTION: &str = "vault.accept-from";
/// `k2 vault accept-from add <sender> --workspace <receiver>`.
pub(crate) const ROUTE: &str = crate::vault_handoff_routes::ACCEPT_FROM;
/// The ingress the card's writes run with (the Access Audit's `via`).
const INGRESS: &str = "permission-card";
/// Receivers on one card.
pub(crate) const MAX_RECEIVERS: usize = 8;

pub(crate) static ACCEPT_FROM: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "sender", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "receivers", kind: ParamKind::Workspace, required: true, list: true, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    // `{who}` and `{from}` are K2's own words ([`with_computed`]): the
    // receivers' and the sender's names, never ids.
    words: Words {
        setting: "Accept vault sends from {from}",
        what_changes: "Let {who} accept vault sends from {from}",
        consequences: concat!(
            "From now on a secret {from} sends to {who} goes straight into their vault, without a person accepting it each time. ",
            "It doesn't let them send anything, or read anyone else's secrets. Take it back for one of them with ",
            cli_name!(),
            " vault accept-from remove <sender> --workspace <agent>, or in Settings → Workspaces / Agents → <agent> → Permissions."
        ),
        scope: "{who}",
        duration: "until a person removes it",
        retry: "send it again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces", path: "Settings → Workspaces / Agents → <agent> → Permissions" },
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

/// The receivers the request names, in order, without repeats.
fn receivers_of(ctx: &Ctx) -> Vec<String> {
    let raw: Vec<String> = match ctx.params.get("receivers") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).flat_map(|s| s.split(',')).map(|s| s.trim().to_string()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).collect(),
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

/// The sender as the accept-from list keeps it: a local agent is
/// `ws:<id>` (a bare name or `ws:<name>` names one), anything else
/// (`team:<id>`, `agent::host`) as given — the route's own rule.
fn sender_key(ctx: &Ctx) -> Result<String, String> {
    let raw = text_param(ctx, "sender").ok_or("name the sender: sender=ws:<agent> (or agent::host for another server)")?;
    if !raw.contains(':') && !raw.contains('@') {
        return crate::vault::workspace_id(raw).map(|id| format!("ws:{id}")).ok_or_else(|| format!("no agent named {raw} on this server"));
    }
    if let Some(n) = raw.strip_prefix("ws:") {
        return crate::vault::workspace_id(n).map(|id| format!("ws:{id}")).ok_or_else(|| format!("no agent named {n} on this server"));
    }
    Ok(raw.to_string())
}

/// Each receiver's workspace id, or why not.
fn receiver_ids(ctx: &Ctx) -> Result<Vec<(String, String)>, String> {
    let names = receivers_of(ctx);
    if names.is_empty() {
        return Err("name who accepts: receivers=<agent>[,<agent>…]".into());
    }
    if names.len() > MAX_RECEIVERS {
        return Err(format!("at most {MAX_RECEIVERS} receivers on one card"));
    }
    names
        .iter()
        .map(|n| {
            let t = n.strip_prefix("ws:").unwrap_or(n);
            crate::vault::workspace_id(t).map(|id| (n.clone(), id)).ok_or_else(|| format!("no agent named {t} on this server"))
        })
        .collect()
}

/// "A", "A and B", "A, B and C".
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The sender's name: a local agent's handle, else the address as given.
fn sender_words(ctx: &Ctx) -> String {
    match sender_key(ctx) {
        Ok(k) => match k.strip_prefix("ws:") {
            Some(id) => crate::vault::workspace_label(id),
            None => k,
        },
        Err(_) => text_param(ctx, "sender").unwrap_or("").to_string(),
    }
}

/// The card's computed words. They read names only (never the vault), so
/// the text hash is the same at the ask and the click.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let who: Vec<String> = match receiver_ids(ctx) {
        Ok(ids) => ids.iter().map(|(_, id)| crate::vault::workspace_label(id)).collect(),
        Err(_) => receivers_of(ctx),
    };
    out.params.insert("who".into(), json!(and_list(&who)));
    out.params.insert("from".into(), json!(sender_words(ctx)));
    Some(out)
}

/// Still wanted: the sender and every receiver are known, no receiver is
/// the sender, and not every receiver already accepts from it.
fn check(ctx: &Ctx) -> Check {
    let sender = match sender_key(ctx) {
        Ok(s) => s,
        Err(e) => return Check::Moved(e),
    };
    let ids = match receiver_ids(ctx) {
        Ok(i) => i,
        Err(e) => return Check::Moved(e),
    };
    if ids.iter().any(|(_, id)| sender == format!("ws:{id}")) {
        return Check::Moved("an agent never needs to accept from itself: take the sender out of receivers=".into());
    }
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    let mut already = 0;
    for (_, id) in &ids {
        match vault.accept_from(Some(id)) {
            Ok(list) if list.iter().any(|a| a.sender == sender) => already += 1,
            Ok(_) => {}
            Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
        }
    }
    if already == ids.len() {
        return Check::AlreadyDone;
    }
    Check::Needed
}

/// Through `/cli/vault/accept-from`'s own entry, as the person who clicked,
/// once per receiver.
fn apply(ctx: &Ctx, who: &Answerer, _token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let sender = match sender_key(ctx) {
        Ok(s) => s,
        Err(e) => return refused("bad_params", e),
    };
    let ids = match receiver_ids(ctx) {
        Ok(i) => i,
        Err(e) => return refused("bad_params", e),
    };
    if let Err(r) = route_floor(ROUTE, who) {
        return r;
    }
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return refused(e.code, e.hint),
    };
    let caller = caller_of(who);
    let (mut done, mut failed): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for (_, id) in &ids {
        let label = crate::vault::workspace_label(id);
        let body = json!({ "workspace": id, "sender": sender, "action": "add" });
        let r = crate::vault_handoff_routes::handle(&vault, ROUTE, &caller, body.to_string().as_bytes(), &HashMap::new(), INGRESS);
        if r.status.starts_with("200") {
            done.push(label);
        } else {
            let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
            let (code, hint) = error_of(&v, "accept_from_refused");
            failed.push(format!("{label} ({code}: {hint})"));
        }
    }
    if done.is_empty() {
        return refused("accept_from_refused", format!("Nothing changed: {}.", failed.join("; ")));
    }
    let mut msg = format!("{} now accept vault sends from {} without a person each time.", and_list(&done), sender_words(ctx));
    if !failed.is_empty() {
        msg.push_str(&format!(" Not changed: {}.", failed.join("; ")));
    }
    Applied::Done { message: msg }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &ACCEPT_FROM, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_accept_from_tests.rs"]
mod tests;
