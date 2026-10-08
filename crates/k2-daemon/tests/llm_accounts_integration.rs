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
printf 'HOOK=%s\nCELL=%s\nCLAUDE_CONFIG_DIR=%s\nCODEX_HOME=%s\nGROK_HOME=%s\nHOME=%s\nKEYSET=%s\n' "$K2_HOOK_TOKEN" "$K2_CELL" "$CLAUDE_CONFIG_DIR" "$CODEX_HOME" "$GROK_HOME" "$HOME" "${{ANTHROPIC_API_KEY:+yes}}${{CODEX_API_KEY:+yes}}" > "$f.env"
printf '%s\n' "$@" > "$f.argv"
case "$n" in
  claude)
    if [ "$1" != "auth" ]; then exec /bin/cat; fi
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
    if [ "$1" != "login" ]; then exec /bin/cat; fi
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
    conn.execute_batch(
        "DELETE FROM llm_accounts; DELETE FROM llm_active; DELETE FROM llm_account_pins; DELETE FROM llm_session_logins;",
    )
    .expect("reset");
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
    assert_eq!(
        list["switchNote"],
        "Changing the server default token affects every chat that uses Server default on this server."
    );
    let default = account_by_label(&list, "claude", "Default").clone();
    assert_eq!(default["active"], true);
    assert_eq!(default["email"], "first@example.test");
    assert_eq!(default["plan"], "max");
    assert_eq!(default["kind"], "subscription");
    assert_eq!(tool_doc(&list, "gemini")["supported"], true);
    assert_eq!(tool_doc(&list, "gemini")["subscription"], false, "Gemini: API keys only");
    assert_eq!(tool_doc(&list, "cursor")["supported"], false);
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

    // A Connect login is how a desktop window on ANOTHER computer reads this
    // server (Settings → LLMs per server, 0.45.1): it gets this server's
    // tokens — the same rows the owner token sees — and never a filesystem
    // path of this server.
    let owner_list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let r = get(port, "/cli/llm/accounts/list", &member);
    assert_eq!(r.status, 200, "member list: {}", r.body);
    let member_list = js(&r);
    let ids = |l: &Value| -> Vec<String> {
        tool_doc(l, "claude")["accounts"]
            .as_array()
            .expect("claude accounts")
            .iter()
            .map(|a| a["id"].as_str().expect("id").to_string())
            .collect()
    };
    assert_eq!(ids(&member_list), ids(&owner_list), "a login sees the server's tokens");
    assert_eq!(ids(&member_list).len(), 2, "Default + Work: {}", r.body);
    assert_eq!(tool_doc(&member_list, "claude")["activeId"], work_id.as_str());
    let home_str = env.home.display().to_string();
    assert!(!r.body.contains(&home_str), "no server path in the list: {}", r.body);
    assert!(!r.body.contains("llm-accounts/"), "no slot path in the list: {}", r.body);
    assert!(!r.body.contains(MARKER), "no token in the list");

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

// ── Pins at the spawn doors ─────────────────────────────────────────

const KEY: &str = "sk-test-K2TEST_SECRET_MARKER-apikey-0001";

fn seed_project(env: &TestEnv, handle: &str) -> (String, PathBuf) {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
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

/// The env file of the one shim run whose argv contains `needle`.
fn run_env(env: &TestEnv, name: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut hits = Vec::new();
        for e in std::fs::read_dir(&env.log).expect("log").flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if f.starts_with(&format!("{name}-")) && f.ends_with(".argv") {
                let argv = std::fs::read_to_string(e.path()).unwrap_or_default();
                if argv.lines().any(|l| l == needle) {
                    let envp = e.path().with_extension("env");
                    hits.push(std::fs::read_to_string(envp).expect("env file"));
                }
            }
        }
        if hits.len() == 1 {
            return hits.remove(0);
        }
        assert!(hits.len() < 2, "more than one {name} run with {needle}");
        assert!(Instant::now() < deadline, "no {name} run with {needle}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_b(project_id: &str, cwd: &Path, key: &str, command: &str, args: &[&str]) {
    k2_daemon::spawn::spawn_agent_session_v2_blocking(k2_daemon::spawn::SpawnWorkspaceSessionRequest {
        agent_name: key.to_string(),
        project_id: Some(project_id.to_string()),
        cwd: cwd.to_string_lossy().into_owned(),
        command: Some(command.to_string()),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        cols: 80,
        rows: 24,
        canonical_key: Some(key.to_string()),
        env: std::collections::HashMap::new(),
        launch_prompt: None,
        label: None,
    })
    .expect("spawn");
}

fn kill_sessions_under(home: &Path) {
    for (key, s) in k2_daemon::v2_session_map::snapshot() {
        if s.cwd.as_ref().is_some_and(|p| p.starts_with(home)) {
            k2_daemon::v2_session_map::unregister(&key);
            s.kill();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinned_sessions_run_on_their_login_and_resume_keeps_the_recorded_home() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;
    let (pid, ws) = seed_project(&env, "pinws");

    // Live Default + a second signed-in Claude login.
    std::fs::create_dir_all(env.home.join(".claude")).unwrap();
    std::fs::write(env.home.join(".claude/.credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-live"}}}}"#)).unwrap();
    std::fs::write(env.home.join(".claude/settings.json"), "{}").unwrap();
    k2_daemon::llm_accounts_runtime::boot_import();
    let second = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let e = k2_core::llm_accounts::wallet::begin_new(&conn, k2_core::llm_accounts::Tool::Claude, "Second", None).unwrap();
        let slot = env.home.join(format!(".k2/llm-accounts/claude/{}", e.id));
        std::fs::write(slot.join(".credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-second"}}}}"#)).unwrap();
        k2_core::llm_accounts::wallet::finalize_login(&conn, &e.id, None).unwrap()
    };
    let slot = env.home.join(format!(".k2/llm-accounts/claude/{}", second.id));
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let default_id = account_by_label(&list, "claude", "Default")["id"].as_str().unwrap().to_string();

    // The pool's live login can't be pinned; a passport can't pin.
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope":"workspace","workspace":"pinws","id": default_id}));
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(code(&r), "pinned_active");
    let r = post(port, "/cli/llm/accounts/pin", &passport(), json!({"scope":"workspace","workspace":"pinws","id": second.id}));
    assert_eq!(r.status, 403);
    assert_eq!(code(&r), "owner_only");
    let r = req(port, "GET", &format!("/cli/llm/accounts/pin?token={OWNER}"), None);
    assert_eq!(r.status, 405);

    // Pin the workspace (by handle) to Second.
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope":"workspace","workspace":"pinws","id": second.id}));
    assert_eq!(r.status, 200, "pin: {}", r.body);
    let v = js(&r);
    assert_eq!(v["pin"]["scopeId"], pid.as_str());
    assert_eq!(v["pin"]["label"], "pinws");
    assert!(v["note"].as_str().unwrap().contains("This chat's Claude history will stay with this subscription."));
    // A pinned login can't become the pool's active login.
    let r = post(port, "/cli/llm/accounts/switch", OWNER, json!({"id": second.id}));
    assert_eq!(r.status, 409);
    assert_eq!(code(&r), "login_pinned");

    // Door B (daemon-internal spawn): the canonical chat runs from Second's slot.
    spawn_b(&pid, &ws, &pid, "claude", &["--session-id", "conv-pinned-1"]);
    let e1 = run_env(&env, "claude", "conv-pinned-1");
    assert!(e1.contains(&format!("CLAUDE_CONFIG_DIR={}\n", slot.display())), "{e1}");
    assert!(e1.contains(&format!("HOME={}\n", env.home.display())), "HOME never changes: {e1}");
    assert!(slot.join("settings.json").is_symlink(), "shared settings linked into the slot");
    let cfg: Value = serde_json::from_slice(&std::fs::read(slot.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cfg["projects"][ws.to_string_lossy().as_ref()]["hasTrustDialogAccepted"], true);
    // In use while that session runs.
    let list = js(&get(port, "/cli/llm/accounts/list", OWNER));
    let row = account_by_label(&list, "claude", "Second");
    assert_eq!(row["inUse"], true, "{row}");
    assert_eq!(row["pinnedTo"][0]["label"], "pinws");

    // Another workspace is on the pool: no env.
    let (pid2, ws2) = seed_project(&env, "poolws");
    spawn_b(&pid2, &ws2, &pid2, "claude", &["--session-id", "conv-pool-1"]);
    let e2 = run_env(&env, "claude", "conv-pool-1");
    assert!(e2.contains("CLAUDE_CONFIG_DIR=\n"), "pool sessions launch exactly as before: {e2}");

    // Re-pin the workspace to an API key.
    let r = post(port, "/cli/llm/accounts/add-key", OWNER, json!({"tool":"claude","label":"Metered","key": KEY}));
    assert_eq!(r.status, 200, "add-key: {}", r.body);
    let key_id = js(&r)["account"]["id"].as_str().unwrap().to_string();
    assert_eq!(js(&r)["account"]["billedPerToken"], true);
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope":"workspace","scopeId": pid, "id": key_id}));
    assert_eq!(r.status, 200, "{}", r.body);
    spawn_b(&pid, &ws, &format!("{pid}:hb:daily"), "claude", &["--session-id", "conv-key-1"]);
    let e3 = run_env(&env, "claude", "conv-key-1");
    assert!(e3.contains("KEYSET=yes"), "API key injected: {e3}");
    assert!(e3.contains("CLAUDE_CONFIG_DIR=\n"), "an API key needs no home: {e3}");

    // Resume the first conversation: its recorded home, not the new pin.
    kill_sessions_under(&env.home);
    spawn_b(&pid, &ws, &pid, "claude", &["--resume", "conv-pinned-1"]);
    let e4 = {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut found = None;
            for e in std::fs::read_dir(&env.log).unwrap().flatten() {
                let f = e.file_name().to_string_lossy().to_string();
                if f.starts_with("claude-") && f.ends_with(".argv") {
                    let a = std::fs::read_to_string(e.path()).unwrap_or_default();
                    if a.lines().any(|l| l == "--resume") && a.lines().any(|l| l == "conv-pinned-1") {
                        found = std::fs::read_to_string(e.path().with_extension("env")).ok();
                    }
                }
            }
            if let Some(f) = found {
                break f;
            }
            assert!(Instant::now() < deadline, "no resume run");
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    assert!(e4.contains(&format!("CLAUDE_CONFIG_DIR={}\n", slot.display())), "resume keeps its home: {e4}");
    assert!(!e4.contains("KEYSET=yes"));

    // Door A (/cli/sessions/v2/spawn) with a session pin on a tab key.
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope":"session","session":"tab-door-a","tool":"claude","id": key_id}));
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(
        port,
        "/cli/sessions/v2/spawn",
        OWNER,
        json!({"agent_name":"tab-door-a","cwd": ws.to_string_lossy(),"command":"claude","args":["--session-id","conv-door-a"],"cols":80,"rows":24}),
    );
    assert_eq!(r.status, 200, "v2 spawn: {}", r.body);
    let e5 = run_env(&env, "claude", "conv-door-a");
    assert!(e5.contains("KEYSET=yes"), "{e5}");

    // Unpin; pins listing; nothing secret anywhere.
    let r = post(port, "/cli/llm/accounts/unpin", OWNER, json!({"scope":"session","session":"tab-door-a","tool":"claude"}));
    assert_eq!(js(&r)["unpinned"], true);
    for (path, tok) in [
        ("/cli/llm/accounts/list", OWNER.to_string()),
        ("/cli/llm/accounts/pins", OWNER.to_string()),
        ("/cli/llm/accounts/usage", OWNER.to_string()),
        ("/cli/llm/accounts/pins", passport()),
    ] {
        let r = get(port, path, &tok);
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        assert!(!r.body.contains(MARKER), "{path} leaks a secret");
    }
    let r = get(port, &format!("/cli/llm/accounts/status?id={key_id}"), OWNER);
    assert!(!r.body.contains(MARKER));
    let dump = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut out = String::new();
        for t in ["llm_accounts", "llm_active", "llm_account_pins", "llm_session_logins"] {
            let mut st = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
            let n = st.column_count();
            let mut rows = st.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..n {
                    let v: rusqlite::types::Value = row.get(i).unwrap();
                    out.push_str(&format!("{v:?}|"));
                }
            }
        }
        out
    };
    assert!(!dump.contains(MARKER), "no secret in the DB");
    let audit = std::fs::read_to_string(env.home.join(".k2/auth-audit.jsonl")).unwrap_or_default();
    assert!(!audit.contains(MARKER), "no secret in the audit log");
    assert!(audit.contains("llm_accounts.pin"));
    kill_sessions_under(&env.home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinned_codex_session_runs_in_a_shadow_home_sharing_conversations() {
    let env = setup();
    let _d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let (pid, ws) = seed_project(&env, "codexws");
    std::fs::create_dir_all(env.home.join(".codex")).unwrap();
    std::fs::write(env.home.join(".codex/config.toml"), "model = \"m\"\n").unwrap();
    std::fs::write(env.home.join(".codex/history.jsonl"), "").unwrap();
    let e = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let e = k2_core::llm_accounts::wallet::begin_new(&conn, k2_core::llm_accounts::Tool::Codex, "Team", None).unwrap();
        let slot = env.home.join(format!(".k2/llm-accounts/codex/{}", e.id));
        std::fs::write(slot.join("auth.json"), format!(r#"{{"tokens":{{"refresh_token":"{MARKER}"}}}}"#)).unwrap();
        let e = k2_core::llm_accounts::wallet::finalize_login(&conn, &e.id, None).unwrap();
        // Finalize made it active (nothing was live); give the pool another
        // login so this one can be pinned.
        let other = k2_core::llm_accounts::wallet::begin_new(&conn, k2_core::llm_accounts::Tool::Codex, "Other", None).unwrap();
        let oslot = env.home.join(format!(".k2/llm-accounts/codex/{}", other.id));
        std::fs::write(oslot.join("auth.json"), r#"{"tokens":{"refresh_token":"x"}}"#).unwrap();
        k2_core::llm_accounts::wallet::finalize_login(&conn, &other.id, None).unwrap();
        k2_core::llm_accounts::wallet::switch(&conn, k2_core::llm_accounts::Tool::Codex, &other.id, None, None).unwrap();
        k2_core::llm_accounts::pins::pin(&conn, k2_core::llm_accounts::pins::ScopeKind::Workspace, &pid, None, &e.id, None).unwrap();
        e
    };
    let slot = env.home.join(format!(".k2/llm-accounts/codex/{}", e.id));
    spawn_b(&pid, &ws, &pid, "codex", &["--yolo"]);
    let env_text = run_env(&env, "codex", "--yolo");
    assert!(env_text.contains(&format!("CODEX_HOME={}\n", slot.display())), "{env_text}");
    let sessions = std::fs::read_link(slot.join("sessions")).expect("sessions is a link");
    assert_eq!(sessions, env.home.join(".codex/sessions"), "conversations land in the normal Codex home");
    assert!(env.home.join(".codex/sessions").is_dir());
    assert!(std::fs::read_to_string(slot.join("config.toml")).unwrap().contains("cli_auth_credentials_store = \"file\""));
    assert!(!slot.join("auth.json").is_symlink());
    kill_sessions_under(&env.home);
}

// ── A token picked on any chat tab sticks across revivals ───────────

/// The env files of every `name` shim run whose argv has all `needles`,
/// once there are exactly `want` of them.
fn run_envs(env: &TestEnv, name: &str, needles: &[&str], want: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut hits = Vec::new();
        for e in std::fs::read_dir(&env.log).expect("log").flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if f.starts_with(&format!("{name}-")) && f.ends_with(".argv") {
                let argv = std::fs::read_to_string(e.path()).unwrap_or_default();
                if needles.iter().all(|n| argv.lines().any(|l| l == *n)) {
                    if let Ok(envf) = std::fs::read_to_string(e.path().with_extension("env")) {
                        hits.push(envf);
                    }
                }
            }
        }
        if hits.len() == want {
            return hits;
        }
        assert!(hits.len() < want, "more than {want} {name} runs with {needles:?}");
        assert!(Instant::now() < deadline, "{} of {want} {name} runs with {needles:?}", hits.len());
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_a(port: u16, key: &str, cwd: &Path, command: &str, args: &[&str]) {
    let r = post(
        port,
        "/cli/sessions/v2/spawn",
        OWNER,
        json!({"agent_name": key, "cwd": cwd.to_string_lossy(), "command": command, "args": args, "cols": 80, "rows": 24}),
    );
    assert_eq!(r.status, 200, "v2 spawn {key}: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_picked_on_an_extra_chat_tab_sticks_when_the_chat_is_revived() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    reset_wallet_tables();
    let port = d.port;
    let (pid, ws) = seed_project(&env, "tabws");

    // Live Default + a second signed-in Claude subscription.
    std::fs::create_dir_all(env.home.join(".claude")).unwrap();
    std::fs::write(env.home.join(".claude/.credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-live"}}}}"#)).unwrap();
    k2_daemon::llm_accounts_runtime::boot_import();
    let second = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let e = k2_core::llm_accounts::wallet::begin_new(&conn, k2_core::llm_accounts::Tool::Claude, "Second", None).unwrap();
        let slot = env.home.join(format!(".k2/llm-accounts/claude/{}", e.id));
        std::fs::write(slot.join(".credentials.json"), format!(r#"{{"claudeAiOauth":{{"refreshToken":"{MARKER}-second"}}}}"#)).unwrap();
        k2_core::llm_accounts::wallet::finalize_login(&conn, &e.id, None).unwrap()
    };
    let slot = env.home.join(format!(".k2/llm-accounts/claude/{}", second.id));

    // An extra chat tab (not the pinned chat) starts on the server default.
    spawn_a(port, "tab-pane1", &ws, "claude", &["--session-id", "conv-tab-1"]);
    let e0 = run_env(&env, "claude", "conv-tab-1");
    assert!(e0.contains("CLAUDE_CONFIG_DIR=\n"), "{e0}");
    let transcript = env.home.join(".claude/projects/-tabws/conv-tab-1.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{\"turn\":1}\n").unwrap();

    // Picking a token: people only, POST only.
    let body = json!({"scope":"session","scopeId":"tab-pane1","conversationId":"conv-tab-1","tool":"claude","id": second.id});
    let r = post(port, "/cli/llm/accounts/pin", &passport(), body.clone());
    assert_eq!((r.status, code(&r)), (403, "owner_only".to_string()), "{}", r.body);
    let r = req(port, "GET", &format!("/cli/llm/accounts/pin?token={OWNER}&scope=session&scopeId=tab-pane1"), None);
    assert_eq!(r.status, 405);
    let r = post(port, "/cli/llm/accounts/pin", OWNER, body);
    assert_eq!(r.status, 200, "pin: {}", r.body);
    assert!(js(&r)["note"].as_str().unwrap().contains("from its next start"), "{}", r.body);
    let pins = js(&get(port, "/cli/llm/accounts/pins", OWNER));
    let mut keys: Vec<String> = pins["pins"].as_array().unwrap().iter().map(|p| p["scopeId"].as_str().unwrap().to_string()).collect();
    keys.sort();
    assert_eq!(keys, vec!["conversation:conv-tab-1".to_string(), "tab-pane1".to_string()]);
    // The subscription now set for a chat can't be the server default.
    let r = post(port, "/cli/llm/accounts/switch", OWNER, json!({"id": second.id}));
    assert_eq!((r.status, code(&r)), (409, "login_pinned".to_string()), "{}", r.body);

    // Killed and revived in the same tab (refresh / restart recovery resume):
    // it runs from the picked subscription's home, history copied there.
    kill_sessions_under(&env.home);
    spawn_a(port, "tab-pane1", &ws, "claude", &["--resume", "conv-tab-1"]);
    let e1 = run_envs(&env, "claude", &["--resume", "conv-tab-1"], 1).remove(0);
    assert!(e1.contains(&format!("CLAUDE_CONFIG_DIR={}\n", slot.display())), "{e1}");
    assert_eq!(std::fs::read_to_string(slot.join("projects/-tabws/conv-tab-1.jsonl")).unwrap(), "{\"turn\":1}\n");
    assert!(transcript.is_file(), "copied, not moved");

    // Closed and reopened from history in a new tab: a new key, same chat.
    kill_sessions_under(&env.home);
    spawn_a(port, "tab-pane2", &ws, "claude", &["--resume", "conv-tab-1"]);
    let all = run_envs(&env, "claude", &["--resume", "conv-tab-1"], 2);
    for e in &all {
        assert!(e.contains(&format!("CLAUDE_CONFIG_DIR={}\n", slot.display())), "{e}");
    }
    // A pool chat elsewhere in the workspace is untouched.
    spawn_a(port, "tab-pane3", &ws, "claude", &["--session-id", "conv-tab-3"]);
    assert!(run_env(&env, "claude", "conv-tab-3").contains("CLAUDE_CONFIG_DIR=\n"));

    // Codex: picked before Codex mints its id; adoption moves it onto the id.
    let r = post(port, "/cli/llm/accounts/add-key", OWNER, json!({"tool":"codex","label":"CodexKey","key": KEY}));
    assert_eq!(r.status, 200, "add-key: {}", r.body);
    let ck = js(&r)["account"]["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/llm/accounts/pin", OWNER, json!({"scope":"session","scopeId":"tab-cx","tool":"codex","id": ck}));
    assert_eq!(r.status, 200, "{}", r.body);
    spawn_a(port, "tab-cx", &ws, "codex", &["--yolo"]);
    assert!(run_env(&env, "codex", "--yolo").contains("KEYSET=yes"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceTabSession::upsert(
            &conn,
            &k2_core::db::schema::WorkspaceTabSession {
                project_id: pid.clone(),
                pane_group_id: "cx".into(),
                agent_name: "tab-cx".into(),
                session_id: None,
                command: Some("codex".into()),
                args_json: Some("[\"--yolo\"]".into()),
                cwd: Some(ws.to_string_lossy().into_owned()),
                last_seen_at: 0,
                pinned_cols: None,
                pinned_rows: None,
                pinned_set_by: None,
            },
        )
        .unwrap();
        k2_core::db::schema::WorkspaceTabSession::stamp_session_id(&conn, &pid, "cx", "019a-codex-conv").unwrap();
    }
    let pins = js(&get(port, "/cli/llm/accounts/pins", OWNER));
    assert!(
        pins["pins"].as_array().unwrap().iter().any(|p| p["scopeId"] == "conversation:019a-codex-conv" && p["accountId"] == ck.as_str()),
        "{pins}"
    );
    kill_sessions_under(&env.home);
    spawn_a(port, "tab-cx-reopened", &ws, "codex", &["resume", "019a-codex-conv"]);
    let ce = run_envs(&env, "codex", &["resume", "019a-codex-conv"], 1).remove(0);
    assert!(ce.contains("KEYSET=yes"), "the reopened Codex chat runs on its picked token: {ce}");

    // Back to default for the Claude tab: its key and its conversation go.
    let r = post(port, "/cli/llm/accounts/unpin", OWNER, json!({"scope":"session","scopeId":"tab-pane2","conversationId":"conv-tab-1","tool":"claude"}));
    assert_eq!(js(&r)["unpinned"], true, "{}", r.body);
    let r = post(port, "/cli/llm/accounts/unpin", OWNER, json!({"scope":"session","scopeId":"tab-pane1","tool":"claude"}));
    assert_eq!(js(&r)["unpinned"], true, "{}", r.body);
    let pins = js(&get(port, "/cli/llm/accounts/pins", OWNER));
    assert!(!pins["pins"].as_array().unwrap().iter().any(|p| p["tool"] == "claude"), "{pins}");
    let r = post(port, "/cli/llm/accounts/switch", OWNER, json!({"id": second.id}));
    assert_eq!(r.status, 200, "unpinned: may be the server default again: {}", r.body);
    assert!(!get(port, "/cli/llm/accounts/pins", OWNER).body.contains(MARKER));
    kill_sessions_under(&env.home);
}
