//! prd-box-move-carry-v1 S1/S5 (vault side): export → freeze → import,
//! the checks of §4, and the rollbacks of §7. Fakes only.

use std::sync::Arc;

use super::*;
use crate::confirm::KdfCost;
use crate::fsutil::TestDir;
use crate::keystore::fake::FakeKeyStore;
use crate::model::{Delivery, Kind, Namespace};
use crate::store::{PutRequest, SecretStore};

fn opts() -> OpenOptions {
    OpenOptions { kdf: KdfCost::Floor }
}

fn open(dir: &Path, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(dir, ks.clone() as Arc<dyn KeyStore>, opts()).expect("open")
}

fn put_system(v: &LocalVault, engine: &str, name: &str, value: &str) {
    let mut r = PutRequest::new(Namespace::System, Scope::engine(engine), name, Kind::Password);
    r.default_delivery = Some(Delivery::None);
    v.put(r, crate::secret::SecretBytes::from_slice(value.as_bytes()), &Actor::System("t".into())).expect("put system");
}

fn put_user(v: &LocalVault, name: &str) {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::ApiKey);
    r.hosts = vec!["api.github.com".into()];
    v.put(r, crate::secret::SecretBytes::from_slice(b"ghp_test_value_123456"), &Actor::Owner).expect("put user");
}

fn value(v: &LocalVault, engine: &str, name: &str) -> String {
    let r = v.resolve(&Selector::system(Scope::engine(engine), name), Use::System, &Requester::System("t".into())).expect("resolve");
    String::from_utf8(r.value.expose().to_vec()).expect("text")
}

/// The old box: a vault with a tunnel token, a mail credential and two of
/// the owner's secrets.
fn old_box(t: &TestDir) -> (LocalVault, Arc<FakeKeyStore>) {
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t.path().join("old"), &ks);
    put_system(&v, "frpc", "tunnel.token", "k2tok_moved_with_the_vault");
    put_system(&v, "stalwart", "admin", "mail-admin-pass");
    put_user(&v, "GH_TOKEN");
    put_user(&v, "GH_TOKEN_2");
    (v, ks)
}

fn ok() -> crate::policy::HumanProof {
    crate::policy::HumanProof::Confirmed { method: "t".into() }
}

fn src() -> MoveSource {
    MoveSource { host: "acv-old".into(), tunnel_subdomain: Some("acv".into()), tunnel_device_id: Some("dev-1".into()) }
}

#[test]
fn a_move_carries_every_namespace_and_the_new_box_anchors_above_its_old_gen() {
    let t = TestDir::new("move-roundtrip");
    let (old, old_ks) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    assert_eq!(exp.manifest.needs, NEEDS_V1.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "needs from v1");
    assert_eq!((exp.manifest.user_secrets, exp.manifest.system_secrets), old.active_counts());
    assert_eq!(exp.manifest.move_key, fingerprint(&key.recipient()));
    assert!(old.is_frozen());
    assert!(
        old.unconfirmed_holders().expect("doctor").iter().all(|u| u.holder.kind != MOVE_HOLDER),
        "the move holder is a confirmed holder change (VU-B1)"
    );
    // The new box: a reused one whose keeper remembers a much higher gen.
    let new_dir = t.path().join("new");
    let new_ks = Arc::new(FakeKeyStore::new());
    new_ks.force_anchor(Some(Anchor { vault_id: None, gen: 1000, audit_seq: None, audit_mac: None }));
    let mf = open_move_file(&key, &exp.file).expect("open the file");
    // Dry run first: changes nothing here.
    let scratch = scratch_dir(&new_dir);
    let check = check_move(&mf, &key, &scratch, KdfCost::Floor, &[("frpc", "tunnel.token"), ("frpc", "nope")]).expect("dry run");
    assert_eq!(check.system_present, vec![("engine:frpc/tunnel.token".to_string(), true), ("engine:frpc/nope".to_string(), false)]);
    assert!(!new_dir.join("vault.db").exists(), "the dry run never touches this box's vault");
    assert!(!scratch.join("vault.db").exists(), "the scratch copy is gone");
    let imported = apply_move(&new_dir, new_ks.clone() as Arc<dyn KeyStore>, &key, &mf, opts()).expect("apply");
    let v = imported.vault;
    assert_eq!(v.vault_id(), old.vault_id());
    assert_eq!(value(&v, "frpc", "tunnel.token"), "k2tok_moved_with_the_vault");
    assert_eq!(value(&v, "stalwart", "admin"), "mail-admin-pass");
    assert_eq!(v.active_counts(), old.active_counts());
    assert!(!v.in_rollback());
    assert!(v.tamper_notes().is_empty(), "{:?}", v.tamper_notes());
    assert!(!v.wrap_holders().iter().any(|h| h == MOVE_HOLDER), "the move holder is dropped");
    assert!(!v.is_frozen(), "the new box isn't frozen");
    let (gen, _, _) = v.head().expect("head");
    assert!(gen > 1000, "gen = max(snapshot, anchor) + 1, then the holder drop: {gen}");
    assert_eq!(new_ks.anchor().expect("anchor").map(|a| a.gen), Some(gen), "anchored");
    // The new box's own key wraps it: a reopen works and is clean.
    drop(v);
    let v2 = open(&new_dir, &new_ks);
    assert!(!v2.in_rollback());
    assert_eq!(value(&v2, "frpc", "tunnel.token"), "k2tok_moved_with_the_vault");
    // The old box's key and vault were never touched.
    assert!(old_ks.has_kek());
    assert_eq!(value(&old, "frpc", "tunnel.token"), "k2tok_moved_with_the_vault");
}

#[test]
fn a_frozen_vault_reads_refuses_writes_survives_a_restart_and_unfreezes() {
    let t = TestDir::new("move-freeze");
    let (old, ks) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    let (gen0, _, _) = old.head().expect("head");
    assert_eq!(gen0, exp.manifest.gen, "the snapshot is the vault as frozen");
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "NEW_ONE", Kind::ApiKey);
    r.hosts = vec!["api.github.com".into()];
    let e = old.put(r, crate::secret::SecretBytes::from_slice(b"value_123456789"), &Actor::Owner).expect_err("frozen");
    assert_eq!((e.code, e.status), ("vault_moved_out", 423));
    assert_eq!(value(&old, "frpc", "tunnel.token"), "k2tok_moved_with_the_vault", "reads work");
    old.flush_rollups();
    old.tick().expect("tick");
    assert_eq!(old.head().expect("head").0, gen0, "reads, roll-ups and the ticker never move gen while frozen");
    assert!(old.rekey(&Actor::Owner, &crate::policy::HumanProof::Confirmed { method: "t".into() }).is_err());
    // A restart keeps the freeze.
    let dir = old.dir().to_path_buf();
    drop(old);
    let old = open(&dir, &ks);
    assert!(old.is_frozen());
    assert_eq!(old.frozen_marker().expect("marker").move_key, fingerprint(&key.recipient()));
    // Unfreeze: writes work again and the move holder is gone.
    assert!(old.move_unfreeze(&Actor::Owner).expect("unfreeze"));
    assert!(!old.is_frozen());
    assert!(!dir.join(FROZEN_MARKER).exists());
    assert!(!old.wrap_holders().iter().any(|h| h == MOVE_HOLDER));
    put_user(&old, "AFTER_UNFREEZE");
    // Agents never export or unfreeze.
    let agent = Actor::Agent(crate::policy::AgentActor { workspace_id: "w".into(), session_id: None, vault_manage: true, shell: false });
    assert_eq!(old.move_export(&key.recipient(), src(), &agent, &ok()).err().map(|e| e.code), Some("vault_admin_required"));
    assert_eq!(old.move_unfreeze(&agent).err().map(|e| e.code), Some("vault_admin_required"));
    // No export without a person's confirm.
    assert_eq!(old.move_export(&key.recipient(), src(), &Actor::Owner, &crate::policy::HumanProof::None).err().map(|e| e.code), Some("vault_confirm_required"));
    // `add_wrap` can't make a move holder.
    assert_eq!(old.add_wrap(MOVE_HOLDER, &key.recipient(), &Actor::Owner, &ok()).err().map(|e| e.code), Some("holder_reserved"));
}

#[test]
fn an_export_is_refused_mid_rotation_and_in_rollback() {
    let t = TestDir::new("move-refuse");
    let (old, ks) = old_box(&t);
    let key = Identity::generate();
    ks.stage_next().expect("stage");
    assert_eq!(old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).err().map(|e| e.code), Some("kek_rotation_pending"));
    assert!(!old.is_frozen(), "a refused export freezes nothing");
    ks.abort_next().expect("abort");
    old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export after the abort");
}

#[test]
fn tampered_wrong_key_and_newer_files_are_refused_by_name() {
    let t = TestDir::new("move-tamper");
    let (old, _) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    // Another box's move key.
    let other = Identity::generate();
    assert_eq!(open_move_file(&other, &exp.file).err().map(|e| e.code), Some("move_key_wrong"));
    // A flipped byte in the encrypted body.
    let mut flipped = exp.file.clone();
    let n = flipped.len() - 20;
    flipped[n] ^= 0x40;
    assert_eq!(open_move_file(&key, &flipped).err().map(|e| e.code), Some("move_file_bad"));
    // Re-encrypt with a changed manifest and the old signature.
    let raw = agefmt::decrypt(&key, &exp.file).expect("decrypt");
    let env: Envelope = serde_json::from_slice(raw.expose()).expect("env");
    let mut m: MoveManifest = serde_json::from_str(&env.manifest).expect("manifest");
    m.gen += 5;
    let forged = Envelope { manifest: serde_json::to_string(&m).expect("json"), ..clone_env(&env) };
    let file = agefmt::encrypt(&[key.recipient()], &serde_json::to_vec(&forged).expect("json"));
    let e = open_move_file(&key, &file).err().expect("forged manifest");
    assert_eq!(e.code, "move_file_bad");
    assert!(e.hint.contains("signature"), "{}", e.hint);
    // A swapped snapshot under the true manifest.
    let mut snap = STANDARD.decode(&env.snapshot_b64).expect("b64");
    snap[200] ^= 1;
    let swapped = Envelope { snapshot_b64: STANDARD.encode(&snap), ..clone_env(&env) };
    let file = agefmt::encrypt(&[key.recipient()], &serde_json::to_vec(&swapped).expect("json"));
    let e = open_move_file(&key, &file).err().expect("swapped snapshot");
    assert!(e.code == "move_file_bad" && e.hint.contains("sha256"), "{e}");
    // A file that needs a feature this K2 doesn't have, signed properly.
    let mut loose: serde_json::Value = serde_json::from_str(&env.manifest).expect("json");
    loose["needs"] = serde_json::json!(["vault-format:k2-vault/1", "holder:age-x25519", "snapshot:sqlite-vacuum", "vault-format:k2-vault/2"]);
    let ms = serde_json::to_string(&loose).expect("json");
    let sig = old.engine_sign(crate::carry::ENGINE, crate::carry::SIGNING, ms.as_bytes()).expect("sign");
    let newer = Envelope { manifest: ms, sig: crate::crypto::hex(&sig), ..clone_env(&env) };
    let file = agefmt::encrypt(&[key.recipient()], &serde_json::to_vec(&newer).expect("json"));
    let e = open_move_file(&key, &file).err().expect("newer needs");
    assert_eq!(e.code, "move_file_needs_newer");
    assert!(e.hint.contains("vault-format:k2-vault/2"), "{}", e.hint);
    // An unknown outer format.
    let fmt = Envelope { format: "k2-vault-move/9".into(), ..clone_env(&env) };
    let file = agefmt::encrypt(&[key.recipient()], &serde_json::to_vec(&fmt).expect("json"));
    assert_eq!(open_move_file(&key, &file).err().map(|e| e.code), Some("move_file_needs_newer"));
}

fn clone_env(e: &Envelope) -> Envelope {
    Envelope { format: e.format.clone(), manifest: e.manifest.clone(), sig: e.sig.clone(), snapshot_b64: e.snapshot_b64.clone() }
}

#[test]
fn a_manifest_signed_by_another_vault_fails_the_dry_run() {
    let t = TestDir::new("move-otherkey");
    let (old, _) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    let raw = agefmt::decrypt(&key, &exp.file).expect("decrypt");
    let env: Envelope = serde_json::from_slice(raw.expose()).expect("env");
    // A stranger re-signs the same manifest with their own key.
    let stranger = open(&t.path().join("stranger"), &Arc::new(FakeKeyStore::new()));
    let mut m: MoveManifest = serde_json::from_str(&env.manifest).expect("manifest");
    m.source_public_key = stranger.carry_public_key().expect("pk");
    let ms = serde_json::to_string(&m).expect("json");
    let sig = stranger.engine_sign(crate::carry::ENGINE, crate::carry::SIGNING, ms.as_bytes()).expect("sign");
    let forged = Envelope { manifest: ms, sig: crate::crypto::hex(&sig), ..clone_env(&env) };
    let file = agefmt::encrypt(&[key.recipient()], &serde_json::to_vec(&forged).expect("json"));
    let mf = open_move_file(&key, &file).expect("the signature itself verifies");
    let new_dir = t.path().join("new");
    let e = check_move(&mf, &key, &scratch_dir(&new_dir), KdfCost::Floor, &[]).err().expect("wrong signer");
    assert_eq!(e.code, "move_file_bad");
    assert!(e.hint.contains("signed by a key"), "{}", e.hint);
    // And the apply refuses it the same way, leaving nothing behind.
    let ks = Arc::new(FakeKeyStore::new());
    let before = open(&new_dir, &ks);
    let id = before.vault_id().to_string();
    drop(before);
    assert!(apply_move(&new_dir, ks.clone() as Arc<dyn KeyStore>, &key, &mf, opts()).is_err());
    let after = open(&new_dir, &ks);
    assert_eq!(after.vault_id(), id, "auto-undo put this box's own vault back");
    assert!(!after.in_rollback());
}

#[test]
fn a_stale_copy_newer_than_the_export_is_refused() {
    let t = TestDir::new("move-stale");
    let (old, _) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    // The old box was unfrozen and written to after the export, and that
    // vault.db reached the new box (a /home sync that didn't exclude it).
    old.move_unfreeze(&Actor::Owner).expect("unfreeze");
    put_user(&old, "WRITTEN_AFTER");
    let new_dir = t.path().join("new");
    crate::fsutil::ensure_private_dir(&new_dir).expect("mkdir");
    old.checkpoint();
    old.snapshot_to(&new_dir.join("vault.db")).expect("copy the newer vault.db");
    let mf = open_move_file(&key, &exp.file).expect("open");
    let ks = Arc::new(FakeKeyStore::new());
    let e = apply_move(&new_dir, ks as Arc<dyn KeyStore>, &key, &mf, opts()).err().expect("stale");
    assert_eq!(e.code, "move_stale");
    assert!(new_dir.join("vault.db").exists(), "nothing moved");
}

#[test]
fn an_older_same_vault_copy_and_a_closed_vault_are_replaced_and_undo_puts_them_back() {
    let t = TestDir::new("move-undo");
    let (old, _) = old_box(&t);
    // The pre-sync copy: the old vault.db from before the last writes,
    // without its key (the box is stuck on vault_key_missing).
    let new_dir = t.path().join("new");
    crate::fsutil::ensure_private_dir(&new_dir).expect("mkdir");
    old.snapshot_to(&new_dir.join("vault.db")).expect("pre-sync copy");
    put_user(&old, "LATER");
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    let ks = Arc::new(FakeKeyStore::new());
    let e = LocalVault::open(&new_dir, ks.clone() as Arc<dyn KeyStore>, opts()).err().expect("no key here");
    assert_eq!(e.code, "vault_key_missing");
    let mf = open_move_file(&key, &exp.file).expect("open");
    let imported = apply_move(&new_dir, ks.clone() as Arc<dyn KeyStore>, &key, &mf, opts()).expect("import while closed");
    assert_eq!(imported.vault.active_counts(), old.active_counts());
    let aside = new_dir.join(&imported.record.aside);
    assert!(aside.join("vault.db").exists(), "the stale copy is kept aside");
    assert_eq!(last_import(&new_dir).expect("record"), imported.record);
    drop(imported);
    // Undo: the stale copy comes back (closed again, as before), the
    // imported vault is kept in undone-<ts>/.
    let ks_dyn = ks.clone() as Arc<dyn KeyStore>;
    let rec = undo_import(&new_dir, &ks_dyn).expect("undo");
    assert!(!aside.exists());
    assert!(last_import(&new_dir).is_none());
    let undone: Vec<_> = std::fs::read_dir(&new_dir)
        .expect("ls")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("undone-"))
        .collect();
    assert_eq!(undone.len(), 1);
    assert!(undone[0].path().join("vault.db").exists());
    let e = LocalVault::open(&new_dir, ks.clone() as Arc<dyn KeyStore>, opts()).err().expect("closed as before");
    assert_eq!(e.code, "vault_substituted", "this box's key is the import's now; the copy wasn't made with it");
    assert_eq!(rec.vault_id, old.vault_id());
    assert_eq!(undo_import(&new_dir, &ks_dyn).err().map(|e| e.code), Some("no_import"));
}

#[test]
fn undo_restores_an_open_vault_and_its_anchor() {
    let t = TestDir::new("move-undo-open");
    let (old, _) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    let new_dir = t.path().join("new");
    let ks = Arc::new(FakeKeyStore::new());
    let mine = open(&new_dir, &ks);
    put_system(&mine, "frpc", "tunnel.token", "new-box-own-token");
    let my_id = mine.vault_id().to_string();
    drop(mine);
    let mf = open_move_file(&key, &exp.file).expect("open");
    let imported = apply_move(&new_dir, ks.clone() as Arc<dyn KeyStore>, &key, &mf, opts()).expect("apply");
    drop(imported);
    undo_import(&new_dir, &(ks.clone() as Arc<dyn KeyStore>)).expect("undo");
    let back = open(&new_dir, &ks);
    assert_eq!(back.vault_id(), my_id);
    assert!(!back.in_rollback(), "the anchor came back too");
    assert_eq!(value(&back, "frpc", "tunnel.token"), "new-box-own-token");
}

/// The daemon's served vault holds vault.db open while an import moves it
/// aside. release_files closes it first (the daemon does that on Windows,
/// where an open file can't be renamed); the files then move on every OS,
/// and the released vault no longer reads the old file.
#[test]
fn a_served_vault_released_first_is_replaced_by_the_import() {
    let t = TestDir::new("move-served");
    let (old, _) = old_box(&t);
    let key = Identity::generate();
    let exp = old.move_export(&key.recipient(), src(), &Actor::Owner, &ok()).expect("export");
    let new_dir = t.path().join("new");
    let ks = Arc::new(FakeKeyStore::new());
    let served = open(&new_dir, &ks);
    put_system(&served, "frpc", "tunnel.token", "new-box-own-token");
    served.release_files().expect("release");
    assert_eq!(served.user_secret_count(), 0, "a released vault reads nothing");
    let mf = open_move_file(&key, &exp.file).expect("open");
    let imported = apply_move(&new_dir, ks.clone() as Arc<dyn KeyStore>, &key, &mf, opts()).expect("import over a released vault");
    assert_eq!(imported.vault.active_counts(), old.active_counts());
    assert_eq!(value(&imported.vault, "frpc", "tunnel.token"), "k2tok_moved_with_the_vault");
    assert!(new_dir.join(&imported.record.aside).join("vault.db").exists(), "the served file was moved aside");
    drop(imported);
    undo_import(&new_dir, &(ks.clone() as Arc<dyn KeyStore>)).expect("undo");
    assert_eq!(value(&open(&new_dir, &ks), "frpc", "tunnel.token"), "new-box-own-token");
}

#[test]
fn a_second_export_while_frozen_replaces_the_move_key() {
    let t = TestDir::new("move-reexport");
    let (old, _) = old_box(&t);
    let k1 = Identity::generate();
    let k2 = Identity::generate();
    old.move_export(&k1.recipient(), src(), &Actor::Owner, &ok()).expect("first");
    let exp2 = old.move_export(&k2.recipient(), src(), &Actor::Owner, &ok()).expect("again while frozen");
    assert!(open_move_file(&k1, &exp2.file).is_err());
    let mf = open_move_file(&k2, &exp2.file).expect("the new key opens it");
    let new_dir = t.path().join("new");
    let ks = Arc::new(FakeKeyStore::new());
    apply_move(&new_dir, ks as Arc<dyn KeyStore>, &k2, &mf, opts()).expect("apply");
}
