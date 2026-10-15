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
    // An admin login is not the owner.
    assert_eq!(code(&t.call(&login(Role::Admin), "/cli/sew/server/recovery-key", json!({})).1), "owner_only");
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
