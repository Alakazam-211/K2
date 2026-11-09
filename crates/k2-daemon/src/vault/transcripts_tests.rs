//! A7 (V19): a temp HOME, fake transcripts holding a fake value, a temp
//! vault and the fake key store. Never the real `~/.claude`, `~/.codex`,
//! `~/.k2` or keychain.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_core::test_env::{EnvVar, TempHome};
use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::store::PutRequest;
use k2_vault::{Actor, AgentActor, Delivery, Kind, KeyStore, Namespace, OpenOptions, Role, Scope as VScope, SecretBytes, SecretStore};
use serde_json::Value;

use super::*;
use crate::vault::actor::Channel;
use crate::vault_routes::{handle_on, DOCTOR};

const PLANTED: &str = "k2test_PLANTED_TRANSCRIPT_VALUE_9f8e7d6c5b4a";
const OLD_VALUE: &str = "k2test_ROTATED_OUT_VALUE_1a2b3c4d5e6f7a8b";
const FETCH_ONLY: &str = "k2test_FETCH_ONLY_VALUE_aa11bb22cc33dd44";
/// Sits on the same line as the value: a line excerpt would carry it.
const NEARBY: &str = "LINE_EXCERPT_CANARY_do_not_print";

struct Env {
    home: TempHome,
    _vars: [EnvVar; 3],
    _dir: TestDir,
    v: Arc<LocalVault>,
}

fn env() -> Env {
    let home = TempHome::new();
    // A session's own config folder must not reach a test, or the scan
    // would read the real transcripts.
    let vars = [EnvVar::remove("CLAUDE_CONFIG_DIR"), EnvVar::remove("CODEX_HOME"), EnvVar::remove("GROK_HOME")];
    let dir = TestDir::new("transcripts");
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    Env { home, _vars: vars, _dir: dir, v }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn agent(ws: &str) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-1".into()),
    }
}

fn put(e: &Env, scope: VScope, name: &str, value: &str, delivery: Delivery) {
    let mut req = PutRequest::new(Namespace::User, scope, name, Kind::ApiKey);
    req.hosts = vec!["api.example.com".into()];
    req.default_delivery = Some(delivery);
    req.delivery_ceiling = Some(delivery);
    e.v.put(req, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put");
}

fn doctor_route(e: &Env, c: &VaultCaller) -> (u16, String) {
    let params = HashMap::from([("transcripts".to_string(), "1".to_string())]);
    let r = handle_on(&e.v, DOCTOR, Ok(c.clone()), b"", &params, "test", None);
    (r.status[..3].parse().expect("status"), r.body)
}

fn scan(e: &Env, c: &VaultCaller) -> (String, Value) {
    let (s, body) = doctor_route(e, c);
    assert_eq!(s, 200, "{body}");
    let v: Value = serde_json::from_str(&body).expect("json");
    (body, v)
}

fn leaks(v: &Value) -> Vec<Value> {
    v["findings"].as_array().expect("findings").iter().filter(|f| f["code"] == "transcript_leak").cloned().collect()
}

fn leak_names(v: &Value) -> Vec<String> {
    let mut n: Vec<String> = leaks(v).iter().map(|f| f["secret"].as_str().expect("secret").to_string()).collect();
    n.sort();
    n
}

/// A registered workspace under the temp HOME: (id, path).
fn workspace(e: &Env, tag: &str) -> (String, String) {
    let dir = e.home.path().join(tag);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, tag, path]).expect("insert project");
    (id, path)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn claude_file(e: &Env, ws_path: &str, name: &str) -> PathBuf {
    e.home.path().join(".claude/projects").join(claude_project_hash(ws_path)).join(name)
}

fn line_with(value: &str) -> String {
    format!("{{\"type\":\"user\",\"message\":\"{NEARBY} {value}\"}}\n")
}

/// Every run of 8 or more characters of `value` is absent from `body`: not
/// the value, and no fragment long enough to identify it.
fn assert_no_fragment(body: &str, value: &str) {
    for len in 8..=value.len() {
        for i in 0..=(value.len() - len) {
            let frag = &value[i..i + len];
            assert!(!body.contains(frag), "a piece of the value is in the answer ({len} chars at {i})");
        }
    }
}

#[test]
fn names_the_secret_and_file_and_never_the_value() {
    let e = env();
    let (_, path) = workspace(&e, "ops");
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let file = claude_file(&e, &path, "aaa.jsonl");
    write(&file, &line_with(PLANTED));
    let (body, v) = scan(&e, &owner());
    assert_no_fragment(&body, PLANTED);
    assert!(!body.contains(NEARBY), "a line of the transcript is in the answer");
    assert_eq!(leak_names(&v), vec!["STRIPE_KEY"], "{body}");
    let f = &leaks(&v)[0];
    assert_eq!(f["fileCount"], 1, "{body}");
    assert_eq!(f["cli"], "claude", "{body}");
    assert_eq!(f["workspace"], "ops", "{body}");
    assert_eq!(f["files"][0], file.to_string_lossy().as_ref(), "{body}");
    let msg = f["message"].as_str().expect("message");
    assert!(msg.contains("STRIPE_KEY appears in 1 file of ops's Claude transcripts (newest "), "{msg}");
    assert!(msg.contains("k2 vault rotate STRIPE_KEY") && msg.contains("Trash") && msg.contains("provider may already hold"), "{msg}");
    assert_eq!(v["ok"], false, "{body}");
    assert_eq!(v["complete"], true, "{body}");
}

#[test]
fn an_encoded_copy_is_found_and_its_encoding_is_not_printed() {
    use base64::Engine as _;
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let encoded = base64::engine::general_purpose::STANDARD.encode(PLANTED);
    write(&e.home.path().join(".codex/sessions/2026/11/08/rollout-1.jsonl"), &format!("{{\"auth\":\"{encoded}\"}}\n"));
    let (body, v) = scan(&e, &owner());
    assert_eq!(leak_names(&v), vec!["STRIPE_KEY"], "{body}");
    assert_no_fragment(&body, &encoded);
    assert_eq!(leaks(&v)[0]["cli"], "codex", "{body}");
}

/// A clean machine is `ok`, with nothing reported.
#[test]
fn no_copy_means_no_finding() {
    let e = env();
    let (_, path) = workspace(&e, "ops");
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    write(&claude_file(&e, &path, "aaa.jsonl"), "{\"type\":\"user\",\"message\":\"nothing here\"}\n");
    let (body, v) = scan(&e, &owner());
    assert!(leaks(&v).is_empty(), "{body}");
    assert_eq!(v["ok"], true, "{body}");
    assert_eq!(v["scan"]["files"], 1, "{body}");
}

/// A path can hold a value too (a folder named after a pasted key).
#[test]
fn a_value_in_a_path_is_redacted() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    write(&e.home.path().join(".pi/agent/sessions").join(format!("slug-{PLANTED}")).join("a.jsonl"), &line_with(PLANTED));
    let (body, v) = scan(&e, &owner());
    assert_no_fragment(&body, PLANTED);
    assert_eq!(leak_names(&v), vec!["STRIPE_KEY"], "{body}");
    assert!(leaks(&v)[0]["files"][0].as_str().expect("path").contains("[k2v:STRIPE_KEY]"), "{body}");
}

/// A value on a read-chunk boundary.
#[test]
fn a_value_across_a_read_chunk_is_found() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let mut text = "x".repeat(CHUNK - 5);
    text.push_str(PLANTED);
    write(&e.home.path().join(".codex/sessions/2026/11/08/big.jsonl"), &text);
    let (body, v) = scan(&e, &owner());
    assert_eq!(leak_names(&v), vec!["STRIPE_KEY"], "{body}");
}

/// V19: an agent's scan never checks a value it can't read.
#[test]
fn an_agent_does_not_see_a_fetch_only_hit() {
    let e = env();
    let (id, path) = workspace(&e, "ops");
    put(&e, VScope::workspace(&id), "FILE_KEY", PLANTED, Delivery::File);
    put(&e, VScope::workspace(&id), "FETCH_KEY", FETCH_ONLY, Delivery::Fetch);
    write(&claude_file(&e, &path, "aaa.jsonl"), &format!("{}{}", line_with(PLANTED), line_with(FETCH_ONLY)));
    let (body, v) = scan(&e, &agent(&id));
    assert_eq!(leak_names(&v), vec!["FILE_KEY"], "{body}");
    assert!(!body.contains("FETCH_KEY"), "{body}");
    assert_eq!(v["scope"], "workspace", "{body}");
    assert!(body.contains(ASK_OWNER), "{body}");
    assert_no_fragment(&body, PLANTED);
    let (body, v) = scan(&e, &owner());
    assert_eq!(leak_names(&v), vec!["FETCH_KEY", "FILE_KEY"], "the owner's scan sees both: {body}");
}

/// V19: another workspace's transcripts and the shared history files are
/// outside an agent's scan, even for a value it can read.
#[test]
fn an_agent_scans_only_its_own_workspace() {
    let e = env();
    let (id, path) = workspace(&e, "ops");
    let (_, other) = workspace(&e, "sales");
    put(&e, VScope::workspace(&id), "FILE_KEY", PLANTED, Delivery::File);
    write(&claude_file(&e, &other, "b.jsonl"), &line_with(PLANTED));
    write(&e.home.path().join(".claude/history.jsonl"), &line_with(PLANTED));
    write(&e.home.path().join(".hermes/state.db"), &line_with(PLANTED));
    let codex = e.home.path().join(".codex/sessions/2026/11/08");
    write(&codex.join("rollout-other.jsonl"), &format!("{{\"payload\":{{\"cwd\":\"{other}\"}}}}\n{}", line_with(PLANTED)));
    write(&codex.join("rollout-unknown.jsonl"), &line_with(PLANTED));
    let (body, v) = scan(&e, &agent(&id));
    assert_eq!(leak_names(&v), Vec::<String>::new(), "{body}");
    assert_eq!(v["scan"]["files"], 0, "{body}");

    write(&codex.join("rollout-mine.jsonl"), &format!("{{\"payload\":{{\"cwd\":\"{path}\"}}}}\n{}", line_with(PLANTED)));
    // The hash resolves links only for a folder that exists.
    std::fs::create_dir_all(format!("{path}/.worktrees/x")).expect("worktree dir");
    write(&claude_file(&e, &format!("{path}/.worktrees/x"), "c.jsonl"), &line_with(PLANTED));
    let (body, v) = scan(&e, &agent(&id));
    assert_eq!(v["scan"]["files"], 2, "{body}");
    assert_eq!(leaks(&v).len(), 2, "one finding per CLI: {body}");
    let (body, v) = scan(&e, &owner());
    assert_eq!(v["scan"]["files"], 7, "the owner's scan reads them all: {body}");
}

/// The rotated-out value is found for a person. An agent can't read old
/// versions, so its scan doesn't check them.
#[test]
fn an_older_version_is_found_for_a_person_only() {
    let e = env();
    let (id, path) = workspace(&e, "ops");
    put(&e, VScope::workspace(&id), "FILE_KEY", OLD_VALUE, Delivery::File);
    let sel = k2_vault::Selector::user(VScope::workspace(&id), "FILE_KEY");
    e.v.rotate(&sel, SecretBytes::from_slice(PLANTED.as_bytes()), &Actor::Owner).expect("rotate");
    write(&claude_file(&e, &path, "a.jsonl"), &line_with(OLD_VALUE));
    let (body, v) = scan(&e, &owner());
    assert_eq!(leak_names(&v), vec!["FILE_KEY"], "{body}");
    assert_no_fragment(&body, OLD_VALUE);
    let (body, v) = scan(&e, &agent(&id));
    assert!(leaks(&v).is_empty(), "an agent can't read the old version: {body}");
}

/// Newest first, stops at the budget, says so, and isn't `ok`.
#[test]
fn the_byte_budget_stops_the_scan_and_says_so() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let dir = e.home.path().join(".claude/projects/slug");
    let older = dir.join("older.jsonl");
    let newer = dir.join("newer.jsonl");
    write(&older, &format!("{}{}", "o".repeat(4096), line_with(PLANTED)));
    write(&newer, &format!("{}{}", line_with(PLANTED), "n".repeat(4096)));
    let old = std::time::SystemTime::now() - Duration::from_secs(86_400);
    File::options().write(true).open(&older).expect("open").set_modified(old).expect("mtime");
    let budget = std::fs::metadata(&newer).expect("stat").len();
    let v = run(&e.v, &owner(), Limits { bytes: budget, time: Duration::from_secs(60) }).expect("scan");
    let body = v.to_string();
    let leak = &leaks(&v)[0];
    assert_eq!(leak["fileCount"], 1, "only the newest was read: {body}");
    assert!(leak["files"][0].as_str().expect("path").ends_with("newer.jsonl"), "{body}");
    assert_eq!(v["complete"], false, "{body}");
    assert_eq!(v["ok"], false, "{body}");
    let capped: Vec<&Value> = v["findings"].as_array().expect("findings").iter().filter(|f| f["code"] == "transcript_scan_capped").collect();
    assert_eq!(capped.len(), 1, "{body}");
    assert_eq!(capped[0]["unchecked"], 1, "{body}");
    assert!(capped[0]["message"].as_str().expect("message").contains("older file(s) weren't checked"), "{body}");
    assert_eq!(v["scan"]["bytes"], budget, "{body}");
}

/// A budget that fits exactly stops nothing.
#[test]
fn a_budget_that_covers_everything_is_complete() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let f = e.home.path().join(".claude/projects/slug/a.jsonl");
    write(&f, &line_with("nothing"));
    let budget = std::fs::metadata(&f).expect("stat").len();
    let v = run(&e.v, &owner(), Limits { bytes: budget, time: Duration::from_secs(60) }).expect("scan");
    assert_eq!(v["complete"], true, "{v}");
    assert_eq!(v["ok"], true, "{v}");
}

/// K2's per-login folders and a config folder the daemon runs with are
/// transcript homes too.
#[test]
fn per_agent_config_folders_are_scanned() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let slot = store::slot_dir(Tool::Claude, "acct1");
    assert!(slot.starts_with(e.home.path()), "the wallet must be under the temp HOME");
    let in_slot = slot.join("projects/slug/a.jsonl");
    write(&in_slot, &line_with(PLANTED));
    let custom = e.home.path().join("custom-codex");
    let _var = EnvVar::set("CODEX_HOME", &custom);
    let in_custom = custom.join("sessions/2026/11/08/b.jsonl");
    write(&in_custom, &line_with(PLANTED));
    let (body, v) = scan(&e, &owner());
    assert_eq!(v["scan"]["files"], 2, "{body}");
    assert!(body.contains(in_slot.to_string_lossy().as_ref()) && body.contains(in_custom.to_string_lossy().as_ref()), "{body}");
}

/// A link planted in a transcript folder reads nothing, a FIFO doesn't hang.
#[cfg(unix)]
#[test]
fn links_and_fifos_are_not_read() {
    let e = env();
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    let outside = e.home.path().join("outside.txt");
    write(&outside, &line_with(PLANTED));
    let dir = e.home.path().join(".claude/projects/slug");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::os::unix::fs::symlink(&outside, dir.join("link.jsonl")).expect("symlink");
    std::os::unix::fs::symlink(e.home.path(), dir.join("linkdir")).expect("symlink");
    let fifo = std::ffi::CString::new(dir.join("pipe.jsonl").to_string_lossy().as_bytes()).expect("path");
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "mkfifo");
    let (body, v) = scan(&e, &owner());
    assert!(leaks(&v).is_empty(), "{body}");
    assert_eq!(v["scan"]["files"], 0, "{body}");
}

#[test]
fn a_member_or_guest_is_refused() {
    let e = env();
    let member = VaultCaller { actor: Actor::Login { username: "lee".into(), role: Role::Member }, channel: Channel::Login, session_id: None };
    let (s, body) = doctor_route(&e, &member);
    assert_eq!(s, 403, "{body}");
    assert!(body.contains("doctor_transcripts_admin_only") && body.contains(ASK_OWNER), "{body}");
    let guest = VaultCaller { actor: Actor::Guest { username: "g".into(), workspace_id: "w".into() }, channel: Channel::Plain, session_id: None };
    assert_eq!(doctor_route(&e, &guest).0, 403);
    let admin = VaultCaller { actor: Actor::Login { username: "ada".into(), role: Role::Admin }, channel: Channel::Login, session_id: None };
    assert_eq!(doctor_route(&e, &admin).0, 200);
}

/// A workspace K2 doesn't know refuses rather than guessing.
#[test]
fn an_unregistered_workspace_is_refused() {
    let e = env();
    let (s, body) = doctor_route(&e, &agent("no-such-workspace"));
    assert_eq!(s, 404, "{body}");
}

#[test]
fn one_scan_at_a_time() {
    let e = env();
    let held = RunGuard::take().expect("first");
    let (s, body) = doctor_route(&e, &owner());
    assert_eq!(s, 409, "{body}");
    assert!(body.contains("transcript_scan_running"), "{body}");
    drop(held);
    assert_eq!(doctor_route(&e, &owner()).0, 200);
}

#[test]
fn the_scan_is_audited_by_scope() {
    let e = env();
    let _ = scan(&e, &owner());
    let row = e.v.audit_consumers("doctor_transcripts", &Actor::Owner).expect("audit");
    assert_eq!(row, vec!["all".to_string()]);
}

#[test]
fn plain_doctor_does_not_scan_and_a_bad_flag_is_refused() {
    let e = env();
    let (_, path) = workspace(&e, "ops");
    put(&e, VScope::server(), "STRIPE_KEY", PLANTED, Delivery::Fetch);
    write(&claude_file(&e, &path, "a.jsonl"), &line_with(PLANTED));
    let r = handle_on(&e.v, DOCTOR, Ok(owner()), b"", &HashMap::new(), "test", None);
    assert!(r.status.starts_with("200"), "{}", r.body);
    assert!(!r.body.contains("transcript_leak"), "{}", r.body);
    let params = HashMap::from([("transcripts".to_string(), "yes".to_string())]);
    let r = handle_on(&e.v, DOCTOR, Ok(owner()), b"", &params, "test", None);
    assert!(r.status.starts_with("400") && r.body.contains("bad_transcripts_flag"), "{}", r.body);
}
