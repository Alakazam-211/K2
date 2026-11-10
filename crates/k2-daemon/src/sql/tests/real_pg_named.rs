//! Real Postgres 17: a named cluster beside a hand-made one
//! (prd-named-pg-clusters-v1 S5). `#[ignore]`: it runs only where
//! REALPG_NAMED=1 is set and Postgres 17's binaries are there
//! (`REALPG_BIN`, default `/usr/lib/postgresql/17/bin`; a compute job may
//! unpack a relocatable 17 build and point at its `bin`), as an ordinary
//! user, with
//! two throwaway servers it makes itself under a temp dir:
//!
//! - **17/k2db** (port 5446): the stand-in for akzm's hand-made staging
//!   cluster. It gets a role, a database and a row, is stopped, and every
//!   file of it is hashed. At the end every file is byte-identical and no
//!   call ever named it.
//! - **17/feedback** (port 5447): made when the daemon asks the helper
//!   stand-in for `ensure-cluster 17-feedback 5447 --wal-archive`.
//!
//! The helper stand-in ([`NamedOps::run_helper`]) answers `clusters`,
//! `ensure-cluster <M>-<name>`, `psql --cluster <M>-<name>` and `systemctl
//! restart <M>-<name>` the way the protocol-15 root helper does (the
//! helper's own refusals, `name_taken` / `not_k2_cluster`, for anything it
//! didn't make), with `initdb`, `pg_ctl` and `psql` against those servers.
//! The archive conf is the helper's canned template, with only the spool
//! moved under the temp dir. The daemon's own client calls (the agent's
//! DSN, `exec_as_role`) run unchanged.
//!
//! Run: `REALPG_NAMED=1 cargo test -p k2-daemon --lib real_pg_named -- --ignored --nocapture`
//! with every K2_* variable cleared. Every check prints PASS/FAIL; the test
//! fails if any failed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;
use crate::sql::cluster;
use crate::sql::secrets::MemSecretStore;
use crate::sql::sysops::{RealSystemOps, SystemOps};

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}
const K2DB_PORT: u16 = 5446;
const FEEDBACK_PORT: u16 = 5447;

struct NamedOps {
    root: PathBuf,
    /// name → port of the clusters the stand-in helper made.
    made: Mutex<HashMap<String, u16>>,
    calls: Mutex<Vec<String>>,
}

fn run(prog: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
    let out = RealSystemOps.run_cmd(prog, args, &[("PSQLRC", "/dev/null"), ("LC_ALL", "C")], stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

impl NamedOps {
    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    fn data(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn start(&self, name: &str, port: u16) -> Result<(), String> {
        let d = self.data(name).to_string_lossy().into_owned();
        let opts = format!("-p {port} -k {} -c listen_addresses=localhost", self.sock());
        run(&format!("{}/pg_ctl", bin()), &["-D", &d, "-o", &opts, "-l", &format!("{d}.log"), "-w", "start"], None).map(|_| ())
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        let d = self.data(name).to_string_lossy().into_owned();
        run(&format!("{}/pg_ctl", bin()), &["-D", &d, "-m", "fast", "-w", "stop"], None).map(|_| ())
    }
    fn initdb(&self, name: &str) -> Result<(), String> {
        let d = self.data(name).to_string_lossy().into_owned();
        run(
            &format!("{}/initdb", bin()),
            &["-D", &d, "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"],
            None,
        )
        .map(|_| ())
    }
    fn psql(&self, port: u16, db: &str, flags: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let p = port.to_string();
        let sock = self.sock();
        let mut a: Vec<&str> = vec!["-X", "-h", &sock, "-p", &p, "-U", "postgres", "-d", db];
        a.extend_from_slice(flags);
        run(&format!("{}/psql", bin()), &a, stdin)
    }
    fn json_of(&self, name: &str, port: u16, running: bool, wal: bool) -> serde_json::Value {
        serde_json::json!({
            "major": 17, "name": name, "port": port,
            "dataDir": k2_root_proto::pg_named::data_dir(17, name),
            "running": running, "startConf": "auto",
            "k2Named": self.made.lock().unwrap().contains_key(name),
            "walArchive": wal,
        })
    }
}

impl SystemOps for NamedOps {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        RealSystemOps.write_file(path, contents, mode)
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        RealSystemOps.read_file(path)
    }
    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        RealSystemOps.create_dir_all(path)
    }
    fn remove_path(&self, path: &str) -> Result<(), String> {
        RealSystemOps.remove_path(path)
    }
    fn path_exists(&self, path: &str) -> bool {
        RealSystemOps.path_exists(path)
    }
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Err(format!("systemctl {} is not part of the drill", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        String::new()
    }
    fn helper_pgnamed(&self) -> Result<(), String> {
        Ok(())
    }
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        self.calls.lock().unwrap().push(args.join(" "));
        let named = |t: &str| -> Result<(String, u16), String> {
            let (_, n) = k2_root_proto::pg_named::parse_ref(t).ok_or(format!("refused arguments: {t}"))?;
            let port = self.made.lock().unwrap().get(n.as_str()).copied();
            port.map(|p| (n.as_str().to_string(), p))
                .ok_or_else(|| "not_k2_cluster: that cluster isn't one the root helper made; nothing changed".to_string())
        };
        match args {
            ["clusters"] => {
                let mut rows = vec![self.json_of("k2db", K2DB_PORT, false, false)];
                for (n, p) in self.made.lock().unwrap().clone() {
                    let wal = n == "feedback";
                    rows.push(self.json_of(&n, p, true, wal));
                }
                Ok(serde_json::json!({ "clusters": rows }).to_string())
            }
            ["ensure-cluster", r, rest @ ..] => {
                let (_, n) = k2_root_proto::pg_named::parse_ref(r).ok_or("refused arguments")?;
                let n = n.as_str().to_string();
                if n == "k2db" {
                    return Err("name_taken: a cluster or a data/config dir named k2db already exists on this box (any major); K2 never takes one over; nothing changed".into());
                }
                let wal = rest.contains(&"--wal-archive");
                let port = rest.iter().find_map(|t| t.parse::<u16>().ok()).unwrap_or(FEEDBACK_PORT);
                if port == K2DB_PORT {
                    return Err("port_taken: another cluster is configured on that port".into());
                }
                if !self.made.lock().unwrap().contains_key(&n) {
                    self.initdb(&n)?;
                    let confd = self.data(&n).join("conf.d");
                    std::fs::create_dir_all(&confd).map_err(|e| e.to_string())?;
                    let mut inc = String::from("include_dir = 'conf.d'\n");
                    std::fs::write(confd.join("98-k2-cluster.conf"), k2_root_proto::pg_major::cluster_conf().replace("listen_addresses = 'localhost'\n", ""))
                        .map_err(|e| e.to_string())?;
                    std::fs::write(confd.join("99-k2-ram.conf"), k2_root_proto::templates::pg_guc_conf()).map_err(|e| e.to_string())?;
                    if wal {
                        let spool = self.root.join("k2-wal");
                        std::fs::create_dir_all(spool.join(format!("17-{n}"))).map_err(|e| e.to_string())?;
                        let conf = k2_root_proto::pg_named::wal_archive_conf(17, &n)
                            .replace(k2_root_proto::pg_named::WAL_SPOOL_ROOT, &spool.to_string_lossy());
                        std::fs::write(confd.join(k2_root_proto::pg_named::WAL_ARCHIVE_CONF_FILE), conf).map_err(|e| e.to_string())?;
                    }
                    let pg_conf = self.data(&n).join("postgresql.conf");
                    let mut body = std::fs::read_to_string(&pg_conf).map_err(|e| e.to_string())?;
                    inc.insert_str(0, "\n");
                    body.push_str(&inc);
                    std::fs::write(&pg_conf, body).map_err(|e| e.to_string())?;
                    self.start(&n, port)?;
                    self.made.lock().unwrap().insert(n.clone(), port);
                }
                let mut v = self.json_of(&n, port, true, wal);
                v["created"] = serde_json::json!(true);
                Ok(v.to_string())
            }
            ["psql", "--cluster", r, rest @ ..] => {
                let (_, port) = named(r)?;
                let mut db = "postgres";
                let mut flags: Vec<&str> = Vec::new();
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-d" {
                        db = rest[i + 1];
                        i += 2;
                    } else {
                        flags.push(rest[i]);
                        i += 1;
                    }
                }
                self.psql(port, db, &flags, stdin)
            }
            ["systemctl", "restart", r] => {
                let (n, port) = named(r)?;
                self.stop(&n)?;
                self.start(&n, port)?;
                Ok(serde_json::json!({"unit": k2_root_proto::pg_named::unit(17, &n), "op": "restart", "online": true}).to_string())
            }
            _ => Err(format!("helper {} is not part of the drill", args.join(" "))),
        }
    }
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        // The daemon's client tools (`psql`, `pg_dump`…) are this drill's 17.
        let tool = cmd.rsplit('/').next().unwrap_or(cmd);
        let cmd = if ["psql", "pg_dump", "pg_restore"].contains(&tool) { format!("{}/{tool}", bin()) } else { cmd.to_string() };
        RealSystemOps.run_cmd(&cmd, args, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

struct Checks(Vec<(String, Result<(), String>)>);

impl Checks {
    fn check(&mut self, name: &str, r: Result<(), String>) {
        match &r {
            Ok(()) => println!("PASS {name}"),
            Err(e) => println!("FAIL {name}: {e}"),
        }
        self.0.push((name.to_string(), r));
    }
}

fn want(cond: bool, msg: String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg)
    }
}

/// Every file under a tree, path → bytes (sorted).
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, p: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut es: Vec<_> = std::fs::read_dir(p).unwrap().map(|e| e.unwrap().path()).collect();
        es.sort();
        for e in es {
            let m = std::fs::symlink_metadata(&e).unwrap();
            let rel = e.strip_prefix(base).unwrap().display().to_string();
            if m.is_dir() {
                out.push((format!("{rel}/"), vec![]));
                walk(base, &e, out);
            } else {
                out.push((rel, std::fs::read(&e).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v
}

#[test]
#[ignore = "runs only with REALPG_NAMED=1 and Postgres 17 binaries (a Linux compute job)"]
fn real_pg_named_cluster_beside_a_hand_made_one() {
    assert_eq!(std::env::var("REALPG_NAMED").as_deref(), Ok("1"), "REALPG_NAMED=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres 17 (REALPG_BIN)", bin());
    // Short: a Unix socket path must stay under 108 bytes.
    let root = PathBuf::from(format!("/tmp/k2rpn-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(&root).unwrap();
    struct Tmp(PathBuf);
    impl Tmp {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    let tmp = Tmp(root.clone());
    let ops: &'static NamedOps = Box::leak(Box::new(NamedOps {
        root: root.clone(),
        made: Mutex::new(HashMap::new()),
        calls: Mutex::new(Vec::new()),
    }));
    std::fs::create_dir_all(tmp.path().join("sock")).unwrap();
    let _home = crate::test_support::TempHome::new();
    let _g = sql_server_test_lock();
    k2_core::db::init_for_tests();
    seed_running_sidecar();
    let mut c = Checks(Vec::new());

    // 17/k2db: made by hand, holds staging data, then stopped and hashed.
    ops.initdb("k2db").expect("initdb k2db");
    ops.start("k2db", K2DB_PORT).expect("start k2db");
    ops.psql(
        K2DB_PORT,
        "postgres",
        &["-v", "ON_ERROR_STOP=1"],
        Some(b"CREATE ROLE staging LOGIN PASSWORD 'staging-pw';\nCREATE DATABASE staging_data OWNER staging;\n"),
    )
    .expect("k2db staging");
    ops.psql(K2DB_PORT, "staging_data", &["-v", "ON_ERROR_STOP=1"], Some(b"CREATE TABLE t (v text);\nINSERT INTO t VALUES ('staging row');\n"))
        .expect("k2db row");
    ops.stop("k2db").expect("stop k2db");
    let before = tree(&tmp.path().join("k2db"));

    // The helper's view: 17/k2db is recorded foreign.
    let folded = cluster::reconcile(ops);
    c.check(
        "17/k2db is recorded foreign",
        folded.and_then(|v| {
            let k = v.iter().find(|x| x.name == "k2db").ok_or("no k2db")?;
            want(k.role == "foreign" && k.port == K2DB_PORT, format!("{k:?}"))
        }),
    );
    let dir = std::env::temp_dir().join(format!("k2-realpgn-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let pid = insert_project_handle("realpgn", &path, &format!("rpn{}", &uuid::Uuid::new_v4().to_string()[..6]));

    // It can't be made, created into or restarted.
    c.check(
        "ensure 17/k2db: refused, nothing sent",
        match cluster::ensure_named_cluster(ops, 17, "k2db", None, false, &pid) {
            Err(e) => want(e.code() == "cluster_refused" && ops.calls.lock().unwrap().iter().all(|l| !l.contains("k2db")), format!("{e:?}")),
            Ok(c) => Err(format!("made {c:?}")),
        },
    );
    let secrets = MemSecretStore::default();
    c.check(
        "create in 17/k2db: refused",
        match ops::create_database_in_cluster(ops, &secrets, &pid, 5, None, Some("x"), None, "k2db") {
            Err(ops::OpsError::Refused(code, _)) => want(code == "cluster_refused", code.to_string()),
            other => Err(format!("{other:?}")),
        },
    );

    // 17/feedback through the helper stand-in.
    let fb = cluster::ensure_named_cluster(ops, 17, "feedback", Some(FEEDBACK_PORT), true, &pid);
    c.check(
        "17/feedback made and recorded K2's",
        fb.as_ref().map_err(|e| format!("{e:?}")).and_then(|c| {
            want(c.role == "k2" && c.origin == "created" && c.port == FEEDBACK_PORT && c.wal_archive, format!("{c:?}"))
        }),
    );
    let created = ops::create_database_in_cluster(ops, &secrets, &pid, 5, None, Some("feedback"), Some(17), "feedback");
    let dsn = created.as_ref().ok().and_then(|v| v["dsn"].as_str().map(str::to_string)).unwrap_or_default();
    c.check(
        "a database in it, DSN on its port",
        created
            .as_ref()
            .map_err(|e| format!("{e:?}"))
            .and_then(|v| want(v["cluster"] == "17/feedback" && dsn.contains(&format!(":{FEEDBACK_PORT}/feedback")), v.to_string())),
    );
    c.check(
        "the agent's DSN logs in to feedback as its agent role",
        run(&format!("{}/psql", bin()), &["-X", "-tA", &dsn, "-c", "SELECT current_user || ' ' || current_database()"], None)
            .and_then(|o| want(o == "feedback_agent feedback", o)),
    );
    c.check(
        "the agent's store works there (exec_as_role on the cluster's port)",
        ops::store_put(ops, &secrets, &pid, "cards", "c1", &serde_json::json!({"t": "named"}))
            .and_then(|_| ops::store_get(ops, &secrets, &pid, "cards", "c1", false))
            .map_err(|e| format!("{e:?}"))
            .and_then(|v| want(v.to_string().contains("named"), v.to_string())),
    );

    // k2-watch's health block: the fixed SQL runs on a real 17.
    let cl = cluster::for_name("feedback").expect("record");
    let h = cluster::health_of(ops, &cl);
    c.check(
        "health: connections, sizes, archive_mode on",
        want(
            h["online"] == true
                && h["maxConnections"].as_u64() == Some(50)
                && h["archiveMode"] == "on"
                && h["databases"].as_array().is_some_and(|d| d.iter().any(|x| x["name"] == "feedback" && x["bytes"].as_u64().unwrap_or(0) > 0)),
            h.to_string(),
        ),
    );
    // The canned archive command really archives (a forced segment switch).
    let _ = cl.helper_psql(ops, "postgres", &["-tA"], Some(b"SELECT pg_switch_wal();\n"));
    let spool = tmp.path().join("k2-wal/17-feedback");
    let mut archived = 0;
    for _ in 0..30 {
        archived = std::fs::read_dir(&spool).map(|d| d.count()).unwrap_or(0);
        if archived > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let h2 = cluster::health_of(ops, &cl);
    c.check(
        "WAL archive: a segment lands in the spool, the archiver counts it",
        want(archived > 0 && h2["archiver"]["archivedCount"].as_u64().unwrap_or(0) >= 1 && h2["archiver"]["failedCount"] == 0, format!("{archived} files; {h2}")),
    );
    c.check(
        "restart only that cluster, then it answers again",
        cluster::restart_named(ops, &cl)
            .map_err(|e| format!("{e:?}"))
            .and_then(|_| run(&format!("{}/psql", bin()), &["-X", "-tA", &dsn, "-c", "SELECT 1"], None))
            .and_then(|o| want(o == "1", o)),
    );
    // Upgrades leave it alone.
    let m = crate::sql::supervisor::majors_json();
    c.check(
        "no upgrade offered for a named cluster's database",
        want(m["upgradeAvailable"].as_array().unwrap().iter().all(|u| u["name"] != "feedback"), m.to_string()),
    );

    // 17/k2db: every file byte-identical; nothing ever named it.
    let after = tree(&tmp.path().join("k2db"));
    c.check(
        "every file of 17/k2db is unchanged",
        want(after == before, format!("{} files before, {} after", before.len(), after.len())),
    );
    let calls = ops.calls.lock().unwrap().clone();
    c.check(
        "no helper call named 17/k2db or its port",
        want(calls.iter().all(|l| !l.contains("17-k2db") && !l.contains(&K2DB_PORT.to_string())), format!("{calls:?}")),
    );
    let _ = ops.stop("feedback");
    let _ = std::fs::remove_dir_all(&root);

    let failed: Vec<&String> = c.0.iter().filter(|(_, r)| r.is_err()).map(|(n, _)| n).collect();
    assert!(failed.is_empty(), "failed: {failed:?}");
}
