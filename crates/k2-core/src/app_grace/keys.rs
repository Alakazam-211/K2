//! The App-scoped key (vs-live 2 §6, S45; Rosson Q3: "yes; create too,
//! only for its own App, never host").
//!
//! A `k2sk_` row in `api_keys` with `app_service_id` set
//! ([`crate::api_keys::create_app_key`]). It opens only
//! `/v1/apps/<that service id>/…`, and only what its scopes name:
//!
//! | scope   | what the App may do                                                   |
//! |---------|-----------------------------------------------------------------------|
//! | `users` | list, create, disable/enable, remove, unlock and reset-link **its own** App users |
//! | `rooms` | set the Thread rooms of its own App users, within the key's ceiling  |
//! | `fetch` | call an outside API through K2 with a secret granted to the service, by name |
//! | `push`  | send notifications to its own guests' phones (prd-custom-app-push-v1, 0.46.4) |
//! | `email` | send email from the App's own sender (prd-email-notifications-core-v1, 0.46.5) |
//!
//! `spaces` (Space grants) is named so a mint can say "later": it arrives
//! with Sew in K2. The ceiling (`rooms` workspace ids, `roles` role ids) is
//! the most a key can hand out: a create or a rooms change outside it is
//! refused. The key never names another App or `host`, never touches roles,
//! templates or the box login, and is minted only with a fresh owner
//! confirm (every caller, owner token included: agents can read the token,
//! not pass the confirm).

use serde::{Deserialize, Serialize};

pub const SCOPE_USERS: &str = "users";
pub const SCOPE_ROOMS: &str = "rooms";
pub const SCOPE_FETCH: &str = "fetch";
/// Send notifications to the App's own guests (`/v1/apps/<id>/push/*`).
pub const SCOPE_PUSH: &str = "push";
/// Send email from the App's own sender (`/v1/apps/<id>/email/*`,
/// prd-email-notifications-core-v1, 0.46.5).
pub const SCOPE_EMAIL: &str = "email";
/// Scopes a key can hold today.
pub const SCOPES: &[&str] = &[SCOPE_USERS, SCOPE_ROOMS, SCOPE_FETCH, SCOPE_PUSH, SCOPE_EMAIL];
/// Named for later: refused at mint with a pointer.
pub const LATER_SCOPES: &[&str] = &["spaces"];
/// Rooms one key may name.
pub const MAX_ROOMS: usize = 200;
/// Roles one key may name.
pub const MAX_ROLES: usize = 20;

/// What one App key may do (`api_keys.app_key`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppKeyGrant {
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Workspace ids the key may put an App user in.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Role ids (this App's or box-wide) the key may give.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The server-scope vault secret holding the key, when K2 delivered it
    /// to the service (`$K2_SECRETS_DIR/K2_APP_KEY`). Revoking the key
    /// removes that grant and secret too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_secret: Option<String>,
    /// The agent workspace that minted this key itself (switch 2 + "Let
    /// this agent manage its Apps' users"). Its `users` / `rooms` scopes
    /// work only while that switch stays on (review 0.46.4); `None` for a
    /// key the owner minted or approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minted_by_agent: Option<String>,
}

impl AppKeyGrant {
    /// Parse the stored JSON. Anything unreadable is the empty grant
    /// (nothing allowed): fail closed.
    pub fn from_stored(raw: Option<&str>) -> Self {
        raw.and_then(|r| serde_json::from_str::<AppKeyGrant>(r).ok())
            .map(|mut g| {
                g.scopes.retain(|s| SCOPES.contains(&s.as_str()));
                g
            })
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn has(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Every room in `rooms` is inside the ceiling (an empty list is
    /// allowed: it takes rooms away).
    pub fn allows_rooms(&self, rooms: &[String]) -> bool {
        rooms.iter().all(|r| self.rooms.contains(r))
    }

    /// The role id for `token` when the ceiling allows it on `app`: one the
    /// key names, or a role of that App no wider than one it names.
    pub fn allowed_role(&self, app: &str, token: &str) -> Option<String> {
        let want = crate::skin::role_for_app(app, token.trim()).ok().flatten()?;
        if !want.room_policy.keys().all(|room| self.rooms.contains(room)) {
            return None;
        }
        let ok = self.roles.iter().any(|m| match crate::skin::role_for_app(app, m).ok().flatten() {
            Some(top) => top.id == want.id || crate::skin::policy_within(&want.room_policy, &top.room_policy),
            None => false,
        });
        ok.then_some(want.id)
    }

    /// Is `self` no wider than `other` (a narrowing)?
    pub fn within(&self, other: &AppKeyGrant) -> bool {
        self.scopes.iter().all(|s| other.has(s))
            && self.rooms.iter().all(|r| other.rooms.contains(r))
            && self.roles.iter().all(|r| other.roles.contains(r))
    }
}

/// Normalize scope names (case, duplicates). Unknown names and the
/// later-scopes are refused with a teaching message.
pub fn parse_scopes(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for s in raw.iter().flat_map(|s| s.split(',')) {
        let s = s.trim().to_ascii_lowercase();
        if s.is_empty() {
            continue;
        }
        if LATER_SCOPES.contains(&s.as_str()) {
            return Err(format!("scope {s:?} isn't available yet (Space grants arrive with Chat / Voice Hosting in K2)"));
        }
        if !SCOPES.contains(&s.as_str()) {
            return Err(format!("unknown scope {s:?}; App keys take {}", SCOPES.join(", ")));
        }
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.is_empty() {
        return Err(format!("name at least one scope ({})", SCOPES.join(", ")));
    }
    Ok(out)
}

/// Build a grant for App `app` (a published service id) from request
/// tokens: rooms are workspace handles/ids/paths, roles are names/ids
/// usable on that App. Unknown rooms or roles are refused.
pub fn grant_for_app(app: &str, scopes: &[String], rooms: &[String], roles: &[String]) -> Result<AppKeyGrant, String> {
    let app = app.trim();
    if app.is_empty() || app == crate::skin::HOST_APP_ID {
        return Err("an App key is bound to one published App, never the box login".into());
    }
    let scopes = parse_scopes(scopes)?;
    let mut room_ids: Vec<String> = Vec::new();
    for t in rooms.iter().flat_map(|r| r.split(',')).map(str::trim).filter(|t| !t.is_empty()) {
        let id = crate::skin::resolve_room_tokens(&[t.to_string()])
            .map_err(|e| format!("room {t:?}: {e}"))?
            .into_iter()
            .next()
            .ok_or_else(|| format!("room {t:?}: unknown workspace"))?;
        if !room_ids.contains(&id) {
            room_ids.push(id);
        }
    }
    if room_ids.len() > MAX_ROOMS {
        return Err(format!("at most {MAX_ROOMS} rooms per key"));
    }
    let mut role_ids: Vec<String> = Vec::new();
    for t in roles.iter().flat_map(|r| r.split(',')).map(str::trim).filter(|t| !t.is_empty()) {
        let role = crate::skin::role_for_app(app, t)
            .map_err(|e| format!("role lookup: {e}"))?
            .ok_or_else(|| format!("no role {t:?} usable on this App"))?;
        if !role.room_policy.keys().all(|r| room_ids.contains(r)) {
            return Err(format!("role {t:?} reaches rooms outside the key's rooms; add them with --rooms"));
        }
        if !role_ids.contains(&role.id) {
            role_ids.push(role.id);
        }
    }
    if role_ids.len() > MAX_ROLES {
        return Err(format!("at most {MAX_ROLES} roles per key"));
    }
    let needs_ceiling = scopes.iter().any(|s| s == SCOPE_ROOMS);
    if needs_ceiling && room_ids.is_empty() {
        return Err("the rooms scope needs a ceiling: name the rooms this key may hand out (--rooms a,b)".into());
    }
    Ok(AppKeyGrant { scopes, rooms: room_ids, roles: role_ids, vault_secret: None, minted_by_agent: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_parse_and_refuse() {
        assert_eq!(parse_scopes(&["Users,rooms".into(), "users".into()]).unwrap(), vec!["users", "rooms"]);
        assert!(parse_scopes(&["spaces".into()]).unwrap_err().contains("Chat / Voice Hosting"));
        assert!(parse_scopes(&["admin".into()]).unwrap_err().contains("unknown scope"));
        assert!(parse_scopes(&[]).is_err(), "a key with no scope is refused");
    }

    #[test]
    fn stored_grant_fails_closed() {
        assert_eq!(AppKeyGrant::from_stored(None), AppKeyGrant::default());
        assert_eq!(AppKeyGrant::from_stored(Some("not json")), AppKeyGrant::default());
        let g = AppKeyGrant::from_stored(Some(r#"{"scopes":["users","owner"],"rooms":["w1"],"roles":[]}"#));
        assert_eq!(g.scopes, vec!["users"], "an unknown scope in storage is dropped, never honored");
        assert!(g.allows_rooms(&["w1".into()]) && g.allows_rooms(&[]));
        assert!(!g.allows_rooms(&["w2".into()]));
        let narrower = AppKeyGrant { scopes: vec!["users".into()], ..Default::default() };
        assert!(narrower.within(&g) && !g.within(&narrower));
    }

    #[test]
    fn grant_never_binds_the_box_login() {
        assert!(grant_for_app("host", &["users".into()], &[], &[]).is_err());
        assert!(grant_for_app(" ", &["users".into()], &[], &[]).is_err());
    }
}
