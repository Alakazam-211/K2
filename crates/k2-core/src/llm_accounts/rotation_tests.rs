//! Refresh-token rotation (field report 2026-11-17: subscription logins
//! "signed out" on a headless Linux server where every agent shares one
//! `~/.claude/.credentials.json`). The provider rotates the refresh token
//! on every refresh; a rotated-away one gets `400 invalid_grant`. The CLIs
//! refresh the live store; K2 must never refresh its own copy of that
//! login, must read the newer set back, must refresh any one login once
//! at a time, and must not mark a login "sign in again" on one refusal.
//!
//! Temp `HOME` under the ONE env lock; no keychain, no network: a fake
//! token endpoint ([`RotatingVendor`]) behind `wallet::claude_refresh_slot`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{json, Value};

use super::identity::{self, AccountIdentity, Source};
use super::store;
use super::wallet::{self, Refresher};
use super::*;

const MARKER: &str = "K2TEST_SECRET_MARKER";
const SCRUB_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP", "K2_LLM_LOGIN_LIVE"];
const FAR: i64 = 4_102_444_800_000;

struct Home {
    _vars: Vec<crate::test_env::EnvVar>,
    home: crate::test_env::TempHome,
}

impl Home {
    fn new() -> Home {
        let home = crate::test_env::TempHome::new();
        let _vars = SCRUB_VARS.iter().map(|v| crate::test_env::EnvVar::remove(v)).collect();
        crate::airgap::set_setting_enabled(false);
        Home { _vars, home }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.home.path().join(rel)
    }
}

fn db() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn cred(tag: &str, expires_ms: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({
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

/// Expires in a minute: due for keep-warm.
fn soon() -> i64 {
    now() * 1000 + 60_000
}

/// Expires in eight hours (what a refresh hands out).
fn later() -> i64 {
    now() * 1000 + 8 * 3600 * 1000
}

fn write_live(h: &Home, bytes: &[u8]) {
    let p = h.p(".claude/.credentials.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn live(h: &Home) -> Vec<u8> {
    fs::read(h.p(".claude/.credentials.json")).unwrap()
}

/// `~/.claude.json` as Claude writes it after a sign-in.
fn live_profile_names(h: &Home, uuid: &str, email: &str) {
    fs::write(h.p(".claude.json"), json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}}).to_string()).unwrap();
}

fn refresh_token(bytes: &[u8]) -> String {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["refreshToken"].as_str().unwrap().to_string()
}

fn slot(id: &str) -> Vec<u8> {
    fs::read(store::slot_cred_path(Tool::Claude, id)).unwrap()
}

/// A fake token endpoint for one account with rotation: only the current
/// refresh token works, once; a refresh hands out a new one; any other
/// gets `400 invalid_grant` (the provider's answer to a rotated-away
/// token). `during` runs inside the vendor call (another writer acting
/// meanwhile).
struct RotatingVendor {
    current: Mutex<String>,
    issued: AtomicUsize,
    calls: AtomicUsize,
    delay: Duration,
    during: Option<Box<dyn Fn() + Sync>>,
}

impl RotatingVendor {
    fn new(current_tag: &str) -> RotatingVendor {
        RotatingVendor {
            current: Mutex::new(format!("{MARKER}-refresh-{current_tag}")),
            issued: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
            during: None,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    fn post(&self, body: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        if let Some(f) = &self.during {
            f();
        }
        let mut cur = self.current.lock().unwrap();
        if body["refresh_token"].as_str() != Some(cur.as_str()) {
            return Err("400 invalid_grant".into());
        }
        let n = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        *cur = format!("{MARKER}-refresh-k2rot{n}");
        Ok(json!({"access_token": format!("{MARKER}-access-k2rot{n}"), "refresh_token": cur.clone(), "expires_in": 28800}))
    }

    /// One of the CLIs sharing the live store refreshes it with the
    /// current refresh token (asserting K2 didn't spend it) and writes the
    /// rotated set back.
    fn cli_refreshes_live(&self, h: &Home, new_tag: &str) {
        let mut cur = self.current.lock().unwrap();
        assert_eq!(refresh_token(&live(h)), *cur, "the CLI's refresh token is still the current one");
        *cur = format!("{MARKER}-refresh-{new_tag}");
        write_live(h, &cred(new_tag, later()));
    }
}

impl Refresher for RotatingVendor {
    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String> {
        assert_eq!(tool, Tool::Claude);
        wallet::claude_refresh_slot(id, &|_url, body| self.post(body))
    }
}

/// The daemon's shared database, for keep-warm from several threads.
struct SharedConn(Mutex<Connection>);

impl wallet::DbAccess for SharedConn {
    fn run(&self, f: &mut dyn FnMut(&Connection)) {
        f(&self.0.lock().unwrap())
    }
}

/// THE FIELD RACE. Token A is the login the CLIs run on (the live store
/// names its account), but it isn't on record as the live token. A CLI
/// refreshes the live store (a0 → a1), so A's slot still holds a0, a
/// refresh token the provider rotated away. Keep-warm used to refresh A
/// with a0 (`400 invalid_grant`, then "sign in again"). Now it never
/// refreshes the live login: it reads the newer set back, and the CLI's
/// own refresh token is never spent by K2.
#[test]
fn keep_warm_never_refreshes_the_live_login_and_reads_its_rotated_tokens_back() {
    let h = Home::new();
    let conn = db();
    live_profile_names(&h, "uuid-a", "a@example.test");
    write_live(&h, &cred("a0", soon()));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    assert!(identity::read_record(Tool::Claude, &a.id).is_some(), "A's account is on record");
    // Nothing on record as live any more (the default moved to an API
    // token that recorded no live login): only the account says A is live.
    conn.execute("DELETE FROM llm_active", []).unwrap();
    let vendor = RotatingVendor::new("a0");
    vendor.cli_refreshes_live(&h, "a1");
    assert!(identity::is_live_login(Tool::Claude, &a.id), "the same account, by account (not by refresh token)");
    let res = wallet::keep_warm(&conn, &vendor, now(), &|_| false).unwrap();
    assert_eq!(vendor.calls(), 0, "K2 never refreshes the live login: {res:?}");
    assert_eq!(slot(&a.id), live(&h), "the CLI's newer set was read back into A");
    assert_eq!(get(&conn, &a.id).unwrap().unwrap().state, state::SIGNED_IN);
    // A switch's phase 1 doesn't refresh it either.
    let gen = wallet::generation(&conn, Tool::Claude).unwrap();
    let a_now = get(&conn, &a.id).unwrap().unwrap();
    wallet::precheck(Tool::Claude, &a_now, gen, Some(&vendor)).unwrap();
    assert_eq!(vendor.calls(), 0, "no refresh of the live login in a switch either");
    // The CLI's own next refresh still works: K2 never spent its token.
    vendor.cli_refreshes_live(&h, "a2");
}

/// Two K2 refreshers of one login at once (two keep-warm passes; the
/// usage probe works the same way): one refresh, one rotation. The second
/// waits for the login's slot lock, sees it fresh and does nothing, so it
/// never sends a refresh token the first one just rotated away.
#[test]
fn two_refreshers_of_one_login_refresh_it_once() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("live", FAR));
    wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    wallet::file_bytes(Tool::Claude, &b.id, Some(&b), &cred("b0", soon()), wallet::Incoming::Unknown, identity::origin::K2_SIGN_IN).unwrap();
    let mut vendor = RotatingVendor::new("b0");
    vendor.delay = Duration::from_millis(300);
    let shared = SharedConn(Mutex::new(conn));
    let barrier = std::sync::Barrier::new(2);
    let results: Vec<Vec<wallet::WarmResult>> = std::thread::scope(|s| {
        let runs: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    wallet::keep_warm_with(&shared, &vendor, now(), &|_| false).unwrap()
                })
            })
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    assert_eq!(vendor.calls(), 1, "one refresh per login at a time: {results:?}");
    let refreshed: usize = results.iter().flatten().filter(|r| r.refreshed).count();
    assert_eq!(refreshed, 1, "{results:?}");
    assert!(results.iter().flatten().all(|r| r.error.is_none()), "no invalid_grant: {results:?}");
    let conn = shared.0.lock().unwrap();
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, state::SIGNED_IN);
    assert_eq!(refresh_token(&slot(&b.id)), vendor.current(), "the slot holds the one valid refresh token");
}

/// One `invalid_grant` is not a sign-out: the token says so and is checked
/// again on the next pass; the SAME bytes refused again (a fresh read)
/// mark it "sign in again"; after that K2 stops sending its dead refresh
/// token until a new sign-in.
#[test]
fn a_refused_refresh_token_is_marked_only_on_a_second_fresh_refusal_then_left_alone() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("live", FAR));
    wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    wallet::file_bytes(Tool::Claude, &b.id, Some(&b), &cred("dead", soon()), wallet::Incoming::Unknown, identity::origin::K2_SIGN_IN).unwrap();
    update_meta(&conn, &b.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    // The provider's current refresh token for B is one K2 never saw.
    let vendor = RotatingVendor::new("elsewhere");
    let res = wallet::keep_warm(&conn, &vendor, now(), &|_| false).unwrap();
    assert_eq!((vendor.calls(), res.len()), (1, 1), "{res:?}");
    let b1 = get(&conn, &b.id).unwrap().unwrap();
    assert_eq!(b1.state, state::SIGNED_IN, "one refusal never signs a login out");
    assert!(b1.detail.as_deref().unwrap_or("").contains("once"), "{:?}", b1.detail);
    let res = wallet::keep_warm(&conn, &vendor, now(), &|_| false).unwrap();
    assert_eq!((vendor.calls(), res.len()), (2, 1), "{res:?}");
    let b2 = get(&conn, &b.id).unwrap().unwrap();
    assert_eq!(b2.state, state::NEEDS_LOGIN, "refused twice, same bytes: sign in again");
    let res = wallet::keep_warm(&conn, &vendor, now() + 3600, &|_| false).unwrap();
    assert!(res.is_empty(), "{res:?}");
    assert_eq!(vendor.calls(), 2, "the dead refresh token is not sent again");
}

/// A refusal while a CLI rotated the same login live: K2 reads the live
/// store again before anything is marked, adopts the newer set, and the
/// token stays signed in.
#[test]
fn a_refusal_re_reads_the_live_store_before_marking() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("other", FAR));
    live_profile_names(&h, "uuid-x", "x@example.test");
    let x = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    let who_b = AccountIdentity { account_uuid: Some("uuid-b".into()), email: Some("b@example.test".into()), org_uuid: None };
    wallet::file_bytes(Tool::Claude, &b.id, Some(&b), &cred("b0", soon()), wallet::Incoming::Known(&who_b, Source::Vendor), identity::origin::K2_SIGN_IN)
        .unwrap();
    update_meta(&conn, &b.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    let _ = x;
    // While K2's refresh is in flight, B becomes the CLIs' login (a
    // `/login` in a terminal) and a CLI rotates it: b0 is dead now.
    let home = h.home.path().to_path_buf();
    let mut vendor = RotatingVendor::new("b1");
    vendor.during = Some(Box::new(move || {
        fs::write(home.join(".claude/.credentials.json"), cred("b1", later())).unwrap();
        fs::write(home.join(".claude.json"), json!({"oauthAccount": {"accountUuid": "uuid-b", "emailAddress": "b@example.test"}}).to_string()).unwrap();
    }));
    let res = wallet::keep_warm(&conn, &vendor, now(), &|_| false).unwrap();
    assert_eq!(vendor.calls(), 1, "{res:?}");
    assert!(res.iter().all(|r| r.error.is_none()), "read back, not a failure: {res:?}");
    assert_eq!(slot(&b.id), live(&h), "the newer live set was adopted");
    let b_now = get(&conn, &b.id).unwrap().unwrap();
    assert_eq!(b_now.state, state::SIGNED_IN);
    assert_eq!(b_now.detail, None);
}

/// The CLIs lost the live login (the credentials file has no
/// `claudeAiOauth` token): a switch saves nothing back from it, keeps the
/// outgoing token's own copy and says so, and reports `outgoing_unsaved`
/// so nothing probes that copy right after.
#[test]
fn a_switch_over_a_lost_live_login_keeps_the_outgoing_copy_and_says_so() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("a", FAR));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    wallet::file_bytes(Tool::Claude, &b.id, Some(&b), &cred("b", FAR), wallet::Incoming::Unknown, identity::origin::K2_SIGN_IN).unwrap();
    write_live(&h, br#"{"mcpOAuth":{}}"#);
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.outgoing_unsaved, "{out:?}");
    assert!(!out.saved_outgoing, "{out:?}");
    assert_eq!(slot(&a.id), cred("a", FAR), "A keeps its own copy");
    assert_eq!(live(&h), cred("b", FAR), "B is live");
    let a_now = get(&conn, &a.id).unwrap().unwrap();
    assert!(a_now.detail.as_deref().unwrap_or("").contains("lost"), "{:?}", a_now.detail);
}

/// A switch saves the outgoing live login back only when it is the newer
/// set: an older live copy never overwrites the slot's newer one.
#[test]
fn a_switch_never_saves_an_older_live_copy_over_a_newer_slot() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("a", soon()));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "b", None).unwrap();
    wallet::file_bytes(Tool::Claude, &b.id, Some(&b), &cred("b", FAR), wallet::Incoming::Unknown, identity::origin::K2_SIGN_IN).unwrap();
    // A's slot got a newer set of the same sign-in (a read-back).
    let mut newer: Value = serde_json::from_slice(&cred("a", soon())).unwrap();
    newer["claudeAiOauth"]["expiresAt"] = json!(later());
    newer["claudeAiOauth"]["accessToken"] = json!(format!("{MARKER}-access-a-newer"));
    let newer = serde_json::to_vec(&newer).unwrap();
    wallet::file_bytes(Tool::Claude, &a.id, Some(&a), &newer, wallet::Incoming::Unknown, identity::origin::IMPORT).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.saved_outgoing && !out.outgoing_unsaved, "{out:?}");
    assert_eq!(slot(&a.id), newer, "the newer slot copy is kept");
}
