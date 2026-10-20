//! Examples for the wire types S14 adds or extends (prd-sew-14 §15): read states, the threads
//! inbox, bulk acks, change pages and `/me/sync`.

use super::{
    ID_CAROL, ID_CHANNEL, ID_DM, ID_INSTALL, ID_ME, ID_MESSAGE, ID_OTHER_CHANNEL, ID_REPLY,
    message_full, t, thread_full,
};
use crate::api::changes::*;
use crate::api::read_state::*;
use crate::changes::*;
use crate::gateway::server::ThreadsInbox;
use crate::id::IdList;
use crate::registry::Example;

fn ids(v: Vec<crate::id::Id>) -> IdList {
    match IdList::new(v) {
        Ok(l) => l,
        Err(e) => panic!("example IdList: {e}"),
    }
}

pub(super) fn read_state_full() -> ReadState {
    ReadState {
        channel_id: ID_CHANNEL,
        last_read_id: Some(ID_MESSAGE),
        last_message_id: Some(ID_REPLY),
        mention_count: 2,
        unread: true,
        muted: true,
        seq: 4_411,
    }
}

fn read_state_min() -> ReadState {
    ReadState {
        channel_id: ID_DM,
        last_read_id: None,
        last_message_id: None,
        mention_count: 0,
        unread: false,
        muted: false,
        seq: 0,
    }
}

/// A READY read state: no `last_message_id` (the channel entry has it).
pub(super) fn ready_read_state() -> ReadState {
    ReadState {
        last_message_id: None,
        muted: false,
        ..read_state_full()
    }
}

pub(super) fn inbox_full() -> ThreadsInbox {
    ThreadsInbox {
        unread_count: 3,
        mention_count: 1,
        seq: Some(4_412),
    }
}

fn thread_state_full() -> ThreadReadState {
    ThreadReadState {
        root_id: ID_MESSAGE,
        channel_id: Some(ID_CHANNEL),
        last_read_id: Some(ID_MESSAGE),
        last_reply_id: Some(ID_REPLY),
        unread: true,
        unread_replies: 2,
        mention_count: 1,
        following: true,
        seq: 4_412,
    }
}

impl Example for ReadState {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", read_state_full()), ("min", read_state_min())]
    }
}

impl Example for ThreadReadState {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", thread_state_full()),
            (
                "min",
                ThreadReadState {
                    root_id: ID_MESSAGE,
                    channel_id: None,
                    last_read_id: None,
                    last_reply_id: None,
                    unread: false,
                    unread_replies: 0,
                    mention_count: 0,
                    following: false,
                    seq: 0,
                },
            ),
        ]
    }
}

impl Example for ThreadsInbox {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", inbox_full()),
            (
                "min",
                ThreadsInbox {
                    unread_count: 0,
                    mention_count: 0,
                    seq: None,
                },
            ),
        ]
    }
}

impl Example for AckRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                AckRequest {
                    message_id: Some(ID_MESSAGE),
                },
            ),
            ("min", AckRequest { message_id: None }),
        ]
    }
}

impl Example for MarkUnreadRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MarkUnreadRequest {
                message_id: ID_MESSAGE,
            },
        )]
    }
}

impl Example for BulkAckRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                BulkAckRequest {
                    channels: vec![
                        AckItem {
                            channel_id: ID_CHANNEL,
                            message_id: Some(ID_REPLY),
                        },
                        AckItem {
                            channel_id: ID_DM,
                            message_id: None,
                        },
                    ],
                    threads: vec![ThreadAckItem {
                        root_id: ID_MESSAGE,
                        message_id: Some(ID_REPLY),
                    }],
                    include_threads: true,
                },
            ),
            (
                "min",
                BulkAckRequest {
                    channels: vec![AckItem {
                        channel_id: ID_CHANNEL,
                        message_id: None,
                    }],
                    threads: vec![],
                    include_threads: false,
                },
            ),
        ]
    }
}

impl Example for BulkAckResponse {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            BulkAckResponse {
                read_states: vec![read_state_full()],
                threads: vec![thread_state_full()],
                threads_inbox: inbox_full(),
                threads_truncated: true,
            },
        )]
    }
}

impl Example for ThreadReadUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ThreadReadUpdated {
                    thread: thread_state_full(),
                    inbox: Some(inbox_full()),
                },
            ),
            (
                "min",
                ThreadReadUpdated {
                    thread: thread_state_full(),
                    inbox: None,
                },
            ),
        ]
    }
}

impl Example for ReadStatesQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ReadStatesQuery {
                    channel_ids: Some(ids(vec![ID_CHANNEL, ID_DM])),
                },
            ),
            ("min", ReadStatesQuery { channel_ids: None }),
        ]
    }
}

impl Example for ThreadReadsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ThreadReadsQuery {
                root_ids: ids(vec![ID_MESSAGE]),
            },
        )]
    }
}

impl Example for ThreadsInboxQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ThreadsInboxQuery {
                    before: Some(ID_REPLY),
                    limit: Some(20),
                    filter: Some(ThreadsInboxFilter::Unread),
                },
            ),
            (
                "min",
                ThreadsInboxQuery {
                    before: None,
                    limit: None,
                    filter: None,
                },
            ),
        ]
    }
}

impl Example for ThreadInboxItem {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ThreadInboxItem {
                activity_id: ID_REPLY,
                root: message_full(),
                thread: thread_full(),
                read: thread_state_full(),
            },
        )]
    }
}

impl Example for crate::http::Page<ThreadInboxItem> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            crate::http::Page {
                items: vec![ThreadInboxItem {
                    activity_id: ID_REPLY,
                    root: message_full(),
                    thread: thread_full(),
                    read: thread_state_full(),
                }],
                has_more: true,
            },
        )]
    }
}

impl Example for crate::http::Page<ReadState> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            crate::http::Page {
                items: vec![read_state_full()],
                has_more: false,
            },
        )]
    }
}

impl Example for crate::http::Page<ThreadReadState> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            crate::http::Page {
                items: vec![thread_state_full()],
                has_more: false,
            },
        )]
    }
}

fn sync_channel_full() -> SyncChannel {
    SyncChannel {
        id: ID_CHANNEL,
        head_seq: 815,
        floor_seq: 12,
        last_message_id: Some(ID_REPLY),
    }
}

impl Example for SyncChannel {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", sync_channel_full()),
            (
                "min",
                SyncChannel {
                    id: ID_OTHER_CHANNEL,
                    head_seq: 0,
                    floor_seq: 0,
                    last_message_id: None,
                },
            ),
        ]
    }
}

impl Example for MeSync {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MeSync {
                install_id: ID_INSTALL,
                sync_epoch: "7713305120993321451".into(),
                member_id: ID_ME,
                member_seq: 4_412,
                channels: vec![sync_channel_full()],
                read_states: vec![read_state_full()],
                threads_inbox: inbox_full(),
            },
        )]
    }
}

impl Example for ChannelChangesPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ChannelChangesPage {
                entries: vec![
                    ChannelChange {
                        seq: 1_021,
                        kind: ChannelChangeKind::MessageCreated,
                        entity_id: ID_REPLY,
                        actor_id: Some(ID_CAROL),
                        at: t(60_000),
                    },
                    ChannelChange {
                        seq: 1_022,
                        kind: ChannelChangeKind::MessageDeleted,
                        entity_id: ID_MESSAGE,
                        actor_id: None,
                        at: t(120_000),
                    },
                ],
                head: 1_024,
                floor: 12,
                next_since: 1_023,
                has_more: true,
                sync_epoch: "7713305120993321451".into(),
            },
        )]
    }
}

impl Example for crate::api::messages::TimelineQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        use crate::api::messages::TimelineQuery;
        vec![
            (
                "full",
                TimelineQuery {
                    after: Some(ID_MESSAGE),
                    limit: Some(20),
                    wait_ms: Some(25_000),
                    ..TimelineQuery::default()
                },
            ),
            (
                "full_before",
                TimelineQuery {
                    before: Some(ID_REPLY),
                    ..TimelineQuery::default()
                },
            ),
            (
                "full_around",
                TimelineQuery {
                    around: Some(ID_REPLY),
                    ..TimelineQuery::default()
                },
            ),
            ("min", TimelineQuery::default()),
        ]
    }
}

impl Example for MemberChangesPage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MemberChangesPage {
                entries: vec![
                    MemberChange {
                        seq: 77,
                        kind: MemberChangeKind::ChannelJoined,
                        entity_id: Some(ID_CHANNEL),
                        reason: None,
                        ref_id: None,
                        at: t(0),
                    },
                    MemberChange {
                        seq: 78,
                        kind: MemberChangeKind::Notification,
                        entity_id: None,
                        reason: Some(NotificationReason::Mention),
                        ref_id: Some(ID_REPLY),
                        at: t(60_000),
                    },
                ],
                head: 78,
                floor: 3,
                next_since: 78,
                has_more: false,
                sync_epoch: "7713305120993321451".into(),
            },
        )]
    }
}
