//! Examples for the wire types S13 adds (prd-sew-13 §19).

use super::{ID_BOB, ID_CAROL, ID_CHANNEL, ID_EMOJI, ID_MESSAGE, ID_REPLY, t};
use crate::api::messages::*;
use crate::gateway::events::*;
use crate::http::Page;
use crate::id::IdList;
use crate::registry::Example;

impl Example for RemoveReactionRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RemoveReactionRequest {
                    emoji: EmojiInput::Unicode {
                        value: "👍".into()
                    },
                    member_id: Some(ID_BOB),
                },
            ),
            (
                "min",
                RemoveReactionRequest {
                    emoji: EmojiInput::Custom {
                        id: ID_EMOJI,
                        name: "party".into(),
                    },
                    member_id: None,
                },
            ),
        ]
    }
}

impl Example for MessagesByIdsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        let ids = match IdList::new(vec![ID_MESSAGE, ID_REPLY]) {
            Ok(l) => l,
            Err(e) => panic!("example IdList: {e}"),
        };
        vec![("full", MessagesByIdsQuery { ids })]
    }
}

impl Example for ReactorsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ReactorsQuery {
                    emoji: "u:👍".into(),
                    after: Some(ID_BOB),
                    limit: Some(20),
                },
            ),
            (
                "min",
                ReactorsQuery {
                    emoji: "c:48831765147549709".into(),
                    after: None,
                    limit: None,
                },
            ),
        ]
    }
}

fn reactor() -> Reactor {
    Reactor {
        member_id: ID_CAROL,
        created_at: t(90_000),
    }
}

impl Example for Reactor {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", reactor())]
    }
}

impl Example for Page<Reactor> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![reactor()],
                has_more: true,
            },
        )]
    }
}

impl Example for MessagePinned {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MessagePinned {
                channel_id: ID_CHANNEL,
                message_id: ID_MESSAGE,
                pinned_by: ID_BOB,
                pinned_at: t(120_000),
                seq: 1_040,
            },
        )]
    }
}

impl Example for MessageUnpinned {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MessageUnpinned {
                channel_id: ID_CHANNEL,
                message_id: ID_MESSAGE,
                seq: 1_041,
            },
        )]
    }
}
