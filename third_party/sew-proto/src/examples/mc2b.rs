//! Examples for the wire types Meet MC2b adds (prd-meet-01 §6.5.4, §6.6, §6.14.2).

use super::{id, t};
use crate::api::meetings::*;
use crate::registry::Example;

const ID_MEETING: crate::id::Id = id(600);
const ID_CHANNEL: crate::id::Id = id(601);
const ID_GUEST: crate::id::Id = id(610);
const ID_ENTRY: crate::id::Id = id(611);
const ID_MEMBER: crate::id::Id = id(612);
const ID_ENTRY_2: crate::id::Id = id(613);

/// One minute.
const MIN: u64 = 60_000;

fn waiting() -> MeetingLobbyEntry {
    MeetingLobbyEntry {
        entry_id: ID_ENTRY,
        meeting_id: ID_MEETING,
        member_id: ID_GUEST,
        display_name: "Dana from Acme".into(),
        kind: MeetingLobbyKind::LinkGuest,
        state: MeetingLobbyState::Waiting,
        knocked_at: t(0),
        decided_at: None,
        waited_minutes: 2,
    }
}

fn admitted() -> MeetingLobbyEntry {
    MeetingLobbyEntry {
        entry_id: ID_ENTRY_2,
        member_id: ID_MEMBER,
        display_name: "Sam Rivera".into(),
        kind: MeetingLobbyKind::Member,
        state: MeetingLobbyState::Admitted,
        decided_at: Some(t(3 * MIN)),
        waited_minutes: 3,
        ..waiting()
    }
}

impl Example for MeetingLobbyEntry {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", admitted()), ("min", waiting())]
    }
}

impl Example for MeetingLobby {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingLobby {
                    items: vec![waiting()],
                },
            ),
            ("min", MeetingLobby { items: vec![] }),
        ]
    }
}

impl Example for MeetingLobbyDecided {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingLobbyDecided {
                    items: vec![admitted()],
                    still_waiting: 1,
                },
            ),
            (
                "min",
                MeetingLobbyDecided {
                    items: vec![],
                    still_waiting: 0,
                },
            ),
        ]
    }
}

impl Example for MeetingKnocked {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingKnocked {
                    meeting_id: ID_MEETING,
                    state: MeetingRedeemState::Waiting,
                    lobby_entry_id: Some(ID_ENTRY_2),
                },
            ),
            (
                "min",
                MeetingKnocked {
                    meeting_id: ID_MEETING,
                    state: MeetingRedeemState::Admitted,
                    lobby_entry_id: None,
                },
            ),
        ]
    }
}

impl Example for MeetingLobbyStatus {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingLobbyStatus {
                    entry_id: ID_ENTRY,
                    meeting_id: ID_MEETING,
                    state: MeetingLobbyState::Admitted,
                    knocked_at: t(0),
                    decided_at: Some(t(MIN)),
                    channel_id: Some(ID_CHANNEL),
                },
            ),
            (
                "min",
                MeetingLobbyStatus {
                    entry_id: ID_ENTRY,
                    meeting_id: ID_MEETING,
                    state: MeetingLobbyState::Waiting,
                    knocked_at: t(0),
                    decided_at: None,
                    channel_id: None,
                },
            ),
        ]
    }
}

impl Example for MeetingLobbyMe {
    fn examples() -> Vec<(&'static str, Self)> {
        let one = MeetingLobbyStatus::examples().remove(0).1;
        vec![
            ("full", MeetingLobbyMe { items: vec![one] }),
            ("min", MeetingLobbyMe { items: vec![] }),
        ]
    }
}

impl Example for MeetingLobbyUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingLobbyUpdated {
                meeting_id: ID_MEETING,
                entry: waiting(),
                waiting: 1,
            },
        )]
    }
}

impl Example for MeetingAdmission {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                MeetingAdmission {
                    meeting_id: ID_MEETING,
                    entry_id: ID_ENTRY,
                    outcome: MeetingAdmissionOutcome::Admitted,
                    channel_id: Some(ID_CHANNEL),
                },
            ),
            (
                "min",
                MeetingAdmission {
                    meeting_id: ID_MEETING,
                    entry_id: ID_ENTRY,
                    outcome: MeetingAdmissionOutcome::Denied,
                    channel_id: None,
                },
            ),
        ]
    }
}

impl Example for MeetingGuestUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeetingGuestUpdated {
                meeting_id: ID_MEETING,
                state: MeetingState::Live,
                locked: false,
            },
        )]
    }
}
