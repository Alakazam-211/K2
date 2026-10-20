//! The one list of wire types (S1 §8). Tests, schema emission and `docs/contracts.md` read it.
//!
//! Top-level types have their own fixtures (`tests/fixtures/<module>/<Type>.<case>.json`) and
//! schema (`schema/<Type>.json`). Nested types ([`NESTED`]) are covered through their parents'
//! `full` fixtures.

use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::Validate;

/// Implemented by every wire unit enum (via the internal `unit_enum!` macro).
pub trait UnitEnum: Copy + Sized + 'static {
    /// Every known value, without `Unknown`.
    fn all() -> &'static [Self];
    /// The wire string.
    fn wire_str(self) -> &'static str;
    /// Whether this is the forward-compat `Unknown` value.
    fn is_unknown_value(self) -> bool;
}

/// Example values of a top-level wire type, by case name (`full`, `min`, …).
pub trait Example: Sized {
    /// The cases. `full` sets every optional field; `min` sets only required ones.
    fn examples() -> Vec<(&'static str, Self)>;
}

/// What a wire type is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireRole {
    /// An HTTP request body or query, a gateway client payload, or a mgmt request.
    /// Denies unknown fields.
    Request,
    /// An HTTP response body or a gateway server payload.
    Response,
    /// A dispatch event body.
    Event,
    /// A record that appears in responses and events.
    Record,
    /// A unit enum. Its fixture is the array of every value.
    Enum,
}

impl WireRole {
    /// Lowercase name for docs.
    pub const fn as_str(self) -> &'static str {
        match self {
            WireRole::Request => "request",
            WireRole::Response => "response",
            WireRole::Event => "event",
            WireRole::Record => "record",
            WireRole::Enum => "enum",
        }
    }
}

/// One registered top-level wire type.
#[derive(Debug, Clone, Copy)]
pub struct WireType {
    /// The Rust type name; the fixture and schema file stem.
    pub name: &'static str,
    /// Fixture subdirectory, for example `api/messages`.
    pub module: &'static str,
    /// What it is for.
    pub role: WireRole,
    /// The route, op or event it belongs to (may be empty).
    pub note: &'static str,
    /// Its JSON Schema.
    pub schema: fn() -> Schema,
    /// Its serialized examples by case.
    pub examples: fn() -> Vec<(&'static str, Value)>,
    /// Parse as the type and serialize back.
    pub roundtrip: fn(&Value) -> Result<Value, String>,
    /// Parse as the type and, for requests, run `validate()`.
    pub check: fn(&Value) -> Result<(), String>,
}

fn schema_of<T: JsonSchema>() -> Schema {
    schemars::schema_for!(T)
}

fn to_value_checked<T: Serialize>(name: &str, v: &T) -> Value {
    match serde_json::to_value(v) {
        Ok(v) => v,
        Err(e) => panic!("sew-proto registry: example of {name} does not serialize: {e}"),
    }
}

fn examples_of<T: Example + Serialize>() -> Vec<(&'static str, Value)> {
    T::examples()
        .into_iter()
        .map(|(case, v)| (case, to_value_checked(std::any::type_name::<T>(), &v)))
        .collect()
}

fn enum_examples<T: UnitEnum + Serialize>() -> Vec<(&'static str, Value)> {
    vec![(
        "all",
        to_value_checked(std::any::type_name::<T>(), &T::all()),
    )]
}

fn roundtrip_of<T: Serialize + DeserializeOwned>(v: &Value) -> Result<Value, String> {
    let parsed: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    serde_json::to_value(&parsed).map_err(|e| e.to_string())
}

fn roundtrip_vec<T: Serialize + DeserializeOwned>(v: &Value) -> Result<Value, String> {
    roundtrip_of::<Vec<T>>(v)
}

fn check_parse<T: DeserializeOwned>(v: &Value) -> Result<(), String> {
    serde_json::from_value::<T>(v.clone())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn check_vec<T: DeserializeOwned>(v: &Value) -> Result<(), String> {
    check_parse::<Vec<T>>(v)
}

fn check_request<T: DeserializeOwned + Validate>(v: &Value) -> Result<(), String> {
    let parsed: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    parsed.validate().map_err(|e| e.to_string())
}

macro_rules! wire {
    (Enum, $module:literal, $ty:ident, $note:literal) => {
        WireType {
            name: stringify!($ty),
            module: $module,
            role: WireRole::Enum,
            note: $note,
            schema: schema_of::<$ty>,
            examples: enum_examples::<$ty>,
            roundtrip: roundtrip_vec::<$ty>,
            check: check_vec::<$ty>,
        }
    };
    (Request, $module:literal, $ty:ident, $note:literal) => {
        WireType {
            name: stringify!($ty),
            module: $module,
            role: WireRole::Request,
            note: $note,
            schema: schema_of::<$ty>,
            examples: examples_of::<$ty>,
            roundtrip: roundtrip_of::<$ty>,
            check: check_request::<$ty>,
        }
    };
    ($role:ident, $module:literal, $ty:ident, $note:literal) => {
        WireType {
            name: stringify!($ty),
            module: $module,
            role: WireRole::$role,
            note: $note,
            schema: schema_of::<$ty>,
            examples: examples_of::<$ty>,
            roundtrip: roundtrip_of::<$ty>,
            check: check_parse::<$ty>,
        }
    };
}

/// Struct and enum names that are only ever nested (no fixture or schema of their own).
pub const NESTED: &[&str] = &[
    "ApiError",
    "Actor",
    "ActorVia",
    "Attachment",
    "AuditTarget",
    "ChannelChange",
    "ChannelPerms",
    "ClientInfo",
    "Delivery",
    "DeliveryCall",
    "DeliveryMessage",
    "Emoji",
    "EmojiInput",
    "ForwardedMessage",
    "GifItem",
    "GifMedia",
    "MemberChange",
    "MemberListSub",
    "MentionTargets",
    "MessageReference",
    "MgmtPrincipal",
    "OwnerApproval",
    "Overwrite",
    "PolicyDigest",
    "PreviewImage",
    "ProfileInput",
    "ProfilePatch",
    "ReactionSummary",
    "ReadyCategory",
    "ReadyChannel",
    "ReadyInstall",
    "ReadySpace",
    "ReadyVersions",
    "ReferenceInput",
    "ReorderCategoryItem",
    "ReorderChannelItem",
    "Highlights",
    "SearchChannelRef",
    "SearchFileHit",
    "SearchFileRef",
    "SearchMessageHit",
    "SessionLimits",
    "SetupLink",
    "StatusBackup",
    "StatusDb",
    "StatusDrill",
    "StatusFiles",
    "StatusIdentity",
    "StatusJobs",
    "StatusKeys",
    "StatusKeysMirror",
    "StatusSearch",
    // S14: bulk ack items (covered by BulkAckRequest's full fixture).
    "AckItem",
    "ThreadAckItem",
    "StatusSearchDocs",
    "StatusSearchRebuild",
    // S18
    "CustomFieldInput",
    "DirectoryEntry",
    "DndRule",
    "DndSchedule",
    "MemberListItem",
    "MemberListOp",
    "MemberStatus",
    "Mentionable",
    "MgmtCustomFieldInput",
    // SW15: one bundle of the backups list.
    "MgmtBackup",
    // SW13: the members of a mgmt Space add.
    "MgmtSpaceMemberRef",
    // Meet MC1: attendees as given and as the organiser reads them.
    "MeetingAttendeeInput",
    "MeetingAttendee",
    "MgmtSpaceNotAdded",
    "PresenceEntry",
    "ProfileFieldValue",
    // S22
    "StatusRetention",
    "VanishedAttachment",
    // S17
    "DraftUpload",
    "PollOption",
    "Poll",
    // S19
    "GuestChannelRef",
    // S21
    "DigestAvailability",
    "MuteInput",
    "StatusMail",
    "StatusDigest",
    // S23
    "ExportProgress",
    "ExportError",
    "ComplianceOfficer",
    "StatusExports",
    "LastVerify",
    "Producer",
    "ArchiveOptions",
    "ArchiveScope",
    "TrailerEntry",
    "SourceRef",
    "StatusRecord",
    "AvatarRecord",
    "IconRecord",
    "OverwriteRecord",
    "ReferenceRecord",
    "MentionsRecord",
    "DeletedRecord",
    "PinRecord",
    "ReactionRecord",
    "RemovedRecord",
    "AttachmentRecord",
    "PollOptionRecord",
    "PollRecord",
    "NotifySettingsRecord",
    "NotifyChannelPrefRecord",
    "NotifySpacePrefRecord",
    // S35 v2 (in RecordingRecord's full fixture)
    "RecordingParticipantRecord",
    "RecordingRunRecord",
    "RecordingTrackRecord",
    "RecordingGapRecord",
    "RecordingSegmentRecord",
    "RecordingMixRecord",
    "TranscriptTurnRecord",
    // S35 v2
    "RecordingParticipant",
    "PromptMember",
    "PromptRoom",
    "PromptTranscript",
    "SummaryAudio",
    "SummaryTranscript",
    "TurnPiece",
    "MixedPart",
    "AudioTrack",
    "DownloadSpeaker",
    // S34 v2
    "CallRing",
    "MyRing",
    "LayerHint",
    "TrackHint",
    "Vp9Hint",
];

/// Generic wrappers, registered through their concrete aliases (for example `MessagePage`).
pub const GENERIC: &[&str] = &["Page"];

/// Every top-level wire type.
pub fn all() -> Vec<WireType> {
    use crate::api::audit::*;
    use crate::api::auth::*;
    use crate::api::changes::*;
    use crate::api::channels::*;
    use crate::api::compliance::*;
    use crate::api::exports::*;
    use crate::api::extras::*;
    use crate::api::files::*;
    use crate::api::guests::*;
    use crate::api::media::*;
    use crate::api::meetings::*;
    use crate::api::members::*;
    use crate::api::messages::*;
    use crate::api::moderation::*;
    use crate::api::notify::*;
    use crate::api::overwrites::*;
    use crate::api::people::*;
    use crate::api::read_state::*;
    use crate::api::resolve::*;
    use crate::api::roles::*;
    use crate::api::search::*;
    use crate::api::spaces::*;
    use crate::audit::*;
    use crate::calls::*;
    use crate::changes::*;
    use crate::e2ee::*;
    use crate::error::*;
    use crate::export::*;
    use crate::gateway::client::*;
    use crate::gateway::events::*;
    use crate::gateway::server::*;
    use crate::gateway::*;
    use crate::http::*;
    use crate::install::*;
    use crate::mgmt::local::*;
    use crate::mgmt::*;
    use crate::policy::*;
    use crate::recording::*;
    use crate::search::query::{FilterName, HasValue, IsValue};
    use crate::voice::*;
    use crate::when::*;

    vec![
        // api/auth
        wire!(Request, "api/auth", K2PassRequest, "POST /api/v1/auth/k2"),
        wire!(Request, "api/auth", RenewRequest, "POST /api/v1/auth/renew"),
        wire!(Request, "api/auth", LoginRequest, "POST /api/v1/auth/login"),
        wire!(
            Request,
            "api/auth",
            PasswordSetRequest,
            "POST /api/v1/auth/password/set"
        ),
        wire!(
            Response,
            "api/auth",
            SessionResponse,
            "POST /api/v1/auth/k2, /renew, /login"
        ),
        wire!(
            Response,
            "api/auth",
            GatewayTicketResponse,
            "POST /api/v1/gateway/ticket"
        ),
        // api/members
        wire!(Enum, "api/members", MemberKind, ""),
        wire!(Enum, "api/members", AgentKind, ""),
        wire!(Enum, "api/members", InstallRole, ""),
        wire!(Record, "api/members", Member, "event member_updated"),
        wire!(Response, "api/members", MemberPage, "member lists"),
        // api/spaces
        wire!(Record, "api/spaces", Space, "event space_updated"),
        wire!(
            Request,
            "api/spaces",
            CreateSpaceRequest,
            "create space (S10)"
        ),
        wire!(
            Request,
            "api/spaces",
            UpdateSpaceRequest,
            "update space (S10)"
        ),
        wire!(Record, "api/spaces", Category, "categories (S10)"),
        wire!(
            Request,
            "api/spaces",
            CreateCategoryRequest,
            "create category (S10)"
        ),
        wire!(
            Request,
            "api/spaces",
            UpdateCategoryRequest,
            "update category (S10)"
        ),
        wire!(Record, "api/spaces", Role, "event role_updated"),
        wire!(Enum, "api/spaces", OverwriteTarget, ""),
        // api/channels
        wire!(Enum, "api/channels", ChannelType, ""),
        wire!(Enum, "api/channels", Visibility, ""),
        wire!(
            Record,
            "api/channels",
            Channel,
            "events channel_created, channel_updated"
        ),
        wire!(
            Request,
            "api/channels",
            CreateChannelRequest,
            "create channel (S10)"
        ),
        wire!(
            Request,
            "api/channels",
            UpdateChannelRequest,
            "update channel (S10)"
        ),
        wire!(
            Record,
            "api/channels",
            ChannelMember,
            "channel member lists"
        ),
        wire!(
            Response,
            "api/channels",
            ChannelMemberPage,
            "channel member lists"
        ),
        wire!(
            Request,
            "api/channels",
            AddChannelMembersRequest,
            "add channel members (S10)"
        ),
        wire!(
            Request,
            "api/channels",
            RemoveChannelMemberRequest,
            "remove channel member (S10)"
        ),
        wire!(Request, "api/channels", OpenDmRequest, "open DM (S10)"),
        // api/resolve
        wire!(Request, "api/resolve", ResolveQuery, "GET /api/v1/resolve"),
        wire!(
            Response,
            "api/resolve",
            ResolveResponse,
            "GET /api/v1/resolve"
        ),
        // api/messages
        wire!(Enum, "api/messages", ReferenceKind, ""),
        wire!(
            Record,
            "api/messages",
            Message,
            "events message_created, message_updated"
        ),
        wire!(
            Response,
            "api/messages",
            MessagePage,
            "GET /api/v1/channels/{id}/messages"
        ),
        wire!(
            Record,
            "api/messages",
            MessageVersion,
            "GET /api/v1/messages/{id}/history"
        ),
        wire!(
            Response,
            "api/messages",
            MessageVersionPage,
            "GET /api/v1/messages/{id}/history"
        ),
        wire!(
            Record,
            "api/messages",
            ThreadSummary,
            "event thread_updated"
        ),
        wire!(
            Request,
            "api/messages",
            CreateMessageRequest,
            "POST /api/v1/channels/{id}/messages"
        ),
        wire!(
            Request,
            "api/messages",
            EditMessageRequest,
            "POST /api/v1/messages/{id}/edit"
        ),
        wire!(
            Request,
            "api/messages",
            ForwardMessageRequest,
            "forward (S13)"
        ),
        wire!(
            Request,
            "api/messages",
            ReactionRequest,
            "add reaction (S13)"
        ),
        // api/read_state
        wire!(
            Record,
            "api/read_state",
            ReadState,
            "event read_state_updated"
        ),
        wire!(
            Record,
            "api/read_state",
            ThreadReadState,
            "thread read cursors (S14)"
        ),
        wire!(
            Request,
            "api/read_state",
            AckRequest,
            "POST /api/v1/channels/{id}/ack, POST /api/v1/threads/{id}/ack"
        ),
        // ---- S14: sync (prd-sew-14 §15)
        wire!(
            Response,
            "api/read_state",
            ThreadsInbox,
            "GET /api/v1/me/threads/summary, READY, S14 events"
        ),
        wire!(
            Request,
            "api/read_state",
            MarkUnreadRequest,
            "POST /api/v1/channels/{id}/mark_unread, POST /api/v1/threads/{id}/mark_unread"
        ),
        wire!(
            Request,
            "api/read_state",
            BulkAckRequest,
            "POST /api/v1/me/read_states/ack"
        ),
        wire!(
            Response,
            "api/read_state",
            BulkAckResponse,
            "POST /api/v1/me/read_states/ack, event read_states_bulk_updated"
        ),
        wire!(
            Response,
            "api/read_state",
            ThreadReadUpdated,
            "thread ack / mark unread, event thread_read_updated"
        ),
        wire!(
            Request,
            "api/read_state",
            ReadStatesQuery,
            "GET /api/v1/me/read_states"
        ),
        wire!(
            Request,
            "api/read_state",
            ThreadReadsQuery,
            "GET /api/v1/me/thread_reads"
        ),
        wire!(Enum, "api/read_state", ThreadsInboxFilter, ""),
        wire!(
            Request,
            "api/read_state",
            ThreadsInboxQuery,
            "GET /api/v1/me/threads"
        ),
        wire!(
            Record,
            "api/read_state",
            ThreadInboxItem,
            "GET /api/v1/me/threads"
        ),
        wire!(
            Response,
            "api/read_state",
            ThreadInboxPage,
            "GET /api/v1/me/threads"
        ),
        wire!(
            Response,
            "api/read_state",
            ReadStatesPage,
            "GET /api/v1/me/read_states"
        ),
        wire!(
            Response,
            "api/read_state",
            ThreadReadsPage,
            "GET /api/v1/me/thread_reads"
        ),
        wire!(Record, "api/read_state", SyncChannel, "GET /api/v1/me/sync"),
        wire!(Response, "api/read_state", MeSync, "GET /api/v1/me/sync"),
        wire!(
            Request,
            "api/messages",
            TimelineQuery,
            "GET /api/v1/channels/{id}/messages, GET /api/v1/messages/{id}/replies"
        ),
        wire!(
            Enum,
            "error",
            ResyncReason,
            "details.reason of resync_required"
        ),
        // api/changes
        wire!(
            Response,
            "api/changes",
            ChannelChangesPage,
            "GET /api/v1/channels/{id}/changes"
        ),
        wire!(
            Response,
            "api/changes",
            MemberChangesPage,
            "GET /api/v1/me/changes"
        ),
        // changes
        wire!(Enum, "changes", ChannelChangeKind, "channel_changes.kind"),
        wire!(Enum, "changes", MemberChangeKind, "member_changes.kind"),
        wire!(
            Enum,
            "changes",
            NotificationReason,
            "member_changes.entity_id for notification"
        ),
        // error
        wire!(Enum, "error", Code, ""),
        wire!(Response, "error", ErrorBody, "every error response"),
        wire!(
            Record,
            "error",
            ValidationDetails,
            "details of validation_failed"
        ),
        wire!(
            Record,
            "error",
            RetryAfterDetails,
            "details of rate_limited"
        ),
        wire!(
            Record,
            "error",
            ResyncRequiredDetails,
            "details of resync_required"
        ),
        wire!(
            Record,
            "error",
            ReasonDetails,
            "details of mgmt and act-as refusals (S9)"
        ),
        wire!(Enum, "error", PermissionReason, ""),
        wire!(Enum, "error", InvalidStateReason, ""),
        wire!(
            Record,
            "error",
            PermissionDeniedDetails,
            "details of missing_permission (S5)"
        ),
        wire!(
            Record,
            "error",
            PolicyDeniedDetails,
            "details of policy_denied"
        ),
        wire!(
            Record,
            "error",
            InvalidStateDetails,
            "details of invalid_state (S5)"
        ),
        // http
        wire!(Request, "http", PageQuery, "id-keyed list queries"),
        wire!(Request, "http", ChangesQuery, "change-log catch-up queries"),
        wire!(Request, "http", EmptyRequest, "bodiless POSTs"),
        wire!(Response, "http", OkResponse, "mutations with no object"),
        // gateway
        wire!(Record, "gateway", Frame, "every gateway frame"),
        wire!(Request, "gateway", GatewayQuery, "GET /api/v1/gateway"),
        wire!(Enum, "gateway", Encoding, ""),
        wire!(Enum, "gateway", Compression, ""),
        // gateway/client
        wire!(Request, "gateway/client", IdentifyRequest, "op 2 IDENTIFY"),
        wire!(Request, "gateway/client", ResumeRequest, "op 4 RESUME"),
        wire!(Enum, "gateway/client", PresenceSetStatus, ""),
        wire!(
            Request,
            "gateway/client",
            PresenceSetRequest,
            "op 3 PRESENCE_SET"
        ),
        wire!(
            Request,
            "gateway/client",
            SubscribeRequest,
            "op 5 SUBSCRIBE"
        ),
        wire!(Request, "gateway/client", TypingRequest, "op 10 TYPING"),
        wire!(Request, "gateway/client", ReauthRequest, "op 11 REAUTH"),
        wire!(Request, "gateway/client", ActivityRequest, "op 12 ACTIVITY"),
        // gateway/server
        wire!(Response, "gateway/server", Hello, "op 8 HELLO"),
        wire!(Response, "gateway/server", Reconnect, "op 6 RECONNECT"),
        wire!(
            Response,
            "gateway/server",
            InvalidSession,
            "op 7 INVALID_SESSION"
        ),
        wire!(
            Response,
            "gateway/server",
            RateLimitedCloseReason,
            "close 4008 reason"
        ),
        wire!(Event, "gateway/server", Ready, "event ready"),
        // gateway/events
        wire!(Enum, "gateway/events", EventName, "dispatch t"),
        wire!(Event, "gateway/events", Resumed, "event resumed"),
        wire!(
            Event,
            "gateway/events",
            MessageDeleted,
            "event message_deleted"
        ),
        wire!(
            Event,
            "gateway/events",
            ReactionEvent,
            "events reaction_added, reaction_removed"
        ),
        wire!(
            Event,
            "gateway/events",
            ChannelMemberEvent,
            "events channel_member_added, channel_member_removed"
        ),
        wire!(
            Event,
            "gateway/events",
            TypingStarted,
            "event typing_started"
        ),
        wire!(Enum, "gateway/events", PresenceStatus, ""),
        wire!(
            Event,
            "gateway/events",
            PresenceUpdated,
            "event presence_updated"
        ),
        wire!(Event, "gateway/events", PermsChanged, "event perms_changed"),
        // mgmt
        wire!(Response, "mgmt", Health, "GET /mgmt/v1/health"),
        wire!(Enum, "mgmt", ProviderKind, ""),
        wire!(Response, "mgmt", Status, "GET /mgmt/v1/status"),
        wire!(
            Request,
            "mgmt",
            ProvisionRequest,
            "POST /mgmt/v1/principals/provision"
        ),
        wire!(
            Response,
            "mgmt",
            ProvisionResponse,
            "POST /mgmt/v1/principals/provision"
        ),
        wire!(
            Request,
            "mgmt",
            UpdatePrincipalRequest,
            "POST /mgmt/v1/principals/update"
        ),
        wire!(
            Request,
            "mgmt",
            DeprovisionRequest,
            "POST /mgmt/v1/principals/deprovision"
        ),
        wire!(
            Response,
            "mgmt",
            DeprovisionResponse,
            "POST /mgmt/v1/principals/deprovision"
        ),
        wire!(
            Request,
            "mgmt",
            RevokeSessionsRequest,
            "POST /mgmt/v1/sessions/revoke"
        ),
        wire!(
            Response,
            "mgmt",
            RevokeSessionsResponse,
            "POST /mgmt/v1/sessions/revoke"
        ),
        wire!(Request, "mgmt", OwnerSetRequest, "POST /mgmt/v1/owner/set"),
        wire!(
            Request,
            "mgmt",
            PinRequest,
            "POST /mgmt/v1/agents/pin, /unpin"
        ),
        wire!(
            Request,
            "mgmt",
            DeliveriesQuery,
            "GET /mgmt/v1/agents/{member_id}/deliveries"
        ),
        wire!(
            Response,
            "mgmt",
            DeliveriesPage,
            "GET /mgmt/v1/agents/{member_id}/deliveries"
        ),
        wire!(Enum, "mgmt", DeliveryKind, "DeliveryMessage.kind"),
        wire!(Enum, "mgmt", DeliveryPlace, "DeliveryMessage.place"),
        wire!(
            Response,
            "mgmt",
            StartJobResponse,
            "POST /mgmt/v1/backup, /export, /keys/rotate-master"
        ),
        wire!(Enum, "mgmt", JobState, ""),
        wire!(Response, "mgmt", JobStatus, "GET /mgmt/v1/jobs/{id}"),
        wire!(Enum, "mgmt", StatusAlarm, "status.alarms"),
        wire!(
            Response,
            "mgmt",
            OwnerSetResponse,
            "POST /mgmt/v1/owner/set"
        ),
        wire!(
            Response,
            "mgmt",
            PinResponse,
            "POST /mgmt/v1/agents/pin, /unpin"
        ),
        wire!(Request, "mgmt", PrincipalsQuery, "GET /mgmt/v1/principals"),
        wire!(Enum, "mgmt", PrincipalsInclude, ""),
        wire!(
            Response,
            "mgmt",
            MgmtPrincipalPage,
            "GET /mgmt/v1/principals"
        ),
        wire!(Request, "mgmt", JobsQuery, "GET /mgmt/v1/jobs"),
        wire!(Response, "mgmt", JobStatusPage, "GET /mgmt/v1/jobs"),
        // mgmt backups (SW15)
        wire!(Response, "mgmt", MgmtBackupList, "GET /mgmt/v1/backups"),
        wire!(
            Record,
            "mgmt",
            BackupJobResult,
            "JobStatus.result of a backup job"
        ),
        // mgmt spaces (SW11, SW13)
        wire!(
            Request,
            "mgmt",
            MgmtSpaceCreateRequest,
            "POST /mgmt/v1/spaces/create"
        ),
        wire!(
            Response,
            "mgmt",
            MgmtSpaceResponse,
            "POST /mgmt/v1/spaces/create, POST /mgmt/v1/spaces/bind"
        ),
        wire!(
            Request,
            "mgmt",
            MgmtSpaceBindRequest,
            "POST /mgmt/v1/spaces/bind"
        ),
        wire!(
            Request,
            "mgmt",
            MgmtSpaceMembersAddRequest,
            "POST /mgmt/v1/spaces/members/add"
        ),
        wire!(
            Response,
            "mgmt",
            MgmtSpaceMembersAddResponse,
            "POST /mgmt/v1/spaces/members/add"
        ),
        wire!(Enum, "mgmt", MgmtSpaceNotAddedReason, ""),
        wire!(
            Request,
            "mgmt",
            MgmtSpaceMemberRemoveRequest,
            "POST /mgmt/v1/spaces/members/remove"
        ),
        wire!(
            Response,
            "mgmt",
            MgmtSpaceMemberRemoveResponse,
            "POST /mgmt/v1/spaces/members/remove"
        ),
        // ops (SW5): install inputs as JSON (`--config-json`)
        wire!(Request, "ops", InstallInputs, "sew install --config-json"),
        wire!(
            Request,
            "ops",
            MediaInstallInputs,
            "sew media install --config-json"
        ),
        // mgmt/local (contracts rev 17, S24 §7)
        wire!(Enum, "mgmt/local", LocalRole, ""),
        wire!(Enum, "mgmt/local", LocalSettableRole, ""),
        wire!(
            Request,
            "mgmt/local",
            LocalUserCreateRequest,
            "POST /mgmt/v1/local/users"
        ),
        wire!(
            Request,
            "mgmt/local",
            SetupLinkRequest,
            "POST /mgmt/v1/local/setup-link"
        ),
        wire!(
            Request,
            "mgmt/local",
            LocalUserRef,
            "POST /mgmt/v1/local/users/reactivate"
        ),
        wire!(
            Request,
            "mgmt/local",
            LocalRoleRequest,
            "POST /mgmt/v1/local/users/role"
        ),
        wire!(
            Request,
            "mgmt/local",
            LocalGuestRequest,
            "POST /mgmt/v1/local/users/guest"
        ),
        wire!(
            Response,
            "mgmt/local",
            LocalUserResponse,
            "every /mgmt/v1/local/* route"
        ),
        // mgmt ops (contracts rev 17, S24 §7.2)
        wire!(Enum, "mgmt", AuditVerifyMode, ""),
        wire!(
            Request,
            "mgmt",
            AuditVerifyRequest,
            "POST /mgmt/v1/audit/verify"
        ),
        wire!(
            Record,
            "mgmt",
            AuditVerifyReport,
            "JobStatus.result of an audit_verify job"
        ),
        wire!(Request, "mgmt", ExportRequest, "POST /mgmt/v1/export"),
        // policy
        wire!(Enum, "policy", PolicyKey, ""),
        wire!(Enum, "policy", SpacesCreate, "spaces.create"),
        wire!(Enum, "policy", MeetingsCreate, "meetings.create"),
        wire!(
            Enum,
            "policy",
            ChannelCreateFloor,
            "channels.create_*_floor"
        ),
        wire!(Enum, "policy", ArchiveWho, "channels.archive"),
        wire!(Enum, "policy", GuestInvite, "guests.invite"),
        wire!(Record, "policy", Policies, "every org policy"),
        wire!(Request, "policy", PolicyPatch, "partial policy change"),
        wire!(
            Response,
            "policy",
            PoliciesResponse,
            "GET /api/v1/install/policies, GET /mgmt/v1/policies"
        ),
        wire!(
            Request,
            "policy",
            SetPoliciesRequest,
            "POST /api/v1/install/policies, POST /mgmt/v1/policies"
        ),
        // audit
        wire!(Enum, "audit", ActorKind, ""),
        wire!(Enum, "audit", AuditResult, ""),
        wire!(Enum, "audit", Action, ""),
        wire!(Record, "audit", AuditEvent, "audit_events rows"),
        // when
        wire!(
            Request,
            "when",
            When,
            "scheduled sends, reminders, snoozes (S17)"
        ),
        // ---- S10: spaces, categories, channels, membership, DMs (prd-sew-10 §8)
        wire!(Enum, "api/members", SettableInstallRole, ""),
        wire!(
            Request,
            "api/members",
            SetInstallRoleRequest,
            "POST /api/v1/install/members/{id}/role"
        ),
        wire!(Enum, "api/spaces", SpaceJoinPolicy, ""),
        wire!(Enum, "api/spaces", SpaceListScope, ""),
        wire!(Request, "api/spaces", SpaceListQuery, "GET /api/v1/spaces"),
        wire!(Response, "api/spaces", SpacePage, "GET /api/v1/spaces"),
        wire!(
            Record,
            "api/spaces",
            SpaceMember,
            "GET /api/v1/spaces/{id}/members"
        ),
        wire!(
            Response,
            "api/spaces",
            SpaceMemberPage,
            "GET /api/v1/spaces/{id}/members"
        ),
        wire!(
            Request,
            "api/spaces",
            SpaceMembersQuery,
            "GET /api/v1/spaces/{id}/members"
        ),
        wire!(
            Request,
            "api/spaces",
            AddSpaceMembersRequest,
            "POST /api/v1/spaces/{id}/members/add"
        ),
        wire!(
            Response,
            "api/spaces",
            AddSpaceMembersResponse,
            "POST /api/v1/spaces/{id}/members/add"
        ),
        wire!(
            Request,
            "api/spaces",
            RemoveSpaceMemberRequest,
            "POST /api/v1/spaces/{id}/members/remove"
        ),
        wire!(
            Request,
            "api/spaces",
            TransferSpaceRequest,
            "POST /api/v1/spaces/{id}/transfer"
        ),
        wire!(
            Request,
            "api/spaces",
            ReorderCategoriesRequest,
            "POST /api/v1/spaces/{id}/categories/reorder"
        ),
        wire!(
            Response,
            "api/spaces",
            CategoriesResponse,
            "GET /api/v1/spaces/{id}/categories, category reorder"
        ),
        wire!(
            Response,
            "api/channels",
            ChannelPage,
            "GET /api/v1/spaces/{id}/channels, GET /api/v1/me/channels"
        ),
        wire!(Enum, "api/channels", ChannelListScope, ""),
        wire!(
            Request,
            "api/channels",
            ChannelListQuery,
            "GET /api/v1/spaces/{id}/channels"
        ),
        wire!(Enum, "api/channels", MyChannelsKind, ""),
        wire!(
            Request,
            "api/channels",
            MyChannelsQuery,
            "GET /api/v1/me/channels"
        ),
        wire!(
            Response,
            "api/channels",
            AddChannelMembersResponse,
            "POST /api/v1/channels/{id}/members/add"
        ),
        wire!(
            Request,
            "api/channels",
            ReorderChannelsRequest,
            "POST /api/v1/spaces/{id}/channels/reorder"
        ),
        wire!(
            Response,
            "api/channels",
            ReorderChannelsResponse,
            "POST /api/v1/spaces/{id}/channels/reorder"
        ),
        wire!(
            Record,
            "error",
            ConflictDetails,
            "details of conflict (S10)"
        ),
        wire!(
            Record,
            "error",
            QuotaDetails,
            "details of quota_exceeded (S10)"
        ),
        wire!(
            Event,
            "gateway/events",
            SpaceMemberAdded,
            "event space_member_added"
        ),
        wire!(
            Event,
            "gateway/events",
            SpaceMemberRemoved,
            "event space_member_removed"
        ),
        wire!(
            Event,
            "gateway/events",
            CategoryDeleted,
            "event category_deleted"
        ),
        // ---- S13: messaging (prd-sew-13 §19)
        wire!(
            Request,
            "api/messages",
            RemoveReactionRequest,
            "POST /api/v1/messages/{id}/reactions/remove"
        ),
        wire!(
            Request,
            "api/messages",
            MessagesByIdsQuery,
            "GET /api/v1/messages?ids="
        ),
        wire!(
            Request,
            "api/messages",
            ReactorsQuery,
            "GET /api/v1/messages/{id}/reactors"
        ),
        wire!(
            Record,
            "api/messages",
            Reactor,
            "GET /api/v1/messages/{id}/reactors"
        ),
        wire!(
            Response,
            "api/messages",
            ReactorPage,
            "GET /api/v1/messages/{id}/reactors"
        ),
        wire!(
            Event,
            "gateway/events",
            MessagePinned,
            "event message_pinned"
        ),
        wire!(
            Event,
            "gateway/events",
            MessageUnpinned,
            "event message_unpinned"
        ),
        // ---- S12: roles, overwrites, moderation (prd-sew-12 §7)
        wire!(
            Request,
            "api/roles",
            CreateRoleRequest,
            "POST /api/v1/spaces/{id}/roles"
        ),
        wire!(
            Request,
            "api/roles",
            UpdateRoleRequest,
            "POST /api/v1/roles/{id}/edit"
        ),
        wire!(
            Request,
            "api/roles",
            ReorderRolesRequest,
            "POST /api/v1/spaces/{id}/roles/reorder"
        ),
        wire!(
            Request,
            "api/roles",
            MemberRolesRequest,
            "POST /api/v1/spaces/{id}/members/{member_id}/roles/add, /remove"
        ),
        wire!(
            Response,
            "api/roles",
            RolePage,
            "GET /api/v1/spaces/{id}/roles, role reorder"
        ),
        wire!(
            Request,
            "api/overwrites",
            SetOverwriteRequest,
            "POST /api/v1/channels/{id}/overwrites/set, category set"
        ),
        wire!(
            Request,
            "api/overwrites",
            OverwriteRef,
            "POST /api/v1/channels/{id}/overwrites/delete, category delete"
        ),
        wire!(
            Response,
            "api/overwrites",
            ChannelOverwrites,
            "GET /api/v1/channels/{id}/overwrites, overwrite edits, sync"
        ),
        wire!(
            Request,
            "api/moderation",
            TimeoutRequest,
            "POST /api/v1/spaces/{id}/members/{member_id}/timeout"
        ),
        wire!(
            Request,
            "api/moderation",
            SlowModeRequest,
            "POST /api/v1/channels/{id}/slow_mode/set"
        ),
        wire!(Event, "gateway/events", RoleDeleted, "event role_deleted"),
        wire!(
            Event,
            "gateway/events",
            InstallUpdated,
            "event install_updated"
        ),
        // ---- S18: people, presence, directory, user groups (prd-sew-18 §14)
        wire!(Enum, "api/people", ProfileFieldKind, ""),
        wire!(Enum, "api/people", ProfileFieldManagedBy, ""),
        wire!(Enum, "api/people", ProfileFieldVisibility, ""),
        wire!(Enum, "api/people", ResettableField, ""),
        wire!(Enum, "api/people", ManualPresence, ""),
        wire!(Enum, "api/people", SelfStatus, ""),
        wire!(Enum, "api/people", DndDay, ""),
        wire!(Enum, "api/people", DndSource, ""),
        wire!(Enum, "api/people", MentionableKind, ""),
        wire!(
            Request,
            "api/people",
            SetStatusRequest,
            "POST /api/v1/me/status"
        ),
        wire!(
            Request,
            "api/people",
            UpdateMyProfileRequest,
            "POST /api/v1/me/profile"
        ),
        wire!(
            Record,
            "api/people",
            ProfileField,
            "custom profile field definitions"
        ),
        wire!(
            Request,
            "api/people",
            CreateProfileFieldRequest,
            "POST /api/v1/install/profile-fields, POST /mgmt/v1/profile-fields/upsert"
        ),
        wire!(
            Request,
            "api/people",
            UpdateProfileFieldRequest,
            "POST /api/v1/install/profile-fields/{id}/update"
        ),
        wire!(
            Request,
            "api/people",
            ReorderProfileFieldsRequest,
            "POST /api/v1/install/profile-fields/reorder"
        ),
        wire!(
            Response,
            "api/people",
            ProfileFieldsResponse,
            "GET /api/v1/install/profile-fields, GET /mgmt/v1/profile-fields"
        ),
        wire!(
            Request,
            "api/people",
            ResetProfileRequest,
            "POST /api/v1/members/{id}/profile/reset"
        ),
        wire!(Record, "api/people", DndState, "DND now (S21 dnd_state)"),
        wire!(
            Record,
            "api/people",
            PresenceSettings,
            "GET/POST /api/v1/me/presence, event presence_settings_updated"
        ),
        wire!(
            Request,
            "api/people",
            SetPresenceRequest,
            "POST /api/v1/me/presence"
        ),
        wire!(
            Request,
            "api/people",
            SetDndScheduleRequest,
            "POST /api/v1/me/dnd/schedule"
        ),
        wire!(Request, "api/people", PresenceQuery, "GET /api/v1/presence"),
        wire!(Response, "api/people", PresenceList, "GET /api/v1/presence"),
        wire!(
            Request,
            "api/people",
            DirectoryQuery,
            "GET /api/v1/directory"
        ),
        wire!(
            Response,
            "api/people",
            DirectoryPage,
            "GET /api/v1/directory"
        ),
        wire!(Request, "api/people", MembersQuery, "GET /api/v1/members"),
        wire!(
            Record,
            "api/people",
            UserGroup,
            "user groups, event user_group_updated"
        ),
        wire!(
            Request,
            "api/people",
            CreateUserGroupRequest,
            "POST /api/v1/groups"
        ),
        wire!(
            Request,
            "api/people",
            UpdateUserGroupRequest,
            "POST /api/v1/groups/{id}/update"
        ),
        wire!(
            Request,
            "api/people",
            GroupMembersRequest,
            "POST /api/v1/groups/{id}/members/add|remove"
        ),
        wire!(
            Response,
            "api/people",
            AddGroupMembersResponse,
            "POST /api/v1/groups/{id}/members/add"
        ),
        wire!(
            Response,
            "api/people",
            RemoveGroupMembersResponse,
            "POST /api/v1/groups/{id}/members/remove"
        ),
        wire!(
            Record,
            "api/people",
            GroupMember,
            "GET /api/v1/groups/{id}/members"
        ),
        wire!(
            Response,
            "api/people",
            GroupMemberPage,
            "GET /api/v1/groups/{id}/members"
        ),
        wire!(Request, "api/people", GroupsQuery, "GET /api/v1/groups"),
        wire!(
            Response,
            "api/people",
            UserGroupsResponse,
            "GET /api/v1/groups"
        ),
        wire!(
            Request,
            "api/people",
            MentionablesQuery,
            "GET /api/v1/channels/{id}/mentionables"
        ),
        wire!(
            Response,
            "api/people",
            MentionablesResponse,
            "GET /api/v1/channels/{id}/mentionables"
        ),
        wire!(
            Event,
            "api/people",
            PresenceSnapshot,
            "event presence_snapshot"
        ),
        wire!(
            Event,
            "api/people",
            MemberListUpdated,
            "event member_list_updated"
        ),
        wire!(
            Event,
            "api/people",
            UserGroupDeleted,
            "event user_group_deleted"
        ),
        wire!(
            Event,
            "api/people",
            ProfileFieldsUpdated,
            "event profile_fields_updated"
        ),
        wire!(
            Request,
            "api/people",
            MgmtAvatarQuery,
            "POST /mgmt/v1/principals/avatar"
        ),
        wire!(
            Request,
            "api/people",
            MgmtAvatarClearRequest,
            "POST /mgmt/v1/principals/avatar/clear"
        ),
        wire!(
            Response,
            "api/people",
            MgmtAvatarResponse,
            "POST /mgmt/v1/principals/avatar[/clear]"
        ),
        // api/files (S15)
        wire!(Enum, "api/files", FileKind, ""),
        wire!(
            Request,
            "api/files",
            CreateUploadRequest,
            "POST /api/v1/channels/{id}/uploads"
        ),
        wire!(
            Response,
            "api/files",
            CreateUploadResponse,
            "POST /api/v1/channels/{id}/uploads"
        ),
        wire!(
            Response,
            "api/files",
            UploadStatus,
            "POST, GET /api/v1/uploads/{id}/data (signed)"
        ),
        wire!(Response, "api/files", Upload, "GET /api/v1/uploads/{id}"),
        wire!(
            Request,
            "api/files",
            RemoveAttachmentsRequest,
            "POST /api/v1/messages/{id}/attachments/remove"
        ),
        wire!(
            Request,
            "api/files",
            FilesQuery,
            "GET /api/v1/channels/{id}/files"
        ),
        wire!(Record, "api/files", ChannelFile, "GET /api/v1/files/{id}"),
        wire!(
            Response,
            "api/files",
            ChannelFilePage,
            "GET /api/v1/channels/{id}/files"
        ),
        wire!(
            Event,
            "gateway/events",
            AttachmentsUpdated,
            "event attachments_updated"
        ),
        // ---- S16: emoji, GIFs, link previews, secrets (prd-sew-16 §16)
        wire!(
            Record,
            "api/media",
            CustomEmoji,
            "GET /api/v1/emoji/{id}, emoji create/edit"
        ),
        wire!(
            Response,
            "api/media",
            CustomEmojiPage,
            "GET /api/v1/spaces/{id}/emoji"
        ),
        wire!(Record, "api/media", CustomEmojiRef, "Message.emojis"),
        wire!(
            Request,
            "api/media",
            CreateEmojiQuery,
            "POST /api/v1/spaces/{id}/emoji?name=&aliases="
        ),
        wire!(
            Request,
            "api/media",
            UpdateEmojiRequest,
            "POST /api/v1/emoji/{id}/edit"
        ),
        wire!(Enum, "api/media", LinkPreviewKind, "LinkPreview.kind"),
        wire!(Record, "api/media", LinkPreview, "Message.previews"),
        wire!(
            Request,
            "api/media",
            GifSearchQuery,
            "GET /api/v1/gifs/search"
        ),
        wire!(
            Request,
            "api/media",
            GifPageQuery,
            "GET /api/v1/gifs/trending"
        ),
        wire!(
            Response,
            "api/media",
            GifPage,
            "GET /api/v1/gifs/search, trending"
        ),
        wire!(
            Request,
            "api/media",
            GifAttachRequest,
            "POST /api/v1/channels/{id}/gifs/attach"
        ),
        wire!(
            Response,
            "api/media",
            GifAttachResponse,
            "POST /api/v1/channels/{id}/gifs/attach"
        ),
        wire!(
            Request,
            "api/media",
            SetSecretRequest,
            "POST /api/v1/install/secrets/set, POST /mgmt/v1/secrets/set"
        ),
        wire!(
            Request,
            "api/media",
            ClearSecretRequest,
            "POST /api/v1/install/secrets/clear, POST /mgmt/v1/secrets/clear"
        ),
        wire!(Enum, "api/media", SecretSetVia, "SecretStatus.set_via"),
        wire!(
            Response,
            "api/media",
            SecretStatus,
            "install secrets set/clear"
        ),
        wire!(
            Response,
            "api/media",
            SecretsResponse,
            "GET /api/v1/install/secrets, GET /mgmt/v1/secrets"
        ),
        wire!(Event, "gateway/events", EmojiUpdated, "event emoji_updated"),
        wire!(Event, "gateway/events", EmojiDeleted, "event emoji_deleted"),
        wire!(
            Event,
            "gateway/events",
            MessagePreviewsUpdated,
            "event message_previews_updated"
        ),
        // api/compliance (S22)
        wire!(Enum, "api/compliance", PolicyState, ""),
        wire!(Enum, "api/compliance", CreatedVia, ""),
        wire!(Enum, "api/compliance", RunState, ""),
        wire!(Enum, "api/compliance", RunTrigger, ""),
        wire!(Enum, "api/compliance", HoldStateFilter, ""),
        wire!(Enum, "api/compliance", VanishReason, ""),
        wire!(Record, "api/compliance", RetentionScope, ""),
        wire!(Record, "api/compliance", RetentionValues, ""),
        wire!(
            Record,
            "api/compliance",
            RetentionPolicy,
            "GET /api/v1/compliance/retention"
        ),
        wire!(
            Response,
            "api/compliance",
            RetentionPolicyPage,
            "GET /api/v1/compliance/retention"
        ),
        wire!(
            Request,
            "api/compliance",
            RetentionListQuery,
            "GET /api/v1/compliance/retention"
        ),
        wire!(
            Request,
            "api/compliance",
            EffectiveRetentionQuery,
            "GET /api/v1/compliance/retention/effective"
        ),
        wire!(
            Response,
            "api/compliance",
            EffectiveRetention,
            "GET /api/v1/compliance/retention/effective"
        ),
        wire!(
            Request,
            "api/compliance",
            PreviewRetentionRequest,
            "POST /api/v1/compliance/retention/preview"
        ),
        wire!(
            Response,
            "api/compliance",
            RetentionPreview,
            "POST /api/v1/compliance/retention/preview, GET /api/v1/compliance/retention/previews/{id}"
        ),
        wire!(
            Request,
            "api/compliance",
            SetRetentionRequest,
            "POST /api/v1/compliance/retention/set"
        ),
        wire!(Record, "api/compliance", RetentionTotals, ""),
        wire!(Record, "api/compliance", ChannelCounts, ""),
        wire!(
            Response,
            "api/compliance",
            RunRetentionResponse,
            "POST /api/v1/compliance/retention/run"
        ),
        wire!(
            Record,
            "api/compliance",
            RetentionRun,
            "GET /api/v1/compliance/retention/runs"
        ),
        wire!(
            Response,
            "api/compliance",
            RetentionRunPage,
            "GET /api/v1/compliance/retention/runs"
        ),
        wire!(
            Request,
            "api/compliance",
            RunsQuery,
            "GET /api/v1/compliance/retention/runs"
        ),
        wire!(
            Response,
            "api/compliance",
            LegalHold,
            "GET, POST /api/v1/compliance/holds[/{id}[/update|/release]]"
        ),
        wire!(
            Response,
            "api/compliance",
            LegalHoldPage,
            "GET /api/v1/compliance/holds"
        ),
        wire!(
            Request,
            "api/compliance",
            HoldsQuery,
            "GET /api/v1/compliance/holds"
        ),
        wire!(
            Request,
            "api/compliance",
            CreateHoldRequest,
            "POST /api/v1/compliance/holds"
        ),
        wire!(
            Request,
            "api/compliance",
            UpdateHoldRequest,
            "POST /api/v1/compliance/holds/{id}/update"
        ),
        wire!(
            Request,
            "api/compliance",
            ReleaseHoldRequest,
            "POST /api/v1/compliance/holds/{id}/release"
        ),
        wire!(
            Record,
            "api/compliance",
            ComplianceChannel,
            "GET /api/v1/compliance/channels"
        ),
        wire!(
            Response,
            "api/compliance",
            ComplianceChannelPage,
            "GET /api/v1/compliance/channels"
        ),
        wire!(
            Request,
            "api/compliance",
            ComplianceChannelsQuery,
            "GET /api/v1/compliance/channels"
        ),
        wire!(
            Event,
            "gateway/events",
            ContentVanished,
            "event content_vanished"
        ),
        // ---- S17: drafts, saved, scheduled, reminders, polls, bookmarks (prd-sew-17 §20)
        wire!(
            Response,
            "api/extras",
            DeletedRef,
            "POST …/delete (S17), *_deleted events"
        ),
        wire!(
            Record,
            "api/extras",
            Draft,
            "POST /api/v1/channels/{id}/draft, event draft_updated"
        ),
        wire!(Response, "api/extras", DraftPage, "GET /api/v1/me/drafts"),
        wire!(
            Request,
            "api/extras",
            SaveDraftRequest,
            "POST /api/v1/channels/{id}/draft"
        ),
        wire!(
            Request,
            "api/extras",
            DeleteDraftRequest,
            "POST /api/v1/channels/{id}/draft/delete"
        ),
        wire!(Request, "api/extras", DraftsQuery, "GET /api/v1/me/drafts"),
        wire!(Event, "gateway/events", DraftDeleted, "event draft_deleted"),
        wire!(Enum, "api/extras", SavedState, "SavedItem.state"),
        wire!(
            Record,
            "api/extras",
            SavedItem,
            "POST /api/v1/messages/{id}/save, event saved_item_updated"
        ),
        wire!(
            Response,
            "api/extras",
            SavedItemPage,
            "GET /api/v1/me/saved"
        ),
        wire!(
            Request,
            "api/extras",
            SaveMessageRequest,
            "POST /api/v1/messages/{id}/save"
        ),
        wire!(
            Request,
            "api/extras",
            EditSavedRequest,
            "POST /api/v1/saved/{id}/edit"
        ),
        wire!(Request, "api/extras", SavedQuery, "GET /api/v1/me/saved"),
        wire!(
            Event,
            "gateway/events",
            SavedItemDeleted,
            "event saved_item_deleted"
        ),
        wire!(Enum, "api/extras", ScheduledState, "ScheduledMessage.state"),
        wire!(
            Enum,
            "api/extras",
            ScheduledFailReason,
            "ScheduledMessage.fail_reason"
        ),
        wire!(
            Record,
            "api/extras",
            ScheduledMessage,
            "POST /api/v1/channels/{id}/scheduled, event scheduled_message_updated"
        ),
        wire!(
            Response,
            "api/extras",
            ScheduledMessagePage,
            "GET /api/v1/me/scheduled"
        ),
        wire!(
            Request,
            "api/extras",
            CreateScheduledRequest,
            "POST /api/v1/channels/{id}/scheduled"
        ),
        wire!(
            Request,
            "api/extras",
            EditScheduledRequest,
            "POST /api/v1/scheduled/{id}/edit"
        ),
        wire!(
            Request,
            "api/extras",
            ScheduledQuery,
            "GET /api/v1/me/scheduled"
        ),
        wire!(Enum, "api/extras", ReminderState, "Reminder.state"),
        wire!(
            Record,
            "api/extras",
            Reminder,
            "POST /api/v1/me/reminders, event reminder_updated"
        ),
        wire!(
            Response,
            "api/extras",
            ReminderPage,
            "GET /api/v1/me/reminders"
        ),
        wire!(
            Request,
            "api/extras",
            CreateReminderRequest,
            "POST /api/v1/me/reminders"
        ),
        wire!(
            Request,
            "api/extras",
            EditReminderRequest,
            "POST /api/v1/reminders/{id}/edit"
        ),
        wire!(
            Request,
            "api/extras",
            SnoozeReminderRequest,
            "POST /api/v1/reminders/{id}/snooze"
        ),
        wire!(
            Request,
            "api/extras",
            RemindersQuery,
            "GET /api/v1/me/reminders"
        ),
        wire!(
            Event,
            "gateway/events",
            ReminderFired,
            "event reminder_fired"
        ),
        wire!(
            Enum,
            "api/extras",
            PollResultsVisibility,
            "Poll.results_visibility"
        ),
        wire!(Enum, "api/extras", PollState, "Poll.state"),
        wire!(Record, "api/extras", PollInput, "CreateMessageRequest.poll"),
        wire!(
            Request,
            "api/extras",
            PollVoteRequest,
            "POST /api/v1/messages/{id}/poll/vote"
        ),
        wire!(
            Request,
            "api/extras",
            PollVotersQuery,
            "GET /api/v1/messages/{id}/poll/voters"
        ),
        wire!(
            Record,
            "api/extras",
            PollVoter,
            "GET /api/v1/messages/{id}/poll/voters"
        ),
        wire!(
            Response,
            "api/extras",
            PollVoterPage,
            "GET /api/v1/messages/{id}/poll/voters"
        ),
        wire!(Event, "gateway/events", PollUpdated, "event poll_updated"),
        wire!(
            Record,
            "api/extras",
            Bookmark,
            "POST /api/v1/channels/{id}/bookmarks, event bookmark_updated"
        ),
        wire!(
            Response,
            "api/extras",
            BookmarkPage,
            "GET /api/v1/channels/{id}/bookmarks"
        ),
        wire!(
            Request,
            "api/extras",
            CreateBookmarkRequest,
            "POST /api/v1/channels/{id}/bookmarks"
        ),
        wire!(
            Request,
            "api/extras",
            EditBookmarkRequest,
            "POST /api/v1/bookmarks/{id}/edit"
        ),
        wire!(
            Request,
            "api/extras",
            ReorderBookmarksRequest,
            "POST /api/v1/channels/{id}/bookmarks/reorder"
        ),
        wire!(
            Request,
            "api/extras",
            BookmarksQuery,
            "GET /api/v1/channels/{id}/bookmarks"
        ),
        wire!(
            Event,
            "gateway/events",
            BookmarkDeleted,
            "event bookmark_deleted"
        ),
        // ---- S20: search (prd-sew-20 §20)
        wire!(
            Request,
            "api/search",
            SearchRequest,
            "POST /api/v1/search/messages, /api/v1/search/files"
        ),
        wire!(
            Response,
            "api/search",
            SearchMessagesResponse,
            "POST /api/v1/search/messages"
        ),
        wire!(
            Response,
            "api/search",
            SearchFilesResponse,
            "POST /api/v1/search/files"
        ),
        wire!(Enum, "api/search", SearchSort, ""),
        wire!(Enum, "api/search", SearchErrorReason, "search errors"),
        wire!(Enum, "api/search", FilterName, "query filters"),
        wire!(Enum, "api/search", HasValue, "has: values"),
        wire!(Enum, "api/search", IsValue, "is: values"),
        wire!(Enum, "mgmt", SearchState, "status.search.state"),
        wire!(
            Request,
            "mgmt",
            SearchRebuildRequest,
            "POST /mgmt/v1/search/rebuild"
        ),
        wire!(
            Response,
            "mgmt",
            SearchRebuildResponse,
            "POST /mgmt/v1/search/rebuild"
        ),
        // api/guests (S19)
        wire!(
            Request,
            "api/guests",
            SetGuestRequest,
            "POST /api/v1/install/members/{id}/guest"
        ),
        wire!(
            Response,
            "api/guests",
            GuestConvertResponse,
            "POST /api/v1/install/members/{id}/guest"
        ),
        wire!(
            Record,
            "api/guests",
            GuestSummary,
            "GET /api/v1/install/guests"
        ),
        wire!(
            Response,
            "api/guests",
            GuestSummaryPage,
            "GET /api/v1/install/guests"
        ),
        wire!(
            Request,
            "api/guests",
            GuestListQuery,
            "GET /api/v1/install/guests"
        ),
        // ---- S21: notifications and the email digest (prd-sew-21 §18)
        wire!(Enum, "api/notify", NotifyLevel, "notification levels"),
        wire!(Enum, "api/notify", DigestInterval, "digest intervals"),
        wire!(Enum, "api/notify", DigestDetail, "policy digest.detail"),
        wire!(
            Response,
            "api/notify",
            NotifySettings,
            "GET /api/v1/me/notify, POST /api/v1/me/notify/set"
        ),
        wire!(
            Request,
            "api/notify",
            UpdateNotifySettingsRequest,
            "POST /api/v1/me/notify/set"
        ),
        wire!(
            Response,
            "api/notify",
            ChannelNotifyPrefs,
            "GET/POST /api/v1/channels/{id}/notify[/set]"
        ),
        wire!(
            Request,
            "api/notify",
            SetChannelNotifyRequest,
            "POST /api/v1/channels/{id}/notify/set"
        ),
        wire!(
            Response,
            "api/notify",
            SpaceNotifyPrefs,
            "GET/POST /api/v1/spaces/{id}/notify[/set]"
        ),
        wire!(
            Request,
            "api/notify",
            SetSpaceNotifyRequest,
            "POST /api/v1/spaces/{id}/notify/set"
        ),
        wire!(
            Response,
            "api/notify",
            NotifyPrefsList,
            "GET /api/v1/me/notify/prefs"
        ),
        wire!(
            Event,
            "gateway/events",
            Notification,
            "event notification_created"
        ),
        wire!(
            Event,
            "gateway/events",
            NotificationsReleased,
            "event notifications_released"
        ),
        wire!(
            Event,
            "gateway/events",
            NotificationPrefsUpdated,
            "event notification_prefs_updated"
        ),
        wire!(Record, "gateway/server", ReadyNotify, "Ready.notify"),
        wire!(Enum, "mgmt", MailTls, "status.mail.tls"),
        wire!(Request, "mgmt", MailTestRequest, "POST /mgmt/v1/mail/test"),
        wire!(
            Response,
            "mgmt",
            MailTestResponse,
            "POST /mgmt/v1/mail/test"
        ),
        // S23: compliance flag, audit API, exports (prd-sew-23 §14.3)
        wire!(Enum, "audit", AuditVisibility, "Action::visibility"),
        wire!(Enum, "api/exports", ExportState, "Export.state"),
        wire!(Enum, "api/exports", RequestedVia, "Export.requested_via"),
        wire!(
            Request,
            "api/exports",
            CreateEdiscoveryRequest,
            "POST /api/v1/compliance/exports, …/exports/estimate"
        ),
        wire!(
            Record,
            "api/exports",
            Export,
            "POST /api/v1/compliance/exports, GET …/exports/{id}"
        ),
        wire!(
            Response,
            "api/exports",
            ExportPage,
            "GET /api/v1/compliance/exports, GET /mgmt/v1/exports"
        ),
        wire!(
            Request,
            "api/exports",
            ExportsQuery,
            "GET /api/v1/compliance/exports, GET /mgmt/v1/exports"
        ),
        wire!(
            Response,
            "api/exports",
            ExportEstimate,
            "POST /api/v1/compliance/exports/estimate"
        ),
        wire!(
            Response,
            "api/exports",
            SignedLink,
            "POST /api/v1/compliance/exports/{id}/link"
        ),
        wire!(
            Request,
            "api/exports",
            SetComplianceRequest,
            "POST /api/v1/install/members/{id}/compliance"
        ),
        wire!(
            Response,
            "api/exports",
            ComplianceFlagResponse,
            "POST /api/v1/install/members/{id}/compliance, POST /mgmt/v1/principals/compliance"
        ),
        wire!(
            Response,
            "api/exports",
            ComplianceOfficers,
            "GET /api/v1/compliance/officers"
        ),
        wire!(
            Request,
            "mgmt",
            PrincipalComplianceRequest,
            "POST /mgmt/v1/principals/compliance"
        ),
        wire!(
            Request,
            "api/audit",
            AuditQuery,
            "GET /api/v1/audit, GET /mgmt/v1/audit"
        ),
        wire!(Record, "api/audit", AuditEntry, "GET /api/v1/audit"),
        wire!(
            Response,
            "api/audit",
            AuditEntryPage,
            "GET /api/v1/audit, GET /mgmt/v1/audit"
        ),
        wire!(
            Response,
            "api/audit",
            AuditChainStatus,
            "GET /api/v1/audit/chain"
        ),
        // The export archive, format 1 (prd-sew-23 §8)
        wire!(Enum, "export", ExportKind, "Manifest.kind"),
        wire!(Enum, "export", FileMode, "Manifest.options.files"),
        wire!(Enum, "export", MemberMode, "Manifest.scope.member_mode"),
        wire!(Enum, "export", DeleteKind, "MessageRecord.deleted.kind"),
        wire!(Enum, "export", RemoveKind, "AttachmentRecord.removed.kind"),
        wire!(Record, "export", Manifest, "manifest.json"),
        wire!(Record, "export", Trailer, "trailer.json"),
        wire!(Record, "export", InstallRecord, "install.ndjson"),
        wire!(
            Record,
            "export",
            ProfileFieldRecord,
            "profile_fields.ndjson"
        ),
        wire!(Record, "export", MemberRecord, "members.ndjson"),
        wire!(
            Record,
            "export",
            PresenceSettingsRecord,
            "presence_settings.ndjson"
        ),
        wire!(Record, "export", SpaceRecord, "spaces.ndjson"),
        wire!(Record, "export", RoleRecord, "roles.ndjson"),
        wire!(Record, "export", SpaceMemberRecord, "space_members.ndjson"),
        wire!(Record, "export", UserGroupRecord, "user_groups.ndjson"),
        wire!(Record, "export", CategoryRecord, "categories.ndjson"),
        wire!(Record, "export", ChannelRecord, "channels.ndjson"),
        wire!(
            Record,
            "export",
            ChannelMemberRecord,
            "channel_members.ndjson"
        ),
        wire!(
            Record,
            "export",
            ChannelPastMemberRecord,
            "channel_past_members.ndjson"
        ),
        wire!(Record, "export", EmojiRecord, "emoji.ndjson"),
        wire!(
            Record,
            "export",
            MessageRecord,
            "messages/<channel>/<part>.ndjson"
        ),
        wire!(
            Record,
            "export",
            RevisionRecord,
            "revisions/<channel>/<part>.ndjson"
        ),
        wire!(Record, "export", BookmarkRecord, "bookmarks.ndjson"),
        wire!(
            Record,
            "export",
            ThreadFollowRecord,
            "thread_follows.ndjson"
        ),
        wire!(Record, "export", ReadStateRecord, "read_states.ndjson"),
        wire!(Record, "export", ThreadReadRecord, "thread_reads.ndjson"),
        wire!(Record, "export", MentionRecord, "mentions.ndjson"),
        wire!(Record, "export", SavedItemRecord, "saved_items.ndjson"),
        wire!(Record, "export", ReminderRecord, "reminders.ndjson"),
        wire!(Record, "export", DraftRecord, "drafts.ndjson"),
        wire!(Record, "export", ScheduledRecord, "scheduled.ndjson"),
        wire!(Record, "export", NotifyPrefsRecord, "notify_prefs.ndjson"),
        wire!(Record, "export", AuditRecord, "audit.ndjson"),
        // S35 v2 §10.1: recordings in the archive.
        wire!(Enum, "export", SegmentState, "RecordingSegmentRecord.state"),
        wire!(Enum, "export", RunEnd, "RecordingRunRecord.end_reason"),
        wire!(Record, "export", RecordingRecord, "recordings.ndjson"),
        // S50: call key delivery (prd-sew-50 §20).
        wire!(
            Request,
            "e2ee",
            E2eeCaps,
            "E2eeJoin.caps, KeyJoinRequest.caps"
        ),
        wire!(Request, "e2ee", E2eeJoin, "VoiceJoinRequest.e2ee (K1)"),
        wire!(
            Request,
            "e2ee",
            KeyJoinRequest,
            "POST /api/v1/me/voice/keys/join (K2)"
        ),
        wire!(
            Record,
            "e2ee",
            CallKeysParams,
            "CallKeysInit.params, ReadyCallKeys.params"
        ),
        wire!(
            Response,
            "e2ee",
            CallKeysInit,
            "K1 VoiceJoinResponse.e2ee, K2, dispatch init"
        ),
        wire!(
            Request,
            "e2ee",
            KeyPackageUploadRequest,
            "POST /api/v1/me/voice/keys/key_packages (K16)"
        ),
        wire!(
            Record,
            "e2ee",
            KeyPackageRefItem,
            "KeyPackageUploadResponse.items"
        ),
        wire!(
            Response,
            "e2ee",
            KeyPackageUploadResponse,
            "POST /api/v1/me/voice/keys/key_packages (K16)"
        ),
        wire!(
            Record,
            "e2ee",
            LeafBindingEntryJson,
            "LeafBindingTable.entries"
        ),
        wire!(
            Record,
            "e2ee",
            LeafBindingTable,
            "call_keys_delivered, K3 reply, BindingsUpdate"
        ),
        wire!(
            Record,
            "e2ee",
            BindingsUpdate,
            "RecorderSink::bindings (S35 v2, S53)"
        ),
        wire!(
            Record,
            "e2ee",
            CallPublicKey,
            "ReadyCallKeys.ca, CallPublicKeys"
        ),
        wire!(
            Record,
            "e2ee",
            CallPublicKeys,
            "CallKeyService::public_keys (S35 v2)"
        ),
        wire!(Enum, "e2ee", KeyStateStatus, "KeyStateResponse.status"),
        wire!(
            Request,
            "e2ee",
            KeyStateRequest,
            "POST /api/v1/calls/{call_id}/keys/state (K3)"
        ),
        wire!(
            Response,
            "e2ee",
            KeyStateResponse,
            "POST /api/v1/calls/{call_id}/keys/state (K3)"
        ),
        wire!(
            Request,
            "e2ee",
            KeyCommitRequest,
            "POST /api/v1/calls/{call_id}/keys/commit (K4)"
        ),
        wire!(
            Response,
            "e2ee",
            KeyCommitAccepted,
            "POST /api/v1/calls/{call_id}/keys/commit (K4)"
        ),
        wire!(
            Request,
            "e2ee",
            KeyReadyRequest,
            "POST /api/v1/calls/{call_id}/keys/ready (K5)"
        ),
        wire!(Enum, "e2ee", InvalidReason, "KeyInvalidRequest.reason"),
        wire!(
            Request,
            "e2ee",
            KeyInvalidRequest,
            "POST /api/v1/calls/{call_id}/keys/invalid (K6)"
        ),
        wire!(
            Request,
            "e2ee",
            KeyResetPackageRequest,
            "POST /api/v1/calls/{call_id}/keys/key_package (K7)"
        ),
        wire!(
            Request,
            "e2ee",
            KeyReleaseRequest,
            "POST /api/v1/calls/{call_id}/keys/release (K8)"
        ),
        wire!(
            Record,
            "e2ee",
            Release,
            "RecorderSink::release (S35 v2, S53)"
        ),
        wire!(Enum, "e2ee", RekeyReason, "KeyRekeyRequest.reason"),
        wire!(
            Request,
            "e2ee",
            KeyRekeyRequest,
            "POST /api/v1/calls/{call_id}/keys/rekey (K9)"
        ),
        wire!(Enum, "e2ee", ReportKind, "KeyReportRequest.kind"),
        wire!(Request, "e2ee", ReportDetails, "KeyReportRequest.details"),
        wire!(
            Request,
            "e2ee",
            KeyReportRequest,
            "POST /api/v1/calls/{call_id}/keys/report (K10)"
        ),
        wire!(
            Enum,
            "e2ee",
            TransitionReason,
            "call_keys_delivered proposals.reason"
        ),
        wire!(
            Enum,
            "e2ee",
            ResetReason,
            "call_keys_delivered reset.reason"
        ),
        wire!(
            Enum,
            "e2ee",
            KeyResyncReason,
            "call_keys_delivered resync.reason"
        ),
        wire!(
            Enum,
            "e2ee",
            RemoveReason,
            "call_keys_delivered removed.reason"
        ),
        wire!(Event, "e2ee", CallKeysMessage, "event call_keys_delivered"),
        wire!(
            Enum,
            "e2ee",
            VerificationCode,
            "policy calls.verification_code"
        ),
        wire!(Record, "e2ee", ReadyCallKeys, "READY call_keys"),
        wire!(Enum, "e2ee", ConsentState, "ConsentSource (S35 v2)"),
        wire!(Record, "e2ee", StatusVoiceE2ee, "status.voice.e2ee"),
        wire!(Enum, "e2ee", DevicePlatform, "DeviceEnrollRequest.platform"),
        wire!(
            Request,
            "e2ee",
            DeviceEnrollRequest,
            "POST /api/v1/devices (K11)"
        ),
        wire!(
            Response,
            "e2ee",
            DeviceCertificateResponse,
            "POST /api/v1/devices (K11), …/renew (K12)"
        ),
        wire!(
            Record,
            "e2ee",
            DeviceSummary,
            "GET /api/v1/me/devices (K13)"
        ),
        wire!(Response, "e2ee", DevicePage, "GET /api/v1/me/devices (K13)"),
        wire!(
            Enum,
            "e2ee",
            DeviceRevokeReason,
            "DeviceRevokeRequest.reason"
        ),
        wire!(
            Request,
            "e2ee",
            DeviceRevokeRequest,
            "POST /api/v1/devices/{id}/revoke (K14)"
        ),
        wire!(
            Response,
            "e2ee",
            DeviceRevokeResponse,
            "POST /api/v1/devices/{id}/revoke (K14)"
        ),
        wire!(Response, "e2ee", CrlPage, "GET /api/v1/devices/crl (K15)"),
        wire!(Request, "e2ee", CrlQuery, "GET /api/v1/devices/crl (K15)"),
        // S33 v2: voice channels (prd-sew-33 v2 §19).
        wire!(Enum, "voice", VoiceConnection, "VoiceState.connection"),
        wire!(Enum, "voice", VoiceLeaveReason, "VoiceStateUpdated.reason"),
        wire!(
            Record,
            "voice",
            VoiceState,
            "voice states, READY voice_states"
        ),
        wire!(
            Request,
            "voice",
            VoiceJoinRequest,
            "POST /api/v1/channels/{id}/voice/join"
        ),
        wire!(
            Response,
            "voice",
            VoiceJoinResponse,
            "POST /api/v1/channels/{id}/voice/join"
        ),
        wire!(
            Request,
            "voice",
            VoiceSelfStateRequest,
            "POST /api/v1/me/voice/state"
        ),
        // F21: the call-stats beacon (prd-meet-01 CC14).
        wire!(Record, "voice", CallStatsBeacon, "VoiceJoinResponse.stats"),
        wire!(Enum, "voice", CallStatsClient, "CallStatsSample.client"),
        wire!(
            Enum,
            "voice",
            CallStatsCandidate,
            "CallStatsSample.candidate"
        ),
        wire!(
            Enum,
            "voice",
            CallStatsRelayProtocol,
            "CallStatsSample.relay_protocol"
        ),
        wire!(
            Request,
            "voice",
            CallStatsSample,
            "POST /api/v1/me/voice/stats"
        ),
        wire!(
            Request,
            "voice",
            VoiceFlagsRequest,
            "POST /api/v1/spaces/{id}/members/{member_id}/voice"
        ),
        wire!(
            Response,
            "voice",
            VoiceFlags,
            "POST /api/v1/spaces/{id}/members/{member_id}/voice"
        ),
        wire!(
            Request,
            "voice",
            VoiceMoveRequest,
            "POST /api/v1/spaces/{id}/members/{member_id}/voice/move"
        ),
        wire!(
            Event,
            "voice",
            VoiceStateUpdated,
            "event voice_state_updated"
        ),
        wire!(Enum, "voice", KeyJoinMode, "VoiceMoveKeyJoin.key_join"),
        wire!(Record, "voice", VoiceMoveKeyJoin, "VoiceServerUpdated.e2ee"),
        wire!(
            Event,
            "voice",
            VoiceServerUpdated,
            "event voice_server_updated"
        ),
        wire!(Record, "voice", ReadyVoiceChannel, "READY voice_channels"),
        wire!(Response, "voice", MeVoice, "GET /api/v1/me/voice"),
        wire!(
            Response,
            "voice",
            VoiceStatePage,
            "GET /api/v1/spaces/{id}/voice/states, /channels/{id}/voice/states"
        ),
        wire!(Enum, "voice", VoiceEngineState, "VoiceEngineStatus.state"),
        wire!(Record, "voice", VoiceEngineStatus, "status.voice.engine"),
        wire!(Record, "voice", VoiceStatus, "status.voice"),
        // S34 v2: video, DM calls and the media budget (prd-sew-34 v2 §24).
        wire!(
            Enum,
            "voice",
            MediaTier,
            "MediaPolicy.tier, status.voice.media.media_tier"
        ),
        wire!(Enum, "voice", VideoCodec, "MediaPolicy.codecs"),
        wire!(
            Enum,
            "voice",
            BudgetSource,
            "status.voice.media.budget_source"
        ),
        wire!(
            Event,
            "voice",
            MediaPolicy,
            "event media_policy_updated, join answers, READY install.media"
        ),
        wire!(Record, "voice", VoiceMediaStatus, "status.voice.media"),
        wire!(Enum, "calls", CallState, "Call.state"),
        wire!(Enum, "calls", RingState, "CallRing.state"),
        wire!(
            Enum,
            "calls",
            CallEndReason,
            "Call.end_reason, CallSummary.end_reason"
        ),
        wire!(
            Record,
            "calls",
            Call,
            "call_updated, GET /api/v1/calls/{call_id}"
        ),
        wire!(Record, "calls", CallSummary, "Message.call"),
        wire!(
            Request,
            "calls",
            CallJoinRequest,
            "POST /api/v1/channels/{id}/call/join"
        ),
        wire!(
            Response,
            "calls",
            CallJoinResponse,
            "POST /api/v1/channels/{id}/call/join"
        ),
        wire!(
            Request,
            "calls",
            CallDeclineRequest,
            "POST /api/v1/channels/{id}/call/decline"
        ),
        wire!(
            Request,
            "calls",
            CallRingRequest,
            "POST /api/v1/channels/{id}/call/ring"
        ),
        wire!(
            Response,
            "calls",
            ChannelCall,
            "GET /api/v1/channels/{id}/call"
        ),
        wire!(Response, "calls", MyCalls, "GET /api/v1/me/calls"),
        wire!(Event, "calls", CallUpdated, "event call_updated"),
        wire!(Event, "calls", CallRingEvent, "event call_ring"),
        // S35 v2: recording and transcripts (prd-sew-35 v2 §23).
        wire!(Enum, "recording", RecordingState, "Recording.state"),
        wire!(Enum, "recording", StopReason, "Recording.stop_reason"),
        wire!(Enum, "recording", Consent, "RecordingParticipant.consent"),
        wire!(Enum, "recording", Capture, "RecordingParticipant.capture"),
        wire!(
            Enum,
            "recording",
            NotRecordedReason,
            "RecordingParticipant.reason"
        ),
        wire!(Enum, "recording", ConsentVia, "audit recording.consent via"),
        wire!(
            Enum,
            "recording",
            ConsentSurface,
            "audit recording.consent surface"
        ),
        wire!(Enum, "recording", DeclineMode, "ConsentPrompt.decline_mode"),
        wire!(Enum, "recording", PromptRoomKind, "ConsentPrompt.room.kind"),
        wire!(
            Enum,
            "recording",
            AudioState,
            "RecordingSummary.audio.state"
        ),
        wire!(
            Enum,
            "recording",
            TranscriptState,
            "RecordingSummary.transcript.state"
        ),
        wire!(Enum, "recording", GapReason, "recording gaps"),
        wire!(Enum, "recording", Playback, "RecordingAudio.playback"),
        wire!(Enum, "recording", DownloadWhat, "DownloadRequest.what"),
        wire!(
            Request,
            "recording",
            RecordingNotice,
            "VoiceJoinRequest/CallJoinRequest.recording_notice"
        ),
        wire!(
            Record,
            "recording",
            ConsentPrompt,
            "409 recording_consent_required details, Recording.consent_prompt"
        ),
        wire!(
            Record,
            "recording",
            Recording,
            "GET /api/v1/recordings/{id}, join answers, READY recordings"
        ),
        wire!(
            Event,
            "recording",
            RecordingUpdated,
            "event recording_updated"
        ),
        wire!(
            Request,
            "recording",
            StartRecordingRequest,
            "POST /api/v1/channels/{id}/recording/start"
        ),
        wire!(
            Request,
            "recording",
            ConsentRequest,
            "POST /api/v1/recordings/{id}/consent"
        ),
        wire!(
            Request,
            "recording",
            AudioRetentionRequest,
            "POST /api/v1/recordings/{id}/audio/retention"
        ),
        wire!(
            Response,
            "recording",
            ActiveRecording,
            "GET /api/v1/channels/{id}/recording"
        ),
        wire!(
            Record,
            "recording",
            RecordingSummary,
            "Message.recording, DeliveryMessage.recording"
        ),
        wire!(Record, "recording", TranscriptTurn, "Message.transcript"),
        wire!(
            Response,
            "recording",
            RecordingAudio,
            "GET /api/v1/recordings/{id}/audio"
        ),
        wire!(
            Record,
            "recording",
            AudioSegment,
            "GET /api/v1/recordings/{id}/audio/tracks/{member_id}"
        ),
        wire!(
            Request,
            "recording",
            DownloadRequest,
            "POST /api/v1/compliance/recordings/{id}/downloads"
        ),
        wire!(
            Response,
            "recording",
            RecordingDownload,
            "POST /api/v1/compliance/recordings/{id}/downloads"
        ),
        wire!(
            Record,
            "recording",
            DownloadSegment,
            "GET /api/v1/compliance/recordings/{id}/downloads/{download_id}/segments"
        ),
        wire!(Record, "recording", StatusRecording, "status.recording"),
        // ---- Meet MC1: meetings for members (prd-meet-01 §6.14.2, CC4–CC10)
        wire!(Enum, "api/meetings", MeetingKind, "Meeting.kind"),
        wire!(Enum, "api/meetings", MeetingState, "Meeting.state"),
        wire!(Enum, "api/meetings", MeetingEndReason, "Meeting.end_reason"),
        wire!(
            Enum,
            "api/meetings",
            MeetingAttendeeKind,
            "attendees[].kind"
        ),
        wire!(
            Enum,
            "api/meetings",
            MeetingListScope,
            "GET /api/v1/meetings scope"
        ),
        wire!(
            Record,
            "api/meetings",
            Meeting,
            "GET /api/v1/meetings/{id}, event meeting_updated"
        ),
        wire!(
            Request,
            "api/meetings",
            CreateMeetingRequest,
            "POST /api/v1/meetings"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingCreated,
            "POST /api/v1/meetings"
        ),
        wire!(
            Request,
            "api/meetings",
            MeetingListQuery,
            "GET /api/v1/meetings"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingPage,
            "GET /api/v1/meetings"
        ),
        wire!(
            Request,
            "api/meetings",
            UpdateMeetingRequest,
            "POST /api/v1/meetings/{id}/update"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingAttendees,
            "GET /api/v1/meetings/{id}/attendees"
        ),
        wire!(
            Request,
            "api/meetings",
            RemoveMeetingAttendeeRequest,
            "POST /api/v1/meetings/{id}/attendees/remove"
        ),
        wire!(
            Event,
            "gateway/events",
            MeetingUpdated,
            "event meeting_updated"
        ),
        // ---- Meet MC2a: links and link guests (prd-meet-01 §6.4, §6.5, §6.14.1, CC1–CC10)
        wire!(
            Record,
            "api/meetings",
            MeetingLink,
            "MeetingCreated.link, GET /api/v1/meetings/{id}/link"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingLinkResponse,
            "GET /api/v1/meetings/{id}/link, POST …/link/rotate, …/link/revoke"
        ),
        wire!(
            Request,
            "api/meetings",
            RotateMeetingLinkRequest,
            "POST /api/v1/meetings/{id}/link/rotate"
        ),
        wire!(
            Request,
            "api/meetings",
            MeetingLinkRef,
            "POST /api/v1/meetings/peek, POST /api/v1/meetings/resolve"
        ),
        wire!(
            Record,
            "api/meetings",
            MeetingPeekRecording,
            "MeetingPeek.recording"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingPeek,
            "POST /api/v1/meetings/peek"
        ),
        wire!(
            Request,
            "api/meetings",
            MeetingRedeemRequest,
            "POST /api/v1/meetings/redeem"
        ),
        wire!(
            Enum,
            "api/meetings",
            MeetingRedeemState,
            "MeetingRedeemed.state"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingRedeemed,
            "POST /api/v1/meetings/redeem"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingResolved,
            "POST /api/v1/meetings/resolve"
        ),
        wire!(Record, "api/meetings", ChannelMeeting, "Channel.meeting"),
        // ---- Meet MC2b: the lobby (prd-meet-01 §6.5.4, §6.6, §6.14.2; CC5, CC6, CC10, CC12)
        wire!(
            Enum,
            "api/meetings",
            MeetingLobbyState,
            "MeetingLobbyEntry.state, MeetingLobbyStatus.state"
        ),
        wire!(
            Enum,
            "api/meetings",
            MeetingLobbyKind,
            "MeetingLobbyEntry.kind"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingLobbyEntry,
            "POST /api/v1/meetings/{id}/lobby/{entry}/admit, …/deny, event meeting_lobby_updated"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingLobby,
            "GET /api/v1/meetings/{id}/lobby"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingLobbyDecided,
            "POST /api/v1/meetings/{id}/lobby/admit_all"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingKnocked,
            "POST /api/v1/meetings/knock"
        ),
        wire!(
            Record,
            "api/meetings",
            MeetingLobbyStatus,
            "MeetingLobbyMe.items"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingLobbyMe,
            "GET /api/v1/meetings/lobby/me"
        ),
        wire!(
            Event,
            "gateway/events",
            MeetingLobbyUpdated,
            "event meeting_lobby_updated"
        ),
        wire!(
            Enum,
            "api/meetings",
            MeetingAdmissionOutcome,
            "MeetingAdmission.outcome"
        ),
        wire!(
            Event,
            "gateway/events",
            MeetingAdmission,
            "event meeting_admission"
        ),
        wire!(
            Event,
            "gateway/events",
            MeetingGuestUpdated,
            "event meeting_updated (link guests)"
        ),
        // ---- Meet MC3: host controls (prd-meet-01 §6.7; CC9, CC10)
        wire!(
            Request,
            "api/meetings",
            LockMeetingRequest,
            "POST /api/v1/meetings/{id}/lock"
        ),
        wire!(
            Request,
            "api/meetings",
            RemoveMeetingMemberRequest,
            "POST /api/v1/meetings/{id}/remove"
        ),
        // ---- Meet MC3b: names for everyone in a meeting channel
        wire!(
            Record,
            "api/meetings",
            MeetingPerson,
            "MeetingPersonPage.items, ChannelMemberEvent.person"
        ),
        wire!(
            Response,
            "api/meetings",
            MeetingPersonPage,
            "GET /api/v1/channels/{id}/people"
        ),
        wire!(Request, "mgmt", MeetInitRequest, "POST /mgmt/v1/meet/init"),
        wire!(
            Response,
            "mgmt",
            MeetInitResponse,
            "POST /mgmt/v1/meet/init"
        ),
    ]
}
