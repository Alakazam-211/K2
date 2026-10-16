//! `GET /cli/skin/me`: who a pass is (room-scope R6, RS9; the read-only
//! half of runtime S25's `me`, pulled into 0.46.1).
//!
//! Any live `k2skn_` pass (a guest's login, an App's own link pass, an
//! owner-minted platform pass) reads its own view; nothing else does
//! (403 `skin_token`, like `skin/agents`). The answer is the catalog's
//! `me.get` row (`AppMe`):
//!
//! ```text
//! { ok, kind: guest|app|token, username, app, principalId,
//!   rooms: [{room, handle, caps}], features: [...], catalogVersion }
//! ```
//!
//! - `kind`: `guest` for a login session; `app` for this App's pass on its
//!   own gateway link; `token` for a platform pass.
//! - `rooms`: only rooms whose agent still exists, each with the caps the
//!   pass holds there (a session's per-room policy; a platform or link
//!   pass's flat caps in each of its rooms). It never lists a room the
//!   pass doesn't hold, so it is not an oracle.
//! - `features`: this server's `/boot-status` keys
//!   (`boot_status::features()`), so an App can tell what the server has
//!   without the owner-only `/boot-status`.
//!
//! The write half (invites, profile) is S25, 0.46.2.

use k2_core::skin::SkinPass;
use serde_json::json;

use crate::cli_response::CliResponse;

/// The route.
pub const ROUTE: &str = "/cli/skin/me";

/// `guest`, `app` or `token` for `pass` (`on_link` = the request came in on
/// an App's gateway link).
pub fn kind_of(pass: &SkinPass, on_link: bool) -> &'static str {
    if pass.session {
        "guest"
    } else if on_link {
        "app"
    } else {
        "token"
    }
}

/// The view (pure over the pass; the handles come from the live rooms).
pub fn view(pass: &SkinPass, on_link: bool) -> serde_json::Value {
    let room_ids: Vec<String> = if pass.session { pass.room_policy.keys().cloned().collect() } else { pass.rooms.clone() };
    let live = k2_core::skin::live_agents(&room_ids);
    let rooms: Vec<serde_json::Value> = live
        .iter()
        .map(|a| {
            let caps: Vec<String> = if pass.session {
                pass.room_policy.get(&a.project_id).cloned().unwrap_or_default()
            } else {
                pass.caps.clone()
            };
            json!({ "room": a.project_id, "handle": a.handle, "caps": caps })
        })
        .collect();
    json!({
        "ok": true,
        "kind": kind_of(pass, on_link),
        "username": pass.username,
        "app": pass.app,
        "principalId": pass.principal_id,
        "rooms": rooms,
        "features": crate::boot_status::features(),
        "catalogVersion": k2_core::contract::catalog().catalog_version,
    })
}

/// `GET /cli/skin/me` for a resolved pass.
pub fn handle(pass: &SkinPass, on_link: bool) -> CliResponse {
    CliResponse::ok_json(view(pass, on_link).to_string())
}

/// A credential that isn't a skin pass.
pub fn forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": "skin_token", "hint": "GET /cli/skin/me is for a live app or guest pass (k2skn_)" },
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(session: bool) -> SkinPass {
        SkinPass {
            id: "t1".into(),
            principal_id: session.then(|| "p1".to_string()),
            username: "dana".into(),
            caps: vec!["thread:read".into()],
            rooms: vec![],
            session,
            room_policy: Default::default(),
            app: if session { "host".into() } else { String::new() },
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(&pass(true), false), "guest");
        assert_eq!(kind_of(&pass(true), true), "guest", "a guest session stays a guest on a link");
        assert_eq!(kind_of(&pass(false), true), "app");
        assert_eq!(kind_of(&pass(false), false), "token");
    }

    #[test]
    fn the_route_is_an_app_row_on_both_doors() {
        let v = k2_core::contract::catalog().verb("me.get").expect("me.get row");
        assert_eq!(v.bindings.http.as_ref().map(|h| h.route.as_str()), Some(ROUTE));
        assert!(k2_core::app_surface::link_allows_http("GET", ROUTE));
        assert!(k2_core::app_surface::helper_allows_http("GET", ROUTE));
    }
}
