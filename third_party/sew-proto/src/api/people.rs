//! People (slice S18, prd-sew-18 §14): profiles, custom fields, status, presence settings, DND
//! schedules, the directory, user groups and the mention namespace.
//!
//! The server re-checks every text rule (trimming, control characters, IANA zones, emoji); the
//! validators here check shapes, counts and lengths.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Handle;
use crate::api::members::{Member, MemberKind};
use crate::error::{
    Validate, ValidationError, check_chars, check_known, check_opt_chars, check_unique_ids,
};
use crate::gateway::events::PresenceStatus;
use crate::http::{Page, Patch};
use crate::id::{Id, IdList};
use crate::time::Timestamp;

/// Most custom fields a profile write names (and most live field definitions, S18 §7.3).
pub const PROFILE_FIELDS_MAX: usize = 50;
/// Longest custom field value, in characters.
pub const PROFILE_FIELD_VALUE_MAX_CHARS: usize = 500;
/// Longest status text, in characters.
pub const STATUS_TEXT_MAX_CHARS: usize = 100;
/// Longest status emoji, in bytes.
pub const STATUS_EMOJI_MAX_BYTES: usize = 64;
/// Most rules in a DND schedule.
pub const DND_RULES_MAX: usize = 14;
/// Most ids per group add or remove call.
pub const GROUP_MEMBERS_PER_CALL_MAX: usize = 100;
/// Longest directory query.
pub const DIRECTORY_Q_MAX_CHARS: usize = 80;

/// `details.reason` values S18 adds (prd-sew-18 §14).
pub mod reasons {
    /// A K2-managed (or provider) field was named in a client write.
    pub const MANAGED_BY_PROVIDER: &str = "managed_by_provider";
    /// The avatar body is not PNG, JPEG, WebP or GIF.
    pub const UNSUPPORTED_IMAGE: &str = "unsupported_image";
    /// An avatar side is larger than 4096 px.
    pub const IMAGE_TOO_LARGE: &str = "image_too_large";
    /// The avatar did not decode.
    pub const BAD_IMAGE: &str = "bad_image";
    /// A custom emoji in a status.
    pub const CUSTOM_EMOJI_NOT_SUPPORTED: &str = "custom_emoji_not_supported";
    /// A reserved group handle (`here`, `channel`, `everyone`, `all`).
    pub const RESERVED_HANDLE: &str = "reserved_handle";
    /// A group handle equal to a live group's handle or an active username.
    pub const HANDLE_TAKEN: &str = "handle_taken";
    /// A space group member who is not in the space.
    pub const TARGET_NOT_SPACE_MEMBER: &str = "target_not_space_member";
    /// A guest named as a group member.
    pub const GUEST_NOT_ALLOWED: &str = "guest_not_allowed";
    /// An assistant named as a group member.
    pub const ASSISTANT_MEMBERSHIP: &str = "assistant_membership";
    /// A deactivated member named as a group member.
    pub const TARGET_DEACTIVATED: &str = "target_deactivated";
    /// An agent tried to set presence or DND.
    pub const AGENT_PRESENCE_FIXED: &str = "agent_presence_fixed";
    /// Mgmt named a member-managed custom field.
    pub const FIELD_NOT_PROVIDER_MANAGED: &str = "field_not_provider_managed";
    /// Mgmt named a custom field key that does not exist.
    pub const UNKNOWN_FIELD: &str = "unknown_field";
    /// A K2 avatar push found the member's own avatar and left it.
    pub const MEMBER_AVATAR_KEPT: &str = "member_avatar_kept";
    /// An upsert tried to change a live field's kind.
    pub const FIELD_KIND_FIXED: &str = "field_kind_fixed";
    /// A signed URL is past its `exp` (contracts rev 28).
    pub const URL_EXPIRED: &str = "url_expired";
    /// A signed URL's signature or path does not check out (contracts rev 28).
    pub const URL_INVALID: &str = "url_invalid";
}

// ---------------------------------------------------------------- status

/// A member's status: text and/or one emoji, with an optional expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberStatus {
    /// Status text, 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// One Unicode emoji sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// When it clears by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// Body of `POST /api/v1/me/status`. At least one of `text`, `emoji`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetStatusRequest {
    /// Status text, 1–100 characters after trimming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// One Unicode emoji sequence (≤ 64 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Absent = never. After now and at most 366 days ahead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

impl Validate for SetStatusRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.text.is_none() && self.emoji.is_none() {
            return Err(ValidationError::invalid(
                "text",
                "a status needs text or an emoji",
            ));
        }
        check_opt_chars("text", self.text.as_deref(), 0, 1_000)?;
        if let Some(e) = &self.emoji
            && (e.is_empty() || e.len() > STATUS_EMOJI_MAX_BYTES)
        {
            return Err(ValidationError::invalid(
                "emoji",
                format!("emoji must be 1–{STATUS_EMOJI_MAX_BYTES} bytes"),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- profiles

/// One custom field value in a profile write (`value: null` clears it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomFieldInput {
    /// The field definition.
    pub field_id: Id,
    /// The value; `null` clears.
    pub value: Option<String>,
}

/// Body of `POST /api/v1/me/profile`. At least one field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMyProfileRequest {
    /// A new display name (standalone only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Set or clear the title (standalone only).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub title: Patch<String>,
    /// Set or clear the pronouns (standalone only).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub pronouns: Patch<String>,
    /// Set or clear the IANA timezone.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub timezone: Patch<String>,
    /// Custom field values (at most 50).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_fields: Vec<CustomFieldInput>,
}

impl UpdateMyProfileRequest {
    /// Whether the request names no field.
    pub fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.title.is_keep()
            && self.pronouns.is_keep()
            && self.timezone.is_keep()
            && self.custom_fields.is_empty()
    }
}

impl Validate for UpdateMyProfileRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.custom_fields.len() > PROFILE_FIELDS_MAX {
            return Err(ValidationError::invalid(
                "custom_fields",
                format!("at most {PROFILE_FIELDS_MAX} custom fields"),
            ));
        }
        let ids: Vec<Id> = self.custom_fields.iter().map(|c| c.field_id).collect();
        check_unique_ids("custom_fields", &ids, 0, PROFILE_FIELDS_MAX)?;
        Ok(())
    }
}

unit_enum! {
    /// A custom field's value kind.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ProfileFieldKind {
        /// Free text (1–500 characters).
        Text = "text",
        /// An `http(s)` URL.
        Url = "url",
        /// One of the field's options.
        Select = "select",
        /// `YYYY-MM-DD`.
        Date = "date",
    }
}

unit_enum! {
    /// Who writes a custom field.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ProfileFieldManagedBy {
        /// The member.
        Member = "member",
        /// The identity provider (mgmt only).
        Provider = "provider",
    }
}

unit_enum! {
    /// Who sees a custom field.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ProfileFieldVisibility {
        /// Everyone who can see the member, guests included.
        Everyone = "everyone",
        /// Everyone but guests.
        NonGuests = "non_guests",
        /// Install owner and admins, and the member themself.
        Admins = "admins",
    }
}

/// A custom field definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileField {
    /// The field id.
    pub id: Id,
    /// `[a-z][a-z0-9_]{0,39}`, fixed after create.
    pub key: String,
    /// Label, 1–60 characters.
    pub label: String,
    /// Value kind, fixed after create.
    pub kind: ProfileFieldKind,
    /// `select` options (empty for other kinds).
    #[serde(default)]
    pub options: Vec<String>,
    /// Who writes it.
    pub managed_by: ProfileFieldManagedBy,
    /// Who sees it.
    pub visibility: ProfileFieldVisibility,
    /// Display order.
    pub position: u32,
}

/// One custom field value on a `Member`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileFieldValue {
    /// The field.
    pub field_id: Id,
    /// The field's key.
    pub key: String,
    /// The value.
    pub value: String,
}

fn check_field_key(key: &str) -> Result<(), ValidationError> {
    let mut chars = key.chars();
    let ok = key.len() <= 40
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ValidationError::invalid(
            "key",
            "key must match [a-z][a-z0-9_]{0,39}",
        ))
    }
}

fn check_options(options: &[String]) -> Result<(), ValidationError> {
    if options.is_empty() || options.len() > 50 {
        return Err(ValidationError::invalid(
            "options",
            "options must hold 1–50 items",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for (i, o) in options.iter().enumerate() {
        check_chars(&format!("options[{i}]"), o, 1, 100)?;
        if !seen.insert(o) {
            return Err(ValidationError::invalid(
                format!("options[{i}]"),
                "options must be unique",
            ));
        }
    }
    Ok(())
}

/// Body of `POST /api/v1/install/profile-fields`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProfileFieldRequest {
    /// `[a-z][a-z0-9_]{0,39}`.
    pub key: String,
    /// Label, 1–60 characters.
    pub label: String,
    /// Value kind.
    pub kind: ProfileFieldKind,
    /// `select` only: 1–50 options of 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Who writes it.
    pub managed_by: ProfileFieldManagedBy,
    /// Who sees it.
    pub visibility: ProfileFieldVisibility,
    /// Display order (default: last).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl Validate for CreateProfileFieldRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_field_key(&self.key)?;
        check_chars("label", self.label.trim(), 1, 60)?;
        check_known("kind", self.kind)?;
        check_known("managed_by", self.managed_by)?;
        check_known("visibility", self.visibility)?;
        match (&self.options, self.kind) {
            (Some(o), ProfileFieldKind::Select) => check_options(o),
            (None, ProfileFieldKind::Select) => Err(ValidationError::invalid(
                "options",
                "a select field needs options",
            )),
            (Some(_), _) => Err(ValidationError::invalid(
                "options",
                "only select fields have options",
            )),
            (None, _) => Ok(()),
        }
    }
}

/// Body of `POST /mgmt/v1/profile-fields/upsert` (create or update by `key`).
pub type UpsertProfileFieldRequest = CreateProfileFieldRequest;

/// Body of `POST /api/v1/install/profile-fields/{id}/update`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileFieldRequest {
    /// A new label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// New `select` options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// New visibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<ProfileFieldVisibility>,
    /// New position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl Validate for UpdateProfileFieldRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.label.is_none()
            && self.options.is_none()
            && self.visibility.is_none()
            && self.position.is_none()
        {
            return Err(ValidationError::invalid("body", "name at least one field"));
        }
        if let Some(l) = &self.label {
            check_chars("label", l.trim(), 1, 60)?;
        }
        if let Some(o) = &self.options {
            check_options(o)?;
        }
        if let Some(v) = self.visibility {
            check_known("visibility", v)?;
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/install/profile-fields/reorder`: every live field, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderProfileFieldsRequest {
    /// Field ids in their new order.
    pub field_ids: Vec<Id>,
}

impl Validate for ReorderProfileFieldsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("field_ids", &self.field_ids, 0, PROFILE_FIELDS_MAX)
    }
}

/// `GET /api/v1/install/profile-fields` and the reorder response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileFieldsResponse {
    /// Live fields by position.
    pub items: Vec<ProfileField>,
    /// The etag (`fields_rev`).
    pub version: String,
}

unit_enum! {
    /// What an admin can reset on another member's profile.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ResettableField {
        /// The avatar.
        Avatar = "avatar",
        /// The status.
        Status = "status",
    }
}

/// Body of `POST /api/v1/members/{id}/profile/reset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetProfileRequest {
    /// 1–2 unique fields.
    pub fields: Vec<ResettableField>,
}

impl Validate for ResetProfileRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.fields.is_empty() || self.fields.len() > 2 {
            return Err(ValidationError::invalid("fields", "name 1–2 fields"));
        }
        for f in &self.fields {
            check_known("fields", *f)?;
        }
        if self.fields.len() == 2 && self.fields[0] == self.fields[1] {
            return Err(ValidationError::invalid("fields", "fields must be unique"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- presence and DND

unit_enum! {
    /// A member's manual presence state (persisted).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ManualPresence {
        /// Automatic.
        None = "none",
        /// Shown as idle.
        Away = "away",
        /// Do not disturb.
        Dnd = "dnd",
        /// Shown offline to others.
        Invisible = "invisible",
    }
}

unit_enum! {
    /// What a member sees about their own presence.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SelfStatus {
        /// Online.
        Online = "online",
        /// Idle or away.
        Idle = "idle",
        /// Do not disturb.
        Dnd = "dnd",
        /// Invisible (others see offline).
        Invisible = "invisible",
        /// No live session.
        Offline = "offline",
    }
}

unit_enum! {
    /// A weekday of a DND rule.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DndDay {
        /// Monday.
        Mon = "mon",
        /// Tuesday.
        Tue = "tue",
        /// Wednesday.
        Wed = "wed",
        /// Thursday.
        Thu = "thu",
        /// Friday.
        Fri = "fri",
        /// Saturday.
        Sat = "sat",
        /// Sunday.
        Sun = "sun",
    }
}

/// One weekly DND rule: from `start` on each listed day to `end` (the next day when `end <
/// start`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DndRule {
    /// 1–7 unique days.
    pub days: Vec<DndDay>,
    /// `HH:MM`, `00:00`–`23:59`.
    pub start: String,
    /// `HH:MM`, `00:01`–`24:00`.
    pub end: String,
}

/// A recurring weekly DND schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DndSchedule {
    /// IANA override; absent = the member's profile timezone, else UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// 1–14 rules.
    pub rules: Vec<DndRule>,
}

unit_enum! {
    /// Why DND is on.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum DndSource {
        /// Off.
        None = "none",
        /// Set by hand.
        Manual = "manual",
        /// A schedule window.
        Schedule = "schedule",
    }
}

/// Whether a member is on DND now, and until when (S21 holds notifications until `until`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DndState {
    /// On now.
    pub active: bool,
    /// Why.
    pub source: DndSource,
    /// When it ends; absent = indefinite (or off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
}

/// A member's presence settings and what they see about themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSettings {
    /// The manual state.
    pub manual: ManualPresence,
    /// When the manual state ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_until: Option<Timestamp>,
    /// What the member sees about themself.
    pub self_status: SelfStatus,
    /// DND now.
    pub dnd: DndState,
    /// The recurring schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd_schedule: Option<DndSchedule>,
    /// The current schedule window is skipped until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd_skip_until: Option<Timestamp>,
    /// The timezone schedules use (`UTC` when none is set).
    pub effective_timezone: String,
}

/// Body of `POST /api/v1/me/presence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPresenceRequest {
    /// The manual state.
    pub manual: ManualPresence,
    /// When it ends (only with `manual ≠ none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
}

impl Validate for SetPresenceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("manual", self.manual)?;
        if self.manual == ManualPresence::None && self.until.is_some() {
            return Err(ValidationError::invalid(
                "until",
                "until needs a manual state other than none",
            ));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/me/dnd/schedule` (`schedule: null` removes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDndScheduleRequest {
    /// The schedule, or `null`.
    pub schedule: Option<DndSchedule>,
}

impl Validate for SetDndScheduleRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // Rules are checked by the server, which names the failing path
        // (`schedule.rules[i].<field>`, prd-sew-18 §15.3).
        Ok(())
    }
}

/// One member's shown presence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceEntry {
    /// The member.
    pub member_id: Id,
    /// Shown status.
    pub status: PresenceStatus,
}

/// `GET /api/v1/presence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceList {
    /// In request order.
    pub items: Vec<PresenceEntry>,
}

/// Query of `GET /api/v1/presence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresenceQuery {
    /// 1–100 member ids.
    pub ids: IdList,
}

impl Validate for PresenceQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

// ---------------------------------------------------------------- directory

/// Query of `GET /api/v1/directory`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectoryQuery {
    /// 1–80 characters; every token prefix-matches a name, title or username word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Only members of this space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Only members of this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// Persons or agents only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MemberKind>,
    /// `next_cursor` of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for DirectoryQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        // `space_id` with `channel_id` is refused by the server (400, field `channel_id`).
        check_opt_chars("q", self.q.as_deref(), 1, DIRECTORY_Q_MAX_CHARS)?;
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        check_opt_chars("cursor", self.cursor.as_deref(), 1, 400)?;
        crate::api::spaces::check_page_limit(self.limit)
    }
}

/// One directory row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryEntry {
    /// The member (no custom fields).
    pub member: Member,
    /// Shown presence for the caller.
    pub presence: PresenceStatus,
}

/// `GET /api/v1/directory`: ordered by name, then id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryPage {
    /// The rows.
    pub items: Vec<DirectoryEntry>,
    /// Pass as `cursor` for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// More rows follow.
    pub has_more: bool,
}

/// Query of `GET /api/v1/members`: exactly one of `ids`, `username`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MembersQuery {
    /// 1–100 member ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdList>,
    /// One handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<Handle>,
}

impl Validate for MembersQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        // Exactly one of the two is the server's check (400), so a full fixture can set both.
        Ok(())
    }
}

// ---------------------------------------------------------------- user groups

/// A user group: a named mention list, install-wide (`space_id` absent) or per space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserGroup {
    /// The group id.
    pub id: Id,
    /// The space (absent = install-wide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// `@handle`.
    pub handle: Handle,
    /// Name, 1–100 characters.
    pub name: String,
    /// Description, 1–250 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Anyone who can post may mention it.
    pub mentionable: bool,
    /// Current members.
    pub member_count: u32,
    /// Creator.
    pub created_by: Id,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Soft-deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
}

fn check_group_name(name: &str) -> Result<(), ValidationError> {
    check_chars("name", name.trim(), 1, 100)
}

fn check_description(d: Option<&str>) -> Result<(), ValidationError> {
    match d {
        Some(d) => check_chars("description", d.trim(), 1, 250),
        None => Ok(()),
    }
}

/// Body of `POST /api/v1/groups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUserGroupRequest {
    /// A space group (absent = install-wide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// `@handle`.
    pub handle: Handle,
    /// Name.
    pub name: String,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Initial members (at most 100).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<Id>,
}

impl Validate for CreateUserGroupRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_group_name(&self.name)?;
        check_description(self.description.as_deref())?;
        check_unique_ids(
            "member_ids",
            &self.member_ids,
            0,
            GROUP_MEMBERS_PER_CALL_MAX,
        )
    }
}

/// Body of `POST /api/v1/groups/{id}/update`. At least one field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserGroupRequest {
    /// A new handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<Handle>,
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Set or clear the description.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub description: Patch<String>,
    /// Mentionable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
}

impl Validate for UpdateUserGroupRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.handle.is_none()
            && self.name.is_none()
            && self.description.is_keep()
            && self.mentionable.is_none()
        {
            return Err(ValidationError::invalid("body", "name at least one field"));
        }
        if let Some(n) = &self.name {
            check_group_name(n)?;
        }
        check_description(self.description.as_set().map(String::as_str))
    }
}

/// Body of `POST /api/v1/groups/{id}/members/add|remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupMembersRequest {
    /// 1–100 unique member ids.
    pub member_ids: Vec<Id>,
}

impl Validate for GroupMembersRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids(
            "member_ids",
            &self.member_ids,
            1,
            GROUP_MEMBERS_PER_CALL_MAX,
        )
    }
}

/// Response of `POST /api/v1/groups/{id}/members/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddGroupMembersResponse {
    /// Newly added.
    pub added: Vec<Id>,
    /// Already members.
    pub already: Vec<Id>,
    /// The group after the change.
    pub group: UserGroup,
}

/// Response of `POST /api/v1/groups/{id}/members/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveGroupMembersResponse {
    /// Removed.
    pub removed: Vec<Id>,
    /// Were not members.
    pub absent: Vec<Id>,
    /// The group after the change.
    pub group: UserGroup,
}

/// One group membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GroupMember {
    /// The group.
    pub group_id: Id,
    /// The member.
    pub member_id: Id,
    /// When added.
    pub added_at: Timestamp,
}

/// `GET /api/v1/groups/{id}/members`, keyed by `member_id`.
pub type GroupMemberPage = Page<GroupMember>;

/// Query of `GET /api/v1/groups`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupsQuery {
    /// Also this space's groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
}

impl Validate for GroupsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// `GET /api/v1/groups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserGroupsResponse {
    /// Live groups.
    pub items: Vec<UserGroup>,
    /// `"<install_groups_rev>.<space groups_rev>"` (the `ETag`).
    pub version: String,
}

unit_enum! {
    /// What a mention target is backed by.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MentionableKind {
        /// A space role with `mentionable` (or the caller may mention any role).
        Role = "role",
        /// A user group.
        Group = "group",
        /// `here` or `channel`.
        Special = "special",
    }
}

/// One mention target for autocomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Mentionable {
    /// Role, group or special.
    pub kind: MentionableKind,
    /// Role or group id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// Group handle or special name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Display name.
    pub name: String,
    /// Group members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_count: Option<u32>,
    /// The space of a role or space group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
}

/// Query of `GET /api/v1/channels/{id}/mentionables`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MentionablesQuery {
    /// Prefix of the folded handle or name (≤ 64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// 1–50 (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for MentionablesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check_opt_chars("q", self.q.as_deref(), 0, 64)?;
        if let Some(l) = self.limit
            && !(1..=50).contains(&l)
        {
            return Err(ValidationError::invalid("limit", "limit must be 1–50"));
        }
        Ok(())
    }
}

/// `GET /api/v1/channels/{id}/mentionables`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MentionablesResponse {
    /// Specials, then groups, then roles.
    pub items: Vec<Mentionable>,
}

// ---------------------------------------------------------------- gateway bodies

/// Op 5's presence answer: one entry per subscribed id, in request order (always sent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSnapshot {
    /// The entries.
    pub items: Vec<PresenceEntry>,
}

/// One member-list row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberListItem {
    /// The member (no custom fields).
    pub member: Member,
    /// Shown presence.
    pub presence: PresenceStatus,
}

/// One member-list operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemberListOp {
    /// The rows at `range[0]..=min(range[1], member_count - 1)`.
    Sync {
        /// Inclusive range.
        range: [u32; 2],
        /// The rows.
        items: Vec<MemberListItem>,
    },
    /// One row changed in place.
    Update {
        /// The row.
        index: u32,
        /// The new row.
        item: Box<MemberListItem>,
    },
}

/// `member_list_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberListUpdated {
    /// The channel.
    pub channel_id: Id,
    /// Active members.
    pub member_count: u32,
    /// Access lost: the subscription is dropped.
    #[serde(default)]
    pub gone: bool,
    /// Operations in order.
    pub ops: Vec<MemberListOp>,
}

/// `user_group_deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserGroupDeleted {
    /// The group.
    pub id: Id,
    /// Its space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
}

/// `profile_fields_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileFieldsUpdated {
    /// The new `fields_rev`.
    pub version: String,
}

// ---------------------------------------------------------------- mgmt

/// A provider custom field value in provision/update (`value: null` clears).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtCustomFieldInput {
    /// The field key.
    pub key: String,
    /// The value; `null` clears.
    pub value: Option<String>,
}

/// Query of `POST /mgmt/v1/principals/avatar` (the body is the raw image).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtAvatarQuery {
    /// The principal.
    pub external_id: crate::external_id::ExternalId,
    /// Replace a member-chosen avatar too.
    #[serde(default)]
    pub force: bool,
}

impl Validate for MgmtAvatarQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /mgmt/v1/principals/avatar/clear`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtAvatarClearRequest {
    /// The principal.
    pub external_id: crate::external_id::ExternalId,
    /// Clear a member-chosen avatar too.
    #[serde(default)]
    pub force: bool,
}

impl Validate for MgmtAvatarClearRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response of the mgmt avatar routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtAvatarResponse {
    /// The member.
    pub member_id: Id,
    /// Whether the avatar changed.
    pub applied: bool,
    /// Why not (`member_avatar_kept`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
