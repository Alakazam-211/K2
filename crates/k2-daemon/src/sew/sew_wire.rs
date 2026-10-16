//! TEMP — replace with vendored sew-proto (K1).
//!
//! The slice of Sew's mgmt wire the S29 supervisor reads, mirrored by hand
//! from `sew-proto` tag `sew-proto-v0.1.0-sw2` (Sew main `901b20c7`,
//! `crates/sew-proto/src/mgmt.rs` and `mgmt/backups.rs`): SW15's backups
//! over mgmt, the job status the backup job reports through, and the
//! `status.backup` / `status.drill` blocks. Same field names and
//! optionality as Sew's types. Deliberate differences, like S27's
//! `tool_hook/sew_wire.rs`: ids and timestamps are `String`, and no
//! `deny_unknown_fields` on answers (Sew may add fields).
//!
//! Routes (Sew `docs/mgmt.md`):
//! - `POST /mgmt/v1/backup` (body `{}`, `Idempotency-Key`) →
//!   [`StartJobResponse`]; job kind [`BACKUP_JOB_KIND`]. `409 conflict`
//!   [`BACKUP_RUNNING`] while another backup holds the lock; `503`
//!   [`BACKUPS_UNAVAILABLE`] without a backup engine.
//! - `GET /mgmt/v1/jobs/{id}` → [`JobStatus`]; a succeeded backup job's
//!   `result` is a [`BackupJobResult`].
//! - `GET /mgmt/v1/backups` → [`MgmtBackupList`], newest first.
//! - `GET /mgmt/v1/backups/{id|latest}/archive` → `application/x-tar`
//!   (not read by K2 yet: off-box shipping is S32).
//!
//! Sew has no mgmt restore: restore stays `sew restore` through the root
//! helper.

use serde::{Deserialize, Serialize};

/// The job kind of `POST /mgmt/v1/backup`.
pub const BACKUP_JOB_KIND: &str = "backup";
/// SW15: another backup holds the backup lock (409).
pub const BACKUP_RUNNING: &str = "backup_running";
/// SW15: this server has no backup engine (503).
pub const BACKUPS_UNAVAILABLE: &str = "backups_unavailable";

/// One bundle in `GET /mgmt/v1/backups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MgmtBackup {
    /// `YYYYMMDDTHHMMSSZ`.
    pub id: String,
    /// `scheduled`, `manual`, `pre-upgrade` or `pre-restore`.
    pub kind: String,
    pub created_at: String,
    pub bytes: u64,
    pub schema_version: i64,
    pub sew_version: String,
}

/// `GET /mgmt/v1/backups`: newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MgmtBackupList {
    pub items: Vec<MgmtBackup>,
}

/// `JobStatus.result` of a succeeded `backup` job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupJobResult {
    pub id: String,
    pub bytes: u64,
    pub duration_ms: u64,
    pub linked: u64,
    pub copied: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pruned: Vec<String>,
}

/// Response of the job-starting routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartJobResponse {
    pub job_id: String,
}

/// A job's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Dead,
}

impl JobState {
    /// `succeeded` or `dead`. A `failed` job may still retry.
    pub fn is_final(self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Dead)
    }
}

/// `GET /mgmt/v1/jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: String,
    pub kind: String,
    pub state: JobState,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// `status.backup` (contracts rev 21; SW15 adds `last_kind`, `last_error_at`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBackup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
}

/// `status.drill` (contracts rev 21; SW15 adds `age_ms`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDrill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

/// Sew's backup / drill alarms (contracts rev 21; SW15 fills them).
pub const BACKUP_ALARMS: &[&str] = &["backup_failed", "backup_stale", "drill_failed", "drill_stale"];

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture shapes from Sew's `docs/mgmt.md` (SW15) parse.
    #[test]
    fn sew_sw15_shapes_parse() {
        let l: MgmtBackupList = serde_json::from_str(
            r#"{"items":[{"id":"20261016T120000Z","kind":"manual","created_at":"2026-10-16T12:00:00Z","bytes":4096,"schema_version":920002,"sew_version":"0.1.0"}]}"#,
        )
        .unwrap();
        assert_eq!(l.items[0].kind, "manual");
        let j: JobStatus = serde_json::from_str(
            r#"{"id":"98765","kind":"backup","state":"succeeded","attempts":1,"created_at":"2026-10-16T12:00:00Z","finished_at":"2026-10-16T12:00:01Z","result":{"id":"20261016T120000Z","bytes":4096,"duration_ms":812,"linked":0,"copied":3}}"#,
        )
        .unwrap();
        assert!(j.state.is_final());
        let r: BackupJobResult = serde_json::from_value(j.result.unwrap()).unwrap();
        assert_eq!(r.id, "20261016T120000Z");
        assert!(r.pruned.is_empty());
        assert!(!serde_json::from_str::<JobState>(r#""failed""#).unwrap().is_final());
        let b: StatusBackup = serde_json::from_str(r#"{"last_ok_at":"2026-10-16T12:00:00Z","last_id":"x","age_ms":5,"last_kind":"manual","future":1}"#).unwrap();
        assert_eq!(b.last_kind.as_deref(), Some("manual"));
        let d: StatusDrill = serde_json::from_str("{}").unwrap();
        assert_eq!(d, StatusDrill::default());
        let s: StartJobResponse = serde_json::from_str(r#"{"job_id":"42"}"#).unwrap();
        assert_eq!(s.job_id, "42");
    }
}
