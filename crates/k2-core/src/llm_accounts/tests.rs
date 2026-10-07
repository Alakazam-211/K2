//! Wallet tests. Every test runs under a temp `HOME` (crate-wide
//! `HOME_LOCK`), with fake credential files that carry a marker string.
//! No real CLI, no keychain (`keychain_enabled()` is false under
//! `cfg(test)`), no real `~/.claude` / `~/.codex` / `~/.grok`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::MutexGuard;
use rusqlite::Connection;

use super::store::{self, CredMeta};
use super::wallet::{self, Refresher};
use super::*;
use crate::themes::HOME_LOCK;

const MARKER: &str = "K2TEST_SECRET_MARKER";

const SCRUB_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP", "K2_LLM_LOGIN_LIVE"];

struct Home {
    path: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    prev_vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Home {
    fn new(label: &str) -> Home {
        let lock = HOME_LOCK.lock();
        crate::test_isolation::assert_no_prod_env();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("k2-llm-acc-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(path.join(".k2")).unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_vars = SCRUB_VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
        std::env::set_var("HOME", &path);
        for v in SCRUB_VARS {
            std::env::remove_var(v);
        }
        crate::airgap::set_setting_enabled(false);
        Home { path, prev_home, prev_vars, _lock: lock }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        for (k, v) in self.prev_vars.drain(..) {
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

fn claude_cred(tag: &str, expires_ms: i64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "claudeAiOauth": {
            "accessToken": format!("{MARKER}-access-{tag}"),
            "refreshToken": format!("{MARKER}-refresh-{tag}"),
            "expiresAt": expires_ms,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        }
    }))
    .unwrap()
}

fn far_future_ms() -> i64 {
    (now() + 30 * 24 * 3600) * 1000
}

fn write_live_claude(h: &Home, bytes: &[u8]) {
    let p = h.p(".claude/.credentials.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn read_live_claude(h: &Home) -> Vec<u8> {
    fs::read(h.p(".claude/.credentials.json")).unwrap()
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn dump_db(conn: &Connection) -> String {
    let mut out = String::new();
    for table in ["llm_accounts", "llm_active"] {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
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

struct FakeRefresher {
    calls: AtomicUsize,
    fail_with: Option<String>,
}

impl Refresher for FakeRefresher {
    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        assert_eq!(tool, Tool::Claude, "fake refresher is claude-only");
        wallet::claude_refresh_slot(id, &|_url, _body| {
            Ok(serde_json::json!({"access_token": format!("{MARKER}-access-refreshed"), "refresh_token": format!("{MARKER}-refresh-rotated"), "expires_in": 28800}))
        })
    }
}

#[test]
fn migration_creates_wallet_tables_without_rows() {
    let conn = db();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM llm_accounts", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM llm_active", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn labels_validate_and_stay_unique_per_tool() {
    let _h = Home::new("labels");
    let conn = db();
    let a = create(&conn, Tool::Claude, "work", Some("owner-token")).unwrap();
    assert_eq!(a.state, state::NOT_SET_UP);
    assert_eq!(create(&conn, Tool::Claude, "WORK", None).unwrap_err().code(), "duplicate_label");
    create(&conn, Tool::Codex, "work", None).unwrap();
    assert_eq!(create(&conn, Tool::Claude, "  ", None).unwrap_err().code(), "invalid_label");
    assert_eq!(create(&conn, Tool::Claude, "a/b", None).unwrap_err().code(), "invalid_label");
    let b = create(&conn, Tool::Claude, "personal", None).unwrap();
    assert_eq!(a.kind, "subscription");
    assert_eq!((a.position, b.position), (0, 1), "new logins join the end of the pool");
    let order: Vec<String> = list_for(&conn, Tool::Claude).unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(order, vec![a.id.clone(), b.id.clone()]);
    assert_eq!(rename(&conn, &b.id, "work").unwrap_err().code(), "duplicate_label");
    assert_eq!(rename(&conn, &b.id, "home").unwrap().label, "home");
    assert_eq!(lookup(&conn, Some(Tool::Claude), "HOME").unwrap().id, b.id);
}

#[test]
fn boot_import_saves_the_live_login_as_the_active_default() {
    let h = Home::new("import");
    let conn = db();
    assert!(wallet::import_live(&conn, Tool::Claude, Some("boot")).unwrap().is_none(), "signed out → nothing");
    write_live_claude(&h, &claude_cred("a", far_future_ms()));
    fs::write(
        h.p(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"person@example.test","organizationName":"Example Org"}}"#,
    )
    .unwrap();
    let e = wallet::import_live(&conn, Tool::Claude, Some("boot")).unwrap().expect("imported");
    assert_eq!(e.label, "Default");
    assert_eq!(e.state, state::SIGNED_IN);
    assert_eq!(e.plan.as_deref(), Some("max"));
    assert_eq!(e.email.as_deref(), Some("person@example.test"));
    assert_eq!(e.org.as_deref(), Some("Example Org"));
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(e.id.as_str()));
    let slot = store::slot_cred_path(Tool::Claude, &e.id);
    assert_eq!(fs::read(&slot).unwrap(), read_live_claude(&h));
    #[cfg(unix)]
    {
        assert_eq!(mode(&slot), 0o600);
        assert_eq!(mode(slot.parent().unwrap()), 0o700);
        assert_eq!(mode(&store::wallet_root()), 0o700);
    }
    assert!(slot.starts_with(h.p(".k2/llm-accounts/claude")), "{slot:?}");
    // Second boot: already active → no new entry.
    assert!(wallet::import_live(&conn, Tool::Claude, Some("boot")).unwrap().is_none());
    assert_eq!(list(&conn).unwrap().len(), 1);
    assert!(!dump_db(&conn).contains(MARKER), "no token in the DB");
}

#[test]
fn switch_saves_outgoing_live_login_then_swaps_in_the_target() {
    let h = Home::new("switch");
    let conn = db();
    write_live_claude(&h, &claude_cred("a", far_future_ms()));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "second", None).unwrap();
    // B has no sign-in yet → refused, live untouched.
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "not_signed_in");
    store::write_slot(Tool::Claude, &b.id, &claude_cred("b", far_future_ms())).unwrap();
    // The CLI refreshed A's token in the live store since import.
    let a_refreshed = claude_cred("a-cli-refreshed", far_future_ms());
    write_live_claude(&h, &a_refreshed);
    let out = wallet::switch(&conn, Tool::Claude, "second", Some("user:alice"), None).unwrap();
    assert!(out.saved_outgoing);
    assert!(!out.already_active);
    assert_eq!(out.from.as_deref(), Some(a.id.as_str()));
    assert_eq!(read_live_claude(&h), claude_cred("b", far_future_ms()));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), a_refreshed, "outgoing saved back");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(b.id.as_str()));
    let b_row = get(&conn, &b.id).unwrap().unwrap();
    assert!(b_row.last_used_at.is_some());
    #[cfg(unix)]
    assert_eq!(mode(&h.p(".claude/.credentials.json")), 0o600);
    // Same target again → no-op.
    assert!(wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap().already_active);
    // next wraps back to A.
    assert_eq!(next_signed_in(&conn, Tool::Claude).unwrap().unwrap().id, a.id);
}

#[test]
fn switch_refreshes_an_expiring_idle_slot_first_and_never_under_airgap() {
    let h = Home::new("lazy");
    let conn = db();
    write_live_claude(&h, &claude_cred("a", far_future_ms()));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    store::write_slot(Tool::Claude, &b.id, &claude_cred("b", now() * 1000 + 60_000)).unwrap();
    let r = FakeRefresher { calls: AtomicUsize::new(0), fail_with: None };
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&r)).unwrap();
    assert!(out.refreshed);
    assert_eq!(r.calls.load(Ordering::SeqCst), 1);
    let live: serde_json::Value = serde_json::from_slice(&read_live_claude(&h)).unwrap();
    assert_eq!(live["claudeAiOauth"]["refreshToken"], format!("{MARKER}-refresh-rotated"));
    // Back to A, make A expiring, switch under air-gap: no refresh call.
    store::write_slot(Tool::Claude, &a.id, &claude_cred("a", now() * 1000 + 60_000)).unwrap();
    std::env::set_var("K2_AIRGAP", "1");
    let out = wallet::switch(&conn, Tool::Claude, &a.id, None, Some(&r)).unwrap();
    std::env::remove_var("K2_AIRGAP");
    assert!(!out.refreshed);
    assert_eq!(r.calls.load(Ordering::SeqCst), 1, "air-gap: no refresh");
}

#[test]
fn remove_refuses_the_active_login_and_trashes_an_idle_one() {
    let h = Home::new("remove");
    let conn = db();
    write_live_claude(&h, &claude_cred("a", far_future_ms()));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    assert_eq!(wallet::remove(&conn, &a.id).unwrap_err().code(), "active_login");
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    store::write_slot(Tool::Claude, &b.id, &claude_cred("b", far_future_ms())).unwrap();
    let out = wallet::remove(&conn, &b.id).unwrap();
    let moved = PathBuf::from(out.moved_to.unwrap());
    assert!(moved.starts_with(store::removed_root()));
    assert!(moved.join(".credentials.json").exists(), "Trash, never expunge");
    assert!(!store::slot_dir(Tool::Claude, &b.id).exists());
    assert!(list(&conn).unwrap().iter().all(|e| e.id != b.id));
    assert_eq!(read_live_claude(&h), claude_cred("a", far_future_ms()), "live untouched");
}

#[test]
fn keep_warm_refreshes_due_idle_slots_only() {
    let h = Home::new("warm");
    let conn = db();
    write_live_claude(&h, &claude_cred("a", now() * 1000 + 60_000)); // active, expiring
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    store::write_slot(Tool::Claude, &b.id, &claude_cred("b", now() * 1000 + 60_000)).unwrap();
    let c = create(&conn, Tool::Claude, "c", None).unwrap();
    store::write_slot(Tool::Claude, &c.id, &claude_cred("c", far_future_ms())).unwrap();
    update_meta(&conn, &c.id, &MetaUpdate { refreshed_at: Some(now()), ..Default::default() }).unwrap();
    let r = FakeRefresher { calls: AtomicUsize::new(0), fail_with: None };
    let res = wallet::keep_warm(&conn, &r, now()).unwrap();
    assert_eq!(res.len(), 1, "{res:?}");
    assert_eq!(res[0].id, b.id);
    assert!(res[0].refreshed);
    assert_eq!(r.calls.load(Ordering::SeqCst), 1, "never the active login, not the fresh one");
    let live: serde_json::Value = serde_json::from_slice(&read_live_claude(&h)).unwrap();
    assert_eq!(live["claudeAiOauth"]["refreshToken"], format!("{MARKER}-refresh-a"), "active slot never refreshed by K2");
    let _ = a;
    // A permanent failure marks the idle login needs_login.
    store::write_slot(Tool::Claude, &b.id, &claude_cred("b", now() * 1000 + 60_000)).unwrap();
    let bad = FakeRefresher { calls: AtomicUsize::new(0), fail_with: Some("400 invalid_grant".into()) };
    let res = wallet::keep_warm(&conn, &bad, now()).unwrap();
    assert_eq!(res.len(), 1);
    assert!(!res[0].refreshed);
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, state::NEEDS_LOGIN);
    // Air-gap: nothing at all.
    std::env::set_var("K2_AIRGAP", "1");
    let res = wallet::keep_warm(&conn, &bad, now() + 10 * 24 * 3600).unwrap();
    std::env::remove_var("K2_AIRGAP");
    assert!(res.is_empty());
    assert!(!dump_db(&conn).contains(MARKER));
}

#[test]
fn warm_due_rules() {
    let e = Entry {
        id: "acc_x".into(),
        tool: "claude".into(),
        label: "x".into(),
        kind: "subscription".into(),
        position: 0,
        created_by: None,
        created_at: 1_000,
        last_used_at: None,
        state: state::SIGNED_IN.into(),
        detail: None,
        email: None,
        org: None,
        plan: None,
        expires_at: None,
        refreshed_at: Some(1_000),
        usage_json: None,
        usage_checked_at: None,
        removed_at: None,
    };
    let fresh = CredMeta { present: true, has_refresh: true, expires_at: Some(1_000 + 7200), ..Default::default() };
    assert!(!wallet::warm_due(Tool::Claude, &fresh, &e, 1_100));
    assert!(wallet::warm_due(Tool::Claude, &fresh, &e, 1_000 + 7200 - 100), "near expiry");
    assert!(wallet::warm_due(Tool::Claude, &fresh, &e, 1_000 + wallet::WARM_EVERY_SECS), "daily roll");
    let no_rt = CredMeta { present: true, has_refresh: false, ..fresh.clone() };
    assert!(!wallet::warm_due(Tool::Claude, &no_rt, &e, 1_000 + wallet::WARM_EVERY_SECS));
    assert!(!wallet::warm_due(Tool::Claude, &CredMeta::default(), &e, 9_999_999));
}

#[test]
fn claude_refresh_grant_shape_and_apply() {
    let slot = claude_cred("x", 1);
    let body = wallet::test_access::claude_refresh_body(&slot).unwrap();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["client_id"], wallet::CLAUDE_CLIENT_ID);
    assert_eq!(body["scope"], "user:inference user:profile");
    assert_eq!(body["refresh_token"], format!("{MARKER}-refresh-x"));
    let resp = serde_json::json!({"access_token":"new-at","expires_in":3600,"scope":"user:inference"});
    let out = wallet::test_access::claude_apply_refresh(&slot, &resp, 5_000).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["claudeAiOauth"]["accessToken"], "new-at");
    assert_eq!(v["claudeAiOauth"]["expiresAt"], 5_000 + 3_600_000, "milliseconds");
    assert_eq!(v["claudeAiOauth"]["refreshToken"], format!("{MARKER}-refresh-x"), "kept when not rotated");
    assert_eq!(v["claudeAiOauth"]["subscriptionType"], "max");
    assert!(wallet::test_access::claude_apply_refresh(&slot, &serde_json::json!({"error":"invalid_grant"}), 0).is_err());
}

#[test]
fn temp_home_login_finalize_records_and_activates_only_when_nothing_is_active() {
    let h = Home::new("finalize");
    let conn = db();
    let e = wallet::begin_new(&conn, Tool::Codex, "work", Some("user:alice")).unwrap();
    assert_eq!(e.state, state::SIGNING_IN);
    // Sign-in that never finished.
    let err = wallet::finalize_login(&conn, &e.id, None).unwrap_err();
    assert_eq!(err.code(), "not_signed_in");
    assert_eq!(get(&conn, &e.id).unwrap().unwrap().state, state::NOT_SET_UP);
    // The tool's login wrote auth.json into the slot (fake codex token).
    let jwt = |claims: serde_json::Value| {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", enc.encode(b"{}"), enc.encode(claims.to_string()))
    };
    let auth = serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": jwt(serde_json::json!({"email":"coder@example.test","https://api.openai.com/auth":{"chatgpt_plan_type":"pro"}})),
            "access_token": jwt(serde_json::json!({"exp": 4_000_000_000i64, "marker": MARKER})),
            "refresh_token": format!("{MARKER}-rt"),
        },
        "last_refresh": "2026-10-07T00:00:00Z"
    });
    fs::write(store::slot_cred_path(Tool::Codex, &e.id), serde_json::to_vec(&auth).unwrap()).unwrap();
    let done = wallet::finalize_login(&conn, &e.id, Some("user:alice")).unwrap();
    assert_eq!(done.state, state::SIGNED_IN);
    assert_eq!(done.email.as_deref(), Some("coder@example.test"));
    assert_eq!(done.plan.as_deref(), Some("pro"));
    assert_eq!(done.expires_at, Some(4_000_000_000));
    #[cfg(unix)]
    assert_eq!(mode(&store::slot_cred_path(Tool::Codex, &e.id)), 0o600);
    // Nothing was active and the live store was empty → it became active.
    assert_eq!(active_id(&conn, Tool::Codex).unwrap().as_deref(), Some(done.id.as_str()));
    assert!(h.p(".codex/auth.json").exists());
    // A second login does not touch the live store.
    let live_before = fs::read(h.p(".codex/auth.json")).unwrap();
    let e2 = wallet::begin_new(&conn, Tool::Codex, "second", None).unwrap();
    fs::write(store::slot_cred_path(Tool::Codex, &e2.id), b"{\"tokens\":{\"refresh_token\":\"x\"}}").unwrap();
    wallet::finalize_login(&conn, &e2.id, None).unwrap();
    assert_eq!(fs::read(h.p(".codex/auth.json")).unwrap(), live_before);
    assert_eq!(active_id(&conn, Tool::Codex).unwrap().as_deref(), Some(done.id.as_str()));
    assert!(!dump_db(&conn).contains(MARKER));
}

#[test]
fn discard_unfinished_deletes_only_empty_new_logins() {
    let _h = Home::new("discard");
    let conn = db();
    let e = wallet::begin_new(&conn, Tool::Grok, "g", None).unwrap();
    assert!(store::slot_dir(Tool::Grok, &e.id).exists());
    wallet::discard_unfinished(&conn, &e.id).unwrap();
    assert!(get(&conn, &e.id).unwrap().is_none());
    assert!(!store::slot_dir(Tool::Grok, &e.id).exists());
    let keep = wallet::begin_new(&conn, Tool::Grok, "kept", None).unwrap();
    store::write_slot(Tool::Grok, &keep.id, b"{\"refresh\":\"x\"}").unwrap();
    wallet::discard_unfinished(&conn, &keep.id).unwrap();
    assert!(get(&conn, &keep.id).unwrap().is_some(), "a saved sign-in is never discarded");
}

#[test]
fn live_swap_captures_the_new_login_and_restores_the_previous_one() {
    let h = Home::new("liveswap");
    let conn = db();
    write_live_claude(&h, &claude_cred("prev", far_future_ms()));
    let new = wallet::begin_new(&conn, Tool::Claude, "new", None).unwrap();
    let ticket = wallet::begin_live_swap(&conn, Tool::Claude, &new.id, None).unwrap();
    let prev = ticket.previous_id.clone().expect("the live login was imported as Default");
    assert_eq!(get(&conn, &prev).unwrap().unwrap().label, "Default");
    assert!(wallet::live_swap_poll(&ticket).unwrap().is_none(), "unchanged");
    // The tool's normal login replaced the live credential.
    write_live_claude(&h, &claude_cred("new", far_future_ms()));
    let changed = wallet::live_swap_poll(&ticket).unwrap().expect("changed");
    let e = wallet::live_swap_complete(&conn, &ticket, &changed, None).unwrap();
    assert_eq!(e.state, state::SIGNED_IN);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &new.id)).unwrap(), claude_cred("new", far_future_ms()));
    assert_eq!(read_live_claude(&h), claude_cred("prev", far_future_ms()), "previous login restored");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(prev.as_str()), "nobody moved");
}

#[test]
fn live_swap_abort_restores_the_snapshot_and_deletes_the_half_made_slot() {
    let h = Home::new("liveabort");
    let conn = db();
    write_live_claude(&h, &claude_cred("prev", far_future_ms()));
    let new = wallet::begin_new(&conn, Tool::Claude, "new", None).unwrap();
    let ticket = wallet::begin_live_swap(&conn, Tool::Claude, &new.id, None).unwrap();
    // A half-finished login scribbled over the live store.
    write_live_claude(&h, b"{\"claudeAiOauth\":{}}");
    wallet::live_swap_abort(&conn, &ticket).unwrap();
    assert_eq!(read_live_claude(&h), claude_cred("prev", far_future_ms()));
    assert!(get(&conn, &new.id).unwrap().is_none());
    assert!(!store::slot_dir(Tool::Claude, &new.id).exists());
}

#[test]
fn live_swap_with_no_previous_login_keeps_the_new_one_active() {
    let h = Home::new("livefirst");
    let conn = db();
    let new = wallet::begin_new(&conn, Tool::Grok, "first", None).unwrap();
    let ticket = wallet::begin_live_swap(&conn, Tool::Grok, &new.id, None).unwrap();
    assert!(ticket.previous_id.is_none());
    fs::create_dir_all(h.p(".grok")).unwrap();
    fs::write(h.p(".grok/auth.json"), format!("{{\"refresh\":\"{MARKER}\"}}")).unwrap();
    let bytes = wallet::live_swap_poll(&ticket).unwrap().unwrap();
    wallet::live_swap_complete(&conn, &ticket, &bytes, None).unwrap();
    assert_eq!(active_id(&conn, Tool::Grok).unwrap().as_deref(), Some(new.id.as_str()));
    assert!(store::slot_has_cred(Tool::Grok, &new.id));
}

#[test]
fn login_method_defaults_to_temp_home_with_a_live_swap_escape_hatch() {
    let _h = Home::new("method");
    for t in Tool::ALL {
        assert_eq!(wallet::login_method(t), wallet::LoginMethod::TempHome);
    }
    std::env::set_var("K2_LLM_LOGIN_LIVE", "grok");
    assert_eq!(wallet::login_method(Tool::Grok), wallet::LoginMethod::LiveSwap);
    assert_eq!(wallet::login_method(Tool::Claude), wallet::LoginMethod::TempHome);
    std::env::remove_var("K2_LLM_LOGIN_LIVE");
}

#[test]
fn grok_live_write_takes_groks_own_lock() {
    let h = Home::new("groklock");
    fs::create_dir_all(h.p(".grok")).unwrap();
    store::write_live(Tool::Grok, b"{\"a\":1}").unwrap();
    assert!(h.p(".grok/auth.json.lock").exists(), "flock file created");
    let held = store::FileLock::acquire(&h.p(".grok/auth.json.lock"), std::time::Duration::from_millis(100)).unwrap();
    let busy = store::FileLock::acquire(&h.p(".grok/auth.json.lock"), std::time::Duration::from_millis(100));
    assert!(busy.is_err(), "flock is exclusive");
    drop(held);
}

#[test]
fn keychain_is_never_touched_in_tests_and_naming_follows_claude() {
    assert!(!store::keychain_enabled());
    assert_eq!(store::claude_keychain_service(None), "Claude Code-credentials");
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(b"/tmp/slot");
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        store::claude_keychain_service(Some(Path::new("/tmp/slot"))),
        format!("Claude Code-credentials-{}", &hex[..8])
    );
}

#[test]
fn secret_shaped_metadata_is_refused() {
    assert!(looks_like_secret("sk-ant-oat01-abc"));
    assert!(looks_like_secret("accessToken=abc"));
    assert!(looks_like_secret(&"A".repeat(48)));
    assert!(!looks_like_secret("person@example.test"));
    assert!(!looks_like_secret("refresh failed: 400 Bad Request"));
    let _h = Home::new("scrub");
    let conn = db();
    let e = create(&conn, Tool::Claude, "x", None).unwrap();
    let err = update_meta(&conn, &e.id, &MetaUpdate { email: Some("sk-ant-oat01-zzz".into()), ..Default::default() }).unwrap_err();
    assert_eq!(err.code(), "io");
}
