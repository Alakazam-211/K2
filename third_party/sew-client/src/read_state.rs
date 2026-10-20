//! S14 calls: acks, mark unread, read states, the threads inbox, change logs and `/me/sync`
//! (prd-sew-14 §9, §16).

use sew_proto::api::changes::{ChannelChangesPage, MemberChangesPage};
use sew_proto::api::messages::Message;
use sew_proto::api::read_state::{
    AckRequest, BulkAckRequest, BulkAckResponse, MarkUnreadRequest, MeSync, ReadState,
    ThreadInboxItem, ThreadReadState, ThreadReadUpdated, ThreadsInboxQuery,
};
use sew_proto::gateway::server::ThreadsInbox;
use sew_proto::http::Page;
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

fn ids(v: &[Id]) -> String {
    v.iter().map(Id::to_string).collect::<Vec<_>>().join(",")
}

fn query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
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

impl Client {
    /// `POST /api/v1/channels/{id}/ack`: move the cursor forward to `message_id` (absent = the
    /// newest message now).
    pub async fn channel_ack(
        &self,
        channel: Id,
        message_id: Option<Id>,
    ) -> Result<ReadState, ClientError> {
        self.post(
            &with_id(r::CHANNEL_ACK, channel),
            &AckRequest { message_id },
        )
        .await
    }

    /// `POST /api/v1/channels/{id}/mark_unread`: `message_id` and everything after it read as
    /// unread again.
    pub async fn channel_mark_unread(
        &self,
        channel: Id,
        message_id: Id,
    ) -> Result<ReadState, ClientError> {
        self.post(
            &with_id(r::CHANNEL_MARK_UNREAD, channel),
            &MarkUnreadRequest { message_id },
        )
        .await
    }

    /// `POST /api/v1/threads/{root_id}/ack`.
    pub async fn thread_ack(
        &self,
        root: Id,
        message_id: Option<Id>,
    ) -> Result<ThreadReadUpdated, ClientError> {
        self.post(&with_id(r::THREAD_ACK, root), &AckRequest { message_id })
            .await
    }

    /// `POST /api/v1/threads/{root_id}/mark_unread`.
    pub async fn thread_mark_unread(
        &self,
        root: Id,
        message_id: Id,
    ) -> Result<ThreadReadUpdated, ClientError> {
        self.post(
            &with_id(r::THREAD_MARK_UNREAD, root),
            &MarkUnreadRequest { message_id },
        )
        .await
    }

    /// `POST /api/v1/me/read_states/ack`: bulk ack ("mark all read").
    pub async fn me_read_states_ack(
        &self,
        req: &BulkAckRequest,
    ) -> Result<BulkAckResponse, ClientError> {
        self.post(r::ME_READ_STATES_ACK, req).await
    }

    /// `GET /api/v1/me/read_states`: the sync set's read states, or those of `channel_ids`
    /// (1–100).
    pub async fn me_read_states(
        &self,
        channel_ids: Option<&[Id]>,
    ) -> Result<Page<ReadState>, ClientError> {
        self.get(&query(
            r::ME_READ_STATES,
            &[("channel_ids", channel_ids.map(ids))],
        ))
        .await
    }

    /// `GET /api/v1/me/thread_reads?root_ids=` (1–100 roots).
    pub async fn me_thread_reads(
        &self,
        root_ids: &[Id],
    ) -> Result<Page<ThreadReadState>, ClientError> {
        self.get(&query(
            r::ME_THREAD_READS,
            &[("root_ids", Some(ids(root_ids)))],
        ))
        .await
    }

    /// `GET /api/v1/me/threads`: the threads inbox, newest activity first.
    pub async fn me_threads(
        &self,
        q: &ThreadsInboxQuery,
    ) -> Result<Page<ThreadInboxItem>, ClientError> {
        self.get(&query(
            r::ME_THREADS,
            &[
                ("before", q.before.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
                ("filter", q.filter.map(|f| f.as_str().to_owned())),
            ],
        ))
        .await
    }

    /// `GET /api/v1/me/threads/summary`: the threads inbox totals.
    pub async fn me_threads_summary(&self) -> Result<ThreadsInbox, ClientError> {
        self.get(r::ME_THREADS_SUMMARY).await
    }

    /// `GET /api/v1/channels/{id}/changes?since=&limit=`. Continue with the page's
    /// `next_since`.
    pub async fn channel_changes(
        &self,
        channel: Id,
        since: u64,
        limit: Option<u32>,
    ) -> Result<ChannelChangesPage, ClientError> {
        self.get(&query(
            &with_id(r::CHANNEL_CHANGES, channel),
            &[
                ("since", Some(since.to_string())),
                ("limit", limit.map(|l| l.to_string())),
            ],
        ))
        .await
    }

    /// `GET /api/v1/me/changes?since=&limit=`. Continue with the page's `next_since`.
    pub async fn me_changes(
        &self,
        since: u64,
        limit: Option<u32>,
    ) -> Result<MemberChangesPage, ClientError> {
        self.get(&query(
            r::ME_CHANGES,
            &[
                ("since", Some(since.to_string())),
                ("limit", limit.map(|l| l.to_string())),
            ],
        ))
        .await
    }

    /// `GET /api/v1/me/sync`: READY's sync section over HTTP.
    pub async fn me_sync(&self) -> Result<MeSync, ClientError> {
        self.get(r::ME_SYNC).await
    }

    /// `GET /api/v1/channels/{id}/messages?after=&wait_ms=`: messages after `after`; an empty
    /// page waits up to `wait_ms` (≤ 25,000) for a new one (contracts rev 31).
    pub async fn messages_wait(
        &self,
        channel: Id,
        after: Id,
        limit: Option<u32>,
        wait_ms: u32,
    ) -> Result<Page<Message>, ClientError> {
        self.get(&query(
            &with_id(r::CHANNEL_MESSAGES, channel),
            &[
                ("after", Some(after.to_string())),
                ("limit", limit.map(|l| l.to_string())),
                ("wait_ms", Some(wait_ms.to_string())),
            ],
        ))
        .await
    }

    /// `GET /api/v1/messages/{root_id}/replies?after=&wait_ms=`: replies after `after`, with
    /// the same long-poll.
    pub async fn thread_replies_wait(
        &self,
        root: Id,
        after: Id,
        limit: Option<u32>,
        wait_ms: u32,
    ) -> Result<Page<Message>, ClientError> {
        self.get(&query(
            &with_id(r::THREAD_MESSAGES, root),
            &[
                ("after", Some(after.to_string())),
                ("limit", limit.map(|l| l.to_string())),
                ("wait_ms", Some(wait_ms.to_string())),
            ],
        ))
        .await
    }
}
