//! S13 calls: messages, threads, reactions, pins, history (prd-sew-13 §9; contracts rev 22
//! names the reaction and reply routes).

use sew_proto::api::messages::{
    CreateMessageRequest, EditMessageRequest, EmojiInput, ForwardMessageRequest, Message,
    MessageVersion, ReactionRequest, Reactor, ReactorsQuery, RemoveReactionRequest, ThreadSummary,
};
use sew_proto::http::{EmptyRequest, Page, PageQuery};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

/// Percent-encodes a query value (RFC 3986 unreserved characters stay).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let q: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", enc(v))))
        .collect();
    if q.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", q.join("&"))
    }
}

fn page_pairs(q: &PageQuery) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("before", q.before.map(|v| v.to_string())),
        ("after", q.after.map(|v| v.to_string())),
        ("around", q.around.map(|v| v.to_string())),
        ("limit", q.limit.map(|v| v.to_string())),
    ]
}

impl Client {
    /// `POST /api/v1/channels/{id}/messages`: post, reply, quote.
    pub async fn post_message(
        &self,
        channel: Id,
        req: &CreateMessageRequest,
    ) -> Result<Message, ClientError> {
        self.post(&with_id(r::CHANNEL_MESSAGES, channel), req).await
    }

    /// `GET /api/v1/channels/{id}/messages`: the channel timeline.
    pub async fn messages(&self, channel: Id, q: &PageQuery) -> Result<Page<Message>, ClientError> {
        self.get(&with_query(
            &with_id(r::CHANNEL_MESSAGES, channel),
            &page_pairs(q),
        ))
        .await
    }

    /// `GET /api/v1/messages?ids=`: live messages and tombstones; hidden ids are left out.
    pub async fn messages_by_ids(&self, ids: &[Id]) -> Result<Page<Message>, ClientError> {
        let list: Vec<String> = ids.iter().map(Id::to_string).collect();
        self.get(&format!("{}?ids={}", r::MESSAGES_BY_IDS, list.join(",")))
            .await
    }

    /// `POST /api/v1/messages/{id}/edit`.
    pub async fn edit_message(&self, id: Id, body: &str) -> Result<Message, ClientError> {
        self.post(
            &with_id(r::MESSAGE_EDIT, id),
            &EditMessageRequest {
                body: body.to_owned(),
            },
        )
        .await
    }

    /// `POST /api/v1/messages/{id}/delete`: returns the tombstone.
    pub async fn delete_message(&self, id: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/messages/{id}/history`: previous versions, newest first.
    pub async fn message_history(
        &self,
        id: Id,
        q: &PageQuery,
    ) -> Result<Page<MessageVersion>, ClientError> {
        self.get(&with_query(
            &with_id(r::MESSAGE_HISTORY, id),
            &page_pairs(q),
        ))
        .await
    }

    /// `POST /api/v1/messages/{id}/forward`.
    pub async fn forward_message(
        &self,
        id: Id,
        req: &ForwardMessageRequest,
    ) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_FORWARD, id), req).await
    }

    /// `POST /api/v1/messages/{id}/reactions/add`.
    pub async fn add_reaction(&self, id: Id, emoji: &EmojiInput) -> Result<Message, ClientError> {
        self.post(
            &with_id(r::MESSAGE_REACTIONS_ADD, id),
            &ReactionRequest {
                emoji: emoji.clone(),
            },
        )
        .await
    }

    /// `POST /api/v1/messages/{id}/reactions/remove` (another member's needs MANAGE_MESSAGES).
    pub async fn remove_reaction(
        &self,
        id: Id,
        req: &RemoveReactionRequest,
    ) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_REACTIONS_REMOVE, id), req)
            .await
    }

    /// `GET /api/v1/messages/{id}/reactors`.
    pub async fn reactors(&self, id: Id, q: &ReactorsQuery) -> Result<Page<Reactor>, ClientError> {
        self.get(&with_query(
            &with_id(r::MESSAGE_REACTORS, id),
            &[
                ("emoji", Some(q.emoji.clone())),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    /// `POST /api/v1/messages/{id}/pin`.
    pub async fn pin_message(&self, id: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_PIN, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/messages/{id}/unpin`.
    pub async fn unpin_message(&self, id: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_UNPIN, id), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/channels/{id}/pins`.
    pub async fn channel_pins(
        &self,
        channel: Id,
        q: &PageQuery,
    ) -> Result<Page<Message>, ClientError> {
        self.get(&with_query(
            &with_id(r::CHANNEL_PINS, channel),
            &page_pairs(q),
        ))
        .await
    }

    /// `GET /api/v1/messages/{root_id}/replies`: a thread's live replies.
    pub async fn thread_replies(
        &self,
        root: Id,
        q: &PageQuery,
    ) -> Result<Page<Message>, ClientError> {
        self.get(&with_query(
            &with_id(r::THREAD_MESSAGES, root),
            &page_pairs(q),
        ))
        .await
    }

    /// `POST /api/v1/threads/{root_id}/follow`.
    pub async fn follow_thread(&self, root: Id) -> Result<ThreadSummary, ClientError> {
        self.post(&with_id(r::THREAD_FOLLOW, root), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/threads/{root_id}/unfollow`.
    pub async fn unfollow_thread(&self, root: Id) -> Result<ThreadSummary, ClientError> {
        self.post(&with_id(r::THREAD_UNFOLLOW, root), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/threads/{root_id}/delete`: a whole-thread delete (MANAGE_THREADS).
    pub async fn delete_thread(&self, root: Id) -> Result<Message, ClientError> {
        self.post(&with_id(r::THREAD_DELETE, root), &EmptyRequest {})
            .await
    }
}
