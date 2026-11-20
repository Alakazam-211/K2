//! The external ship (0.46.17): the box backup (K2 backups, restic)
//! carrying Sew's bundles clears `k2_backup_offbox_none` while its newest
//! snapshot is fresh, raises `k2_backup_offbox_stale` when it isn't, and a
//! box without it keeps today's alarm. The key-copy warning never moves.
//! Machines run inline over the fake box (`FakeOps`).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::sew::box_backup::{BoxBackupFacts, FileRead};
use crate::sew::ops::fake::FakeOps;
use crate::sew::ops::SewOps;
use crate::sew::routes::{handle_env, Who};
use crate::sew::secrets::MemSecrets;

const TARBALL: &str = "/home/k2/sew.tar.gz";
const HOUR: i64 = 3600;

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
    T { env, ops, _db: db }
}

impl T {
    fn status(&self) -> Value {
        let r = handle_env(self.env, "/cli/sew/status", &Who::Owner, b"", &HashMap::new(), false);
        assert_eq!(r.status.to_string(), "200 OK", "{}", r.body);
        serde_json::from_str(&r.body).unwrap()
    }
    fn ship(&self, body: Value) -> Value {
        let r = handle_env(self.env, crate::sew::s32::SHIP, &Who::Owner, body.to_string().as_bytes(), &HashMap::new(), false);
        assert_eq!(r.status.to_string(), "200 OK", "{}", r.body);
        serde_json::from_str(&r.body).unwrap()
    }
    fn enable(&self) {
        let body = json!({"tarballPath": TARBALL}).to_string();
        let r = handle_env(self.env, "/cli/sew/server/enable", &Who::Owner, body.as_bytes(), &HashMap::new(), false);
        assert_eq!(r.status.to_string(), "200 OK", "{}", r.body);
        self.ops.clear();
    }
    fn now(&self) -> i64 {
        *self.ops.clock.lock().unwrap()
    }
    /// The box backup covers `/var/lib` and its timer is on.
    fn on_backup_platform(&self) {
        *self.ops.box_backup.lock().unwrap() = BoxBackupFacts {
            paths: FileRead::Text { text: "/etc\n/home\n/var/lib\n".into() },
            excludes: FileRead::Text { text: "/var/lib/docker\n".into() },
            timer: "enabled".into(),
        };
    }
    /// systemd's last run of k2-backup.service.
    fn last_run(&self, result: &str, status: i32, finished_at: Option<i64>) {
        *self.ops.box_backup_run.lock().unwrap() =
            RunFacts { result: result.into(), exit_status: Some(status), finished_at, state: "inactive".into() };
    }
}

fn alarms(s: &Value) -> Vec<String> {
    serde_json::from_value(s["alarms"].clone()).expect("alarms")
}

#[test]
fn a_fresh_external_backup_clears_the_offbox_alarm() {
    let t = t();
    t.enable();
    t.on_backup_platform();
    let at = t.now() - 5 * HOUR;
    t.last_run("success", 0, Some(at));
    let s = t.status();
    let a = alarms(&s);
    assert!(!a.iter().any(|x| x == ALARM_NONE), "{a:?}");
    assert!(!a.iter().any(|x| x == ALARM_STALE), "{a:?}");
    assert_eq!(a, Vec::<String>::new(), "{s}");
    assert_eq!(s["state"], "running", "{s}");
    let ship = &s["backup"]["ship"];
    assert_eq!(ship["kind"], EXTERNAL, "{ship}");
    assert_eq!(ship["setting"], "none", "{ship}");
    assert_eq!(ship["label"], LABEL, "{ship}");
    assert_eq!(ship["lastShippedAt"], at, "{ship}");
    assert_eq!(ship["path"], Value::Null, "old field kept: {ship}");
    assert_eq!(ship["boxBackup"]["ok"], true, "{ship}");
    let x = &ship["external"];
    assert_eq!(x["fresh"], true, "{x}");
    assert_eq!(x["ageHours"], 5, "{x}");
    assert_eq!(x["staleAfterHours"], STALE_HOURS, "{x}");
    assert_eq!(x["lastRun"]["ok"], true, "{x}");
    assert_eq!(x["line"], "K2 backups (restic) carry Sew's bundles off the box (newest snapshot 5 h ago)", "{x}");
    // moveOffK2 counts it as the bundle copy and names it; the keys stay
    // missing on purpose.
    let mo = &s["moveOffK2"];
    assert_eq!(mo["missing"], json!(["keyring_mirror"]), "{mo}");
    assert_eq!(mo["offbox"], LABEL, "{mo}");
    let line = mo["line"].as_str().expect("line");
    assert!(line.contains("(bundles off the box in K2 backups (restic))"), "{line}");
    assert!(line.contains("leave the keys out until key escrow lands"), "{line}");
    // K2 kept the run: a reboot (systemd forgets) or a later failed run
    // doesn't lose the newest good snapshot.
    assert_eq!(t.ops.record_load(RUN_RECORD).expect("run record")["lastOkAt"], at);
    t.last_run("exit-code", 1, Some(t.now() - HOUR));
    let s = t.status();
    assert_eq!(s["backup"]["ship"]["lastShippedAt"], at, "{s}");
    assert_eq!(s["backup"]["ship"]["external"]["lastRun"]["ok"], false, "{s}");
    assert!(!alarms(&s).iter().any(|x| x.starts_with("k2_backup_offbox")), "{s}");
    *t.ops.box_backup_run.lock().unwrap() = RunFacts::default();
    let s = t.status();
    assert_eq!(s["backup"]["ship"]["lastShippedAt"], at, "{s}");
    // `backups` shows the same ship.
    let r = handle_env(t.env, "/cli/sew/backups", &Who::Owner, b"", &HashMap::new(), false);
    let b: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(b["ship"]["kind"], EXTERNAL, "{b}");
    assert_eq!(b["ship"]["external"]["fresh"], true, "{b}");
    // A chosen `box-backup` keeps its kind (older clients' picker) and
    // gains the same fields.
    let v = t.ship(json!({"kind": "box-backup"}));
    assert!(v["note"].as_str().unwrap().contains("k2_backup_offbox_none clears"), "{v}");
    let s = t.status();
    assert_eq!(s["backup"]["ship"]["kind"], "box-backup", "{s}");
    assert_eq!(s["backup"]["ship"]["setting"], "box-backup", "{s}");
    assert_eq!(s["backup"]["ship"]["external"]["fresh"], true, "{s}");
    assert!(!alarms(&s).iter().any(|x| x.starts_with("k2_backup_offbox")), "{s}");
}

#[test]
fn a_stale_external_backup_raises_offbox_stale() {
    let t = t();
    t.enable();
    t.on_backup_platform();
    let at = t.now() - (STALE_HOURS + 4) * HOUR;
    t.last_run("success", 0, Some(at));
    let s = t.status();
    let a = alarms(&s);
    assert!(a.iter().any(|x| x == ALARM_STALE), "{a:?}");
    assert!(!a.iter().any(|x| x == ALARM_NONE), "stale is not none: {a:?}");
    assert_eq!(s["state"], "degraded", "{s}");
    let ship = &s["backup"]["ship"];
    assert_eq!(ship["kind"], EXTERNAL, "{ship}");
    assert_eq!(ship["lastShippedAt"], at, "{ship}");
    assert_eq!(ship["external"]["fresh"], false, "{ship}");
    assert_eq!(ship["external"]["ageHours"], STALE_HOURS + 4, "{ship}");
    let line = ship["external"]["line"].as_str().expect("line");
    assert!(line.contains("is 40 h old (over 36 h)") && line.contains("k2-backup.service"), "{line}");
    // Exactly at the limit is still fresh; one second over is stale.
    t.last_run("success", 0, Some(t.now() - STALE_HOURS * HOUR));
    assert!(!alarms(&t.status()).iter().any(|x| x == ALARM_STALE));
    t.ops.records.lock().unwrap().remove(RUN_RECORD);
    t.last_run("success", 0, Some(t.now() - STALE_HOURS * HOUR - 1));
    assert!(alarms(&t.status()).iter().any(|x| x == ALARM_STALE));
    // No good run K2 has ever seen: stale too, and the line says why.
    t.ops.records.lock().unwrap().remove(RUN_RECORD);
    t.last_run("exit-code", 3, Some(t.now() - HOUR));
    let s = t.status();
    assert!(alarms(&s).iter().any(|x| x == ALARM_STALE), "{s}");
    assert_eq!(s["backup"]["ship"]["lastShippedAt"], Value::Null, "{s}");
    assert!(s["backup"]["ship"]["external"]["line"].as_str().unwrap().contains("hasn't seen a good run"), "{s}");
    // A fresh run clears it.
    t.last_run("success", 0, Some(t.now() - 60));
    assert!(!alarms(&t.status()).iter().any(|x| x.starts_with("k2_backup_offbox")));
}

#[test]
fn no_backup_platform_keeps_todays_offbox_none() {
    let t = t();
    t.enable();
    // The fake's default: no /etc/k2-backup, the timer not found. A run in
    // systemd (left from something else) counts for nothing.
    t.last_run("success", 0, Some(t.now() - HOUR));
    let s = t.status();
    let a = alarms(&s);
    assert!(a.iter().any(|x| x == ALARM_NONE), "{a:?}");
    assert!(!a.iter().any(|x| x == ALARM_STALE), "{a:?}");
    assert_eq!(s["state"], "degraded", "{s}");
    let ship = &s["backup"]["ship"];
    assert_eq!(ship["kind"], "none", "{ship}");
    assert_eq!(ship["setting"], "none", "{ship}");
    assert_eq!(ship["external"], Value::Null, "{ship}");
    assert!(ship.get("boxBackup").is_none(), "{ship}");
    assert!(ship.get("label").is_none(), "{ship}");
    assert_eq!(s["moveOffK2"]["missing"], json!(["ship_folder", "keyring_mirror"]), "{s}");
    assert_eq!(s["moveOffK2"]["offbox"], Value::Null, "{s}");
    assert!(s["moveOffK2"]["line"].as_str().unwrap().contains("an off-box backup folder (k2 hostchat ship dir"), "{s}");
    assert!(t.ops.record_load(RUN_RECORD).is_none(), "nothing recorded without the platform");
    // Paths that miss Sew's folder: still none, whatever systemd says.
    *t.ops.box_backup.lock().unwrap() =
        BoxBackupFacts { paths: FileRead::Text { text: "/etc\n/home\n".into() }, excludes: FileRead::Missing, timer: "enabled".into() };
    let a = alarms(&t.status());
    assert!(a.iter().any(|x| x == ALARM_NONE), "{a:?}");
    assert!(!a.iter().any(|x| x == ALARM_STALE), "{a:?}");
    // A dir ship never reads the box backup's run.
    t.on_backup_platform();
    crate::sew::store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.ship_path = Some("/mnt/offbox/sew".into());
    })
    .unwrap();
    let s = t.status();
    assert!(!alarms(&s).iter().any(|x| x.starts_with("k2_backup_offbox")), "{s}");
    assert_eq!(s["backup"]["ship"]["kind"], "dir", "{s}");
    assert_eq!(s["backup"]["ship"]["external"], Value::Null, "{s}");
}

#[test]
fn the_key_copy_warning_is_unchanged_by_the_external_backup() {
    let t = t();
    t.enable();
    {
        let mut st = t.ops.mgmt_status.lock().unwrap();
        st["alarms"] = json!(["keys_mirror_stale"]);
        st["keys"]["mirror"] = json!({"kind": "none", "stale": true, "pending": 0});
    }
    let before = t.status();
    let w = before["warnings"].as_array().expect("warnings").iter().find(|w| w["code"] == "no_offbox_key_copy").cloned().expect("no_offbox_key_copy");
    assert_eq!(w["alarm"], "keys_mirror_stale");
    t.on_backup_platform();
    t.last_run("success", 0, Some(t.now() - HOUR));
    let after = t.status();
    let w2 = after["warnings"].as_array().unwrap().iter().find(|w| w["code"] == "no_offbox_key_copy").cloned().expect("still warned");
    assert_eq!(w, w2, "the same warning, word for word");
    assert!(!alarms(&after).iter().any(|a| a == "keys_mirror_stale"), "a warning, not an alarm: {after}");
    assert_eq!(after["state"], "running", "{after}");
    assert!(after["moveOffK2"]["missing"].as_array().unwrap().iter().any(|m| m == "keyring_mirror"), "{after}");
}

#[test]
fn ship_none_says_the_box_backup_still_carries_the_bundles() {
    let t = t();
    t.enable();
    let v = t.ship(json!({"kind": "none"}));
    assert!(v.get("note").is_none_or(Value::is_null), "no platform, no note: {v}");
    t.on_backup_platform();
    let v = t.ship(json!({"kind": "none"}));
    assert_eq!(v["note"], NONE_BUT_CARRIED, "{v}");
}

#[test]
fn systemctl_show_parses_both_timestamp_forms() {
    let unix = "Result=success\nExecMainStatus=0\nExecMainExitTimestamp=@1795000000\nActiveState=inactive\n";
    assert_eq!(
        parse_show(unix),
        RunFacts { result: "success".into(), exit_status: Some(0), finished_at: Some(1_795_000_000), state: "inactive".into() }
    );
    assert!(parse_show(unix).good());
    let utc = "Result=exit-code\nExecMainStatus=1\nExecMainExitTimestamp=Thu 2026-11-19 07:41:02 UTC\nActiveState=failed\n";
    let f = parse_show(utc);
    assert_eq!(f.finished_at, Some(1_795_074_062), "{f:?}");
    assert!(!f.good());
    // Never ran since boot: nothing.
    let f = parse_show("Result=success\nExecMainStatus=0\nExecMainExitTimestamp=\nActiveState=inactive\n");
    assert_eq!(f.finished_at, None);
    assert!(!f.good(), "no end time is no good run");
    assert_eq!(parse_timestamp("n/a"), None);
    assert_eq!(parse_timestamp("@0"), None);
    assert_eq!(parse_timestamp("Thu 2026-11-19 07:41:02 MST"), None, "only UTC (K2 sets TZ=UTC)");
    assert_eq!(parse_show(""), RunFacts::default());
}
