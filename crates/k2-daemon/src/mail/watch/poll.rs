//! Finding new mail without touching it (prd §4.6, vs-live W19, W20, W30).
//! One poller per inbox kind, each peek-only: nothing here may set `\Seen`.
//!
//! A first poll only sets the cursor, so a watcher turned on today isn't
//! flooded with last year's mail. A failure is either Auth (stop until the
//! credential changes) or Transient (back off): a revoked app password
//! retried every two minutes is how a whole IP gets banned.

use crate::mail::jmap::MailAddr;
use crate::mail::messages::AuthAuthority;
use crate::mail::watch::store::Cursor;

/// One new message, as much as classification needs. Subject and snippet are
/// kept in memory for this pass only.
#[derive(Debug, Clone)]
pub struct NewMail {
    /// The backend's message id (`email_id`).
    pub email_id: String,
    pub rfc_message_id: String,
    pub from: Vec<MailAddr>,
    pub subject: String,
    pub received_at: String,
    pub auth_lines: Vec<String>,
    pub auto_submitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollError {
    /// The credential was refused: stop polling this inbox until it changes.
    Auth(String),
    /// Network, 5xx, 429: try again later.
    Transient(String),
}

/// Whether a failure message says the credential itself was refused.
pub fn is_auth_failure(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    [
        "authenticationfailed",
        "[auth",
        "invalid credentials",
        "bad credentials",
        "invalid_grant",
        "invalidauthenticationtoken",
        "authentication failed",
        "unauthorized",
        "no response",
        "re-link",
    ]
    .iter()
    .any(|needle| m.contains(needle))
        || has_status_401(&m)
}

/// `401` as a status, not as part of a longer number (a port, a size).
fn has_status_401(m: &str) -> bool {
    let bytes = m.as_bytes();
    m.match_indices("401").any(|(i, _)| {
        let before = i == 0 || !bytes[i - 1].is_ascii_digit();
        let after = i + 3 >= bytes.len() || !bytes[i + 3].is_ascii_digit();
        before && after
    })
}

pub fn classify_failure(msg: String) -> PollError {
    if is_auth_failure(&msg) {
        PollError::Auth(msg)
    } else {
        PollError::Transient(msg)
    }
}

/// One inbox's source of new mail.
pub trait Poller {
    /// Move the cursor and return what's new since it. On the first poll the
    /// cursor is set and nothing is returned.
    fn poll(&mut self, cursor: &mut Cursor) -> Result<Vec<NewMail>, PollError>;
    /// Whose Authentication-Results this inbox's mail can be read by.
    fn authority(&self) -> AuthAuthority;
    /// When the next poll is worth doing, in seconds.
    fn interval_secs(&self) -> i64;
}

// ── Hosted (JMAP) ───────────────────────────────────────────────────────

/// A hosted inbox on this server's Stalwart. `Email/changes` from the stored
/// state (W19), falling back to a date query when the server can't compute
/// changes. Only Inbox mail counts: junk stays out.
pub struct HostedPoller {
    pub client: crate::mail::jmap::StalwartClient,
    pub mail_host: Option<String>,
    pub account_id: String,
    pub inbox_id: Option<String>,
}

impl Poller for HostedPoller {
    fn authority(&self) -> AuthAuthority {
        AuthAuthority::ThisBox(self.mail_host.clone())
    }

    fn interval_secs(&self) -> i64 {
        60
    }

    fn poll(&mut self, cursor: &mut Cursor) -> Result<Vec<NewMail>, PollError> {
        let inbox_id = match &self.inbox_id {
            Some(id) => id.clone(),
            None => {
                let id = self.client.mailbox_inbox_id(&self.account_id).map_err(classify_failure)?;
                self.inbox_id = Some(id.clone());
                id
            }
        };
        let Some(state) = cursor.jmap_state.clone() else {
            // First poll: the state of now is the cursor. Nothing is news.
            let state = self.client.email_state(&self.account_id).map_err(classify_failure)?;
            cursor.jmap_state = Some(state);
            cursor.last_received_at = Some(now_rfc3339());
            return Ok(Vec::new());
        };
        let ids = match self.client.email_changes(&self.account_id, &state, 256) {
            Ok(changes) => {
                cursor.jmap_state = Some(changes.new_state);
                changes.created
            }
            Err(e) if e.contains("cannotCalculateChanges") => {
                let since = cursor.last_received_at.clone().unwrap_or_else(now_rfc3339);
                let ids = self
                    .client
                    .email_query_ids(
                        &self.account_id,
                        serde_json::json!({ "inMailbox": inbox_id, "after": since }),
                        256,
                        0,
                    )
                    .map_err(classify_failure)?;
                cursor.jmap_state = Some(self.client.email_state(&self.account_id).map_err(classify_failure)?);
                ids
            }
            Err(e) => return Err(classify_failure(e)),
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = self.client.email_get_watch(&self.account_id, &ids, &inbox_id).map_err(classify_failure)?;
        out.sort_by(|a, b| a.received_at.cmp(&b.received_at));
        if let Some(last) = out.last() {
            cursor.last_received_at = Some(last.received_at.clone());
        }
        Ok(out)
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ── Linked (IMAP) ───────────────────────────────────────────────────────

/// A linked IMAP inbox. One logged-in session is kept between polls (W20);
/// a dropped one reconnects on the next poll, with the credential's verdict
/// from a fresh login.
pub struct ImapPoller {
    pub inbox: k2_core::db::schema::MailExternalInbox,
    pub password: String,
    pub session: Option<crate::mail::external_imap::ImapSession>,
}

impl Poller for ImapPoller {
    fn authority(&self) -> AuthAuthority {
        AuthAuthority::Provider
    }

    fn interval_secs(&self) -> i64 {
        120
    }

    fn poll(&mut self, cursor: &mut Cursor) -> Result<Vec<NewMail>, PollError> {
        let mut session = match self.session.take() {
            Some(s) => s,
            None => crate::mail::external_imap::watch_open(&self.inbox, &self.password).map_err(classify_failure)?,
        };
        let polled = crate::mail::external_imap::watch_poll(&mut session, cursor);
        match polled {
            Ok(out) => {
                self.session = Some(session);
                Ok(out)
            }
            // A kept session the server dropped: once, a fresh login. A bad
            // credential answers here, as Auth.
            Err(_) => {
                let mut fresh = crate::mail::external_imap::watch_open(&self.inbox, &self.password).map_err(classify_failure)?;
                let out = crate::mail::external_imap::watch_poll(&mut fresh, cursor).map_err(PollError::Transient)?;
                self.session = Some(fresh);
                Ok(out)
            }
        }
    }
}

// ── Linked (Graph) ──────────────────────────────────────────────────────

/// A Microsoft inbox over Graph. There is no change cursor here: a date
/// cursor on `receivedDateTime` plus the notice store's dedupe by message id.
pub struct GraphPoller {
    pub backend: crate::mail::graph::GraphBackend,
    pub account_id: String,
}

impl Poller for GraphPoller {
    fn authority(&self) -> AuthAuthority {
        AuthAuthority::Provider
    }

    fn interval_secs(&self) -> i64 {
        120
    }

    fn poll(&mut self, cursor: &mut Cursor) -> Result<Vec<NewMail>, PollError> {
        use crate::mail::messages::{ListError, ListFilter, ReadBackend};
        let Some(since) = cursor.last_received_at.clone() else {
            cursor.last_received_at = Some(now_rfc3339());
            return Ok(Vec::new());
        };
        let since_unix = chrono::DateTime::parse_from_rfc3339(&since)
            .map(|t| t.timestamp())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp());
        let filter = ListFilter { since_unix: Some(since_unix), ..Default::default() };
        let listed = self.backend.list_inbox(&self.account_id, &filter, 100).map_err(|e| match e {
            ListError::Usage(m) | ListError::Engine(m) => classify_failure(m),
            ListError::UnknownFolder { requested, .. } => PollError::Transient(format!("unknown folder {requested}")),
        })?;
        let mut out = Vec::new();
        let mut newest = since.clone();
        for s in listed {
            if s.received_at <= since {
                continue;
            }
            if s.received_at > newest {
                newest = s.received_at.clone();
            }
            let fields = self.backend.watch_fields(&self.account_id, &s.id).map_err(classify_failure)?;
            let (rfc_message_id, auth_lines, auto_submitted) = fields.unwrap_or_default();
            out.push(NewMail {
                email_id: s.id,
                rfc_message_id,
                from: s.from,
                subject: s.subject,
                received_at: s.received_at,
                auth_lines,
                auto_submitted,
            });
        }
        cursor.last_received_at = Some(newest);
        out.sort_by(|a, b| a.received_at.cmp(&b.received_at));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failures_are_told_from_transient_ones() {
        for m in [
            "login: No Response: [AUTHENTICATIONFAILED] Invalid credentials",
            "oauth refresh: invalid_grant",
            "POST /me: 401 Unauthorized",
            "GET: InvalidAuthenticationToken",
        ] {
            assert!(matches!(classify_failure(m.to_string()), PollError::Auth(_)), "{m}");
        }
        assert!(matches!(classify_failure("connect 127.0.0.1:4010 refused".to_string()), PollError::Transient(_)), "a port is not a status");
        for m in ["connect: connection refused", "FETCH: timed out", "POST: 503 Service Unavailable", "429 throttled"] {
            assert!(matches!(classify_failure(m.to_string()), PollError::Transient(_)), "{m}");
        }
    }
}
