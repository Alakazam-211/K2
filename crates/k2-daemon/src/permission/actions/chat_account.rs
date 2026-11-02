//! `chat.account [scope=coworker|assistant:<person>]` (0.46.8): an agent
//! asks for its **own** workspace's chat account, the Sew member the
//! `chat` grant makes. Until now only the owner made one, from a terminal:
//! `k2 app grant add --kind chat --subject-kind workspace --subject <ws>
//! --scope coworker|assistant:<user>` (`POST /cli/skin/grants`,
//! `owner_role_identity`). An agent needs it, for example, to be the
//! manager of a Space it creates (`k2 chat space create … --app`): Sew
//! names only a person or an agent with its own account a manager.
//!
//! The walls (cards on): an agent's own `k2 app grant add --kind chat
//! --subject-kind workspace --subject <its own workspace>` (`owner_only`),
//! `k2 chat space create` refused up front with `chat_account_needed`, and
//! any `k2 chat` read refused `not_granted` while the workspace has no
//! account (`error.details.chatAccount`).
//!
//! - **Who asks:** an agent, for its own workspace only: the card has no
//!   workspace value (the workspace comes from the passport), and the
//!   grant wall teaches it only when the refused grant names the agent's
//!   own workspace. Another workspace's chat grant stays "It can't be
//!   requested."
//! - **Scope:** `coworker` (the default) or `assistant:<person>` (a
//!   username or id on this box, or `owner`), as the grant takes it. A
//!   workspace that already has an account with the other scope is the
//!   owner's to change (the grant is deleted and added again).
//! - **Who answers:** who the route admits: the owner token or an Owner
//!   login. Proof: a click, like the route.
//! - **On Allow:** the grants route's own handler runs as the person who
//!   clicked, with exactly the body `k2 app grant add` sends. Once: the
//!   answer is a compare-and-set.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::tool_hook::CHAT_KIND;
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) const ACTION: &str = "chat.account";
/// The grants route the owner uses (`k2 app grant add`).
pub(crate) const ROUTE: &str = "/cli/skin/grants";
/// `k2 chat space create`: refused up front without an account.
pub(crate) const SPACE_CREATE: &str = "/cli/sew/chat/manage/space/create";
/// Every `k2 chat` agent route (a read with no account: `not_granted`).
pub(crate) const CHAT_ROUTES: &str = "/cli/sew/chat/";
/// The route's own floor (`owner_role_identity`): the owner.
pub(crate) const FLOOR: Floor = Floor::Owner;
pub(crate) const DEFAULT_SCOPE: &str = "coworker";

pub(crate) static CHAT_ACCOUNT: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Sew,
    params: &[ParamSpec { key: "scope", kind: ParamKind::ChatScope, required: false, list: false, choices: &[] }],
    floor: FLOOR,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{ws}'s chat account",
        what_changes: "{what}",
        consequences: "{effects}",
        scope: "{ws}",
        duration: "until someone removes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef {
        page: "people",
        path: "k2 app grant add --kind chat --subject-kind workspace --subject {ws} --scope coworker (a terminal outside K2)",
    },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: ROUTE,
            method: WallMethod::Post,
            params: &[("scope", ParamFrom::Body("/scope"))],
        },
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::sew_chat::routes::CHAT_ACCOUNT_NEEDED },
            route_prefix: SPACE_CREATE,
            method: WallMethod::Post,
            params: &[],
        },
        Wall { matches: WallMatch::Code { status: 403, code: "not_granted" }, route_prefix: CHAT_ROUTES, method: WallMethod::Any, params: &[] },
    ],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[],
};

/// The scope asked for, checked against the box's people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    /// What the grant stores (`coworker`, `assistant:owner`,
    /// `assistant:<principal id>`), to compare with an existing grant.
    pub stored: String,
    /// What the card says (`a coworker`, `dana's assistant`).
    pub label: String,
    /// The person an assistant speaks for, by name.
    pub person: Option<String>,
}

pub(crate) fn scope_of(ctx: &Ctx) -> Result<Scope, String> {
    let raw = text_param(ctx, "scope").unwrap_or(DEFAULT_SCOPE);
    let (kw, rest) = match raw.split_once(':') {
        Some((k, r)) => (k.trim().to_ascii_lowercase(), Some(r.trim())),
        None => (raw.trim().to_ascii_lowercase(), None),
    };
    match (kw.as_str(), rest) {
        ("coworker", None) => Ok(Scope { stored: "coworker".into(), label: "a coworker".into(), person: None }),
        ("assistant", Some(p)) if p.eq_ignore_ascii_case("owner") => {
            Ok(Scope { stored: "assistant:owner".into(), label: "the owner's assistant".into(), person: Some("the owner".into()) })
        }
        ("assistant", Some(p)) if !p.is_empty() => {
            let want = p.trim_start_matches('@');
            let found = k2_core::skin::list_principals()?
                .into_iter()
                .find(|x| x.username.eq_ignore_ascii_case(want) || x.id == want)
                .ok_or_else(|| format!("there's no person named {want} on this server (k2 app user list)"))?;
            if k2_core::skin::principal_agent_workspace(&found.id).map_err(|e| e.to_string())?.is_some() {
                return Err(format!("{} is an agent's login: an agent assists a person, never another agent", found.username));
            }
            Ok(Scope { stored: format!("assistant:{}", found.id), label: format!("{}'s assistant", found.username), person: Some(found.username) })
        }
        _ => Err(format!("scope={raw}: coworker, or assistant:<person> (a username, or owner)")),
    }
}

/// The workspace's chat grant, if any: `(enabled, stored scope)`.
fn existing(workspace_id: &str) -> Result<Option<(bool, String)>, String> {
    Ok(k2_core::skin::list_grants()?
        .into_iter()
        .find(|g| g.kind == CHAT_KIND && g.subject_kind == "workspace" && g.subject_id.eq_ignore_ascii_case(workspace_id))
        .map(|g| (g.enabled, g.scope.unwrap_or_default())))
}

fn chat_on() -> Result<bool, String> {
    Ok(k2_core::tool_hook::get_target(CHAT_KIND)?.is_some_and(|t| t.enabled))
}

fn label_of_stored(stored: &str) -> String {
    match stored {
        "coworker" => "a coworker".into(),
        "assistant:owner" => "the owner's assistant".into(),
        s => match s.strip_prefix("assistant:") {
            Some(id) => {
                let name = k2_core::skin::list_principals().ok().and_then(|ps| ps.into_iter().find(|p| p.id == id)).map(|p| p.username);
                format!("{}'s assistant", name.as_deref().unwrap_or(id))
            }
            None => s.to_string(),
        },
    }
}

/// The values hold, and what the card would do.
fn check(ctx: &Ctx) -> Check {
    let scope = match scope_of(ctx) {
        Ok(s) => s,
        Err(e) => return Check::Moved(e),
    };
    if super::switches::ws_path(&ctx.workspace_id).is_none() {
        return Check::Moved(super::switches::GONE.into());
    }
    match chat_on() {
        Ok(true) => {}
        Ok(false) => {
            return Check::Unavailable {
                code: "chat_unavailable",
                why: "Chat / Voice Hosting isn't on for this server, so there are no chat accounts yet: the owner turns it on first (k2 hostchat status)".into(),
            }
        }
        Err(e) => return Check::Unavailable { code: "chat_unavailable", why: e },
    }
    match existing(&ctx.workspace_id) {
        Err(e) => Check::Unavailable { code: "chat_unavailable", why: e },
        Ok(None) => Check::Needed,
        Ok(Some((true, stored))) if stored == scope.stored => Check::AlreadyDone,
        Ok(Some((true, stored))) => Check::Moved(format!(
            "{} already has a chat account, as {}: changing it to {} is the owner's (they remove the chat grant and add it again)",
            ctx.workspace_handle,
            label_of_stored(&stored),
            scope.label
        )),
        Ok(Some((false, _))) => Check::Moved(format!(
            "{}'s chat account is turned off: turning it back on is the owner's (k2 app grant list shows it)",
            ctx.workspace_handle
        )),
    }
}

/// The card's words from the stored values: `{what}`, `{effects}`.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let ws = &ctx.workspace_handle;
    let (what, effects) = match scope_of(ctx) {
        Ok(s) => match &s.person {
            None => (
                format!("Give {ws} its own chat account, as a coworker"),
                format!(
                    "{ws} signs in to chat as itself, an agent. It sees only the Spaces a person gives it (k2 chat access grant) and the Spaces it creates, and it can be the manager of a Space it creates. Sew's roles still decide what it may do in each Space. To take it back: k2 app grant list, then k2 app grant rm --id <the chat grant's id>."
                ),
            ),
            Some(p) => (
                format!("Give {ws} its own chat account, as {p}'s assistant"),
                format!(
                    "{ws} signs in to chat as an agent that speaks for {p}: what it posts for {p} is marked as theirs. An assistant is never a Space's manager. It sees only the Spaces a person gives it (k2 chat access grant). To take it back: k2 app grant list, then k2 app grant rm --id <the chat grant's id>."
                ),
            ),
        },
        Err(e) => (format!("Give {ws} its own chat account"), e),
    };
    out.params.insert("what".into(), json!(what));
    out.params.insert("effects".into(), json!(effects));
    Some(out)
}

/// The route's own actor label for who clicked (`owner_role_identity`).
fn actor_of(who: &Answerer) -> String {
    match who {
        Answerer::OwnerToken => "owner-token".to_string(),
        Answerer::Login { user, .. } => format!("user:{user}"),
    }
}

/// Through the grants route's own handler, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let scope = match scope_of(ctx) {
        Ok(s) => s,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    // `owner_role_identity`: the owner token or an Owner login, nobody else.
    if let Answerer::Login { user, .. } = who {
        let owner = k2_core::connect_users::role_for_user(user).is_some_and(k2_core::connect_users::can_change_roles);
        if !owner {
            return Applied::Refused { code: "owner_only".into(), message: "only the server's owner gives an agent a chat account".into() };
        }
    }
    // The body `k2 app grant add --kind chat --subject-kind workspace
    // --subject <ws> --scope <scope>` sends (the target defaults to Sew).
    let body = json!({ "subjectKind": "workspace", "subjectId": ctx.workspace_id, "kind": CHAT_KIND, "scope": scope.stored });
    let r = crate::skin_routes::handle_grants_post(body.to_string().as_bytes(), &actor_of(who));
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "refused");
        return Applied::Refused { code, message };
    }
    Applied::Done {
        message: format!(
            "{} has its own chat account now, as {}; it stays until someone removes it. Sew sets it up within a few seconds; then run your command again.",
            ctx.workspace_handle, scope.label
        ),
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &CHAT_ACCOUNT, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "chat_account_tests.rs"]
mod tests;
