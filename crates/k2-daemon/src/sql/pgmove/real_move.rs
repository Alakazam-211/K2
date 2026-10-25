//! The first real dump→restore test (V43): a full move on real Postgres
//! through the real root helper. `#[ignore]`; it needs a throwaway Linux
//! box (a systemd container is enough) set up as K2's daemon box:
//!
//! - this test binary runs AS the daemon: the systemd unit named in
//!   `/etc/k2-root-helper/daemon.toml` (its main process), as that user,
//!   with the sudoers line for `/usr/local/libexec/k2-root-helper`;
//! - the protocol-9 helper installed there;
//! - distro Postgres 16 with `16/main` on 5432; network to PGDG (the move
//!   installs 17 through `pg add-major`);
//! - `REALMOVE=1` (and nothing else from K2 in the environment).
//!
//! Runbook: `.k2/prds/runbook-pg-drills-0463.md`. The test builds its own
//! fixture (roles with passwords, a grantee login, a bind role, RLS with
//! FORCE, policies, sequences, an identity, default privileges, a SECURITY
//! DEFINER function, a view, a large object, a database setting, the
//! pgcrypto / pg_trgm / uuid-ossp extensions), moves it 16 → 17, checks
//! everything as the roles themselves over TCP, rolls back, moves again,
//! forgets the old copy, then proves two refusals (an extension with no
//! 17 build, low disk) change nothing. Every check panics on failure.
//!
//! Security review cases: H1 (a CHECK, a domain and a generated column
//! call an owner-planted function that tries `ALTER ROLE … SUPERUSER`;
//! the restore must run it as the owner, never the superuser) and F1 (a
//! database whose owner planted `public.=` operators, function overloads
//! and `ALTER DATABASE … SET search_path`; check, verify, the move, the
//! K2 helper install, the boot seal and privilege sync must not run them
//! as the superuser).

use std::cell::RefCell;

use serde_json::Value;

use super::*;
use crate::root_helper::RealDoor;

const PW_MIG: &str = "mig-Pw-0463-a";
const PW_AGENT: &str = "agent-Pw-0463-b";
const PW_OTHER: &str = "other-Pw-0463-c";

struct E2eEnv {
    rec: RefCell<DbRecord>,
    dsn: RefCell<String>,
    log: RefCell<Vec<String>>,
}

impl E2eEnv {
    fn new(db: &str) -> Self {
        E2eEnv {
            rec: RefCell::new(DbRecord {
                name: db.into(),
                project_id: "fx".into(),
                major: 16,
                k2_roles: vec![format!("{db}_migrator"), format!("{db}_agent"), format!("{db}_bind"), "ws_other_agent".into()],
                moved_from: None,
                moved_at: None,
                rollback_until: None,
                source_state: SourceState::None,
                move_tag: None,
            }),
            dsn: RefCell::new(format!("postgres://{db}_agent@127.0.0.1:5432/{db}")),
            log: RefCell::new(Vec::new()),
        }
    }
    fn note(&self, s: String) {
        println!("  env: {s}");
        self.log.borrow_mut().push(s);
    }
}

impl MoveEnv for E2eEnv {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }
    fn sleep_secs(&self, s: u64) {
        self.note(format!("sleep {s}"));
    }
    fn record(&self, _db: &str) -> Result<DbRecord, String> {
        Ok(self.rec.borrow().clone())
    }
    fn cluster_role(&self, _m: Major) -> ClusterRole {
        ClusterRole::K2
    }
    fn logins(&self, db: &str) -> Result<Vec<(String, String)>, String> {
        Ok(vec![(format!("{db}_agent"), PW_AGENT.into()), (format!("{db}_migrator"), PW_MIG.into())])
    }
    fn users(&self, _db: &str) -> Users {
        Users::default()
    }
    fn notice(&self, _db: &str, text: &str) {
        self.note(format!("notice: {text}"));
    }
    fn clear_notice(&self, _db: &str) {}
    fn pause_doors(&self, _db: &str, paused: bool) {
        self.note(format!("doors paused={paused}"));
    }
    fn stop_apps(&self, apps: &[String]) -> Result<Vec<String>, String> {
        Ok(apps.to_vec())
    }
    fn start_apps(&self, _apps: &[String]) -> Vec<String> {
        vec![]
    }
    fn rerun_k2_setup(&self, _m: Major, _db: &str) -> Result<(), String> {
        Ok(())
    }
    fn verify_k2(&self, _m: Major, _db: &str) -> Result<(), String> {
        Ok(())
    }
    fn repoint(&self, _db: &str, from_port: u16, to_port: u16) -> Result<Vec<String>, (Vec<String>, String)> {
        let new = self.dsn.borrow().replace(&format!(":{from_port}/"), &format!(":{to_port}/"));
        *self.dsn.borrow_mut() = new;
        Ok(vec!["vault DATABASE_URL".into()])
    }
    fn unpoint(&self, _db: &str, _done: &[String], from_port: u16, to_port: u16) -> Result<(), String> {
        let new = self.dsn.borrow().replace(&format!(":{to_port}/"), &format!(":{from_port}/"));
        *self.dsn.borrow_mut() = new;
        Ok(())
    }
    fn set_major(&self, _db: &str, m: Major) -> Result<(), String> {
        self.rec.borrow_mut().major = m;
        Ok(())
    }
    fn set_moved(&self, _db: &str, info: Option<&MovedInfo>) -> Result<(), String> {
        let mut r = self.rec.borrow_mut();
        r.moved_from = info.map(|i| i.from);
        r.moved_at = info.map(|i| i.at);
        r.rollback_until = info.map(|i| i.until);
        r.move_tag = info.map(|i| i.tag.clone());
        r.source_state = info.map(|i| i.source_state).unwrap_or_default();
        Ok(())
    }
    fn record_cluster(&self, m: Major, port: u16) {
        self.note(format!("cluster {m} on {port}"));
    }
    fn progress(&self, p: &MoveProgress) -> Result<(), String> {
        println!(
            "  progress: {} {:?} {:?}{}",
            p.kind,
            p.step,
            p.state,
            p.last_error.as_deref().map(|e| format!(" — {e}")).unwrap_or_default()
        );
        Ok(())
    }
    fn audit(&self, act: &str, line: Value) {
        self.note(format!("audit {act} {line}"));
    }
    fn inbox(&self, title: &str, _body: &str) {
        self.note(format!("inbox: {title}"));
    }
}

/// Low disk on top of the real ops (the refusal test).
struct LowDisk<'a>(&'a HelperOps<'a>);

macro_rules! fwd {
    ($($name:ident($($a:ident: $t:ty),*) -> $r:ty;)*) => {
        $(fn $name(&self, $($a: $t),*) -> $r { self.0.$name($($a),*) })*
    };
}

impl ClusterOps for LowDisk<'_> {
    fwd! {
        clusters() -> Result<Vec<ClusterRow>, String>;
        installed() -> Result<Vec<Major>, String>;
        band_listening() -> Result<Vec<u16>, String>;
        add_major(m: Major) -> Result<(), String>;
        ensure_cluster(m: Major, port: u16) -> Result<(), String>;
        sql(m: Major, db: &str, sql: &str) -> Result<String, String>;
        copy_roles(from: Major, to: Major, roles: &[String], dry_run: bool) -> Result<Value, String>;
        allow_conn(m: Major, db: &str, mode: ConnMode) -> Result<(), String>;
        dump(from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String>;
        restore(to: Major, db: &str, tag: &str, role: &str, password: &str) -> Result<(), String>;
        drop_db(m: Major, db: &str) -> Result<bool, String>;
        remove_dump(db: &str, tag: &str) -> Result<(), String>;
        extension_available(m: Major, ext: &str) -> Option<bool>;
        login(m: Major, port: u16, db: &str, role: &str, password: &str) -> Result<(), String>;
        moves_bytes() -> u64;
    }
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(1 << 20)
    }
}

use helper_ops::HelperOps;

/// `psql` as a role over TCP: the answer, or the error text.
fn as_role(m: Major, port: u16, db: &str, role: &str, pw: &str, sql: &str) -> Result<String, String> {
    let psql = format!("/usr/lib/postgresql/{m}/bin/psql");
    let out = std::process::Command::new(&psql)
        .args(["-X", "-q", "-h", "127.0.0.1", "-p", &port.to_string(), "-U", role, "-d", db, "-tA", "-v", "ON_ERROR_STOP=1", "-c", sql])
        .env_clear()
        .env("PGPASSWORD", pw)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn want(got: Result<String, String>, expect: &str, what: &str) {
    match got {
        Ok(v) if v == expect => println!("PASS {what}"),
        other => panic!("FAIL {what}: expected {expect:?}, got {other:?}"),
    }
}

fn denied(got: Result<String, String>, needle: &str, what: &str) {
    match got {
        Err(e) if e.contains(needle) => println!("PASS {what} ({needle})"),
        other => panic!("FAIL {what}: expected an error containing {needle:?}, got {other:?}"),
    }
}

fn fixture(ops: &dyn ClusterOps, db: &str, extra_ext: Option<&str>) {
    let roles = format!(
        "DO $$ BEGIN\n\
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{db}_migrator') THEN\n\
             CREATE ROLE {db}_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT PASSWORD '{PW_MIG}';\n\
             CREATE ROLE {db}_agent LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT PASSWORD '{PW_AGENT}';\n\
             CREATE ROLE {db}_bind NOLOGIN;\n\
             GRANT {db}_migrator TO {db}_agent;\n\
             GRANT {db}_agent TO {db}_bind;\n\
           END IF;\n\
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'ws_other_agent') THEN\n\
             CREATE ROLE ws_other_agent LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT PASSWORD '{PW_OTHER}';\n\
           END IF;\n\
         END $$;\n"
    );
    ops.sql(16, "postgres", &roles).expect("fixture roles");
    ops.sql(16, "postgres", &format!("CREATE DATABASE {db} OWNER {db}_migrator;\n")).expect("fixture db");
    ops.sql(
        16,
        "postgres",
        &format!(
            "REVOKE ALL ON DATABASE {db} FROM PUBLIC;\n\
             GRANT CONNECT ON DATABASE {db} TO {db}_agent, ws_other_agent;\n\
             ALTER DATABASE {db} SET app.mode = 'fixture';\n"
        ),
    )
    .expect("fixture db grants");
    let ext = extra_ext.map(|x| format!("CREATE EXTENSION {x};\n")).unwrap_or_default();
    let body = format!(
        "SET ROLE {db}_migrator;\n\
         SET search_path = public;\n\
         CREATE EXTENSION pgcrypto;\n\
         CREATE EXTENSION pg_trgm;\n\
         CREATE EXTENSION \"uuid-ossp\";\n\
         CREATE SCHEMA k2;\n\
         CREATE FUNCTION k2.whoami() RETURNS text LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $f$ SELECT current_user::text $f$;\n\
         REVOKE ALL ON FUNCTION k2.whoami() FROM PUBLIC;\n\
         GRANT USAGE ON SCHEMA k2 TO {db}_agent;\n\
         GRANT EXECUTE ON FUNCTION k2.whoami() TO {db}_agent;\n\
         CREATE TABLE jobs (id bigserial PRIMARY KEY, owner text NOT NULL DEFAULT current_user, title text, tok uuid DEFAULT gen_random_uuid(), u2 uuid DEFAULT uuid_generate_v4());\n\
         CREATE INDEX jobs_title_trgm ON jobs USING gin (title gin_trgm_ops);\n\
         INSERT INTO jobs (owner, title) SELECT CASE WHEN g % 2 = 0 THEN '{db}_agent' ELSE 'someone' END, 'job ' || g FROM generate_series(1, 500) g;\n\
         ALTER TABLE jobs ENABLE ROW LEVEL SECURITY;\n\
         ALTER TABLE jobs FORCE ROW LEVEL SECURITY;\n\
         CREATE POLICY jobs_own ON jobs FOR ALL TO {db}_agent USING (owner = current_user) WITH CHECK (owner = current_user);\n\
         CREATE POLICY jobs_mig ON jobs FOR SELECT TO {db}_migrator USING (true);\n\
         GRANT SELECT, INSERT, UPDATE ON jobs TO {db}_agent;\n\
         GRANT USAGE ON SEQUENCE jobs_id_seq TO {db}_agent;\n\
         CREATE TABLE notes (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, body text);\n\
         INSERT INTO notes (body) SELECT 'n' || g FROM generate_series(1, 50) g;\n\
         GRANT SELECT ON notes TO {db}_agent, ws_other_agent;\n\
         CREATE TABLE secret (x int);\n\
         INSERT INTO secret VALUES (1);\n\
         CREATE VIEW jobs_v AS SELECT id, title FROM jobs;\n\
         GRANT SELECT ON jobs_v TO {db}_agent;\n\
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO {db}_agent;\n\
         SELECT lo_from_bytea(0, 'hello lo'::bytea) > 0;\n\
         {ext}\
         RESET ROLE;\n"
    );
    ops.sql(16, db, &body).expect("fixture objects");
    // K2's own helpers as `k2 db create` installs them: as postgres, most
    // owned by postgres (0.46.4 drill A: the owner can't replay those).
    let k2 = format!(
        "{}\n{}\n",
        crate::sql::ops::ensure_k2_helpers_sql(),
        crate::sql::ops::k2_helper_owner_sql(db)
    );
    ops.sql(16, db, &k2).expect("fixture: K2's helpers");
    println!("fixture {db} on 16: ready");
}

/// H1: objects whose restore runs an owner-written function (a CHECK, a
/// domain CHECK, a generated column, an index expression, and, the 0.46.4
/// drill's finding, a generated column calling an IMMUTABLE function with
/// a constant argument: CREATE TABLE plans it, so it runs during pre-data). If the function runs as a
/// superuser it records that and makes the owner a superuser.
fn plant_h1(ops: &dyn ClusterOps, db: &str) {
    let mig = format!("{db}_migrator");
    let body = format!(
        "SET ROLE {mig};\n\
         SET search_path = public;\n\
         CREATE TABLE h1_hits (cu text, su text);\n\
         CREATE FUNCTION public.h1_probe(v int) RETURNS boolean LANGUAGE plpgsql AS $f$\n\
         BEGIN\n\
           IF (SELECT rolsuper FROM pg_catalog.pg_roles WHERE rolname = current_user) THEN\n\
             INSERT INTO public.h1_hits VALUES (current_user, session_user);\n\
             EXECUTE 'ALTER ROLE {mig} SUPERUSER';\n\
           END IF;\n\
           RETURN v IS NOT NULL;\n\
         END $f$;\n\
         CREATE FUNCTION public.h1_who(v int) RETURNS text LANGUAGE plpgsql IMMUTABLE AS $f$\n\
         BEGIN\n\
           PERFORM public.h1_probe(v);\n\
           RETURN current_user::text || '/' || session_user::text;\n\
         END $f$;\n\
         CREATE DOMAIN h1_dom AS int CHECK (public.h1_probe(VALUE));\n\
         CREATE TABLE h1 (id h1_dom PRIMARY KEY CHECK (public.h1_probe(id)), who text GENERATED ALWAYS AS (public.h1_who(id)) STORED,\n\
           k text GENERATED ALWAYS AS (public.h1_who(0)) STORED);\n\
         CREATE INDEX h1_ix ON h1 ((public.h1_who(id)));\n\
         INSERT INTO h1 (id) SELECT g FROM generate_series(1, 5) g;\n\
         RESET ROLE;\n"
    );
    ops.sql(16, db, &body).expect("H1 plant");
    println!("H1 plant in {db}: ready");
}

/// H1 after a restore onto `m`: the planted function ran (the generated
/// column was recomputed) and only ever as the owner.
fn h1_checks(ops: &dyn ClusterOps, m: Major, db: &str) {
    let mig = format!("{db}_migrator");
    want(
        ops.sql(m, db, "SELECT string_agg(DISTINCT who, ',') FROM public.h1;\n"),
        &format!("{mig}/{mig}"),
        &format!("H1 {m}: the planted function ran during the restore, as the owner (current and session user)"),
    );
    want(ops.sql(m, db, "SELECT count(*) FROM public.h1_hits;\n"), "0", &format!("H1 {m}: it never ran as a superuser"));
    want(
        ops.sql(m, "postgres", &format!("SELECT rolsuper::text FROM pg_roles WHERE rolname = '{mig}';\n")),
        "false",
        &format!("H1 {m}: the owner is not a superuser"),
    );
}

/// Drill A: K2's helpers on the target, canned (never replayed from the
/// dump), with the owners `k2 db create` gives them; the tenant's own
/// function in schema `k2` came along as the owner's.
fn k2_helper_checks(ops: &dyn ClusterOps, m: Major, db: &str) {
    let mig = format!("{db}_migrator");
    let owners = "SELECT string_agg(p.oid::regprocedure::text || '=' || pg_get_userbyid(p.proowner), ',' ORDER BY 1) \
                  FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'k2';\n";
    want(
        ops.sql(m, db, owners),
        &format!(
            "k2.actor()=postgres,k2.actor_info()={mig},k2.actor_kind()=postgres,k2.clear_principal()=postgres,\
             k2.principal_hygiene()=postgres,k2.set_principal(uuid)=postgres,k2.skin_uid()={mig},k2.whoami()={mig}"
        ),
        &format!("{m}: K2's helpers canned with K2's owners; the tenant's k2.whoami() the owner's"),
    );
    want(
        ops.sql(m, db, "SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'k2';\n"),
        &mig,
        &format!("{m}: schema k2 is the migrator's"),
    );
}

/// F1: the owner plants `public.=` operators and overloads of functions
/// K2's SQL calls, each logging (tag, current_user, session_user) to
/// `evil.hits`, then points the database's search_path at them.
fn plant_f1(ops: &dyn ClusterOps, db: &str) {
    let mig = format!("{db}_migrator");
    let mut body = format!(
        "SET ROLE {mig};\n\
         SET search_path = public;\n\
         CREATE SCHEMA evil;\n\
         CREATE TABLE evil.hits (tag text, cu text, su text);\n\
         GRANT USAGE ON SCHEMA evil TO PUBLIC;\n\
         GRANT INSERT, SELECT ON evil.hits TO PUBLIC;\n\
         CREATE FUNCTION evil.hit(t text) RETURNS void LANGUAGE sql AS $f$ INSERT INTO evil.hits VALUES (t, current_user, session_user) $f$;\n\
         GRANT EXECUTE ON FUNCTION evil.hit(text) TO PUBLIC;\n"
    );
    for (name, ty) in [("text", "text"), ("name", "name"), ("oid", "oid")] {
        body.push_str(&format!(
            "CREATE FUNCTION public.k2evil_eq_{name}(a {ty}, b {ty}) RETURNS boolean LANGUAGE plpgsql AS $f$\n\
             BEGIN PERFORM evil.hit('={name}'); RETURN a OPERATOR(pg_catalog.=) b; END $f$;\n\
             CREATE OPERATOR public.= (LEFTARG = {ty}, RIGHTARG = {ty}, FUNCTION = public.k2evil_eq_{name});\n"
        ));
    }
    body.push_str(
        "CREATE FUNCTION public.k2evil_eq_name_text(a name, b text) RETURNS boolean LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('=name_text'); RETURN a OPERATOR(pg_catalog.=) b; END $f$;\n\
         CREATE OPERATOR public.= (LEFTARG = name, RIGHTARG = text, FUNCTION = public.k2evil_eq_name_text);\n\
         CREATE FUNCTION public.json_build_object(a text, b bigint) RETURNS json LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('json_build_object(text,bigint)'); RETURN pg_catalog.json_build_object(a, b); END $f$;\n\
         CREATE FUNCTION public.json_build_object(a text, b text) RETURNS json LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('json_build_object(text,text)'); RETURN pg_catalog.json_build_object(a, b); END $f$;\n\
         CREATE FUNCTION public.format(a text, b text) RETURNS text LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('format(text,text)'); RETURN pg_catalog.format(a, b); END $f$;\n\
         CREATE FUNCTION public.quote_ident(a text) RETURNS text LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('quote_ident'); RETURN pg_catalog.quote_ident(a); END $f$;\n\
         CREATE FUNCTION public.to_regclass(a text) RETURNS regclass LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('to_regclass'); RETURN pg_catalog.to_regclass(a); END $f$;\n\
         CREATE FUNCTION public.lower(a text) RETURNS text LANGUAGE plpgsql AS $f$\n\
         BEGIN PERFORM evil.hit('lower'); RETURN pg_catalog.lower(a); END $f$;\n",
    );
    // ...and its own role for every new session (re-check #2 L-a).
    body.push_str(&format!(
        "ALTER DATABASE {db} SET search_path = evil, public, pg_catalog;\nALTER DATABASE {db} SET role = {mig};\nRESET ROLE;\n"
    ));
    ops.sql(16, db, &body).expect("F1 plant");
    println!("F1 plant in {db}: ready");
}

/// Every row in `evil.hits` (read with pg_catalog first, so the read
/// itself never fires a plant).
fn f1_hits(ops: &dyn ClusterOps, m: Major, db: &str) -> String {
    ops.sql(m, db, "SELECT coalesce(string_agg(tag || ' by ' || cu || '/' || su, '; ' ORDER BY tag), '') FROM evil.hits;\n").expect("read evil.hits")
}

fn f1_clear(ops: &dyn ClusterOps, m: Major, db: &str) {
    ops.sql(m, db, "TRUNCATE evil.hits;\n").expect("clear evil.hits");
}

/// K2's superuser SQL on a tenant database, as the daemon sends it: the
/// helper pins search_path (PGOPTIONS) and the daemon prefixes
/// `SET search_path = pg_catalog, pg_temp` (both F1).
fn k2_superuser_paths(ops: &dyn ClusterOps, m: Major, db: &str) {
    use crate::sql::ops as k2ops;
    let mut sql = k2ops::ensure_k2_helpers_sql();
    sql.push('\n');
    sql.push_str(&k2ops::k2_helper_owner_sql(db));
    sql.push('\n');
    sql.push_str(&k2ops::seal_k2_internal_tables_sql(db));
    sql.push('\n');
    let pinned = String::from_utf8(k2ops::pin_search_path(sql.as_bytes())).unwrap();
    ops.sql(m, db, &pinned).expect("K2 helpers + boot seal (daemon pin + helper pin)");
    // The helper's PGOPTIONS pin alone (an older daemon's SQL).
    ops.sql(m, db, &sql).expect("K2 helpers + boot seal (helper pin)");
    // The daemon's prefix alone beats a session that already chose the
    // planted path and the tenant's role.
    let hostile_start = format!("SET ROLE {db}_migrator;\nSET search_path = evil, public, pg_catalog;\n");
    let mut hostile = hostile_start.clone();
    hostile.push_str(&String::from_utf8(k2ops::pin_search_path(sql.as_bytes())).unwrap());
    ops.sql(m, db, &hostile).expect("K2 helpers + boot seal (daemon pin over a hostile path and role)");
    let probe = format!("{hostile_start}{}SELECT current_user || '/' || current_setting('search_path');\n", k2ops::PIN_SEARCH_PATH);
    // (The answer's last line: psql prints a tag for each SET.)
    want(
        ops.sql(m, db, &probe).map(|s| s.lines().last().unwrap_or_default().to_string()),
        "postgres/pg_catalog, pg_temp",
        &format!("F1 {m}: the daemon pin resets a hostile role and path"),
    );
    want(
        ops.sql(m, db, "SELECT current_user || '/' || current_setting('search_path');\n"),
        // (PGOPTIONS spells the path without a space.)
        "postgres/pg_catalog,pg_temp",
        &format!("F1 {m}: a helper session is postgres on pg_catalog despite ALTER DATABASE SET role/search_path"),
    );
}

fn agent_checks(m: Major, port: u16, db: &str, jobs_seen: &str, next_id: &str) {
    let agent = format!("{db}_agent");
    let mig = format!("{db}_migrator");
    let a = |sql: &str| as_role(m, port, db, &agent, PW_AGENT, sql);
    want(a("SELECT count(*) FROM jobs"), jobs_seen, &format!("{m}: RLS shows the agent its own rows"));
    want(a("SELECT k2.whoami()"), &mig, &format!("{m}: SECURITY DEFINER function runs as its owner"));
    want(a("SELECT count(*) FROM notes"), "50", &format!("{m}: granted SELECT"));
    denied(a("INSERT INTO notes (body) VALUES ('x')"), "permission denied", &format!("{m}: no INSERT where none was granted"));
    denied(a("SELECT * FROM secret"), "permission denied", &format!("{m}: no grant, no read"));
    want(a("SELECT current_setting('app.mode')"), "fixture", &format!("{m}: ALTER DATABASE SET carried"));
    want(
        a("SELECT similarity('job 1', 'job 2') > 0 AND digest('a', 'sha256') IS NOT NULL AND uuid_generate_v4() IS NOT NULL"),
        "t",
        &format!("{m}: pg_trgm, pgcrypto, uuid-ossp work"),
    );
    want(a("INSERT INTO jobs (title) VALUES ('after') RETURNING id"), next_id, &format!("{m}: the sequence continues"));
    denied(a("INSERT INTO jobs (owner, title) VALUES ('someone', 'x')"), "row-level security", &format!("{m}: WITH CHECK enforced"));
    want(as_role(m, port, db, "ws_other_agent", PW_OTHER, "SELECT count(*) FROM notes"), "50", &format!("{m}: the shared grantee login reads its grant"));
    denied(as_role(m, port, db, "ws_other_agent", PW_OTHER, "SELECT count(*) FROM jobs"), "permission denied", &format!("{m}: the grantee sees nothing else"));
}

#[test]
#[ignore = "needs a throwaway systemd box where this binary is the daemon (REALMOVE=1); see the runbook"]
fn real_move_16_to_17_rollback_forget_and_refusals() {
    assert_eq!(std::env::var("REALMOVE").as_deref(), Ok("1"), "REALMOVE=1 must be set (throwaway box only)");
    let door = RealDoor;
    let ops = HelperOps { door: &door };
    let db = "ws_fx";
    fixture(&ops, db, None);
    plant_h1(&ops, db);
    let env = E2eEnv::new(db);
    let opts = MoveOpts { to: Some(17), actor: "owner".into(), now: true, allow_adopted: false, helper_ok: true };

    // --check: read-only.
    let r = check_only(&ops, &env, db, &opts).expect("check");
    for l in &r.lines {
        println!("  check {:?} {}: {}", l.level, l.id, l.text);
    }
    assert!(!r.refused(), "check refused");
    assert_eq!(r.roles, vec![format!("{db}_agent"), format!("{db}_bind"), format!("{db}_migrator"), "ws_other_agent".to_string()]);

    // The move.
    let p = run_move(&ops, &env, db, &opts).expect("move");
    assert_eq!(p.state, MoveState::Succeeded, "move: {:?} diff {:?}", p.last_error, p.diff);
    let port17 = p.to_port.expect("17's port");
    println!("PASS moved 16 → 17 on port {port17}, dump {} bytes", p.dump_bytes.unwrap_or(0));
    assert!(env.dsn.borrow().contains(&format!(":{port17}/")), "DSN re-pointed: {}", env.dsn.borrow());
    agent_checks(17, port17, db, "250", "501");
    h1_checks(&ops, 17, db);
    k2_helper_checks(&ops, 17, db);
    want(as_role(17, port17, db, &format!("{db}_migrator"), PW_MIG, "SELECT count(*) FROM jobs"), "501", "17: FORCE RLS on the owner, its own policy");
    want(ops.sql(17, db, "SELECT count(*) FROM pg_largeobject_metadata;\n"), "1", "17: the large object came along");
    want(
        ops.sql(17, db, "SELECT relrowsecurity::text || relforcerowsecurity::text FROM pg_class WHERE relname = 'jobs';\n"),
        "truetrue",
        "17: RLS + FORCE flags",
    );
    want(
        as_role(17, port17, db, &format!("{db}_migrator"), PW_MIG, "CREATE TABLE t_after (x int); SELECT 1"),
        "1",
        "17: the owner creates a table",
    );
    want(as_role(17, port17, db, &format!("{db}_agent"), PW_AGENT, "SELECT count(*) FROM t_after"), "0", "17: default privileges carried");
    denied(as_role(16, 5432, db, &format!("{db}_agent"), PW_AGENT, "SELECT 1"), "not currently accepting connections", "16: the kept source is closed");
    want(
        ops.sql(17, "postgres", "SELECT setting FROM pg_settings WHERE name = 'wal_level';\n"),
        "logical",
        "17: a new cluster starts wal_level=logical",
    );

    // Roll back: 16 serves again; what was written on 17 stays behind.
    let move_tag = env.rec.borrow().move_tag.clone().unwrap();
    let v = rollback(&ops, &env, db, &RollbackOpts { actor: "owner".into(), yes: true, now: true, owner: true }).expect("rollback");
    assert_eq!(v.state, MoveState::Succeeded, "rollback: {:?} warnings {:?}", v.last_error, v.warnings);
    assert_eq!(v.kind, "rollback");
    println!("PASS rollback → {:?}", v.kept_as);
    assert!(env.dsn.borrow().contains(":5432/"), "DSN back: {}", env.dsn.borrow());
    want(as_role(16, 5432, db, &format!("{db}_agent"), PW_AGENT, "SELECT count(*) FROM jobs"), "250", "16 after rollback: the 17 write is not carried back");
    let kept = v.kept_as.clone().expect("the 17 copy renamed");
    want(ops.sql(17, "postgres", &format!("SELECT datallowconn::text FROM pg_database WHERE datname = '{kept}';\n")), "false", "17: the rolled-back copy is kept, closed");
    want(
        ops.sql(16, "postgres", &format!("SELECT count(*) FROM pg_ls_dir('{}') f WHERE f = '{db}-{move_tag}.dump';\n", k2_root_proto::pg_major::MOVES_DIR)),
        "0",
        "the rolled-back move's dump is removed (F7)",
    );
    assert!(rollback(&ops, &env, db, &RollbackOpts { actor: "owner".into(), yes: true, now: true, owner: true }).unwrap_err().starts_with("not_moved"));
    println!("PASS a second rollback has nothing to do");

    // One kept rollback copy per database (F7): a new move waits for
    // forget-old to delete it.
    let r = check_only(&ops, &env, db, &opts).expect("check with a rollback copy");
    assert!(r.lines.iter().any(|l| l.id == "kept" && l.level == super::check::Level::Refuse && l.text.contains(&kept)), "{:#?}", r.lines);
    println!("PASS check refuses while a rollback copy is kept");
    let v = forget_old(&ops, &env, db, &ForgetOpts { actor: "owner".into(), owner: true, force_now: true }).expect("forget the rollback copy");
    assert_eq!(v["rollbackCopies"], serde_json::json!([format!("17/{kept}")]), "{v}");
    want(ops.sql(17, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{kept}';\n")), "0", "forget-old dropped the rollback copy");
    want(as_role(16, 5432, db, &format!("{db}_agent"), PW_AGENT, "SELECT count(*) FROM jobs"), "250", "16 still serves");

    // Move again, then forget the old copy (owner).
    let p = run_move(&ops, &env, db, &opts).expect("move again");
    assert_eq!(p.state, MoveState::Succeeded, "move again: {:?} diff {:?}", p.last_error, p.diff);
    agent_checks(17, port17, db, "250", "501");
    h1_checks(&ops, 17, db);
    let tag = env.rec.borrow().move_tag.clone().unwrap();
    let v = forget_old(&ops, &env, db, &ForgetOpts { actor: "owner".into(), owner: true, force_now: true }).expect("forget-old");
    println!("PASS forget-old → {v}");
    want(ops.sql(16, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{db}';\n")), "0", "16: the old copy is gone");
    want(
        ops.sql(16, "postgres", &format!("SELECT count(*) FROM pg_ls_dir('{}') f WHERE f = '{db}-{tag}.dump';\n", k2_root_proto::pg_major::MOVES_DIR)),
        "0",
        "the dump is gone",
    );
    want(ops.sql(16, "postgres", "SELECT 1;\n"), "1", "16 keeps running (never dropped, packages kept)");

    // Refused: an extension with no 17 build. Nothing changes.
    let db2 = "ws_fx_ext";
    fixture(&ops, db2, Some("k2fake"));
    let env2 = E2eEnv::new(db2);
    let p = run_move(&ops, &env2, db2, &opts).expect("refused move");
    assert_eq!(p.state, MoveState::Aborted, "{p:?}");
    assert!(p.last_error.as_deref().unwrap().contains("k2fake: no 17 build"), "{:?}", p.last_error);
    want(ops.sql(17, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{db2}';\n")), "0", "refused (extension): nothing on 17");
    want(as_role(16, 5432, db2, &format!("{db2}_agent"), PW_AGENT, "SELECT count(*) FROM jobs"), "250", "refused (extension): 16 still serves");

    // Refused: low disk. Nothing changes.
    let db3 = "ws_fx_disk";
    fixture(&ops, db3, None);
    let env3 = E2eEnv::new(db3);
    let low = LowDisk(&ops);
    let p = run_move(&low, &env3, db3, &opts).expect("refused move");
    assert_eq!(p.state, MoveState::Aborted, "{p:?}");
    assert!(p.last_error.as_deref().unwrap().contains("needed (2 × the database + 1 GiB)"), "{:?}", p.last_error);
    want(ops.sql(17, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{db3}';\n")), "0", "refused (disk): nothing on 17");
    want(as_role(16, 5432, db3, &format!("{db3}_agent"), PW_AGENT, "SELECT count(*) FROM jobs"), "250", "refused (disk): 16 still serves");

    // F1: a hostile owner's search_path plants.
    let db4 = "ws_fx_f1";
    fixture(&ops, db4, None);
    plant_f1(&ops, db4);
    let mig4 = format!("{db4}_migrator");
    // Positive controls: the plants are live for a default session (the
    // owner) and for a superuser session that takes the planted path.
    let hits = as_role(16, 5432, db4, &mig4, PW_MIG, "SELECT 'a'::text = 'a'::text").and_then(|_| {
        as_role(16, 5432, db4, &mig4, PW_MIG, "SELECT count(*) FROM evil.hits")
    });
    want(hits, "1", "F1 control: the planted public.= runs in a default session");
    f1_clear(&ops, 16, db4);
    ops.sql(16, db4, "SET search_path = evil, public, pg_catalog;\nSELECT 'a'::text = 'a'::text;\n").expect("superuser control");
    let h = f1_hits(&ops, 16, db4);
    assert!(h.contains("=text by postgres/postgres"), "F1 control: a superuser on the planted path runs it: {h:?}");
    println!("PASS F1 control: a superuser on the planted path runs it ({h})");
    let probe = as_role(
        16,
        5432,
        db4,
        &mig4,
        PW_MIG,
        "SELECT json_build_object('a', 1::bigint)::text, format('%s', 'x'::text), quote_ident('x'), lower('X'::text)",
    );
    println!("  F1 control (owner, default path): {probe:?}; hits now: {}", f1_hits(&ops, 16, db4));
    f1_clear(&ops, 16, db4);

    // check, counts, fingerprint, K2 helpers + boot seal as the superuser.
    let env4 = E2eEnv::new(db4);
    let r = check_only(&ops, &env4, db4, &opts).expect("F1 check");
    assert!(!r.refused(), "F1 check refused: {:#?}", r.lines);
    row_counts(&ops, 16, db4).expect("F1 counts");
    fingerprint(&ops, 16, db4).expect("F1 fingerprint");
    k2_superuser_paths(&ops, 16, db4);
    want(Ok(f1_hits(&ops, 16, db4)), "", "F1 16: check, counts, fingerprint, helpers and the boot seal ran no plant");
    // Privilege sync runs as the migrator (never a superuser): a plant
    // there runs with the owner's own rights only.
    let sync = crate::sql::ops::privilege_sync_sql(db4, &[]);
    as_role(16, 5432, db4, &mig4, PW_MIG, &sync).expect("privilege sync as the migrator");
    let h = f1_hits(&ops, 16, db4);
    assert!(!h.contains("postgres"), "F1 16: privilege sync ran a plant as a superuser: {h:?}");
    println!("PASS F1 16: privilege sync runs as the migrator only ({h:?})");
    f1_clear(&ops, 16, db4);

    // The move (pre-data restore as the superuser, data as the owner,
    // verify): any plant firing on 17 changes evil.hits' row count and
    // fails verify; a plant firing on 16 rides the dump.
    let p = run_move(&ops, &env4, db4, &opts).expect("F1 move");
    assert_eq!(p.state, MoveState::Succeeded, "F1 move: {:?} diff {:?}", p.last_error, p.diff);
    want(Ok(f1_hits(&ops, 17, db4)), "", "F1 17: the move ran no plant");
    want(
        ops.sql(17, "postgres", &format!("SELECT string_agg(x, ';' ORDER BY x) FROM pg_db_role_setting s JOIN pg_database d ON d.oid = s.setdatabase, unnest(s.setconfig) x WHERE d.datname = '{db4}' AND s.setrole = 0;\n")),
        &format!("app.mode=fixture;role={mig4};search_path=\"evil, public, pg_catalog\""),
        "F1 17: the owner's database setting is carried as it was",
    );
    k2_superuser_paths(&ops, 17, db4);
    want(Ok(f1_hits(&ops, 17, db4)), "", "F1 17: K2 helpers and the boot seal ran no plant");

    // Re-check #1 R3: a 54-character name (the longest whose migrator
    // isn't cut) rolls back to a 63-character `<first 46>__k2rb_<ts>` copy
    // that the helper can still close and drop.
    let db5 = format!("ws_fx_long_{}", "n".repeat(43));
    assert_eq!(db5.len(), 54);
    fixture(&ops, &db5, None);
    let env5 = E2eEnv::new(&db5);
    let p = run_move(&ops, &env5, &db5, &opts).expect("long-name move");
    assert_eq!(p.state, MoveState::Succeeded, "long-name move: {:?} diff {:?}", p.last_error, p.diff);
    let v = rollback(&ops, &env5, &db5, &RollbackOpts { actor: "owner".into(), yes: true, now: true, owner: true }).expect("long-name rollback");
    assert_eq!(v.state, MoveState::Succeeded, "{:?}", v.last_error);
    assert!(v.warnings.is_empty(), "the renamed copy was closed: {:?}", v.warnings);
    let kept5 = v.kept_as.clone().expect("renamed");
    assert_eq!(kept5.len(), 63, "{kept5}");
    want(ops.sql(17, "postgres", &format!("SELECT datallowconn::text FROM pg_database WHERE datname = '{kept5}';\n")), "false", "R3: the 63-character rollback copy is closed");
    want(as_role(16, 5432, &db5, &format!("{db5}_agent"), PW_AGENT, "SELECT count(*) FROM jobs"), "250", "R3: the long name serves on 16 again");
    let f = forget_old(&ops, &env5, &db5, &ForgetOpts { actor: "owner".into(), owner: true, force_now: true }).expect("long-name forget-old");
    assert_eq!(f["rollbackCopies"], serde_json::json!([format!("17/{kept5}")]), "{f}");
    want(ops.sql(17, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{kept5}';\n")), "0", "R3: forget-old dropped the 63-character copy");
    println!("ALL PASS");
}
