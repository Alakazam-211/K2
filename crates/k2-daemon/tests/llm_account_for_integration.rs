//! `GET /cli/llm/accounts/for` + `POST /cli/llm/accounts/for/refresh` —
//! headless daemon tests (the phone's chat-header chip, Companion
//! 2026-10-24).
//!
//! Real dispatcher (`test_harness::start`, ephemeral port, temp `$HOME`);
//! every CLI is a `/bin/cat` shim under `K2_TEST_AGENT_SHIM_DIR`. No real
//! CLI, keychain, `~/.k2` or vendor call (temp HOME turns vendor calls off).
//!
//! Covers: the answer for the workspace's pinned chat and for a sidecar
//! (`ws/N`) and by `?conversation=`; source `default` (with the SAME
//! maxPercent as the desktop chip's `/cli/usage/subscriptions` row),
//! `pinned` (workspace pin, the sidecar's own chat pin, a sidecar with no
//! pin of its own falling back to the workspace's), `resumed` (beats a
//! newer workspace pin); a running chat keeps the token it started on and
//! names `nextStart`; an API token has no number (`windows: []`,
//! `maxPercent: null`); refresh is POST-only, people-only and rate-limited
//! per token; `pinnedTo` carries ids; the boot-status feature key; no
//! secret in any answer.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER: &str = "owner-token-llm-account-for";
const MARKER: &str = "K2TEST_SECRET_MARKER";
const KEY: &str = "sk-test-K2TEST_SECRET_MARKER-apikey-0002";

const VARS: &[&str] = &[
    "HOME",
    "PATH",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "K2_TEST_AGENT_SHIM_DIR",
    "K2_AIRGAP",
    "K2_LLM_LOGIN_LIVE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GROK_HOME",
];

struct TestEnv {
    home: PathBuf,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (k, v) in self.prev.drain(..) {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = PathBuf::from(format!("/tmp/k2laf-{:x}", nanos % 0xffff_ffff));
    let shim = home.join("shim");
    std::fs::create_dir_all(&shim).expect("shim dir");
    for name in ["claude", "codex", "grok"] {
        let p = shim.join(name);
        k2_core::test_env::write_executable(&p, "#!/bin/sh\nexec /bin/cat\n");
    }
    let prev = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
    for v in VARS {
        std::env::remove_var(v);
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", shim.display()));
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim);
    k2_core::airgap::set_setting_enabled(false);
    TestEnv { home, prev, _guard: guard }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

struct Resp {
    status: u16,
    body: String,
}

fn req(port: u16, method: &str, pq: &str, body: Option<&str>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).expect("write");
    let mut buf = Vec::new();
    // A reset after the full response is fine; a missing status line panics below.
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn js(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {}", r.body))
}

fn get(port: u16, path: &str, token: &str) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    req(port, "GET", &format!("{path}{sep}token={token}"), None)
}

fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    req(port, "POST", &format!("{path}?token={token}"), Some(&body.to_string()))
}

fn code(r: &Resp) -> String {
    js(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("error.code: {}", r.body)).to_string()
}

fn passport() -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: "ws-test".into(), agent_address: "tester".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn member_login(port: u16, username: &str) -> String {
    let r = req(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER}"),
        Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)),
    );
    assert_eq!(r.status, 200, "users/add: {}", r.body);
    let r = req(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)),
    );
    assert_eq!(r.status, 200, "login: {}", r.body);
    js(&r)["token"].as_str().expect("token").to_string()
}

fn reset_wallet_tables() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch(
        "DELETE FROM llm_accounts; DELETE FROM llm_active; DELETE FROM llm_account_pins; DELETE FROM llm_session_logins;",
    )
    .expect("reset");
}

fn seed_project(env: &TestEnv, handle: &str) -> (String, PathBuf) {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).expect("ws dir");
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    (id, path)
}

/// Door B: the daemon-internal spawn (the pinned chat's key is its id).
fn spawn_b(project_id: &str, cwd: &Path, key: &str, args: &[&str]) {
    k2_daemon::spawn::spawn_agent_session_v2_blocking(k2_daemon::spawn::SpawnWorkspaceSessionRequest {
        agent_name: key.to_string(),
        project_id: Some(project_id.to_string()),
        cwd: cwd.to_string_lossy().into_owned(),
        command: Some("claude".to_string()),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        cols: 80,
        rows: 24,
        canonical_key: Some(key.to_string()),
        env: std::collections::HashMap::new(),
        env_owner_set: false,
        launch_prompt: None,
        label: None,
    })
    .expect("spawn");
}

/// Door A: `/cli/sessions/v2/spawn` (an extra chat tab = a sidecar).
fn spawn_a(port: u16, key: &str, cwd: &Path, args: &[&str]) {
    let r = post(
        port,
        "/cli/sessions/v2/spawn",
        OWNER,
        json!({"agent_name": key, "cwd": cwd.to_string_lossy(), "command": "claude", "args": args, "cols": 80, "rows": 24}),
    );
    assert_eq!(r.status, 200, "v2 spawn {key}: {}", r.body);
}

fn kill_sessions_under(home: &Path) {
    for (key, s) in k2_daemon::v2_session_map::snapshot() {
        if s.cwd.as_ref().is_some_and(|p| p.starts_with(home)) {
            k2_daemon::v2_session_map::unregister(&key);
            s.kill();
        }
    }
}

fn for_addr(port: u16, addr: &str) -> Value {
    let r = get(port, &format!("/cli/llm/accounts/for?addr={addr}"), OWNER);
    assert_eq!(r.status, 200, "for {addr}: {}", r.body);
    assert!(!r.body.contains(MARKER), "for {addr} leaks a secret");
    js(&r)
}

/// A Claude row of the shared cache (the desktop chip's reading).
fn write_shared_cache(account_id: &str) {
    let path = k2_daemon::subscription_usage::cache_path();
    std::fs::create_dir_all(path.parent().expect("cache dir")).expect("mkdir usage");
    let doc = json!({"harnesses": [{
        "harness": "claude", "plan": "Max", "checkedAt": "2026-10-24T11:58:00Z", "status": "",
        "accountId": account_id,
        "windows": [
            {"label": "Session", "used": 0.41496, "resetsAt": "2099-01-01T05:00:00Z", "id": "five_hour", "kind": "session"},
            {"label": "Weekly", "used": 0.2, "resetsAt": "2099-01-07T00:00:00Z", "id": "seven_day", "kind": "weekly"},
            {"label": "Fable Weekly", "used": 0.99, "resetsAt": "2099-01-07T00:00:00Z", "id": "model:fable:weekly", "kind": "weekly"}
        ]
    }]});
    std::fs::write(&path, doc.to_string()).expect("write cache");
}

fn sidecar_ordinal(project_id: &str, conversation: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT ordinal FROM workspace_session_handles WHERE project_id = ?1 AND conversation_key = ?2",
        rusqlite::params![project_id, conversation],
        |r| r.get(0),
    )
    .unwrap_or_else(|e| panic!("no sidecar handle for {conversation}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_reports_the_token_each_chat_runs_on_with_the_chip_number() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;
    let (pid, ws) = seed_project(&env, "forws");

    // The live Default + a second signed-in Claude subscription with its own reading.
    std::fs::create_dir_all(env.home.join(".claude")).expect("claude dir");
    std::fs::write(env.home.join(".claude/.credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-live"}}}}"#))
        .expect("live cred");
    k2_daemon::llm_accounts_runtime::boot_import();
    let (default_id, second_id) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let default_id = k2_core::llm_accounts::active_id(&conn, k2_core::llm_accounts::Tool::Claude)
            .expect("active")
            .expect("boot import made the live login the default");
        let e = k2_core::llm_accounts::wallet::begin_new(&conn, k2_core::llm_accounts::Tool::Claude, "Second", None).expect("begin");
        let slot = env.home.join(format!(".k2/llm-accounts/claude/{}", e.id));
        std::fs::write(slot.join(".credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-second"}}}}"#))
            .expect("slot cred");
        let e = k2_core::llm_accounts::wallet::finalize_login(&conn, &e.id, None).expect("finalize");
        let reading = json!({"harness": "claude", "plan": "Max", "checkedAt": "2026-10-24T11:00:00Z", "status": "",
            "windows": [{"label": "Session", "used": 0.73, "resetsAt": "2099-01-01T05:00:00Z"}]});
        k2_core::llm_accounts::update_meta(
            &conn,
            &e.id,
            &k2_core::llm_accounts::MetaUpdate { usage_json: Some(reading.to_string()), ..Default::default() },
        )
        .expect("second's reading");
        (default_id, e.id)
    };
    write_shared_cache(&default_id);

    // The desktop chip's number for the default token.
    let r = get(port, "/cli/usage/subscriptions", OWNER);
    assert_eq!(r.status, 200, "{}", r.body);
    let chip_doc = js(&r);
    let chip_row = chip_doc["harnesses"].as_array().expect("harnesses").iter().find(|h| h["harness"] == "claude").expect("claude row").clone();
    assert_eq!(chip_row["maxPercent"], 42, "Session 41.5 → 42; Fable Weekly never drives the chip: {chip_row}");
    let chips: Vec<bool> = chip_row["windows"].as_array().expect("windows").iter().map(|w| w["chip"].as_bool().expect("chip flag")).collect();
    assert_eq!(chips, vec![true, true, false]);

    // ── The pinned chat, running on the server default ────────────────
    spawn_b(&pid, &ws, &pid, &["--session-id", "conv-c1"]);
    let v = for_addr(port, "forws");
    assert_eq!(v["tool"], "claude", "{v}");
    assert_eq!(v["source"], "default", "{v}");
    assert_eq!(v["accountId"], default_id.as_str());
    assert_eq!(v["label"], "Default");
    assert_eq!(v["live"], true);
    assert_eq!(v["sidecar"], false);
    assert_eq!(v["address"], "forws");
    assert_eq!(v["workspaceId"], pid.as_str());
    assert_eq!(v["conversationId"], "conv-c1");
    assert_eq!(v["maxPercent"], chip_row["maxPercent"], "the chat on the default token shows exactly the chip's number");
    assert_eq!(v["windows"], chip_row["windows"], "same windows as the chip's row");
    assert_eq!(v["updatedAt"], "2026-10-24T11:58:00Z");
    assert!(v.get("nextStart").is_none(), "{v}");

    // A workspace pin made while it runs: the running chat keeps its token;
    // nextStart names the pin.
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope": "workspace", "workspace": "forws", "id": second_id}));
    assert_eq!(r.status, 200, "pin: {}", r.body);
    let v = for_addr(port, "forws");
    assert_eq!((v["source"].as_str(), v["accountId"].as_str()), (Some("default"), Some(default_id.as_str())), "{v}");
    assert_eq!(v["nextStart"], json!({"accountId": second_id, "source": "pinned"}), "{v}");
    assert_eq!(v["maxPercent"], 42);

    // pinnedTo carries the workspace id.
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let tool = list["tools"].as_array().expect("tools").iter().find(|t| t["tool"] == "claude").expect("claude").clone();
    let second_row = tool["accounts"].as_array().expect("accounts").iter().find(|a| a["id"] == second_id.as_str()).expect("second").clone();
    assert_eq!(second_row["pinnedTo"][0]["scopeKind"], "workspace", "{second_row}");
    assert_eq!(second_row["pinnedTo"][0]["workspaceId"], pid.as_str(), "{second_row}");
    assert_eq!(second_row["pinnedTo"][0]["conversationId"], Value::Null);

    // Restarted fresh: it runs on the pin, with that token's OWN reading.
    kill_sessions_under(&env.home);
    spawn_b(&pid, &ws, &pid, &["--session-id", "conv-c2"]);
    let v = for_addr(port, "forws");
    assert_eq!(v["source"], "pinned", "{v}");
    assert_eq!(v["pinScope"], "workspace");
    assert_eq!(v["accountId"], second_id.as_str());
    assert_eq!(v["label"], "Second");
    assert_eq!(v["kind"], "subscription");
    assert_eq!(v["maxPercent"], 73, "{v}");
    assert_eq!(v["updatedAt"], "2026-10-24T11:00:00Z");
    assert_eq!(v["usageFrom"], "token");

    // ── A sidecar with no pin of its own: the workspace's pin ──────────
    spawn_a(port, "tab-side1", &ws, &["--session-id", "conv-s1"]);
    let n = sidecar_ordinal(&pid, "conv-s1");
    let side_addr = format!("forws/{n}");
    let v = for_addr(port, &side_addr);
    assert_eq!(v["sidecar"], true, "{v}");
    assert_eq!(v["address"], side_addr.as_str());
    assert_eq!(v["sessionKey"], "tab-side1");
    assert_eq!((v["source"].as_str(), v["pinScope"].as_str()), (Some("pinned"), Some("workspace")), "{v}");
    assert_eq!(v["accountId"], second_id.as_str());

    // Its own chat pin (an API token) while it runs: nextStart.
    let r = post(port, "/cli/llm/accounts/add-key", OWNER, json!({"tool": "claude", "label": "Metered", "key": KEY}));
    assert_eq!(r.status, 200, "add-key: {}", r.body);
    let key_id = js(&r)["account"]["id"].as_str().expect("key id").to_string();
    let r = post(
        port,
        "/cli/llm/accounts/pin",
        OWNER,
        json!({"scope": "session", "scopeId": "tab-side1", "conversationId": "conv-s1", "tool": "claude", "id": key_id}),
    );
    assert_eq!(r.status, 200, "chat pin: {}", r.body);
    let v = for_addr(port, &side_addr);
    assert_eq!(v["accountId"], second_id.as_str(), "the running sidecar keeps its token: {v}");
    assert_eq!(v["nextStart"], json!({"accountId": key_id, "source": "pinned"}), "{v}");

    // pinnedTo ids for a chat pin: the session key and the conversation.
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let tool = list["tools"].as_array().expect("tools").iter().find(|t| t["tool"] == "claude").expect("claude").clone();
    let key_row = tool["accounts"].as_array().expect("accounts").iter().find(|a| a["id"] == key_id.as_str()).expect("key").clone();
    let pins = key_row["pinnedTo"].as_array().expect("pinnedTo").clone();
    let by_key = pins.iter().find(|p| p["scopeId"] == "tab-side1").unwrap_or_else(|| panic!("tab pin: {key_row}"));
    assert_eq!(by_key["sessionKey"], "tab-side1");
    assert_eq!(by_key["workspaceId"], pid.as_str());
    let by_conv = pins.iter().find(|p| p["scopeId"] == "conversation:conv-s1").unwrap_or_else(|| panic!("conv pin: {key_row}"));
    assert_eq!(by_conv["conversationId"], "conv-s1");
    assert_eq!(by_conv["workspaceId"], pid.as_str());

    // ── Nothing running: the next start decides ──────────────────────
    kill_sessions_under(&env.home);
    // The pinned chat's saved conversation (as a stop leaves it), and the
    // workspace re-pinned to the API token: a resume keeps the token the
    // conversation started on.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceSession::upsert(&conn, "wss-forws", &pid, None, Some("conv-c2"), "claude", "owner", "sleeping")
            .expect("workspace session");
    }
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope": "workspace", "workspace": "forws", "id": key_id}));
    assert_eq!(r.status, 200, "re-pin: {}", r.body);
    let v = for_addr(port, "forws");
    assert_eq!(v["live"], false, "{v}");
    assert_eq!(v["source"], "resumed", "{v}");
    assert_eq!(v["accountId"], second_id.as_str());
    assert_eq!(v["maxPercent"], 73);

    // The sidecar resumes on ITS chat pin: an API token has no number.
    let v = for_addr(port, &side_addr);
    assert_eq!((v["source"].as_str(), v["pinScope"].as_str()), (Some("pinned"), Some("chat")), "{v}");
    assert_eq!(v["accountId"], key_id.as_str());
    assert_eq!(v["kind"], "api_key");
    assert_eq!(v["windows"], json!([]));
    assert_eq!(v["maxPercent"], Value::Null);
    assert_eq!(v["updatedAt"], Value::Null);
    // The same chat by its conversation id.
    let r = get(port, "/cli/llm/accounts/for?conversation=conv-s1", OWNER);
    assert_eq!(r.status, 200, "{}", r.body);
    let c = js(&r);
    assert_eq!((c["sessionKey"].as_str(), c["accountId"].as_str()), (Some("tab-side1"), Some(key_id.as_str())), "{c}");

    // Its chat pin removed: the token its conversation started on.
    let r = post(port, "/cli/llm/accounts/unpin", OWNER, json!({"scope": "session", "scopeId": "tab-side1", "conversationId": "conv-s1", "tool": "claude"}));
    assert_eq!(js(&r)["unpinned"], true, "{}", r.body);
    let v = for_addr(port, &side_addr);
    assert_eq!((v["source"].as_str(), v["accountId"].as_str()), (Some("resumed"), Some(second_id.as_str())), "{v}");

    // ── Errors and callers ────────────────────────────────────────────
    let r = get(port, "/cli/llm/accounts/for", OWNER);
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = get(port, "/cli/llm/accounts/for?addr=nosuchagent", OWNER);
    assert_eq!((r.status, code(&r)), (404, "not_found".to_string()), "{}", r.body);
    let r = get(port, "/cli/llm/accounts/for?addr=forws/99", OWNER);
    assert_eq!(r.status, 404, "{}", r.body);
    let r = get(port, "/cli/llm/accounts/for?addr=forws", &passport());
    assert_eq!(r.status, 200, "passports read like the list: {}", r.body);
    let member = member_login(port, "phoneuser");
    let r = get(port, "/cli/llm/accounts/for?addr=forws", &member);
    assert_eq!(r.status, 200, "a Connect login (the phone) reads it: {}", r.body);
    assert_eq!(js(&r)["maxPercent"], 73);
    let r = get(port, "/cli/llm/accounts/for?addr=forws", "wrong-token");
    assert!(matches!(r.status, 401 | 403), "a bad token is refused: {} {}", r.status, r.body);
    assert!(!r.body.contains("forws") && !r.body.contains(&second_id), "{}", r.body);

    // ── Refresh: POST only, people only, once a minute per token ──────
    let r = req(port, "GET", &format!("/cli/llm/accounts/for/refresh?token={OWNER}&addr=forws"), None);
    assert_eq!(r.status, 405, "{}", r.body);
    let r = post(port, "/cli/llm/accounts/for/refresh", &passport(), json!({"addr": "forws"}));
    assert_eq!((r.status, code(&r)), (403, "owner_only".to_string()), "{}", r.body);
    let r = post(port, "/cli/llm/accounts/for/refresh", OWNER, json!({"addr": "forws"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = js(&r);
    // Tests never call a vendor (temp HOME): the slot is taken, no probe.
    assert_eq!((v["refreshed"].as_bool(), v["reason"].as_str()), (Some(false), Some("probes_off")), "{v}");
    assert_eq!(v["accountId"], second_id.as_str());
    assert_eq!(v["maxPercent"], 73, "the cached reading comes back");
    let r = post(port, "/cli/llm/accounts/for/refresh", &member, json!({"addr": "forws"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = js(&r);
    assert_eq!((v["refreshed"].as_bool(), v["reason"].as_str()), (Some(false), Some("rate_limited")), "{v}");
    let wait = v["retryAfterSecs"].as_u64().unwrap_or_else(|| panic!("retryAfterSecs: {v}"));
    assert!((1..=60).contains(&wait), "{v}");
    assert_eq!(v["maxPercent"], 73);
    let r = post(port, "/cli/llm/accounts/for/refresh", OWNER, json!({}));
    assert_eq!(r.status, 400, "{}", r.body);

    kill_sessions_under(&env.home);
    drop(d);
}

#[test]
fn boot_status_lists_llm_account_for() {
    assert!(
        k2_daemon::boot_status::features().contains(&"llm-account-for-v1"),
        "{:?}",
        k2_daemon::boot_status::features()
    );
}
