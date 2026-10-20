//! Examples for the wire types Meet MC2a adds (prd-meet-01 §6.4, §6.5, §6.14.1).

use super::{id, t};
use crate::api::auth::SessionResponse;
use crate::api::meetings::*;
use crate::registry::Example;

const ID_MEETING: crate::id::Id = id(600);
const ID_GUEST: crate::id::Id = id(610);

/// One hour.
const HOUR: u64 = 3_600_000;

const LINK_ID: &str = "K7QWE2RTY4UIOPA3";
const SECRET: &str = "q4Zr8bXk2nWmVt7yLp0aSg";

pub(super) fn link() -> MeetingLink {
    MeetingLink {
        link_id: LINK_ID.into(),
        secret: SECRET.into(),
        path: format!("/j/{LINK_ID}#{SECRET}"),
        valid_until: t(12 * HOUR),
        max_guests_inside: MEETING_LINK_MAX_GUESTS_DEFAULT,
        max_waiting: MEETING_LINK_MAX_WAITING_DEFAULT,
        created_at: t(0),
    }
}

impl Example for MeetingLink {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", link())]
    }
}

impl Example for MeetingLinkResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", MeetingLinkResponse { link: Some(link()) }),
            ("min", MeetingLinkResponse { link: None }),
        ]
    }
}

impl Example for RotateMeetingLinkRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RotateMeetingLinkRequest {
                    client_nonce: Some("r0tAte-n0nce_0001".into()),
                },
            ),
            ("min", RotateMeetingLinkRequest::default()),
        ]
    }
}

impl Example for MeetingLinkRef {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingLinkRef {
                link_id: LINK_ID.into(),
                secret: SECRET.into(),
            },
        )]
    }
}

impl Example for MeetingPeek {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetingPeek {
            title: Some("Quarterly planning".into()),
            organiser_name: Some("Rosson Long".into()),
            starts_at: t(HOUR),
            ends_at: t(2 * HOUR),
            tz: "Europe/Berlin".into(),
            state: MeetingState::Scheduled,
            locked: false,
            opens_at: t(HOUR - HOUR / 4),
            recording: MeetingPeekRecording {
                auto_record: true,
                active: false,
            },
            refused_browsers: vec!["firefox".into()],
            privacy_contact: Some("privacy@example.com".into()),
        };
        let min = MeetingPeek {
            title: None,
            organiser_name: None,
            state: MeetingState::Live,
            recording: MeetingPeekRecording {
                auto_record: false,
                active: true,
            },
            refused_browsers: vec![],
            privacy_contact: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MeetingRedeemRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingRedeemRequest {
                link_id: LINK_ID.into(),
                secret: SECRET.into(),
                display_name: "Dana from Acme".into(),
                join_nonce: "3q2-7wAAAAAAAAAAAAAAAA".into(),
            },
        )]
    }
}

impl Example for MeetingRedeemed {
    fn examples() -> Vec<(&'static str, Self)> {
        let mut member = super::member_min();
        member.id = ID_GUEST;
        member.install_role = crate::api::members::InstallRole::LinkGuest;
        let full = MeetingRedeemed {
            session: SessionResponse {
                token: "sews_AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    .into(),
                expires_at: t(HOUR),
                member,
            },
            member_id: ID_GUEST,
            lobby_entry_id: Some(id(611)),
            state: MeetingRedeemState::Waiting,
            valid_until: t(12 * HOUR),
        };
        let min = MeetingRedeemed {
            lobby_entry_id: None,
            state: MeetingRedeemState::Admitted,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MeetingResolved {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingResolved {
                meeting_id: ID_MEETING,
                member_of_channel: true,
            },
        )]
    }
}

impl Example for ChannelMeeting {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", ChannelMeeting { id: ID_MEETING })]
    }
}

impl Example for MeetingPeekRecording {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingPeekRecording {
                auto_record: true,
                active: true,
            },
        )]
    }
}
