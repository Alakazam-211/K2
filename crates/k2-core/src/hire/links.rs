//! Team links (0161, S6): "this team serves that workspace". A link is a
//! K2 record keyed on workspace ids; its messaging edges are
//! `workspace_relations` rows tagged with the link id, so removing the
//! link removes exactly what it added and nothing a person connected by
//! hand. Links are made and removed by the owner, an Admin login or an API
//! key with the `link` scope — never by the linked agents themselves.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{now_secs, HireCaller, HireError};

/// `origin_link_id` prefix for a team's own internal edges (made at hire).
pub const INSTANCE_ORIGIN_PREFIX: &str = "inst:";

pub fn parse_kind(raw: Option<&str>) -> Result<&'static str, HireError> {
    match raw.map(str::trim).unwrap_or("serves") {
        "" | "serves" => Ok("serves"),
        "helper_of" | "helper-of" | "helps" => Ok("helper_of"),
        other => Err(HireError::usage(format!("link kind '{other}': serves or helper_of"))),
    }
}

/// The relation type a link writes (consumer → team lead).
pub fn relation_type_for(kind: &str) -> &'static str {
    if kind == "helper_of" {
        "helper_of"
    } else {
        "served_by"
    }
}

/// A resolved local workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ws {
    pub id: String,
    pub path: String,
    pub handle: String,
}

/// Resolve a workspace token (id, handle, alias, name). Ambiguous names
/// resolve to nothing (fail closed).
pub fn resolve_workspace(conn: &Connection, token: &str) -> Option<Ws> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let row = |sql: &str| -> Vec<Ws> {
        let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
        stmt.query_map(params![t], |r| {
            Ok(Ws {
                id: r.get(0)?,
                path: r.get(1)?,
                handle: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    };
    for sql in [
        "SELECT id, path, handle FROM projects WHERE id = ?1",
        "SELECT id, path, handle FROM projects WHERE handle = ?1 COLLATE NOCASE",
        "SELECT p.id, p.path, p.handle FROM project_handle_aliases a JOIN projects p ON p.id = a.project_id \
         WHERE a.alias = ?1 COLLATE NOCASE",
        "SELECT id, path, handle FROM projects WHERE path = ?1",
        "SELECT id, path, handle FROM projects WHERE name = ?1 COLLATE NOCASE",
    ] {
        let hits = row(sql);
        match hits.len() {
            0 => continue,
            1 => return hits.into_iter().next(),
            _ => return None,
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub id: String,
    pub instance_id: Option<String>,
    pub team_group_id: Option<String>,
    pub team_lead_workspace_id: String,
    pub team_lead_handle: Option<String>,
    pub team_name: Option<String>,
    pub consumer_workspace_id: String,
    pub consumer_handle: Option<String>,
    pub kind: String,
    pub topology: String,
    pub scope: Option<serde_json::Value>,
    pub created_by_actor: String,
    pub api_key_id: Option<String>,
    /// App-asserted, not verified.
    pub on_behalf_of: Option<String>,
    pub status: String,
    pub created_at: i64,
}

const LINK_COLS: &str = "l.id, l.instance_id, l.team_group_id, l.team_lead_workspace_id, \
    (SELECT handle FROM projects WHERE id = l.team_lead_workspace_id), \
    (SELECT team_name FROM agent_instances WHERE id = l.instance_id), \
    l.consumer_workspace_id, (SELECT handle FROM projects WHERE id = l.consumer_workspace_id), \
    l.kind, l.topology, l.scope_json, l.created_by_actor, l.api_key_id, l.on_behalf_of, \
    CASE WHEN l.status = 'active' AND ( \
        NOT EXISTS (SELECT 1 FROM projects WHERE id = l.team_lead_workspace_id) OR \
        NOT EXISTS (SELECT 1 FROM projects WHERE id = l.consumer_workspace_id)) \
      THEN 'orphaned' ELSE l.status END, \
    l.created_at";

fn row_to_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<Link> {
    Ok(Link {
        id: r.get(0)?,
        instance_id: r.get(1)?,
        team_group_id: r.get(2)?,
        team_lead_workspace_id: r.get(3)?,
        team_lead_handle: r.get(4)?,
        team_name: r.get(5)?,
        consumer_workspace_id: r.get(6)?,
        consumer_handle: r.get(7)?,
        kind: r.get(8)?,
        topology: r.get(9)?,
        scope: r
            .get::<_, Option<String>>(10)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        created_by_actor: r.get(11)?,
        api_key_id: r.get(12)?,
        on_behalf_of: r.get(13)?,
        status: r.get(14)?,
        created_at: r.get(15)?,
    })
}

pub fn get(conn: &Connection, id: &str) -> Option<Link> {
    conn.query_row(&format!("SELECT {LINK_COLS} FROM team_links l WHERE l.id = ?1"), params![id], row_to_link)
        .optional()
        .ok()
        .flatten()
}

fn query(conn: &Connection, where_sql: &str, arg: &str) -> Vec<Link> {
    let sql = format!("SELECT {LINK_COLS} FROM team_links l WHERE {where_sql} ORDER BY l.created_at, l.rowid");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![arg], row_to_link)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Links where `workspace_id` is the consumer ("served by …").
pub fn links_out(conn: &Connection, workspace_id: &str) -> Vec<Link> {
    query(conn, "l.status = 'active' AND l.consumer_workspace_id = ?1", workspace_id)
}

/// Links where `workspace_id` is on the team side: the lead, or a member of
/// the linked team instance ("serves …").
pub fn links_in(conn: &Connection, workspace_id: &str) -> Vec<Link> {
    query(
        conn,
        "l.status = 'active' AND (l.team_lead_workspace_id = ?1 OR l.instance_id IN \
         (SELECT instance_id FROM agent_instance_members WHERE workspace_id = ?1 AND status = 'live'))",
        workspace_id,
    )
}

/// Every active link touching an instance (either side).
pub fn links_of_instance(conn: &Connection, instance_id: &str) -> Vec<Link> {
    query(
        conn,
        "l.status = 'active' AND (l.instance_id = ?1 OR l.consumer_workspace_id IN \
         (SELECT workspace_id FROM agent_instance_members WHERE instance_id = ?1 AND status = 'live'))",
        instance_id,
    )
}

/// Active links made with one API key.
pub fn links_of_key(conn: &Connection, api_key_id: &str) -> Vec<Link> {
    query(conn, "l.status = 'active' AND l.api_key_id = ?1", api_key_id)
}

/// A link to create (already authorized by the caller's door).
#[derive(Debug, Clone)]
pub struct NewLink<'a> {
    pub instance_id: Option<&'a str>,
    pub team_group_id: Option<&'a str>,
    pub team_lead: &'a str,
    /// Other live members of the team (for `mesh`).
    pub team_members: Vec<String>,
    pub consumer: &'a str,
    pub kind: &'static str,
    pub topology: &'static str,
    pub scope: Option<&'a serde_json::Value>,
    pub on_behalf_of: Option<&'a str>,
}

/// Write a link and its relation rows. Returns `(link, created)`; an
/// identical active link (same lead, consumer, kind) is returned as is.
pub fn create(conn: &Connection, caller: &HireCaller, nl: &NewLink<'_>) -> Result<(Link, bool), HireError> {
    if nl.team_lead == nl.consumer || nl.team_members.iter().any(|m| m == nl.consumer) {
        return Err(HireError::usage("a team can't be linked to one of its own members"));
    }
    if let Ok(existing) = conn.query_row(
        "SELECT id FROM team_links WHERE status = 'active' AND team_lead_workspace_id = ?1 \
         AND consumer_workspace_id = ?2 AND kind = ?3",
        params![nl.team_lead, nl.consumer, nl.kind],
        |r| r.get::<_, String>(0),
    ) {
        let link = get(conn, &existing).ok_or_else(|| HireError::internal("link vanished"))?;
        return Ok((link, false));
    }
    let id = format!("lnk_{}", uuid::Uuid::new_v4().simple());
    let scope = nl.scope.map(|s| s.to_string());
    conn.execute(
        "INSERT INTO team_links (id, instance_id, team_group_id, team_lead_workspace_id, consumer_workspace_id, \
         kind, topology, scope_json, created_by_actor, api_key_id, on_behalf_of, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'active', ?12)",
        params![
            id,
            nl.instance_id,
            nl.team_group_id,
            nl.team_lead,
            nl.consumer,
            nl.kind,
            nl.topology,
            scope,
            caller.actor(),
            caller.api_key_id(),
            nl.on_behalf_of,
            now_secs()
        ],
    )
    .map_err(|e| HireError::internal(format!("link: {e}")))?;
    let rel = relation_type_for(nl.kind);
    let mut targets: Vec<&str> = vec![nl.team_lead];
    if nl.topology == "mesh" {
        targets.extend(nl.team_members.iter().map(String::as_str));
    }
    for t in targets {
        insert_edge(conn, nl.consumer, t, rel, &id)?;
    }
    let link = get(conn, &id).ok_or_else(|| HireError::internal("link vanished"))?;
    Ok((link, true))
}

/// One managed relation row (`origin_link_id` set).
pub fn insert_edge(conn: &Connection, source: &str, target: &str, rel: &str, origin: &str) -> Result<(), HireError> {
    conn.execute(
        "INSERT OR IGNORE INTO workspace_relations (id, source_project_id, target_project_id, relation_type, created_at, origin_link_id) \
         VALUES (?1, ?2, ?3, ?4, unixepoch(), ?5)",
        params![uuid::Uuid::new_v4().to_string(), source, target, rel, origin],
    )
    .map_err(|e| HireError::internal(format!("relation: {e}")))?;
    Ok(())
}

/// Remove a link: delete exactly its relation rows, mark it removed.
pub fn remove(conn: &Connection, link_id: &str) -> Result<bool, HireError> {
    conn.execute("DELETE FROM workspace_relations WHERE origin_link_id = ?1", params![link_id])
        .map_err(|e| HireError::internal(format!("unlink edges: {e}")))?;
    let n = conn
        .execute(
            "UPDATE team_links SET status = 'removed', removed_at = ?2 WHERE id = ?1 AND status != 'removed'",
            params![link_id, now_secs()],
        )
        .map_err(|e| HireError::internal(format!("unlink: {e}")))?;
    Ok(n > 0)
}

/// Write a team's internal edges at hire (`star`: lead ↔ each member;
/// `mesh`: every pair).
pub fn write_team_edges(
    conn: &Connection,
    instance_id: &str,
    lead: &str,
    members: &[String],
    topology: &str,
) -> Result<usize, HireError> {
    let origin = format!("{INSTANCE_ORIGIN_PREFIX}{instance_id}");
    let mut n = 0;
    match topology {
        "none" => {}
        "mesh" => {
            let mut all: Vec<&str> = vec![lead];
            all.extend(members.iter().map(String::as_str));
            for i in 0..all.len() {
                for j in (i + 1)..all.len() {
                    insert_edge(conn, all[i], all[j], "team", &origin)?;
                    n += 1;
                }
            }
        }
        _ => {
            for m in members {
                insert_edge(conn, lead, m, "team", &origin)?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// The link or instance that manages a relation row, if any.
pub fn managing_origin(conn: &Connection, relation_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT origin_link_id FROM workspace_relations WHERE id = ?1",
        params![relation_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

/// Teaching text for a refused manual removal of a managed edge.
pub fn managed_hint(origin: &str) -> String {
    if let Some(inst) = origin.strip_prefix(INSTANCE_ORIGIN_PREFIX) {
        format!(
            "this connection belongs to the hired team {inst}; it goes away when the team is retired (k2 agent instances retire {inst})"
        )
    } else {
        format!("this connection belongs to team link {origin}; remove the link instead (k2 agent team unlink {origin})")
    }
}
