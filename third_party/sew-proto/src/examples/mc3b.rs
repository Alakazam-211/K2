//! Examples for the wire types Meet MC3b adds (a meeting channel's people).

use super::id;
use crate::api::meetings::*;
use crate::http::Page;
use crate::registry::Example;

const ID_GUEST: crate::id::Id = id(610);
const ID_MEMBER: crate::id::Id = id(612);

fn guest() -> MeetingPerson {
    MeetingPerson {
        member_id: ID_GUEST,
        display_name: "Dana from Acme".into(),
        kind: MeetingLobbyKind::LinkGuest,
    }
}

fn member() -> MeetingPerson {
    MeetingPerson {
        member_id: ID_MEMBER,
        display_name: "Sam Rivera".into(),
        kind: MeetingLobbyKind::Member,
    }
}

impl Example for MeetingPerson {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", guest())]
    }
}

impl Example for Page<MeetingPerson> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![guest(), member()],
                has_more: false,
            },
        )]
    }
}
