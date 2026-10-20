//! The logical export archive, format 1 (prd-sew-23 §8, contracts rev 37).
//!
//! One format serves three uses: eDiscovery exports, box moves (full export, then `sew import`)
//! and a later Slack converter. An archive is an age-encrypted (X25519) zstd stream of a
//! deterministic POSIX tar holding:
//!
//! - `manifest.json` first ([`Manifest`]), `trailer.json` last ([`Trailer`]);
//! - one NDJSON entry per entity (`members.ndjson`, `messages/<channel_id>/000001.ndjson`, …),
//!   one record per line, keys in the declared field order, `null` and empty lists omitted
//!   unless a field says *always*;
//! - `files/<file_id>`: one blob's plaintext; `file_id` is the smallest link id that points at
//!   the blob;
//! - `view/…` (eDiscovery only): escaped static HTML and formula-safe CSV.
//!
//! Records are strict: an importer refuses unknown fields and unknown enum strings, and a
//! reader refuses a `format_minor` newer than it knows (`format_too_new`). Within a minor the
//! field set is fixed, so "import, then re-export" is byte-equal. Ids are decimal strings,
//! times RFC 3339 UTC with milliseconds, enums lowercase strings.
//!
//! Content (bodies, file names, poll text, notes) appears only here, decrypted, inside the
//! age layer. Purged content is a stub (metadata plus `purged_at`); content whose month key is
//! shredded is marked `unreadable`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::channels::{ChannelType, Visibility};
use crate::api::compliance::{LegalHold, RetentionPolicy};
use crate::api::files::FileKind;
use crate::api::members::{AgentKind, InstallRole, MemberKind};
use crate::api::messages::ReferenceKind;
use crate::id::Id;
use crate::time::Timestamp;

/// Largest plaintext of one NDJSON part, HTML page or CSV file before it splits (32 MiB).
pub const PART_BYTES_DEFAULT: u64 = 32 * 1024 * 1024;

/// What the manifest lists as never exported.
pub const EXCLUDED: &[&str] = &[
    "keys",
    "keyring",
    "master_key",
    "passwords",
    "sessions",
    "secrets",
    "search_index",
    "change_logs",
    "jobs",
    "idempotency",
    "link_previews",
    "uploads",
    "exports",
];

/// `details.reason` values of archive readers (import, `--verify-only`, the kit).
pub mod reasons {
    /// The identity does not decrypt the archive header.
    pub const IDENTITY_MISMATCH: &str = "identity_mismatch";
    /// The identity file is not root-owned with mode ≤ 0600.
    pub const IDENTITY_UNSAFE: &str = "identity_unsafe";
    /// The archive is truncated, altered, or its entries do not match the trailer.
    pub const ARCHIVE_CORRUPT: &str = "archive_corrupt";
    /// `export_format` is not 1.
    pub const FORMAT_UNSUPPORTED: &str = "format_unsupported";
    /// `format_minor` is newer than this build reads.
    pub const FORMAT_TOO_NEW: &str = "format_too_new";
    /// An eDiscovery archive given to `sew import`.
    pub const NOT_A_FULL_EXPORT: &str = "not_a_full_export";
    /// The archive's identity provider differs from the target's.
    pub const PROVIDER_MISMATCH: &str = "provider_mismatch";
    /// The target install is not pristine.
    pub const TARGET_NOT_EMPTY: &str = "target_not_empty";
    /// Not enough free space.
    pub const STORAGE_FULL: &str = "storage_full";
    /// A record failed validation (entry and line in the details).
    pub const RECORD_INVALID: &str = "record_invalid";
    /// The loaded audit chain does not verify.
    pub const AUDIT_CHAIN_BROKEN: &str = "audit_chain_broken";
    /// The service is running (`systemctl stop sew` first).
    pub const SERVICE_RUNNING: &str = "service_running";
    /// Not run as root.
    pub const NOT_ROOT: &str = "not_root";
    /// The database service is not active.
    pub const DB_NOT_ACTIVE: &str = "db_not_active";
    /// The key mirror folder of the adopted install id already holds keys (contracts rev 42).
    pub const MIRROR_NOT_EMPTY: &str = "mirror_not_empty";
}

unit_enum! {
    /// What an archive is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ExportKind {
        /// Everything, for a box move (mgmt only).
        Full = "full",
        /// A scoped compliance export.
        Ediscovery = "ediscovery",
    }
}

unit_enum! {
    /// Whether file bytes are in the archive.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum FileMode {
        /// `files/<file_id>` entries hold the plaintext bytes.
        #[default]
        Include = "include",
        /// Attachment records keep metadata only; no `files/` entries.
        ManifestOnly = "manifest_only",
    }
}

unit_enum! {
    /// How a member target selects messages in eDiscovery (prd-sew-23 §9.1).
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MemberMode {
        /// Everything the member wrote or could read (the legal-hold member rule).
        #[default]
        Conversations = "conversations",
        /// Only what the member wrote.
        Authored = "authored",
    }
}

unit_enum! {
    /// Why a message was deleted.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DeleteKind {
        /// Its author deleted it.
        Own = "own",
        /// A moderator deleted it.
        Moderator = "moderator",
        /// A moderator deleted its whole thread.
        Thread = "thread",
        /// Retention expired it.
        Retention = "retention",
    }
}

unit_enum! {
    /// Why an attachment was removed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RemoveKind {
        /// Its uploader removed it.
        Own = "own",
        /// A moderator removed it.
        Moderator = "moderator",
        /// File retention removed it.
        Retention = "retention",
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

// ---------------------------------------------------------------- manifest and trailer

/// Who wrote the archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    /// `sew`, or a converter's name.
    pub name: String,
    /// The producer's version.
    pub version: String,
    /// The source schema version (highest applied migration).
    pub schema_version: u64,
}

/// What an archive holds beyond records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveOptions {
    /// File bytes or metadata only.
    pub files: FileMode,
    /// Whether `view/` (HTML and CSV) is present.
    pub views: bool,
    /// Whether `audit.ndjson` is present.
    pub audit: bool,
}

/// An eDiscovery scope as recorded: ids, dates and a keyword count, never the keywords.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveScope {
    /// The requester's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Member targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Id>,
    /// Channel targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Id>,
    /// Space targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spaces: Vec<Id>,
    /// Legal holds whose selectors joined the scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<Id>,
    /// Window start (inclusive) of the request's own selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// Window end (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
    /// How member targets select.
    pub member_mode: MemberMode,
    /// How many keywords filtered the scope (the words never leave the request).
    pub keyword_count: u32,
}

/// `manifest.json`, always the first entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always 1 for this format.
    pub export_format: u32,
    /// Additive revisions of format 1; readers refuse a higher one.
    pub format_minor: u32,
    /// Full or eDiscovery.
    pub kind: ExportKind,
    /// Who wrote it.
    pub producer: Producer,
    /// The export id (= the job id).
    pub export_id: Id,
    /// The source install; `null` only from a converter (import keeps the target's id).
    pub install_id: Option<Id>,
    /// The install's name (empty when the install has none).
    pub install_name: String,
    /// `k2` or `standalone`.
    pub identity_provider: String,
    /// The source `sync_epoch` (opaque decimal string).
    pub source_sync_epoch: String,
    /// When the export was requested.
    pub created_at: Timestamp,
    /// The snapshot every record was read from; also every tar entry's mtime.
    pub snapshot_at: Timestamp,
    /// Files, views, audit.
    pub options: ArchiveOptions,
    /// The eDiscovery scope; `null` for a full export (*always*).
    pub scope: Option<ArchiveScope>,
    /// Entity paths in tar order (message and revision parts as globs), not files or views.
    pub entries: Vec<String>,
    /// What is never exported.
    pub excluded: Vec<String>,
    /// The age recipients the archive is encrypted to.
    pub recipients: Vec<String>,
}

/// One entry in the trailer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailerEntry {
    /// The tar path.
    pub path: String,
    /// Its size.
    pub bytes: u64,
    /// SHA-256 of its bytes, 64 lowercase hex.
    pub sha256: String,
}

/// `trailer.json`, always the last entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Trailer {
    /// Every entry before this one, in tar order.
    pub entries: Vec<TrailerEntry>,
    /// Records per entity (and `files`).
    pub counts: BTreeMap<String, u64>,
    /// SHA-256 over `"<path>\t<bytes>\t<sha256>\n"` of every entry, in order, 64 hex.
    pub root_hash: String,
}

/// A converter's reference to the record it came from (ignored and not stored by Sew v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    /// The source system, e.g. `slack`.
    pub system: String,
    /// Its reference.
    #[serde(rename = "ref")]
    pub reference: String,
}

// ---------------------------------------------------------------- install and people

/// `install.ndjson`: one record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    /// The install id.
    pub install_id: Id,
    /// The install name (empty when unnamed).
    pub name: String,
    /// When the install was created.
    pub created_at: Timestamp,
    /// Org policy values by key (retention mirrors excluded).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub policies: BTreeMap<String, serde_json::Value>,
    /// The id clock high-water mark (Unix ms).
    pub id_hwm_ms: u64,
}

/// `profile_fields.ndjson`: a custom profile field definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileFieldRecord {
    /// The field id.
    pub id: Id,
    /// Its key.
    pub key: String,
    /// Its label.
    pub label: String,
    /// `text`, `url`, `select`, `date`.
    pub kind: String,
    /// Select options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    /// `member` or `provider`.
    pub managed_by: String,
    /// `everyone`, `non_guests`, `admins`.
    pub visibility: String,
    /// Order.
    pub position: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Soft-deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
    /// Converter reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// A member's status in `members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusRecord {
    /// The status text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The status emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// When it clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// When it was set.
    pub set_at: Timestamp,
}

/// A member's avatar in `members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AvatarRecord {
    /// The image (`files/<file>`; also the avatar link id).
    pub file: Id,
    /// The public avatar version id (in avatar URLs).
    pub avatar_id: Id,
    /// `member` or `provider`.
    pub source: String,
}

/// `members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberRecord {
    /// The member id.
    pub id: Id,
    /// The external id (`k2:principal:<uuid>`, `local:<name>`, `import:<system>:<id>`).
    pub external_id: String,
    /// Person or agent.
    pub kind: MemberKind,
    /// For agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    /// For pinned assistants: their person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<Id>,
    /// When the assistant was pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<Timestamp>,
    /// Install role.
    pub install_role: InstallRole,
    /// The compliance flag.
    pub compliance: bool,
    /// When the flag last changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance_set_at: Option<Timestamp>,
    /// Who changed it (absent for the mgmt service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance_set_by: Option<Id>,
    /// Handle.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Pronouns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// IANA time zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Custom field values by field id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    /// Status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusRecord>,
    /// Avatar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<AvatarRecord>,
    /// Created.
    pub created_at: Timestamp,
    /// Deactivated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deactivated_at: Option<Timestamp>,
    /// The member change-log head (*always*; full exports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_seq: Option<u64>,
    /// Converter reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// `presence_settings.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresenceSettingsRecord {
    /// The member.
    pub member_id: Id,
    /// `none`, `away`, `dnd`, `invisible`.
    pub manual: String,
    /// When a timed manual status ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_until: Option<Timestamp>,
    /// The weekly DND schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd_schedule: Option<serde_json::Value>,
    /// A skipped DND window ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd_skip_until: Option<Timestamp>,
    /// Last change.
    pub updated_at: Timestamp,
}

// ---------------------------------------------------------------- structure

/// `spaces.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceRecord {
    /// The space id.
    pub id: Id,
    /// Slug.
    pub slug: String,
    /// Name.
    pub name: String,
    /// Owner.
    pub owner_id: Id,
    /// Creator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// The icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<IconRecord>,
    /// Created.
    pub created_at: Timestamp,
    /// `open` or `invite`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_policy: Option<String>,
    /// Default channels in their set order (minor 1, Hearth HC6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_channel_ids: Vec<Id>,
    /// Description (minor 1, Hearth HC4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Rules channel (minor 1, Hearth HC4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_channel_id: Option<Id>,
    /// Welcome channel (minor 1, Hearth HC4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_channel_id: Option<Id>,
    /// The App the space belongs to (minor 2, SW13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// A hidden Space, such as the Meetings Space (minor 3, Meet MC1); omitted when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Converter reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// A space icon in `spaces.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IconRecord {
    /// The image (`files/<file>`).
    pub file: Id,
    /// The icon's own link id.
    pub link_id: Id,
    /// The public icon version id.
    pub icon_id: Id,
}

/// `roles.ndjson` (the `@everyone` role has the space's id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleRecord {
    /// The role id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// Name.
    pub name: String,
    /// Position (0 = `@everyone`).
    pub position: u32,
    /// Permission bits, decimal string.
    pub perms: String,
    /// RGB color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Mentionable.
    pub mentionable: bool,
    /// Shown apart in member lists.
    pub hoist: bool,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// `space_members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceMemberRecord {
    /// The space.
    pub space_id: Id,
    /// The member.
    pub member_id: Id,
    /// Joined.
    pub joined_at: Timestamp,
    /// Who added them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<Id>,
    /// Roles held (not `@everyone`), ascending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_ids: Vec<Id>,
    /// Timed out until.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_until: Option<Timestamp>,
}

/// `user_groups.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserGroupRecord {
    /// The group id.
    pub id: Id,
    /// Its space; absent = install-wide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Handle.
    pub handle: String,
    /// Name.
    pub name: String,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Mentionable.
    pub mentionable: bool,
    /// Members, ascending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<Id>,
    /// Creator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Soft-deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
}

/// A permission overwrite on a category or channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverwriteRecord {
    /// `role` or `member`.
    pub target_kind: String,
    /// The role or member.
    pub target_id: Id,
    /// Allowed bits, decimal string.
    pub allow: String,
    /// Denied bits, decimal string.
    pub deny: String,
}

/// `categories.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryRecord {
    /// The category id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// Name.
    pub name: String,
    /// Position.
    pub position: u32,
    /// Overwrites, by target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overwrites: Vec<OverwriteRecord>,
    /// Created.
    pub created_at: Timestamp,
}

/// `channels.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelRecord {
    /// The channel id.
    pub id: Id,
    /// Its space (absent for DMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Its category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Id>,
    /// Type.
    #[serde(rename = "type")]
    pub kind: ChannelType,
    /// Public or private.
    pub visibility: Visibility,
    /// Slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Position.
    pub position: u32,
    /// Overwrites follow the category.
    pub synced: bool,
    /// The channel's own overwrites (full exports).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overwrites: Vec<OverwriteRecord>,
    /// Slow mode seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_mode_s: Option<u32>,
    /// DM and group DM members, in DM-key order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dm_member_ids: Vec<Id>,
    /// Creator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// Created.
    pub created_at: Timestamp,
    /// Archived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Timestamp>,
    /// Archived by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_by: Option<Id>,
    /// The channel change-log head (*always* in full exports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_seq: Option<u64>,
    /// S35 v2: auto-record (voice channels).
    #[serde(default, skip_serializing_if = "is_false")]
    pub recording_auto: bool,
    /// S35 v2: the channel's default audio keep days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_audio_days: Option<u16>,
    /// S35 v2: declining means not joining (absent: the org policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_require_consent: Option<bool>,
    /// Converter reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// `channel_members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelMemberRecord {
    /// The channel.
    pub channel_id: Id,
    /// The member.
    pub member_id: Id,
    /// Joined.
    pub joined_at: Timestamp,
    /// Who added them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<Id>,
}

/// `channel_past_members.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelPastMemberRecord {
    /// The channel.
    pub channel_id: Id,
    /// The member.
    pub member_id: Id,
    /// When they left.
    pub left_at: Timestamp,
}

/// `emoji.ndjson`: custom emoji (deleted ones keep their row so old tokens render).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmojiRecord {
    /// The emoji id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// Primary name.
    pub name: String,
    /// Other live names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Animated.
    pub animated: bool,
    /// Width (px).
    pub width: u32,
    /// Height (px).
    pub height: u32,
    /// The image (`files/<file>`; absent once deleted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Id>,
    /// The image's own link id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<Id>,
    /// Creator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
    /// Deleted by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_by: Option<Id>,
}

// ---------------------------------------------------------------- messages

/// A message's reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRecord {
    /// Reply, quote or forward.
    pub kind: ReferenceKind,
    /// The referenced message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Forward: the original author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_author_id: Option<Id>,
    /// Forward: when the original was posted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_created_at: Option<Timestamp>,
    /// Forward: the copied text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_body: Option<String>,
    /// Forward: the copied text's key is shredded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub forward_unreadable: bool,
}

/// Who a message (or revision) mentions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MentionsRecord {
    /// Members.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Id>,
    /// Roles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Id>,
    /// User groups.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Id>,
    /// `@channel`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub channel: bool,
    /// `@here`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub here: bool,
}

impl MentionsRecord {
    /// Whether nothing is mentioned.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
            && self.roles.is_empty()
            && self.groups.is_empty()
            && !self.channel
            && !self.here
    }
}

/// A message's deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeletedRecord {
    /// When.
    pub at: Timestamp,
    /// By whom (absent for retention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Id>,
    /// Why.
    pub kind: DeleteKind,
}

/// A pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinRecord {
    /// Who pinned.
    pub by: Id,
    /// When.
    pub at: Timestamp,
}

/// One emoji's reactions (current state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReactionRecord {
    /// The emoji key: `u:<unicode>` or `c:<custom emoji id>`.
    pub emoji: String,
    /// Who reacted, in reaction order.
    pub member_ids: Vec<Id>,
    /// When each reacted (same order as `member_ids`).
    pub at: Vec<Timestamp>,
}

/// An attachment's removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemovedRecord {
    /// When.
    pub at: Timestamp,
    /// By whom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Id>,
    /// Why.
    pub kind: RemoveKind,
}

/// An attachment (its id is also its file link id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRecord {
    /// The attachment id.
    pub id: Id,
    /// Order on the message, 0–9.
    pub position: u32,
    /// Uploader.
    pub uploader_id: Id,
    /// File name (absent when purged or unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// The name's key is shredded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unreadable: bool,
    /// Sniffed content type.
    pub content_type: String,
    /// Kind.
    pub kind: FileKind,
    /// Shown inline.
    pub inline: bool,
    /// Plaintext size.
    pub size: u64,
    /// Width (images, video).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height (images, video).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Animated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub animated: bool,
    /// The attachment it was forwarded from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_attachment_id: Option<Id>,
    /// Created.
    pub created_at: Timestamp,
    /// Removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<RemovedRecord>,
    /// Purged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purged_at: Option<Timestamp>,
    /// The bytes (`files/<file>`); absent when purged or with `files: manifest_only`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Id>,
}

/// A poll option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollOptionRecord {
    /// The option row id.
    pub id: Id,
    /// 0-based index.
    pub index: u32,
    /// Text (absent when purged or unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Voters (absent in eDiscovery for anonymous polls).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voter_ids: Vec<Id>,
    /// When each voted (same order as `voter_ids`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voted_at: Vec<Timestamp>,
    /// Votes (*always*).
    pub count: u64,
}

/// A poll on its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollRecord {
    /// The question (absent when purged or unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The poll text's key is shredded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unreadable: bool,
    /// Options in order.
    pub options: Vec<PollOptionRecord>,
    /// Choices per voter.
    pub max_choices: u32,
    /// Anonymous.
    pub anonymous: bool,
    /// `always` or `after_close`.
    pub results_visibility: String,
    /// Timed close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closes_at: Option<Timestamp>,
    /// Closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<Timestamp>,
    /// Closed by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<Id>,
    /// Created.
    pub created_at: Timestamp,
}

/// `messages/<channel_id>/<part>.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageRecord {
    /// The message id (`id_to_time(id) == created_at`).
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Author.
    pub author_id: Id,
    /// For assistant posts: the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<Id>,
    /// The thread root, for replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// A reply also sent to the channel.
    #[serde(default, skip_serializing_if = "is_false")]
    pub also_in_channel: bool,
    /// Reply, quote or forward reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ReferenceRecord>,
    /// The body as stored (markdown); absent when purged or unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Edit version (0 = never edited).
    pub version: u32,
    /// The channel seq of the message's latest change (full exports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Mentions (absent when purged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<MentionsRecord>,
    /// Usable custom emoji ids of the body, ascending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_emoji_ids: Vec<Id>,
    /// Link previews turned off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub previews_suppressed: bool,
    /// Created.
    pub created_at: Timestamp,
    /// Last edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<Timestamp>,
    /// Deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<DeletedRecord>,
    /// Purged (content destroyed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purged_at: Option<Timestamp>,
    /// The body's key is shredded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unreadable: bool,
    /// Pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<PinRecord>,
    /// Reactions (current).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionRecord>,
    /// Attachments (removed ones included).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentRecord>,
    /// A poll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<PollRecord>,
    /// eDiscovery: a thread root added so a reviewer can read an included reply.
    #[serde(default, skip_serializing_if = "is_false")]
    pub context: bool,
    /// eDiscovery with keywords: a keyword matched.
    #[serde(default, skip_serializing_if = "is_false")]
    pub matched: bool,
    /// Converter reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// `revisions/<channel_id>/<part>.ndjson`: edit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionRecord {
    /// The revision row id.
    pub id: Id,
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The version this body was.
    pub version: u32,
    /// The body (absent when purged or unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Mentions of that version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<MentionsRecord>,
    /// When the version was written.
    pub created_at: Timestamp,
    /// When it was replaced.
    pub replaced_at: Timestamp,
    /// The body was purged.
    #[serde(default, skip_serializing_if = "is_false")]
    pub purged: bool,
    /// The body's key is shredded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unreadable: bool,
}

/// `bookmarks.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BookmarkRecord {
    /// The bookmark id.
    pub id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Order.
    pub position: u32,
    /// The decrypted bookmark object (`{title, url, emoji?}`); absent when unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Creator.
    pub created_by: Id,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// `thread_follows.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadFollowRecord {
    /// The member.
    pub member_id: Id,
    /// The thread root.
    pub root_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// `following` or `unfollowed`.
    pub state: String,
    /// `root_author`, `replied`, `mentioned`, `manual`.
    pub reason: String,
    /// Last change.
    pub followed_at: Timestamp,
}

/// `read_states.ndjson` (full exports only): S14's channel read cursors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadStateRecord {
    /// The member.
    pub member_id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The cursor (a comparator; mark-unread writes `message_id - 1`). Absent = nothing read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_read_id: Option<Id>,
    /// Last move.
    pub updated_at: Timestamp,
}

/// `thread_reads.ndjson` (full exports only): S14's thread read cursors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadReadRecord {
    /// The member.
    pub member_id: Id,
    /// The thread root.
    pub root_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The cursor (≥ the root id).
    pub last_read_id: Id,
    /// Last move.
    pub updated_at: Timestamp,
}

/// `mentions.ndjson`: the mention inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MentionRecord {
    /// The member.
    pub member_id: Id,
    /// The channel.
    pub channel_id: Id,
    /// The message.
    pub message_id: Id,
    /// The thread root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<Id>,
    /// The notification reason code (1 mention … 6 dm).
    pub reason: u32,
    /// Written.
    pub created_at: Timestamp,
}

/// `saved_items.ndjson` (full exports only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedItemRecord {
    /// The saved item id.
    pub id: Id,
    /// Its member.
    pub member_id: Id,
    /// The message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// `active`, `done`, `archived`.
    pub state: String,
    /// The member's note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Last state change.
    pub state_changed_at: Timestamp,
}

/// `reminders.ndjson` (full exports only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReminderRecord {
    /// The reminder id.
    pub id: Id,
    /// Its member.
    pub member_id: Id,
    /// The message (message reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Id>,
    /// The message's channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// The note (free-text reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When it is due.
    pub at: Timestamp,
    /// IANA zone it was set in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `pending`, `fired`, `done`.
    pub state: String,
    /// Fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<Timestamp>,
    /// Done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<Timestamp>,
    /// Snoozes so far.
    pub snooze_count: u32,
    /// Row version.
    pub version: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// `drafts.ndjson` (full exports only; drafts holding only pending uploads are not carried).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftRecord {
    /// The draft id.
    pub id: Id,
    /// Its member.
    pub member_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// Reply also to the channel.
    #[serde(default, skip_serializing_if = "is_false")]
    pub also_send_to_channel: bool,
    /// The text.
    pub body: String,
    /// Draft revision.
    pub rev: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change (its seal month).
    pub updated_at: Timestamp,
}

/// `scheduled.ndjson`: pending and failed scheduled messages (full exports only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledRecord {
    /// The scheduled message id.
    pub id: Id,
    /// Its author.
    pub member_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// Its thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<Id>,
    /// Reply also to the channel.
    #[serde(default, skip_serializing_if = "is_false")]
    pub also_send_to_channel: bool,
    /// The text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// When it sends.
    pub send_at: Timestamp,
    /// IANA zone it was set in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `pending` or `failed`.
    pub state: String,
    /// Failure code (1–13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<u32>,
    /// Failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<Timestamp>,
    /// Row version.
    pub version: u32,
    /// Created.
    pub created_at: Timestamp,
    /// Last change (its seal month).
    pub updated_at: Timestamp,
}

/// `notify_prefs.ndjson` (S21): one record per member with notification settings or explicit
/// channel or space prefs. DND lives in `presence_settings.ndjson` (S18 owns it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifyPrefsRecord {
    /// The member.
    pub member_id: Id,
    /// The member's settings row (absent: every default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<NotifySettingsRecord>,
    /// Explicit channel and DM prefs, by channel id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_prefs: Vec<NotifyChannelPrefRecord>,
    /// Explicit space levels, by space id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub space_prefs: Vec<NotifySpacePrefRecord>,
}

/// A member's notification settings (`notify_prefs.settings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifySettingsRecord {
    /// `all`, `mentions` or `none` (absent: inherit the install default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
    /// Followed-thread replies notify.
    pub thread_replies: bool,
    /// `@here`/`@channel`/`@everyone` notify.
    pub broad_mentions: bool,
    /// Mentions still notify in a muted channel.
    pub mute_lets_mentions: bool,
    /// `off`, `15m`, `1h` or `1d`.
    pub digest_interval: String,
    /// The normalized keyword list, decrypted (absent: none, or its key is shredded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Settings revision (`if_rev`).
    pub rev: u32,
    /// Last change.
    pub updated_at: Timestamp,
}

/// One explicit channel or DM pref (`notify_prefs.channel_prefs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifyChannelPrefRecord {
    /// The channel.
    pub channel_id: Id,
    /// `all`, `mentions` or `none` (absent: inherit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Muted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// When a timed mute ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<Timestamp>,
    /// Last change.
    pub updated_at: Timestamp,
}

/// One explicit space level (`notify_prefs.space_prefs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifySpacePrefRecord {
    /// The space.
    pub space_id: Id,
    /// `all`, `mentions` or `none`.
    pub level: String,
    /// Last change.
    pub updated_at: Timestamp,
}

/// `audit.ndjson`: one audit row, or (`pending`) one outbox row not chained at the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    /// The chain position (absent for pending rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_seq: Option<u64>,
    /// Previous hash, hex (absent for pending rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
    /// This row's hash, hex (absent for pending rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// An outbox row.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
    /// The exact canonical event string that was hashed.
    pub event_json: String,
}

/// `retention_policies.ndjson` (S22's wire shape).
pub type RetentionPolicyRecord = RetentionPolicy;
/// `legal_holds.ndjson`, `holds_in_scope.ndjson` (S22's wire shape).
pub type LegalHoldRecord = LegalHold;

/// Every entity entry name, in tar order (prd-sew-23 §8.3). Message and revision parts use the
/// `messages/` and `revisions/` prefixes.
pub const ENTITY_ORDER: &[&str] = &[
    "install.ndjson",
    "profile_fields.ndjson",
    "members.ndjson",
    "presence_settings.ndjson",
    "spaces.ndjson",
    "roles.ndjson",
    "space_members.ndjson",
    "user_groups.ndjson",
    "categories.ndjson",
    "channels.ndjson",
    "channel_members.ndjson",
    "channel_past_members.ndjson",
    "emoji.ndjson",
    "messages/*/*.ndjson",
    "revisions/*/*.ndjson",
    "bookmarks.ndjson",
    "thread_follows.ndjson",
    "read_states.ndjson",
    "thread_reads.ndjson",
    "mentions.ndjson",
    "saved_items.ndjson",
    "reminders.ndjson",
    "drafts.ndjson",
    "scheduled.ndjson",
    "recordings.ndjson",
    "notify_prefs.ndjson",
    "retention_policies.ndjson",
    "legal_holds.ndjson",
    "holds_in_scope.ndjson",
    "audit.ndjson",
];

// ---------------------------------------------------------------- S35 v2 recordings

coded_enum! {
    /// A recording segment's state (`recording_segments.state`, prd-sew-35 v2 §10).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SegmentState {
        /// Stored; awaiting transcription.
        Stored = "stored" => 1,
        /// Transcribed.
        Transcribed = "transcribed" => 2,
        /// No speech.
        Empty = "empty" => 3,
        /// Transcription failed (or the audio went first).
        Failed = "failed" => 4,
        /// Every piece posted.
        Posted = "posted" => 5,
    }
}

coded_enum! {
    /// Why a recorder run ended (`recording_runs.end_reason`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RunEnd {
        /// The recording stopped.
        Stopped = "stopped" => 1,
        /// The recorder restarted.
        Restarted = "restarted" => 2,
        /// The recorder rejoined with a new admit.
        Reconnected = "reconnected" => 3,
        /// The run failed.
        Failed = "failed" => 4,
    }
}

/// One participant's consent record (`recording_participants`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingParticipantRecord {
    /// The member.
    pub member_id: Id,
    /// What the person said.
    pub consent: crate::recording::Consent,
    /// Where they said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<crate::recording::ConsentVia>,
    /// Whether their audio was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<crate::recording::Capture>,
    /// Why not, when not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_reason: Option<crate::recording::NotRecordedReason>,
    /// First seen.
    pub first_at: Timestamp,
    /// When they answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<Timestamp>,
    /// Last change.
    pub updated_at: Timestamp,
    /// In the room at the snapshot.
    #[serde(default, skip_serializing_if = "is_false")]
    pub in_room: bool,
    /// Ever granted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ever_granted: bool,
    /// The prompt version the answer echoed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_version: Option<u64>,
    /// The surface of the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<crate::recording::ConsentSurface>,
}

/// One recorder run (`recording_runs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingRunRecord {
    /// From 1.
    pub run: u16,
    /// Started.
    pub started_at: Timestamp,
    /// Joined the room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<Timestamp>,
    /// Ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<RunEnd>,
}

/// One speaker's track progress (`recording_tracks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingTrackRecord {
    /// The speaker.
    pub member_id: Id,
    /// Captured through (ms from the start).
    pub through_ms: u64,
    /// Open at the snapshot.
    #[serde(default, skip_serializing_if = "is_false")]
    pub open: bool,
    /// Last progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<Timestamp>,
}

/// One missing span (`recording_gaps`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingGapRecord {
    /// The speaker (absent: everyone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<Id>,
    /// From (ms from the start).
    pub from_ms: u64,
    /// To.
    pub to_ms: u64,
    /// Why.
    pub reason: crate::recording::GapReason,
}

/// One stored segment's metadata (`recording_segments`, without `result_ct`). Its audio is
/// `files/<file>`: the **recorder-sealed** Ogg (S4 stream format under the segment key derived
/// from the recording's `content_key`, run, `join_id` and `seq`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingSegmentRecord {
    /// The segment.
    pub id: Id,
    /// The speaker.
    pub member_id: Id,
    /// The recorder run.
    pub run: u16,
    /// The identity's join (32 hex; an input of the segment key).
    pub join_id: String,
    /// Per run and identity, from 1.
    pub seq: u32,
    /// Offset from the recording start.
    pub start_ms: u64,
    /// Length.
    pub duration_ms: u32,
    /// Speech in it.
    pub speech_ms: u32,
    /// Mean level of its speech frames (dBov).
    pub level_dbov: f32,
    /// The sealed audio (absent once purged, or when the archive carries no files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Id>,
    /// Sealed size.
    pub bytes: u32,
    /// Plaintext size.
    pub plain_bytes: u32,
    /// Its state.
    pub state: SegmentState,
    /// Counted attempts.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub attempts: u16,
    /// The last error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Detected language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Its probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang_prob: Option<f32>,
    /// Pieces posted through.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub posted_through_ms: u64,
    /// Ingest time.
    pub created_at: Timestamp,
}

/// One mixed part (`recording_mixes`); its audio is `files/<file>`, recorder-sealed under the
/// part key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingMixRecord {
    /// From 1.
    pub part: u16,
    /// Offset from the recording start.
    pub offset_ms: u64,
    /// Length.
    pub duration_ms: u64,
    /// The sealed audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Id>,
    /// Sealed size.
    pub bytes: u64,
}

/// One transcript turn's timing (`transcript_turns`; the text is its message's body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptTurnRecord {
    /// The turn message.
    pub message_id: Id,
    /// The speaker.
    pub speaker_id: Id,
    /// Start (ms from the recording start).
    pub start_ms: u64,
    /// End.
    pub end_ms: u64,
    /// When it was said.
    pub spoken_at: Timestamp,
    /// Its language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// The pieces (numbers only).
    pub pieces: serde_json::Value,
}

/// `recordings.ndjson` (prd-sew-35 v2 §10.1): one call recording with its consent record, runs,
/// tracks, gaps, segment and mix metadata, and turn timings. The audio leaves only as
/// recorder-sealed `files/…` plus `content_key`, and only inside the age layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingRecord {
    /// The recording.
    pub id: Id,
    /// The voice channel or DM.
    pub channel_id: Id,
    /// The DM call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_record_id: Option<Id>,
    /// The record message.
    pub message_id: Id,
    /// The starter.
    pub started_by: Id,
    /// When.
    pub started_at: Timestamp,
    /// Auto-started.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto: bool,
    /// Its state (an import loads active ones `stopped`, `disabled`).
    pub state: crate::recording::RecordingState,
    /// When it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<Timestamp>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<crate::recording::StopReason>,
    /// When the recorder first joined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_joined_at: Option<Timestamp>,
    /// Recorder runs.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub recorder_runs: u16,
    /// The language hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// A mixed file was asked for.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mixed: bool,
    /// Audio keep days.
    pub audio_keep_days: u16,
    /// The audio's state.
    pub audio_state: crate::recording::AudioState,
    /// When the audio expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_expires_at: Option<Timestamp>,
    /// Stored sealed bytes.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub audio_bytes: u64,
    /// The transcript's state.
    pub transcript_state: crate::recording::TranscriptState,
    /// When it became final.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_final_at: Option<Timestamp>,
    /// The merge frontier.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub posted_through_ms: u64,
    /// Turns posted.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub turn_count: u32,
    /// The transcriber's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Its backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// The record message was deleted or expired.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// When the purge hook stubbed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purged_at: Option<Timestamp>,
    /// The 32-byte content key, base64url without padding: only when the archive carries the
    /// audio files and the audio is not purged (never outside the age layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_key: Option<String>,
    /// The consent record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<RecordingParticipantRecord>,
    /// Recorder runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RecordingRunRecord>,
    /// Track progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<RecordingTrackRecord>,
    /// Gaps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<RecordingGapRecord>,
    /// Segments (metadata; audio in `files/`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<RecordingSegmentRecord>,
    /// Mixed parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mixes: Vec<RecordingMixRecord>,
    /// Transcript turn timings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<TranscriptTurnRecord>,
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
