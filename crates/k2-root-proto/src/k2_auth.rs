//! The Supabase-compatible identity layer in an App's database
//! (prd-app-supabase-compat-v1 §5, §9.3; lane A): the standard roles, the
//! door's login, the `auth` schema and K2's claims source. Canned SQL,
//! shared by the daemon (`k2 app compat on`) and the import mapper (lane B),
//! so both install the same bytes.
//!
//! **Where the claims come from** (`k2.claims()`, which `auth.jwt()`,
//! `auth.uid()`, `auth.role()` and `auth.email()` read):
//! 1. a claims lock K2 wrote on this connection: the brokered lane's
//!    (`/cli/db/query`, written by the migrator before `SET ROLE`) or
//!    `k2.set_claims(token)`'s (a token the daemon signed, checked here). A
//!    lock counts only when the definer (the migrator) owns it, as for
//!    `k2.actor_info()`;
//! 2. else the settings `request.jwt.claims` / `request.jwt.claim.sub`, but
//!    only on the door's own login (`<db>_authenticator`), which runs only
//!    SQL the daemon built (PostgREST's model). On any other login the
//!    settings are not an identity: anyone can set a setting.
//!
//! So a reader, a grantee or the App's own login can't become an App user
//! through `auth.*` by setting a value. (`k2.skin_uid()`'s setting is the
//! older App lane and stays open; see `k2 study db`.)

use crate::pg_major::role_name_ok;

/// The standard role names (cluster-wide in Postgres, shared by every
/// compat database of a cluster; rights live on objects).
pub const STANDARD_ROLES: [&str; 3] = ["anon", "authenticated", "service_role"];

/// The one statement that may give a role BYPASSRLS: K2's own
/// `service_role`, NOLOGIN (the daemon's role-SQL check allows exactly
/// this text and nothing else with BYPASSRLS).
pub const SERVICE_ROLE_BYPASS_SQL: &str =
    "ALTER ROLE service_role WITH NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION BYPASSRLS;";

/// Make (or put back) the three standard roles. Run as the cluster
/// superuser in database `postgres`. Idempotent; an existing role of that
/// name gets K2's options again (never LOGIN).
pub fn standard_roles_sql() -> String {
    let mut s = String::new();
    for r in STANDARD_ROLES {
        s.push_str(&format!(
            "DO $k2c$\nBEGIN\n  CREATE ROLE {r} NOLOGIN NOINHERIT;\nEXCEPTION WHEN duplicate_object THEN NULL;\nEND $k2c$;\n"
        ));
        if r == "service_role" {
            s.push_str(SERVICE_ROLE_BYPASS_SQL);
        } else {
            s.push_str(&format!("ALTER ROLE {r} WITH NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;"));
        }
        s.push('\n');
    }
    s
}

/// Before the standard roles get K2's attributes (service_role's BYPASSRLS
/// above all): strip every membership a role of that name already had,
/// in both directions, except K2's own members (each compat
/// database's door login in all three, its migrator in anon and
/// authenticated only). Otherwise someone granted a
/// pre-existing `service_role` could `SET ROLE` to it and bypass row
/// security. Run first, in the same session as [`standard_roles_sql`]; its
/// last line names what was stripped (`member>role` pairs, or empty).
/// `None`: a name isn't a role name.
pub fn standard_roles_strip_sql(authenticators: &[String], migrators: &[String]) -> Option<String> {
    if authenticators.iter().chain(migrators).any(|k| !role_name_ok(k)) {
        return None;
    }
    let lits = |v: &[String]| v.iter().map(|k| literal(k)).collect::<Vec<_>>().join(", ");
    let (auth_lits, mig_lits) = (lits(authenticators), lits(migrators));
    Some(format!(
        "CREATE TEMP TABLE IF NOT EXISTS k2_stripped (pair text);\n\
         DO $k2c$\n\
         DECLARE\n\
           authn text[] := ARRAY[{auth_lits}]::text[];\n\
           migr text[] := ARRAY[{mig_lits}]::text[];\n\
           r record;\n\
         BEGIN\n\
           FOR r IN\n\
             SELECT pg_catalog.pg_get_userbyid(m.roleid) AS grp, pg_catalog.pg_get_userbyid(m.member) AS mem,\n\
                    pg_catalog.pg_get_userbyid(m.grantor) AS gby\n\
             FROM pg_catalog.pg_auth_members m\n\
             WHERE (pg_catalog.pg_get_userbyid(m.roleid) IN ('anon', 'authenticated', 'service_role')\n\
                    AND NOT (pg_catalog.pg_get_userbyid(m.member) = ANY (authn)\n\
                             OR (pg_catalog.pg_get_userbyid(m.member) = ANY (migr) AND pg_catalog.pg_get_userbyid(m.roleid) <> 'service_role')))\n\
                OR pg_catalog.pg_get_userbyid(m.member) IN ('anon', 'authenticated', 'service_role')\n\
           LOOP\n\
             EXECUTE format('REVOKE %I FROM %I GRANTED BY %I', r.grp, r.mem, r.gby);\n\
             INSERT INTO pg_temp.k2_stripped VALUES (r.mem || '>' || r.grp);\n\
           END LOOP;\n\
           IF EXISTS (\n\
             SELECT 1 FROM pg_catalog.pg_auth_members m\n\
             WHERE (pg_catalog.pg_get_userbyid(m.roleid) IN ('anon', 'authenticated', 'service_role')\n\
                    AND NOT (pg_catalog.pg_get_userbyid(m.member) = ANY (authn)\n\
                             OR (pg_catalog.pg_get_userbyid(m.member) = ANY (migr) AND pg_catalog.pg_get_userbyid(m.roleid) <> 'service_role')))\n\
                OR pg_catalog.pg_get_userbyid(m.member) IN ('anon', 'authenticated', 'service_role')\n\
           ) THEN\n\
             RAISE EXCEPTION 'k2: a membership of anon, authenticated or service_role could not be revoked';\n\
           END IF;\n\
         END\n\
         $k2c$;\n\
         SELECT 'k2-stripped:' || coalesce(string_agg(pair, ', ' ORDER BY pair), '') FROM pg_temp.k2_stripped;\n"
    ))
}

/// `<db>_authenticator`, or for a name too long for Postgres the first
/// characters of the database and a short hash. `None`: not a role name.
pub fn authenticator_role(db: &str) -> Option<String> {
    let full = format!("{db}_authenticator");
    if full.len() <= 63 {
        return role_name_ok(&full).then_some(full);
    }
    // FNV-1a: stable across builds, no crate.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in db.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let tail = format!("_{:08x}_authn", (h & 0xffff_ffff) as u32);
    let head: String = db.chars().take(63 - tail.len()).collect();
    let name = format!("{head}{tail}");
    role_name_ok(&name).then_some(name)
}

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The door's login for `db` (cluster superuser, database `postgres`):
/// LOGIN, NOINHERIT, a member of the three standard roles, nothing else;
/// CONNECT on `db` only. Also takes CONNECT on `db` from PUBLIC and gives
/// it back to K2's own roles of `db` (the migrator owns it; the agent and
/// every grantee and App group hold their own grant). `None`: a name isn't
/// a role name.
pub fn authenticator_sql(db: &str, login: &str, password: &str, conn_limit: u32) -> Option<String> {
    if !role_name_ok(login) || db.is_empty() || db.len() > 63 {
        return None;
    }
    let migrator = format!("{db}_migrator");
    let agent = format!("{db}_agent");
    if !role_name_ok(&migrator) || !role_name_ok(&agent) {
        return None;
    }
    let (l, pw) = (ident(login), literal(password));
    let opts = format!("NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS CONNECTION LIMIT {conn_limit}");
    Some(format!(
        "DO $k2c$\nBEGIN\n  CREATE ROLE {l} LOGIN PASSWORD {pw} {opts};\nEXCEPTION WHEN duplicate_object THEN NULL;\nEND $k2c$;\n\
         ALTER ROLE {l} WITH LOGIN PASSWORD {pw} {opts};\n\
         GRANT anon, authenticated, service_role TO {l};\n\
         GRANT anon, authenticated TO {m};\n\
         REVOKE CONNECT ON DATABASE {d} FROM PUBLIC;\n\
         GRANT CONNECT ON DATABASE {d} TO {m}, {a}, {l};\n",
        m = ident(&migrator),
        a = ident(&agent),
        d = ident(db),
    ))
}

/// Take the door's login away (cluster superuser, database `postgres`):
/// it stops logging in and its sessions end. The standard roles and the
/// CONNECT wall stay.
pub fn authenticator_off_sql(login: &str) -> Option<String> {
    if !role_name_ok(login) {
        return None;
    }
    let ll = literal(login);
    Some(format!(
        "DO $k2c$\nBEGIN\n  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {ll}) THEN\n    EXECUTE format('ALTER ROLE %I NOLOGIN', {ll});\n  END IF;\nEND $k2c$;\n\
         SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE usename = {ll};\n"
    ))
}

/// The comment that marks schema `auth` as K2's. Turn-on refuses an `auth`
/// schema without it (it is the App's own, or a Supabase dump's).
pub const AUTH_SCHEMA_MARK: &str = "k2 compat: auth schema (prd-app-supabase-compat-v1)";

/// The lock-owner test both claims readers use: the temp table `rel` is the
/// definer's (the migrator's). Same text as `k2.actor_info()`.
const OWNER_CHECK: &str = "SELECT 1 FROM pg_catalog.pg_class c\n    WHERE c.oid = rel\n      AND c.relpersistence = 't'\n      AND c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)";

/// HMAC-SHA-256 (RFC 2104) on Postgres's own `sha256()`: no extension.
const HMAC_SQL: &str = r#"CREATE OR REPLACE FUNCTION k2.hmac_sha256(key bytea, msg bytea) RETURNS bytea
LANGUAGE plpgsql
IMMUTABLE STRICT
SET search_path = pg_catalog, pg_temp
AS $k2fn$
DECLARE
  k bytea := key;
  ipad bytea;
  opad bytea;
  i int;
BEGIN
  IF length(k) > 64 THEN
    k := sha256(k);
  END IF;
  k := k || decode(repeat('00', 64 - length(k)), 'hex');
  ipad := k;
  opad := k;
  FOR i IN 0..63 LOOP
    ipad := set_byte(ipad, i, get_byte(k, i) # 54);
    opad := set_byte(opad, i, get_byte(k, i) # 92);
  END LOOP;
  RETURN sha256(opad || sha256(ipad || msg));
END;
$k2fn$;
"#;

/// The prefix of a claims token: `k2c1.<hex payload>.<hex mac>`.
pub const CLAIMS_TOKEN_PREFIX: &str = "k2c1";

/// The `auth` schema and K2's claims source for `db` (cluster superuser,
/// in `db`). Owners: the migrator (`k2.claims()` and `k2.set_claims()` are
/// SECURITY DEFINER, so a lock counts only when the migrator owns it).
/// `auth.users` is a table the daemon writes (its `id` takes foreign keys);
/// no App role reads it directly. Idempotent: an existing install is
/// replaced in place and `auth.users` keeps its rows. `None`: a name isn't
/// a role name.
pub fn auth_schema_sql(db: &str, authenticator: &str) -> Option<String> {
    let migrator = format!("{db}_migrator");
    if !role_name_ok(&migrator) || !role_name_ok(authenticator) {
        return None;
    }
    let m = ident(&migrator);
    let authn = literal(authenticator);
    let mark = literal(AUTH_SCHEMA_MARK);
    let mut s = String::new();
    s.push_str("CREATE SCHEMA IF NOT EXISTS k2;\nGRANT USAGE ON SCHEMA k2 TO PUBLIC;\nCREATE SCHEMA IF NOT EXISTS auth;\n");
    s.push_str(&format!("COMMENT ON SCHEMA auth IS {mark};\n"));
    s.push_str("GRANT USAGE ON SCHEMA auth TO PUBLIC;\n");
    s.push_str(HMAC_SQL);
    s.push_str(&format!(
        r#"CREATE OR REPLACE FUNCTION k2.claims() RETURNS jsonb
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $k2fn$
DECLARE
  rel oid;
  c jsonb;
  sub text;
BEGIN
  BEGIN
    rel := 'pg_temp.k2_claims_lock'::regclass;
  EXCEPTION
    WHEN undefined_table OR invalid_schema_name THEN
      rel := NULL;
  END;
  IF rel IS NOT NULL AND EXISTS (
    {OWNER_CHECK}
  ) THEN
    SELECT l.claims INTO c FROM pg_temp.k2_claims_lock l LIMIT 1;
    IF FOUND THEN
      RETURN c;
    END IF;
  END IF;
  IF session_user::text = {authn} THEN
    c := nullif(current_setting('request.jwt.claims', true), '')::jsonb;
    sub := nullif(current_setting('request.jwt.claim.sub', true), '');
    IF sub IS NOT NULL THEN
      c := coalesce(c, '{{}}'::jsonb) || jsonb_build_object('sub', sub);
    END IF;
    RETURN c;
  END IF;
  RETURN NULL;
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.claims() TO PUBLIC;
CREATE TABLE IF NOT EXISTS k2.claims_key (
  id int PRIMARY KEY CHECK (id = 1),
  key bytea NOT NULL CHECK (length(key) >= 32)
);
REVOKE ALL ON TABLE k2.claims_key FROM PUBLIC;
CREATE OR REPLACE FUNCTION k2.set_claims(token text) RETURNS jsonb
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $k2fn$
DECLARE
  parts text[];
  payload bytea;
  mac bytea;
  k bytea;
  body jsonb;
  now_s bigint;
  rel oid;
BEGIN
  parts := string_to_array(token, '.');
  IF parts IS NULL OR coalesce(array_length(parts, 1), 0) <> 3 OR parts[1] <> 'k2c1' THEN
    RAISE EXCEPTION 'k2.set_claims: not a K2 claims token' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  BEGIN
    payload := decode(parts[2], 'hex');
    mac := decode(parts[3], 'hex');
  EXCEPTION
    WHEN others THEN
      RAISE EXCEPTION 'k2.set_claims: not a K2 claims token' USING ERRCODE = 'invalid_authorization_specification';
  END;
  -- The key is a secret: only its owner (this function's definer) may
  -- read it. Refuse to sign anyone in while any other role could.
  IF EXISTS (
       SELECT 1 FROM pg_catalog.pg_class c CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) a
       WHERE c.oid = 'k2.claims_key'::regclass AND a.grantee <> c.relowner)
     OR EXISTS (
       SELECT 1 FROM pg_catalog.pg_attribute t
       WHERE t.attrelid = 'k2.claims_key'::regclass AND t.attacl IS NOT NULL)
     OR EXISTS (
       SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_roles r ON r.oid = c.relowner
       WHERE c.oid = 'k2.claims_key'::regclass AND r.rolname <> current_user)
     OR EXISTS (
       SELECT 1 FROM pg_catalog.pg_auth_members m
       WHERE m.roleid = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)
          OR m.roleid IN (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname IN ('pg_read_all_data', 'pg_write_all_data')))
  THEN
    RAISE EXCEPTION 'k2.set_claims: the claims key is readable by another role (k2 app compat on puts it back)' USING ERRCODE = 'insufficient_privilege';
  END IF;
  SELECT ck.key INTO k FROM k2.claims_key ck WHERE ck.id = 1;
  IF k IS NULL THEN
    RAISE EXCEPTION 'k2.set_claims: this database has no claims key (k2 app compat on)' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  -- Compared as hashes, so the time taken says nothing about the MAC.
  IF sha256(k2.hmac_sha256(k, convert_to(parts[1] || '.' || parts[2], 'UTF8'))) IS DISTINCT FROM sha256(mac) THEN
    RAISE EXCEPTION 'k2.set_claims: bad signature' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  body := convert_from(payload, 'UTF8')::jsonb;
  IF body ->> 'db' IS DISTINCT FROM current_database() THEN
    RAISE EXCEPTION 'k2.set_claims: the token is for another database' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  now_s := extract(epoch FROM clock_timestamp())::bigint;
  IF (body ->> 'exp')::bigint < now_s OR (body ->> 'iat')::bigint > now_s + 60 THEN
    RAISE EXCEPTION 'k2.set_claims: the token has expired' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  BEGIN
    rel := 'pg_temp.k2_claims_lock'::regclass;
  EXCEPTION
    WHEN undefined_table OR invalid_schema_name THEN
      rel := NULL;
  END;
  IF rel IS NULL THEN
    CREATE TEMP TABLE k2_claims_lock (claims jsonb NOT NULL) ON COMMIT DELETE ROWS;
  ELSIF NOT EXISTS (
    {OWNER_CHECK}
  ) THEN
    RAISE EXCEPTION 'k2.set_claims: a k2_claims_lock K2 did not write is on this connection' USING ERRCODE = 'invalid_authorization_specification';
  END IF;
  DELETE FROM pg_temp.k2_claims_lock;
  INSERT INTO pg_temp.k2_claims_lock (claims) VALUES (body -> 'claims');
  RETURN body -> 'claims';
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.set_claims(text) TO PUBLIC;
CREATE OR REPLACE FUNCTION auth.jwt() RETURNS jsonb
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT k2.claims()
$k2fn$;
CREATE OR REPLACE FUNCTION auth.uid() RETURNS uuid
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT nullif(k2.claims() ->> 'sub', '')::uuid
$k2fn$;
CREATE OR REPLACE FUNCTION auth.role() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT nullif(k2.claims() ->> 'role', '')
$k2fn$;
CREATE OR REPLACE FUNCTION auth.email() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT nullif(k2.claims() ->> 'email', '')
$k2fn$;
GRANT EXECUTE ON FUNCTION auth.jwt(), auth.uid(), auth.role(), auth.email() TO PUBLIC;
CREATE TABLE IF NOT EXISTS auth.users (
  instance_id uuid,
  id uuid PRIMARY KEY,
  aud text NOT NULL DEFAULT 'authenticated',
  role text NOT NULL DEFAULT 'authenticated',
  email text,
  encrypted_password text CHECK (encrypted_password IS NULL),
  email_confirmed_at timestamptz,
  invited_at timestamptz,
  last_sign_in_at timestamptz,
  raw_app_meta_data jsonb NOT NULL DEFAULT '{{}}'::jsonb,
  raw_user_meta_data jsonb NOT NULL DEFAULT '{{}}'::jsonb,
  is_super_admin boolean,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  phone text,
  banned_until timestamptz,
  deleted_at timestamptz,
  is_anonymous boolean NOT NULL DEFAULT false,
  is_sso_user boolean NOT NULL DEFAULT false
);
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS email text;
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS raw_app_meta_data jsonb NOT NULL DEFAULT '{{}}'::jsonb;
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS raw_user_meta_data jsonb NOT NULL DEFAULT '{{}}'::jsonb;
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS created_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS last_sign_in_at timestamptz;
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS email_confirmed_at timestamptz;
ALTER TABLE auth.users ADD COLUMN IF NOT EXISTS banned_until timestamptz;
CREATE INDEX IF NOT EXISTS users_email_idx ON auth.users (lower(email));
REVOKE ALL ON TABLE auth.users FROM PUBLIC, anon, authenticated, service_role;
CREATE OR REPLACE FUNCTION k2.sessions_not_built() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $k2fn$
BEGIN
  RAISE EXCEPTION 'auth.sessions and auth.refresh_tokens are not built in K2 yet: end a user''s sessions with k2 app user sessions <user> --app <app> --revoke-all'
    USING ERRCODE = 'feature_not_supported';
END;
$k2fn$;
CREATE OR REPLACE FUNCTION k2.sessions_stub()
RETURNS TABLE (id uuid, user_id uuid, created_at timestamptz, updated_at timestamptz, factor_id uuid, aal text,
               not_after timestamptz, refreshed_at timestamp, user_agent text, ip inet, tag text)
LANGUAGE plpgsql
VOLATILE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
BEGIN
  RAISE EXCEPTION 'auth.sessions is not built in K2 yet: end a user''s sessions with k2 app user sessions <user> --app <app> --revoke-all'
    USING ERRCODE = 'feature_not_supported';
END;
$k2fn$;
CREATE OR REPLACE FUNCTION k2.refresh_tokens_stub()
RETURNS TABLE (instance_id uuid, id bigint, token text, user_id text, revoked boolean, created_at timestamptz,
               updated_at timestamptz, parent text, session_id uuid)
LANGUAGE plpgsql
VOLATILE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
BEGIN
  RAISE EXCEPTION 'auth.refresh_tokens is not built in K2 yet: end a user''s sessions with k2 app user sessions <user> --app <app> --revoke-all'
    USING ERRCODE = 'feature_not_supported';
END;
$k2fn$;
CREATE OR REPLACE VIEW auth.sessions AS SELECT * FROM k2.sessions_stub();
CREATE OR REPLACE VIEW auth.refresh_tokens AS SELECT * FROM k2.refresh_tokens_stub();
ALTER FUNCTION k2.sessions_stub() OWNER TO {m};
ALTER FUNCTION k2.refresh_tokens_stub() OWNER TO {m};
DROP TRIGGER IF EXISTS k2_not_built ON auth.sessions;
CREATE TRIGGER k2_not_built INSTEAD OF INSERT OR UPDATE OR DELETE ON auth.sessions FOR EACH ROW EXECUTE FUNCTION k2.sessions_not_built();
DROP TRIGGER IF EXISTS k2_not_built ON auth.refresh_tokens;
CREATE TRIGGER k2_not_built INSTEAD OF INSERT OR UPDATE OR DELETE ON auth.refresh_tokens FOR EACH ROW EXECUTE FUNCTION k2.sessions_not_built();
REVOKE ALL ON auth.sessions, auth.refresh_tokens FROM PUBLIC;
ALTER VIEW auth.sessions OWNER TO {m};
ALTER VIEW auth.refresh_tokens OWNER TO {m};
ALTER FUNCTION k2.sessions_not_built() OWNER TO {m};
COMMENT ON TABLE auth.users IS 'K2 writes this table from the App''s users (k2 app compat sync-users); it is not the user store.';
ALTER SCHEMA auth OWNER TO {m};
ALTER TABLE auth.users OWNER TO {m};
ALTER TABLE k2.claims_key OWNER TO {m};
ALTER FUNCTION k2.claims() OWNER TO {m};
ALTER FUNCTION k2.set_claims(text) OWNER TO {m};
ALTER FUNCTION k2.hmac_sha256(bytea, bytea) OWNER TO {m};
ALTER FUNCTION auth.jwt() OWNER TO {m};
ALTER FUNCTION auth.uid() OWNER TO {m};
ALTER FUNCTION auth.role() OWNER TO {m};
ALTER FUNCTION auth.email() OWNER TO {m};
REVOKE ALL ON TABLE k2.claims_key, auth.users FROM PUBLIC;
DO $k2c$
DECLARE
  g record;
BEGIN
  FOR g IN
    SELECT DISTINCT c.oid::regclass::text AS rel, pg_catalog.pg_get_userbyid(a.grantee) AS who
    FROM pg_catalog.pg_class c CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) a
    WHERE c.oid IN ('k2.claims_key'::regclass, 'auth.users'::regclass) AND a.grantee <> 0 AND a.grantee <> c.relowner
  LOOP
    EXECUTE format('REVOKE ALL ON TABLE %s FROM %I', g.rel, g.who);
  END LOOP;
  FOR g IN
    SELECT DISTINCT c.oid::regclass::text AS rel, t.attname, a.grantee AS gid, pg_catalog.pg_get_userbyid(a.grantee) AS who
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_attribute t ON t.attrelid = c.oid
    CROSS JOIN LATERAL pg_catalog.aclexplode(t.attacl) a
    WHERE c.oid IN ('k2.claims_key'::regclass, 'auth.users'::regclass) AND a.grantee <> c.relowner
  LOOP
    IF g.gid = 0 THEN
      EXECUTE format('REVOKE ALL (%I) ON TABLE %s FROM PUBLIC', g.attname, g.rel);
    ELSE
      EXECUTE format('REVOKE ALL (%I) ON TABLE %s FROM %I', g.attname, g.rel, g.who);
    END IF;
  END LOOP;
END
$k2c$;
"#
    ));
    Some(s)
}

/// Is schema `auth` in this database K2's, absent, or someone else's?
/// The query (any role) prints `none`, `k2` or `other`.
pub const AUTH_SCHEMA_OWNER_QUERY: &str = "SELECT CASE WHEN n.oid IS NULL THEN 'none' \
     WHEN obj_description(n.oid, 'pg_namespace') = 'k2 compat: auth schema (prd-app-supabase-compat-v1)' THEN 'k2' \
     ELSE 'other' END FROM (SELECT 1) one LEFT JOIN pg_catalog.pg_namespace n ON n.nspname = 'auth';";

/// Put this database's claims key, as the database's migrator (the key's
/// owner; never the superuser, so an App trigger on the table runs unprivileged).
/// `key_hex`: 64+ hex digits.
pub fn claims_key_sql(key_hex: &str) -> Option<String> {
    if key_hex.len() < 64 || key_hex.len() % 2 != 0 || !key_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "INSERT INTO k2.claims_key (id, key) VALUES (1, decode('{key_hex}', 'hex')) ON CONFLICT (id) DO UPDATE SET key = excluded.key;\n"
    ))
}

/// The brokered lane's claims lock (the migrator, before `SET ROLE`, in the
/// same transaction as the principal lock), and the plain settings for a
/// policy that reads `current_setting('request.jwt.claims')` itself. The
/// lane's caller can't run `SET` or `set_config`, so neither moves.
pub fn claims_lock_sql(claims_json: &str, sub: Option<&str>) -> String {
    let c = literal(claims_json);
    let sub = literal(sub.unwrap_or(""));
    format!(
        "CREATE TEMP TABLE pg_temp.k2_claims_lock (claims jsonb NOT NULL) ON COMMIT PRESERVE ROWS;\n\
         INSERT INTO pg_temp.k2_claims_lock (claims) VALUES ({c}::jsonb);\n\
         SET \"request.jwt.claims\" TO {c};\n\
         SET \"request.jwt.claim.sub\" TO {sub};\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_roles_never_log_in_and_only_service_role_bypasses() {
        let s = standard_roles_sql();
        for r in STANDARD_ROLES {
            assert!(s.contains(&format!("CREATE ROLE {r} NOLOGIN NOINHERIT;")), "{s}");
        }
        assert_eq!(s.matches(" BYPASSRLS").count(), 1, "{s}");
        assert!(s.contains(SERVICE_ROLE_BYPASS_SQL), "{s}");
        assert!(!s.to_ascii_uppercase().contains(" LOGIN "), "{s}");
    }

    #[test]
    fn the_strip_keeps_only_k2s_members() {
        let s = standard_roles_strip_sql(&["ws_a_authenticator".into()], &["ws_a_migrator".into()]).unwrap();
        assert!(s.contains("authn text[] := ARRAY['ws_a_authenticator']::text[];"), "{s}");
        assert!(s.contains("migr text[] := ARRAY['ws_a_migrator']::text[];"), "{s}");
        assert!(s.contains("AND pg_catalog.pg_get_userbyid(m.roleid) <> 'service_role'"), "a migrator never keeps service_role: {s}");
        assert!(s.contains("OR pg_catalog.pg_get_userbyid(m.member) IN ('anon', 'authenticated', 'service_role')"), "what the roles are members of goes too: {s}");
        assert!(s.contains("REVOKE %I FROM %I GRANTED BY %I"), "{s}");
        assert!(s.trim_end().ends_with("FROM pg_temp.k2_stripped;"), "{s}");
        assert!(standard_roles_strip_sql(&[], &[]).unwrap().contains("ARRAY[]::text[]"));
        assert!(standard_roles_strip_sql(&["x'; DROP".into()], &[]).is_none());
    }

    #[test]
    fn the_authenticator_name_fits_postgres() {
        assert_eq!(authenticator_role("ws_jobs").as_deref(), Some("ws_jobs_authenticator"));
        let long = "w".repeat(60);
        let a = authenticator_role(&long).unwrap();
        assert!(a.len() <= 63 && a.ends_with("_authn") && a.starts_with("www"), "{a}");
        assert_eq!(authenticator_role(&long), authenticator_role(&long), "stable");
        assert_ne!(authenticator_role(&format!("{long}x")), authenticator_role(&long));
        assert!(authenticator_role("Bad-Name").is_none());
    }

    #[test]
    fn the_authenticator_is_noinherit_and_walls_the_database() {
        let s = authenticator_sql("ws_jobs", "ws_jobs_authenticator", "p'w", 40).unwrap();
        assert!(s.contains("CREATE ROLE \"ws_jobs_authenticator\" LOGIN PASSWORD 'p''w' NOINHERIT NOSUPERUSER"), "{s}");
        assert!(s.contains("NOBYPASSRLS CONNECTION LIMIT 40"), "{s}");
        assert!(s.contains("GRANT anon, authenticated, service_role TO \"ws_jobs_authenticator\";"), "{s}");
        assert!(s.contains("GRANT anon, authenticated TO \"ws_jobs_migrator\";"), "{s}");
        assert!(!s.contains("service_role TO \"ws_jobs_migrator\""), "{s}");
        let revoke = s.find("REVOKE CONNECT ON DATABASE \"ws_jobs\" FROM PUBLIC;").unwrap();
        let back = s.find("GRANT CONNECT ON DATABASE \"ws_jobs\" TO \"ws_jobs_migrator\", \"ws_jobs_agent\", \"ws_jobs_authenticator\";").unwrap();
        assert!(revoke < back, "{s}");
        assert!(authenticator_sql("ws_jobs", "x\"; DROP", "p", 1).is_none());
    }

    #[test]
    fn the_auth_schema_reads_the_setting_only_on_the_doors_login() {
        let s = auth_schema_sql("ws_jobs", "ws_jobs_authenticator").unwrap();
        assert!(s.contains("IF session_user::text = 'ws_jobs_authenticator' THEN"), "{s}");
        assert_eq!(s.matches("c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)").count(), 2, "{s}");
        for f in ["auth.jwt()", "auth.uid()", "auth.role()", "auth.email()", "k2.claims()", "k2.set_claims(text)"] {
            assert!(s.contains(&format!("ALTER FUNCTION {f} OWNER TO \"ws_jobs_migrator\";")), "{f}: {s}");
        }
        assert!(s.contains("REVOKE ALL ON TABLE auth.users FROM PUBLIC, anon, authenticated, service_role;"), "{s}");
        assert!(s.contains("REVOKE ALL ON TABLE k2.claims_key FROM PUBLIC;"), "{s}");
        assert!(s.contains("encrypted_password text CHECK (encrypted_password IS NULL)"), "{s}");
        assert!(AUTH_SCHEMA_OWNER_QUERY.contains(AUTH_SCHEMA_MARK));
        assert!(!s.contains("{{") && !s.contains("}}"), "format escapes: {s}");
        assert!(auth_schema_sql("ws_jobs", "bad name").is_none());
    }

    #[test]
    fn the_claims_key_is_hex_only() {
        assert!(claims_key_sql(&"ab".repeat(32)).unwrap().contains("decode('abab"));
        assert!(claims_key_sql("abcd").is_none());
        assert!(claims_key_sql(&format!("{}'); DROP", "ab".repeat(32))).is_none());
    }

    #[test]
    fn the_brokered_lock_quotes_its_values() {
        let s = claims_lock_sql(r#"{"sub":"a'b"}"#, Some("a'b"));
        assert!(s.contains(r#"VALUES ('{"sub":"a''b"}'::jsonb);"#), "{s}");
        assert!(s.contains("SET \"request.jwt.claim.sub\" TO 'a''b';"), "{s}");
        assert!(!s.contains("SELECT"), "no result rows before the caller's own: {s}");
        assert!(s.starts_with("CREATE TEMP TABLE pg_temp.k2_claims_lock"), "{s}");
    }
}
