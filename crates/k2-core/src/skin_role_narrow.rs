//! Narrowing a box-wide role (no App) to one App (0.46.18 papercut 3:
//! `k2 app role update <role> --app <app>`), refused while anyone holds the
//! role outside that App.

use rusqlite::params;

use super::{attach_role_handles, guest_rooms, role_by_id_or_name, with_conn, SkinRole, HOST_APP_ID};

/// Why [`narrow_role_to_app`] changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarrowRefused {
    /// No role by that id or name.
    Unknown,
    /// The role already belongs to another App (its service id).
    OtherApp(String),
    /// Logins that hold the role outside `app` (a box login, another App's
    /// grant, or the role as their login role): sorted usernames.
    HeldElsewhere(Vec<String>),
    /// The database said no.
    Db(String),
}

/// Narrow a box-wide role (no App) to one App, `app_id` (a service id).
/// Refused while anyone holds it outside that App: the box login's grant,
/// another App's grant, or the role as a login's own role (it applies
/// everywhere). Already that App's: unchanged, `Ok`. One transaction: the
/// check and the write see the same rows.
pub fn narrow_role_to_app(id_or_name: &str, app_id: &str) -> Result<SkinRole, NarrowRefused> {
    let app_id = app_id.trim();
    if app_id.is_empty() || app_id == HOST_APP_ID {
        return Err(NarrowRefused::Db("name one App".to_string()));
    }
    let role = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role begin: {e}"))?;
        let Some(mut role) = role_by_id_or_name(&tx, id_or_name.trim())? else {
            return Ok(Err(NarrowRefused::Unknown));
        };
        match role.app_id.as_deref() {
            Some(a) if a == app_id => return Ok(Ok(role)),
            Some(a) => return Ok(Err(NarrowRefused::OtherApp(a.to_string()))),
            None => {}
        }
        let mut holders: Vec<String> = {
            let mut st = tx
                .prepare(
                    "SELECT DISTINCT COALESCE(p.username, g.subject_kind || ':' || g.subject_id)
                       FROM grants g LEFT JOIN principals p ON g.subject_kind = 'principal' AND p.id = g.subject_id
                      WHERE g.role_id = ?1 AND NOT (g.kind = 'app' AND g.target_id = ?2)
                     UNION
                     SELECT username FROM principals WHERE role_id = ?1",
                )
                .map_err(|e| format!("skin role holders: {e}"))?;
            let rows = st.query_map(params![role.id, app_id], |r| r.get::<_, String>(0)).map_err(|e| format!("skin role holders: {e}"))?;
            rows.collect::<Result<_, _>>().map_err(|e| format!("skin role holders: {e}"))?
        };
        if !holders.is_empty() {
            holders.sort();
            holders.dedup();
            return Ok(Err(NarrowRefused::HeldElsewhere(holders)));
        }
        tx.execute("UPDATE roles SET app_id = ?1 WHERE id = ?2", params![app_id, role.id]).map_err(|e| format!("skin role narrow: {e}"))?;
        guest_rooms::note_role_conn(&tx, &role.id, Some(app_id))?;
        tx.commit().map_err(|e| format!("skin role commit: {e}"))?;
        role.app_id = Some(app_id.to_string());
        Ok(Ok(role))
    })
    .map_err(NarrowRefused::Db)??;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}
