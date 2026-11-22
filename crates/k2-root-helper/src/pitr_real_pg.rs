//! Point-in-time restore against a REAL Postgres (protocol 19). The helper's
//! own verbs run unchanged (`pg ensure-cluster --wal-archive`, `pg
//! basebackup`, `pg restore-to`, `pg drop-drill`); only the box under them is
//! a stand-in: Debian's `pg_createcluster`, `pg_lsclusters`, `systemctl` and
//! `runuser` become `initdb` and `pg_ctl` as the current user (drop-drill
//! removes the drill with the helper's own `rm` as postgres and fsx), with every
//! `/var/lib/postgresql`, `/etc/postgresql` and `/var/log/postgresql` path
//! moved under the test's temp root (also inside the cluster settings the
//! helper writes, such as `archive_command` and `restore_command`).
//!
//! The drill: a synthetic named cluster gets rows 1-3, a base backup, rows
//! 4-5, then a chosen minute passes, then rows 6-7. A restore to that minute
//! makes a new cluster holding rows 1-5 exactly; the source keeps 1-7. Then:
//! a spool with a missing segment is refused before anything is made, a
//! time later than the newest WAL is refused, and the drill cluster is
//! removed. Every check fails loudly.
//!
//! `#[ignore]`: it runs only where REALPG_PITR=1 is set, with a Postgres
//! server's binaries (`REALPG_BIN`, default `/usr/lib/postgresql/17/bin`;
//! `REALPG_MAJOR`, default 17), as an ordinary user:
//! `REALPG_PITR=1 cargo test -p k2-root-helper -- --ignored pitr_real_pg --nocapture`
//! It waits up to a minute for the chosen minute.

use super::*;
use std::process::{Command, Stdio};

use k2_root_proto::pg_major::{ENV as ENV_BIN, PG_SOCKET_DIR};
use k2_root_proto::pg_restore::UtcTime;

const MOVED: &[&str] = &["/var/lib/postgresql", "/etc/postgresql", "/var/log/postgresql"];

struct StandIn {
    root: PathBuf,
    sock: String,
    bin: String,
    /// (major, name, port) of the clusters `pg_createcluster` made.
    clusters: RefCell<Vec<(u32, String, u16)>>,
}

impl StandIn {
    fn map(&self, a: &str) -> String {
        if let Some(rest) = a.strip_prefix("of=") {
            return format!("of={}", self.map(rest));
        }
        for p in MOVED {
            if a == *p || a.starts_with(&format!("{p}/")) {
                return under(&self.root, a).to_string_lossy().into_owned();
            }
        }
        if a == PG_SOCKET_DIR {
            return self.sock.clone();
        }
        if let Some(rest) = a.strip_prefix("/usr/lib/postgresql/") {
            if let Some(tool) = rest.splitn(3, '/').nth(2) {
                return format!("{}/{tool}", self.bin);
            }
        }
        a.to_string()
    }
    fn rewrite(&self, text: &str) -> String {
        let mut t = text.to_string();
        for p in MOVED {
            t = t.replace(p, &under(&self.root, p).to_string_lossy());
        }
        t
    }
    fn data(&self, m: u32, n: &str) -> String {
        self.map(&format!("/var/lib/postgresql/{m}/{n}"))
    }
    fn log(&self, m: u32, n: &str) -> String {
        self.map(&format!("/var/log/postgresql/postgresql-{m}-{n}.log"))
    }
    fn online(&self, m: u32, n: &str) -> bool {
        Command::new(format!("{}/pg_ctl", self.bin))
            .args(["status", "-D", &self.data(m, n)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    fn lsclusters(&self) -> ExecOut {
        let s: String = self
            .clusters
            .borrow()
            .iter()
            // Like pg_lsclusters: a cluster is its config dir.
            .filter(|(m, n, _)| under(&self.root, &format!("/etc/postgresql/{m}/{n}")).exists())
            .map(|(m, n, p)| {
                format!(
                    "{m} {n} {p} {} postgres /var/lib/postgresql/{m}/{n} /var/log/postgresql/postgresql-{m}-{n}.log\n",
                    if self.online(*m, n) { "online" } else { "down" }
                )
            })
            .collect();
        out(0, &s)
    }
    fn port_of(&self, m: u32, n: &str) -> u16 {
        self.clusters.borrow().iter().find(|c| c.0 == m && c.1 == n).map(|c| c.2).expect("cluster")
    }
    /// `systemctl start`: the cluster's settings (conf.d, paths moved),
    /// then `pg_ctl start` and wait.
    fn start(&self, m: u32, n: &str) -> ExecOut {
        let data = self.data(m, n);
        let confd = under(&self.root, &format!("/etc/postgresql/{m}/{n}/conf.d"));
        let mut conf = format!(
            "data_directory = '{data}'\nport = {}\nunix_socket_directories = '{}'\nhba_file = '{data}/pg_hba.conf'\nident_file = '{data}/pg_ident.conf'\n",
            self.port_of(m, n),
            self.sock
        );
        let mut files: Vec<_> = std::fs::read_dir(&confd).unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        for f in files {
            conf.push_str(&self.rewrite(&std::fs::read_to_string(&f).unwrap()));
            conf.push('\n');
        }
        // No TCP listener on a shared node: the socket only.
        conf.push_str("listen_addresses = ''\n");
        let cf = self.root.join(format!("standin-{m}-{n}.conf"));
        std::fs::write(&cf, conf).unwrap();
        if n.starts_with("pitrslow") {
            // A slow start: the unit's start gives up at once (a wrapper's
            // wait ran out) and Postgres only comes up 8 s later, then
            // replays. The helper must wait, not report a failure.
            let (bin, log, cfs) = (self.bin.clone(), self.log(m, n), cf.display().to_string());
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(8));
                let _ = Command::new(format!("{bin}/pg_ctl"))
                    .args(["-D", &data, "-o", &format!("-c config_file={cfs}"), "-l", &log, "-W", "start"])
                    .output();
            });
            return ExecOut { status: 1, stdout: vec![], stderr: b"Job for the unit failed: timeout".to_vec() };
        }
        let o = Command::new(format!("{}/pg_ctl", self.bin))
            .args(["-D", &data, "-o", &format!("-c config_file={}", cf.display()), "-l", &self.log(m, n), "-w", "-t", "120", "start"])
            .output()
            .unwrap();
        ExecOut { status: o.status.code().unwrap_or(1), stdout: o.stdout, stderr: o.stderr }
    }
    fn stop(&self, m: u32, n: &str) {
        if self.online(m, n) {
            let _ = Command::new(format!("{}/pg_ctl", self.bin)).args(["-D", &self.data(m, n), "-m", "fast", "-w", "stop"]).output();
        }
    }
    fn psql(&self, m: u32, n: &str, sql: &str) -> String {
        let o = Command::new(format!("{}/psql", self.bin))
            .args(["-X", "-h", &self.sock, "-p", &self.port_of(m, n).to_string(), "-U", "postgres", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-tA", "-c", sql])
            .output()
            .unwrap();
        assert!(o.status.success(), "psql {m}/{n}: {sql}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }
}

/// Every cluster stops when the test ends, passed or not.
struct StopAll(Rc<StandIn>);
impl Drop for StopAll {
    fn drop(&mut self) {
        let cs = self.0.clusters.borrow().clone();
        for (m, n, _) in cs {
            self.0.stop(m, &n);
        }
        let _ = std::fs::remove_dir_all(&self.0.sock);
    }
}

fn run_real(prog: &str, args: &[String], envs: &[(String, String)], stdin: Option<&[u8]>) -> ExecOut {
    let mut c = Command::new(prog);
    c.args(args).env("PGUSER", "postgres").env("LC_ALL", "C").env("PSQLRC", "/dev/null");
    for (k, v) in envs {
        c.env(k, v);
    }
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().unwrap_or_else(|e| panic!("{prog}: {e}"));
    if let Some(b) = stdin {
        use std::io::Write;
        ch.stdin.take().unwrap().write_all(b).unwrap();
    }
    let o = ch.wait_with_output().unwrap();
    ExecOut { status: o.status.code().unwrap_or(1), stdout: o.stdout, stderr: o.stderr }
}

fn wire(t: &T, si: &Rc<StandIn>) {
    let mut log = t.exec.0.borrow_mut();
    let s = si.clone();
    log.programs_in.insert(
        RUNUSER.into(),
        Rc::new(move |a, stdin| {
            assert_eq!(&a[..3], &["-u", "postgres", "--"], "everything runs as postgres: {a:?}");
            let mut rest: Vec<String> = a[3..].to_vec();
            let mut envs = Vec::new();
            if rest[0] == ENV_BIN {
                rest.remove(0);
                while rest[0].contains('=') && !rest[0].starts_with('-') {
                    let kv = rest.remove(0);
                    let (k, v) = kv.split_once('=').unwrap();
                    envs.push((k.to_string(), v.to_string()));
                }
            }
            if rest[0] == PG_LSCLUSTERS {
                return s.lsclusters();
            }
            let prog = s.map(&rest[0]);
            let args: Vec<String> = rest[1..].iter().map(|x| s.map(x)).collect();
            let mut o = run_real(&prog, &args, &envs, stdin);
            if prog.ends_with("/stat") {
                // The stand-in's files are the test user's: read as postgres's.
                let me = String::from_utf8_lossy(&Command::new("id").arg("-un").output().unwrap().stdout).trim().to_string();
                o.stdout = String::from_utf8_lossy(&o.stdout).replace(&me, "postgres").into_bytes();
            }
            o
        }),
    );
    let s = si.clone();
    log.programs.insert(
        PG_CREATECLUSTER.into(),
        Rc::new(move |a| {
            let m: u32 = a[0].parse().unwrap();
            let n = a[1].clone();
            let p: u16 = arg_after(a, "-p").unwrap().parse().unwrap();
            let data = s.data(m, &n);
            let o = Command::new(format!("{}/initdb", s.bin))
                .args(["-D", &data, "-U", "postgres", "--auth-local=trust", "--auth-host=trust", "-E", "UTF8"])
                .output()
                .unwrap();
            if !o.status.success() {
                return ExecOut { status: 1, stdout: vec![], stderr: o.stderr };
            }
            let etc = under(&s.root, &format!("/etc/postgresql/{m}/{n}"));
            std::fs::create_dir_all(etc.join("conf.d")).unwrap();
            std::fs::write(etc.join("start.conf"), "auto\n").unwrap();
            std::fs::create_dir_all(under(&s.root, "/var/log/postgresql")).unwrap();
            s.clusters.borrow_mut().retain(|c| !(c.0 == m && c.1 == n));
            s.clusters.borrow_mut().push((m, n, p));
            out(0, "")
        }),
    );
    let s = si.clone();
    log.programs.insert(
        SYSTEMCTL.into(),
        Rc::new(move |a| {
            let op = a.first().map(String::as_str).unwrap_or("");
            let unit = a.get(1).cloned().unwrap_or_default();
            let Some(inst) = unit.strip_prefix("postgresql@").and_then(|u| u.strip_suffix(".service")) else {
                return out(0, "");
            };
            let (m, n) = inst.split_once('-').unwrap();
            let m: u32 = m.parse().unwrap();
            match op {
                "start" => s.start(m, n),
                "restart" => {
                    s.stop(m, n);
                    s.start(m, n)
                }
                "stop" => {
                    s.stop(m, n);
                    out(0, "")
                }
                "is-active" => out(0, if s.online(m, n) { "active\n" } else { "inactive\n" }),
                _ => out(0, ""),
            }
        }),
    );
    log.programs.insert(SS.into(), Rc::new(|_| out(0, "")));
}

fn ok_json(t: &T, argv: &[&str]) -> serde_json::Value {
    let o = t.call(argv);
    assert_eq!(o.code, exit::OK, "{argv:?}: {}", o.stderr);
    serde_json::from_slice(&o.stdout).unwrap()
}

fn refused(t: &T, argv: &[&str], code: &str) -> String {
    let o = t.call(argv);
    assert_ne!(o.code, exit::OK, "{argv:?} must be refused");
    assert!(o.stderr.starts_with(code), "{argv:?}: want {code}, got {}", o.stderr);
    o.stderr.clone()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn sleep_until(t: i64) {
    while now() < t {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
#[ignore = "needs Postgres binaries (REALPG_PITR=1, REALPG_BIN, REALPG_MAJOR)"]
fn pitr_real_pg() {
    assert_eq!(std::env::var("REALPG_PITR").as_deref(), Ok("1"), "set REALPG_PITR=1 to run the real-Postgres drill");
    let bin = std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into());
    let m: u32 = std::env::var("REALPG_MAJOR").ok().and_then(|v| v.parse().ok()).unwrap_or(17);
    for tool in ["initdb", "pg_ctl", "psql", "pg_basebackup", "postgres"] {
        assert!(Path::new(&format!("{bin}/{tool}")).exists(), "{bin}/{tool}: install Postgres {m} (REALPG_BIN)");
    }
    let mut t = box_root();
    t.ctx.waits = true;
    t.ctx.lock_timeout_ms = 5_000;
    bins(&t.root, m);
    for b in [PG_LSCLUSTERS, PG_CREATECLUSTER, SS] {
        touch(&t.root, b);
    }
    let sock = format!("/tmp/k2pitr-{}", std::process::id());
    std::fs::create_dir_all(&sock).unwrap();
    let si = Rc::new(StandIn { root: t.root.clone(), sock, bin, clusters: RefCell::new(Vec::new()) });
    let _stop = StopAll(si.clone());
    wire(&t, &si);
    let src = format!("{m}-pitrsrc");

    // A synthetic named cluster with archiving, and its first rows.
    let v = ok_json(&t, &["pg", "ensure-cluster", &src, "5446", "--wal-archive"]);
    assert_eq!((v["created"].clone(), v["walArchive"].clone()), (json!(true), json!(true)), "{v}");
    si.psql(m, "pitrsrc", "CREATE TABLE drill (id int PRIMARY KEY, at timestamptz NOT NULL DEFAULT clock_timestamp())");
    si.psql(m, "pitrsrc", "INSERT INTO drill (id) SELECT generate_series(1, 3)");
    // A base backup, then rows 4-5.
    let b = ok_json(&t, &["pg", "basebackup", &src]);
    assert_eq!(b["complete"], true, "{b}");
    println!("PASS base backup {} ({}..{})", b["stamp"], b["startWal"], b["endWal"]);
    si.psql(m, "pitrsrc", "INSERT INTO drill (id) VALUES (4), (5)");
    // The chosen minute passes; rows 6-7 come after it.
    let minute = (now() / 60 + 1) * 60;
    sleep_until(minute + 2);
    si.psql(m, "pitrsrc", "INSERT INTO drill (id) VALUES (6), (7)");
    let target = UtcTime::from_epoch(minute);
    println!("chosen minute {}", target.token());

    // Restore to that minute, as a drill: a new cluster with rows 1-5.
    let r = ok_json(&t, &["pg", "restore-to", &src, "pitrdrill", &target.stamp(), "--drill"]);
    assert_eq!((r["created"].clone(), r["k2Named"].clone(), r["restoredFrom"].clone()), (json!(true), json!(true), json!(format!("{m}/pitrsrc"))), "{r}");
    assert_eq!(si.psql(m, "pitrdrill", "SELECT count(*) || ':' || max(id) FROM drill"), "5:5", "the restore holds exactly the rows of that minute");
    assert_eq!(si.psql(m, "pitrdrill", "SELECT pg_is_in_recovery()::text"), "false", "promoted");
    assert_eq!(si.psql(m, "pitrsrc", "SELECT count(*) FROM drill"), "7", "the source is unchanged");
    assert!(
        si.psql(m, "pitrdrill", "SELECT max(at) FROM drill WHERE id = 5")
            < si.psql(m, "pitrsrc", "SELECT min(at) FROM drill WHERE id = 6"),
        "row 5 is before row 6"
    );
    assert!(!under(&t.root, &format!("/etc/postgresql/{m}/pitrdrill/conf.d/96-k2-restore.conf")).exists(), "the recovery settings are gone");
    println!("PASS restore to {}: rows 1-5, not 6-7", target.token());

    // A slow start (the unit's start errors, Postgres comes up 8 s later and
    // replays): waited for, never reported as a failure.
    let t0 = now();
    let r = ok_json(&t, &["pg", "restore-to", &src, "pitrslow", &target.stamp(), "--drill"]);
    assert_eq!(r["created"], true, "{r}");
    assert!(now() - t0 >= 8, "it waited for the late start");
    assert_eq!(si.psql(m, "pitrslow", "SELECT count(*) || ':' || max(id) FROM drill"), "5:5", "the slow restore holds the same rows");
    assert!(!under(&t.root, &format!("/etc/systemd/system/postgresql@{m}-pitrslow.service.d/k2-restore.conf")).exists());
    ok_json(&t, &["pg", "drop-drill", &format!("{m}-pitrslow")]);
    println!("PASS a slow start (start error, up 8 s later) is waited for, not failed");

    // A hole in the spool after the backup: refused before anything is made.
    for i in 8..11 {
        si.psql(m, "pitrsrc", &format!("INSERT INTO drill (id) VALUES ({i})"));
        si.psql(m, "pitrsrc", "SELECT pg_switch_wal()");
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    let spool = under(&t.root, &format!("/var/lib/postgresql/k2-wal/{src}"));
    let mut segs: Vec<String> = std::fs::read_dir(&spool)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 24)
        .collect();
    segs.sort();
    let start = b["startWal"].as_str().unwrap().to_string();
    let after: Vec<&String> = segs.iter().filter(|s| **s > start).collect();
    assert!(after.len() >= 3, "segments after the backup: {after:?}");
    let hole = after[after.len() / 2].clone();
    let aside = t.root.join("aside");
    std::fs::rename(spool.join(&hole), &aside).unwrap();
    let e = refused(&t, &["pg", "restore-to", &src, "pitrgap", &target.stamp(), "--drill"], "wal_gap");
    assert!(e.contains(&hole), "names the missing segment {hole}: {e}");
    assert!(!under(&t.root, &format!("/var/lib/postgresql/{m}/pitrgap")).exists(), "nothing made");
    std::fs::rename(&aside, spool.join(&hole)).unwrap();
    println!("PASS a missing segment ({hole}) is refused");

    // A time later than the newest WAL: refused (never "restored to the end").
    ok_json(&t, &["pg", "systemctl", "stop", &src]);
    std::thread::sleep(std::time::Duration::from_secs(3));
    let late = UtcTime::from_epoch(now() - 1);
    refused(&t, &["pg", "restore-to", &src, "pitrlate", &late.stamp(), "--drill"], "target_after_wal");
    assert!(!under(&t.root, &format!("/var/lib/postgresql/{m}/pitrlate")).exists(), "nothing made");
    println!("PASS a time after the newest WAL is refused");

    // The drill goes; the source can't be dropped.
    refused(&t, &["pg", "drop-drill", &src], "not_drill");
    let d = ok_json(&t, &["pg", "drop-drill", &format!("{m}-pitrdrill")]);
    assert_eq!(d["dropped"], true, "{d}");
    assert!(!under(&t.root, &format!("/var/lib/postgresql/{m}/pitrdrill")).exists());
    println!("PASS the drill cluster is removed");
}
