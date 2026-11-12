//! A tier-0 doc's new version, told to every client
//! (`doc_version_changed` on the session bus): a tab with the file open
//! re-reads its bytes (the 3D model, image, PDF and other any-file
//! previews). Text tabs follow their file by their own read.
//!
//! The kit has no event for a blob version, so K2 tells it where one is
//! made:
//! - `write` and `restore` on `/cli/docs/*` ([`super::tier0`]), when a
//!   version was recorded (equal bytes record nothing);
//! - the disk sync ([`super::watch`]): another program's save that became a
//!   version ([`disk_version`]: ingested, a conflict or a merge).
//!
//! The kit's own write to the file comes back to the watcher as an echo
//! (`OwnWrite`), so each version is told once. Off the session lock;
//! metadata only (the workspace, the path, the number).

use k2_mpc_base::session::DiskCheck;
use k2_mpc_base::store::{DocKey, VersionId};

/// The version an outside save made, if the disk check recorded one.
pub fn disk_version(check: &DiskCheck) -> Option<VersionId> {
    match check {
        DiskCheck::Ingested { version } | DiskCheck::Conflict { version, .. } | DiskCheck::Merged { version } => Some(*version),
        _ => None,
    }
}

/// The event for `key` at `version`, with the workspace's folder (a tab
/// matches `workspace` + `rel` to its file).
pub fn event(key: &DocKey, workspace: String, version: VersionId) -> crate::session_events::SessionEvent {
    crate::session_events::SessionEvent::DocVersionChanged {
        project_id: key.space.clone(),
        workspace,
        rel: key.path.clone(),
        version,
    }
}

/// Tell every client `key`'s doc is at `version` now. A workspace K2 no
/// longer knows has no open tab to tell.
pub fn announce(key: &DocKey, version: VersionId) {
    let workspace = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, &key.space).ok().map(|p| p.path)
    };
    let Some(workspace) = workspace else {
        k2_core::log_debug!("[docs] {}: version {version}, but its workspace is gone; nobody told", super::host::key_label(key));
        return;
    };
    let _ = crate::session_events::emit(event(key, workspace, version));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_a_save_that_became_a_version_is_told() {
        assert_eq!(disk_version(&DiskCheck::Ingested { version: 3 }), Some(3));
        assert_eq!(disk_version(&DiskCheck::Conflict { version: 5, kept: 4, copy: Some(PathBuf::from("a (copy).obj")) }), Some(5));
        assert_eq!(disk_version(&DiskCheck::Merged { version: 6 }), Some(6));
        assert_eq!(disk_version(&DiskCheck::Unchanged), None);
        assert_eq!(disk_version(&DiskCheck::OwnWrite), None, "the kit's own write was told by its route");
        assert_eq!(disk_version(&DiskCheck::Missing), None);
    }

    #[test]
    fn the_event_names_the_workspace_the_path_and_the_version() {
        let key = DocKey { space: "p1".into(), path: "play/house.obj".into() };
        let v = serde_json::to_value(event(&key, "/srv/acme".into(), 4)).expect("json");
        assert_eq!(
            v,
            serde_json::json!({"kind": "doc_version_changed", "projectId": "p1", "workspace": "/srv/acme", "rel": "play/house.obj", "version": 4})
        );
    }
}
