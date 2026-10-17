//! The mail SEND seam (prd-app-mail-calendar-surfaces-v1 §3.1/§3.3, slice
//! MCS2): one [`MailSubmit`] per backend, chosen from the From address,
//! so the governed pipeline (audit row first, rate limits, the approval
//! queue) works the same for every inbox:
//!
//! | From inbox              | backend                                     |
//! |-------------------------|---------------------------------------------|
//! | hosted (K2-minted)      | [`HostedSubmit`] — JMAP `EmailSubmission`   |
//! | linked IMAP / Gmail     | [`SmtpSubmit`] — SMTP (app-password/XOAUTH2)|
//! | linked Microsoft 365    | [`GraphSubmit`] — Graph `sendMail` / reply  |
//!
//! [`submit_for`] resolves it; `approve_and_submit_with` (send.rs) calls it
//! at approval time, so an approved message from a LINKED inbox leaves
//! through the same provider a direct send would (the hand-off MCS-A5
//! asks for). Nothing here logs a body or a credential.

use crate::mail::external_smtp::{self, OutAttachment, RealSmtpOps};
use crate::mail::graph::{self, GraphAttachment, GraphSendError};
use crate::mail::send::OutboundMessage;
use k2_core::db::schema::MailExternalInbox;

/// What a submission handed to the provider (the outbox trail). Never the
/// body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub attachments: Vec<String>,
}

/// A submission failure. `Usage` = the caller's to fix (e.g.
/// `attachment_too_large`, attachments on a hosted inbox); `NotFound` =
/// the masked missing reply source; `Engine` = the provider said no.
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    Usage { code: &'static str, hint: String },
    NotFound(String),
    Engine(String),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Usage { hint, .. } => f.write_str(hint),
            SubmitError::NotFound(h) | SubmitError::Engine(h) => f.write_str(h),
        }
    }
}

impl From<GraphSendError> for SubmitError {
    fn from(e: GraphSendError) -> Self {
        match e {
            GraphSendError::TooLarge(h) => SubmitError::Usage {
                code: "attachment_too_large",
                hint: h,
            },
            GraphSendError::NotFound(h) => SubmitError::NotFound(h),
            GraphSendError::Engine(h) => SubmitError::Engine(h),
        }
    }
}

impl From<crate::mail::external::ExtError> for SubmitError {
    fn from(e: crate::mail::external::ExtError) -> Self {
        use crate::mail::external::ExtError;
        match e {
            ExtError::Usage(h) => SubmitError::Usage {
                code: "usage",
                hint: h,
            },
            ExtError::NotFound(h) => SubmitError::NotFound(h),
            ExtError::Exists(h) | ExtError::Engine(h) => SubmitError::Engine(h),
        }
    }
}

/// One backend's way to put a composed message on the wire. `msg` is the
/// governed, server-stamped message (From already owned; policy BCC
/// already applied); `attachments` were read + capped by the caller.
pub trait MailSubmit {
    /// `hosted` | `smtp` | `graph` (the audit trail + capabilities).
    fn backend(&self) -> &'static str;
    /// A fresh message.
    fn submit(
        &self,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError>;
    /// A reply to `source_email_id` (the backend's own message id). The
    /// provider re-locks the recipient to the source's sender.
    fn reply(
        &self,
        source_email_id: &str,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError>;
    /// Who a reply would go to + its subject, read from the source BEFORE
    /// anything is sent (so an approval shows the real recipient). `Ok(None)`
    /// = no such message.
    fn reply_preview(&self, source_email_id: &str)
        -> Result<Option<(String, String)>, SubmitError>;
}

// ── Hosted (local Stalwart, JMAP) ───────────────────────────────────────

/// The hosted backend: the existing [`crate::mail::send::SubmitBackend`]
/// (JMAP `EmailSubmission` on the local Stalwart) for one account. A
/// hosted message carries its threading headers itself, so a reply is a
/// submit.
pub struct HostedSubmit<'a> {
    pub backend: &'a dyn crate::mail::send::SubmitBackend,
    pub account_id: String,
}

impl MailSubmit for HostedSubmit<'_> {
    fn backend(&self) -> &'static str {
        "hosted"
    }
    fn submit(
        &self,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        if !attachments.is_empty() {
            return Err(SubmitError::Usage {
                code: "usage",
                hint: "attachments aren't supported yet when sending from a hosted K2 mailbox"
                    .to_string(),
            });
        }
        self.backend
            .submit(&self.account_id, msg)
            .map_err(SubmitError::Engine)?;
        Ok(Receipt {
            to: msg.to.clone(),
            cc: msg.cc.clone(),
            subject: msg.subject.clone(),
            attachments: Vec::new(),
        })
    }
    fn reply(
        &self,
        _source_email_id: &str,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        self.submit(msg, attachments)
    }
    fn reply_preview(
        &self,
        _source_email_id: &str,
    ) -> Result<Option<(String, String)>, SubmitError> {
        Err(SubmitError::Engine(
            "hosted replies are composed by the reply route, not previewed here".to_string(),
        ))
    }
}

// ── Linked IMAP / Gmail (SMTP submission) ───────────────────────────────

/// A linked IMAP inbox (app password or Gmail XOAUTH2) sending over SMTP
/// submission from the account itself.
pub struct SmtpSubmit {
    pub inbox: MailExternalInbox,
    /// The vaulted app password (empty for an OAuth row — the SMTP layer
    /// mints the XOAUTH2 token itself). Never logged.
    pub password: String,
}

impl MailSubmit for SmtpSubmit {
    fn backend(&self) -> &'static str {
        "smtp"
    }
    fn submit(
        &self,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        let r = external_smtp::send_linked_message(
            &RealSmtpOps,
            &self.inbox,
            &self.password,
            &msg.to,
            &msg.cc,
            &msg.all_bcc(),
            &msg.subject,
            &msg.text_body,
            attachments,
        )?;
        Ok(Receipt {
            to: r.to,
            cc: r.cc,
            subject: r.subject,
            attachments: r.attachments,
        })
    }
    fn reply(
        &self,
        source_email_id: &str,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        let r = external_smtp::send_linked_reply(
            &crate::mail::external_imap::RealImapOps,
            &RealSmtpOps,
            &self.inbox,
            &self.password,
            source_email_id,
            &msg.text_body,
            attachments,
            &msg.all_bcc(),
        )?;
        Ok(Receipt {
            to: r.to,
            cc: r.cc,
            subject: r.subject,
            attachments: r.attachments,
        })
    }
    fn reply_preview(
        &self,
        source_email_id: &str,
    ) -> Result<Option<(String, String)>, SubmitError> {
        use crate::mail::external::ImapOps as _;
        let raw = crate::mail::external_imap::RealImapOps
            .fetch_raw(&self.inbox, &self.password, source_email_id)
            .map_err(SubmitError::Engine)?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let src =
            crate::mail::external::draft_source_from_raw(&raw.raw).map_err(SubmitError::Engine)?;
        let Some(sender) = src.reply_to else {
            return Err(SubmitError::Usage {
                code: "usage",
                hint: "the source message has no sender address to reply to".to_string(),
            });
        };
        let to = crate::mail::addresses::normalize_address(&sender.email).map_err(|_| {
            SubmitError::Usage {
                code: "usage",
                hint: format!(
                    "the source sender's address ('{}') is not a replyable address",
                    sender.email
                ),
            }
        })?;
        Ok(Some((to, crate::mail::send::reply_subject(&src.subject))))
    }
}

// ── Linked Microsoft 365 (Graph) ────────────────────────────────────────

/// A linked Microsoft 365 inbox sending over Microsoft Graph (`sendMail`,
/// `createReply` → `send`). Needs `Mail.Send` (MCS1 scopes).
pub struct GraphSubmit {
    pub backend: graph::GraphBackend,
    pub inbox_id: String,
}

fn graph_atts(attachments: &[OutAttachment]) -> Vec<GraphAttachment<'_>> {
    attachments
        .iter()
        .map(|a| GraphAttachment {
            filename: &a.filename,
            content_type: &a.content_type,
            bytes: &a.bytes,
        })
        .collect()
}

impl MailSubmit for GraphSubmit {
    fn backend(&self) -> &'static str {
        "graph"
    }
    fn submit(
        &self,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        let r = self.backend.send_mail(
            &self.inbox_id,
            &msg.to,
            &msg.cc,
            &msg.all_bcc(),
            &msg.subject,
            &msg.text_body,
            &graph_atts(attachments),
        );
        // Inbox health: a provider failure stamps the row like a bad read;
        // the caller's own mistakes (too large, no source) don't.
        match &r {
            Ok(_) => crate::mail::external::record_check(&self.inbox_id, Ok(())),
            Err(GraphSendError::Engine(h)) => {
                crate::mail::external::record_check(&self.inbox_id, Err(h.as_str()))
            }
            Err(_) => {}
        }
        let r = r?;
        Ok(Receipt {
            to: r.to,
            cc: r.cc,
            subject: r.subject,
            attachments: r.attachments,
        })
    }
    fn reply(
        &self,
        source_email_id: &str,
        msg: &OutboundMessage,
        attachments: &[OutAttachment],
    ) -> Result<Receipt, SubmitError> {
        let r = self.backend.send_reply(
            &self.inbox_id,
            source_email_id,
            &msg.text_body,
            &msg.all_bcc(),
            &graph_atts(attachments),
        )?;
        Ok(Receipt {
            to: r.to,
            cc: r.cc,
            subject: r.subject,
            attachments: r.attachments,
        })
    }
    fn reply_preview(
        &self,
        source_email_id: &str,
    ) -> Result<Option<(String, String)>, SubmitError> {
        self.backend
            .reply_preview(&self.inbox_id, source_email_id)
            .map_err(SubmitError::Engine)
    }
}

// ── Resolution ───────────────────────────────────────────────────────────

/// The vaulted app password of a linked row, or empty for an OAuth row.
pub fn linked_password(inbox: &MailExternalInbox) -> Result<String, String> {
    use crate::mail::secrets::SecretStore as _;
    let is_oauth = matches!(
        crate::mail::external::read_oauth_fields(&inbox.id),
        Ok(f) if f.auth_kind == crate::mail::external::AUTH_OAUTH
    );
    if is_oauth {
        return Ok(String::new());
    }
    match crate::mail::secrets::FileSecretStore::default()
        .resolve(&crate::mail::external::vault_key(&inbox.id))
    {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(format!(
            "credentials for '{}' are missing from the vault — your human can reconnect it with \
             'k2 mail link add'",
            inbox.email_address
        )),
        Err(e) => Err(e),
    }
}

/// The send backend for a LINKED inbox row: Graph for `kind='graph'`,
/// SMTP submission for everything else.
pub fn submit_for_linked(inbox: &MailExternalInbox) -> Result<Box<dyn MailSubmit>, String> {
    if inbox.kind == "graph" {
        let http = std::sync::Arc::new(graph::RealGraphHttp::new(inbox.id.clone()));
        return Ok(Box::new(GraphSubmit {
            backend: graph::GraphBackend::new(inbox.clone(), http),
            inbox_id: inbox.id.clone(),
        }));
    }
    let password = linked_password(inbox)?;
    Ok(Box::new(SmtpSubmit {
        inbox: inbox.clone(),
        password,
    }))
}
