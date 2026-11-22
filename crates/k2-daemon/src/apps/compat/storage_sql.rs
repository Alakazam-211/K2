//! The `storage` schema an App's database gets (PRD §10 "Tables").
//!
//! Supabase's columns and names, so the App's own storage policies load
//! unchanged (`CREATE POLICY … ON storage.objects … USING
//! ((storage.foldername(name))[1] = …)`). Owned by the database's
//! migrator (the App's migrations write policies on these tables, as
//! `postgres` does on Supabase). Row-level security is on for both tables
//! and `anon`, `authenticated` and `service_role` hold the table rights,
//! exactly as on Supabase: with no policy only `service_role`
//! (BYPASSRLS) sees a row.
//!
//! Written for this lane, not copied: the single-version shape (one row
//! per `(bucket_id, name)`), which is what the reference app and the v1
//! verbs use. `version` names the bytes of the current write; the door
//! keeps the bytes under the object's id and that version, never its name
//! (`storage_bytes`).
//!
//! Idempotent: every statement can run again on a database that has it.

/// The schema. Needs the roles `anon`, `authenticated` and `service_role`
/// (lane A makes them per cluster when compat is turned on).
pub const SCHEMA: &str = r#"
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

/// The roles the schema grants to; a database without them is refused
/// before anything runs (they are lane A's, made when compat is turned on).
pub const ROLES: &[&str] = &["anon", "authenticated", "service_role"];

/// SQL that names the roles missing from the cluster, one per row.
pub const MISSING_ROLES_SQL: &str =
    "SELECT r FROM unnest(ARRAY['anon','authenticated','service_role']) AS r WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_idempotent_by_construction() {
        for line in SCHEMA.lines().map(str::trim) {
            let up = line.to_ascii_uppercase();
            if up.starts_with("CREATE TABLE") || up.starts_with("CREATE SCHEMA") || up.starts_with("CREATE UNIQUE INDEX") || up.starts_with("CREATE INDEX") {
                assert!(up.contains("IF NOT EXISTS"), "not idempotent: {line}");
            }
            if up.starts_with("CREATE FUNCTION") {
                panic!("functions must be CREATE OR REPLACE: {line}");
            }
            if up.starts_with("CREATE TRIGGER") {
                assert!(SCHEMA.contains("DROP TRIGGER IF EXISTS update_objects_updated_at"), "{line}");
            }
        }
    }

    #[test]
    fn supabase_columns_are_there() {
        for col in ["bucket_id text", "name text", "owner uuid", "owner_id text", "metadata jsonb", "path_tokens text[]", "version text", "user_metadata jsonb", "last_accessed_at timestamptz"] {
            assert!(SCHEMA.contains(col), "storage.objects lacks {col}");
        }
        for col in ["public boolean", "file_size_limit bigint", "allowed_mime_types text[]"] {
            assert!(SCHEMA.contains(col), "storage.buckets lacks {col}");
        }
        assert!(!SCHEMA.contains("FORCE ROW LEVEL"), "K2's migration gate refuses FORCE RLS");
    }
}
