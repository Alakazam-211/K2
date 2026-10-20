//! Legacy `k2 publish run --cmd` services (0.46.3): the ones no wall runs
//! yet. They keep running exactly as before until 0.46.4
//! (`k2_core::app_grace::CUTOVER_RELEASE`); every list says so, with where
//! to read how an agent moves one to an App itself (`k2 study app-move`).

use serde_json::{json, Value as J};

use k2_core::published_services as ps;

use super::routes::{ws_handle, Who};

/// Is this row a legacy `--cmd` service (not an App, not `--skin`)?
pub fn is_legacy(r: &ps::PublishedService) -> bool {
    r.kind != ps::KIND_SKIN && r.cmd != ps::CMD_APP_SENTINEL && r.cmd != ps::CMD_SKIN_SENTINEL
}

/// The legacy services a caller can see (a passport or shell tab: its own
/// workspace's), labelled, for `k2 app list` and the doctor.
pub(crate) fn services(who: &Who) -> Vec<J> {
    let own = match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.clone()),
        _ => None,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_all(&conn).unwrap_or_default()
    };
    rows.into_iter()
        .filter(is_legacy)
        .filter(|r| own.as_deref().is_none_or(|w| w == r.project_id))
        .map(|r| {
            json!({
                "name": r.name,
                "serviceId": r.id,
                "workspace": r.project_id,
                "workspaceHandle": ws_handle(&r.project_id),
                "desired": r.desired,
                "label": k2_core::app_grace::legacy_label(),
                "hint": k2_core::app_grace::LEGACY_HINT,
                "labelMovedTo": super::labels::moved_to(&r.id),
            })
        })
        .collect()
}
