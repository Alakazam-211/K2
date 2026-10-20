//! Meet MC1: meetings for members (prd-meet-01 §6.2, §6.3, §6.14.2; CC4–CC10).
//!
//! A meeting is a private voice channel in the install's hidden Meetings Space plus one
//! `meetings` row. Its title is sealed under the channel's key; attendee emails are sealed too,
//! readable by the organiser only, and purged 30 days after the meeting ends (or at cancel).
//! Times are UTC instants with the IANA zone they were chosen in (`tz`); `ends_at` is the
//! planned end (calendar and the end job), never a cut-off of a live meeting.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError, check_chars, check_known};
use crate::id::Id;
use crate::limits::{EMAIL_MAX_BYTES, PAGE_LIMIT_MAX};
use crate::time::Timestamp;

/// Longest meeting title, in characters.
pub const MEETING_TITLE_MAX_CHARS: usize = 200;
/// Longest `tz` (an IANA zone name), in bytes.
pub const MEETING_TZ_MAX_BYTES: usize = 64;
/// Most attendees per meeting (members and emails together).
pub const MEETING_ATTENDEES_MAX: usize = 100;
/// Members may join a meeting's voice from this many minutes before `starts_at`.
pub const MEETING_JOIN_EARLY_MINUTES: i64 = 15;
/// Nothing lives past `starts_at` + this many hours (`hard_end_at`).
pub const MEETING_HARD_CAP_HOURS: i64 = 12;
/// An instant meeting's planned length.
pub const MEETING_INSTANT_MINUTES: i64 = 60;
/// Attendee emails are purged this many days after `ended_at` (or at cancel).
pub const MEETING_ATTENDEE_PURGE_DAYS: i64 = 30;
/// An ended meeting's channel is archived this many days after `ended_at`, once no recording
/// of it is still in progress.
pub const MEETING_ARCHIVE_DAYS: i64 = 7;
/// The Meetings Space's slug (`POST /mgmt/v1/meet/init`).
pub const MEETINGS_SPACE_SLUG: &str = "meetings";
/// The Meetings Space's name.
pub const MEETINGS_SPACE_NAME: &str = "Meetings";
/// Chat and transcripts in the Meetings Space are kept this many days by default (§13 Q6).
pub const MEETINGS_RETENTION_DAYS: u32 = 365;
/// Meeting audio is kept this many days by default (§13 Q6; each meeting channel's
/// `recording_audio_days`).
pub const MEETINGS_AUDIO_DAYS: u16 = 30;

/// `details.reason` values of the meeting routes.
pub mod reasons {
    /// `422 invalid_state`: a generic channel route on a meeting channel, or a generic channel
    /// create in a hidden Space (§6.7.1).
    pub const MEETING_CHANNEL: &str = "meeting_channel";
    /// `409 conflict`: the meeting has ended or was cancelled.
    pub const MEETING_CLOSED: &str = "meeting_closed";
    /// `422 invalid_state`: only a scheduled meeting can be cancelled.
    pub const NOT_SCHEDULED: &str = "not_scheduled";
    /// `422 invalid_state`: an app-bound Meetings Space needs attendees who are its members.
    pub const NOT_SPACE_MEMBER: &str = "not_space_member";
    /// `422 invalid_state`: an attendee who cannot be invited (an agent, deactivated, or a guest
    /// outside the Meetings Space).
    pub const ATTENDEE_NOT_ELIGIBLE: &str = "attendee_not_eligible";
    /// `400`: `ends_at` − `starts_at` is not 1 minute to `meetings.max_duration_minutes`.
    pub const BAD_DURATION: &str = "bad_duration";
    /// `400`: `tz` is not an IANA zone name.
    pub const BAD_TIMEZONE: &str = "bad_timezone";
    /// `400`: a scheduled meeting starts more than a minute in the past.
    pub const STARTS_IN_PAST: &str = "starts_in_past";
    /// `403 policy_denied`: `meetings.enabled` is off.
    pub const MEETINGS_DISABLED: &str = "meetings_disabled";
    /// `403 policy_denied`: `meetings.create` is `admins` and the caller is not one.
    pub const MEETINGS_CREATE_ADMINS: &str = "meetings_create_admins";
    /// `503 unavailable`: `POST /mgmt/v1/meet/init` has not run on this install.
    pub const MEET_NOT_INITIALISED: &str = "meet_not_initialised";
    /// `403 missing_permission`: the organiser only (attendee emails).
    pub const ORGANISER_ONLY: &str = "organiser_only";
    /// `422 invalid_state`: End for all needs a live meeting (a scheduled one is cancelled).
    pub const NOT_LIVE: &str = "not_live";
    /// `422 invalid_state`: the organiser cannot be removed from their meeting (MC3).
    pub const ORGANISER_NOT_REMOVABLE: &str = "organiser_not_removable";
}

/// Meet MC3 (§6.7): bucket `meeting_mod`, host actions per host per window.
pub const MEETING_MOD_LIMIT: u32 = 30;
/// Meet MC3 (§6.7): the `meeting_mod` window, in seconds.
pub const MEETING_MOD_WINDOW_SECONDS: u32 = 10;

unit_enum! {
    /// How a meeting started.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingKind {
        /// Start now: `live` at create, planned for 60 minutes.
        Instant = "instant",
        /// For later: `scheduled` until the first voice join.
        Scheduled = "scheduled",
    }
}

unit_enum! {
    /// A meeting's state (§6.3).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingState {
        /// Not started; members may join from `starts_at` − 15 min.
        Scheduled = "scheduled",
        /// Someone joined (or it was instant).
        Live = "live",
        /// Over; voice joins answer `410 meeting_ended`.
        Ended = "ended",
        /// Cancelled before it started.
        Cancelled = "cancelled",
    }
}

unit_enum! {
    /// Why a meeting ended.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingEndReason {
        /// A host ended it for everyone (MC3).
        Host = "host",
        /// The room stayed empty for `meetings.grace_minutes` after `ends_at`.
        EmptyAfterGrace = "empty_after_grace",
        /// `hard_end_at` (`starts_at` + 12 h) passed.
        HardCap = "hard_cap",
        /// The organiser was deactivated and no other host remains.
        OrganiserGone = "organiser_gone",
        /// Cancelled before it started.
        Cancelled = "cancelled",
    }
}

unit_enum! {
    /// An attendee's kind.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingAttendeeKind {
        /// A member of the install: added to the meeting channel (so they skip the lobby).
        Member = "member",
        /// An email address for the calendar file only; never an account.
        Email = "email",
    }
}

unit_enum! {
    /// Which meetings a list returns.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingListScope {
        /// `scheduled` and `live`, soonest first.
        Upcoming = "upcoming",
        /// `ended` and `cancelled`, latest first.
        Past = "past",
    }
}

/// One attendee as given to create and update: `member_id` for `member`, `email` for `email`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeetingAttendeeInput {
    /// `member` or `email`.
    pub kind: MeetingAttendeeKind,
    /// The member (`member` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<Id>,
    /// The address (`email` only; at most 254 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

fn check_email(field: &str, email: &str) -> Result<(), ValidationError> {
    let bad = email.len() > EMAIL_MAX_BYTES
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
        || match email.split_once('@') {
            Some((local, domain)) => local.is_empty() || domain.is_empty() || domain.contains('@'),
            None => true,
        };
    if bad {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must be one address of at most {EMAIL_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn check_attendees(attendees: &[MeetingAttendeeInput]) -> Result<(), ValidationError> {
    if attendees.len() > MEETING_ATTENDEES_MAX {
        return Err(ValidationError::invalid(
            "attendees",
            format!("at most {MEETING_ATTENDEES_MAX} attendees"),
        ));
    }
    for (i, a) in attendees.iter().enumerate() {
        let field = format!("attendees.{i}");
        check_known(&field, a.kind)?;
        match a.kind {
            MeetingAttendeeKind::Member => {
                if a.member_id.is_none() || a.email.is_some() {
                    return Err(ValidationError::invalid(
                        field,
                        "a member attendee has member_id and no email",
                    ));
                }
            }
            MeetingAttendeeKind::Email => {
                let Some(e) = &a.email else {
                    return Err(ValidationError::invalid(
                        field,
                        "an email attendee has email and no member_id",
                    ));
                };
                if a.member_id.is_some() {
                    return Err(ValidationError::invalid(
                        field,
                        "an email attendee has email and no member_id",
                    ));
                }
                check_email(&format!("{field}.email"), e)?;
            }
            MeetingAttendeeKind::Unknown => {}
        }
    }
    Ok(())
}

fn check_tz(tz: &str) -> Result<(), ValidationError> {
    if tz.is_empty() || tz.len() > MEETING_TZ_MAX_BYTES {
        return Err(ValidationError::invalid(
            "tz",
            format!("tz must be an IANA zone name of 1–{MEETING_TZ_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/meetings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMeetingRequest {
    /// The client's retry nonce (MC2a, F10): 16–64 characters of `[A-Za-z0-9_-]`. A create
    /// that repeats an earlier one's nonce (same organiser, within 24 hours) answers that
    /// meeting and its current link instead of making another. Stored only as SHA-256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
    /// `instant` (starts now, 60 minutes) or `scheduled`.
    pub kind: MeetingKind,
    /// The title, 1–200 characters (sealed at rest).
    pub title: String,
    /// When it starts (`scheduled` only; at most a minute in the past).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<Timestamp>,
    /// When it is planned to end (`scheduled` only; 1 minute to
    /// `meetings.max_duration_minutes` after `starts_at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<Timestamp>,
    /// The IANA zone the times were chosen in (the calendar file's TZID).
    pub tz: String,
    /// Members (added to the meeting channel) and emails (for the calendar file), at most 100.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<MeetingAttendeeInput>,
    /// Record from the start (stored; MC4 maps it to the channel's `recording_auto`). Default
    /// off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_record: Option<bool>,
    /// Link guests may share their screen (MC2). Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_screen_share: Option<bool>,
    /// Link guests see the title and the organiser's name before they join (MC2). Default on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title_to_guests: Option<bool>,
}

impl Validate for CreateMeetingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("kind", self.kind)?;
        check_client_nonce(self.client_nonce.as_deref())?;
        check_chars("title", self.title.trim(), 1, MEETING_TITLE_MAX_CHARS)?;
        check_tz(&self.tz)?;
        match self.kind {
            MeetingKind::Instant => {
                if self.starts_at.is_some() || self.ends_at.is_some() {
                    return Err(ValidationError::invalid(
                        "starts_at",
                        "an instant meeting takes no starts_at or ends_at",
                    ));
                }
            }
            _ => {
                if self.starts_at.is_none() {
                    return Err(ValidationError::invalid(
                        "starts_at",
                        "a scheduled meeting needs starts_at",
                    ));
                }
                if self.ends_at.is_none() {
                    return Err(ValidationError::invalid(
                        "ends_at",
                        "a scheduled meeting needs ends_at",
                    ));
                }
            }
        }
        check_attendees(&self.attendees)
    }
}

/// Body of `POST /api/v1/meetings/{id}/update` (hosts). Every field is optional; `attendees`
/// replaces the whole list (members not listed leave the meeting channel, the organiser
/// never does).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMeetingRequest {
    /// A new title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A new start (scheduled meetings only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<Timestamp>,
    /// A new planned end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<Timestamp>,
    /// A new zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// The new attendee list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<MeetingAttendeeInput>>,
    /// Record from the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_record: Option<bool>,
    /// Link guests may share their screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_screen_share: Option<bool>,
    /// Link guests see the title before they join.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title_to_guests: Option<bool>,
}

impl Validate for UpdateMeetingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(t) = &self.title {
            check_chars("title", t.trim(), 1, MEETING_TITLE_MAX_CHARS)?;
        }
        if let Some(tz) = &self.tz {
            check_tz(tz)?;
        }
        if let Some(a) = &self.attendees {
            check_attendees(a)?;
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/meetings/{id}/attendees/remove` (the organiser; erasure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveMeetingAttendeeRequest {
    /// The attendee row.
    pub attendee_id: Id,
}

impl Validate for RemoveMeetingAttendeeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/meetings/{id}/lock` (hosts, MC3): lock or unlock the meeting. Locked,
/// new redeems and knocks answer `423 meeting_locked`; people already admitted or invited may
/// still (re)join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LockMeetingRequest {
    /// `true` to lock, `false` to unlock.
    pub locked: bool,
}

impl Validate for LockMeetingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/meetings/{id}/remove` (hosts, MC3): take someone out of the meeting
/// (out of its call and its channel; a link guest's sessions end and it is never reactivated).
/// The organiser cannot be removed. Answers `{ok: true}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveMeetingMemberRequest {
    /// The member to remove (a member of the meeting channel).
    pub member_id: Id,
}

impl Validate for RemoveMeetingMemberRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/meetings`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeetingListQuery {
    /// `upcoming` (default) or `past`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MeetingListScope>,
    /// `next_cursor` of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 1–100, default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for MeetingListQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.scope {
            check_known("scope", s)?;
        }
        if let Some(l) = self.limit
            && (l == 0 || l > PAGE_LIMIT_MAX)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        if let Some(c) = &self.cursor
            && (c.is_empty() || c.len() > 64)
        {
            return Err(ValidationError::invalid("cursor", "not a cursor"));
        }
        Ok(())
    }
}

/// A meeting, as its channel members see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Meeting {
    /// The meeting id (not the channel id; never in a guest URL).
    pub id: Id,
    /// Its private voice channel.
    pub channel_id: Id,
    /// The Meetings Space.
    pub space_id: Id,
    /// Who made it (always a host).
    pub organiser_member_id: Id,
    /// The title; absent when its key was shredded (retention, crypto-shred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `instant` or `scheduled`.
    pub kind: MeetingKind,
    /// The state.
    pub state: MeetingState,
    /// The start.
    pub starts_at: Timestamp,
    /// The planned end.
    pub ends_at: Timestamp,
    /// The IANA zone.
    pub tz: String,
    /// Members may join voice from here (`starts_at` − 15 min).
    pub opens_at: Timestamp,
    /// Nothing lives past it (`starts_at` + 12 h).
    pub hard_end_at: Timestamp,
    /// Locked (MC3; always `false` in MC1).
    pub locked: bool,
    /// Record from the start (MC4 maps it).
    pub auto_record: bool,
    /// Link guests may share their screen.
    pub guest_screen_share: bool,
    /// Link guests see the title before they join.
    pub show_title_to_guests: bool,
    /// The calendar UID's local part, `meet-<channel_id>`; the App appends `@<its origin
    /// host>`.
    pub ics_uid: String,
    /// The calendar SEQUENCE: rises on time, title and cancel changes.
    pub ics_sequence: u32,
    /// The caller holds the host overwrite (MANAGE_CHANNELS on the channel).
    pub is_host: bool,
    /// When it ended or was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Who ended it (absent for the jobs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<Id>,
    /// Why it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<MeetingEndReason>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

/// Answer of `POST /api/v1/meetings`. F10 `KEY_IGNORED` (MC2a): it holds a fresh link secret,
/// so no idempotency row ever stores it; a retry with the same `client_nonce` answers the same
/// meeting with its current link instead.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingCreated {
    /// The meeting.
    pub meeting: Meeting,
    /// Its voice channel (also `meeting.channel_id`).
    pub channel_id: Id,
    /// The guest link as a URL path, `/j/<link_id>#<secret>` (MC2a): the Meet App puts its
    /// own origin in front. Absent when `meetings.link_guests` is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
    /// The guest link (MC2a; absent when `meetings.link_guests` is off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<MeetingLink>,
}

impl std::fmt::Debug for MeetingCreated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingCreated")
            .field("meeting", &self.meeting)
            .field("channel_id", &self.channel_id)
            .field("link_url", &self.link_url.as_ref().map(|_| "<redacted>"))
            .field("link", &self.link)
            .finish()
    }
}

/// A page of meetings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingPage {
    /// The meetings.
    pub items: Vec<Meeting>,
    /// Pass as `cursor` for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// More pages follow.
    pub has_more: bool,
}

/// One attendee (organiser view).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingAttendee {
    /// The row id (for `attendees/remove`).
    pub id: Id,
    /// `member` or `email`.
    pub kind: MeetingAttendeeKind,
    /// The member (`member`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<Id>,
    /// The address (`email`; absent once its key is shredded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Answer of `GET /api/v1/meetings/{id}/attendees`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingAttendees {
    /// Every attendee, members first, then emails, in the order given.
    pub items: Vec<MeetingAttendee>,
    /// Emails were purged (30 days after the end, or at cancel).
    pub emails_purged: bool,
}

/// Event `meeting_updated` (to the meeting channel's members): the meeting after a create,
/// update, cancel, start or end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingUpdated {
    /// The meeting (`is_host` as the receiver sees it).
    pub meeting: Meeting,
}

// ---------------------------------------------------------------- MC2a: links and link guests

/// A meeting link's public id: 10 random bytes as RFC 4648 base32 (`A–Z2–7`), 16 characters.
pub const MEETING_LINK_ID_CHARS: usize = 16;
/// A meeting link's secret: 16 random bytes, base64url without padding (22 characters).
pub const MEETING_LINK_SECRET_BYTES: usize = 16;
/// The fewest bytes a redeem's `join_nonce` may carry (128 bits).
pub const JOIN_NONCE_MIN_BYTES: usize = 16;
/// The most bytes a `join_nonce` may carry.
pub const JOIN_NONCE_MAX_BYTES: usize = 64;
/// Longest link-guest display name, in characters (after trimming and NFC).
pub const LINK_GUEST_NAME_MAX_CHARS: usize = 64;
/// A link-guest session lasts this long past its last renewal (sliding; capped at the link's
/// `valid_until`).
pub const LINK_GUEST_SESSION_MINUTES: i64 = 60;
/// `max_guests_inside` of a new link.
pub const MEETING_LINK_MAX_GUESTS_DEFAULT: u32 = 50;
/// `max_waiting` of a new link.
pub const MEETING_LINK_MAX_WAITING_DEFAULT: u32 = 20;
/// A `client_nonce` repeats an earlier create or rotate for this long.
pub const MEETING_CLIENT_NONCE_HOURS: i64 = 24;
/// Shortest `client_nonce`.
pub const CLIENT_NONCE_MIN_CHARS: usize = 16;
/// Longest `client_nonce`.
pub const CLIENT_NONCE_MAX_CHARS: usize = 64;

/// `details.reason` values MC2a adds.
pub mod link_reasons {
    /// `403 policy_denied`: `meetings.link_guests` is off.
    pub const LINK_GUESTS_DISABLED: &str = "link_guests_disabled";
    /// `400 validation_failed` `display_name`: the name is the same as a member's in the
    /// meeting (case-folded).
    pub const NAME_TAKEN: &str = "name_taken";
    /// `409 conflict`: the meeting has no active link (revoked, or link guests off).
    pub const NO_ACTIVE_LINK: &str = "no_active_link";
}

fn check_client_nonce(n: Option<&str>) -> Result<(), ValidationError> {
    let Some(n) = n else {
        return Ok(());
    };
    let ok = (CLIENT_NONCE_MIN_CHARS..=CLIENT_NONCE_MAX_CHARS).contains(&n.len())
        && n.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
    if !ok {
        return Err(ValidationError::invalid(
            "client_nonce",
            format!(
                "client_nonce must be {CLIENT_NONCE_MIN_CHARS}–{CLIENT_NONCE_MAX_CHARS} characters of [A-Za-z0-9_-]"
            ),
        ));
    }
    Ok(())
}

/// A meeting's active guest link, as hosts see it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLink {
    /// The public lookup id (16 base32 characters; safe in logs).
    pub link_id: String,
    /// The secret (base64url, 22 characters). Hosts only; never logged or stored in plain text.
    pub secret: String,
    /// The guest URL's path, `/j/<link_id>#<secret>` (the secret rides in the fragment, which
    /// no server sees in a GET).
    pub path: String,
    /// Redeem works until here: `hard_end_at` while the meeting is open.
    pub valid_until: Timestamp,
    /// Most link guests admitted at once.
    pub max_guests_inside: u32,
    /// Most link guests waiting at once.
    pub max_waiting: u32,
    /// When this link was made.
    pub created_at: Timestamp,
}

impl std::fmt::Debug for MeetingLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingLink")
            .field("link_id", &self.link_id)
            .field("secret", &"<redacted>")
            .field("valid_until", &self.valid_until)
            .field("max_guests_inside", &self.max_guests_inside)
            .field("max_waiting", &self.max_waiting)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Answer of `GET /api/v1/meetings/{id}/link`, `…/link/rotate` and `…/link/revoke` (hosts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLinkResponse {
    /// The active link; absent once revoked, after the meeting closed, or while
    /// `meetings.link_guests` is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<MeetingLink>,
}

/// Body of `POST /api/v1/meetings/{id}/link/rotate` (hosts). F10 `KEY_IGNORED`: the answer holds
/// a fresh secret; a retry with the same `client_nonce` (within 24 hours) answers the link that
/// rotate made instead of rotating again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RotateMeetingLinkRequest {
    /// The client's retry nonce: 16–64 characters of `[A-Za-z0-9_-]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
}

impl Validate for RotateMeetingLinkRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_client_nonce(self.client_nonce.as_deref())
    }
}

/// Body of `POST /api/v1/meetings/peek` and `POST /api/v1/meetings/resolve`: a link's id and
/// secret (from the URL fragment). A malformed id or secret is answered like a wrong one
/// (`404 link_invalid`), never `400`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeetingLinkRef {
    /// The link's id.
    pub link_id: String,
    /// The link's secret.
    pub secret: String,
}

impl std::fmt::Debug for MeetingLinkRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingLinkRef")
            .field("link_id", &self.link_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl Validate for MeetingLinkRef {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// The recording part of a peek (MC4 adds `consent_prompt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingPeekRecording {
    /// The meeting records from the start.
    pub auto_record: bool,
    /// A recording is running in the meeting now.
    pub active: bool,
}

/// Answer of `POST /api/v1/meetings/peek` (public): what the join page may show before the
/// guest types a name. Writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingPeek {
    /// The title (only with `show_title_to_guests`; absent once its key is shredded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The organiser's display name (only with `show_title_to_guests`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organiser_name: Option<String>,
    /// The start.
    pub starts_at: Timestamp,
    /// The planned end.
    pub ends_at: Timestamp,
    /// The IANA zone.
    pub tz: String,
    /// `scheduled` or `live` (a closed meeting's link is `404 link_invalid`).
    pub state: MeetingState,
    /// Locked (MC3).
    pub locked: bool,
    /// Redeem opens here (`starts_at` − 15 min).
    pub opens_at: Timestamp,
    /// Recording facts for the notice.
    pub recording: MeetingPeekRecording,
    /// Browsers refused for encrypted calls (the install's `voice.e2ee.refused_browsers`).
    pub refused_browsers: Vec<String>,
    /// `meetings.privacy_contact`, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_contact: Option<String>,
}

/// Body of `POST /api/v1/meetings/redeem` (public; the Meet App's BFF calls it).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeetingRedeemRequest {
    /// The link's id.
    pub link_id: String,
    /// The link's secret.
    pub secret: String,
    /// The guest's name: 1–64 characters after trimming, NFC and removing control and
    /// bidi-override characters.
    pub display_name: String,
    /// The browser tab's attempt id: 16–64 random bytes, base64url without padding. Stored
    /// only as SHA-256. Not a pass: it never works without the link's secret.
    pub join_nonce: String,
}

impl std::fmt::Debug for MeetingRedeemRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingRedeemRequest")
            .field("link_id", &self.link_id)
            .field("secret", &"<redacted>")
            .field("display_name", &"<redacted>")
            .field("join_nonce", &"<redacted>")
            .finish()
    }
}

/// How many bytes a `join_nonce` (base64url without padding) carries, or `None` when it is not
/// 16–64 bytes of canonical base64url.
pub fn join_nonce_len(s: &str) -> Option<usize> {
    let ok_chars = s
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if !ok_chars || s.len() % 4 == 1 || s.len() > JOIN_NONCE_MAX_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let n = s.len() / 4 * 3 + (s.len() % 4).saturating_sub(1);
    (JOIN_NONCE_MIN_BYTES..=JOIN_NONCE_MAX_BYTES)
        .contains(&n)
        .then_some(n)
}

/// Whether `c` is removed from a link guest's name: controls; bidi overrides, embeddings,
/// isolates and marks; and (MQ1 L2) every other format character (`General_Category = Cf`) and
/// default-ignorable code point (zero-width spaces and joiners, the word joiner, the BOM, the
/// soft hyphen, variation selectors, Hangul and Mongolian fillers, tags). These render as
/// nothing, so a name that differs from another only by them would show identically.
pub fn is_name_strip_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF0}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

impl Validate for MeetingRedeemRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if join_nonce_len(&self.join_nonce).is_none() {
            return Err(ValidationError::invalid(
                "join_nonce",
                format!(
                    "join_nonce must be {JOIN_NONCE_MIN_BYTES}–{JOIN_NONCE_MAX_BYTES} random bytes, base64url without padding"
                ),
            ));
        }
        let kept: String = self
            .display_name
            .chars()
            .filter(|c| !is_name_strip_char(*c))
            .collect();
        check_chars("display_name", kept.trim(), 1, LINK_GUEST_NAME_MAX_CHARS)
    }
}

unit_enum! {
    /// Where a redeemed link guest stands.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingRedeemState {
        /// In the lobby (`lobby_entry_id` names the waiting entry): no channel yet.
        Waiting = "waiting",
        /// Admitted before and still in the meeting channel (re-admitted without knocking).
        Admitted = "admitted",
    }
}

/// Answer of `POST /api/v1/meetings/redeem`. F10: the route is public, so no idempotency row
/// exists; a retry with the same `join_nonce` gets the same member and a new session (the
/// earlier one is revoked).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingRedeemed {
    /// The link-guest session (for the BFF only; never shown to the browser).
    pub session: super::auth::SessionResponse,
    /// The link guest.
    pub member_id: Id,
    /// The waiting lobby entry (MC2b); absent when `state` is `admitted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lobby_entry_id: Option<Id>,
    /// `waiting` or `admitted`.
    pub state: MeetingRedeemState,
    /// The session never lives past here (the link's `valid_until`).
    pub valid_until: Timestamp,
}

impl std::fmt::Debug for MeetingRedeemed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingRedeemed")
            .field("session", &self.session)
            .field("member_id", &self.member_id)
            .field("lobby_entry_id", &self.lobby_entry_id)
            .field("state", &self.state)
            .field("valid_until", &self.valid_until)
            .finish()
    }
}

/// Answer of `POST /api/v1/meetings/resolve` (signed-in callers): which meeting a link names,
/// so the Meet App sends members straight in. A bad link is `404 link_invalid`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingResolved {
    /// The meeting.
    pub meeting_id: Id,
    /// The caller is in the meeting's channel already.
    pub member_of_channel: bool,
}

/// `meeting` on a meeting channel's JSON (Meet MC2a, CC6): the meeting it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelMeeting {
    /// The meeting id.
    pub id: Id,
}

// ---------------------------------------------------------------- MC2b: the lobby

/// A waiting lobby entry expires this many minutes after its knock.
pub const LOBBY_WAIT_MINUTES: i64 = 10;
/// Two denials of the same join nonce (or signed-in member) within this many minutes block it.
pub const KNOCK_DENY_WINDOW_MINUTES: i64 = 10;
/// A blocked join nonce (or signed-in member) may knock again this many minutes after the
/// second denial (`403 knock_blocked` until then).
pub const KNOCK_BLOCK_MINUTES: i64 = 60;
/// At most one `meeting_knock` push per meeting per this many minutes (bucket `meeting_knock`).
pub const KNOCK_PUSH_EVERY_MINUTES: i64 = 2;
/// The guest page polls `GET /api/v1/meetings/lobby/me` this often when it has no socket.
pub const LOBBY_POLL_SECONDS: u32 = 5;
/// `GET /api/v1/meetings/lobby/me` lists the caller's entries knocked within this many hours.
pub const LOBBY_ME_HOURS: i64 = 12;
/// Most entries in `GET /api/v1/meetings/lobby/me`.
pub const LOBBY_ME_MAX: usize = 20;

/// `details.reason` values MC2b adds.
pub mod lobby_reasons {
    /// `422 invalid_state`: the lobby entry is no longer waiting (admitted, denied or expired).
    pub const NOT_WAITING: &str = "lobby_entry_not_waiting";
    /// `422 invalid_state`: admitting one more link guest would pass the link's
    /// `max_guests_inside`.
    pub const GUESTS_INSIDE_FULL: &str = "guests_inside_full";
}

unit_enum! {
    /// A lobby entry's state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingLobbyState {
        /// Knocked; a host has not decided. Expires after 10 minutes.
        Waiting = "waiting",
        /// A host let them in: they are a member of the meeting channel.
        Admitted = "admitted",
        /// A host did not let them in (a link guest's session ended with it).
        Denied = "denied",
        /// Nobody decided within 10 minutes, the link was rotated or revoked, or the meeting
        /// closed.
        Expired = "expired",
        /// The knocker withdrew before a decision (MC3b: `POST
        /// /api/v1/meetings/{id}/lobby/me/withdraw`).
        Left = "left",
    }
}

unit_enum! {
    /// Who knocked.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingLobbyKind {
        /// A link guest (redeemed the link without an account).
        LinkGuest = "link_guest",
        /// A signed-in member of the install who knocked with the link.
        Member = "member",
    }
}

/// One lobby entry as the meeting's hosts see it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobbyEntry {
    /// The entry.
    pub entry_id: Id,
    /// The meeting.
    pub meeting_id: Id,
    /// Who knocked.
    pub member_id: Id,
    /// The name they gave (a link guest's is attacker-chosen text: show it as plain text).
    pub display_name: String,
    /// A link guest or a signed-in member.
    pub kind: MeetingLobbyKind,
    /// Where the entry stands.
    pub state: MeetingLobbyState,
    /// The knock.
    pub knocked_at: Timestamp,
    /// The decision (admit, deny, expiry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<Timestamp>,
    /// Whole minutes waited (to the decision, or to now while waiting).
    pub waited_minutes: u32,
}

impl std::fmt::Debug for MeetingLobbyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingLobbyEntry")
            .field("entry_id", &self.entry_id)
            .field("meeting_id", &self.meeting_id)
            .field("member_id", &self.member_id)
            .field("display_name", &"<redacted>")
            .field("kind", &self.kind)
            .field("state", &self.state)
            .field("knocked_at", &self.knocked_at)
            .field("decided_at", &self.decided_at)
            .finish()
    }
}

/// Answer of `GET /api/v1/meetings/{id}/lobby` (hosts): the waiting entries, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobby {
    /// Waiting entries, oldest knock first.
    pub items: Vec<MeetingLobbyEntry>,
}

/// Answer of `POST /api/v1/meetings/{id}/lobby/admit_all` (hosts): the entries admitted, oldest
/// knock first. Entries past the link's `max_guests_inside` stay waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobbyDecided {
    /// The entries this call admitted.
    pub items: Vec<MeetingLobbyEntry>,
    /// Entries still waiting after it.
    pub still_waiting: u32,
}

/// Answer of `POST /api/v1/meetings/knock` (a signed-in member with the link). F10
/// `KEY_IGNORED`: a retry finds the caller's open entry (one per meeting and member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingKnocked {
    /// The meeting.
    pub meeting_id: Id,
    /// `admitted` (the caller is in the meeting channel already: go straight in) or `waiting`.
    pub state: MeetingRedeemState,
    /// The waiting entry; absent when `admitted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lobby_entry_id: Option<Id>,
}

/// One of the caller's own lobby entries (`GET /api/v1/meetings/lobby/me`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobbyStatus {
    /// The entry.
    pub entry_id: Id,
    /// The meeting.
    pub meeting_id: Id,
    /// Where it stands.
    pub state: MeetingLobbyState,
    /// The knock.
    pub knocked_at: Timestamp,
    /// The decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<Timestamp>,
    /// The meeting channel, once admitted (join its voice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
}

/// Answer of `GET /api/v1/meetings/lobby/me`: the caller's entries knocked in the last 12
/// hours, newest first (at most 20). The guest page's polling fallback (every 5 s) when it has
/// no gateway socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobbyMe {
    /// Newest knock first.
    pub items: Vec<MeetingLobbyStatus>,
}

/// Event `meeting_lobby_updated` (to the meeting's hosts only, never to guests): an entry was
/// made or decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingLobbyUpdated {
    /// The meeting.
    pub meeting_id: Id,
    /// The entry after the change.
    pub entry: MeetingLobbyEntry,
    /// Entries waiting now.
    pub waiting: u32,
}

unit_enum! {
    /// What a host decided about a knock (or that nobody did).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MeetingAdmissionOutcome {
        /// Let in: join the meeting channel's voice.
        Admitted = "admitted",
        /// Not let in.
        Denied = "denied",
        /// Nobody decided in time, or the link changed: knock again.
        Expired = "expired",
    }
}

/// Event `meeting_admission` (to the knocking member's sessions): the decision on its entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingAdmission {
    /// The meeting.
    pub meeting_id: Id,
    /// The entry.
    pub entry_id: Id,
    /// The decision.
    pub outcome: MeetingAdmissionOutcome,
    /// The meeting channel, when admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
}

/// Event `meeting_updated` as a link guest gets it (§6.5.4): only what a guest may see. Members
/// get [`MeetingUpdated`] (its `meeting` key tells the two apart).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingGuestUpdated {
    /// The meeting.
    pub meeting_id: Id,
    /// Its state.
    pub state: MeetingState,
    /// Locked (MC3).
    pub locked: bool,
}

// ---------------------------------------------------------------- Meet MC3b: names, withdraw

/// One person in a meeting channel, as everyone in that channel sees them (Meet MC3b): the
/// display name and whether they are a link guest, nothing else (no handle, email, role,
/// status, profile field or Space membership). Answered by `GET /api/v1/channels/{id}/people`
/// and carried as `person` by a meeting channel's `channel_member_added`. A link guest's name is
/// attacker-chosen text: show it as plain text, after a fixed "Guest" badge.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetingPerson {
    /// The member.
    pub member_id: Id,
    /// Their display name.
    pub display_name: String,
    /// A link guest or a member of the install (the lobby's `kind`).
    pub kind: MeetingLobbyKind,
}

impl std::fmt::Debug for MeetingPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingPerson")
            .field("member_id", &self.member_id)
            .field("display_name", &"<redacted>")
            .field("kind", &self.kind)
            .finish()
    }
}

/// Answer of `GET /api/v1/channels/{id}/people` (Meet MC3b; a meeting channel's members,
/// ascending by member id, `after`/`limit` paging).
pub type MeetingPersonPage = crate::http::Page<MeetingPerson>;
