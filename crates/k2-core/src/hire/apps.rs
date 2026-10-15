//! App logins for hired agents (S5, after per-App logins S24). A hired
//! member may get a login on ONE published App: an App grant (never the
//! box login, S24-19), whole rooms only until folder grants land (T17),
//! marked as that workspace's agent. Room token `self` means the member's
//! own workspace.

use super::HireError;

/// The room token for "this member's own workspace".
pub const SELF_ROOM: &str = "self";

/// A published App: its service id, or `<workspace>/<publish name>`.
/// `host` (the box login) is never an App here. `None` = no such App.
pub fn resolve_app(raw: &str) -> Result<Option<String>, HireError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(HireError::usage("appLogin.app is empty"));
    }
    if t.eq_ignore_ascii_case(crate::skin::HOST_APP_ID) {
        return Err(HireError::usage(
            "appLogin.app can't be host (the box login): name a published App",
        ));
    }
    if let Some((ws, name)) = t.split_once('/') {
        let Ok(ids) = crate::skin::resolve_room_tokens(&[ws.trim().to_string()]) else {
            return Ok(None);
        };
        let Some(project_id) = ids.first() else { return Ok(None) };
        let db = crate::db::shared();
        let conn = db.lock();
        return crate::published_services::get_by_project_name(&conn, project_id, name.trim())
            .map(|r| r.map(|row| row.id))
            .map_err(|e| HireError::internal(format!("app lookup: {e}")));
    }
    let db = crate::db::shared();
    let conn = db.lock();
    crate::published_services::get_by_id(&conn, t)
        .map(|r| r.map(|row| row.id))
        .map_err(|e| HireError::internal(format!("app lookup: {e}")))
}

/// Room tokens → workspace ids, keeping `self`. Unknown → usage error.
pub fn resolve_rooms(tokens: &[String]) -> Result<Vec<String>, HireError> {
    let mut out: Vec<String> = Vec::new();
    for raw in tokens {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let id = if t.eq_ignore_ascii_case(SELF_ROOM) {
            SELF_ROOM.to_string()
        } else {
            crate::skin::resolve_room_tokens(&[t.to_string()])
                .map_err(|e| HireError::usage(format!("room {t:?}: {e}")))?
                .into_iter()
                .next()
                .ok_or_else(|| HireError::usage(format!("room {t:?}: unknown workspace")))?
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// What Settings → API Keys can pick for a hire key's App access limit:
/// every published App on this box (with the roles usable on it and the
/// rooms it already uses), and every room (workspace), `self` first. Owner
/// route only; read-only.
pub fn ceiling_options() -> Result<serde_json::Value, HireError> {
    let (services, rooms) = {
        let db = crate::db::shared();
        let conn = db.lock();
        let services = crate::published_services::list_all(&conn)
            .map_err(|e| HireError::internal(format!("app list: {e}")))?;
        let mut stmt = conn
            // Not the internal audit sentinels (`_orphan`, `_broadcast`).
            .prepare(
                "SELECT id, COALESCE(handle, ''), name FROM projects \
                 WHERE id NOT IN ('_orphan', '_broadcast') ORDER BY name COLLATE NOCASE",
            )
            .map_err(|e| HireError::internal(format!("room list: {e}")))?;
        let rooms: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| HireError::internal(format!("room list: {e}")))?
            .filter_map(Result::ok)
            .collect();
        (services, rooms)
    };
    let grants = crate::skin::list_grants().map_err(HireError::internal)?;
    let handle_of = |id: &str| rooms.iter().find(|r| r.0 == id).map(|r| r.1.clone()).unwrap_or_default();
    let mut apps = Vec::new();
    for s in services {
        let roles = crate::skin::roles_for_app(&s.id).map_err(HireError::internal)?;
        let mut used: Vec<String> = vec![s.project_id.clone()];
        let mut add = |id: &str| {
            if !id.is_empty() && !used.iter().any(|u| u == id) {
                used.push(id.to_string());
            }
        };
        for r in roles.iter().filter(|r| r.app_id.as_deref() == Some(s.id.as_str())) {
            r.rooms.iter().for_each(|id| add(id));
        }
        for g in grants.iter().filter(|g| g.kind == "app" && g.target_id == s.id && g.role_id.is_none()) {
            let scope: Vec<String> = g.scope.as_deref().and_then(|x| serde_json::from_str(x).ok()).unwrap_or_default();
            scope.iter().for_each(|id| add(id));
        }
        apps.push(serde_json::json!({
            "id": s.id,
            "name": s.name,
            "kind": s.kind,
            "workspaceId": s.project_id,
            "workspace": handle_of(&s.project_id),
            "roomIds": used,
            "roles": roles.iter().map(|r| serde_json::json!({
                "id": r.id,
                "name": r.name,
                "appId": r.app_id,
                "rooms": r.rooms,
            })).collect::<Vec<_>>(),
        }));
    }
    let mut room_list = vec![serde_json::json!({
        "id": SELF_ROOM,
        "handle": SELF_ROOM,
        "name": "The hired agent's own workspace",
    })];
    room_list.extend(rooms.iter().map(|(id, handle, name)| serde_json::json!({ "id": id, "handle": handle, "name": name })));
    Ok(serde_json::json!({ "apps": apps, "rooms": room_list }))
}

/// A 24-character random password (base62, OS CSPRNG), shown once.
pub fn random_password() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    const A: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(24);
    let mut scratch = [0u8; 64];
    while out.len() < 24 {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b < 248 && out.len() < 24 {
                out.push(A[(b % 62) as usize] as char);
            }
        }
    }
    out
}
