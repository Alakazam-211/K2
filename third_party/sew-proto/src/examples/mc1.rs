//! Examples for the wire types Meet MC1 adds (prd-meet-01 §6.3, §6.14.2, CC7).

use super::{ID_BOB, ID_CHANNEL, ID_ME, ID_SPACE, id, t};
use crate::api::meetings::*;
use crate::mgmt::{MeetInitRequest, MeetInitResponse};
use crate::registry::Example;

const ID_MEETING: crate::id::Id = id(600);
const ID_ATTENDEE: crate::id::Id = id(601);
const ID_ATTENDEE_EMAIL: crate::id::Id = id(602);

/// One hour.
const HOUR: u64 = 3_600_000;

fn meeting_full() -> Meeting {
    Meeting {
        id: ID_MEETING,
        channel_id: ID_CHANNEL,
        space_id: ID_SPACE,
        organiser_member_id: ID_ME,
        title: Some("Quarterly planning".into()),
        kind: MeetingKind::Scheduled,
        state: MeetingState::Ended,
        starts_at: t(24 * HOUR),
        ends_at: t(25 * HOUR),
        tz: "Europe/Berlin".into(),
        opens_at: t(24 * HOUR - HOUR / 4),
        hard_end_at: t(36 * HOUR),
        locked: false,
        auto_record: true,
        guest_screen_share: true,
        show_title_to_guests: false,
        ics_uid: format!("meet-{ID_CHANNEL}"),
        ics_sequence: 2,
        is_host: true,
        ended_at: Some(t(25 * HOUR + HOUR / 2)),
        ended_by: Some(ID_ME),
        end_reason: Some(MeetingEndReason::Host),
        created_at: t(0),
        updated_at: t(25 * HOUR + HOUR / 2),
    }
}

fn meeting_min() -> Meeting {
    Meeting {
        title: None,
        kind: MeetingKind::Instant,
        state: MeetingState::Live,
        starts_at: t(0),
        ends_at: t(HOUR),
        tz: "UTC".into(),
        opens_at: t(0),
        hard_end_at: t(12 * HOUR),
        auto_record: false,
        guest_screen_share: false,
        show_title_to_guests: true,
        ics_sequence: 0,
        is_host: false,
        ended_at: None,
        ended_by: None,
        end_reason: None,
        updated_at: t(0),
        ..meeting_full()
    }
}

impl Example for Meeting {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", meeting_full()), ("min", meeting_min())]
    }
}

impl Example for CreateMeetingRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = CreateMeetingRequest {
            client_nonce: Some("b7Hq2kV9xM4pT1sW".into()),
            kind: MeetingKind::Scheduled,
            title: "Quarterly planning".into(),
            starts_at: Some(t(24 * HOUR)),
            ends_at: Some(t(25 * HOUR)),
            tz: "Europe/Berlin".into(),
            attendees: vec![
                MeetingAttendeeInput {
                    kind: MeetingAttendeeKind::Member,
                    member_id: Some(ID_BOB),
                    email: None,
                },
                MeetingAttendeeInput {
                    kind: MeetingAttendeeKind::Email,
                    member_id: None,
                    email: Some("dana@example.com".into()),
                },
            ],
            auto_record: Some(true),
            guest_screen_share: Some(true),
            show_title_to_guests: Some(false),
        };
        let min = CreateMeetingRequest {
            client_nonce: None,
            kind: MeetingKind::Instant,
            title: "Quick sync".into(),
            starts_at: None,
            ends_at: None,
            tz: "UTC".into(),
            attendees: vec![],
            auto_record: None,
            guest_screen_share: None,
            show_title_to_guests: None,
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for UpdateMeetingRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = UpdateMeetingRequest {
            title: Some("Quarterly planning (moved)".into()),
            starts_at: Some(t(48 * HOUR)),
            ends_at: Some(t(49 * HOUR)),
            tz: Some("Europe/Berlin".into()),
            attendees: Some(vec![MeetingAttendeeInput {
                kind: MeetingAttendeeKind::Member,
                member_id: Some(ID_BOB),
                email: None,
            }]),
            auto_record: Some(false),
            guest_screen_share: Some(false),
            show_title_to_guests: Some(true),
        };
        vec![("full", full), ("min", UpdateMeetingRequest::default())]
    }
}

impl Example for RemoveMeetingAttendeeRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RemoveMeetingAttendeeRequest {
                attendee_id: ID_ATTENDEE_EMAIL,
            },
        )]
    }
}

impl Example for MeetingListQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetingListQuery {
            scope: Some(MeetingListScope::Past),
            cursor: Some("1790596800123.48831765147550296".into()),
            limit: Some(20),
        };
        vec![("full", full), ("min", MeetingListQuery::default())]
    }
}

impl Example for MeetingCreated {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetingCreated {
            meeting: meeting_min(),
            channel_id: ID_CHANNEL,
            link_url: Some(super::mc2a::link().path),
            link: Some(super::mc2a::link()),
        };
        let min = MeetingCreated {
            link_url: None,
            link: None,
            ..full.clone()
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MeetingPage {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetingPage {
            items: vec![meeting_full()],
            next_cursor: Some("1790596800123.48831765147550296".into()),
            has_more: true,
        };
        let min = MeetingPage {
            items: vec![],
            next_cursor: None,
            has_more: false,
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MeetingAttendees {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetingAttendees {
            items: vec![
                MeetingAttendee {
                    id: ID_ATTENDEE,
                    kind: MeetingAttendeeKind::Member,
                    member_id: Some(ID_BOB),
                    email: None,
                },
                MeetingAttendee {
                    id: ID_ATTENDEE_EMAIL,
                    kind: MeetingAttendeeKind::Email,
                    member_id: None,
                    email: Some("dana@example.com".into()),
                },
            ],
            emails_purged: false,
        };
        let min = MeetingAttendees {
            items: vec![],
            emails_purged: true,
        };
        vec![("full", full), ("min", min)]
    }
}

impl Example for MeetingUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingUpdated {
                    meeting: meeting_full(),
                },
            ),
            (
                "min",
                MeetingUpdated {
                    meeting: meeting_min(),
                },
            ),
        ]
    }
}

impl Example for MeetInitRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetInitRequest {
            app_id: Some("meet".into()),
            retention_days: Some(365),
        };
        vec![("full", full), ("min", MeetInitRequest::default())]
    }
}

impl Example for MeetInitResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        let full = MeetInitResponse {
            space_id: ID_SPACE,
            created: true,
            app_id: Some("meet".into()),
            retention_days: Some(365),
        };
        let min = MeetInitResponse {
            space_id: ID_SPACE,
            created: false,
            app_id: None,
            retention_days: None,
        };
        vec![("full", full), ("min", min)]
    }
}
