//! LLM login wallet — headless daemon tests (`/cli/llm/accounts/*`).
//!
//! Real dispatcher (`test_harness::start`, in-process, ephemeral port,
//! temp `$HOME`). Every CLI is a shim under `K2_TEST_AGENT_SHIM_DIR` that
//! fakes its own login: prints a sign-in URL / device code / paste prompt,
//! then writes a fake credential carrying a marker into its home. No real
//! CLI, no keychain (temp HOME turns it off), no real `~/.claude`.
//!
//! Covers: boot import of the live login as the active Default; list
//! shape (no token, passports see no email); POST-only + 405; Member
//! floor; passports read-only (owner_only); the temp-home login (URL via
//! the BROWSER capture script, paste-code, slot 0600, live untouched, no
//! passport in the login env); switch saves the outgoing live login and
//! swaps in the target; next; rename; remove (active refused); Codex
//! device code; cancel discards a half-made login; the live-swap fallback
//! captures then restores; air-gap refuses sign-in but allows local
//! changes; secrets and pasted codes never appear in responses, the DB or
//! the audit log.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER: &str = "owner-token-llm-accounts";
const MARKER: &str = "K2TEST_SECRET_MARKER";
const PASTED: &str = "PASTEDCODE-7731";

struct TestEnv {
    home: PathBuf,
    log: PathBuf,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

const VARS: &[&str] = &[
    "HOME",
    "K2_TEST_AGENT_SHIM_DIR",
    "K2_AIRGAP",
    "K2_LLM_LOGIN_LIVE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GROK_HOME",
];

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

/// Shim bodies. Each records its env + argv, then fakes the CLI.
fn shim_script(log: &Path) -> String {
    format!(
        r#"#!/bin/sh
n=$(/usr/bin/basename "$0")
f="{log}/$n-$$"
printf 'HOOK=%s\nCELL=%s\nCLAUDE_CONFIG_DIR=%s\nCODEX_HOME=%s\nGROK_HOME=%s\nHOME=%s\n' "$K2_HOOK_TOKEN" "$K2_CELL" "$CLAUDE_CONFIG_DIR" "$CODEX_HOME" "$GROK_HOME" "$HOME" > "$f.env"
printf '%s\n' "$@" > "$f.argv"
case "$n" in
  claude)
    printf 'Opening browser to sign in...\n'
    "$BROWSER" "https://claude.example.test/oauth/authorize?state=s1"
    printf "If the browser didn't open, visit: https://claude.example.test/oauth/authorize?state=s1\n"
    printf 'Paste code here if prompted > '
    read code
    printf '%s' "$code" > "{log}/claude-received-code"
    d="$CLAUDE_CONFIG_DIR"
    printf '{{"claudeAiOauth":{{"accessToken":"{marker}-at-new","refreshToken":"{marker}-rt-new","expiresAt":4000000000000,"scopes":["user:inference"],"subscriptionType":"pro"}}}}' > "$d/.credentials.json"
    printf '{{"oauthAccount":{{"emailAddress":"second@example.test","organizationName":"Example Two"}}}}' > "$d/.claude.json"
    printf 'Login successful.\n'
    exit 0 ;;
  codex)
    printf 'Open https://auth.example.test/codex/device and enter this one-time code: ABCD-12345\n'
    /bin/sleep 2
    printf '{{"tokens":{{"refresh_token":"{marker}-codex-rt"}}}}' > "$CODEX_HOME/auth.json"
    exit 0 ;;
  grok)
    if [ -n "$GROK_HOME" ]; then exec /bin/cat; fi
    # Live-swap fallback: the normal login replaces the live store.
    /bin/sleep 1
    printf '{{"refresh":"{marker}-grok-new"}}' > "$HOME/.grok/auth.json"
    exit 0 ;;
esac
exec /bin/cat
"#,
        log = log.display(),
        marker = MARKER
    )
}

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = PathBuf::from(format!("/tmp/k2la-{:x}", nanos % 0xffff_ffff));
    let shim = home.join("shim");
    let log = home.join("shim-log");
    std::fs::create_dir_all(&shim).expect("shim dir");
    std::fs::create_dir_all(&log).expect("log dir");
    let body = shim_script(&log);
    for name in ["claude", "codex", "grok"] {
        let p = shim.join(name);
        std::fs::write(&p, &body).expect("shim");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let prev = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
    for v in VARS {
        std::env::remove_var(v);
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim);
    k2_core::airgap::set_setting_enabled(false);
    TestEnv { home, log, prev, _guard: guard }
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

fn poll_login(port: u16, login_id: &str, until: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let r = get(port, &format!("/cli/llm/accounts/login/status?loginId={login_id}"), OWNER);
        assert_eq!(r.status, 200, "login/status: {}", r.body);
        let v = js(&r)["login"].clone();
        if until(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "login never reached the state; last: {v}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tool_doc<'a>(list: &'a Value, tool: &str) -> &'a Value {
    list["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["tool"] == tool)
        .unwrap_or_else(|| panic!("no {tool} in {list}"))
}

fn account_by_label<'a>(list: &'a Value, tool: &str, label: &str) -> &'a Value {
    tool_doc(list, tool)["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|a| a["label"] == label)
        .unwrap_or_else(|| panic!("no {label} in {}", tool_doc(list, tool)))
}

fn db_dump() -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = String::new();
    for t in ["llm_accounts", "llm_active"] {
        let mut st = conn.prepare(&format!("SELECT * FROM {t}")).expect("prepare");
        let n = st.column_count();
        let mut rows = st.query([]).expect("query");
        while let Some(r) = rows.next().expect("row") {
            for i in 0..n {
                let v: rusqlite::types::Value = r.get(i).expect("col");
                out.push_str(&format!("{v:?}|"));
            }
            out.push('\n');
        }
    }
    out
}

fn reset_wallet_tables() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("DELETE FROM llm_accounts; DELETE FROM llm_active;").expect("reset");
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).expect("meta").permissions().mode() & 0o777
}

fn shim_env(env: &TestEnv, name: &str) -> String {
    let mut found = Vec::new();
    for e in std::fs::read_dir(&env.log).expect("log").flatten() {
        let f = e.file_name().to_string_lossy().to_string();
        if f.starts_with(&format!("{name}-")) && f.ends_with(".env") {
            found.push(std::fs::read_to_string(e.path()).expect("env"));
        }
    }
    assert_eq!(found.len(), 1, "exactly one {name} run: {found:?}");
    found.remove(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wallet_routes_end_to_end() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;

    // The live Claude login exists before K2 knows about it.
    let live = env.home.join(".claude/.credentials.json");
    std::fs::create_dir_all(live.parent().unwrap()).unwrap();
    let first = format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-1","refreshToken":"{MARKER}-rt-1","expiresAt":4000000000000,"subscriptionType":"max"}}}}"#);
    std::fs::write(&live, &first).unwrap();
    std::fs::write(env.home.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"first@example.test"}}"#).unwrap();
    let mut events = k2_daemon::session_events::subscribe();
    k2_daemon::llm_accounts_runtime::boot_import();

    // List: Default imported and active; 7 tools; no token anywhere.
    let r = get(port, "/cli/llm/accounts/list", OWNER);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(MARKER), "no token in list");
    let list = js(&r);
    assert_eq!(list["tools"].as_array().unwrap().len(), 7);
    assert_eq!(list["switchNote"], "Switching a login affects every session on this server.");
    let default = account_by_label(&list, "claude", "Default").clone();
    assert_eq!(default["active"], true);
    assert_eq!(default["email"], "first@example.test");
    assert_eq!(default["plan"], "max");
    assert_eq!(tool_doc(&list, "gemini")["supported"], false);
    assert_eq!(tool_doc(&list, "claude")["loginMethod"], "temp_home");
    let mut saw_event = false;
    while let Ok(ev) = events.try_recv() {
        if let k2_daemon::session_events::SessionEvent::LlmAccountsChanged { tool } = ev {
            assert_eq!(tool.as_deref(), Some("claude"));
            saw_event = true;
        }
    }
    assert!(saw_event, "boot import emits llm_accounts_changed");

    // POST-only.
    let r = req(port, "GET", &format!("/cli/llm/accounts/add?token={OWNER}"), None);
    assert_eq!(r.status, 405, "{}", r.body);
    let r = req(port, "GET", &format!("/cli/llm/accounts/switch?token={OWNER}"), None);
    assert_eq!(r.status, 405);

    // A passport (agent or K2 shell tab) reads, sees no email, can't change.
    let pp = passport();
    let r = get(port, "/cli/llm/accounts/list", &pp);
    assert_eq!(r.status, 200, "passport list: {}", r.body);
    assert_eq!(account_by_label(&js(&r), "claude", "Default")["email"], Value::Null);
    let r = get(port, "/cli/llm/accounts/usage", &pp);
    assert_eq!(r.status, 200, "passport usage: {}", r.body);
    let r = get(port, "/cli/llm/accounts/status?id=Default&tool=claude", &pp);
    assert_eq!(r.status, 200, "passport status: {}", r.body);
    for (path, body) in [
        ("/cli/llm/accounts/add", json!({"tool":"claude","label":"x"})),
        ("/cli/llm/accounts/switch", json!({"id": default["id"]})),
        ("/cli/llm/accounts/remove", json!({"id": default["id"]})),
        ("/cli/llm/accounts/rename", json!({"id": default["id"], "label":"y"})),
    ] {
        let r = post(port, path, &pp, body);
        assert_eq!(r.status, 403, "{path}: {}", r.body);
        assert_eq!(code(&r), "owner_only");
    }
    let r = get(port, "/cli/llm/accounts/list", "not-a-token");
    assert_eq!(r.status, 403);

    // Add a second Claude login as a Member (the /cli/agents/* floor).
    let member = member_login(port, "member1");
    let r = post(port, "/cli/llm/accounts/add", &member, json!({"tool":"claude","label":"Work","mode":"other_device"}));
    assert_eq!(r.status, 200, "add: {}", r.body);
    let v = js(&r);
    let login_id = v["login"]["loginId"].as_str().expect("loginId").to_string();
    let work_id = v["account"]["id"].as_str().expect("id").to_string();
    assert_eq!(v["login"]["method"], "temp_home");
    assert_eq!(v["login"]["accountId"], work_id.as_str());
    // A second add for the same label is a duplicate.
    let r = post(port, "/cli/llm/accounts/add", OWNER, json!({"tool":"claude","label":"work"}));
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(code(&r), "duplicate_label");

    // The owner may watch; another Member may not see URL/code.
    let st = poll_login(port, &login_id, |l| l["state"] == "waiting_for_code");
    assert_eq!(st["url"], "https://claude.example.test/oauth/authorize?state=s1");
    let other = member_login(port, "member2");
    let r = get(port, &format!("/cli/llm/accounts/login/status?loginId={login_id}"), &other);
    assert_eq!(r.status, 200);
    assert_eq!(js(&r)["login"]["url"], Value::Null, "only the starter sees the URL");
    let r = post(port, "/cli/llm/accounts/login/input", &other, json!({"loginId": login_id, "text": "nope"}));
    assert_eq!(r.status, 403, "{}", r.body);
    // Passports never reach a login terminal.
    let r = get(port, &format!("/cli/llm/accounts/login/status?loginId={login_id}"), &pp);
    assert_eq!(r.status, 403);

    // The starter pastes the code.
    let r = post(port, "/cli/llm/accounts/login/input", &member, json!({"loginId": login_id, "text": PASTED}));
    assert_eq!(r.status, 200, "{}", r.body);
    let st = poll_login(port, &login_id, |l| l["done"] == true);
    assert_eq!(st["state"], "signed_in", "{st}");
    assert_eq!(st["offerMakeActive"], true);
    assert_eq!(std::fs::read_to_string(env.log.join("claude-received-code")).unwrap(), PASTED);

    // Slot written 0600 under ~/.k2/llm-accounts; live login untouched.
    let slot = env.home.join(format!(".k2/llm-accounts/claude/{work_id}"));
    assert_eq!(mode(&slot.join(".credentials.json")), 0o600);
    assert_eq!(mode(&slot), 0o700);
    assert_eq!(std::fs::read_to_string(&live).unwrap(), first, "the active login is never touched by a sign-in");
    let senv = shim_env(&env, "claude");
    assert!(senv.contains(&format!("CLAUDE_CONFIG_DIR={}", slot.display())), "{senv}");
    assert!(senv.contains("HOOK=\n"), "no passport in a login terminal: {senv}");
    assert!(senv.contains("CELL=login\n"), "{senv}");
    assert!(senv.contains(&format!("HOME={}\n", env.home.display())), "HOME unchanged: {senv}");

    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let work = account_by_label(&list, "claude", "Work");
    assert_eq!(work["active"], false);
    assert_eq!(work["state"], "signed_in");
    assert_eq!(work["email"], "second@example.test");
    assert_eq!(work["plan"], "pro");

    // The CLI refreshed the active login since import.
    let refreshed = format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-1b","refreshToken":"{MARKER}-rt-1b","expiresAt":4000000000000}}}}"#);
    std::fs::write(&live, &refreshed).unwrap();

    // Switch: outgoing saved back, target swapped in.
    let r = post(port, "/cli/llm/accounts/switch", &member, json!({"id": work_id}));
    assert_eq!(r.status, 200, "switch: {}", r.body);
    assert!(!r.body.contains(MARKER));
    let v = js(&r);
    assert_eq!(v["switch"]["savedOutgoing"], true);
    assert_eq!(v["account"]["active"], true);
    let live_now = std::fs::read_to_string(&live).unwrap();
    assert!(live_now.contains("-at-new"), "target swapped in: {live_now}");
    let default_id = default["id"].as_str().unwrap();
    let default_slot = env.home.join(format!(".k2/llm-accounts/claude/{default_id}/.credentials.json"));
    assert_eq!(std::fs::read_to_string(&default_slot).unwrap(), refreshed, "outgoing saved back");

    // Remove the active one → refused. Next → back to Default.
    let r = post(port, "/cli/llm/accounts/remove", OWNER, json!({"id": work_id}));
    assert_eq!(r.status, 409);
    assert_eq!(code(&r), "active_login");
    let r = post(port, "/cli/llm/accounts/next", OWNER, json!({"tool":"claude"}));
    assert_eq!(r.status, 200, "next: {}", r.body);
    assert_eq!(js(&r)["switch"]["to"], default_id);
    assert_eq!(std::fs::read_to_string(&live).unwrap(), refreshed);
    let r = post(port, "/cli/llm/accounts/next", OWNER, json!({"tool":"codex"}));
    assert_eq!(r.status, 404);
    assert_eq!(code(&r), "no_next");

    // Rename, then remove the idle one (Trash, never expunge).
    let r = post(port, "/cli/llm/accounts/rename", OWNER, json!({"id": work_id, "label":"Second"}));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(js(&r)["account"]["label"], "Second");
    let r = post(port, "/cli/llm/accounts/remove", OWNER, json!({"id": work_id}));
    assert_eq!(r.status, 200, "{}", r.body);
    let moved = PathBuf::from(js(&r)["removed"]["movedTo"].as_str().unwrap());
    assert!(moved.starts_with(env.home.join(".k2/llm-accounts/.removed")));
    assert!(moved.join(".credentials.json").exists());

    // Refresh re-reads state.
    let r = post(port, "/cli/llm/accounts/refresh", OWNER, json!({}));
    assert_eq!(r.status, 200, "{}", r.body);

    // Nothing secret anywhere.
    assert!(!db_dump().contains(MARKER), "no token in the DB");
    let audit = std::fs::read_to_string(env.home.join(".k2/auth-audit.jsonl")).unwrap_or_default();
    assert!(!audit.contains(MARKER) && !audit.contains(PASTED), "audit stays clean");
    assert!(audit.contains("llm_accounts.switch"), "switch is audited: {audit}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_device_code_and_cancel_discards_a_half_made_login() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;

    let r = post(port, "/cli/llm/accounts/add", OWNER, json!({"tool":"codex","label":"Team","mode":"other_device"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = js(&r);
    let login_id = v["login"]["loginId"].as_str().unwrap().to_string();
    let st = poll_login(port, &login_id, |l| l["code"].is_string());
    assert_eq!(st["code"], "ABCD-12345");
    assert_eq!(st["url"], "https://auth.example.test/codex/device");
    assert_eq!(st["state"], "waiting_for_browser");
    let st = poll_login(port, &login_id, |l| l["done"] == true);
    assert_eq!(st["state"], "signed_in", "{st}");
    let argv = std::fs::read_dir(&env.log)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("codex-") && e.file_name().to_string_lossy().ends_with(".argv"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .expect("codex argv");
    assert!(argv.contains("--device-auth"), "{argv}");
    assert!(argv.contains("cli_auth_credentials_store=\"file\""), "file store, never the keyring: {argv}");
    // Nothing was active and the live store was empty → it became active.
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    assert_eq!(account_by_label(&list, "codex", "Team")["active"], true);
    assert!(env.home.join(".codex/auth.json").exists());

    // A grok sign-in that never finishes → cancel → the half-made login is gone.
    let r = post(port, "/cli/llm/accounts/add", OWNER, json!({"tool":"grok","label":"Hanging"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = js(&r);
    let login_id = v["login"]["loginId"].as_str().unwrap().to_string();
    let gid = v["account"]["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/llm/accounts/login/cancel", OWNER, json!({"loginId": login_id}));
    assert_eq!(r.status, 200, "{}", r.body);
    let st = poll_login(port, &login_id, |l| l["done"] == true);
    assert_eq!(st["state"], "cancelled");
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    assert!(tool_doc(&list, "grok")["accounts"].as_array().unwrap().is_empty(), "{list}");
    assert!(!env.home.join(format!(".k2/llm-accounts/grok/{gid}")).exists());
    let gargv = std::fs::read_dir(&env.log)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("grok-") && e.file_name().to_string_lossy().ends_with(".argv"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .expect("grok argv");
    assert!(gargv.contains("--leader-socket"), "isolated leader: {gargv}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_swap_fallback_captures_then_restores_the_previous_login() {
    let env = setup();
    std::env::set_var("K2_LLM_LOGIN_LIVE", "grok");
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;
    let live = env.home.join(".grok/auth.json");
    std::fs::create_dir_all(live.parent().unwrap()).unwrap();
    let before = format!(r#"{{"refresh":"{MARKER}-grok-old"}}"#);
    std::fs::write(&live, &before).unwrap();

    let r = post(port, "/cli/llm/accounts/add", OWNER, json!({"tool":"grok","label":"Second"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = js(&r);
    assert_eq!(v["login"]["method"], "live_swap");
    assert_eq!(
        v["login"]["banner"],
        "Signing in temporarily switches this tool for every session on this server."
    );
    let new_id = v["account"]["id"].as_str().unwrap().to_string();
    let login_id = v["login"]["loginId"].as_str().unwrap().to_string();
    let st = poll_login(port, &login_id, |l| l["done"] == true);
    assert_eq!(st["state"], "signed_in", "{st}");
    assert_eq!(st["offerMakeActive"], true);
    assert_eq!(std::fs::read_to_string(&live).unwrap(), before, "previous login restored");
    let slot = env.home.join(format!(".k2/llm-accounts/grok/{new_id}/auth.json"));
    assert!(std::fs::read_to_string(&slot).unwrap().contains("grok-new"));
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    assert_eq!(account_by_label(&list, "grok", "Default")["active"], true, "nobody moved");
    assert_eq!(account_by_label(&list, "grok", "Second")["active"], false);
    std::env::remove_var("K2_LLM_LOGIN_LIVE");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn airgap_refuses_sign_in_but_allows_local_changes() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;
    let live = env.home.join(".claude/.credentials.json");
    std::fs::create_dir_all(live.parent().unwrap()).unwrap();
    std::fs::write(&live, format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}"}}}}"#)).unwrap();
    k2_daemon::llm_accounts_runtime::boot_import();
    std::env::set_var("K2_AIRGAP", "1");
    let r = post(port, "/cli/llm/accounts/add", OWNER, json!({"tool":"claude","label":"x"}));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(code(&r), "airgap");
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    assert_eq!(list["airgap"], true);
    let id = account_by_label(&list, "claude", "Default")["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/llm/accounts/rename", OWNER, json!({"id": id, "label": "Main"}));
    assert_eq!(r.status, 200, "{}", r.body);
    // Keep-warm does nothing under air-gap.
    assert!(k2_daemon::llm_accounts_runtime::keep_warm_tick().is_empty());
    // The retired active-login refresher refuses.
    let r = req(port, "POST", &format!("/cli/claude-auth/refresh-now?token={OWNER}"), Some("{}"));
    assert_eq!(r.status, 410, "{}", r.body);
    std::env::remove_var("K2_AIRGAP");
    assert!(!env.home.join(".k2/llm-accounts/.login").read_dir().map(|mut d| d.next().is_some()).unwrap_or(false));
}
