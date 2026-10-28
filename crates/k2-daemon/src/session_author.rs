//! The session author: who a Thread, card, feedback or folder-trust
//! request came from, as the string those handlers take.
//!
//! - The owner token is [`OWNER_TOKEN`] (`owner`). Since 0.46.6 no Connect
//!   login can have that name (`k2_core::connect_users::RESERVED_USERNAMES`),
//!   so `owner` always means the owner token, never a login.
//! - A Connect session is its username.
//! - A session whose login has a reserved name (one made before 0.46.6) is
//!   [`RESERVED_LOGIN`]: a form no username can take, which matches no
//!   login, no role and no person, so it never falls back to the owner.
//! - Anything else (an agent passport, an App pass, no token) is the
//!   route's own fallback, as before.

use k2_core::connect_users;

/// The owner token's session author. No Connect login can have this name.
pub const OWNER_TOKEN: &str = "owner";

/// A session of a login with a reserved name. `!` is never in a username
/// (`^[a-z0-9_-]{2,}$`), so this can't name a login, a role or a person.
pub const RESERVED_LOGIN: &str = "!reserved-login";

/// Is `author` the owner token's? True only for [`OWNER_TOKEN`], which no
/// login can produce (see the module docs).
pub fn is_owner_token(author: &str) -> bool {
    author == OWNER_TOKEN
}

/// The session author for a request: the owner token, a live Connect
/// session's username, [`RESERVED_LOGIN`] for a reserved-name login's
/// session, else `fallback`.
pub fn for_request(query: &str, owner_token: &str, fallback: &str) -> String {
    if crate::routes::http::token_is_owner(query, owner_token) {
        return OWNER_TOKEN.to_string();
    }
    for_token(crate::routes::http::extract_token(query), fallback)
}

/// [`for_request`] once the owner token is ruled out.
pub fn for_token(token: Option<&str>, fallback: &str) -> String {
    let Some(t) = token.filter(|t| !t.is_empty()) else {
        return fallback.to_string();
    };
    match connect_users::validate_session(t) {
        // `validate_session` never answers a reserved name; the check is a
        // second belt for the owner's own author string.
        Some(u) if !connect_users::is_reserved_username(&u) => u,
        Some(_) => RESERVED_LOGIN.to_string(),
        None if connect_users::reserved_login_session(t).is_some() => RESERVED_LOGIN.to_string(),
        None => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Connect login named `owner` (made before 0.46.6) is never the
    /// owner token's author, even on a route whose fallback is `owner`;
    /// the real owner token still is, and other logins are themselves.
    #[test]
    fn a_login_named_owner_never_reads_as_the_owner_token() {
        let _home = crate::test_support::TempHome::new();
        connect_users::test_add_legacy_reserved_user("owner", "password123").expect("legacy owner login");
        connect_users::add_user("gina", "password123").expect("gina");
        let named_owner = connect_users::create_session("owner");
        let gina = connect_users::create_session("gina");
        let owner_token = "k2test-owner-token-0123456789";
        for fallback in [OWNER_TOKEN, "agent"] {
            let a = for_token(Some(&named_owner), fallback);
            assert_eq!(a, RESERVED_LOGIN, "fallback {fallback}");
            assert!(!is_owner_token(&a));
            assert_eq!(connect_users::role_for_user(&a), None, "names no login");
            assert_eq!(for_token(Some(&gina), fallback), "gina");
            assert_eq!(for_token(None, fallback), fallback, "no token: the route's own fallback");
            assert_eq!(for_token(Some("not-a-session"), fallback), fallback);
        }
        let q = format!("token={owner_token}");
        assert!(is_owner_token(&for_request(&q, owner_token, "agent")), "the real owner token");
        assert_eq!(for_request(&format!("token={named_owner}"), owner_token, OWNER_TOKEN), RESERVED_LOGIN);
        assert_eq!(for_request(&format!("token={gina}"), owner_token, OWNER_TOKEN), "gina");
    }
}
