//! An expired access token on a good sign-in:
//! the server default's access token expired and nothing renewed it, while
//! the list still said plain `signed_in`.
//!
//! - keep-warm never refreshes the server default (its CLIs own the
//!   refresh; a refresh by K2 rotates the refresh token under them), with
//!   or without a session running on it, and leaves its bytes alone; an
//!   idle token with an expired access token is refreshed;
//! - each reconcile keeps the default's recorded expiry in step with the
//!   live store, so the list reads a current one;
//! - the three answers: fine, expired (who renews it), signed out.
//!
//! Temp `HOME` under the ONE env lock; no keychain, no network, fake
//! credentials with a marker string that must never reach a note.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;
use serde_json::json;

use super::super::wallet::{self, Refresher};
use super::super::*;
use super::*;

const MARKER: &str = "K2TEST_SECRET_MARKER";
const SCRUB_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP", "K2_LLM_LOGIN_LIVE"];
const FAR_MS: i64 = 4_102_444_800_000;

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

/// A Claude credential: `tag` names its tokens, `expires_ms` its access
/// token's expiry. The refresh token is good (the sign-in hasn't ended).
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

fn write_live(h: &Home, bytes: &[u8]) {
    let p = h.p(".claude/.credentials.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn read_live(h: &Home) -> Vec<u8> {
    fs::read(h.p(".claude/.credentials.json")).unwrap()
}

/// Counts refreshes; a refresh writes a fresh access token (and rotates
/// the refresh token) into the slot, as Claude's grant would.
struct Counting {
    ids: std::sync::Mutex<Vec<String>>,
    calls: AtomicUsize,
}

impl Counting {
    fn new() -> Counting {
        Counting { ids: std::sync::Mutex::new(Vec::new()), calls: AtomicUsize::new(0) }
    }
    fn refreshed(&self) -> Vec<String> {
        self.ids.lock().unwrap().clone()
    }
}

impl Refresher for Counting {
    fn refresh(&self, tool: Tool, id: &str) -> Result<(), String> {
        assert_eq!(tool, Tool::Claude);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.ids.lock().unwrap().push(id.to_string());
        wallet::claude_refresh_slot(id, &|_url, _body| {
            Ok(json!({"access_token": format!("{MARKER}-access-new"), "refresh_token": format!("{MARKER}-refresh-rotated"), "expires_in": 28800}))
        })
    }
}

/// The report's server: the default's access token expired an hour ago
/// (its sign-in still good), and an idle token's expired too.
fn expired_default_and_idle(h: &Home, conn: &Connection) -> (Entry, Entry) {
    let hour_ago_ms = (now() - 3600) * 1000;
    write_live(h, &cred("def", hour_ago_ms));
    let def = wallet::import_live(conn, Tool::Claude, None).unwrap().expect("the live sign-in is the default");
    assert_eq!(live_owner(conn, Tool::Claude).unwrap().as_deref(), Some(def.id.as_str()));
    let idle = create(conn, Tool::Claude, "idle", None).unwrap();
    store::write_slot(Tool::Claude, &idle.id, &cred("idle", hour_ago_ms)).unwrap();
    (def, idle)
}

#[test]
fn keep_warm_never_refreshes_the_server_default_with_no_session_on_it() {
    let h = Home::new();
    let conn = db();
    let (def, idle) = expired_default_and_idle(&h, &conn);
    let live_before = read_live(&h);
    let slot_before = slots::read_slot(Tool::Claude, &def.id).unwrap();
    let r = Counting::new();
    let res = wallet::keep_warm(&conn, &r, now(), &|_| false).unwrap();
    // The idle token is renewed; the default never is.
    assert_eq!(r.refreshed(), vec![idle.id.clone()], "{res:?}");
    assert_eq!(res.len(), 1, "{res:?}");
    assert!(res[0].refreshed && res[0].id == idle.id, "{res:?}");
    assert_eq!(read_live(&h), live_before, "the live store is the CLIs' own: never written by keep-warm");
    assert_eq!(slots::read_slot(Tool::Claude, &def.id).unwrap(), slot_before, "the default's saved copy is not refreshed either");
    let d = get(&conn, &def.id).unwrap().unwrap();
    assert_eq!(d.state, state::SIGNED_IN, "{d:?}");
    // It reads as expired, renewed by the CLI, not as signed out.
    let v = access_of(&d, now(), true);
    assert_eq!(v.access, Some(Access::Expired), "{v:?}");
    let note = v.note.expect("a note");
    assert!(note.contains("Claude renews it by itself the next time it runs on this token"), "{note}");
    assert!(note.contains("Nothing to do"), "{note}");
    assert!(!note.contains(MARKER), "{note}");
    // A second pass: still never the default.
    let _ = wallet::keep_warm(&conn, &r, now(), &|_| false).unwrap();
    assert!(!r.refreshed().contains(&def.id), "{:?}", r.refreshed());
    assert_eq!(read_live(&h), live_before);
}

#[test]
fn keep_warm_never_refreshes_the_server_default_with_a_session_on_it() {
    let h = Home::new();
    let conn = db();
    let (def, idle) = expired_default_and_idle(&h, &conn);
    let live_before = read_live(&h);
    let r = Counting::new();
    // A running session holds the default (and nothing else).
    let def_id = def.id.clone();
    let in_use = move |id: &str| id == def_id;
    let res = wallet::keep_warm(&conn, &r, now(), &in_use).unwrap();
    assert_eq!(r.refreshed(), vec![idle.id.clone()], "{res:?}");
    assert_eq!(read_live(&h), live_before);
    // A session on an idle token makes it CLI-owned too: skipped.
    store::write_slot(Tool::Claude, &idle.id, &cred("idle2", (now() - 60) * 1000)).unwrap();
    let r2 = Counting::new();
    let idle_id = idle.id.clone();
    let res = wallet::keep_warm(&conn, &r2, now(), &move |id: &str| id == idle_id).unwrap();
    assert!(res.is_empty() && r2.calls.load(Ordering::SeqCst) == 0, "{res:?}");
}

#[test]
fn reconcile_keeps_the_defaults_recorded_expiry_in_step_with_the_live_store() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &cred("def", FAR_MS));
    let def = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    // The CLI renewed it (same sign-in, a new expiry), K2's row is older.
    update_meta(&conn, &def.id, &MetaUpdate { expires_at: Some(Some(1_000)), ..Default::default() }).unwrap();
    let later_ms = (now() + 7200) * 1000;
    write_live(&h, &cred("def", later_ms));
    wallet::reconcile_live(&conn, Tool::Claude, None, false).unwrap();
    let d = get(&conn, &def.id).unwrap().unwrap();
    assert_eq!(d.expires_at, Some(later_ms / 1000), "{d:?}");
    assert_eq!(access_of(&d, now(), true).access, Some(Access::Fine));
    // The CLI hasn't run since and the access token expired: the next
    // reconcile records that expiry, and the list reads it as expired.
    let past_ms = (now() - 120) * 1000;
    write_live(&h, &cred("def", past_ms));
    wallet::reconcile_live(&conn, Tool::Claude, None, false).unwrap();
    let d = get(&conn, &def.id).unwrap().unwrap();
    assert_eq!(d.expires_at, Some(past_ms / 1000), "{d:?}");
    assert_eq!(d.state, state::SIGNED_IN, "an expired access token is not a sign-out");
    assert_eq!(access_of(&d, now(), true).access, Some(Access::Expired));
}

fn entry(st: &str, expires_at: Option<i64>) -> Entry {
    Entry {
        id: "acc_x".into(),
        tool: "claude".into(),
        label: "x".into(),
        kind: kind::SUBSCRIPTION.into(),
        position: 0,
        created_by: None,
        created_at: 1_000,
        last_used_at: None,
        state: st.into(),
        detail: None,
        email: None,
        org: None,
        plan: None,
        expires_at,
        refreshed_at: None,
        usage_json: None,
        usage_checked_at: None,
        removed_at: None,
        sign_in_ends_at: None,
    }
}

#[test]
fn the_three_answers_fine_expired_and_signed_out() {
    let now = 1_800_000_000;
    // Fine: not yet expired, or no expiry on record.
    assert_eq!(access_of(&entry(state::SIGNED_IN, Some(now + 60)), now, true), AccessView { access: Some(Access::Fine), note: None });
    assert_eq!(access_of(&entry(state::SIGNED_IN, None), now, false).access, Some(Access::Fine));
    // Expired: when, and who renews it.
    let held = access_of(&entry(state::SIGNED_IN, Some(now - 1)), now, true);
    assert_eq!(held.access, Some(Access::Expired));
    assert_eq!(
        held.note.as_deref(),
        Some("Its access token expired on 2027-01-15 07:59 UTC. Its sign-in is still good: Claude renews it by itself the next time it runs on this token. Nothing to do.")
    );
    let idle = access_of(&entry(state::SIGNED_IN, Some(now)), now, false);
    assert_eq!(idle.access, Some(Access::Expired));
    assert!(idle.note.as_deref().is_some_and(|n| n.contains("the next keep-warm pass (every 30 minutes) renews it, or a switch to it does")), "{idle:?}");
    // Signed out: an ended or refused sign-in, or none yet. Its detail
    // says why; no second note.
    for st in [state::NEEDS_LOGIN, state::NOT_SET_UP] {
        assert_eq!(access_of(&entry(st, Some(now - 1)), now, true), AccessView { access: Some(Access::SignedOut), note: None }, "{st}");
    }
    // No answer: a sign-in in progress, unknown, an API key, Grok's expiry.
    for st in [state::SIGNING_IN, state::UNKNOWN] {
        assert_eq!(access_of(&entry(st, Some(now - 1)), now, true).access, None, "{st}");
    }
    let mut key = entry(state::SIGNED_IN, Some(now - 1));
    key.kind = kind::API_KEY.into();
    assert_eq!(access_of(&key, now, false).access, None);
    let mut grok = entry(state::SIGNED_IN, Some(now - 1));
    grok.tool = "grok".into();
    assert_eq!(access_of(&grok, now, false).access, Some(Access::Fine));
    // The wire names.
    assert_eq!(
        [Access::Fine, Access::Expired, Access::SignedOut].map(|a| (a.as_str(), serde_json::to_value(a).unwrap())),
        [("fine", json!("fine")), ("expired", json!("expired")), ("signed_out", json!("signed_out"))]
    );
}
