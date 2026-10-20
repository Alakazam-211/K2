//! Examples for the wire types S23 adds (prd-sew-23 §8, §14.3): the compliance flag, the audit
//! API, exports and the format-1 archive records.

use std::collections::BTreeMap;

use serde_json::json;

use super::{
    ID_ATTACHMENT, ID_AUDIT, ID_BOB, ID_CAROL, ID_CATEGORY, ID_CHANNEL, ID_DM, ID_EMOJI, ID_GROUP,
    ID_INSTALL, ID_JOB, ID_ME, ID_MESSAGE, ID_REPLY, ID_ROLE, ID_SPACE, id, t,
};
use crate::api::audit::*;
use crate::api::channels::{ChannelType, Visibility};
use crate::api::exports::*;
use crate::api::files::FileKind;
use crate::api::members::{AgentKind, InstallRole, MemberKind};
use crate::api::messages::ReferenceKind;
use crate::audit::{AuditEvent, AuditResult};
use crate::export::*;
use crate::http::Page;
use crate::mgmt::{AuditVerifyMode, AuditVerifyReport, PrincipalComplianceRequest};
use crate::registry::Example;

const ID_HOLD: crate::id::Id = id(81);
const ID_FIELD: crate::id::Id = id(82);
const ID_REVISION: crate::id::Id = id(83);
const ID_OPTION: crate::id::Id = id(84);
const ID_BOOKMARK: crate::id::Id = id(85);
const ID_SAVED: crate::id::Id = id(86);
const ID_REMINDER: crate::id::Id = id(87);
const ID_DRAFT: crate::id::Id = id(88);
const ID_SCHEDULED: crate::id::Id = id(89);
const ID_AVATAR: crate::id::Id = id(90);
const ID_ICON: crate::id::Id = id(91);
const ID_ICON_LINK: crate::id::Id = id(92);
const ID_EMOJI_LINK: crate::id::Id = id(93);
const ID_SOURCE_ATTACHMENT: crate::id::Id = id(94);

const RECIPIENT: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
const PRINCIPAL: &str = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
const HEX: &str = "3f0c1a2b3f0c1a2b3f0c1a2b3f0c1a2b3f0c1a2b3f0c1a2b3f0c1a2b3f0c1a2b";
const HEX2: &str = "9a1b00ff9a1b00ff9a1b00ff9a1b00ff9a1b00ff9a1b00ff9a1b00ff9a1b00ff";

fn scope_full() -> ArchiveScope {
    ArchiveScope {
        label: Some("Q3 review".into()),
        members: vec![ID_ME],
        channels: vec![ID_CHANNEL],
        spaces: vec![ID_SPACE],
        holds: vec![ID_HOLD],
        from: Some(t(0)),
        to: Some(t(86_400_000)),
        member_mode: MemberMode::Authored,
        keyword_count: 2,
    }
}

fn scope_min() -> ArchiveScope {
    ArchiveScope {
        label: None,
        members: Vec::new(),
        channels: vec![ID_CHANNEL],
        spaces: Vec::new(),
        holds: Vec::new(),
        from: None,
        to: None,
        member_mode: MemberMode::Conversations,
        keyword_count: 0,
    }
}

fn options_full() -> ArchiveOptions {
    ArchiveOptions {
        files: FileMode::ManifestOnly,
        views: true,
        audit: true,
    }
}

fn counts() -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    m.insert("members".into(), 212);
    m.insert("messages".into(), 1_843);
    m.insert("files".into(), 44);
    m
}

impl Example for CreateEdiscoveryRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateEdiscoveryRequest {
                    label: Some("Q3 review".into()),
                    members: vec![ID_ME],
                    channels: vec![ID_CHANNEL],
                    spaces: vec![ID_SPACE],
                    holds: vec![ID_HOLD],
                    from: Some(t(0)),
                    to: Some(t(86_400_000)),
                    member_mode: MemberMode::Authored,
                    keywords: vec!["invoice".into(), "Ünïcode".into()],
                    files: FileMode::ManifestOnly,
                    views: false,
                    audit: false,
                    recipients: vec![RECIPIENT.into()],
                },
            ),
            (
                "min",
                CreateEdiscoveryRequest {
                    label: None,
                    members: vec![ID_ME],
                    channels: Vec::new(),
                    spaces: Vec::new(),
                    holds: Vec::new(),
                    from: None,
                    to: None,
                    member_mode: MemberMode::Conversations,
                    keywords: Vec::new(),
                    files: FileMode::Include,
                    views: true,
                    audit: true,
                    recipients: vec![RECIPIENT.into()],
                },
            ),
        ]
    }
}

fn export_full() -> Export {
    Export {
        id: ID_JOB,
        kind: ExportKind::Ediscovery,
        label: Some("Q3 review".into()),
        state: ExportState::Done,
        requested_by: Some(ID_CAROL),
        requested_via: RequestedVia::Member,
        scope: Some(scope_full()),
        options: options_full(),
        recipients: vec![RECIPIENT.into()],
        progress: ExportProgress {
            phase: Some("finish".into()),
            channels_done: 3,
            channels_total: 3,
            messages: 1_843,
            files: 44,
            bytes_written: 9_812_331,
        },
        archive_bytes: Some(9_812_331),
        archive_sha256: Some(HEX.into()),
        root_hash: Some(HEX2.into()),
        counts: Some(counts()),
        error: Some(ExportError {
            code: "storage_full".into(),
            reason: "storage_full".into(),
        }),
        created_at: t(0),
        started_at: Some(t(1_000)),
        finished_at: Some(t(61_000)),
        expires_at: Some(t(604_861_000)),
    }
}

fn export_min() -> Export {
    Export {
        id: ID_JOB,
        kind: ExportKind::Full,
        label: None,
        state: ExportState::Queued,
        requested_by: None,
        requested_via: RequestedVia::Service,
        scope: None,
        options: ArchiveOptions {
            files: FileMode::Include,
            views: false,
            audit: true,
        },
        recipients: vec![RECIPIENT.into()],
        progress: ExportProgress::default(),
        archive_bytes: None,
        archive_sha256: None,
        root_hash: None,
        counts: None,
        error: None,
        created_at: t(0),
        started_at: None,
        finished_at: None,
        expires_at: None,
    }
}

impl Example for Export {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", export_full()), ("min", export_min())]
    }
}

impl Example for ExportPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![export_full(), export_min()],
                has_more: true,
            },
        )]
    }
}

impl Example for ExportsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ExportsQuery {
                    kind: Some(ExportKind::Ediscovery),
                    state: Some(ExportState::Done),
                    before: Some(ID_JOB),
                    limit: Some(20),
                },
            ),
            ("min", ExportsQuery::default()),
        ]
    }
}

impl Example for ExportEstimate {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ExportEstimate {
                channels: 3,
                messages: 1_843,
                attachments: 44,
                file_bytes: 20_931_000,
                keywords_applied: false,
            },
        )]
    }
}

impl Example for SignedLink {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SignedLink {
                url: format!(
                    "/api/v1/compliance/exports/{ID_JOB}/archive?m={ID_CAROL}&exp=1790514000&sig=Zm9v"
                ),
                expires_at: t(3_600_000),
            },
        )]
    }
}

impl Example for SetComplianceRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", SetComplianceRequest { compliance: true })]
    }
}

impl Example for ComplianceFlagResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ComplianceFlagResponse {
                member_id: ID_CAROL,
                compliance: true,
                changed: true,
            },
        )]
    }
}

impl Example for ComplianceOfficers {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ComplianceOfficers {
                items: vec![
                    ComplianceOfficer {
                        member_id: ID_CAROL,
                        set_at: Some(t(0)),
                        set_by: Some(ID_ME),
                    },
                    ComplianceOfficer {
                        member_id: ID_BOB,
                        set_at: None,
                        set_by: None,
                    },
                ],
            },
        )]
    }
}

impl Example for PrincipalComplianceRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PrincipalComplianceRequest {
                external_id: match PRINCIPAL.parse() {
                    Ok(v) => v,
                    Err(e) => panic!("example external id: {e}"),
                },
                compliance: false,
            },
        )]
    }
}

impl Example for AuditQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                AuditQuery {
                    space_id: Some(ID_SPACE),
                    channel_id: Some(ID_CHANNEL),
                    actor_id: Some(ID_ME),
                    target_kind: Some("channel".into()),
                    target_id: Some(ID_CHANNEL),
                    action: vec!["channel.archive".into(), "channel.update".into()],
                    action_prefix: Some("channel.".into()),
                    result: Some(AuditResult::Ok),
                    from: Some(t(0)),
                    to: Some(t(86_400_000)),
                    before: Some(ID_AUDIT),
                    after: None,
                    limit: Some(100),
                },
            ),
            (
                "full_after",
                AuditQuery {
                    after: Some(ID_AUDIT),
                    ..AuditQuery::default()
                },
            ),
            ("min", AuditQuery::default()),
        ]
    }
}

fn entry(event: AuditEvent) -> AuditEntry {
    AuditEntry {
        chain_seq: 1_204,
        hash: HEX.into(),
        prev_hash: HEX2.into(),
        event,
    }
}

fn events() -> Vec<AuditEvent> {
    <AuditEvent as Example>::examples()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

impl Example for AuditEntry {
    fn examples() -> Vec<(&'static str, Self)> {
        let mut ev = events().into_iter();
        let full = ev.next().map(entry);
        let min = ev.next().map(entry);
        full.into_iter()
            .map(|e| ("full", e))
            .chain(min.into_iter().map(|e| ("min", e)))
            .collect()
    }
}

impl Example for AuditEntryPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: events().into_iter().map(entry).collect(),
                has_more: true,
            },
        )]
    }
}

impl Example for AuditChainStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                AuditChainStatus {
                    head_seq: 1_204,
                    head_hash: HEX.into(),
                    verified_seq: 1_200,
                    verified_at: Some(t(0)),
                    broken_at_seq: Some(812),
                    outbox_len: 3,
                    last_verify: Some(LastVerify {
                        job_id: ID_JOB,
                        mode: AuditVerifyMode::Full,
                        finished_at: t(60_000),
                        report: AuditVerifyReport {
                            mode: AuditVerifyMode::Full,
                            checked: 811,
                            head_seq: 811,
                            head_hash: HEX.into(),
                            broken_at: Some(812),
                            reason: Some("hash mismatch at chain_seq 812".into()),
                        },
                    }),
                },
            ),
            (
                "min",
                AuditChainStatus {
                    head_seq: 0,
                    head_hash: String::new(),
                    verified_seq: 0,
                    verified_at: None,
                    broken_at_seq: None,
                    outbox_len: 0,
                    last_verify: None,
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------- archive

impl Example for Manifest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = Manifest {
            export_format: 1,
            format_minor: 0,
            kind: ExportKind::Ediscovery,
            producer: Producer {
                name: "sew".into(),
                version: "0.1.0".into(),
                schema_version: 230_001,
            },
            export_id: ID_JOB,
            install_id: Some(ID_INSTALL),
            install_name: "Acme".into(),
            identity_provider: "k2".into(),
            source_sync_epoch: "7741029384756".into(),
            created_at: t(0),
            snapshot_at: t(412),
            options: options_full(),
            scope: Some(scope_full()),
            entries: vec![
                "members.ndjson".into(),
                "channels.ndjson".into(),
                "messages/*/*.ndjson".into(),
            ],
            excluded: EXCLUDED.iter().map(|s| (*s).to_owned()).collect(),
            recipients: vec![RECIPIENT.into()],
        };
        let min = Manifest {
            kind: ExportKind::Full,
            install_id: None,
            install_name: String::new(),
            options: ArchiveOptions {
                files: FileMode::Include,
                views: false,
                audit: true,
            },
            scope: None,
            ..full.clone()
        };
        let scoped = Manifest {
            scope: Some(scope_min()),
            ..full.clone()
        };
        vec![("full", full), ("min", min), ("scope_min", scoped)]
    }
}

impl Example for Trailer {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Trailer {
                entries: vec![
                    TrailerEntry {
                        path: "manifest.json".into(),
                        bytes: 912,
                        sha256: HEX.into(),
                    },
                    TrailerEntry {
                        path: "members.ndjson".into(),
                        bytes: 81_331,
                        sha256: HEX2.into(),
                    },
                ],
                counts: counts(),
                root_hash: HEX.into(),
            },
        )]
    }
}

fn source() -> Option<SourceRef> {
    Some(SourceRef {
        system: "slack".into(),
        reference: "C024BE91L/1503435956.000247".into(),
    })
}

impl Example for InstallRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let mut policies = BTreeMap::new();
        policies.insert("gifs.enabled".into(), json!(true));
        policies.insert("messages.edit_window_s".into(), json!(900));
        vec![
            (
                "full",
                InstallRecord {
                    install_id: ID_INSTALL,
                    name: "Acme".into(),
                    created_at: t(0),
                    policies,
                    id_hwm_ms: 1_790_510_400_123,
                },
            ),
            (
                "min",
                InstallRecord {
                    install_id: ID_INSTALL,
                    name: String::new(),
                    created_at: t(0),
                    policies: BTreeMap::new(),
                    id_hwm_ms: 0,
                },
            ),
        ]
    }
}

impl Example for ProfileFieldRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ProfileFieldRecord {
            id: ID_FIELD,
            key: "team".into(),
            label: "Team".into(),
            kind: "select".into(),
            options: Some(json!(["Sales", "Eng"])),
            managed_by: "provider".into(),
            visibility: "non_guests".into(),
            position: 1,
            created_at: t(0),
            updated_at: t(1_000),
            deleted_at: Some(t(2_000)),
            source: source(),
        };
        let min = ProfileFieldRecord {
            kind: "text".into(),
            options: None,
            managed_by: "member".into(),
            visibility: "everyone".into(),
            deleted_at: None,
            source: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MemberRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let mut fields = BTreeMap::new();
        fields.insert(ID_FIELD.to_string(), "Eng".to_owned());
        let full = MemberRecord {
            id: ID_BOB,
            external_id: "k2:workspace:7c9e6679-7425-40de-944b-e07fc1f90ae7".into(),
            kind: MemberKind::Agent,
            agent_kind: Some(AgentKind::Assistant),
            assistant_of: Some(ID_ME),
            pinned_at: Some(t(1_000)),
            install_role: InstallRole::Member,
            compliance: true,
            compliance_set_at: Some(t(2_000)),
            compliance_set_by: Some(ID_ME),
            username: "rosson-assistant".into(),
            display_name: "Rosson's assistant".into(),
            email: Some("bot@example.com".into()),
            title: Some("Assistant".into()),
            pronouns: Some("it/its".into()),
            timezone: Some("Europe/Berlin".into()),
            fields,
            status: Some(StatusRecord {
                text: Some("On leave".into()),
                emoji: Some("🌴".into()),
                expires_at: Some(t(86_400_000)),
                set_at: t(3_000),
            }),
            avatar: Some(AvatarRecord {
                file: ID_AVATAR,
                avatar_id: id(95),
                source: "member".into(),
            }),
            created_at: t(0),
            deactivated_at: Some(t(90_000)),
            head_seq: Some(42),
            source: source(),
        };
        let min = MemberRecord {
            id: ID_ME,
            external_id: PRINCIPAL.into(),
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            pinned_at: None,
            install_role: InstallRole::Owner,
            compliance: false,
            compliance_set_at: None,
            compliance_set_by: None,
            username: "rosson".into(),
            display_name: "Rosson".into(),
            email: None,
            title: None,
            pronouns: None,
            timezone: None,
            fields: BTreeMap::new(),
            status: None,
            avatar: None,
            created_at: t(0),
            deactivated_at: None,
            head_seq: None,
            source: None,
        };
        let status_min = MemberRecord {
            status: Some(StatusRecord {
                text: None,
                emoji: Some("🌴".into()),
                expires_at: None,
                set_at: t(3_000),
            }),
            ..min.clone()
        };
        vec![("full", full), ("min", min), ("status_min", status_min)]
    }
}

impl Example for PresenceSettingsRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = PresenceSettingsRecord {
            member_id: ID_ME,
            manual: "dnd".into(),
            manual_until: Some(t(3_600_000)),
            dnd_schedule: Some(json!({"timezone": "Europe/Berlin", "rules": []})),
            dnd_skip_until: Some(t(7_200_000)),
            updated_at: t(0),
        };
        let min = PresenceSettingsRecord {
            manual: "none".into(),
            manual_until: None,
            dnd_schedule: None,
            dnd_skip_until: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for SpaceRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = SpaceRecord {
            id: ID_SPACE,
            slug: "acme".into(),
            name: "Acme".into(),
            owner_id: ID_ME,
            created_by: Some(ID_ME),
            icon: Some(IconRecord {
                file: ID_ICON_LINK,
                link_id: ID_ICON_LINK,
                icon_id: ID_ICON,
            }),
            created_at: t(0),
            join_policy: Some("open".into()),
            default_channel_ids: vec![ID_CHANNEL],
            description: Some("Acme's home.".into()),
            rules_channel_id: Some(ID_CHANNEL),
            welcome_channel_id: Some(ID_CHANNEL),
            app_id: Some("hearth".into()),
            hidden: true,
            source: source(),
        };
        let min = SpaceRecord {
            created_by: None,
            icon: None,
            join_policy: None,
            default_channel_ids: vec![],
            description: None,
            rules_channel_id: None,
            welcome_channel_id: None,
            app_id: None,
            hidden: false,
            source: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for RoleRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = RoleRecord {
            id: ID_ROLE,
            space_id: ID_SPACE,
            name: "Moderators".into(),
            position: 2,
            perms: "3072".into(),
            color: Some(0x00ff_8800),
            mentionable: true,
            hoist: true,
            created_at: t(0),
            updated_at: t(1_000),
        };
        let min = RoleRecord {
            id: ID_SPACE,
            name: "@everyone".into(),
            position: 0,
            color: None,
            mentionable: false,
            hoist: false,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for SpaceMemberRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = SpaceMemberRecord {
            space_id: ID_SPACE,
            member_id: ID_BOB,
            joined_at: t(0),
            added_by: Some(ID_ME),
            role_ids: vec![ID_ROLE],
            timeout_until: Some(t(600_000)),
        };
        let min = SpaceMemberRecord {
            added_by: None,
            role_ids: Vec::new(),
            timeout_until: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for UserGroupRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = UserGroupRecord {
            id: ID_GROUP,
            space_id: Some(ID_SPACE),
            handle: "ops".into(),
            name: "Ops".into(),
            description: Some("On call".into()),
            mentionable: true,
            member_ids: vec![ID_ME, ID_BOB],
            created_by: Some(ID_ME),
            created_at: t(0),
            updated_at: t(1_000),
            deleted_at: Some(t(2_000)),
        };
        let min = UserGroupRecord {
            space_id: None,
            description: None,
            mentionable: false,
            member_ids: Vec::new(),
            created_by: None,
            deleted_at: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

fn overwrites() -> Vec<OverwriteRecord> {
    vec![
        OverwriteRecord {
            target_kind: "role".into(),
            target_id: ID_SPACE,
            allow: "0".into(),
            deny: "4".into(),
        },
        OverwriteRecord {
            target_kind: "member".into(),
            target_id: ID_BOB,
            allow: "4".into(),
            deny: "0".into(),
        },
    ]
}

impl Example for CategoryRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = CategoryRecord {
            id: ID_CATEGORY,
            space_id: ID_SPACE,
            name: "Projects".into(),
            position: 1,
            overwrites: overwrites(),
            created_at: t(0),
        };
        let min = CategoryRecord {
            overwrites: Vec::new(),
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ChannelRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ChannelRecord {
            id: ID_CHANNEL,
            space_id: Some(ID_SPACE),
            category_id: Some(ID_CATEGORY),
            kind: ChannelType::Text,
            visibility: Visibility::Private,
            slug: Some("legal".into()),
            name: Some("legal".into()),
            topic: Some("Contracts".into()),
            purpose: Some("Legal review".into()),
            position: 3,
            synced: true,
            overwrites: overwrites(),
            slow_mode_s: Some(30),
            dm_member_ids: vec![ID_ME, ID_BOB],
            created_by: Some(ID_ME),
            created_at: t(0),
            archived_at: Some(t(5_000)),
            archived_by: Some(ID_ME),
            head_seq: Some(812),
            recording_auto: true,
            recording_audio_days: Some(30),
            recording_require_consent: Some(true),
            source: source(),
        };
        let min = ChannelRecord {
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
            overwrites: Vec::new(),
            slow_mode_s: None,
            dm_member_ids: vec![ID_ME, ID_BOB],
            created_by: None,
            created_at: t(0),
            archived_at: None,
            archived_by: None,
            head_seq: None,
            recording_auto: false,
            recording_audio_days: None,
            recording_require_consent: None,
            source: None,
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ChannelMemberRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ChannelMemberRecord {
            channel_id: ID_CHANNEL,
            member_id: ID_BOB,
            joined_at: t(0),
            added_by: Some(ID_ME),
        };
        let min = ChannelMemberRecord {
            added_by: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ChannelPastMemberRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ChannelPastMemberRecord {
                channel_id: ID_CHANNEL,
                member_id: ID_BOB,
                left_at: t(9_000),
            },
        )]
    }
}

impl Example for EmojiRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = EmojiRecord {
            id: ID_EMOJI,
            space_id: ID_SPACE,
            name: "partyparrot".into(),
            aliases: vec!["parrot".into()],
            animated: true,
            width: 128,
            height: 128,
            file: Some(ID_EMOJI_LINK),
            link_id: Some(ID_EMOJI_LINK),
            created_by: Some(ID_ME),
            created_at: t(0),
            updated_at: t(1_000),
            deleted_at: Some(t(2_000)),
            deleted_by: Some(ID_ME),
        };
        let min = EmojiRecord {
            aliases: Vec::new(),
            animated: false,
            file: None,
            link_id: None,
            created_by: None,
            deleted_at: None,
            deleted_by: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

fn mentions_full() -> MentionsRecord {
    MentionsRecord {
        members: vec![ID_BOB],
        roles: vec![ID_ROLE],
        groups: vec![ID_GROUP],
        channel: true,
        here: true,
    }
}

fn message_full() -> MessageRecord {
    MessageRecord {
        id: ID_REPLY,
        channel_id: ID_CHANNEL,
        author_id: ID_BOB,
        on_behalf_of: Some(ID_ME),
        thread_root_id: Some(ID_MESSAGE),
        also_in_channel: true,
        reference: Some(ReferenceRecord {
            kind: ReferenceKind::Forward,
            message_id: ID_MESSAGE,
            channel_id: ID_CHANNEL,
            forward_author_id: Some(ID_ME),
            forward_created_at: Some(t(0)),
            forward_body: Some("the original text".into()),
            forward_unreadable: true,
        }),
        body: Some("see <@48831765147549700> CANARY".into()),
        version: 3,
        seq: Some(812),
        mentions: Some(mentions_full()),
        custom_emoji_ids: vec![ID_EMOJI],
        previews_suppressed: true,
        created_at: t(0),
        edited_at: Some(t(60_000)),
        deleted: Some(DeletedRecord {
            at: t(120_000),
            by: Some(ID_ME),
            kind: DeleteKind::Moderator,
        }),
        purged_at: Some(t(180_000)),
        unreadable: true,
        pin: Some(PinRecord {
            by: ID_ME,
            at: t(30_000),
        }),
        reactions: vec![ReactionRecord {
            emoji: "u:👍".into(),
            member_ids: vec![ID_ME, ID_BOB],
            at: vec![t(1_000), t(2_000)],
        }],
        attachments: vec![AttachmentRecord {
            id: ID_ATTACHMENT,
            position: 0,
            uploader_id: ID_BOB,
            filename: Some("q3.pdf".into()),
            unreadable: true,
            content_type: "application/pdf".into(),
            kind: FileKind::Document,
            inline: false,
            size: 20_931,
            width: Some(800),
            height: Some(600),
            animated: true,
            source_attachment_id: Some(ID_SOURCE_ATTACHMENT),
            created_at: t(0),
            removed: Some(RemovedRecord {
                at: t(90_000),
                by: Some(ID_ME),
                kind: RemoveKind::Moderator,
            }),
            purged_at: Some(t(180_000)),
            file: Some(ID_ATTACHMENT),
        }],
        poll: Some(PollRecord {
            question: Some("Lunch?".into()),
            unreadable: true,
            options: vec![
                PollOptionRecord {
                    id: ID_OPTION,
                    index: 0,
                    text: Some("Pizza".into()),
                    voter_ids: vec![ID_ME],
                    voted_at: vec![t(5_000)],
                    count: 1,
                },
                PollOptionRecord {
                    id: id(96),
                    index: 1,
                    text: None,
                    voter_ids: Vec::new(),
                    voted_at: Vec::new(),
                    count: 0,
                },
            ],
            max_choices: 1,
            anonymous: true,
            results_visibility: "after_close".into(),
            closes_at: Some(t(3_600_000)),
            closed_at: Some(t(3_600_000)),
            closed_by: Some(ID_ME),
            created_at: t(0),
        }),
        context: true,
        matched: true,
        source: source(),
    }
}

impl Example for MessageRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = MessageRecord {
            id: ID_MESSAGE,
            channel_id: ID_CHANNEL,
            author_id: ID_ME,
            on_behalf_of: None,
            thread_root_id: None,
            also_in_channel: false,
            reference: None,
            body: Some("hello".into()),
            version: 0,
            seq: None,
            mentions: None,
            custom_emoji_ids: Vec::new(),
            previews_suppressed: false,
            created_at: t(0),
            edited_at: None,
            deleted: None,
            purged_at: None,
            unreadable: false,
            pin: None,
            reactions: Vec::new(),
            attachments: Vec::new(),
            poll: None,
            context: false,
            matched: false,
            source: None,
        };
        let quote = MessageRecord {
            reference: Some(ReferenceRecord {
                kind: ReferenceKind::Quote,
                message_id: ID_REPLY,
                channel_id: ID_CHANNEL,
                forward_author_id: None,
                forward_created_at: None,
                forward_body: None,
                forward_unreadable: false,
            }),
            attachments: vec![AttachmentRecord {
                id: ID_ATTACHMENT,
                position: 0,
                uploader_id: ID_ME,
                filename: Some("a.png".into()),
                unreadable: false,
                content_type: "image/png".into(),
                kind: FileKind::Image,
                inline: true,
                size: 812,
                width: None,
                height: None,
                animated: false,
                source_attachment_id: None,
                created_at: t(0),
                removed: Some(RemovedRecord {
                    at: t(9_000),
                    by: None,
                    kind: RemoveKind::Retention,
                }),
                purged_at: None,
                file: None,
            }],
            deleted: Some(DeletedRecord {
                at: t(10_000),
                by: None,
                kind: DeleteKind::Retention,
            }),
            ..min.clone()
        };
        vec![("full", message_full()), ("min", min), ("quote", quote)]
    }
}

impl Example for RevisionRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = RevisionRecord {
            id: ID_REVISION,
            message_id: ID_MESSAGE,
            channel_id: ID_CHANNEL,
            version: 1,
            body: Some("first try".into()),
            mentions: Some(mentions_full()),
            created_at: t(0),
            replaced_at: t(60_000),
            purged: true,
            unreadable: true,
        };
        let min = RevisionRecord {
            body: None,
            mentions: None,
            purged: false,
            unreadable: false,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for BookmarkRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = BookmarkRecord {
            id: ID_BOOKMARK,
            channel_id: ID_CHANNEL,
            position: 0,
            data: Some(json!({"title": "Runbook", "url": "https://example.com/runbook"})),
            created_by: ID_ME,
            created_at: t(0),
            updated_at: t(1_000),
        };
        let min = BookmarkRecord {
            data: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ThreadFollowRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ThreadFollowRecord {
                member_id: ID_BOB,
                root_id: ID_MESSAGE,
                channel_id: ID_CHANNEL,
                state: "following".into(),
                reason: "replied".into(),
                followed_at: t(0),
            },
        )]
    }
}

impl Example for ReadStateRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ReadStateRecord {
            member_id: ID_BOB,
            channel_id: ID_CHANNEL,
            last_read_id: Some(ID_MESSAGE),
            updated_at: t(0),
        };
        let min = ReadStateRecord {
            last_read_id: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ThreadReadRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ThreadReadRecord {
                member_id: ID_BOB,
                root_id: ID_MESSAGE,
                channel_id: ID_CHANNEL,
                last_read_id: ID_REPLY,
                updated_at: t(0),
            },
        )]
    }
}

impl Example for MentionRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MentionRecord {
            member_id: ID_BOB,
            channel_id: ID_CHANNEL,
            message_id: ID_REPLY,
            root_id: Some(ID_MESSAGE),
            reason: 1,
            created_at: t(0),
        };
        let min = MentionRecord {
            root_id: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for SavedItemRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = SavedItemRecord {
            id: ID_SAVED,
            member_id: ID_BOB,
            message_id: ID_MESSAGE,
            channel_id: ID_CHANNEL,
            state: "active".into(),
            note: Some("follow up".into()),
            created_at: t(0),
            updated_at: t(1_000),
            state_changed_at: t(0),
        };
        let min = SavedItemRecord {
            note: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ReminderRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ReminderRecord {
            id: ID_REMINDER,
            member_id: ID_BOB,
            message_id: Some(ID_MESSAGE),
            channel_id: Some(ID_CHANNEL),
            note: Some("call back".into()),
            at: t(3_600_000),
            timezone: Some("Europe/Berlin".into()),
            state: "done".into(),
            fired_at: Some(t(3_600_000)),
            done_at: Some(t(3_700_000)),
            snooze_count: 1,
            version: 3,
            created_at: t(0),
            updated_at: t(3_700_000),
        };
        let min = ReminderRecord {
            message_id: None,
            channel_id: None,
            timezone: None,
            state: "pending".into(),
            fired_at: None,
            done_at: None,
            snooze_count: 0,
            version: 1,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for DraftRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = DraftRecord {
            id: ID_DRAFT,
            member_id: ID_BOB,
            channel_id: ID_CHANNEL,
            thread_root_id: Some(ID_MESSAGE),
            also_send_to_channel: true,
            body: "half written".into(),
            rev: 4,
            created_at: t(0),
            updated_at: t(1_000),
        };
        let min = DraftRecord {
            thread_root_id: None,
            also_send_to_channel: false,
            rev: 1,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for ScheduledRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = ScheduledRecord {
            id: ID_SCHEDULED,
            member_id: ID_BOB,
            channel_id: ID_CHANNEL,
            thread_root_id: Some(ID_MESSAGE),
            also_send_to_channel: true,
            body: Some("good morning".into()),
            send_at: t(86_400_000),
            timezone: Some("Europe/Berlin".into()),
            state: "failed".into(),
            fail_reason: Some(4),
            failed_at: Some(t(86_400_000)),
            version: 2,
            created_at: t(0),
            updated_at: t(1_000),
        };
        let min = ScheduledRecord {
            thread_root_id: None,
            also_send_to_channel: false,
            timezone: None,
            state: "pending".into(),
            fail_reason: None,
            failed_at: None,
            version: 1,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for NotifyPrefsRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = NotifyPrefsRecord {
            member_id: ID_ME,
            settings: Some(NotifySettingsRecord {
                default_level: Some("mentions".into()),
                thread_replies: false,
                broad_mentions: false,
                mute_lets_mentions: true,
                digest_interval: "1d".into(),
                keywords: vec!["deploy".into(), "outage".into()],
                rev: 3,
                updated_at: t(1_000),
            }),
            channel_prefs: vec![
                NotifyChannelPrefRecord {
                    channel_id: ID_CHANNEL,
                    level: Some("all".into()),
                    muted: false,
                    muted_until: None,
                    updated_at: t(2_000),
                },
                NotifyChannelPrefRecord {
                    channel_id: ID_DM,
                    level: None,
                    muted: true,
                    muted_until: Some(t(86_400_000)),
                    updated_at: t(3_000),
                },
            ],
            space_prefs: vec![NotifySpacePrefRecord {
                space_id: ID_SPACE,
                level: "none".into(),
                updated_at: t(4_000),
            }],
        };
        let min = NotifyPrefsRecord {
            member_id: ID_BOB,
            settings: None,
            channel_prefs: Vec::new(),
            space_prefs: vec![NotifySpacePrefRecord {
                space_id: ID_SPACE,
                level: "all".into(),
                updated_at: t(0),
            }],
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for AuditRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        let json = r#"{"action":"login.fail","actor":{"kind":"system"},"at":"2026-09-27T12:00:00.123Z","details":{},"id":"48831765147549708","result":"denied"}"#;
        vec![
            (
                "full",
                AuditRecord {
                    chain_seq: Some(1),
                    prev_hash: Some(HEX2.into()),
                    hash: Some(HEX.into()),
                    pending: true,
                    event_json: json.into(),
                },
            ),
            (
                "min",
                AuditRecord {
                    chain_seq: None,
                    prev_hash: None,
                    hash: None,
                    pending: true,
                    event_json: json.into(),
                },
            ),
        ]
    }
}

impl Example for RecordingRecord {
    fn examples() -> Vec<(&'static str, Self)> {
        use crate::recording::{
            AudioState, Capture, Consent, ConsentSurface, ConsentVia, GapReason, NotRecordedReason,
            RecordingState, StopReason, TranscriptState,
        };
        let rec = id(120);
        let full = RecordingRecord {
            id: rec,
            channel_id: ID_CHANNEL,
            call_record_id: Some(id(121)),
            message_id: ID_MESSAGE,
            started_by: ID_ME,
            started_at: t(0),
            auto: true,
            state: RecordingState::Stopped,
            stopped_at: Some(t(3_600_000)),
            stop_reason: Some(StopReason::Stopped),
            recorder_joined_at: Some(t(900)),
            recorder_runs: 2,
            language: Some("en".into()),
            mixed: true,
            audio_keep_days: 30,
            audio_state: AudioState::Available,
            audio_expires_at: Some(t(2_595_600_000)),
            audio_bytes: 2_406_400,
            transcript_state: TranscriptState::Partial,
            transcript_final_at: Some(t(3_700_000)),
            posted_through_ms: 3_590_000,
            turn_count: 1,
            model: Some("small-q8_0".into()),
            backend: Some("candle".into()),
            hidden: true,
            purged_at: Some(t(4_000_000)),
            content_key: Some("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8".into()),
            participants: vec![RecordingParticipantRecord {
                member_id: ID_BOB,
                consent: Consent::Granted,
                via: Some(ConsentVia::Prompt),
                capture: Some(Capture::NotRecorded),
                capture_reason: Some(NotRecordedReason::NoKey),
                first_at: t(1_000),
                answered_at: Some(t(2_000)),
                updated_at: t(3_000),
                in_room: true,
                ever_granted: true,
                notice_version: Some(7),
                surface: Some(ConsentSurface::Modal),
            }],
            runs: vec![RecordingRunRecord {
                run: 1,
                started_at: t(0),
                joined_at: Some(t(900)),
                ended_at: Some(t(60_000)),
                end_reason: Some(RunEnd::Reconnected),
            }],
            tracks: vec![RecordingTrackRecord {
                member_id: ID_ME,
                through_ms: 3_590_000,
                open: true,
                last_progress_at: Some(t(3_590_000)),
            }],
            gaps: vec![RecordingGapRecord {
                member_id: Some(ID_BOB),
                from_ms: 60_000,
                to_ms: 61_500,
                reason: GapReason::RecorderReconnected,
            }],
            segments: vec![RecordingSegmentRecord {
                id: id(122),
                member_id: ID_ME,
                run: 1,
                join_id: "0123456789abcdef0123456789abcdef".into(),
                seq: 1,
                start_ms: 1_500,
                duration_ms: 61_000,
                speech_ms: 52_000,
                level_dbov: -21.5,
                file: Some(id(123)),
                bytes: 240_640,
                plain_bytes: 240_560,
                state: SegmentState::Posted,
                attempts: 1,
                error: Some("audio_gone".into()),
                lang: Some("en".into()),
                lang_prob: Some(0.5),
                posted_through_ms: 61_000,
                created_at: t(63_000),
            }],
            mixes: vec![RecordingMixRecord {
                part: 1,
                offset_ms: 0,
                duration_ms: 3_600_000,
                file: Some(id(124)),
                bytes: 14_400_000,
            }],
            turns: vec![TranscriptTurnRecord {
                message_id: ID_REPLY,
                speaker_id: ID_ME,
                start_ms: 1_500,
                end_ms: 4_000,
                spoken_at: t(1_500),
                lang: Some("en".into()),
                pieces: json!([{"start_ms": 1500, "end_ms": 4000, "offset": 0, "len": 5}]),
            }],
        };
        let min = RecordingRecord {
            id: rec,
            channel_id: ID_DM,
            call_record_id: None,
            message_id: ID_MESSAGE,
            started_by: ID_ME,
            started_at: t(0),
            auto: false,
            state: RecordingState::Stopped,
            stopped_at: None,
            stop_reason: None,
            recorder_joined_at: None,
            recorder_runs: 0,
            language: None,
            mixed: false,
            audio_keep_days: 0,
            audio_state: AudioState::Purged,
            audio_expires_at: None,
            audio_bytes: 0,
            transcript_state: TranscriptState::Done,
            transcript_final_at: None,
            posted_through_ms: 0,
            turn_count: 0,
            model: None,
            backend: None,
            hidden: false,
            purged_at: None,
            content_key: None,
            participants: Vec::new(),
            runs: Vec::new(),
            tracks: Vec::new(),
            gaps: Vec::new(),
            segments: Vec::new(),
            mixes: Vec::new(),
            turns: Vec::new(),
        };
        vec![("full", full), ("min", min)]
    }
}
