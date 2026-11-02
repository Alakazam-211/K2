//! `mail.stop-sending target=<workspace>` (0.46.8): an agent asks for one
//! workspace's agent email sending to be turned off, its own or another
//! one's on this server. `k2 mail sending <ws> off` (`POST
//! /cli/mail/config/set {workspace, agentSend:"off"}`) is a person's: an
//! agent without "manage hosted mail" is refused `owner_only`, and one with
//! it is refused on its own workspace (it can't change the rules on its
//! own sends). With cards on, that refusal ends with this card's line.
//!
//! **Why any workspace, and only off.** K2 has no relation in which one
//! workspace oversees another: `workspace_relations` rows are connections,
//! shown both ways ("connected", 0.39.0), whose stored `oversees` is only
//! the default label every `k2 connections add` writes; `team_links` say a
//! team serves a workspace, not who answers for it. So no agent asks to
//! widen another workspace's sending. Turning sending off only tightens,
//! and a person still allows every card, so any agent on the server may
//! ask for it. Turning its own on (approval or freely) is the
//! `mail.send-mode` card, and `k2 mail sending <own> approval|on` teaches
//! that one.
//!
//! - **Who answers:** the route's floor, an Admin (the owner token, an Owner
//!   or an Admin login). Proof: a click, like Settings.
//! - **On Allow:** `mail_routes::mail_send_mode_entry` (what Settings
//!   writes) as the person who clicked, with `off`. Once.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};

use super::held::text_param;
use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "mail.stop-sending";
/// `k2 mail sending <ws> off` (and Settings → Email sending).
pub(crate) const ROUTE: &str = "/cli/mail/config/set";

pub(crate) static STOP_SENDING: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Mail,
    params: &[ParamSpec { key: "target", kind: ParamKind::Workspace, required: true, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{target}'s email sending",
        what_changes: "Stop {target} sending email",
        consequences: "{target}'s agents can't send email from any inbox (hosted or linked) until someone turns sending on again (k2 mail sending {target} approval|on, or Settings). Mail they already sent stays sent. {ws} gets no other access to {target}.",
        scope: "{target}",
        duration: "until someone turns it on again",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{target}", path: "Settings → Workspaces / Agents → {target} → Email sending" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("target", ParamFrom::Body("/workspace"))],
    }],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

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

/// The card names the workspace by its handle, however the agent wrote it
/// (a path, an id).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    if let Ok((handle, _)) = target_of(ctx) {
        out.params.insert("target".into(), serde_json::json!(handle));
    }
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    match target_of(ctx) {
        Err(e) => Check::Moved(e),
        Ok((_, path)) if k2_core::workspace::settings::mail_agent_send_for_path(&path) == "off" => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (handle, path) = match target_of(ctx) {
        Ok(t) => t,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    let r = crate::mail_routes::mail_send_mode_entry(who, &path, "off");
    applied(&r, format!("{handle}'s agents can't send email now: sending stays off until someone turns it on again."))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &STOP_SENDING, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "mail_stop_tests.rs"]
mod tests;
