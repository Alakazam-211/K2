//! Management API bodies and act-as headers (C9; gaps G17–G21, G31).
//!
//! HTTP over the Unix socket `/run/sew/mgmt.sock` with `Bearer sewsvc_…`. `Idempotency-Key` is
//! required on every mgmt `POST`.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::{Addr, Handle};
use crate::api::members::{AgentKind, MemberKind};
use crate::changes::NotificationReason;
use crate::error::{Validate, ValidationError, check_chars, check_known, check_opt_chars};
use crate::external_id::{ExternalId, is_lower_uuid};
use crate::http::Patch;
use crate::id::{Id, parse_id_str};
use crate::limits::{
    DELIVERIES_LONG_POLL_MAX_MS, DISPLAY_NAME_MAX_CHARS, EMAIL_MAX_BYTES, K2_SESSION_ID_MAX_LEN,
    PRONOUNS_MAX_CHARS, TITLE_MAX_CHARS,
};
use crate::time::Timestamp;

mod backups;
pub mod local;
mod meet;
mod ops;
mod spaces;

pub use backups::{BACKUP_JOB_KIND, BackupJobResult, MgmtBackup, MgmtBackupList};
pub use meet::{MEET_RETENTION_DAYS_MAX, MeetInitRequest, MeetInitResponse};
pub use ops::{
    AuditVerifyMode, AuditVerifyReport, AuditVerifyRequest, EXPORT_RECIPIENTS_MAX, ExportRequest,
};
pub use spaces::{
    APP_ID_MAX_BYTES, MgmtSpaceBindRequest, MgmtSpaceCreateRequest, MgmtSpaceMemberRef,
    MgmtSpaceMemberRemoveRequest, MgmtSpaceMemberRemoveResponse, MgmtSpaceMembersAddRequest,
    MgmtSpaceMembersAddResponse, MgmtSpaceNotAdded, MgmtSpaceNotAddedReason, MgmtSpaceResponse,
    SPACE_MANAGER_ROLE_NAME, SPACE_MANAGERS_MAX, SPACE_MEMBERS_ADD_MAX, is_app_id,
};

// ---- act-as ----

/// The `Sew-Act-As` header: a member id (decimal) or an external id (contains `:`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ActAsTarget {
    /// A member id.
    Member(Id),
    /// An external id.
    External(ExternalId),
}

impl FromStr for ActAsTarget {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains(':') {
            return s
                .parse()
                .map(ActAsTarget::External)
                .map_err(|e: crate::external_id::ExternalIdError| e.to_string());
        }
        parse_id_str(s)
            .map(ActAsTarget::Member)
            .ok_or_else(|| format!("invalid Sew-Act-As {s:?}: expected a member id or external id"))
    }
}

impl fmt::Display for ActAsTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActAsTarget::Member(id) => fmt::Display::fmt(id, f),
            ActAsTarget::External(e) => fmt::Display::fmt(e, f),
        }
    }
}

/// Whether `s` is 1–128 chars of `[A-Za-z0-9_.-]` (a K2 session id, gap G31, rev 15).
pub fn is_k2_session_id(s: &str) -> bool {
    (1..=K2_SESSION_ID_MAX_LEN).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-')
}

/// The `Sew-Via` header, exactly `k2; agent=<uuid>; k2_session=<id>` (gap G31). Only
/// `system = k2` exists in v1.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SewVia {
    /// The agent's K2 workspace uuid.
    pub agent: String,
    /// The K2 session id, 1–128 chars of `[A-Za-z0-9_.-]`.
    pub k2_session: String,
}

impl FromStr for SewVia {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || format!("invalid Sew-Via {s:?}: expected k2; agent=<uuid>; k2_session=<id>");
        let rest = s.strip_prefix("k2; agent=").ok_or_else(err)?;
        let (agent, session) = rest.split_once("; k2_session=").ok_or_else(err)?;
        if !is_lower_uuid(agent) || !is_k2_session_id(session) {
            return Err(err());
        }
        Ok(SewVia {
            agent: agent.to_owned(),
            k2_session: session.to_owned(),
        })
    }
}

impl fmt::Display for SewVia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "k2; agent={}; k2_session={}",
            self.agent, self.k2_session
        )
    }
}

// ---- health and status ----

/// Response of `GET /mgmt/v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    /// Whether the process is live.
    pub ok: bool,
}

unit_enum! {
    /// The install's identity provider (C6).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ProviderKind {
        /// K2 grants create accounts; K2 signs the pass.
        K2 = "k2",
        /// Local users and passwords.
        Standalone = "standalone",
        /// A meeting-link session (Meet MC2a, CC1: session provider 3, made by redeeming a
        /// meeting link). Never an install's provider.
        Link = "link",
    }
}

/// Database status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusDb {
    /// Whether the database answers (`SELECT 1` within 1 s).
    pub ok: bool,
    /// Replica lag; absent in v1 (single node).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_ms: Option<u64>,
    /// Round trip of the `SELECT 1` probe; absent when it failed (S9 G14, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_ms: Option<u64>,
}

/// Job queue status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusJobs {
    /// Jobs waiting (not under a live lease).
    pub queued: u64,
    /// Jobs that exhausted their attempts.
    pub dead: u64,
    /// Jobs under a live lease (S9, additive).
    #[serde(default)]
    pub running: u64,
    /// `now − min(run_at)` over due, unleased jobs; absent if none (S9, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_due_age_ms: Option<u64>,
}

unit_enum! {
    /// A status alarm (S9 §6.2, contracts revs 11 and 21). K2 shows each one verbatim.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum StatusAlarm {
        /// `db.ok = false`.
        DbDown = "db_down",
        /// `jobs.dead > 0`.
        JobsDead = "jobs_dead",
        /// `keys.provider_ok = false`.
        KeysProviderDown = "keys_provider_down",
        /// `keys.current_month_ready = false`.
        KeysCurrentMonthMissing = "keys_current_month_missing",
        /// The keyring mirror is stale (or kind `none`).
        KeysMirrorStale = "keys_mirror_stale",
        /// A crypto-shred started but did not finish.
        KeysShredPending = "keys_shred_pending",
        /// Old key files for shredded key ids were found and refused.
        KeysResurrected = "keys_resurrected",
        /// Keys wait for a re-wrap but no retiring KEK is loaded.
        KeysRewrapStalled = "keys_rewrap_stalled",
        /// Search indexing trails writes by more than 60 s (S20 §15).
        SearchLagging = "search_lagging",
        /// The search index is missing and being rebuilt; searches answer 503 (S20 §15).
        SearchUnavailable = "search_unavailable",
        /// Three search index commits in a row failed (S20 §15).
        SearchCommitFailing = "search_commit_failing",
        /// Free disk under the search index is below its size plus 1 GiB (S20 §13).
        SearchDiskLow = "search_disk_low",
        /// The last good backup is older than 36 h (contracts rev 21; S6 sets it).
        BackupStale = "backup_stale",
        /// The last backup failed (contracts rev 21; S6 sets it).
        BackupFailed = "backup_failed",
        /// The last restore drill failed (contracts rev 21; S6 sets it).
        DrillFailed = "drill_failed",
        /// The last good restore drill is older than 8 days (contracts rev 21; S6 sets it).
        DrillStale = "drill_stale",
        /// GIF search is on but its provider key does not resolve or was refused (S16 §9.1).
        GifsUnconfigured = "gifs_unconfigured",
        /// A finite retention policy is set and no run finished `done` for 48 h (S22 §18).
        RetentionOverdue = "retention_overdue",
        /// Purged messages' search documents are still waiting after 24 h (S22 §10.5).
        RetentionSearchBacklog = "retention_search_backlog",
        /// Three or more mail sends in a row failed over at least 10 minutes (S21 §12.6).
        MailFailing = "mail_failing",
        /// `digest.enabled` is on but `[mail.smtp]` is not configured (S21 §11.1).
        DigestMailUnconfigured = "digest_mail_unconfigured",
        /// A running export wrote no progress for 10 minutes (S23 §16).
        ExportStuck = "export_stuck",
        /// Voice is enabled and the media engine has been unreachable for more than 60 s
        /// (S33 v2 §22).
        VoiceMediaDown = "voice_media_down",
        /// The snapshot audit found voice state Sew did not expect (S33 v2 §14.3).
        VoiceDrift = "voice_drift",
        /// The media engine reported a panic in the last hour (S33 v2 §22, S37 §13.2).
        MediaPanic = "media_panic",
        /// More than 3 call key group resets install-wide in the last hour, or a call in reset
        /// back-off (S50 §19.3).
        E2eeGroupResets = "e2ee_group_resets",
        /// A key removal was not activated by its hard bound in the last hour (S50 §19.3).
        E2eeRemovalOverdue = "e2ee_removal_overdue",
        /// A key group rebuild ended in a new group or a reset in the last hour (S50 §19.3).
        E2eeRebuildFailed = "e2ee_rebuild_failed",
        /// A client reported a roster mismatch in the last hour (S50 §19.3).
        E2eeRosterMismatch = "e2ee_roster_mismatch",
        /// A stale-slot resync or a `peer_undecryptable` quorum in the last hour (S50 §19.3).
        E2eeSenderStale = "e2ee_sender_stale",
        /// A client's bridge reported a misbehaving engine in the last hour (S50 §19.3).
        E2eeEngineAnomaly = "e2ee_engine_anomaly",
        /// Voice is enabled and the call DS or CA key failed to load (S50 §19.3).
        E2eeKeysUnavailable = "e2ee_keys_unavailable",
        /// The media budget tier has been `critical` for more than 5 min (S34 v2 §25.3);
        /// clears when the tier drops.
        MediaBudgetCritical = "media_budget_critical",
        /// The recorder failed health for 60 s (S35 v2 §22.3).
        RecorderDown = "recorder_down",
        /// The transcriber failed health for 60 s (S35 v2 §22.3).
        TranscriberDown = "transcriber_down",
        /// The oldest untranscribed segment is older than 30 min (S35 v2 §13.4).
        TranscribeBacklog = "transcribe_backlog",
        /// A recording stopped on `storage_full` in the last 24 h (S35 v2 §22.3).
        RecordingStorageFull = "recording_storage_full",
        /// Three auto-starts failed in 10 min for one channel (S35 v2 §8.3).
        RecordingAutoFailed = "recording_auto_failed",
        /// A segment of an ungranted identity reached `sew` in the last 24 h (S35 v2 §9.3).
        RecordingConsentViolation = "recording_consent_violation",
    }
}

/// Identity provider state in `status` (contracts rev 21, S29 G5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusIdentity {
    /// The provider.
    pub provider: ProviderKind,
    /// `identity.k2.jwks_url` (K2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
    /// Whether the JWKS cache holds keys (K2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_ok: Option<bool>,
}

/// Backup state in `status` (contracts rev 21, S6 G8). S6 fills it; absent until then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusBackup {
    /// The last good backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<Timestamp>,
    /// The last good backup's bundle id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
    /// Age of the last good backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    /// Why the last run failed, if it did (cleared by the next good one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// SW15: the last good backup's kind (`scheduled`, `manual`, …; additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_kind: Option<String>,
    /// SW15: when the last failed run failed (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<Timestamp>,
}

/// Restore-drill state in `status` (contracts rev 21, S6 G8). S6 fills it; absent until then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusDrill {
    /// The last good drill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<Timestamp>,
    /// Whether the last drill passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<bool>,
    /// Why the last drill failed, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// SW15: age of the last good drill (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

/// Search index status (S9; S20 §15 adds every field after `lag_ms`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusSearch {
    /// How far indexing trails writes: `now − min(created_at)` over pending or running
    /// `search_index` / `search_delete` jobs; 0 when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_ms: Option<u64>,
    /// `ready` or `missing` (rebuilding; searches answer 503).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SearchState>,
    /// Live documents in the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<StatusSearchDocs>,
    /// Bytes of the live index on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_bytes: Option<u64>,
    /// The live index generation (`gen-<n>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    /// When the indexer last committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit_at: Option<Timestamp>,
    /// Present only while a rebuild runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<StatusSearchRebuild>,
}

unit_enum! {
    /// Search index state (S20 §12.3).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SearchState {
        /// Queries use the live index.
        Ready = "ready",
        /// No usable index; a rebuild runs and searches answer `503 search_rebuilding`.
        Missing = "missing",
    }
}

/// Live search documents by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusSearchDocs {
    /// Message documents.
    pub messages: u64,
    /// File-name documents.
    pub files: u64,
}

/// A running search rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusSearchRebuild {
    /// Why: `marker`, `missing`, `corrupt`, `meta_mismatch` or `admin`.
    pub reason: String,
    /// The generation being built.
    pub generation: u64,
    /// 0–99 until the swap.
    pub progress_pct: u8,
    /// Documents in the building index so far.
    pub docs: u64,
    /// When it started.
    pub started_at: Timestamp,
}

/// Body of `POST /mgmt/v1/search/rebuild` (S20 §14.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchRebuildRequest {
    /// Free text for the operator, at most 200 bytes (never audited as content).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Validate for SearchRebuildRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(r) = &self.reason
            && r.len() > crate::limits::SEARCH_REBUILD_REASON_MAX_BYTES
        {
            return Err(ValidationError::invalid(
                "reason",
                format!(
                    "reason must be at most {} bytes",
                    crate::limits::SEARCH_REBUILD_REASON_MAX_BYTES
                ),
            ));
        }
        Ok(())
    }
}

/// Response of `POST /mgmt/v1/search/rebuild`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchRebuildResponse {
    /// The `search_rebuild` job (poll `GET /mgmt/v1/jobs/{id}`).
    pub job_id: Id,
}

/// Key provider and keyring status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusKeys {
    /// Whether the master key provider works.
    pub provider_ok: bool,
    /// Whether this month's keys exist.
    pub current_month_ready: bool,
    /// Whether the off-box keyring mirror is current (gap G21).
    pub mirror_ok: bool,
    /// Age of the last good mirror write; absent if never ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_age_ms: Option<u64>,
    /// The key provider kind: `systemd-credential`, `file` (contracts rev 3 G8, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Current KEK id, 16 hex (rev 3 G8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kek_id: Option<String>,
    /// Retiring KEK ids, 16 hex each (rev 3 G8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retiring_kek_ids: Vec<String>,
    /// Keyring entries still wrapped under a retiring KEK (rev 3 G8).
    #[serde(default)]
    pub rewrap_pending: u64,
    /// Whether next month's install key exists (rev 3 G8).
    #[serde(default)]
    pub next_month_prepared: bool,
    /// Keys created lazily at write time this month (rev 3 G8).
    #[serde(default)]
    pub lazy_creates_this_month: u64,
    /// Shreds started but not finished (rev 3 G8).
    #[serde(default)]
    pub shred_pending: u64,
    /// Old key files for shredded kids, refused at open (rev 3 G8).
    #[serde(default)]
    pub resurrected_keys: u64,
    /// The keyring mirror block (rev 3 G8); absent from servers that predate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<StatusKeysMirror>,
}

/// `status.keys.mirror` (contracts rev 3 G8, rev 42).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusKeysMirror {
    /// Mirror kind: `dir`, `none` (`k2` reserved).
    pub kind: String,
    /// Alarm: a pending write is older than the stale threshold, or kind `none`.
    pub stale: bool,
    /// Names waiting to be pushed.
    pub pending: u64,
    /// Age of the oldest pending name, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_pending_age_s: Option<u64>,
    /// Last successful mirror operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<Timestamp>,
    /// Mirror entries that are neither local nor on the ledger.
    pub unknown_keys: u64,
}

/// Response of `GET /mgmt/v1/status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    /// The Sew version.
    pub version: String,
    /// The applied schema version.
    pub schema_version: u64,
    /// The install id (gap G20).
    pub install_id: Id,
    /// The identity provider.
    pub provider: ProviderKind,
    /// Database status.
    pub db: StatusDb,
    /// Job queue status.
    pub jobs: StatusJobs,
    /// Search status.
    pub search: StatusSearch,
    /// Key status.
    pub keys: StatusKeys,
    /// Live login sessions.
    pub sessions: u64,
    /// Open gateway sockets.
    pub sockets: u64,
    /// When this process started (S9, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Time since this process started (S9, additive).
    #[serde(default)]
    pub uptime_ms: u64,
    /// Every active alarm, sorted (S9, additive). K2 shows them verbatim.
    #[serde(default)]
    pub alarms: Vec<StatusAlarm>,
    /// The identity provider's state (contracts rev 21, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<StatusIdentity>,
    /// The client listener as `http://<bound address>` (contracts rev 21, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_listen: Option<String>,
    /// Backups (contracts rev 21; S6 fills it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<StatusBackup>,
    /// Restore drills (contracts rev 21; S6 fills it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<StatusDrill>,
    /// Files (S15 G21, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<StatusFiles>,
    /// Retention runs, pending policies, holds and shred candidates (S22 §18, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<crate::api::compliance::StatusRetention>,
    /// Outgoing mail (S21 §12.6, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<StatusMail>,
    /// The email digest (S21 §12.6, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<StatusDigest>,
    /// Exports running, queued and on disk (S23 §16, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exports: Option<crate::api::exports::StatusExports>,
    /// Voice, the media engine and call keys (S33 v2 §22, S50 §19.2, additive). Absent while
    /// voice was never configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<crate::voice::VoiceStatus>,
    /// Recording and transcription (S35 v2 §22.3, additive). Absent while recording is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::StatusRecording>,
}

unit_enum! {
    /// How the SMTP connection is secured (`[mail.smtp] tls`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MailTls {
        /// Plain SMTP; loopback relays only (contracts rev 4).
        None = "none",
        /// STARTTLS, required.
        Starttls = "starttls",
        /// Implicit TLS.
        Tls = "tls",
    }
}

/// Outgoing mail in `status` (S21 §12.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusMail {
    /// `[mail.smtp]` is set.
    pub configured: bool,
    /// The connection security (absent when not configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<MailTls>,
    /// The last accepted send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<Timestamp>,
    /// The last failure (at most 200 characters, never an address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// When the last failure happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<Timestamp>,
    /// Sends accepted in the last 24 h.
    pub sent_24h: u64,
    /// Sends that failed in the last 24 h.
    pub failed_24h: u64,
}

/// The email digest in `status` (S21 §12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusDigest {
    /// Org policy `digest.enabled`.
    pub enabled: bool,
    /// Queued `email_digest` jobs.
    pub pending_jobs: u64,
}

/// Body of `POST /mgmt/v1/mail/test` (S21 §12.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailTestRequest {
    /// The recipient (never audited).
    pub to: String,
}

impl Validate for MailTestRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_email("to", &self.to)
    }
}

/// Response of `POST /mgmt/v1/mail/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailTestResponse {
    /// The relay accepted the mail.
    pub ok: bool,
    /// The relay's reply code, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_code: Option<u16>,
    /// Why it failed (at most 200 characters, never the address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// File state in `status` (S15 G21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusFiles {
    /// Whether `ffmpeg` answered `-version` at boot (video posters need it).
    pub ffmpeg_available: bool,
}

// ---- provisioning ----

fn check_email(field: &str, email: &str) -> Result<(), ValidationError> {
    if email.is_empty() || email.len() > EMAIL_MAX_BYTES || !email.contains('@') {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must be an address of at most {EMAIL_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// A profile at provisioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    /// Display name, 1–100 characters.
    pub display_name: String,
    /// Handle.
    pub username: Handle,
    /// Email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Title, at most 100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Pronouns, at most 40 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// Provider custom fields (S18): declarative, unlisted provider fields are cleared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_fields: Vec<crate::api::people::MgmtCustomFieldInput>,
}

impl Validate for ProfileInput {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars(
            "display_name",
            &self.display_name,
            1,
            DISPLAY_NAME_MAX_CHARS,
        )?;
        if let Some(e) = &self.email {
            check_email("email", e)?;
        }
        check_opt_chars("title", self.title.as_deref(), 0, TITLE_MAX_CHARS)?;
        check_opt_chars("pronouns", self.pronouns.as_deref(), 0, PRONOUNS_MAX_CHARS)
    }
}

/// Body of `POST /mgmt/v1/principals/provision`. Idempotent by `external_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionRequest {
    /// Who the member is outside Sew.
    pub external_id: ExternalId,
    /// Person or agent.
    pub kind: MemberKind,
    /// Required iff `kind = agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    /// The assistant's person; required iff `agent_kind = assistant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<ExternalId>,
    /// Provision as a guest; only for persons.
    #[serde(default)]
    pub guest: bool,
    /// The profile.
    pub profile: ProfileInput,
}

impl Validate for ProvisionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("kind", self.kind)?;
        if let Some(k) = self.agent_kind {
            check_known("agent_kind", k)?;
        }
        match self.kind {
            MemberKind::Agent => {
                let Some(agent_kind) = self.agent_kind else {
                    return Err(ValidationError::invalid(
                        "agent_kind",
                        "agent_kind is required for agents",
                    ));
                };
                if !self.external_id.is_agent() {
                    return Err(ValidationError::invalid(
                        "external_id",
                        "an agent's external_id must be k2:workspace:<uuid>",
                    ));
                }
                if self.guest {
                    return Err(ValidationError::invalid("guest", "agents cannot be guests"));
                }
                match (agent_kind, &self.assistant_of) {
                    (AgentKind::Assistant, None) => {
                        return Err(ValidationError::invalid(
                            "assistant_of",
                            "assistant_of is required for assistants",
                        ));
                    }
                    (AgentKind::Coworker, Some(_)) => {
                        return Err(ValidationError::invalid(
                            "assistant_of",
                            "assistant_of is only for assistants",
                        ));
                    }
                    _ => {}
                }
                if let Some(person) = &self.assistant_of
                    && person.is_agent()
                {
                    return Err(ValidationError::invalid(
                        "assistant_of",
                        "an assistant belongs to a person",
                    ));
                }
            }
            _ => {
                if self.agent_kind.is_some() {
                    return Err(ValidationError::invalid(
                        "agent_kind",
                        "agent_kind is only for agents",
                    ));
                }
                if self.assistant_of.is_some() {
                    return Err(ValidationError::invalid(
                        "assistant_of",
                        "assistant_of is only for assistants",
                    ));
                }
                if self.external_id.is_agent() {
                    return Err(ValidationError::invalid(
                        "external_id",
                        "a person's external_id must not be k2:workspace:*",
                    ));
                }
            }
        }
        self.profile.validate().map_err(|e| e.nested("profile"))
    }
}

/// Response of `POST /mgmt/v1/principals/provision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProvisionResponse {
    /// The member id.
    pub member_id: Id,
    /// Whether this call created the member.
    pub created: bool,
    /// Whether this call reactivated a deactivated member (S9 G6, additive).
    #[serde(default)]
    pub reactivated: bool,
    /// Whether an existing member's profile, guest flag, agent kind or pin changed (S9 G6,
    /// additive).
    #[serde(default)]
    pub changed: bool,
}

/// A partial profile update from K2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    /// A new display name, 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A new handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<Handle>,
    /// Set or clear the email.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub email: Patch<String>,
    /// Set or clear the title (≤ 100).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub title: Patch<String>,
    /// Set or clear the pronouns (≤ 40).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub pronouns: Patch<String>,
    /// Provider custom fields to set or clear (S18); unlisted keys are kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_fields: Vec<crate::api::people::MgmtCustomFieldInput>,
}

impl Validate for ProfilePatch {
    fn validate(&self) -> Result<(), ValidationError> {
        check_opt_chars(
            "display_name",
            self.display_name.as_deref(),
            1,
            DISPLAY_NAME_MAX_CHARS,
        )?;
        if let Some(e) = self.email.as_set() {
            check_email("email", e)?;
        }
        check_opt_chars(
            "title",
            self.title.as_set().map(String::as_str),
            0,
            TITLE_MAX_CHARS,
        )?;
        check_opt_chars(
            "pronouns",
            self.pronouns.as_set().map(String::as_str),
            0,
            PRONOUNS_MAX_CHARS,
        )
    }
}

/// Body of `POST /mgmt/v1/principals/update` (gap G17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePrincipalRequest {
    /// Who to update.
    pub external_id: ExternalId,
    /// What to change.
    pub profile: ProfilePatch,
}

impl Validate for UpdatePrincipalRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.profile.validate().map_err(|e| e.nested("profile"))
    }
}

/// Body of `POST /mgmt/v1/principals/deprovision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeprovisionRequest {
    /// Who to deactivate.
    pub external_id: ExternalId,
}

impl Validate for DeprovisionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response of `POST /mgmt/v1/principals/deprovision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeprovisionResponse {
    /// The deactivated member.
    pub member_id: Id,
    /// Sessions revoked.
    pub sessions_revoked: u64,
}

/// Body of `POST /mgmt/v1/sessions/revoke`: at least one of the two.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeSessionsRequest {
    /// Revoke every session of this principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<ExternalId>,
    /// Revoke sessions bound to this K2 login session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2_sid: Option<String>,
}

impl Validate for RevokeSessionsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.external_id.is_none() && self.k2_sid.is_none() {
            return Err(ValidationError::invalid(
                "external_id",
                "send external_id, k2_sid, or both",
            ));
        }
        if let Some(sid) = &self.k2_sid
            && !is_k2_session_id(sid)
        {
            return Err(ValidationError::invalid(
                "k2_sid",
                "k2_sid must be 1–128 chars of [A-Za-z0-9_.-]",
            ));
        }
        Ok(())
    }
}

/// Response of `POST /mgmt/v1/sessions/revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RevokeSessionsResponse {
    /// Sessions revoked.
    pub revoked: u64,
}

/// The current owner's inline approval of an owner transfer (S9 G7). `Debug` redacts the
/// secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnerApproval {
    /// K2 mode: a chat pass (C6) whose `sub` is the current owner.
    OwnerPass {
        /// The pass.
        pass: String,
    },
    /// Standalone: the current owner's password.
    OwnerPassword {
        /// The password.
        password: String,
    },
}

impl OwnerApproval {
    /// The wire `kind`.
    pub fn kind_str(&self) -> &'static str {
        match self {
            OwnerApproval::OwnerPass { .. } => "owner_pass",
            OwnerApproval::OwnerPassword { .. } => "owner_password",
        }
    }
}

impl fmt::Debug for OwnerApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OwnerApproval::OwnerPass { .. } => f
                .debug_struct("OwnerPass")
                .field("pass", &"<redacted>")
                .finish(),
            OwnerApproval::OwnerPassword { .. } => f
                .debug_struct("OwnerPassword")
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

/// Body of `POST /mgmt/v1/owner/set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerSetRequest {
    /// The new install owner.
    pub external_id: ExternalId,
    /// The current owner's approval; not needed while no owner exists (S9 G7, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<OwnerApproval>,
    /// Break-glass transfer: standalone only, and only for peer uid 0 (S9 G7, additive).
    #[serde(default)]
    pub force: bool,
}

impl Validate for OwnerSetRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.external_id.is_agent() {
            return Err(ValidationError::invalid(
                "external_id",
                "the install owner must be a person",
            ));
        }
        match &self.approval {
            Some(OwnerApproval::OwnerPass { pass }) if pass.is_empty() || pass.len() > 4_096 => {
                Err(ValidationError::invalid(
                    "approval.pass",
                    "approval.pass must be 1–4096 bytes",
                ))
            }
            Some(OwnerApproval::OwnerPassword { password })
                if password.is_empty() || password.len() > crate::limits::PASSWORD_MAX_BYTES =>
            {
                Err(ValidationError::invalid(
                    "approval.password",
                    "approval.password must be 1–1024 bytes",
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Response of `POST /mgmt/v1/owner/set` (S9 G7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OwnerSetResponse {
    /// The install owner after the call.
    pub owner_id: Id,
    /// The owner before the call, now an admin; absent on the first call or when unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_owner_id: Option<Id>,
    /// Whether the owner changed.
    pub changed: bool,
}

/// Body of `POST /mgmt/v1/agents/pin` and `unpin` (gap G19: external ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinRequest {
    /// The assistant, `k2:workspace:<uuid>`.
    pub agent: ExternalId,
    /// The person.
    pub person: ExternalId,
}

impl Validate for PinRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if !self.agent.is_agent() {
            return Err(ValidationError::invalid(
                "agent",
                "agent must be k2:workspace:<uuid>",
            ));
        }
        if self.person.is_agent() {
            return Err(ValidationError::invalid(
                "person",
                "person must be a person",
            ));
        }
        Ok(())
    }
}

/// Response of `POST /mgmt/v1/agents/pin` and `unpin` (S9 §6.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PinResponse {
    /// The assistant.
    pub agent_id: Id,
    /// The person it is pinned to; absent after unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<Id>,
    /// Whether the pin changed.
    pub changed: bool,
}

// ---- principals list ----

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

/// Query of `GET /mgmt/v1/principals` (S9 G5, contracts rev 17): pages by member id,
/// ascending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrincipalsQuery {
    /// Members with an id above this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–500 (default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only persons or only agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MemberKind>,
    /// Only this external id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<ExternalId>,
    /// Only this handle (exact match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<Handle>,
    /// Include deactivated members (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub include_deactivated: bool,
    /// Only compliance-flag holders (`true`) or only non-holders (`false`) (S23 §6.3, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance: Option<bool>,
    /// `link_guests` also lists meeting link guests (`link:*` members), which are left out by
    /// default so K2's reconcile never sees them (Meet MC2a, CC7, K-M6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<PrincipalsInclude>,
}

unit_enum! {
    /// `GET /mgmt/v1/principals?include=`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PrincipalsInclude {
        /// Meeting link guests (`link:*`, install role `link_guest`) too.
        LinkGuests = "link_guests",
    }
}

/// Body of `POST /mgmt/v1/principals/compliance` (S23 §6.3): grant or revoke the compliance
/// flag. Returns `ComplianceFlagResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrincipalComplianceRequest {
    /// The member.
    pub external_id: ExternalId,
    /// Grant (`true`) or revoke (`false`).
    pub compliance: bool,
}

impl Validate for PrincipalComplianceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Largest `GET /mgmt/v1/principals` page.
pub const PRINCIPALS_LIMIT_MAX: u32 = 500;
/// Default `GET /mgmt/v1/principals` page.
pub const PRINCIPALS_LIMIT_DEFAULT: u32 = 100;

impl PrincipalsQuery {
    /// The page size, default 100.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(PRINCIPALS_LIMIT_DEFAULT)
    }
}

impl Validate for PrincipalsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.limit
            && !(1..=PRINCIPALS_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PRINCIPALS_LIMIT_MAX}"),
            ));
        }
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        if let Some(i) = self.include {
            check_known("include", i)?;
        }
        Ok(())
    }
}

/// One principal in `GET /mgmt/v1/principals` (directory metadata only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtPrincipal {
    /// The member id.
    pub member_id: Id,
    /// Who the member is outside Sew.
    pub external_id: ExternalId,
    /// Person or agent.
    pub kind: MemberKind,
    /// For agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    /// For pinned assistants, the person's external id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<ExternalId>,
    /// Install role.
    pub install_role: crate::api::members::InstallRole,
    /// The handle.
    pub username: Handle,
    /// The display name.
    pub display_name: String,
    /// SW14: the email, as provisioned (absent when unset; additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// SW14: the title, as provisioned (absent when unset; additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// SW14: the pronouns, as provisioned (absent when unset; additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// Whether the member is deactivated.
    pub deactivated: bool,
    /// When the member was created.
    pub created_at: Timestamp,
    /// When the member was deactivated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deactivated_at: Option<Timestamp>,
}

/// Response of `GET /mgmt/v1/principals`.
pub type MgmtPrincipalPage = crate::http::Page<MgmtPrincipal>;

// ---- act-as and mgmt reasons ----

/// Whether `s` is a valid `Sew-Actor` header: 1–128 chars of `[A-Za-z0-9_.:@-]` (contracts
/// rev 13, S27 G8). It names the K2 person behind a mgmt call, for audit only.
pub fn is_sew_actor(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes().all(|c| {
            c.is_ascii_alphanumeric()
                || c == b'_'
                || c == b'.'
                || c == b':'
                || c == b'@'
                || c == b'-'
        })
}

/// `details.reason` values of mgmt and act-as errors (S9 §12, contracts revs 13 and 17).
/// Stable strings; no new error codes.
pub mod reasons {
    /// A peer uid outside `[mgmt] allowed_uids` (403).
    pub const PEER_UID_NOT_ALLOWED: &str = "peer_uid_not_allowed";
    /// Act-as headers on a `/mgmt/v1/*` route (400).
    pub const ACT_AS_ON_MGMT_ROUTE: &str = "act_as_on_mgmt_route";
    /// `/api/v1/*` on the mgmt socket without both act-as headers (400).
    pub const ACT_AS_HEADERS_REQUIRED: &str = "act_as_headers_required";
    /// An act-as header does not parse (400).
    pub const ACT_AS_HEADER_INVALID: &str = "act_as_header_invalid";
    /// Act-as headers on the TCP client listener (400).
    pub const ACT_AS_ONLY_ON_MGMT_SOCKET: &str = "act_as_only_on_mgmt_socket";
    /// A route act-as may never reach (403).
    pub const ACT_AS_ROUTE_FORBIDDEN: &str = "act_as_route_forbidden";
    /// `Sew-Via` names another agent than `Sew-Act-As` (403).
    pub const VIA_AGENT_MISMATCH: &str = "via_agent_mismatch";
    /// The handle is held by another active member (409).
    pub const USERNAME_TAKEN: &str = "username_taken";
    /// The install owner cannot be deprovisioned or made a guest (422).
    pub const IS_OWNER: &str = "is_owner";
    /// The person is deactivated (422).
    pub const PERSON_DEACTIVATED: &str = "person_deactivated";
    /// The person is a guest (422).
    pub const PERSON_IS_GUEST: &str = "person_is_guest";
    /// The person is an agent (422).
    pub const PERSON_IS_AGENT: &str = "person_is_agent";
    /// Provisioning an assistant whose person is not provisioned yet (422, contracts rev 13;
    /// K2 retries for up to 1 h).
    pub const ASSISTANT_PERSON_UNKNOWN: &str = "assistant_person_unknown";
    /// Provisioning an assistant whose person is deactivated (422, contracts rev 13; K2 stops
    /// retrying).
    pub const ASSISTANT_PERSON_INACTIVE: &str = "assistant_person_inactive";
    /// The agent is deactivated (422).
    pub const AGENT_DEACTIVATED: &str = "agent_deactivated";
    /// Pinning a coworker; convert it through provision (422).
    pub const AGENT_IS_COWORKER: &str = "agent_is_coworker";
    /// Unpin names another person than the current pin (409).
    pub const PIN_MISMATCH: &str = "pin_mismatch";
    /// The owner must be an active, non-guest person (422).
    pub const TARGET_NOT_ELIGIBLE: &str = "target_not_eligible";
    /// SW15: this server has no backup engine (503).
    pub const BACKUPS_UNAVAILABLE: &str = "backups_unavailable";
    /// SW15: another backup holds the backup lock (409).
    pub const BACKUP_RUNNING: &str = "backup_running";
    /// SW11/SW13: an owner, manager or member names no provisioned principal (422).
    pub const PRINCIPAL_UNKNOWN: &str = "principal_unknown";
    /// SW11: the slug is held by a Space of another owner or App (409).
    pub const SLUG_TAKEN: &str = "slug_taken";
    /// SW13: `app_id` does not name the Space's App (409).
    pub const APP_MISMATCH: &str = "app_mismatch";
    /// SW13: the Space is bound to another App already (409).
    pub const SPACE_BOUND: &str = "space_bound";
    /// The owner approval did not verify (403).
    pub const APPROVAL_INVALID: &str = "approval_invalid";
    /// A transfer needs the current owner's approval (403).
    pub const OWNER_APPROVAL_REQUIRED: &str = "owner_approval_required";
    /// `force` needs a standalone install and peer uid 0 (403).
    pub const FORCE_NOT_ALLOWED: &str = "force_not_allowed";
    /// A live job holds the dead job's dedupe key (409).
    pub const DEDUPE_KEY_TAKEN: &str = "dedupe_key_taken";
    /// Only dead jobs can be retried (422).
    pub const NOT_DEAD: &str = "not_dead";
    /// Master rotation needs `sew-master-key-prev` loaded (422).
    pub const NO_RETIRING_MASTER_KEY: &str = "no_retiring_master_key";
    /// An assistant may not open DMs (403, stage B).
    pub const ASSISTANT_CANNOT_OPEN_DM: &str = "assistant_cannot_open_dm";
    /// Assistants hold no space or channel memberships (422, stage B).
    pub const ASSISTANT_NOT_JOINABLE: &str = "assistant_not_joinable";
    /// A 1:1 DM with a deactivated partner is read-only (422, stage B).
    pub const DM_PARTNER_DEACTIVATED: &str = "dm_partner_deactivated";
    /// `sew init` when an owner exists (409, S24).
    pub const OWNER_EXISTS: &str = "owner_exists";
    /// The local user is deactivated (409/422, S24).
    pub const USER_DEACTIVATED: &str = "user_deactivated";
    /// The member is not a local person (422, S24).
    pub const NOT_A_LOCAL_PERSON: &str = "not_a_local_person";
    /// A local user's handle is part of its external id and cannot change (422, S24).
    pub const LOCAL_USERNAME_FIXED: &str = "local_username_fixed";
    /// No mgmt or act-as route matches (404, contracts rev 17).
    pub const UNKNOWN_ROUTE: &str = "unknown_route";
    /// The install owner's role changes only through `owner/set` (422, S10 name).
    pub const OWNER_ROLE_FIXED: &str = "owner_role_fixed";
    /// A guest's role changes only through guest conversion (422, S10 name).
    pub const GUEST_ROLE_FIXED: &str = "guest_role_fixed";
    /// Agents are always `member` (422, S10 name).
    pub const AGENT_ROLE_FIXED: &str = "agent_role_fixed";
    /// A meeting link guest is never converted or given another role (422, Meet MC2a).
    pub const LINK_GUEST_FIXED: &str = "link_guest_fixed";
    /// A person with an active pinned assistant cannot become a guest (422, S19 §7.4; matches
    /// S27's K2-side refusal). `details.assistant_ids` names them.
    pub const PERSON_HAS_ASSISTANTS: &str = "person_has_assistants";
    /// A guest login while `guests.enabled` is off (audit `details.reason`, S19 §7.7).
    pub const GUESTS_DISABLED: &str = "guests_disabled";
    /// The compliance flag is never on a guest (422, S19 gap G16).
    pub const TARGET_IS_GUEST: &str = "target_is_guest";

    /// Every reason above.
    pub const ALL: &[&str] = &[
        PEER_UID_NOT_ALLOWED,
        ACT_AS_ON_MGMT_ROUTE,
        ACT_AS_HEADERS_REQUIRED,
        ACT_AS_HEADER_INVALID,
        ACT_AS_ONLY_ON_MGMT_SOCKET,
        ACT_AS_ROUTE_FORBIDDEN,
        VIA_AGENT_MISMATCH,
        USERNAME_TAKEN,
        IS_OWNER,
        PERSON_DEACTIVATED,
        PERSON_IS_GUEST,
        PERSON_IS_AGENT,
        ASSISTANT_PERSON_UNKNOWN,
        ASSISTANT_PERSON_INACTIVE,
        AGENT_DEACTIVATED,
        AGENT_IS_COWORKER,
        PIN_MISMATCH,
        TARGET_NOT_ELIGIBLE,
        APPROVAL_INVALID,
        OWNER_APPROVAL_REQUIRED,
        FORCE_NOT_ALLOWED,
        DEDUPE_KEY_TAKEN,
        NOT_DEAD,
        NO_RETIRING_MASTER_KEY,
        ASSISTANT_CANNOT_OPEN_DM,
        ASSISTANT_NOT_JOINABLE,
        DM_PARTNER_DEACTIVATED,
        OWNER_EXISTS,
        USER_DEACTIVATED,
        NOT_A_LOCAL_PERSON,
        LOCAL_USERNAME_FIXED,
        UNKNOWN_ROUTE,
        OWNER_ROLE_FIXED,
        GUEST_ROLE_FIXED,
        AGENT_ROLE_FIXED,
        LINK_GUEST_FIXED,
        PERSON_HAS_ASSISTANTS,
        GUESTS_DISABLED,
        TARGET_IS_GUEST,
    ];
}

// ---- agent deliveries ----

/// Largest `limit` of a deliveries page (S9 §6.11).
pub const DELIVERIES_LIMIT_MAX: u32 = 500;
/// Default `limit` of a deliveries page.
pub const DELIVERIES_LIMIT_DEFAULT: u32 = 100;
/// A delivery's `body` is cut to at most this many bytes, at a char boundary (S9 §6.11).
pub const DELIVERY_BODY_MAX_BYTES: usize = 4_000;

/// Query of `GET /mgmt/v1/agents/{member_id}/deliveries`: long-poll after a member seq.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeliveriesQuery {
    /// Return entries with `seq > after`. K2 sends the previous page's `next_after`.
    pub after: u64,
    /// Long-poll up to this long (≤ 25000, default 25000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// At most this many entries (1–500, default 100; S9 §12, additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl DeliveriesQuery {
    /// The long-poll timeout, default 25 s.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(DELIVERIES_LONG_POLL_MAX_MS)
    }

    /// The page size, default 100.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(DELIVERIES_LIMIT_DEFAULT)
    }
}

impl Validate for DeliveriesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(t) = self.timeout_ms
            && t > DELIVERIES_LONG_POLL_MAX_MS
        {
            return Err(ValidationError::invalid(
                "timeout_ms",
                format!("timeout_ms must be at most {DELIVERIES_LONG_POLL_MAX_MS}"),
            ));
        }
        if let Some(l) = self.limit
            && !(1..=DELIVERIES_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{DELIVERIES_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

unit_enum! {
    /// What kind of message a delivery carries (contracts rev 36 G3). v1 sends `text` only;
    /// `Unknown` renders as text.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DeliveryKind {
        /// A chat message.
        #[default]
        Text = "text",
        /// A DM call record (S34).
        CallRecord = "call_record",
        /// A poll (S17).
        Poll = "poll",
        /// A call transcript (S35).
        Transcript = "transcript",
    }
}

unit_enum! {
    /// Where a delivered message lives, from the agent's side (contracts rev 36 G5).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DeliveryPlace {
        /// A space channel.
        SpaceChannel = "space_channel",
        /// A 1:1 DM the agent is in.
        Dm = "dm",
        /// A group DM the agent is in.
        GroupDm = "group_dm",
        /// A DM or group DM of an assistant's person, read (never posted to) by the assistant.
        PersonDm = "person_dm",
    }
}

/// A call record's summary in a delivery (`kind = call_record`; contracts rev 36 G3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryCall {
    /// The call was missed.
    pub missed: bool,
    /// How long it lasted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A message summary in an agent delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryMessage {
    /// The message.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its space; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Its thread root, for replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// The author.
    pub author_id: Id,
    /// The author's current display name (also for deactivated authors).
    pub author_display_name: String,
    /// The author's current username (contracts rev 36 G4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_username: Option<Handle>,
    /// Person or agent.
    pub author_kind: MemberKind,
    /// The message's conversation address (the thread address for replies).
    pub addr: Addr,
    /// Where a reply goes (contracts rev 31): `<channel addr>/<message id>` for a top-level
    /// space message, the thread address for replies, the DM address for DMs. Absent for
    /// `place = person_dm` (the assistant may not post there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_addr: Option<Addr>,
    /// Where the message lives from the agent's side (contracts rev 36 G5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<DeliveryPlace>,
    /// What kind of message it is (contracts rev 36 G3).
    #[serde(default)]
    pub kind: DeliveryKind,
    /// A call record's summary (`kind = call_record` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<DeliveryCall>,
    /// A recording record's summary (`kind = transcript` only, S35 v2 §17.2). Never audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<crate::recording::RecordingSummary>,
    /// The current body (after edits), cut to 4,000 bytes at a char boundary.
    pub body: String,
    /// Whether `body` was cut.
    #[serde(default)]
    pub body_truncated: bool,
    /// How many attachments the message has.
    #[serde(default)]
    pub attachment_count: u32,
    /// For assistant-authored messages: the person the assistant posted for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<Id>,
    /// That person's current display name (contracts rev 36 G4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of_display_name: Option<String>,
    /// When it was posted.
    pub created_at: Timestamp,
}

/// One agent delivery: a `notification` entry of the agent's member log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    /// The member seq.
    pub seq: u64,
    /// When it was written.
    pub at: Timestamp,
    /// Why the agent was notified.
    pub reason: NotificationReason,
    /// The message.
    pub message: DeliveryMessage,
}

/// Response of `GET /mgmt/v1/agents/{member_id}/deliveries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveriesPage {
    /// Deliveries with `seq > after`, oldest first. Entries the agent cannot read now are
    /// skipped without a trace.
    pub entries: Vec<Delivery>,
    /// The member log head.
    pub head: u64,
    /// The next `after`: the seq of the last row scanned (delivered or skipped), `after` when
    /// nothing was scanned, `head` after an empty long-poll (S9 §6.11, additive).
    #[serde(default)]
    pub next_after: u64,
    /// Whether more entries follow `next_after`.
    pub has_more: bool,
    /// The install's `sync_epoch` (opaque; changes on restore; contracts revs 7, 36).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_epoch: Option<String>,
}

// ---- jobs ----

/// Response of the job-starting routes (backup, export, rotate-master).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartJobResponse {
    /// The job.
    pub job_id: Id,
}

unit_enum! {
    /// A job's state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum JobState {
        /// Waiting to run.
        Queued = "queued",
        /// Running.
        Running = "running",
        /// Finished.
        Succeeded = "succeeded",
        /// Failed; may retry.
        Failed = "failed",
        /// Exhausted its attempts.
        Dead = "dead",
    }
}

/// Response of `GET /mgmt/v1/jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobStatus {
    /// The job.
    pub id: Id,
    /// The job kind (for example `backup`).
    pub kind: String,
    /// The state.
    pub state: JobState,
    /// Attempts so far.
    pub attempts: u32,
    /// The last error kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// When it was queued.
    pub created_at: Timestamp,
    /// When it finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// What a `succeeded` job recorded (`jobs_done.result`; contracts rev 17, additive).
    /// Absent otherwise. Ids and counts only, never content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Query of `GET /mgmt/v1/jobs` (S9 G5): newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobsQuery {
    /// Only jobs in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<JobState>,
    /// Only jobs of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Jobs with an id below this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl JobsQuery {
    /// The page size, default 50.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(crate::limits::PAGE_LIMIT_DEFAULT)
    }
}

impl Validate for JobsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.state {
            check_known("state", s)?;
        }
        if let Some(k) = &self.kind
            && (k.is_empty()
                || k.len() > 64
                || !k
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
        {
            return Err(ValidationError::invalid(
                "kind",
                "kind must be 1–64 chars of [a-z0-9_]",
            ));
        }
        if let Some(l) = self.limit
            && !(1..=crate::limits::PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{}", crate::limits::PAGE_LIMIT_MAX),
            ));
        }
        Ok(())
    }
}

/// Response of `GET /mgmt/v1/jobs`.
pub type JobStatusPage = crate::http::Page<JobStatus>;
