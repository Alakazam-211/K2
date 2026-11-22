//! The dump's table of contents (`pg_restore -l`), read and sorted into what
//! K2 restores, what it re-grants, what it reads as data, and what it drops
//! or can't carry.
//!
//! A line is `<id>; <catalog oid> <oid> <DESC> <namespace> <tag> <owner>`,
//! where DESC may be several words (`TABLE DATA`, `FK CONSTRAINT`,
//! `DEFAULT ACL`) and the namespace is `-` for objects outside a schema.

use std::collections::{BTreeMap, BTreeSet};

/// Every entry kind `pg_dump` writes, longest first so `TABLE DATA` wins
/// over `TABLE`.
const DESCS: &[&str] = &[
    "PUBLICATION TABLES IN SCHEMA",
    "TEXT SEARCH CONFIGURATION",
    "TEXT SEARCH DICTIONARY",
    "MATERIALIZED VIEW DATA",
    "FOREIGN DATA WRAPPER",
    "TEXT SEARCH TEMPLATE",
    "PROCEDURAL LANGUAGE",
    "TEXT SEARCH PARSER",
    "SUBSCRIPTION TABLE",
    "DATABASE PROPERTIES",
    "PUBLICATION TABLE",
    "SEQUENCE OWNED BY",
    "MATERIALIZED VIEW",
    "CHECK CONSTRAINT",
    "STATISTICS DATA",
    "OPERATOR FAMILY",
    "OPERATOR CLASS",
    "FOREIGN SERVER",
    "SECURITY LABEL",
    "LARGE OBJECTS",
    "EVENT TRIGGER",
    "FOREIGN TABLE",
    "FK CONSTRAINT",
    "ACCESS METHOD",
    "SEQUENCE SET",
    "SUBSCRIPTION",
    "INDEX ATTACH",
    "TABLE ATTACH",
    "LARGE OBJECT",
    "ROW SECURITY",
    "USER MAPPING",
    "DEFAULT ACL",
    "PUBLICATION",
    "STDSTRINGS",
    "SEARCHPATH",
    "STATISTICS",
    "CONVERSION",
    "CONSTRAINT",
    "SHELL TYPE",
    "TABLE DATA",
    "BLOB DATA",
    "COLLATION",
    "PROCEDURE",
    "AGGREGATE",
    "TRANSFORM",
    "EXTENSION",
    "SEQUENCE",
    "FUNCTION",
    "OPERATOR",
    "ENCODING",
    "DATABASE",
    "LANGUAGE",
    "TRIGGER",
    "COMMENT",
    "DEFAULT",
    "POLICY",
    "SCHEMA",
    "DOMAIN",
    "SERVER",
    "TABLE",
    "INDEX",
    "CAST",
    "RULE",
    "TYPE",
    "VIEW",
    "BLOB",
    "ACL",
];

/// Supabase's own schemas: dropped with everything in them (PRD §9.1). K2
/// installs its own `auth` and `storage` and makes `extensions` itself.
pub const SUPABASE_SCHEMAS: &[&str] = &[
    "auth",
    "storage",
    "realtime",
    "_realtime",
    "supabase_functions",
    "supabase_migrations",
    "graphql",
    "graphql_public",
    "pgsodium",
    "pgsodium_masks",
    "vault",
    "extensions",
    "net",
    "pgbouncer",
    "cron",
    "_analytics",
    "pgtle",
    "_supavisor",
];

/// Supabase's own extensions (and the ones it runs its platform on): never
/// carried. Objects that call them are found by the reference scan.
pub const SUPABASE_EXTENSIONS: &[&str] = &[
    "pg_graphql",
    "pgsodium",
    "supabase_vault",
    "pg_net",
    "pg_cron",
    "pgjwt",
    "pg_stat_statements",
    "pg_stat_monitor",
    "pgaudit",
    "pg_tle",
    "wrappers",
    "index_advisor",
    "hypopg",
    "plpgsql_check",
    "supautils",
    "safeupdate",
];

/// Roles that are Supabase's machinery, never an App's.
pub fn is_supabase_role(r: &str) -> bool {
    matches!(r, "postgres" | "supabase_admin" | "dashboard_user" | "authenticator" | "pgbouncer" | "pg_database_owner")
        || r.starts_with("supabase_")
        || r.starts_with("pgsodium_")
}

/// One parsed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub line: String,
    pub desc: &'static str,
    /// `-` for an object outside a schema.
    pub ns: String,
    /// The tag and the owner (the last word, empty for some entries).
    pub rest: String,
}

impl Entry {
    /// The owner: the last word (`""` when the entry has none).
    pub fn owner(&self) -> &str {
        let r = self.rest.trim_end();
        match r.rsplit_once(' ') {
            Some((_, o)) => o,
            None => "",
        }
    }
    /// The tag without its owner.
    pub fn tag(&self) -> &str {
        let r = self.rest.trim_end();
        match r.rsplit_once(' ') {
            Some((t, _)) => t.trim_end(),
            None => r,
        }
    }
    /// The first word of the tag: the table of a policy or trigger, the
    /// name of a schema or extension, the object kind of an ACL/COMMENT.
    pub fn first(&self) -> &str {
        self.rest.split_whitespace().next().unwrap_or("")
    }
    /// The second word of the tag (an ACL's or COMMENT's object name).
    pub fn second(&self) -> &str {
        self.rest.split_whitespace().nth(1).unwrap_or("")
    }
}

fn number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// One `pg_restore -l` line, or `None` for comments, blanks and anything
/// not shaped like an entry.
pub fn parse_line(l: &str) -> Option<Entry> {
    let t = l.trim_start();
    if t.is_empty() || t.starts_with(';') {
        return None;
    }
    let (id, after) = t.split_once(';')?;
    if !number(id.trim()) {
        return None;
    }
    let mut rest = after.trim_start();
    for _ in 0..2 {
        let (n, r) = rest.split_once(' ')?;
        if !number(n) {
            return None;
        }
        rest = r.trim_start();
    }
    let desc = DESCS.iter().copied().find(|d| rest.starts_with(d) && rest[d.len()..].starts_with(' '))?;
    let after_desc = rest[desc.len()..].trim_start();
    let (ns, tail) = after_desc.split_once(' ').unwrap_or((after_desc, ""));
    Some(Entry { line: l.trim_end().to_string(), desc, ns: ns.to_string(), rest: tail.trim_end().to_string() })
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s)
}

/// The data K2 reads out of the dump instead of restoring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Extract {
    /// `auth.users`: the projection's rows (never the password).
    AuthUsers,
    /// `supabase_migrations.schema_migrations`: the migration map.
    Migrations,
    StorageBuckets,
    StorageObjects,
}

/// Something the import can't carry and won't guess at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unresolved {
    pub object: String,
    pub reference: String,
}

/// The sorted table of contents.
#[derive(Debug, Default)]
pub struct Plan {
    /// Restored by `pg_restore -L` as the migrator.
    pub keep: Vec<String>,
    /// ACL and DEFAULT ACL entries: printed, rewritten, run as the migrator.
    pub acl: Vec<String>,
    pub extract: BTreeMap<Extract, String>,
    /// Supabase schemas dropped, with how many entries each had.
    pub dropped_schemas: BTreeMap<String, usize>,
    pub dropped_extensions: BTreeSet<String>,
    pub carried_extensions: BTreeSet<String>,
    /// App triggers and policies kept on `auth.users` and `storage.*`.
    pub kept_on_supabase_tables: Vec<String>,
    /// Triggers on `auth.users` / `storage.*`, kept or dropped by
    /// [`decide_triggers`] once their definitions are read.
    pub pending_triggers: Vec<String>,
    pub notes: BTreeSet<String>,
    pub unresolved: BTreeSet<Unresolved>,
    pub policies: usize,
    pub tables: usize,
    pub functions: usize,
}

/// Sort every line of `toc`. `trusted` are the extensions the target
/// cluster has and lets a database owner create (`pg_available_extension_versions.trusted`).
pub fn classify(toc: &str, trusted: &BTreeSet<String>) -> Plan {
    let mut p = Plan::default();
    let supa_schema = |s: &str| SUPABASE_SCHEMAS.contains(&s);
    for l in toc.lines() {
        let Some(e) = parse_line(l) else { continue };
        let ns = e.ns.as_str();
        let drop_in = |p: &mut Plan, schema: &str| *p.dropped_schemas.entry(schema.to_string()).or_default() += 1;
        match (e.desc, ns) {
            // Session settings the restore writes first.
            ("ENCODING" | "STDSTRINGS" | "SEARCHPATH", _) => p.keep.push(e.line.clone()),
            ("SCHEMA", "-") => {
                let name = unquote(e.first());
                if supa_schema(name) {
                    drop_in(&mut p, name);
                } else if name == "public" {
                    p.notes.insert("schema public: exists already; not re-created".into());
                } else {
                    p.keep.push(e.line.clone());
                }
            }
            ("EXTENSION", "-") => {
                let name = unquote(e.first()).to_string();
                if name == "plpgsql" {
                    continue;
                } else if SUPABASE_EXTENSIONS.contains(&name.as_str()) {
                    if name == "pg_stat_statements" {
                        p.notes.insert("pg_stat_statements: skipped (it needs a preload setting this server doesn't write)".into());
                    }
                    p.dropped_extensions.insert(name);
                } else if trusted.contains(&name) {
                    p.carried_extensions.insert(name);
                    p.keep.push(e.line.clone());
                } else {
                    p.unresolved.insert(Unresolved {
                        object: format!("EXTENSION {name}"),
                        reference: "not on this box as an extension a database owner may create (the import carries trusted ones: pgcrypto, uuid-ossp, pg_trgm, citext…)".into(),
                    });
                }
            }
            ("COMMENT", "-") if e.first() == "EXTENSION" => {
                if p.carried_extensions.contains(unquote(e.second())) {
                    p.keep.push(e.line.clone());
                }
            }
            ("COMMENT" | "ACL", "-") if e.first() == "SCHEMA" => {
                let name = unquote(e.second());
                if supa_schema(name) && name != "extensions" {
                    drop_in(&mut p, name);
                } else if e.desc == "ACL" {
                    p.acl.push(e.line.clone());
                } else if name != "public" && name != "extensions" {
                    p.keep.push(e.line.clone());
                }
            }
            ("ACL", "-") if matches!(e.first(), "LANGUAGE" | "DATABASE" | "FOREIGN") => {
                p.notes.insert(format!("{}: grants on it are the box's, not carried", e.tag()));
            }
            ("PUBLICATION" | "PUBLICATION TABLE" | "PUBLICATION TABLES IN SCHEMA", _) => {
                p.notes.insert(format!("publication {}: Realtime isn't carried; dropped", e.first()));
            }
            ("SUBSCRIPTION" | "SUBSCRIPTION TABLE", _) => {
                p.notes.insert(format!("subscription {}: dropped", e.first()));
            }
            ("EVENT TRIGGER", _) => {
                p.notes.insert(format!("event trigger {}: Supabase's own (it needs a superuser); dropped", e.first()));
            }
            ("DATABASE" | "DATABASE PROPERTIES", _) => {
                p.notes.insert("database-level settings in the dump: not carried".into());
            }
            ("LANGUAGE" | "PROCEDURAL LANGUAGE", _) => {
                if e.first() != "plpgsql" {
                    p.unresolved.insert(Unresolved { object: format!("LANGUAGE {}", e.first()), reference: "a procedural language the import doesn't carry".into() });
                }
            }
            ("FOREIGN DATA WRAPPER" | "FOREIGN SERVER" | "SERVER" | "USER MAPPING" | "FOREIGN TABLE", _) => {
                p.unresolved.insert(Unresolved {
                    object: format!("{} {}{}", e.desc, if ns == "-" { String::new() } else { format!("{ns}.") }, e.first()),
                    reference: "foreign data (wrappers, servers, foreign tables) isn't carried".into(),
                });
            }
            // The rows K2 reads itself.
            ("TABLE DATA", "auth") if e.first() == "users" => {
                p.extract.insert(Extract::AuthUsers, e.line.clone());
            }
            ("TABLE DATA", "supabase_migrations") if e.first() == "schema_migrations" => {
                p.extract.insert(Extract::Migrations, e.line.clone());
            }
            ("TABLE DATA", "storage") if e.first() == "buckets" => {
                p.extract.insert(Extract::StorageBuckets, e.line.clone());
            }
            ("TABLE DATA", "storage") if e.first() == "objects" => {
                p.extract.insert(Extract::StorageObjects, e.line.clone());
            }
            // The App's policies on Storage's tables (Storage ships none of
            // its own) stay on K2's tables of the same name; every entry on
            // these shows the table's owner, a Supabase role.
            ("POLICY", "storage") if matches!(e.first(), "objects" | "buckets") => {
                p.policies += 1;
                p.kept_on_supabase_tables.push(format!("policy on storage.{}", e.tag()));
                p.keep.push(e.line.clone());
            }
            // Triggers on `auth.users` (an App's profile row at sign-up) and
            // on Storage's tables: decided by what they call
            // ([`decide_triggers`]): the App's own functions stay, Supabase's
            // (`storage.update_updated_at_column()`) go.
            ("TRIGGER", "auth") if e.first() == "users" => p.pending_triggers.push(e.line.clone()),
            ("TRIGGER", "storage") if matches!(e.first(), "objects" | "buckets") => p.pending_triggers.push(e.line.clone()),
            (_, s) if supa_schema(s) => drop_in(&mut p, s),
            ("ACL" | "DEFAULT ACL", _) => p.acl.push(e.line.clone()),
            (d, _) => {
                match d {
                    "POLICY" => p.policies += 1,
                    "TABLE" => p.tables += 1,
                    "FUNCTION" | "PROCEDURE" => p.functions += 1,
                    _ => {}
                }
                p.keep.push(e.line.clone());
            }
        }
    }
    p
}

/// Decide the pending triggers from their definitions (`pg_restore -s -L`
/// of just those entries): one whose function is outside Supabase's schemas
/// is the App's and is kept (onto K2's table of the same name); one that
/// calls into `storage.`/`auth.` is Supabase's own and is dropped.
pub fn decide_triggers(p: &mut Plan, script: &str) {
    // `-- Name: users on_auth_user_created; Type: TRIGGER; Schema: auth; …`
    // then `CREATE TRIGGER … EXECUTE FUNCTION public.handle_new_user();`
    let mut calls: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut current: Option<(String, String)> = None;
    for l in script.lines() {
        if let Some(h) = l.strip_prefix("-- Name: ") {
            let mut parts = h.split("; ");
            let name = parts.next().unwrap_or("").to_string();
            let kind = parts.next().unwrap_or("");
            let schema = parts.next().and_then(|x| x.strip_prefix("Schema: ")).unwrap_or("").to_string();
            current = (kind == "Type: TRIGGER").then_some((schema, name));
            continue;
        }
        let Some(key) = &current else { continue };
        let at = l.find("EXECUTE FUNCTION ").map(|i| i + 17).or_else(|| l.find("EXECUTE PROCEDURE ").map(|i| i + 18));
        if let Some(i) = at {
            calls.insert(key.clone(), l[i..].split('(').next().unwrap_or("").trim().to_string());
        }
    }
    for line in std::mem::take(&mut p.pending_triggers) {
        let Some(e) = parse_line(&line) else { continue };
        let target = calls.get(&(e.ns.clone(), e.tag().to_string())).cloned().unwrap_or_default();
        let schema = unquote(target.split('.').next().unwrap_or("")).to_string();
        let on = format!("{}.{}", e.ns, e.tag());
        if target.is_empty() {
            p.unresolved.insert(Unresolved { object: format!("TRIGGER {on}"), reference: "its definition wasn't in the dump's schema".into() });
        } else if target.contains('.') && SUPABASE_SCHEMAS.contains(&schema.as_str()) && schema != "extensions" {
            *p.dropped_schemas.entry(e.ns.clone()).or_default() += 1;
        } else {
            p.kept_on_supabase_tables.push(format!("trigger on {on} (calls {target})"));
            p.keep.push(line);
        }
    }
}
