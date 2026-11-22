//! **Stand-ins for other lanes' canned SQL**, written against the PRD's
//! contract until those lanes land (prd-app-supabase-compat-v1 §5.1, §5.2,
//! §5.4, §10). The import needs them before it restores anything: policies
//! name `anon`/`authenticated`, foreign keys point at `auth.users(id)`, and
//! Storage's policies sit on `storage.objects`.
//!
//! - [`roles_sql`]: lane A's three cluster roles (§5.1), exactly as specified:
//!   `anon` and `authenticated` NOLOGIN, `service_role` NOLOGIN BYPASSRLS.
//!   Idempotent; a role of that name with other attributes is refused.
//! - [`auth_sql`]: lane A's `auth` schema (§5.4): `auth.users` as a table
//!   (owned by postgres, never holding a password), Supabase's `auth.uid()`,
//!   `role()`, `email()` and `jwt()` reading `request.jwt.claims` (§5.2; lane
//!   A adds the brokered lane's lock and the owner check).
//! - [`storage_sql`]: lane D's `storage.buckets` / `storage.objects` with
//!   Supabase's columns, the three path helpers and the `updated_at`
//!   trigger (§10), copied verbatim from lane D's branch.
//!
//! Every statement is `IF NOT EXISTS` / `CREATE OR REPLACE`, so the lanes'
//! own installs can run over them. When lane A and lane D land, these
//! bodies are replaced by calls to their installs; the import's steps don't
//! change.

/// The three standard roles, as postgres (on the `postgres` database), and
/// the check that prints their attributes (`name|login|super|bypassrls`).
pub fn roles_sql() -> String {
    "DO $k2roles$\nBEGIN\n\
       IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'anon') THEN CREATE ROLE anon NOLOGIN NOINHERIT; END IF;\n\
       IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'authenticated') THEN CREATE ROLE authenticated NOLOGIN NOINHERIT; END IF;\n\
       IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'service_role') THEN CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS; END IF;\n\
     END\n$k2roles$;\n"
        .to_string()
}

/// The attribute check after [`roles_sql`].
pub const ROLES_CHECK: &str = "SELECT rolname || '|' || rolcanlogin || '|' || rolsuper || '|' || rolbypassrls FROM pg_catalog.pg_roles \
     WHERE rolname IN ('anon', 'authenticated', 'service_role') ORDER BY rolname;\n";

/// `Ok` when the check's answer is the three roles as specified.
pub fn roles_ok(answer: &str) -> Result<(), String> {
    let want = ["anon|false|false|false", "authenticated|false|false|false", "service_role|false|false|true"];
    let got: Vec<&str> = answer.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if got == want {
        return Ok(());
    }
    Err(format!(
        "roles_conflict: this cluster's anon/authenticated/service_role aren't the standard ones (want NOLOGIN, only service_role BYPASSRLS; found {}). \
         the import won't run over them: use a cluster of the App's own ({} db cluster create)",
        got.join(", "),
        k2_core::brand::CLI_NAME
    ))
}

fn q(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// `auth`, as postgres in the target database. `migrator` and `agent` are
/// the database's own roles: the migrator may reference `auth.users` (the
/// App's foreign keys), put triggers on it (a profile row at sign-up) and
/// read it (the App's SECURITY DEFINER helpers), never write it.
pub fn auth_sql(migrator: &str, agent: &str) -> String {
    let (m, a) = (q(migrator), q(agent));
    format!(
        "CREATE SCHEMA IF NOT EXISTS auth;\n\
         CREATE TABLE IF NOT EXISTS auth.users (\n\
           instance_id uuid,\n\
           id uuid NOT NULL PRIMARY KEY,\n\
           aud text,\n\
           role text,\n\
           email text,\n\
           encrypted_password text CHECK (encrypted_password IS NULL),\n\
           email_confirmed_at timestamptz,\n\
           last_sign_in_at timestamptz,\n\
           raw_app_meta_data jsonb,\n\
           raw_user_meta_data jsonb,\n\
           created_at timestamptz,\n\
           updated_at timestamptz,\n\
           banned_until timestamptz,\n\
           phone text,\n\
           deleted_at timestamptz,\n\
           is_anonymous boolean NOT NULL DEFAULT false\n\
         );\n\
         CREATE OR REPLACE FUNCTION auth.jwt() RETURNS jsonb LANGUAGE sql STABLE AS $f$\n\
           SELECT coalesce(nullif(pg_catalog.current_setting('request.jwt.claim', true), ''), nullif(pg_catalog.current_setting('request.jwt.claims', true), ''))::jsonb\n\
         $f$;\n\
         CREATE OR REPLACE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql STABLE AS $f$\n\
           SELECT coalesce(nullif(pg_catalog.current_setting('request.jwt.claim.sub', true), ''), (nullif(pg_catalog.current_setting('request.jwt.claims', true), '')::jsonb ->> 'sub'))::uuid\n\
         $f$;\n\
         CREATE OR REPLACE FUNCTION auth.role() RETURNS text LANGUAGE sql STABLE AS $f$\n\
           SELECT coalesce(nullif(pg_catalog.current_setting('request.jwt.claim.role', true), ''), (nullif(pg_catalog.current_setting('request.jwt.claims', true), '')::jsonb ->> 'role'))::text\n\
         $f$;\n\
         CREATE OR REPLACE FUNCTION auth.email() RETURNS text LANGUAGE sql STABLE AS $f$\n\
           SELECT coalesce(nullif(pg_catalog.current_setting('request.jwt.claim.email', true), ''), (nullif(pg_catalog.current_setting('request.jwt.claims', true), '')::jsonb ->> 'email'))::text\n\
         $f$;\n\
         GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role, {m}, {a};\n\
         GRANT SELECT, REFERENCES, TRIGGER ON auth.users TO {m};\n"
    )
}

/// The projection columns K2 fills from the dump's `auth.users` (never the
/// password).
pub const AUTH_USER_COLUMNS: &[&str] = &[
    "instance_id",
    "id",
    "aud",
    "role",
    "email",
    "email_confirmed_at",
    "last_sign_in_at",
    "raw_app_meta_data",
    "raw_user_meta_data",
    "created_at",
    "updated_at",
    "banned_until",
    "phone",
    "deleted_at",
    "is_anonymous",
];

/// `storage`, as the migrator in the target database (it owns the tables,
/// so the App's Storage policies restore onto them). **A verbatim copy of
/// lane D's `apps/compat/storage_sql.rs::SCHEMA` (supa-lane-d at 9c6e5406c9);
/// lane D's file wins at merge**, and this becomes a call to it.
pub fn storage_sql() -> String {
    STORAGE_SCHEMA.to_string()
}

const STORAGE_SCHEMA: &str = r#"
CREATE SCHEMA IF NOT EXISTS storage;

CREATE TABLE IF NOT EXISTS storage.buckets (
    id text NOT NULL PRIMARY KEY,
    name text NOT NULL,
    owner uuid,
    owner_id text,
    public boolean DEFAULT false,
    avif_autodetection boolean DEFAULT false,
    file_size_limit bigint,
    allowed_mime_types text[],
    type text NOT NULL DEFAULT 'STANDARD',
    created_at timestamptz DEFAULT now(),
    updated_at timestamptz DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS bname ON storage.buckets USING btree (name);

CREATE TABLE IF NOT EXISTS storage.objects (
    id uuid NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    bucket_id text REFERENCES storage.buckets (id),
    name text,
    owner uuid,
    owner_id text,
    created_at timestamptz DEFAULT now(),
    updated_at timestamptz DEFAULT now(),
    last_accessed_at timestamptz DEFAULT now(),
    metadata jsonb,
    path_tokens text[] GENERATED ALWAYS AS (string_to_array(name, '/')) STORED,
    version text,
    user_metadata jsonb
);
CREATE UNIQUE INDEX IF NOT EXISTS bucketid_objname ON storage.objects USING btree (bucket_id, name);
CREATE INDEX IF NOT EXISTS name_prefix_search ON storage.objects USING btree (name text_pattern_ops);

ALTER TABLE storage.buckets ENABLE ROW LEVEL SECURITY;
ALTER TABLE storage.objects ENABLE ROW LEVEL SECURITY;

-- 'a/b/c.png' -> {a,b}
CREATE OR REPLACE FUNCTION storage.foldername(name text) RETURNS text[]
LANGUAGE sql IMMUTABLE AS $f$
    SELECT (string_to_array(name, '/'))[1:array_length(string_to_array(name, '/'), 1) - 1]
$f$;

-- 'a/b/c.png' -> 'c.png'
CREATE OR REPLACE FUNCTION storage.filename(name text) RETURNS text
LANGUAGE sql IMMUTABLE AS $f$
    SELECT (string_to_array(name, '/'))[array_length(string_to_array(name, '/'), 1)]
$f$;

-- 'a/b/c.png' -> 'png'
CREATE OR REPLACE FUNCTION storage.extension(name text) RETURNS text
LANGUAGE sql IMMUTABLE AS $f$
    SELECT reverse(split_part(reverse(storage.filename(name)), '.', 1))
$f$;

CREATE OR REPLACE FUNCTION storage.update_updated_at_column() RETURNS trigger
LANGUAGE plpgsql AS $f$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END
$f$;

DROP TRIGGER IF EXISTS update_objects_updated_at ON storage.objects;
CREATE TRIGGER update_objects_updated_at BEFORE UPDATE ON storage.objects
    FOR EACH ROW EXECUTE FUNCTION storage.update_updated_at_column();

GRANT USAGE ON SCHEMA storage TO anon, authenticated, service_role;
GRANT ALL ON TABLE storage.buckets, storage.objects TO anon, authenticated, service_role;
GRANT EXECUTE ON FUNCTION storage.foldername(text), storage.filename(text), storage.extension(text)
    TO anon, authenticated, service_role;
"#;

/// The columns of `storage.buckets` / `storage.objects` K2 fills from the
/// dump (`path_tokens` is generated).
pub const BUCKET_COLUMNS: &[&str] = &["id", "name", "owner", "owner_id", "created_at", "updated_at", "public", "avif_autodetection", "file_size_limit", "allowed_mime_types", "type"];
pub const OBJECT_COLUMNS: &[&str] = &["id", "bucket_id", "name", "owner", "owner_id", "created_at", "updated_at", "last_accessed_at", "metadata", "version", "user_metadata"];
