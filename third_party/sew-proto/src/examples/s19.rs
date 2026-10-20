//! Examples for the wire types S19 adds (prd-sew-19 §18).

use super::{ID_BOB, ID_CHANNEL, ID_OTHER_CHANNEL, ID_SPACE, handle, id, member_min};
use crate::api::channels::Visibility;
use crate::api::guests::*;
use crate::api::members::{InstallRole, Member};
use crate::http::Page;
use crate::mgmt::local::LocalGuestRequest;
use crate::registry::Example;

fn guest() -> Member {
    Member {
        id: ID_BOB,
        install_role: InstallRole::Guest,
        username: handle("gia"),
        display_name: "Gia".into(),
        ..member_min()
    }
}

impl Example for SetGuestRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", SetGuestRequest { guest: true }),
            ("member", SetGuestRequest { guest: false }),
        ]
    }
}

impl Example for GuestConvertResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GuestConvertResponse {
                    member: guest(),
                    changed: true,
                    roles_removed: vec![id(61)],
                    overwrites_trimmed: 1,
                    spaces_transferred: vec![id(62)],
                    spaces_left: vec![id(63)],
                    groups_left: 1,
                },
            ),
            (
                "min",
                GuestConvertResponse {
                    member: guest(),
                    changed: false,
                    roles_removed: vec![],
                    overwrites_trimmed: 0,
                    spaces_transferred: vec![],
                    spaces_left: vec![],
                    groups_left: 0,
                },
            ),
        ]
    }
}

fn summary_full() -> GuestSummary {
    GuestSummary {
        member: guest(),
        spaces: vec![ID_SPACE],
        channels: vec![
            GuestChannelRef {
                channel_id: ID_CHANNEL,
                space_id: ID_SPACE,
                name: "vendor".into(),
                visibility: Visibility::Public,
            },
            GuestChannelRef {
                channel_id: ID_OTHER_CHANNEL,
                space_id: ID_SPACE,
                name: "partners".into(),
                visibility: Visibility::Private,
            },
        ],
    }
}

impl Example for GuestSummary {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", summary_full()),
            (
                "min",
                GuestSummary {
                    member: guest(),
                    spaces: vec![],
                    channels: vec![],
                },
            ),
        ]
    }
}

impl Example for GuestSummaryPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![summary_full()],
                has_more: true,
            },
        )]
    }
}

impl Example for GuestListQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                GuestListQuery {
                    after: Some(ID_BOB),
                    limit: Some(20),
                    include_deactivated: Some(true),
                },
            ),
            ("min", GuestListQuery::default()),
        ]
    }
}

impl Example for LocalGuestRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                LocalGuestRequest {
                    username: handle("bob"),
                    guest: true,
                },
            ),
            (
                "member",
                LocalGuestRequest {
                    username: handle("bob"),
                    guest: false,
                },
            ),
        ]
    }
}
