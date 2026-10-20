//! Examples for the wire types S18 adds (prd-sew-18 §14).

use super::{ID_BOB, ID_CAROL, ID_CHANNEL, ID_GROUP, ID_ME, ID_ROLE, ID_SPACE, handle, id, t};
use super::{ext, member_full, member_min};
use crate::api::members::MemberKind;
use crate::api::people::*;
use crate::gateway::events::PresenceStatus;
use crate::http::{Page, Patch};
use crate::id::IdList;
use crate::registry::Example;

const ID_FIELD: crate::id::Id = id(41);
const ID_FIELD_2: crate::id::Id = id(42);
const PRINCIPAL: &str = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";

impl Example for SetStatusRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetStatusRequest {
                    text: Some("In a meeting".into()),
                    emoji: Some("📅".into()),
                    expires_at: Some(t(1_800_000)),
                },
            ),
            (
                "min",
                SetStatusRequest {
                    emoji: Some("🌴".into()),
                    ..SetStatusRequest::default()
                },
            ),
        ]
    }
}

impl Example for UpdateMyProfileRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateMyProfileRequest {
                    display_name: Some("Rosson L.".into()),
                    title: Patch::Set("Engineering lead".into()),
                    pronouns: Patch::Clear,
                    timezone: Patch::Set("America/Denver".into()),
                    custom_fields: vec![
                        CustomFieldInput {
                            field_id: ID_FIELD,
                            value: Some("Platform".into()),
                        },
                        CustomFieldInput {
                            field_id: ID_FIELD_2,
                            value: None,
                        },
                    ],
                },
            ),
            (
                "min",
                UpdateMyProfileRequest {
                    timezone: Patch::Set("Europe/Berlin".into()),
                    ..UpdateMyProfileRequest::default()
                },
            ),
        ]
    }
}

pub(super) fn profile_field_full() -> ProfileField {
    ProfileField {
        id: ID_FIELD,
        key: "team".into(),
        label: "Team".into(),
        kind: ProfileFieldKind::Select,
        options: vec!["Platform".into(), "Apps".into()],
        managed_by: ProfileFieldManagedBy::Member,
        visibility: ProfileFieldVisibility::Everyone,
        position: 0,
    }
}

impl Example for ProfileField {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", profile_field_full())]
    }
}

impl Example for CreateProfileFieldRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateProfileFieldRequest {
                    key: "team".into(),
                    label: "Team".into(),
                    kind: ProfileFieldKind::Select,
                    options: Some(vec!["Platform".into(), "Apps".into()]),
                    managed_by: ProfileFieldManagedBy::Member,
                    visibility: ProfileFieldVisibility::Everyone,
                    position: Some(0),
                },
            ),
            (
                "min",
                CreateProfileFieldRequest {
                    key: "cost_center".into(),
                    label: "Cost center".into(),
                    kind: ProfileFieldKind::Text,
                    options: None,
                    managed_by: ProfileFieldManagedBy::Provider,
                    visibility: ProfileFieldVisibility::Admins,
                    position: None,
                },
            ),
        ]
    }
}

impl Example for UpdateProfileFieldRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateProfileFieldRequest {
                    label: Some("Team name".into()),
                    options: Some(vec!["Platform".into(), "Apps".into(), "Ops".into()]),
                    visibility: Some(ProfileFieldVisibility::NonGuests),
                    position: Some(2),
                },
            ),
            (
                "min",
                UpdateProfileFieldRequest {
                    label: Some("Desk".into()),
                    ..UpdateProfileFieldRequest::default()
                },
            ),
        ]
    }
}

impl Example for ReorderProfileFieldsRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReorderProfileFieldsRequest {
                field_ids: vec![ID_FIELD_2, ID_FIELD],
            },
        )]
    }
}

impl Example for ProfileFieldsResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ProfileFieldsResponse {
                items: vec![profile_field_full()],
                version: "3".into(),
            },
        )]
    }
}

impl Example for ResetProfileRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ResetProfileRequest {
                fields: vec![ResettableField::Avatar, ResettableField::Status],
            },
        )]
    }
}

fn schedule_full() -> DndSchedule {
    DndSchedule {
        timezone: Some("America/New_York".into()),
        rules: vec![
            DndRule {
                days: vec![
                    DndDay::Mon,
                    DndDay::Tue,
                    DndDay::Wed,
                    DndDay::Thu,
                    DndDay::Fri,
                ],
                start: "18:00".into(),
                end: "09:00".into(),
            },
            DndRule {
                days: vec![DndDay::Sat, DndDay::Sun],
                start: "00:00".into(),
                end: "24:00".into(),
            },
        ],
    }
}

fn dnd_state_full() -> DndState {
    DndState {
        active: true,
        source: DndSource::Schedule,
        until: Some(t(3_600_000)),
    }
}

impl Example for DndState {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", dnd_state_full()),
            (
                "min",
                DndState {
                    active: false,
                    source: DndSource::None,
                    until: None,
                },
            ),
        ]
    }
}

pub(super) fn presence_settings_full() -> PresenceSettings {
    PresenceSettings {
        manual: ManualPresence::Dnd,
        manual_until: Some(t(3_600_000)),
        self_status: SelfStatus::Dnd,
        dnd: dnd_state_full(),
        dnd_schedule: Some(schedule_full()),
        dnd_skip_until: Some(t(60_000)),
        effective_timezone: "America/New_York".into(),
    }
}

impl Example for PresenceSettings {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", presence_settings_full()),
            (
                "min",
                PresenceSettings {
                    manual: ManualPresence::None,
                    manual_until: None,
                    self_status: SelfStatus::Online,
                    dnd: DndState {
                        active: false,
                        source: DndSource::None,
                        until: None,
                    },
                    dnd_schedule: None,
                    dnd_skip_until: None,
                    effective_timezone: "UTC".into(),
                },
            ),
        ]
    }
}

impl Example for SetPresenceRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetPresenceRequest {
                    manual: ManualPresence::Dnd,
                    until: Some(t(3_600_000)),
                },
            ),
            (
                "min",
                SetPresenceRequest {
                    manual: ManualPresence::None,
                    until: None,
                },
            ),
        ]
    }
}

impl Example for SetDndScheduleRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SetDndScheduleRequest {
                    schedule: Some(schedule_full()),
                },
            ),
            ("min", SetDndScheduleRequest { schedule: None }),
        ]
    }
}

fn entry(member_id: crate::id::Id, status: PresenceStatus) -> PresenceEntry {
    PresenceEntry { member_id, status }
}

impl Example for PresenceList {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PresenceList {
                items: vec![
                    entry(ID_BOB, PresenceStatus::Online),
                    entry(ID_CAROL, PresenceStatus::Offline),
                ],
            },
        )]
    }
}

fn ids(v: Vec<crate::id::Id>) -> IdList {
    match IdList::new(v) {
        Ok(l) => l,
        Err(e) => panic!("example id list: {e}"),
    }
}

impl Example for PresenceQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PresenceQuery {
                ids: ids(vec![ID_BOB, ID_CAROL]),
            },
        )]
    }
}

impl Example for DirectoryQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DirectoryQuery {
                    q: Some("jos alv".into()),
                    space_id: Some(ID_SPACE),
                    channel_id: Some(ID_CHANNEL),
                    kind: Some(MemberKind::Person),
                    cursor: Some("am9zw6kAMTIz".into()),
                    limit: Some(50),
                },
            ),
            ("min", DirectoryQuery::default()),
        ]
    }
}

impl Example for DirectoryPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            DirectoryPage {
                items: vec![DirectoryEntry {
                    member: member_min(),
                    presence: PresenceStatus::Online,
                }],
                next_cursor: Some("cm9zc29uADQ4ODMxNzY1MTQ3NTQ5Njk5".into()),
                has_more: true,
            },
        )]
    }
}

impl Example for MembersQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MembersQuery {
                    ids: Some(ids(vec![ID_BOB, ID_ME])),
                    username: Some(handle("rosson")),
                },
            ),
            (
                "min",
                MembersQuery {
                    ids: None,
                    username: Some(handle("rosson")),
                },
            ),
        ]
    }
}

pub(super) fn group_full() -> UserGroup {
    UserGroup {
        id: ID_GROUP,
        space_id: Some(ID_SPACE),
        handle: handle("acme-eng"),
        name: "Acme engineering".into(),
        description: Some("Everyone who ships code".into()),
        mentionable: true,
        member_count: 12,
        created_by: ID_ME,
        created_at: t(0),
        updated_at: t(60_000),
        deleted_at: Some(t(120_000)),
    }
}

fn group_min() -> UserGroup {
    UserGroup {
        id: ID_GROUP,
        space_id: None,
        handle: handle("oncall"),
        name: "On call".into(),
        description: None,
        mentionable: true,
        member_count: 3,
        created_by: ID_ME,
        created_at: t(0),
        updated_at: t(0),
        deleted_at: None,
    }
}

impl Example for UserGroup {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", group_full()), ("min", group_min())]
    }
}

impl Example for CreateUserGroupRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateUserGroupRequest {
                    space_id: Some(ID_SPACE),
                    handle: handle("acme-eng"),
                    name: "Acme engineering".into(),
                    description: Some("Everyone who ships code".into()),
                    mentionable: Some(true),
                    member_ids: vec![ID_ME, ID_BOB],
                },
            ),
            (
                "min",
                CreateUserGroupRequest {
                    space_id: None,
                    handle: handle("oncall"),
                    name: "On call".into(),
                    description: None,
                    mentionable: None,
                    member_ids: vec![],
                },
            ),
        ]
    }
}

impl Example for UpdateUserGroupRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateUserGroupRequest {
                    handle: Some(handle("platform")),
                    name: Some("Platform".into()),
                    description: Patch::Set("The platform team".into()),
                    mentionable: Some(false),
                },
            ),
            (
                "min",
                UpdateUserGroupRequest {
                    mentionable: Some(true),
                    ..UpdateUserGroupRequest::default()
                },
            ),
        ]
    }
}

impl Example for GroupMembersRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            GroupMembersRequest {
                member_ids: vec![ID_BOB, ID_CAROL],
            },
        )]
    }
}

impl Example for AddGroupMembersResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            AddGroupMembersResponse {
                added: vec![ID_BOB],
                already: vec![ID_ME],
                group: group_min(),
            },
        )]
    }
}

impl Example for RemoveGroupMembersResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RemoveGroupMembersResponse {
                removed: vec![ID_BOB],
                absent: vec![ID_CAROL],
                group: group_min(),
            },
        )]
    }
}

fn group_member() -> GroupMember {
    GroupMember {
        group_id: ID_GROUP,
        member_id: ID_BOB,
        added_at: t(0),
    }
}

impl Example for GroupMember {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", group_member())]
    }
}

impl Example for Page<GroupMember> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![group_member()],
                has_more: false,
            },
        )]
    }
}

impl Example for GroupsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GroupsQuery {
                    space_id: Some(ID_SPACE),
                },
            ),
            ("min", GroupsQuery::default()),
        ]
    }
}

impl Example for UserGroupsResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            UserGroupsResponse {
                items: vec![group_min()],
                version: "4.2".into(),
            },
        )]
    }
}

impl Example for MentionablesQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MentionablesQuery {
                    q: Some("des".into()),
                    limit: Some(20),
                },
            ),
            ("min", MentionablesQuery::default()),
        ]
    }
}

impl Example for MentionablesResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MentionablesResponse {
                items: vec![
                    Mentionable {
                        kind: MentionableKind::Special,
                        id: None,
                        handle: Some("here".into()),
                        name: "here".into(),
                        member_count: None,
                        space_id: None,
                    },
                    Mentionable {
                        kind: MentionableKind::Group,
                        id: Some(ID_GROUP),
                        handle: Some("acme-eng".into()),
                        name: "Acme engineering".into(),
                        member_count: Some(12),
                        space_id: Some(ID_SPACE),
                    },
                    Mentionable {
                        kind: MentionableKind::Role,
                        id: Some(ID_ROLE),
                        handle: None,
                        name: "Designers".into(),
                        member_count: None,
                        space_id: Some(ID_SPACE),
                    },
                ],
            },
        )]
    }
}

impl Example for PresenceSnapshot {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PresenceSnapshot {
                items: vec![
                    entry(ID_BOB, PresenceStatus::Online),
                    entry(ID_CAROL, PresenceStatus::Offline),
                ],
            },
        )]
    }
}

impl Example for MemberListUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MemberListUpdated {
                    channel_id: ID_CHANNEL,
                    member_count: 150,
                    gone: false,
                    ops: vec![
                        MemberListOp::Sync {
                            range: [0, 99],
                            items: vec![MemberListItem {
                                member: member_min(),
                                presence: PresenceStatus::Idle,
                            }],
                        },
                        MemberListOp::Update {
                            index: 5,
                            item: Box::new(MemberListItem {
                                member: member_full(),
                                presence: PresenceStatus::Dnd,
                            }),
                        },
                    ],
                },
            ),
            (
                "min",
                MemberListUpdated {
                    channel_id: ID_CHANNEL,
                    member_count: 0,
                    gone: true,
                    ops: vec![],
                },
            ),
        ]
    }
}

impl Example for UserGroupDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            UserGroupDeleted {
                id: ID_GROUP,
                space_id: Some(ID_SPACE),
            },
        )]
    }
}

impl Example for ProfileFieldsUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ProfileFieldsUpdated {
                version: "4".into(),
            },
        )]
    }
}

impl Example for MgmtAvatarQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MgmtAvatarQuery {
                external_id: ext(PRINCIPAL),
                force: true,
            },
        )]
    }
}

impl Example for MgmtAvatarClearRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MgmtAvatarClearRequest {
                external_id: ext(PRINCIPAL),
                force: false,
            },
        )]
    }
}

impl Example for MgmtAvatarResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MgmtAvatarResponse {
                    member_id: ID_ME,
                    applied: false,
                    reason: Some(reasons::MEMBER_AVATAR_KEPT.into()),
                },
            ),
            (
                "min",
                MgmtAvatarResponse {
                    member_id: ID_ME,
                    applied: true,
                    reason: None,
                },
            ),
        ]
    }
}
