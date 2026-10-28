//! Saved logins in the vault (prd-llm-logins-in-vault-v1; Rosson
//! 2026-10-27: 0.46.6 keeps both stores, the switch picks the source of
//! truth, nothing is wiped). Temp HOME under the one env lock, an
//! in-memory fake vault, fake credentials carrying a marker. No CLI, no
//! keychain (off under cfg(test)), no network.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use rusqlite::Connection;

use super::*;
use crate::llm_accounts::{self as la, pins, state, wallet};
use crate::sealed_slots::fake::FakeSealer;

const MARKER: &str = "K2TEST_VAULT_LOGIN_MARKER";
const KEY_A: &str = "sk-test-K2TEST-key-aaaaaaaa";
const KEY_B: &str = "sk-test-K2TEST-key-bbbbbbbb";

struct Env {
    // Drop order: vars and the sealer first, then the home (the lock).
    _mode: Option<crate::test_env::EnvVar>,
    _scrub: Vec<crate::test_env::EnvVar>,
    v: Arc<FakeSealer>,
    home: crate::test_env::TempHome,
}

impl Env {
    fn new(mode: Storage) -> Env {
        let home = crate::test_env::TempHome::new();
        let _scrub = ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP", "K2_LLM_LOGIN_LIVE", ENV]
            .iter()
            .map(|v| crate::test_env::EnvVar::remove(v))
            .collect();
        crate::airgap::set_setting_enabled(false);
        set_setting(None);
        let v = Arc::new(FakeSealer::default());
        install_sealer(v.clone());
        let mut e = Env { _mode: None, _scrub, v, home };
        e.set_mode(mode);
        e
    }
    fn set_mode(&mut self, mode: Storage) {
        self._mode = None;
        self._mode = Some(crate::test_env::EnvVar::set(ENV, mode.as_str()));
        assert_eq!(storage(), mode);
    }
    fn live(&self) -> PathBuf {
        self.home.path().join(".claude/.credentials.json")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        uninstall_sealer();
        set_setting(None);
    }
}

fn db() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn cred(tag: &str) -> Vec<u8> {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":4102444800000,"subscriptionType":"max"}}}}"#)
        .into_bytes()
}

fn write_live(e: &Env, b: &[u8]) {
    fs::create_dir_all(e.live().parent().unwrap()).unwrap();
    fs::write(e.live(), b).unwrap();
}

fn slot_file(id: &str) -> PathBuf {
    store::slot_cred_path(Tool::Claude, id)
}

/// A signed-in idle Claude login whose CLI sign-in wrote `cred(tag)`.
fn idle(conn: &Connection, label: &str, tag: &str) -> la::Entry {
    let e = la::create(conn, Tool::Claude, label, None).unwrap();
    store::write_private(&slot_file(&e.id), &cred(tag)).unwrap();
    la::update_meta(conn, &e.id, &la::MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() }).unwrap();
    la::get(conn, &e.id).unwrap().unwrap()
}

fn vault_of(e: &Env, id: &str) -> Option<Vec<u8>> {
    e.v.value(&addr(Tool::Claude, id))
}

// ── The switch ──────────────────────────────────────────────────────

#[test]
fn files_is_the_default_and_the_env_wins_over_the_setting() {
    let _e = Env::new(Storage::Files);
    let _clear = crate::test_env::EnvVar::remove(ENV);
    assert_eq!((storage(), storage_source()), (Storage::Files, "default"));
    set_setting(Some(Storage::Vault));
    assert_eq!((storage(), storage_source()), (Storage::Vault, "setting"));
    let _env = crate::test_env::EnvVar::set(ENV, "files");
    assert_eq!((storage(), storage_source()), (Storage::Files, "env"));
    assert_eq!(Storage::parse(" VAULT "), Some(Storage::Vault));
    assert_eq!(Storage::parse("both"), None);
}

#[test]
fn files_mode_never_touches_the_vault() {
    let e = Env::new(Storage::Files);
    let conn = db();
    write_live(&e, &cred("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = idle(&conn, "b", "b");
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    pins::add_api_key(&conn, Tool::Codex, "key", KEY_A, None).unwrap();
    wallet::remove(&conn, &a.id).unwrap();
    assert_eq!(e.v.puts.load(Ordering::SeqCst), 0, "files mode is today's code: no vault write");
    assert!(e.v.m.lock().unwrap().is_empty());
}

// ── Vault mode: both stores written, the vault is the truth ────────

#[test]
fn vault_mode_seals_every_write_and_keeps_the_files() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    write_live(&e, &cred("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    assert_eq!(vault_of(&e, &a.id).expect("import sealed"), cred("a"));
    assert_eq!(fs::read(slot_file(&a.id)).unwrap(), cred("a"), "the file copy stays");
    let b = idle(&conn, "b", "b");
    // The CLI refreshed A in the live store since import; the switch saves
    // it back (file + vault) and swaps B in.
    write_live(&e, &cred("a2"));
    let out = wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    assert!(out.saved_outgoing);
    assert_eq!(fs::read(e.live()).unwrap(), cred("b"));
    assert_eq!(vault_of(&e, &a.id).unwrap(), cred("a2"), "outgoing sealed");
    assert_eq!(fs::read(slot_file(&a.id)).unwrap(), cred("a2"), "and kept as a file");
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"), "B was sealed when the switch read it");
    // The vault's item names only: no value in the DB.
    let mut st = conn.prepare("SELECT * FROM llm_accounts").unwrap();
    let n = st.column_count();
    let mut rows = st.query([]).unwrap();
    while let Some(r) = rows.next().unwrap() {
        for i in 0..n {
            if let Ok(Some(s)) = r.get::<_, Option<String>>(i) {
                assert!(!s.contains(MARKER), "a token reached the database");
            }
        }
    }
}

#[test]
fn plaintext_a_cli_wrote_is_newest_and_is_sealed_before_use() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    let b = idle(&conn, "b", "b");
    assert_eq!(read_slot(Tool::Claude, &b.id).unwrap().unwrap(), cred("b"));
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"));
    // A pinned session's CLI refreshed the slot on its own.
    store::write_private(&slot_file(&b.id), &cred("b-cli")).unwrap();
    assert_eq!(slot_meta(Tool::Claude, &b.id).plan.as_deref(), Some("max"));
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b-cli"), "the file won and was sealed");
}

#[test]
fn a_broken_file_never_replaces_a_good_vault_copy() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    let b = idle(&conn, "b", "b");
    assert!(has_cred(Tool::Claude, &b.id));
    store::write_private(&slot_file(&b.id), b"{\"claudeAiOauth\":{\"accessTo").unwrap();
    assert_eq!(read_slot(Tool::Claude, &b.id).unwrap().unwrap(), cred("b"), "the vault's whole copy is used");
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"), "never sealed over");
    let r = sync_to_vault(&la::list(&conn).unwrap(), &[]);
    assert_eq!(r.broken, vec![b.id.clone()]);
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"));
}

#[test]
fn with_no_file_the_vault_copy_is_used_and_a_spawn_gets_it_back() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    write_live(&e, &cred("a"));
    wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = idle(&conn, "b", "b");
    assert!(has_cred(Tool::Claude, &b.id));
    // A later release's explicit wipe (simulated): only the vault holds B.
    fs::remove_file(slot_file(&b.id)).unwrap();
    assert!(has_cred(Tool::Claude, &b.id));
    assert_eq!(la::next_signed_in(&conn, Tool::Claude).unwrap().map(|n| n.id), Some(b.id.clone()));
    {
        let _l = store::lock_slot(Tool::Claude, &b.id, std::time::Duration::from_secs(5)).unwrap();
        assert!(ensure_plaintext(Tool::Claude, &b.id).unwrap(), "unsealed into the slot for a CLI");
        assert!(!ensure_plaintext(Tool::Claude, &b.id).unwrap(), "already there");
    }
    assert_eq!(fs::read(slot_file(&b.id)).unwrap(), cred("b"));
    fs::remove_file(slot_file(&b.id)).unwrap();
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).expect("switch from the vault copy");
    assert_eq!(fs::read(e.live()).unwrap(), cred("b"));
}

/// prd-llm-tokens-outside-logins-v1 LT-A13 (PRD test 20): a slot file of
/// ANOTHER account (a `/login` typed in a session running in this slot's
/// home) is never sealed as the login's newest copy, and drift repair
/// moves it to its account's token and puts the vault's last matching
/// copy back, with no new sign-in.
#[test]
fn another_accounts_slot_file_is_never_sealed_and_repair_restores_the_vault_copy() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    let b = idle(&conn, "b", "b");
    let who_b = la::identity::AccountIdentity { account_uuid: Some("uuid-b".into()), email: Some("b@example.test".into()), org_uuid: None };
    la::identity::write_record(Tool::Claude, &b.id, &who_b, la::identity::Source::Vendor, la::identity::origin::K2_SIGN_IN).unwrap();
    assert_eq!(read_slot(Tool::Claude, &b.id).unwrap().unwrap(), cred("b"));
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"));
    // Claude, with its home at B's slot, signed in as C.
    store::write_private(&slot_file(&b.id), &cred("c")).unwrap();
    fs::write(
        store::slot_dir(Tool::Claude, &b.id).join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"uuid-c","emailAddress":"c@example.test"}}"#,
    )
    .unwrap();
    assert_eq!(read_slot(Tool::Claude, &b.id).unwrap().unwrap(), cred("c"), "the file is what is there");
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"), "never sealed over B's copy");
    let r = sync_to_vault(&la::list(&conn).unwrap(), &[]);
    assert_eq!(r.drift, vec![b.id.clone()]);
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"), "the sync pass doesn't seal it either");
    // Drift repair: C's sign-in to its own token; B's own copy back.
    let b_now = la::get(&conn, &b.id).unwrap().unwrap();
    let moved = wallet::heal_drift(&conn, Tool::Claude, &b_now, None, None, None).unwrap().expect("moved");
    assert_eq!(moved.label, "c@example.test");
    assert_eq!(read_slot(Tool::Claude, &moved.id).unwrap().unwrap(), cred("c"));
    assert_eq!(read_slot(Tool::Claude, &b.id).unwrap().unwrap(), cred("b"), "B's own sealed copy is back");
    assert_eq!(la::get(&conn, &b.id).unwrap().unwrap().state, state::SIGNED_IN, "no new sign-in needed");
}

#[test]
fn a_closed_vault_keeps_the_files_working_and_refuses_what_only_it_holds() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    write_live(&e, &cred("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = idle(&conn, "b", "b");
    let c = idle(&conn, "c", "c");
    assert!(has_cred(Tool::Claude, &c.id));
    fs::remove_file(slot_file(&c.id)).unwrap();
    e.v.set_broken(true);
    // B still has its file: it reads and switches (sealed later).
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).expect("file copy works");
    assert_eq!(fs::read(e.live()).unwrap(), cred("b"));
    // C lives only in the closed vault: 503, nothing moves.
    let err = slots_read_err(Tool::Claude, &c.id);
    assert_eq!(err.code(), "vault_unavailable");
    assert_eq!(err.http_status(), "503 Service Unavailable");
    let err = wallet::switch(&conn, Tool::Claude, &c.id, None, None).unwrap_err();
    assert_eq!(err.code(), "vault_unavailable", "{err}");
    assert_eq!(fs::read(e.live()).unwrap(), cred("b"), "live untouched");
    // A new API key is refused with nothing saved (no row, no file).
    let before = la::list(&conn).unwrap().len();
    let err = pins::add_api_key(&conn, Tool::Codex, "key", KEY_A, None).unwrap_err();
    assert_eq!(err.code(), "vault_unavailable");
    assert_eq!(la::list(&conn).unwrap().len(), before, "no row");
    // The sweep reports the vault closed, and catches up once it opens.
    let r = sync_to_vault(&la::list(&conn).unwrap(), &[]);
    assert!(r.vault_closed);
    e.v.set_broken(false);
    let r = sync_to_vault(&la::list(&conn).unwrap(), &[]);
    assert!(!r.vault_closed);
    assert_eq!(vault_of(&e, &a.id).unwrap(), cred("a"), "A's mirror sealed after the vault opened");
}

fn slots_read_err(tool: Tool, id: &str) -> WalletError {
    read_slot(tool, id).expect_err("must refuse")
}

#[test]
fn api_keys_go_to_both_stores_and_a_failed_seal_saves_nothing() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    let k = pins::add_api_key(&conn, Tool::Codex, "key", KEY_A, None).unwrap();
    let a = addr(Tool::Codex, &k.id);
    assert_eq!(e.v.value(&a).unwrap(), KEY_A.as_bytes());
    assert_eq!(e.v.kind(&a), Some(crate::sealed_slots::SealKind::ApiKey));
    assert_eq!(fs::read_to_string(store::api_key_path(Tool::Codex, &k.id)).unwrap(), KEY_A, "file kept");
    // Replace: a corrupted vault write leaves both stores as they were.
    e.v.corrupts.store(true, Ordering::SeqCst);
    let err = pins::replace_api_key(&conn, &k.id, KEY_B).unwrap_err();
    assert_eq!(err.code(), "vault_unavailable");
    assert!(err.to_string().contains("nothing was saved"), "{err}");
    assert_eq!(fs::read_to_string(store::api_key_path(Tool::Codex, &k.id)).unwrap(), KEY_A, "file put back");
    e.v.corrupts.store(false, Ordering::SeqCst);
    pins::replace_api_key(&conn, &k.id, KEY_B).unwrap();
    assert_eq!(e.v.value(&a).unwrap(), KEY_B.as_bytes());
    // Without the file, the key still reaches a spawn env from the vault.
    fs::remove_file(store::api_key_path(Tool::Codex, &k.id)).unwrap();
    assert_eq!(read_api_key(Tool::Codex, &k.id).unwrap().as_deref(), Some(KEY_B));
}

#[test]
fn remove_keeps_the_sealed_copy_disabled_and_the_files_in_trash() {
    let e = Env::new(Storage::Vault);
    let conn = db();
    let b = idle(&conn, "b", "b");
    assert!(has_cred(Tool::Claude, &b.id));
    let out = wallet::remove(&conn, &b.id).unwrap();
    let moved = PathBuf::from(out.moved_to.unwrap());
    assert_eq!(fs::read(moved.join(".credentials.json")).unwrap(), cred("b"), "Trash keeps the file (no wipe in 0.46.6)");
    let a = addr(Tool::Claude, &b.id);
    assert_eq!(e.v.state(&a), Some(SealedState::Disabled));
    assert_eq!(e.v.value(&a).unwrap(), cred("b"), "kept, not destroyed");
    assert!(!has_cred(Tool::Claude, &b.id), "a disabled copy is not usable");
}

// ── Migration, the way back, and back and forth ─────────────────────

#[test]
fn migration_seals_every_login_and_removed_ones_as_disabled_idempotently() {
    let mut e = Env::new(Storage::Files);
    let conn = db();
    write_live(&e, &cred("a"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = idle(&conn, "b", "b");
    let gone = idle(&conn, "gone", "gone");
    let k = pins::add_api_key(&conn, Tool::Grok, "key", KEY_A, None).unwrap();
    wallet::remove(&conn, &gone.id).unwrap();
    // An older K2's Trash name (`<id>-<ts>`) for another removed login.
    let old = idle(&conn, "old", "old");
    let old_dir = store::removed_root().join(format!("{}-{}", old.id, 1_700_000_000));
    fs::create_dir_all(store::removed_root()).unwrap();
    fs::rename(store::slot_dir(Tool::Claude, &old.id), &old_dir).unwrap();
    la::mark_removed(&conn, &old.id).unwrap();
    // A login whose sign-in never finished is skipped.
    let half = la::create(&conn, Tool::Claude, "half", None).unwrap();
    assert_eq!(e.v.puts.load(Ordering::SeqCst), 0);

    e.set_mode(Storage::Vault);
    let entries = la::list(&conn).unwrap();
    let removed = la::list_removed(&conn).unwrap();
    let r = sync_to_vault(&entries, &removed);
    assert_eq!((r.sealed, r.disabled, r.waiting, r.broken.len()), (3, 2, 0, 0), "{r:?}");
    assert_eq!(r.empty, 1, "the unfinished sign-in");
    assert_eq!(vault_of(&e, &a.id).unwrap(), cred("a"));
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b"));
    assert_eq!(e.v.value(&addr(Tool::Grok, &k.id)).unwrap(), KEY_A.as_bytes());
    for r in [&gone, &old] {
        let a = addr(Tool::Claude, &r.id);
        assert_eq!(e.v.state(&a), Some(SealedState::Disabled), "{} kept disabled", r.label);
        assert_eq!(e.v.value(&a).unwrap(), cred(&r.label));
    }
    assert!(old_dir.join(".credentials.json").exists(), "nothing wiped");
    assert!(slot_file(&b.id).exists(), "nothing wiped");
    assert!(e.v.value(&addr(Tool::Claude, &half.id)).is_none());
    // Again: nothing new.
    let puts = e.v.puts.load(Ordering::SeqCst);
    let r2 = sync_to_vault(&entries, &removed);
    assert_eq!((r2.sealed, r2.disabled), (0, 0), "{r2:?}");
    assert_eq!(e.v.puts.load(Ordering::SeqCst), puts, "idempotent: no vault write");
    let f = facts(&entries);
    let fb = f.iter().find(|x| x.id == b.id).unwrap();
    assert_eq!((fb.file, fb.vault, fb.in_step), (true, "active", Some(true)));
}

#[test]
fn switching_back_to_files_writes_missing_files_from_the_vault() {
    let mut e = Env::new(Storage::Vault);
    let conn = db();
    let b = idle(&conn, "b", "b");
    let k = pins::add_api_key(&conn, Tool::Codex, "key", KEY_A, None).unwrap();
    assert!(has_cred(Tool::Claude, &b.id));
    fs::remove_file(slot_file(&b.id)).unwrap();
    fs::remove_file(store::api_key_path(Tool::Codex, &k.id)).unwrap();
    let r = export_to_files(&la::list(&conn).unwrap());
    assert_eq!((r.restored, r.already, r.failed.len()), (2, 0, 0), "{r:?}");
    e.set_mode(Storage::Files);
    assert_eq!(fs::read(slot_file(&b.id)).unwrap(), cred("b"));
    assert_eq!(read_api_key(Tool::Codex, &k.id).unwrap().as_deref(), Some(KEY_A));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(slot_file(&b.id)).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

/// Rosson 2026-10-27: switch back and forth with no loss.
#[test]
fn switching_back_and_forth_loses_nothing() {
    let mut e = Env::new(Storage::Files);
    let conn = db();
    write_live(&e, &cred("a1"));
    let a = wallet::import_live(&conn, Tool::Claude, None).unwrap().unwrap();
    let b = idle(&conn, "b", "b1");
    let k = pins::add_api_key(&conn, Tool::Codex, "key", KEY_A, None).unwrap();

    // files → vault: the migration seals everything.
    e.set_mode(Storage::Vault);
    let r = sync_to_vault(&la::list(&conn).unwrap(), &la::list_removed(&conn).unwrap());
    assert_eq!(r.sealed, 3, "{r:?}");
    // Work in vault mode: switch to B (A's refreshed token saved), add C,
    // rotate the key.
    write_live(&e, &cred("a2"));
    wallet::switch(&conn, Tool::Claude, &b.id, None, None).unwrap();
    let c = idle(&conn, "c", "c1");
    pins::replace_api_key(&conn, &k.id, KEY_B).unwrap();

    // vault → files: instant, every file already in step.
    let r = export_to_files(&la::list(&conn).unwrap());
    assert_eq!((r.restored, r.failed.len()), (0, 0), "{r:?}");
    e.set_mode(Storage::Files);
    assert_eq!(read_slot(Tool::Claude, &a.id).unwrap().unwrap(), cred("a2"));
    assert_eq!(read_slot(Tool::Claude, &c.id).unwrap().unwrap(), cred("c1"));
    assert_eq!(read_api_key(Tool::Codex, &k.id).unwrap().as_deref(), Some(KEY_B));
    // Work in files mode (the vault falls behind): back to A, C refreshed
    // by its CLI, D added, C removed.
    write_live(&e, &cred("b2"));
    wallet::switch(&conn, Tool::Claude, &a.id, None, None).unwrap();
    let d = idle(&conn, "d", "d1");
    wallet::remove(&conn, &c.id).unwrap();

    // files → vault again: the newer files win, removed C is disabled.
    e.set_mode(Storage::Vault);
    let r = sync_to_vault(&la::list(&conn).unwrap(), &la::list_removed(&conn).unwrap());
    assert_eq!((r.waiting, r.broken.len()), (0, 0), "{r:?}");
    assert_eq!(vault_of(&e, &b.id).unwrap(), cred("b2"), "B's newer file sealed");
    assert_eq!(vault_of(&e, &d.id).unwrap(), cred("d1"));
    assert_eq!(e.v.state(&addr(Tool::Claude, &c.id)), Some(SealedState::Disabled));
    // Every login reads the same in both modes.
    let want = [(&a.id, cred("a2")), (&b.id, cred("b2")), (&d.id, cred("d1"))];
    for mode in [Storage::Vault, Storage::Files, Storage::Vault] {
        e.set_mode(mode);
        for (id, bytes) in &want {
            if la::active_id(&conn, Tool::Claude).unwrap().as_deref() == Some(id.as_str()) {
                assert_eq!(fs::read(e.live()).unwrap(), *bytes, "live A in {mode:?}");
            } else {
                assert_eq!(read_slot(Tool::Claude, id).unwrap().unwrap(), *bytes, "{id} in {mode:?}");
            }
        }
        assert_eq!(read_api_key(Tool::Codex, &k.id).unwrap().as_deref(), Some(KEY_B), "key in {mode:?}");
    }
}

// ── The switch's compare-and-swap (A3) ──────────────────────────────

#[test]
fn a_live_store_the_cli_rewrote_after_the_save_is_never_overwritten() {
    let e = Env::new(Storage::Files);
    write_live(&e, &cred("a"));
    let seen = {
        use sha2::{Digest, Sha256};
        let d: [u8; 32] = Sha256::digest(cred("a")).into();
        d
    };
    // The CLI renewed its token after the switch read and saved it.
    write_live(&e, &cred("a-renewed"));
    let err = store::write_live_expecting(Tool::Claude, &cred("b"), Some(Some(seen))).unwrap_err();
    assert_eq!(err.code(), "live_changed");
    assert_eq!(fs::read(e.live()).unwrap(), cred("a-renewed"), "the renewed token stays live");
    // The bytes it expects: written.
    let now_fp = {
        use sha2::{Digest, Sha256};
        let d: [u8; 32] = Sha256::digest(cred("a-renewed")).into();
        d
    };
    store::write_live_expecting(Tool::Claude, &cred("b"), Some(Some(now_fp))).unwrap();
    assert_eq!(fs::read(e.live()).unwrap(), cred("b"));
}

// ── Every server-login slot read goes through `slots` (A13) ────────

#[test]
fn no_server_login_code_reads_a_slot_around_the_store_switch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        root.join("src/llm_accounts/mod.rs"),
        root.join("src/llm_accounts/wallet.rs"),
        root.join("src/llm_accounts/pins.rs"),
        root.join("src/llm_accounts/identity.rs"),
        root.join("../k2-daemon/src/llm_accounts_runtime.rs"),
        root.join("../k2-daemon/src/llm_accounts_routes.rs"),
        root.join("../k2-daemon/src/llm_account_for.rs"),
        root.join("../k2-daemon/src/subscription_usage.rs"),
    ];
    let banned = [
        "store::read_slot(",
        "store::write_slot(",
        "store::slot_has_cred(",
        "store::slot_meta(",
        "store::read_api_key(",
        "store::write_api_key(",
        "store::has_api_key(",
        "store::slot_claude_access_token(",
    ];
    let mut hits = Vec::new();
    for f in &files {
        let text = fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        // Test modules may arrange slot files directly.
        let code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        for (n, line) in code.lines().enumerate() {
            for b in banned {
                if line.contains(b) {
                    hits.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "read saved logins through llm_accounts::slots, not the file store:\n{}", hits.join("\n"));
}
