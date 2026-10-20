//! Example values for every top-level wire type (S1 §8).
//!
//! Ids are realistic single-node ids made at 2026-09-27T12:00:00.123Z (ms since the Sew epoch
//! `23_284_800_123`, node 0) with distinct sequence numbers, so `ID_MESSAGE` is
//! `48831765147549701` (seq 5). The `full` case sets every optional field and fills every list;
//! `min` sets only the required fields.

use serde_json::{Map, Value, json};

use crate::addr::{Addr, Handle, Slug};
use crate::api::auth::*;
use crate::api::channels::*;
use crate::api::files::*;
use crate::api::members::*;
use crate::api::messages::*;
use crate::api::resolve::*;
use crate::api::spaces::*;
use crate::audit::*;
use crate::changes::*;
use crate::error::*;
use crate::external_id::ExternalId;
use crate::gateway::client::*;
use crate::gateway::events::*;
use crate::gateway::server::*;
use crate::gateway::*;
use crate::http::*;
use crate::id::Id;
use crate::mgmt::local::*;
use crate::mgmt::*;
use crate::perms::{self, Perms};
use crate::policy::*;
use crate::registry::Example;
use crate::time::Timestamp;
use crate::when::When;

// S10's new wire types (spaces, categories, channels, membership, DMs).
mod s10;
// S13's new wire types (messaging).
mod s13;
// S12's new wire types (roles, overwrites, moderation).
mod s12;
mod s24;
// S18's new wire types (people, presence, groups).
mod s18;
// S16's new wire types (emoji, previews, GIFs, secrets).
mod s16;
// S22's new wire types (retention, legal holds, content_vanished).
mod s22;
// S17's new wire types (drafts, saved, scheduled, reminders, polls, bookmarks).
mod s17;
// S14's read state, changes and sync types.
mod s14;
// S20's new wire types (search).
mod s20;
// S19's new wire types (guest conversion and the guest list).
mod s19;
// S21's new wire types (notifications, the email digest).
mod s21;
// S23's new wire types (compliance flag, audit API, exports, the archive records).
mod s23;
// S33 v2's voice types (voice states, joins, moderation, voice dispatches, status).
mod s33;
// S34 v2's DM call and media policy types (calls, rings, summaries, `status.voice` media).
mod s34;
// S50's call key types (the key wire, devices, READY `call_keys`).
mod s50;
// S35 v2's recording types (recordings, consent, summaries, turns, audio, downloads).
mod s35;
// Meet MC1's meeting types (meetings, attendees, `meet/init`).
mod mc1;
// Meet MC2a's link and link-guest types (links, peek, redeem, resolve).
mod mc2a;
// Meet MC2b's lobby types (entries, knock, admission, the guest-shaped meeting_updated).
mod mc2b;
// Meet MC3's host-control bodies (lock, remove).
mod mc3;
// Meet MC3b's meeting people (names for everyone in a meeting channel).
mod mc3b;

const fn id(seq: u64) -> Id {
    // 23_284_800_123 << 21 = 48_831_765_147_549_696
    Id::new_const(48_831_765_147_549_696 + seq)
}

const ID_SPACE: Id = id(1);
const ID_CHANNEL: Id = id(2);
const ID_ME: Id = id(3);
const ID_BOB: Id = id(4);
const ID_MESSAGE: Id = id(5);
const ID_CATEGORY: Id = id(6);
const ID_ROLE: Id = id(7);
const ID_REPLY: Id = id(8);
const ID_ATTACHMENT: Id = id(9);
const ID_JOB: Id = id(10);
const ID_INSTALL: Id = id(11);
const ID_AUDIT: Id = id(12);
const ID_EMOJI: Id = id(13);
const ID_GROUP: Id = id(14);
const ID_DM: Id = id(15);
const ID_CAROL: Id = id(16);
const ID_OTHER_CHANNEL: Id = id(17);

/// 2026-09-27T12:00:00.123Z.
const T0_MS: u64 = 1_790_510_400_123;

fn t(offset_ms: u64) -> Timestamp {
    Timestamp::from_unix_ms_saturating(T0_MS + offset_ms)
}

fn slug(s: &str) -> Slug {
    match Slug::new(s) {
        Ok(v) => v,
        Err(e) => panic!("example slug {s:?}: {e}"),
    }
}

fn handle(s: &str) -> Handle {
    match Handle::new(s) {
        Ok(v) => v,
        Err(e) => panic!("example handle {s:?}: {e}"),
    }
}

fn ext(s: &str) -> ExternalId {
    match s.parse() {
        Ok(v) => v,
        Err(e) => panic!("example external id {s:?}: {e}"),
    }
}

fn addr(s: &str) -> Addr {
    match s.parse() {
        Ok(v) => v,
        Err(e) => panic!("example addr {s:?}: {e}"),
    }
}

const PRINCIPAL: &str = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
const WORKSPACE: &str = "k2:workspace:7c9e6679-7425-40de-944b-e07fc1f90ae7";
/// SW2: a K2 Connect user (`k2:connect:<uuid>`, contracts rev 134).
const CONNECT_USER: &str = "k2:connect:3d6f0a1e-5b2c-4e8d-9f7a-1c2b3d4e5f60";
const PASS: &str = "eyJhbGciOiJFUzI1NiIsImtpZCI6IjAxMjM0NTY3ODlhYmNkZWYiLCJ0eXAiOiJrMi1jaGF0LXBhc3Mrand0In0.eyJzdWIiOiJrMjpwcmluY2lwYWw6MGY4ZmFkNWIifQ.c2lnbmF0dXJl";

fn session_token() -> String {
    format!(
        "sews_example-selector-00001.example-secret-{}",
        "x".repeat(28)
    )
}

fn ticket() -> String {
    format!("sewtk_example-ticket-{}", "y".repeat(28))
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("example details must be an object, got {other}"),
    }
}

// ---------------------------------------------------------------- api/auth

impl Example for K2PassRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", K2PassRequest { pass: PASS.into() })]
    }
}

impl Example for RenewRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", RenewRequest { pass: PASS.into() })]
    }
}

impl Example for LoginRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            LoginRequest {
                username: handle("rosson"),
                password: "correct horse battery staple".into(),
            },
        )]
    }
}

impl Example for PasswordSetRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PasswordSetRequest {
                token: format!("sewpw_example-setup-token-{}", "z".repeat(24)),
                password: "correct horse battery staple".into(),
            },
        )]
    }
}

impl Example for SessionResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SessionResponse {
                token: session_token(),
                expires_at: t(3_600_000),
                member: member_min(),
            },
        )]
    }
}

impl Example for GatewayTicketResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            GatewayTicketResponse {
                ticket: ticket(),
                url: "wss://chat.example.com/api/v1/gateway?v=1&encoding=json".into(),
                expires_in_ms: 30_000,
            },
        )]
    }
}

// ---------------------------------------------------------------- api/members

fn member_full() -> Member {
    Member {
        id: ID_CAROL,
        kind: MemberKind::Agent,
        agent_kind: Some(AgentKind::Assistant),
        assistant_of: Some(ID_ME),
        install_role: InstallRole::Member,
        username: handle("rosson-assistant"),
        display_name: "Rosson's assistant".into(),
        title: Some("Assistant".into()),
        pronouns: Some("it/its".into()),
        avatar_url: Some("/api/v1/avatars/48831765147549712?exp=1790514000&sig=c2ln".into()),
        deactivated: false,
        created_at: t(0),
        email: Some("assistant@example.com".into()),
        compliance: Some(false),
        avatar_thumb_url: Some(
            "/api/v1/avatars/48831765147549712/48831765147549740/64?exp=1791158400&sig=c2ln".into(),
        ),
        timezone: Some("America/New_York".into()),
        status: Some(crate::api::people::MemberStatus {
            text: Some("In a meeting".into()),
            emoji: Some("📅".into()),
            expires_at: Some(t(1_800_000)),
        }),
        profile_rev: 7,
        custom_fields: Some(vec![crate::api::people::ProfileFieldValue {
            field_id: id(41),
            key: "team".into(),
            value: "Platform".into(),
        }]),
        managed_fields: Some(vec![
            "display_name".into(),
            "username".into(),
            "email".into(),
            "title".into(),
            "pronouns".into(),
        ]),
    }
}

fn member_min() -> Member {
    Member {
        id: ID_ME,
        kind: MemberKind::Person,
        agent_kind: None,
        assistant_of: None,
        install_role: InstallRole::Owner,
        username: handle("rosson"),
        display_name: "Rosson".into(),
        title: None,
        pronouns: None,
        avatar_url: None,
        deactivated: false,
        created_at: t(0),
        email: None,
        compliance: None,
        avatar_thumb_url: None,
        timezone: None,
        status: None,
        profile_rev: 0,
        custom_fields: None,
        managed_fields: None,
    }
}

impl Example for Member {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", member_full()), ("min", member_min())]
    }
}

impl Example for Page<Member> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![member_min(), member_full()],
                has_more: true,
            },
        )]
    }
}

// ---------------------------------------------------------------- api/spaces

fn space_min() -> Space {
    Space {
        id: ID_SPACE,
        slug: slug("acme-eng"),
        name: "Acme Engineering".into(),
        icon_url: None,
        icon_thumb_url: None,
        owner_id: ID_ME,
        member_count: 42,
        created_at: t(0),
        join_policy: SpaceJoinPolicy::Invite,
        my_perms: None,
        default_channel_ids: vec![],
        description: None,
        rules_channel_id: None,
        welcome_channel_id: None,
        app_id: None,
        hidden: false,
    }
}

impl Example for Space {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = Space {
            icon_url: Some("/api/v1/space-icons/48831765147549697/48831765147549716/512?exp=1791158400&sig=c2ln".into()),
            icon_thumb_url: Some("/api/v1/space-icons/48831765147549697/48831765147549716/64?exp=1791158400&sig=c2ln".into()),
            join_policy: SpaceJoinPolicy::Open,
            my_perms: Some(perms::DEFAULT_EVERYONE),
            default_channel_ids: vec![ID_CHANNEL],
            description: Some("Where Acme's engineers talk.".into()),
            rules_channel_id: Some(ID_OTHER_CHANNEL),
            welcome_channel_id: Some(ID_CHANNEL),
            app_id: Some("hearth".into()),
            hidden: true,
            ..space_min()
        };
        vec![("full", full), ("min", space_min())]
    }
}

impl Example for CreateSpaceRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateSpaceRequest {
                    slug: slug("acme-eng"),
                    name: "Acme Engineering".into(),
                    join_policy: Some(SpaceJoinPolicy::Open),
                },
            ),
            (
                "min",
                CreateSpaceRequest {
                    slug: slug("acme-eng"),
                    name: "Acme Engineering".into(),
                    join_policy: None,
                },
            ),
        ]
    }
}

impl Example for UpdateSpaceRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateSpaceRequest {
                    slug: Some(slug("acme-platform")),
                    name: Some("Acme Platform".into()),
                    join_policy: Some(SpaceJoinPolicy::Invite),
                    default_channel_ids: Some(vec![ID_CHANNEL]),
                    description: Patch::Set("Where Acme's engineers talk.".into()),
                    rules_channel_id: Patch::Set(ID_OTHER_CHANNEL),
                    welcome_channel_id: Patch::Clear,
                },
            ),
            ("min", UpdateSpaceRequest::default()),
        ]
    }
}

pub(super) fn overwrite() -> Overwrite {
    Overwrite {
        target_kind: OverwriteTarget::Role,
        target_id: ID_SPACE,
        allow: Perms::SEND_IN_THREADS,
        deny: Perms::SEND_MESSAGES,
    }
}

impl Example for Category {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Category {
                id: ID_CATEGORY,
                space_id: ID_SPACE,
                name: "Engineering".into(),
                position: 1,
                overwrites: vec![overwrite()],
            },
        )]
    }
}

impl Example for CreateCategoryRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateCategoryRequest {
                    name: "Engineering".into(),
                    position: Some(1),
                },
            ),
            (
                "min",
                CreateCategoryRequest {
                    name: "Engineering".into(),
                    position: None,
                },
            ),
        ]
    }
}

impl Example for UpdateCategoryRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateCategoryRequest {
                    name: Some("Platform".into()),
                    position: Some(2),
                },
            ),
            ("min", UpdateCategoryRequest::default()),
        ]
    }
}

pub(super) fn role_min() -> Role {
    Role {
        id: ID_SPACE,
        space_id: ID_SPACE,
        name: "@everyone".into(),
        color: None,
        position: 0,
        perms: perms::DEFAULT_EVERYONE,
        mentionable: false,
        is_everyone: true,
        hoist: false,
    }
}

impl Example for Role {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = Role {
            id: ID_ROLE,
            space_id: ID_SPACE,
            name: "Moderators".into(),
            color: Some(0x3b_82_f6),
            position: 3,
            perms: Perms::MANAGE_MESSAGES | Perms::PIN_MESSAGES | Perms::TIMEOUT_MEMBERS,
            mentionable: true,
            is_everyone: false,
            hoist: true,
        };
        vec![("full", full), ("min", role_min())]
    }
}

// ---------------------------------------------------------------- api/channels

fn channel_full() -> Channel {
    Channel {
        id: ID_CHANNEL,
        space_id: Some(ID_SPACE),
        category_id: Some(ID_CATEGORY),
        kind: ChannelType::Text,
        visibility: Visibility::Public,
        slug: Some(slug("deploys")),
        name: Some("deploys".into()),
        topic: Some("Release train status".into()),
        purpose: Some("Coordinate deploys".into()),
        position: 4,
        synced: true,
        archived_at: Some(t(86_400_000)),
        head_seq: 1_024,
        last_message_id: Some(ID_MESSAGE),
        dm_member_ids: vec![ID_ME, ID_BOB],
        my_perms: Some(perms::DEFAULT_EVERYONE),
        created_at: t(0),
        member_count: 12,
        created_by: Some(ID_ME),
        slow_mode_s: 30,
        retention_days: Some(90),
        file_retention_days: Some(30),
        has_guests: true,
        assistant_reader_ids: vec![ID_CAROL],
        user_limit: 0,
        recording_auto: false,
        recording_audio_days: None,
        recording_require_consent: None,
        meeting: None,
    }
}

/// A voice channel with a user limit (S33).
fn channel_voice() -> Channel {
    Channel {
        kind: ChannelType::Voice,
        slug: Some(slug("lounge")),
        name: Some("lounge".into()),
        archived_at: None,
        dm_member_ids: vec![],
        assistant_reader_ids: vec![],
        user_limit: 25,
        recording_auto: true,
        recording_audio_days: Some(30),
        recording_require_consent: Some(true),
        meeting: Some(crate::api::meetings::ChannelMeeting { id: id(600) }),
        ..channel_full()
    }
}

fn channel_min() -> Channel {
    Channel {
        id: ID_DM,
        space_id: None,
        category_id: None,
        kind: ChannelType::Dm,
        visibility: Visibility::Private,
        slug: None,
        name: None,
        topic: None,
        purpose: None,
        position: 0,
        synced: false,
        archived_at: None,
        head_seq: 0,
        last_message_id: None,
        dm_member_ids: vec![],
        my_perms: None,
        created_at: t(0),
        member_count: 0,
        created_by: None,
        slow_mode_s: 0,
        retention_days: None,
        file_retention_days: None,
        has_guests: false,
        assistant_reader_ids: vec![],
        user_limit: 0,
        recording_auto: false,
        recording_audio_days: None,
        recording_require_consent: None,
        meeting: None,
    }
}

impl Example for Channel {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", channel_full()),
            ("full_voice", channel_voice()),
            ("min", channel_min()),
        ]
    }
}

impl Example for CreateChannelRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = CreateChannelRequest {
            slug: slug("deploys"),
            name: "deploys".into(),
            kind: ChannelType::Text,
            visibility: Visibility::Public,
            category_id: None,
            topic: None,
            purpose: None,
            position: None,
            member_ids: vec![],
            user_limit: None,
        };
        let voice = CreateChannelRequest {
            slug: slug("lounge"),
            name: "lounge".into(),
            kind: ChannelType::Voice,
            user_limit: Some(25),
            ..min.clone()
        };
        let full = CreateChannelRequest {
            kind: ChannelType::Announcement,
            visibility: Visibility::Private,
            category_id: Some(ID_CATEGORY),
            topic: Some("Release train status".into()),
            purpose: Some("Coordinate deploys".into()),
            position: Some(0),
            member_ids: vec![ID_BOB, ID_CAROL],
            ..min.clone()
        };
        vec![("full", full), ("full_voice", voice), ("min", min)]
    }
}

impl Example for UpdateChannelRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateChannelRequest {
                    slug: Some(slug("releases")),
                    name: Some("releases".into()),
                    topic: Patch::Set("Release status".into()),
                    purpose: Patch::Set("Ship it".into()),
                    category_id: Patch::Set(ID_CATEGORY),
                    position: Some(2),
                    user_limit: Some(10),
                    recording_auto: Some(true),
                    recording_audio_days: Patch::Set(30),
                    recording_require_consent: Patch::Clear,
                },
            ),
            ("min", UpdateChannelRequest::default()),
            (
                "clear",
                UpdateChannelRequest {
                    topic: Patch::Clear,
                    category_id: Patch::Clear,
                    ..UpdateChannelRequest::default()
                },
            ),
        ]
    }
}

fn channel_member() -> ChannelMember {
    ChannelMember {
        channel_id: ID_CHANNEL,
        member_id: ID_BOB,
        joined_at: t(0),
    }
}

impl Example for ChannelMember {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", channel_member())]
    }
}

impl Example for Page<ChannelMember> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![channel_member()],
                has_more: false,
            },
        )]
    }
}

impl Example for AddChannelMembersRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            AddChannelMembersRequest {
                member_ids: vec![ID_BOB, ID_CAROL],
            },
        )]
    }
}

impl Example for RemoveChannelMemberRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", RemoveChannelMemberRequest { member_id: ID_BOB })]
    }
}

impl Example for OpenDmRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                OpenDmRequest {
                    member_ids: vec![ID_BOB],
                    usernames: vec![handle("carol")],
                },
            ),
            // SW12: by handle only.
            (
                "usernames",
                OpenDmRequest {
                    member_ids: vec![],
                    usernames: vec![handle("bob")],
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- api/resolve

impl Example for ResolveQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ResolveQuery {
                    addr: addr("acme-eng/#deploys/48831765147549701"),
                },
            ),
            (
                "dm",
                ResolveQuery {
                    addr: addr("dm/@rosson"),
                },
            ),
        ]
    }
}

impl Example for ResolveResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ResolveResponse {
                    space_id: Some(ID_SPACE),
                    channel_id: ID_CHANNEL,
                    thread_id: Some(ID_MESSAGE),
                },
            ),
            (
                "min",
                ResolveResponse {
                    space_id: None,
                    channel_id: ID_DM,
                    thread_id: None,
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- api/messages

fn thread_full() -> ThreadSummary {
    ThreadSummary {
        root_id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        reply_count: 3,
        last_reply_id: Some(ID_REPLY),
        last_reply_at: Some(t(60_000)),
        following: true,
        seq: Some(1_025),
    }
}

fn thread_min() -> ThreadSummary {
    ThreadSummary {
        root_id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        reply_count: 0,
        last_reply_id: None,
        last_reply_at: None,
        following: false,
        seq: None,
    }
}

fn message_full() -> Message {
    Message {
        id: ID_REPLY,
        channel_id: ID_CHANNEL,
        author_id: ID_CAROL,
        on_behalf_of: Some(ID_ME),
        body: "Deploy finished, <@48831765147549700> please verify.".into(),
        nonce: Some(Nonce::new("c1-7f3a")),
        thread_root_id: Some(ID_MESSAGE),
        also_in_channel: true,
        reference: Some(MessageReference {
            kind: ReferenceKind::Reply,
            message_id: ID_MESSAGE,
            channel_id: ID_CHANNEL,
        }),
        forwarded: Some(ForwardedMessage {
            message_id: ID_MESSAGE,
            channel_id: ID_OTHER_CHANNEL,
            author_id: ID_BOB,
            body: "Deploying 0.41.0 now.".into(),
            created_at: t(0),
        }),
        mentions: MentionTargets {
            members: vec![ID_BOB],
            roles: vec![ID_ROLE],
            groups: vec![ID_GROUP],
            channel: true,
            here: true,
        },
        reactions: vec![
            ReactionSummary {
                emoji: Emoji::Unicode {
                    value: "👍".into()
                },
                count: 2,
                me: true,
            },
            ReactionSummary {
                emoji: Emoji::Custom {
                    id: ID_EMOJI,
                    name: "party".into(),
                    animated: true,
                    url: Some(
                        "/api/v1/emoji-images/48831765147549709/full?exp=1791158400&sig=c2ln"
                            .into(),
                    ),
                    available: true,
                },
                count: 1,
                me: false,
            },
        ],
        attachments: vec![attachment_full()],
        pinned: true,
        thread: Some(thread_full()),
        seq: 1_024,
        created_at: t(60_000),
        edited_at: Some(t(120_000)),
        deleted_at: Some(t(180_000)),
        previews: vec![s16::link_preview_full(), s16::link_preview_min()],
        previews_suppressed: true,
        emojis: vec![s16::emoji_ref_full(), s16::emoji_ref_deleted()],
        poll: Some(s17::poll_full()),
        mentions_me: true,
        call: Some(s34::call_summary_full()),
        recording: Some(s35::recording_summary_full()),
        transcript: Some(s35::transcript_turn_full()),
    }
}

fn message_min() -> Message {
    Message {
        id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        author_id: ID_ME,
        on_behalf_of: None,
        body: "Deploying 0.41.0 now.".into(),
        nonce: None,
        thread_root_id: None,
        also_in_channel: false,
        reference: None,
        forwarded: None,
        mentions: MentionTargets::default(),
        reactions: vec![],
        attachments: vec![],
        pinned: false,
        thread: None,
        seq: 1_020,
        created_at: t(0),
        edited_at: None,
        deleted_at: None,
        previews: vec![],
        previews_suppressed: false,
        emojis: vec![],
        poll: None,
        mentions_me: false,
        call: None,
        recording: None,
        transcript: None,
    }
}

impl Example for Message {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", message_full()), ("min", message_min())]
    }
}

// ---------------------------------------------------------------- api/files (S15)

fn attachment_full() -> Attachment {
    Attachment {
        id: ID_ATTACHMENT,
        filename: "deploy.png".into(),
        content_type: "image/png".into(),
        size: 48_213,
        url: "/api/v1/files/48831765147549705?exp=1790514000&sig=c2ln".into(),
        thumbnail_url: Some("/api/v1/files/48831765147549705/thumb?exp=1790514000&sig=c2ln".into()),
        width: Some(1280),
        height: Some(720),
        kind: FileKind::Image,
        inline: true,
        animated: false,
        media_pending: false,
        preview_url: Some("/api/v1/files/48831765147549705/preview?exp=1790514000&sig=c2ln".into()),
        poster_url: None,
        url_expires_at: Some(t(3_600_000)),
    }
}

fn attachment_min() -> Attachment {
    Attachment {
        id: ID_ATTACHMENT,
        filename: "notes.txt".into(),
        content_type: "text/plain".into(),
        size: 0,
        url: "/api/v1/files/48831765147549705?exp=1790514000&sig=c2ln".into(),
        thumbnail_url: None,
        width: None,
        height: None,
        kind: FileKind::Document,
        inline: false,
        animated: false,
        media_pending: false,
        preview_url: None,
        poster_url: None,
        url_expires_at: None,
    }
}

fn attachment_video() -> Attachment {
    Attachment {
        id: ID_REPLY,
        filename: "demo.mp4".into(),
        content_type: "video/mp4".into(),
        size: 7_340_032,
        url: "/api/v1/files/48831765147549704?exp=1790514000&sig=c2ln".into(),
        thumbnail_url: Some("/api/v1/files/48831765147549704/thumb?exp=1790514000&sig=c2ln".into()),
        width: Some(640),
        height: Some(360),
        kind: FileKind::Video,
        inline: true,
        animated: false,
        media_pending: false,
        preview_url: None,
        poster_url: Some("/api/v1/files/48831765147549704/poster?exp=1790514000&sig=c2ln".into()),
        url_expires_at: Some(t(3_600_000)),
    }
}

const UPLOAD_URL: &str = "/api/v1/uploads/48831765147549705/data?exp=1790514000&sig=c2ln";

impl Example for CreateUploadRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateUploadRequest {
                    filename: "Q3 report.pdf".into(),
                    size: 4_194_304,
                    content_type: Some("application/pdf".into()),
                },
            ),
            (
                "min",
                CreateUploadRequest {
                    filename: "notes.txt".into(),
                    size: 0,
                    content_type: None,
                },
            ),
        ]
    }
}

impl Example for CreateUploadResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            CreateUploadResponse {
                upload_id: ID_ATTACHMENT,
                upload_url: UPLOAD_URL.into(),
                url_expires_at: t(3_600_000),
                chunk_size: 1_048_576,
                committed_bytes: 0,
                size: 4_194_304,
                complete: false,
                expires_at: t(86_400_000),
            },
        )]
    }
}

impl Example for UploadStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            UploadStatus {
                upload_id: ID_ATTACHMENT,
                committed_bytes: 3_145_728,
                size: 4_194_304,
                complete: false,
                expires_at: t(86_400_000),
            },
        )]
    }
}

impl Example for Upload {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = Upload {
            upload_id: ID_ATTACHMENT,
            committed_bytes: 1_048_576,
            size: 4_194_304,
            complete: false,
            expires_at: t(86_400_000),
            channel_id: ID_CHANNEL,
            filename: "Q3 report.pdf".into(),
            chunk_size: 1_048_576,
            upload_url: UPLOAD_URL.into(),
            url_expires_at: t(3_600_000),
            content_type: None,
            kind: None,
            width: None,
            height: None,
            media_pending: false,
            thumbnail_url: None,
        };
        let full = Upload {
            committed_bytes: 48_213,
            size: 48_213,
            complete: true,
            filename: "deploy.png".into(),
            content_type: Some("image/png".into()),
            kind: Some(FileKind::Image),
            width: Some(1280),
            height: Some(720),
            media_pending: true,
            thumbnail_url: Some(
                "/api/v1/files/48831765147549705/thumb?exp=1790514000&sig=c2ln".into(),
            ),
            ..min.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for RemoveAttachmentsRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RemoveAttachmentsRequest {
                attachment_ids: vec![ID_ATTACHMENT],
            },
        )]
    }
}

impl Example for FilesQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                FilesQuery {
                    before: Some(ID_ATTACHMENT),
                    after: None,
                    around: None,
                    limit: Some(50),
                    kind: Some(FileKind::Image),
                    from: Some(ID_BOB),
                },
            ),
            (
                "full_after",
                FilesQuery {
                    after: Some(ID_ATTACHMENT),
                    ..FilesQuery::default()
                },
            ),
            (
                "full_around",
                FilesQuery {
                    around: Some(ID_ATTACHMENT),
                    ..FilesQuery::default()
                },
            ),
            ("min", FilesQuery::default()),
        ]
    }
}

fn channel_file_full() -> ChannelFile {
    ChannelFile {
        attachment: attachment_full(),
        message_id: ID_REPLY,
        channel_id: ID_CHANNEL,
        thread_root_id: Some(ID_MESSAGE),
        author_id: ID_CAROL,
        created_at: t(60_000),
    }
}

impl Example for ChannelFile {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", channel_file_full()),
            (
                "min",
                ChannelFile {
                    attachment: attachment_min(),
                    message_id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    thread_root_id: None,
                    author_id: ID_BOB,
                    created_at: t(0),
                },
            ),
        ]
    }
}

impl Example for Page<ChannelFile> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![channel_file_full()],
                has_more: true,
            },
        )]
    }
}

impl Example for AttachmentsUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                AttachmentsUpdated {
                    channel_id: ID_CHANNEL,
                    message_id: ID_REPLY,
                    thread_root_id: Some(ID_MESSAGE),
                    attachments: vec![attachment_full(), attachment_video(), attachment_min()],
                    seq: 1_026,
                },
            ),
            (
                "min",
                AttachmentsUpdated {
                    channel_id: ID_CHANNEL,
                    message_id: ID_MESSAGE,
                    thread_root_id: None,
                    attachments: Vec::new(),
                    seq: 0,
                },
            ),
        ]
    }
}

impl Example for Page<Message> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![message_full(), message_min()],
                has_more: true,
            },
        )]
    }
}

fn message_version() -> MessageVersion {
    MessageVersion {
        id: Some(id(18)),
        message_id: ID_MESSAGE,
        version: 0,
        body: "Deploying 0.40.9 now.".into(),
        created_at: t(0),
        replaced_at: Some(t(30_000)),
    }
}

impl Example for MessageVersion {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", message_version())]
    }
}

impl Example for Page<MessageVersion> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![message_version()],
                has_more: false,
            },
        )]
    }
}

impl Example for ThreadSummary {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", thread_full()), ("min", thread_min())]
    }
}

impl Example for CreateMessageRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateMessageRequest {
                    body: "Looks good to me.".into(),
                    nonce: Some(Nonce::new("c1-7f3a")),
                    thread_root_id: Some(ID_MESSAGE),
                    also_send_to_channel: true,
                    reference: Some(ReferenceInput {
                        message_id: ID_MESSAGE,
                        kind: ReferenceKind::Quote,
                    }),
                    upload_ids: vec![ID_ATTACHMENT],
                    suppress_previews: true,
                    poll: Some(crate::api::extras::PollInput {
                        question: "Lunch?".into(),
                        options: vec!["Tacos".into(), "Pizza".into()],
                        max_choices: None,
                        anonymous: false,
                        results_visibility: None,
                        duration_s: Some(3_600),
                    }),
                    clear_draft: true,
                },
            ),
            (
                "min",
                CreateMessageRequest {
                    body: "Deploying 0.41.0 now.".into(),
                    nonce: None,
                    thread_root_id: None,
                    also_send_to_channel: false,
                    reference: None,
                    upload_ids: Vec::new(),
                    suppress_previews: false,
                    poll: None,
                    clear_draft: false,
                },
            ),
        ]
    }
}

impl Example for EditMessageRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            EditMessageRequest {
                body: "Deploying 0.41.1 now.".into(),
            },
        )]
    }
}

impl Example for ForwardMessageRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ForwardMessageRequest {
                    channel_id: ID_OTHER_CHANNEL,
                    thread_root_id: Some(ID_REPLY),
                    body: Some("FYI".into()),
                    nonce: Some(Nonce::new("fw-1")),
                    attachments: false,
                    suppress_previews: true,
                },
            ),
            (
                "min",
                ForwardMessageRequest {
                    channel_id: ID_OTHER_CHANNEL,
                    thread_root_id: None,
                    body: None,
                    nonce: None,
                    attachments: true,
                    suppress_previews: false,
                },
            ),
        ]
    }
}

impl Example for ReactionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ReactionRequest {
                    emoji: EmojiInput::Unicode {
                        value: "🚀".into()
                    },
                },
            ),
            (
                "custom",
                ReactionRequest {
                    emoji: EmojiInput::Custom {
                        id: ID_EMOJI,
                        name: "party".into(),
                    },
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- error

impl Example for ErrorBody {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ErrorBody::new(ApiError::new(
                    Code::MissingPermission,
                    "SEND_MESSAGES in #deploys",
                )),
            ),
            (
                "rate_limited",
                ErrorBody::new(ApiError::new(Code::RateLimited, "slow down").with_details(
                    RetryAfterDetails {
                        retry_after_ms: 1_500,
                        reason: None,
                    },
                )),
            ),
            (
                "validation_failed",
                ErrorBody::new(ApiError {
                    code: Code::ValidationFailed,
                    hint: "unknown field `zz`".into(),
                    details: obj(json!({"field": "zz"})),
                }),
            ),
            (
                "resync_required",
                ErrorBody::new(
                    ApiError::new(Code::ResyncRequired, "cursor is behind the floor")
                        .with_details(ResyncRequiredDetails::new(900)),
                ),
            ),
        ]
    }
}

impl Example for ValidationDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ValidationDetails {
                field: "reference.kind".into(),
            },
        )]
    }
}

impl Example for RetryAfterDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RetryAfterDetails {
                retry_after_ms: 1_500,
                reason: Some(RATE_REASON_SLOW_MODE.into()),
            },
        )]
    }
}

impl Example for ResyncRequiredDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("min", ResyncRequiredDetails::new(900)),
            (
                "full",
                ResyncRequiredDetails {
                    floor: 900,
                    head: Some(950),
                    reason: Some(ResyncReason::BehindFloor),
                    sync_epoch: Some("7713305120993321451".into()),
                },
            ),
        ]
    }
}

impl Example for ReasonDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ReasonDetails::with_field("username_taken", "username"),
            ),
            ("min", ReasonDetails::new("is_owner")),
        ]
    }
}

impl Example for PermissionDeniedDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PermissionDeniedDetails {
                    reason: PermissionReason::UnheldBits,
                    missing: Some(Perms::MANAGE_MESSAGES | Perms::TIMEOUT_MEMBERS),
                },
            ),
            (
                "min",
                PermissionDeniedDetails {
                    reason: PermissionReason::Hierarchy,
                    missing: None,
                },
            ),
        ]
    }
}

impl Example for PolicyDeniedDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PolicyDeniedDetails {
                policy: PolicyKey::GuestsInvite,
            },
        )]
    }
}

impl Example for InvalidStateDetails {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            InvalidStateDetails {
                reason: InvalidStateReason::ChannelArchived,
            },
        )]
    }
}

// ---------------------------------------------------------------- http

impl Example for PageQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                // Every field set is invalid (one cursor at most); the fixture shows the shape.
                PageQuery {
                    before: Some(ID_REPLY),
                    after: Some(ID_MESSAGE),
                    around: Some(ID_MESSAGE),
                    limit: Some(100),
                },
            ),
            ("min", PageQuery::default()),
            (
                "before",
                PageQuery {
                    before: Some(ID_REPLY),
                    limit: Some(50),
                    ..PageQuery::default()
                },
            ),
        ]
    }
}

impl Example for ChangesQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ChangesQuery {
                    since: 1_020,
                    limit: Some(500),
                },
            ),
            (
                "min",
                ChangesQuery {
                    since: 0,
                    limit: None,
                },
            ),
        ]
    }
}

impl Example for EmptyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", EmptyRequest {})]
    }
}

impl Example for OkResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", OkResponse::default())]
    }
}

// ---------------------------------------------------------------- gateway

impl Example for Frame {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ServerOp::Dispatch {
                    s: 42,
                    event: Event::MessageDeleted(MessageDeleted {
                        id: ID_REPLY,
                        channel_id: ID_CHANNEL,
                        thread_root_id: Some(ID_MESSAGE),
                        seq: 1_030,
                        tombstone: false,
                        whole_thread: false,
                        channel_last_message_id: None,
                    }),
                }
                .encode(),
            ),
            ("min", ClientOp::Heartbeat(None).encode()),
            ("hello", ServerOp::Hello(Hello::default()).encode()),
            (
                "dispatch_message_created",
                ServerOp::Dispatch {
                    s: 2,
                    event: Event::MessageCreated(Box::new(message_min())),
                }
                .encode(),
            ),
            ("heartbeat", ClientOp::Heartbeat(Some(41)).encode()),
            ("heartbeat_ack", ServerOp::HeartbeatAck.encode()),
            ("identify", ClientOp::Identify(identify_ticket()).encode()),
        ]
    }
}

impl Example for GatewayQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GatewayQuery {
                    v: 1,
                    encoding: Encoding::Json,
                    compress: Some(Compression::ZstdStream),
                },
            ),
            (
                "min",
                GatewayQuery {
                    v: 1,
                    encoding: Encoding::Json,
                    compress: None,
                },
            ),
        ]
    }
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "sew-web".into(),
        version: "0.1.0".into(),
    }
}

fn identify_ticket() -> IdentifyRequest {
    IdentifyRequest {
        auth: GatewayAuth::Ticket(ticket()),
        client: client_info(),
        capabilities: 0,
    }
}

impl Example for IdentifyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", identify_ticket()),
            (
                "full_token",
                IdentifyRequest {
                    auth: GatewayAuth::Token(session_token()),
                    client: client_info(),
                    capabilities: 0,
                },
            ),
        ]
    }
}

impl Example for ResumeRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ResumeRequest {
                    auth: GatewayAuth::Ticket(ticket()),
                    session_id: "gw_5f2b9c0e".into(),
                    seq: 41,
                },
            ),
            (
                "full_token",
                ResumeRequest {
                    auth: GatewayAuth::Token(session_token()),
                    session_id: "gw_5f2b9c0e".into(),
                    seq: 41,
                },
            ),
        ]
    }
}

impl Example for PresenceSetRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PresenceSetRequest {
                status: PresenceSetStatus::Dnd,
            },
        )]
    }
}

impl Example for SubscribeRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SubscribeRequest {
                    space_id: SubscribeScope::Space(ID_SPACE),
                    typing: vec![ID_CHANNEL],
                    presence: vec![ID_BOB, ID_CAROL],
                    member_list: Some(MemberListSub {
                        channel_id: ID_CHANNEL,
                        ranges: vec![[0, 99], [100, 199]],
                    }),
                },
            ),
            (
                "min",
                SubscribeRequest {
                    space_id: SubscribeScope::Dms,
                    typing: vec![],
                    presence: vec![],
                    member_list: None,
                },
            ),
        ]
    }
}

impl Example for TypingRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                TypingRequest {
                    channel_id: ID_CHANNEL,
                    thread_id: Some(ID_MESSAGE),
                },
            ),
            (
                "min",
                TypingRequest {
                    channel_id: ID_CHANNEL,
                    thread_id: None,
                },
            ),
        ]
    }
}

impl Example for ReauthRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReauthRequest {
                token: session_token(),
            },
        )]
    }
}

impl Example for ActivityRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", ActivityRequest { idle: true })]
    }
}

impl Example for Hello {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", Hello::default())]
    }
}

impl Example for Reconnect {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", Reconnect { after_ms: 7_312 })]
    }
}

impl Example for InvalidSession {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", InvalidSession { resumable: false })]
    }
}

impl Example for RateLimitedCloseReason {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RateLimitedCloseReason {
                retry_after_ms: 5_000,
            },
        )]
    }
}

fn ready_full() -> Ready {
    Ready {
        session_id: "gw_5f2b9c0e".into(),
        resume_url: "wss://chat.example.com/api/v1/gateway".into(),
        protocol: crate::PROTOCOL_VERSION,
        member: member_min(),
        install: ReadyInstall {
            id: ID_INSTALL,
            name: "Acme".into(),
            policy_digest: Policies::default().digest(),
            sync_epoch: "7713305120993321451".into(),
            profile_fields_version: Some("3".into()),
            voice_enabled: true,
            video_enabled: true,
            media: Some(s34::media_policy_full()),
        },
        spaces: vec![ReadySpace {
            id: ID_SPACE,
            slug: slug("acme-eng"),
            name: "Acme Engineering".into(),
            icon_url: Some("/api/v1/space-icons/48831765147549697/48831765147549716/512?exp=1791158400&sig=c2ln".into()),
            icon_thumb_url: Some("/api/v1/space-icons/48831765147549697/48831765147549716/64?exp=1791158400&sig=c2ln".into()),
            my_role_ids: vec![ID_ROLE],
            member_count: 42,
            versions: ReadyVersions {
                emoji: "e-3".into(),
                roles: "r7".into(),
                groups: "g-1".into(),
            },
            my_space_perms: perms::DEFAULT_EVERYONE,
            hidden: true,
        }],
        categories: vec![ReadyCategory {
            id: ID_CATEGORY,
            space_id: ID_SPACE,
            name: "Engineering".into(),
            position: 1,
        }],
        channels: vec![
            ReadyChannel {
                id: ID_CHANNEL,
                space_id: Some(ID_SPACE),
                category_id: Some(ID_CATEGORY),
                kind: ChannelType::Text,
                visibility: Visibility::Public,
                slug: Some(slug("deploys")),
                name: Some("deploys".into()),
                position: 4,
                head_seq: 1_024,
                last_message_id: Some(ID_MESSAGE),
                my_perms: perms::DEFAULT_EVERYONE,
                dm_member_ids: vec![],
                slow_mode_s: Some(30),
                retention_days: Some(90),
                file_retention_days: Some(30),
                has_guests: true,
                assistant_reader_ids: vec![],
                recording_auto: false,
                recording_audio_days: None,
                recording_require_consent: None,
            },
            ReadyChannel {
                id: ID_DM,
                space_id: None,
                category_id: None,
                kind: ChannelType::Dm,
                visibility: Visibility::Private,
                slug: None,
                name: None,
                position: 0,
                head_seq: 3,
                last_message_id: None,
                my_perms: Perms(0b1110_1111),
                dm_member_ids: vec![ID_ME, ID_BOB],
                slow_mode_s: None,
                retention_days: None,
                file_retention_days: None,
                has_guests: false,
                assistant_reader_ids: vec![ID_CAROL],
                recording_auto: false,
                recording_audio_days: None,
                recording_require_consent: None,
            },
            // S35 v2: an auto-record voice channel.
            ReadyChannel {
                id: ID_OTHER_CHANNEL,
                space_id: Some(ID_SPACE),
                category_id: Some(ID_CATEGORY),
                kind: ChannelType::Voice,
                visibility: Visibility::Public,
                slug: Some(slug("standup")),
                name: Some("standup".into()),
                position: 5,
                head_seq: 7,
                last_message_id: None,
                my_perms: perms::DEFAULT_EVERYONE,
                dm_member_ids: vec![],
                slow_mode_s: None,
                retention_days: None,
                file_retention_days: None,
                has_guests: false,
                assistant_reader_ids: vec![],
                recording_auto: true,
                recording_audio_days: Some(30),
                recording_require_consent: Some(true),
            },
        ],
        read_states: vec![s14::ready_read_state()],
        member_seq: 78,
        threads_inbox: s14::inbox_full(),
        presence_settings: Some(s18::presence_settings_full()),
        dm_members: vec![member_full()],
        notify: Some(s21::ready_notify_full()),
        call_keys: Some(s50::ready_call_keys_full()),
        voice_states: vec![s33::voice_state_full()],
        voice_states_partial: true,
        voice_channels: vec![s33::ready_voice_channel_full()],
        calls: vec![s34::call_full()],
        recordings: vec![s35::recording_full()],
    }
}

impl Example for Ready {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", ready_full())]
    }
}

impl Example for Resumed {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", Resumed {})]
    }
}

impl Example for MessageDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MessageDeleted {
                    id: ID_REPLY,
                    channel_id: ID_CHANNEL,
                    thread_root_id: Some(ID_MESSAGE),
                    seq: 1_031,
                    tombstone: true,
                    whole_thread: false,
                    channel_last_message_id: Some(ID_MESSAGE),
                },
            ),
            (
                "min",
                MessageDeleted {
                    id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    thread_root_id: None,
                    seq: 1_032,
                    tombstone: false,
                    whole_thread: false,
                    channel_last_message_id: None,
                },
            ),
        ]
    }
}

impl Example for ReactionEvent {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReactionEvent {
                message_id: ID_MESSAGE,
                channel_id: ID_CHANNEL,
                member_id: ID_BOB,
                emoji: Emoji::Unicode {
                    value: "👍".into()
                },
                count: 2,
                seq: 1_033,
            },
        )]
    }
}

impl Example for ChannelMemberEvent {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ChannelMemberEvent {
                    channel_id: ID_CHANNEL,
                    member_id: ID_BOB,
                    seq: Some(1_025),
                    channel: Some(Box::new(channel_full())),
                    person: Some(crate::api::meetings::MeetingPerson {
                        member_id: ID_BOB,
                        display_name: "Bob Builder".into(),
                        kind: crate::api::meetings::MeetingLobbyKind::Member,
                    }),
                },
            ),
            (
                "min",
                ChannelMemberEvent {
                    channel_id: ID_CHANNEL,
                    member_id: ID_BOB,
                    seq: None,
                    channel: None,
                    person: None,
                },
            ),
        ]
    }
}

impl Example for TypingStarted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                TypingStarted {
                    channel_id: ID_CHANNEL,
                    thread_id: Some(ID_MESSAGE),
                    member_id: ID_BOB,
                },
            ),
            (
                "min",
                TypingStarted {
                    channel_id: ID_CHANNEL,
                    thread_id: None,
                    member_id: ID_BOB,
                },
            ),
        ]
    }
}

impl Example for PresenceUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PresenceUpdated {
                member_id: ID_BOB,
                status: PresenceStatus::Idle,
            },
        )]
    }
}

impl Example for PermsChanged {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PermsChanged {
                    space_id: Some(ID_SPACE),
                    channels: vec![ChannelPerms {
                        channel_id: ID_CHANNEL,
                        my_perms: perms::DEFAULT_EVERYONE,
                    }],
                    my_space_perms: Some(perms::DEFAULT_EVERYONE),
                    full: true,
                },
            ),
            (
                "min",
                PermsChanged {
                    space_id: None,
                    channels: vec![],
                    my_space_perms: None,
                    full: false,
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- mgmt

impl Example for Health {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", Health { ok: true })]
    }
}

impl Example for Status {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = Status {
            version: "0.1.0".into(),
            schema_version: 90_001,
            install_id: ID_INSTALL,
            provider: ProviderKind::K2,
            db: StatusDb {
                ok: true,
                lag_ms: None,
                ping_ms: None,
            },
            jobs: StatusJobs {
                queued: 3,
                dead: 0,
                running: 0,
                oldest_due_age_ms: None,
            },
            search: StatusSearch::default(),
            keys: StatusKeys {
                provider_ok: true,
                current_month_ready: true,
                mirror_ok: false,
                mirror_age_ms: None,
                provider: None,
                kek_id: None,
                retiring_kek_ids: Vec::new(),
                rewrap_pending: 0,
                next_month_prepared: false,
                lazy_creates_this_month: 0,
                shred_pending: 0,
                resurrected_keys: 0,
                mirror: None,
            },
            sessions: 12,
            sockets: 9,
            started_at: None,
            uptime_ms: 0,
            alarms: Vec::new(),
            identity: None,
            client_listen: None,
            backup: None,
            drill: None,
            files: None,
            retention: None,
            mail: None,
            digest: None,
            exports: None,
            voice: None,
            recording: None,
        };
        let full = Status {
            db: StatusDb {
                ok: true,
                lag_ms: Some(0),
                ping_ms: Some(1),
            },
            jobs: StatusJobs {
                queued: 3,
                dead: 1,
                running: 1,
                oldest_due_age_ms: Some(120),
            },
            search: StatusSearch {
                lag_ms: Some(1_200),
                state: Some(SearchState::Ready),
                docs: Some(StatusSearchDocs {
                    messages: 1_203_311,
                    files: 88_410,
                }),
                index_bytes: Some(81_234_567),
                generation: Some(7),
                last_commit_at: Some(t(1_100)),
                rebuild: Some(StatusSearchRebuild {
                    reason: "admin".into(),
                    generation: 8,
                    progress_pct: 41,
                    docs: 493_000,
                    started_at: t(0),
                }),
            },
            keys: StatusKeys {
                provider_ok: true,
                current_month_ready: true,
                mirror_ok: true,
                mirror_age_ms: Some(60_000),
                provider: Some("systemd-credential".into()),
                kek_id: Some("1f2e3d4c5b6a7988".into()),
                retiring_kek_ids: vec!["0a1b2c3d4e5f6071".into()],
                rewrap_pending: 12,
                next_month_prepared: true,
                lazy_creates_this_month: 0,
                shred_pending: 0,
                resurrected_keys: 0,
                mirror: Some(StatusKeysMirror {
                    kind: "dir".into(),
                    stale: false,
                    pending: 1,
                    oldest_pending_age_s: Some(4),
                    last_ok_at: Some(t(60_000)),
                    unknown_keys: 0,
                }),
            },
            started_at: Some(t(0)),
            uptime_ms: 3_600_000,
            alarms: vec![StatusAlarm::BackupStale, StatusAlarm::JobsDead],
            identity: Some(StatusIdentity {
                provider: ProviderKind::K2,
                jwks_url: Some("http://127.0.0.1:7300/v1/chat/jwks".into()),
                jwks_ok: Some(true),
            }),
            client_listen: Some("http://127.0.0.1:8470".into()),
            backup: Some(StatusBackup {
                last_ok_at: Some(t(0)),
                last_id: Some("20260927T031512Z".into()),
                age_ms: Some(129_600_001),
                last_error: Some("disk_full".into()),
                last_kind: Some("scheduled".into()),
                last_error_at: Some(t(3_600_000)),
            }),
            drill: Some(StatusDrill {
                last_ok_at: Some(t(0)),
                last_ok: Some(true),
                last_error: Some("checksum_mismatch".into()),
                age_ms: Some(86_400_000),
            }),
            files: Some(StatusFiles {
                ffmpeg_available: true,
            }),
            retention: Some(crate::api::compliance::StatusRetention {
                last_run_at: Some(t(7_200_000)),
                last_run_state: Some(crate::api::compliance::RunState::Done),
                last_ok_at: Some(t(7_200_000)),
                overdue: false,
                pending_policies: 1,
                active_holds: 2,
                search_pending: 0,
                shred_candidates: 14,
            }),
            mail: Some(s21::status_mail_full()),
            digest: Some(StatusDigest {
                enabled: true,
                pending_jobs: 3,
            }),
            exports: Some(crate::api::exports::StatusExports {
                running: Some(ID_JOB),
                queued: 2,
                on_disk_bytes: 104_857_600,
                last_done_at: Some(t(3_600_000)),
                last_failed_at: Some(t(1_800_000)),
            }),
            ..min.clone()
        };
        let voice = Status {
            alarms: vec![StatusAlarm::E2eeRosterMismatch, StatusAlarm::VoiceMediaDown],
            voice: Some(s33::voice_status_full()),
            ..min.clone()
        };
        let recording = Status {
            alarms: vec![StatusAlarm::RecorderDown, StatusAlarm::TranscribeBacklog],
            recording: Some(s35::status_recording_full()),
            ..min.clone()
        };
        vec![
            ("full", full),
            ("full_voice", voice),
            ("full_recording", recording),
            ("min", min),
        ]
    }
}

fn provision_agent() -> ProvisionRequest {
    ProvisionRequest {
        external_id: ext(WORKSPACE),
        kind: MemberKind::Agent,
        agent_kind: Some(AgentKind::Assistant),
        assistant_of: Some(ext(PRINCIPAL)),
        guest: false,
        profile: ProfileInput {
            display_name: "Rosson's assistant".into(),
            username: handle("rosson-assistant"),
            email: Some("assistant@example.com".into()),
            title: Some("Assistant".into()),
            pronouns: Some("it/its".into()),
            custom_fields: vec![crate::api::people::MgmtCustomFieldInput {
                key: "cost_center".into(),
                value: Some("42".into()),
            }],
        },
    }
}

impl Example for ProvisionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = ProvisionRequest {
            external_id: ext(PRINCIPAL),
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            guest: false,
            profile: ProfileInput {
                display_name: "Rosson".into(),
                username: handle("rosson"),
                email: None,
                title: None,
                pronouns: None,
                custom_fields: vec![],
            },
        };
        // SW2 (contracts rev 134): a K2 Connect user of the server, a person and a member.
        let connect = ProvisionRequest {
            external_id: ext(CONNECT_USER),
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            guest: false,
            profile: ProfileInput {
                display_name: "Dana from Connect".into(),
                username: handle("dana"),
                email: None,
                title: None,
                pronouns: None,
                custom_fields: vec![],
            },
        };
        vec![
            ("full", provision_agent()),
            ("min", min),
            ("connect", connect),
        ]
    }
}

impl Example for ProvisionResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ProvisionResponse {
                member_id: ID_CAROL,
                created: true,
                reactivated: false,
                changed: false,
            },
        )]
    }
}

impl Example for UpdatePrincipalRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdatePrincipalRequest {
                    external_id: ext(PRINCIPAL),
                    profile: ProfilePatch {
                        display_name: Some("Rosson L.".into()),
                        username: Some(handle("rosson")),
                        email: Patch::Set("rosson@example.com".into()),
                        title: Patch::Clear,
                        pronouns: Patch::Set("he/him".into()),
                        custom_fields: vec![crate::api::people::MgmtCustomFieldInput {
                            key: "cost_center".into(),
                            value: None,
                        }],
                    },
                },
            ),
            (
                "min",
                UpdatePrincipalRequest {
                    external_id: ext(PRINCIPAL),
                    profile: ProfilePatch::default(),
                },
            ),
        ]
    }
}

impl Example for DeprovisionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DeprovisionRequest {
                external_id: ext(PRINCIPAL),
            },
        )]
    }
}

impl Example for DeprovisionResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DeprovisionResponse {
                member_id: ID_BOB,
                sessions_revoked: 2,
            },
        )]
    }
}

impl Example for RevokeSessionsRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RevokeSessionsRequest {
                    external_id: Some(ext(PRINCIPAL)),
                    k2_sid: Some("skn_4f1c2a".into()),
                },
            ),
            (
                "k2_sid",
                RevokeSessionsRequest {
                    external_id: None,
                    k2_sid: Some("own_0123456789abcdef0123456789abcdef".into()),
                },
            ),
        ]
    }
}

impl Example for RevokeSessionsResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", RevokeSessionsResponse { revoked: 2 })]
    }
}

impl Example for OwnerSetRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                OwnerSetRequest {
                    external_id: ext(PRINCIPAL),
                    approval: Some(OwnerApproval::OwnerPass { pass: PASS.into() }),
                    force: true,
                },
            ),
            (
                "min",
                OwnerSetRequest {
                    external_id: ExternalId::K2Owner,
                    approval: None,
                    force: false,
                },
            ),
            (
                "password",
                OwnerSetRequest {
                    external_id: ext("local:bob"),
                    approval: Some(OwnerApproval::OwnerPassword {
                        password: "correct horse battery staple".into(),
                    }),
                    force: false,
                },
            ),
        ]
    }
}

impl Example for PinRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PinRequest {
                agent: ext(WORKSPACE),
                person: ext(PRINCIPAL),
            },
        )]
    }
}

impl Example for DeliveriesQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DeliveriesQuery {
                    after: 77,
                    timeout_ms: Some(25_000),
                    limit: Some(100),
                },
            ),
            (
                "min",
                DeliveriesQuery {
                    after: 77,
                    timeout_ms: None,
                    limit: None,
                },
            ),
        ]
    }
}

impl Example for DeliveriesPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DeliveriesPage {
                entries: vec![
                    Delivery {
                        seq: 78,
                        at: t(60_000),
                        reason: NotificationReason::Mention,
                        message: DeliveryMessage {
                            id: ID_REPLY,
                            channel_id: ID_CHANNEL,
                            space_id: Some(ID_SPACE),
                            thread_root_id: Some(ID_MESSAGE),
                            author_id: ID_BOB,
                            author_display_name: "Bob".into(),
                            author_username: Some(handle("bob")),
                            author_kind: MemberKind::Person,
                            addr: addr("acme-eng/#deploys/48831765147549701"),
                            reply_addr: Some(addr("acme-eng/#deploys/48831765147549701")),
                            place: Some(DeliveryPlace::SpaceChannel),
                            kind: DeliveryKind::Text,
                            call: None,
                            recording: Some(s35::recording_summary_full()),
                            body: "@rosson-assistant can you check the logs?".into(),
                            body_truncated: false,
                            attachment_count: 1,
                            on_behalf_of: None,
                            on_behalf_of_display_name: None,
                            created_at: t(60_000),
                        },
                    },
                    Delivery {
                        seq: 79,
                        at: t(120_000),
                        reason: NotificationReason::Dm,
                        message: DeliveryMessage {
                            id: ID_MESSAGE,
                            channel_id: ID_DM,
                            space_id: None,
                            thread_root_id: None,
                            author_id: ID_BOB,
                            author_display_name: "Bob".into(),
                            author_username: Some(handle("bob")),
                            author_kind: MemberKind::Agent,
                            addr: addr("dm/id:48831765147549711"),
                            reply_addr: None,
                            place: Some(DeliveryPlace::PersonDm),
                            kind: DeliveryKind::CallRecord,
                            call: Some(DeliveryCall {
                                missed: false,
                                duration_ms: Some(90_000),
                            }),
                            recording: None,
                            body: "ping".into(),
                            body_truncated: true,
                            attachment_count: 0,
                            on_behalf_of: Some(ID_CAROL),
                            on_behalf_of_display_name: Some("Carol".into()),
                            created_at: t(120_000),
                        },
                    },
                ],
                head: 81,
                next_after: 80,
                has_more: true,
                sync_epoch: Some("7713305120993321451".into()),
            },
        )]
    }
}

impl Example for StartJobResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", StartJobResponse { job_id: ID_JOB })]
    }
}

impl Example for JobStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                JobStatus {
                    id: ID_JOB,
                    kind: "backup".into(),
                    state: JobState::Failed,
                    attempts: 2,
                    last_error: Some("disk_full".into()),
                    created_at: t(0),
                    finished_at: Some(t(90_000)),
                    result: Some(json!({"bundle_id": "20260927T031512Z", "bytes": 1_048_576})),
                },
            ),
            ("min", job_min()),
        ]
    }
}

fn job_min() -> JobStatus {
    JobStatus {
        id: ID_JOB,
        kind: "backup".into(),
        state: JobState::Queued,
        attempts: 0,
        last_error: None,
        created_at: t(0),
        finished_at: None,
        result: None,
    }
}

impl Example for JobsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                JobsQuery {
                    state: Some(JobState::Dead),
                    kind: Some("backup".into()),
                    before: Some(ID_JOB),
                    limit: Some(20),
                },
            ),
            ("min", JobsQuery::default()),
        ]
    }
}

impl Example for JobStatusPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![job_min()],
                has_more: true,
            },
        )]
    }
}

impl Example for OwnerSetResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                OwnerSetResponse {
                    owner_id: ID_CAROL,
                    previous_owner_id: Some(ID_ME),
                    changed: true,
                },
            ),
            (
                "min",
                OwnerSetResponse {
                    owner_id: ID_ME,
                    previous_owner_id: None,
                    changed: false,
                },
            ),
        ]
    }
}

impl Example for PinResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PinResponse {
                    agent_id: ID_CAROL,
                    person_id: Some(ID_ME),
                    changed: true,
                },
            ),
            (
                "min",
                PinResponse {
                    agent_id: ID_CAROL,
                    person_id: None,
                    changed: false,
                },
            ),
        ]
    }
}

impl Example for PrincipalsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PrincipalsQuery {
                    after: Some(ID_ME),
                    limit: Some(200),
                    kind: Some(MemberKind::Agent),
                    external_id: Some(ext(WORKSPACE)),
                    username: Some(handle("rosson-assistant")),
                    include_deactivated: false,
                    compliance: Some(true),
                    include: Some(PrincipalsInclude::LinkGuests),
                },
            ),
            (
                "min",
                PrincipalsQuery {
                    after: None,
                    limit: None,
                    kind: None,
                    external_id: None,
                    username: None,
                    include_deactivated: true,
                    compliance: None,
                    include: None,
                },
            ),
        ]
    }
}

impl Example for MgmtPrincipalPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![
                    MgmtPrincipal {
                        member_id: ID_CAROL,
                        external_id: ext(WORKSPACE),
                        kind: MemberKind::Agent,
                        agent_kind: Some(AgentKind::Assistant),
                        assistant_of: Some(ext(PRINCIPAL)),
                        install_role: InstallRole::Member,
                        username: handle("rosson-assistant"),
                        display_name: "Rosson's assistant".into(),
                        email: Some("assistant@example.com".into()),
                        title: Some("Assistant".into()),
                        pronouns: Some("it/its".into()),
                        deactivated: true,
                        created_at: t(0),
                        deactivated_at: Some(t(60_000)),
                    },
                    MgmtPrincipal {
                        member_id: ID_ME,
                        external_id: ext(PRINCIPAL),
                        kind: MemberKind::Person,
                        agent_kind: None,
                        assistant_of: None,
                        install_role: InstallRole::Owner,
                        username: handle("rosson"),
                        display_name: "Rosson".into(),
                        email: None,
                        title: None,
                        pronouns: None,
                        deactivated: false,
                        created_at: t(0),
                        deactivated_at: None,
                    },
                ],
                has_more: false,
            },
        )]
    }
}

// ---------------------------------------------------------------- mgmt/local

fn local_user_response() -> LocalUserResponse {
    LocalUserResponse {
        member_id: ID_ME,
        username: handle("rosson"),
        install_role: InstallRole::Owner,
        deactivated: false,
        created: true,
        reactivated: false,
        changed: true,
        setup_link: Some(SetupLink {
            url: format!(
                "https://chat.example.com/setup#token=sewpw_{}",
                "z".repeat(43)
            ),
            expires_at: t(86_400_000),
        }),
    }
}

impl Example for LocalUserCreateRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                LocalUserCreateRequest {
                    username: handle("bob"),
                    display_name: "Bob B".into(),
                    install_role: LocalRole::Admin,
                    email: Some("bob@example.com".into()),
                    setup_link: false,
                },
            ),
            (
                "min",
                LocalUserCreateRequest {
                    username: handle("bob"),
                    display_name: "Bob".into(),
                    install_role: LocalRole::Member,
                    email: None,
                    setup_link: true,
                },
            ),
        ]
    }
}

impl Example for SetupLinkRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetupLinkRequest {
                    username: handle("rosson"),
                    owner: true,
                    display_name: Some("Rosson".into()),
                },
            ),
            (
                "min",
                SetupLinkRequest {
                    username: handle("bob"),
                    owner: false,
                    display_name: None,
                },
            ),
        ]
    }
}

impl Example for LocalUserRef {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            LocalUserRef {
                username: handle("bob"),
            },
        )]
    }
}

impl Example for LocalRoleRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            LocalRoleRequest {
                username: handle("bob"),
                install_role: LocalSettableRole::Admin,
            },
        )]
    }
}

impl Example for LocalUserResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = LocalUserResponse {
            setup_link: None,
            created: false,
            changed: false,
            install_role: InstallRole::Member,
            username: handle("bob"),
            member_id: ID_BOB,
            ..local_user_response()
        };
        vec![("full", local_user_response()), ("min", min)]
    }
}

// ---------------------------------------------------------------- policy

impl Example for Policies {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = Policies {
            spaces_create: SpacesCreate::Members,
            channels_create_public_floor: ChannelCreateFloor::SpaceAdmins,
            channels_create_private_floor: ChannelCreateFloor::InstallAdmins,
            gifs_enabled: true,
            gifs_provider: Some("giphy".into()),
            gifs_api_key_ref: Some("secret:giphy".into()),
            meetings_enabled: true,
            meetings_privacy_contact: Some("privacy@example.com".into()),
            ..Policies::default()
        };
        vec![("full", full), ("min", Policies::default())]
    }
}

fn policy_patch_full() -> PolicyPatch {
    PolicyPatch {
        spaces_create: Some(SpacesCreate::Members),
        channels_create_public_floor: Some(ChannelCreateFloor::Perm),
        channels_create_private_floor: Some(ChannelCreateFloor::SpaceAdmins),
        channels_archive: Some(ArchiveWho::Managers),
        mentions_channel_max_members: Some(500),
        guests_invite: Some(GuestInvite::Perm),
        guests_dm_non_channel_mates: Some(true),
        messages_edit_window_s: Some(3_600),
        messages_delete_own: Some(false),
        gifs_enabled: Some(true),
        gifs_provider: Patch::Set("giphy".into()),
        gifs_api_key_ref: Patch::Set("secret:giphy".into()),
        link_previews_enabled: Some(false),
        files_max_bytes: Some(104_857_600),
        files_space_quota_bytes: Some(10_737_418_240),
        emoji_max_per_space: Some(1_000),
        retention_default_days: Some(365),
        private_space_admins_can_join: Some(true),
        presence_show_to_guests: Some(true),
        digest_enabled: Some(true),
        agents_allowed_in_dms: Some(false),
        files_strip_image_location: Some(false),
        messages_deleted_purge_days: Some(30),
        guests_enabled: Some(false),
        notifications_default_level: Some(crate::api::notify::NotifyLevel::All),
        digest_detail: Some(crate::api::notify::DigestDetail::Counts),
        calls_verification_code: Some(crate::e2ee::VerificationCode::Hidden),
        calls_dm_enabled: Some(false),
        calls_stats_beacon: Some(true),
        recording_enabled: Some(true),
        recording_dm_enabled: Some(false),
        recording_audio_default_days: Some(30),
        recording_audio_max_days: Some(365),
        recording_require_consent_to_join: Some(false),
        meetings_enabled: Some(true),
        meetings_create: Some(MeetingsCreate::Admins),
        meetings_max_participants: Some(100),
        meetings_max_duration_minutes: Some(240),
        meetings_grace_minutes: Some(10),
        meetings_link_guests: Some(false),
        meetings_privacy_contact: Patch::Set("privacy@example.com".into()),
    }
}

impl Example for PolicyPatch {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", policy_patch_full()),
            ("min", PolicyPatch::default()),
            (
                "clear_gifs",
                PolicyPatch {
                    gifs_enabled: Some(false),
                    gifs_provider: Patch::Clear,
                    gifs_api_key_ref: Patch::Clear,
                    ..PolicyPatch::default()
                },
            ),
        ]
    }
}

impl Example for PoliciesResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PoliciesResponse {
                policies: Policies::default(),
            },
        )]
    }
}

impl Example for SetPoliciesRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetPoliciesRequest {
                    changes: policy_patch_full(),
                },
            ),
            (
                "gifs_on",
                SetPoliciesRequest {
                    changes: PolicyPatch {
                        gifs_enabled: Some(true),
                        ..PolicyPatch::default()
                    },
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- audit

impl Example for AuditEvent {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = AuditEvent {
            id: ID_AUDIT,
            at: t(0),
            actor: Actor {
                member_id: Some(ID_CAROL),
                kind: ActorKind::Agent,
                via: Some(ActorVia {
                    system: "k2".into(),
                    agent: "7c9e6679-7425-40de-944b-e07fc1f90ae7".into(),
                    k2_session: "skn_4f1c2a".into(),
                }),
                on_behalf_of: Some(ID_ME),
                peer_uid: Some(1_001),
                ip: Some("127.0.0.1".into()),
            },
            action: Action::ChannelArchive,
            target: Some(AuditTarget {
                kind: "channel".into(),
                id: ID_CHANNEL,
            }),
            space_id: Some(ID_SPACE),
            channel_id: Some(ID_CHANNEL),
            result: AuditResult::Ok,
            details: obj(json!({"old": {"archived": false}, "new": {"archived": true}})),
        };
        let min = AuditEvent {
            id: ID_AUDIT,
            at: t(0),
            actor: Actor {
                member_id: None,
                kind: ActorKind::System,
                via: None,
                on_behalf_of: None,
                peer_uid: None,
                ip: None,
            },
            action: Action::LoginFail,
            target: None,
            space_id: None,
            channel_id: None,
            result: AuditResult::Denied,
            details: Map::new(),
        };
        vec![("full", full), ("min", min)]
    }
}

// ---------------------------------------------------------------- when

impl Example for When {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("at", When::At { at: t(3_600_000) }),
            (
                "local",
                When::Local {
                    date_time: "2026-10-01T09:00".into(),
                    timezone: Some("America/Los_Angeles".into()),
                },
            ),
            (
                "local_min",
                When::Local {
                    date_time: "2026-10-01T09:00".into(),
                    timezone: None,
                },
            ),
            ("in", When::In { ms: 1_800_000 }),
        ]
    }
}

// ---- SW5: install inputs as JSON

impl Example for crate::install::InstallInputs {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                crate::install::InstallInputs {
                    provider: Some("k2".into()),
                    k2_jwks_url: Some("http://127.0.0.1:7300/v1/chat/jwks".into()),
                    profile: Some("shared".into()),
                    pg_major: Some(16),
                    master_key_from: Some("/run/k2/sew/master.key".into()),
                    cred_mode: Some("encrypted".into()),
                    mgmt_users: vec!["k2".into()],
                    mgmt_allow_uids: Some(vec![988]),
                },
            ),
            ("min", crate::install::InstallInputs::default()),
        ]
    }
}

impl Example for crate::install::MediaInstallInputs {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                crate::install::MediaInstallInputs {
                    tarball: Some("/var/lib/k2/staging/sew/sew-media.tar.zst".into()),
                    public_url: Some("wss://voice.box.k2.dev".into()),
                    node_ip: Some("203.0.113.7".into()),
                    stun_servers: vec!["stun.example.net:3478".into()],
                    allow_private_node_ip: Some(false),
                    udp_port: Some(7882),
                    tcp_port: Some(7881),
                    bind_v4: Some("0.0.0.0".into()),
                    bind_v6: Some(String::new()),
                    turn_tls: Some("direct".into()),
                    turn_domain: Some("turn.box.k2.dev".into()),
                    turn_cert: Some("/var/lib/k2/staging/sew/turn-fullchain.pem".into()),
                    turn_key: Some("/var/lib/k2/staging/sew/turn-key.pem".into()),
                    turn_advertise_port: Some(5349),
                    turn_proxy_protocol: Some("v2".into()),
                    turn_plain: vec!["udp".into()],
                    profile: Some("shared".into()),
                    cred_mode: Some("encrypted".into()),
                    sysctl: Some("apply".into()),
                },
            ),
            ("min", crate::install::MediaInstallInputs::default()),
        ]
    }
}

// ---- SW11, SW13: spaces over mgmt

impl Example for MgmtSpaceCreateRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MgmtSpaceCreateRequest {
            slug: slug("hearth-lobby"),
            name: "Hearth lobby".into(),
            owner: ExternalId::K2Owner,
            managers: vec![ext(WORKSPACE), ext(PRINCIPAL)],
            app_id: Some("hearth".into()),
            hidden: true,
        };
        let min = MgmtSpaceCreateRequest {
            slug: slug("ops"),
            name: "Ops".into(),
            owner: ext(PRINCIPAL),
            managers: vec![],
            app_id: None,
            hidden: false,
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MgmtSpaceResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MgmtSpaceResponse {
            space_id: ID_SPACE,
            slug: slug("hearth-lobby"),
            name: "Hearth lobby".into(),
            owner_member_id: ID_ME,
            app_id: Some("hearth".into()),
            created: true,
            manager_role_id: Some(ID_ROLE),
            manager_member_ids: vec![ID_BOB, ID_CAROL],
            hidden: true,
        };
        let min = MgmtSpaceResponse {
            app_id: None,
            created: false,
            manager_role_id: None,
            manager_member_ids: vec![],
            hidden: false,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MgmtSpaceBindRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MgmtSpaceBindRequest {
                space_id: ID_SPACE,
                app_id: "hearth".into(),
            },
        )]
    }
}

impl Example for MgmtSpaceMembersAddRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MgmtSpaceMembersAddRequest {
            space_id: ID_SPACE,
            app_id: Some("hearth".into()),
            external_ids: vec![ext(PRINCIPAL), ext(CONNECT_USER)],
        };
        let min = MgmtSpaceMembersAddRequest {
            app_id: None,
            external_ids: vec![ext(PRINCIPAL)],
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MgmtSpaceMembersAddResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MgmtSpaceMembersAddResponse {
                added: vec![MgmtSpaceMemberRef {
                    external_id: ext(PRINCIPAL),
                    member_id: ID_BOB,
                }],
                already: vec![MgmtSpaceMemberRef {
                    external_id: ExternalId::K2Owner,
                    member_id: ID_ME,
                }],
                skipped: vec![ID_CAROL],
                not_added: vec![MgmtSpaceNotAdded {
                    external_id: ext(CONNECT_USER),
                    reason: MgmtSpaceNotAddedReason::NotProvisioned,
                }],
            },
        )]
    }
}

impl Example for MgmtSpaceMemberRemoveRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MgmtSpaceMemberRemoveRequest {
            space_id: ID_SPACE,
            app_id: Some("hearth".into()),
            external_id: ext(PRINCIPAL),
        };
        let min = MgmtSpaceMemberRemoveRequest {
            app_id: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MgmtSpaceMemberRemoveResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", MgmtSpaceMemberRemoveResponse { removed: true })]
    }
}

// ---- SW15: backups over mgmt

impl Example for MgmtBackupList {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MgmtBackupList {
                items: vec![MgmtBackup {
                    id: "20260927T031512Z".into(),
                    kind: "manual".into(),
                    created_at: t(0),
                    bytes: 52_428_800,
                    schema_version: 920_001,
                    sew_version: "0.1.0".into(),
                }],
            },
        )]
    }
}

impl Example for BackupJobResult {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = BackupJobResult {
            id: "20260927T031512Z".into(),
            bytes: 52_428_800,
            duration_ms: 4_210,
            linked: 1_200,
            copied: 3,
            pruned: vec!["20260920T031502Z".into()],
        };
        let min = BackupJobResult {
            pruned: vec![],
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}
