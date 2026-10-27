//! Secret handoff across two real daemons (prd-agent-secret-handoff-v1
//! §14 "headless two-daemon smoke"), plus the CLI exit codes.
//!
//! Three `k2-daemon` children, each under its own temp HOME (the harness
//! of `home_two_daemons_integration`): A and B with federation on, paired
//! by writing each other's pinned key into `federation-peers.json` (the
//! store is read per request, so this works after boot, with the real
//! ports), and C with federation off, standing in for an old peer (every
//! `/cli/federation/*` route 404s there, exactly like 0.46.0).
//!
//! What it pins:
//!   - A sends a vault secret by name to `ops` on B: it arrives pending,
//!     the owner on B accepts it, and B's copy carries A's provenance;
//!   - C (federation off / old) answers `peer_too_old` and nothing is sent;
//!   - a 200 KiB body to `/cli/federation/vault-handoff` is 413;
//!   - the audit lines show in each server's Settings logs source
//!     (`/cli/users/audit`): send, receive, accept, refusals;
//!   - the planted value appears in no file under either HOME except the
//!     encrypted vault, and in no daemon log;
//!   - `k2 vault send` exits 0 landed, 3 pending / a person must act,
//!     2 peer_too_old / a value on stdin to a local agent, 4 no such secret.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Daemon {
    _child: KillOnDrop,
    home: PathBuf,
    port: u16,
    owner: String,
    _rt: k2_core::test_env::DaemonRuntime,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self._child.0.kill();
        let _ = self._child.0.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn isolated_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        // Short: `$HOME/.k2/vault.sock` must fit a Unix socket path (104
        // bytes on macOS), or the person channel never binds.
        "k2ho-{tag}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME");
    home
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn spawn_daemon(tag: &str, federation: bool) -> Daemon {
    let home = isolated_home(tag);
    let shim_dir = home.join("agent-shim-empty");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let rt = k2_core::test_env::DaemonRuntime::new(&home);
    let stderr = std::fs::File::create(home.join("daemon.stderr.log")).expect("stderr log");
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
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
    if federation {
        cmd.env("K2_FEDERATION", "1");
    }
    let child = KillOnDrop(cmd.spawn().expect("spawn k2-daemon"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let (port, owner) = loop {
        let port = read_trimmed(&home.join(".k2/daemon.port")).and_then(|p| p.parse::<u16>().ok());
        let owner = read_trimmed(&home.join(".k2/daemon.token"));
        if let (Some(p), Some(o)) = (port, owner) {
            break (p, o);
        }
        assert!(Instant::now() < deadline, "daemon {tag} never published its port and token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { _child: child, home, port, owner, _rt: rt };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(r) = try_http(d.port, "GET", "/boot-status", None) {
            if r.status == 200 && serde_json::from_str::<serde_json::Value>(&r.body).map(|v| v["phase"] == "ready").unwrap_or(false) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon {tag} never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

struct Resp {
    status: u16,
    body: String,
}

fn try_http(port: u16, method: &str, path: &str, body: Option<&[u8]>) -> Option<Resp> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
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
    Some(Resp { status, body })
}

fn http(d: &Daemon, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let sep = if path.contains('?') { '&' } else { '?' };
    let p = format!("{path}{sep}token={}", d.owner);
    let b = body.map(|v| v.to_string().into_bytes());
    let r = try_http(d.port, method, &p, b.as_deref()).unwrap_or_else(|| panic!("{method} {path} on :{} got no answer", d.port));
    let v = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    (r.status, v)
}

fn http_as(d: &Daemon, token: &str, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let sep = if path.contains('?') { '&' } else { '?' };
    let p = format!("{path}{sep}token={token}");
    let b = body.map(|v| v.to_string().into_bytes());
    let r = try_http(d.port, method, &p, b.as_deref()).unwrap_or_else(|| panic!("{method} {path} on :{} got no answer", d.port));
    (r.status, serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null))
}

fn pubkey(d: &Daemon) -> (String, String) {
    let (st, v) = http(d, "GET", "/cli/federation/pubkey", None);
    assert_eq!(st, 200, "pubkey: {v}");
    (v["public_key_pem"].as_str().expect("pem").to_string(), v["fingerprint"].as_str().expect("fp").to_string())
}

/// Pin `peers` (label, pem, base url) as Trusted with inbound + roster.
fn write_peers(d: &Daemon, peers: &[(&str, &str, String)]) {
    let rows: Vec<serde_json::Value> = peers
        .iter()
        .map(|(label, pem, base)| {
            let mut p = k2_core::federation::FederationPeer::pin(*label, *label, *pem).expect("pin");
            p.trust = k2_core::federation::PeerTrust::Trusted;
            p.capabilities.insert("inbound".into());
            p.capabilities.insert("roster".into());
            p.base_url = base.clone();
            serde_json::to_value(p).expect("peer json")
        })
        .collect();
    let path = d.home.join(".k2/federation-peers.json");
    std::fs::write(&path, serde_json::json!({ "version": 1, "peers": rows }).to_string()).expect("peer store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    }
}

fn create_workspace(d: &Daemon, name: &str) -> String {
    let path = d.home.join("ws").join(name);
    std::fs::create_dir_all(&path).expect("ws dir");
    let p = path.to_string_lossy().to_string();
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&p);
    std::fs::create_dir_all(persona.parent().expect("parent")).expect("persona dir");
    std::fs::write(&persona, format!("---\nname: {name}\n---\n# {name}\n")).expect("persona");
    let (st, v) = http(d, "POST", "/cli/projects/create", Some(serde_json::json!({ "name": name, "path": p })));
    assert_eq!(st, 200, "projects/create {name}: {v}");
    p
}

fn set_secret(d: &Daemon, ws: &str, name: &str, value: &str, scope: &str) {
    use base64::Engine as _;
    let mut b = serde_json::json!({ "name": name, "kind": "api_key", "hosts": ["api.stripe.com"],
        "valueB64": base64::engine::general_purpose::STANDARD.encode(value) });
    if scope == "server" {
        b["scope"] = "server".into();
    } else {
        b["scope"] = "workspace".into();
        b["workspace"] = ws.into();
    }
    let (st, v) = http(d, "POST", "/cli/vault/set", Some(b));
    assert_eq!(st, 200, "vault/set {name}: {v}");
}

fn handoff_audit(d: &Daemon) -> Vec<String> {
    let (st, v) = http(d, "GET", "/cli/users/audit?tail=1000", None);
    assert_eq!(st, 200, "users/audit: {v}");
    v["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|e| e["event"] == "vault-handoff")
        .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Every file under `dir` except the encrypted vault holds no `needle`.
fn assert_not_on_disk(dir: &Path, needle: &str) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("vault.db") {
                continue; // ciphertext
            }
            if let Ok(bytes) = std::fs::read(&p) {
                assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()), "the value is in plaintext in {}", p.display());
            }
        }
    }
}

fn cli(d: &Daemon, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let k2 = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let mut cmd = Command::new("bash");
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            cmd.env_remove(k);
        }
    }
    cmd.arg(&k2).args(args).env("HOME", &d.home).env("PORT", d.port.to_string());
    cmd.stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("run cli/k2");
    if let Some(s) = stdin {
        child.stdin.take().expect("stdin").write_all(s.as_bytes()).expect("write stdin");
    }
    let out = child.wait_with_output().expect("cli output");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn two_daemons_hand_a_secret_over_sealed_and_old_peers_get_peer_too_old() {
    let a = spawn_daemon("a", true);
    let b = spawn_daemon("b", true);
    let c = spawn_daemon("c", false);
    let (a_pem, _a_fp) = pubkey(&a);
    let (b_pem, b_fp) = pubkey(&b);
    // C has federation off (an old peer): any key stands in for its pin.
    let c_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    write_peers(&a, &[("bravo", &b_pem, format!("http://127.0.0.1:{}", b.port)), ("charlie", &c_key.public_key_pem(), format!("http://127.0.0.1:{}", c.port))]);
    write_peers(&b, &[("alpha", &a_pem, format!("http://127.0.0.1:{}", a.port))]);

    create_workspace(&a, "sales");
    create_workspace(&b, "ops");
    create_workspace(&b, "billing");
    // B lists its agents to peers and takes their messages (opt-in on).
    let (st, v) = http(&b, "POST", "/cli/settings/update", Some(serde_json::json!({ "allowRemoteInstruct": true })));
    assert_eq!(st, 200, "settings: {v}");

    // The owner gave sales this key, so it's person-given (R3).
    let secret = format!("k2test_TWO_DAEMON_SENTINEL_{}", uuid::Uuid::new_v4().simple());
    set_secret(&a, "sales", "STRIPE_KEY", &secret, "workspace");
    let send_body = serde_json::json!({ "name": "STRIPE_KEY", "scope": "workspace", "workspace": "sales", "to": "ops::bravo", "note": "for the refund job" });

    // A person-given secret to another server needs a person's vault confirm.
    let (st, v) = http(&a, "POST", "/cli/vault/send", Some(send_body.clone()));
    assert_eq!((st, v["error"]["code"].as_str().unwrap_or("")), (403, "confirm_required"), "{v}");
    let secret_id = v["secretId"].as_str().expect("secretId").to_string();
    // An Admin login confirms with its Connect password, then sends.
    let password = "Handoff-Two-Daemons-9!";
    let (st, v) = http(&a, "POST", "/cli/users/add", Some(serde_json::json!({ "username": "lee", "password": password })));
    assert_eq!(st, 200, "users/add: {v}");
    let (st, v) = http(&a, "POST", "/cli/users/set-role", Some(serde_json::json!({ "username": "lee", "role": "admin" })));
    assert_eq!(st, 200, "set-role: {v}");
    let r = try_http(a.port, "POST", "/cli/auth/login", Some(serde_json::json!({ "username": "lee", "password": password }).to_string().as_bytes())).expect("login");
    assert_eq!(r.status, 200, "login: {}", r.body);
    let login: String = serde_json::from_str::<serde_json::Value>(&r.body).expect("json")["token"].as_str().expect("token").to_string();
    let (st, v) = http_as(&a, &login, "POST", "/cli/vault/confirm/finish", Some(serde_json::json!({
        "action": "handoff-send", "secretId": secret_id, "method": "connect-password", "password": password })));
    assert_eq!(st, 200, "confirm: {v}");
    let mut confirmed = send_body.clone();
    confirmed["confirmToken"] = v["token"].clone();

    // A → ops on B: sealed, pending for B's owner (person-given: a person decides).
    let (st, v) = http_as(&a, &login, "POST", "/cli/vault/send", Some(confirmed));
    assert_eq!(st, 200, "send: {v}");
    assert_eq!(v["status"], "pending", "{v}");
    assert_eq!(v["reason"], "person_given", "{v}");
    assert_eq!(v["cross"], true);
    let id = v["deliveryId"].as_str().expect("id").to_string();
    let (_, sent) = http(&a, "GET", "/cli/vault/sent?workspace=sales", None);
    assert_eq!(sent["items"][0]["deliveryId"], id.as_str(), "{sent}");

    let (st, inc) = http(&b, "GET", "/cli/vault/incoming?workspace=ops", None);
    assert_eq!(st, 200, "{inc}");
    let item = &inc["items"][0];
    assert_eq!(item["deliveryId"], id.as_str());
    assert_eq!(item["status"], "pending");
    assert_eq!(item["peer"], format!("sales::127.0.0.1:{}", a.port), "{item}");
    assert_eq!(item["note"], "for the refund job");

    // B's owner accepts it.
    let (st, d) = http(&b, "POST", "/cli/vault/incoming/decide", Some(serde_json::json!({ "id": id, "action": "accept" })));
    assert_eq!(st, 200, "decide: {d}");
    assert_eq!(d["delivery"]["status"], "landed");
    assert!(d["ref"].as_str().expect("ref").starts_with("k2v://ws/"), "{d}");
    let (st, show) = http(&b, "GET", "/cli/vault/show?name=STRIPE_KEY&scope=workspace&workspace=ops", None);
    assert_eq!(st, 200, "show: {show}");
    let m = &show["secret"];
    assert_eq!(m["defaultDelivery"], "fetch");
    assert_ne!(m["deliveryCeiling"], "api", "never api (H12)");
    assert_eq!(m["createdBy"], "system:handoff");
    assert_eq!(m["humanGiven"], false, "H10");
    assert_eq!(m["handoff"]["deliveryId"], id.as_str());

    // C is federation-off / old: peer_too_old, nothing sent, no row, no
    // confirm asked of anyone.
    let (st, v) = http(&a, "POST", "/cli/vault/send", Some(serde_json::json!({ "name": "STRIPE_KEY", "scope": "workspace", "workspace": "sales", "to": "ops::charlie" })));
    assert_eq!((st, v["error"]["code"].as_str().unwrap_or("")), (422, "peer_too_old"), "{v}");
    let (_, sent) = http(&a, "GET", "/cli/vault/sent?workspace=sales", None);
    assert_eq!(sent["items"].as_array().expect("items").len(), 1, "no row for the refused send: {sent}");

    // The public route reads at most 160 KiB.
    let big = vec![b'x'; 200 * 1024];
    let r = try_http(b.port, "POST", "/cli/federation/vault-handoff", Some(&big)).expect("answer");
    assert_eq!(r.status, 413, "{}", r.body);

    // Settings → Logs → Access Audit on each server.
    let la = handoff_audit(&a);
    assert!(la.iter().any(|l| l == "deny send STRIPE_KEY to ops::bravo: confirm_required"), "{la:?}");
    assert!(la.iter().any(|l| l.starts_with("ok send STRIPE_KEY to ops::127.0.0.1") && l.contains(&id) && l.ends_with("pending")), "{la:?}");
    assert!(la.iter().any(|l| l == "deny send STRIPE_KEY to ops::charlie: peer_too_old"), "{la:?}");
    let lb = handoff_audit(&b);
    assert!(lb.iter().any(|l| l.starts_with("ok receive STRIPE_KEY from sales::127.0.0.1") && l.contains(&id)), "{lb:?}");
    assert!(lb.iter().any(|l| l.starts_with("ok accept STRIPE_KEY from sales::127.0.0.1") && l.contains(&id)), "{lb:?}");
    let _ = b_fp;

    // ── CLI exit codes (the owner's own terminal, no K2 session) ──
    // B: the landed copy (not person-given) to billing: pending until
    // billing accepts from ops, then it lands.
    let (code, out, err) = cli(&b, &["vault", "send", "STRIPE_KEY", "--scope", "ws:ops", "--to", "billing"], None);
    assert_eq!(code, 3, "pending: a person must act: {out} {err}");
    assert!(out.contains(": pending"), "{out}");
    assert!(err.contains("a person on this server must accept it"), "{err}");
    let (code, out, err) = cli(&b, &["vault", "accept-from", "add", "ops", "--workspace", "billing"], None);
    assert_eq!(code, 0, "{out} {err}");
    let (code, out, err) = cli(&b, &["vault", "send", "STRIPE_KEY", "--scope", "ws:ops", "--to", "billing", "--as", "STRIPE_FROM_OPS"], None);
    assert_eq!(code, 0, "an allow-listed local send lands: {out} {err}");
    assert!(out.contains(": landed"), "{out}");
    let (code, out, _) = cli(&b, &["vault", "incoming", "--workspace", "billing"], None);
    assert_eq!(code, 0);
    assert!(out.contains("STRIPE_FROM_OPS") && out.contains("landed"), "{out}");
    // A: no such secret (4), an old peer (2), a typed value to a local
    // agent (2), a server-wide secret (3).
    let (code, _, err) = cli(&a, &["vault", "send", "NOPE", "--scope", "ws:sales", "--to", "ops::bravo"], None);
    assert_eq!(code, 4, "no such secret: {err}");
    let (code, _, err) = cli(&a, &["vault", "send", "STRIPE_KEY", "--scope", "ws:sales", "--to", "ops::charlie"], None);
    assert_eq!(code, 2, "peer_too_old: {err}");
    assert!(err.contains("peer_too_old"), "{err}");
    let (code, _, err) = cli(&a, &["vault", "send", "TYPED", "--to", "sales", "--stdin"], Some("a-value-typed-in-123"));
    assert_eq!(code, 2, "a typed value goes to another server only: {err}");
    set_secret(&a, "", "SERVER_WIDE", "server-wide-value-456", "server");
    let (code, _, err) = cli(&a, &["vault", "send", "SERVER_WIDE", "--scope", "server", "--to", "sales"], None);
    assert_eq!(code, 3, "server-wide secrets aren't sent: {err}");
    assert!(err.contains("k2 vault grant"), "{err}");

    // The value is nowhere in plaintext on either server.
    for d in [&a, &b, &c] {
        assert_not_on_disk(&d.home, &secret);
    }
}
