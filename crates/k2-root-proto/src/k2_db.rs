//! K2's own helpers in every workspace database (schema `k2`): the
//! canned SQL `k2 db create` installs as postgres, shared by the daemon
//! and the root helper's restore (0.46.4 drill A). The restore never
//! replays K2's own objects from a dump; it installs these instead, as
//! postgres, before any tenant object exists in the target (no tenant DDL
//! ever runs as postgres), and only the ones the dump lists ([`is_k2_entry`]).

/// The principal helpers. `k2.skin_uid()` is owned by the migrator.
pub const PRINCIPAL_HELPERS_SQL: &str = r#"CREATE SCHEMA IF NOT EXISTS k2;
GRANT USAGE ON SCHEMA k2 TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.skin_uid() RETURNS uuid
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
GRANT EXECUTE ON FUNCTION k2.skin_uid() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.set_principal(p uuid) RETURNS void
LANGUAGE plpgsql
AS $k2fn$
BEGIN
  PERFORM set_config('k2.skin_principal', COALESCE(p::text, ''), true);
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.set_principal(uuid) TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.clear_principal() RETURNS void
LANGUAGE sql
AS $k2fn$
  SELECT set_config('k2.skin_principal', '', true)
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.clear_principal() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.principal_hygiene() RETURNS boolean
LANGUAGE sql
STABLE
AS $k2fn$
  SELECT coalesce(current_setting('k2.skin_principal', true), '') = ''
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.principal_hygiene() TO PUBLIC;
"#;

/// `k2.actor_info()` / `k2.actor_kind()` / `k2.actor()`. `k2.actor_info()`
/// is owned by the migrator.
pub const ACTOR_HELPERS_SQL: &str = r#"CREATE OR REPLACE FUNCTION k2.actor_info() RETURNS jsonb
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $k2fn$
DECLARE
  rel oid;
  info jsonb;
BEGIN
  BEGIN
    rel := 'pg_temp.k2_actor_lock'::regclass;
  EXCEPTION
    WHEN undefined_table OR invalid_schema_name THEN
      RETURN NULL;
  END;
  IF NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_class c
    WHERE c.oid = rel
      AND c.relpersistence = 't'
      AND c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)
  ) THEN
    RETURN NULL;
  END IF;
  SELECT pg_catalog.jsonb_build_object(
           'kind', l.kind,
           'actor', l.actor,
           'principal', l.principal,
           'pass', l.pass,
           'workspace', l.workspace,
           'on_behalf_of', l.on_behalf_of)
    INTO info
    FROM pg_temp.k2_actor_lock l
    LIMIT 1;
  RETURN info;
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor_info() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.actor_kind() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT k2.actor_info() ->> 'kind'
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor_kind() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.actor() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT k2.actor_info() ->> 'actor'
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor() TO PUBLIC;
"#;

/// The guard K2's internal-table policies call (the boot seal); owned by
/// the migrator.
pub const INTERNAL_TABLE_OK_SQL: &str = r#"CREATE OR REPLACE FUNCTION k2.internal_table_ok() RETURNS boolean
LANGUAGE plpgsql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
BEGIN
  IF k2.skin_uid() IS NOT NULL THEN
    RAISE EXCEPTION 'permission denied for K2 internal table' USING ERRCODE = 'insufficient_privilege';
  END IF;
  RETURN true;
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.internal_table_ok() TO PUBLIC;
"#;

/// K2's functions in schema `k2`, as `pg_restore -l` names them, with
/// whether the migrator owns them (else postgres).
pub const K2_FUNCTIONS: [(&str, bool); 8] = [
    ("skin_uid()", true),
    ("set_principal(uuid)", false),
    ("clear_principal()", false),
    ("principal_hygiene()", false),
    ("actor_info()", true),
    ("actor_kind()", false),
    ("actor()", false),
    ("internal_table_ok()", true),
];

fn all_sql() -> String {
    [PRINCIPAL_HELPERS_SQL, ACTOR_HELPERS_SQL, INTERNAL_TABLE_OK_SQL].concat()
}

/// One function's canned `CREATE OR REPLACE` + its `GRANT`.
pub fn function_sql(sig: &str) -> Option<String> {
    K2_FUNCTIONS.iter().find(|(s, _)| *s == sig)?;
    let name = sig.split('(').next()?;
    let all = all_sql();
    let head = format!("CREATE OR REPLACE FUNCTION k2.{name}(");
    let at = all.find(&head)?;
    let rest = &all[at..];
    let end = rest[head.len()..].find("CREATE OR REPLACE FUNCTION").map(|i| i + head.len()).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn owner_ok(owner: &str, migrator: &str) -> bool {
    owner == "postgres" || owner == migrator
}

/// Is this `pg_restore -l` line one of K2's own objects (the schema `k2`
/// itself, or one of [`K2_FUNCTIONS`], its ACL or comment), owned by
/// postgres or the database's migrator? Exact fields only. Those entries
/// are left out of the restore and installed from [`install_sql`].
/// Returns what it is: `"schema"` or the function's signature.
pub fn is_k2_entry(line: &str, migrator: &str) -> Option<String> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let id_ok = f.first().and_then(|x| x.strip_suffix(';')).is_some_and(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
    if !id_ok || f.len() < 7 {
        return None;
    }
    let sig_ok = |s: &str| K2_FUNCTIONS.iter().any(|(k, _)| *k == s);
    // `<id>; 2615 <oid> SCHEMA - k2 <owner>`
    if f.len() == 7 && f[1] == "2615" && f[3] == "SCHEMA" && f[4] == "-" && f[5] == "k2" && owner_ok(f[6], migrator) {
        return Some("schema".into());
    }
    // `<id>; 1255 <oid> FUNCTION k2 <sig> <owner>`
    if f.len() == 7 && f[1] == "1255" && f[3] == "FUNCTION" && f[4] == "k2" && sig_ok(f[5]) && owner_ok(f[6], migrator) {
        return Some(f[5].to_string());
    }
    // `<id>; 0 0 ACL|COMMENT k2 FUNCTION <sig> <owner>`
    if f.len() == 8 && f[1] == "0" && f[2] == "0" && (f[3] == "ACL" || f[3] == "COMMENT") && f[4] == "k2" && f[5] == "FUNCTION" && sig_ok(f[6]) && owner_ok(f[7], migrator) {
        return Some(format!("{} {}", f[3], f[6]));
    }
    None
}

/// The install the restore runs as postgres for what [`is_k2_entry`] left
/// out: the schema (owned by the migrator, its ACL comes from the dump as
/// the owner), each listed function (canned; owners as `k2 db create`
/// sets them). `None`: the migrator name isn't a role name.
pub fn install_sql(schema: bool, sigs: &[String], migrator: &str) -> Option<String> {
    if !crate::pg_major::role_name_ok(migrator) {
        return None;
    }
    let mut out = String::from("SET check_function_bodies = off;\n");
    if schema {
        out.push_str("CREATE SCHEMA k2;\n");
    }
    for (sig, _) in K2_FUNCTIONS.iter().filter(|(s, _)| sigs.iter().any(|x| x == s)) {
        out.push_str(&function_sql(sig)?);
    }
    for (sig, mig) in K2_FUNCTIONS.iter().filter(|(s, m)| *m && sigs.iter().any(|x| x == s)) {
        let _ = mig;
        out.push_str(&format!("ALTER FUNCTION k2.{sig} OWNER TO \"{migrator}\";\n"));
    }
    if schema {
        out.push_str(&format!("ALTER SCHEMA k2 OWNER TO \"{migrator}\";\n"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_k2_function_has_its_canned_sql() {
        for (sig, _) in K2_FUNCTIONS {
            let s = function_sql(sig).unwrap_or_else(|| panic!("{sig}"));
            let name = sig.split('(').next().unwrap();
            assert!(s.starts_with(&format!("CREATE OR REPLACE FUNCTION k2.{name}(")), "{s}");
            assert_eq!(s.matches("CREATE OR REPLACE FUNCTION").count(), 1, "{s}");
            assert!(s.contains(&format!("GRANT EXECUTE ON FUNCTION k2.{name}(")), "{s}");
        }
        assert!(function_sql("evil()").is_none());
    }

    #[test]
    fn k2_entries_are_matched_exactly() {
        let mig = "ws_jobs_migrator";
        assert_eq!(is_k2_entry("7; 2615 16500 SCHEMA - k2 ws_jobs_migrator", mig).as_deref(), Some("schema"));
        assert_eq!(is_k2_entry("7; 2615 16500 SCHEMA - k2 postgres", mig).as_deref(), Some("schema"));
        assert_eq!(is_k2_entry("9; 1255 16510 FUNCTION k2 actor() postgres", mig).as_deref(), Some("actor()"));
        assert_eq!(is_k2_entry("9; 1255 16511 FUNCTION k2 set_principal(uuid) postgres", mig).as_deref(), Some("set_principal(uuid)"));
        assert_eq!(is_k2_entry("10; 0 0 ACL k2 FUNCTION skin_uid() ws_jobs_migrator", mig).as_deref(), Some("ACL skin_uid()"));
        for not in [
            "9; 1255 16510 FUNCTION k2 whoami() ws_jobs_migrator",
            "9; 1255 16510 FUNCTION public actor() postgres",
            "9; 1255 16510 FUNCTION k2 actor() someone_else",
            "9; 1255 16510 FUNCTION k2 actor() postgres extra",
            "7; 2615 16500 SCHEMA - k2x ws_jobs_migrator",
            "7; 2615 16500 SCHEMA - evil ws_jobs_migrator",
            "10; 0 0 ACL - SCHEMA k2 ws_jobs_migrator",
            "x; 1255 16510 FUNCTION k2 actor() postgres",
            "11; 1259 16520 TABLE k2 actor() postgres",
        ] {
            assert_eq!(is_k2_entry(not, mig), None, "{not}");
        }
    }

    #[test]
    fn the_install_is_canned_and_sets_k2s_owners() {
        let sigs: Vec<String> = ["actor()", "skin_uid()", "actor_info()"].iter().map(|s| s.to_string()).collect();
        let s = install_sql(true, &sigs, "ws_jobs_migrator").unwrap();
        assert!(s.starts_with("SET check_function_bodies = off;\nCREATE SCHEMA k2;\n"), "{s}");
        assert!(s.find("FUNCTION k2.skin_uid(").unwrap() < s.find("FUNCTION k2.actor_info(").unwrap());
        assert!(s.find("FUNCTION k2.actor_info(").unwrap() < s.find("FUNCTION k2.actor() RETURNS").unwrap(), "canned order");
        assert!(s.contains("ALTER FUNCTION k2.skin_uid() OWNER TO \"ws_jobs_migrator\";"));
        assert!(s.contains("ALTER FUNCTION k2.actor_info() OWNER TO \"ws_jobs_migrator\";"));
        assert!(!s.contains("ALTER FUNCTION k2.actor() OWNER"), "postgres keeps it");
        assert!(!s.contains("set_principal"), "only what the dump lists");
        assert!(s.trim_end().ends_with("ALTER SCHEMA k2 OWNER TO \"ws_jobs_migrator\";"));
        assert!(install_sql(true, &sigs, "x\"; DROP").is_none());
        assert_eq!(install_sql(false, &[], "ws_jobs_migrator").unwrap(), "SET check_function_bodies = off;\n");
    }
}
