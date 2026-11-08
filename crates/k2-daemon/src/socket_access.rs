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
//!
//! For an App pass, [`room_access`]: the pass as it resolves now. Pass gone:
//! 4003. Pass fine, this socket's room or capability gone:
//! [`crate::presence::access_lost_close_message`] (4013). A resolve holds the
//! skin mutex for 100 to 400 µs, so it never runs per frame:
//! - **Before a frame leaves:** [`SkinGate`]. One atomic load while access is
//!   quiet. After any access write, the next frame resolves first, so a revoke, a
//!   role edit or a deleted user stops the very next frame.
//! - **When the hub says so** ([`crate::access_live`]): at once, idle or not, with
//!   the answer the hub already read.
//! - **On the [`TICK`], idle or not:** always resolves. That's the bound (5 s) with
//!   the hub off, and on what no write shows: a moved pin, a pass's expiry and the
//!   other daemon's write during a handoff.

use std::sync::Arc;
use std::time::Duration;

use k2_core::skin::SkinPass;
use tokio_tungstenite::tungstenite::Message;

use crate::apps::gateway::LinkCtx;

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

/// The incoming-frame check for a socket that acts on client frames
/// (security review L7): grid, bytes, sessions, session events, awareness,
/// ops and the simulator stream. For a Connect login it is a [`LoginCheck`]
/// at the socket's floor (Member: [`crate::routes::http::token_still_valid`]),
/// asked before a Text or Binary frame is acted on, so a login moved to Voice
/// & Chat only can't drive a socket it opened as a Member even for the
/// seconds before the tick. Any other credential (the owner token, a stream
/// token, a passport) keeps its socket's own tick rule: `frame_ok` is true.
#[derive(Debug)]
pub struct IncomingCheck(Option<LoginCheck>);

impl IncomingCheck {
    pub fn new(token: &str, owner: &str) -> Self {
        let login = !token.is_empty()
            && !crate::routes::http::ct_eq_token(token, owner)
            && k2_core::connect_users::looks_like_session_token(token);
        Self(login.then(|| LoginCheck::new(token, owner)))
    }

    /// May this frame from the client be acted on? Control frames (ping,
    /// pong, close) always may.
    pub fn frame_ok<E>(&mut self, msg: &Option<Result<Message, E>>) -> bool {
        let acts = matches!(msg, Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))));
        match (&mut self.0, acts) {
            (Some(check), true) => check.frame_ok(),
            _ => true,
        }
    }
}

#[cfg(test)]
mod incoming_tests {
    use super::*;

    type Frame = Option<Result<Message, std::io::Error>>;

    /// Security review L7: a login's frame is refused the moment the login
    /// is moved to Voice & Chat only; the owner token, a passport and a
    /// stream token are left to their socket's own rule; control frames
    /// always pass.
    #[test]
    fn a_downgraded_login_s_frame_is_refused_at_once() {
        crate::test_support::with_temp_home(|| {
            k2_core::connect_users::add_user("inmem", "password123").expect("add");
            let tok = k2_core::connect_users::create_session("inmem");
            let mut check = IncomingCheck::new(&tok, "owner-tok");
            let text: Frame = Some(Ok(Message::Text("{}".into())));
            let ping: Frame = Some(Ok(Message::Ping(Vec::new().into())));
            assert!(check.frame_ok(&text), "a Member drives it");
            k2_core::connect_users::set_role("inmem", k2_core::connect_users::Role::VoiceChat).expect("down");
            assert!(!check.frame_ok(&text), "the very next client frame is refused");
            assert!(check.frame_ok(&ping), "control frames pass");
            let mut owner = IncomingCheck::new("owner-tok", "owner-tok");
            assert!(owner.frame_ok(&text));
            let mut passport = IncomingCheck::new("sid-1.secret", "owner-tok");
            assert!(passport.frame_ok(&text), "a passport keeps its socket's own rule");
        });
    }

    /// L7: every client-driven socket asks [`IncomingCheck`] before acting
    /// on a client frame (a new socket family must, too).
    #[test]
    fn every_client_driven_socket_checks_incoming_frames() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for f in [
            "sessions_grid_ws.rs",
            "sessions_bytes_ws.rs",
            "sessions_ws.rs",
            "session_events_ws.rs",
            "awareness_ws.rs",
            "ops_stream_ws.rs",
            "sim_stream_ws.rs",
        ] {
            let s = std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
            assert!(s.contains("crate::socket_access::IncomingCheck::new("), "{f} builds the check");
            let at = s.find("msg = read.next() => {").unwrap_or_else(|| panic!("{f}: its read branch"));
            let next = &s[at..at + 600.min(s.len() - at)];
            assert!(next.contains("if !incoming.frame_ok(&msg)"), "{f}: the check runs first in the read branch");
        }
    }
}

/// Has any access been written since this socket last checked its pass?
#[derive(Debug, Default)]
pub struct SkinGate {
    settled_at: Option<u64>,
}

impl SkinGate {
    /// Must the pass be resolved before the next frame?
    pub fn due(&self) -> bool {
        self.settled_at != Some(k2_core::skin::write_generation())
    }

    /// Read this before resolving, and hand it to [`Self::settled`] after: a
    /// write that lands in between moves the generation, so the next frame asks
    /// again.
    pub fn generation(&self) -> u64 {
        k2_core::skin::write_generation()
    }

    pub fn settled(&mut self, generation: u64) {
        self.settled_at = Some(generation);
    }
}

/// A pass as it resolves now, on the link its socket arrived on. `Ok(None)`:
/// gone (revoked, expired, its user deleted). On a suspended App's link too:
/// closing a link leaves its open connections running, and a guest's session
/// doesn't look at suspend, so the question is asked here. `Err`: the database
/// couldn't say, which is no answer about the pass.
pub(crate) fn live_pass(link: Option<&LinkCtx>, raw: &str) -> Result<Option<SkinPass>, String> {
    if link.is_some_and(|l| crate::apps::suspend::is_suspended(&l.app_id)) {
        return Ok(None);
    }
    crate::apps::gateway::try_resolve_pass_on(link, raw)
}

/// [`live_pass`], or the close a socket owes: 4003 for a dead pass, a plain close
/// for a read error (the client reconnects and asks again, and nobody is told to
/// sign in).
pub(crate) fn live_pass_or_close(link: Option<&LinkCtx>, raw: &str) -> Result<SkinPass, Message> {
    match live_pass(link, raw) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(crate::presence::session_revoked_close_message()),
        Err(e) => {
            k2_core::log_debug!("[socket_access] a pass couldn't be read: {e}");
            Err(Message::Close(None))
        }
    }
}

/// What an open App socket may do now.
#[derive(Debug)]
pub enum RoomAccess {
    Still(SkinPass),
    /// The pass lives, but the room or every capability this socket needs is
    /// gone.
    Lost,
    PassGone,
    /// The check itself failed: no frame, and a plain close (the client
    /// reconnects and the door decides).
    Failed,
}

impl RoomAccess {
    /// How the socket ends. Only for the ending kinds; a pass that's still
    /// fine is never closed by this.
    pub fn close_message(&self) -> Message {
        match self {
            RoomAccess::PassGone => crate::presence::session_revoked_close_message(),
            RoomAccess::Lost => crate::presence::access_lost_close_message(),
            RoomAccess::Still(_) | RoomAccess::Failed => Message::Close(None),
        }
    }
}

/// Does `raw` still hold `room` with at least one of `caps`? Reads skin.db: a
/// blocking thread ([`room_access_now`]).
pub(crate) fn room_access(link: Option<&LinkCtx>, raw: &str, room: &str, caps: &[&str]) -> RoomAccess {
    match live_pass(link, raw) {
        Ok(None) => RoomAccess::PassGone,
        Ok(Some(p)) if p.has_any_cap_in_room(room, caps) => match room_exists(room) {
            Ok(true) => RoomAccess::Still(p),
            // A role can go on naming a room whose agent is gone: not held any more.
            Ok(false) => RoomAccess::Lost,
            Err(e) => {
                k2_core::log_debug!("[socket_access] a room couldn't be read: {e}");
                RoomAccess::Failed
            }
        },
        Ok(Some(_)) => RoomAccess::Lost,
        Err(e) => {
            k2_core::log_debug!("[socket_access] a pass couldn't be read: {e}");
            RoomAccess::Failed
        }
    }
}

/// Is this room's agent still here? Files and activity frames match a room by its
/// folder, and the same folder can come back under a new id (security review A1).
fn room_exists(room: &str) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", [room], |r| r.get(0)).map_err(|e| format!("room lookup: {e}"))
}

/// [`room_access`] off the async worker. A long-lived socket captures its link
/// once: a blocking thread can't see the task-local.
pub(crate) async fn room_access_now(link: Option<Arc<LinkCtx>>, raw: String, room: String, caps: &'static [&'static str]) -> RoomAccess {
    tokio::task::spawn_blocking(move || room_access(link.as_deref(), &raw, &room, caps)).await.unwrap_or(RoomAccess::Failed)
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
    fn the_skin_gate_moves_on_a_write_and_never_on_a_read() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, format!("g{}", &id[..8]), format!("/tmp/{id}")]).expect("project");
        }
        let (user, role) = (format!("gu{}", &id[..8]), format!("gr{}", &id[..8]));
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert(id.clone(), vec![k2_core::skin::CAP_THREAD_READ.to_string()]);
        k2_core::skin::create_role_for_app(&role, &policy, None).expect("role");
        k2_core::skin::add_principal(&user).expect("person");
        k2_core::skin::assign_role(&user, &role).expect("assign");
        let (meta, raw) = k2_core::skin::create_session_token(&user).expect("session");

        let mut gate = SkinGate::default();
        assert!(gate.due(), "the first frame always asks");
        gate.settled(gate.generation());
        assert!(!gate.due(), "settled");
        assert!(k2_core::skin::resolve_skin_token(&raw).is_some(), "live");
        assert!(k2_core::skin::resolve_skin_token("k2skn_not_a_real_pass").is_none());
        assert!(!gate.due(), "a read moves nothing, or every socket would resolve for every other");
        assert!(k2_core::skin::revoke_token(&meta.id).expect("revoke"));
        assert!(gate.due(), "a revoke reaches the very next frame");
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
