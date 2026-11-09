//! `/cli/vault/run` route tests (prd-vault-for-agents-v1 A1 §2.6). Temp
//! vault, fake key store and temp folders; never the real home or keychain.

use std::collections::HashMap;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, AuditQuery, KeyStore, OpenOptions, Role, SecretStore};
use serde_json::{json, Value};

use super::*;
use crate::vault::actor::{Channel, VaultCaller};
use crate::vault_routes::{handle_on, GRANT, SET};

const SENTINEL: &str = "k2test_RUN_SENTINEL_77c1f0a2b3";

struct T {
    _dir: TestDir,
    v: Arc<LocalVault>,
    ctx: RunCtx,
}

fn vault() -> T {
    let dir = TestDir::new("run-routes");
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace folder");
    let ctx = RunCtx { sandboxed: false, workspace_root: Some(workspace), secrets_root: Some(dir.path().join("secrets")) };
    T { _dir: dir, v, ctx }
}

fn agent() -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: "ws1".into(), session_id: Some("sess-9".into()), shell: false, vault_manage: true }),
        channel: Channel::Plain,
        session_id: Some("sess-9".into()),
    }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn call(t: &T, path: &str, c: &VaultCaller, body: Value) -> (u16, Value) {
    let r = handle_on(&t.v, path, Ok(c.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None);
    let code: u16 = r.status[..3].parse().expect("status");
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json body for {path}: {}", r.body));
    (code, v)
}

/// The route, with this test's folders in place of the daemon's.
fn run_it(t: &T, c: &VaultCaller, body: Value) -> Result<Value, VaultError> {
    run(&t.v, c, body.to_string().as_bytes(), &t.ctx)
}

fn b64_s(s: &str) -> String {
    b64(s.as_bytes())
}

fn set_own(t: &T, name: &str, delivery: &str) {
    let (s, v) = call(t, SET, &agent(), json!({ "name": name, "delivery": delivery, "ceiling": "env", "valueB64": b64_s(SENTINEL) }));
    assert_eq!(s, 200, "{v}");
}

fn set_fetch_only(t: &T, name: &str) {
    let (s, v) = call(t, SET, &agent(), json!({ "name": name, "hosts": ["api.example.com"], "valueB64": b64_s(SENTINEL) }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["secret"]["defaultDelivery"], "fetch", "{v}");
}

fn set_server(t: &T, name: &str) {
    let (s, v) = call(t, SET, &owner(), json!({ "name": name, "scope": "server", "delivery": "env", "ceiling": "env", "valueB64": b64_s(SENTINEL) }));
    assert_eq!(s, 200, "{v}");
}

fn audit_rows(t: &T) -> Vec<k2_vault::AuditRow> {
    t.v.flush_rollups();
    t.v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit")
}

fn run_rows(t: &T) -> Vec<k2_vault::AuditRow> {
    audit_rows(t).into_iter().filter(|r| r.consumer.as_deref() == Some("run:sess-9")).collect()
}

fn item<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["items"].as_array().expect("items").iter().find(|i| i["name"] == name).unwrap_or_else(|| panic!("no item {name}: {v}"))
}

#[test]
fn routes_are_wired_and_post_only() {
    assert!(crate::vault_routes::is_route(RUN));
    assert!(crate::vault_routes::is_route(RUN_HELPER));
    // The guards in cell_server, the dispatcher and human.rs answer 405 from
    // this list.
    assert!(crate::vault_routes::is_post_route(RUN), "POST /cli/vault/run: GET must be 405");
    assert!(!crate::vault_routes::is_post_route(RUN_HELPER), "run-helper is read-only");
}

#[test]
fn an_env_secret_comes_back_as_env_under_its_default_name() {
    let t = vault();
    set_own(&t, "db-url", "env");
    let v = run_it(&t, &agent(), json!({ "secrets": [{ "name": "db-url" }], "cmd": "node" })).expect("run");
    let i = item(&v, "db-url");
    assert_eq!(i["how"], "env", "{v}");
    assert_eq!(i["env"], "DB_URL", "{v}");
    assert_eq!(i["valueB64"], b64_s(SENTINEL));
    assert!(i.get("path").is_none(), "{v}");
    assert!(v.get("runDir").is_none(), "no folder for env only: {v}");
}

#[test]
fn the_callers_name_beats_the_grants_and_the_grants_beats_the_default() {
    // A grant to `session:<id>` looks in ~/.k2/hook-sessions.json.
    let _home = crate::test_support::TempHome::new();
    let t = vault();
    set_own(&t, "GH", "fetch");
    let (s, g) = call(&t, GRANT, &agent(), json!({ "name": "GH", "to": "session:sess-9", "envName": "GH_TOKEN" }));
    assert_eq!(s, 200, "{g}");
    let v = run_it(&t, &agent(), json!({ "secrets": [{ "name": "GH" }], "cmd": "gh" })).expect("run");
    assert_eq!(item(&v, "GH")["env"], "GH_TOKEN", "the grant's env name: {v}");
    let v = run_it(&t, &agent(), json!({ "secrets": [{ "name": "GH", "env": "MY_TOKEN" }], "cmd": "gh" })).expect("run");
    assert_eq!(item(&v, "GH")["env"], "MY_TOKEN", "--secret NAME=ENV: {v}");
}

#[test]
fn a_bad_or_denied_env_name_is_refused_before_any_use() {
    let t = vault();
    set_own(&t, "K", "env");
    for (env, want) in [("lower", "bad_env_name"), ("LD_PRELOAD", "env_name_not_allowed"), ("OPENAI_BASE_URL", "env_name_not_allowed")] {
        let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "K", "env": env }], "cmd": "x" })).expect_err("refused");
        assert_eq!(e.code, want, "{env}");
    }
    assert!(run_rows(&t).is_empty(), "nothing was used");
}

/// V22: file-level is never widened to env, and the child's env gets a path.
#[cfg(unix)]
#[test]
fn a_file_level_secret_is_a_0600_file_and_never_env() {
    use std::os::unix::fs::PermissionsExt;
    let t = vault();
    set_own(&t, "CODE", "file");
    let v = run_it(&t, &agent(), json!({ "secrets": [{ "name": "CODE" }], "cmd": "tool" })).expect("run");
    let i = item(&v, "CODE");
    assert_eq!(i["how"], "file", "{v}");
    assert_eq!(i["env"], "CODE_FILE", "{v}");
    assert!(i.get("valueB64").is_none(), "a file-level value must not be offered as env: {v}");
    let path = i["path"].as_str().expect("path");
    assert_eq!(std::fs::metadata(path).expect("file").permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read(path).expect("read"), SENTINEL.as_bytes());
    let dir = v["runDir"].as_str().expect("runDir");
    assert!(Path::new(path).starts_with(dir), "{path} in {dir}");
    assert!(Path::new(dir).starts_with(t.ctx.secrets_root.as_ref().expect("root").join("run")), "{dir}");
    assert_eq!(std::fs::metadata(dir).expect("dir").permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_fetch_only_secret_is_refused_with_the_fetch_hint() {
    let t = vault();
    set_fetch_only(&t, "STRIPE");
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "STRIPE" }], "cmd": "x" })).expect_err("refused");
    assert_eq!((e.code, e.status), ("delivery_not_granted", 403), "{}", e.hint);
    assert!(e.hint.contains("k2 vault fetch --secret STRIPE"), "{}", e.hint);
}

#[test]
fn a_name_not_granted_is_refused_in_use_refusal_words() {
    let t = vault();
    set_server(&t, "OPENAI");
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "OPENAI", }], "scope": "server", "cmd": "x" })).expect_err("refused");
    assert_eq!((e.code, e.status), ("delivery_not_granted", 403), "{}", e.hint);
}

#[test]
fn no_such_secret_is_404() {
    let t = vault();
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "NOPE" }], "cmd": "x" })).expect_err("refused");
    assert_eq!(e.status, 404, "{} {}", e.code, e.hint);
}

#[test]
fn people_and_the_owner_token_are_refused() {
    let t = vault();
    set_own(&t, "K", "env");
    let e = run_it(&t, &owner(), json!({ "secrets": [{ "name": "K" }] })).expect_err("owner refused");
    assert_eq!(e.code, "use_run_as_agent");
    let login = VaultCaller { actor: Actor::Login { username: "lee".into(), role: Role::Owner }, channel: Channel::Login, session_id: None };
    let e = run_it(&t, &login, json!({ "secrets": [{ "name": "K" }] })).expect_err("login refused");
    assert_eq!(e.code, "use_run_as_agent");
}

#[test]
fn a_sandboxed_session_is_refused() {
    let mut t = vault();
    set_own(&t, "K", "env");
    t.ctx.sandboxed = true;
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "K" }], "cmd": "x" })).expect_err("refused");
    assert_eq!(e.code, "run_not_in_sandbox");
}

/// One row per secret, env or file: the file one must not also leave a
/// deny row from trying env first.
#[test]
fn one_audit_row_per_secret_with_the_run_consumer() {
    let t = vault();
    set_own(&t, "E", "env");
    set_own(&t, "F", "file");
    run_it(&t, &agent(), json!({ "secrets": [{ "name": "E" }, { "name": "F" }], "cmd": "/usr/bin/node" })).expect("run");
    let rows = run_rows(&t);
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_eq!(r.action, "inject", "{r:?}");
        assert_eq!(r.outcome, "ok:run node", "the basename only: {r:?}");
        assert_eq!(r.count, 1, "{r:?}");
    }
    let denied: Vec<_> = audit_rows(&t).into_iter().filter(|r| r.outcome.starts_with("deny")).collect();
    assert!(denied.is_empty(), "no deny row for a secret that was delivered: {denied:?}");
}

#[test]
fn no_mask_is_logged() {
    let t = vault();
    set_own(&t, "E", "env");
    run_it(&t, &agent(), json!({ "secrets": [{ "name": "E" }], "cmd": "vim", "noMask": true })).expect("run");
    let rows = run_rows(&t);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].outcome, "ok:run vim no-mask");
}

#[test]
fn a_refused_run_is_audited_under_the_run_consumer() {
    let t = vault();
    set_fetch_only(&t, "S");
    run_it(&t, &agent(), json!({ "secrets": [{ "name": "S" }], "cmd": "x" })).expect_err("refused");
    let rows = run_rows(&t);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].outcome, "deny:delivery_not_granted");
}

#[test]
fn the_audit_keeps_a_plain_basename_and_nothing_else() {
    assert_eq!(audit_cmd(Some("/opt/bin/tool")), "tool");
    assert_eq!(audit_cmd(Some("tool --token=abc")), "tool--tokenabc", "only ever the basename is sent; odd characters still can't land");
    assert_eq!(audit_cmd(Some("a\nb\u{1b}[2J")), "ab2J");
    assert_eq!(audit_cmd(None), "command");
    assert_eq!(audit_cmd(Some(&"x".repeat(200))).len(), 64);
}

#[test]
fn two_secrets_on_one_env_name_are_refused() {
    let t = vault();
    set_own(&t, "A", "env");
    set_own(&t, "B", "env");
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "A", "env": "X" }, { "name": "B", "env": "X" }], "cmd": "x" })).expect_err("refused");
    assert_eq!(e.code, "duplicate_env_name");
}

#[test]
fn a_later_refusal_leaves_no_file_behind() {
    let t = vault();
    set_own(&t, "F", "file");
    set_fetch_only(&t, "S");
    run_it(&t, &agent(), json!({ "secrets": [{ "name": "F" }, { "name": "S" }], "cmd": "x" })).expect_err("refused");
    assert!(!t.ctx.secrets_root.as_ref().expect("root").join("run").exists(), "nothing is written until every secret passed");
}

#[test]
fn a_value_with_nul_cant_be_env() {
    let t = vault();
    let (s, v) = call(&t, SET, &agent(), json!({ "name": "BIN", "kind": "generic", "delivery": "env", "ceiling": "env", "valueB64": b64(b"k2testNULpart\0more") }));
    assert_eq!(s, 200, "{v}");
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "BIN" }], "cmd": "x" })).expect_err("refused");
    assert_eq!(e.code, "value_has_nul");
    assert!(!e.hint.contains("k2testNULpart"), "{}", e.hint);
}

fn env_file(t: &T, name: &str, text: &str) -> String {
    let p = t.ctx.workspace_root.as_ref().expect("root").join(name);
    std::fs::write(&p, text).expect("write env file");
    p.display().to_string()
}

#[test]
fn an_env_file_resolves_references_and_passes_other_lines_through() {
    let t = vault();
    // A session secret: its reference needs no workspace lookup in the DB.
    let (s, g) = call(&t, SET, &agent(), json!({ "name": "TOK", "scope": "session", "ttl": "2h", "delivery": "env", "ceiling": "env", "valueB64": b64_s(SENTINEL) }));
    assert_eq!(s, 200, "{g}");
    let path = env_file(&t, ".env.k2", "# a comment\nexport API_KEY=k2v://session/sess-9/TOK\nMODE=\"debug\"\nlower_ok=1\n");
    let v = run_it(&t, &agent(), json!({ "envFile": path, "cmd": "x" })).expect("run");
    let items = v["items"].as_array().expect("items");
    let by_env = |e: &str| items.iter().find(|i| i["env"] == e).unwrap_or_else(|| panic!("no {e}: {v}"));
    assert_eq!(by_env("API_KEY")["how"], "env", "{v}");
    assert_eq!(by_env("API_KEY")["valueB64"], b64_s(SENTINEL));
    assert_eq!(by_env("MODE")["how"], "plain", "{v}");
    assert_eq!(by_env("MODE")["valueB64"], b64_s("debug"));
    assert_eq!(by_env("lower_ok")["how"], "plain", "{v}");
    assert_eq!(items.len(), 3, "the comment is no entry: {v}");
}

#[test]
fn a_secret_beats_a_plain_line_of_the_same_name() {
    let t = vault();
    set_own(&t, "K", "env");
    let path = env_file(&t, "e", "K=plain-text\n");
    let v = run_it(&t, &agent(), json!({ "envFile": path, "secrets": [{ "name": "K" }], "cmd": "x" })).expect("run");
    let items = v["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["how"], "env", "{v}");
}

#[test]
fn an_env_file_reference_obeys_the_same_grants() {
    let t = vault();
    set_server(&t, "SRV");
    let path = env_file(&t, ".env.k2", "API_KEY=k2v://server/SRV\n");
    let e = run_it(&t, &agent(), json!({ "envFile": path, "cmd": "x" })).expect_err("not granted");
    assert_eq!(e.code, "delivery_not_granted", "{}", e.hint);
}

#[test]
fn an_env_file_reference_cannot_set_a_denied_name_or_pin_a_version() {
    let t = vault();
    let path = env_file(&t, "a", "LD_PRELOAD=k2v://server/SRV\n");
    let e = run_it(&t, &agent(), json!({ "envFile": path, "cmd": "x" })).expect_err("denied name");
    assert_eq!(e.code, "bad_env_name");
    let path = env_file(&t, "b", "KEY=k2v://server/SRV@3\n");
    let e = run_it(&t, &agent(), json!({ "envFile": path, "cmd": "x" })).expect_err("pinned");
    assert_eq!(e.code, "run_pinned_ref");
}

#[test]
fn a_malformed_env_file_line_is_not_echoed() {
    let t = vault();
    let path = env_file(&t, "a", "sk-live-pasted-value=1\n");
    let e = run_it(&t, &agent(), json!({ "envFile": path, "cmd": "x" })).expect_err("bad name");
    assert_eq!(e.code, "bad_env_name");
    assert!(!e.hint.contains("sk-live"), "{}", e.hint);
}

#[test]
fn an_env_file_outside_the_workspace_is_refused() {
    let t = vault();
    let outside = t._dir.path().join("outside.env");
    std::fs::write(&outside, "A=1\n").expect("write");
    let e = run_it(&t, &agent(), json!({ "envFile": outside.display().to_string(), "cmd": "x" })).expect_err("outside");
    assert_eq!(e.code, "path_outside_workspace");
    let e = run_it(&t, &agent(), json!({ "envFile": "../outside.env", "cmd": "x" })).expect_err("dotdot");
    assert_eq!(e.code, "path_outside_workspace");
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_workspace_is_refused() {
    let t = vault();
    let outside = t._dir.path().join("outside.env");
    std::fs::write(&outside, "A=1\n").expect("write");
    let root = t.ctx.workspace_root.as_ref().expect("root");
    std::os::unix::fs::symlink(&outside, root.join("link.env")).expect("link");
    let e = run_it(&t, &agent(), json!({ "envFile": "link.env", "cmd": "x" })).expect_err("link");
    assert_eq!(e.code, "path_outside_workspace");
}

#[test]
fn a_run_names_something() {
    let t = vault();
    let e = run_it(&t, &agent(), json!({ "cmd": "x" })).expect_err("empty");
    assert_eq!(e.code, "secret_required");
}

#[test]
fn the_value_is_in_the_answer_only_as_base64_and_never_in_errors() {
    let t = vault();
    set_own(&t, "E", "env");
    set_fetch_only(&t, "S");
    let v = run_it(&t, &agent(), json!({ "secrets": [{ "name": "E" }], "cmd": "x" })).expect("run");
    assert!(!v.to_string().contains(SENTINEL), "raw value in the answer");
    let e = run_it(&t, &agent(), json!({ "secrets": [{ "name": "S" }], "cmd": "x" })).expect_err("refused");
    assert!(!e.hint.contains(SENTINEL));
}

#[test]
fn the_helper_is_this_binary_even_after_an_update_replaced_it() {
    assert_eq!(installed_exe(PathBuf::from("/opt/k2/k2-daemon (deleted)")), PathBuf::from("/opt/k2/k2-daemon"));
    assert_eq!(installed_exe(PathBuf::from("/opt/k2/k2-daemon")), PathBuf::from("/opt/k2/k2-daemon"));
    let v = run_helper().expect("helper");
    let p = v["helper"].as_str().expect("helper path");
    assert!(Path::new(p).is_absolute() && Path::new(p).is_file(), "{p}");
}

#[test]
fn scope_spellings_reach_the_selector() {
    let s = sel_named("N", Some("ws:sales"));
    assert_eq!((s.scope.as_deref(), s.workspace.as_deref()), (Some("workspace"), Some("sales")));
    let s = sel_named("N", Some("session:abc"));
    assert_eq!((s.scope.as_deref(), s.session.as_deref()), (Some("session"), Some("abc")));
    assert_eq!(sel_named("N", Some("server")).scope.as_deref(), Some("server"));
    assert_eq!(sel_named("N", None).scope, None);
}
