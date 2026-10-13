//! Agent hire through the v1 API, end to end through the REAL dispatcher
//! (`k2_daemon::test_harness`): owner sets the hire root → mints a hire key
//! (owner confirm required even with the owner token) → the key hires a job
//! lead and a linked Desk team with an `Idempotency-Key` → replay → list →
//! links both ways → teardown (real archive-retire) → audit lines.
//!
//! ISOLATION: `$HOME` points at a fresh temp dir (DB is the in-memory test
//! DB; the vault opens on a file key under the temp home; retire archives
//! into the temp home; rolled-back folders go to a temp trash dir). No real
//! AI CLI runs: no member launches.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-v1-agent-hire-0456";

struct Resp {
    status: u16,
    body: Value,
}

fn http(port: u16, method: &str, path_and_query: &str, bearer: Option<&str>, idem: Option<&str>, body: Option<&Value>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut head = format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    if let Some(k) = idem {
        head.push_str(&format!("Idempotency-Key: {k}\r\n"));
    }
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    if body.is_some() {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body_text = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    let body = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Resp { status, body }
}

fn owner_q(path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}token={OWNER_TOKEN}")
}

struct Env {
    prev_home: Option<std::ffi::OsString>,
    prev_api: Option<std::ffi::OsString>,
    tmp: std::path::PathBuf,
}

impl Env {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let prev_api = std::env::var_os("K2_API");
        let tmp = std::env::temp_dir().join(format!(
            "k2-v1-agent-hire-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("home")).expect("mkdir home");
        std::env::set_var("HOME", tmp.join("home"));
        std::env::set_var("K2_API", "1");
        k2_core::hire::apply::set_trash_override(Some(tmp.join("trash")));
        Self { prev_home, prev_api, tmp }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.prev_api {
            Some(v) => std::env::set_var("K2_API", v),
            None => std::env::remove_var("K2_API"),
        }
        k2_core::hire::apply::set_trash_override(None);
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hire_key_mints_hires_links_replays_and_retires_end_to_end() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;

    // The owner sets the hire root.
    let root = env.tmp.join("agents");
    let r = http(port, "POST", &owner_q("/cli/agents/hire-root"), None, None, Some(&json!({ "root": root })));
    assert_eq!(r.status, 200, "{}", r.body);
    let root = std::path::PathBuf::from(r.body["root"].as_str().unwrap());
    let r = http(port, "GET", &owner_q("/cli/agents/hire-root"), None, None, None);
    assert_eq!(r.body["default"], false, "{}", r.body);

    // Minting a hire key with only the owner token fails (HT5/T18).
    let mint = json!({ "label": "jobs-app", "hire": { "link": true, "retire": true, "folder": "acme", "maxMembersPerTeam": 4 } });
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&mint));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "vault_confirm_required");
    let confirm = k2_daemon::vault::get().expect("vault open").confirm.mint("hire-key-mint", None, "owner", "local-auth");
    let mut with_confirm = mint.clone();
    with_confirm["confirmToken"] = json!(confirm);
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&with_confirm));
    assert_eq!(r.status, 200, "{}", r.body);
    let key = r.body["key"].as_str().expect("raw key").to_string();
    assert_eq!(r.body["capabilities"]["hire"], true);
    assert_eq!(r.body["capabilities"]["hostSessions"], false);

    // The key hires a job lead.
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-lead"), Some(&json!({
        "agent": { "role": "joblead", "handle": "j1-lead", "name": "Job J1 Lead", "persona": "You run job J1." },
        "onBehalfOf": "alex@acme (job J1)",
    })));
    assert_eq!(r.status, 200, "{}", r.body);
    let lead = r.body["members"][0].clone();
    assert_eq!(lead["handle"], "j1-lead");
    assert!(lead["path"].as_str().unwrap().starts_with(root.join("acme").to_str().unwrap()), "{lead}");

    // …then a Desk team linked to it, with an Idempotency-Key.
    let desk = json!({
        "team": { "name": "Plans Desk J1", "topology": "star" },
        "members": [
            { "role": "librarian", "lead": true, "persona": "Index plan set ps-4471." },
            { "role": "structural" },
        ],
        "linkTo": [{ "workspace": lead["workspaceId"], "kind": "serves", "scope": { "planSets": ["ps-4471"] } }],
        "onBehalfOf": "alex@acme (job J1)",
    });
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-desk"), Some(&desk));
    assert_eq!(r.status, 200, "{}", r.body);
    let team = r.body.clone();
    assert_eq!(team["members"][0]["handle"], "plans-desk-j1-librarian");
    assert_eq!(team["links"][0]["kind"], "serves");
    let inst = team["instance"].as_str().unwrap().to_string();

    // Replay: same key + same body → the same instance, nothing new.
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-desk"), Some(&desk));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["changed"], false);
    assert_eq!(r.body["instance"], inst.as_str());

    // Method guards.
    assert_eq!(http(port, "GET", "/v1/agents/hire", Some(&key), None, None).status, 405);
    assert_eq!(http(port, "POST", "/v1/agents/instances", Some(&key), None, Some(&json!({}))).status, 405);

    // List own instances; links both ways.
    let r = http(port, "GET", "/v1/agents/instances", Some(&key), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["instances"].as_array().unwrap().len(), 2, "{}", r.body);
    let r = http(port, "GET", &format!("/v1/agent-teams/links?workspace={}", lead["workspaceId"].as_str().unwrap()), Some(&key), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["servedBy"][0]["teamName"], "Plans Desk J1", "{}", r.body);
    assert_eq!(r.body["servedBy"][0]["scope"]["planSets"][0], "ps-4471");

    // A key without the hire scope sees nothing.
    let (_, plain) = k2_core::api_keys::create_api_key("plain", None, Some("*"), None, None, None).expect("plain key");
    assert_eq!(http(port, "POST", "/v1/agents/hire", Some(&plain), None, Some(&json!({ "agent": {} }))).status, 404);
    assert_eq!(http(port, "GET", "/v1/agents/instances", Some(&plain), None, None).status, 404);

    // Teardown: unlink, delete the group (past the PoC guard), archive-retire.
    let r = http(port, "POST", "/v1/agents/retire", Some(&key), None, Some(&json!({ "instance": inst })));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["status"], "retired", "{}", r.body);
    assert_eq!(r.body["unlinked"].as_array().unwrap().len(), 1);
    for m in team["members"].as_array().unwrap() {
        let p = m["path"].as_str().unwrap();
        assert!(!std::path::Path::new(p).exists(), "archived (moved): {p}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE path = ?1", rusqlite::params![p], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "deregistered: {p}");
    }
    assert!(k2_core::paths::k2_home().join("archive").is_dir(), "retire archives under the (temp) home");
    let r = http(port, "GET", &format!("/v1/agents/instances?instance={inst}"), Some(&key), None, None);
    assert_eq!(r.body["instance"]["status"], "retired", "{}", r.body);
    let actions: Vec<&str> = r.body["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    for want in ["agent-hire", "agent-link", "agent-unlink", "agent-retire"] {
        assert!(actions.contains(&want), "{want} ledgered: {actions:?}");
    }

    // One auth-audit line per mint, hire, link, retire (HT19).
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit file");
    for want in ["hire-key-mint", "agent-hire", "agent-link", "agent-retire"] {
        assert!(audit.contains(&format!("\"event\":\"{want}\"")), "{want} audited:\n{audit}");
    }
    drop(env);
}
