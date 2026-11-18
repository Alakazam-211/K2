//! The reply-relevant slice of one message ([`ReplyContext`]) and its pure
//! `Email/get` parser, moved out of `jmap.rs` (0.46.15, which added the
//! original's To and Cc for `--reply-all`).

use super::{parse_addr_list, MailAddr, AUTH_RESULTS_PROP, MSGID_PROP, REFS_PROP};

/// The reply-relevant slice of one message (S5 `k2 mail reply`,
/// §8.4). Raw wire values only — verdict parsing (DMARC) and the
/// guardrail decisions live at the ops layer ([`crate::mail::send`]),
/// per the §17.5 route-layer rule.
#[derive(Debug, Clone)]
pub struct ReplyContext {
    pub from: Vec<MailAddr>,
    /// The original's To and Cc (`--reply-all` copies them, minus the
    /// replying mailbox).
    pub to: Vec<MailAddr>,
    pub cc: Vec<MailAddr>,
    /// The original's Reply-To (it is on the original too).
    pub reply_to: Vec<MailAddr>,
    pub subject: String,
    pub thread_id: Option<String>,
    /// The original's `Message-ID` header (trimmed), for In-Reply-To.
    pub message_id: Option<String>,
    /// The original's raw `References` header text (loop caps + the
    /// outgoing References chain).
    pub references: Option<String>,
    pub auth_results: Vec<String>,
}

/// Pure `Email/get` (reply-context) parser: `Ok(None)` when the server
/// put the id in `notFound`; loud when it names neither.
pub(super) fn parse_reply_context(
    email_id: &str,
    args: &serde_json::Value,
) -> Result<Option<ReplyContext>, String> {
    let not_found = args
        .get("notFound")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(email_id)))
        .unwrap_or(false);
    if not_found {
        return Ok(None);
    }
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(email_id))
        })
        .ok_or_else(|| {
            format!("Email/get: '{email_id}' in neither list nor notFound")
        })?;
    let header_text = |prop: &str| -> Option<String> {
        entry
            .get(prop)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let auth_results = entry
        .get(AUTH_RESULTS_PROP)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
        .unwrap_or_default();
    Ok(Some(ReplyContext {
        from: parse_addr_list(entry.get("from")),
        to: parse_addr_list(entry.get("to")),
        cc: parse_addr_list(entry.get("cc")),
        reply_to: parse_addr_list(entry.get("replyTo")),
        subject: entry
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        thread_id: entry
            .get("threadId")
            .and_then(|v| v.as_str())
            .map(String::from),
        message_id: header_text(MSGID_PROP),
        references: header_text(REFS_PROP),
        auth_results,
    }))
}
