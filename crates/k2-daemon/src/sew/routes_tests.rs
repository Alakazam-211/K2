//! `/cli/sew/*` gates (K12 + K19): owner / admin / Manage-Sew agent pass,
//! an agent without the switch and every non-admin credential get 403, and
//! the person-only steps need the owner plus a vault confirm bound to this
//! install. Machines run inline over a fake box.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::sew::ops::fake::{FakeOps, INSTALL_ID};
use crate::sew::secrets::MemSecrets;

const TARBALL: &str = "/home/k2/sew.tar.gz";

struct T {
    env: Env<'static>,
    ops: &'static FakeOps,
    _db: k2_core::db::TestDbGuard,
}

fn t() -> T {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps::new()));
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew/sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz");
    ops.files.lock().unwrap().insert(TARBALL.into(), std::fs::read(p).unwrap());
    let env = Env {
        ops,
        secrets: Box::leak(Box::new(MemSecrets::default())),
        latch: Box::leak(Box::new(AtomicBool::new(false))),
        token: Box::leak(Box::new(Mutex::new(None))),
    };
    let db = k2_core::db::scoped_for_test();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, on) in [("w-it", 1), ("w-plain", 0)] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle, agents_can_manage_chat) VALUES (?1, ?1, ?2, ?1, ?3)",
                rusqlite::params![id, format!("/t/{id}"), on],
            )
            .unwrap();
        }
    }
    T { env, ops, _db: db }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: "s1".into() }
}
fn shell(ws: &str) -> Who {
    Who::Shell { project_id: ws.into(), session_id: "s2".into() }
}
fn login(role: Role) -> Who {
    Who::Login { username: "ana".into(), role }
}

impl T {
    fn call(&self, who: &Who, path: &str, body: Value) -> (String, Value) {
        let r = handle_env(self.env, path, who, body.to_string().as_bytes(), &HashMap::new(), false);
        (r.status.to_string(), serde_json::from_str(&r.body).unwrap())
    }
    fn get(&self, who: &Who, path: &str) -> (String, Value) {
        let r = handle_env(self.env, path, who, b"", &HashMap::new(), false);
        (r.status.to_string(), serde_json::from_str(&r.body).unwrap())
    }
    fn enable(&self) {
        let (st, v) = self.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": TARBALL}));
        assert_eq!(st, "200 OK", "{v}");
        assert_eq!(v["state"], "running");
        self.ops.clear();
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[test]
fn reads_need_any_real_credential() {
    let t = t();
    for who in [Who::Owner, login(Role::Member), agent("w-plain"), shell("w-plain")] {
        let (st, v) = t.get(&who, "/cli/sew/status");
        assert_eq!(st, "200 OK", "{who:?}");
        assert_eq!(v["state"], "not-installed");
        assert_eq!(t.get(&who, "/cli/sew/preflight").0, "200 OK");
    }
    for who in [Who::Wrong, Who::Invalid] {
        let (st, v) = t.get(&who, "/cli/sew/status");
        assert_eq!(st, "403 Forbidden");
        assert!(matches!(code(&v), "wrong_credential" | "forbidden"));
    }
}

#[test]
fn manage_sew_gate_owner_admin_or_the_switch() {
    // Refused callers change nothing.
    for (who, want) in [
        (login(Role::Member), "owner_only"),
        (agent("w-plain"), "chat_manage_off"),
        (shell("w-plain"), "chat_manage_off"),
        (Who::Wrong, "wrong_credential"),
        (Who::Invalid, "forbidden"),
    ] {
        let t = t();
        for path in [
            "/cli/sew/server/enable",
            "/cli/sew/server/disable",
            "/cli/sew/server/uninstall",
            "/cli/sew/server/upgrade",
            "/cli/sew/server/rollback",
            "/cli/sew/server/backup",
            "/cli/sew/server/doctor",
            "/cli/sew/server/rotate-master",
        ] {
            let (st, v) = t.call(&who, path, json!({"tarballPath": TARBALL}));
            assert_eq!(st, "403 Forbidden", "{who:?} {path}");
            assert_eq!(code(&v), want, "{who:?} {path}");
        }
        let (st, _) = t.call(&who, "/cli/sew/server/restore", json!({"bundleId": "20260101T031500Z", "confirm": true}));
        assert_eq!(st, "403 Forbidden");
        assert!(t.ops.effects().is_empty(), "{who:?}: {:?}", t.ops.effects());
        assert!(crate::sew::store::load().is_none());
    }
    // Allowed: owner, admin and owner logins, an agent (or shell tab) whose
    // workspace has Manage Sew on.
    for who in [Who::Owner, login(Role::Admin), login(Role::Owner), agent("w-it"), shell("w-it")] {
        let t = t();
        let (st, v) = t.call(&who, "/cli/sew/server/enable", json!({"tarballPath": TARBALL}));
        assert_eq!(st, "200 OK", "{who:?}: {v}");
        let (st, v) = t.call(&who, "/cli/sew/server/disable", json!({}));
        assert_eq!(st, "200 OK", "{who:?}: {v}");
        assert_eq!(v["state"], "disabled");
    }
}

#[test]
fn the_switch_is_off_by_default_and_never_an_agent_verb() {
    let t = t();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-new', 'n', '/t/n', 'n')", []).unwrap();
    }
    assert!(!k2_core::workspace::settings::agents_can_manage_chat("w-new"));
    assert_eq!(code(&t.call(&agent("w-new"), "/cli/sew/server/enable", json!({})).1), "chat_manage_off");
    assert!(!crate::session_token::is_agent_verb("/cli/agent-access/set"), "agents never loosen their own access");
}

#[test]
fn person_only_steps_refuse_every_passport_and_need_the_owner_and_a_confirm() {
    let t = t();
    t.enable();
    let vault = crate::vault::ensure_test_vault();
    let purge = json!({"purge": true, "confirmHostname": "lztek"});
    for who in [agent("w-it"), shell("w-it")] {
        for (path, body) in [
            ("/cli/sew/server/uninstall", purge.clone()),
            ("/cli/sew/server/recovery-key", json!({})),
            ("/cli/sew/server/rotate-master", json!({"phase": "drop-prev"})),
        ] {
            let (st, v) = t.call(&who, path, body);
            assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "person_only"), "{who:?} {path}");
        }
    }
    // An admin login reaches the confirm check (K2's lean); a member never does.
    assert_eq!(code(&t.call(&login(Role::Admin), "/cli/sew/server/recovery-key", json!({})).1), "owner_confirm_required");
    assert_eq!(code(&t.call(&login(Role::Member), "/cli/sew/server/recovery-key", json!({})).1), "owner_only");
    // The owner without a confirm, with one for another action or install,
    // and with a used one.
    assert_eq!(code(&t.call(&Who::Owner, "/cli/sew/server/recovery-key", json!({})).1), "owner_confirm_required");
    let wrong_action = vault.confirm.mint(CONFIRM_PURGE, Some(INSTALL_ID), "owner", "vault-password");
    assert_eq!(code(&t.call(&Who::Owner, "/cli/sew/server/recovery-key", json!({"confirmToken": wrong_action})).1), "owner_confirm_required");
    let wrong_install = vault.confirm.mint(CONFIRM_RECOVERY, Some("123"), "owner", "vault-password");
    assert_eq!(code(&t.call(&Who::Owner, "/cli/sew/server/recovery-key", json!({"confirmToken": wrong_install})).1), "owner_confirm_required");
    let good = vault.confirm.mint(CONFIRM_RECOVERY, Some(INSTALL_ID), "owner", "vault-password");
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/recovery-key", json!({"confirmToken": good}));
    assert_eq!(st, "200 OK", "{v}");
    assert!(k2_root_proto::sew::master_key_text_ok(v["recoveryKey"].as_str().unwrap()));
    let (_, again) = t.call(&Who::Owner, "/cli/sew/server/recovery-key", json!({"confirmToken": good}));
    assert_eq!(code(&again), "owner_confirm_required", "a confirm works once");
    // An Owner login's confirm is stamped with its own name.
    let ana = vault.confirm.mint(CONFIRM_RECOVERY, Some(INSTALL_ID), "connect:ana", "connect-password");
    assert_eq!(t.call(&login(Role::Owner), "/cli/sew/server/recovery-key", json!({"confirmToken": ana})).0, "200 OK");
    assert!(t.ops.effects().is_empty(), "no root call for a reveal");
}

#[test]
fn purge_needs_the_confirm_and_the_hostname_and_then_removes_everything() {
    let t = t();
    t.enable();
    let vault = crate::vault::ensure_test_vault();
    let tok = vault.confirm.mint(CONFIRM_PURGE, Some(INSTALL_ID), "owner", "vault-password");
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/uninstall", json!({"purge": true, "confirmToken": tok, "confirmHostname": "other"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "confirm_hostname_mismatch"));
    assert!(t.ops.effects().is_empty());
    let tok = vault.confirm.mint(CONFIRM_PURGE, Some(INSTALL_ID), "owner", "vault-password");
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/uninstall", json!({"purge": true, "confirmToken": tok, "confirmHostname": "lztek"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["purged"], true);
    assert_eq!(v["confirmedBy"], "vault-password");
    assert!(crate::sew::store::load().is_none());
}

/// Rosson's open question (2026-10-15), K2's lean: purge stays the
/// owner's; an Admin login with its own confirm may reveal the recovery key
/// and drop the previous master key. The admin's confirm is stamped with
/// its own name: the owner's confirm is not theirs to spend.
#[test]
fn admin_with_a_confirm_may_reveal_the_key_and_drop_prev_but_never_purge() {
    let t = t();
    t.enable();
    let vault = crate::vault::ensure_test_vault();
    let admin = login(Role::Admin);
    // Purge: owner only, even with an admin's own confirm. Nothing ran.
    let tok = vault.confirm.mint(CONFIRM_PURGE, Some(INSTALL_ID), "connect:ana", "connect-password");
    let (st, v) = t.call(&admin, "/cli/sew/server/uninstall", json!({"purge": true, "confirmToken": tok, "confirmHostname": "lztek"}));
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "owner_only"), "{v}");
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    assert!(crate::sew::store::load().is_some(), "purge refused: the install stays");
    // The owner's confirm is not the admin's.
    let owners = vault.confirm.mint(CONFIRM_RECOVERY, Some(INSTALL_ID), "owner", "vault-password");
    assert_eq!(code(&t.call(&admin, "/cli/sew/server/recovery-key", json!({"confirmToken": owners})).1), "owner_confirm_required");
    // The admin's own confirm reveals the key.
    let mine = vault.confirm.mint(CONFIRM_RECOVERY, Some(INSTALL_ID), "connect:ana", "connect-password");
    let (st, v) = t.call(&admin, "/cli/sew/server/recovery-key", json!({"confirmToken": mine}));
    assert_eq!(st, "200 OK", "{v}");
    assert!(k2_root_proto::sew::master_key_text_ok(v["recoveryKey"].as_str().expect("recoveryKey")));
    // drop-prev after a rotation, with the admin's own confirm.
    let (st, v) = t.call(&admin, "/cli/sew/server/rotate-master", json!({"phase": "start"}));
    assert_eq!(st, "200 OK", "{v}");
    let mine = vault.confirm.mint(CONFIRM_DROP_PREV, Some(INSTALL_ID), "connect:ana", "connect-password");
    let (st, v) = t.call(&admin, "/cli/sew/server/rotate-master", json!({"phase": "drop-prev", "confirmToken": mine}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["dropped"], true);
    assert_eq!(v["confirmedBy"], "connect-password");
    // A member login never reaches a confirm for any of the three.
    for (path, body) in [
        ("/cli/sew/server/recovery-key", json!({})),
        ("/cli/sew/server/rotate-master", json!({"phase": "drop-prev"})),
    ] {
        assert_eq!(code(&t.call(&login(Role::Member), path, body).1), "owner_only", "{path}");
    }
    // The route floors match: Admin for the reveal, never Member.
    let floor = crate::routes::route_policy::lookup("/cli/sew/server/recovery-key").unwrap().post.unwrap();
    assert!(floor.admits(Role::Admin) && !floor.admits(Role::Member));
}

#[test]
fn drop_prev_is_person_only_but_rotate_start_is_manage_sew() {
    let t = t();
    t.enable();
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/server/rotate-master", json!({"phase": "start"}));
    assert_eq!(st, "200 OK", "{v}");
    let tok = crate::vault::ensure_test_vault().confirm.mint(CONFIRM_DROP_PREV, Some(INSTALL_ID), "owner", "vault-password");
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/rotate-master", json!({"phase": "drop-prev", "confirmToken": tok}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["dropped"], true);
}

#[test]
fn restore_needs_confirm_true() {
    let t = t();
    t.enable();
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/restore", json!({"bundleId": "20260101T031500Z"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "confirm_required"));
    assert!(t.ops.effects().is_empty());
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/server/restore", json!({"bundleId": "20260101T031500Z", "confirm": true}));
    assert_eq!(st, "200 OK", "{v}");
}

#[test]
fn bodies_and_paths_are_checked() {
    let t = t();
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": "relative/sew.tar.gz"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"));
    let r = handle_env(t.env, "/cli/sew/server/enable", &Who::Owner, b"[1]", &HashMap::new(), false);
    assert_eq!(r.status, "400 Bad Request");
    assert_eq!(code(&t.call(&Who::Owner, "/cli/sew/server/rotate-master", json!({"phase": "sideways"})).1), "bad_request");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/job").0, "400 Bad Request");
}

#[test]
fn every_route_is_classified_post_only_where_it_mutates_and_on_the_agent_list() {
    use crate::routes::route_policy::{lookup, post_allowed};
    for p in ROUTES {
        let row = lookup(p).unwrap_or_else(|| panic!("{p} is not in route_policy::ROUTES"));
        if is_post_route(p) {
            assert!(row.get.is_none() && row.post.is_some(), "{p}: POST-only");
            assert!(post_allowed(p));
        } else {
            assert!(row.get.is_some() && row.post.is_none(), "{p}: a read");
        }
    }
    for p in POST_ROUTES {
        assert!(ROUTES.contains(p));
        let floor = lookup(p).unwrap().post.unwrap();
        assert!(!floor.admits(Role::Member), "{p}: no Member login");
        let agent_ok = crate::session_token::is_agent_verb(p);
        assert_eq!(agent_ok, *p != "/cli/sew/server/recovery-key", "{p}");
    }
    for p in ["/cli/sew/status", "/cli/sew/preflight", "/cli/sew/backups", "/cli/sew/job"] {
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
}

// ── a release build without a Sew pin ───────────────────────────────────

const NOT_RELEASED: &str = "Sew isn't available in this K2 release yet. It will be turned on in a later K2 update.";

/// A release build while K2's Sew pins are placeholders: enable, preflight,
/// upgrade, voice preflight and voice install refuse `sew_not_released`
/// before any effect (no helper call, no row, no voice record), even for
/// the owner with a tarball in hand; status says so plainly.
#[test]
fn unreleased_pin_refuses_every_install_path_before_any_effect() {
    let t = t();
    with_media(&t);
    *t.ops.released.lock().unwrap() = false;
    for (path, body) in [
        ("/cli/sew/server/enable", json!({"tarballPath": TARBALL})),
        ("/cli/sew/server/enable", json!({"tarballPath": TARBALL, "voice": {"mediaTarballPath": MEDIA}})),
        ("/cli/sew/server/enable", json!({"identity": "standalone"})),
        ("/cli/sew/server/upgrade", json!({"tarballPath": TARBALL})),
        ("/cli/sew/server/upgrade", json!({"tarballPath": TARBALL, "dryRun": true})),
        ("/cli/sew/media/install", json!({"mediaTarballPath": MEDIA})),
    ] {
        let (st, v) = t.call(&Who::Owner, path, body.clone());
        assert_eq!(st, "409 Conflict", "{path} {body}: {v}");
        assert_eq!(code(&v), "sew_not_released", "{path} {body}: {v}");
        assert_eq!(v["error"]["hint"], NOT_RELEASED, "{path}");
    }
    for path in ["/cli/sew/preflight", "/cli/sew/media/preflight"] {
        let (st, v) = t.get(&Who::Owner, path);
        assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "sew_not_released"), "{path}: {v}");
        assert_eq!(v["error"]["hint"], NOT_RELEASED, "{path}");
    }
    // The supervisor entry points refuse on their own too.
    assert_eq!(sup::enable_begin(&t.env, &sup::EnableRequest::default()).err().unwrap().code, "sew_not_released");
    assert_eq!(sup::upgrade_begin(&t.env, &sup::UpgradeRequest::default()).err().unwrap().code, "sew_not_released");
    assert_eq!(
        crate::sew::voice::install_begin(&t.env, &crate::sew::voice::InstallRequest::default()).err().unwrap().code,
        "sew_not_released"
    );
    assert!(t.ops.argvs().is_empty(), "no helper call at all: {:?}", t.ops.argvs());
    assert!(crate::sew::store::load().is_none(), "no Sew row");
    assert!(t.ops.voice.lock().unwrap().is_none(), "no voice record");
    assert!(!t.env.latch.load(std::sync::atomic::Ordering::SeqCst), "the latch is free");
    // Status: plain words, not an error.
    let (st, v) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["released"], false);
    assert_eq!(v["notice"], NOT_RELEASED);
    assert_eq!(v["state"], "not-installed");
}

/// A box that already runs Sew (installed by a pinned build) and then
/// takes a release build without a pin: upgrade refuses and changes
/// nothing; status still reports the install, with the notice.
#[test]
fn unreleased_pin_refuses_upgrade_on_a_running_install() {
    let t = t();
    t.enable();
    let before = crate::sew::store::load().unwrap();
    let (_, was) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(was["released"], true);
    *t.ops.released.lock().unwrap() = false;
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/upgrade", json!({"tarballPath": TARBALL}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "sew_not_released"), "{v}");
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    let after = crate::sew::store::load().unwrap();
    assert_eq!((after.status, after.installed_version), (before.status, before.installed_version));
    let (st, v) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(st, "200 OK");
    assert_eq!(v["state"], was["state"], "status still reports the install");
    assert_eq!(v["installedVersion"], was["installedVersion"]);
    assert_eq!(v["released"], false);
    assert_eq!(v["notice"], NOT_RELEASED);
}

/// With a pin (the test-signer fixtures here; `sew-test-pins` likewise)
/// nothing changes: status says released, no notice, and enable runs.
#[test]
fn pinned_build_is_unchanged() {
    let t = t();
    let (st, v) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(st, "200 OK");
    assert_eq!(v["released"], true);
    assert!(v.get("notice").is_none(), "{v}");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/preflight").0, "200 OK");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/media/preflight").0, "200 OK");
    t.enable();
    assert_eq!(crate::sew::store::load().unwrap().status, "running");
}

// ── voice + users + standalone (K13/K14, 0.46.0 demo) ───────────────────

const MEDIA: &str = "/home/k2/sew-media.tar.gz";

fn with_media(t: &T) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew/sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz");
    t.ops.files.lock().unwrap().insert(MEDIA.into(), std::fs::read(p).unwrap());
}

#[test]
fn voice_and_users_routes_sit_behind_manage_sew() {
    let t = t();
    for (path, body) in [
        ("/cli/sew/media/install", json!({})),
        ("/cli/sew/media/url", json!({"publicUrl": "wss://hearth.lztek.k2.dev"})),
        ("/cli/sew/media/renew", json!({})),
        ("/cli/sew/media/probe", json!({"udp": "ok", "tcp": "ok", "turnTls": "ok"})),
        ("/cli/sew/media/uninstall", json!({})),
        ("/cli/sew/server/config", json!({"publicUrl": "https://hearth.lztek.k2.dev"})),
        ("/cli/sew/meet/bind", json!({"app": "meetdemo"})),
        ("/cli/sew/meet/unbind", json!({})),
        ("/cli/sew/users/invite", json!({"user": "bob"})),
        ("/cli/sew/users/owner", json!({"user": "rosson"})),
        ("/cli/sew/users/reset", json!({"user": "bob"})),
        ("/cli/sew/users/revoke", json!({"user": "bob"})),
        ("/cli/sew/users/reactivate", json!({"user": "bob"})),
        ("/cli/sew/users/role", json!({"user": "bob", "role": "admin"})),
    ] {
        let (st, v) = t.call(&agent("w-plain"), path, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "chat_manage_off"), "{path}");
        let (st, v) = t.call(&shell("w-plain"), path, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "chat_manage_off"), "{path}");
        let (st, v) = t.call(&login(Role::Member), path, body);
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "owner_only"), "{path}");
    }
    let (st, v) = t.get(&agent("w-plain"), "/cli/sew/users");
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "chat_manage_off"));
    // The preflight is a read.
    assert_eq!(t.get(&agent("w-plain"), "/cli/sew/media/preflight").0, "200 OK");
    // A Manage Sew agent gets through the gate to the checks.
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/media/url", json!({"publicUrl": "wss://evil.example"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "url_not_on_this_box"), "{v}");
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/media/probe", json!({"udp": "yes", "tcp": "ok", "turnTls": "ok"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"), "{v}");
    assert!(t.ops.effects().is_empty());
}

/// K-M2: the reserved `meet` label binds box-level to an installed App's
/// front port (never through the publish routes), one App at a time, and
/// unbinds; status shows it.
#[test]
fn the_meet_label_binds_to_an_apps_front_port_and_unbinds() {
    let t = t();
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/meet/bind", json!({"app": "meetdemo"}));
    assert_eq!((st.as_str(), code(&v)), ("404 Not Found", "app_not_found"), "{v}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (app, port) in [("meetdemo", 38701u16), ("other", 38702)] {
            let svc = k2_core::published_services::insert(&conn, "w-it", app, "(app)", "/x", 0, "local", "stopped", "cmd", "").unwrap();
            k2_core::deploy::store::insert_app(
                &conn,
                &k2_core::deploy::store::NewApp {
                    app_id: app,
                    service_id: &svc.id,
                    seed_id: None,
                    owner_workspace: "w-it",
                    isolation: "process",
                    memory_limit_bytes: None,
                    front_port: port,
                    created_by: "test",
                },
            )
            .unwrap();
        }
    }
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/meet/bind", json!({"app": "MeetDemo"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["url"], "https://meet.lztek.k2.dev");
    assert_eq!(v["frontPort"], 38701);
    assert!(v["hint"].as_str().unwrap().contains("k2 app restart meetdemo"), "{v}");
    assert_eq!(t.ops.nested.lock().unwrap().last().cloned(), Some(("meet".to_string(), Some("127.0.0.1:38701".to_string()))));
    let s = t.get(&Who::Owner, "/cli/sew/status").1;
    assert_eq!(s["meet"]["app"], "meetdemo", "{s}");
    // One App at a time.
    let (st, v) = t.call(&Who::Owner, "/cli/sew/meet/bind", json!({"app": "other"}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "meet_bound"), "{v}");
    // Unbind deletes the label and the record.
    let (st, v) = t.call(&Who::Owner, "/cli/sew/meet/unbind", json!({}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["unbound"], "meetdemo");
    assert_eq!(t.ops.nested.lock().unwrap().last().cloned(), Some(("meet".to_string(), None)));
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["meet"], Value::Null);
    let (st, v) = t.call(&Who::Owner, "/cli/sew/meet/bind", json!({"app": "other"}));
    assert_eq!(st, "200 OK", "{v}");
    // No K2 Connect label: refused before any label call.
    t.call(&Who::Owner, "/cli/sew/meet/unbind", json!({}));
    *t.ops.label.lock().unwrap() = None;
    let n = t.ops.nested.lock().unwrap().len();
    let (st, v) = t.call(&Who::Owner, "/cli/sew/meet/bind", json!({"app": "other"}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "no_label"), "{v}");
    assert_eq!(t.ops.nested.lock().unwrap().len(), n);
}

/// 0.46.3 (ops on a real box): Sew asks "Uninstall voice? Live calls end."
/// and refused with no terminal. The route now needs `confirm: true` (the
/// client asked the person) and runs nothing without it.
#[test]
fn media_uninstall_needs_an_explicit_confirm_and_runs_nothing_without_it() {
    let t = t();
    with_media(&t);
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": TARBALL, "voice": {"mediaTarballPath": MEDIA}}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["voice"]["state"], "running");
    t.ops.clear();
    for body in [json!({}), json!({"confirm": false}), json!({"confirm": "yes"}), json!({"yes": true})] {
        let (st, v) = t.call(&Who::Owner, "/cli/sew/media/uninstall", body.clone());
        assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "confirm_required"), "{body}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("--yes"), "the hint names the CLI flag: {v}");
    }
    assert_eq!(t.ops.effects(), Vec::<String>::new(), "no confirm, no helper call");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["voice"]["state"], "running", "voice untouched");
    t.ops.clear(); // the status read asked `media status`
    // A Manage Sew agent with the confirm gets through.
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/media/uninstall", json!({"confirm": true}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["state"], "not-installed");
    assert_eq!(v["engine"], "removed");
    assert_eq!(t.ops.effects(), vec!["engine sew media uninstall"]);
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["voice"]["state"], "not-installed");
}

#[test]
fn enable_takes_an_explicit_standalone_identity_and_voice_in_one_call() {
    let t = t();
    with_media(&t);
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": TARBALL, "identity": "ldap"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"));
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/enable", json!({"tarballPath": TARBALL, "voice": {"publicUrl": "wss://evil.example"}}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "url_not_on_this_box"), "refused before any effect");
    assert!(t.ops.effects().is_empty());
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    let (st, v) = t.call(
        &agent("w-it"),
        "/cli/sew/server/enable",
        json!({"tarballPath": TARBALL, "identity": "standalone",
               "voice": {"publicUrl": "wss://hearth.lztek.k2.dev", "mediaTarballPath": MEDIA}}),
    );
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["state"], "running");
    assert_eq!(v["publicUrl"], "wss://hearth.lztek.k2.dev");
    assert_eq!(v["turnHost"], "turn-lztek.k2.dev");
    let s = t.get(&Who::Owner, "/cli/sew/status").1;
    assert_eq!(s["identityProvider"], "standalone");
    assert_eq!(s["voice"]["publicUrl"], "wss://hearth.lztek.k2.dev");
    // Voice URL later, and setup links on the App.
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/server/config", json!({"publicUrl": "https://hearth.lztek.k2.dev"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["setupLinks"], "https://hearth.lztek.k2.dev/setup");
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/server/config", json!({"setupUrl": "https://hearth.lztek.k2.dev/setup"}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"), "setupUrl is never sent: {v}");
    // Users: the setup link comes back to the caller.
    t.ops.mgmt_answers.lock().unwrap().entry("POST /mgmt/v1/local/users".into()).or_default().push_back(Ok(json!({
        "member_id": "5", "username": "bob", "install_role": "member", "deactivated": false, "created": true,
        "reactivated": false, "changed": true,
        "setup_link": {"url": "https://hearth.lztek.k2.dev/setup#token=sewpw_x", "expires_at": "2026-10-17T00:00:00Z"}})));
    let (st, v) = t.call(&agent("w-it"), "/cli/sew/users/invite", json!({"user": "bob", "role": "member"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["setupLink"]["url"], "https://hearth.lztek.k2.dev/setup#token=sewpw_x");
    let _ = INSTALL_ID;
}

#[test]
fn the_new_sew_routes_are_classified() {
    use crate::routes::route_policy::lookup;
    for p in [
        "/cli/sew/media/install",
        "/cli/sew/media/url",
        "/cli/sew/media/renew",
        "/cli/sew/media/probe",
        "/cli/sew/media/uninstall",
        "/cli/sew/server/config",
        "/cli/sew/meet/bind",
        "/cli/sew/meet/unbind",
        "/cli/sew/users/invite",
        "/cli/sew/users/owner",
        "/cli/sew/users/reset",
        "/cli/sew/users/revoke",
        "/cli/sew/users/reactivate",
        "/cli/sew/users/role",
    ] {
        assert!(is_post_route(p), "{p}");
        assert!(lookup(p).unwrap().post.is_some(), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
    for p in ["/cli/sew/users", "/cli/sew/media/preflight"] {
        assert!(!is_post_route(p) && is_route(p), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
}

/// item 12 (0.46.5): `k2 hostchat upgrade --accept-binary <sha>` is the
/// owner's, with a fresh vault confirm bound to this install AND that sha.
/// Agents (even with Manage Sew), K2 terminals and admin logins are
/// refused; a confirm for another sha, or none, is refused; a malformed sha
/// is a 400. Nothing reaches the helper's upgrade until all of that holds.
#[test]
fn accept_binary_is_the_owners_with_a_confirm_bound_to_that_sha() {
    use crate::sew::ops::fake::UNKNOWN_SHA;
    let t = t();
    t.enable();
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    t.ops.clear();
    let vault = crate::vault::ensure_test_vault();
    let up = |who: &Who, accept: Option<&str>, tok: Option<String>| {
        let mut b = json!({"tarballPath": TARBALL});
        if let Some(a) = accept {
            b["acceptBinary"] = json!(a);
        }
        if let Some(tok) = tok {
            b["confirmToken"] = json!(tok);
        }
        t.call(who, "/cli/sew/server/upgrade", b)
    };
    let owner_tok = |sha: &str| vault.confirm.mint(CONFIRM_ACCEPT_BINARY, Some(&accept_binary_secret_id(sha)), "owner", "vault-password");
    let upgraded = || t.ops.effects().iter().any(|e| e.starts_with("engine sew upgrade"));

    // Without the flag, even the owner: refused, the hint names the way.
    let (st, v) = up(&Who::Owner, None, None);
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "unknown_sew_binary"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("--accept-binary {UNKNOWN_SHA}")), "{v}");
    // A passport (Manage Sew on), a K2 terminal: never, confirm or not.
    for who in [agent("w-it"), shell("w-it")] {
        let (st, v) = up(&who, Some(UNKNOWN_SHA), Some(owner_tok(UNKNOWN_SHA)));
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "person_only"), "{who:?}: {v}");
    }
    // An admin login with its own confirm: the owner's alone.
    let admins = vault.confirm.mint(CONFIRM_ACCEPT_BINARY, Some(&accept_binary_secret_id(UNKNOWN_SHA)), "connect:ana", "connect-password");
    let (st, v) = up(&login(Role::Admin), Some(UNKNOWN_SHA), Some(admins));
    assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "owner_only"), "{v}");
    // The owner without a confirm, or with one for another sha.
    assert_eq!(code(&up(&Who::Owner, Some(UNKNOWN_SHA), None).1), "owner_confirm_required");
    let other = "a".repeat(64);
    assert_eq!(code(&up(&Who::Owner, Some(UNKNOWN_SHA), Some(owner_tok(&other))).1), "owner_confirm_required");
    // A confirm for another action doesn't do either.
    let purge = vault.confirm.mint(CONFIRM_PURGE, Some(INSTALL_ID), "owner", "vault-password");
    assert_eq!(code(&up(&Who::Owner, Some(UNKNOWN_SHA), Some(purge)).1), "owner_confirm_required");
    // Malformed: 400 before any gate.
    for bad in [json!("7ED8"), json!(UNKNOWN_SHA.to_uppercase()), json!(5)] {
        let (st, v) = t.call(&Who::Owner, "/cli/sew/server/upgrade", json!({"tarballPath": TARBALL, "acceptBinary": bad}));
        assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"), "{v}");
    }
    // The owner, confirmed, but for a sha that isn't the binary's.
    let (st, v) = up(&Who::Owner, Some(&other), Some(owner_tok(&other)));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "accept_binary_mismatch"), "{v}");
    assert!(!upgraded(), "{:?}", t.ops.effects());
    assert_eq!(crate::sew::store::load().unwrap().status, "running");
    // The owner, confirmed for exactly that sha: upgraded to the pin.
    let (st, v) = up(&Who::Owner, Some(UNKNOWN_SHA), Some(owner_tok(UNKNOWN_SHA)));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["upgrade"]["result"], "done", "{v}");
    assert!(t.ops.effects().iter().any(|e| *e == format!("engine sew upgrade accept-binary {UNKNOWN_SHA}")), "{:?}", t.ops.effects());
    let row = crate::sew::store::load().unwrap();
    assert_eq!(row.installed_version.as_deref(), Some(k2_root_proto::sew::pinned().version));
    // ...and K2 never rolls back onto it.
    let (st, v) = t.call(&Who::Owner, "/cli/sew/server/rollback", json!({}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "rollback_refused"), "{v}");
    // A confirm works once.
    let tok = owner_tok(UNKNOWN_SHA);
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    let _ = up(&Who::Owner, Some(UNKNOWN_SHA), Some(tok.clone()));
    assert_eq!(code(&up(&Who::Owner, Some(UNKNOWN_SHA), Some(tok)).1), "owner_confirm_required");
}
