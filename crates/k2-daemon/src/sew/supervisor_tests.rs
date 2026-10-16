//! Supervisor state machines over a fake box (S29 §23). Nothing here runs
//! sudo, systemd, Sew or the network; every test gets its own DB, latch,
//! token cache, fake ops and in-memory vault.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::json;

use super::*;
use crate::sew::ops::fake::{helper_ok, FakeOps, GIB, INSTALL_ID};
use crate::sew::secrets::MemSecrets;

struct T {
    ops: FakeOps,
    secrets: MemSecrets,
    latch: AtomicBool,
    token: Mutex<Option<String>>,
    _db: k2_core::db::TestDbGuard,
}

const TARBALL: &str = "/home/k2/sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz";

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn t() -> T {
    let ops = FakeOps::new();
    ops.files.lock().unwrap().insert(TARBALL.into(), fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    T { ops, secrets: MemSecrets::default(), latch: AtomicBool::new(false), token: Mutex::new(None), _db: k2_core::db::scoped_for_test() }
}

impl T {
    fn env(&self) -> Env<'_> {
        Env { ops: &self.ops, secrets: &self.secrets, latch: &self.latch, token: &self.token }
    }
    fn req(&self) -> EnableRequest {
        EnableRequest { tarball_path: Some(TARBALL.into()), recovery_key_file: None, ..Default::default() }
    }
    /// The full enable, inline. Panics with the refusal on failure.
    fn enable(&self) -> Result<(), String> {
        let env = self.env();
        let latch = match enable_begin(&env, &self.req()) {
            Ok(Ok(l)) => l,
            Ok(Err(v)) => return Err(format!("answered without a machine: {v}")),
            Err(f) => return Err(format!("{}: {}", f.code, f.hint)),
        };
        let r = enable_machine(&env, &self.req());
        drop(latch);
        r
    }
    fn enabled(&self) {
        self.enable().expect("enable");
        assert_eq!(store::load().unwrap().status, "running");
        self.ops.clear();
    }
    fn row(&self) -> Row {
        store::load().expect("row")
    }
    fn master_key(&self) -> String {
        let r = self.row().master_key_ref.expect("ref");
        self.secrets.resolve(&r).unwrap().expect("key")
    }
}

// ── enable ───────────────────────────────────────────────────────────────

#[test]
fn enable_sequence_is_recorded_exactly_and_the_key_rides_only_install_stdin() {
    let t = t();
    t.enable().unwrap();
    assert_eq!(
        t.ops.effects(),
        vec![
            "engine sew stage",
            "engine sew install-bin",
            "engine sew install",
            "engine sew systemctl enable",
            "engine sew systemctl start-db",
            "engine sew systemctl migrate",
            "engine sew systemctl start", // no restart: Sew wrote allow_uids before its first start (SW5)
            "engine sew token",
            "engine sew unstage",
        ]
    );
    assert_eq!(*t.ops.mgmt_calls.lock().unwrap(), vec!["GET /mgmt/v1/health", "GET /mgmt/v1/status"]);
    let row = t.row();
    assert_eq!(row.status, "running");
    assert_eq!(row.installed_version.as_deref(), Some(pinned().version));
    assert_eq!(row.install_id.as_deref(), Some(INSTALL_ID));
    assert_eq!(row.enable_progress.as_ref().unwrap()["result"], "done");
    assert!(row.installed_at.is_some());

    let key = t.master_key();
    assert!(pin::master_key_text_ok(&key));
    let calls = t.ops.calls.lock().unwrap().clone();
    for c in &calls {
        assert!(!c.argv.contains(&key), "key in argv: {}", c.argv);
        let in_stdin = c.stdin.as_ref().is_some_and(|s| String::from_utf8_lossy(s).contains(&key));
        assert_eq!(in_stdin, c.argv == "engine sew install", "{}", c.argv);
    }
    let install = calls.iter().find(|c| c.argv == "engine sew install").unwrap();
    let body: Value = serde_json::from_slice(install.stdin.as_ref().unwrap()).unwrap();
    assert_eq!(body["jwksUrl"], "http://127.0.0.1:38471/v1/chat/jwks");
    // Nowhere in the row or its progress records.
    let all = format!("{:?}", t.row());
    assert!(!all.contains(&key));
    // The stage step sent the pinned tarball itself.
    let stage = calls.iter().find(|c| c.argv == "engine sew stage").unwrap();
    assert_eq!(stage.stdin.as_deref(), Some(&fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz")[..]));
}

#[test]
fn enable_resumes_after_a_mid_flow_failure_without_redoing_marked_steps() {
    let t = t();
    t.ops.queue("engine sew systemctl migrate", Err("root helper engine sew systemctl migrate: exit 5".into()));
    let e = t.enable().unwrap_err();
    assert!(e.starts_with("migrate: "), "{e}");
    let row = t.row();
    assert_eq!(row.status, "error");
    assert!(row.last_error.as_deref().unwrap().starts_with("migrate: "));
    let key = t.master_key();
    t.ops.clear();
    t.enable().unwrap();
    assert_eq!(
        t.ops.effects(),
        vec![
            "engine sew systemctl enable",
            "engine sew systemctl start-db",
            "engine sew systemctl migrate",
            "engine sew systemctl start",
            "engine sew token",
            "engine sew unstage",
        ],
        "no re-download, no re-install; every systemd step again"
    );
    assert_eq!(t.master_key(), key, "the key is never regenerated");
    assert_eq!(t.row().status, "running");
}

#[test]
fn enable_after_disable_reruns_the_systemd_steps_h4() {
    let t = t();
    t.enabled();
    disable(&t.env()).unwrap();
    assert_eq!(t.row().status, "disabled");
    assert!(!t.ops.unit(UNIT_SEW).is_active());
    t.ops.clear();
    t.enable().unwrap();
    let eff = t.ops.effects();
    for s in ["engine sew systemctl enable", "engine sew systemctl start-db", "engine sew systemctl migrate", "engine sew systemctl start"] {
        assert!(eff.iter().any(|e| e == s), "{s} missing: {eff:?}");
    }
    assert!(!eff.iter().any(|e| e == "engine sew install"));
    assert!(t.ops.unit(UNIT_SEW).is_active());
}

#[test]
fn already_enabled_is_a_noop() {
    let t = t();
    t.enabled();
    match enable_begin(&t.env(), &t.req()) {
        Ok(Err(v)) => assert_eq!(v["alreadyEnabled"], true),
        _ => panic!("expected the no-op answer"),
    }
    assert!(t.ops.effects().is_empty());
}

#[test]
fn preflight_hard_stops_refuse_before_any_effect() {
    type Setup = Box<dyn Fn(&T)>;
    let cases: Vec<(&str, Setup)> = vec![
        ("systemd", Box::new(|t: &T| *t.ops.systemd.lock().unwrap() = Some(249))),
        ("postgres", Box::new(|t: &T| *t.ops.pg.lock().unwrap() = vec![14, 15])),
        ("helper", Box::new(|t: &T| *t.ops.helper.lock().unwrap() = helper_ok(6, true, pinned().version))),
        ("helper", Box::new(|t: &T| *t.ops.helper.lock().unwrap() = helper_ok(7, false, pinned().version))),
        ("helper", Box::new(|t: &T| *t.ops.helper.lock().unwrap() = helper_ok(7, true, "0.0.1"))),
        ("ram", Box::new(|t: &T| *t.ops.mem.lock().unwrap() = Some(3 * GIB))),
        ("disk", Box::new(|t: &T| *t.ops.disk.lock().unwrap() = Some(5 * GIB))),
        ("port", Box::new(|t: &T| t.ops.taken.lock().unwrap().push((8470, false)))),
        ("binary", Box::new(|t: &T| *t.ops.on_disk.lock().unwrap() = Some("unknown".into()))),
        ("binary", Box::new(|t: &T| *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into()))),
        ("existing_data", Box::new(|t: &T| t.ops.exists.lock().unwrap().push("/var/lib/sew".into()))),
        ("daemon_port", Box::new(|t: &T| *t.ops.port.lock().unwrap() = None)),
        ("vault", Box::new(|t: &T| *t.secrets.fail_store.lock().unwrap() = true)),
    ];
    for (id, setup) in cases {
        let t = t();
        setup(&t);
        let f = match enable_begin(&t.env(), &t.req()) {
            Err(f) => f,
            Ok(_) => panic!("{id}: enable started"),
        };
        assert_eq!(f.code, "preflight_failed", "{id}");
        let report = f.extra.unwrap()["report"].clone();
        let hit = report.as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(hit["status"], "fail", "{id}: {hit}");
        assert!(t.ops.effects().is_empty(), "{id}: effects {:?}", t.ops.effects());
        assert!(store::load().is_none(), "{id}: no row");
        assert!(!t.latch.load(std::sync::atomic::Ordering::SeqCst), "{id}: latch released");
    }
    // No tarball and nothing installed: the SW1 seam names --tarball.
    let t1 = t();
    let f = enable_begin(&t1.env(), &EnableRequest::default()).err().unwrap();
    let rep = f.extra.unwrap()["report"].clone();
    let src = rep.as_array().unwrap().iter().find(|c| c["id"] == "source").unwrap().clone();
    assert_eq!(src["status"], "fail");
    assert!(src["hint"].as_str().unwrap().contains("--tarball"));
    // Not Linux: refused before anything, supported:false in status.
    drop(t1);
    let mut t2 = t();
    t2.ops.linux = false;
    assert_eq!(enable_begin(&t2.env(), &t2.req()).err().unwrap().code, "unsupported");
    assert_eq!(status(&t2.env(), false)["supported"], false);
}

/// S29 VM cycle: a `k2:k2` or group-writable /usr/local/bin/sew was
/// reported as "not a Sew release this K2 knows"; say what is wrong.
#[test]
fn preflight_says_why_the_helper_refused_the_binary() {
    let t = t();
    *t.ops.on_disk.lock().unwrap() = Some("not-root".into());
    let b = preflight(&t.env(), &t.req()).into_iter().find(|c| c["id"] == "binary").unwrap();
    assert_eq!(b["status"], "fail");
    assert!(b["detail"].as_str().unwrap().contains("not root-owned"), "{b}");
    assert!(b["hint"].as_str().unwrap().contains("root:root"), "{b}");
}

/// S29 VM cycle: on a fresh box the vault check said it "holds Sew's
/// master key" before enable had stored one.
#[test]
fn preflight_vault_check_says_whether_a_key_is_there() {
    let t = t();
    let vault = |t: &T| preflight(&t.env(), &t.req()).into_iter().find(|c| c["id"] == "vault").unwrap();
    let before = vault(&t);
    assert_eq!(before["status"], "ok");
    assert!(before["detail"].as_str().unwrap().contains("is open"), "{before}");
    t.enabled();
    assert!(vault(&t)["detail"].as_str().unwrap().contains("holds Sew's master key"));
}

#[test]
fn voice_ports_warn_and_a_taken_443_means_no_turn_on_443() {
    let t = t();
    t.ops.taken.lock().unwrap().extend([(443, false), (7882, true)]);
    let c = preflight(&t.env(), &t.req());
    let get = |id: &str| c.iter().find(|x| x["id"] == id).unwrap().clone();
    assert_eq!(get("voice_ports")["status"], "warn");
    assert!(get("voice_ports")["detail"].as_str().unwrap().contains("UDP 7882"));
    assert_eq!(get("turn_443")["status"], "info");
    assert!(get("turn_443")["hint"].as_str().unwrap().contains("not offered"));
    assert!(!c.iter().any(|x| x["status"] == "fail"), "voice never blocks chat: {c:?}");
}

#[test]
fn ram_between_4_and_8_gib_warns_and_shows_the_split() {
    let t = t();
    *t.ops.mem.lock().unwrap() = Some(6 * GIB);
    let c = preflight(&t.env(), &t.req());
    let ram = c.iter().find(|x| x["id"] == "ram").unwrap();
    assert_eq!(ram["status"], "warn");
    assert!(ram["detail"].as_str().unwrap().contains("sew 1.0 GiB"), "{ram}");
    let s = memory_split(64 * GIB);
    assert_eq!(s["sewBytes"], 8 * GIB, "sew clamps at 8 GiB");
    assert_eq!(s["sewDbBytes"].as_u64().unwrap(), 64 * GIB / 100 * 15);
}

#[test]
fn a_tarball_that_is_not_the_pin_is_never_staged() {
    let t = t();
    t.ops.files.lock().unwrap().insert(TARBALL.into(), fixture("sew-0.0.1-x86_64-unknown-linux-gnu.tar.gz"));
    let e = t.enable().unwrap_err();
    assert!(e.contains("sha256 mismatch") && e.contains("NOT installing"), "{e}");
    assert!(!t.ops.effects().iter().any(|a| a == "engine sew stage"));
}

#[test]
fn link_refuses_standalone_wrong_version_no_id_and_a_foreign_kek() {
    let key = crate::sew::secrets::generate_master_key().unwrap();
    let pin_v = pinned().version;
    let good = json!({"provider": "k2", "version": format!("{pin_v}+gabc"), "install_id": INSTALL_ID,
                      "keys": {"kek_id": kek_id(&key).unwrap()}});
    assert_eq!(link_check(&good, pin_v, &key, "k2").unwrap(), INSTALL_ID);
    let mut s = good.clone();
    s["provider"] = json!("standalone");
    assert!(link_check(&s, pin_v, &key, "k2").unwrap_err().contains("will not adopt"));
    // A standalone install K2 made itself (enable --identity standalone).
    assert_eq!(link_check(&s, pin_v, &key, "standalone").unwrap(), INSTALL_ID);
    assert!(link_check(&good, pin_v, &key, "standalone").unwrap_err().contains("expected standalone"));
    let mut s = good.clone();
    s["version"] = json!("9.9.9");
    assert!(link_check(&s, pin_v, &key, "k2").is_err());
    let mut s = good.clone();
    s["install_id"] = json!("0");
    assert!(link_check(&s, pin_v, &key, "k2").is_err());
    let mut s = good.clone();
    s["keys"]["kek_id"] = json!("0000000000000001");
    assert!(link_check(&s, pin_v, &key, "k2").unwrap_err().contains("k2_vault_key_mismatch"));
    // In the machine: a foreign KEK fails enable at `link`.
    let t = t();
    t.ops.mgmt_status.lock().unwrap()["keys"]["kek_id"] = json!("0000000000000001");
    assert!(t.enable().unwrap_err().starts_with("link: "));
    assert_eq!(t.row().status, "error");
}

#[test]
fn a_recovery_key_file_adopts_existing_data_and_never_replaces_a_vault_key() {
    let t = t();
    let key = crate::sew::secrets::generate_master_key().unwrap();
    t.ops.files.lock().unwrap().insert("/home/k2/recovery.txt".into(), format!("{key}\n").into_bytes());
    t.ops.exists.lock().unwrap().push("/etc/sew".into());
    let req = EnableRequest {
        tarball_path: Some(TARBALL.into()),
        recovery_key_file: Some("/home/k2/recovery.txt".into()),
        ..Default::default()
    };
    let env = t.env();
    let l = enable_begin(&env, &req).unwrap().unwrap();
    enable_machine(&env, &req).unwrap();
    drop(l);
    assert_eq!(t.master_key(), key);
    // A different file later: refused, the vault key stays.
    let other = crate::sew::secrets::generate_master_key().unwrap();
    t.ops.files.lock().unwrap().insert("/home/k2/recovery.txt".into(), other.into_bytes());
    store::update(1, |r| r.enable_progress = None).unwrap();
    let e = enable_machine(&env, &req).unwrap_err();
    assert!(e.contains("different Sew master key"), "{e}");
    assert_eq!(t.master_key(), key);
}

// ── status / reconcile / health loop ─────────────────────────────────────

fn u(active: &str, ems: i64) -> crate::sew::ops::UnitInfo {
    crate::sew::ops::UnitInfo { active: active.into(), exec_main_status: ems, ..Default::default() }
}

#[test]
fn status_is_never_running_unless_both_units_are_active() {
    let (a, i, f, f78) = (u("active", 0), u("inactive", 0), u("failed", 1), u("failed", 78));
    let r = |row: &str, db: &crate::sew::ops::UnitInfo, sew: &crate::sew::ops::UnitInfo| reconcile(row, db, sew, "running");
    for row in ["installing", "upgrading", "removed"] {
        assert_eq!(r(row, &i, &i).state, row);
        assert!(r(row, &a, &a).consistent);
    }
    assert_eq!((r("disabled", &a, &a).state.as_str(), r("disabled", &a, &a).consistent), ("disabled", false));
    assert!(r("disabled", &i, &i).consistent);
    assert_eq!(r("running", &a, &a).state, "running");
    assert_eq!(reconcile("running", &a, &a, "degraded").state, "degraded");
    assert_eq!((r("running", &i, &a).state.as_str(), r("running", &i, &a).consistent), ("stopped", false));
    assert_eq!(r("running", &f, &a).state, "error");
    assert_eq!(r("degraded", &a, &f78).state, "error");
    assert!(r("degraded", &a, &f78).note.unwrap().contains("exit 78"));
    assert_eq!(r("running", &a, &i).state, "stopped");
    assert_eq!(r("running", &a, &f).state, "error");
    assert_eq!((r("stopped", &a, &a).state.as_str(), r("stopped", &a, &a).consistent), ("running", true));
    assert_eq!((r("error", &a, &a).state.as_str(), r("error", &a, &a).consistent), ("error", false));
    assert!(r("error", &i, &f).consistent);
    for row in ["running", "degraded", "stopped", "error", "disabled", "installing", "upgrading", "removed"] {
        for db in [&a, &i, &f] {
            for sew in [&a, &i, &f, &f78] {
                let s = r(row, db, sew).state;
                if s == "running" || s == "degraded" {
                    assert!(db.is_active() && sew.is_active(), "{row} {db:?} {sew:?} → {s}");
                }
            }
        }
    }
}

#[test]
fn status_envelope_alarms_and_degraded_verdict() {
    let t = t();
    let s = status(&t.env(), false);
    assert_eq!((s["ok"].clone(), s["state"].clone()), (json!(true), json!("not-installed")));
    t.enabled();
    let s = status(&t.env(), false);
    assert_eq!(s["ok"], true, "the envelope");
    // No off-box backups and a recovery key never shown: alarmed (G18).
    let alarms: Vec<String> = serde_json::from_value(s["alarms"].clone()).unwrap();
    assert!(alarms.contains(&"k2_backup_offbox_none".to_string()), "{alarms:?}");
    assert!(alarms.contains(&"k2_recovery_key_not_saved".to_string()));
    assert_eq!(s["state"], "degraded");
    assert!(s.get("health").is_none(), "health only with ?health=1");
    assert_eq!(status(&t.env(), true)["health"]["mgmt"], "ok");
    assert_eq!(status(&t.env(), true)["onDiskVersion"], pinned().version);
    // Clear the K2 alarms: running.
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    assert_eq!(status(&t.env(), false)["state"], "running");
    // A Sew alarm degrades; mgmt down is k2_mgmt_unreachable.
    t.ops.mgmt_status.lock().unwrap()["alarms"] = json!(["backup_stale"]);
    assert_eq!(status(&t.env(), false)["state"], "degraded");
    t.ops.mgmt_status.lock().unwrap()["alarms"] = json!([]);
    *t.ops.mgmt_health_ok.lock().unwrap() = false;
    let s = status(&t.env(), false);
    assert_eq!(s["state"], "degraded");
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_mgmt_unreachable"));
    // A KEK the vault doesn't know.
    *t.ops.mgmt_health_ok.lock().unwrap() = true;
    t.ops.mgmt_status.lock().unwrap()["keys"]["kek_id"] = json!("0000000000000001");
    assert!(status(&t.env(), false)["alarms"].as_array().unwrap().iter().any(|a| a == "k2_vault_key_mismatch"));
}

#[test]
fn the_health_loop_skips_while_latched_and_persists_only_up_down_transitions() {
    let t = t();
    t.enabled();
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    t.ops.set_unit(UNIT_SEW, "inactive", 0);
    {
        let _l = begin(&t.env()).unwrap();
        assert_eq!(health_tick(&t.env()), None, "latched: skipped");
        assert_eq!(t.row().status, "running");
    }
    assert_eq!(health_tick(&t.env()).as_deref(), Some("stopped"));
    t.ops.set_unit(UNIT_SEW, "active", 0);
    assert_eq!(health_tick(&t.env()).as_deref(), Some("running"));
    store::update(1, |r| r.status = "disabled".into()).unwrap();
    assert_eq!(health_tick(&t.env()), None, "never over disabled");
}

#[test]
fn a_second_lifecycle_step_is_409() {
    let t = t();
    t.enabled();
    let _l = begin(&t.env()).unwrap();
    assert_eq!(disable(&t.env()).unwrap_err().code, "lifecycle_in_progress");
    assert_eq!(rollback(&t.env()).unwrap_err().code, "lifecycle_in_progress");
}

// ── disable / uninstall / purge ──────────────────────────────────────────

#[test]
fn disable_and_uninstall_keep_data_and_the_vault_key() {
    let t = t();
    t.enabled();
    let key = t.master_key();
    let r = disable(&t.env()).unwrap();
    assert!(r["warning"].as_str().unwrap().contains("Data, keys and backups are kept"));
    assert_eq!(t.ops.effects(), vec!["engine sew systemctl disable-now"]);
    t.ops.clear();
    let r = uninstall(&t.env(), false).unwrap();
    assert_eq!(r["state"], "removed");
    assert_eq!(t.ops.effects(), vec!["engine sew systemctl disable-now", "engine sew uninstall"]);
    let row = t.row();
    assert_eq!(row.status, "removed");
    assert_eq!(row.install_id.as_deref(), Some(INSTALL_ID));
    assert_eq!(t.master_key(), key);
    assert_eq!(uninstall(&t.env(), false).unwrap()["alreadyRemoved"], true);
    // Enable brings it back: `sew install` runs again with the same key.
    t.ops.clear();
    t.enable().unwrap();
    let eff = t.ops.effects();
    assert!(eff.iter().any(|e| e == "engine sew install"), "{eff:?}");
    assert!(!eff.iter().any(|e| e == "engine sew stage"), "the pinned binary stayed on disk");
    assert_eq!(t.master_key(), key);
    assert_eq!(t.row().status, "running");
}

#[test]
fn purge_checks_the_hostname_first_and_deletes_vault_keys_last() {
    let t = t();
    t.enabled();
    let key_ref = t.row().master_key_ref.unwrap();
    let f = purge(&t.env(), "not-this-box", false).unwrap_err();
    assert_eq!(f.code, "confirm_hostname_mismatch");
    assert!(f.hint.contains("lztek"));
    assert!(t.ops.effects().is_empty(), "nothing before the hostname check");
    // Sew's purge fails: vault untouched, row kept as error.
    t.ops.queue(&format!("engine sew purge {INSTALL_ID}"), Ok(json!({"sewExit": 3, "sew": {"error": {"message": "confirm mismatch"}}})));
    assert_eq!(purge(&t.env(), "lztek", false).unwrap_err().code, "purge_failed");
    assert!(t.secrets.resolve(&key_ref).unwrap().is_some());
    assert_eq!(t.row().status, "error");
    t.ops.clear();
    let r = purge(&t.env(), "lztek", true).unwrap();
    assert_eq!(r["purged"], true);
    // sew-db stays up so Sew can read the install id it checks the
    // typed-back id against (S29 VM cycle: stopping it first made every
    // purge a mismatch).
    assert_eq!(t.ops.effects(), vec![format!("engine sew purge {INSTALL_ID} keep-backups")]);
    assert_eq!(r["backupsAt"], "/var/lib/sew-backups-20260103T000000Z", "where Sew moved the kept backups");
    assert!(t.secrets.resolve(&key_ref).unwrap().is_none(), "vault key gone");
    assert!(store::load().is_none(), "not installed");
}

/// After `uninstall` sew-db is down, so Sew reads the install id as `none`
/// and only `--yes-delete-all-data=none` matches (Sew's contract).
#[test]
fn purge_after_uninstall_types_back_none() {
    let t = t();
    t.enabled();
    uninstall(&t.env(), false).unwrap();
    assert!(!t.ops.unit(pin::UNIT_DB).is_active());
    t.ops.clear();
    let r = purge(&t.env(), "lztek", false).unwrap();
    assert_eq!(r["purged"], true);
    assert_eq!(t.ops.effects(), vec!["engine sew purge none".to_string()]);
    assert!(r["backupsAt"].is_null());
    assert!(store::load().is_none());
}

// ── upgrade / rollback ───────────────────────────────────────────────────

fn on_old_release(t: &T) {
    t.enabled();
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    store::update(1, |r| r.installed_version = Some("0.0.1".into())).unwrap();
    t.ops.clear();
}

fn upgrade(t: &T) -> Value {
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), dry_run: false };
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &req).unwrap().unwrap();
    let p = upgrade_machine(&env, &req, &from);
    drop(l);
    p
}

#[test]
fn upgrade_preconditions_refuse_without_effects() {
    let t = t();
    t.enabled();
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), dry_run: false };
    match upgrade_begin(&t.env(), &req).unwrap() {
        Err(v) => assert_eq!(v["alreadyCurrent"], true),
        Ok(_) => panic!("same version must be a no-op"),
    }
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    assert_eq!(upgrade_begin(&t.env(), &req).err().unwrap().code, "unknown_sew_binary");
    *t.ops.on_disk.lock().unwrap() = Some("9.0.0".into());
    assert_eq!(upgrade_begin(&t.env(), &req).err().unwrap().code, "downgrade_refused");
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    assert_eq!(upgrade_begin(&t.env(), &UpgradeRequest::default()).err().unwrap().code, "release_source_unset");
    *t.ops.helper.lock().unwrap() = helper_ok(7, true, "0.0.1");
    assert_eq!(upgrade_begin(&t.env(), &req).err().unwrap().code, "helper_outdated");
    *t.ops.helper.lock().unwrap() = helper_ok(7, true, pinned().version);
    let dry = upgrade_begin(&t.env(), &UpgradeRequest { dry_run: true, ..req.clone() }).unwrap().err().unwrap();
    assert_eq!(dry["dryRun"], true);
    assert!(t.ops.effects().is_empty());
    assert_eq!(t.row().status, "running");
}

#[test]
fn upgrade_success_records_the_pin_and_the_previous_version() {
    let t = t();
    on_old_release(&t);
    let p = upgrade(&t);
    assert_eq!(p["result"], "done");
    assert_eq!(t.ops.effects(), vec!["engine sew stage", "engine sew upgrade", "engine sew token", "engine sew unstage"]);
    let row = t.row();
    assert_eq!((row.status.as_str(), row.installed_version.as_deref(), row.previous_version.as_deref()), ("running", Some(pinned().version), Some("0.0.1")));
}

#[test]
fn upgrade_rolls_back_automatically_when_k2s_health_check_fails() {
    let t = t();
    on_old_release(&t);
    // Sew says upgraded, but the new binary never gets healthy; the
    // rollback brings the old one back healthy.
    *t.ops.mgmt_health_ok.lock().unwrap() = false;
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), dry_run: false };
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &req).unwrap().unwrap();
    let p = upgrade_machine_with_flip(&env, &req, &from, &t.ops);
    drop(l);
    assert_eq!(p["result"], "rolled_back", "{p}");
    let eff = t.ops.effects();
    assert!(eff.iter().any(|e| e == "engine sew rollback"), "{eff:?}");
    let row = t.row();
    assert_eq!(row.installed_version.as_deref(), Some("0.0.1"));
    assert_eq!(row.status, "running");
    assert!(row.last_error.unwrap().contains("rolled back to 0.0.1"));
}

/// Run the upgrade machine, restoring health (and the old version on disk)
/// as soon as the machine asks the helper to roll back.
fn upgrade_machine_with_flip(env: &Env, req: &UpgradeRequest, from: &str, ops: &FakeOps) -> Value {
    struct Watch<'a> {
        inner: &'a FakeOps,
    }
    impl SewOps for Watch<'_> {
        fn linux(&self) -> bool { self.inner.linux() }
        fn arch(&self) -> String { self.inner.arch() }
        fn helper_state(&self) -> crate::root_helper::RootHelperState { self.inner.helper_state() }
        fn helper(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Value, String> {
            let r = self.inner.helper(argv, stdin);
            if argv.join(" ") == "engine sew rollback" {
                *self.inner.mgmt_health_ok.lock().unwrap() = true;
                *self.inner.on_disk.lock().unwrap() = Some("0.0.1".into());
            }
            r
        }
        fn unit(&self, u: &str) -> crate::sew::ops::UnitInfo { self.inner.unit(u) }
        fn read_file(&self, p: &std::path::Path, c: usize) -> Result<Vec<u8>, String> { self.inner.read_file(p, c) }
        fn path_exists(&self, p: &str) -> bool { self.inner.path_exists(p) }
        fn systemd_version(&self) -> Option<u32> { self.inner.systemd_version() }
        fn pg_majors(&self) -> Vec<u32> { self.inner.pg_majors() }
        fn mem_total(&self) -> Option<u64> { self.inner.mem_total() }
        fn disk_free(&self, p: &str) -> Option<u64> { self.inner.disk_free(p) }
        fn port_taken(&self, a: &str, p: u16, u: bool) -> bool { self.inner.port_taken(a, p, u) }
        fn daemon_port(&self) -> Option<u16> { self.inner.daemon_port() }
        fn hostname(&self) -> String { self.inner.hostname() }
        fn mgmt(&self, m: &str, p: &str, t: &str, b: Option<&Value>) -> Result<Value, crate::sew::ops::MgmtError> { self.inner.mgmt(m, p, t, b) }
        fn now(&self) -> i64 { self.inner.now() }
        fn sleep_ms(&self, ms: u64) { self.inner.sleep_ms(ms) }
        fn box_label(&self) -> Option<String> { self.inner.box_label() }
        fn bound_apexes(&self) -> Vec<String> { self.inner.bound_apexes() }
        fn public_ips(&self) -> Vec<String> { self.inner.public_ips() }
        fn cp(&self, m: &str, p: &str, b: Option<&Value>) -> Result<Value, crate::sew::ops::CpError> { self.inner.cp(m, p, b) }
        fn nested_label(&self, l: &str, t: Option<&str>) -> Result<(), String> { self.inner.nested_label(l, t) }
        fn served_leaf(&self, c: &str, s: &str, p: u16) -> Option<String> { self.inner.served_leaf(c, s, p) }
        fn frpc_logins(&self) -> u64 { self.inner.frpc_logins() }
        fn voice_load(&self) -> Option<Value> { self.inner.voice_load() }
        fn voice_save(&self, v: Option<&Value>) -> Result<(), String> { self.inner.voice_save(v) }
    }
    let w = Watch { inner: ops };
    let env2 = Env { ops: &w, secrets: env.secrets, latch: env.latch, token: env.token };
    upgrade_machine(&env2, req, from)
}

#[test]
fn upgrade_rollback_failure_is_an_error_with_an_alarm() {
    let t = t();
    on_old_release(&t);
    *t.ops.mgmt_health_ok.lock().unwrap() = false;
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 2, "sew": {"error": {"message": "ROLLBACK FAILED"}}})));
    let p = upgrade(&t);
    assert_eq!(p["result"], "rollback_failed");
    let row = t.row();
    assert_eq!(row.status, "error");
    assert!(row.last_error.as_deref().unwrap().contains("A person must act"));
    *t.ops.mgmt_health_ok.lock().unwrap() = true;
    store::update(1, |r| r.status = "running".into()).unwrap();
    assert!(status(&t.env(), false)["alarms"].as_array().unwrap().iter().any(|a| a == "k2_upgrade_rollback_failed"));
}

#[test]
fn upgrade_maps_sews_exit_codes() {
    // 1: Sew rolled back itself.
    let t = t();
    on_old_release(&t);
    t.ops.queue("engine sew upgrade", Ok(json!({"sewExit": 1, "sew": {"error": {"message": "migration failed"}}})));
    assert_eq!(upgrade(&t)["result"], "rolled_back");
    let row = t.row();
    assert_eq!(row.installed_version.as_deref(), Some("0.0.1"));
    assert!(row.last_error.unwrap().contains("Sew rolled back to 0.0.1: migration failed"));
    assert!(!t.ops.effects().iter().any(|e| e == "engine sew rollback"), "Sew already rolled back");
    // 2: rollback failed inside Sew.
    let t2 = t2_old();
    t2.ops.queue("engine sew upgrade", Ok(json!({"sewExit": 2, "sew": {"error": {"message": "x"}}})));
    assert_eq!(upgrade(&t2)["result"], "rollback_failed");
    assert_eq!(t2.row().status, "error");
    assert!(t2.row().last_error.unwrap().contains(SEW_UPGRADE_STATE));
    // 3: refused; the prior state comes back.
    let t3 = t2_old();
    t3.ops.queue("engine sew upgrade", Ok(json!({"sewExit": 3, "sew": {"error": {"message": "unknown migration"}}})));
    assert_eq!(upgrade(&t3)["result"], "refused");
    assert_eq!(t3.row().status, "running");
}

fn t2_old() -> T {
    let t = t();
    on_old_release(&t);
    t
}

#[test]
fn an_upgrade_interrupted_by_a_daemon_restart_finishes_or_rolls_back_at_boot() {
    let t = t();
    on_old_release(&t);
    store::update(1, |r| {
        r.status = "upgrading".into();
        r.upgrade_progress = Some(json!({"from": "0.0.1", "to": pinned().version, "current": "upgrade", "steps": {}}));
    })
    .unwrap();
    // Sew finished the swap before the daemon died: healthy on the pin.
    *t.ops.on_disk.lock().unwrap() = Some(pinned().version.into());
    assert_eq!(recover_interrupted_upgrade(&t.env())["result"], "done");
    assert_eq!(t.row().installed_version.as_deref(), Some(pinned().version));
    assert_eq!(t.row().status, "running");
}

#[test]
fn manual_rollback_never_passes_a_data_loss_flag_and_explains_a_schema_refusal() {
    let t = t();
    t.enabled();
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 3, "sew": {"error": {"message": "schema changed since 0.0.1"}}})));
    let f = rollback(&t.env()).unwrap_err();
    assert_eq!(f.code, "rollback_refused");
    assert!(f.hint.contains("backup restore"));
    assert!(t.ops.argvs().iter().all(|a| !a.contains("accept-data-loss")));
    // Success: the version Sew reports is recorded.
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 0, "sew": {"state": "rolled_back", "to": "0.0.1"}})));
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    let r = rollback(&t.env()).unwrap();
    assert_eq!(r["state"], "running");
    assert_eq!(t.row().installed_version.as_deref(), Some("0.0.1"));
}

// ── backups / restore ────────────────────────────────────────────────────

fn queue_mgmt(t: &T, key: &str, a: Result<Value, crate::sew::ops::MgmtError>) {
    t.ops.mgmt_answers.lock().unwrap().entry(key.to_string()).or_default().push_back(a);
}

#[test]
fn backups_list_and_run_over_mgmt_not_the_helper() {
    use crate::sew::ops::MgmtError;
    let t = t();
    t.enabled();
    let b = backups(&t.env()).unwrap();
    assert_eq!(b["backups"].as_array().unwrap().len(), 2);
    assert_eq!(b["latest"]["id"], "20260102T031500Z", "Sew lists newest first");
    assert_eq!(b["backups"][0]["schema_version"], 920002);
    let r = backup_run(&t.env()).unwrap();
    assert_eq!(r["ok"], true);
    let id = r["backup"]["id"].as_str().unwrap().to_string();
    assert_eq!(r["jobId"], format!("bk_{id}"));
    assert_eq!(backups(&t.env()).unwrap()["latest"]["id"], id.as_str());
    let calls = t.ops.mgmt_calls.lock().unwrap().clone();
    assert!(calls.contains(&"POST /mgmt/v1/backup".to_string()), "{calls:?}");
    assert!(calls.contains(&format!("GET /mgmt/v1/jobs/bk_{id}")), "{calls:?}");
    assert!(t.ops.effects().iter().all(|a| !a.contains("backup")), "no root helper for backups: {:?}", t.ops.effects());

    // 409 backup_running (the timer or a CLI backup holds the lock).
    queue_mgmt(&t, "POST /mgmt/v1/backup", Err(MgmtError::Status(409, r#"{"error":{"kind":"conflict","reason":"backup_running"}}"#.into())));
    assert_eq!(backup_run(&t.env()).unwrap_err().code, "backup_busy");
    // A job that loses the race fails with that reason.
    queue_mgmt(&t, "POST /mgmt/v1/backup", Ok(json!({"job_id": "77"})));
    queue_mgmt(
        &t,
        "GET /mgmt/v1/jobs/77",
        Ok(json!({"id": "77", "kind": "backup", "state": "dead", "attempts": 1, "last_error": "backup_running", "created_at": "x"})),
    );
    assert_eq!(backup_run(&t.env()).unwrap_err().code, "backup_busy");
    // A dead job is a failure with Sew's words; a running one is polled.
    queue_mgmt(&t, "POST /mgmt/v1/backup", Ok(json!({"job_id": "78"})));
    for st in ["queued", "running"] {
        queue_mgmt(&t, "GET /mgmt/v1/jobs/78", Ok(json!({"id": "78", "kind": "backup", "state": st, "attempts": 1, "created_at": "x"})));
    }
    queue_mgmt(
        &t,
        "GET /mgmt/v1/jobs/78",
        Ok(json!({"id": "78", "kind": "backup", "state": "dead", "attempts": 3, "last_error": "pg_dump: disk full", "created_at": "x"})),
    );
    let f = backup_run(&t.env()).unwrap_err();
    assert_eq!(f.code, "backup_failed");
    assert!(f.hint.contains("disk full"), "{}", f.hint);
    // No backup engine.
    queue_mgmt(&t, "POST /mgmt/v1/backup", Err(MgmtError::Status(503, r#"{"error":{"reason":"backups_unavailable"}}"#.into())));
    assert_eq!(backup_run(&t.env()).unwrap_err().code, "backups_unavailable");
    // Sew down: the list needs the mgmt socket.
    t.ops.set_unit(pin::UNIT_SEW, "inactive", 0);
    assert_eq!(backups(&t.env()).unwrap_err().code, "mgmt_unreachable");
}

/// Sew answer 2026-10-16: `keys_mirror_stale` with `keys.mirror.kind ==
/// "none"` is intended (no off-box key copy yet): a setup warning, never
/// `degraded`. A stale dir mirror is broken and stays an alarm.
#[test]
fn a_missing_key_mirror_is_a_setup_warning_but_a_stale_dir_mirror_degrades() {
    let t = t();
    t.enabled();
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    assert_eq!(status(&t.env(), false)["state"], "running");
    // kind none + keys_mirror_stale: running, warned, not alarmed.
    {
        let mut st = t.ops.mgmt_status.lock().unwrap();
        st["alarms"] = json!(["keys_mirror_stale"]);
        st["keys"]["mirror"] = json!({"kind": "none", "stale": true, "pending": 0});
    }
    let s = status(&t.env(), false);
    assert_eq!(s["state"], "running", "{s}");
    assert!(!s["alarms"].as_array().unwrap().iter().any(|a| a == "keys_mirror_stale"), "{}", s["alarms"]);
    assert_eq!(s["warnings"][0]["code"], "no_offbox_key_copy");
    assert!(s["warnings"][0]["message"].as_str().unwrap().starts_with("no off-box key copy"));
    // The same alarm with a dir mirror: broken, degrades, no warning.
    t.ops.mgmt_status.lock().unwrap()["keys"]["mirror"] = json!({"kind": "dir", "stale": true, "pending": 3});
    let s = status(&t.env(), false);
    assert_eq!(s["state"], "degraded");
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "keys_mirror_stale"));
    assert_eq!(s["warnings"], json!([]));
    // Another Sew alarm next to a missing mirror still degrades.
    {
        let mut st = t.ops.mgmt_status.lock().unwrap();
        st["keys"]["mirror"] = json!({"kind": "none", "stale": true});
        st["alarms"] = json!(["keys_mirror_stale", "backup_failed"]);
    }
    let s = status(&t.env(), false);
    assert_eq!(s["state"], "degraded");
    assert_eq!(s["alarms"].as_array().unwrap().iter().filter(|a| *a == "backup_failed").count(), 1);
    assert_eq!(s["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn status_carries_sews_backup_and_drill_state() {
    let t = t();
    t.enabled();
    let s = status(&t.env(), false);
    assert_eq!(s["backup"]["ship"]["kind"], "none");
    assert!(s["backup"]["lastOkAt"].is_null(), "Sew has not reported one");
    t.ops.mgmt_status.lock().unwrap()["backup"] =
        json!({"last_ok_at": "2026-10-16T12:00:00Z", "last_id": "20261016T120000Z", "age_ms": 5000, "last_kind": "manual"});
    t.ops.mgmt_status.lock().unwrap()["drill"] = json!({"last_ok": false, "last_error": "restore drill: checksum", "age_ms": 1});
    t.ops.mgmt_status.lock().unwrap()["alarms"] = json!(["drill_failed", "backup_stale"]);
    let s = status(&t.env(), false);
    assert_eq!(s["backup"]["lastId"], "20261016T120000Z");
    assert_eq!(s["backup"]["lastKind"], "manual");
    assert_eq!(s["backup"]["ageMs"], 5000);
    assert_eq!(s["backup"]["alarms"], json!(["backup_stale"]));
    assert_eq!(s["drill"]["lastOk"], false);
    assert_eq!(s["drill"]["alarms"], json!(["drill_failed"]));
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "drill_failed"));
    assert_eq!(s["state"], "degraded");
}

#[test]
fn restore_relies_on_sews_safety_bundle_and_never_takes_a_manual_backup() {
    let t = t();
    t.enabled();
    assert_eq!(restore(&t.env(), "20990101T000000Z").unwrap_err().code, "bundle_not_found");
    assert_eq!(restore(&t.env(), "../etc").unwrap_err().code, "bad_request");
    t.ops.clear();
    let r = restore(&t.env(), "20260101T031500Z").unwrap();
    assert_eq!(r["restored"], true);
    assert_eq!(r["safetyBundle"], "20260103T000000Z-safety");
    // S29 VM cycle: a `manual` mgmt backup before the restore made Sew's
    // retention (newest per day) delete the same-day target bundle. K2
    // makes none; Sew's `pre-restore` safety bundle is the backup.
    assert!(
        !t.ops.mgmt_calls.lock().unwrap().iter().any(|c| c == "POST /mgmt/v1/backup"),
        "{:?}",
        t.ops.mgmt_calls.lock().unwrap()
    );
    assert_eq!(t.ops.effects().iter().filter(|e| e.starts_with("engine sew restore")).count(), 1);
    assert_eq!(backups(&t.env()).unwrap()["backups"].as_array().unwrap().len(), 2, "no bundle made or pruned by K2");
    assert_eq!(t.row().restore_progress.unwrap()["result"], "done");
    // A Sew refusal (its safety backup failed, or no such bundle) is data:
    // nothing was replaced, and Sew keeps serving.
    t.ops.queue(
        "engine sew restore 20260101T031500Z",
        Ok(json!({"sewExit": 4, "sew": {"error": {"message": "the safety backup (sew-backup@pre-restore) failed"}}, "restarted": true})),
    );
    let f = restore(&t.env(), "20260101T031500Z").unwrap_err();
    assert_eq!(f.code, "restore_failed");
    assert!(f.hint.contains("safety backup") && f.hint.contains("not replaced"), "{}", f.hint);
    assert_eq!(t.row().status, "running");
    // Exit 1 can come after Sew dropped the schema (S29 VM cycle): never
    // "not replaced"; point at the safety bundle.
    t.ops.queue(
        "engine sew restore 20260101T031500Z",
        Ok(json!({"sewExit": 1, "sew": {"error": {"message": "pg_restore failed (exit status: 1); schema sew is now empty"}}, "restarted": false})),
    );
    t.ops.set_unit(pin::UNIT_SEW, "failed", 1);
    let f = restore(&t.env(), "20260101T031500Z").unwrap_err();
    assert!(!f.hint.contains("not replaced"), "{}", f.hint);
    assert!(f.hint.contains("safety") && f.hint.contains("may be changed or empty"), "{}", f.hint);
    assert_eq!(t.row().status, "error");
}

/// S29 VM cycle: a failed restore left Sew's schema empty and sew.service
/// down; restore must still work then (mgmt is unreachable, so Sew checks
/// the bundle id itself).
#[test]
fn restore_works_on_a_stopped_server_without_the_mgmt_list() {
    let t = t();
    t.enabled();
    t.ops.set_unit(pin::UNIT_SEW, "failed", 1);
    store::update(1, |r| r.status = "stopped".into()).unwrap();
    assert_eq!(backups(&t.env()).unwrap_err().code, "mgmt_unreachable");
    t.ops.clear();
    let r = restore(&t.env(), "20261016T154452Z").unwrap();
    assert_eq!(r["restored"], true);
    // The restore, then the health check's token refetch; nothing else.
    let eff: Vec<String> = t.ops.effects().into_iter().filter(|e| e != "engine sew token").collect();
    assert_eq!(eff, vec!["engine sew restore 20261016T154452Z".to_string()]);
    assert_eq!(t.row().status, "running");
    // Never into a removed install.
    store::update(1, |r| r.status = "removed".into()).unwrap();
    assert_eq!(restore(&t.env(), "20261016T154452Z").unwrap_err().code, "not_installed");
}

// ── keys ─────────────────────────────────────────────────────────────────

#[test]
fn rotation_swaps_refs_resets_the_recovery_alarm_and_keeps_the_key_on_stdin() {
    let t = t();
    t.enabled();
    store::update(1, |r| r.recovery_shown_at = Some(5)).unwrap();
    let old_ref = t.row().master_key_ref.unwrap();
    let r = rotate_start(&t.env()).unwrap();
    assert_eq!(r["jobId"], "job_rot1");
    let row = t.row();
    assert_eq!(row.master_prev_ref.as_deref(), Some(old_ref.as_str()));
    assert!(row.master_next_ref.is_none());
    assert_ne!(row.master_key_ref.as_deref(), Some(old_ref.as_str()));
    assert!(row.recovery_shown_at.is_none(), "the new key must be saved again");
    let new_key = t.master_key();
    assert_eq!(r["kekId"], kek_id(&new_key).unwrap());
    let call = t.ops.calls.lock().unwrap().iter().find(|c| c.argv == "engine sew rotate-master").cloned().unwrap();
    assert_eq!(call.stdin.unwrap(), format!("{new_key}\n").into_bytes());
    assert!(t.ops.mgmt_calls.lock().unwrap().iter().any(|c| c == "POST /mgmt/v1/keys/rotate-master"));
    // A second start while the old key is loaded: refused.
    assert_eq!(rotate_start(&t.env()).unwrap_err().code, "rotation_in_progress");
    // drop-prev waits for the rewrap.
    t.ops.mgmt_status.lock().unwrap()["keys"]["rewrap_pending"] = json!(12);
    assert_eq!(rotate_drop_prev(&t.env()).unwrap_err().code, "rewrap_pending");
    assert!(t.secrets.resolve(&old_ref).unwrap().is_some());
    t.ops.mgmt_status.lock().unwrap()["keys"]["rewrap_pending"] = json!(0);
    rotate_drop_prev(&t.env()).unwrap();
    assert!(t.secrets.resolve(&old_ref).unwrap().is_none(), "the old key leaves the vault");
    assert!(t.row().master_prev_ref.is_none());
}

#[test]
fn the_recovery_key_is_the_vault_key_and_marks_it_saved() {
    let t = t();
    t.enabled();
    let r = recovery_key(&t.env()).unwrap();
    assert_eq!(r["recoveryKey"], t.master_key());
    assert!(r["warning"].as_str().unwrap().contains("only way back"));
    assert!(t.row().recovery_shown_at.is_some());
}

#[test]
fn doctor_stores_its_report() {
    let t = t();
    t.enabled();
    let r = doctor(&t.env()).unwrap();
    assert_eq!(r["report"]["ok"], true);
    assert!(t.row().doctor.is_some());
}

#[test]
fn a_stale_mgmt_token_is_refetched_once() {
    let t = t();
    t.enabled();
    *t.token.lock().unwrap() = Some("sewsvc_stale".into());
    t.ops.clear();
    mgmt(&t.env(), "GET", "/mgmt/v1/health", None).unwrap();
    assert_eq!(t.ops.effects(), vec!["engine sew token"]);
}
