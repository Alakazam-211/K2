//! Point-in-time restore (protocol 19, `namespaces.pgrestore`) over the
//! fake box of `pg_major_tests.rs`: `pg basebackup`, `pg backups`, `pg
//! restore-to` and `pg drop-drill`. Nothing real runs; the real-Postgres
//! drill is `pitr_real_pg.rs`.

use super::*;
use k2_root_proto::pg_restore::{self as pr, BackupInfo, RestoresState, UtcTime};

const SPOOL: &str = "/var/lib/postgresql/k2-wal/17-feedback";
const BASE: &str = "/var/lib/postgresql/k2-wal/17-feedback.base";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// The postgres-side tools the restore verbs add, against the temp root.
fn fake_tools(root: &Path, prog: &str, a: &[String], stdin: Option<&[u8]>) -> Option<ExecOut> {
    match prog {
        "pg_basebackup" => {
            let dir = under(root, arg_after(a, "-D").expect("-D"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("backup_manifest"),
                r#"{"PostgreSQL-Backup-Manifest-Version": 2, "Files": [], "WAL-Ranges": [{"Timeline": 1, "Start-LSN": "0/3000028", "End-LSN": "0/4000100"}]}"#,
            )
            .unwrap();
            std::fs::write(dir.join("base.tar.gz"), b"base").unwrap();
            std::fs::write(dir.join("pg_wal.tar.gz"), b"wal").unwrap();
            Some(out(0, ""))
        }
        "tar" => Some(out(0, "")),
        "tail" => Some(out(0, &std::fs::read_to_string(under(root, a.last().unwrap())).unwrap_or_default())),
        "pg_controldata" => Some(out(0, &format!("pg_control version number: 1300\nDatabase system identifier: {}\n", controldata_id(root)))),
        "dd" => {
            let of = a.iter().find_map(|x| x.strip_prefix("of=")).unwrap();
            if !of.starts_with("/var/lib/postgresql/") {
                return None;
            }
            let p = under(root, of);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, stdin.unwrap_or_default()).unwrap();
            Some(out(0, ""))
        }
        "rm" if a.iter().any(|x| x == "-rf") => {
            let p = under(root, a.last().unwrap());
            std::fs::remove_dir_all(p).unwrap();
            Some(out(0, ""))
        }
        _ => None,
    }
}

/// What the fake `pg_controldata` answers: `<root>/controldata-id` when a
/// test wrote one, else the id `pg_createcluster` gives every empty cluster.
fn controldata_id(root: &Path) -> String {
    std::fs::read_to_string(root.join("controldata-id")).map(|s| s.trim().to_string()).unwrap_or_else(|_| "7000000000000000001".into())
}

/// akzm's box plus `17/feedback` (port 5434) made with archiving, the
/// restore tools faked, the source answering a WAL switch and its
/// archiver, and a restore promoting at once.
fn archiving_box() -> (T, W) {
    let (t, w) = akzm_box();
    ok(&t.call(&["pg", "ensure-cluster", "17-feedback", "5434", "--wal-archive"]));
    let root = t.root.clone();
    let mut b = w.borrow_mut();
    b.extra = Some(Rc::new(move |prog, a, stdin| fake_tools(&root, prog, a, stdin)));
    b.answers.push(("pg_switch_wal".into(), "1\n000000010000000000000005\n".into()));
    b.answers.push(("pg_stat_archiver".into(), "000000010000000000000005\n".into()));
    b.answers.push(("pg_is_in_recovery".into(), "false\n".into()));
    b.answers.push(("pg_database_size".into(), "1048576\n".into()));
    b.answers.push(("rolcanlogin".into(), "3\n0\n".into()));
    drop(b);
    (t, w)
}

/// WAL files in the spool, archived at `at`.
fn spool(t: &T, segs: &[&str], at: i64) {
    mkdirs(&t.root, &[SPOOL]);
    for s in segs {
        let p = t.p(&format!("{SPOOL}/{s}"));
        std::fs::write(&p, b"wal").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(at as u64)).unwrap();
    }
}

/// A finished base backup of `17/feedback` that ended at `ended`.
fn base_backup(t: &T, ended: i64, start_wal: &str) -> String {
    let stamp = UtcTime::from_epoch(ended).stamp();
    let dir = format!("{BASE}/{stamp}");
    mkdirs(&t.root, &[dir.as_str()]);
    let info = BackupInfo {
        stamp: stamp.clone(),
        major: 17,
        name: "feedback".into(),
        system_id: named_rec(t, 17, "feedback").system_id,
        started_at: ended - 5,
        ended_at: ended,
        start_wal: start_wal.into(),
        end_wal: start_wal.into(),
        source_bytes: 1048576,
    };
    std::fs::write(t.p(&format!("{dir}/{}", pr::BACKUP_INFO_FILE)), serde_json::to_vec(&info).unwrap()).unwrap();
    for f in ["base.tar.gz", "pg_wal.tar.gz"] {
        std::fs::write(t.p(&format!("{dir}/{f}")), b"x").unwrap();
    }
    stamp
}

fn restores(t: &T) -> RestoresState {
    serde_json::from_slice(&std::fs::read(t.p(pr::RESTORES_STATE)).unwrap()).unwrap()
}

fn err_of(o: &crate::Outcome) -> String {
    assert_ne!(o.code, exit::OK, "expected a refusal, got {}", String::from_utf8_lossy(&o.stdout));
    o.stderr.clone()
}

fn created_clusters(t: &T) -> Vec<String> {
    t.non_query_calls().into_iter().filter(|c| c.starts_with(PG_CREATECLUSTER)).collect()
}

#[test]
fn version_announces_pgrestore() {
    let t = box_root();
    let v: serde_json::Value = serde_json::from_slice(&t.call(&["helper", "version"]).stdout).unwrap();
    assert_eq!(v["protocol"], k2_root_proto::version::PGRESTORE_MIN_PROTOCOL);
    assert_eq!(v["namespaces"]["pgrestore"], true, "{v}");
}

/// A base backup as postgres into the folder beside the spool, its info
/// from the manifest, and the keep: unfinished runs and the oldest beyond
/// seven go; the newest seven stay.
#[test]
fn basebackup_writes_its_info_and_keeps_seven() {
    let (t, w) = archiving_box();
    for i in 0..7 {
        base_backup(&t, 1_760_000_000 + i * 86_400, "000000010000000000000001");
    }
    mkdirs(&t.root, &[&format!("{BASE}/20250101T000000Z")]);
    let v = json(&t.call(&["pg", "basebackup", "17-feedback"]));
    for (k, want) in [("complete", json!(true)), ("startWal", json!("000000010000000000000003")), ("endWal", json!("000000010000000000000004")), ("name", json!("feedback"))] {
        assert_eq!(v[k], want, "{k}: {v}");
    }
    let stamp = v["stamp"].as_str().unwrap().to_string();
    let info: BackupInfo = serde_json::from_slice(&std::fs::read(t.p(&format!("{BASE}/{stamp}/{}", pr::BACKUP_INFO_FILE))).unwrap()).unwrap();
    assert_eq!(info.system_id, named_rec(&t, 17, "feedback").system_id);
    assert!(info.valid());
    let oldest = UtcTime::from_epoch(1_760_000_000).stamp();
    assert_eq!(v["pruned"], json!(["20250101T000000Z", oldest]), "{v}");
    assert!(!t.p(&format!("{BASE}/{oldest}")).exists());
    let calls = w.borrow().postgres_calls.clone();
    let bb = calls.iter().find(|c| c.contains("pg_basebackup")).expect("pg_basebackup ran as postgres");
    assert!(bb.starts_with("/usr/lib/postgresql/17/bin/pg_basebackup -h /var/run/postgresql -p 5434 "), "{bb}");
    for f in ["-Ft", "-z", "-X stream", &format!("-D {BASE}/{stamp}")] {
        assert!(bb.contains(f), "{f}: {bb}");
    }
    let listed = json(&t.call(&["pg", "backups", "17-feedback"]));
    assert_eq!(listed["baseBackups"].as_array().unwrap().len(), 7, "{listed}");
    // Nothing in the spool is ever removed or written by a backup.
    for c in t.non_query_calls().iter().chain(calls.iter()) {
        let in_spool = c.contains(&format!("{SPOOL}/")) || c.ends_with(SPOOL);
        assert!(!(in_spool && (c.contains("/usr/bin/rm") || c.contains("of="))), "{c}");
    }
    never_destructive(&t);
}

#[test]
fn basebackup_refuses_without_archiving_and_on_any_other_cluster() {
    let (t, _w) = akzm_box();
    ok(&t.call(&["pg", "ensure-cluster", "17-feedback", "5434"]));
    assert!(err_of(&t.call(&["pg", "basebackup", "17-feedback"])).starts_with("no_archive"));
    assert!(err_of(&t.call(&["pg", "basebackup", "17-k2db"])).starts_with("not_k2_cluster"));
    assert!(err_of(&t.call(&["pg", "basebackup", "17-ghost"])).starts_with("not_k2_cluster"));
    assert!(!t.p(BASE).exists());
}

/// A drill: a new cluster from the newest base backup before the time,
/// the source flushed and only read, the recovery settings gone after
/// promotion, the records; then drop-drill removes it and only it.
#[test]
fn restore_to_a_time_makes_a_new_cluster_and_drop_drill_removes_it() {
    let (t, w) = archiving_box();
    let n = now();
    let older = base_backup(&t, n - 7200, "000000010000000000000001");
    let stamp = base_backup(&t, n - 600, "000000010000000000000003");
    base_backup(&t, n - 30, "000000010000000000000005");
    spool(&t, &["000000010000000000000001", "000000010000000000000002", "000000010000000000000003", "000000010000000000000004", "000000010000000000000005"], n);
    let spool_before = tree(&t.p(SPOOL));
    let target = UtcTime::from_epoch(n - 60);
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &target.stamp(), "--drill"]));
    for (k, want) in [
        ("name", json!("fb_drill")),
        ("created", json!(true)),
        ("k2Named", json!(true)),
        ("walArchive", json!(false)),
        ("restoredFrom", json!("17/feedback")),
        ("target", json!(target.token())),
        ("baseBackup", json!(stamp)),
        ("drill", json!(true)),
    ] {
        assert_eq!(v[k], want, "{k}: {v}");
    }
    assert_ne!(v["baseBackup"], json!(older), "the newest backup before the time");
    assert_eq!(v["walReplayed"]["first"], "000000010000000000000003", "{v}");
    let port = v["port"].as_u64().unwrap() as u16;
    assert!(port != 5434 && port != 5433 && port != 5432, "{port}");
    // The source: flushed (a commit, a switch, the archiver read), its
    // spool byte-identical.
    let sql = w.borrow().sql.clone();
    assert!(sql.iter().any(|(p, s)| *p == 5434 && s == pr::SWITCH_SQL), "{sql:?}");
    assert_eq!(tree(&t.p(SPOOL)), spool_before, "the spool is only read");
    // The data: cleared, the base backup and its WAL unpacked, as postgres.
    let calls = w.borrow().postgres_calls.clone();
    let data = "/var/lib/postgresql/17/fb_drill";
    for want in [
        format!("/usr/bin/find {data} -mindepth 1 -delete"),
        format!("/usr/bin/tar -xzf {BASE}/{stamp}/base.tar.gz -C {data}"),
        format!("/usr/bin/tar -xzf {BASE}/{stamp}/pg_wal.tar.gz -C {data}/pg_wal"),
    ] {
        assert!(calls.contains(&want), "{want}: {calls:?}");
    }
    assert!(t.p(&format!("{data}/recovery.signal")).exists());
    // The recovery settings were there for the start and are gone now.
    assert!(!t.p(&format!("/etc/postgresql/17/fb_drill/conf.d/{}", pr::RESTORE_CONF_FILE)).exists());
    assert!(t.p("/etc/postgresql/17/fb_drill/conf.d/98-k2-cluster.conf").exists());
    assert!(!t.p("/etc/postgresql/17/fb_drill/conf.d/97-k2-wal-archive.conf").exists(), "a restore doesn't archive");
    assert!(!t.p("/etc/systemd/system/postgresql@17-fb_drill.service.d/k2-restore.conf").exists(), "the start's long wait is gone after promotion");
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/17/fb_drill/pg_ctl.conf")).unwrap().lines().last(), Some("pg_ctl_options = ''"));
    let rec = named_rec(&t, 17, "fb_drill");
    assert_eq!((rec.port, rec.wal_archive), (port, false));
    let r = restores(&t).get(17, "fb_drill").cloned().unwrap();
    assert!(r.ok && r.drill && r.from == "feedback" && r.base_backup == stamp, "{r:?}");
    // The source and the hand-made cluster can't be dropped.
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-feedback"])).starts_with("not_drill"));
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-k2db"])).starts_with("not_drill"));
    // The drill goes.
    let v = json(&t.call(&["pg", "drop-drill", "17-fb_drill"]));
    assert_eq!((v["dropped"].clone(), v["drill"].clone()), (json!(true), json!(true)), "{v}");
    // Review H1: the fixed path, as postgres, never across a mount; the
    // config dir through fsx; never pg_dropcluster.
    assert!(
        w.borrow().postgres_calls.iter().any(|c| c == &format!("/usr/bin/rm -rf --one-file-system -- {data}")),
        "{:?}",
        w.borrow().postgres_calls
    );
    assert!(!t.p(data).exists());
    assert!(!t.p("/etc/postgresql/17/fb_drill").exists());
    assert!(restores(&t).get(17, "fb_drill").is_none());
    let st: k2_root_proto::pg_named::NamedState =
        serde_json::from_slice(&std::fs::read(t.p(k2_root_proto::pg_named::NAMED_STATE)).unwrap()).unwrap();
    assert!(st.get(17, "fb_drill").is_none() && st.get(17, "feedback").is_some());
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-fb_drill"])).starts_with("not_drill"), "only once");
    assert_eq!(tree(&t.p(SPOOL)), spool_before);
    never_destructive(&t);
}

/// A finished restore that isn't a drill is never dropped.
#[test]
fn a_finished_restore_is_not_a_drill() {
    let (t, _w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_restored", &UtcTime::from_epoch(n - 60).stamp()]));
    assert_eq!(v["drill"], false);
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-fb_restored"])).starts_with("not_drill"));
    assert!(t.p("/var/lib/postgresql/17/fb_restored").exists());
}

/// Every refusal comes before anything is made: a hole in the spool, a
/// time later than the newest WAL (no silent "restored to the end"), a
/// time in the future, no backup ended by the time, a name in use.
#[test]
fn restore_refusals_make_nothing() {
    let (t, _w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    let t60 = UtcTime::from_epoch(n - 60).stamp();

    spool(&t, &["000000010000000000000003", "000000010000000000000005"], n);
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &t60, "--drill"]));
    assert!(e.starts_with("wal_gap: WAL segment 000000010000000000000004 is missing"), "{e}");

    std::fs::remove_dir_all(t.p(SPOOL)).unwrap();
    spool(&t, &["000000010000000000000003", "000000010000000000000004"], n - 300);
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &t60, "--drill"]));
    assert!(e.starts_with("target_after_wal"), "{e}");
    assert!(e.contains(&UtcTime::from_epoch(n - 300).token()), "names the newest WAL's time: {e}");

    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &UtcTime::from_epoch(n + 3600).stamp()]));
    assert!(e.starts_with("target_in_future"), "{e}");

    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &UtcTime::from_epoch(n - 3600).stamp()]));
    assert!(e.starts_with("no_base_backup"), "{e}");

    // The time fits now, but the name is the hand-made cluster's.
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "k2db", &UtcTime::from_epoch(n - 400).stamp()]));
    assert!(e.starts_with("name_taken"), "{e}");
    // A cluster the helper didn't make is never a source.
    let e = err_of(&t.call(&["pg", "restore-to", "17-k2db", "fb_x", &UtcTime::from_epoch(n - 400).stamp()]));
    assert!(e.starts_with("not_k2_cluster"), "{e}");
    assert!(created_clusters(&t).iter().all(|c| !c.contains("fb_x") && !c.contains(" k2db ")), "{:?}", created_clusters(&t));
    assert!(!t.p("/var/lib/postgresql/17/fb_x").exists());
}

/// Postgres ends recovery before the time (it stops): a loud failure with
/// the log's lines, the cluster left stopped and recorded as a failed
/// restore, which drop-drill may remove.
#[test]
fn recovery_that_stops_early_fails_loudly() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    w.borrow_mut().stay_down.insert("fb_bad".into());
    mkdirs(&t.root, &["/var/log/postgresql"]);
    std::fs::write(
        t.p("/var/log/postgresql/postgresql-17-fb_bad.log"),
        "LOG:  starting point-in-time recovery to 2026-11-22 10:15:00+00\nFATAL:  recovery ended before configured recovery target was reached\n",
    )
    .unwrap();
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_bad", &UtcTime::from_epoch(n - 60).stamp()]));
    assert!(e.starts_with("recovery_failed"), "{e}");
    assert!(e.contains("recovery ended before configured recovery target was reached"), "{e}");
    let r = restores(&t).get(17, "fb_bad").cloned().unwrap();
    assert!(!r.ok && !r.drill, "{r:?}");
    let st: k2_root_proto::pg_named::NamedState =
        serde_json::from_slice(&std::fs::read(t.p(k2_root_proto::pg_named::NAMED_STATE)).unwrap()).unwrap();
    assert!(st.get(17, "fb_bad").is_none(), "a failed restore is never a recorded cluster");
    let v = json(&t.call(&["pg", "drop-drill", "17-fb_bad"]));
    assert_eq!(v["dropped"], true, "{v}");
    assert!(!t.p("/var/lib/postgresql/17/fb_bad").exists());
}

/// `pg backups`: the window a restore can reach, and a gap named.
#[test]
fn backups_report_the_window_and_a_gap() {
    let (t, _w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003", "000000010000000000000004"], n - 10);
    let v = json(&t.call(&["pg", "backups", "17-feedback"]));
    assert_eq!(v["restorableFrom"], json!(UtcTime::from_epoch(n - 600).token()), "{v}");
    assert_eq!(v["restorableTo"], json!(UtcTime::from_epoch(n - 10).token()), "{v}");
    assert_eq!(v["wal"]["gap"], json!(null));
    assert_eq!(v["wal"]["count"], 2);
    spool(&t, &["000000010000000000000006"], n);
    let v = json(&t.call(&["pg", "backups", "17-feedback"]));
    assert_eq!(v["wal"]["gap"], json!("000000010000000000000005"), "{v}");
    assert_eq!(v["restorableTo"], json!(null));
}

/// A long replay: the unit's start errors (a wrapper's wait ran out) while
/// Postgres keeps running and replaying for many polls. That is not a
/// failure: the helper keeps waiting with no start timeout of its own (the
/// drop-in and pg_ctl.conf say so while it replays) and promotes.
#[test]
fn a_long_replay_is_waited_for_not_failed() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    {
        let mut b = w.borrow_mut();
        b.start_errs_but_runs.insert("fb_slow".into());
        b.recovery_polls = 500;
    }
    // While it starts, the long waits are in place (seen by the fake start).
    let r = t.root.clone();
    let seen = Rc::new(RefCell::new(None));
    let s2 = seen.clone();
    let prev = t.exec.0.borrow().programs.get(SYSTEMCTL).cloned().unwrap();
    t.exec.0.borrow_mut().programs.insert(
        SYSTEMCTL.into(),
        Rc::new(move |a| {
            if a.first().map(String::as_str) == Some("start") && a.get(1).is_some_and(|u| u.contains("fb_slow")) {
                let dropin = std::fs::read_to_string(under(&r, "/etc/systemd/system/postgresql@17-fb_slow.service.d/k2-restore.conf")).ok();
                let ctl = std::fs::read_to_string(under(&r, "/etc/postgresql/17/fb_slow/pg_ctl.conf")).ok();
                *s2.borrow_mut() = Some((dropin, ctl));
            }
            prev(a)
        }),
    );
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_slow", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert_eq!((v["created"].clone(), v["name"].clone()), (json!(true), json!("fb_slow")), "{v}");
    assert_eq!(w.borrow().recovery_polls, 0, "it waited through every replaying poll");
    let (dropin, ctl) = seen.borrow().clone().expect("the start ran");
    assert_eq!(dropin.as_deref(), Some(pr::restore_dropin()), "no start timeout while it replays");
    assert_eq!(ctl, Some(pr::restore_pg_ctl_conf()), "pg_ctlcluster waits as long as the replay may");
    assert!(!t.p("/etc/systemd/system/postgresql@17-fb_slow.service.d/k2-restore.conf").exists());
    assert!(restores(&t).get(17, "fb_slow").unwrap().ok);
    // drop-drill removes the drop-in folder's files too.
    ok(&t.call(&["pg", "drop-drill", "17-fb_slow"]));
    assert!(!t.p("/etc/systemd/system/postgresql@17-fb_slow.service.d/k2-slice.conf").exists());
}

// ── security review (protocol 19): H1, M1-M4, L5 ───────────────────────

/// H1: drop-drill never follows a link on the way to the paths it removes
/// and never runs pg_dropcluster: a linked /var/lib/postgresql/17 is
/// refused and nothing is removed.
#[test]
fn review_h1_drop_drill_refuses_a_link_above_and_never_runs_pg_dropcluster() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    // Someone running as postgres swaps a link in where root will look.
    let real = t.p("/var/lib/postgresql/17");
    std::fs::rename(&real, t.p("/var/lib/postgresql/17-real")).unwrap();
    symlink(t.p("/var/lib/postgresql/17-real"), &real).unwrap();
    let e = err_of(&t.call(&["pg", "drop-drill", "17-fb_drill"]));
    assert!(e.starts_with("cluster_layout_refused"), "{e}");
    assert!(t.p("/var/lib/postgresql/17-real/fb_drill").exists(), "nothing removed");
    assert!(t.p("/etc/postgresql/17/fb_drill").exists());
    assert!(!w.borrow().postgres_calls.iter().any(|c| c.contains("/usr/bin/rm")), "{:?}", w.borrow().postgres_calls);
    never_destructive(&t);
}

/// M1: a restore record never vouches for a later cluster of the same
/// name, and drop-drill also needs the cluster's own system identifier and
/// a recorded source.
#[test]
fn review_m1_a_reused_name_or_a_foreign_identity_is_never_dropped() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    // A restore that fails (Postgres never comes up)...
    w.borrow_mut().stay_down.insert("fb_x".into());
    err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &UtcTime::from_epoch(n - 60).stamp()]));
    assert!(restores(&t).get(17, "fb_x").is_some());
    // ...removed by hand, then a real cluster of that name made: the
    // record goes with the new cluster's creation.
    std::fs::remove_dir_all(t.p("/var/lib/postgresql/17/fb_x")).unwrap();
    std::fs::remove_dir_all(t.p("/etc/postgresql/17/fb_x")).unwrap();
    w.borrow_mut().stay_down.clear();
    ok(&t.call(&["pg", "ensure-cluster", "17-fb_x"]));
    assert!(restores(&t).get(17, "fb_x").is_none(), "a new cluster of that name forgets the old record");
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-fb_x"])).starts_with("not_drill"));
    assert!(t.p("/var/lib/postgresql/17/fb_x").exists());
    // A drill whose data has another identity than its source's or the one
    // it was made with: refused.
    json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    mkdirs(&t.root, &["/var/lib/postgresql/17/fb_drill/global"]);
    std::fs::write(t.p("/var/lib/postgresql/17/fb_drill/global/pg_control"), b"x").unwrap();
    std::fs::write(t.root.join("controldata-id"), "7999999999999999999").unwrap();
    let e = err_of(&t.call(&["pg", "drop-drill", "17-fb_drill"]));
    assert!(e.starts_with("not_drill") && e.contains("7999999999999999999"), "{e}");
    assert!(t.p("/var/lib/postgresql/17/fb_drill").exists());
    // Re-review M1: a refused drop stops nothing.
    assert!(!t.non_query_calls().iter().any(|c| c == &format!("{SYSTEMCTL} stop postgresql@17-fb_drill.service")), "{:?}", t.non_query_calls());
    // Its source's identity: dropped.
    std::fs::write(t.root.join("controldata-id"), named_rec(&t, 17, "feedback").system_id).unwrap();
    assert_eq!(json(&t.call(&["pg", "drop-drill", "17-fb_drill"]))["dropped"], true);
    // A record whose source is no longer recorded proves nothing.
    json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    let mut st: k2_root_proto::pg_named::NamedState =
        serde_json::from_slice(&std::fs::read(t.p(k2_root_proto::pg_named::NAMED_STATE)).unwrap()).unwrap();
    st.clusters.retain(|c| c.name != "feedback");
    std::fs::write(t.p(k2_root_proto::pg_named::NAMED_STATE), serde_json::to_vec(&st).unwrap()).unwrap();
    assert!(err_of(&t.call(&["pg", "drop-drill", "17-fb_drill"])).starts_with("not_drill"));
}

/// M2: a restore's cluster starts `manual` and turns `auto` once recorded;
/// a failure on any path after its layout (here the extract) leaves it
/// stopped, manual, with no recovery settings, start drop-in or wait.
#[test]
fn review_m2_a_failed_restore_cannot_start_or_promote_by_itself() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    // The extract fails (a `?` exit, not the recovery wait).
    let root = t.root.clone();
    w.borrow_mut().extra = Some(Rc::new(move |prog, a, stdin| {
        if prog == "tar" {
            return Some(ExecOut { status: 2, stdout: vec![], stderr: b"tar: unexpected EOF".to_vec() });
        }
        fake_tools(&root, prog, a, stdin)
    }));
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_bad", &UtcTime::from_epoch(n - 60).stamp()]));
    assert!(e.contains("tar"), "{e}");
    let etc = "/etc/postgresql/17/fb_bad";
    assert_eq!(std::fs::read_to_string(t.p(&format!("{etc}/start.conf"))).unwrap().lines().last(), Some("manual"));
    assert!(!t.p(&format!("{etc}/conf.d/{}", pr::RESTORE_CONF_FILE)).exists(), "no recovery settings left");
    assert!(!t.p("/etc/systemd/system/postgresql@17-fb_bad.service.d/k2-restore.conf").exists());
    assert_eq!(std::fs::read_to_string(t.p(&format!("{etc}/pg_ctl.conf"))).unwrap().lines().last(), Some("pg_ctl_options = ''"));
    assert!(t.non_query_calls().iter().any(|c| c == &format!("{SYSTEMCTL} stop postgresql@17-fb_bad.service")), "stopped");
    // drop-drill still removes it (its identity is the empty cluster's).
    assert_eq!(json(&t.call(&["pg", "drop-drill", "17-fb_bad"]))["dropped"], true);
    // A success: auto only after it is recorded.
    let root = t.root.clone();
    w.borrow_mut().extra = Some(Rc::new(move |prog, a, stdin| fake_tools(&root, prog, a, stdin)));
    json(&t.call(&["pg", "restore-to", "17-feedback", "fb_ok", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/17/fb_ok/start.conf")).unwrap().lines().last(), Some("auto"));
    assert!(t.non_query_calls().iter().any(|c| c.starts_with(PG_CREATECLUSTER) && c.contains("fb_ok") && c.contains("--start-conf manual")));
    // A drop-in folder with someone else's file: the name is taken.
    mkdirs(&t.root, &["/etc/systemd/system/postgresql@17-fb_other.service.d"]);
    std::fs::write(t.p("/etc/systemd/system/postgresql@17-fb_other.service.d/override.conf"), "[Service]\n").unwrap();
    assert!(err_of(&t.call(&["pg", "ensure-cluster", "17-fb_other"])).starts_with("name_taken"));
}

/// M3: a restore that will serve locks its logins until K2 sets their
/// current passwords; a drill listens on its socket only.
#[test]
fn review_m3_logins_locked_and_drills_local_only() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_serve", &UtcTime::from_epoch(n - 60).stamp()]));
    assert_eq!(v["loginsLocked"], 3, "{v}");
    let port = v["port"].as_u64().unwrap() as u16;
    assert!(w.borrow().sql.iter().any(|(p, s)| *p == port && s == pr::LOCK_LOGINS_SQL), "the lock ran on the restored cluster");
    assert!(!w.borrow().sql.iter().any(|(p, s)| *p == 5434 && s == pr::LOCK_LOGINS_SQL), "never on the source");
    assert!(!t.p(&format!("/etc/postgresql/17/fb_serve/conf.d/{}", pr::DRILL_LOCAL_CONF_FILE)).exists());
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert_eq!(v["loginsLocked"], 0);
    assert_eq!(std::fs::read_to_string(t.p(&format!("/etc/postgresql/17/fb_drill/conf.d/{}", pr::DRILL_LOCAL_CONF_FILE))).unwrap(), pr::DRILL_LOCAL_CONF);
    // A lock that leaves a login: the restore fails, and is cleaned up.
    w.borrow_mut().answers.retain(|(k, _)| k != "rolcanlogin");
    w.borrow_mut().answers.insert(0, ("rolcanlogin".into(), "3\n1\n".into()));
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_leak", &UtcTime::from_epoch(n - 60).stamp()]));
    assert!(e.starts_with("recovery_failed") && e.contains("logins could not all be locked"), "{e}");
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/17/fb_leak/start.conf")).unwrap().lines().last(), Some("manual"));
}

/// M4: no room for twice the source: refused before anything is written.
#[test]
fn review_m4_a_full_disk_is_refused_first() {
    let (mut t, _w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    t.ctx.free_bytes = |_| Ok(1024 * 1024);
    let e = err_of(&t.call(&["pg", "basebackup", "17-feedback"]));
    assert!(e.starts_with("disk_low"), "{e}");
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_x", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert!(e.starts_with("disk_low"), "{e}");
    assert!(!t.p("/var/lib/postgresql/17/fb_x").exists(), "nothing made");
}

/// L5: the restored postgresql.auto.conf loses commands and other servers.
#[test]
fn review_l5_auto_conf_is_stripped() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    let root = t.root.clone();
    w.borrow_mut().extra = Some(Rc::new(move |prog, a, stdin| {
        if prog == "tar" && a.iter().any(|x| x.ends_with("base.tar.gz")) {
            let dir = under(&root, a.last().unwrap());
            std::fs::write(dir.join("postgresql.auto.conf"), "work_mem = '8MB'\narchive_command = 'curl far'\n").unwrap();
        }
        fake_tools(&root, prog, a, stdin)
    }));
    json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert_eq!(std::fs::read_to_string(t.p("/var/lib/postgresql/17/fb_drill/postgresql.auto.conf")).unwrap(), "work_mem = '8MB'\n");
}

// ── re-review: N1, M2(a), M2(c) ─────────────────────────────────────────

/// What the cluster's conf.d and hba hold when its unit starts.
fn capture_start(t: &T, name: &str) -> Rc<RefCell<Vec<(String, Option<String>, Option<String>)>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (s2, r, n) = (seen.clone(), t.root.clone(), name.to_string());
    let prev = t.exec.0.borrow().programs.get(SYSTEMCTL).cloned().unwrap();
    t.exec.0.borrow_mut().programs.insert(
        SYSTEMCTL.into(),
        Rc::new(move |a| {
            if matches!(a.first().map(String::as_str), Some("start" | "restart")) && a.get(1).is_some_and(|u| u.contains(&format!("17-{n}."))) {
                let local = std::fs::read_to_string(under(&r, &format!("/etc/postgresql/17/{n}/conf.d/{}", pr::RESTORE_LOCAL_CONF_FILE))).ok();
                let hba = std::fs::read_to_string(under(&r, &format!("/etc/postgresql/17/{n}/{}", pr::RESTORE_HBA_FILE))).ok();
                s2.borrow_mut().push((a[0].clone(), local, hba));
            }
            prev(a)
        }),
    );
    seen
}

/// N1: every restore replays socket-only, with no sessions and postgres
/// alone; a serving one ends every other session after locking its logins
/// and only then restarts open; a drill stays socket-only.
#[test]
fn n1_restores_replay_socket_only_and_open_only_after_the_lock() {
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    let seen = capture_start(&t, "fb_serve");
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_serve", &UtcTime::from_epoch(n - 60).stamp()]));
    assert_eq!(v["socketOnly"], false, "{v}");
    let starts = seen.borrow().clone();
    assert_eq!(starts[0].0, "start");
    assert_eq!(starts[0].1.as_deref(), Some(pr::restore_local_conf(17, "fb_serve").as_str()), "replays socket-only, hot_standby off");
    assert_eq!(starts[0].2.as_deref(), Some(pr::RESTORE_HBA), "postgres alone");
    assert_eq!(starts.last().unwrap().0, "restart");
    assert_eq!(starts.last().unwrap().1, None, "restarts open only without the socket-only settings");
    // The order: lock, then every other session ended, then the restart.
    let port = v["port"].as_u64().unwrap() as u16;
    let sql: Vec<String> = w.borrow().sql.iter().filter(|(p, _)| *p == port).map(|(_, s)| s.clone()).collect();
    let lock = sql.iter().position(|s| s == pr::LOCK_LOGINS_SQL).expect("locked");
    let end = sql.iter().position(|s| s == pr::END_SESSIONS_SQL).expect("sessions ended");
    assert!(lock < end, "NOLOGIN before ending the sessions");
    let restart_at = t.non_query_calls().iter().position(|c| c == &format!("{SYSTEMCTL} restart postgresql@17-fb_serve.service")).expect("a restart");
    assert!(t.non_query_calls()[..restart_at].iter().all(|c| !c.contains("fb_serve") || !c.contains("restart")));
    assert!(!t.p(&format!("/etc/postgresql/17/fb_serve/conf.d/{}", pr::RESTORE_LOCAL_CONF_FILE)).exists());
    // A drill keeps it (and is never restarted open).
    let seen = capture_start(&t, "fb_drill");
    let v = json(&t.call(&["pg", "restore-to", "17-feedback", "fb_drill", &UtcTime::from_epoch(n - 60).stamp(), "--drill"]));
    assert_eq!(v["socketOnly"], true);
    assert!(seen.borrow().iter().all(|(op, local, _)| op == "start" && local.is_some()), "{:?}", seen.borrow());
    assert!(t.p(&format!("/etc/postgresql/17/fb_drill/conf.d/{}", pr::RESTORE_LOCAL_CONF_FILE)).exists());
    // A restore that dies (here: Postgres stops) keeps the socket-only
    // settings and the postgres-only hba: nothing else reaches it.
    w.borrow_mut().stay_down.insert("fb_dead".into());
    err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_dead", &UtcTime::from_epoch(n - 60).stamp()]));
    assert_eq!(
        std::fs::read_to_string(t.p(&format!("/etc/postgresql/17/fb_dead/conf.d/{}", pr::RESTORE_LOCAL_CONF_FILE))).unwrap(),
        pr::restore_local_conf(17, "fb_dead")
    );
    assert_eq!(std::fs::read_to_string(t.p(&format!("/etc/postgresql/17/fb_dead/{}", pr::RESTORE_HBA_FILE))).unwrap(), pr::RESTORE_HBA);
}

/// M2(a): the cleanup of a failed restore never waits on the pg lock: here
/// another caller holds it when the extract fails, and the cleanup still
/// runs in full.
#[test]
fn m2a_cleanup_runs_while_the_pg_lock_is_held() {
    use std::os::fd::AsRawFd;
    let (t, w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    let root = t.root.clone();
    let held: Rc<RefCell<Option<std::fs::File>>> = Rc::new(RefCell::new(None));
    let h2 = held.clone();
    w.borrow_mut().extra = Some(Rc::new(move |prog, a, stdin| {
        if prog == "tar" {
            let f = std::fs::File::open(under(&root, "/run/k2-root-helper.pg.lock")).unwrap();
            // SAFETY: a valid fd we own.
            assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "the test takes the pg lock");
            *h2.borrow_mut() = Some(f);
            return Some(ExecOut { status: 2, stdout: vec![], stderr: b"tar: unexpected EOF".to_vec() });
        }
        fake_tools(&root, prog, a, stdin)
    }));
    let e = err_of(&t.call(&["pg", "restore-to", "17-feedback", "fb_bad", &UtcTime::from_epoch(n - 60).stamp()]));
    assert!(held.borrow().is_some(), "the pg lock was held during the failure");
    assert!(!e.contains("cleanup left"), "{e}");
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/17/fb_bad/start.conf")).unwrap().lines().last(), Some("manual"));
    assert!(!t.p("/etc/systemd/system/postgresql@17-fb_bad.service.d/k2-restore.conf").exists());
    assert!(!t.p(&format!("/etc/postgresql/17/fb_bad/conf.d/{}", pr::RESTORE_CONF_FILE)).exists());
    assert!(t.non_query_calls().iter().any(|c| c == &format!("{SYSTEMCTL} stop postgresql@17-fb_bad.service")));
}

/// M2(c): an error after the restore is recorded finished never stops it.
#[test]
fn m2c_an_error_after_the_record_never_stops_a_finished_restore() {
    let (t, _w) = archiving_box();
    let n = now();
    base_backup(&t, n - 600, "000000010000000000000003");
    spool(&t, &["000000010000000000000003"], n);
    crate::pg_restore::test_seam::FAIL_AFTER_RECORD.with(|f| f.set(true));
    let o = t.call(&["pg", "restore-to", "17-feedback", "fb_done", &UtcTime::from_epoch(n - 60).stamp()]);
    crate::pg_restore::test_seam::FAIL_AFTER_RECORD.with(|f| f.set(false));
    assert!(o.stderr.contains("injected failure after the record"), "{}", o.stderr);
    assert!(restores(&t).get(17, "fb_done").unwrap().ok);
    assert!(!t.non_query_calls().iter().any(|c| c == &format!("{SYSTEMCTL} stop postgresql@17-fb_done.service")), "never stopped");
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/17/fb_done/start.conf")).unwrap().lines().last(), Some("auto"));
}
