//! Examples for the wire types S22 adds (prd-sew-22 §16.3).

use super::{ID_BOB, ID_CHANNEL, ID_DM, ID_JOB, ID_ME, ID_MESSAGE, ID_SPACE, id, t};
use crate::api::channels::{ChannelType, Visibility};
use crate::api::compliance::*;
use crate::http::{Page, Patch};
use crate::registry::Example;

const ID_POLICY: crate::id::Id = id(51);
const ID_PREVIEW: crate::id::Id = id(52);
const ID_RUN: crate::id::Id = id(53);
const ID_HOLD: crate::id::Id = id(54);
const ID_FILE: crate::id::Id = id(55);

impl Example for RetentionScope {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", RetentionScope::Channel { id: ID_CHANNEL }),
            ("min", RetentionScope::Install {}),
            ("space", RetentionScope::Space { id: ID_SPACE }),
        ]
    }
}

impl Example for RetentionValues {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RetentionValues {
                    messages_days: Some(90),
                    files_days: Some(30),
                    deleted_purge_days: Some(30),
                },
            ),
            ("min", RetentionValues::default()),
        ]
    }
}

fn policy_full() -> RetentionPolicy {
    RetentionPolicy {
        id: ID_POLICY,
        scope: RetentionScope::Channel { id: ID_CHANNEL },
        messages_days: Some(90),
        files_days: Some(30),
        deleted_purge_days: None,
        state: PolicyState::Pending,
        effective_at: t(604_800_000),
        preview_id: Some(ID_PREVIEW),
        created_by: Some(ID_ME),
        created_via: CreatedVia::Member,
        created_at: t(0),
        ended_at: Some(t(700_000_000)),
    }
}

fn policy_min() -> RetentionPolicy {
    RetentionPolicy {
        id: ID_POLICY,
        scope: RetentionScope::Install {},
        messages_days: None,
        files_days: None,
        deleted_purge_days: Some(30),
        state: PolicyState::Active,
        effective_at: t(0),
        preview_id: None,
        created_by: None,
        created_via: CreatedVia::System,
        created_at: t(0),
        ended_at: None,
    }
}

impl Example for RetentionPolicy {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", policy_full()), ("min", policy_min())]
    }
}

impl Example for Page<RetentionPolicy> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![policy_full(), policy_min()],
                has_more: false,
            },
        )]
    }
}

impl Example for EffectiveRetention {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                EffectiveRetention {
                    channel_id: ID_CHANNEL,
                    messages_days: Some(20),
                    files_days: Some(7),
                    deleted_purge_days: Some(30),
                    messages_from: RetentionScope::Install {},
                    files_from: RetentionScope::Channel { id: ID_CHANNEL },
                    clamped: true,
                },
            ),
            (
                "min",
                EffectiveRetention {
                    channel_id: ID_DM,
                    messages_days: None,
                    files_days: None,
                    deleted_purge_days: None,
                    messages_from: RetentionScope::Install {},
                    files_from: RetentionScope::Install {},
                    clamped: false,
                },
            ),
        ]
    }
}

impl Example for EffectiveRetentionQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            EffectiveRetentionQuery {
                channel_id: ID_CHANNEL,
            },
        )]
    }
}

impl Example for RetentionListQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RetentionListQuery {
                    scope_kind: Some("channel".into()),
                    scope_id: Some(ID_CHANNEL),
                    history: Some(true),
                    before: Some(ID_POLICY),
                    limit: Some(20),
                },
            ),
            ("min", RetentionListQuery::default()),
        ]
    }
}

impl Example for PreviewRetentionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PreviewRetentionRequest {
                    scope: RetentionScope::Install {},
                    messages_days: Some(30),
                    files_days: Some(7),
                    deleted_purge_days: Some(14),
                    effective_at: Some(t(604_800_000)),
                },
            ),
            (
                "min",
                PreviewRetentionRequest {
                    scope: RetentionScope::Channel { id: ID_CHANNEL },
                    messages_days: Some(90),
                    files_days: None,
                    deleted_purge_days: None,
                    effective_at: None,
                },
            ),
        ]
    }
}

impl Example for SetRetentionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetRetentionRequest {
                    scope: Some(RetentionScope::Install {}),
                    messages_days: Some(3_650),
                    files_days: Some(365),
                    deleted_purge_days: Some(30),
                    ..SetRetentionRequest::default()
                },
            ),
            (
                "full_preview",
                SetRetentionRequest {
                    preview_id: Some(ID_PREVIEW),
                    ..SetRetentionRequest::default()
                },
            ),
            (
                "min",
                SetRetentionRequest {
                    scope: Some(RetentionScope::Space { id: ID_SPACE }),
                    ..SetRetentionRequest::default()
                },
            ),
        ]
    }
}

fn totals_full() -> RetentionTotals {
    RetentionTotals {
        messages_expired: 1_200,
        messages_purged: 1_150,
        messages_held: 50,
        revisions_purged: 40,
        files_removed: 12,
        files_purged: 30,
        files_held: 2,
        file_bytes_purged: 52_428_800,
        drafts_deleted: 3,
        scheduled_deleted: 1,
    }
}

impl Example for RetentionTotals {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", totals_full()), ("min", RetentionTotals::default())]
    }
}

fn counts_full() -> ChannelCounts {
    ChannelCounts {
        channel_id: ID_CHANNEL,
        messages_expired: 1_200,
        messages_purged: 1_150,
        messages_held: 50,
        files_removed: 12,
        files_purged: 30,
        files_held: 2,
    }
}

impl Example for ChannelCounts {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", counts_full())]
    }
}

impl Example for RetentionPreview {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = RetentionPreview {
            id: ID_PREVIEW,
            job_id: ID_JOB,
            state: RunState::Done,
            scope: RetentionScope::Install {},
            proposal: RetentionValues {
                messages_days: Some(30),
                files_days: Some(7),
                deleted_purge_days: None,
            },
            as_of: t(604_800_000),
            totals: Some(totals_full()),
            per_channel: vec![counts_full()],
            clamped_scopes: vec![
                RetentionScope::Space { id: ID_SPACE },
                RetentionScope::Channel { id: ID_CHANNEL },
            ],
            created_at: t(0),
            expires_at: t(86_400_000),
        };
        let min = RetentionPreview {
            state: RunState::Running,
            totals: None,
            per_channel: Vec::new(),
            clamped_scopes: Vec::new(),
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

fn run_full() -> RetentionRun {
    RetentionRun {
        id: ID_RUN,
        trigger: RunTrigger::PolicyEffective,
        as_of: t(604_800_000),
        state: RunState::Done,
        phase: "report".into(),
        totals: totals_full(),
        search_docs_deleted: 1_150,
        rows_resealed: 4,
        links_rewrapped: 2,
        shreds_enqueued: 3,
        started_at: t(604_800_000),
        finished_at: Some(t(604_860_000)),
    }
}

impl Example for RetentionRun {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = RetentionRun {
            trigger: RunTrigger::Schedule,
            state: RunState::Running,
            phase: "channels".into(),
            totals: RetentionTotals::default(),
            search_docs_deleted: 0,
            rows_resealed: 0,
            links_rewrapped: 0,
            shreds_enqueued: 0,
            finished_at: None,
            ..run_full()
        };
        vec![("full", run_full()), ("min", min)]
    }
}

impl Example for Page<RetentionRun> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![run_full()],
                has_more: true,
            },
        )]
    }
}

impl Example for RunRetentionResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", RunRetentionResponse { job_id: ID_JOB })]
    }
}

impl Example for RunsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RunsQuery {
                    before: Some(ID_RUN),
                    limit: Some(10),
                },
            ),
            ("min", RunsQuery::default()),
        ]
    }
}

fn hold_full() -> LegalHold {
    LegalHold {
        id: ID_HOLD,
        name: "Matter 2026-014".into(),
        members: vec![ID_BOB],
        channels: vec![ID_CHANNEL],
        spaces: vec![ID_SPACE],
        from: Some(t(0)),
        to: Some(t(2_592_000_000)),
        created_by: Some(ID_ME),
        created_at: t(0),
        updated_at: t(60_000),
        released_at: Some(t(86_400_000)),
        released_by: Some(ID_ME),
        coverage_channel_count: 4,
    }
}

impl Example for LegalHold {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = LegalHold {
            members: Vec::new(),
            spaces: Vec::new(),
            from: None,
            to: None,
            created_by: None,
            updated_at: t(0),
            released_at: None,
            released_by: None,
            coverage_channel_count: 1,
            ..hold_full()
        };
        vec![("full", hold_full()), ("min", min)]
    }
}

impl Example for Page<LegalHold> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![hold_full()],
                has_more: false,
            },
        )]
    }
}

impl Example for CreateHoldRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateHoldRequest {
                    name: "Matter 2026-014".into(),
                    members: vec![ID_BOB],
                    channels: vec![ID_CHANNEL],
                    spaces: vec![ID_SPACE],
                    from: Some(t(0)),
                    to: Some(t(2_592_000_000)),
                },
            ),
            (
                "min",
                CreateHoldRequest {
                    name: "Matter 2026-015".into(),
                    members: Vec::new(),
                    channels: vec![ID_CHANNEL],
                    spaces: Vec::new(),
                    from: None,
                    to: None,
                },
            ),
        ]
    }
}

impl Example for UpdateHoldRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateHoldRequest {
                    name: Some("Matter 2026-014 (b)".into()),
                    add_members: vec![ID_ME],
                    add_channels: vec![ID_DM],
                    add_spaces: vec![ID_SPACE],
                    from: Patch::Clear,
                    to: Patch::Set(t(5_184_000_000)),
                },
            ),
            ("min", UpdateHoldRequest::default()),
        ]
    }
}

impl Example for ReleaseHoldRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReleaseHoldRequest {
                confirm_name: "Matter 2026-014".into(),
            },
        )]
    }
}

impl Example for HoldsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                HoldsQuery {
                    state: Some(HoldStateFilter::Active),
                    before: Some(ID_HOLD),
                    limit: Some(25),
                },
            ),
            ("min", HoldsQuery::default()),
        ]
    }
}

fn compliance_channel_full() -> ComplianceChannel {
    ComplianceChannel {
        id: ID_CHANNEL,
        space_id: Some(ID_SPACE),
        kind: ChannelType::Text,
        visibility: Visibility::Private,
        slug: Some("deploys".into()),
        name: Some("deploys".into()),
        dm_member_ids: Vec::new(),
        archived: true,
    }
}

impl Example for ComplianceChannel {
    fn examples() -> Vec<(&'static str, Self)> {
        let min = ComplianceChannel {
            id: ID_DM,
            space_id: None,
            kind: ChannelType::Dm,
            visibility: Visibility::Private,
            slug: None,
            name: None,
            dm_member_ids: vec![ID_ME, ID_BOB],
            archived: false,
        };
        vec![("full", compliance_channel_full()), ("min", min)]
    }
}

impl Example for Page<ComplianceChannel> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![compliance_channel_full()],
                has_more: true,
            },
        )]
    }
}

impl Example for ComplianceChannelsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ComplianceChannelsQuery {
                    space_id: Some(ID_SPACE),
                    kind: Some(ChannelType::Text),
                    before: Some(ID_CHANNEL),
                    limit: Some(100),
                },
            ),
            ("min", ComplianceChannelsQuery::default()),
        ]
    }
}

impl Example for ContentVanished {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ContentVanished {
                    channel_id: ID_CHANNEL,
                    reason: VanishReason::Retention,
                    message_ids: vec![ID_MESSAGE, id(56)],
                    tombstone_ids: vec![ID_MESSAGE],
                    attachments: vec![VanishedAttachment {
                        message_id: id(57),
                        attachment_id: ID_FILE,
                    }],
                    seq: 4_410,
                },
            ),
            (
                "min",
                ContentVanished {
                    channel_id: ID_CHANNEL,
                    reason: VanishReason::Retention,
                    message_ids: Vec::new(),
                    tombstone_ids: Vec::new(),
                    attachments: Vec::new(),
                    seq: 1,
                },
            ),
        ]
    }
}
