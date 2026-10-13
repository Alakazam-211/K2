//! The hire ledger (0160): `agent_instances`, `agent_instance_members`,
//! `hire_events`. Durable, so quotas survive restarts and every hire, link,
//! retire and hire-key mint is on record with who did it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{now_secs, HireCaller, HireError};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceMember {
    pub workspace_id: String,
    pub role: String,
    pub handle: String,
    pub path: String,
    pub lead: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub api_key_id: Option<String>,
    pub key_label: Option<String>,
    pub idem_key: Option<String>,
    #[serde(skip)]
    pub request_hash: String,
    pub actor: String,
    /// App-asserted, not verified.
    pub on_behalf_of: Option<String>,
    pub group_id: Option<String>,
    pub team_name: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub members: Vec<InstanceMember>,
    #[serde(skip)]
    pub result_json: Option<String>,
}

fn row_to_instance(r: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    Ok(Instance {
        id: r.get(0)?,
        api_key_id: r.get(1)?,
        key_label: r.get(2)?,
        idem_key: r.get(3)?,
        request_hash: r.get(4)?,
        actor: r.get(5)?,
        on_behalf_of: r.get(6)?,
        group_id: r.get(7)?,
        team_name: r.get(8)?,
        status: r.get(9)?,
        reason: r.get(10)?,
        result_json: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
        members: Vec::new(),
    })
}

const INSTANCE_COLS: &str = "id, api_key_id, key_label, idem_key, request_hash, actor, on_behalf_of, \
     group_id, team_name, status, reason, result_json, created_at, updated_at";

pub fn members_of(conn: &Connection, instance_id: &str) -> Vec<InstanceMember> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT workspace_id, role, handle, path, lead, status FROM agent_instance_members \
         WHERE instance_id = ?1 ORDER BY lead DESC, created_at, rowid",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![instance_id], |r| {
        Ok(InstanceMember {
            workspace_id: r.get(0)?,
            role: r.get(1)?,
            handle: r.get(2)?,
            path: r.get(3)?,
            lead: r.get::<_, i64>(4)? != 0,
            status: r.get(5)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

pub fn get(conn: &Connection, id: &str) -> Option<Instance> {
    let mut inst = conn
        .query_row(
            &format!("SELECT {INSTANCE_COLS} FROM agent_instances WHERE id = ?1"),
            params![id],
            row_to_instance,
        )
        .optional()
        .ok()
        .flatten()?;
    inst.members = members_of(conn, &inst.id);
    Some(inst)
}

/// The instance a caller made under an idempotency key.
pub fn by_idem(conn: &Connection, api_key_id: Option<&str>, idem: &str) -> Option<Instance> {
    let mut inst = conn
        .query_row(
            &format!(
                "SELECT {INSTANCE_COLS} FROM agent_instances \
                 WHERE COALESCE(api_key_id, '') = ?1 AND idem_key = ?2"
            ),
            params![api_key_id.unwrap_or(""), idem],
            row_to_instance,
        )
        .optional()
        .ok()
        .flatten()?;
    inst.members = members_of(conn, &inst.id);
    Some(inst)
}

/// Instances, newest first. `api_key_id = Some` → that key's only.
pub fn list(conn: &Connection, api_key_id: Option<&str>, limit: usize) -> Vec<Instance> {
    let sql = match api_key_id {
        Some(_) => format!("SELECT {INSTANCE_COLS} FROM agent_instances WHERE api_key_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"),
        None => format!("SELECT {INSTANCE_COLS} FROM agent_instances WHERE ?1 = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"),
    };
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let rows: Vec<Instance> = stmt
        .query_map(params![api_key_id.unwrap_or(""), limit as i64], row_to_instance)
        .map(|r| r.flatten().collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|mut i| {
            i.members = members_of(conn, &i.id);
            i
        })
        .collect()
}

/// Insert the `applying` row (claims the idempotency key).
pub fn insert_applying(
    conn: &Connection,
    id: &str,
    caller: &HireCaller,
    idem: Option<&str>,
    request_hash: &str,
    on_behalf_of: Option<&str>,
    team_name: Option<&str>,
    planned_members: usize,
) -> Result<(), HireError> {
    let now = now_secs();
    conn.execute(
        "INSERT INTO agent_instances (id, api_key_id, key_label, idem_key, request_hash, actor, \
         on_behalf_of, team_name, planned_members, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?10, 'applying', ?9, ?9)",
        params![
            id,
            caller.api_key_id(),
            caller.key_label(),
            idem,
            request_hash,
            caller.actor(),
            on_behalf_of,
            team_name,
            now,
            planned_members as i64
        ],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            HireError::new(409, "hire_in_progress", "a hire with this idempotency key is already running; retry in a moment")
        } else {
            HireError::internal(format!("ledger: {e}"))
        }
    })?;
    Ok(())
}

pub fn add_member(
    conn: &Connection,
    instance_id: &str,
    workspace_id: &str,
    role: &str,
    handle: &str,
    path: &str,
    lead: bool,
) -> Result<(), HireError> {
    conn.execute(
        "INSERT INTO agent_instance_members (instance_id, workspace_id, role, handle, path, lead, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'live', ?7)",
        params![instance_id, workspace_id, role, handle, path, lead as i64, now_secs()],
    )
    .map_err(|e| HireError::internal(format!("ledger member: {e}")))?;
    Ok(())
}

pub fn set_member_status(conn: &Connection, instance_id: &str, workspace_id: &str, status: &str) {
    let _ = conn.execute(
        "UPDATE agent_instance_members SET status = ?3 WHERE instance_id = ?1 AND workspace_id = ?2",
        params![instance_id, workspace_id, status],
    );
}

pub fn set_group(conn: &Connection, instance_id: &str, group_id: &str) {
    let _ = conn.execute(
        "UPDATE agent_instances SET group_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![instance_id, group_id, now_secs()],
    );
}

/// Final status. A `failed` hire releases its idempotency key (the same
/// key may try again) and keeps the row for audit.
pub fn finish(conn: &Connection, instance_id: &str, status: &str, reason: Option<&str>, result_json: Option<&str>) {
    let release = status == "failed";
    let _ = conn.execute(
        "UPDATE agent_instances SET status = ?2, reason = ?3, result_json = COALESCE(?4, result_json), \
         idem_key = CASE WHEN ?5 THEN NULL ELSE idem_key END, updated_at = ?6 WHERE id = ?1",
        params![instance_id, status, reason, result_json, release, now_secs()],
    );
}

/// Members this key created in the last 24 h (rolled-back ones count too:
/// their handles are burned, so they cost the box the same).
pub fn members_created_since(conn: &Connection, api_key_id: &str, since: i64) -> u32 {
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM agent_instance_members m JOIN agent_instances i ON i.id = m.instance_id \
                 WHERE i.api_key_id = ?1 AND m.created_at >= ?2 AND i.status != 'applying') \
              + (SELECT COALESCE(SUM(planned_members), 0) FROM agent_instances \
                 WHERE api_key_id = ?1 AND status = 'applying')",
        params![api_key_id, since],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u32)
    .unwrap_or(u32::MAX)
}

/// Live members this key made (fail closed: an error counts as "full").
pub fn live_members(conn: &Connection, api_key_id: &str) -> u32 {
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM agent_instance_members m JOIN agent_instances i ON i.id = m.instance_id \
                 WHERE i.api_key_id = ?1 AND m.status = 'live' AND i.status != 'applying') \
              + (SELECT COALESCE(SUM(planned_members), 0) FROM agent_instances \
                 WHERE api_key_id = ?1 AND status = 'applying')",
        params![api_key_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u32)
    .unwrap_or(u32::MAX)
}

/// The API key that created live workspace `workspace_id`, if any.
pub fn creator_key_of(conn: &Connection, workspace_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT i.api_key_id FROM agent_instance_members m JOIN agent_instances i ON i.id = m.instance_id \
         WHERE m.workspace_id = ?1 AND m.status = 'live' AND i.api_key_id IS NOT NULL \
         ORDER BY m.created_at DESC LIMIT 1",
        params![workspace_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// The live instance a workspace belongs to.
pub fn instance_of_workspace(conn: &Connection, workspace_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT instance_id FROM agent_instance_members WHERE workspace_id = ?1 AND status = 'live' \
         ORDER BY created_at DESC LIMIT 1",
        params![workspace_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// One audit row.
#[allow(clippy::too_many_arguments)]
pub fn event(
    conn: &Connection,
    action: &str,
    caller: &HireCaller,
    instance_id: Option<&str>,
    link_id: Option<&str>,
    on_behalf_of: Option<&str>,
    result: &str,
    reason: Option<&str>,
    detail: Option<&serde_json::Value>,
) {
    let _ = conn.execute(
        "INSERT INTO hire_events (at, action, actor, api_key_id, key_label, instance_id, link_id, on_behalf_of, result, reason, detail) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            now_secs(),
            action,
            caller.actor(),
            caller.api_key_id(),
            caller.key_label(),
            instance_id,
            link_id,
            on_behalf_of,
            result,
            reason,
            detail.map(|d| d.to_string()),
        ],
    );
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HireEvent {
    pub at: i64,
    pub action: String,
    pub actor: String,
    pub api_key_id: Option<String>,
    pub instance_id: Option<String>,
    pub link_id: Option<String>,
    /// App-asserted, not verified.
    pub on_behalf_of: Option<String>,
    pub result: String,
    pub reason: Option<String>,
}

pub fn events_for_instance(conn: &Connection, instance_id: &str) -> Vec<HireEvent> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT at, action, actor, api_key_id, instance_id, link_id, on_behalf_of, result, reason \
         FROM hire_events WHERE instance_id = ?1 ORDER BY id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![instance_id], |r| {
        Ok(HireEvent {
            at: r.get(0)?,
            action: r.get(1)?,
            actor: r.get(2)?,
            api_key_id: r.get(3)?,
            instance_id: r.get(4)?,
            link_id: r.get(5)?,
            on_behalf_of: r.get(6)?,
            result: r.get(7)?,
            reason: r.get(8)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Stable request hash for idempotency (sorted-key JSON, sha-256 hex).
pub fn request_hash(v: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    fn canon(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), canon(&m[k]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(canon).collect()),
            other => other.clone(),
        }
    }
    let mut h = Sha256::new();
    h.update(canon(v).to_string().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_ignores_key_order() {
        let a = serde_json::json!({"a": 1, "b": {"y": 2, "x": [1, {"q": 1, "p": 2}]}});
        let b = serde_json::json!({"b": {"x": [1, {"p": 2, "q": 1}], "y": 2}, "a": 1});
        assert_eq!(request_hash(&a), request_hash(&b));
        assert_ne!(request_hash(&a), request_hash(&serde_json::json!({"a": 2})));
    }
}
