//! Pins, API keys and the per-session login decision. Temp HOME, fake
//! credential files with a marker, no real CLI or keychain.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::MutexGuard;
use rusqlite::Connection;

use super::pins::{self, ScopeKind, Source};
use super::store;
use super::wallet::{self, Refresher};
use super::*;
use crate::themes::HOME_LOCK;

const MARKER: &str = "K2TEST_SECRET_MARKER";
const API_KEY: &str = "sk-test-K2TEST_SECRET_MARKER-0123456789";

struct Home {
    path: PathBuf,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

const VARS: &[&str] = &["HOME", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP"];

impl Home {
    fn new(label: &str) -> Home {
        let lock = HOME_LOCK.lock();
        crate::test_isolation::assert_no_prod_env();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("k2-llm-pins-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(path.join(".k2")).unwrap();
        let prev = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
        for v in VARS {
            std::env::remove_var(v);
        }
        std::env::set_var("HOME", &path);
        crate::airgap::set_setting_enabled(false);
        Home { path, prev, _lock: lock }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        for (k, v) in self.prev.drain(..) {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn db() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn cred(tag: &str) -> Vec<u8> {
    format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":{}}}}}"#,
        (now() + 30 * 24 * 3600) * 1000
    )
    .into_bytes()
}

fn signed_in(conn: &Connection, tool: Tool, label: &str) -> Entry {
    let e = create(conn, tool, label, None).unwrap();
    store::write_slot(tool, &e.id, &cred(label)).unwrap();
    update_meta(conn, &e.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    get(conn, &e.id).unwrap().unwrap()
}

/// Pool with a live Default + two idle Claude logins.
fn pool(h: &Home, conn: &Connection) -> (Entry, Entry, Entry) {
    fs::create_dir_all(h.p(".claude")).unwrap();
    fs::write(h.p(".claude/.credentials.json"), cred("live")).unwrap();
    let d = wallet::import_live(conn, Tool::Claude, None).unwrap().unwrap();
    let a = signed_in(conn, Tool::Claude, "a");
    let b = signed_in(conn, Tool::Claude, "b");
    (d, a, b)
}

fn dump(conn: &Connection) -> String {
    let mut out = String::new();
    for t in ["llm_accounts", "llm_active", "llm_account_pins", "llm_session_logins"] {
        let mut st = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            for i in 0..n {
                let v: rusqlite::types::Value = r.get(i).unwrap();
                out.push_str(&format!("{v:?}|"));
            }
            out.push('\n');
        }
    }
    out
}

#[test]
fn precedence_session_pin_beats_workspace_pin_beats_pool() {
    let h = Home::new("prec");
    let conn = db();
    let (_d, a, b) = pool(&h, &conn);
    // No pins → pool → live home, no env.
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-1", Some("proj-1"), None, false).unwrap();
    assert_eq!(l.source, Source::Pool);
    assert!(l.env.is_empty() && l.home.is_none() && l.account_id.is_none());
    // Workspace pin.
    pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &a.id, Some("owner-token")).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-1", Some("proj-1"), None, false).unwrap();
    assert_eq!(l.source, Source::WorkspacePin);
    assert_eq!(l.account_id.as_deref(), Some(a.id.as_str()));
    let slot_a = store::slot_dir(Tool::Claude, &a.id);
    assert_eq!(l.env, vec![("CLAUDE_CONFIG_DIR".to_string(), slot_a.to_string_lossy().into_owned())]);
    // Another workspace stays on the pool.
    let other = pins::decide_spawn(&conn, Tool::Claude, "tab-9", Some("proj-2"), None, false).unwrap();
    assert_eq!(other.source, Source::Pool);
    // Session pin wins over the workspace pin.
    pins::pin(&conn, ScopeKind::Session, "tab-1", Some(Tool::Claude), "b", None).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-1", Some("proj-1"), None, false).unwrap();
    assert_eq!(l.source, Source::SessionPin);
    assert_eq!(l.account_id.as_deref(), Some(b.id.as_str()));
    // Pins are per tool: Codex in the same workspace is on the pool.
    let c = pins::decide_spawn(&conn, Tool::Codex, "tab-1", Some("proj-1"), None, false).unwrap();
    assert_eq!(c.source, Source::Pool);
    // Unpin the session → back to the workspace pin.
    assert!(pins::unpin(&conn, ScopeKind::Session, "tab-1", Tool::Claude).unwrap());
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-1", Some("proj-1"), None, false).unwrap();
    assert_eq!(l.account_id.as_deref(), Some(a.id.as_str()));
}

#[test]
fn resume_uses_the_recorded_login_even_after_pins_change() {
    let h = Home::new("resume");
    let conn = db();
    let (_d, a, b) = pool(&h, &conn);
    pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &a.id, None).unwrap();
    // Fresh pinned spawn with a pre-minted conversation id.
    let l = pins::decide_spawn(&conn, Tool::Claude, "proj-1", Some("proj-1"), Some("conv-1"), false).unwrap();
    pins::record_spawn(&conn, &l, "proj-1", Some("conv-1")).unwrap();
    // A pool conversation on another key.
    let p = pins::decide_spawn(&conn, Tool::Claude, "tab-x", None, Some("conv-pool"), false).unwrap();
    pins::record_spawn(&conn, &p, "tab-x", Some("conv-pool")).unwrap();
    // The pin moves to b.
    pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &b.id, None).unwrap();
    let r = pins::decide_spawn(&conn, Tool::Claude, "proj-1", Some("proj-1"), Some("conv-1"), true).unwrap();
    assert_eq!(r.source, Source::Resume);
    assert_eq!(r.account_id.as_deref(), Some(a.id.as_str()), "resume keeps the conversation's own home");
    assert_eq!(r.home, Some(store::slot_dir(Tool::Claude, &a.id)));
    // A pool conversation resumes in the live home even inside a pinned workspace.
    pins::pin(&conn, ScopeKind::Workspace, "proj-x", None, &b.id, None).unwrap();
    let r = pins::decide_spawn(&conn, Tool::Claude, "tab-x", Some("proj-x"), Some("conv-pool"), true).unwrap();
    assert!(r.account_id.is_none() && r.env.is_empty(), "started in the live home → stays there");
    // A conversation the CLI minted itself (no id at spawn) uses the key's record.
    let r = pins::decide_spawn(&conn, Tool::Claude, "proj-1", Some("proj-1"), Some("conv-unknown"), true).unwrap();
    assert_eq!(r.account_id.as_deref(), Some(a.id.as_str()));
    // A conversation from before K2 tracked logins → live home.
    let r = pins::decide_spawn(&conn, Tool::Claude, "tab-new", Some("proj-1"), Some("conv-old"), true).unwrap();
    assert!(r.account_id.is_none());
}

#[test]
fn pin_and_pool_are_exclusive_for_subscription_logins() {
    let h = Home::new("excl");
    let conn = db();
    let (d, a, b) = pool(&h, &conn);
    // The pool's live login can't be pinned.
    let err = pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &d.id, None).unwrap_err();
    assert_eq!(err.code(), "pinned_active");
    // A pinned login can't become the pool's active login…
    pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &a.id, None).unwrap();
    let err = wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap_err();
    assert_eq!(err.code(), "login_pinned");
    // …is skipped by "switch to next"…
    assert_eq!(next_signed_in(&conn, Tool::Claude).unwrap().unwrap().id, b.id);
    // …can't be removed or signed in again while pinned.
    assert_eq!(wallet::remove(&conn, &a.id).unwrap_err().code(), "login_pinned");
    assert_eq!(wallet::begin_relogin(&conn, &a.id).unwrap_err().code(), "login_pinned");
    // Unpinned → the pool may use it again.
    pins::unpin(&conn, ScopeKind::Workspace, "proj-1", Tool::Claude).unwrap();
    wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    assert_eq!(fs::read(h.p(".claude/.credentials.json")).unwrap(), cred("a"));
    // And now `a` is live → can't be pinned; the old Default can.
    assert_eq!(pins::pin(&conn, ScopeKind::Session, "s", None, &a.id, None).unwrap_err().code(), "pinned_active");
    pins::pin(&conn, ScopeKind::Session, "s", None, &d.id, None).unwrap();
    // A login with no saved sign-in can't be pinned.
    let empty = create(&conn, Tool::Claude, "empty", None).unwrap();
    assert_eq!(pins::pin(&conn, ScopeKind::Session, "s2", None, &empty.id, None).unwrap_err().code(), "not_signed_in");
}

struct CountingRefresher(AtomicUsize, std::sync::Mutex<Vec<String>>);
impl Refresher for CountingRefresher {
    fn refresh(&self, _tool: Tool, id: &str) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        self.1.lock().unwrap().push(id.to_string());
        Ok(())
    }
}

#[test]
fn keep_warm_never_refreshes_the_live_login_or_a_pinned_slot_in_use() {
    let h = Home::new("own");
    let conn = db();
    let (d, a, b) = pool(&h, &conn);
    let k = add_api_key(&conn, Tool::Claude, "key", API_KEY);
    let k = k.unwrap();
    pins::pin(&conn, ScopeKind::Workspace, "proj-1", None, &a.id, None).unwrap();
    // Everyone is due (old refresh).
    for id in [&d.id, &a.id, &b.id] {
        update_meta(&conn, id, &MetaUpdate { refreshed_at: Some(1), ..Default::default() }).unwrap();
    }
    let r = CountingRefresher(AtomicUsize::new(0), Default::default());
    let a_id = a.id.clone();
    let res = wallet::keep_warm(&conn, &r, now(), &move |id| id == a_id).unwrap();
    let refreshed: Vec<String> = r.1.lock().unwrap().clone();
    assert_eq!(refreshed, vec![b.id.clone()], "only the idle, unpinned-in-use slot: {res:?}");
    assert!(!refreshed.contains(&d.id), "the live login is the CLI's");
    assert!(!refreshed.contains(&k.id), "API keys never refresh");
    // The pinned slot with no running session is idle → K2 keeps it warm.
    let r2 = CountingRefresher(AtomicUsize::new(0), Default::default());
    wallet::keep_warm(&conn, &r2, now() + 2 * wallet::WARM_EVERY_SECS, &|_| false).unwrap();
    assert!(r2.1.lock().unwrap().contains(&a.id));
    assert!(!r2.1.lock().unwrap().contains(&d.id));
}

fn add_api_key(conn: &Connection, tool: Tool, label: &str, key: &str) -> Result<Entry, WalletError> {
    pins::add_api_key(conn, tool, label, key, Some("owner-token"))
}

#[test]
fn api_key_logins_inject_env_and_never_leave_the_slot() {
    let h = Home::new("apikey");
    let conn = db();
    let (_d, a, _b) = pool(&h, &conn);
    assert_eq!(add_api_key(&conn, Tool::Claude, "bad", "has space").unwrap_err().code(), "invalid_label");
    let k = add_api_key(&conn, Tool::Claude, "Metered", API_KEY).unwrap();
    assert_eq!(k.kind, kind::API_KEY);
    assert_eq!(k.state, state::SIGNED_IN);
    let path = store::api_key_path(Tool::Claude, &k.id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!dump(&conn).contains(MARKER), "the key never reaches the DB");
    // Pinned: env only, no home.
    pins::pin(&conn, ScopeKind::Session, "tab-k", None, &k.id, None).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-k", None, None, false).unwrap();
    assert_eq!(l.env, vec![("ANTHROPIC_API_KEY".to_string(), API_KEY.to_string())]);
    assert!(l.home.is_none());
    pins::record_spawn(&conn, &l, "tab-k", Some("c1")).unwrap();
    assert!(!dump(&conn).contains(MARKER), "recording a session never stores the key");
    // In the pool: active without touching the live store; new pool sessions get the env.
    let live_before = fs::read(h.p(".claude/.credentials.json")).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &k.id, None, None).unwrap();
    assert!(!out.saved_outgoing);
    assert_eq!(fs::read(h.p(".claude/.credentials.json")).unwrap(), live_before, "live store untouched");
    let pool_l = pins::decide_spawn(&conn, Tool::Claude, "tab-z", None, None, false).unwrap();
    assert_eq!(pool_l.env, vec![("ANTHROPIC_API_KEY".to_string(), API_KEY.to_string())]);
    // The subscription left in the live store still counts as live.
    let owner = live_owner(&conn, Tool::Claude).unwrap().unwrap();
    assert_eq!(get(&conn, &owner).unwrap().unwrap().label, "Default");
    // API keys aren't in the "next" cycle.
    let nxt = next_signed_in(&conn, Tool::Claude).unwrap().unwrap();
    assert!(!nxt.is_api_key());
    // Back to a subscription: the live store's owner is saved first.
    fs::write(h.p(".claude/.credentials.json"), cred("live-cli-refreshed")).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    assert!(out.saved_outgoing);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &owner)).unwrap(), cred("live-cli-refreshed"));
    assert_eq!(fs::read(h.p(".claude/.credentials.json")).unwrap(), cred("a"));
    assert_eq!(live_owner(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
    // API key active again, then back to the login still in the live store: no copy.
    wallet::switch(&conn, Tool::Claude, &k.id, None, None).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    assert!(!out.saved_outgoing && !out.already_active);
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
    // Codex sets both names it reads; Gemini supports keys only.
    let ck = add_api_key(&conn, Tool::Codex, "ck", API_KEY).unwrap();
    pins::pin(&conn, ScopeKind::Workspace, "p", None, &ck.id, None).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Codex, "t", Some("p"), None, false).unwrap();
    let names: Vec<&str> = l.env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, vec!["CODEX_API_KEY", "OPENAI_API_KEY"]);
    assert_eq!(create(&conn, Tool::Gemini, "sub", None).unwrap_err().code(), "live_store_unavailable");
    let gk = add_api_key(&conn, Tool::Gemini, "gk", API_KEY).unwrap();
    pins::pin(&conn, ScopeKind::Workspace, "p", None, &gk.id, None).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Gemini, "t", Some("p"), None, false).unwrap();
    assert_eq!(l.env[0].0, "GEMINI_API_KEY");
}

fn is_link_to(p: &Path, target: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
        && fs::read_link(p).unwrap() == target
}

#[test]
fn codex_shadow_home_layout() {
    let h = Home::new("shadow");
    let shared = h.p(".codex");
    fs::create_dir_all(shared.join("skills")).unwrap();
    fs::write(shared.join("history.jsonl"), "{}\n").unwrap();
    fs::write(shared.join("state_5.sqlite"), "db").unwrap();
    fs::write(shared.join("auth.json"), format!("{{\"live\":\"{MARKER}\"}}")).unwrap();
    fs::write(shared.join("config.toml"), "model = \"gpt-x\"\ncli_auth_credentials_store = \"keyring\"\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n").unwrap();
    let slot = h.p(".k2/llm-accounts/codex/acc_x");
    fs::create_dir_all(&slot).unwrap();
    fs::write(slot.join("auth.json"), "{\"slot\":true}").unwrap();
    // A sign-in run left a real sessions folder in the slot.
    fs::create_dir_all(slot.join("sessions")).unwrap();
    fs::write(slot.join("sessions/stray.jsonl"), "x").unwrap();
    pins::materialize_codex_shadow(&slot, &shared).unwrap();
    // Private: the slot's own login, never linked.
    assert_eq!(fs::read_to_string(slot.join("auth.json")).unwrap(), "{\"slot\":true}");
    assert!(!fs::symlink_metadata(slot.join("auth.json")).unwrap().file_type().is_symlink());
    // Shared: sessions (created in the shared home), history, sqlite, skills.
    assert!(shared.join("sessions").is_dir());
    for name in ["sessions", "history.jsonl", "state_5.sqlite", "skills"] {
        assert!(is_link_to(&slot.join(name), &shared.join(name)), "{name} must link to the shared home");
    }
    // The stray folder was moved aside, not deleted.
    let aside: Vec<_> = fs::read_dir(&slot)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("sessions.k2-local-"))
        .collect();
    assert_eq!(aside.len(), 1);
    // Local folders stay real.
    for d in pins::CODEX_LOCAL_DIRS {
        let m = fs::symlink_metadata(slot.join(d)).unwrap();
        assert!(m.is_dir() && !m.file_type().is_symlink(), "{d}");
    }
    // config.toml: a private copy forcing the file store, keeping the rest.
    let cfg = fs::read_to_string(slot.join("config.toml")).unwrap();
    assert!(cfg.contains("cli_auth_credentials_store = \"file\""));
    assert!(!cfg.contains("keyring"));
    assert!(cfg.contains("model = \"gpt-x\"") && cfg.contains("trust_level = \"trusted\""));
    assert!(!fs::symlink_metadata(slot.join("config.toml")).unwrap().file_type().is_symlink());
    // A new shared entry appears on the next run; re-runs are idempotent.
    fs::write(shared.join("new_file.json"), "{}").unwrap();
    pins::materialize_codex_shadow(&slot, &shared).unwrap();
    assert!(is_link_to(&slot.join("new_file.json"), &shared.join("new_file.json")));
    // A symlinked auth.json is refused.
    fs::remove_file(slot.join("auth.json")).unwrap();
    std::os::unix::fs::symlink(shared.join("auth.json"), slot.join("auth.json")).unwrap();
    let err = pins::materialize_codex_shadow(&slot, &shared).unwrap_err();
    assert!(err.contains("symlinked auth.json"), "{err}");
}

#[test]
fn claude_pinned_home_links_settings_and_trusts_the_folder() {
    let h = Home::new("claudehome");
    let shared = h.p(".claude");
    fs::create_dir_all(shared.join("skills")).unwrap();
    fs::write(shared.join("settings.json"), "{\"hooks\":{}}").unwrap();
    fs::create_dir_all(shared.join("projects")).unwrap();
    let slot = h.p(".k2/llm-accounts/claude/acc_y");
    fs::create_dir_all(&slot).unwrap();
    fs::write(slot.join(".claude.json"), "{\"oauthAccount\":{\"emailAddress\":\"x@example.test\"}}").unwrap();
    let cwd = h.p("work");
    pins::prepare_claude_home(&slot, &shared, Some(&cwd)).unwrap();
    assert!(is_link_to(&slot.join("settings.json"), &shared.join("settings.json")));
    assert!(is_link_to(&slot.join("skills"), &shared.join("skills")));
    assert!(!slot.join("projects").exists(), "history stays with the slot, never linked");
    let v: serde_json::Value = serde_json::from_slice(&fs::read(slot.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(v["projects"][cwd.to_string_lossy().as_ref()]["hasTrustDialogAccepted"], true);
    assert_eq!(v["oauthAccount"]["emailAddress"], "x@example.test", "merge, not overwrite");
}

#[test]
fn removed_login_resume_is_refused_not_silently_moved() {
    let h = Home::new("removed");
    let conn = db();
    let (_d, a, _b) = pool(&h, &conn);
    pins::pin(&conn, ScopeKind::Session, "tab-1", None, &a.id, None).unwrap();
    let l = pins::decide_spawn(&conn, Tool::Claude, "tab-1", None, Some("c9"), false).unwrap();
    pins::record_spawn(&conn, &l, "tab-1", Some("c9")).unwrap();
    pins::unpin(&conn, ScopeKind::Session, "tab-1", Tool::Claude).unwrap();
    wallet::remove(&conn, &a.id).unwrap();
    let err = pins::decide_spawn(&conn, Tool::Claude, "tab-1", None, Some("c9"), true).unwrap_err();
    assert_eq!(err.code(), "account_unavailable");
}
