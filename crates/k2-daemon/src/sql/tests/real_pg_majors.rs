//! Real Postgres, two majors side by side (prd-pg17-and-db-upgrade-v1 S1/S3
//! drill). `#[ignore]`: it runs ONLY inside a throwaway container (Ubuntu
//! 24.04 + PGDG `postgresql-16` and `postgresql-17`) that holds two Debian
//! clusters and nothing else:
//!
//!   16/main on 5432 and 17/main on 5433, both online, scram, and
//!   REALPG_MAJORS=1 (a guard: the test refuses to run without it).
//!
//! It runs as root inside the container. The root helper's `pg psql
//! [--cluster M] -d <db>` is played by `runuser -u postgres -- psql
//! [--cluster M/main]` (what the protocol-9 helper does), and `pg
//! clusters` / `pg ensure-cluster` by `pg_lsclusters`. The daemon's own
//! client calls run unchanged: `-p <recorded port>` and that major's
//! binaries under /usr/lib/postgresql/<M>/bin.
//!
//! Run: `cargo test -p k2-daemon --lib real_pg_majors -- --ignored --nocapture`
//! with every K2_*/K2SO_* variable cleared. Every check prints PASS/FAIL;
//! the test fails if any failed.

use super::*;
use crate::sql::cluster;
use crate::sql::secrets::{MemSecretStore, SecretStore};
use crate::sql::sysops::{RealSystemOps, SystemOps};

struct ContainerOps;

/// `pg_lsclusters -h` lines → the helper's JSON shape.
fn lsclusters_json() -> Result<Vec<serde_json::Value>, String> {
    let out = RealSystemOps.run_cmd("/usr/bin/pg_lsclusters", &["-h"], &[], None)?;
    let mut v = Vec::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        v.push(serde_json::json!({
            "major": f[0].parse::<u64>().map_err(|e| format!("{line}: {e}"))?,
            "name": f[1],
            "port": f[2].parse::<u64>().map_err(|e| format!("{line}: {e}"))?,
            "running": f[3].starts_with("online"),
            "dataDir": f[5],
            "startConf": "auto",
        }));
    }
    Ok(v)
}

impl SystemOps for ContainerOps {
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
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        match args.first().copied() {
            Some("psql") => {
                let mut full: Vec<String> = vec!["-u".into(), "postgres".into(), "--".into(), "/usr/bin/psql".into(), "-X".into()];
                let mut rest = &args[1..];
                if rest.first().copied() == Some("--cluster") {
                    let m = rest.get(1).ok_or("--cluster without a major")?;
                    full.push("--cluster".into());
                    full.push(format!("{m}/main"));
                    rest = &rest[2..];
                }
                full.extend(rest.iter().map(|s| s.to_string()));
                let refs: Vec<&str> = full.iter().map(String::as_str).collect();
                let out = RealSystemOps.run_cmd("/usr/sbin/runuser", &refs, &[("PSQLRC", "/dev/null")], stdin)?;
                Ok(String::from_utf8_lossy(&out).trim().to_string())
            }
            Some("clusters") => Ok(serde_json::json!({ "clusters": lsclusters_json()? }).to_string()),
            Some("ensure-cluster") => {
                let m: u64 = args.get(1).and_then(|m| m.parse().ok()).ok_or("ensure-cluster needs a major")?;
                lsclusters_json()?
                    .into_iter()
                    .find(|c| c["major"] == m && c["name"] == "main")
                    .map(|c| c.to_string())
                    .ok_or_else(|| format!("no {m}/main in this container"))
            }
            _ => Err(format!("helper {} is not part of the drill", args.join(" "))),
        }
    }
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        RealSystemOps.run_cmd(cmd, args, &env, stdin)
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

/// `SHOW server_version_num` over a DSN with that major's psql.
fn server_major_via_dsn(dsn: &str, major_bin: u32) -> Result<u32, String> {
    let psql = format!("/usr/lib/postgresql/{major_bin}/bin/psql");
    let out = RealSystemOps.run_cmd(&psql, &["-X", "-tA", dsn, "-c", "SHOW server_version_num"], &[("PSQLRC", "/dev/null")], None)?;
    let n: u64 = String::from_utf8_lossy(&out).trim().parse().map_err(|e| format!("{e}"))?;
    k2_root_proto::pg_major::major_from_version_num(n).ok_or_else(|| format!("odd version {n}"))
}

fn databases_on(ops: &ContainerOps, major: u32) -> Result<Vec<String>, String> {
    let c = cluster::for_major(major).ok_or(format!("no {major} record"))?;
    let out = c.helper_psql(ops, "postgres", &["-tA"], Some(b"SELECT datname FROM pg_database ORDER BY 1;"))?;
    Ok(out.lines().map(str::to_string).collect())
}

#[test]
#[ignore = "runs only in a throwaway container with 16/main on 5432 and 17/main on 5433 (REALPG_MAJORS=1)"]
fn real_pg_majors_side_by_side() {
    assert_eq!(std::env::var("REALPG_MAJORS").as_deref(), Ok("1"), "REALPG_MAJORS=1 marks the throwaway container");
    let ops: &'static ContainerOps = Box::leak(Box::new(ContainerOps));
    let _home = crate::test_support::TempHome::new();
    let _g = sql_server_test_lock();
    k2_core::db::init_for_tests();
    seed_running_sidecar();
    let mut c = Checks(Vec::new());

    // The 5432 cluster as enable records it, then the helper's view folded in.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cluster::record_primary_major(&conn, 16).expect("record 16/main");
    }
    let folded = cluster::reconcile(ops);
    c.check(
        "records: 16 on 5432 (k2), 17 on 5433 seen",
        folded.and_then(|v| {
            let p = v.iter().find(|x| x.major == 16).ok_or("no 16")?;
            let s = v.iter().find(|x| x.major == 17).ok_or("no 17")?;
            want(p.port == 5432 && p.role == "k2" && s.port == 5433 && s.state == "online", format!("{v:?}"))
        }),
    );
    // A 17 cluster K2 didn't make is recorded adopted by the fold; this
    // container's 17 stands for one `ensure-cluster` created.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut s = cluster::for_major_in(&conn, 17).unwrap();
        s.role = "k2".into();
        s.origin = "created".into();
        cluster::record_in(&conn, &s, true).unwrap();
    }

    let secrets = MemSecretStore::default();
    let mk = |tag: &str| {
        let dir = std::env::temp_dir().join(format!("k2-realpgm-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".k2/db/migrations")).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let handle = format!("rpm{tag}{}", &uuid::Uuid::new_v4().to_string()[..6]);
        (insert_project_handle(&format!("realpgm-{tag}"), &path, &handle), path)
    };
    let (pa, path_a) = mk("a");
    let (pb, path_b) = mk("b");

    // A on 16 (explicit), B on the default (17).
    let a = ops::create_database_on(ops, &secrets, &pa, 1, None, None, Some(16));
    let b = ops::create_database(ops, &secrets, &pb, 1, None, None);
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => panic!("create failed: {a:?} / {b:?}"),
    };
    let name_a = a["name"].as_str().unwrap().to_string();
    let name_b = b["name"].as_str().unwrap().to_string();
    let dsn_a = a["dsn"].as_str().unwrap().to_string();
    let dsn_b = b["dsn"].as_str().unwrap().to_string();
    c.check("create: A on 16, B on 17 (default)", want(a["major"] == 16 && b["major"] == 17, format!("{a} {b}")));
    c.check("DSN ports follow the record", want(dsn_a.contains(":5432/") && dsn_b.contains(":5433/"), format!("{dsn_a} {dsn_b}")));
    c.check("A's DSN reaches a 16 server", server_major_via_dsn(&dsn_a, 16).and_then(|m| want(m == 16, format!("{m}"))));
    c.check("B's DSN reaches a 17 server", server_major_via_dsn(&dsn_b, 17).and_then(|m| want(m == 17, format!("{m}"))));
    c.check(
        "each database exists only on its own cluster",
        databases_on(ops, 16).and_then(|d16| {
            let d17 = databases_on(ops, 17)?;
            want(
                d16.contains(&name_a) && !d16.contains(&name_b) && d17.contains(&name_b) && !d17.contains(&name_a),
                format!("16: {d16:?} 17: {d17:?}"),
            )
        }),
    );

    // Migrations as each migrator, on its own cluster.
    let migration = "CREATE EXTENSION IF NOT EXISTS pgcrypto;\n\
        CREATE TABLE notes (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), owner uuid NOT NULL, body text);\n\
        ALTER TABLE notes ENABLE ROW LEVEL SECURITY;\n\
        CREATE POLICY notes_own ON notes FOR ALL TO PUBLIC USING (owner = k2.skin_uid()) WITH CHECK (owner = k2.skin_uid());\n";
    for p in [&path_a, &path_b] {
        std::fs::write(format!("{p}/.k2/db/migrations/0001_notes.sql"), migration).unwrap();
    }
    c.check("migrate A (16)", ops::migrate(ops, &secrets, &pa, &path_a, None).map(|_| ()).map_err(|e| format!("{e:?}")));
    c.check("migrate B (17)", ops::migrate(ops, &secrets, &pb, &path_b, None).map(|_| ()).map_err(|e| format!("{e:?}")));

    // The agent's own calls (exec_as_role: -p + that major's psql).
    c.check(
        "store put/get on B (17)",
        ops::store_put(ops, &secrets, &pb, "cards", "c1", &serde_json::json!({"t": "seventeen"}))
            .and_then(|_| ops::store_get(ops, &secrets, &pb, "cards", "c1", false))
            .map_err(|e| format!("{e:?}"))
            .and_then(|v| want(v.to_string().contains("seventeen"), v.to_string())),
    );
    c.check(
        "store put/get on A (16)",
        ops::store_put(ops, &secrets, &pa, "cards", "c1", &serde_json::json!({"t": "sixteen"}))
            .and_then(|_| ops::store_get(ops, &secrets, &pa, "cards", "c1", false))
            .map_err(|e| format!("{e:?}"))
            .and_then(|v| want(v.to_string().contains("sixteen"), v.to_string())),
    );

    // V39: 17's pg_dump for the 17 database (16's would refuse the server).
    c.check(
        "dump B with 17's pg_dump",
        ops::dump(ops, &secrets, &pb, &path_b, Some(".k2/db/dumps/b.dump"))
            .map_err(|e| format!("{e:?}"))
            .and_then(|_| {
                let len = std::fs::metadata(format!("{path_b}/.k2/db/dumps/b.dump")).map(|m| m.len()).unwrap_or(0);
                want(len > 0, format!("dump is {len} bytes"))
            }),
    );

    // V32: grant B (on 17) to workspace A: A's login (minted on 16) is
    // brought into 17 with its vaulted password and can connect.
    let grant = ops::grant_access(ops, &secrets, None, &name_b, &pa, "read", false, &[]);
    c.check("grant B (17) to A", grant.as_ref().map(|_| ()).map_err(|e| format!("{e:?}")));
    let login_a = ops::default_agent_role(&pa);
    c.check(
        "A's login connects to B on 5433 with its one password",
        (|| {
            let sref = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.query_row("SELECT agent_secret_ref FROM sql_databases WHERE name = ?1", [&name_a], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
            };
            let pw = secrets.resolve(&sref)?.ok_or("no vaulted password")?;
            let dsn = format!("postgres://{login_a}:{pw}@127.0.0.1:5433/{name_b}");
            server_major_via_dsn(&dsn, 17).and_then(|m| want(m == 17, format!("{m}")))
        })(),
    );

    // V50: the boot seal reaches each database on its own cluster.
    let lines = ops::seal_existing_databases(ops);
    c.check("boot seal: both sealed", want(lines.iter().all(|l| l.ends_with(": sealed")) && lines.len() >= 2, format!("{lines:?}")));

    c.check(
        "both clusters online together",
        lsclusters_json().and_then(|v| want(v.iter().filter(|x| x["running"] == true).count() >= 2, format!("{v:?}"))),
    );

    // Drop B: gone from 17, A untouched on 16.
    c.check("drop B", ops::drop_database(ops, &pb).map(|_| ()).map_err(|e| format!("{e:?}")));
    c.check(
        "after drop: B gone from 17, A still on 16",
        databases_on(ops, 17).and_then(|d17| {
            let d16 = databases_on(ops, 16)?;
            want(!d17.contains(&name_b) && d16.contains(&name_a), format!("16: {d16:?} 17: {d17:?}"))
        }),
    );
    c.check("drop A", ops::drop_database(ops, &pa).map(|_| ()).map_err(|e| format!("{e:?}")));

    let failed: Vec<&String> = c.0.iter().filter(|(_, r)| r.is_err()).map(|(n, _)| n).collect();
    println!("{} checks, {} failed", c.0.len(), failed.len());
    assert!(failed.is_empty(), "failed: {failed:?}");
}
