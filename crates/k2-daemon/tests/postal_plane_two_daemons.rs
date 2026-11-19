//! Postal in K2 end to end (prd-postal-in-k2-v1 S3 network, S4, S5): two
//! real `k2-daemon` children, each under its own temp HOME, talking to a
//! real Postal plane v1 (K2 Web's `/api/postal/v1`).
//!
//! **Needs a throwaway plane**, so it is `#[ignore]`d and run on purpose:
//!
//! ```text
//! K2_POSTAL_E2E_CREDS=/path/creds.json cargo nextest run -p k2-daemon \
//!   --test postal_plane_two_daemons --run-ignored ignored-only
//! ```
//!
//! `creds.json` (never printed): `{base_url, labels: {ptest-a, ptest-b:
//! {connect_credential, decide_token}}}`. Without it the test fails, loudly.
//! Each run uses fresh handle names (the plane allows 5 pair starts per
//! handle pair per hour).
//!
//! What it pins:
//! - each box gets a `pst_` session from its Connect credential (the K2
//!   Connect token in `tunnel.json`, moved into the vault at boot) and
//!   registers its handle key (`PUT /me`);
//! - a pair request from A reaches B; B answers, A reveals; **both** boxes
//!   then hold a request with a pairing code; each person types the code
//!   shown on the OTHER box; each box signs only after its own check and
//!   confirms with the owner's `k2c_pd_`; both pin the peer key;
//! - a box credential (`pst_`) can't confirm: `owner_required`;
//! - a message each way lands (stored, verified against its badge);
//! - B down: A's message waits on the plane (`held`) and lands when B is
//!   back;
//! - A removes B: revoked on the plane, and B's next message fails;
//! - no message body is in either daemon's logs.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Daemon {
    child: Option<KillOnDrop>,
    home: PathBuf,
    port: u16,
    owner: String,
    _rt: k2_core::test_env::DaemonRuntime,
}

impl Daemon {
    fn stop(&mut self) {
        self.child = None;
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.child = None;
        if std::env::var_os("K2_POSTAL_E2E_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }
}

struct Creds {
    base: String,
    labels: Value,
}

fn creds() -> Creds {
    let path = std::env::var("K2_POSTAL_E2E_CREDS").expect("K2_POSTAL_E2E_CREDS must name the throwaway plane's creds.json (this test needs a plane)");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read creds")).expect("creds json");
    let base = v["base_url"].as_str().expect("base_url").trim_end_matches('/').trim_end_matches("/api/postal/v1").to_string();
    Creds { base, labels: v["labels"].clone() }
}

fn cred(c: &Creds, label: &str, field: &str) -> String {
    c.labels[label][field].as_str().unwrap_or_else(|| panic!("creds: {label}.{field} missing")).to_string()
}

fn isolated_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("k2pp-{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(k2_core::paths::k2_home_in(&home)).expect("create temp HOME");
    home
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// A daemon under `home` (made when `None`) with Postal's plane at `base`
/// and `tunnel.json` holding the label's Connect credential (no auto start).
fn spawn(tag: &str, home: Option<PathBuf>, base: &str, connect: &str, label: &str) -> Daemon {
    let home = home.unwrap_or_else(|| isolated_home(tag));
    let k2 = k2_core::paths::k2_home_in(&home);
    let tunnel = k2.join("tunnel.json");
    if !tunnel.exists() {
        std::fs::write(&tunnel, json!({ "token": connect, "subdomain": label, "auto_start": false }).to_string()).expect("tunnel.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tunnel, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        }
    }
    let _ = std::fs::remove_file(k2.join("daemon.port"));
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let rt = k2_core::test_env::DaemonRuntime::new(&home);
    let stderr = std::fs::OpenOptions::new().create(true).append(true).open(home.join("daemon.stderr.log")).expect("stderr log");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            cmd.env_remove(k);
        }
    }
    cmd.env("HOME", &home)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_POSTAL_PLANE_URL", base)
        .env("K2_POSTAL_EVENTS_WAIT", "2")
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
    let child = KillOnDrop(cmd.spawn().expect("spawn k2-daemon"));
    let deadline = Instant::now() + Duration::from_secs(60);
    let (port, owner) = loop {
        let port = read_trimmed(&k2.join("daemon.port")).and_then(|p| p.parse::<u16>().ok());
        let owner = read_trimmed(&k2.join("daemon.token"));
        if let (Some(p), Some(o)) = (port, owner) {
            break (p, o);
        }
        assert!(Instant::now() < deadline, "daemon {tag} never published its port and token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child: Some(child), home, port, owner, _rt: rt };
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some((200, v)) = try_http(d.port, "GET", "/boot-status", None) {
            if v["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon {tag} never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

fn try_http(port: u16, method: &str, path: &str, body: Option<&[u8]>) -> Option<(u16, Value)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(90))).ok()?;
    let body = body.unwrap_or(b"");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).ok()?;
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Some((status, serde_json::from_str(&body).unwrap_or(Value::String(body))))
}

fn http(d: &Daemon, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let sep = if path.contains('?') { '&' } else { '?' };
    let p = format!("{path}{sep}token={}", d.owner);
    let b = body.map(|v| v.to_string().into_bytes());
    try_http(d.port, method, &p, b.as_deref()).unwrap_or_else(|| panic!("{method} {path} on :{} got no answer", d.port))
}

fn create_workspace(d: &Daemon, name: &str) -> String {
    let path = d.home.join("ws").join(name);
    std::fs::create_dir_all(&path).expect("ws dir");
    let p = path.to_string_lossy().to_string();
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&p);
    std::fs::create_dir_all(persona.parent().expect("parent")).expect("persona dir");
    std::fs::write(&persona, format!("---\nname: {name}\n---\n# {name}\n")).expect("persona");
    let (st, v) = http(d, "POST", "/cli/projects/create", Some(json!({ "name": name, "path": p })));
    assert_eq!(st, 200, "projects/create {name}: {v}");
    p
}

/// Poll `f` until it answers `Some`, at most `secs`.
fn until<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(t) = f() {
            return t;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn friend_of(d: &Daemon, ws: &str, peer: &str) -> Option<Value> {
    let (st, v) = http(d, "GET", &format!("/cli/postal/friends?workspace={}", urlenc(ws)), None);
    assert_eq!(st, 200, "friends: {v}");
    v["friends"].as_array()?.iter().find(|f| f["peer"] == peer).cloned()
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn sha_hex(s: &str) -> String {
    k2_core::hire::deliveries::body_sha256(s)
}

/// The receiving side holds a delivery under `badge` whose text is `text`.
fn landed(d: &Daemon, badge: &str, text: &str) -> Value {
    until(&format!("{badge} delivered"), 120, || {
        let (st, v) = http(d, "GET", &format!("/cli/msg/verify?code={badge}&text_sha256={}", sha_hex(text)), None);
        (st == 200 && v["delivered"] == true).then_some(v)
    })
}

fn send(d: &Daemon, ws: &str, to: &str, text: &str) -> Value {
    let (st, v) = http(d, "POST", "/cli/postal/send", Some(json!({ "workspace": ws, "to": to, "text": text, "wake": false })));
    assert_eq!(st, 200, "send: {v}");
    v
}

fn assert_not_in_logs(home: &Path, needle: &str) {
    let mut stack = vec![home.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !(name.ends_with(".log") || name.contains(".log.")) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&p) {
                assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()), "a message body is in the log {}", p.display());
            }
        }
    }
}

#[test]
#[ignore = "needs a throwaway Postal plane: K2_POSTAL_E2E_CREDS (see the module doc)"]
fn two_k2_servers_pair_and_mail_each_other_through_the_plane() {
    let c = creds();
    let run = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let mut a = spawn("a", None, &c.base, &cred(&c, "ptest-a", "connect_credential"), "ptest-a");
    let mut b = spawn("b", None, &c.base, &cred(&c, "ptest-b", "connect_credential"), "ptest-b");
    for d in [&a, &b] {
        let (st, v) = http(d, "POST", "/cli/postal/settings", Some(json!({ "enabled": true })));
        assert_eq!((st, v["enabled"].as_bool()), (200, Some(true)), "{v}");
    }
    let ws_a = create_workspace(&a, &format!("alice{run}"));
    let ws_b = create_workspace(&b, &format!("bob{run}"));
    let addr_a = format!("alice{run}::ptest-a.postal.bot");
    let addr_b = format!("bob{run}::ptest-b.postal.bot");
    for (d, ws, addr, label) in [(&a, &ws_a, &addr_a, "ptest-a"), (&b, &ws_b, &addr_b, "ptest-b")] {
        let (st, v) = http(d, "POST", "/cli/postal/claim", Some(json!({ "workspace": ws, "address": addr })));
        assert_eq!(st, 200, "claim: {v}");
        // A token handed over by the owner (headless boxes); never echoed.
        let token = cred(&c, label, "decide_token");
        let (st, v) = http(d, "POST", "/cli/postal/owner-link", Some(json!({ "token": token })));
        assert_eq!((st, v["linked"].as_bool()), (200, Some(true)), "owner-link: {v}");
        assert!(!v.to_string().contains(&token), "the token is never answered back");
        until(&format!("{addr} on the Postal network"), 120, || {
            let (_, v) = http(d, "GET", &format!("/cli/postal/status?workspace={}", urlenc(ws)), None);
            (v["plane"]["connected"] == true && v["handle"]["status"] == "claimed").then_some(())
        });
    }

    // ── pairing: A asks; both boxes end up holding a code ──
    let (st, v) = http(&a, "POST", "/cli/postal/invite", Some(json!({ "workspace": ws_a, "to": addr_b, "why": "e2e" })));
    assert_eq!(st, 200, "invite: {v}");
    let fb = until("the request at B, with its code", 120, || friend_of(&b, &ws_b, &addr_a).filter(|f| f["status"] == "pending_in" && f["codeWaiting"] == true));
    let fa = until("A's handshake, with its code", 120, || friend_of(&a, &ws_a, &addr_b).filter(|f| f["status"] == "pending_in" && f["codeWaiting"] == true));
    let code = |d: &Daemon, ws: &str, id: &str| -> String {
        let (st, v) = http(d, "GET", &format!("/cli/postal/code?workspace={}&id={id}", urlenc(ws)), None);
        assert_eq!(st, 200, "code: {v}");
        v["code"].as_str().expect("code").replace(' ', "")
    };
    let code_a = code(&a, &ws_a, fa["id"].as_str().unwrap());
    let code_b = code(&b, &ws_b, fb["id"].as_str().unwrap());
    assert_eq!(code_a, code_b, "nobody in the middle: the same code on both boxes");

    // A box credential can't confirm (the plane's owner gate): a pst_.
    {
        let pair = fb["pairId"].as_str().expect("pair id").to_string();
        let client = reqwest::blocking::Client::new();
        let s: Value = client
            .post(format!("{}/api/postal/v1/session", c.base))
            .bearer_auth(cred(&c, "ptest-b", "connect_credential"))
            .json(&json!({}))
            .send()
            .expect("session")
            .json()
            .expect("session json");
        let pst = s["token"].as_str().expect("pst_");
        let r = client
            .post(format!("{}/api/postal/v1/pairs/{pair}/confirm", c.base))
            .bearer_auth(pst)
            .json(&json!({ "side": "to", "decision": "accept", "transcript_sha256": "00".repeat(32), "sig": "A".repeat(86) + "==" }))
            .send()
            .expect("confirm");
        assert_eq!(r.status().as_u16(), 403);
        let e: Value = r.json().expect("error json");
        assert_eq!(e["error"]["code"], "owner_required", "{e}");
    }

    // Each person types the code shown on the OTHER box (a code that
    // doesn't match would sign `mismatch` and fail the pair on both).
    let (st, v) = http(&b, "POST", "/cli/postal/decide", Some(json!({ "workspace": ws_b, "id": fb["id"], "decision": "accept", "code": code_a })));
    assert_eq!(st, 200, "B accepts: {v}");
    let (st, v) = http(&a, "POST", "/cli/postal/decide", Some(json!({ "workspace": ws_a, "id": fa["id"], "decision": "accept", "code": code_b })));
    assert_eq!(st, 200, "A accepts: {v}");
    for (d, ws, peer) in [(&a, &ws_a, &addr_b), (&b, &ws_b, &addr_a)] {
        until(&format!("{peer} a friend"), 120, || friend_of(d, ws, peer).filter(|f| f["status"] == "accepted" && f["peerFingerprint"].is_string()));
    }

    // ── mail both ways (inbox routes: no agent to wake in a test daemon) ──
    for (d, ws) in [(&a, &ws_a), (&b, &ws_b)] {
        let (st, v) = http(d, "POST", "/cli/postal/route", Some(json!({ "workspace": ws, "targets": "inbox", "wake": false })));
        assert_eq!(st, 200, "route: {v}");
    }
    let t1 = format!("hello bob, run {run}, one");
    let s1 = send(&a, &ws_a, &addr_b, &t1);
    assert!(matches!(s1["status"].as_str(), Some("held") | Some("delivered")), "{s1}");
    let badge1 = s1["badge"].as_str().expect("badge").to_string();
    let v1 = landed(&b, &badge1, &t1);
    assert_eq!((v1["content"].as_str(), v1["sender"]["address"].as_str()), (Some("match"), Some(addr_a.as_str())), "{v1}");
    assert_eq!(v1["via"], "postal");
    let t2 = format!("hello alice, run {run}, two");
    let s2 = send(&b, &ws_b, &addr_a, &t2);
    let v2 = landed(&a, s2["badge"].as_str().expect("badge"), &t2);
    assert_eq!(v2["content"], "match", "{v2}");
    // A's own record of message 1 says delivered (the plane's receipt).
    until("A's receipt for message 1", 120, || {
        let (_, v) = http(&a, "GET", &format!("/cli/msg/verify?code={}", s1["deliveryId"].as_str().unwrap()), None);
        (v["landed"]["state"] == "delivered").then_some(())
    });

    // ── B is down: A's message waits on the plane, then lands ──
    let b_home = b.home.clone();
    b.stop();
    let t3 = format!("while you were out, run {run}, three");
    let s3 = send(&a, &ws_a, &addr_b, &t3);
    assert_eq!(s3["status"], "held", "{s3}");
    // Keep B's HOME: the restarted daemon is the same server.
    let keep = std::mem::replace(&mut b.home, isolated_home("gone"));
    assert_eq!(keep, b_home);
    let b2 = spawn("b", Some(b_home), &c.base, "", "ptest-b");
    let v3 = landed(&b2, s3["badge"].as_str().expect("badge"), &t3);
    assert_eq!(v3["content"], "match", "{v3}");
    // One record per message: the held copy wasn't stored twice.
    let (_, again) = http(&b2, "GET", &format!("/cli/msg/verify?code={}", s3["badge"].as_str().unwrap()), None);
    assert_eq!(again["postalMessageId"], v3["postalMessageId"]);

    // ── A removes B: revoked on the plane; B's next message fails ──
    let (st, v) = http(&a, "POST", "/cli/postal/remove", Some(json!({ "workspace": ws_a, "peer": addr_b })));
    assert_eq!(st, 200, "remove: {v}");
    assert_eq!(v["revokedOnNetwork"], true, "{v}");
    let (st, v) = http(&b2, "POST", "/cli/postal/send", Some(json!({ "workspace": ws_b, "to": addr_a, "text": "after the end", "wake": false })));
    let refused = st == 403 && v["error"]["code"] == "postal_not_friends";
    let failed = st == 200 && v["status"] == "failed";
    assert!(refused || failed, "a revoked friend can't send: {st} {v}");
    until("B sees the friendship ended", 120, || friend_of(&b2, &ws_b, &addr_a).filter(|f| f["status"] == "revoked"));

    // ── no body text in any log ──
    for t in [&t1, &t2, &t3] {
        assert_not_in_logs(&a.home, t);
        assert_not_in_logs(&b2.home, t);
    }
    a.stop();
}
