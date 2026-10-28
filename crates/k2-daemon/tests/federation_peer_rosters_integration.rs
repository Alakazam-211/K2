//! `GET /cli/federation/peer-rosters` across real daemons (0.46.1, the one
//! Connections picker; eval-connections-pickers-v1 option b2).
//!
//! Two `k2-daemon` children under their own temp HOMEs (the harness of
//! `vault_handoff_two_daemons_integration`), A and B, federation on, paired
//! by writing each other's pinned key. A also pins three servers that never
//! give a roster: one on a closed port (offline at once) and two that
//! accept and then say nothing (they hang until the 5 s cap).
//!
//! What it pins:
//!   - one call lists This server first, then every paired server A to Z
//!     (case-insensitive), each server's agents A to Z;
//!   - B answers `ok` with its exposed agents; the closed port and the two
//!     hanging servers answer `offline`; the older pairing is flagged;
//!   - the hanging servers cost ONE 5 s cap together (parallel), never the
//!     old 20 s;
//!   - `?peer=` asks one server; POST is 405; a Member login is 403;
//!   - `/cli/federation/peers` and `k2 fed peers` list the same order.

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
        // Short: `$HOME/.k2/vault.sock` must fit a Unix socket path.
        "k2pr-{tag}-{}",
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

fn spawn_daemon(tag: &str) -> Daemon {
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
        .env("K2_FEDERATION", "1")
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
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
            let ready = serde_json::from_str::<serde_json::Value>(&r.body)
                .map(|v| v["phase"] == "ready")
                .unwrap_or(false);
            if r.status == 200 && ready {
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

fn http_as(d: &Daemon, token: &str, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let sep = if path.contains('?') { '&' } else { '?' };
    let p = format!("{path}{sep}token={token}");
    let b = body.map(|v| v.to_string().into_bytes());
    let r = try_http(d.port, method, &p, b.as_deref())
        .unwrap_or_else(|| panic!("{method} {path} on :{} got no answer", d.port));
    (r.status, serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null))
}

fn http(d: &Daemon, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    http_as(d, &d.owner.clone(), method, path, body)
}

fn pubkey(d: &Daemon) -> (String, String) {
    let (st, v) = http(d, "GET", "/cli/federation/pubkey", None);
    assert_eq!(st, 200, "pubkey: {v}");
    (
        v["public_key_pem"].as_str().expect("pem").to_string(),
        v["fingerprint"].as_str().expect("fp").to_string(),
    )
}

/// Pin `peers` (label, pem, base url, pairing version) as Trusted with
/// inbound + roster.
fn write_peers(d: &Daemon, peers: &[(&str, &str, String, u32)]) {
    let rows: Vec<serde_json::Value> = peers
        .iter()
        .map(|(label, pem, base, version)| {
            let mut p = k2_core::federation::FederationPeer::pin(*label, *label, *pem).expect("pin");
            p.trust = k2_core::federation::PeerTrust::Trusted;
            p.capabilities.insert("inbound".into());
            p.capabilities.insert("roster".into());
            p.base_url = base.clone();
            p.pairing_version = *version;
            serde_json::to_value(p).expect("peer json")
        })
        .collect();
    let path = d.home.join(".k2/federation-peers.json");
    std::fs::write(&path, serde_json::json!({ "version": 1, "peers": rows }).to_string()).expect("peer store");
    k2_core::private_fs::make_private_file(&path).expect("peer store 0600 / owner-only");
}

fn create_workspace(d: &Daemon, name: &str) {
    let path = d.home.join("ws").join(name);
    std::fs::create_dir_all(&path).expect("ws dir");
    let p = path.to_string_lossy().to_string();
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&p);
    std::fs::create_dir_all(persona.parent().expect("parent")).expect("persona dir");
    std::fs::write(&persona, format!("---\nname: {name}\n---\n# {name}\n")).expect("persona");
    let (st, v) = http(d, "POST", "/cli/projects/create", Some(serde_json::json!({ "name": name, "path": p })));
    assert_eq!(st, 200, "projects/create {name}: {v}");
}

fn new_pem() -> String {
    rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key").public_key_pem()
}

/// A port with nothing listening (bound, then closed): connect is refused.
fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    l.local_addr().expect("addr").port()
}

/// A server that accepts every connection and never answers.
fn hanging_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = l.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in l.incoming().flatten() {
            held.push(s);
        }
    });
    port
}

fn server<'a>(v: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
    v["servers"]
        .as_array()
        .expect("servers")
        .iter()
        .find(|s| s["label"] == label)
        .unwrap_or_else(|| panic!("no server {label} in {v}"))
}

fn names(s: &serde_json::Value) -> Vec<String> {
    s["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .map(|a| a["workspace_name"].as_str().expect("name").to_string())
        .collect()
}

fn cli(d: &Daemon, args: &[&str]) -> (i32, String, String) {
    let k2 = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let mut cmd = Command::new(k2_core::test_env::bash_program());
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            cmd.env_remove(k);
        }
    }
    cmd.arg(&k2).args(args).env("HOME", &d.home).env("PORT", d.port.to_string());
    cmd.stdin(std::process::Stdio::null());
    let out = cmd.output().expect("run cli/k2");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn peer_rosters_lists_this_server_then_paired_servers_a_to_z_with_offline_ones_capped() {
    let a = spawn_daemon("a");
    let b = spawn_daemon("b");
    let (a_pem, _a_fp) = pubkey(&a);
    let (b_pem, b_fp) = pubkey(&b);
    let dead = dead_port();
    let (hang1, hang2) = (hanging_port(), hanging_port());
    let (k_dead, k_h1, k_h2) = (new_pem(), new_pem(), new_pem());
    // Paired in this (not alphabetical) order; one with the older code.
    write_peers(
        &a,
        &[
            ("Zulu-old", &k_dead, format!("http://127.0.0.1:{dead}"), 1),
            ("bravo", &b_pem, format!("http://127.0.0.1:{}", b.port), 2),
            ("Mike-hang", &k_h2, format!("http://127.0.0.1:{hang2}"), 2),
            ("alpha-hang", &k_h1, format!("http://127.0.0.1:{hang1}"), 2),
        ],
    );
    write_peers(&b, &[("alpha", &a_pem, format!("http://127.0.0.1:{}", a.port), 2)]);

    create_workspace(&a, "sales");
    create_workspace(&a, "Accounts");
    create_workspace(&b, "ops");
    create_workspace(&b, "Billing");
    create_workspace(&b, "archive");
    // B lists its agents to paired servers.
    let (st, v) = http(&b, "POST", "/cli/settings/update", Some(serde_json::json!({ "allowRemoteInstruct": true })));
    assert_eq!(st, 200, "settings: {v}");

    let t = Instant::now();
    let (st, v) = http(&a, "GET", "/cli/federation/peer-rosters", None);
    let took = t.elapsed();
    assert_eq!(st, 200, "peer-rosters: {v}");
    let labels: Vec<&str> = v["servers"].as_array().expect("servers").iter().map(|s| s["label"].as_str().expect("label")).collect();
    assert_eq!(labels, vec!["This server", "alpha-hang", "bravo", "Mike-hang", "Zulu-old"], "{v}");
    assert_eq!(v["servers"][0]["kind"], "local");
    assert_eq!(names(server(&v, "This server")), vec!["Accounts", "sales"]);

    let bravo = server(&v, "bravo");
    assert_eq!(bravo["status"], "ok", "{bravo}");
    assert_eq!(bravo["fingerprint"], b_fp.as_str());
    assert_eq!(bravo["host"], format!("127.0.0.1:{}", b.port));
    assert_eq!(names(bravo), vec!["archive", "Billing", "ops"]);
    assert_eq!(bravo["repair_recommended"], false);

    let zulu = server(&v, "Zulu-old");
    assert_eq!(zulu["status"], "offline", "{zulu}");
    assert_eq!(zulu["repair_recommended"], true);
    assert!(zulu["agents"].as_array().expect("agents").is_empty());
    for l in ["alpha-hang", "Mike-hang"] {
        let s = server(&v, l);
        assert_eq!(s["status"], "offline", "{s}");
        assert!(s["error"].as_str().expect("error").contains("no answer within 5"), "{s}");
    }
    // Two hanging servers asked at once: one 5 s cap, never 2 x 5 s or 20 s.
    assert!(took >= Duration::from_millis(4500), "the cap is 5 s: {took:?}");
    assert!(took < Duration::from_secs(9), "hanging servers are asked in parallel: {took:?}");

    // Retry: one server only, This server left out.
    let (st, one) = http(&a, "GET", &format!("/cli/federation/peer-rosters?peer={b_fp}"), None);
    assert_eq!(st, 200, "{one}");
    let one_labels: Vec<&str> = one["servers"].as_array().expect("servers").iter().map(|s| s["label"].as_str().expect("label")).collect();
    assert_eq!(one_labels, vec!["bravo"]);
    let (st, missing) = http(&a, "GET", "/cli/federation/peer-rosters?peer=nobody", None);
    assert_eq!(st, 404, "{missing}");

    // A read: POST is refused.
    let (st, posted) = http(&a, "POST", "/cli/federation/peer-rosters", Some(serde_json::json!({})));
    assert_eq!(st, 405, "{posted}");

    // A Member login can't list paired servers (same gate as peer-roster).
    let password = "Peer-Rosters-Member-9!";
    let (st, added) = http(&a, "POST", "/cli/users/add", Some(serde_json::json!({ "username": "mo", "password": password })));
    assert_eq!(st, 200, "users/add: {added}");
    let r = try_http(
        a.port,
        "POST",
        "/cli/auth/login",
        Some(serde_json::json!({ "username": "mo", "password": password }).to_string().as_bytes()),
    )
    .expect("login");
    assert_eq!(r.status, 200, "login: {}", r.body);
    let member: String = serde_json::from_str::<serde_json::Value>(&r.body).expect("json")["token"]
        .as_str()
        .expect("token")
        .to_string();
    let (st, denied) = http_as(&a, &member, "GET", "/cli/federation/peer-rosters", None);
    assert_eq!(st, 403, "a Member must not list paired servers: {denied}");

    // The same order everywhere: /cli/federation/peers and `k2 fed peers`.
    let (st, peers) = http(&a, "GET", "/cli/federation/peers", None);
    assert_eq!(st, 200, "{peers}");
    let peer_labels: Vec<&str> = peers["peers"].as_array().expect("peers").iter().map(|p| p["label"].as_str().expect("label")).collect();
    assert_eq!(peer_labels, vec!["alpha-hang", "bravo", "Mike-hang", "Zulu-old"]);
    let (code, out, err) = cli(&a, &["fed", "peers"]);
    assert_eq!(code, 0, "k2 fed peers: {out}\n{err}");
    let printed: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split("label=").nth(1))
        .map(|s| s.split_whitespace().next().unwrap_or(""))
        .collect();
    assert_eq!(printed, vec!["alpha-hang", "bravo", "Mike-hang", "Zulu-old"], "{out}");
}
