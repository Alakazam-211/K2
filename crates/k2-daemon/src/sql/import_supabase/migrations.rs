//! Supabase's migration history, renamed to K2's form (PRD §9.1).
//!
//! Supabase names a migration by a 14-digit timestamp
//! (`20240101120000_create_profiles.sql`) and records it in
//! `supabase_migrations.schema_migrations (version, statements, name)`. K2's
//! `k2 db migrate` reads `NNNN_name.sql` and records `_k2_migrations
//! (version, checksum)`. The import numbers the dump's migrations in version
//! order (`0001_create_profiles`, …), writes each as a file rebuilt from its
//! recorded statements, records it as applied (with that file's checksum, so
//! a later `k2 db migrate` skips it and the App's next migration is the next
//! number), and keeps the map in `.k2/db/supabase-migrations.json`.

use serde_json::{json, Value as J};

use super::copy::{parse_text_array, CopyTable};

/// One migration, renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub supabase_version: String,
    pub name: Option<String>,
    /// `0001_create_profiles`.
    pub k2_version: String,
    /// The file K2 writes for it.
    pub sql: String,
}

/// `create profiles!` → `create_profiles`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' };
        if !(c == '_' && (out.is_empty() || out.ends_with('_'))) {
            out.push(c);
        }
    }
    let mut out = out.trim_end_matches('_').to_string();
    out.truncate(60);
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() {
        "migration".into()
    } else {
        out
    }
}

/// The dump's `schema_migrations` rows as K2 migrations, oldest first.
pub fn plan(t: &CopyTable) -> Result<Vec<Mapped>, String> {
    if t.col("version").is_none() {
        return Err("supabase_migrations.schema_migrations has no version column".into());
    }
    let mut rows: Vec<(String, Option<String>, Option<Vec<Option<String>>>)> = Vec::new();
    for r in &t.rows {
        let version = t.get(r, "version").ok_or("a migration row has no version")?.trim().to_string();
        let name = t.get(r, "name").map(str::to_string).filter(|n| !n.trim().is_empty());
        let statements = match t.get(r, "statements") {
            Some(s) => Some(parse_text_array(s).map_err(|e| format!("migration {version}'s statements: {e}"))?),
            None => None,
        };
        rows.push((version, name, statements));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    if rows.len() > 9999 {
        return Err(format!("{} migrations: the renamed files have four digits", rows.len()));
    }
    let mut out = Vec::new();
    for (i, (version, name, statements)) in rows.into_iter().enumerate() {
        let k2_version = format!("{:04}_{}", i + 1, slug(name.as_deref().unwrap_or("migration")));
        let mut sql = format!(
            "-- Supabase migration {version}{} ({cli} db import supabase). Already applied: {cli} db migrate skips it.\n",
            name.as_deref().map(|n| format!("_{n}")).unwrap_or_default(),
            cli = k2_core::brand::CLI_NAME
        );
        match statements {
            Some(list) if !list.is_empty() => {
                for s in list.into_iter().flatten() {
                    sql.push('\n');
                    sql.push_str(s.trim_end().trim_end_matches(';'));
                    sql.push_str(";\n");
                }
            }
            _ => sql.push_str("-- (the dump kept no statements for this migration)\n"),
        }
        out.push(Mapped { supabase_version: version, name, k2_version, sql });
    }
    Ok(out)
}

/// The map file's JSON.
pub fn map_json(list: &[Mapped]) -> J {
    json!({
        "from": "supabase_migrations.schema_migrations",
        "migrations": list.iter().map(|m| json!({ "supabase": m.supabase_version, "name": m.name, "renamed": m.k2_version })).collect::<Vec<_>>(),
    })
}
