//! A Connect login hears `access_changed` on `/cli/sessions/events` when its own
//! access changes (K2 Companion: a person's phone holds that socket, never
//! `me.events`). Only that login's sockets hear it: not another login's, not
//! the owner token's, not an App pass's `me.events`. An agent passport, an API
//! key and an App pass can't open the socket at all, as before.
//!
//! Real daemon, real sockets, headless.

use crate::support::app_fixture;
use crate::support::live_sockets;

use app_fixture::*;
use live_sockets::*;
use serde_json::json;

/// The whole frame, the same one `me.events` sends an App pass.
const FRAME: &str = r#"{"kind":"access_changed"}"#;
/// The watch looks every 200 ms; room for a busy node.
const HEARS_WITHIN_MS: u64 = 3_000;
/// How long a socket that must hear nothing is watched.
const QUIET_MS: u64 = 1_500;

async fn connect_login(port: u16, username: &str) -> String {
    owner_ok(port, "/cli/users/add", json!({ "username": username, "password": LOGIN_PW, "mustChangePassword": false })).await;
    let b = json!({ "username": username, "password": LOGIN_PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(b))).await.expect("join");
    assert_eq!(r.status, 200, "login: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// An app-level `/cli/sessions/events` socket, past its hello.
async fn events(port: u16, token: &str) -> Sock {
    let mut s = open(port, &format!("/cli/sessions/events?token={}", enc(token))).await;
    match next_frame(&mut s, 5_000).await {
        Ok(Some(f)) => assert!(f.contains(r#""kind":"hello""#), "the first frame is the hello: {f}"),
        other => panic!("no hello: {other:?}"),
    }
    s
}

/// The `access_changed` frames that arrive within `ms`. Every other kind (the
/// presence roster, say) is skipped. The socket must stay open.
async fn access_frames(s: &mut Sock, ms: u64, who: &str) -> Vec<String> {
    let seen = watch(s, ms).await;
    assert!(!seen.ended, "{who}: the socket ended: {seen:?}");
    seen.frames.into_iter().filter(|f| f.contains(r#""access_changed""#)).collect()
}

async fn hears_once(s: &mut Sock, who: &str) {
    assert_eq!(access_frames(s, HEARS_WITHIN_MS, who).await, vec![FRAME.to_string()], "{who}: one frame, and nothing in it");
}

async fn hears_none(s: &mut Sock, who: &str) {
    assert_eq!(access_frames(s, QUIET_MS, who).await, Vec::<String>::new(), "{who}: must hear nothing");
}

async fn set_can_clone(port: u16, username: &str, on: bool) {
    owner_ok(port, "/cli/users/set-can-clone", json!({ "username": username, "canClone": on })).await;
}

async fn whoami_can_clone(port: u16, token: &str) -> bool {
    let r = get(port, "/cli/auth/whoami", token, "x=1").await;
    assert_eq!(r.status, 200, "whoami: {}", r.body);
    r.json()["canClone"].as_bool().unwrap_or_else(|| panic!("no canClone: {}", r.body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_hears_only_its_own_access_change() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let gina = connect_login(port, "lagina").await;
    let bob = connect_login(port, "labob").await;
    let ws = seed_ws(&env, "laroom");
    let agent = passport(&ws);
    let (_, guest) = guest_in(port, &ws, "laguest", ALL_CAPS).await;

    // The door: no agent passport, API key or App pass holds this socket, so
    // none of them can hear a person's frame.
    for (who, tok) in [("agent passport", agent.as_str()), ("api key", "k2sk_notarealkey"), ("app pass", guest.as_str())] {
        let pq = format!("/cli/sessions/events?token={}", enc(tok));
        match tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}{pq}")).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403, "{who}"),
            other => panic!("{who}: {:?}", other.map(|_| "opened")),
        }
    }

    let mut g = events(port, &gina).await;
    let mut b = events(port, &bob).await;
    let mut o = events(port, OWNER).await;
    let mut me = open(port, &format!("/cli/skin/me/events?scope=me&token={}", enc(&guest))).await;
    // Opening and the roster frames are no access change.
    hears_none(&mut g, "gina at rest").await;
    hears_none(&mut b, "bob at rest").await;

    // The owner turns Gina's "Can clone" off: her socket hears it, and her
    // access read then shows it.
    assert!(whoami_can_clone(port, &gina).await);
    set_can_clone(port, "lagina", false).await;
    hears_once(&mut g, "gina").await;
    assert!(!whoami_can_clone(port, &gina).await, "the read after the frame shows the change");
    hears_none(&mut b, "bob, on gina's change").await;
    hears_none(&mut o, "the owner token, on gina's change").await;
    let seen = watch(&mut me, QUIET_MS).await;
    assert!(seen.frames.is_empty() && !seen.ended, "an App pass's me.events, on gina's change: {seen:?}");

    // The same value again writes the store but changes nothing.
    set_can_clone(port, "lagina", false).await;
    hears_none(&mut g, "gina, on a no-op").await;

    // Bob's own change reaches Bob only.
    set_can_clone(port, "labob", false).await;
    hears_once(&mut b, "bob").await;
    hears_none(&mut g, "gina, on bob's change").await;
    hears_none(&mut o, "the owner token, on bob's change").await;
}

// ── `/cli/voice/events`, a Voice & Chat only login's socket ───────────

/// The voice rooms preview for this test only (the socket is dark without it).
struct VoicePreview;

impl VoicePreview {
    fn on() -> Self {
        k2_core::voice_preview::set_enabled(true);
        VoicePreview
    }
}

impl Drop for VoicePreview {
    fn drop(&mut self) {
        k2_core::voice_preview::set_enabled(false);
    }
}

async fn voice_chat_login(port: u16, username: &str, agents: &[&str]) -> String {
    owner_ok(
        port,
        "/cli/users/add",
        json!({ "username": username, "password": LOGIN_PW, "mustChangePassword": false, "role": "voice_chat", "shareAgents": agents }),
    )
    .await;
    let b = json!({ "username": username, "password": LOGIN_PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(b))).await.expect("join");
    assert_eq!(r.status, 200, "login: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// A `/cli/voice/events` socket, past its hello.
async fn voice_events(port: u16, token: &str) -> Sock {
    let mut s = open(port, &format!("/cli/voice/events?token={}", enc(token))).await;
    match next_frame(&mut s, 5_000).await {
        Ok(Some(f)) => assert_eq!(f, r#"{"kind":"hello"}"#, "the first frame is the hello"),
        other => panic!("no hello: {other:?}"),
    }
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_chat_login_hears_its_share_change_on_the_voice_socket() {
    let env = setup(&[]);
    let _preview = VoicePreview::on();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "lavcagent");
    pin_chat(&ws);
    let vc = voice_chat_login(port, "lavcgina", &[ws.handle.as_str()]).await;
    let other = voice_chat_login(port, "lavcbob", &[]).await;
    let mut v = voice_events(port, &vc).await;
    let mut o = voice_events(port, &other).await;
    hears_none(&mut v, "gina at rest").await;

    // The owner revokes Gina's share: her voice socket hears it once, and her
    // shares read then shows it.
    let r = get(port, "/cli/shared", &vc, "x=1").await;
    assert_eq!(r.status, 200, "shared: {}", r.body);
    assert_eq!(r.json()["agents"].as_array().map(Vec::len), Some(1), "{}", r.body);
    owner_ok(port, "/cli/users/set-shares", json!({ "username": "lavcgina", "agents": [], "gardens": [] })).await;
    hears_once(&mut v, "gina's voice socket").await;
    let r = get(port, "/cli/shared", &vc, "x=1").await;
    assert_eq!(r.json()["agents"].as_array().map(Vec::len), Some(0), "the read after the frame shows the change: {}", r.body);
    hears_none(&mut o, "bob's voice socket, on gina's change").await;

    // The rooms frame reaches both, as before.
    k2_daemon::session_events::emit(k2_daemon::session_events::SessionEvent::VoiceRoomsChanged { rev: 9 }).expect("emit");
    for (who, s) in [("gina", &mut v), ("bob", &mut o)] {
        let seen = watch(s, QUIET_MS).await;
        assert!(!seen.ended, "{who}: {seen:?}");
        assert!(seen.frames.iter().any(|f| f == r#"{"kind":"voice_rooms_changed","rev":9}"#), "{who}: the rooms frame: {seen:?}");
        assert!(!seen.has("access_changed"), "{who}: no access frame rides with it: {seen:?}");
    }
}
