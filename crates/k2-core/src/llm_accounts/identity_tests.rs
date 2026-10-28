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

fn by_label(conn: &Connection, tool: Tool, label: &str) -> Entry {
    lookup(conn, Some(tool), label).unwrap_or_else(|e| panic!("no token {label:?}: {e}"))
}

/// Rosson 2026-10-27: an outside `/login` while A is the default. The
/// switch to B saves the outside sign-in as its own token C and goes
/// through; A's slot is never touched, so switching back needs no
/// sign-in; adding works; the same account again updates C (no D); the
/// notice flag is set once.
#[test]
fn an_outside_login_is_adopted_as_its_own_token_and_the_switch_goes_through() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    let a_slot_before = fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap();
    // Someone ran `/login` as C outside K2: the live token is C's, while
    // K2's record says A is live.
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    let vendor = FakeVendor::new(&[
        ("a", "uuid-a", "a@example.test"),
        ("b", "uuid-b", "b@example.test"),
        ("c", "uuid-c", "c@example.test"),
        ("c2", "uuid-c", "c@example.test"),
        ("e", "uuid-e", "e@example.test"),
    ]);
    // Adding a token works in this state (the live store isn't touched).
    let added = wallet::begin_new(&conn, Tool::Claude, "added", None).unwrap();
    store::write_slot(Tool::Claude, &added.id, &cred("e")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &added.id).join(".claude.json"),
        json!({"oauthAccount": profile("uuid-e", "e@example.test")}).to_string(),
    )
    .unwrap();
    assert_eq!(wallet::finalize_login(&conn, &added.id, None).unwrap().state, state::SIGNED_IN);
    assert_eq!(live_bytes(&h), cred("c"), "adding left the live store alone");

    let out = wallet::switch(&conn, Tool::Claude, &b.id, Some("owner-token"), Some(&vendor)).unwrap();
    assert_eq!(out.adopted.len(), 1, "{out:?}");
    let c = &out.adopted[0];
    assert_eq!((c.label.as_str(), c.created, c.notify), ("c@example.test", true, true));
    assert_eq!((c.found_in.as_str(), c.found_under.as_str()), ("live", "Default"));
    assert!(out.needs_sign_in.is_empty());
    let c_entry = get(&conn, &c.id).unwrap().unwrap();
    assert_eq!(c_entry.created_by.as_deref(), Some(wallet::OUTSIDE_K2), "marked added outside K2");
    assert_eq!(c_entry.state, state::SIGNED_IN);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &c.id)).unwrap(), cred("c"), "C's sign-in kept whole");
    assert_eq!(identity::slot_profile(&c.id).unwrap().0["accountUuid"], "uuid-c");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), a_slot_before, "A's slot untouched");
    assert_eq!(identity::slot_profile(&a.id).unwrap().0["accountUuid"], "uuid-a", "A's record untouched");
    assert_eq!(live_bytes(&h), cred("b"));
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(b.id.as_str()));
    assert!(!serde_json::to_string(&out).unwrap().contains(MARKER), "no token in the outcome");

    // Back to A: no sign-in, its own token.
    let out = wallet::switch(&conn, Tool::Claude, &a.id, None, Some(&vendor)).unwrap();
    assert!(out.adopted.is_empty());
    assert_eq!(live_bytes(&h), cred("a"));

    // C signs in outside K2 again (a new token of the same account):
    // C's token is updated, no D, no second notice.
    write_live(&h, "c2");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap();
    assert_eq!(out.adopted.len(), 1);
    assert_eq!((out.adopted[0].id.as_str(), out.adopted[0].created, out.adopted[0].notify), (c.id.as_str(), false, false));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &c.id)).unwrap(), cred("c2"));
    assert_eq!(list_for(&conn, Tool::Claude).unwrap().len(), 4, "A, B, added, C: no duplicate");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "A still its own");
}

#[test]
fn without_the_vendor_claude_json_names_the_outside_login_and_it_is_adopted() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // `/login` as C rewrote both the token and ~/.claude.json; Anthropic
    // can't be asked (air-gap, offline, an expired access token).
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert_eq!(out.adopted.len(), 1);
    assert_eq!(out.adopted[0].label, "c@example.test");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &out.adopted[0].id)).unwrap(), cred("c"));
    assert_eq!(live_bytes(&h), cred("b"));
}

#[test]
fn a_saved_sign_in_of_another_account_is_moved_to_its_own_token_and_never_made_live() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    // B's slot holds D's token (crossed earlier); its record says B.
    store::write_slot(Tool::Claude, &b.id, &cred("d")).unwrap();
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("d", "uuid-d", "d@example.test")]);
    let err = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap_err();
    assert_eq!(err.code(), "account_moved");
    let text = err.to_string();
    assert!(text.contains("saved sign-in for \"b\" belongs to d@example.test"), "{text}");
    assert!(text.contains("\"b\" needs signing in again"), "{text}");
    assert!(!text.contains(MARKER));
    let WalletError::Moved(_, moved) = err else { panic!("not Moved") };
    assert_eq!(moved.len(), 1);
    assert_eq!((moved[0].label.as_str(), moved[0].found_in.as_str(), moved[0].notify), ("d@example.test", "slot", true));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &moved[0].id)).unwrap(), cred("d"), "D's sign-in kept");
    let b_now = get(&conn, &b.id).unwrap().unwrap();
    assert_eq!(b_now.state, state::NEEDS_LOGIN, "B says sign in again");
    assert_eq!(b_now.email.as_deref(), Some("b@example.test"), "B still shows its own account");
    assert!(!store::slot_has_cred(Tool::Claude, &b.id), "D's sign-in is no longer under B");
    assert_eq!(identity::slot_profile(&b.id).unwrap().0["accountUuid"], "uuid-b");
    let backups: Vec<_> = fs::read_dir(store::removed_root()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert!(backups.iter().any(|n| n.contains(&b.id) && n.starts_with("drift-")), "a copy in .removed: {backups:?}");
    assert_eq!(live_bytes(&h), cred("a"), "live untouched");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(a.id.as_str()));
    let live_cfg: Value = serde_json::from_str(&fs::read_to_string(h.p(".claude.json")).unwrap()).unwrap();
    assert_eq!(live_cfg["oauthAccount"]["accountUuid"], "uuid-a", "profile untouched");
    // A recheck keeps "sign in again" (not "not set up").
    assert_eq!(wallet::recheck(&conn, &b.id).unwrap().state, state::NEEDS_LOGIN);
    // And B can be signed in again.
    assert_eq!(wallet::begin_relogin(&conn, &b.id).unwrap().state, state::SIGNING_IN);
}

/// The sewlab state (2026-10-27): an outside `/login` as C, and the
/// default token A's own slot ALSO holds C's sign-in (crossed earlier),
/// while A's record still names A. Switching to R (A's account, re-added
/// as a new token) goes through; C ends up as its own token; A says
/// "sign in again" and still shows A.
#[test]
fn slot_drift_on_the_default_token_is_repaired_on_the_next_switch() {
    let h = Home::new();
    let conn = db();
    let (a, _b) = two_logins(&h, &conn);
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    store::write_slot(Tool::Claude, &a.id, &cred("c")).unwrap();
    // A page refresh in that state (field: A's row showed C's email).
    assert_eq!(wallet::recheck(&conn, &a.id).unwrap().email.as_deref(), Some("a@example.test"));
    let r = wallet::begin_new(&conn, Tool::Claude, "rA", None).unwrap();
    store::write_slot(Tool::Claude, &r.id, &cred("r")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &r.id).join(".claude.json"),
        json!({"oauthAccount": profile("uuid-a", "a@example.test")}).to_string(),
    )
    .unwrap();
    let r = wallet::finalize_login(&conn, &r.id, None).unwrap();
    let vendor = FakeVendor::new(&[("c", "uuid-c", "c@example.test"), ("r", "uuid-a", "a@example.test")]);
    let target = get(&conn, &r.id).unwrap().unwrap();
    let pre = wallet::precheck(Tool::Claude, &target, wallet::generation(&conn, Tool::Claude).unwrap(), Some(&vendor)).unwrap();
    let out = wallet::switch_prechecked(&conn, Tool::Claude, &r.id, Some("owner-token"), &pre).unwrap();
    assert_eq!(live_bytes(&h), cred("r"), "rA is live");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(r.id.as_str()));
    let c = by_label(&conn, Tool::Claude, "c@example.test");
    assert_eq!(c.created_by.as_deref(), Some(wallet::OUTSIDE_K2));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &c.id)).unwrap(), cred("c"));
    assert_eq!(out.needs_sign_in, vec!["Default".to_string()]);
    assert_eq!(out.adopted.iter().filter(|x| x.notify).count(), 1, "one notice: {out:?}");
    assert!(out.adopted.iter().all(|x| x.id == c.id), "both finds went to C: {out:?}");
    let a_now = get(&conn, &a.id).unwrap().unwrap();
    assert_eq!(a_now.state, state::NEEDS_LOGIN);
    assert_eq!(a_now.email.as_deref(), Some("a@example.test"));
    assert!(!store::slot_has_cred(Tool::Claude, &a.id));
    assert_eq!(list_for(&conn, Tool::Claude).unwrap().len(), 4, "Default, b, rA, C");
}

/// The same state with no answer from Anthropic: the default token's slot
/// holds the same sign-in as the live one, and Claude's own config in
/// that slot names the outside account. Still repaired.
#[test]
fn slot_drift_is_repaired_without_the_vendor_when_the_slot_config_names_the_other_account() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    store::write_slot(Tool::Claude, &a.id, &cred("c")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &a.id).join(".claude.json"),
        json!({"oauthAccount": profile("uuid-c", "c@example.test"), "projects": {}}).to_string(),
    )
    .unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert_eq!(out.needs_sign_in, vec!["Default".to_string()], "{out:?}");
    let c = by_label(&conn, Tool::Claude, "c@example.test");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &c.id)).unwrap(), cred("c"));
    assert!(!store::slot_has_cred(Tool::Claude, &a.id));
    // Claude's config in A's slot names A again; the rest is kept.
    let cfg: Value = serde_json::from_str(&fs::read_to_string(store::slot_dir(Tool::Claude, &a.id).join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cfg["oauthAccount"]["accountUuid"], "uuid-a");
    assert!(cfg["projects"].is_object());
    assert_eq!(live_bytes(&h), cred("b"));
}

/// Someone signed in outside K2 as B, an account that already has a token,
/// then switches to B: B's token takes the newer sign-in, no new token.
#[test]
fn an_outside_login_of_the_target_account_goes_to_the_target_token() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    write_live(&h, "b2");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-b", "b@example.test"))).unwrap();
    let vendor = FakeVendor::new(&[("a", "uuid-a", "a@example.test"), ("b", "uuid-b", "b@example.test"), ("b2", "uuid-b", "b@example.test")]);
    let target = get(&conn, &b.id).unwrap().unwrap();
    let pre = wallet::precheck(Tool::Claude, &target, wallet::generation(&conn, Tool::Claude).unwrap(), Some(&vendor)).unwrap();
    let out = wallet::switch_prechecked(&conn, Tool::Claude, &b.id, None, &pre).unwrap();
    assert_eq!(out.adopted.len(), 1);
    assert_eq!((out.adopted[0].id.as_str(), out.adopted[0].created, out.adopted[0].notify), (b.id.as_str(), false, false));
    assert_eq!(live_bytes(&h), cred("b2"), "the newer sign-in stays live");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &b.id)).unwrap(), cred("b2"));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "A untouched");
    assert_eq!(active_id(&conn, Tool::Claude).unwrap().as_deref(), Some(b.id.as_str()));
    assert_eq!(list_for(&conn, Tool::Claude).unwrap().len(), 2);
}

#[test]
fn signing_in_again_works_while_another_account_is_live() {
    let h = Home::new();
    let conn = db();
    let (a, _b) = two_logins(&h, &conn);
    write_live(&h, "c");
    fs::write(h.p(".claude.json"), claude_json(&profile("uuid-c", "c@example.test"))).unwrap();
    // A is on record as live, but C is: signing A in again saves C first.
    let (e, adopted) = wallet::begin_relogin_settled(&conn, &a.id, Some("owner-token")).unwrap();
    assert_eq!(e.state, state::SIGNING_IN);
    assert_eq!(adopted.len(), 1);
    assert_eq!(live_owner(&conn, Tool::Claude).unwrap().as_deref(), Some(adopted[0].id.as_str()), "C is on record as live");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "A untouched");
    assert_eq!(live_bytes(&h), cred("c"), "the live store untouched");
    restore_signed_in(&conn, &a.id);
}

#[test]
fn a_live_swap_sign_in_works_while_another_account_is_live() {
    let h2 = Home::new();
    let conn = db();
    let (a, _b) = two_logins(&h2, &conn);
    write_live(&h2, "d");
    fs::write(h2.p(".claude.json"), claude_json(&profile("uuid-d", "d@example.test"))).unwrap();
    let new = wallet::begin_new(&conn, Tool::Claude, "fresh", None).unwrap();
    let ticket = wallet::begin_live_swap(&conn, Tool::Claude, &new.id, None).unwrap();
    assert_eq!(ticket.adopted.len(), 1);
    assert!(ticket.adopted[0].notify);
    assert_eq!(ticket.previous_id.as_deref(), Some(ticket.adopted[0].id.as_str()), "D goes back after the sign-in");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "A untouched");
    // The tool's own `/login` writes the token and its profile.
    fs::write(h2.p(".claude/.credentials.json"), cred("f")).unwrap();
    fs::write(h2.p(".claude.json"), claude_json(&profile("uuid-f", "f@example.test"))).unwrap();
    let got = wallet::live_swap_poll(&ticket).unwrap().expect("changed");
    wallet::live_swap_complete(&conn, &ticket, &got, None).unwrap();
    assert_eq!(fs::read(h2.p(".claude/.credentials.json")).unwrap(), cred("d"), "D restored");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &new.id)).unwrap(), cred("f"));
    assert_eq!(identity::read_record(Tool::Claude, &new.id).unwrap().account_uuid.as_deref(), Some("uuid-f"));
}

fn restore_signed_in(conn: &Connection, id: &str) {
    update_meta(conn, id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
}

/// LT-A3 + lineage: after K2's own switches, ~/.claude.json is K2's
/// writing, so it names nobody. The live sign-in is A's own (the CLI
/// renewed its access token; same refresh token as A's slot): it goes back
/// into A's slot, with no "checking" token and nothing adopted.
#[test]
fn a_claude_json_k2_wrote_and_the_owners_own_sign_in_capture_into_the_owner() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    let mut renewed: Value = serde_json::from_slice(&cred("a")).unwrap();
    renewed["claudeAiOauth"]["accessToken"] = json!(format!("{MARKER}-access-a-renewed"));
    let renewed = serde_json::to_vec(&renewed).unwrap();
    fs::write(h.p(".claude/.credentials.json"), &renewed).unwrap();
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.adopted.is_empty(), "{out:?}");
    assert!(out.saved_outgoing);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), renewed, "A's own renewed sign-in");
    assert!(list_for(&conn, Tool::Claude).unwrap().iter().all(|e| e.state != state::UNKNOWN), "no checking token");
    assert_eq!(list_for(&conn, Tool::Claude).unwrap().len(), 2);
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
fn codex_an_outside_login_is_adopted_by_its_own_id_token_and_nothing_is_blocked() {
    let h = Home::new();
    let conn = db();
    let jwt = |claims: Value| {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", enc.encode(b"{}"), enc.encode(claims.to_string()))
    };
    let auth = |sub: &str, email: &str, n: u32| {
        serde_json::to_vec(&json!({"tokens": {
            "id_token": jwt(json!({"sub": sub, "email": email})),
            "refresh_token": format!("{MARKER}-rt-{sub}-{n}")
        }}))
        .unwrap()
    };
    fs::create_dir_all(h.p(".codex")).unwrap();
    fs::write(h.p(".codex/auth.json"), auth("user-a", "a@example.test", 1)).unwrap();
    let a = wallet::import_live(&conn, Tool::Codex, None).unwrap().unwrap();
    let b = create(&conn, Tool::Codex, "b", None).unwrap();
    store::write_slot(Tool::Codex, &b.id, &auth("user-b", "b@example.test", 1)).unwrap();
    // A `codex login` as C outside K2.
    fs::write(h.p(".codex/auth.json"), auth("user-c", "c@example.test", 1)).unwrap();
    // A page refresh keeps A's account on A.
    assert_eq!(wallet::recheck(&conn, &a.id).unwrap().email.as_deref(), Some("a@example.test"));
    // Adding a token works in this state.
    let d = wallet::begin_new(&conn, Tool::Codex, "d", None).unwrap();
    store::write_slot(Tool::Codex, &d.id, &auth("user-d", "d@example.test", 1)).unwrap();
    assert_eq!(wallet::finalize_login(&conn, &d.id, None).unwrap().state, state::SIGNED_IN);

    let out = wallet::switch(&conn, Tool::Codex, &b.id, None, None).unwrap();
    assert_eq!(out.adopted.len(), 1);
    let c = out.adopted[0].clone();
    assert_eq!((c.label.as_str(), c.created, c.notify), ("c@example.test", true, true));
    assert_eq!(get(&conn, &c.id).unwrap().unwrap().created_by.as_deref(), Some(wallet::OUTSIDE_K2));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Codex, &c.id)).unwrap(), auth("user-c", "c@example.test", 1));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Codex, &a.id)).unwrap(), auth("user-a", "a@example.test", 1), "A untouched");
    assert_eq!(fs::read(h.p(".codex/auth.json")).unwrap(), auth("user-b", "b@example.test", 1));

    // Back to A with no sign-in.
    let out = wallet::switch(&conn, Tool::Codex, &a.id, None, None).unwrap();
    assert!(out.adopted.is_empty());
    assert_eq!(fs::read(h.p(".codex/auth.json")).unwrap(), auth("user-a", "a@example.test", 1));

    // C again (a new sign-in of the same account): C's token, no new one.
    fs::write(h.p(".codex/auth.json"), auth("user-c", "c@example.test", 2)).unwrap();
    let out = wallet::switch(&conn, Tool::Codex, &b.id, None, None).unwrap();
    assert_eq!((out.adopted[0].id.as_str(), out.adopted[0].created, out.adopted[0].notify), (c.id.as_str(), false, false));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Codex, &c.id)).unwrap(), auth("user-c", "c@example.test", 2));
    assert_eq!(list_for(&conn, Tool::Codex).unwrap().len(), 4, "Default, b, d, C");
}

#[test]
fn codex_a_slot_holding_another_accounts_sign_in_is_moved_out() {
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
    let _a = wallet::import_live(&conn, Tool::Codex, None).unwrap().unwrap();
    let b = wallet::begin_new(&conn, Tool::Codex, "b", None).unwrap();
    store::write_slot(Tool::Codex, &b.id, &auth("user-b", "b@example.test")).unwrap();
    let b = wallet::finalize_login(&conn, &b.id, None).unwrap();
    assert_eq!(b.email.as_deref(), Some("b@example.test"));
    // A `codex login` as E ran with CODEX_HOME at B's slot.
    store::write_slot(Tool::Codex, &b.id, &auth("user-e", "e@example.test")).unwrap();
    assert_eq!(wallet::recheck(&conn, &b.id).unwrap().email.as_deref(), Some("b@example.test"), "B keeps its account");
    let err = wallet::switch(&conn, Tool::Codex, &b.id, None, None).unwrap_err();
    assert_eq!(err.code(), "account_moved", "{err}");
    let e = by_label(&conn, Tool::Codex, "e@example.test");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Codex, &e.id)).unwrap(), auth("user-e", "e@example.test"));
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, state::NEEDS_LOGIN);
    assert!(!store::slot_has_cred(Tool::Codex, &b.id));
    assert_eq!(fs::read(h.p(".codex/auth.json")).unwrap(), auth("user-a", "a@example.test"), "live untouched");
}

/// A vendor spy: counts refreshes (each one succeeds and changes nothing)
/// and answers who each fake token belongs to.
struct Spy {
    refreshes: std::sync::atomic::AtomicUsize,
    vendor: FakeVendor,
}

impl Refresher for Spy {
    fn refresh(&self, _tool: Tool, _id: &str) -> Result<(), String> {
        self.refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn identify(&self, tool: Tool, b: &[u8]) -> Option<AccountIdentity> {
        self.vendor.identify(tool, b)
    }
}

fn spy(rows: &[(&str, &str, &str)]) -> Spy {
    Spy { refreshes: std::sync::atomic::AtomicUsize::new(0), vendor: FakeVendor::new(rows) }
}

/// A Claude credential of sign-in `tag` whose access token expires in a
/// minute (due for a refresh).
fn expiring(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "claudeAiOauth": {
            "accessToken": format!("{MARKER}-access-{tag}"),
            "refreshToken": format!("{MARKER}-refresh-{tag}"),
            "expiresAt": now() * 1000 + 60_000,
            "subscriptionType": "max"
        }
    }))
    .unwrap()
}

/// LT-A3 + LT-A8 (PRD tests 5 and 7): `~/.claude.json` K2 wrote itself
/// never names the live sign-in. With no vendor answer the live bytes go
/// to a "checking" token, never into A's slot, and the switch completes.
/// A later answer names it (renamed, recorded, one notice).
#[test]
fn a_claude_json_k2_wrote_never_names_the_live_sign_in_and_unknown_goes_to_a_checking_token() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    let cfg: Value = serde_json::from_str(&fs::read_to_string(h.p(".claude.json")).unwrap()).unwrap();
    assert_eq!(cfg["oauthAccount"]["accountUuid"], "uuid-a", "K2 wrote A's profile");
    // An outside `/login` as C left C's token live; the file still names A
    // (written by K2), and Anthropic can't be asked.
    write_live(&h, "c");
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.adopted.is_empty(), "nobody could name it: {out:?}");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "never into A's slot");
    assert_eq!(live_bytes(&h), cred("b"), "the switch completed");
    let checking: Vec<Entry> = list_for(&conn, Tool::Claude).unwrap().into_iter().filter(|e| e.state == state::UNKNOWN).collect();
    assert_eq!(checking.len(), 1);
    let u = &checking[0];
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &u.id)).unwrap(), cred("c"));
    assert!(identity::read_record(Tool::Claude, &u.id).is_none(), "no account on record yet");
    // Later Anthropic answers: it is C, a new account here.
    let v = spy(&[("c", "uuid-c", "c@example.test")]);
    let named = wallet::resolve_unverified(&conn, Tool::Claude, &v, now()).unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!((named[0].id.as_str(), named[0].label.as_str(), named[0].created, named[0].notify), (u.id.as_str(), "c@example.test", true, true));
    let u_now = get(&conn, &u.id).unwrap().unwrap();
    assert_eq!(u_now.state, state::SIGNED_IN);
    assert_eq!(u_now.created_by.as_deref(), Some(wallet::OUTSIDE_K2));
    assert_eq!(identity::read_record(Tool::Claude, &u.id).unwrap().account_uuid.as_deref(), Some("uuid-c"));
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "C's bytes were never in A's slot");
    assert_eq!(v.refreshes.load(std::sync::atomic::Ordering::SeqCst), 0, "far from expiry: no refresh");
}

/// LT-A8: a checking token whose account already has an idle token merges
/// into it (newer bytes win) and the checking row is gone.
#[test]
fn a_checking_token_of_a_known_account_merges_into_that_token() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    // A's own CLI rotated its sign-in (a2, a later expiry); offline.
    let a2 = serde_json::to_vec(&json!({"claudeAiOauth": {
        "accessToken": format!("{MARKER}-access-a2"), "refreshToken": format!("{MARKER}-refresh-a2"),
        "expiresAt": FAR + 1000, "subscriptionType": "max"
    }}))
    .unwrap();
    fs::write(h.p(".claude/.credentials.json"), &a2).unwrap();
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    let u = list_for(&conn, Tool::Claude).unwrap().into_iter().find(|e| e.state == state::UNKNOWN).expect("a checking token");
    let mut rows = FakeVendor::new(&[]).accounts.into_inner().unwrap();
    rows.push((format!("{MARKER}-access-a2"), AccountIdentity { account_uuid: Some("uuid-a".into()), email: Some("a@example.test".into()), org_uuid: None }));
    let v = Spy { refreshes: std::sync::atomic::AtomicUsize::new(0), vendor: FakeVendor { accounts: Mutex::new(rows) } };
    let named = wallet::resolve_unverified(&conn, Tool::Claude, &v, now()).unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!((named[0].id.as_str(), named[0].created, named[0].notify), (a.id.as_str(), false, false));
    assert!(get(&conn, &u.id).unwrap().is_none(), "the checking row is gone");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), a2, "A has its newer sign-in back");
}

/// LT-A10 (PRD test 9): a slot holding the live sign-in's refresh token is
/// never refreshed by K2 (keep-warm, the switch's lazy refresh).
#[test]
fn a_copy_of_the_live_sign_in_is_never_refreshed() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    // B's slot holds a copy of the live sign-in (same refresh token), due.
    let mut copy: Value = serde_json::from_slice(&expiring("a")).unwrap();
    copy["claudeAiOauth"]["accessToken"] = json!(format!("{MARKER}-access-b-copy"));
    store::write_slot(Tool::Claude, &b.id, &serde_json::to_vec(&copy).unwrap()).unwrap();
    assert!(identity::shares_live_grant(Tool::Claude, &b.id));
    let v = spy(&[]);
    let res = wallet::keep_warm(&conn, &v, now(), &|_| false).unwrap();
    assert!(res.is_empty(), "{res:?}");
    let target = get(&conn, &b.id).unwrap().unwrap();
    wallet::precheck(Tool::Claude, &target, wallet::generation(&conn, Tool::Claude).unwrap(), Some(&v)).unwrap();
    assert_eq!(v.refreshes.load(std::sync::atomic::Ordering::SeqCst), 0, "no refresh of the live lineage");
}

/// LT-A9: keep-warm refreshes an idle slot, then the vendor names it: it
/// is another account's, so it moves to that account's token.
#[test]
fn keep_warm_finds_and_repairs_a_drifted_slot() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    let d = expiring("d");
    store::write_slot(Tool::Claude, &b.id, &d).unwrap();
    let v = spy(&[("d", "uuid-d", "d@example.test")]);
    let res = wallet::keep_warm(&conn, &v, now(), &|_| false).unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(v.refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
    let moved = res[0].adopted.clone().expect("moved");
    assert_eq!((moved.label.as_str(), moved.found_in.as_str(), moved.notify), ("d@example.test", "slot", true));
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, state::NEEDS_LOGIN);
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &moved.id)).unwrap(), d, "D's sign-in kept whole");
    assert!(!store::slot_has_cred(Tool::Claude, &b.id));
    assert_eq!(identity::read_record(Tool::Claude, &b.id).unwrap().account_uuid.as_deref(), Some("uuid-b"));
}

/// LT-A4: the one guarded write refuses another account's bytes, and bytes
/// nobody can name unless they are the same sign-in.
#[test]
fn the_guarded_write_never_files_another_accounts_sign_in() {
    let h = Home::new();
    let conn = db();
    let (a, _b) = two_logins(&h, &conn);
    let c = AccountIdentity { account_uuid: Some("uuid-c".into()), email: Some("c@example.test".into()), org_uuid: None };
    let a_entry = get(&conn, &a.id).unwrap().unwrap();
    let err = wallet::file_bytes(Tool::Claude, &a.id, Some(&a_entry), &cred("c"), wallet::Incoming::Known(&c, Source::Vendor), "test").unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    let err = wallet::file_bytes(Tool::Claude, &a.id, Some(&a_entry), &cred("c"), wallet::Incoming::Unknown, "test").unwrap_err();
    assert_eq!(err.code(), "account_mismatch");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), cred("a"), "nothing written");
    // The same sign-in (a newer access token) and K2's own refresh go in.
    let mut newer: Value = serde_json::from_slice(&cred("a")).unwrap();
    newer["claudeAiOauth"]["accessToken"] = json!(format!("{MARKER}-access-a-new"));
    let newer = serde_json::to_vec(&newer).unwrap();
    wallet::file_bytes(Tool::Claude, &a.id, Some(&a_entry), &newer, wallet::Incoming::Unknown, "test").unwrap();
    wallet::file_bytes(Tool::Claude, &a.id, None, &newer, wallet::Incoming::OwnRefresh, "test").unwrap();
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &a.id)).unwrap(), newer);
}

/// LT-A11 (PRD test 12): signing token B in again as another account files
/// that sign-in under the account's own token; B keeps its record and
/// needs its own sign-in.
#[test]
fn signing_a_token_in_again_as_another_account_files_it_elsewhere() {
    let h = Home::new();
    let conn = db();
    let (_a, b) = two_logins(&h, &conn);
    wallet::begin_relogin(&conn, &b.id).unwrap();
    // Claude's own login, with its home at B's slot, as C.
    fs::write(store::slot_cred_path(Tool::Claude, &b.id), cred("c")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &b.id).join(".claude.json"),
        json!({"oauthAccount": profile("uuid-c", "c@example.test")}).to_string(),
    )
    .unwrap();
    let err = wallet::finalize_login(&conn, &b.id, None).unwrap_err();
    assert_eq!(err.code(), "account_moved");
    assert!(err.to_string().contains("That sign-in is c@example.test's"), "{err}");
    let WalletError::Moved(_, moved) = err else { panic!("not Moved") };
    assert!(!moved[0].notify, "a K2 sign-in, not one made outside K2");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Claude, &moved[0].id)).unwrap(), cred("c"));
    assert_eq!(identity::read_record(Tool::Claude, &b.id).unwrap().account_uuid.as_deref(), Some("uuid-b"), "B's record never moves");
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, state::NEEDS_LOGIN);
    assert_eq!(identity::slot_profile(&b.id).unwrap().0["accountUuid"], "uuid-b");
}

/// LT-A12 (PRD test 14): one notice per account per server, even after the
/// adopted token is removed and the account signs in outside K2 again.
/// The decisions file holds account keys only.
#[test]
fn an_account_is_told_about_once() {
    let h = Home::new();
    let conn = db();
    let (a, b) = two_logins(&h, &conn);
    let vendor = FakeVendor::new(&[
        ("a", "uuid-a", "a@example.test"),
        ("b", "uuid-b", "b@example.test"),
        ("c", "uuid-c", "c@example.test"),
        ("c2", "uuid-c", "c@example.test"),
    ]);
    write_live(&h, "c");
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap();
    assert!(out.adopted[0].notify);
    wallet::remove(&conn, &out.adopted[0].id).unwrap();
    wallet::switch(&conn, Tool::Claude, &a.id, None, Some(&vendor)).unwrap();
    write_live(&h, "c2");
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, Some(&vendor)).unwrap();
    assert_eq!(out.adopted.len(), 1);
    assert!(out.adopted[0].created, "a new token: the old one was removed");
    assert!(!out.adopted[0].notify, "the same account is not told about twice");
    let decisions = fs::read_to_string(wallet::outside_decisions_path(Tool::Claude)).unwrap();
    assert!(decisions.contains("uuid-c"), "{decisions}");
    assert!(!decisions.contains(MARKER) && !decisions.contains("@"), "account keys only: {decisions}");
}

#[test]
fn grok_names_its_account_from_auth_json_and_an_outside_login_is_adopted() {
    let h = Home::new();
    let conn = db();
    let auth = |user: &str, email: &str, n: u32| {
        serde_json::to_vec(&json!({"https://auth.example.test::client": {
            "user_id": user, "email": email, "principal_id": format!("p-{user}"), "team_id": "t1",
            "refresh_token": format!("{MARKER}-grok-rt-{user}-{n}"), "key": "jwt"
        }}))
        .unwrap()
    };
    let who = identity::cred_identity(Tool::Grok, &auth("g-a", "a@example.test", 1)).expect("named");
    assert_eq!((who.account_uuid.as_deref(), who.email.as_deref()), (Some("g-a"), Some("a@example.test")));
    let two: Value = json!({
        "i1::c": {"user_id": "g-a", "refresh_token": "r1"},
        "i2::c": {"user_id": "g-b", "refresh_token": "r2"}
    });
    assert!(identity::cred_identity(Tool::Grok, two.to_string().as_bytes()).is_none(), "two users: nobody");
    fs::create_dir_all(h.p(".grok")).unwrap();
    fs::write(h.p(".grok/auth.json"), auth("g-a", "a@example.test", 1)).unwrap();
    let a = wallet::import_live(&conn, Tool::Grok, None).unwrap().unwrap();
    let b = create(&conn, Tool::Grok, "b", None).unwrap();
    store::write_slot(Tool::Grok, &b.id, &auth("g-b", "b@example.test", 1)).unwrap();
    fs::write(h.p(".grok/auth.json"), auth("g-c", "c@example.test", 1)).unwrap();
    let out = wallet::switch(&conn, Tool::Grok, &b.id, None, None).unwrap();
    assert_eq!(out.adopted.len(), 1);
    assert_eq!(out.adopted[0].label, "c@example.test");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Grok, &a.id)).unwrap(), auth("g-a", "a@example.test", 1), "A untouched");
    assert_eq!(fs::read(store::slot_cred_path(Tool::Grok, &out.adopted[0].id)).unwrap(), auth("g-c", "c@example.test", 1));
}
