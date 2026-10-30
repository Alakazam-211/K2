//! S32 daemon side: status fields (`you`, owner/target/mirror/ship), the
//! app-url / ship / mirror routes, the off-box shipper, K31's App Space
//! routes, and every route's floor. Machines run inline over the fake box
//! (`FakeOps`); skin.db only under a temp HOME.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::*;
use crate::sew::ops::fake::FakeOps;
use crate::sew::routes::handle_env;
use crate::sew::secrets::MemSecrets;

const TARBALL: &str = "/home/k2/sew.tar.gz";
const OFFBOX: &str = "/mnt/offbox/sew";

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

/// A ShipFs over memory: folders that exist, what was shipped.
#[derive(Default)]
struct MemFs {
    dirs: Mutex<Vec<String>>,
    shipped: Mutex<Vec<(String, String, String)>>,
    fail: Mutex<Option<String>>,
    pruned: Mutex<usize>,
}

impl ShipFs for MemFs {
    fn check_dir(&self, root: &str) -> Result<(), String> {
        if self.dirs.lock().unwrap().iter().any(|d| d == root) {
            Ok(())
        } else {
            Err(format!("{root} can't be read: No such file or directory"))
        }
    }
    fn ship(&self, _env: &Env, root: &str, install_id: &str, bundle_id: &str) -> Result<ShipReceipt, String> {
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        self.shipped.lock().unwrap().push((root.into(), install_id.into(), bundle_id.into()));
        Ok(ShipReceipt { bytes: 140, sha256: "ab".repeat(32) })
    }
    fn prune(&self, _root: &str, _install_id: &str, _keep: usize) -> Result<usize, String> {
        *self.pruned.lock().unwrap() += 1;
        Ok(0)
    }
}

fn memfs() -> &'static MemFs {
    let fs: &'static MemFs = Box::leak(Box::new(MemFs::default()));
    fs.dirs.lock().unwrap().push(OFFBOX.into());
    use_fs_for_tests(Some(fs));
    fs
}

// ── you ─────────────────────────────────────────────────────────────────

#[test]
fn status_you_says_what_this_caller_may_do() {
    let t = t();
    let cases: Vec<(Who, [bool; 6])> = vec![
        // canManage, isOwner, isAdmin, canRecoveryKey, canPurge, canOpenAsOwner
        (Who::Owner, [true, true, true, true, true, true]),
        (login(Role::Owner), [true, true, true, true, true, true]),
        (login(Role::Admin), [true, false, true, RECOVERY_KEY_ADMIN, false, false]),
        (login(Role::Member), [false, false, false, false, false, false]),
        // Manage Sew on: manage yes, every person-only step no.
        (agent("w-it"), [true, false, false, false, false, false]),
        (shell("w-it"), [true, false, false, false, false, false]),
        (agent("w-plain"), [false, false, false, false, false, false]),
    ];
    for (who, want) in cases {
        let (st, v) = t.get(&who, "/cli/sew/status");
        assert_eq!(st, "200 OK", "{who:?}");
        let y = &v["you"];
        let got = [
            y["canManage"].as_bool().unwrap(),
            y["isOwner"].as_bool().unwrap(),
            y["isAdmin"].as_bool().unwrap(),
            y["canRecoveryKey"].as_bool().unwrap(),
            y["canPurge"].as_bool().unwrap(),
            y["canOpenAsOwner"].as_bool().unwrap(),
        ];
        assert_eq!(got, want, "{who:?}: {y}");
        assert_eq!(y["passport"].as_bool().unwrap(), who.passport().is_some(), "{who:?}");
    }
}

#[test]
fn installed_status_carries_owner_target_mirror_and_ship() {
    let home = k2_core::test_env::TempHome::short();
    let t = t();
    t.enable();
    let (_, v) = t.get(&Who::Owner, "/cli/sew/status");
    // No hook read in this test: the fields still exist.
    assert_eq!(v["owner"], "none");
    assert_eq!(v["target"]["enabled"], false);
    assert_eq!(v["mirror"], json!({"kind": "none", "path": null}));
    assert_eq!(v["backup"]["ship"]["kind"], "none");
    assert!(v["backup"]["ship"].get("lastShippedId").is_some());
    assert!(v["backup"].get("lastOkAt").is_some(), "S29's backup fields stay");
    // With the hook read (temp HOME): the S27 chat target and k2:owner.
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target(k2_core::tool_hook::CHAT_KIND, true, Some("/tmp/x.sock"), Some("/tmp/x.token"), "owner-token").unwrap();
    read_hook_in_tests(true);
    let (_, v) = t.get(&Who::Owner, "/cli/sew/status");
    read_hook_in_tests(false);
    assert_eq!(v["target"]["enabled"], true);
    assert_eq!(v["owner"], "pending", "k2:owner is queued, not yet delivered");
    drop(home);
}

// ── app URL ─────────────────────────────────────────────────────────────

#[test]
fn app_url_validation_table() {
    for (raw, want) in [
        ("https://hearth.lztek.k2.dev", Some("https://hearth.lztek.k2.dev")),
        ("https://Hearth.Example.com:8443/app/", Some("https://hearth.example.com:8443/app")),
        ("http://127.0.0.1:5173", Some("http://127.0.0.1:5173")),
        ("http://localhost/x", Some("http://localhost/x")),
        ("  ", None),
        ("", None),
    ] {
        assert_eq!(check_app_url(raw).unwrap().as_deref(), want, "{raw}");
    }
    for bad in [
        "http://hearth.example.com",
        "ftp://x.dev",
        "https://x.dev/?a=1",
        "https://x.dev/#frag",
        "https://user@x.dev",
        "https://x.dev:0",
        "https://x.dev:70000",
        "https://-x.dev",
        "https://x dev",
        "hearth.example.com",
        "https://x.dev/../etc",
        &format!("https://x.dev/{}", "a".repeat(600)),
    ] {
        let e = check_app_url(bad).unwrap_err();
        assert_eq!(e.code, "invalid_url", "{bad}");
        assert_eq!(e.status, 400);
    }
}

#[test]
fn app_url_sets_clears_and_shows_in_status() {
    let t = t();
    let (st, v) = t.call(&Who::Owner, APP_URL, json!({"url": "https://hearth.lztek.k2.dev/"}));
    assert_eq!((st.as_str(), code(&v)), ("404 Not Found", "not_installed"), "{v}");
    t.enable();
    let _ = take_emitted();
    let (st, v) = t.call(&agent("w-it"), APP_URL, json!({"url": "https://hearth.lztek.k2.dev/"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(take_emitted(), vec!["app-url"], "K26: one refetch signal");
    assert_eq!(v["appUrl"], "https://hearth.lztek.k2.dev");
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["appUrl"], "https://hearth.lztek.k2.dev");
    let (_, v) = t.call(&Who::Owner, APP_URL, json!({"url": ""}));
    assert_eq!(v["appUrl"], Value::Null);
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["appUrl"], Value::Null);
    let _ = take_emitted();
    let (st, v) = t.call(&Who::Owner, APP_URL, json!({}));
    assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", "bad_request"));
    assert!(take_emitted().is_empty(), "a refusal changes nothing");
}

// ── ship / mirror ───────────────────────────────────────────────────────

#[test]
fn ship_sets_dir_or_none_and_refuses_the_vault_system_paths_and_k2() {
    let t = t();
    let fs = memfs();
    t.enable();
    for (body, want) in [
        (json!({"kind": "dir"}), "bad_request"),
        (json!({"kind": "dir", "path": "relative/x"}), "invalid_path"),
        (json!({"kind": "dir", "path": "/var/lib/sew/backups"}), "invalid_path"),
        (json!({"kind": "dir", "path": "/home/k2/.k2/vault"}), "invalid_path"),
        (json!({"kind": "dir", "path": "/etc/sew"}), "invalid_path"),
        (json!({"kind": "dir", "path": "/mnt/missing"}), "invalid_path"),
        (json!({"kind": "k2"}), "not_available"),
        (json!({"kind": "s3"}), "bad_request"),
    ] {
        let (st, v) = t.call(&Who::Owner, SHIP, body.clone());
        assert_eq!(st, "400 Bad Request", "{body}: {v}");
        assert_eq!(code(&v), want, "{body}: {v}");
    }
    assert_eq!(crate::sew::store::load().unwrap().ship_kind, "none", "nothing changed");
    assert!(take_emitted().is_empty());
    let (st, v) = t.call(&agent("w-it"), SHIP, json!({"kind": "dir", "path": OFFBOX}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(take_emitted(), vec!["ship"]);
    let row = crate::sew::store::load().unwrap();
    assert_eq!((row.ship_kind.as_str(), row.ship_path.as_deref()), ("dir", Some(OFFBOX)));
    let (_, s) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(s["backup"]["ship"]["path"], OFFBOX);
    assert!(!s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_backup_offbox_none"), "{s}");
    let (st, _) = t.call(&Who::Owner, SHIP, json!({"kind": "none"}));
    assert_eq!(st, "200 OK");
    let (_, s) = t.get(&Who::Owner, "/cli/sew/status");
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_backup_offbox_none"));
    assert!(fs.shipped.lock().unwrap().is_empty(), "setting a destination ships nothing by itself");
    use_fs_for_tests(None);
}

#[test]
fn mirror_goes_through_engine_sew_config_then_restarts() {
    let t = t();
    t.enable();
    t.ops.exists.lock().unwrap().push("/mnt/keys".into());
    for (body, want) in [
        (json!({"kind": "dir", "path": "/home/k2/.k2/vault"}), "invalid_path"),
        (json!({"kind": "dir", "path": "/run/sew"}), "invalid_path"),
        (json!({"kind": "k2"}), "not_available"),
    ] {
        let (st, v) = t.call(&Who::Owner, MIRROR, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", want), "{body}: {v}");
    }
    assert!(t.ops.effects().is_empty(), "refusals reach no helper: {:?}", t.ops.effects());
    let _ = take_emitted();
    let (st, v) = t.call(&Who::Owner, MIRROR, json!({"kind": "dir", "path": "/mnt/keys"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["restarted"], true);
    assert_eq!(take_emitted(), vec!["mirror"]);
    let calls = t.ops.calls.lock().unwrap().clone();
    let cfg = calls.iter().find(|c| c.argv == "engine sew config").expect("config call");
    let stdin: Value = serde_json::from_slice(cfg.stdin.as_deref().expect("stdin")).unwrap();
    assert_eq!(stdin, json!({"keys.mirror.path": "/mnt/keys", "keys.mirror.kind": "dir"}));
    serde_json::from_value::<k2_root_proto::sew::ConfigRequest>(stdin.clone()).expect("the helper's own type takes it").validate().expect("valid");
    assert!(t.ops.effects().iter().any(|e| e == "engine sew systemctl restart"));
    let row = crate::sew::store::load().unwrap();
    assert_eq!((row.mirror_kind.as_str(), row.mirror_path.as_deref()), ("dir", Some("/mnt/keys")));
    assert_eq!(t.get(&Who::Owner, "/cli/sew/status").1["mirror"], json!({"kind": "dir", "path": "/mnt/keys"}));
    let (st, _) = t.call(&Who::Owner, MIRROR, json!({"kind": "none"}));
    assert_eq!(st, "200 OK");
    assert_eq!(crate::sew::store::load().unwrap().mirror_kind, "none");
}

#[test]
fn ship_tick_copies_each_new_bundle_once_and_alarms_on_failure() {
    let t = t();
    let fs: &'static MemFs = Box::leak(Box::new(MemFs::default()));
    fs.dirs.lock().unwrap().push(OFFBOX.into());
    t.enable();
    let install = crate::sew::store::load().unwrap().install_id.unwrap();
    // No backup yet: nothing to say.
    assert_eq!(ship_tick_with(&t.env, fs), None);
    t.ops.mgmt_status.lock().unwrap()["backup"] = json!({"last_id": "20260103T000000Z", "last_ok_at": "2026-01-03T00:00:00Z"});
    // Sew made a bundle: the `backup` signal, nothing shipped (kind none).
    let _ = take_emitted();
    assert_eq!(ship_tick_with(&t.env, fs).as_deref(), Some("backup"));
    assert_eq!(ship_tick_with(&t.env, fs), None, "said once");
    assert_eq!(take_emitted(), vec!["backup"], "K26 emit point: a new bundle");
    assert!(fs.shipped.lock().unwrap().is_empty());
    // A destination: the next tick ships that bundle, once, and prunes.
    crate::sew::store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.ship_path = Some(OFFBOX.into());
    })
    .unwrap();
    assert_eq!(ship_tick_with(&t.env, fs).as_deref(), Some("backup-shipped"));
    assert_eq!(ship_tick_with(&t.env, fs), None);
    assert_eq!(*fs.shipped.lock().unwrap(), vec![(OFFBOX.to_string(), install.clone(), "20260103T000000Z".to_string())]);
    assert_eq!(*fs.pruned.lock().unwrap(), 1);
    let (_, s) = t.get(&Who::Owner, "/cli/sew/status");
    assert_eq!(s["backup"]["ship"]["lastShippedId"], "20260103T000000Z");
    assert!(!s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_backup_not_shipped"));
    // The next bundle fails to copy: alarm, said once, cleared on success.
    t.ops.mgmt_status.lock().unwrap()["backup"]["last_id"] = json!("20260104T000000Z");
    *fs.fail.lock().unwrap() = Some("write: No space left on device".into());
    assert_eq!(ship_tick_with(&t.env, fs).as_deref(), Some("ship-failed"));
    assert_eq!(ship_tick_with(&t.env, fs), None, "the same error is said once");
    let (_, s) = t.get(&Who::Owner, "/cli/sew/status");
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_backup_not_shipped"), "{s}");
    assert_eq!(s["state"], "degraded");
    assert_eq!(s["backup"]["ship"]["lastError"], "write: No space left on device");
    *fs.fail.lock().unwrap() = None;
    assert_eq!(ship_tick_with(&t.env, fs).as_deref(), Some("backup-shipped"));
    let (_, s) = t.get(&Who::Owner, "/cli/sew/status");
    assert!(!s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_backup_not_shipped"));
    // A lifecycle step holds the latch: the tick stays out of its way.
    t.env.latch.store(true, std::sync::atomic::Ordering::SeqCst);
    t.ops.mgmt_status.lock().unwrap()["backup"]["last_id"] = json!("20260105T000000Z");
    assert_eq!(ship_tick_with(&t.env, fs), None);
    t.env.latch.store(false, std::sync::atomic::Ordering::SeqCst);
}

// ── K31 App Spaces ──────────────────────────────────────────────────────

#[test]
fn app_space_bind_binds_sew_first_then_records_and_unbind_forgets() {
    let home = k2_core::test_env::TempHome::short();
    let t = t();
    t.enable();
    for (body, want) in [
        (json!({"app": "host", "spaceId": "spc_1"}), "bad_request"),
        (json!({"app": "hearth", "spaceId": "bad id"}), "bad_request"),
        (json!({"app": "", "spaceId": "spc_1"}), "bad_request"),
    ] {
        let (st, v) = t.call(&Who::Owner, APP_SPACES_BIND, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("400 Bad Request", want), "{body}: {v}");
    }
    assert!(t.ops.mgmt_calls.lock().unwrap().iter().all(|c| !c.contains("spaces")), "nothing reached Sew");
    // Sew says the Space belongs to another App.
    t.ops
        .mgmt_answers
        .lock()
        .unwrap()
        .entry("POST /mgmt/v1/spaces/bind".into())
        .or_default()
        .push_back(Err(crate::sew::ops::MgmtError::Status(409, "{\"error\":{\"code\":\"conflict\"}}".into())));
    let (st, v) = t.call(&Who::Owner, APP_SPACES_BIND, json!({"app": "hearth", "spaceId": "spc_1"}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "space_bound"), "{v}");
    assert!(k2_core::tool_hook::app_spaces().unwrap().is_empty());
    // Sew 0.5.0: an orphan of another App (a person releases it first).
    t.ops
        .mgmt_answers
        .lock()
        .unwrap()
        .entry("POST /mgmt/v1/spaces/bind".into())
        .or_default()
        .push_back(Err(crate::sew::ops::MgmtError::Status(
            409,
            "{\"error\":{\"code\":\"conflict\",\"details\":{\"reason\":\"space_bound_elsewhere\"}}}".into(),
        )));
    let (st, v) = t.call(&Who::Owner, APP_SPACES_BIND, json!({"app": "hearth", "spaceId": "spc_1"}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "space_bound_elsewhere"), "{v}");
    assert!(v["error"]["hint"].as_str().is_some_and(|h| h.contains("release")), "{v}");
    assert!(k2_core::tool_hook::app_spaces().unwrap().is_empty());
    let (_, v) = t.get(&Who::Owner, APP_SPACES);
    assert_eq!(v["unbound"], json!([]), "no removed App's Space yet: {v}");
    // Bound.
    t.ops
        .mgmt_answers
        .lock()
        .unwrap()
        .entry("POST /mgmt/v1/spaces/bind".into())
        .or_default()
        .push_back(Ok(json!({"space_id": "spc_1", "app_id": "hearth", "created": false})));
    let (st, v) = t.call(&agent("w-it"), APP_SPACES_BIND, json!({"app": "Hearth", "spaceId": "spc_1"}));
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!((v["app"].as_str(), v["created"].as_bool()), (Some("hearth"), Some(true)));
    let bodies = t.ops.mgmt_bodies.lock().unwrap().clone();
    assert!(bodies.iter().flatten().any(|b| *b == json!({"space_id": "spc_1", "app_id": "hearth"})), "{bodies:?}");
    let (st, v) = t.get(&login(Role::Member), APP_SPACES);
    assert_eq!(st, "200 OK", "any real credential reads the list");
    assert_eq!(v["spaces"][0]["appId"], "hearth");
    assert_eq!(v["spaces"][0]["spaceId"], "spc_1");
    assert_eq!(v["spaces"][0]["boundBy"], "agent:w-it");
    assert_eq!(v["spaces"][0]["members"], json!({"added": 0, "held": 0, "pending": 0}));
    let (st, v) = t.call(&Who::Owner, APP_SPACES_UNBIND, json!({"app": "hearth", "spaceId": "spc_2"}));
    assert_eq!((st.as_str(), code(&v)), ("404 Not Found", "not_found"));
    let (st, v) = t.call(&Who::Owner, APP_SPACES_UNBIND, json!({"app": "hearth", "spaceId": "spc_1"}));
    assert_eq!(st, "200 OK", "{v}");
    assert!(k2_core::tool_hook::app_spaces().unwrap().is_empty());
    drop(home);
}

#[test]
fn app_space_bind_needs_a_running_k2_identity_sew() {
    let home = k2_core::test_env::TempHome::short();
    let t = t();
    let (st, v) = t.call(&Who::Owner, APP_SPACES_BIND, json!({"app": "hearth", "spaceId": "spc_1"}));
    assert_eq!((st.as_str(), code(&v)), ("404 Not Found", "not_installed"));
    t.enable();
    t.call(&Who::Owner, "/cli/sew/server/disable", json!({}));
    let (st, v) = t.call(&Who::Owner, APP_SPACES_BIND, json!({"app": "hearth", "spaceId": "spc_1"}));
    assert_eq!((st.as_str(), code(&v)), ("409 Conflict", "sew_not_running"));
    drop(home);
}

// ── floors: every S32 route ─────────────────────────────────────────────

/// Sew 0.6.0 (AG-7): Manage Sew lets an agent reach the governance switch,
/// but only a person (owner or Admin, with a fresh confirm) flips it.
#[test]
fn governance_needs_manage_sew_and_then_a_person() {
    let t = t();
    t.enable();
    let body = json!({"app": "hearth", "spaceId": "spc_1", "governance": "app"});
    for who in [agent("w-it"), shell("w-it")] {
        let (st, v) = t.call(&who, APP_SPACES_GOVERNANCE, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "person_only"), "{who:?}: {v}");
    }
    for who in [Who::Owner, login(Role::Admin)] {
        let (st, v) = t.call(&who, APP_SPACES_GOVERNANCE, body.clone());
        assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", "owner_confirm_required"), "{who:?}: {v}");
    }
    assert!(t.ops.effects().is_empty(), "no refused call reached the helper: {:?}", t.ops.effects());
}

#[test]
fn every_s32_mutation_needs_manage_sew_and_post() {
    let t = t();
    let _fs = memfs();
    t.enable();
    let bodies: [(&str, Value); 6] = [
        (APP_URL, json!({"url": "https://x.dev"})),
        (SHIP, json!({"kind": "none"})),
        (MIRROR, json!({"kind": "none"})),
        (APP_SPACES_BIND, json!({"app": "hearth", "spaceId": "spc_1"})),
        (APP_SPACES_UNBIND, json!({"app": "hearth", "spaceId": "spc_1"})),
        // Sew 0.6.0: the governance switch has the Manage Sew floor too
        // (and a person gate on top: the test below).
        (APP_SPACES_GOVERNANCE, json!({"app": "hearth", "spaceId": "spc_1", "governance": "app"})),
    ];
    assert_eq!(bodies.len(), POST_ROUTES.len());
    for (path, body) in &bodies {
        assert!(POST_ROUTES.contains(path));
        assert!(crate::sew::routes::is_route(path) && crate::sew::routes::is_post_route(path), "{path}");
        for (who, want) in [
            (login(Role::Member), "owner_only"),
            (agent("w-plain"), "chat_manage_off"),
            (shell("w-plain"), "chat_manage_off"),
            (Who::Wrong, "wrong_credential"),
            (Who::Invalid, "forbidden"),
        ] {
            let (st, v) = t.call(&who, path, body.clone());
            assert_eq!((st.as_str(), code(&v)), ("403 Forbidden", want), "{who:?} {path}: {v}");
        }
    }
    assert!(t.ops.effects().is_empty(), "no refused call reached the helper: {:?}", t.ops.effects());
    // The read: any real credential, never an app pass.
    assert!(crate::sew::routes::is_route(APP_SPACES) && !crate::sew::routes::is_post_route(APP_SPACES));
    for who in [Who::Wrong, Who::Invalid] {
        assert_eq!(t.get(&who, APP_SPACES).0, "403 Forbidden");
    }
    // Route policy: Admin floor for every mutation, Member for the read;
    // the agent verbs list has them (the handler checks Manage Sew).
    use crate::routes::route_policy::lookup;
    for p in POST_ROUTES {
        let r = lookup(p).unwrap_or_else(|| panic!("{p} is not classified"));
        assert_eq!((r.get.map(|f| f.as_wire()), r.post.map(|f| f.as_wire())), (None, Some("admin")), "{p}");
        assert!(crate::session_token::is_agent_verb(p), "{p}");
    }
    let r = lookup(APP_SPACES).expect("classified");
    assert_eq!((r.get.map(|f| f.as_wire()), r.post.map(|f| f.as_wire())), (Some("member"), None));
    assert!(crate::session_token::is_agent_verb(APP_SPACES));
    use_fs_for_tests(None);
}

#[test]
fn unsupported_box_refuses_ship_and_mirror_but_reads_you() {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps { linux: false, ..FakeOps::new() }));
    let env = Env {
        ops,
        secrets: Box::leak(Box::new(MemSecrets::default())),
        latch: Box::leak(Box::new(AtomicBool::new(false))),
        token: Box::leak(Box::new(Mutex::new(None))),
    };
    let _db = k2_core::db::scoped_for_test();
    let r = handle_env(env, "/cli/sew/status", &Who::Owner, b"", &HashMap::new(), false);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["supported"], false);
    assert_eq!(v["you"]["canManage"], true);
    for p in [SHIP, MIRROR] {
        let r = handle_env(env, p, &Who::Owner, br#"{"kind":"none"}"#, &HashMap::new(), false);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((r.status, code(&v)), ("409 Conflict", "unsupported"), "{p}");
    }
}

// ── the real disk + the real socket ─────────────────────────────────────

#[cfg(unix)]
#[test]
fn check_dir_on_disk_refuses_files_and_the_data_disk_and_accepts_a_writable_folder() {
    let home = k2_core::test_env::TempHome::short();
    let base = home.path().join("offbox");
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("f");
    std::fs::write(&file, b"x").unwrap();
    let other = std::path::Path::new("/nonexistent-k2-sew-data");
    assert!(check_dir_on_disk(&file, other).unwrap_err().contains("not a folder"));
    assert!(check_dir_on_disk(&base.join("missing"), other).unwrap_err().contains("can't be read"));
    // Sew's data on the same disk (a sibling folder) → refused.
    let data = home.path().join("sewdata");
    std::fs::create_dir_all(&data).unwrap();
    assert!(check_dir_on_disk(&base, &data).unwrap_err().contains("same disk"));
    // A data folder that isn't there (Sew never installed): only writability.
    check_dir_on_disk(&base, other).expect("writable folder");
    assert_eq!(std::fs::read_dir(&base).unwrap().count(), 1, "the probe file is gone");
}

#[cfg(unix)]
#[test]
fn prune_keeps_the_newest_bundles_only() {
    let home = k2_core::test_env::TempHome::short();
    let d = home.path().join("i1");
    std::fs::create_dir_all(&d).unwrap();
    for i in 1..=9 {
        std::fs::write(d.join(bundle_file(&format!("2026010{i}T000000Z"))), b"t").unwrap();
    }
    std::fs::write(d.join("notes.txt"), b"keep me").unwrap();
    assert_eq!(prune_dir(&d, SHIP_KEEP).unwrap(), 2);
    let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left.len(), 8);
    let bundles: Vec<&String> = left.iter().filter(|n| n.starts_with("sew-backup-")).collect();
    assert_eq!(bundles.len(), SHIP_KEEP);
    assert_eq!(*bundles[0], bundle_file("20260103T000000Z"), "the two oldest went");
    assert_eq!(*bundles[6], bundle_file("20260109T000000Z"));
    assert!(left.contains(&"notes.txt".to_string()), "other files stay");
}

#[cfg(unix)]
#[test]
fn download_archive_streams_length_chunked_and_eof_bodies_and_maps_errors() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    let dir = std::path::PathBuf::from(format!("/tmp/k2s32-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("m.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let tar = b"bundle-bytes-0123456789".to_vec();
    let answers: Vec<Vec<u8>> = vec![
        [format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: {}\r\n\r\n", tar.len()).into_bytes(), tar.clone()].concat(),
        [b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(), b"a\r\nbundle-byt\r\nd\r\nes-0123456789\r\n0\r\n\r\n".to_vec()].concat(),
        [b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), tar.clone()].concat(),
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nno bundle".to_vec(),
        // Cut short: Content-Length promises more than arrives.
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort".to_vec(),
    ];
    let n = answers.len();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for a in answers {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let k = s.read(&mut buf).unwrap();
            seen.push(String::from_utf8_lossy(&buf[..k]).to_string());
            s.write_all(&a).unwrap();
        }
        seen
    });
    use sha2::{Digest, Sha256};
    let want_sha: String = Sha256::digest(&tar).iter().map(|b| format!("{b:02x}")).collect();
    for _ in 0..3 {
        let mut out = Vec::new();
        let r = download_archive(&sock, "tok", "20260103T000000Z", &mut out).expect("download");
        assert_eq!(out, tar);
        assert_eq!(r, ShipReceipt { bytes: tar.len() as u64, sha256: want_sha.clone() });
    }
    let mut out = Vec::new();
    assert_eq!(download_archive(&sock, "tok", "x", &mut out).unwrap_err(), crate::sew::ops::MgmtError::Unauthorized);
    assert!(matches!(download_archive(&sock, "tok", "x", &mut out), Err(crate::sew::ops::MgmtError::Status(404, b)) if b == "no bundle"));
    assert!(matches!(download_archive(&sock, "tok", "x", &mut out), Err(crate::sew::ops::MgmtError::Unreachable(e)) if e.contains("cut short")));
    let seen = server.join().unwrap();
    assert_eq!(seen.len(), n);
    assert!(seen[0].starts_with("GET /mgmt/v1/backups/20260103T000000Z/archive HTTP/1.1\r\n"), "{}", seen[0]);
    assert!(seen[0].contains("Authorization: Bearer tok\r\n"));
    std::fs::remove_dir_all(&dir).unwrap();
}
