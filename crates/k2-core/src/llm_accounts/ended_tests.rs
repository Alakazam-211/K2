//! A Claude sign-in that has ended (field report 2026-11-18): Claude Code
//! 2.1.319 keeps the `claudeAiOauth` block when a sign-in reaches its time
//! limit, with both tokens empty, `expiresAt: 0` and
//! `loginEnded: "time_limit"`. K2 must read that as signed out, say why,
//! never pick it for "next", and never rewrite the person's file.
//!
//! Temp `HOME` under the ONE env lock; no keychain, no network.

use std::fs;
use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::json;

use super::store::{self, ClaudeEnded};
use super::wallet;
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

fn good(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "claudeAiOauth": {
            "accessToken": format!("{MARKER}-access-{tag}"),
            "refreshToken": format!("{MARKER}-refresh-{tag}"),
            "expiresAt": FAR_MS,
            "subscriptionType": "max"
        }
    }))
    .unwrap()
}

/// What Claude Code 2.1.319 leaves after the time limit, with the
/// person's other keys (MCP sign-ins) beside it.
fn ended_file() -> Vec<u8> {
    br#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0,"scopes":["user:inference"],"subscriptionType":"max","loginEnded":"time_limit"},"mcpOAuth":{"one":{"accessToken":"K2TEST_SECRET_MARKER-mcp"}}}"#.to_vec()
}

fn write_live(h: &Home, bytes: &[u8]) {
    let p = h.p(".claude/.credentials.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn read_live(h: &Home) -> Vec<u8> {
    fs::read(h.p(".claude/.credentials.json")).unwrap()
}

/// Put bytes the wallet itself would refuse (an ended sign-in) straight
/// into a token's slot, as a CLI running there would.
fn put_raw_slot(id: &str, bytes: &[u8]) {
    let dir = store::slot_dir(Tool::Claude, id);
    store::ensure_private_dir(&dir).unwrap();
    store::write_private(&store::slot_cred_path(Tool::Claude, id), bytes).unwrap();
}

#[test]
fn the_time_limit_file_is_ended_with_the_time_limit_reason() {
    let now = 1_800_000_000;
    let v: serde_json::Value = serde_json::from_slice(&ended_file()).unwrap();
    let why = store::claude_login_ended(&v["claudeAiOauth"], now).expect("ended");
    assert_eq!(why, ClaudeEnded::TimeLimit(None));
    assert_eq!(why.words(), "Claude's sign-in reached its time limit; sign in again.");
    assert!(why.by_claude());
    // With refreshTokenExpiresAt known (milliseconds), the words say when.
    let at = json!({"accessToken": "", "refreshToken": "", "expiresAt": 0, "loginEnded": "time_limit", "refreshTokenExpiresAt": 1_763_388_180_000i64});
    let why = store::claude_login_ended(&at, now).expect("ended");
    assert_eq!(why, ClaudeEnded::TimeLimit(Some(1_763_388_180)));
    assert_eq!(why.words(), "Claude's sign-in reached its time limit on 2025-11-17 14:03 UTC; sign in again.");
    // Any other loginEnded value is ended too.
    let other = json!({"accessToken": "a", "refreshToken": "r", "loginEnded": "revoked"});
    assert_eq!(store::claude_login_ended(&other, now), Some(ClaudeEnded::Ended));
}

#[test]
fn an_empty_or_missing_refresh_token_alone_is_signed_out() {
    let now = 1_800_000_000;
    let empty = json!({"accessToken": "a", "refreshToken": "", "expiresAt": FAR_MS});
    assert_eq!(store::claude_login_ended(&empty, now), Some(ClaudeEnded::NoRefresh));
    let missing = json!({"accessToken": "a", "expiresAt": FAR_MS});
    assert_eq!(store::claude_login_ended(&missing, now), Some(ClaudeEnded::NoRefresh));
    assert!(!ClaudeEnded::NoRefresh.by_claude());
}

#[test]
fn a_past_refresh_token_expiry_is_signed_out() {
    let now = 1_800_000_000;
    for v in [json!(1_700_000_000i64), json!(1_700_000_000_000i64), json!("1700000000000"), json!("2023-11-14T22:13:20Z")] {
        let o = json!({"accessToken": "a", "refreshToken": "r", "expiresAt": FAR_MS, "refreshTokenExpiresAt": v});
        assert_eq!(store::claude_login_ended(&o, now), Some(ClaudeEnded::TimeLimit(Some(1_700_000_000))), "{o}");
    }
}

#[test]
fn a_good_file_stays_signed_in_and_unknown_fields_are_ignored() {
    let now = 1_800_000_000;
    let o = json!({"accessToken": "a", "refreshToken": "r", "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS, "brandNewField": {"x": 1}, "loginEnded": null});
    assert_eq!(store::claude_login_ended(&o, now), None);
    let blank = json!({"accessToken": "a", "refreshToken": "r", "loginEnded": ""});
    assert_eq!(store::claude_login_ended(&blank, now), None);
    let m = store::parse_meta(Tool::Claude, &good("a"));
    assert_eq!(m.ended, None);
    assert!(store::validate_cred(Tool::Claude, &good("a")).is_ok());
    // An ended sign-in with tokens still in it is never a whole one.
    let flagged = br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","loginEnded":"time_limit"}}"#;
    assert!(store::validate_cred(Tool::Claude, flagged).is_err());
}

#[test]
fn the_default_whose_live_sign_in_ended_reads_signed_out_and_the_file_is_untouched() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &good("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    assert_eq!(a.state, state::SIGNED_IN);
    write_live(&h, &ended_file());
    let e = wallet::recheck(&conn, &a.id).unwrap();
    assert_eq!(e.state, state::NEEDS_LOGIN);
    assert!(e.detail.as_deref().unwrap().starts_with("Claude's sign-in reached its time limit"), "{:?}", e.detail);
    assert_eq!(read_live(&h), ended_file(), "the person's file is only read");
    // The background pass says the same (and still writes nothing).
    update_meta(&conn, &a.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), detail: Some(None), ..Default::default() }).unwrap();
    wallet::reconcile_live(&conn, Tool::Claude, None, true).unwrap();
    let e = get(&conn, &a.id).unwrap().unwrap();
    assert_eq!(e.state, state::NEEDS_LOGIN);
    assert!(e.detail.as_deref().unwrap().contains("time limit"));
    assert_eq!(read_live(&h), ended_file());
    assert_ne!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), ended_file(), "never mirrored into the slot");
    // Signed in again outside K2: signed in, and the old reason goes.
    write_live(&h, &good("a"));
    let e = wallet::recheck(&conn, &a.id).unwrap();
    assert_eq!(e.state, state::SIGNED_IN);
    assert_eq!(e.detail, None);
}

#[test]
fn an_idle_token_whose_sign_in_ended_reads_signed_out_and_is_not_refreshed() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &good("a"));
    let _a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "second", None).unwrap();
    put_raw_slot(&b.id, &ended_file());
    update_meta(&conn, &b.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    struct Never;
    impl wallet::Refresher for Never {
        fn refresh(&self, _: Tool, _: &str) -> Result<(), String> {
            panic!("an ended sign-in must not be refreshed")
        }
    }
    let res = wallet::keep_warm(&conn, &Never, chrono::Utc::now().timestamp() + 2 * 86_400, &|_| false).unwrap();
    assert!(res.is_empty(), "{res:?}");
    let e = get(&conn, &b.id).unwrap().unwrap();
    assert_eq!(e.state, state::NEEDS_LOGIN);
    assert!(e.detail.as_deref().unwrap().contains("time limit"));
    let e = wallet::recheck(&conn, &b.id).unwrap();
    assert_eq!(e.state, state::NEEDS_LOGIN);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &b.id)).unwrap(), ended_file());
}

#[test]
fn next_skips_an_ended_token() {
    let h = Home::new();
    let conn = db();
    write_live(&h, &good("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = create(&conn, Tool::Claude, "ended", None).unwrap();
    put_raw_slot(&b.id, &ended_file());
    // Even while its row still says signed in (nothing rechecked it yet).
    update_meta(&conn, &b.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    assert!(next_signed_in(&conn, Tool::Claude).unwrap().is_none(), "the only other token has ended");
    // A token marked "sign in again" is skipped too.
    let c = create(&conn, Tool::Claude, "marked", None).unwrap();
    wallet::file_bytes(Tool::Claude, &c.id, None, &good("c"), wallet::Incoming::Unknown, "test").unwrap();
    update_meta(&conn, &c.id, &MetaUpdate { state: Some(state::NEEDS_LOGIN.into()), ..Default::default() }).unwrap();
    assert!(next_signed_in(&conn, Tool::Claude).unwrap().is_none());
    update_meta(&conn, &c.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    assert_eq!(next_signed_in(&conn, Tool::Claude).unwrap().map(|e| e.id), Some(c.id.clone()));
    assert_eq!(active_id(&conn, Tool::Claude).unwrap(), Some(a.id));
}

// ── Part 2: a new sign-in for the default goes live (merged) ────────

fn profile(uuid: &str, email: &str) -> serde_json::Value {
    json!({"accountUuid": uuid, "emailAddress": email, "organizationUuid": format!("org-{uuid}")})
}

fn sign_into_slot(id: &str, bytes: &[u8], uuid: &str, email: &str) {
    store::write_slot(Tool::Claude, id, bytes).unwrap();
    fs::write(store::slot_dir(Tool::Claude, id).join(".claude.json"), json!({"oauthAccount": profile(uuid, email)}).to_string()).unwrap();
}

/// The default "a" (uuid-a), imported with its profile; then Claude's
/// time limit ends the live sign-in (the MCP sign-ins stay in the file).
fn ended_default(h: &Home, conn: &Connection) -> Entry {
    write_live(h, &good("a"));
    fs::write(h.p(".claude.json"), json!({"oauthAccount": profile("uuid-a", "a@example.test")}).to_string()).unwrap();
    let a = wallet::import_live(conn, Tool::Claude, None).unwrap().unwrap();
    write_live(h, &ended_file());
    a
}

const MCP_TAIL: &[u8] = br#","mcpOAuth":{"one":{"accessToken":"K2TEST_SECRET_MARKER-mcp"}}}"#;

fn live_oauth(h: &Home) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(&read_live(h)).unwrap();
    v["claudeAiOauth"].clone()
}

fn oauth_of(bytes: &[u8]) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"].clone()
}

#[test]
fn the_merge_replaces_only_claude_ai_oauth_byte_for_byte() {
    let live = b"{\n  \"mcpOAuth\": {\"x\": {\"accessToken\": \"m\\u00e9\", \"n\": 1.50}},\n  \"claudeAiOauth\": {\"accessToken\": \"\", \"loginEnded\": \"time_limit\"},\n  \"other\": [1, {\"a\": \"}\"}]\n}\n";
    let incoming = br#"{"claudeAiOauth":{"accessToken":"new","refreshToken":"r"}}"#;
    let out = store::merge_claude_oauth(live, incoming).expect("merged");
    let want = b"{\n  \"mcpOAuth\": {\"x\": {\"accessToken\": \"m\\u00e9\", \"n\": 1.50}},\n  \"claudeAiOauth\": {\"accessToken\":\"new\",\"refreshToken\":\"r\"},\n  \"other\": [1, {\"a\": \"}\"}]\n}\n";
    assert_eq!(String::from_utf8_lossy(&out), String::from_utf8_lossy(want));
    // No block yet: it goes in front, the rest untouched.
    let out = store::merge_claude_oauth(br#"{"mcpOAuth":{"x":1}}"#, incoming).expect("merged");
    assert_eq!(out, br#"{"claudeAiOauth":{"accessToken":"new","refreshToken":"r"},"mcpOAuth":{"x":1}}"#.to_vec());
    // Nothing else to keep, or not an object: written as it is.
    assert_eq!(store::merge_claude_oauth(br#"{"claudeAiOauth":{"accessToken":"old"}}"#, incoming), None);
    assert_eq!(store::merge_claude_oauth(b"[1]", incoming), None);
    assert_eq!(store::merge_claude_oauth(b"{\"a\":", incoming), None);
}

#[test]
fn a_re_sign_in_for_the_default_updates_the_live_file_and_keeps_mcp_oauth() {
    let h = Home::new();
    let conn = db();
    let a = ended_default(&h, &conn);
    // Field: "Add a token", signed in as the default's own account.
    let r = wallet::begin_new(&conn, Tool::Claude, "again", None).unwrap();
    sign_into_slot(&r.id, &good("a2"), "uuid-a", "a@example.test");
    let kept = wallet::finalize_login(&conn, &r.id, None).unwrap();
    assert_eq!(kept.id, a.id, "one row per account");
    assert_eq!(live_oauth(&h), oauth_of(&good("a2")), "the CLIs read the new sign-in");
    assert!(read_live(&h).ends_with(MCP_TAIL), "{}", String::from_utf8_lossy(&read_live(&h)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(h.p(".claude/.credentials.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let e = wallet::recheck(&conn, &a.id).unwrap();
    assert_eq!((e.state.as_str(), e.detail.as_deref()), (state::SIGNED_IN, None));
    // Again on the default itself ("Sign in again" on its row), once the
    // live sign-in ended once more.
    let mut again = ended_file();
    again.splice(1..1, b"\"note\":1,".iter().copied());
    write_live(&h, &again);
    assert_eq!(wallet::begin_relogin(&conn, &a.id).unwrap().state, state::SIGNING_IN);
    sign_into_slot(&a.id, &good("a3"), "uuid-a", "a@example.test");
    wallet::finalize_login(&conn, &a.id, None).unwrap();
    assert_eq!(live_oauth(&h), oauth_of(&good("a3")));
    assert!(read_live(&h).starts_with(b"{\"note\":1,") && read_live(&h).ends_with(MCP_TAIL));
}

#[test]
fn a_non_default_tokens_sign_in_leaves_the_live_file_alone() {
    let h = Home::new();
    let conn = db();
    let _a = ended_default(&h, &conn);
    let b = wallet::begin_new(&conn, Tool::Claude, "b", None).unwrap();
    sign_into_slot(&b.id, &good("b"), "uuid-b", "b@example.test");
    wallet::finalize_login(&conn, &b.id, None).unwrap();
    assert_eq!(read_live(&h), ended_file());
    // A live sign-in that still works is never replaced by one for the
    // default either: the CLIs own it.
    let mut working = good("a-cli").to_vec();
    working.truncate(working.len() - 1);
    working.extend_from_slice(MCP_TAIL);
    write_live(&h, &working);
    let r = wallet::begin_new(&conn, Tool::Claude, "again", None).unwrap();
    sign_into_slot(&r.id, &good("a4"), "uuid-a", "a@example.test");
    wallet::finalize_login(&conn, &r.id, None).unwrap();
    assert_eq!(read_live(&h), working);
}

#[test]
fn an_ended_live_file_with_a_good_store_reads_signed_out_and_apply_fixes_it() {
    let h = Home::new();
    let conn = db();
    let a = ended_default(&h, &conn);
    assert_ne!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), ended_file(), "the store still looks good");
    let e = wallet::recheck(&conn, &a.id).unwrap();
    assert_eq!(e.state, state::NEEDS_LOGIN);
    let detail = e.detail.clone().unwrap();
    assert!(detail.starts_with("Claude's sign-in reached its time limit") && detail.contains("Apply saved sign-in"), "{detail}");
    assert!(wallet::can_apply_saved(&conn, Tool::Claude, &a.id).unwrap());
    let out = wallet::switch(&conn, Tool::Claude, &a.id, Some("owner-token"), None).unwrap();
    assert!(out.applied_saved && !out.already_active, "{out:?}");
    assert_eq!(live_oauth(&h), oauth_of(&good("a")));
    assert!(read_live(&h).ends_with(MCP_TAIL));
    assert!(!wallet::can_apply_saved(&conn, Tool::Claude, &a.id).unwrap());
    assert_eq!(wallet::recheck(&conn, &a.id).unwrap().state, state::SIGNED_IN);
    // Once applied, the same click is a no-op.
    assert!(wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap().already_active);
}

#[cfg(unix)]
#[test]
fn the_live_file_is_never_written_through_a_symlink() {
    let h = Home::new();
    let conn = db();
    let a = ended_default(&h, &conn);
    let real = h.p("elsewhere.json");
    fs::write(&real, ended_file()).unwrap();
    let link = h.p(".claude/.credentials.json");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
    assert_eq!(fs::read(&real).unwrap(), ended_file(), "nothing written through it");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}
