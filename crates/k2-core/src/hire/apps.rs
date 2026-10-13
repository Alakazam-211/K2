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
