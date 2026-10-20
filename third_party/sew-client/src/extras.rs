//! S17 calls: drafts, saved items, scheduled messages, reminders, polls and channel bookmarks
//! (prd-sew-17 §14).

use sew_proto::api::extras::{
    Bookmark, BookmarksQuery, CreateBookmarkRequest, CreateReminderRequest, CreateScheduledRequest,
    DeleteDraftRequest, DeletedRef, Draft, DraftsQuery, EditBookmarkRequest, EditReminderRequest,
    EditSavedRequest, EditScheduledRequest, PollVoteRequest, PollVoter, PollVotersQuery, Reminder,
    RemindersQuery, ReorderBookmarksRequest, SaveDraftRequest, SaveMessageRequest, SavedItem,
    SavedQuery, ScheduledMessage, ScheduledQuery, SnoozeReminderRequest,
};
use sew_proto::api::messages::Message;
use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::{Id, IdList};
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let q: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={v}")))
        .collect();
    if q.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", q.join("&"))
    }
}

fn ids(l: &Option<IdList>) -> Option<String> {
    l.as_ref().map(|l| {
        l.ids()
            .iter()
            .map(Id::to_string)
            .collect::<Vec<_>>()
            .join(",")
    })
}

impl Client {
    // ---------------------------------------------------------------- drafts

    /// `POST /api/v1/channels/{id}/draft`: save the caller's draft.
    pub async fn save_draft(
        &self,
        channel: Id,
        req: &SaveDraftRequest,
    ) -> Result<Draft, ClientError> {
        self.post(&with_id(r::CHANNEL_DRAFT, channel), req).await
    }

    /// `POST /api/v1/channels/{id}/draft/delete`.
    pub async fn delete_draft(
        &self,
        channel: Id,
        req: &DeleteDraftRequest,
    ) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::CHANNEL_DRAFT_DELETE, channel), req)
            .await
    }

    /// `GET /api/v1/me/drafts`.
    pub async fn drafts(&self, q: &DraftsQuery) -> Result<Page<Draft>, ClientError> {
        self.get(&with_query(
            r::ME_DRAFTS,
            &[
                ("ids", ids(&q.ids)),
                ("channel_id", q.channel_id.map(|c| c.to_string())),
            ],
        ))
        .await
    }

    // ---------------------------------------------------------------- saved items

    /// `POST /api/v1/messages/{id}/save`.
    pub async fn save_message(
        &self,
        message: Id,
        req: &SaveMessageRequest,
    ) -> Result<SavedItem, ClientError> {
        self.post(&with_id(r::MESSAGE_SAVE, message), req).await
    }

    /// `POST /api/v1/messages/{id}/unsave`.
    pub async fn unsave_message(&self, message: Id) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::MESSAGE_UNSAVE, message), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/saved/{id}/edit`.
    pub async fn edit_saved(
        &self,
        saved: Id,
        req: &EditSavedRequest,
    ) -> Result<SavedItem, ClientError> {
        self.post(&with_id(r::SAVED_EDIT, saved), req).await
    }

    /// `POST /api/v1/saved/{id}/delete`.
    pub async fn delete_saved(&self, saved: Id) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::SAVED_DELETE, saved), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/me/saved`.
    pub async fn saved(&self, q: &SavedQuery) -> Result<Page<SavedItem>, ClientError> {
        self.get(&with_query(
            r::ME_SAVED,
            &[
                ("state", q.state.map(|s| s.as_str().to_owned())),
                ("ids", ids(&q.ids)),
                ("before", q.before.map(|v| v.to_string())),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    // ---------------------------------------------------------------- scheduled messages

    /// `POST /api/v1/channels/{id}/scheduled`.
    pub async fn schedule_message(
        &self,
        channel: Id,
        req: &CreateScheduledRequest,
    ) -> Result<ScheduledMessage, ClientError> {
        self.post(&with_id(r::CHANNEL_SCHEDULED, channel), req)
            .await
    }

    /// `POST /api/v1/scheduled/{id}/edit`.
    pub async fn edit_scheduled(
        &self,
        id: Id,
        req: &EditScheduledRequest,
    ) -> Result<ScheduledMessage, ClientError> {
        self.post(&with_id(r::SCHEDULED_EDIT, id), req).await
    }

    /// `POST /api/v1/scheduled/{id}/send_now`.
    pub async fn send_scheduled_now(&self, id: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::SCHEDULED_SEND_NOW, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/scheduled/{id}/delete`.
    pub async fn delete_scheduled(&self, id: Id) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::SCHEDULED_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/me/scheduled`.
    pub async fn scheduled(
        &self,
        q: &ScheduledQuery,
    ) -> Result<Page<ScheduledMessage>, ClientError> {
        self.get(&with_query(
            r::ME_SCHEDULED,
            &[
                ("state", q.state.map(|s| s.as_str().to_owned())),
                ("channel_id", q.channel_id.map(|c| c.to_string())),
                ("ids", ids(&q.ids)),
            ],
        ))
        .await
    }

    // ---------------------------------------------------------------- reminders

    /// `POST /api/v1/me/reminders`.
    pub async fn create_reminder(
        &self,
        req: &CreateReminderRequest,
    ) -> Result<Reminder, ClientError> {
        self.post(r::ME_REMINDERS, req).await
    }

    /// `GET /api/v1/me/reminders`.
    pub async fn reminders(&self, q: &RemindersQuery) -> Result<Page<Reminder>, ClientError> {
        self.get(&with_query(
            r::ME_REMINDERS,
            &[
                ("state", q.state.map(|s| s.as_str().to_owned())),
                ("ids", ids(&q.ids)),
                ("before", q.before.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    /// `POST /api/v1/reminders/{id}/edit`.
    pub async fn edit_reminder(
        &self,
        id: Id,
        req: &EditReminderRequest,
    ) -> Result<Reminder, ClientError> {
        self.post(&with_id(r::REMINDER_EDIT, id), req).await
    }

    /// `POST /api/v1/reminders/{id}/snooze`.
    pub async fn snooze_reminder(
        &self,
        id: Id,
        req: &SnoozeReminderRequest,
    ) -> Result<Reminder, ClientError> {
        self.post(&with_id(r::REMINDER_SNOOZE, id), req).await
    }

    /// `POST /api/v1/reminders/{id}/complete`.
    pub async fn complete_reminder(&self, id: Id) -> Result<Reminder, ClientError> {
        self.post(&with_id(r::REMINDER_COMPLETE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/reminders/{id}/delete`.
    pub async fn delete_reminder(&self, id: Id) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::REMINDER_DELETE, id), &EmptyRequest {})
            .await
    }

    // ---------------------------------------------------------------- polls

    /// `POST /api/v1/messages/{id}/poll/vote`: the set replaces the caller's selection.
    pub async fn poll_vote(&self, message: Id, option_ids: &[u8]) -> Result<Message, ClientError> {
        self.post(
            &with_id(r::MESSAGE_POLL_VOTE, message),
            &PollVoteRequest {
                option_ids: option_ids.to_vec(),
            },
        )
        .await
    }

    /// `POST /api/v1/messages/{id}/poll/close`.
    pub async fn poll_close(&self, message: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_POLL_CLOSE, message), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/messages/{id}/poll/voters`.
    pub async fn poll_voters(
        &self,
        message: Id,
        q: &PollVotersQuery,
    ) -> Result<Page<PollVoter>, ClientError> {
        self.get(&with_query(
            &with_id(r::MESSAGE_POLL_VOTERS, message),
            &[
                ("option_id", Some(q.option_id.to_string())),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    // ---------------------------------------------------------------- bookmarks

    /// `GET /api/v1/channels/{id}/bookmarks`.
    pub async fn bookmarks(
        &self,
        channel: Id,
        q: &BookmarksQuery,
    ) -> Result<Page<Bookmark>, ClientError> {
        self.get(&with_query(
            &with_id(r::CHANNEL_BOOKMARKS, channel),
            &[("ids", ids(&q.ids))],
        ))
        .await
    }

    /// `POST /api/v1/channels/{id}/bookmarks`.
    pub async fn create_bookmark(
        &self,
        channel: Id,
        req: &CreateBookmarkRequest,
    ) -> Result<Bookmark, ClientError> {
        self.post(&with_id(r::CHANNEL_BOOKMARKS, channel), req)
            .await
    }

    /// `POST /api/v1/bookmarks/{id}/edit`.
    pub async fn edit_bookmark(
        &self,
        id: Id,
        req: &EditBookmarkRequest,
    ) -> Result<Bookmark, ClientError> {
        self.post(&with_id(r::BOOKMARK_EDIT, id), req).await
    }

    /// `POST /api/v1/bookmarks/{id}/delete`.
    pub async fn delete_bookmark(&self, id: Id) -> Result<DeletedRef, ClientError> {
        self.post(&with_id(r::BOOKMARK_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/channels/{id}/bookmarks/reorder`.
    pub async fn reorder_bookmarks(
        &self,
        channel: Id,
        ids: &[Id],
    ) -> Result<Page<Bookmark>, ClientError> {
        self.post(
            &with_id(r::CHANNEL_BOOKMARKS_REORDER, channel),
            &ReorderBookmarksRequest { ids: ids.to_vec() },
        )
        .await
    }
}
