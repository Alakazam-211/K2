//! An open socket follows its caller's access (`prd-live-access-changes-v1`
//! S1a). A check at the upgrade only says who may open a socket; these keep
//! asking while it stays open.
//!
//! Two questions, for a Connect login on a long-lived socket:
//! - **Before a frame leaves, or a client frame is acted on:**
//!   [`LoginCheck::frame_ok`]. One atomic load while nothing moved, so a busy
//!   socket pays nothing; the session store is read only when a session or a
//!   login was written since.
//! - **On a tick, idle or not:** [`LoginCheck::valid_now`], every [`TICK`] (the
//!   same 5 s as the other people sockets). The tick is the backstop for what
//!   the generation can't see: a session's expiry, and a write made by the other
//!   daemon during a handoff.
//!
//! A dead credential closes with [`crate::presence::session_revoked_close_message`]
//! (4003). A client must not rely on seeing the code: it reconnects and the
//! door refuses it.

use std::time::Duration;

/// How often an open socket asks again, whether or not it is sending.
pub const TICK: Duration = Duration::from_secs(5);

/// A [`TICK`] timer for one socket's loop. The first tick is one period away,
/// and a late tick never bursts.
pub fn tick() -> tokio::time::Interval {
    let mut t = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    t
}

/// The owner token or a Connect login behind one open socket.
#[derive(Debug, Clone)]
pub struct LoginCheck {
    token: String,
    owner: String,
    /// The store generation the token last validated at. `None` until the
    /// first check, so a revoke between the door and that check isn't missed.
    seen: Option<u64>,
}

impl LoginCheck {
    pub fn new(token: &str, owner: &str) -> Self {
        Self { token: token.to_string(), owner: owner.to_string(), seen: None }
    }

    /// For a socket the dispatcher admitted with `token_ok` on this query
    /// (`effective_auth_query` has folded a Bearer or cookie credential in).
    pub(crate) fn from_query(query: &str, owner: &str) -> Self {
        Self::new(crate::routes::http::extract_token(query).unwrap_or_default(), owner)
    }

    /// May a frame go out (or a client's frame be acted on) now?
    pub fn frame_ok(&mut self) -> bool {
        // Read before validating: a write that lands in between moves the
        // generation again, so the next frame asks again.
        let now = k2_core::connect_users::auth_generation();
        if self.seen == Some(now) {
            return true;
        }
        let ok = self.valid_now();
        if ok {
            self.seen = Some(now);
        }
        ok
    }

    /// Ask the store now. Revoked, expired, a removed or disabled login, or a
    /// bumped epoch (password or role change): `false`. The owner token is never
    /// revoked and reads no store.
    pub fn valid_now(&self) -> bool {
        crate::routes::http::token_still_valid(&self.token, &self.owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;

    #[test]
    fn a_frame_check_follows_a_revoke_at_once_and_reads_nothing_while_quiet() {
        with_temp_home(|| {
            let owner = "owner-token-socket-access";
            k2_core::connect_users::add_user("sock_user", "password123").expect("add_user");
            k2_core::connect_users::add_user("sock_other", "password123").expect("add_user");
            let token = k2_core::connect_users::create_session("sock_user");
            let other = k2_core::connect_users::create_session("sock_other");
            let mut check = LoginCheck::new(&token, owner);
            let mut control = LoginCheck::new(&other, owner);
            assert!(check.frame_ok(), "a live session passes");
            assert!(control.frame_ok(), "a live session passes");
            let quiet = k2_core::connect_users::auth_generation();
            assert!(check.frame_ok(), "nothing moved");
            assert_eq!(k2_core::connect_users::auth_generation(), quiet, "a check writes nothing");

            k2_core::connect_users::revoke_user_sessions("sock_user");
            assert_ne!(k2_core::connect_users::auth_generation(), quiet, "a revoke moves the generation");
            assert!(!check.frame_ok(), "the very next frame is refused, no tick needed");
            assert!(!check.valid_now());
            assert!(control.frame_ok(), "another login's session is unaffected");
            assert!(control.valid_now());

            let mut boss = LoginCheck::new(owner, owner);
            assert!(boss.frame_ok() && boss.valid_now(), "the owner token never lapses");
        });
    }

    #[test]
    fn removing_a_login_stops_its_next_frame() {
        with_temp_home(|| {
            let owner = "owner-token-socket-access";
            k2_core::connect_users::add_user("sock_gone", "password123").expect("add_user");
            let token = k2_core::connect_users::create_session("sock_gone");
            let mut check = LoginCheck::new(&token, owner);
            assert!(check.frame_ok());
            k2_core::connect_users::remove_user("sock_gone").expect("remove");
            assert!(!check.frame_ok(), "a removed login's next frame is refused");
        });
    }
}
