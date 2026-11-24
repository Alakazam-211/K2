//! `k2 db import supabase` and `k2 app user import` against real Postgres
//! 17 (prd-app-supabase-compat-v1 §12's import tests, lane B's done-when).
//! `#[ignore]`: it runs only where REALPG_IMPORT=1 is set, with a Postgres
//! server's binaries in `REALPG_BIN` (`initdb`, `pg_ctl`, `postgres`,
//! `psql`, `pg_dump`, `pg_restore`; a compute job may unpack a relocatable
//! 17 build and point there), as an ordinary user.
//!
//! Two throwaway servers under a temp dir, both made here:
//! - **the source** (port 5452): a synthetic Supabase-style database. No
//!   real data: Supabase's roles, `auth` / `storage` owned by its admin
//!   roles, `realtime`, `vault`, `graphql_public`, `supabase_migrations`, a
//!   publication, Supabase's grants and default privileges, and an App in
//!   `public` with row policies, a FORCE table, SECURITY DEFINER helpers, a
//!   sign-up trigger on `auth.users`, Storage policies, a function reading
//!   `vault`, bcrypt passwords made by pgcrypto as GoTrue makes them.
//! - **the target** (port 5453): recorded as the workspace's named cluster
//!   `17/k2supa`; K2 makes the database in it (`k2 db create --cluster`).
//!
//! The source is dumped with `pg_dump -Fc`, imported, and checked: owners,
//! policies answering per user under `request.jwt.claims`, grants, FORCE
//! lifted, Supabase's schemas gone, K2's `auth.users` with the same ids and
//! no passwords, Storage rows and policies, the migration map and a later
//! `k2 db migrate`. Then the users file is imported into an App and each
//! person signs in with the password they had, keeping their id, and is
//! rehashed to Argon2. Every check prints PASS/FAIL; the test fails if any
//! failed.
//!
//! Run: `REALPG_IMPORT=1 REALPG_BIN=… cargo test -p k2-daemon --lib real_pg_import -- --ignored --nocapture --test-threads=1`
//! with every K2_* variable cleared.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value as J;

use super::{import, Opts, Refusal};
use crate::sql::cluster::{self, Cluster};
use crate::sql::ops;
use crate::sql::secrets::MemSecretStore;
use crate::sql::sysops::{RealSystemOps, SystemOps};

const SRC_PORT: u16 = 5452;
const DST_PORT: u16 = 5453;
const ADA: &str = "0a1d0000-0000-4000-8000-00000000a0da";
const BO: &str = "0b000000-0000-4000-8000-0000000000b0";
const CY: &str = "0c000000-0000-4000-8000-0000000000c1";
const ORG: &str = "0f000000-0000-4000-8000-0000000000f1";

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn run(prog: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
    let out = RealSystemOps.run_cmd(prog, args, &[("PSQLRC", "/dev/null"), ("LC_ALL", "C")], stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

struct Box2 {
    root: PathBuf,
}

impl Box2 {
    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    fn data(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }
    fn up(&self, name: &str, port: u16) {
        let d = self.data(name);
        run(&format!("{}/initdb", bin()), &["-D", &d, "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], None).expect("initdb");
        let opts = format!("-p {port} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", self.sock());
        run(&format!("{}/pg_ctl", bin()), &["-D", &d, "-o", &opts, "-l", &format!("{d}.log"), "-w", "start"], None).expect("pg_ctl start");
    }
    /// Superuser SQL on `port`'s server, over its socket; `-q -tA` rows only.
    fn su(&self, port: u16, db: &str, sql: &str) -> String {
        let p = port.to_string();
        run(&format!("{}/psql", bin()), &["-X", "-q", "-tA", "-h", &self.sock(), "-p", &p, "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1"], Some(sql.as_bytes()))
            .unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sql}"))
    }
}

/// K2's view of the boxes: the root helper's `psql --cluster 17-k2supa`
/// is the target's superuser over its socket; the daemon's own client
/// calls (`psql`, `pg_dump`, `pg_restore`) use `REALPG_BIN`.
impl SystemOps for Box2 {
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
        Err(format!("systemctl {} is not part of this test", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of this test", args.join(" ")));
        }
        let mut rest: Vec<&str> = Vec::new();
        let mut it = args[1..].iter();
        while let Some(a) = it.next() {
            if *a == "--cluster" {
                let c = it.next().copied().unwrap_or_default();
                if c != "17-k2supa" {
                    return Err(format!("psql --cluster {c}: only 17-k2supa is made here"));
                }
                continue;
            }
            rest.push(a);
        }
        let (p, sock) = (DST_PORT.to_string(), self.sock());
        let mut full: Vec<&str> = vec!["-X", "-h", &sock, "-p", &p, "-U", "postgres"];
        full.extend(rest);
        run(&format!("{}/psql", bin()), &full, stdin)
    }
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let tool = Path::new(cmd).file_name().and_then(|n| n.to_str()).unwrap_or(cmd);
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        RealSystemOps.run_cmd(&format!("{}/{tool}", bin()), args, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    fn helper_pgnamed(&self) -> Result<(), String> {
        Ok(())
    }
}

struct Stop(&'static Box2);

impl Drop for Stop {
    fn drop(&mut self) {
        for n in ["src", "dst"] {
            let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.0.data(n), "-m", "immediate", "-w", "stop"], None);
        }
        let _ = std::fs::remove_dir_all(&self.0.root);
    }
}

struct Checks(Vec<String>);

impl Checks {
    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok {
            eprintln!("PASS {name}");
        } else {
            eprintln!("FAIL {name}: {detail}");
            self.0.push(format!("{name}: {detail}"));
        }
    }
}

/// The synthetic Supabase project (roles first, on the `postgres` database).
const SRC_ROLES: &str = "CREATE ROLE supabase_admin; CREATE ROLE supabase_auth_admin NOINHERIT; CREATE ROLE supabase_storage_admin NOINHERIT;
CREATE ROLE dashboard_user; CREATE ROLE authenticator NOINHERIT LOGIN;
CREATE ROLE anon NOLOGIN NOINHERIT; CREATE ROLE authenticated NOLOGIN NOINHERIT; CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS;
CREATE ROLE app_reporter NOLOGIN;
CREATE DATABASE supa_src;
";

fn src_sql() -> String {
    format!(
        r#"
CREATE SCHEMA extensions;
CREATE EXTENSION pgcrypto WITH SCHEMA extensions;
CREATE EXTENSION "uuid-ossp" WITH SCHEMA extensions;
CREATE EXTENSION pg_trgm WITH SCHEMA extensions;
GRANT USAGE ON SCHEMA extensions TO anon, authenticated, service_role;
CREATE SCHEMA auth AUTHORIZATION supabase_auth_admin;
CREATE SCHEMA storage AUTHORIZATION supabase_storage_admin;
CREATE SCHEMA realtime; CREATE SCHEMA vault; CREATE SCHEMA graphql_public; CREATE SCHEMA supabase_migrations;

CREATE TABLE auth.users (instance_id uuid, id uuid PRIMARY KEY, aud varchar(255), role varchar(255), email varchar(255),
  encrypted_password varchar(255), email_confirmed_at timestamptz, last_sign_in_at timestamptz, raw_app_meta_data jsonb,
  raw_user_meta_data jsonb, created_at timestamptz, updated_at timestamptz, phone text, banned_until timestamptz,
  is_sso_user boolean NOT NULL DEFAULT false, deleted_at timestamptz, is_anonymous boolean NOT NULL DEFAULT false);
CREATE TABLE auth.identities (id uuid PRIMARY KEY, user_id uuid REFERENCES auth.users(id) ON DELETE CASCADE, provider text);
CREATE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql STABLE AS $$
  SELECT nullif(coalesce(current_setting('request.jwt.claim.sub', true), (current_setting('request.jwt.claims', true)::jsonb ->> 'sub')), '')::uuid $$;
CREATE FUNCTION auth.role() RETURNS text LANGUAGE sql STABLE AS $$
  SELECT nullif(coalesce(current_setting('request.jwt.claim.role', true), (current_setting('request.jwt.claims', true)::jsonb ->> 'role')), '')::text $$;
ALTER TABLE auth.users OWNER TO supabase_auth_admin; ALTER TABLE auth.identities OWNER TO supabase_auth_admin;
ALTER FUNCTION auth.uid() OWNER TO supabase_auth_admin; ALTER FUNCTION auth.role() OWNER TO supabase_auth_admin;
GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role;
GRANT ALL ON auth.users TO dashboard_user;

CREATE TABLE storage.buckets (id text PRIMARY KEY, name text NOT NULL, owner uuid, created_at timestamptz DEFAULT now(),
  updated_at timestamptz DEFAULT now(), public boolean DEFAULT false, file_size_limit bigint, allowed_mime_types text[], owner_id text);
CREATE TABLE storage.objects (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), bucket_id text REFERENCES storage.buckets(id), name text,
  owner uuid, created_at timestamptz DEFAULT now(), updated_at timestamptz DEFAULT now(), last_accessed_at timestamptz DEFAULT now(),
  metadata jsonb, path_tokens text[] GENERATED ALWAYS AS (string_to_array(name, '/')) STORED, version text, owner_id text,
  user_metadata jsonb, level int);
CREATE FUNCTION storage.update_updated_at_column() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.updated_at = now(); RETURN NEW; END $$;
CREATE TRIGGER update_objects_updated_at BEFORE UPDATE ON storage.objects FOR EACH ROW EXECUTE FUNCTION storage.update_updated_at_column();
CREATE FUNCTION storage.foldername(name text) RETURNS text[] LANGUAGE plpgsql AS $$
  DECLARE _parts text[]; BEGIN SELECT string_to_array(name, '/') INTO _parts; RETURN _parts[1:array_length(_parts, 1) - 1]; END $$;
ALTER TABLE storage.buckets ENABLE ROW LEVEL SECURITY; ALTER TABLE storage.objects ENABLE ROW LEVEL SECURITY;
ALTER TABLE storage.buckets OWNER TO supabase_storage_admin; ALTER TABLE storage.objects OWNER TO supabase_storage_admin;
ALTER FUNCTION storage.foldername(text) OWNER TO supabase_storage_admin; ALTER FUNCTION storage.update_updated_at_column() OWNER TO supabase_storage_admin;
GRANT USAGE ON SCHEMA storage TO anon, authenticated, service_role;
GRANT ALL ON storage.objects, storage.buckets TO anon, authenticated, service_role;
CREATE POLICY "Own avatar folder" ON storage.objects FOR SELECT TO authenticated
  USING (bucket_id = 'avatars' AND (storage.foldername(name))[1] = auth.uid()::text);
CREATE POLICY "Public buckets" ON storage.buckets FOR SELECT TO anon, authenticated USING (public);

CREATE TABLE realtime.messages (id bigserial PRIMARY KEY, topic text);
CREATE TABLE vault.secrets (id uuid PRIMARY KEY, name text, secret text);
CREATE VIEW vault.decrypted_secrets AS SELECT id, name, secret AS decrypted_secret FROM vault.secrets;
CREATE TABLE supabase_migrations.schema_migrations (version text PRIMARY KEY, statements text[], name text);
INSERT INTO supabase_migrations.schema_migrations VALUES
  ('20240101120000', ARRAY['create table public.orgs (id uuid primary key, name text not null)', 'alter table public.orgs enable row level security'], 'init'),
  ('20240215093000', ARRAY['create table public.notes (id bigint generated always as identity primary key, body text)'], 'add notes');
CREATE PUBLICATION supabase_realtime;

CREATE TABLE public.orgs (id uuid PRIMARY KEY DEFAULT extensions.uuid_generate_v4(), name text NOT NULL);
CREATE TABLE public.profiles (id uuid PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE, org_id uuid REFERENCES public.orgs(id),
  full_name text, search_name text GENERATED ALWAYS AS (lower(full_name)) STORED);
CREATE INDEX profiles_name_trgm ON public.profiles USING gin (full_name extensions.gin_trgm_ops);
CREATE TABLE public.notes (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  owner_id uuid NOT NULL DEFAULT auth.uid() REFERENCES auth.users(id) ON DELETE CASCADE, org_id uuid REFERENCES public.orgs(id), body text NOT NULL);
CREATE TABLE public.audit (id serial PRIMARY KEY, what text);
ALTER TABLE public.orgs ENABLE ROW LEVEL SECURITY; ALTER TABLE public.profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.notes ENABLE ROW LEVEL SECURITY; ALTER TABLE public.notes FORCE ROW LEVEL SECURITY;
CREATE FUNCTION public.user_org_ids() RETURNS SETOF uuid LANGUAGE sql STABLE SECURITY DEFINER SET search_path = '' AS $$
  SELECT org_id FROM public.profiles WHERE id = auth.uid() $$;
CREATE FUNCTION public.org_member_count(o uuid) RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER SET search_path = '' AS $$
  SELECT count(*) FROM public.profiles WHERE org_id = o $$;
CREATE FUNCTION public.handle_new_user() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = '' AS $$
  BEGIN INSERT INTO public.profiles (id, full_name) VALUES (NEW.id, NEW.raw_user_meta_data ->> 'full_name'); RETURN NEW; END $$;
CREATE TRIGGER on_auth_user_created AFTER INSERT ON auth.users FOR EACH ROW EXECUTE FUNCTION public.handle_new_user();
CREATE FUNCTION public.read_secret(n text) RETURNS text LANGUAGE plpgsql SECURITY DEFINER AS $$
  BEGIN RETURN (SELECT decrypted_secret FROM vault.decrypted_secrets WHERE name = n); END $$;
CREATE POLICY "Members read their orgs" ON public.orgs FOR SELECT TO authenticated USING (id IN (SELECT public.user_org_ids()));
CREATE POLICY "Own profile" ON public.profiles FOR ALL TO authenticated USING (id = auth.uid()) WITH CHECK (id = auth.uid());
CREATE POLICY "Own notes" ON public.notes FOR SELECT TO authenticated USING (owner_id = auth.uid());
CREATE POLICY "Write own notes" ON public.notes FOR INSERT TO authenticated WITH CHECK (owner_id = auth.uid());

GRANT USAGE ON SCHEMA public TO anon, authenticated, service_role;
GRANT ALL ON ALL TABLES IN SCHEMA public TO anon, authenticated, service_role;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO anon, authenticated, service_role;
GRANT ALL ON ALL FUNCTIONS IN SCHEMA public TO anon, authenticated, service_role;
REVOKE ALL ON FUNCTION public.read_secret(text) FROM PUBLIC, anon, authenticated;
ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public GRANT ALL ON TABLES TO anon, authenticated, service_role;
ALTER DEFAULT PRIVILEGES FOR ROLE supabase_admin IN SCHEMA public GRANT ALL ON TABLES TO anon, authenticated, service_role;
GRANT SELECT ON public.audit TO app_reporter;
GRANT ALL ON public.orgs TO supabase_admin;

INSERT INTO public.orgs VALUES ('{ORG}', 'Synthetic Org');
INSERT INTO auth.users (instance_id, id, aud, role, email, encrypted_password, email_confirmed_at, raw_app_meta_data, raw_user_meta_data, created_at, updated_at, phone) VALUES
  ('00000000-0000-0000-0000-000000000000', '{ADA}', 'authenticated', 'authenticated', 'ada@example.com', extensions.crypt('ada-old-pw', extensions.gen_salt('bf', 8)), now(), '{{"provider":"email","org":"synthetic"}}', '{{"full_name":"Ada Example"}}', '2024-03-01 10:00:00+00', now(), NULL),
  ('00000000-0000-0000-0000-000000000000', '{BO}', 'authenticated', 'authenticated', 'bo@example.com', extensions.crypt('bo-old-pw', extensions.gen_salt('bf', 8)), now(), '{{"provider":"email"}}', '{{"full_name":"Bo Example"}}', '2024-03-02 10:00:00+00', now(), '+15550100'),
  ('00000000-0000-0000-0000-000000000000', '{CY}', 'authenticated', 'authenticated', 'cy@example.com', extensions.crypt('cy-old-pw', extensions.gen_salt('bf', 8)), now(), '{{"provider":"email"}}', '{{"full_name":"Cy Example"}}', '2024-03-03 10:00:00+00', now(), NULL);
INSERT INTO auth.identities VALUES (gen_random_uuid(), '{ADA}', 'email');
UPDATE public.profiles SET org_id = '{ORG}' WHERE id IN ('{ADA}', '{BO}');
INSERT INTO public.notes (owner_id, org_id, body) VALUES ('{ADA}', '{ORG}', 'ada one'), ('{ADA}', '{ORG}', 'ada two'), ('{BO}', '{ORG}', 'bo one');
INSERT INTO public.audit (what) VALUES ('seeded');
INSERT INTO storage.buckets (id, name, public) VALUES ('avatars', 'avatars', false);
INSERT INTO storage.objects (bucket_id, name, owner, metadata, level) VALUES
  ('avatars', '{ADA}/a.png', '{ADA}', '{{"size": 10}}', 2), ('avatars', '{BO}/b.png', '{BO}', '{{"size": 20}}', 2);
INSERT INTO vault.secrets VALUES (gen_random_uuid(), 'k', 'synthetic');
"#
    )
}

/// `sql` as `role` with these claims, rolled back (the doors' shape).
fn as_claims(role: &str, sub: Option<&str>, sql: &str) -> String {
    let claims = match sub {
        Some(s) => format!("{{\"sub\":\"{s}\",\"role\":\"{role}\"}}"),
        None => format!("{{\"role\":\"{role}\"}}"),
    };
    format!("BEGIN;\nSET LOCAL ROLE {role};\nSET LOCAL request.jwt.claims = '{claims}';\n{sql}\nROLLBACK;\n")
}

#[test]
#[ignore = "runs only with REALPG_IMPORT=1 and Postgres 17 binaries (a Linux compute job)"]
fn real_pg_import_supabase_end_to_end() {
    assert_eq!(std::env::var("REALPG_IMPORT").as_deref(), Ok("1"), "REALPG_IMPORT=1 marks the throwaway job");
    for t in ["initdb", "pg_ctl", "postgres", "psql", "pg_dump", "pg_restore"] {
        assert!(Path::new(&format!("{}/{t}", bin())).exists(), "{}/{t}: REALPG_BIN needs it", bin());
    }
    let version = run(&format!("{}/postgres", bin()), &["--version"], None).expect("postgres --version");
    let major: u32 = version.split_whitespace().last().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok()).expect("a major");
    assert_eq!(major, 17, "the target is a Postgres 17 named cluster: {version}");
    let root = std::env::temp_dir().join(format!("k2rpi-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let b: &'static Box2 = Box::leak(Box::new(Box2 { root }));
    let _stop = Stop(b);
    b.up("src", SRC_PORT);
    b.up("dst", DST_PORT);
    let mut c = Checks(Vec::new());

    // The source project, dumped as `pg_dump -Fc` would be by its owner.
    b.su(SRC_PORT, "postgres", SRC_ROLES);
    b.su(SRC_PORT, "supa_src", &src_sql());
    let profiles = b.su(SRC_PORT, "supa_src", "SELECT count(*) FROM public.profiles;");
    c.check("source: the sign-up trigger made a profile per user", profiles == "3", &profiles);

    // K2's records: a running SQL server, the workspace, its named cluster.
    let _home = crate::test_support::TempHome::new();
    let _g = crate::sql::sql_server_test_lock();
    let _db = k2_core::db::scoped_for_test();
    let ws_dir = std::env::temp_dir().join(format!("k2-rpi-ws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(k2_core::workspace_dot_dir(&ws_dir).join("db/dumps")).unwrap();
    let ws_path = ws_dir.to_string_lossy().into_owned();
    let ws = format!("ws-rpi-{}", &uuid::Uuid::new_v4().to_string()[..6]);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![ws, ws_path]).unwrap();
        for t in ["sql_grants", "sql_databases", "sql_clusters", "sql_server"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 17, 'localhost', 1)", []).unwrap();
        let mut cl = Cluster::layout(17, DST_PORT, "created");
        cl.name = "k2supa".into();
        cl.state = "online".into();
        cl.owner_project = Some(ws.clone());
        cluster::record_in(&conn, &cl, true).unwrap();
    }
    let secrets = MemSecretStore::default();
    let made = ops::create_database_in_cluster(b, &secrets, &ws, 0, None, Some("supa_app"), Some(17), "17/k2supa");
    c.check("db create --cluster 17/k2supa --name supa_app", made.is_ok(), format!("{made:?}"));
    let db = "supa_app";
    let migrator = format!("{db}_migrator");
    let dump = k2_core::workspace_dot_dir(&ws_dir).join("db/dumps/supa.dump").to_string_lossy().into_owned();
    let sp = SRC_PORT.to_string();
    run(&format!("{}/pg_dump", bin()), &["-Fc", "-h", &b.sock(), "-p", &sp, "-U", "postgres", "-d", "supa_src", "-f", &dump], None).expect("pg_dump");
    let (ws_r, wp_r) = (ws.as_str(), ws_path.as_str());
    let opts = move |allow: bool, dry: bool| Opts { project_id: ws_r, ws_path: wp_r, file: ".k2/db/dumps/supa.dump", db, allow_unresolved: allow, dry_run: dry };
    let target_rels = || b.su(DST_PORT, db, "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relkind = 'r';");

    // 1. The function reading vault stops the import, and nothing is written.
    match import(b, &secrets, &opts(false, false)) {
        Err(Refusal::Unresolved(r)) => {
            let list = r["unresolved"].to_string();
            c.check("unresolved: vault.decrypted_secrets named, by object", list.contains("vault.decrypted_secrets") && list.contains("read_secret"), &list);
        }
        other => c.check("an unresolved reference stops the import", false, format!("{other:?}")),
    }
    let rels = target_rels();
    c.check("nothing written after the stop (only the two _k2_ tables)", rels == "2", &rels);

    // 2. A dry run with --allow-unresolved reports and writes nothing.
    match import(b, &secrets, &opts(true, true)) {
        Ok(r) => {
            c.check("dry run: dryRun and the counts", r["dryRun"] == true && r["authUsers"] == 3 && r["storage"]["objects"] == 2, &r);
            c.check("dry run: FORCE found", r["forceLifted"].to_string().contains("public.notes"), &r["forceLifted"]);
        }
        Err(e) => c.check("dry run", false, format!("{e:?}")),
    }
    c.check("dry run wrote nothing", target_rels() == "2", target_rels());

    // 3. The import.
    let rep = match import(b, &secrets, &opts(true, false)) {
        Ok(r) => r,
        Err(e) => {
            c.check("the import", false, format!("{e:?}"));
            panic!("{} check(s) failed:\n{}", c.0.len(), c.0.join("\n"));
        }
    };
    eprintln!("report: {rep}");
    let dropped = rep["dropped"]["schemas"].to_string();
    for s in ["auth", "storage", "realtime", "vault", "graphql_public", "supabase_migrations", "extensions"] {
        c.check(&format!("report: Supabase's {s} dropped"), dropped.contains(&format!("\"{s}\"")), &dropped);
    }
    c.check("report: pgcrypto, uuid-ossp, pg_trgm carried", rep["carried"]["extensions"] == serde_json::json!(["pg_trgm", "pgcrypto", "uuid-ossp"]), &rep["carried"]);
    c.check("report: app_reporter's grant dropped and named", rep["grants"]["droppedRoles"] == serde_json::json!(["app_reporter"]), &rep["grants"]);
    c.check("report: columns not carried named", rep["columnsNotCarried"].to_string().contains("storage.objects.level"), &rep["columnsNotCarried"]);
    c.check("report: the App's trigger on auth.users kept", rep["carried"]["onSupabaseTables"].to_string().contains("public.handle_new_user"), &rep["carried"]);

    // Owners remapped: the migrator owns every App object.
    let foreign = b.su(DST_PORT, db, &format!(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname IN ('public', 'storage') \
         AND c.relkind IN ('r', 'S', 'v', 'i', 'm') AND pg_get_userbyid(c.relowner) <> '{migrator}';"
    ));
    c.check("every table, sequence and index in public and storage is the migrator's", foreign == "0", &foreign);
    let fowner = b.su(DST_PORT, db, "SELECT string_agg(DISTINCT pg_get_userbyid(p.proowner), ',') FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'public';");
    c.check("functions (SECURITY DEFINER too) are the migrator's", fowner == migrator, &fowner);
    let gone = b.su(DST_PORT, db, "SELECT string_agg(nspname, ',' ORDER BY nspname) FROM pg_namespace WHERE nspname IN ('realtime', 'vault', 'graphql_public', 'supabase_migrations');");
    c.check("Supabase's own schemas are gone", gone.is_empty(), &gone);
    let roles = b.su(DST_PORT, "postgres", "SELECT string_agg(rolname || ':' || rolcanlogin || rolbypassrls, ',' ORDER BY rolname) FROM pg_roles WHERE rolname IN ('anon', 'authenticated', 'service_role', 'supabase_admin', 'app_reporter');");
    c.check("the standard roles exist (service_role BYPASSRLS), Supabase's don't", roles == "anon:falsefalse,authenticated:falsefalse,service_role:falsetrue", &roles);

    // Policies intact and answering per user.
    let pols = b.su(DST_PORT, db, "SELECT string_agg(schemaname || '.' || tablename || ':' || policyname || ':' || array_to_string(roles, '+'), ',' ORDER BY schemaname, tablename, policyname) FROM pg_policies WHERE tablename NOT IN ('_k2_migrations', '_k2_store');");
    c.check(
        "every App policy restored, for its standard roles (the _k2_ tables' own seal aside)",
        pols == "public.notes:Own notes:authenticated,public.notes:Write own notes:authenticated,public.orgs:Members read their orgs:authenticated,\
                 public.profiles:Own profile:authenticated,storage.buckets:Public buckets:anon+authenticated,storage.objects:Own avatar folder:authenticated",
        &pols,
    );
    let force = b.su(DST_PORT, db, "SELECT relrowsecurity || '/' || relforcerowsecurity FROM pg_class WHERE oid = 'public.notes'::regclass;");
    c.check("notes: row security on, FORCE lifted", force == "true/false", &force);
    let n_ada = b.su(DST_PORT, db, &as_claims("authenticated", Some(ADA), "SELECT count(*) FROM public.notes;"));
    let n_bo = b.su(DST_PORT, db, &as_claims("authenticated", Some(BO), "SELECT count(*) FROM public.notes;"));
    let n_anon = b.su(DST_PORT, db, &as_claims("anon", None, "SELECT count(*) FROM public.notes;"));
    let n_svc = b.su(DST_PORT, db, &as_claims("service_role", None, "SELECT count(*) FROM public.notes;"));
    c.check("auth.uid() policies: Ada 2, Bo 1, anon 0, service_role all 3", (n_ada.as_str(), n_bo.as_str(), n_anon.as_str(), n_svc.as_str()) == ("2", "1", "0", "3"), format!("{n_ada} {n_bo} {n_anon} {n_svc}"));
    let orgs = b.su(DST_PORT, db, &as_claims("authenticated", Some(ADA), "SELECT count(*) FROM public.orgs;"));
    let members = b.su(DST_PORT, db, &as_claims("authenticated", Some(ADA), &format!("SELECT public.org_member_count('{ORG}');")));
    let own = b.su(DST_PORT, db, &as_claims("authenticated", Some(ADA), "SELECT count(*) FROM public.profiles;"));
    c.check("SECURITY DEFINER helpers still bypass row security (owner = migrator)", orgs == "1" && members == "2" && own == "1", format!("orgs {orgs} members {members} own profiles {own}"));
    let wrote = b.su(DST_PORT, db, &as_claims("authenticated", Some(BO), "INSERT INTO public.notes (body) VALUES ('bo two') RETURNING owner_id;"));
    c.check("an insert as Bo: owner_id defaults to auth.uid() and passes WITH CHECK", wrote == BO, &wrote);
    let avatars = b.su(DST_PORT, db, &as_claims("authenticated", Some(ADA), "SELECT string_agg(name, ',') FROM storage.objects;"));
    c.check("Storage policy: Ada sees her own folder only", avatars == format!("{ADA}/a.png"), &avatars);

    // Grants mapped.
    let g = b.su(DST_PORT, db, &format!(
        "SELECT has_table_privilege('anon', 'public.orgs', 'SELECT') || '/' || (SELECT coalesce(array_to_string(relacl, ','), '') LIKE '%app_reporter%' FROM pg_class WHERE oid = 'public.audit'::regclass) || '/' \
         || has_function_privilege('authenticated', 'public.read_secret(text)', 'EXECUTE') || '/' || has_table_privilege('{db}_agent', 'public.orgs', 'SELECT');"
    ));
    c.check("grants: anon kept, app_reporter dropped, a revoke kept, the workspace's agent synced", g == "true/false/false/true", &g);
    let later = b.su(DST_PORT, db, &format!("SET ROLE \"{migrator}\"; CREATE TABLE public.later (x int); RESET ROLE; SELECT has_table_privilege('authenticated', 'public.later', 'SELECT');"));
    c.check("default privileges: postgres's are now the migrator's", later == "t", &later);
    let path = b.su(DST_PORT, "postgres", &format!("SELECT array_to_string(setconfig, ';') FROM pg_db_role_setting s JOIN pg_database d ON d.oid = s.setdatabase WHERE d.datname = '{db}';"));
    c.check("search_path includes extensions", path.contains("extensions"), &path);

    // K2's auth.users: same ids, no passwords; the data and Storage rows.
    let users = b.su(DST_PORT, db, "SELECT string_agg(id::text || ' ' || email, ',' ORDER BY email) || '|' || count(encrypted_password) FROM auth.users;");
    c.check("auth.users: the same ids and emails, no password held", users == format!("{ADA} ada@example.com,{BO} bo@example.com,{CY} cy@example.com|0"), &users);
    let data = b.su(DST_PORT, db, "SELECT (SELECT count(*) FROM public.profiles) || '/' || (SELECT count(*) FROM public.notes) || '/' || (SELECT count(*) FROM storage.objects) || '/' || (SELECT string_agg(array_to_string(path_tokens, '>'), ',' ORDER BY name) FROM storage.objects);");
    c.check("data: 3 profiles, 3 notes (Bo's insert was rolled back), 2 objects with path tokens", data == format!("3/3/2/{ADA}>a.png,{BO}>b.png"), &data);
    let trig = b.su(DST_PORT, db, "SELECT string_agg(tgname, ',' ORDER BY tgname) FROM pg_trigger WHERE NOT tgisinternal;");
    c.check("triggers: the App's on auth.users and the storage schema's own updated_at one (the dump's copy dropped)", trig == "on_auth_user_created,update_objects_updated_at", &trig);
    let tfn = b.su(DST_PORT, db, "SELECT p.pronamespace::regnamespace || '.' || p.proname FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid WHERE t.tgname = 'update_objects_updated_at';");
    c.check("the storage trigger is the installed schema's (lane D's)", tfn == "storage.update_updated_at_column", &tfn);
    let cascade = b.su(DST_PORT, db, &format!("DELETE FROM auth.users WHERE id = '{CY}'; SELECT count(*) FROM public.profiles WHERE id = '{CY}';"));
    c.check("ON DELETE CASCADE from auth.users still works", cascade == "0", &cascade);

    // The migration map and a later k2 db migrate.
    let applied = b.su(DST_PORT, db, "SELECT string_agg(version, ',' ORDER BY version) FROM _k2_migrations;");
    c.check("migrations recorded as NNNN_name", applied == "0001_init,0002_add_notes", &applied);
    let file = std::fs::read_to_string(k2_core::workspace_dot_dir(&ws_dir).join("db/migrations/0002_add_notes.sql")).unwrap_or_default();
    c.check("the renamed file holds its statements", file.contains("20240215093000_add notes") && file.contains("create table public.notes"), &file);
    let map = std::fs::read_to_string(ws_dir.join(super::MAP_FILE)).unwrap_or_default();
    c.check("the map is recorded", map.contains("\"supabase\": \"20240101120000\"") && map.contains("\"renamed\": \"0001_init\""), &map);
    std::fs::write(k2_core::workspace_dot_dir(&ws_dir).join("db/migrations/0003_more.sql"), "CREATE TABLE public.more (x int);\n").unwrap();
    let migrated = crate::sql::routes::with_test_ops(b, || ops::migrate_db(b, &secrets, &ws, Some(db), &ws_path, None));
    c.check("db migrate then applies only 0003", migrated.as_ref().is_ok_and(|m| m["applied"] == serde_json::json!(["0003_more"])), format!("{migrated:?}"));

    // 4. Again into the same database: refused.
    match import(b, &secrets, &opts(true, false)) {
        Err(Refusal::Ops(e)) => c.check("a second import into it is refused", e.code() == "import_target_not_empty", e.hint()),
        other => c.check("a second import into it is refused", false, format!("{other:?}")),
    }

    // 5. The users, with their ids and bcrypt hashes, into an App.
    let lines = b.su(SRC_PORT, "supa_src", "SELECT json_build_object('id', id, 'email', email, 'encrypted_password', encrypted_password, \
        'raw_app_meta_data', raw_app_meta_data, 'raw_user_meta_data', raw_user_meta_data, 'created_at', created_at, 'banned_until', banned_until) FROM auth.users ORDER BY email;");
    c.check("the users file is Supabase's bcrypt ($2a$)", lines.matches("\"$2a$08$").count() == 3, lines.lines().count());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let (rows, bad) = k2_core::skin::import::parse_jsonl(&lines, now);
    let opts = k2_core::skin::import::Options { app: "svc-rpi", role: None, apply: true, agent_workspace: None, actor: "owner", reserved: &[] };
    let rep: J = k2_core::skin::import::import(&rows, &bad, opts).unwrap_or_else(|e| e);
    c.check("app user import --apply: 3 new", rep["new"] == 3 && rep["written"] == true, &rep);
    for (email, pw, id) in [("ada@example.com", "ada-old-pw", ADA), ("bo@example.com", "bo-old-pw", BO)] {
        let out = k2_core::skin::check_and_record_login(email, pw);
        let ok = matches!(&out, k2_core::skin::SkinLoginOutcome::Ok(p) if p.id == id);
        c.check(&format!("{email} signs in with the old password and keeps id {id}"), ok, format!("{out:?}"));
        let user = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.id == id).map(|p| p.username).unwrap_or_default();
        let hash = k2_core::skin::import::stored_hash_for_tests(&user).unwrap_or_default();
        c.check(&format!("{email} is rehashed to Argon2"), hash.starts_with("$argon2"), &hash[..hash.len().min(10)]);
    }
    let wrong = k2_core::skin::check_and_record_login("cy@example.com", "not-cys-pw");
    c.check("a wrong password is refused", matches!(wrong, k2_core::skin::SkinLoginOutcome::BadCreds), format!("{wrong:?}"));
    let in_db: Vec<String> = b.su(DST_PORT, db, "SELECT id FROM auth.users ORDER BY id;").lines().map(str::to_string).collect();
    let in_store: Vec<String> = {
        let mut v: Vec<String> = k2_core::skin::list_principals().unwrap().into_iter().map(|p| p.id).filter(|i| in_db.contains(i)).collect();
        v.sort();
        v
    };
    c.check("every auth.users id is an App user's id", in_store == in_db && !in_db.is_empty(), format!("{in_db:?} vs {in_store:?}"));

    let _ = std::fs::remove_dir_all(&ws_dir);
    assert!(c.0.is_empty(), "{} check(s) failed:\n{}", c.0.len(), c.0.join("\n"));
}
