//! Account identity on switch, and Claude's `oauthAccount` swap (field
//! bug 2026-10-14: `/status` named the old account after a switch, and a
//! switch could save another account's token into a slot). Temp `HOME`
//! under the ONE env lock; no keychain, no network (a fake vendor), never
//! the real `~/.claude.json`.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

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

fn cred(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "claudeAiOauth": {
            "accessToken": format!("{MARKER}-access-{tag}"),
            "refreshToken": format!("{MARKER}-refresh-{tag}"),
            "expiresAt": FAR,
            "subscriptionType": "max"
        }
    }))
    .unwrap()
}

fn profile(uuid: &str, email: &str) -> Value {
    json!({
        "accountUuid": uuid,
        "emailAddress": email,
        "organizationUuid": format!("org-{uuid}"),
        "displayName": format!("Name {uuid}"),
        "billingType": "stripe_subscription"
    })
}

/// A `~/.claude.json` the way Claude 2.1.300 writes it: two-space pretty
/// JSON with many other settings around `oauthAccount`.
fn claude_json(oauth: &Value) -> String {
    let rendered = serde_json::to_string_pretty(oauth).unwrap().replace('\n', "\n  ");
    format!(
        "{{\n  \"numStartups\": 412,\n  \"theme\": \"dark\",\n  \"userID\": \"0123456789abcdef\",\n  \"projects\": {{\n    \"/tmp/x\": {{\n      \"allowedTools\": [],\n      \"hasTrustDialogAccepted\": true\n    }}\n  }},\n  \"oauthAccount\": {rendered},\n  \"hasCompletedOnboarding\": true,\n  \"note\": \"braces {{ }} and \\\"quotes\\\" in a string\"\n}}\n"
    )
}

fn write_live(h: &Home, tag: &str) {
    let p = h.p(".claude/.credentials.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, cred(tag)).unwrap();
}

fn live_bytes(h: &Home) -> Vec<u8> {
    fs::read(h.p(".claude/.credentials.json")).unwrap()
}

/// The vendor: which account each fake token belongs to.
struct FakeVendor {
    accounts: Mutex<Vec<(String, AccountIdentity)>>,
}

impl FakeVendor {
    fn new(rows: &[(&str, &str, &str)]) -> FakeVendor {
        FakeVendor {
            accounts: Mutex::new(
                rows.iter()
                    .map(|(tag, uuid, email)| {
                        (
                            format!("{MARKER}-access-{tag}"),
                            AccountIdentity { account_uuid: Some(uuid.to_string()), email: Some(email.to_string()), org_uuid: None },
                        )
                    })
                    .collect(),
            ),
        }
    }
}

impl Refresher for FakeVendor {
    fn refresh(&self, _tool: Tool, _id: &str) -> Result<(), String> {
        panic!("no refresh expected (every fake token is far from expiry)");
    }
    fn identify(&self, tool: Tool, b: &[u8]) -> Option<AccountIdentity> {
        assert_eq!(tool, Tool::Claude);
        let v: Value = serde_json::from_slice(b).unwrap();
        let at = v["claudeAiOauth"]["accessToken"].as_str().unwrap().to_string();
        self.accounts.lock().unwrap().iter().find(|(t, _)| *t == at).map(|(_, id)| id.clone())
    }
}

/// Live = A (uuid-a), imported with A's profile; B signed in to its slot
/// with B's own profile (a temp-home sign-in writes `<slot>/.claude.json`).
fn two_logins(h: &Home, conn: &Connection) -> (Entry, Entry) {
    write_live(h, "a");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-a", "a@example.test"))).unwrap();
    let a = wallet::import_live(conn, Tool::Claude, None).unwrap().unwrap();
    let b = wallet::begin_new(conn, Tool::Claude, "b", None).unwrap();
    store::write_slot(Tool::Claude, &b.id, &cred("b")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &b.id).join(".claude.json"),
        json!({"oauthAccount": profile("uuid-b", "b@example.test")}).to_string(),
    )
    .unwrap();
    let b = wallet::finalize_login(conn, &b.id, None).unwrap();
    (a, b)
}

#[test]
fn import_and_sign_in_record_each_tokens_account() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    let (pa, src) = identity::slot_profile(&a.id).expect("A's profile recorded at import");
    assert_eq!(src, Source::K2Record);
    assert_eq!(pa["accountUuid"], "uuid-a");
    let (pb, src) = identity::slot_profile(&b.id).expect("B's profile recorded at sign-in");
    assert_eq!(src, Source::K2Record);
    assert_eq!(pb["emailAddress"], "b@example.test");
    let rec = fs::read_to_string(identity::slot_profile_path(&b.id)).unwrap();
    assert!(!rec.contains(MARKER), "no token in the record");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = fs::metadata(identity::slot_profile_path(&b.id)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
    }
}

#[test]
fn a_live_login_of_another_account_is_never_saved_into_the_owners_slot() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    let a_slot_before = fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap();
    // Someone ran `/login` as C outside K2: the live token is C's, while
    // K2's record says A is live.
    write_live(&h, "c");
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("b", "uuid-b", "b@example.test"), ("c", "uuid-c", "c@example.test")]);
    let err = wallet::switch(&conn, Tool::Claude, &b.id, Some("owner-token"), Some(&vendor)).unwrap_err();
    assert_eq!(err.code(), "account_mismatch", "{err}");
    let text = err.to_string();
    assert!(text.contains("c@example.test") && text.contains("a@example.test"), "{text}");
    assert!(text.contains("checked with the provider"), "{text}");
    assert!(!text.contains(MARKER), "no token in the error: {text}");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), a_slot_before, "A's slot untouched");
    assert_eq!(live_bytes(&h), cred("c"), "live untouched");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()), "default unchanged");
    // The live token really is A's: the same switch goes through.
    write_live(&h, "a");
    let out = wallet::switch(&conn, Tool::Claude, &b.id, Some("owner-token"), Some(&vendor)).unwrap();
    assert!(out.saved_outgoing);
    assert_eq!(out.verified_by, Some(Source::Vendor));
    assert_eq!(live_bytes(&h), cred("b"));
}

#[test]
fn without_the_vendor_a_stale_claude_json_naming_another_account_also_refuses() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // `/login` as C rewrote both the token and ~/.claude.json.
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    assert!(err.to_string().contains("run /login as a@example.test"), "{err}");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"));
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
}

#[test]
fn a_saved_sign_in_of_another_account_is_never_made_live() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // B's slot holds D's token (crossed earlier); its record says B.
    store::write_slot(Tool::Claude, &b.id, &cred("d")).unwrap();
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("d", "uuid-d", "d@example.test")]);
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    assert!(err.to_string().contains("saved sign-in for \"b\" belongs to d@example.test"), "{err}");
    assert_eq!(live_bytes(&h), cred("a"), "live untouched");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
    let live_cfg: Value = serde_json::from_str(&fs::read_to_string(h.p(".claude.json")).unwrap()).unwrap();
    assert_eq!(live_cfg["oauthAccount"]["accountUuid"], "uuid-a", "profile untouched");
}

#[test]
fn switch_swaps_the_oauth_account_profile_and_leaves_every_other_key_intact() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // Claude refreshed A's profile fields since import.
    let mut a_now = profile("uuid-a", "a@example.test");
    a_now["displayName"] = json!("Renamed A");
    let before = claude_json(&a_now);
    fs::write(h.p(".claude.json"), &before).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(h.p(".claude.json"), fs::Permissions::from_mode(0o644)).unwrap();
    }
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.profile_swapped);
    assert_eq!(out.from_label.as_deref(), Some("Default"));
    assert_eq!(out.to_label, "b");
    let after = fs::read_to_string(h.p(".claude.json")).unwrap();
    assert_eq!(after, claude_json(&profile("uuid-b", "b@example.test")), "only oauthAccount changed, byte for byte");
    let v: Value = serde_json::from_str(&after).unwrap();
    assert_eq!(v["numStartups"], 412);
    assert_eq!(v["projects"]["/tmp/x"]["hasTrustDialogAccepted"], true);
    assert_eq!(v["note"], "braces { } and \"quotes\" in a string");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = fs::metadata(h.p(".claude.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o644, "mode kept");
    }
    // A backup of the file as it was.
    assert_eq!(fs::read_to_string(identity::live_config_backup_path()).unwrap(), before);
    // A's fresher profile went back with A's token.
    let (pa, _) = identity::slot_profile(&a.id).unwrap();
    assert_eq!(pa["displayName"], "Renamed A");
    // And back again.
    let out = wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    assert!(out.profile_swapped);
    assert_eq!(fs::read_to_string(h.p(".claude.json")).unwrap(), before);
    assert!(!fs::read_to_string(identity::live_config_backup_path()).unwrap().contains(MARKER));
}

#[test]
fn a_stale_live_profile_is_not_kept_as_the_outgoing_accounts_profile() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // The live token is A's (the vendor says so) but ~/.claude.json still
    // names Z (the field state before this fix).
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-z", "z@example.test"))).unwrap();
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("b", "uuid-b", "b@example.test")]);
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap();
    assert!(out.saved_outgoing);
    let (pa, _) = identity::slot_profile(&a.id).unwrap();
    assert_eq!(pa["accountUuid"], "uuid-a", "Z's profile was not saved as A's");
    let v: Value = serde_json::from_str(&fs::read_to_string(h.p(".claude.json")).unwrap()).unwrap();
    assert_eq!(v["oauthAccount"]["emailAddress"], "b@example.test", "/status now names B");
}

#[test]
fn no_live_claude_json_is_left_alone() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    fs::remove_file(h.p(".claude.json")).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(!out.profile_swapped);
    assert!(!h.p(".claude.json").exists(), "K2 never creates Claude's config");
}

#[test]
fn splice_replaces_or_adds_one_key_and_refuses_what_it_cannot_do_safely() {
    let v = json!({"accountUuid": "u2", "emailAddress": "two@example.test"});
    let text = "{\n  \"a\": 1,\n  \"oauthAccount\": {\n    \"accountUuid\": \"u1\"\n  },\n  \"z\": [1, {\"oauthAccount\": 3}]\n}\n";
    let out = identity::splice_top_level_key(text, "oauthAccount", &v).unwrap();
    assert!(out.starts_with("{\n  \"a\": 1,\n  \"oauthAccount\": {\n    \"accountUuid\": \"u2\""), "{out}");
    assert!(out.ends_with("},\n  \"z\": [1, {\"oauthAccount\": 3}]\n}\n"), "nested key untouched: {out}");
    // Missing key: added last.
    let out = identity::splice_top_level_key("{\n  \"a\": 1\n}\n", "oauthAccount", &v).unwrap();
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["a"], 1);
    assert_eq!(parsed["oauthAccount"], v);
    assert!(out.starts_with("{\n  \"a\": 1,\n  \"oauthAccount\""), "{out}");
    let out = identity::splice_top_level_key("{}", "oauthAccount", &v).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["oauthAccount"], v);
    // Refusals: not JSON, not an object, a duplicate key.
    assert!(identity::splice_top_level_key("{\"a\":", "oauthAccount", &v).is_err());
    assert!(identity::splice_top_level_key("[1]", "oauthAccount", &v).is_err());
    assert!(identity::splice_top_level_key("{\"oauthAccount\":1,\"oauthAccount\":2}", "oauthAccount", &v).is_err());
}

#[test]
fn identities_compare_by_uuid_then_email() {
    let a = AccountIdentity { account_uuid: Some("U1".into()), email: Some("x@example.test".into()), org_uuid: None };
    let b = AccountIdentity { account_uuid: Some("u1".into()), email: Some("other@example.test".into()), org_uuid: None };
    assert_eq!(a.same_account(&b), Some(true), "uuid wins");
    let c = AccountIdentity { account_uuid: Some("u2".into()), ..a.clone() };
    assert_eq!(a.same_account(&c), Some(false));
    let e = AccountIdentity::from_email(Some("X@example.test")).unwrap();
    assert_eq!(a.same_account(&e), Some(true), "email, case-insensitive");
    let none = AccountIdentity { org_uuid: Some("o".into()), ..Default::default() };
    assert_eq!(a.same_account(&none), None);
    assert!(AccountIdentity::from_oauth_account(&json!({"displayName": "x"})).is_none());
    let p = AccountIdentity::from_claude_profile_response(&json!({"account": {"uuid": "u9", "email": "n@example.test"}, "organization": {"uuid": "o9"}})).unwrap();
    assert_eq!(p.account_uuid.as_deref(), Some("u9"));
    assert_eq!(p.org_uuid.as_deref(), Some("o9"));
}

#[test]
fn switching_to_a_login_while_it_signs_in_is_refused_and_next_skips_it() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    wallet::begin_relogin(&conn, &b.id).unwrap();
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "signing_in");
    assert_eq!(live_bytes(&h), cred("a"));
    assert!(next_signed_in(&conn, Tool::Claude).unwrap().is_none(), "the pool cycle skips it");
    let _ = a;
}

#[test]
fn active_info_says_since_when_and_by_whom() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    let t0 = now();
    wallet::switch(&conn, Tool::Claude, &b.id, Some("login:rosson"), None).unwrap();
    let info = active_info(&conn, Tool::Claude).unwrap().unwrap();
    assert_eq!(info.account_id, b.id);
    assert_eq!(info.switched_by.as_deref(), Some("login:rosson"));
    assert!(info.switched_at >= t0);
    assert!(active_info(&conn, Tool::Grok).unwrap().is_none());
}

#[test]
fn two_phase_switch_uses_phase_one_answers_and_refuses_when_anything_moved() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("b", "uuid-b", "b@example.test")]);
    let target = get(&conn, &b.id).unwrap().unwrap();
    // Phase 1 needs no connection; phase 2 makes no vendor call (None here).
    let pre = wallet::precheck(Tool::Claude, &target, wallet::generation(&conn, Tool::Claude).unwrap(), Some(&vendor)).unwrap();
    // The CLI renewed the live token in between: refused, nothing moved.
    write_live(&h, "a");
    let mut renewed: Value = serde_json::from_slice(&cred("a")).unwrap();
    renewed["claudeAiOauth"]["expiresAt"] = json!(FAR + 1000);
    fs::write(h.p(".claude/.credentials.json"), serde_json::to_vec(&renewed).unwrap()).unwrap();
    let err = wallet::switch_prechecked(&conn, Tool::Claude, &b.id, None, &pre).unwrap_err();
    assert_eq!(err.code(), "changed", "{err}");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
    // Fresh phase 1 → phase 2 commits with the vendor's answers.
    let pre = wallet::precheck(Tool::Claude, &target, wallet::generation(&conn, Tool::Claude).unwrap(), Some(&vendor)).unwrap();
    let out = wallet::switch_prechecked(&conn, Tool::Claude, &b.id, None, &pre).unwrap();
    assert_eq!(out.verified_by, Some(Source::Vendor));
    assert_eq!(live_bytes(&h), cred("b"));
    // A stale phase 1 (the default moved since) is refused.
    let err = wallet::switch_prechecked(&conn, Tool::Claude, &a.id, None, &pre).unwrap_err();
    assert_eq!(err.code(), "changed");
}

#[test]
fn the_air_gap_fallback_names_what_the_file_says_and_what_was_expected() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    // No vendor answer (air-gap or a failed check): ~/.claude.json decides.
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    let text = err.to_string();
    assert!(text.contains("~/.claude.json names c@example.test"), "{text}");
    assert!(text.contains("you expected \"Default\" (a@example.test)"), "{text}");
}

#[test]
fn codex_capture_is_checked_against_the_sign_ins_own_id_token() {
    let h = Home::new();
    let conn = db();
    let jwt = |claims: Value| {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", enc.encode(b"{}"), enc.encode(claims.to_string()))
    };
    let auth = |sub: &str, email: &str| {
        serde_json::to_vec(&json!({"tokens": {
            "id_token": jwt(json!({"sub": sub, "email": email})),
            "refresh_token": format!("{MARKER}-rt-{sub}")
        }}))
        .unwrap()
    };
    fs::create_dir_all(h.p(".codex")).unwrap();
    fs::write(h.p(".codex/auth.json"), auth("user-a", "a@example.test")).unwrap();
    let a = wallet::import_live(&conn, Tool::Codex, None).unwrap().unwrap();
    let b = create(&conn, Tool::Codex, "b", None).unwrap();
    store::write_slot(Tool::Codex, &b.id, &auth("user-b", "b@example.test")).unwrap();
    // A `codex login` as C outside K2.
    fs::write(h.p(".codex/auth.json"), auth("user-c", "c@example.test")).unwrap();
    let err = wallet::switch(&conn, Tool::Codex, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Codex, &a.id)).unwrap(), auth("user-a", "a@example.test"));
}
