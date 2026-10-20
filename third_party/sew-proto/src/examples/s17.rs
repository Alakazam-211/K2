//! Examples for the wire types S17 adds (prd-sew-17 §20).

use super::{ID_CHANNEL, ID_ME, ID_MESSAGE, ID_REPLY, id, t};
use crate::api::extras::*;
use crate::http::{Page, Patch};
use crate::id::IdList;
use crate::registry::Example;
use crate::when::When;

const ID_DRAFT: crate::id::Id = id(30);
const ID_SAVED: crate::id::Id = id(31);
const ID_SCHEDULED: crate::id::Id = id(32);
const ID_REMINDER: crate::id::Id = id(33);
const ID_BOOKMARK: crate::id::Id = id(34);
const ID_UPLOAD: crate::id::Id = id(35);

fn ids(v: &[crate::id::Id]) -> IdList {
    match IdList::new(v.to_vec()) {
        Ok(l) => l,
        Err(e) => panic!("example id list: {e}"),
    }
}

fn local_when() -> When {
    When::Local {
        date_time: "2026-10-01T08:50".into(),
        timezone: Some("America/New_York".into()),
    }
}

fn upload() -> DraftUpload {
    DraftUpload {
        upload_id: ID_UPLOAD,
        filename: "notes.pdf".into(),
        size: 48_213,
        content_type: "application/pdf".into(),
        expires_at: t(604_800_000),
    }
}

impl Example for DeletedRef {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", DeletedRef { id: ID_DRAFT })]
    }
}

pub(super) fn draft_full() -> Draft {
    Draft {
        id: ID_DRAFT,
        channel_id: ID_CHANNEL,
        thread_root_id: Some(ID_MESSAGE),
        body: "Checking the logs now".into(),
        uploads: vec![upload()],
        also_send_to_channel: true,
        rev: 3,
        created_at: t(0),
        updated_at: t(60_000),
    }
}

fn draft_min() -> Draft {
    Draft {
        id: ID_DRAFT,
        channel_id: ID_CHANNEL,
        thread_root_id: None,
        body: "hi".into(),
        uploads: vec![],
        also_send_to_channel: false,
        rev: 1,
        created_at: t(0),
        updated_at: t(0),
    }
}

impl Example for Draft {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", draft_full()), ("min", draft_min())]
    }
}

impl Example for Page<Draft> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![draft_full(), draft_min()],
                has_more: false,
            },
        )]
    }
}

impl Example for SaveDraftRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SaveDraftRequest {
                    thread_root_id: Some(ID_MESSAGE),
                    body: "Checking the logs now".into(),
                    upload_ids: vec![ID_UPLOAD],
                    also_send_to_channel: true,
                    if_rev: Some(2),
                },
            ),
            (
                "min",
                SaveDraftRequest {
                    body: "hi".into(),
                    ..SaveDraftRequest::default()
                },
            ),
        ]
    }
}

impl Example for DeleteDraftRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DeleteDraftRequest {
                    thread_root_id: Some(ID_MESSAGE),
                    if_rev: Some(3),
                },
            ),
            ("min", DeleteDraftRequest::default()),
        ]
    }
}

impl Example for DraftsQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DraftsQuery {
                    ids: Some(ids(&[ID_DRAFT])),
                    channel_id: Some(ID_CHANNEL),
                },
            ),
            ("min", DraftsQuery::default()),
        ]
    }
}

impl Example for DraftDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                DraftDeleted {
                    id: ID_DRAFT,
                    channel_id: ID_CHANNEL,
                    thread_root_id: Some(ID_MESSAGE),
                },
            ),
            (
                "min",
                DraftDeleted {
                    id: ID_DRAFT,
                    channel_id: ID_CHANNEL,
                    thread_root_id: None,
                },
            ),
        ]
    }
}

fn saved_full() -> SavedItem {
    SavedItem {
        id: ID_SAVED,
        message_id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        state: SavedState::Done,
        note: Some("reply to Bob".into()),
        message: Some(Box::new(super::message_min())),
        created_at: t(0),
        updated_at: t(60_000),
        state_changed_at: t(60_000),
    }
}

fn saved_min() -> SavedItem {
    SavedItem {
        id: ID_SAVED,
        message_id: ID_MESSAGE,
        channel_id: ID_CHANNEL,
        state: SavedState::Active,
        note: None,
        message: None,
        created_at: t(0),
        updated_at: t(0),
        state_changed_at: t(0),
    }
}

impl Example for SavedItem {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", saved_full()), ("min", saved_min())]
    }
}

impl Example for Page<SavedItem> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![saved_full()],
                has_more: true,
            },
        )]
    }
}

impl Example for SaveMessageRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SaveMessageRequest {
                    note: Some("reply to Bob".into()),
                },
            ),
            ("min", SaveMessageRequest::default()),
        ]
    }
}

impl Example for EditSavedRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                EditSavedRequest {
                    state: Some(SavedState::Archived),
                    note: Patch::Set("later".into()),
                },
            ),
            ("min", EditSavedRequest::default()),
        ]
    }
}

impl Example for SavedQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                SavedQuery {
                    state: Some(SavedState::Done),
                    ids: Some(ids(&[ID_SAVED])),
                    before: Some(ID_SAVED),
                    after: Some(ID_SAVED),
                    limit: Some(50),
                },
            ),
            ("min", SavedQuery::default()),
        ]
    }
}

impl Example for SavedItemDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SavedItemDeleted {
                id: ID_SAVED,
                message_id: ID_MESSAGE,
            },
        )]
    }
}

fn scheduled_full() -> ScheduledMessage {
    ScheduledMessage {
        id: ID_SCHEDULED,
        channel_id: ID_CHANNEL,
        thread_root_id: Some(ID_MESSAGE),
        also_send_to_channel: true,
        body: "Standup in 10 <@48831765147549699>".into(),
        uploads: vec![upload()],
        send_at: t(3_600_000),
        timezone: Some("America/New_York".into()),
        state: ScheduledState::Failed,
        fail_reason: Some(ScheduledFailReason::MissingPermission),
        failed_at: Some(t(3_600_412)),
        message_id: Some(ID_REPLY),
        sent_at: Some(t(3_600_200)),
        version: 2,
        created_at: t(0),
        updated_at: t(3_600_412),
    }
}

fn scheduled_min() -> ScheduledMessage {
    ScheduledMessage {
        id: ID_SCHEDULED,
        channel_id: ID_CHANNEL,
        thread_root_id: None,
        also_send_to_channel: false,
        body: "standup".into(),
        uploads: vec![],
        send_at: t(3_600_000),
        timezone: None,
        state: ScheduledState::Pending,
        fail_reason: None,
        failed_at: None,
        message_id: None,
        sent_at: None,
        version: 1,
        created_at: t(0),
        updated_at: t(0),
    }
}

impl Example for ScheduledMessage {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", scheduled_full()), ("min", scheduled_min())]
    }
}

impl Example for Page<ScheduledMessage> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![scheduled_min()],
                has_more: false,
            },
        )]
    }
}

impl Example for CreateScheduledRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateScheduledRequest {
                    body: "Standup in 10".into(),
                    thread_root_id: Some(ID_MESSAGE),
                    also_send_to_channel: true,
                    upload_ids: vec![ID_UPLOAD],
                    when: local_when(),
                    clear_draft: true,
                },
            ),
            (
                "min",
                CreateScheduledRequest {
                    body: "standup".into(),
                    thread_root_id: None,
                    also_send_to_channel: false,
                    upload_ids: vec![],
                    when: When::In { ms: 3_600_000 },
                    clear_draft: false,
                },
            ),
        ]
    }
}

impl Example for EditScheduledRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                EditScheduledRequest {
                    body: Some("Standup in 5".into()),
                    upload_ids: Some(vec![ID_UPLOAD]),
                    also_send_to_channel: Some(false),
                    when: Some(When::At { at: t(7_200_000) }),
                },
            ),
            ("min", EditScheduledRequest::default()),
        ]
    }
}

impl Example for ScheduledQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ScheduledQuery {
                    state: Some(ScheduledState::Failed),
                    channel_id: Some(ID_CHANNEL),
                    ids: Some(ids(&[ID_SCHEDULED])),
                },
            ),
            ("min", ScheduledQuery::default()),
        ]
    }
}

pub(super) fn reminder_full() -> Reminder {
    Reminder {
        id: ID_REMINDER,
        message_id: Some(ID_MESSAGE),
        channel_id: Some(ID_CHANNEL),
        note: Some("reply to Bob".into()),
        due_at: t(1_800_000),
        timezone: Some("America/New_York".into()),
        state: ReminderState::Fired,
        fired_at: Some(t(1_800_210)),
        done_at: Some(t(1_900_000)),
        snooze_count: 1,
        version: 2,
        created_at: t(0),
        updated_at: t(1_800_210),
        message_available: Some(true),
        message: Some(Box::new(super::message_min())),
    }
}

fn reminder_min() -> Reminder {
    Reminder {
        id: ID_REMINDER,
        message_id: None,
        channel_id: None,
        note: Some("water the plants".into()),
        due_at: t(1_800_000),
        timezone: None,
        state: ReminderState::Pending,
        fired_at: None,
        done_at: None,
        snooze_count: 0,
        version: 1,
        created_at: t(0),
        updated_at: t(0),
        message_available: None,
        message: None,
    }
}

impl Example for Reminder {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", reminder_full()), ("min", reminder_min())]
    }
}

impl Example for Page<Reminder> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![reminder_min()],
                has_more: false,
            },
        )]
    }
}

impl Example for CreateReminderRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateReminderRequest {
                    message_id: Some(ID_MESSAGE),
                    note: Some("reply to Bob".into()),
                    when: When::In { ms: 1_800_000 },
                },
            ),
            (
                "min",
                CreateReminderRequest {
                    message_id: None,
                    note: Some("water the plants".into()),
                    when: When::At { at: t(1_800_000) },
                },
            ),
        ]
    }
}

impl Example for EditReminderRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                EditReminderRequest {
                    note: Patch::Set("reply today".into()),
                    when: Some(local_when()),
                },
            ),
            ("min", EditReminderRequest::default()),
        ]
    }
}

impl Example for SnoozeReminderRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SnoozeReminderRequest {
                when: When::In { ms: 600_000 },
            },
        )]
    }
}

impl Example for RemindersQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                RemindersQuery {
                    state: Some(ReminderState::Done),
                    ids: Some(ids(&[ID_REMINDER])),
                    before: Some(ID_REMINDER),
                    limit: Some(50),
                },
            ),
            ("min", RemindersQuery::default()),
        ]
    }
}

impl Example for ReminderFired {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReminderFired {
                reminder: reminder_full(),
                late_ms: 210,
            },
        )]
    }
}

impl Example for PollInput {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PollInput {
                    question: "Lunch?".into(),
                    options: vec!["Tacos".into(), "Pizza".into(), "Salad".into()],
                    max_choices: Some(2),
                    anonymous: true,
                    results_visibility: Some(PollResultsVisibility::AfterClose),
                    duration_s: Some(3_600),
                },
            ),
            (
                "min",
                PollInput {
                    question: "Ship Friday?".into(),
                    options: vec!["Yes".into(), "No".into()],
                    max_choices: None,
                    anonymous: false,
                    results_visibility: None,
                    duration_s: None,
                },
            ),
        ]
    }
}

pub(super) fn poll_full() -> Poll {
    Poll {
        question: "Ship Friday?".into(),
        options: vec![
            PollOption {
                id: 1,
                text: "Yes".into(),
                votes: Some(12),
            },
            PollOption {
                id: 2,
                text: "No".into(),
                votes: Some(3),
            },
        ],
        max_choices: 1,
        anonymous: true,
        results_visibility: PollResultsVisibility::Always,
        closes_at: Some(t(86_400_000)),
        closed_at: Some(t(3_600_000)),
        closed_by: Some(ID_ME),
        state: PollState::Closed,
        voter_count: Some(15),
        my_votes: Some(vec![1]),
    }
}

fn poll_min() -> Poll {
    Poll {
        question: "Ship Friday?".into(),
        options: vec![
            PollOption {
                id: 1,
                text: "Yes".into(),
                votes: None,
            },
            PollOption {
                id: 2,
                text: "No".into(),
                votes: None,
            },
        ],
        max_choices: 1,
        anonymous: false,
        results_visibility: PollResultsVisibility::AfterClose,
        closes_at: None,
        closed_at: None,
        closed_by: None,
        state: PollState::Open,
        voter_count: None,
        my_votes: None,
    }
}

impl Example for PollVoteRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PollVoteRequest {
                    option_ids: vec![1, 3],
                },
            ),
            ("min", PollVoteRequest { option_ids: vec![] }),
        ]
    }
}

impl Example for PollVotersQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PollVotersQuery {
                    option_id: 1,
                    after: Some(ID_ME),
                    limit: Some(50),
                },
            ),
            (
                "min",
                PollVotersQuery {
                    option_id: 1,
                    after: None,
                    limit: None,
                },
            ),
        ]
    }
}

impl Example for PollVoter {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            PollVoter {
                member_id: ID_ME,
                voted_at: t(60_000),
            },
        )]
    }
}

impl Example for Page<PollVoter> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![PollVoter {
                    member_id: ID_ME,
                    voted_at: t(60_000),
                }],
                has_more: false,
            },
        )]
    }
}

impl Example for PollUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                PollUpdated {
                    message_id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    poll: poll_full(),
                    seq: 1_030,
                },
            ),
            (
                "min",
                PollUpdated {
                    message_id: ID_MESSAGE,
                    channel_id: ID_CHANNEL,
                    poll: poll_min(),
                    seq: 1_030,
                },
            ),
        ]
    }
}

fn bookmark_full() -> Bookmark {
    Bookmark {
        id: ID_BOOKMARK,
        channel_id: ID_CHANNEL,
        title: "Runbook".into(),
        url: "https://wiki.acme.example/runbook".into(),
        emoji: Some("📘".into()),
        position: 0,
        created_by: ID_ME,
        created_at: t(0),
        updated_at: t(60_000),
        seq: Some(1_031),
    }
}

fn bookmark_min() -> Bookmark {
    Bookmark {
        id: ID_BOOKMARK,
        channel_id: ID_CHANNEL,
        title: "Runbook".into(),
        url: "https://wiki.acme.example/runbook".into(),
        emoji: None,
        position: 0,
        created_by: ID_ME,
        created_at: t(0),
        updated_at: t(0),
        seq: None,
    }
}

impl Example for Bookmark {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", bookmark_full()), ("min", bookmark_min())]
    }
}

impl Example for Page<Bookmark> {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            Page {
                items: vec![bookmark_min()],
                has_more: false,
            },
        )]
    }
}

impl Example for CreateBookmarkRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateBookmarkRequest {
                    title: "Runbook".into(),
                    url: "https://wiki.acme.example/runbook".into(),
                    emoji: Some("📘".into()),
                },
            ),
            (
                "min",
                CreateBookmarkRequest {
                    title: "Runbook".into(),
                    url: "https://wiki.acme.example/runbook".into(),
                    emoji: None,
                },
            ),
        ]
    }
}

impl Example for EditBookmarkRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                EditBookmarkRequest {
                    title: Some("Ops runbook".into()),
                    url: Some("https://wiki.acme.example/ops".into()),
                    emoji: Patch::Set("📗".into()),
                },
            ),
            ("min", EditBookmarkRequest::default()),
        ]
    }
}

impl Example for ReorderBookmarksRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReorderBookmarksRequest {
                ids: vec![ID_BOOKMARK, id(36)],
            },
        )]
    }
}

impl Example for BookmarksQuery {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                BookmarksQuery {
                    ids: Some(ids(&[ID_BOOKMARK])),
                },
            ),
            ("min", BookmarksQuery::default()),
        ]
    }
}

impl Example for BookmarkDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            BookmarkDeleted {
                id: ID_BOOKMARK,
                channel_id: ID_CHANNEL,
                seq: 1_032,
            },
        )]
    }
}
