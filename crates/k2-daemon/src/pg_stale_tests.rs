//! A Postgres server K2 runs, against its package's install time: faked
//! unit starts and package times, never a real box.

use std::sync::Mutex;

use super::fake::Fake;
use super::*;

const STARTED: i64 = 1_700_000_000;

fn sew_db_running(f: &Fake) {
    f.units.lock().unwrap().insert("sew-db.service".into(), (4242, STARTED));
    f.cmdlines
        .lock()
        .unwrap()
        .insert(4242, "/usr/lib/postgresql/17/bin/postgres -D /var/lib/sew/pg -c config_file=/etc/sew/pg.conf".into());
    f.versions.lock().unwrap().insert("/usr/lib/postgresql/17/bin/postgres".into(), "17.12".into());
}

#[test]
fn a_package_upgraded_after_the_postmaster_started_is_stale() {
    let f = Fake::default();
    sew_db_running(&f);
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED + 3600);
    let got = check_sew_db(&f).expect("sew-db runs");
    assert_eq!(got.major, Some(17));
    assert_eq!(got.package.as_deref(), Some("postgresql-17"));
    assert_eq!((got.installed_at, got.installed_from), (Some(STARTED + 3600), Some("dpkg")));
    assert_eq!(got.installed_version.as_deref(), Some("17.12"));
    assert_eq!(got.stale, Some(true));
    assert!(got.line().contains("sudo systemctl restart sew-db.service"), "{}", got.line());
    assert_eq!(got.json()["fix"], SEW_DB_FIX);
}

#[test]
fn a_package_installed_before_the_postmaster_started_is_current() {
    let f = Fake::default();
    sew_db_running(&f);
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED - 60);
    let got = check_sew_db(&f).expect("sew-db runs");
    assert_eq!(got.stale, Some(false));
    assert_eq!(got.json()["fix"], Value::Null, "no fix for a current server");
}

#[test]
fn the_install_time_falls_back_to_the_dpkg_list_then_the_binary() {
    let f = Fake::default();
    sew_db_running(&f);
    f.mtimes.lock().unwrap().insert("/var/lib/dpkg/info/postgresql-17.list".into(), STARTED + 10);
    f.mtimes.lock().unwrap().insert("/usr/lib/postgresql/17/bin/postgres".into(), STARTED - 999);
    let got = check_sew_db(&f).unwrap();
    assert_eq!((got.installed_from, got.stale), (Some("dpkg-list"), Some(true)));
    f.mtimes.lock().unwrap().remove("/var/lib/dpkg/info/postgresql-17.list");
    let got = check_sew_db(&f).unwrap();
    assert_eq!((got.installed_from, got.stale), (Some("binary"), Some(false)));
    f.mtimes.lock().unwrap().clear();
    let got = check_sew_db(&f).unwrap();
    assert_eq!((got.installed_from, got.stale), (None, None), "no time at all: K2 can't tell");
}

#[test]
fn a_unit_that_is_not_running_says_nothing() {
    let f = Fake::default();
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED + 3600);
    assert_eq!(check_sew_db(&f), None);
}

#[test]
fn the_major_comes_from_the_command_line_else_the_unit_name() {
    assert_eq!(major_of(Some("/usr/lib/postgresql/16/bin/postgres -D /x"), "sew-db.service"), Some(16));
    assert_eq!(major_of(None, "postgresql@17-feedback.service"), Some(17));
    assert_eq!(major_of(Some("postgres: 17/main: checkpointer"), "postgresql@17-main"), Some(17));
    assert_eq!(major_of(Some("/usr/bin/sew serve"), "sew-db.service"), None);
}

#[test]
fn systemctl_show_and_postgres_v_parse() {
    assert_eq!(parse_unit_start("MainPID=812\nExecMainStartTimestamp=@1700000000\n"), Some((812, 1_700_000_000)));
    assert_eq!(parse_unit_start("MainPID=0\nExecMainStartTimestamp=\n"), None, "not running");
    assert_eq!(parse_postgres_version("postgres (PostgreSQL) 17.12 (Ubuntu 17.12-1.pgdg24.04+1)\n").as_deref(), Some("17.12"));
    assert_eq!(parse_postgres_version("garbage"), None);
}

#[derive(Default)]
struct FakeNotices {
    sent: Mutex<Vec<String>>,
    notified: Mutex<Vec<(String, String)>>,
}

impl Notices for FakeNotices {
    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
    fn save_sent(&self, keys: &[String]) {
        *self.sent.lock().unwrap() = keys.to_vec();
    }
    fn notify(&self, title: &str, body: &str) {
        self.notified.lock().unwrap().push((title.into(), body.into()));
    }
}

#[test]
fn one_notice_per_stale_server_start() {
    let f = Fake::default();
    sew_db_running(&f);
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED + 3600);
    let n = FakeNotices::default();
    let findings = vec![check_sew_db(&f).unwrap()];
    assert_eq!(notice_new(&findings, &n).len(), 1);
    assert_eq!(notice_new(&findings, &n).len(), 0, "the same start is noticed once");
    let notes = n.notified.lock().unwrap().clone();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, "sew-db runs an outdated Postgres");
    // Restarted (fresh), then upgraded again under it: a new notice.
    f.units.lock().unwrap().insert("sew-db.service".into(), (4243, STARTED + 7200));
    f.cmdlines.lock().unwrap().insert(4243, "/usr/lib/postgresql/17/bin/postgres -D /var/lib/sew/pg".into());
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED + 9000);
    let again = vec![check_sew_db(&f).unwrap()];
    assert_eq!(notice_new(&again, &n).len(), 1);
    // A current server is never noticed.
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED);
    let fresh = vec![check_sew_db(&f).unwrap()];
    assert!(notice_new(&fresh, &n).is_empty());
    assert_eq!(n.notified.lock().unwrap().len(), 2);
}

#[test]
fn boot_status_lists_the_stale_ones_from_the_last_check() {
    let f = Fake::default();
    sew_db_running(&f);
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED + 1);
    remember(vec![check_sew_db(&f).unwrap()]);
    let v = boot_status_value();
    assert_eq!(v["stale"][0]["name"], "sew-db", "{v}");
    assert_eq!(v["stale"][0]["installedVersion"], "17.12");
    assert_eq!(last_stale().len(), 1);
    f.dpkg.lock().unwrap().insert("postgresql-17".into(), STARTED - 1);
    remember(vec![check_sew_db(&f).unwrap()]);
    assert_eq!(boot_status_value()["stale"], json!([]));
    assert!(last_stale().is_empty());
}
