//! Backups over mgmt (SW15: S6 G9, S29 G6) so K2 can make, list and ship bundles off the box
//! as user `k2` (bundle files are `sew 0600`).
//!
//! - `POST /mgmt/v1/backup` (body `{}`, `Idempotency-Key`) → `StartJobResponse`: job kind
//!   [`BACKUP_JOB_KIND`], bundle kind `manual`, S6's engine in-process. One advisory lock
//!   (`LOCK_BACKUP`) serializes it with the CLI and timer path (`sew backup now`,
//!   `sew-backup@.service`): while another backup holds it the route answers `409 conflict`
//!   `backup_running`; a job that loses the race fails with that reason. A second `POST` while
//!   a backup job is queued or running answers the same job. The job's `result` is a
//!   [`BackupJobResult`]. `503 unavailable` `backups_unavailable` when this server has no
//!   backup engine (a test server, or `sew serve` without S6's config).
//! - `GET /mgmt/v1/backups` → [`MgmtBackupList`]: every complete bundle, newest first.
//! - `GET /mgmt/v1/backups/{id}/archive`: the S6 §17.2 stable tar of one bundle
//!   (`manifest.json`, `db.dump`, `config/sew.toml`, `blobs.index`, then `blobs/…` sorted;
//!   owner 0:0, mode 0600, mtime the bundle's time), streamed as `application/x-tar` with
//!   `Content-Length` set; the same bundle is byte-equal every time. `id` may be `latest`.
//!   `404 not_found` for an unknown id.
//!
//! `GET /mgmt/v1/status` fills `backup` and `drill` from S6's `state.json` and
//! `drill-state.json`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// The job kind of `POST /mgmt/v1/backup`.
pub const BACKUP_JOB_KIND: &str = "backup";

/// One bundle in `GET /mgmt/v1/backups` (S6 §17.2's `sew backup list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtBackup {
    /// The bundle id, `YYYYMMDDTHHMMSSZ`.
    pub id: String,
    /// Why it was made: `scheduled`, `manual`, `pre-upgrade` or `pre-restore`.
    pub kind: String,
    /// When it was made.
    pub created_at: Timestamp,
    /// Dump and blob bytes.
    pub bytes: u64,
    /// The schema version in the dump.
    pub schema_version: i64,
    /// The Sew version that made it.
    pub sew_version: String,
}

/// Response of `GET /mgmt/v1/backups`: newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtBackupList {
    /// The bundles.
    pub items: Vec<MgmtBackup>,
}

/// `JobStatus.result` of a succeeded `backup` job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupJobResult {
    /// The new bundle's id.
    pub id: String,
    /// Dump and blob bytes.
    pub bytes: u64,
    /// Wall time.
    pub duration_ms: u64,
    /// Blob files hard-linked.
    pub linked: u64,
    /// Blob files copied.
    pub copied: u64,
    /// Bundle ids retention pruned afterwards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pruned: Vec<String>,
}
