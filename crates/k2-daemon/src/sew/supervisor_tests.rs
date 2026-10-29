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

/// 0.46.1: switching identity on an existing install names the real
/// verb, `k2 hostchat purge` (there is no `uninstall --purge`).
#[test]
fn identity_change_names_the_real_purge_verb() {
    let t = t();
    t.enabled();
    store::update(1, |r| r.status = "stopped".into()).expect("update").expect("row");
    let req = EnableRequest { provider: Some("standalone".into()), ..t.req() };
    let f = match enable_begin(&t.env(), &req) {
        Err(f) => f,
        Ok(_) => panic!("identity change must refuse"),
    };
    assert_eq!(f.code, "provider_fixed");
    assert!(f.hint.contains("k2 hostchat purge --confirm-hostname"), "{}", f.hint);
    assert!(!f.hint.contains("uninstall --purge"), "{}", f.hint);
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
}

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

/// 0.46.6: Sew's App kit beside the release tarball is verified against
/// its pin and kept for `k2 app sdk add --sew`; none beside it is fine.
#[test]
fn enable_keeps_the_app_kit_beside_the_tarball() {
    let _home = crate::test_support::TempHome::new();
    let t = t();
    t.ops.files.lock().unwrap().insert("/home/k2/sew-app-kit-0.0.2.tar.gz".into(), fixture("sew-app-kit-0.0.2.tar.gz"));
    t.enable().unwrap();
    let kit = pin::app_kit(pinned().version).expect("the test pin has a kit");
    let kept = k2_core::app_sdk::load_kit(kit.version, kit.sha256).unwrap().expect("the kit is kept");
    assert_eq!(kept.packages.len(), 5);
}

#[test]
fn enable_without_an_app_kit_still_installs() {
    let _home = crate::test_support::TempHome::new();
    let t = t();
    t.enable().unwrap();
    let kit = pin::app_kit(pinned().version).unwrap();
    assert!(k2_core::app_sdk::load_kit(kit.version, kit.sha256).unwrap().is_none());
}

#[test]
fn a_tampered_app_kit_stops_the_install_before_any_change() {
    let _home = crate::test_support::TempHome::new();
    let t = t();
    let mut bad = fixture("sew-app-kit-0.0.2.tar.gz");
    bad[100] ^= 0xff;
    t.ops.files.lock().unwrap().insert("/home/k2/sew-app-kit-0.0.2.tar.gz".into(), bad);
    let e = t.enable().unwrap_err();
    assert!(e.contains("is not Sew's App kit") || t.row().last_error.as_deref().unwrap_or("").contains("is not Sew's App kit"), "{e}");
    assert!(!t.ops.effects().iter().any(|c| c == "engine sew stage"), "nothing staged: {:?}", t.ops.effects());
    let kit = pin::app_kit(pinned().version).unwrap();
    assert!(k2_core::app_sdk::load_kit(kit.version, kit.sha256).unwrap().is_none());
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

/// The sewlab login: bash, home under /home.
const SEWLAB_LOGIN: &str = "sew:x:1002:1002::/home/sew:/bin/bash\n";
/// The account a previous Sew install left: nologin, no keys.
const SEW_SERVICE: &str = "sew:x:998:998:Sew:/var/lib/sew:/usr/sbin/nologin\n";

fn add_passwd(t: &T, line: &str) {
    let mut files = t.ops.files.lock().unwrap();
    let pw = files.get_mut(crate::sew::service_user::PASSWD).expect("the fake box has /etc/passwd");
    pw.extend_from_slice(line.as_bytes());
}

/// Re-enable over a previous install's service account works: the guard
/// only refuses login accounts (shadow and /var/lib/sew unreadable, as for
/// the unprivileged daemon).
#[test]
fn re_enable_with_sews_own_service_account_passes_the_guard() {
    let t = t();
    t.enabled();
    disable(&t.env()).unwrap();
    add_passwd(&t, SEW_SERVICE);
    t.ops.unreadable.lock().unwrap().push("/var/lib/sew".into());
    t.ops.clear();
    let c = preflight(&t.env(), &t.req()).into_iter().find(|c| c["id"] == "service_user").expect("service_user check");
    assert_eq!(c["status"], "ok", "{c}");
    t.enable().expect("re-enable");
    assert_eq!(t.row().status, "running");
    // Root-readable shadow with a locked password: same answer.
    t.ops.files.lock().unwrap().insert(crate::sew::service_user::SHADOW.into(), b"sew:!:20000::::::\n".to_vec());
    let c = preflight(&t.env(), &t.req()).into_iter().find(|c| c["id"] == "service_user").expect("service_user check");
    assert_eq!(c["status"], "ok", "{c}");
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
        ("postgres", Box::new(|t: &T| *t.ops.pg.lock().unwrap() = vec![17])),
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
        // sewlab 2026-10-16: a `sew` login made before enable.
        ("service_user", Box::new(|t: &T| add_passwd(t, SEWLAB_LOGIN))),
        ("service_user", Box::new(|t: &T| {
            t.ops.files.lock().unwrap().remove(crate::sew::service_user::PASSWD).expect("passwd");
        })),
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
        // 0.46.1: hints name only things that exist (no `--with-sew`).
        assert!(!hit["hint"].as_str().unwrap_or("").contains("--with-sew"), "{id}: {hit}");
        assert!(t.ops.effects().is_empty(), "{id}: effects {:?}", t.ops.effects());
        assert!(store::load().is_none(), "{id}: no row");
        assert!(!t.latch.load(std::sync::atomic::Ordering::SeqCst), "{id}: latch released");
    }
    // The refusal says why in words, not only in the report.
    let t0 = t();
    add_passwd(&t0, SEWLAB_LOGIN);
    t0.ops.exists.lock().unwrap().push("/home/sew/.ssh/authorized_keys".into());
    let f = enable_begin(&t0.env(), &t0.req()).err().expect("refused");
    assert_eq!((f.code, f.status), ("preflight_failed", 409));
    assert_eq!(
        f.hint,
        "the box is not ready for Sew; nothing was changed. A login user named 'sew' already exists on this box \
         (shell /bin/bash, SSH keys present). Sew would adopt it as its service account and give that login full \
         Sew admin. Rename or remove that user, then run enable again."
    );
    assert!(t0.ops.effects().is_empty(), "effects {:?}", t0.ops.effects());
    assert!(t0.ops.calls.lock().unwrap().iter().all(|c| c.argv == "engine sew status"), "only the read-only status call");
    drop(t0);
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

/// Sew 0.3.1 readiness (`/api/v1/health`) shows as `apiHealth`; not
/// ready is a `sew_not_ready_<reason>` alarm (degraded), ready adds none.
#[test]
fn status_shows_sews_readiness_and_alarms_when_not_ready() {
    let t = t();
    t.enabled();
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    let s = status(&t.env(), false);
    assert_eq!((s["state"].clone(), s["apiHealth"]["ready"].clone()), (json!("running"), json!(true)), "{s}");
    t.ops.queue_api_health(vec![Ok((503, json!({"ok": false, "ready": false, "reason": "upgrading", "since": 7})))]);
    let s = status(&t.env(), false);
    assert_eq!(s["state"], "degraded", "{s}");
    assert_eq!((s["apiHealth"]["reason"].clone(), s["apiHealth"]["since"].clone()), (json!("upgrading"), json!(7)));
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "sew_not_ready_upgrading"), "{s}");
    t.ops.queue_api_health(vec![Err("connection refused".into())]);
    let s = status(&t.env(), false);
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "sew_not_ready_down"), "{s}");
    assert_eq!(status(&t.env(), false)["state"], "running", "back to ready");
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
    assert_eq!(r["teardown"], "sew", "a known binary: Sew's own purge, unchanged");
    assert!(r.get("note").is_none());
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

// ── a Sew this K2 does not recognise (sewlab, 0.46.3) ────────────────────

/// The box holds a Sew no pin vouches for (an old dev K2's 0.1.2 test
/// build). Purge goes through: the helper tears it down itself, and the
/// daemon finishes exactly as after Sew's own purge.
#[test]
fn purge_of_an_unrecognised_sew_is_k2s_teardown_and_finishes_the_cleanup() {
    let t = t();
    t.enabled();
    let key_ref = t.row().master_key_ref.unwrap();
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    let r = purge(&t.env(), "lztek", false).unwrap();
    assert_eq!(r["purged"], true);
    assert_eq!(r["teardown"], "k2");
    assert_eq!(r["binary"]["executed"], false);
    assert!(r["note"].as_str().unwrap().contains("never ran that binary"), "{r}");
    assert!(r["left"].to_string().contains("sew-master-key.recovery"), "{r}");
    assert_eq!(r["accounts"][0]["loginLike"], true);
    assert_eq!(t.ops.effects(), vec![format!("engine sew purge {INSTALL_ID}")], "one helper call, the purge verb");
    assert!(t.secrets.resolve(&key_ref).unwrap().is_none(), "vault key gone");
    assert!(store::load().is_none(), "no row left, never `error`");
    // A fresh enable starts clean from here.
    t.ops.clear();
    t.enable().unwrap();
    assert!(t.ops.effects().iter().any(|e| e == "engine sew install-bin"), "{:?}", t.ops.effects());
    assert_eq!(t.row().status, "running");
}

/// With `--keep-backups`, K2's teardown says where the backups went, as
/// Sew's does.
#[test]
fn k2_teardown_reports_where_kept_backups_went() {
    let t = t();
    t.enabled();
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    let r = purge(&t.env(), "lztek", true).unwrap();
    assert_eq!(r["teardown"], "k2");
    assert_eq!(r["backupsAt"], "/var/lib/sew-backups-20260103T000000Z");
}

/// A root helper from before 0.46.3 still refuses: the vault and the row
/// stay (resumable), and the answer says to update the helper.
#[test]
fn an_older_helper_refusing_an_unknown_binary_points_at_a_helper_update() {
    let t = t();
    t.enabled();
    let key_ref = t.row().master_key_ref.unwrap();
    t.ops.queue(
        &format!("engine sew purge {INSTALL_ID}"),
        Err("root helper engine sew purge: exit 3: sew helper: unknown sew binary".into()),
    );
    let f = purge(&t.env(), "lztek", false).unwrap_err();
    assert_eq!(f.code, "purge_failed");
    assert!(f.hint.contains("k2 root-helper update"), "{}", f.hint);
    assert!(t.secrets.resolve(&key_ref).unwrap().is_some(), "vault key kept");
    assert_eq!(t.row().status, "error");
}

/// Keep-data uninstall of an unrecognised Sew stays refused (only that Sew
/// knows what of its layout is data), but the row says what is true: Sew
/// is stopped, not `error`, and the way out is named.
#[test]
fn uninstall_of_an_unrecognised_sew_refuses_and_records_it_stopped() {
    let t = t();
    t.enabled();
    let key = t.master_key();
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    let f = uninstall(&t.env(), false).unwrap_err();
    assert_eq!(f.code, "unknown_sew_binary");
    assert!(f.hint.contains("k2 hostchat purge"), "{}", f.hint);
    assert_eq!(t.ops.effects(), vec!["engine sew systemctl disable-now", "engine sew uninstall"]);
    let row = t.row();
    assert_eq!(row.status, "disabled");
    assert!(row.last_error.as_deref().unwrap().contains("not a Sew release this K2 knows"));
    assert_eq!(row.install_id.as_deref(), Some(INSTALL_ID), "nothing forgotten");
    assert_eq!(t.master_key(), key, "vault key kept");
    // A failed stop is not dressed up as `disabled`.
    store::update(1, |r| r.status = "running".into()).unwrap();
    t.ops.queue("engine sew systemctl disable-now", Err("root helper engine sew systemctl disable-now: exit 5: boom".into()));
    assert_eq!(uninstall(&t.env(), false).unwrap_err().code, "unknown_sew_binary");
    assert_eq!(t.row().status, "error");
}

// ── S27's grant hook follows the install ─────────────────────────────────

fn hook_calls(t: &T) -> Vec<String> {
    t.ops.hook_calls.lock().unwrap().clone()
}

#[test]
fn enable_points_the_grant_hook_at_sew_and_records_it() {
    let t = t();
    assert_eq!(grant_hook_boot(&t.env()), json!({"target": "none"}), "nothing installed: no hook");
    t.enable().unwrap();
    // Sew's socket, and Sew's own token file: the worker asks the helper.
    assert_eq!(hook_calls(&t), vec![format!("on {} {}", pin::SEW_MGMT_SOCKET, pin::SEW_MGMT_TOKEN)]);
    let p = t.row().enable_progress.unwrap();
    // S30 K15: the chat backend never installs in unit tests (process-wide
    // registry); production reports "on" here.
    assert_eq!(p["grantHook"], json!({"target": "on", "chat": "off"}));
    assert_eq!(p["owner"], "queued");
    // Boot re-asserts it for an installed Sew (idempotent).
    assert_eq!(grant_hook_boot(&t.env())["target"], "on");
    assert_eq!(hook_calls(&t).len(), 2);
}

#[test]
fn a_standalone_install_takes_no_grant_hook() {
    let t = t();
    let req = EnableRequest { provider: Some(pin::PROVIDER_STANDALONE.into()), ..t.req() };
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    let env = t.env();
    let latch = enable_begin(&env, &req).unwrap().unwrap();
    enable_machine(&env, &req).unwrap();
    drop(latch);
    assert!(hook_calls(&t).is_empty(), "{:?}", hook_calls(&t));
    let p = t.row().enable_progress.unwrap();
    assert_eq!(p["grantHook"]["target"], "none");
    assert_eq!(p["owner"], "pending");
}

#[test]
fn disable_pauses_uninstall_pauses_or_drops_and_purge_drops_the_grant_hook() {
    let t = t();
    t.enabled();
    t.ops.hook_calls.lock().unwrap().truncate(1);
    assert_eq!(disable(&t.env()).unwrap()["grantHook"]["target"], "off");
    assert_eq!(&hook_calls(&t)[1..], ["off"]);
    t.enable().unwrap();
    let r = uninstall(&t.env(), false).unwrap();
    assert_eq!((r["grants"].as_str(), r["grantHook"]["target"].as_str()), (Some("kept"), Some("off")));
    assert_eq!(&hook_calls(&t)[2..], ["on /run/sew/mgmt.sock /etc/sew/mgmt.token", "off"]);
    // Boot after an uninstall keeps it off.
    assert_eq!(grant_hook_boot(&t.env())["target"], "off");
    t.enable().unwrap();
    let r = uninstall(&t.env(), true).unwrap();
    assert_eq!(r["grants"], "dropped");
    assert!(r["grantHook"]["dropped"].is_object(), "{r}");
    assert_eq!(&hook_calls(&t)[5..], ["on /run/sew/mgmt.sock /etc/sew/mgmt.token", "drop", "off"]);
    t.ops.hook_calls.lock().unwrap().clear();
    t.ops.hook_calls.lock().unwrap().push("on (earlier)".into());
    assert_eq!(purge(&t.env(), "lztek", false).unwrap()["grantHook"]["target"], "off");
    assert_eq!(&hook_calls(&t)[1..], ["drop", "off"]);
}

#[test]
fn a_box_that_never_had_the_hook_is_left_alone_on_disable() {
    let t = t();
    t.enabled();
    t.ops.hook_calls.lock().unwrap().clear();
    assert_eq!(disable(&t.env()).unwrap()["grantHook"], json!({"target": "none"}));
    assert!(hook_calls(&t).is_empty());
}

#[test]
fn a_finished_upgrade_gives_an_older_install_its_hook() {
    let t = t();
    on_old_release(&t);
    t.ops.hook_calls.lock().unwrap().clear();
    let out = upgrade(&t);
    assert_eq!(out["result"], "done", "{out}");
    assert_eq!(hook_calls(&t), vec![format!("on {} {}", pin::SEW_MGMT_SOCKET, pin::SEW_MGMT_TOKEN)]);
}

/// The worker's token source: the supervisor's in-memory cache, filled
/// through the helper (`engine sew token`), refreshed on demand.
#[test]
fn mgmt_token_comes_from_the_helper_and_is_cached_in_memory() {
    let t = t();
    let env = t.env();
    assert_eq!(mgmt_token(&env, false).unwrap(), crate::sew::ops::fake::TOKEN);
    assert_eq!(mgmt_token(&env, false).unwrap(), crate::sew::ops::fake::TOKEN);
    let tokens = t.ops.effects().iter().filter(|e| *e == "engine sew token").count();
    assert_eq!(tokens, 1, "the second read is the cache");
    mgmt_token(&env, true).unwrap();
    assert_eq!(t.ops.effects().iter().filter(|e| *e == "engine sew token").count(), 2, "refresh asks again");
}

// ── upgrade / rollback ───────────────────────────────────────────────────

fn on_old_release(t: &T) {
    t.enabled();
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    store::update(1, |r| r.installed_version = Some("0.0.1".into())).unwrap();
    t.ops.clear();
}

fn upgrade(t: &T) -> Value {
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &req).unwrap().unwrap();
    let p = upgrade_machine(&env, &req, &from);
    drop(l);
    p
}

/// Sew 2026-10-27: an upgrade FROM below 0.3.1 (0.3.0) re-runs Sew's
/// installer; nothing else does.
#[test]
fn reconfigure_runs_only_after_an_upgrade_from_below_0_3_1() {
    assert!(needs_reconfigure("0.3.0", "0.3.1"));
    assert!(needs_reconfigure("0.2.1", "0.4.0"));
    assert!(!needs_reconfigure("0.3.1", "0.3.2"), "already reconfigured");
    assert!(!needs_reconfigure("0.0.1", "0.0.2"), "both below: the new release has no reconfigure");
}

/// The step: protocol 11 + `namespaces.sewreconf` → `engine sew install
/// reconfigure` with install's inputs (no key), then restart + health. An
/// older helper says how to finish by hand; nothing runs.
#[test]
fn reconfigure_step_needs_protocol_11_and_sends_installs_inputs_without_a_key() {
    let t = t();
    t.enabled();
    t.ops.clear();
    let env = t.env();
    let e = reconfigure_step(&env).unwrap_err().to_string();
    assert!(e.contains("k2 root-helper update") && e.contains("k2 hostchat reconfigure"), "{e}");
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    let mut st = helper_ok(11, true, pinned().version);
    st.version_json.as_mut().unwrap()["namespaces"]["sewreconf"] = json!(true);
    *t.ops.helper.lock().unwrap() = st;
    let out = reconfigure_step(&env).expect("reconfigure");
    assert_eq!(out["sewExit"], 0);
    let calls = t.ops.calls.lock().unwrap().clone();
    let rc = calls.iter().find(|c| c.argv == "engine sew install reconfigure").expect("reconfigure call");
    let body: Value = serde_json::from_slice(rc.stdin.as_deref().expect("stdin")).unwrap();
    assert!(body["jwksUrl"].as_str().unwrap().ends_with("/v1/chat/jwks"), "{body}");
    assert!(body.get("masterKey").is_none(), "the install keeps its keys: {body}");
    let eff = t.ops.effects();
    let i = eff.iter().position(|e| e == "engine sew install reconfigure").unwrap();
    assert_eq!(eff[i + 1], "engine sew systemctl restart", "{eff:?}");
}

/// `k2 hostchat restart` (S32 Q2): sew-db, then sew, then readiness;
/// reset-failed only on a protocol-11 helper; refused mid-step.
#[test]
fn restart_brings_the_units_back_in_order() {
    let t = t();
    t.enabled();
    t.ops.clear();
    let v = restart(&t.env()).expect("restart");
    assert_eq!(v["steps"], json!(["sew-db", "sew"]), "{v}");
    assert_eq!(t.ops.effects(), vec!["engine sew systemctl start-db", "engine sew systemctl restart"]);
    let mut st = helper_ok(11, true, pinned().version);
    st.version_json.as_mut().unwrap()["namespaces"]["sewreconf"] = json!(true);
    *t.ops.helper.lock().unwrap() = st;
    t.ops.clear();
    let v = restart(&t.env()).expect("restart");
    assert_eq!(v["steps"][0], "reset-failed", "{v}");
    assert_eq!(t.ops.effects()[0], "engine sew systemctl reset-failed");
    // Not ready afterwards (stopping): refused with the state.
    t.ops.queue_api_health(vec![Ok((503, json!({"ready": false, "reason": "stopping"})))]);
    let e = restart(&t.env()).unwrap_err();
    assert_eq!((e.code, e.extra.unwrap()["state"].clone()), ("sew_unavailable", json!("stopping")));
    // Never over a running lifecycle step.
    let env = t.env();
    let _l = begin(&env).unwrap();
    assert_eq!(restart(&env).unwrap_err().code, "lifecycle_in_progress");
}

/// Sew 2026-10-27: on a box where Sew adopted a pre-existing `sew` login
/// account, reconfigure refuses exit 3 `service_account_unsafe` and changes
/// nothing. K2: the upgrade stands (no rollback), a typed blocked state on
/// status with the person's fix, ONE owner notice, no retry loop; a later
/// successful reconfigure clears it.
#[test]
fn a_blocked_reconfigure_keeps_the_upgrade_notices_once_and_shows_on_status() {
    let t = t();
    t.enabled();
    let mut st = helper_ok(11, true, pinned().version);
    st.version_json.as_mut().unwrap()["namespaces"]["sewreconf"] = json!(true);
    *t.ops.helper.lock().unwrap() = st;
    let unsafe_answer = || {
        Ok(json!({"sewExit": 3, "sew": {"ok": false, "error": {"code": "service_account_unsafe",
            "message": "the sew account is a login account (uid 1003)"}}, "reason": "service_account_unsafe"}))
    };
    t.ops.answers.lock().unwrap().insert("engine sew install reconfigure".into(), std::collections::VecDeque::from(vec![unsafe_answer(), unsafe_answer()]));
    let env = t.env();
    let e = reconfigure_step(&env).unwrap_err();
    assert!(matches!(&e, ReconfErr::Blocked { code, .. } if code == "service_account_unsafe"), "{e:?}");
    note_reconfigure(&env, &e, "0.3.0", "0.3.1");
    assert!(!t.ops.effects().iter().any(|x| x.contains("rollback") || x == "engine sew systemctl restart"), "{:?}", t.ops.effects());
    assert_eq!(t.ops.notices.lock().unwrap().len(), 1, "one owner notice");
    assert!(t.ops.notices.lock().unwrap()[0].1.contains("k2 hostchat reconfigure"));
    let s = status(&t.env(), false);
    assert_eq!((s["reconfigure"]["state"].clone(), s["reconfigure"]["code"].clone()), (json!("reconfigure_blocked"), json!("service_account_unsafe")), "{s}");
    assert!(s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_reconfigure_blocked"), "{s}");
    assert_eq!(t.row().status, "running", "the upgrade stands");
    // The person's `k2 hostchat reconfigure` while still blocked: 409, quiet.
    let f = reconfigure(&env).unwrap_err();
    assert_eq!((f.code, f.status), ("reconfigure_blocked", 409));
    assert_eq!(t.ops.notices.lock().unwrap().len(), 1, "the same block never notices twice");
    // Fixed by a person: the next reconfigure succeeds and clears the state.
    reconfigure(&env).expect("reconfigure after the fix");
    let s = status(&t.env(), false);
    assert!(s["reconfigure"].is_null(), "{s}");
    assert!(!s["alarms"].as_array().unwrap().iter().any(|a| a == "k2_reconfigure_blocked"));
}

#[test]
fn upgrade_preconditions_refuse_without_effects() {
    let t = t();
    t.enabled();
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
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

/// A hand-upgraded box: the binary already IS the pin while the row still
/// records the old version. `upgrade` answers alreadyCurrent AND brings the
/// row in line (status stops offering an upgrade); previous_version is not
/// invented.
#[test]
fn upgrade_already_current_resyncs_the_row_to_the_binary() {
    let t = t();
    t.enabled();
    store::update(1, |r| {
        r.installed_version = Some("0.0.1".into());
        r.pinned_version = "0.0.1".into();
    })
    .unwrap();
    assert_eq!(status(&t.env(), false)["upgradeAvailable"], true, "the stale row offers an upgrade");
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let v = match upgrade_begin(&t.env(), &req).unwrap() {
        Err(v) => v,
        Ok(_) => panic!("the pin on disk must answer without a machine"),
    };
    assert_eq!(v["alreadyCurrent"], true);
    assert_eq!(v["resynced"]["installedVersion"]["from"], "0.0.1");
    assert_eq!(v["resynced"]["installedVersion"]["to"], pinned().version);
    let row = t.row();
    assert_eq!(row.installed_version.as_deref(), Some(pinned().version));
    assert_eq!(row.pinned_version, pinned().version);
    assert_eq!(row.previous_version, None, "K2 did not see the move: no previous version invented");
    assert_eq!(row.status, "running");
    let s = status(&t.env(), false);
    assert_eq!(s["installedVersion"], pinned().version);
    assert_eq!(s["upgradeAvailable"], false);
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    assert!(!t.latch.load(std::sync::atomic::Ordering::SeqCst), "the latch was released");
    // Already in sync: nothing to re-sync, nothing written.
    let at = t.row().updated_at;
    *t.ops.clock.lock().unwrap() += 60;
    let v = upgrade_begin(&t.env(), &req).unwrap().err().unwrap();
    assert_eq!(v["alreadyCurrent"], true);
    assert!(v["resynced"].is_null(), "{v}");
    assert_eq!(t.row().updated_at, at);
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
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
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
        fn path_probe(&self, p: &str) -> Option<bool> { self.inner.path_probe(p) }
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
        fn record_load(&self, name: &str) -> Option<Value> { self.inner.record_load(name) }
        fn record_save(&self, name: &str, v: Option<&Value>) -> Result<(), String> { self.inner.record_save(name, v) }
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
    // Rosson 2026-10-28: the refusal points at the person-only emergency
    // rollback (and an agent's card), never at an invented verb.
    assert!(f.hint.contains("k2 hostchat rollback --accept-data-loss") && f.hint.contains("sew.rollback-data-loss"), "{}", f.hint);
    assert!(f.hint.contains("in a terminal outside K2"), "{}", f.hint);
    assert!(!f.hint.contains("backup list") && !f.hint.contains("backup restore"), "no invented verbs: {}", f.hint);
    assert!(t.ops.argvs().iter().all(|a| !a.contains("accept-data-loss")));
    // Success: the version Sew reports is recorded.
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 0, "sew": {"state": "rolled_back", "to": "0.0.1"}})));
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    let r = rollback(&t.env()).unwrap();
    assert_eq!(r["state"], "running");
    assert_eq!(t.row().installed_version.as_deref(), Some("0.0.1"));
}

/// A helper that serves the emergency rollback (protocol 11 +
/// `namespaces.sewrbloss`).
fn helper_with_rbloss(t: &T) {
    let mut st = helper_ok(11, true, pinned().version);
    st.version_json.as_mut().unwrap()["namespaces"]["sewrbloss"] = json!(true);
    *t.ops.helper.lock().unwrap() = st;
}

/// Rosson 2026-10-28 (Sew's order): the upgrade records the rollback
/// point; the emergency rollback backs up first, then rolls back to that
/// bundle accepting the loss, then K2's checks; the point is spent.
#[test]
fn the_emergency_rollback_records_the_point_backs_up_first_and_rolls_back_to_it() {
    let t = t();
    on_old_release(&t);
    assert_eq!(upgrade(&t)["result"], "done");
    let point = t.ops.record_load(ROLLBACK_POINT_RECORD).expect("the upgrade recorded the rollback point");
    assert_eq!((point["from"].as_str(), point["to"].as_str()), (Some("0.0.1"), Some(pinned().version)));
    assert_eq!(point["bundle"], crate::sew::ops::fake::FakeOps::FAKE_PRE_BUNDLE);
    assert!(point["at"].as_i64().is_some());
    let plan = rollback_plan(&t.env()).unwrap();
    assert_eq!(
        (plan["from"].as_str(), plan["to"].as_str(), plan["bundle"].as_str(), plan["source"].as_str()),
        (Some(pinned().version), Some("0.0.1"), Some("20251231T120000Z"), Some("recorded"))
    );
    assert_eq!(plan["since"], "2025-12-31 12:00 UTC");
    t.ops.clear();
    let req = RollbackRequest { media_tarball_path: None };

    // An older helper: nothing runs, nothing is backed up.
    let f = rollback_accept_data_loss(&t.env(), &req, "20251231T120000Z").unwrap_err();
    assert_eq!(f.code, "helper_update_needed", "{}", f.hint);
    assert!(t.ops.mgmt_calls.lock().unwrap().iter().all(|c| c != "POST /mgmt/v1/backup"));
    helper_with_rbloss(&t);
    // Not the plan's bundle (it moved): refused, nothing runs.
    let f = rollback_accept_data_loss(&t.env(), &req, "20200101T000000Z").unwrap_err();
    assert_eq!(f.code, "rollback_point_changed", "{}", f.hint);
    assert!(t.ops.effects().iter().all(|e| !e.contains("accept-data-loss")), "{:?}", t.ops.effects());

    // The backup comes first: when it fails, nothing rolls back.
    queue_mgmt(&t, "POST /mgmt/v1/backup", Err(crate::sew::ops::MgmtError::Status(503, r#"{"error":{"reason":"backups_unavailable"}}"#.into())));
    let f = rollback_accept_data_loss(&t.env(), &req, "20251231T120000Z").unwrap_err();
    assert_eq!(f.code, "backup_first_failed", "{}", f.hint);
    assert!(f.hint.contains("Nothing was changed"), "{}", f.hint);
    assert!(t.ops.effects().iter().all(|e| !e.contains("accept-data-loss")), "{:?}", t.ops.effects());

    // Done: backup, then the rollback to that bundle, then the checks.
    let before = t.ops.mgmt_backups.lock().unwrap().len();
    let r = rollback_accept_data_loss(&t.env(), &req, "20251231T120000Z").unwrap();
    assert_eq!(r["result"], "rolled_back", "{r}");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(t.ops.mgmt_backups.lock().unwrap().len(), before + 1, "a fresh backup of the new version first");
    let kept = t.ops.mgmt_backups.lock().unwrap()[0]["id"].as_str().unwrap().to_string();
    assert_eq!(r["keptBackup"], kept.as_str());
    assert!(t.ops.effects().iter().any(|e| e == "engine sew rollback accept-data-loss 20251231T120000Z"), "{:?}", t.ops.effects());
    assert_eq!(r["upgradeState"]["state"], "rolled_back");
    assert_eq!(r["doctorFails"], json!([]));
    assert!(r["message"].as_str().unwrap().contains(&format!("kept in backup {kept}")), "{r}");
    let row = t.row();
    assert_eq!((row.installed_version.as_deref(), row.previous_version.as_deref(), row.status.as_str()), (Some("0.0.1"), None, "running"));
    assert!(t.ops.record_load(ROLLBACK_POINT_RECORD).is_none(), "the rollback point is spent");
    assert!(t.ops.effects().iter().all(|e| e != "engine sew rollback"), "never the plain rollback");
}

/// Sew's other answers each say plainly what happened; nothing is lost on
/// 1, a person must act on 2.
#[test]
fn the_emergency_rollback_says_what_each_sew_exit_means() {
    let t = t();
    on_old_release(&t);
    upgrade(&t);
    helper_with_rbloss(&t);
    let req = RollbackRequest { media_tarball_path: None };
    let b = "20251231T120000Z";
    let argv = format!("engine sew rollback accept-data-loss {b}");
    t.ops.queue(&argv, Ok(json!({"sewExit": 1, "sew": {"error": {"message": "migrate down failed; put back"}}, "upgradeState": {"state": "restored"}})));
    let r = rollback_accept_data_loss(&t.env(), &req, b).unwrap();
    assert_eq!((r["result"].as_str(), r["ok"].as_bool()), (Some("put_back"), Some(false)), "{r}");
    assert!(r["message"].as_str().unwrap().contains("Nothing was lost"), "{r}");
    assert_eq!(t.row().installed_version.as_deref(), Some(pinned().version), "still the new version");
    t.ops.queue(&argv, Ok(json!({"sewExit": 2, "sew": {"error": {"message": "ROLLBACK FAILED"}}, "upgradeState": {"state": "failed"}})));
    let r = rollback_accept_data_loss(&t.env(), &req, b).unwrap();
    assert_eq!(r["result"], "rollback_failed", "{r}");
    let m = r["message"].as_str().unwrap();
    assert!(m.starts_with("ROLLBACK FAILED") && m.contains("A person must act") && m.contains(SEW_UPGRADE_STATE), "{m}");
    assert_eq!(r["upgradeState"]["state"], "failed");
    assert_eq!(t.row().status, "error");
    t.ops.queue(&argv, Ok(json!({"sewExit": 4, "sew": {"error": {"message": "no such bundle"}}})));
    let f = rollback_accept_data_loss(&t.env(), &req, b).unwrap_err();
    assert_eq!(f.code, "rollback_refused");
    assert!(f.hint.contains("Nothing was changed"), "{}", f.hint);
}

/// No recorded point (an upgrade from before 0.46.6): the newest backup of
/// the previous version is the rollback point; none at all is a refusal.
#[test]
fn without_a_recorded_point_the_plan_takes_the_newest_backup_of_the_previous_version() {
    let t = t();
    on_old_release(&t);
    upgrade(&t);
    t.ops.record_save(ROLLBACK_POINT_RECORD, None).unwrap();
    let f = rollback_plan(&t.env()).unwrap_err();
    assert_eq!(f.code, "no_rollback_point", "the fake's backups are all the new version: {}", f.hint);
    {
        let mut list = t.ops.mgmt_backups.lock().unwrap();
        list.push(json!({"id": "20251230T031500Z", "kind": "scheduled", "created_at": "x", "bytes": 1, "schema_version": 1, "sew_version": "0.0.1"}));
        list.push(json!({"id": "20251229T031500Z", "kind": "scheduled", "created_at": "x", "bytes": 1, "schema_version": 1, "sew_version": "0.0.1"}));
    }
    let plan = rollback_plan(&t.env()).unwrap();
    assert_eq!((plan["bundle"].as_str(), plan["source"].as_str()), (Some("20251230T031500Z"), Some("backups")));
    // Nothing to go back to without a previous version.
    store::update(1, |r| r.previous_version = None).unwrap();
    assert_eq!(rollback_plan(&t.env()).unwrap_err().code, "no_previous_version");
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
    t.ops.records.lock().unwrap().insert("server".into(), json!({"publicUrl": "https://hearth.lztek.k2.dev"}));
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

// ── upgrade: refusal reasons end to end, voice after the core ────────────

/// The box test of 2026-10-17 (Sew 0.1.2 refused a multi-member tarball,
/// helper code 0): every way the helper can answer maps to a result whose
/// reason is in the row's `lastError`, in `upgradeProgress.lastError` and
/// in `k2 hostchat status`. A reasonless "refused" is impossible.
#[test]
fn upgrade_result_mapping_always_carries_the_reason() {
    let refusal = "the tarball must hold exactly one regular file named sew; found \"LICENSE\"";
    let cases: Vec<(Result<Value, String>, &str, &str)> = vec![
        // Protocol 8 helper: `reason` (from Sew's JSON, stderr or stdout).
        (Ok(json!({"sewExit": 3, "sew": {"ok": false, "error": {"exit": 3, "message": refusal}}, "stderrTail": "", "reason": refusal})), "refused", refusal),
        // An older helper: no `reason`; Sew's JSON error.
        (Ok(json!({"sewExit": 3, "sew": {"ok": false, "error": {"exit": 3, "message": refusal}}, "stderrTail": ""})), "refused", refusal),
        // An older helper: the words only on stderr.
        (Ok(json!({"sewExit": 3, "sew": null, "stderrTail": "sew upgrade: refusing"})), "refused", "sew upgrade: refusing"),
        // Nothing anywhere: still a reason, never null.
        (Ok(json!({"sewExit": 3, "sew": null, "stderrTail": ""})), "refused", "Sew exited 3 and gave no reason"),
        // A helper answer without Sew's exit (a broken helper): loud, not "done".
        (Ok(json!({"installed": "0.0.2"})), "refused", "the helper answered without Sew's exit code"),
        // The helper itself refused (never ran Sew).
        (Err("root helper refused `engine sew upgrade`: busy".into()), "refused", "root helper refused `engine sew upgrade`: busy"),
        // Sew failed and rolled back itself.
        (Ok(json!({"sewExit": 1, "sew": null, "stderrTail": "", "reason": "health check failed"})), "rolled_back", "health check failed"),
    ];
    for (answer, want_result, want_reason) in cases {
        let t = t2_old();
        t.ops.queue("engine sew upgrade", answer.clone());
        let p = upgrade(&t);
        assert_eq!(p["result"], want_result, "{answer:?} → {p}");
        let row = t.row();
        let last = row.last_error.clone().unwrap_or_else(|| panic!("{answer:?}: lastError is null"));
        assert!(last.contains(want_reason), "{answer:?}: {last}");
        assert_eq!(p["lastError"].as_str(), Some(last.as_str()), "upgradeProgress.lastError = the row's");
        assert_eq!(row.installed_version.as_deref(), Some("0.0.1"), "{answer:?}");
        assert_eq!(row.status, "running", "{answer:?}: the prior state comes back");
        let s = status(&t.env(), false);
        assert_eq!(s["lastError"].as_str(), Some(last.as_str()));
        assert_eq!(s["upgradeProgress"]["lastError"].as_str(), Some(last.as_str()));
        assert_eq!(t.ops.effects().last().map(String::as_str), Some("engine sew unstage"), "{answer:?}");
    }
    // Success: no reason anywhere.
    let t = t2_old();
    let p = upgrade(&t);
    assert_eq!(p["result"], "done");
    assert!(p["lastError"].is_null(), "{p}");
    assert!(t.row().last_error.is_none());
}

/// A non-`done` end that somehow reached `upgrade_note` without a reason is
/// recorded as a K2 bug, never left null.
#[test]
fn an_upgrade_end_without_a_reason_is_recorded_as_a_k2_bug() {
    let t = t2_old();
    store::update(1, |r| {
        r.upgrade_progress = Some(json!({"from": "0.0.1", "to": pinned().version, "steps": {}, "current": "upgrade"}));
        r.last_error = None;
    })
    .unwrap();
    upgrade_note(&t.env(), None, Some("refused"));
    let row = t.row();
    let last = row.last_error.expect("never null");
    assert!(last.contains("no reason was recorded (a K2 bug"), "{last}");
    assert_eq!(row.upgrade_progress.unwrap()["lastError"].as_str(), Some(last.as_str()));
}

const MEDIA_TARBALL: &str = "/home/k2/sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz";

fn with_voice(t: &T) {
    t.ops.voice_save(Some(&json!({"installed": true, "publicUrl": "wss://voice.box.k2.dev"}))).unwrap();
    t.ops.files.lock().unwrap().insert(MEDIA_TARBALL.into(), fixture("sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
}

fn voice_upgrade(t: &T) -> Value {
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), media_tarball_path: Some(MEDIA_TARBALL.into()), dry_run: false, accept_binary: None };
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &req).unwrap().unwrap();
    let p = upgrade_machine(&env, &req, &from);
    drop(l);
    p
}

/// With voice installed (Sew 2026-10-17): the sew-media pin must be in hand
/// before anything moves; it is staged with the core; the NEW core
/// upgrades the engine only after K2's own health check of the core.
#[test]
fn upgrade_with_voice_upgrades_the_engine_through_the_new_core_after_its_health_check() {
    let t = t2_old();
    with_voice(&t);
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let f = upgrade_begin(&t.env(), &req).err().expect("refused");
    assert_eq!(f.code, "media_tarball_needed", "{}", f.hint);
    assert!(f.hint.contains("--media-tarball"), "{}", f.hint);
    let dry = upgrade_begin(&t.env(), &UpgradeRequest { dry_run: true, media_tarball_path: Some(MEDIA_TARBALL.into()), ..req.clone() })
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(dry["voice"], true);
    assert!(dry["steps"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("sew media upgrade")), "{dry}");
    assert!(t.ops.effects().is_empty(), "nothing moved: {:?}", t.ops.effects());
    assert_eq!(t.row().status, "running");

    let p = voice_upgrade(&t);
    assert_eq!(p["result"], "done", "{p}");
    assert!(p["lastError"].is_null());
    assert_eq!(
        t.ops.effects(),
        vec!["engine sew stage", "engine sew stage-media", "engine sew upgrade", "engine sew token", "engine sew media upgrade", "engine sew unstage"]
    );
    assert!(p["steps"].get("media").is_some(), "{p}");
    // The steps carry their execution order (`at` is whole seconds and the
    // object is key-sorted): the core swap before K2's health check of it.
    let seq = |s: &str| p["steps"][s]["seq"].as_u64().unwrap_or_else(|| panic!("{s}: no seq in {p}"));
    assert_eq!([seq("stage"), seq("upgrade"), seq("health"), seq("media")], [1, 2, 3, 4], "{p}");
    let row = t.row();
    assert_eq!((row.installed_version.as_deref(), row.previous_version.as_deref()), (Some(pinned().version), Some("0.0.1")));
}

/// The engine upgrade fails: the core stays upgraded (it passed K2's
/// check), the result says so, and the reason is recorded. A core that
/// fails K2's check is rolled back before the engine is ever touched.
#[test]
fn upgrade_with_voice_keeps_the_core_and_records_why_when_the_engine_upgrade_fails() {
    let t = t2_old();
    with_voice(&t);
    t.ops.queue(
        "engine sew media upgrade",
        Ok(json!({"sewExit": 75, "sew": {"ok": false, "error": {"exit": 75, "message": "media_not_idle"}}, "reason": "media_not_idle"})),
    );
    let p = voice_upgrade(&t);
    assert_eq!(p["result"], "media_failed", "{p}");
    let row = t.row();
    assert_eq!(row.installed_version.as_deref(), Some(pinned().version));
    let last = row.last_error.expect("reason");
    assert!(last.contains("the voice engine was not") && last.contains("exit 75") && last.contains("media_not_idle"), "{last}");
    assert_eq!(p["lastError"].as_str(), Some(last.as_str()));

    // K2's health check of the core fails: rolled back, engine never run.
    let t = t2_old();
    with_voice(&t);
    *t.ops.mgmt_health_ok.lock().unwrap() = false;
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), media_tarball_path: Some(MEDIA_TARBALL.into()), dry_run: false, accept_binary: None };
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &req).unwrap().unwrap();
    let p = upgrade_machine_with_flip(&env, &req, &from, &t.ops);
    drop(l);
    assert_eq!(p["result"], "rolled_back", "{p}");
    let eff = t.ops.effects();
    assert!(eff.iter().any(|e| e == "engine sew rollback"), "{eff:?}");
    assert!(!eff.iter().any(|e| e == "engine sew media upgrade"), "the engine never moves before the core is good: {eff:?}");
}

/// A bad sew-media tarball aborts before the core moves.
#[test]
fn upgrade_with_voice_aborts_before_any_change_on_a_wrong_media_tarball() {
    let t = t2_old();
    with_voice(&t);
    t.ops.files.lock().unwrap().insert(MEDIA_TARBALL.into(), b"not the pin".to_vec());
    let p = voice_upgrade(&t);
    assert_eq!(p["result"], "aborted", "{p}");
    assert!(!t.ops.effects().iter().any(|e| e == "engine sew upgrade"), "{:?}", t.ops.effects());
    let last = t.row().last_error.expect("reason");
    assert!(last.contains("aborted before any change") && last.contains("sew-media"), "{last}");
    assert_eq!(p["lastError"].as_str(), Some(last.as_str()));
}

// ── rollback with voice: the reverse of the upgrade ──────────────────────

const OLD_MEDIA_TARBALL: &str = "/home/k2/sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz";

/// Upgraded with voice (core + engine on the pin, previous core 0.0.1),
/// the old engine's tarball on disk but not kept by the helper.
fn upgraded_with_voice() -> T {
    let t = t2_old();
    with_voice(&t);
    assert_eq!(voice_upgrade(&t)["result"], "done");
    t.ops.files.lock().unwrap().insert(OLD_MEDIA_TARBALL.into(), fixture("sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz"));
    t.ops.clear();
    t
}

fn rollback_req(media: Option<&str>) -> RollbackRequest {
    RollbackRequest { media_tarball_path: media.map(Into::into) }
}

/// Sew 2026-10-17: core back → K2's check of the OLD core → the OLD core
/// reinstalls the previous release's engine → sew doctor's media checks in
/// the answer. No warning about a stranded engine any more.
#[test]
fn rollback_with_voice_has_the_old_core_put_the_previous_engine_back_and_reports_doctor() {
    let t = upgraded_with_voice();
    // The core is rolled back on the box (Sew swaps sew.prev in).
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 0, "sew": {"state": "rolled_back", "to": "0.0.1"}})));
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    let r = rollback_with(&t.env(), &rollback_req(Some(OLD_MEDIA_TARBALL))).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!((r["media"]["result"].as_str(), r["media"]["to"].as_str()), (Some("done"), Some("0.0.1")), "{r}");
    assert_eq!(r["doctor"]["ok"], true, "{r}");
    assert_eq!(r["doctor"]["mediaVersion"]["detail"], "sew-media 0.0.1");
    assert_eq!(r["doctor"]["mediaBinary"]["status"], "ok");
    assert!(r.get("warning").is_none(), "{r}");
    let eff = t.ops.effects();
    let pos = |e: &str| eff.iter().position(|x| x == e).unwrap_or_else(|| panic!("{e} missing: {eff:?}"));
    assert!(pos("engine sew stage-media") < pos("engine sew rollback"), "the old engine is kept before the core moves: {eff:?}");
    assert!(pos("engine sew rollback") < pos("engine sew media rollback"), "core first: {eff:?}");
    assert!(pos("engine sew media rollback") < pos("engine sew doctor"), "doctor last: {eff:?}");
    assert!(!eff.iter().any(|e| e == "engine sew media upgrade"), "never the new core's verb: {eff:?}");
    let row = t.row();
    assert_eq!((row.status.as_str(), row.installed_version.as_deref()), ("running", Some("0.0.1")));
    assert!(row.last_error.is_none(), "{:?}", row.last_error);
}

/// The previous engine's tarball is neither kept nor named: refused with
/// the fix in the words, BEFORE the core moves.
#[test]
fn rollback_with_voice_refuses_before_anything_moves_without_the_previous_engine() {
    let t = upgraded_with_voice();
    let f = rollback_with(&t.env(), &rollback_req(None)).unwrap_err();
    assert_eq!(f.code, "previous_media_tarball_needed", "{}", f.hint);
    assert!(f.hint.contains("--media-tarball sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz"), "{}", f.hint);
    assert!(f.hint.contains("nothing was changed"));
    assert!(!t.ops.effects().iter().any(|e| e == "engine sew rollback"), "{:?}", t.ops.effects());
    // The wrong tarball: refused, nothing staged or rolled back.
    let f = rollback_with(&t.env(), &rollback_req(Some(TARBALL))).unwrap_err();
    assert_eq!(f.code, "sha256_mismatch", "{}", f.hint);
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    assert_eq!(t.row().installed_version.as_deref(), Some(pinned().version));
}

/// The old core put the engine back, but sew doctor disagrees (the mixed
/// pair 0.1.2 core + 0.2.0 engine fails 0.1.2's media.binary): surfaced
/// in the answer and in lastError, ok false. An engine step Sew refuses is
/// surfaced the same way.
#[test]
fn rollback_with_voice_surfaces_doctor_disagreement_and_engine_failure() {
    let t = upgraded_with_voice();
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 0, "sew": {"state": "rolled_back", "to": "0.0.1"}})));
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    t.ops.queue(
        "engine sew doctor",
        Ok(json!({"sewExit": 1, "sew": {"ok": false, "checks": [
            {"id": "media.binary", "status": "fail", "detail": "the binary's sha256 differs from the pin baked into sew", "hint": ""},
            {"id": "media.version", "status": "warn", "detail": "sew-media 0.0.2, sew 0.0.1: upgrade pending", "hint": ""},
        ]}})),
    );
    let r = rollback_with(&t.env(), &rollback_req(Some(OLD_MEDIA_TARBALL))).unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(r["doctor"]["ok"], false);
    let e = r["error"].as_str().unwrap();
    assert!(e.contains("media.binary fail") && e.contains("differs from the pin"), "{e}");
    assert_eq!(t.row().last_error.as_deref(), Some(e));

    let t = upgraded_with_voice();
    t.ops.queue("engine sew rollback", Ok(json!({"sewExit": 0, "sew": {"state": "rolled_back", "to": "0.0.1"}})));
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    t.ops.queue("engine sew media rollback", Ok(json!({"sewExit": 3, "sew": null, "reason": "media_unpinned_build"})));
    let r = rollback_with(&t.env(), &rollback_req(Some(OLD_MEDIA_TARBALL))).unwrap();
    assert_eq!((r["ok"].as_bool(), r["media"]["result"].as_str()), (Some(false), Some("failed")), "{r}");
    let e = t.row().last_error.unwrap();
    assert!(e.contains("voice engine was not put back") && e.contains("exit 3") && e.contains("media_unpinned_build"), "{e}");
}

// ── boot: JWKS URL drift + install id (S29 §19, 0.46.3) ──────────────────

const OLD_JWKS: &str = "http://127.0.0.1:41111/v1/chat/jwks";

fn sew_reports_jwks(t: &T, url: Option<&str>) {
    let mut st = t.ops.mgmt_status.lock().unwrap();
    match url {
        Some(u) => st["identity"]["jwks_url"] = json!(u),
        None => {
            st["identity"].as_object_mut().unwrap().remove("jwks_url");
        }
    }
}

#[test]
fn jwks_url_drift_is_fixed_on_boot_through_config_restart_and_read_back() {
    let t = t();
    t.enabled();
    sew_reports_jwks(&t, Some(OLD_JWKS));
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "fixed", "{v}");
    assert_eq!(v["from"], OLD_JWKS);
    assert_eq!(v["to"], "http://127.0.0.1:38471/v1/chat/jwks");
    assert!(jwks_settled(&v));
    // config, restart, then the mgmt token is fetched afresh (as after any
    // restart in health_check).
    assert_eq!(t.ops.effects(), vec!["engine sew config", "engine sew systemctl restart", "engine sew token"]);
    // Exactly the one key, on stdin (never argv).
    let calls = t.ops.calls.lock().unwrap();
    let cfg = calls.iter().find(|c| c.argv == "engine sew config").unwrap();
    let body: Value = serde_json::from_slice(cfg.stdin.as_ref().unwrap()).unwrap();
    assert_eq!(body, json!({"identity.k2.jwks_url": "http://127.0.0.1:38471/v1/chat/jwks"}));
    drop(calls);
    assert!(t.ops.records.lock().unwrap().get(JWKS_RECORD).is_none());
    assert!(!t.latch.load(std::sync::atomic::Ordering::SeqCst), "latch released");
    // A second check matches and changes nothing.
    t.ops.clear();
    assert_eq!(jwks_drift_check(&t.env())["result"], "match");
    assert!(t.ops.effects().is_empty());
}

#[test]
fn a_jwks_fix_that_does_not_take_is_the_alarm_until_a_check_matches() {
    let t = t();
    t.enabled();
    sew_reports_jwks(&t, Some(OLD_JWKS));
    // Sew accepts the key but keeps reporting the old URL after the restart.
    t.ops.queue("engine sew config", Ok(json!({"sewExit": 0, "results": []})));
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "failed", "{v}");
    assert!(v["error"].as_str().unwrap().contains(OLD_JWKS), "{v}");
    assert!(jwks_settled(&v), "no restart loop: one try per daemon start");
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    let s = status(&t.env(), false);
    assert!(s["alarms"].as_array().unwrap().contains(&json!(ALARM_JWKS_DRIFT)), "{}", s["alarms"]);
    assert_eq!(s["state"], "degraded");
    // Sew refusing the key outright fails the same way, naming Sew's reason.
    t.ops.queue("engine sew config", Ok(json!({"sewExit": 2, "reason": "identity.k2.jwks_url: not settable"})));
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "failed");
    assert!(v["error"].as_str().unwrap().contains("not settable"), "{v}");
    // The doctor re-runs the check: now the fix takes and the alarm clears.
    let d = doctor(&t.env()).unwrap();
    assert_eq!(d["jwks"]["result"], "fixed", "{d}");
    let s = status(&t.env(), false);
    assert!(!s["alarms"].as_array().unwrap().contains(&json!(ALARM_JWKS_DRIFT)), "{}", s["alarms"]);
}

#[test]
fn the_jwks_check_skips_what_it_cannot_judge_and_retries_what_may_change() {
    let t = t();
    // No install: final, nothing called.
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "not_applicable");
    assert!(jwks_settled(&v));
    t.enabled();
    // Sew does not report the URL (pre-G5): skipped, never "fixed" blind.
    sew_reports_jwks(&t, None);
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "unreported");
    assert!(t.ops.effects().is_empty());
    // Retries: units down, port unknown, a lifecycle step holding the latch.
    sew_reports_jwks(&t, Some(OLD_JWKS));
    t.ops.set_unit(k2_root_proto::sew::UNIT_SEW, "inactive", 0);
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "sew_down");
    assert!(!jwks_settled(&v));
    t.ops.set_unit(k2_root_proto::sew::UNIT_SEW, "active", 0);
    *t.ops.port.lock().unwrap() = None;
    assert_eq!(jwks_drift_check(&t.env())["result"], "no_port");
    *t.ops.port.lock().unwrap() = Some(38471);
    let held = begin(&t.env()).unwrap();
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "busy");
    assert!(!jwks_settled(&v));
    drop(held);
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    // A disabled install is waited for, not settled.
    store::update(1, |r| r.status = "disabled".into()).unwrap();
    assert!(!jwks_settled(&jwks_drift_check(&t.env())));
}

#[test]
fn a_standalone_install_has_no_jwks_to_check() {
    let t = t();
    let req = EnableRequest { provider: Some("standalone".into()), ..t.req() };
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    let env = t.env();
    let l = enable_begin(&env, &req).unwrap().unwrap();
    enable_machine(&env, &req).unwrap();
    drop(l);
    t.ops.clear();
    sew_reports_jwks(&t, Some(OLD_JWKS));
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "not_applicable", "{v}");
    assert!(t.ops.effects().is_empty());
}

#[test]
fn the_boot_check_stores_a_changed_install_id_for_person_confirms() {
    let t = t();
    t.enabled();
    sew_reports_jwks(&t, Some("http://127.0.0.1:38471/v1/chat/jwks"));
    t.ops.mgmt_status.lock().unwrap()["install_id"] = json!("99931765147549702");
    let v = jwks_drift_check(&t.env());
    assert_eq!(v["result"], "match");
    assert_eq!(v["installId"]["from"], INSTALL_ID);
    assert_eq!(v["installId"]["to"], "99931765147549702");
    assert_eq!(t.row().install_id.as_deref(), Some("99931765147549702"));
    // A malformed id is never stored.
    t.ops.mgmt_status.lock().unwrap()["install_id"] = json!("none");
    assert!(jwks_drift_check(&t.env()).get("installId").is_none());
    assert_eq!(t.row().install_id.as_deref(), Some("99931765147549702"));
}

/// Rosson's open question (a), K2's lean: purge types back Sew's public
/// host (the server.public_url K2 set), else the box hostname. A purge
/// also forgets K2's records of Sew's config.
#[test]
fn purge_types_back_sews_public_host_and_forgets_k2s_sew_records() {
    let t = t();
    t.enabled();
    assert_eq!(purge_hostname(&t.env(), &t.row()), "lztek", "no public URL: the box hostname");
    t.ops.records.lock().unwrap().insert(SERVER_RECORD.into(), json!({"publicUrl": "https://Hearth.lztek.k2.dev:443/x"}));
    t.ops.records.lock().unwrap().insert(JWKS_RECORD.into(), json!({"have": OLD_JWKS}));
    assert_eq!(purge_hostname(&t.env(), &t.row()), "hearth.lztek.k2.dev");
    assert_eq!(status(&t.env(), false)["purgeConfirmHostname"], "hearth.lztek.k2.dev");
    let f = purge(&t.env(), "lztek", false).unwrap_err();
    assert_eq!(f.code, "confirm_hostname_mismatch");
    assert!(f.hint.contains("--confirm-hostname hearth.lztek.k2.dev"), "{}", f.hint);
    assert!(t.ops.effects().is_empty());
    let r = purge(&t.env(), "hearth.lztek.k2.dev", false).unwrap();
    assert_eq!(r["purged"], true);
    let recs = t.ops.records.lock().unwrap();
    assert!(recs.get(SERVER_RECORD).is_none() && recs.get(JWKS_RECORD).is_none(), "{recs:?}");
}

// ── sew-db's PostgreSQL major is pinned (Sew ask, 2026-10-20) ────────────

/// 0.46.3 installs PGDG 17 for K2's own databases. A box with only 17 must
/// not pass Sew's preflight (Sew would be told pg_major 16 and fail, or
/// without it silently take 17); 16 next to 17 passes; enable refuses
/// before any effect.
#[test]
fn preflight_needs_exactly_sews_pg_major_not_any_newer() {
    let t = t();
    let pg_check = |t: &T| {
        preflight(&t.env(), &t.req()).into_iter().find(|c| c["id"] == "postgres").expect("postgres check")
    };
    *t.ops.pg.lock().unwrap() = vec![17];
    let c = pg_check(&t);
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["detail"].as_str().unwrap().contains("[17] found, not 16"), "{c}");
    assert!(c["hint"].as_str().unwrap().contains("postgresql-16 package"), "{c}");
    let f = match enable_begin(&t.env(), &t.req()) {
        Err(f) => f,
        Ok(_) => panic!("enable must refuse with only PostgreSQL 17"),
    };
    assert_eq!(f.code, "preflight_failed", "{}: {}", f.code, f.hint);
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    *t.ops.pg.lock().unwrap() = vec![16, 17];
    assert_eq!(pg_check(&t)["status"], "ok");
    *t.ops.pg.lock().unwrap() = vec![];
    assert!(pg_check(&t)["detail"].as_str().unwrap().contains("no PostgreSQL server binaries"));
}

// ── item 12 (0.46.5): upgrade-only test builds and accepted binaries ─────

fn on_test_build(t: &T) {
    t.enabled();
    *t.ops.on_disk.lock().unwrap() = Some(crate::sew::ops::fake::TESTBUILD.into());
    let v = k2_root_proto::sew::upgrade_only_builds()[0].version;
    store::update(1, |r| r.installed_version = Some(v.into())).unwrap();
    t.ops.clear();
}

/// sewlab's case: the helper reports an upgrade-only build (not `known`).
/// It upgrades to the pin; the row's previous version is that build, which
/// is not a release, so `k2 hostchat rollback` refuses (a restore is the way
/// back) and the helper's rollback never runs.
#[test]
fn an_upgrade_only_test_build_upgrades_and_is_never_rolled_back_to() {
    let t = t();
    on_test_build(&t);
    let build = &k2_root_proto::sew::upgrade_only_builds()[0];
    assert!(!is_release(build.version));
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let dry = upgrade_begin(&t.env(), &UpgradeRequest { dry_run: true, ..req.clone() }).unwrap().err().unwrap();
    assert_eq!(dry["from"], build.version, "{dry}");
    assert!(dry["steps"].to_string().contains("never onto the binary on disk now"), "{dry}");
    let p = upgrade(&t);
    assert_eq!(p["result"], "done", "{p}");
    assert!(t.ops.effects().iter().any(|e| e == "engine sew upgrade"), "{:?}", t.ops.effects());
    let row = t.row();
    assert_eq!((row.status.as_str(), row.installed_version.as_deref(), row.previous_version.as_deref()), ("running", Some(pinned().version), Some(build.version)));
    t.ops.clear();
    let e = rollback(&t.env()).err().expect("refused");
    assert_eq!(e.code, "rollback_refused", "{}", e.hint);
    assert!(e.hint.contains("not a release K2 pinned") && e.hint.contains("k2 hostchat backups"), "{}", e.hint);
    assert!(!t.ops.effects().iter().any(|e| e.contains("rollback")), "{:?}", t.ops.effects());
    assert_eq!(t.row().installed_version.as_deref(), Some(pinned().version), "nothing moved");
}

/// A failed K2 health check after upgrading FROM a non-release never rolls
/// back onto it: the row says why and a person decides.
#[test]
fn a_failed_health_check_never_rolls_back_onto_a_test_build() {
    let t = t();
    on_test_build(&t);
    *t.ops.mgmt_health_ok.lock().unwrap() = false;
    let p = upgrade(&t);
    assert_eq!(p["result"], "rollback_skipped", "{p}");
    assert!(!t.ops.effects().iter().any(|e| e == "engine sew rollback"), "{:?}", t.ops.effects());
    let row = t.row();
    assert_eq!(row.status, "error");
    let m = row.last_error.expect("why");
    assert!(m.contains("does not roll back onto") && m.contains("restore"), "{m}");
}

/// An unknown binary: refused without the accept (the hint names its sha
/// and the flag); with it, only on the exact sha, only with a helper that
/// knows the argv, never on a release; then never a rollback target.
#[test]
fn accept_binary_upgrades_only_the_exact_unknown_binary() {
    use crate::sew::ops::fake::UNKNOWN_SHA;
    let t = t();
    t.enabled();
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    t.ops.clear();
    let req = UpgradeRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let e = upgrade_begin(&t.env(), &req).err().unwrap();
    assert_eq!(e.code, "unknown_sew_binary");
    assert!(e.hint.contains(UNKNOWN_SHA) && e.hint.contains("--accept-binary"), "{}", e.hint);
    let wrong = "1".repeat(64);
    let e = upgrade_begin(&t.env(), &UpgradeRequest { accept_binary: Some(wrong.clone()), ..req.clone() }).err().unwrap();
    assert_eq!(e.code, "accept_binary_mismatch", "{}", e.hint);
    assert!(e.hint.contains(UNKNOWN_SHA) && e.hint.contains(&wrong), "{}", e.hint);
    // An old helper: refused before anything, with the way out.
    *t.ops.helper_no_accept.lock().unwrap() = true;
    let accept = UpgradeRequest { accept_binary: Some(UNKNOWN_SHA.into()), ..req.clone() };
    let e = upgrade_begin(&t.env(), &accept).err().unwrap();
    assert_eq!(e.code, "helper_outdated", "{}", e.hint);
    assert!(e.hint.contains("k2 root-helper update"), "{}", e.hint);
    *t.ops.helper_no_accept.lock().unwrap() = false;
    assert!(t.ops.effects().is_empty(), "{:?}", t.ops.effects());
    assert_eq!(t.row().status, "running");
    // A release on disk: the flag is not for it.
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    let e = upgrade_begin(&t.env(), &UpgradeRequest { accept_binary: Some(UNKNOWN_SHA.into()), ..req.clone() }).err().unwrap();
    assert_eq!(e.code, "accept_binary_not_needed", "{}", e.hint);
    // The exact sha: upgraded through the accept argv.
    *t.ops.on_disk.lock().unwrap() = Some("unknown".into());
    let env = t.env();
    let (l, from) = upgrade_begin(&env, &accept).unwrap().unwrap();
    assert_eq!(from, format!("accepted-{}", &UNKNOWN_SHA[..12]));
    let p = upgrade_machine(&env, &accept, &from);
    drop(l);
    assert_eq!(p["result"], "done", "{p}");
    let eff = t.ops.effects();
    assert!(eff.iter().any(|e| *e == format!("engine sew upgrade accept-binary {UNKNOWN_SHA}")), "{eff:?}");
    assert!(!eff.iter().any(|e| e == "engine sew upgrade"), "{eff:?}");
    let row = t.row();
    assert_eq!(row.installed_version.as_deref(), Some(pinned().version));
    assert_eq!(row.previous_version.as_deref(), Some(from.as_str()));
    t.ops.clear();
    assert_eq!(rollback(&t.env()).err().unwrap().code, "rollback_refused");
    assert!(!t.ops.effects().iter().any(|e| e.contains("rollback")), "{:?}", t.ops.effects());
}
