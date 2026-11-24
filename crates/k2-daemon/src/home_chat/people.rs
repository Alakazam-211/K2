//! Who may use Home chat, and who they can chat with (§6.1, §13, §16).
//!
//! The caller is always a person's own login on this server. The people
//! list is answered from this server's own facts, never Sew's: the owner
//! (`k2:owner`) and every enabled Connect login that isn't linked to the
//! owner (an owner-linked login IS the owner). Agents are never listed
//! (D1), App users never (D5), disabled logins never (HP-13), the caller
//! never. A role is shown only to an Owner or Admin caller.

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::tool_hook::{self as hook, members};

use crate::compute_routes::Who;
use crate::voice::bff::{self, Refusal};
use k2_core::brand::PRODUCT_NAME;
use k2_core::{cli_name, product_name};

/// A person calling Home chat from their own login.
#[derive(Debug, Clone)]
pub struct Caller {
    /// `k2:owner` or `k2:connect:<uuid>`.
    pub ext: String,
    pub login: String,
    pub role: Role,
}

impl Caller {
    /// Owner or Admin (the owner's linked login counts as the owner).
    pub fn is_admin(&self) -> bool {
        self.ext == "k2:owner" || self.role >= Role::Admin
    }
}

/// The caller behind a TCP request, or why not.
pub fn caller(query: &str, bearer: Option<&str>, owner_token: &str) -> Result<Caller, Refusal> {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string();
    caller_of(crate::compute_routes::who_from_tcp(query, bearer, owner_token), &presented, owner_token)
}

/// [`caller`] for a resolved [`Who`].
pub fn caller_of(who: Who, presented: &str, owner_token: &str) -> Result<Caller, Refusal> {
    match who {
        Who::Login { username, role } => {
            let p = bff::person_of_login(presented, &username, role, owner_token)?;
            Ok(Caller { ext: p.ext.to_string(), login: username, role })
        }
        // D12: the bare owner token can't chat (agents can read it on
        // shared-user boxes); the owner chats from their linked login.
        Who::Owner => Err(Refusal::new(
            403,
            "person_login_required",
            "Home chat is a person's own: sign in with your login on this server (the owner's linked login), not the owner token",
        )),
        Who::Agent { .. } | Who::Shell { .. } => Err(Refusal::new(
            403,
            "agents_use_k2_chat",
            concat!("Home chat is for this server's people; agents read channels they are granted with ", cli_name!(), " chat"),
        )),
        Who::Wrong => Err(Refusal::new(403, "app_session_not_allowed", "Home chat takes this server's own logins, never an App's session")),
        Who::Invalid if k2_core::connect_users::token_state(presented) == k2_core::connect_users::TokenState::DeviceEnded => {
            Err(bff::device_session_ended())
        }
        _ => Err(Refusal::new(401, "unauthenticated", "that login session is unknown, expired or revoked")),
    }
}

/// One login as Home chat sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub disabled: bool,
    pub owner_linked: bool,
}

/// Every Connect login with its id, name, role and state.
pub fn logins() -> Result<Vec<Login>, String> {
    let linked = k2_core::app_settings::load().owner_accounts;
    let names: std::collections::HashMap<String, String> = k2_core::connect_users::list_users()?
        .into_iter()
        .map(|u| {
            let full = k2_core::person_name::full_name(u.first_name.as_deref(), u.last_name.as_deref());
            let shown = full.filter(|f| !f.trim().is_empty()).unwrap_or_else(|| u.username.clone());
            (u.username, shown)
        })
        .collect();
    Ok(k2_core::connect_users::admin_space_facts()?
        .into_iter()
        .map(|(id, username, role, disabled)| Login {
            display_name: names.get(&username).cloned().unwrap_or_else(|| username.clone()),
            owner_linked: linked.iter().any(|l| l.eq_ignore_ascii_case(&username)),
            id,
            username,
            role,
            disabled,
        })
        .collect())
}

/// The person's Sew member id once their member is set up (`None` while
/// pending): the chat view names a message's author by it.
pub fn ready_member(ext: &str) -> Option<String> {
    hook::chat_member(ext).ok().flatten().filter(|m| m.state == members::ACTIVE).map(|m| m.member_id)
}

/// Every box person's picture rev by ext (prd-home-chat-pushes-v1 §5: the
/// stored picture's hash, the `imageHash` of `GET /cli/people/picture`, and
/// the `avatarRev` of their Home chat pushes). One read for the list.
fn picture_revs() -> Result<std::collections::BTreeMap<String, String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(k2_core::person_picture::hashes(&conn)?
        .into_iter()
        .filter_map(|(key, hash)| {
            if key == k2_core::person_picture::OWNER_KEY {
                return Some(("k2:owner".to_string(), hash));
            }
            key.strip_prefix(k2_core::person_picture::CONNECT_PREFIX).map(|id| (format!("k2:connect:{id}"), hash))
        })
        .collect())
}

/// One box person's picture rev (`None`: no picture).
pub fn picture_rev(ext: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::person_picture::rev_of_ext(&conn, ext).ok().flatten()
}

/// `GET /cli/home-chat/people` (§6.1). Each row's `pictureRev` is the
/// person's picture rev (`null`: no picture); the phone caches the picture
/// as `person:<ext>` and a Home chat push's `avatarRev` names the same rev.
pub fn list(c: &Caller) -> Result<Vec<Value>, String> {
    let revs = picture_revs()?;
    let mut out = Vec::new();
    let show_role = c.is_admin();
    if c.ext != "k2:owner" {
        let member = ready_member("k2:owner");
        let mut v = json!({"ext": "k2:owner", "username": "owner", "displayName": crate::workspace_msg::resolve_owner_from(), "picture": null, "pictureRev": revs.get("k2:owner"), "ready": member.is_some(), "memberId": member});
        if show_role {
            v["role"] = json!("owner");
        }
        out.push(v);
    }
    for l in logins()? {
        if l.disabled || l.owner_linked || l.id.is_empty() {
            continue;
        }
        let ext = format!("k2:connect:{}", l.id);
        if ext == c.ext {
            continue;
        }
        let member = ready_member(&ext);
        let mut v = json!({"ext": ext, "username": l.username, "displayName": l.display_name, "picture": null, "pictureRev": revs.get(&ext), "ready": member.is_some(), "memberId": member});
        if show_role {
            v["role"] = json!(l.role.as_wire());
        }
        out.push(v);
    }
    Ok(out)
}

/// May `ext` be a DM or channel target (§16, HP-13)? A box person who is
/// enabled; never an agent, an App user, or a disabled login (whose Sew
/// member stays, so this check is what holds).
pub fn check_target(ext: &str) -> Result<(), Refusal> {
    if !k2_core::home_chat::is_box_person(ext) {
        return Err(Refusal::new(
            403,
            "not_a_person",
            "Home chat is for this server's people: an agent's own conversation is its Thread",
        ));
    }
    let Some(id) = ext.strip_prefix("k2:connect:") else { return Ok(()) };
    let all = logins().map_err(|e| Refusal::new(503, "chat_unavailable", format!("{PRODUCT_NAME}'s user store is unavailable: {e}")))?;
    match all.iter().find(|l| l.id == id) {
        None => Err(Refusal::new(404, "person_not_found", "no such person on this server")),
        Some(l) if l.disabled => Err(Refusal::new(403, "target_disabled", "that login is disabled on this server")),
        Some(l) if l.owner_linked => Err(Refusal::new(404, "person_not_found", "that login is the owner's: add the owner instead")),
        Some(_) => Ok(()),
    }
}

/// How this server names the person behind Sew member `member_id` (a DM
/// call push's title): the owner's name, or a Connect login's full name.
/// Empty when K2 doesn't know the member.
pub fn name_of_member(member_id: &str) -> String {
    let Ok(Some(m)) = hook::chat_member_by_id(member_id) else { return String::new() };
    if m.external_id == "k2:owner" {
        return crate::workspace_msg::resolve_owner_from();
    }
    let Some(uuid) = m.external_id.strip_prefix("k2:connect:") else { return String::new() };
    logins().ok().and_then(|all| all.into_iter().find(|l| l.id == uuid)).map(|l| l.display_name).unwrap_or_default()
}

/// The Sew member id of box person `ext` (from K2's identity map).
pub fn member_id(ext: &str) -> Result<String, Refusal> {
    match hook::chat_member(ext) {
        Ok(Some(m)) if m.state == members::ACTIVE => Ok(m.member_id),
        Ok(_) => Err(Refusal::new(503, "chat_member_pending", concat!(product_name!(), " is still setting up that person's chat; try again shortly")).retry(bff::RETRY_SECS)),
        Err(e) => Err(Refusal::new(503, "chat_unavailable", e)),
    }
}
