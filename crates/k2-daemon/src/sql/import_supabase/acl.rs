//! The dump's grants, mapped to the standard roles (PRD §9.1).
//!
//! `pg_restore` prints each ACL entry as single-line statements:
//! `GRANT … TO <role>;`, `REVOKE … FROM <role>;`, `ALTER DEFAULT PRIVILEGES
//! FOR ROLE <r> [IN SCHEMA s] GRANT|REVOKE … TO|FROM <role>;`, and, for a
//! grant made by someone other than the owner, `SET SESSION AUTHORIZATION`
//! around it. K2 keeps the statements whose grantee is `anon`,
//! `authenticated`, `service_role` or `PUBLIC`; drops the ones to Supabase's
//! own roles (the migrator owns everything now, and holds every right); and
//! drops, and names, grants to any other role (an App's own login roles are
//! not carried). Default privileges `FOR ROLE postgres` become the
//! migrator's, so tables a later migration makes get the same grants; ones
//! for Supabase's other roles are dropped. Every statement then runs as the
//! migrator. A line it can't read stops the import: it never guesses.

use std::collections::BTreeSet;

use super::toc::is_supabase_role;

/// The roles whose grants carry over.
pub const STANDARD_ROLES: &[&str] = &["anon", "authenticated", "service_role", "PUBLIC"];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AclOut {
    pub statements: Vec<String>,
    pub kept: usize,
    /// Grants to Supabase's own roles (and their default privileges).
    pub dropped_internal: usize,
    /// Other roles whose grants were dropped.
    pub dropped_roles: BTreeSet<String>,
}

fn unquote(s: &str) -> String {
    match s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        Some(x) => x.replace("\"\"", "\""),
        None => s.to_string(),
    }
}

/// The grantee of a statement body (without its `;`): after the last ` TO `
/// (GRANT) or ` FROM ` (REVOKE), minus `WITH GRANT OPTION` / `GRANTED BY …`.
fn grantee(body: &str, grant: bool) -> Option<String> {
    let kw = if grant { " TO " } else { " FROM " };
    let (_, tail) = body.rsplit_once(kw)?;
    let tail = tail.split(" WITH GRANT OPTION").next()?;
    let tail = tail.split(" GRANTED BY ").next()?;
    let t = tail.trim();
    let quoted = t.starts_with('"');
    if t.is_empty() || (!quoted && (t.contains(',') || t.contains(' '))) {
        return None;
    }
    Some(unquote(t))
}

enum Verdict {
    Keep,
    Internal,
    Other(String),
}

fn verdict(role: &str) -> Verdict {
    if STANDARD_ROLES.contains(&role) {
        Verdict::Keep
    } else if is_supabase_role(role) {
        Verdict::Internal
    } else {
        Verdict::Other(role.to_string())
    }
}

/// Rewrite the printed ACL entries for `migrator` (a role name, unquoted).
pub fn rewrite(script: &str, migrator: &str) -> Result<AclOut, String> {
    let mut out = AclOut::default();
    let mig = format!("\"{}\"", migrator.replace('"', "\"\""));
    for raw in script.lines() {
        let l = raw.trim();
        if l.is_empty()
            || l.starts_with("--")
            || l.starts_with('\\')
            || l.starts_with("SET ")
            || l.starts_with("SELECT pg_catalog.set_config(")
            || l == "RESET SESSION AUTHORIZATION;"
        {
            continue;
        }
        let Some(body) = l.strip_suffix(';') else {
            return Err(format!("a grant in the dump spans lines or isn't one statement: {}", short(l)));
        };
        let (grant, default_for) = if let Some(rest) = body.strip_prefix("ALTER DEFAULT PRIVILEGES FOR ROLE ") {
            let (role, after) = split_ident(rest).ok_or_else(|| format!("unreadable default privileges: {}", short(l)))?;
            let g = after.contains(" GRANT ");
            if !g && !after.contains(" REVOKE ") {
                return Err(format!("unreadable default privileges: {}", short(l)));
            }
            (g, Some((role, after.to_string())))
        } else if body.starts_with("GRANT ") {
            (true, None)
        } else if body.starts_with("REVOKE ") {
            (false, None)
        } else {
            return Err(format!("a statement in the dump's grants the import doesn't read: {}", short(l)));
        };
        let who = grantee(body, grant).ok_or_else(|| format!("no single grantee in: {}", short(l)))?;
        let v = verdict(&who);
        if let Some((owner, after)) = default_for {
            match (owner.as_str(), v) {
                ("postgres", Verdict::Keep) => {
                    out.kept += 1;
                    out.statements.push(format!("ALTER DEFAULT PRIVILEGES FOR ROLE {mig}{after};"));
                }
                ("postgres", Verdict::Internal) => out.dropped_internal += 1,
                (_, Verdict::Other(r)) => {
                    out.dropped_roles.insert(r);
                }
                (o, _) if is_supabase_role(o) => out.dropped_internal += 1,
                (o, _) => {
                    out.dropped_roles.insert(o.to_string());
                }
            }
            continue;
        }
        match v {
            Verdict::Keep => {
                out.kept += 1;
                out.statements.push(l.to_string());
            }
            Verdict::Internal => out.dropped_internal += 1,
            Verdict::Other(r) => {
                out.dropped_roles.insert(r);
            }
        }
    }
    Ok(out)
}

/// A leading identifier (bare or `"quoted"`) and what follows it.
fn split_ident(s: &str) -> Option<(String, &str)> {
    if let Some(q) = s.strip_prefix('"') {
        let mut i = 0;
        let b = q.as_bytes();
        while i < b.len() {
            if b[i] == b'"' {
                if b.get(i + 1) == Some(&b'"') {
                    i += 2;
                    continue;
                }
                return Some((q[..i].replace("\"\"", "\""), &q[i + 1..]));
            }
            i += 1;
        }
        None
    } else {
        let end = s.find(' ').unwrap_or(s.len());
        Some((s[..end].to_string(), &s[end..]))
    }
}

fn short(l: &str) -> String {
    l.chars().take(160).collect()
}
