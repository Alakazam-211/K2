//! The SQL a move runs against the clusters (as postgres, through the
//! helper's `psql --cluster`), and the pure parsers and comparisons on
//! its answers. Every name that reaches SQL text is a validated role or
//! database name, or comes back out of the catalog through
//! `quote_ident`/`format('%I')` server-side.

use serde_json::Value;

/// User schemas: not the catalogs, not TOAST, not temp.
const USER_NS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn quote_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn role_array(roles: &[String]) -> String {
    if roles.is_empty() {
        return "ARRAY[]::text[]".into();
    }
    let v: Vec<String> = roles.iter().map(|r| quote_lit(r)).collect();
    format!("ARRAY[{}]::text[]", v.join(", "))
}

/// One JSON object of the facts `--check` needs, read in the database.
pub fn facts_sql(db: &str) -> String {
    let d = quote_lit(db);
    format!(
        "SELECT json_build_object(\
         'size', pg_database_size(current_database()), \
         'extensions', (SELECT coalesce(json_agg(extname ORDER BY extname), '[]'::json) FROM pg_extension), \
         'eventTriggers', (SELECT count(*) FROM pg_event_trigger), \
         'publications', (SELECT count(*) FROM pg_publication), \
         'subscriptions', (SELECT count(*) FROM pg_subscription s JOIN pg_database d ON d.oid = s.subdbid WHERE d.datname = current_database()), \
         'tablespaces', (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.reltablespace <> 0 AND {USER_NS}) \
            + (SELECT count(*) FROM pg_database d JOIN pg_tablespace t ON t.oid = d.dattablespace WHERE d.datname = current_database() AND t.spcname <> 'pg_default'), \
         'largeObjects', (SELECT count(*) FROM pg_largeobject_metadata), \
         'dbSettings', (SELECT count(*) FROM pg_db_role_setting s JOIN pg_database d ON d.oid = s.setdatabase WHERE d.datname = current_database()), \
         'sessions', (SELECT count(*) FROM pg_stat_activity WHERE datname = {d} AND pid <> pg_backend_pid()), \
         'connlimit', (SELECT datconnlimit FROM pg_database WHERE datname = current_database()), \
         'allowConn', (SELECT datallowconn FROM pg_database WHERE datname = current_database()), \
         'unloggedTables', (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relpersistence = 'u' AND {USER_NS})\
         );\n"
    )
}

/// Every role a single-database dump names but doesn't carry: the owner,
/// every grantee of the database / its relations / schemas / functions /
/// default privileges, every object owner, every policy role, plus K2's
/// own roles for the database, closed over their members (the logins
/// that use them). Never `postgres`, `pg_*` or PUBLIC.
pub fn role_set_sql(k2_roles: &[String]) -> String {
    format!(
        "WITH RECURSIVE base(oid) AS (\
           SELECT datdba FROM pg_database WHERE datname = current_database() \
           UNION SELECT (aclexplode(datacl)).grantee FROM pg_database WHERE datname = current_database() AND datacl IS NOT NULL \
           UNION SELECT c.relowner FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE {USER_NS} \
           UNION SELECT (aclexplode(c.relacl)).grantee FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE {USER_NS} AND c.relacl IS NOT NULL \
           UNION SELECT n.nspowner FROM pg_namespace n WHERE {USER_NS} \
           UNION SELECT (aclexplode(n.nspacl)).grantee FROM pg_namespace n WHERE {USER_NS} AND n.nspacl IS NOT NULL \
           UNION SELECT p.proowner FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE {USER_NS} \
           UNION SELECT (aclexplode(p.proacl)).grantee FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE {USER_NS} AND p.proacl IS NOT NULL \
           UNION SELECT t.typowner FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace WHERE {USER_NS} \
           UNION SELECT defaclrole FROM pg_default_acl \
           UNION SELECT (aclexplode(defaclacl)).grantee FROM pg_default_acl \
           UNION SELECT unnest(polroles) FROM pg_policy \
           UNION SELECT oid FROM pg_roles WHERE rolname = ANY({}) \
         ), closure(oid) AS (\
           SELECT oid FROM base \
           UNION SELECT am.member FROM pg_auth_members am JOIN closure c ON am.roleid = c.oid \
         ) \
         SELECT coalesce(json_agg(DISTINCT r.rolname ORDER BY r.rolname), '[]'::json) \
         FROM closure c JOIN pg_roles r ON r.oid = c.oid \
         WHERE r.rolname <> 'postgres' AND r.rolname NOT LIKE 'pg\\_%';\n",
        role_array(k2_roles)
    )
}

/// Memberships among `roles` (PG 16+ grant options).
pub fn memberships_sql(roles: &[String]) -> String {
    let a = role_array(roles);
    format!(
        "SELECT coalesce(json_agg(json_build_array(r.rolname, m.rolname, am.admin_option, am.inherit_option, am.set_option) \
         ORDER BY r.rolname, m.rolname), '[]'::json) \
         FROM pg_auth_members am JOIN pg_roles r ON r.oid = am.roleid JOIN pg_roles m ON m.oid = am.member \
         WHERE r.rolname = ANY({a}) AND m.rolname = ANY({a});\n"
    )
}

/// `GRANT r TO m WITH ADMIN x, INHERIT y, SET z;` for each row of
/// [`memberships_sql`]. `None` when the answer isn't the expected shape.
pub fn membership_grants(answer: &str, ok_name: impl Fn(&str) -> bool) -> Option<String> {
    let rows: Vec<(String, String, bool, bool, bool)> = serde_json::from_str(answer).ok()?;
    let mut s = String::new();
    for (r, m, admin, inherit, set) in rows {
        if !ok_name(&r) || !ok_name(&m) {
            return None;
        }
        s.push_str(&format!(
            "GRANT {} TO {} WITH ADMIN {}, INHERIT {}, SET {};\n",
            quote_ident(&r),
            quote_ident(&m),
            admin,
            inherit,
            set
        ));
    }
    Some(s)
}

/// MD5-hashed passwords among `roles` (names only; a warning).
pub fn md5_roles_sql(roles: &[String]) -> String {
    format!(
        "SELECT coalesce(json_agg(rolname ORDER BY rolname), '[]'::json) FROM pg_authid \
         WHERE rolname = ANY({}) AND rolpassword LIKE 'md5%';\n",
        role_array(roles)
    )
}

/// The database's own properties (owner, encoding, locale) as JSON. Read
/// through `to_jsonb` so 16's `daticulocale` and 17's `datlocale` both
/// come back.
pub fn db_props_sql() -> &'static str {
    "SELECT (to_jsonb(d) - 'datacl') || jsonb_build_object('enc', pg_encoding_to_char(d.encoding), 'owner', pg_get_userbyid(d.datdba)) \
     FROM pg_database d WHERE d.datname = current_database();\n"
}

fn locale_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@'))
}

/// `CREATE DATABASE` on the target with the source's owner, encoding and
/// locale. `None` when the answer carries something K2 won't put in SQL.
pub fn create_db_sql(db: &str, props: &Value, ok_name: impl Fn(&str) -> bool) -> Option<String> {
    let owner = props["owner"].as_str()?;
    let enc = props["enc"].as_str()?;
    let collate = props["datcollate"].as_str()?;
    let ctype = props["datctype"].as_str()?;
    if !ok_name(owner) || !locale_ok(enc) || !locale_ok(collate) || !locale_ok(ctype) {
        return None;
    }
    let mut s = format!(
        "CREATE DATABASE {} OWNER {} TEMPLATE template0 ENCODING {} LC_COLLATE {} LC_CTYPE {}",
        quote_ident(db),
        quote_ident(owner),
        quote_lit(enc),
        quote_lit(collate),
        quote_lit(ctype)
    );
    match props["datlocprovider"].as_str() {
        Some("c") | None => {}
        Some("i") => {
            let icu = props["daticulocale"].as_str().or_else(|| props["datlocale"].as_str())?;
            if !locale_ok(icu) {
                return None;
            }
            s.push_str(&format!(" LOCALE_PROVIDER icu ICU_LOCALE {}", quote_lit(icu)));
        }
        Some(_) => return None,
    }
    s.push_str(";\n");
    Some(s)
}

/// The database ACL as (grantee or `PUBLIC`, privilege, grantable), and
/// whether it is NULL (the defaults).
pub fn datacl_sql() -> &'static str {
    "SELECT json_build_object('isNull', d.datacl IS NULL, 'items', \
       (SELECT coalesce(json_agg(json_build_array(CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END, \
          a.privilege_type, a.is_grantable) ORDER BY 1), '[]'::json) FROM aclexplode(d.datacl) a)) \
     FROM pg_database d WHERE d.datname = current_database();\n"
}

/// The `GRANT … ON DATABASE` replay for [`datacl_sql`]'s answer (a dump
/// without `--create` doesn't carry it, V33).
pub fn datacl_replay(db: &str, answer: &str, ok_name: impl Fn(&str) -> bool) -> Option<String> {
    let v: Value = serde_json::from_str(answer).ok()?;
    if v["isNull"].as_bool()? {
        return Some(String::new());
    }
    let mut s = format!("REVOKE ALL ON DATABASE {} FROM PUBLIC;\n", quote_ident(db));
    for it in v["items"].as_array()? {
        let who = it[0].as_str()?;
        let privilege = it[1].as_str()?;
        let grantable = it[2].as_bool()?;
        if !matches!(privilege, "CONNECT" | "CREATE" | "TEMPORARY") {
            return None;
        }
        let to = if who == "PUBLIC" {
            "PUBLIC".to_string()
        } else if ok_name(who) || who == "postgres" {
            quote_ident(who)
        } else {
            return None;
        };
        s.push_str(&format!(
            "GRANT {privilege} ON DATABASE {} TO {to}{};\n",
            quote_ident(db),
            if grantable { " WITH GRANT OPTION" } else { "" }
        ));
    }
    Some(s)
}

/// `ALTER DATABASE/ROLE … SET` rows tied to this database: (role or "",
/// `name=value`).
pub fn db_settings_sql() -> &'static str {
    "SELECT coalesce(json_agg(json_build_array(coalesce(pg_get_userbyid(nullif(s.setrole, 0)), ''), cfg)), '[]'::json) \
     FROM pg_db_role_setting s, unnest(s.setconfig) cfg \
     WHERE s.setdatabase = (SELECT oid FROM pg_database WHERE datname = current_database());\n"
}

fn guc_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 63 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

pub fn db_settings_replay(db: &str, answer: &str, ok_name: impl Fn(&str) -> bool) -> Option<String> {
    let rows: Vec<(String, String)> = serde_json::from_str(answer).ok()?;
    let mut s = String::new();
    for (role, cfg) in rows {
        let (name, value) = cfg.split_once('=')?;
        if !guc_name_ok(name) {
            return None;
        }
        if role.is_empty() {
            s.push_str(&format!("ALTER DATABASE {} SET {name} = {};\n", quote_ident(db), quote_lit(value)));
        } else if ok_name(&role) {
            s.push_str(&format!(
                "ALTER ROLE {} IN DATABASE {} SET {name} = {};\n",
                quote_ident(&role),
                quote_ident(db),
                quote_lit(value)
            ));
        } else {
            return None;
        }
    }
    Some(s)
}

/// Step one of the row count: the `SELECT … UNION ALL …` text, built
/// server-side with `format('%I')`.
pub fn count_query_sql() -> String {
    format!(
        "SELECT coalesce(string_agg(format('SELECT %L AS t, count(*) AS n FROM %I.%I', n.nspname || '.' || c.relname, n.nspname, c.relname), \
         ' UNION ALL ' ORDER BY n.nspname, c.relname), '') \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'r' AND {USER_NS};\n"
    )
}

/// Step two: wrap step one's text so the answer is one JSON object.
pub fn count_wrap(union: &str) -> Option<String> {
    if union.trim().is_empty() {
        return Some("SELECT '{}'::json;\n".into());
    }
    if union.contains(';') {
        return None;
    }
    Some(format!("SELECT coalesce(json_object_agg(t, n), '{{}}'::json) FROM ({union}) s;\n"))
}

/// The structure a move must carry, as one JSON object (§6.2 step 7):
/// extensions, RLS flags, policies, relation / schema / function /
/// default ACLs and owners, sequences, the database ACL and owner.
pub fn fingerprint_sql() -> String {
    format!(
        "SELECT json_build_object(\
         'extensions', (SELECT coalesce(json_agg(extname ORDER BY extname), '[]'::json) FROM pg_extension), \
         'rls', (SELECT coalesce(json_object_agg(n.nspname || '.' || c.relname, json_build_array(c.relrowsecurity, c.relforcerowsecurity)), '{{}}'::json) \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r', 'p') AND {USER_NS}), \
         'policies', (SELECT coalesce(json_object_agg(schemaname || '.' || tablename || '.' || policyname, \
                 json_build_array(permissive, roles::text, cmd, qual, with_check)), '{{}}'::json) FROM pg_policies), \
         'relations', (SELECT coalesce(json_object_agg(n.nspname || '.' || c.relname, json_build_array(c.relkind, pg_get_userbyid(c.relowner), \
                 ARRAY(SELECT x::text FROM unnest(c.relacl) x ORDER BY 1))), '{{}}'::json) \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f') AND {USER_NS} \
                 AND NOT EXISTS (SELECT 1 FROM pg_depend dp WHERE dp.classid = 'pg_class'::regclass AND dp.objid = c.oid AND dp.deptype = 'e')), \
         'schemas', (SELECT coalesce(json_object_agg(n.nspname, json_build_array(pg_get_userbyid(n.nspowner), \
                 ARRAY(SELECT x::text FROM unnest(n.nspacl) x ORDER BY 1))), '{{}}'::json) FROM pg_namespace n WHERE {USER_NS}), \
         'functions', (SELECT coalesce(json_object_agg(p.oid::regprocedure::text, json_build_array(pg_get_userbyid(p.proowner), p.prosecdef, \
                 ARRAY(SELECT x::text FROM unnest(p.proacl) x ORDER BY 1))), '{{}}'::json) \
                 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE {USER_NS} \
                 AND NOT EXISTS (SELECT 1 FROM pg_depend dp WHERE dp.classid = 'pg_proc'::regclass AND dp.objid = p.oid AND dp.deptype = 'e')), \
         'defaultAcls', (SELECT coalesce(json_object_agg(pg_get_userbyid(defaclrole) || ':' || coalesce(defaclnamespace::regnamespace::text, '') || ':' || defaclobjtype::text, \
                 ARRAY(SELECT x::text FROM unnest(defaclacl) x ORDER BY 1)), '{{}}'::json) FROM pg_default_acl), \
         'sequences', (SELECT coalesce(json_object_agg(schemaname || '.' || sequencename, last_value), '{{}}'::json) FROM pg_sequences), \
         'database', (SELECT json_build_array(pg_get_userbyid(datdba), ARRAY(SELECT x::text FROM unnest(datacl) x ORDER BY 1)) \
                 FROM pg_database WHERE datname = current_database())\
         );\n"
    )
}

/// What differs between two fingerprints (and row counts): one line per
/// differing key, `section key: source → target`. Empty = equal.
pub fn diff(src: &Value, dst: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let empty = serde_json::Map::new();
    let sections: Vec<&String> = src.as_object().map(|o| o.keys().collect()).unwrap_or_default();
    for sec in sections {
        let (a, b) = (&src[sec], &dst[sec]);
        match (a.as_object(), b.as_object()) {
            (Some(ao), Some(_)) | (Some(ao), None) => {
                let bo = b.as_object().unwrap_or(&empty);
                for (k, av) in ao {
                    match bo.get(k) {
                        Some(bv) if bv == av => {}
                        Some(bv) => out.push(format!("{sec} {k}: {av} → {bv}")),
                        None => out.push(format!("{sec} {k}: {av} → missing")),
                    }
                }
                for k in bo.keys().filter(|k| !ao.contains_key(*k)) {
                    out.push(format!("{sec} {k}: missing → {}", bo[k]));
                }
            }
            _ if a == b => {}
            _ => out.push(format!("{sec}: {a} → {b}")),
        }
    }
    out
}

/// Privilege letters a major added (aclitem `grantee=privs/grantor`): a
/// relation owner on 17 holds MAINTAIN (`m`), which 16 has no letter for.
/// Stripped from both fingerprints before comparing a move from an older
/// major, so only real differences remain.
pub fn new_privileges_since(from: u32) -> &'static [char] {
    if from < 17 {
        &['m']
    } else {
        &[]
    }
}

fn strip_acl(item: &str, letters: &[char]) -> String {
    let Some((who, rest)) = item.split_once('=') else { return item.to_string() };
    let (privs, grantor) = rest.split_once('/').unwrap_or((rest, ""));
    let mut out = String::new();
    let mut chars = privs.chars().peekable();
    while let Some(ch) = chars.next() {
        let star = chars.peek() == Some(&'*');
        if star {
            chars.next();
        }
        if !letters.contains(&ch) {
            out.push(ch);
            if star {
                out.push('*');
            }
        }
    }
    if grantor.is_empty() {
        format!("{who}={out}")
    } else {
        format!("{who}={out}/{grantor}")
    }
}

fn strip_list(v: &mut Value, letters: &[char]) {
    if let Some(a) = v.as_array_mut() {
        for x in a.iter_mut() {
            if let Some(s) = x.as_str() {
                *x = Value::String(strip_acl(s, letters));
            }
        }
    }
}

/// [`fingerprint_sql`]'s answer with the privileges `from` doesn't know removed.
pub fn normalize_privileges(print: &mut Value, from: u32) {
    let letters = new_privileges_since(from);
    if letters.is_empty() {
        return;
    }
    if let Some(o) = print.get_mut("relations").and_then(Value::as_object_mut) {
        for v in o.values_mut() {
            if let Some(acl) = v.get_mut(2) {
                strip_list(acl, letters);
            }
        }
    }
    if let Some(o) = print.get_mut("defaultAcls").and_then(Value::as_object_mut) {
        for v in o.values_mut() {
            strip_list(v, letters);
        }
    }
}

/// Pause estimate (§6.1): 10–30 s fixed + size ÷ ~40 MB/s, as a range.
pub fn pause_estimate_secs(size_bytes: u64) -> (u64, u64) {
    let copy = size_bytes / (40 * 1024 * 1024);
    (10 + copy, 30 + copy * 2)
}

/// §6.1 disk rule: free ≥ 2 × size + 1 GiB.
pub fn disk_needed(size_bytes: u64) -> u64 {
    size_bytes.saturating_mul(2).saturating_add(1 << 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn k2(n: &str) -> bool {
        k2_root_proto::pg_major::role_name_ok(n)
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_lit("it's"), "'it''s'");
        assert!(role_set_sql(&["ws_x_agent".into()]).contains("ARRAY['ws_x_agent']::text[]"));
        assert!(role_set_sql(&[]).contains("ARRAY[]::text[]"));
    }

    #[test]
    fn memberships_become_grants_with_options() {
        let a = r#"[["ws_jobs_migrator","ws_jobs_agent",false,true,true],["ws_jobs_agent","ws_jobs_bind",false,false,true]]"#;
        assert_eq!(
            membership_grants(a, k2).unwrap(),
            "GRANT \"ws_jobs_migrator\" TO \"ws_jobs_agent\" WITH ADMIN false, INHERIT true, SET true;\n\
             GRANT \"ws_jobs_agent\" TO \"ws_jobs_bind\" WITH ADMIN false, INHERIT false, SET true;\n"
        );
        assert!(membership_grants(r#"[["postgres","x",false,true,true]]"#, k2).is_none());
        assert!(membership_grants("nope", k2).is_none());
        assert_eq!(membership_grants("[]", k2).unwrap(), "");
    }

    #[test]
    fn create_db_keeps_owner_encoding_locale() {
        let p16 = json!({"owner": "ws_jobs_migrator", "enc": "UTF8", "datcollate": "C.UTF-8", "datctype": "C.UTF-8", "datlocprovider": "c"});
        assert_eq!(
            create_db_sql("ws_jobs", &p16, k2).unwrap(),
            "CREATE DATABASE \"ws_jobs\" OWNER \"ws_jobs_migrator\" TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C.UTF-8' LC_CTYPE 'C.UTF-8';\n"
        );
        let icu16 = json!({"owner": "o", "enc": "UTF8", "datcollate": "en_US.UTF-8", "datctype": "en_US.UTF-8", "datlocprovider": "i", "daticulocale": "en-US"});
        assert!(create_db_sql("d", &icu16, k2).unwrap().contains("LOCALE_PROVIDER icu ICU_LOCALE 'en-US'"));
        let icu17 = json!({"owner": "o", "enc": "UTF8", "datcollate": "C", "datctype": "C", "datlocprovider": "i", "datlocale": "und"});
        assert!(create_db_sql("d", &icu17, k2).unwrap().contains("ICU_LOCALE 'und'"));
        let bad = json!({"owner": "o", "enc": "UTF8", "datcollate": "C'; DROP", "datctype": "C", "datlocprovider": "c"});
        assert!(create_db_sql("d", &bad, k2).is_none());
        let su = json!({"owner": "postgres", "enc": "UTF8", "datcollate": "C", "datctype": "C"});
        assert!(create_db_sql("d", &su, k2).is_none(), "K2 databases are never owned by postgres");
    }

    #[test]
    fn datacl_replays_exactly() {
        let a = r#"{"isNull": false, "items": [["PUBLIC","TEMPORARY",false],["ws_jobs_agent","CONNECT",false],["ws_jobs_migrator","CONNECT",true]]}"#;
        assert_eq!(
            datacl_replay("ws_jobs", a, k2).unwrap(),
            "REVOKE ALL ON DATABASE \"ws_jobs\" FROM PUBLIC;\n\
             GRANT TEMPORARY ON DATABASE \"ws_jobs\" TO PUBLIC;\n\
             GRANT CONNECT ON DATABASE \"ws_jobs\" TO \"ws_jobs_agent\";\n\
             GRANT CONNECT ON DATABASE \"ws_jobs\" TO \"ws_jobs_migrator\" WITH GRANT OPTION;\n"
        );
        assert_eq!(datacl_replay("d", r#"{"isNull": true, "items": []}"#, k2).unwrap(), "");
        assert!(datacl_replay("d", r#"{"isNull": false, "items": [["x","ALL; DROP",false]]}"#, k2).is_none());
    }

    #[test]
    fn settings_replay() {
        let a = r#"[["","search_path=public, k2"],["ws_jobs_agent","statement_timeout=5s"]]"#;
        assert_eq!(
            db_settings_replay("ws_jobs", a, k2).unwrap(),
            "ALTER DATABASE \"ws_jobs\" SET search_path = 'public, k2';\n\
             ALTER ROLE \"ws_jobs_agent\" IN DATABASE \"ws_jobs\" SET statement_timeout = '5s';\n"
        );
        assert!(db_settings_replay("d", r#"[["","bad name=1"]]"#, k2).is_none());
    }

    #[test]
    fn count_wrap_refuses_stacked_statements() {
        assert!(count_wrap("SELECT 'a' AS t, count(*) AS n FROM \"public\".\"a\"").unwrap().starts_with("SELECT coalesce(json_object_agg"));
        assert!(count_wrap("x; DROP TABLE y").is_none());
        assert_eq!(count_wrap("  ").unwrap(), "SELECT '{}'::json;\n");
    }

    #[test]
    fn diffs_name_each_key() {
        let a = json!({"rls": {"public.jobs": [true, true]}, "extensions": ["pgcrypto"], "sequences": {"public.s": 41}});
        let b = json!({"rls": {"public.jobs": [true, false], "public.x": [false, false]}, "extensions": ["pgcrypto"], "sequences": {}});
        let d = diff(&a, &b);
        assert_eq!(d.len(), 3, "{d:?}");
        assert!(d.contains(&"rls public.jobs: [true,true] → [true,false]".to_string()));
        assert!(d.contains(&"rls public.x: missing → [false,false]".to_string()));
        assert!(d.contains(&"sequences public.s: 41 → missing".to_string()));
        assert!(diff(&a, &a).is_empty());
        assert_eq!(diff(&json!({"extensions": ["a"]}), &json!({"extensions": ["b"]})), vec!["extensions: [\"a\"] → [\"b\"]"]);
    }

    #[test]
    fn privileges_new_in_17_are_not_a_difference() {
        let mut src = json!({"relations": {"public.jobs": ["r", "o", ["a=arw/o", "o=arwdDxt/o"]]}, "defaultAcls": {"o::r": ["a=r/o"]}});
        let mut dst = json!({"relations": {"public.jobs": ["r", "o", ["a=arw/o", "o=arwdDxtm/o"]]}, "defaultAcls": {"o::r": ["a=rm*/o"]}});
        assert_eq!(diff(&src, &dst).len(), 2);
        normalize_privileges(&mut src, 16);
        normalize_privileges(&mut dst, 16);
        assert!(diff(&src, &dst).is_empty(), "{:?}", diff(&src, &dst));
        // A real difference stays one.
        let mut lost = json!({"relations": {"public.jobs": ["r", "o", ["o=arwdDxtm/o"]]}, "defaultAcls": {"o::r": ["a=r/o"]}});
        normalize_privileges(&mut lost, 16);
        assert_eq!(diff(&src, &lost).len(), 1);
        // 17 → 18 strips nothing.
        let mut same = dst.clone();
        normalize_privileges(&mut same, 17);
        assert_eq!(same, dst);
        assert_eq!(strip_acl("=Tc/o", &['m']), "=Tc/o");
    }

    #[test]
    fn estimate_and_disk() {
        assert_eq!(pause_estimate_secs(0), (10, 30));
        let gb = 1u64 << 30;
        let (lo, hi) = pause_estimate_secs(gb);
        assert_eq!((lo, hi), (35, 80), "1 GiB ≈ 30–60 s (+ index builds)");
        assert_eq!(disk_needed(gb), 3 * gb);
    }
}
