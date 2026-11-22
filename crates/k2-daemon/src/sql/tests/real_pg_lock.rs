//! Real Postgres: the principal lock counts only when K2 wrote it, and the
//! boot pass brings an existing database's `k2` helpers up to date.
//! `#[ignore]`: it runs only where REALPG_LOCK=1 is set, with a Postgres
//! server's binaries (`REALPG_BIN`: `initdb`, `pg_ctl`, `postgres`, `psql`;
//! default `/usr/lib/postgresql/16/bin`; a compute job may unpack the
//! distro's packages and point at their `bin`), as an ordinary user.
//!
//! It makes its own throwaway server under `/tmp` on port 5461 (K2's band,
//! never the box's own). K2's own create, migrate, grant, guest-query and
//! boot-seal paths run unchanged; only `SystemOps` points at the throwaway
//! server (the root helper's superuser psql over its socket, the daemon's
//! psql over TCP). A grantee's own SQL runs over the socket as its role,
//! as it would on its own DSN.
//!
//! Checked:
//! - a `pg_temp.k2_principal_lock` the caller made itself is ignored by
//!   `k2.skin_uid()` (no rows of the named user), and doesn't outlive a
//!   later `k2.set_principal()` on that connection;
//! - the guest lane's lock (written by the migrator) still counts;
//! - the GUC stays the App's own lane (documented: a grantee can set which
//!   App user it acts as; per-user rules aren't a boundary for it);
//! - the boot pass (`seal_existing_databases`) replaces an older
//!   `k2.skin_uid()` in place, one line per database, changes nothing
//!   else, and a second run changes nothing at all.
//!
//! Run: `REALPG_LOCK=1 cargo test -p k2-daemon --lib real_pg_lock -- --ignored --nocapture --test-threads=1`
//! with every K2_* variable cleared. Every check prints PASS/FAIL; the test
//! fails if any did.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::sql::sysops::{RealSystemOps, SystemOps};

const PORT: u16 = 5461;

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/16/bin".into())
}

fn run(prog: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut e: Vec<(&str, &str)> = vec![("PSQLRC", "/dev/null"), ("LC_ALL", "C")];
    e.extend_from_slice(env);
    let out = RealSystemOps.run_cmd(prog, args, &e, stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

struct Server {
    root: PathBuf,
}

impl Server {
    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    fn data(&self) -> String {
        self.root.join("data").to_string_lossy().into_owned()
    }
    /// A script as `role` over the socket (trust): one session, the rows
    /// of every statement, one per line.
    fn script(&self, role: &str, db: &str, sql: &str) -> Result<Vec<String>, String> {
        let p = PORT.to_string();
        let out = run(
            &format!("{}/psql", bin()),
            &["-X", "-q", "-tA", "-h", &self.sock(), "-p", &p, "-U", role, "-d", db, "-v", "ON_ERROR_STOP=1"],
            &[],
            Some(sql.as_bytes()),
        )?;
        Ok(out.lines().map(str::to_string).collect())
    }
    fn su(&self, db: &str, sql: &str) -> Vec<String> {
        self.script("postgres", db, sql).unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sql}"))
    }
}

struct Stop(&'static Server);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.0.data(), "-m", "immediate", "-w", "stop"], &[], None);
        let _ = std::fs::remove_dir_all(&self.0.root);
    }
}

impl SystemOps for Server {
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
        Err(format!("systemctl {} is not part of the test", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    /// The root helper's `psql [--cluster M] -d <db> …`: superuser, SQL on
    /// stdin, over the throwaway server's socket.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of the test", args.join(" ")));
        }
        let mut rest: Vec<&str> = Vec::new();
        let mut it = args[1..].iter();
        while let Some(a) = it.next() {
            if *a == "--cluster" {
                it.next();
                continue;
            }
            rest.push(a);
        }
        let p = PORT.to_string();
        let sock = self.sock();
        let mut full: Vec<&str> = vec!["-X", "-h", &sock, "-p", &p, "-U", "postgres"];
        full.extend(rest);
        run(&format!("{}/psql", bin()), &full, &[], stdin)
    }
    /// The daemon's own psql runs: this server's binaries and port.
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let tool = Path::new(cmd).file_name().and_then(|n| n.to_str()).unwrap_or(cmd);
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        let p = PORT.to_string();
        let mut mapped: Vec<&str> = args.to_vec();
        for i in 1..mapped.len() {
            if mapped[i - 1] == "-p" {
                mapped[i] = &p;
            }
        }
        RealSystemOps.run_cmd(&format!("{}/{tool}", bin()), &mapped, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

fn check(name: &str, ok: bool, detail: impl std::fmt::Debug, fails: &mut Vec<String>) {
    if ok {
        eprintln!("PASS {name}");
    } else {
        eprintln!("FAIL {name}: {detail:?}");
        fails.push(format!("{name}: {detail:?}"));
    }
}

/// `k2.skin_uid()` as an older K2 installed it (0.41.6 to 0.46.17): the
/// lock was read whoever owned it. The boot pass must replace it.
const OLDER_SKIN_UID: &str = r#"CREATE OR REPLACE FUNCTION k2.skin_uid() RETURNS uuid
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_temp, pg_catalog
AS $k2fn$
DECLARE
  locked uuid;
BEGIN
  BEGIN
    SELECT id INTO locked FROM pg_temp.k2_principal_lock LIMIT 1;
    IF FOUND THEN
      RETURN locked;
    END IF;
  EXCEPTION
    WHEN undefined_table THEN
      NULL;
  END;
  RETURN nullif(current_setting('k2.skin_principal', true), '')::uuid;
END;
$k2fn$;
"#;

/// Everything in the database a boot pass could touch, one line each:
/// relations (owner, ACL, row security), policies, functions (owner, ACL,
/// definer, settings, body hash), schemas, the database ACL, role
/// memberships, and the rows. `skip_fn` leaves one `k2` function out.
fn snapshot(srv: &Server, db: &str, skip_fn: &str) -> Vec<String> {
    let skip = skip_fn.replace('\'', "''");
    srv.su(
        db,
        &format!(
            "SELECT x FROM (\n\
             SELECT 'rel ' || n.nspname || '.' || c.relname || ' ' || c.relkind::text || ' ' || pg_get_userbyid(c.relowner) || ' ' \
               || coalesce(c.relacl::text, '-') || ' rls=' || c.relrowsecurity || ' force=' || c.relforcerowsecurity AS x\n\
               FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace\n\
               WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'\n\
             UNION ALL\n\
             SELECT 'pol ' || polrelid::regclass::text || ' ' || polname || ' ' || polcmd::text || ' ' || polpermissive::text || ' ' \
               || coalesce(pg_get_expr(polqual, polrelid), '-') || ' ' || coalesce(pg_get_expr(polwithcheck, polrelid), '-') || ' ' \
               || (SELECT string_agg(CASE WHEN r = 0 THEN 'public' ELSE pg_get_userbyid(r) END, ',' ORDER BY 1) FROM unnest(polroles) r)\n\
               FROM pg_policy\n\
             UNION ALL\n\
             SELECT 'fn ' || n.nspname || '.' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ') ' \
               || pg_get_userbyid(p.proowner) || ' ' || coalesce(p.proacl::text, '-') || ' def=' || p.prosecdef || ' ' \
               || coalesce(array_to_string(p.proconfig, ','), '-') || ' ' || md5(p.prosrc)\n\
               FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace\n\
               WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'\n\
                 AND NOT (n.nspname = 'k2' AND p.proname = '{skip}')\n\
             UNION ALL\n\
             SELECT 'nsp ' || nspname || ' ' || pg_get_userbyid(nspowner) || ' ' || coalesce(nspacl::text, '-')\n\
               FROM pg_namespace WHERE nspname NOT LIKE 'pg\\_%' AND nspname <> 'information_schema'\n\
             UNION ALL\n\
             SELECT 'db ' || datname || ' ' || pg_get_userbyid(datdba) || ' ' || coalesce(datacl::text, '-')\n\
               FROM pg_database WHERE datname = current_database()\n\
             UNION ALL\n\
             SELECT 'mem ' || pg_get_userbyid(roleid) || '>' || pg_get_userbyid(member) || ' ' || admin_option FROM pg_auth_members\n\
             UNION ALL\n\
             SELECT 'rows ' || count(*) || ' ' || md5(coalesce(string_agg(t::text, ',' ORDER BY t::text), '')) FROM notes t\n\
             ) s ORDER BY x;"
        ),
    )
}

fn skin_uid_def(srv: &Server, db: &str) -> (String, String, String) {
    let rows = srv.su(
        db,
        "SELECT pg_get_userbyid(p.proowner), coalesce(p.proacl::text, '-'), p.prosrc FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'k2' AND p.proname = 'skin_uid';",
    );
    let joined = rows.join("\n");
    let mut parts = joined.splitn(3, '|');
    (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    )
}

#[test]
#[ignore = "runs only with REALPG_LOCK=1 and Postgres binaries (a Linux compute job)"]
fn real_pg_principal_lock_owner_and_boot_pass() {
    assert_eq!(std::env::var("REALPG_LOCK").as_deref(), Ok("1"), "REALPG_LOCK=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
    let version = run(&format!("{}/postgres", bin()), &["--version"], &[], None).expect("postgres --version");
    eprintln!("server: {version}");
    let root = PathBuf::from(format!("/tmp/k2rpl-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let srv: &'static Server = Box::leak(Box::new(Server { root }));
    let _stop = Stop(srv);
    run(&format!("{}/initdb", bin()), &["-D", &srv.data(), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], &[], None)
        .expect("initdb");
    let opts = format!("-p {PORT} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", srv.sock());
    run(&format!("{}/pg_ctl", bin()), &["-D", &srv.data(), "-o", &opts, "-l", &format!("{}.log", srv.data()), "-w", "start"], &[], None).expect("pg_ctl start");

    let _home = crate::test_support::TempHome::new();
    let _g = sql_server_test_lock();
    k2_core::db::init_for_tests();
    seed_running_sidecar();

    // Workspace A's database: notes, one row per App user, rows by
    // `k2.skin_uid()` (the study page's own example policy).
    let dir = std::env::temp_dir().join(format!("k2-rpl-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join(".k2/db/migrations")).unwrap();
    std::fs::write(
        dir.join(".k2/db/migrations/0001_notes.sql"),
        "CREATE TABLE notes (id serial PRIMARY KEY, owner uuid NOT NULL, body text NOT NULL);\n\
         ALTER TABLE notes ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY notes_own ON notes FOR ALL TO PUBLIC USING (owner = k2.skin_uid()) WITH CHECK (owner = k2.skin_uid());\n",
    )
    .unwrap();
    let path = dir.to_string_lossy().into_owned();
    let handle = format!("rpl{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let pid = insert_project_handle("real-pg-lock", &path, &handle);
    let reader_dir = std::env::temp_dir().join(format!("k2-rpl-r-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&reader_dir).unwrap();
    let reader_pid = insert_project_handle("real-pg-lock-reader", &reader_dir.to_string_lossy(), &format!("{handle}r"));
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let created = ops::create_database(srv, &secrets, &pid, 1, None, None).expect("create db");
    let db = created["name"].as_str().expect("db name").to_string();
    ops::migrate(srv, &secrets, &pid, &path, None).expect("migrate 0001");
    ops::grant_access(srv, &secrets, None, &db, &reader_pid, "read", false, &[]).expect("grant read");
    let reader = ops::default_agent_role(&reader_pid);

    let mut policy = k2_core::skin::RoomPolicy::new();
    policy.insert(pid.clone(), vec![k2_core::skin::CAP_STORE_READ.into(), k2_core::skin::CAP_STORE_WRITE.into()]);
    let alice = skin_session("alice", policy.clone());
    let bob = skin_session("bob", policy);
    let a_id = alice.principal_id.clone().expect("alice");
    let b_id = bob.principal_id.clone().expect("bob");
    srv.su(&db, &format!("INSERT INTO notes (owner, body) VALUES ('{a_id}', 'alice note'), ('{b_id}', 'bob note');"));
    eprintln!("db={db} reader={reader} alice={a_id} bob={b_id}");

    let mut fails = Vec::new();
    let planted = |db: &str| {
        srv.script(
            &reader,
            db,
            &format!(
                "CREATE TEMP TABLE k2_principal_lock (id uuid) ON COMMIT PRESERVE ROWS;\n\
                 INSERT INTO k2_principal_lock VALUES ('{b_id}');\n\
                 GRANT SELECT ON k2_principal_lock TO \"{db}_migrator\";\n\
                 SELECT coalesce(k2.skin_uid()::text, 'none');\n\
                 SELECT count(*) FROM notes;\n\
                 BEGIN;\n\
                 SELECT 1 FROM (SELECT k2.set_principal('{a_id}')) s;\n\
                 SELECT k2.skin_uid();\n\
                 SELECT body FROM notes ORDER BY id;\n\
                 COMMIT;\n\
                 SELECT k2.principal_hygiene();\n\
                 SELECT coalesce(k2.skin_uid()::text, 'none');\n"
            ),
        )
    };

    // 1. A lock the caller made is not K2's: ignored, and gone from the
    //    answer for the rest of the connection.
    let out = planted(&db);
    check(
        "a caller-made principal lock is ignored (no principal, no rows)",
        out.as_ref().is_ok_and(|o| o.len() >= 2 && o[0] == "none" && o[1] == "0"),
        &out,
        &mut fails,
    );
    check(
        "set_principal on that connection still decides (alice, her row only)",
        out.as_ref().is_ok_and(|o| o.len() == 7 && o[2] == "1" && o[3] == a_id && o[4] == "alice note"),
        &out,
        &mut fails,
    );
    check(
        "after the transaction: hygiene true and no principal",
        out.as_ref().is_ok_and(|o| o.len() == 7 && o[5] == "t" && o[6] == "none"),
        &out,
        &mut fails,
    );

    // 2. The guest lane's lock (made by the migrator, before SET ROLE)
    //    still counts.
    routes::with_test_ops(srv, || {
        let r = routes::handle_db_query(&query_json(&handle, "SELECT body FROM notes ORDER BY id", Some(serde_json::json!([]))), Some(alice.clone()));
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
        check(
            "the guest lane's lock still counts (alice sees her row only)",
            r.status == "200 OK" && v["rows"] == serde_json::json!([{ "body": "alice note" }]),
            (&r.status, &r.body),
            &mut fails,
        );
        let r = routes::handle_db_query(&query_json(&handle, "SELECT k2.skin_uid() AS uid", Some(serde_json::json!([]))), Some(bob.clone()));
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
        check("the guest lane: skin_uid() is the pass's user", r.status == "200 OK" && v["rows"][0]["uid"] == b_id, (&r.status, &r.body), &mut fails);
    });

    // 3. The setting stays the App's own lane, open to any login that runs
    //    its own SQL (documented in `k2 study db`; S3's k2.set_claims
    //    replaces it). Pinned so a change here is a decision.
    let out = srv.script(&reader, &db, &format!("SET k2.skin_principal = '{b_id}';\nSELECT body FROM notes ORDER BY id;\n"));
    check(
        "documented: a grantee can set which App user it acts as",
        out.as_ref().is_ok_and(|o| o == &vec!["bob note".to_string()]),
        &out,
        &mut fails,
    );

    // 4. The boot pass on a database an older K2 made: skin_uid replaced
    //    in place (owner and grants kept), one line, nothing else changed;
    //    a second run changes nothing.
    let (owner_new, acl_new, _) = skin_uid_def(srv, &db);
    srv.su(&db, OLDER_SKIN_UID);
    let (owner_old, acl_old, src_old) = skin_uid_def(srv, &db);
    check(
        "setup: the older skin_uid is in place, owner and grants as K2 sets them",
        !src_old.contains("relowner") && owner_old == owner_new && acl_old == acl_new && owner_old.ends_with("_migrator"),
        (&owner_old, &acl_old),
        &mut fails,
    );
    let out = planted(&db);
    check(
        "setup: the older skin_uid honours a caller-made lock (the fail-first case)",
        out.as_ref().is_ok_and(|o| o.len() >= 2 && o[0] == b_id && o[1] == "1"),
        &out,
        &mut fails,
    );
    let no_grant = format!("CREATE TEMP TABLE k2_principal_lock (id uuid);\nINSERT INTO k2_principal_lock VALUES ('{b_id}');\nSELECT coalesce(k2.skin_uid()::text, 'none');\n");
    let out = srv.script(&reader, &db, &no_grant);
    check(
        "setup: without a grant to the migrator, the older skin_uid errors (it can't read the caller's table)",
        out.as_ref().err().is_some_and(|e| e.contains("permission denied for table k2_principal_lock")),
        &out,
        &mut fails,
    );
    let before = snapshot(srv, &db, "skin_uid");
    let lines = ops::seal_existing_databases(srv);
    check("the boot pass: one line for the database", lines == vec![format!("{db}: sealed")], &lines, &mut fails);
    let (owner1, acl1, src1) = skin_uid_def(srv, &db);
    check(
        "the boot pass: skin_uid checks the lock's owner now, owner and grants unchanged",
        src1.contains("c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)") && owner1 == owner_old && acl1 == acl_old,
        (&owner1, &acl1),
        &mut fails,
    );
    let after = snapshot(srv, &db, "skin_uid");
    check("the boot pass: nothing else changed", before == after, (&before, &after), &mut fails);
    let full1 = snapshot(srv, &db, "");
    let lines2 = ops::seal_existing_databases(srv);
    let full2 = snapshot(srv, &db, "");
    check("the boot pass again: one line, nothing changed", lines2 == lines && full1 == full2, (&lines2, &full1, &full2), &mut fails);
    let out = planted(&db);
    check(
        "after the boot pass: a caller-made lock is ignored",
        out.as_ref().is_ok_and(|o| o.len() == 7 && o[0] == "none" && o[1] == "0" && o[3] == a_id && o[4] == "alice note"),
        &out,
        &mut fails,
    );

    let out = srv.script(&reader, &db, &no_grant);
    check(
        "after the boot pass: an ungranted caller-made lock is ignored too (no error, no principal)",
        out.as_ref().is_ok_and(|o| o == &vec!["none".to_string()]),
        &out,
        &mut fails,
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&reader_dir);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
