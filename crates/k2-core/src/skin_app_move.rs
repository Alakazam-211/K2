//! A `--skin` site's access, copied onto an App of the same workspace when
//! the App takes the site's label (`k2 app label take`, 0.46.16,
//! `.k2/prds/plan-skin-to-app-move-v1.md`).
//!
//! A site and an App keep their guests in the same skin.db rows, keyed by
//! the published service id: `grants.kind = 'app'` with `target_id` = the
//! service, and roles tagged `roles.app_id` = the service. The copy, in one
//! transaction:
//!
//! - every principal grant on the site → the same grant on the App (its
//!   rooms `scope`, its `enabled` flag: a disabled guest stays disabled,
//!   and its role);
//! - a role tagged to the site → a copy tagged to the App with the same
//!   columns (policy, rooms, folders), named `<role>-<app>` (role names are
//!   unique box-wide); a box-wide role is used as it is;
//! - a role tagged to a third App → that guest isn't copied
//!   (`skipped_other_app_role`, as copy-host does);
//! - a guest another workspace's agent added → not copied
//!   (`skipped_other_workspace_agent`);
//! - a grant the App already has wins (`kept_existing`).
//!
//! The site's own rows are never changed: moving the label back
//! (`k2 app label return`) leaves the site exactly as it was. Guests keep
//! their username and password (principals are box-wide); their sessions
//! don't follow (the site's sids live in its helper's memory).

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::skin::{app_grant, blank_to_none, creators, guest_rooms, members, now_secs, record_access_audit, with_conn, HOST_APP_ID, USERNAME_MAX_LEN};

/// What a copy did (or would do, on a dry run).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMoveReport {
    pub from: String,
    pub to: String,
    pub dry_run: bool,
    /// Usernames that got a grant on the App (or would).
    pub added: Vec<String>,
    /// Usernames the App already had a grant for: it wins. `(user, enabled)`.
    pub kept_existing: Vec<(String, bool)>,
    /// Usernames whose role is tagged for a third App: not copied.
    pub skipped_other_app_role: Vec<String>,
    /// Guests another workspace's agent added: not copied.
    pub skipped_other_workspace_agent: Vec<String>,
    /// Roles copied for the App: `(the site's role, the App's copy)`.
    pub roles_copied: Vec<(String, String)>,
}

/// Why [`copy_site_grants_to_app_expect`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectError {
    /// The guests it would copy now aren't the ones expected (sorted).
    Changed { now: Vec<String> },
    Store(String),
}

/// Copy site `from`'s guests onto App `to` (both published service ids;
/// `to_name` is the App id, used to name copied roles). `app_workspace` is
/// the App's `projects.id` (from the daemon). Audited unless `dry_run`.
pub fn copy_site_grants_to_app(
    from: &str,
    to: &str,
    to_name: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<AppMoveReport, String> {
    let report = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin site copy begin: {e}"))?;
        let r = copy_tx(&tx, from, to, to_name, app_workspace, created_by, dry_run)?;
        tx.commit().map_err(|e| format!("skin site copy commit: {e}"))?;
        Ok(r)
    })?;
    after(created_by, &report);
    Ok(report)
}

/// [`copy_site_grants_to_app`], only when the guests it would copy are
/// exactly `expected` (any order): the comparison and the write are one
/// transaction, so an approved card copies exactly the guests it showed.
pub fn copy_site_grants_to_app_expect(
    from: &str,
    to: &str,
    to_name: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    expected: &[String],
) -> Result<AppMoveReport, ExpectError> {
    let mut want: Vec<String> = expected.iter().map(|u| u.trim().to_ascii_lowercase()).filter(|u| !u.is_empty()).collect();
    want.sort();
    want.dedup();
    let report = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin site copy begin: {e}"))?;
        let mut now = copy_tx(&tx, from, to, to_name, app_workspace, created_by, true)?.added;
        now.sort();
        if now != want {
            return Ok(Err(now));
        }
        let r = copy_tx(&tx, from, to, to_name, app_workspace, created_by, false)?;
        tx.commit().map_err(|e| format!("skin site copy commit: {e}"))?;
        Ok(Ok(r))
    })
    .map_err(ExpectError::Store)?
    .map_err(|now| ExpectError::Changed { now })?;
    after(created_by, &report);
    Ok(report)
}

fn after(actor: &str, r: &AppMoveReport) {
    if r.dry_run {
        return;
    }
    record_access_audit(
        "app_grant_copy_site",
        actor,
        &format!(
            "site {} → app {}: added {} guest(s), kept {} existing App grant(s), skipped {} other-App role(s) and {} other workspace's guest(s), copied {} role(s)",
            r.from,
            r.to,
            r.added.len(),
            r.kept_existing.len(),
            r.skipped_other_app_role.len(),
            r.skipped_other_workspace_agent.len(),
            r.roles_copied.len()
        ),
    );
    for (user, enabled) in &r.kept_existing {
        record_access_audit("app_grant_copy_site", user, &format!("app {}: kept its existing {} App grant", r.to, if *enabled { "enabled" } else { "disabled" }));
    }
    for user in &r.skipped_other_app_role {
        record_access_audit("app_grant_copy_site", user, &format!("app {}: not copied, the role is tagged for another App", r.to));
    }
    for user in &r.skipped_other_workspace_agent {
        record_access_audit("app_grant_copy_site", user, &format!("app {}: not copied, another workspace's agent added this guest", r.to));
    }
    members::changed(None);
}

struct SiteGrant {
    pid: String,
    username: String,
    role_id: Option<String>,
    scope: Option<String>,
    enabled: bool,
    /// The role row's `app_id`, `None` when box-wide; `Err` when the
    /// grant names a role that no longer exists.
    role_app: Result<Option<String>, ()>,
}

#[allow(clippy::too_many_arguments)]
fn copy_tx(
    conn: &Connection,
    from: &str,
    to: &str,
    to_name: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<AppMoveReport, String> {
    let (from, to) = (from.trim(), to.trim());
    if from.is_empty() || to.is_empty() || from == HOST_APP_ID || to == HOST_APP_ID {
        return Err("site copy: name a published site and an App, never host".to_string());
    }
    if from == to {
        return Err("site copy: the site and the App are the same service".to_string());
    }
    let rows: Vec<SiteGrant> = {
        let mut st = conn
            .prepare(
                "SELECT p.id, p.username, g.role_id, g.scope, g.enabled, r.id, r.app_id
                 FROM grants g
                 JOIN principals p ON p.id = g.subject_id
                 LEFT JOIN roles r ON r.id = g.role_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1
                 ORDER BY p.username",
            )
            .map_err(|e| format!("skin site copy: {e}"))?;
        let mapped = st
            .query_map(params![from], |r| {
                let role_id = blank_to_none(r.get::<_, Option<String>>(2)?);
                let role_found: Option<String> = r.get(5)?;
                let role_app = match (&role_id, role_found) {
                    (Some(_), None) => Err(()),
                    _ => Ok(blank_to_none(r.get::<_, Option<String>>(6)?)),
                };
                Ok(SiteGrant {
                    pid: r.get(0)?,
                    username: r.get(1)?,
                    role_id,
                    scope: r.get(3)?,
                    enabled: r.get::<_, i64>(4)? != 0,
                    role_app,
                })
            })
            .map_err(|e| format!("skin site copy: {e}"))?;
        mapped.collect::<Result<Vec<_>, _>>().map_err(|e| format!("skin site copy row: {e}"))?
    };
    let mut report = AppMoveReport { from: from.to_string(), to: to.to_string(), dry_run, ..Default::default() };
    // The site's role id → the App's copy (made once per role).
    let mut copies: HashMap<String, String> = HashMap::new();
    for g in rows {
        if let Some(by_ws) = creators::workspace_of_conn(conn, &g.pid)? {
            if app_workspace != Some(by_ws.as_str()) {
                report.skipped_other_workspace_agent.push(g.username);
                continue;
            }
        }
        if let Some(existing) = app_grant(conn, &g.pid, to)? {
            report.kept_existing.push((g.username, existing.enabled));
            continue;
        }
        let role_id = match (&g.role_id, &g.role_app) {
            (None, _) => None,
            // A grant naming a role that's gone gives no access on the
            // site either: copied role-less with its scope (no wider).
            (Some(_), Err(())) => None,
            (Some(id), Ok(None)) => Some(id.clone()),
            (Some(id), Ok(Some(a))) if a == to => Some(id.clone()),
            (Some(id), Ok(Some(a))) if a == from => match copies.get(id) {
                Some(c) => Some(c.clone()),
                None => {
                    let (copy_id, old_name, new_name) = copy_role(conn, id, to, to_name, dry_run)?;
                    report.roles_copied.push((old_name, new_name));
                    copies.insert(id.clone(), copy_id.clone());
                    Some(copy_id)
                }
            },
            (Some(_), Ok(Some(_))) => {
                report.skipped_other_app_role.push(g.username);
                continue;
            }
        };
        if !dry_run {
            conn.execute(
                "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                 VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
                params![uuid::Uuid::new_v4().to_string(), g.pid, to, role_id, g.scope, now_secs(), if g.enabled { 1i64 } else { 0i64 }, created_by],
            )
            .map_err(|e| format!("skin site copy insert: {e}"))?;
        }
        report.added.push(g.username);
    }
    Ok(report)
}

/// The App's copy of the site's role `id`: `(copy id, site role name, copy
/// name)`. Named `<role>-<app>` (then `-2` … `-9`); an existing role of
/// that name tagged to this App with the same policy is re-used. A dry run
/// writes nothing and answers the name it would use.
fn copy_role(conn: &Connection, id: &str, to: &str, to_name: &str, dry_run: bool) -> Result<(String, String, String), String> {
    type Cols = (String, String, String, String, Option<String>);
    let read = |sql: &str, key: &str| -> Result<Option<(String, Cols)>, String> {
        conn.query_row(sql, params![key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default(), r.get(5)?),
            ))
        })
        .optional()
        .map_err(|e| format!("skin site copy role: {e}"))
    };
    const COLS: &str = "SELECT name, caps, rooms, COALESCE(room_policy, '{}'), app_id, room_paths FROM roles";
    let (old_name, (caps, rooms, policy, app_id, paths)) =
        read(&format!("{COLS} WHERE id = ?1"), id)?.ok_or_else(|| format!("skin site copy: role {id} vanished"))?;
    let _ = app_id;
    let base: String = format!("{old_name}-{to_name}").chars().take(USERNAME_MAX_LEN - 2).collect();
    for n in 1..=9 {
        let name = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let found = conn
            .query_row(
                "SELECT id, COALESCE(app_id, ''), caps, rooms, COALESCE(room_policy, '{}'), room_paths FROM roles WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("skin site copy role: {e}"))?;
        match found {
            Some((cid, capp, ccaps, crooms, cpolicy, cpaths)) => {
                if capp == to && ccaps == caps && crooms == rooms && cpolicy == policy && cpaths == paths {
                    return Ok((cid, old_name, name));
                }
            }
            None => {
                let new_id = uuid::Uuid::new_v4().to_string();
                if !dry_run {
                    conn.execute(
                        "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id, room_paths)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![new_id, name, caps, rooms, now_secs(), policy, to, paths],
                    )
                    .map_err(|e| format!("skin site copy role insert: {e}"))?;
                    guest_rooms::note_role_conn(conn, &new_id, Some(to))?;
                }
                return Ok((new_id, old_name, name));
            }
        }
    }
    Err(format!("skin site copy: no free name for a copy of role '{old_name}' ({base}…{base}-9 are taken)"))
}

#[cfg(test)]
#[path = "skin_app_move_tests.rs"]
mod tests;
