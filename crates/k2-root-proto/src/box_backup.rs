//! The box's own backup client, `k2-backup` (restic, run nightly by
//! `k2-backup.timer`), as far as Sew's off-box copy needs it (protocol 13,
//! `namespaces.sewboxbackup`).
//!
//! `k2 hostchat ship box-backup` says the box backup carries Sew's bundles
//! (`/var/lib/sew/backups`) off the box. The daemon only believes it while
//! it can see the evidence: the client's path list covers that folder, its
//! exclude list doesn't drop it, and the timer is enabled. The files may be
//! root-only, so the helper's `engine sew box-backup` reads them: two fixed
//! files and one fixed unit, never the client's `env` (it holds the restic
//! password), and no path ever comes from the caller.
//!
//! The daemon judges the facts (`k2-daemon` `sew::box_backup`); this
//! module is only the fixed paths and the wire shape.

use serde::{Deserialize, Serialize};

/// The client's config folder.
pub const CONFIG_DIR: &str = "/etc/k2-backup";
/// What restic backs up (one path per line).
pub const PATHS_FILE: &str = "paths";
/// What restic skips (restic exclude patterns, one per line).
pub const EXCLUDES_FILE: &str = "excludes";
/// The nightly run.
pub const TIMER_UNIT: &str = "k2-backup.timer";
/// Sew's backup bundles: what the box backup has to carry.
pub const SEW_BACKUPS_DIR: &str = "/var/lib/sew/backups";
/// Each file is read up to this many bytes; a longer one is an error.
pub const CAP_FILE: usize = 64 * 1024;

/// One config file as read: its text, missing, or why it couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum FileRead {
    Text { text: String },
    Missing,
    Error { error: String },
}

/// `engine sew box-backup`'s answer (and what the daemon reads itself when
/// it may).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxBackupFacts {
    pub paths: FileRead,
    pub excludes: FileRead,
    /// `systemctl is-enabled k2-backup.timer`'s word (`enabled`,
    /// `disabled`, `masked`, `not-found`, …), or `unknown` when it said
    /// nothing.
    pub timer: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_shape_round_trips_and_names_its_states() {
        let f = BoxBackupFacts {
            paths: FileRead::Text { text: "/var/lib\n".into() },
            excludes: FileRead::Missing,
            timer: "enabled".into(),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["paths"], serde_json::json!({"state": "text", "text": "/var/lib\n"}));
        assert_eq!(v["excludes"], serde_json::json!({"state": "missing"}));
        assert_eq!(v["timer"], "enabled");
        let back: BoxBackupFacts = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
        let e: FileRead = serde_json::from_str(r#"{"state":"error","error":"not a regular file"}"#).unwrap();
        assert_eq!(e, FileRead::Error { error: "not a regular file".into() });
    }

    #[test]
    fn the_fixed_paths_never_name_the_client_env() {
        for p in [PATHS_FILE, EXCLUDES_FILE] {
            assert_ne!(p, "env", "the env file holds the restic password; never read it");
            assert!(!p.contains('/'), "{p}: a plain name under CONFIG_DIR");
        }
        assert!(SEW_BACKUPS_DIR.starts_with(crate::sew::SEW_DATA_DIR));
    }
}
