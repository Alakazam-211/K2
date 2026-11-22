//! Everything the restored objects refer to that the import doesn't carry
//! (PRD §9.1: "it never guesses"): a call into a dropped Supabase schema
//! (`vault.decrypted_secrets`, `net.http_post`, `cron.schedule`), an `auth`
//! or `storage` object K2's own schemas don't have, and a policy for a role
//! that isn't one of the standard three.
//!
//! The scan reads the schema-only script `pg_restore` prints for the entries
//! K2 restores, object by object (its `-- Name: …; Type: …; Schema: …`
//! headers), and reports `(object, reference)` pairs. Text in a comment line
//! is skipped; text inside a function body is not (that is where such calls
//! live).

use std::collections::BTreeSet;

use super::toc::{Unresolved, SUPABASE_SCHEMAS};

/// What K2's own `auth` and `storage` schemas have (the identity lane's and
/// the storage lane's canned SQL, PRD §5 and §10). Anything else in them is
/// unresolved.
pub const AUTH_HAS: &[&str] = &["users", "uid", "role", "jwt", "email"];
pub const STORAGE_HAS: &[&str] = &["buckets", "objects", "foldername", "filename", "extension", "update_updated_at_column"];

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// `schema.name` references in one line, for the schemas in `watch`.
fn refs_in(line: &str, watch: &[&str]) -> Vec<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let quoted = chars[i] == '"';
        let start = if quoted { i + 1 } else { i };
        let boundary = i == 0 || !(ident_char(chars[i - 1]) || chars[i - 1] == '.' || chars[i - 1] == '"');
        if !boundary || !(quoted || ident_char(chars[i])) {
            i += 1;
            continue;
        }
        let mut j = start;
        while j < chars.len() && ident_char(chars[j]) {
            j += 1;
        }
        let schema: String = chars[start..j].iter().collect();
        let mut k = j;
        if quoted {
            if chars.get(k) != Some(&'"') {
                i = j.max(i + 1);
                continue;
            }
            k += 1;
        }
        if chars.get(k) == Some(&'.') && watch.contains(&schema.as_str()) {
            let mut m = k + 1;
            let q2 = chars.get(m) == Some(&'"');
            if q2 {
                m += 1;
            }
            let n0 = m;
            while m < chars.len() && ident_char(chars[m]) {
                m += 1;
            }
            if m > n0 {
                out.push((schema, chars[n0..m].iter().collect()));
            }
            i = m;
            continue;
        }
        i = j.max(i + 1);
    }
    out
}

/// The roles a `CREATE POLICY` line names after `TO`.
fn policy_roles(line: &str) -> Vec<String> {
    let Some((_, after)) = line.split_once(" TO ") else { return Vec::new() };
    let end = [" USING ", " WITH CHECK ", ";"].iter().filter_map(|k| after.find(k)).min().unwrap_or(after.len());
    after[..end].split(',').map(|r| r.trim().trim_matches('"').to_string()).filter(|r| !r.is_empty()).collect()
}

/// Scan a schema-only script. References into the schemas K2 makes itself
/// (`auth`, `storage`, `extensions`) are checked against what K2's versions
/// have; any other Supabase schema is not carried at all.
pub fn scan(script: &str) -> BTreeSet<Unresolved> {
    let mut out = BTreeSet::new();
    let mut object = String::from("(the dump's preamble)");
    for line in script.lines() {
        let t = line.trim_start();
        if let Some(h) = t.strip_prefix("-- Name: ") {
            // `-- Name: f(integer); Type: FUNCTION; Schema: public; Owner: -`
            let mut parts = h.split("; ");
            let name = parts.next().unwrap_or("");
            let kind = parts.next().and_then(|p| p.strip_prefix("Type: ")).unwrap_or("");
            let schema = parts.next().and_then(|p| p.strip_prefix("Schema: ")).unwrap_or("-");
            object = if schema == "-" { format!("{kind} {name}") } else { format!("{kind} {schema}.{name}") };
            continue;
        }
        if t.starts_with("--") || t.is_empty() {
            continue;
        }
        for (schema, name) in refs_in(line, SUPABASE_SCHEMAS) {
            let fine = match schema.as_str() {
                "auth" => AUTH_HAS.contains(&name.as_str()),
                "storage" => STORAGE_HAS.contains(&name.as_str()),
                // Extension functions live in `extensions`, which K2 makes;
                // a name the carried extensions lack fails the restore loudly.
                "extensions" => true,
                _ => false,
            };
            if !fine {
                out.insert(Unresolved { object: object.clone(), reference: format!("{schema}.{name}") });
            }
        }
        if t.starts_with("CREATE POLICY ") {
            for r in policy_roles(t) {
                if !matches!(r.as_str(), "anon" | "authenticated" | "service_role" | "public" | "PUBLIC") {
                    out.insert(Unresolved { object: object.clone(), reference: format!("role {r} (only anon, authenticated and service_role are carried)") });
                }
            }
        }
    }
    out
}
