//! Voice & Chat only, the security review's fixes through the REAL HTTP
//! server (`test_harness::start`):
//!
//! - the every-route walk over the wire: every `ROUTES` cell the role isn't
//!   allowed, with the session as a Bearer, as `?token=` and as the
//!   `k2_session` cookie; path variants; and the doors outside `/cli`
//!   (`/events`, `/status`, the call lane and kit door, `/hook`, `/v1`,
//!   the transfer door, `/`, `/account`);
//! - a downgraded login's client frame closes its socket at once (4013) on
//!   the session events, ops and awareness sockets (L7);
//! - a fixed password, the demo-agent check, the set-shares audit actor and
//!   the Admin rule inside the write (M3, H1, L9).
//!
//! Isolation: a temp `$HOME` per test (one at a time), the process's test
//! DB. Fail loudly.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use k2_daemon::routes::route_policy::{self, Floor};
use k2_daemon::test_harness;
use serde_json::{json, Value as J};

const OWNER_TOKEN: &str = "owner-token-voice-chat-security-integration";
const PW: &str = "review-pass-Strong-2";

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Resp {
    status: u16,
    body: String,
}

fn raw(port: u16, method: &str, path: &str, headers: &[(&str, String)], body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let b = body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if body.is_some() {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    req.push_str(b);
    s.write_all(req.as_bytes()).expect("write");
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read {method} {path}: {e:?}"),
    }
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("{method} {path}: no status: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

fn post(port: u16, path: &str, token: &str, body: J) -> Resp {
    raw(port, "POST", &format!("{path}?token={token}"), &[], Some(&body.to_string()))
}

fn json_of(r: &Resp) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", r.body))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let tmp = std::env::temp_dir().join(format!("k2-vc-sec-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).expect("tmp");
    let _r = Restore(std::env::var_os("HOME"), tmp.clone());
    std::env::set_var("HOME", &tmp);
    f();
}

fn login(port: u16, user: &str) -> String {
    let r = raw(port, "POST", "/cli/auth/login", &[], Some(&json!({"username": user, "password": PW}).to_string()));
    assert_eq!(r.status, 200, "login {user}: {}", r.body);
    json_of(&r)["token"].as_str().expect("token").to_string()
}

fn voice_chat_login(port: u16, user: &str) -> String {
    let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": user, "password": PW, "mustChangePassword": false, "role": "voice_chat"}));
    assert_eq!(r.status, 200, "{}", r.body);
    login(port, user)
}

/// The three ways a login's session reaches the daemon.
fn forms(tok: &str, method: &str) -> Vec<(&'static str, String, Vec<(&'static str, String)>)> {
    let mut cookie = vec![("Cookie", format!("k2_session={tok}")), ("Sec-Fetch-Site", "same-origin".to_string())];
    if method == "POST" {
        cookie.push(("X-K2-Client", "web".to_string()));
    }
    vec![
        ("bearer", String::new(), vec![("Authorization", format!("Bearer {tok}"))]),
        ("query", format!("token={tok}"), vec![]),
        ("cookie", String::new(), cookie),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_and_door_refuses_a_voice_chat_login_over_http() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let tok = voice_chat_login(port, "walker");
        let mut checked = 0usize;
        for r in route_policy::ROUTES {
            for (m, floor) in [("GET", r.get), ("POST", r.post)] {
                let Some(floor) = floor else { continue };
                let allowed = route_policy::VOICE_CHAT_ALLOWLIST.contains(&(m, r.path))
                    || (floor == Floor::Public && route_policy::VOICE_CHAT_PUBLIC.contains(&r.path));
                if allowed {
                    // The allowlist is exercised by the role tests; calling
                    // it here would sign this session out (logout).
                    continue;
                }
                for (form, q, headers) in forms(&tok, m) {
                    let path = if q.is_empty() { r.path.to_string() } else { format!("{}?{q}", r.path) };
                    let body = (m == "POST").then_some("{}");
                    let resp = raw(port, m, &path, &headers, body);
                    checked += 1;
                    let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
                    let role_refused = resp.status == 403 && v["error"] == "role_required" && v["role"] == "voice_chat";
                    // A row its switch hides (permission cards, Garden
                    // access) answers as a path with no row: 404 for a
                    // login, or the 405 guard for a POST (never a handler).
                    let hidden = route_policy::live_row(r.path).is_none()
                        && ((resp.status == 404 && v["error"] == "route_unclassified") || resp.status == 405);
                    assert!(role_refused || hidden, "{form} {m} {}: {} {}", r.path, resp.status, resp.body);
                }
            }
        }
        assert!(checked > 5000, "walked only {checked} requests");

        // Path variants never reach a handler for this login.
        for p in [
            "/cli/projects/list/",
            "/cli//projects/list",
            "/cli/projects/list%2F",
            "/CLI/projects/list",
            "/cli/Projects/list",
            "/cli/projects/./list",
            "/cli/projects/list;x",
            "/cli/terminal/spawn/",
        ] {
            for (form, q, headers) in forms(&tok, "GET") {
                let path = if q.is_empty() { p.to_string() } else { format!("{p}?{q}") };
                let resp = raw(port, "GET", &path, &headers, None);
                assert!(
                    matches!(resp.status, 403 | 404 | 405),
                    "{form} GET {p}: {} {}",
                    resp.status,
                    resp.body
                );
                assert!(!resp.body.contains("\"workspaces\":[{"), "{form} {p}: {}", resp.body);
            }
        }

        // Doors outside /cli.
        let bearer = vec![("Authorization", format!("Bearer {tok}"))];
        for (m, p, want) in [
            ("GET", "/events", &[403][..]),
            ("GET", "/status", &[403][..]),
            ("GET", "/_k2/voice/sew/api/v1/members?ids=1", &[401, 403, 404][..]),
            ("POST", "/_k2/voice/sew/api/v1/channels/1/recording/start", &[401, 403, 404][..]),
            ("GET", "/_k2/voice/sew/api/v1/gateway", &[401, 403, 404][..]),
            ("POST", "/hook/event", &[401, 403, 404][..]),
            ("POST", "/v1/agents/hire", &[401, 403, 404][..]),
            ("GET", "/v1/sandboxes", &[401, 403, 404, 405][..]),
            ("GET", "/_k2/transfers/downloads/abc", &[401, 403, 404][..]),
            ("POST", "/_k2/transfers/uploads", &[401, 403, 404, 405][..]),
        ] {
            let body = (m == "POST").then_some("{}");
            let resp = raw(port, m, p, &bearer, body);
            assert!(want.contains(&resp.status), "{m} {p}: {} {}", resp.status, resp.body);
            if p == "/events" || p == "/status" {
                assert_eq!(json_of(&resp)["error"], "role_required", "{p}: {}", resp.body);
            }
        }
        // The account portal shows the person's own page and nobody else.
        for p in ["/", "/account"] {
            let resp = raw(port, "GET", p, &bearer, None);
            assert!(matches!(resp.status, 200 | 302 | 404), "{p}: {}", resp.status);
            assert!(!resp.body.contains(OWNER_TOKEN));
        }
    });
}

/// L7: the very next client frame of a login moved to Voice & Chat only
/// closes the socket with 4013 (before the 5 s tick).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_downgraded_login_s_client_frame_closes_its_socket_at_once() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "framer", "password": PW, "mustChangePassword": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        let tok = login(port, "framer");
        for path in ["/cli/sessions/events", "/cli/ops/stream", "/cli/awareness/subscribe"] {
            let r = post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "framer", "role": "member"}));
            assert_eq!(r.status, 200, "{}", r.body);
            futures_block(async {
                let url = format!("ws://127.0.0.1:{port}{path}?token={tok}");
                let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap_or_else(|e| panic!("{path}: a Member opens it: {e}"));
                let r = post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "framer", "role": "voice_chat"}));
                assert_eq!(r.status, 200, "{}", r.body);
                let sent = Instant::now();
                ws.send(tokio_tungstenite::tungstenite::Message::Text("{}".into())).await.unwrap_or_else(|e| panic!("{path}: send: {e}"));
                let code = tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        match ws.next().await {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(f))) => return f.map(|f| u16::from(f.code)),
                            Some(Ok(_)) => continue,
                            _ => return None,
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("{path}: not closed within 3 s of the frame"));
                assert_eq!(code, Some(4013), "{path}: closed by the frame check (access_lost), {:?} after", sent.elapsed());
            });
        }
    });
}

/// M3, H1, L9 over the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_password_demo_check_and_the_set_shares_actor() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "fixer", "password": PW, "role": "voice_chat", "fixedPassword": true}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json_of(&r)["mustChangePassword"], false, "a fixed password is never temporary");
        let tok = login(port, "fixer");
        let r = post(port, "/cli/auth/change-password", &tok, json!({"currentPassword": PW, "newPassword": "another-Strong-3"}));
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(json_of(&r)["error"], "password_fixed");
        assert_eq!(login(port, "fixer").len(), 64, "the password still works");

        // An Admin sets shares; the audit line names the Admin.
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "shareadm", "password": PW, "mustChangePassword": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "shareadm", "role": "admin"})).status, 200);
        let adm = login(port, "shareadm");
        let r = post(port, "/cli/users/set-shares", &adm, json!({"username": "fixer", "agents": [], "gardens": []}));
        assert_eq!(r.status, 200, "{}", r.body);
        let audit = std::fs::read_to_string(std::path::Path::new(&std::env::var("HOME").expect("home")).join(".k2/auth-audit.jsonl")).expect("audit");
        let line = audit.lines().rev().find(|l| l.contains("\"set-shares\"")).unwrap_or_else(|| panic!("a set-shares line: {audit}"));
        assert!(line.contains("user:shareadm"), "the actor: {line}");
        // L9: an Admin can't move an Admin, refused inside the write.
        let r = post(port, "/cli/users/set-role", &adm, json!({"username": "shareadm", "role": "voice_chat"}));
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(json_of(&r)["error"], "role_change_not_allowed");
    });
}

/// Re-review L9: the voice events socket cap is for Voice & Chat only logins.
/// A Member opens five at once; a Voice & Chat only login gets three and the
/// fourth is refused 429 `too_many_sockets`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_voice_chat_logins_are_held_to_three_voice_event_sockets() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    struct PreviewOff;
    impl Drop for PreviewOff {
        fn drop(&mut self) {
            k2_core::voice_preview::set_enabled(false);
        }
    }
    let _off = PreviewOff;
    k2_core::voice_preview::set_enabled(true);
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "manysock", "password": PW, "mustChangePassword": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        let member = login(port, "manysock");
        let vc = voice_chat_login(port, "fewsock");
        futures_block(async {
            let url = |tok: &str| format!("ws://127.0.0.1:{port}/cli/voice/events?token={tok}");
            let mut held = Vec::new();
            for i in 0..5 {
                let (mut ws, _) = tokio_tungstenite::connect_async(url(&member)).await.unwrap_or_else(|e| panic!("Member socket {i}: {e}"));
                let hello = tokio::time::timeout(Duration::from_secs(5), ws.next())
                    .await
                    .unwrap_or_else(|_| panic!("Member socket {i}: no hello"));
                match hello {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => assert!(t.contains("\"hello\""), "{t}"),
                    other => panic!("Member socket {i}: {other:?}"),
                }
                held.push(ws);
            }
            for i in 0..3 {
                let (ws, _) = tokio_tungstenite::connect_async(url(&vc)).await.unwrap_or_else(|e| panic!("Voice & Chat only socket {i}: {e}"));
                held.push(ws);
            }
            match tokio_tungstenite::connect_async(url(&vc)).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 429),
                other => panic!("a fourth Voice & Chat only socket must be refused 429: {:?}", other.map(|_| ())),
            }
            drop(held);
        });
    });
}
