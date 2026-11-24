//! `mail.address-cap target=<workspace> cap=<n|none>` (0.46.18): an agent
//! asks a person to set or clear a workspace's hosted mail address cap, the
//! way `k2 hostmail config --workspace <ws> --address-cap <n>|none` does
//! for the owner (Rosson: "that command … should also be possible via a
//! card").
//!
//! **Where the agent meets it.** Three refusals end with this card's line
//! (with cards on):
//! - `k2 hostmail create` past the cap (409 `cap_reached`; the refusal
//!   names the workspace);
//! - `k2 hostmail config --workspace <ws> --address-cap …` by an agent
//!   without "manage hosted mail" (403 `agent_send_path_only`);
//! - the same on its **own** workspace by an agent with it (403
//!   `owner_only`): such an agent has no cap unless the owner sets one, and
//!   that cap is the owner's.
//!
//! `cap=0` is no limit, `cap=none` clears a cap set for the workspace (back
//! to no limit for an agent that manages email hosting, the server default
//! for any other). Any workspace on this server, like `mail.stop-sending`:
//! a person allows every card.
//!
//! - **Who answers:** the route's floor, an Admin (the owner token, an Owner
//!   or an Admin login). Proof: a click, like Settings.
//! - **On Allow:** `mail_routes::mail_address_cap_entry` (the
//!   `POST /cli/mail/config/set` route as the person who clicked), once.
//!   Hosted mail runs on Linux only: elsewhere the card isn't offered.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::workspace::settings::{mail_address_cap_info_for_path, mail_cap_clears, mail_manage_allowed_for_path};
use serde_json::json;

use super::held::text_param;
use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};
use k2_core::cli_name;

pub(crate) const ACTION: &str = "mail.address-cap";
/// `k2 hostmail config --workspace <ws> --address-cap <n>|none`.
pub(crate) const ROUTE: &str = "/cli/mail/config/set";
/// `k2 hostmail create` past the cap.
pub(crate) const CREATE_ROUTE: &str = "/cli/mail/address/create";

pub(crate) static ADDRESS_CAP: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Mail,
    params: &[
        ParamSpec { key: "target", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
        ParamSpec { key: "cap", kind: ParamKind::Text, required: true, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{target}'s email address cap",
        what_changes: "{change}",
        consequences: concat!(
            "Today: {now}. Afterwards {target}'s agents may create {after} on this server's hosted domains (only active addresses count; deleting one frees its slot), as ",
            cli_name!(),
            " hostmail config --workspace {target} --address-cap {cap} does. Send limits, mailbox sizes and the server's other limits stay as they are. {ws} gets no other access to {target}."
        ),
        scope: "{target}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{target}/permissions", path: "Settings → Workspaces / Agents → {target} → Permissions → Hosted mail" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 409, code: "cap_reached" },
            route_prefix: CREATE_ROUTE,
            method: WallMethod::Post,
            params: &[("target", ParamFrom::Response("/error/workspace"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: "agent_send_path_only" },
            route_prefix: ROUTE,
            method: WallMethod::Post,
            params: &[("target", ParamFrom::Body("/workspace")), ("cap", ParamFrom::Body("/addressCap"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE,
            method: WallMethod::Post,
            params: &[("target", ParamFrom::Body("/workspace")), ("cap", ParamFrom::Body("/addressCap"))],
        },
    ],
    coverage: WallCoverage::PrefixOnly,
    form: None,
    machine_hints: &[],
};

/// What the card sets: a number (`0` = no limit) or `None` (clear).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapValue {
    Set(u32),
    Clear,
}

fn cap_of(ctx: &Ctx) -> Result<CapValue, String> {
    let raw = text_param(ctx, "cap").ok_or("name the cap: cap=<number> (0 = no limit) or cap=none")?;
    if mail_cap_clears(raw) {
        return Ok(CapValue::Clear);
    }
    raw.parse::<u32>()
        .map(CapValue::Set)
        .map_err(|_| format!("cap must be a whole number (0 = no limit) or none, got {raw}"))
}

/// The named workspace on this server: `(handle, path)`.
fn target_of(ctx: &Ctx) -> Result<(String, String), String> {
    let token = text_param(ctx, "target").ok_or("name the workspace: target=<workspace>")?;
    let path = crate::workspace_msg::resolve_workspace(token).ok_or_else(|| format!("workspace not found: {token}"))?;
    let handle = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COALESCE(handle, name) FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| token.to_string())
    };
    Ok((handle, path))
}

/// "up to 12 email addresses" / "as many email addresses as they need".
fn amount(cap: u32) -> String {
    match cap {
        0 => "as many email addresses as they need".to_string(),
        1 => "1 email address".to_string(),
        n => format!("up to {n} email addresses"),
    }
}

/// The cap after the change, from the workspace's own state.
fn after_of(path: &str, cap: CapValue) -> u32 {
    match cap {
        CapValue::Set(n) => n,
        CapValue::Clear if mail_manage_allowed_for_path(path) => 0,
        CapValue::Clear => k2_core::app_settings::load().mail_address_cap,
    }
}

/// The card names the workspace by its handle and says in plain words what
/// the cap becomes: `{change}`, `{now}`, `{after}`.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let (Ok((handle, path)), Ok(cap)) = (target_of(ctx), cap_of(ctx)) else {
        return Some(out);
    };
    out.params.insert("target".into(), json!(handle));
    let after = after_of(&path, cap);
    let change = match cap {
        CapValue::Set(0) => format!("Let {handle} have as many email addresses as it needs"),
        CapValue::Set(n) => format!("Let {handle} have {}", amount(n)),
        CapValue::Clear if mail_manage_allowed_for_path(&path) => {
            format!("Clear {handle}'s email address cap: it manages email hosting, so it has no limit")
        }
        CapValue::Clear => format!("Clear {handle}'s email address cap: back to the server default, {}", amount(after)),
    };
    let now = mail_address_cap_info_for_path(&path).words();
    out.params.insert("change".into(), json!(change));
    out.params.insert("now".into(), json!(now));
    out.params.insert("after".into(), json!(amount(after)));
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    if !crate::mail::supervisor::mail_supported() {
        return Check::Unavailable { code: "not_available", why: "hosted mail runs on Linux servers only".to_string() };
    }
    let (_, path) = match target_of(ctx) {
        Ok(t) => t,
        Err(e) => return Check::Moved(e),
    };
    let cap = match cap_of(ctx) {
        Ok(c) => c,
        Err(e) => return Check::Moved(e),
    };
    let stored: Option<i64> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT mail_address_cap FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get::<_, Option<i64>>(0))
            .ok()
            .flatten()
    };
    let done = match cap {
        CapValue::Set(n) => stored == Some(i64::from(n)),
        CapValue::Clear => stored.is_none(),
    };
    if done {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (handle, path) = match target_of(ctx) {
        Ok(t) => t,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let cap = match cap_of(ctx) {
        Ok(c) => c,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let wire = match cap {
        CapValue::Set(n) => json!(n),
        CapValue::Clear => json!("none"),
    };
    let r = crate::mail_routes::mail_address_cap_entry(who, &path, wire);
    let now = mail_address_cap_info_for_path(&path).words();
    applied(&r, format!("{handle}'s email address cap is changed. {now}."))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &ADDRESS_CAP, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "mail_address_cap_tests.rs"]
mod tests;
