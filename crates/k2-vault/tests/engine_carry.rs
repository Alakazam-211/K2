//! V13 engine credentials and V10 carries/backups (fakes only).

use std::sync::Arc;

use k2_vault::agefmt::Identity;
use k2_vault::carry::CarryItem;
use k2_vault::confirm::KdfCost;
use k2_vault::engine::Gen;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::*;

fn open(t: &TestDir, sub: &str) -> (LocalVault, Arc<FakeKeyStore>) {
    let ks = Arc::new(FakeKeyStore::new());
    let v = LocalVault::open(t.path().join(sub), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open");
    (v, ks)
}

#[test]
fn engine_credentials_mint_once_and_rotate_only_when_accepted() {
    let t = TestDir::new("engine");
    let (v, _) = open(&t, "v");
    let a = v.engine_ensure("sew", "db-password", Gen::Random32).expect("ensure");
    let b = v.engine_ensure("sew", "db-password", Gen::Random32).expect("again");
    assert_eq!(a.latest_version, b.latest_version, "ensure doesn't rotate");
    let first = v.engine_value("sew", "db-password").expect("value");
    assert_eq!(first.len(), 64);
    let e = v.engine_rotate("sew", "db-password", Gen::Random32, |_| Err("refused".into())).expect_err("refused");
    assert_eq!(e.code, "engine_rejected");
    assert_eq!(v.engine_value("sew", "db-password").expect("value").expose(), first.expose(), "old value kept");
    let mut seen = Vec::new();
    v.engine_rotate("sew", "db-password", Gen::Random32, |new| {
        seen = new.expose().to_vec();
        Ok(())
    })
    .expect("rotate");
    assert_eq!(v.engine_value("sew", "db-password").expect("value").expose(), seen.as_slice());
    // Asymmetric engine token.
    let pk = v.engine_public_key("sew", "signing").expect("pk");
    assert_eq!(pk, v.engine_public_key("sew", "signing").expect("pk"), "stable");
    let sig = v.engine_sign("sew", "signing", b"hello").expect("sign");
    assert!(k2_vault::engine::verify(&pk, b"hello", &sig));
    assert!(!k2_vault::engine::verify(&pk, b"hellO", &sig));
    // Engine secrets are system: no person or agent reads them.
    let m = v.meta(&Selector::system(Scope::engine("sew"), "db-password"), &Actor::Owner).expect("owner sees metadata");
    assert_eq!(m.namespace, Namespace::System);
    let proof = HumanProof::Confirmed { method: "t".into() };
    assert!(v.reveal(&Selector::system(Scope::engine("sew"), "db-password"), &Actor::Owner, &proof).is_err());
}

fn item(name: &str, value: &str) -> CarryItem {
    CarryItem {
        scope_kind: "workspace".into(),
        scope_label: "sales".into(),
        scope_id: None,
        name: name.into(),
        kind: "api_key".into(),
        hosts: vec!["https://api.stripe.com:443".into()],
        inject: None,
        default_delivery: "fetch".into(),
        delivery_ceiling: "env".into(),
        description: String::new(),
        human_given: true,
        value_b64: CarryItem::encode_value(value.as_bytes()),
        source_scope: None,
        grants: vec![],
    }
}

fn binding(clone_id: &str, ttl_secs: i64) -> k2_vault::carry::CloneBinding {
    k2_vault::carry::CloneBinding {
        clone_id: clone_id.into(),
        source_workspace_id: "ws-src".into(),
        source_label: "sales".into(),
        ttl_secs,
    }
}

/// CL26: a clone carry (`/2`) opens only for its clone, only within its
/// hour, and the generic opener refuses it once expired.
#[test]
fn clone_carries_are_bound_to_one_clone_and_expire() {
    let t = TestDir::new("carry-v2");
    let (src, _) = open(&t, "src");
    let (dst, _) = open(&t, "dst");
    let r = dst.carry_recipient().expect("recipient");
    let blank = vec![k2_vault::carry::CarryBlank {
        name: "OPENAI_KEY".into(),
        kind: "api_key".into(),
        hosts: vec![],
        delivery: "fetch".into(),
        env_names: vec!["OPENAI_API_KEY".into()],
        source_scope: "server".into(),
    }];
    let file = src
        .carry_seal_clone(&r, dst.vault_id(), false, binding("clone-1", k2_vault::carry::CLONE_CARRY_TTL_SECS), vec![item("STRIPE_KEY", "k2test_V2_1")], blank)
        .expect("seal");
    let p = dst.carry_open_clone(&file, "clone-1").expect("open for its clone");
    assert_eq!(p.format, k2_vault::carry::FORMAT_V2);
    assert_eq!(p.clone_id.as_deref(), Some("clone-1"));
    assert_eq!(p.blank[0].name, "OPENAI_KEY");
    assert_eq!(p.source_label.as_deref(), Some("sales"));
    assert_eq!(dst.carry_open_clone(&file, "clone-2").expect_err("other clone").code, "carry_clone_mismatch");
    assert_eq!(dst.carry_open_clone(&file, "").expect_err("no clone id").code, "carry_clone_mismatch");
    // A /1 carry is not a clone carry.
    let v1 = src.carry_seal(&r, dst.vault_id(), false, vec![item("X", "y")]).expect("seal v1");
    assert_eq!(dst.carry_open_clone(&v1, "clone-1").expect_err("v1").code, "carry_not_for_clone");
    // Expired: refused by every opener.
    let old = src.carry_seal_clone(&r, dst.vault_id(), false, binding("clone-1", -1), vec![item("STRIPE_KEY", "v")], vec![]).expect("seal");
    assert_eq!(dst.carry_open(&old).expect_err("expired").code, "carry_expired");
    assert_eq!(dst.carry_open_clone(&old, "clone-1").expect_err("expired").code, "carry_expired");
    // No clone id: never sealed.
    assert_eq!(src.carry_seal_clone(&r, dst.vault_id(), false, binding(" ", 60), vec![], vec![]).expect_err("id").code, "clone_id_required");
}

/// CL13: K2 logins, App passes, engine credentials and chat secrets are
/// never sealed, by either carry.
#[test]
fn never_carried_kinds_are_refused_at_seal() {
    let t = TestDir::new("carry-deny");
    let (src, _) = open(&t, "src");
    let (dst, _) = open(&t, "dst");
    let r = dst.carry_recipient().expect("recipient");
    for kind in ["login_slot", "app_pass", "engine_cred"] {
        let mut i = item("LOGIN", "k2test_DENY");
        i.kind = kind.into();
        assert_eq!(src.carry_seal(&r, dst.vault_id(), false, vec![i.clone()]).expect_err(kind).code, "never_carried");
        assert_eq!(
            src.carry_seal_clone(&r, dst.vault_id(), false, binding("c", 60), vec![i], vec![]).expect_err(kind).code,
            "never_carried"
        );
    }
    let mut s = item("CHAT_ONLY", "v");
    s.scope_kind = "session".into();
    assert_eq!(src.carry_seal(&r, dst.vault_id(), false, vec![s]).expect_err("session").code, "never_carried");
    assert!(k2_vault::carry::never_carried_kind("login_slot"));
    assert!(!k2_vault::carry::never_carried_kind("api_key"));
}

#[test]
fn carries_are_sealed_to_one_target_and_signed() {
    let t = TestDir::new("carry");
    let (src, _) = open(&t, "src");
    let (dst, _) = open(&t, "dst");
    let (other, _) = open(&t, "other");
    let r = dst.carry_recipient().expect("recipient");
    assert_eq!(r, dst.carry_recipient().expect("stable"));
    let file = src.carry_seal(&r, dst.vault_id(), true, vec![item("STRIPE_KEY", "k2test_CARRY_1"), item("OTHER", "v2")]).expect("seal");
    let p = dst.carry_open(&file).expect("open");
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.items[0].value().expect("value").expose(), b"k2test_CARRY_1");
    assert_eq!(p.source_vault_id, src.vault_id());
    assert!(p.own_box_move);
    // Another vault can't open it.
    assert!(other.carry_open(&file).is_err());
    // Sealed to dst's key but bound to another vault id: refused.
    let wrong = src.carry_seal(&r, other.vault_id(), false, vec![item("X", "y")]).expect("seal");
    assert_eq!(dst.carry_open(&wrong).expect_err("bound elsewhere").code, "carry_not_for_this_vault");
}

#[test]
fn backup_snapshot_restores_with_the_backup_identity() {
    let t = TestDir::new("backup");
    let (v, _) = open(&t, "v");
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "K", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    v.put(r, SecretBytes::from_slice(b"backed-up-value"), &Actor::Owner).expect("put");
    let backup_id = Identity::generate();
    v.add_wrap("backup", &backup_id.recipient(), &Actor::Owner).expect("wrap");
    let snap_dir = t.path().join("restore-box");
    std::fs::create_dir_all(&snap_dir).expect("dir");
    v.snapshot_to(&snap_dir.join("vault.db")).expect("snapshot");
    assert!(v.snapshot_to(&snap_dir.join("vault.db")).is_err(), "never overwrites");
    let new_ks = Arc::new(FakeKeyStore::new());
    let restored = LocalVault::adopt_with_identity(&snap_dir, new_ks.clone() as Arc<dyn KeyStore>, &backup_id, "backup", OpenOptions { kdf: KdfCost::Floor })
        .expect("restore");
    let got = restored
        .resolve(&Selector::user(Scope::server(), "K"), Use::System, &Requester::System("t".into()))
        .expect("value");
    assert_eq!(got.value.expose(), b"backed-up-value");
    assert_eq!(restored.status().tamper, "ok");
    assert_eq!(new_ks.anchor().expect("a").expect("anchor").vault_id.as_deref(), Some(v.vault_id()));
}
