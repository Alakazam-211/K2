//! LINKED-inbox SEND over SMTP submission (prd-email-server-v1 §17.5 —
//! the opt-in send path that complements the S9 read/draft backend).
//!
//! A workspace granted the 'send' level on a LINKED inbox may send FROM
//! that external account: K2 submits the composed message to the
//! account's own SMTP server, authenticated with the SAME vaulted
//! app-password used for IMAP (`ext-inbox-<row-id>` — never a column,
//! never logged). This is the ONLY module that speaks SMTP wire
//! protocol; everything above talks to the [`SmtpOps`] trait, so ops +
//! routes unit-test with fakes and a scripted loopback mock (127.0.0.1
//! ephemeral port, plaintext — the production path's TLS is rustls-with-
//! verification and is deliberately not mockable, exactly like the IMAP
//! client).
//!
//! GOVERNANCE: linked send is currently UNGATED (Rosson's contract —
//! unified gating for linked arrives with the wider email layer). The
//! access layer's `can_send` decides *whether* a workspace may send from
//! a linked inbox (effective level == 'send'); this module only performs
//! the submission once that check has passed. The obvious future home
//! for a per-message gate is the route branch that calls
//! [`send_linked_message`] / [`send_linked_reply`].
//!
//! ⚠ LIVE-BOX functions (genuinely-uncertain live-server behavior —
//! verify against real providers on a live box before trusting edge
//! cases):
//!   1. [`derive_smtp_route`] — the provider/IMAP-host → SMTP submission
//!      server mapping. Gmail REQUIRES the envelope/From to match the
//!      authenticated user (we always send From = the linked address);
//!      submission-port + STARTTLS-vs-implicit-TLS conventions vary by
//!      provider and by generic `imap.<domain>` hosts. An explicit
//!      `--smtp-host/--smtp-port/--smtp-tls` override sidesteps the whole
//!      guess for a non-conforming host.
//!   2. [`RealSmtpOps::submit`] — the live lettre-over-rustls submission
//!      (AUTH mechanism selection, STARTTLS upgrade, SUBMISSION vs SMTP
//!      port behavior) can only be exercised against a real provider;
//!      the loopback mock covers the plaintext conversation shape.
//!
//! Error strings surface transport/server text only — never the
//! password, never the message body (pre-mortem #16, mirrored from the
//! IMAP path).

use crate::mail::addresses;
use crate::mail::external::{self, ExtError, ImapOps, TLS_IMPLICIT, TLS_STARTTLS};
use crate::mail::jmap::MailAddr;
use k2_core::db::schema::MailExternalInbox;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::{SmtpTransport, SmtpTransportBuilder};
use lettre::{Address, Transport};

// ── The derived submission route ─────────────────────────────────────────

/// SMTP transport security. Implicit TLS wraps the socket from byte one
/// (submission port 465); STARTTLS upgrades a plaintext connection
/// (submission port 587). Plaintext-forever is never a variant — TLS is
/// not optional (the loopback mock's plaintext transport is test-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    ImplicitTls,
    StartTls,
}

/// A resolved SMTP submission target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpRoute {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
}

fn smtp_err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

/// The default submission port for a security kind.
fn default_port(security: SmtpSecurity) -> u16 {
    match security {
        SmtpSecurity::ImplicitTls => 465,
        SmtpSecurity::StartTls => 587,
    }
}

// ── ⚠ LIVE-BOX #1: provider/IMAP-host → SMTP submission server ──────────

/// Resolve the SMTP submission route for a linked inbox: an explicit
/// override (any of `smtp_host`/`smtp_port`/`smtp_tls`) wins field-by-
/// field over the provider derivation.
///
/// Derivation table (from the IMAP host, case-insensitive):
///   imap.gmail.com                    → smtp.gmail.com:587 STARTTLS
///   outlook.office365.com             → smtp.office365.com:587 STARTTLS
///   imap-mail.outlook.com             → smtp.office365.com:587 STARTTLS
///   imap.fastmail.com                 → smtp.fastmail.com:465 implicit-TLS
///   imap.<domain> (generic)           → smtp.<domain>:587 STARTTLS
///   <anything else>                   → <same host>:587 STARTTLS
pub fn derive_smtp_route(inbox: &MailExternalInbox) -> Result<SmtpRoute, String> {
    // Base derivation from the IMAP host / known providers.
    let imap_host = inbox.host.trim().to_ascii_lowercase();
    let (mut host, mut security) = match imap_host.as_str() {
        "imap.gmail.com" => ("smtp.gmail.com".to_string(), SmtpSecurity::StartTls),
        "outlook.office365.com" | "imap-mail.outlook.com" => {
            ("smtp.office365.com".to_string(), SmtpSecurity::StartTls)
        }
        "imap.fastmail.com" => ("smtp.fastmail.com".to_string(), SmtpSecurity::ImplicitTls),
        other => {
            let derived = other
                .strip_prefix("imap.")
                .map(|rest| format!("smtp.{rest}"))
                .unwrap_or_else(|| other.to_string());
            (derived, SmtpSecurity::StartTls)
        }
    };
    let mut port = default_port(security);

    // Field-by-field explicit overrides (validated at add time; a stored
    // value that somehow drifted is a loud error, never a silent guess).
    if let Some(h) = inbox.smtp_host.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        host = h.to_ascii_lowercase();
    }
    if let Some(t) = inbox.smtp_tls.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        security = match t {
            TLS_IMPLICIT => SmtpSecurity::ImplicitTls,
            TLS_STARTTLS => SmtpSecurity::StartTls,
            other => return Err(format!("stored smtp_tls '{other}' is not implicit-tls/starttls")),
        };
        // A tls override without an explicit port re-defaults the port.
        port = default_port(security);
    }
    if let Some(p) = inbox.smtp_port {
        if !(1..=65535).contains(&p) {
            return Err(format!("stored smtp_port {p} is out of range"));
        }
        port = p as u16;
    }
    if host.is_empty() {
        return Err("could not resolve an SMTP host for this inbox".to_string());
    }
    Ok(SmtpRoute { host, port, security })
}

// ── The SMTP effect seam (production: RealSmtpOps) ──────────────────────

/// The one SMTP submission effect, behind a trait so the ops/routes
/// test with fakes. `recipients` = every envelope recipient (to + cc);
/// `from` = the envelope MAIL FROM (always the linked address, so a
/// provider that pins From to the authenticated user is satisfied).
pub trait SmtpOps: Send + Sync {
    fn submit(
        &self,
        route: &SmtpRoute,
        username: &str,
        password: &str,
        from: &str,
        recipients: &[String],
        rfc822: &[u8],
    ) -> Result<(), String>;
}

/// Build the RFC 5321 envelope (validates the addresses). The recipient
/// list may ride the error text (recipients aren't secret to the
/// caller); the password never does.
fn build_envelope(from: &str, recipients: &[String]) -> Result<Envelope, String> {
    let from_addr: Address = from.parse().map_err(|e| smtp_err("from address", e))?;
    let mut to = Vec::with_capacity(recipients.len());
    for r in recipients {
        to.push(r.parse::<Address>().map_err(|e| smtp_err("recipient address", e))?);
    }
    Envelope::new(Some(from_addr), to).map_err(|e| smtp_err("envelope", e))
}

/// Finish a transport off a prepared builder and submit the raw message.
/// Shared by production (TLS builders) and the test loopback (plaintext
/// builder) so the send_raw wiring is exercised identically.
fn submit_with_builder(
    builder: SmtpTransportBuilder,
    port: u16,
    creds: Credentials,
    envelope: &Envelope,
    rfc822: &[u8],
) -> Result<(), String> {
    let mailer = builder.port(port).credentials(creds).build();
    mailer
        .send_raw(envelope, rfc822)
        .map(|_| ())
        .map_err(|e| smtp_err("SMTP submit", e))
}

/// Production [`SmtpOps`] — lettre over rustls. TLS verification is
/// ALWAYS on: `relay`/`starttls_relay` build a rustls-backed transport
/// against the platform root store; there is no code path that relaxes
/// it (`builder_dangerous` is `#[cfg(test)]`-only, below).
pub struct RealSmtpOps;

impl SmtpOps for RealSmtpOps {
    fn submit(
        &self,
        route: &SmtpRoute,
        username: &str,
        password: &str,
        from: &str,
        recipients: &[String],
        rfc822: &[u8],
    ) -> Result<(), String> {
        let envelope = build_envelope(from, recipients)?;
        let creds = Credentials::new(username.to_string(), password.to_string());
        let builder = match route.security {
            SmtpSecurity::ImplicitTls => {
                SmtpTransport::relay(&route.host).map_err(|e| smtp_err("tls relay setup", e))?
            }
            SmtpSecurity::StartTls => SmtpTransport::starttls_relay(&route.host)
                .map_err(|e| smtp_err("starttls relay setup", e))?,
        };
        submit_with_builder(builder, route.port, creds, &envelope, rfc822)
    }
}

// ── Orchestration: compose + submit (fresh send / threaded reply) ───────

fn as_mailaddrs(emails: &[String]) -> Vec<MailAddr> {
    emails.iter().map(|e| MailAddr { name: None, email: e.clone() }).collect()
}

/// Send a FRESH message FROM a linked inbox (`k2 mail send` on a linked
/// address). Recipients arrive already normalized + capped from the
/// route. Composes the RFC 822 (From = the linked account, a real
/// Message-ID) and submits over SMTP. Stamps the row's health either
/// way (mirrors the draft path).
pub fn send_linked_message(
    smtp: &dyn SmtpOps,
    inbox: &MailExternalInbox,
    password: &str,
    to: &[String],
    cc: &[String],
    subject: &str,
    body: &str,
) -> Result<(), ExtError> {
    let route = derive_smtp_route(inbox).map_err(ExtError::Engine)?;
    let to_addrs = as_mailaddrs(to);
    let cc_addrs = as_mailaddrs(cc);
    let date = chrono::Utc::now().to_rfc2822();
    let mid = external::new_message_id(&inbox.email_address);
    let rfc822 = external::compose_outgoing_rfc822(
        inbox, &to_addrs, &cc_addrs, subject, body, &date, &mid, None, None,
    )
    .map_err(ExtError::Engine)?;
    let recipients: Vec<String> = to.iter().chain(cc.iter()).cloned().collect();
    let result = smtp.submit(&route, &inbox.username, password, &inbox.email_address, &recipients, &rfc822);
    external::record_check(&inbox.id, result.as_ref().map(|_| ()).map_err(|e| e.as_str()));
    result.map_err(|e| ExtError::Engine(format!("SMTP send failed: {e}")))
}

/// Send a THREADED reply FROM a linked inbox (`k2 mail reply` on a
/// message living in that linked inbox). Fetches the source over IMAP,
/// extracts the reply context, locks the recipient to the source
/// sender, reuses the hosted `reply_subject`/`build_out_references`
/// guardrail helpers for the Subject/References, then submits over SMTP.
/// Returns the recipient it sent to. Health is stamped on the row.
pub fn send_linked_reply(
    imap: &dyn ImapOps,
    smtp: &dyn SmtpOps,
    inbox: &MailExternalInbox,
    password: &str,
    source_uid_token: &str,
    body: &str,
) -> Result<String, ExtError> {
    let route = derive_smtp_route(inbox).map_err(ExtError::Engine)?;
    let src_raw = imap
        .fetch_raw(inbox, password, source_uid_token)
        .map_err(|e| {
            external::record_check(&inbox.id, Err(&e));
            ExtError::Engine(e)
        })?
        .ok_or_else(|| {
            // Unknown UID / stale UIDVALIDITY — the masked shape the
            // route already uses for every not-yours/not-there case.
            ExtError::NotFound("the source message is no longer on the server".to_string())
        })?;
    let src = external::draft_source_from_raw(&src_raw.raw).map_err(ExtError::Engine)?;
    let Some(reply_to) = src.reply_to.clone() else {
        return Err(ExtError::Usage(
            "the source message has no sender address to reply to".to_string(),
        ));
    };
    // Punycode-normalize the locked recipient (pre-mortem #14).
    let recipient = addresses::normalize_address(&reply_to.email).map_err(|_| {
        ExtError::Usage(format!(
            "the source sender's address ('{}') is not a replyable address",
            reply_to.email
        ))
    })?;
    let subject = crate::mail::send::reply_subject(&src.subject);
    let references = crate::mail::send::build_out_references(
        src.references.as_deref(),
        src.message_id.as_deref(),
    );
    let to_addr = MailAddr { name: reply_to.name.clone(), email: recipient.clone() };
    let date = chrono::Utc::now().to_rfc2822();
    let mid = external::new_message_id(&inbox.email_address);
    let rfc822 = external::compose_outgoing_rfc822(
        inbox,
        std::slice::from_ref(&to_addr),
        &[],
        &subject,
        body,
        &date,
        &mid,
        src.message_id.as_deref(),
        references.as_deref(),
    )
    .map_err(ExtError::Engine)?;
    let recipients = vec![recipient.clone()];
    let result = smtp.submit(&route, &inbox.username, password, &inbox.email_address, &recipients, &rfc822);
    external::record_check(&inbox.id, result.as_ref().map(|_| ()).map_err(|e| e.as_str()));
    result
        .map(|_| recipient)
        .map_err(|e| ExtError::Engine(format!("SMTP send failed: {e}")))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — pure derivation table + a SCRIPTED LOOPBACK SMTP
// MOCK for the AUTH/MAIL/RCPT/DATA conversation (127.0.0.1 ephemeral
// port, no real network, no TLS — production TLS is rustls-with-
// verification and is deliberately not mockable). No real mail is ever
// sent (house rules).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn inbox_with(host: &str, smtp_host: Option<&str>, smtp_port: Option<i64>, smtp_tls: Option<&str>) -> MailExternalInbox {
        let mut ib = external::tests::test_inbox("X1", "p1", "rosson@example.com");
        ib.host = host.to_string();
        ib.smtp_host = smtp_host.map(str::to_string);
        ib.smtp_port = smtp_port;
        ib.smtp_tls = smtp_tls.map(str::to_string);
        ib
    }

    // ── derivation table (the deterministic core) ──

    #[test]
    fn derive_smtp_route_covers_providers_generic_and_overrides() {
        let g = derive_smtp_route(&inbox_with("imap.gmail.com", None, None, None)).unwrap();
        assert_eq!(g, SmtpRoute { host: "smtp.gmail.com".into(), port: 587, security: SmtpSecurity::StartTls });

        let o = derive_smtp_route(&inbox_with("outlook.office365.com", None, None, None)).unwrap();
        assert_eq!(o.host, "smtp.office365.com");
        assert_eq!((o.port, o.security), (587, SmtpSecurity::StartTls));
        let o2 = derive_smtp_route(&inbox_with("imap-mail.outlook.com", None, None, None)).unwrap();
        assert_eq!(o2.host, "smtp.office365.com");

        let f = derive_smtp_route(&inbox_with("imap.fastmail.com", None, None, None)).unwrap();
        assert_eq!(f, SmtpRoute { host: "smtp.fastmail.com".into(), port: 465, security: SmtpSecurity::ImplicitTls });

        // Generic imap.<domain> → smtp.<domain>:587 STARTTLS.
        let generic = derive_smtp_route(&inbox_with("IMAP.Corp.Example", None, None, None)).unwrap();
        assert_eq!(generic, SmtpRoute { host: "smtp.corp.example".into(), port: 587, security: SmtpSecurity::StartTls });

        // Non-imap.* host falls through to itself.
        let weird = derive_smtp_route(&inbox_with("mail.corp.example", None, None, None)).unwrap();
        assert_eq!(weird.host, "mail.corp.example");

        // Explicit host override wins; a tls override re-defaults the port.
        let ov = derive_smtp_route(&inbox_with("imap.gmail.com", Some("smtp.relay.example"), None, Some("implicit-tls"))).unwrap();
        assert_eq!(ov, SmtpRoute { host: "smtp.relay.example".into(), port: 465, security: SmtpSecurity::ImplicitTls });

        // Explicit port wins over the tls default.
        let ov2 = derive_smtp_route(&inbox_with("imap.gmail.com", None, Some(2525), Some("starttls"))).unwrap();
        assert_eq!((ov2.host.as_str(), ov2.port, ov2.security), ("smtp.gmail.com", 2525, SmtpSecurity::StartTls));
    }

    // ── the scripted loopback SMTP mock ──

    /// A minimal scripted SMTP server: greets, EHLO(AUTH PLAIN), accepts
    /// AUTH, MAIL/RCPT/DATA, then QUIT. Returns the lines it saw + the
    /// DATA payload. Panics (test failure) on protocol surprises.
    #[allow(clippy::type_complexity)]
    fn spawn_smtp_mock() -> (u16, std::thread::JoinHandle<(Vec<String>, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .expect("read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut w = stream;
            let mut seen: Vec<String> = Vec::new();
            let mut data: Vec<u8> = Vec::new();
            w.write_all(b"220 mock ESMTP ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let trimmed = line.trim_end().to_string();
                seen.push(trimmed.clone());
                let verb = trimmed.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                match verb.as_str() {
                    "EHLO" | "HELO" => {
                        w.write_all(b"250-mock greets you\r\n250 AUTH PLAIN\r\n").unwrap();
                    }
                    "AUTH" => {
                        w.write_all(b"235 2.7.0 Authentication successful\r\n").unwrap();
                    }
                    "MAIL" => {
                        w.write_all(b"250 2.1.0 OK\r\n").unwrap();
                    }
                    "RCPT" => {
                        w.write_all(b"250 2.1.5 OK\r\n").unwrap();
                    }
                    "DATA" => {
                        w.write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n").unwrap();
                        // Read message body until a bare `.` line.
                        loop {
                            let mut dl = String::new();
                            if reader.read_line(&mut dl).unwrap_or(0) == 0 {
                                break;
                            }
                            if dl == ".\r\n" || dl.trim_end() == "." {
                                break;
                            }
                            data.extend_from_slice(dl.as_bytes());
                        }
                        w.write_all(b"250 2.0.0 OK queued\r\n").unwrap();
                    }
                    "QUIT" => {
                        w.write_all(b"221 2.0.0 Bye\r\n").unwrap();
                        break;
                    }
                    "RSET" => {
                        w.write_all(b"250 2.0.0 OK\r\n").unwrap();
                    }
                    other => panic!("mock got unexpected command '{other}': {trimmed}"),
                }
            }
            (seen, data)
        });
        (port, handle)
    }

    /// Test-only plaintext submit against the loopback mock: the exact
    /// production `send_raw` wiring, minus TLS (the mock can't terminate
    /// TLS; the point is the SMTP conversation, not the handshake).
    fn submit_plaintext(
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        from: &str,
        recipients: &[String],
        rfc822: &[u8],
    ) -> Result<(), String> {
        let envelope = build_envelope(from, recipients)?;
        let creds = Credentials::new(username.to_string(), password.to_string());
        let builder = SmtpTransport::builder_dangerous(host);
        submit_with_builder(builder, port, creds, &envelope, rfc822)
    }

    #[test]
    fn loopback_mock_auth_mail_rcpt_data_and_password_only_in_auth() {
        let (port, handle) = spawn_smtp_mock();
        let inbox = inbox_with("imap.example.com", None, None, None);
        let rfc822 = external::compose_outgoing_rfc822(
            &inbox,
            &[MailAddr { name: Some("Pat".into()), email: "pat@dest.example".into() }],
            &[],
            "Hello there",
            "the body of the message",
            "Thu, 09 Jul 2026 08:00:00 +0000",
            "<mid-1@example.com>",
            None,
            None,
        )
        .expect("compose");
        submit_plaintext(
            "127.0.0.1",
            port,
            "rosson@example.com",
            "app-password",
            "rosson@example.com",
            &["pat@dest.example".to_string()],
            &rfc822,
        )
        .expect("submit over loopback");

        let (seen, data) = handle.join().expect("mock thread");
        assert!(seen.iter().any(|l| l.starts_with("MAIL FROM:")), "{seen:?}");
        assert!(
            seen.iter().any(|l| l.to_uppercase().contains("RCPT TO:") && l.contains("pat@dest.example")),
            "{seen:?}"
        );
        assert!(seen.iter().any(|l| l.starts_with("DATA")), "{seen:?}");
        // The DATA payload is the composed message.
        let body = String::from_utf8_lossy(&data);
        assert!(body.contains("From: \"Rosson\" <rosson@example.com>"), "{body}");
        assert!(body.contains("To: \"Pat\" <pat@dest.example>"), "{body}");
        assert!(body.contains("Subject: Hello there"), "{body}");
        // The password rides ONLY the AUTH line, base64-wrapped — it
        // never appears verbatim anywhere on the wire.
        assert!(!seen.iter().any(|l| l.contains("app-password")), "plaintext password leaked: {seen:?}");
        let auth = seen.iter().find(|l| l.starts_with("AUTH PLAIN")).expect("AUTH line");
        let b64 = auth.trim_start_matches("AUTH PLAIN").trim();
        let decoded = B64.decode(b64).expect("base64 auth");
        assert!(
            String::from_utf8_lossy(&decoded).contains("app-password"),
            "AUTH PLAIN must carry the app-password"
        );
    }
}
