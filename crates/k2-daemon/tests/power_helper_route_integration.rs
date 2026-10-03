//! Power-helper S1 (`prd-power-helper-smappservice-v1.md`) — no surprise
//! admin dialog, through the REAL dispatcher.
//!
//! The power layer is swapped for `FakePowerOs::needs_approval(true)` (a
//! Mac without the 0.43.0 helper whose "dialog" counts and accepts), so no
//! test touches `osascript`, `pmset` or any real power setting. The
//! in-process harness binds a loopback listener (`Ingress::Loopback`) and
//! a tunnel-ingress listener (`Ingress::Tunnel`, a remote client). Pins:
//!   - changing the mode or "Also with the lid closed" never runs the
//!     installer, for any caller;
//!   - `POST /cli/power/helper {action:"setup"}`: a Member login gets 403
//!     `role_required` (route policy Admin); an Admin login or the owner
//!     token over the tunnel gets 403 `host_only`; GET is 405; a local
//!     Admin runs the dialog exactly once;
//!   - an old client's `approveLid` and the wake switch never prompt for
//!     a Member or a remote client.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::connect_users::{self, Role};
use k2_daemon::power::fake::FakePowerOs;
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER: &str = "owner-token-power-helper-s1";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read response: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("{method} {path}: no status line in {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("{method} {path}: no header end in {text:?}"));
    Resp { status, body }
}

fn post(port: u16, path: &str, token: &str, body: serde_json::Value) -> Resp {
    http(port, "POST", &format!("{path}?token={token}"), Some(&body.to_string()))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-power-helper-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME/.k2");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn login(username: &str, role: Role) -> String {
    connect_users::add_user(username, "correct horse 6").expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

fn saved_keep_awake() -> serde_json::Value {
    let home = std::env::var("HOME").expect("HOME");
    let raw = std::fs::read_to_string(std::path::Path::new(&home).join(".k2").join("settings.json"))
        .expect("settings.json written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("settings JSON");
    v["keepAwake"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_mode_change_or_remote_or_member_ever_shows_the_admin_dialog() {
    let _g = lock();
    with_temp_home(|| {
        let fake = Arc::new(FakePowerOs::needs_approval(true));
        k2_daemon::power::install_os(fake.clone());
        let member = login("ph_member", Role::Member);
        let admin = login("ph_admin", Role::Admin);
        let d = futures_block(test_harness::start(OWNER));
        let (local, tunnel) = (d.port, d.tunnel_port);

        // ── Mode and lid switch changes, every caller: never the installer.
        for (port, token) in [(local, OWNER), (local, member.as_str()), (tunnel, OWNER), (tunnel, admin.as_str())] {
            for body in [
                serde_json::json!({ "mode": "working" }),
                serde_json::json!({ "mode": "always" }),
                serde_json::json!({ "lidClosed": true }),
                serde_json::json!({ "mode": "working", "lidClosed": false }),
                serde_json::json!({ "mode": "off" }),
            ] {
                let r = post(port, "/cli/power/keep-awake", token, body.clone());
                assert_eq!(r.status, 200, "{body} on :{port}: {}", r.body);
                assert_eq!(fake.approval_prompts(), 0, "{body} on :{port} ran the installer");
            }
        }

        // Always + the switch on, no helper: lid open only, needs set up.
        let r = post(local, "/cli/power/keep-awake", OWNER, serde_json::json!({ "mode": "always", "lidClosed": true }));
        assert_eq!(r.status, 200, "{}", r.body);
        let k = &r.json()["keepAwake"];
        assert_eq!(k["state"], "lid_open_only", "{k}");
        assert_eq!(k["held"], true);
        assert_eq!(k["lidHeld"], false);
        assert_eq!(k["lidClosed"], true);
        assert_eq!(k["lidSetup"], "needs_setup");
        assert_eq!(k["canSetUp"], true, "the owner token on loopback may set up");
        assert_eq!(k["detail"], "Lid closed will still sleep: the power helper is not set up on this Mac");
        assert_eq!(saved_keep_awake()["lidClosed"], true);
        assert_eq!(fake.approval_prompts(), 0);

        // Status offers Set up to nobody else.
        for (port, token, who) in [(local, member.as_str(), "local member"), (tunnel, admin.as_str(), "remote admin"), (tunnel, OWNER, "remote owner")] {
            let r = http(port, "GET", &format!("/cli/power/status?token={token}"), None);
            assert_eq!(r.status, 200, "{who}: {}", r.body);
            let k = &r.json()["keepAwake"];
            assert_eq!(k["lidSetup"], "needs_setup", "{who}");
            assert_eq!(k["canSetUp"], false, "{who}");
            assert_eq!(k["canApproveLid"], false, "{who}: an old client must not show its dialog button");
        }

        // ── Set up: Member → 403 role_required (route policy Admin).
        let r = post(local, "/cli/power/helper", &member, serde_json::json!({ "action": "setup" }));
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(r.json()["error"], "role_required", "{}", r.body);
        assert_eq!(r.json()["required"], "admin", "{}", r.body);
        // Remote Admin and remote owner token → 403 host_only.
        for (token, who) in [(admin.as_str(), "admin login"), (OWNER, "owner token")] {
            let r = post(tunnel, "/cli/power/helper", token, serde_json::json!({ "action": "setup" }));
            assert_eq!(r.status, 403, "remote {who}: {}", r.body);
            assert_eq!(r.json()["error"], "host_only", "remote {who}: {}", r.body);
            assert!(r.json()["message"].as_str().expect("message").contains("host Mac"), "{}", r.body);
        }
        // No token → 403.
        let r = http(local, "POST", "/cli/power/helper", Some(r#"{"action":"setup"}"#));
        assert_eq!(r.status, 403, "{}", r.body);
        // POST only.
        let r = http(local, "GET", &format!("/cli/power/helper?token={OWNER}"), None);
        assert_eq!(r.status, 405, "{}", r.body);
        assert_eq!(fake.approval_prompts(), 0, "no refused Set up may reach the installer");

        // ── Old clients' approveLid: turns the switch on, never prompts
        // for a Member or a remote client.
        for (port, token, who) in [(local, member.as_str(), "local member"), (tunnel, OWNER, "remote owner"), (tunnel, admin.as_str(), "remote admin")] {
            let r = post(port, "/cli/power/keep-awake", token, serde_json::json!({ "approveLid": true }));
            assert_eq!(r.status, 200, "{who}: {}", r.body);
            let k = &r.json()["keepAwake"];
            assert!(k["message"].as_str().expect("message").contains("host Mac"), "{who}: {k}");
            assert_eq!(fake.approval_prompts(), 0, "{who}: approveLid ran the installer");
        }

        // ── The wake switch: no dialog for a Member or a remote client.
        for (port, token, who) in [(local, member.as_str(), "local member"), (tunnel, OWNER, "remote owner")] {
            let r = post(port, "/cli/heartbeat/wake", token, serde_json::json!({ "enabled": true }));
            assert_eq!(r.status, 200, "{who}: {}", r.body);
            let v = r.json();
            assert_eq!(v["success"], false, "{who}: {v}");
            assert_eq!(v["wakeForHeartbeats"], false, "{who}: the switch stays off");
            assert_eq!(fake.approval_prompts(), 0, "{who}: the wake switch ran the installer");
        }

        // ── A bad action from a local admin is a 400 and runs nothing.
        let r = post(local, "/cli/power/helper", &admin, serde_json::json!({ "action": "remove" }));
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(fake.approval_prompts(), 0);

        // ── A local Admin login runs the dialog exactly once; lid closed holds.
        let r = post(local, "/cli/power/helper", &admin, serde_json::json!({ "action": "setup" }));
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(fake.approval_prompts(), 1);
        assert_eq!(v["keepAwake"]["lidSetup"], "ready", "{v}");
        assert_eq!(v["keepAwake"]["state"], "lid_closed_ok", "{v}");
        assert_eq!(fake.lid_taken(), 1);
        // Set up again: nothing to do, no second dialog.
        let r = post(local, "/cli/power/helper", OWNER, serde_json::json!({ "action": "setup" }));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["message"], "The power helper is already set up.");
        assert_eq!(fake.approval_prompts(), 1);
    });
}
