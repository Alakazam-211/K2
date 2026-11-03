//! `k2 mail forward-message`: send an existing message on to someone else
//! (0.46.8). NOT mailbox forwarding (`k2 mail forward`, `k2 hostmail forward`:
//! a standing rule on a mailbox): this is one send, of one message, on an
//! agent's say-so.
//!
//! It composes a normal [`OutboundMessage`] and hands it to the send pipeline,
//! so governance is the same by construction: the gate, the recipient cap,
//! the owner's BCC, rate limits, the audit row, approvals.
//!
//! What this module owns:
//! - **The From is never the original sender's** (that would fail DMARC): the
//!   route stamps the mailbox's own address and this module never reads the
//!   original's From into a header, only into the quoted text.
//! - **The original is untrusted.** Its subject, names and addresses reach
//!   the new subject and the quoted header lines through [`one_line`], its
//!   filenames through `safe_attachment_name`, its part types through
//!   [`safe_media_type`], so nothing in it can add a header or fake a line.
//! - **Sizes apply to what is sent**, read off the mail server's own numbers
//!   first and off the real bytes after.
//!
//! The bytes are copied at compose time. Under approval the message holds them
//! ([`OutboundMessage::attachments`]), so deleting the original later can't
//! change or break what was approved.

use crate::cli_response::CliResponse;
use crate::mail::external_smtp::OutAttachment;
use crate::mail::jmap::{ForwardSource, MailAddr};
use crate::mail::messages::{self, ReadBackend};
use crate::mail::routes_send::{
    attachment_too_large, error_response, safe_attachment_name, MAX_ATTACHMENTS,
    MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_BYTES,
};
use crate::mail::send::{is_spoofy_format_char, StoredAttachment};

/// The name the original gets under `--as-eml`. Fixed: nothing from the
/// original goes in it.
pub const EML_NAME: &str = "original.eml";
const QUOTE_RULE: &str = "---------- Forwarded message ---------";
/// Longest subject or quoted header value kept, in characters.
const MAX_LINE_CHARS: usize = 400;

/// One line of untrusted text: line breaks, other control characters and
/// invisible formatting characters become spaces, runs of space collapse, at
/// most [`MAX_LINE_CHARS`] characters.
pub fn one_line(raw: &str) -> String {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() || is_spoofy_format_char(c) { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LINE_CHARS).collect()
}

/// `Fwd: <subject>`: cleaned, and never stacked on a subject already marked.
pub fn forward_subject(raw: &str) -> String {
    let clean = one_line(raw);
    let lower = clean.to_ascii_lowercase();
    if lower.starts_with("fwd:") || lower.starts_with("fw:") {
        clean
    } else if clean.is_empty() {
        "Fwd: (no subject)".to_string()
    } else {
        format!("Fwd: {clean}")
    }
}

/// A plain `type/subtype` from a value the original's sender wrote, or
/// `application/octet-stream`. Nothing of `message/*` passes: `external-body`
/// and `partial` make a client fetch or reassemble something the approver never
/// saw, and the mail server sends an attached `rfc822` as base64, which RFC 2046
/// doesn't allow. A multipart isn't a file.
pub fn safe_media_type(raw: &str) -> String {
    let media = raw.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if media.starts_with("multipart/") || media.starts_with("message/") {
        return "application/octet-stream".to_string();
    }
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 127
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match media.split_once('/') {
        Some((a, b)) if token(a) && token(b) => media,
        _ => "application/octet-stream".to_string(),
    }
}

fn quoted_addr(a: &MailAddr) -> String {
    let email = one_line(&a.email);
    match a.name.as_deref().map(one_line).filter(|n| !n.is_empty()) {
        Some(name) => format!("{name} <{email}>"),
        None => email,
    }
}

fn quoted_list(list: &[MailAddr]) -> String {
    list.iter().map(quoted_addr).collect::<Vec<_>>().join(", ")
}

/// The body of a normal forward: the note, the original's headers, its text.
pub fn forward_body(note: Option<&str>, src: &ForwardSource) -> String {
    let s = &src.full.summary;
    let mut out = String::new();
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(n);
        out.push_str("\n\n");
    }
    out.push_str(QUOTE_RULE);
    out.push('\n');
    out.push_str(&format!("From: {}\n", quoted_list(&s.from)));
    let date = src.sent_at.as_deref().unwrap_or(&s.received_at);
    out.push_str(&format!("Date: {}\n", one_line(date)));
    out.push_str(&format!("Subject: {}\n", one_line(&s.subject)));
    out.push_str(&format!("To: {}\n", quoted_list(&s.to)));
    if !src.full.cc.is_empty() {
        out.push_str(&format!("Cc: {}\n", quoted_list(&src.full.cc)));
    }
    out.push('\n');
    match (&src.full.text, &src.full.html) {
        (Some(t), _) => out.push_str(t),
        (None, Some(h)) => out.push_str(&messages::html_to_text(h)),
        (None, None) => {}
    }
    out
}

fn stored(filename: String, content_type: String, bytes: Vec<u8>) -> StoredAttachment {
    StoredAttachment::from_out(&OutAttachment { scanned: false, filename, content_type, bytes })
}

/// `--as-eml`: the original's raw bytes as one file named `original.eml`, read
/// off the mail server and never parsed or rewritten here. Typed as a plain file:
/// the mail server sends a part as base64, which RFC 2046 doesn't allow for
/// `message/rfc822`, so a strict gateway could refuse it or a scanner skip it.
fn original_as_eml(
    read: &dyn ReadBackend,
    account_id: &str,
    src: &ForwardSource,
) -> Result<StoredAttachment, CliResponse> {
    let too_big = |n: u64| attachment_too_large("the original message", n, "for an .eml");
    if src.size > MAX_ATTACHMENT_BYTES {
        return Err(too_big(src.size));
    }
    let Some(blob) = src.full.blob_id.as_deref() else {
        return Err(error_response("502 Bad Gateway", "engine", "the server exposed no raw copy of this message"));
    };
    let bytes = read
        .fetch_blob(account_id, blob, EML_NAME, "message/rfc822")
        .map_err(|hint| error_response("502 Bad Gateway", "engine", &hint))?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(too_big(bytes.len() as u64));
    }
    Ok(stored(EML_NAME.to_string(), "application/octet-stream".to_string(), bytes))
}

/// A normal forward's files: the original's attachments, byte for byte, under
/// the same limits as any other attachment. The server's sizes refuse first,
/// with no download; the real bytes are checked as they arrive.
fn original_attachments(
    read: &dyn ReadBackend,
    account_id: &str,
    src: &ForwardSource,
) -> Result<Vec<StoredAttachment>, CliResponse> {
    let atts = &src.full.attachments;
    if atts.len() > MAX_ATTACHMENTS {
        return Err(error_response(
            "400 Bad Request",
            "attachment_too_large",
            &format!(
                "this message has {} attachments and the limit is {MAX_ATTACHMENTS}. --as-eml sends them \
                 all inside one file, if the whole message fits in 25 MiB",
                atts.len()
            ),
        ));
    }
    let claimed: u64 = atts.iter().map(|a| a.size).fold(0, u64::saturating_add);
    if claimed > MAX_ATTACHMENTS_TOTAL_BYTES {
        return Err(attachment_too_large("this message's attachments", claimed, "in total"));
    }
    let mut out = Vec::with_capacity(atts.len());
    let mut total = 0u64;
    for (i, a) in atts.iter().enumerate() {
        let fallback = format!("attachment-{}", i + 1);
        let name = safe_attachment_name(a.filename.as_deref().unwrap_or(&fallback));
        let media = safe_media_type(&a.mime);
        let bytes = read
            .fetch_blob(account_id, &a.blob_id, &name, &media)
            .map_err(|hint| error_response("502 Bad Gateway", "engine", &hint))?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_ATTACHMENTS_TOTAL_BYTES {
            return Err(attachment_too_large("this message's attachments", total, "in total"));
        }
        out.push(stored(name, media, bytes));
    }
    Ok(out)
}

/// What a forward adds to a message: the route supplies From, recipients and
/// everything governed.
pub struct Composed {
    pub subject: String,
    pub text_body: String,
    pub attachments: Vec<StoredAttachment>,
}

/// Compose a forward of `src`.
pub fn compose(
    read: &dyn ReadBackend,
    account_id: &str,
    src: &ForwardSource,
    note: Option<&str>,
    as_eml: bool,
) -> Result<Composed, CliResponse> {
    let subject = forward_subject(&src.full.summary.subject);
    if as_eml {
        let attachment = original_as_eml(read, account_id, src)?;
        let text_body = match note.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => "The original message is attached.".to_string(),
        };
        return Ok(Composed { subject, text_body, attachments: vec![attachment] });
    }
    Ok(Composed {
        subject,
        text_body: forward_body(note, src),
        attachments: original_attachments(read, account_id, src)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::fake_jmap::FakeJmap;
    use crate::mail::messages::StalwartReadBackend;
    use crate::mail::send::{
        self, DbOutboundStore, Gate, OutboundMessage, OutboundRequest, OutboundStore, SendOutcome,
    };

    #[test]
    fn subjects_are_one_clean_line_and_never_stack() {
        for (raw, want) in [
            ("Invoice 12", "Fwd: Invoice 12"),
            ("  spaced   out  ", "Fwd: spaced out"),
            ("Fwd: already", "Fwd: already"),
            ("FW: old style", "FW: old style"),
            ("fwd:tight", "fwd:tight"),
            ("Re: thread", "Fwd: Re: thread"),
            ("", "Fwd: (no subject)"),
            ("\r\n", "Fwd: (no subject)"),
            ("Hi\r\nBcc: evil@x.example", "Fwd: Hi Bcc: evil@x.example"),
            ("Hi\nX-Evil: 1\n\nbody", "Fwd: Hi X-Evil: 1 body"),
            ("Hi\u{0}there\u{1b}[2J", "Fwd: Hi there [2J"),
            ("pay\u{202E}moc.evil", "Fwd: pay moc.evil"),
        ] {
            assert_eq!(forward_subject(raw), want, "{raw:?}");
            assert!(!forward_subject(raw).contains(['\r', '\n']), "{raw:?}");
        }
        assert!(forward_subject(&"x".repeat(5000)).chars().count() <= MAX_LINE_CHARS + 5);
    }

    #[test]
    fn media_types_are_plain_tokens_or_a_blob() {
        for (raw, want) in [
            ("application/pdf", "application/pdf"),
            ("IMAGE/PNG", "image/png"),
            ("text/plain; charset=utf-8", "text/plain"),
            ("message/rfc822", "application/octet-stream"),
            ("message/external-body", "application/octet-stream"),
            ("message/partial", "application/octet-stream"),
            ("multipart/mixed", "application/octet-stream"),
            ("text/plain\r\nX-Evil: 1", "application/octet-stream"),
            ("nonsense", "application/octet-stream"),
            ("a/b/c", "application/octet-stream"),
            ("", "application/octet-stream"),
        ] {
            assert_eq!(safe_media_type(raw), want, "{raw:?}");
        }
    }

    fn email_entry(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "blobId": "raw-1",
            "threadId": "t1",
            "from": [{ "name": "Sam\r\nX-Evil-Name: 1", "email": "sam@outside.example" }],
            "to": [{ "email": "bot@acme.dev" }],
            "cc": [{ "name": "Pat", "email": "pat@outside.example" }],
            "replyTo": [{ "email": "other@outside.example" }],
            "subject": "Quote\r\nBcc: evil@x.example",
            "sentAt": "2026-11-01T10:00:00Z",
            "receivedAt": "2026-11-01T10:00:05Z",
            "size": 42,
            "keywords": {},
            "hasAttachment": true,
            "textBody": [{ "partId": "1", "type": "text/plain" }],
            "htmlBody": [{ "partId": "1", "type": "text/plain" }],
            "bodyValues": { "1": { "value": "Please see the quote.\nFrom: not a real header line\n", "isTruncated": false } },
            "attachments": [
                { "blobId": "att-1", "name": "../quote\r\nX-Evil-File: 1.pdf", "type": "application/pdf", "size": 7 },
                { "blobId": "att-2", "name": null, "type": "bad type\r\nX: 1", "size": 3 },
            ],
        })
    }

    fn seeded() -> (FakeJmap, String) {
        let fake = FakeJmap::start();
        {
            let mut st = fake.state.lock().unwrap();
            st.emails.insert("e1".to_string(), email_entry("e1"));
            st.blobs.insert("att-1".to_string(), b"%PDF-1.\x00\xff".to_vec()[..7].to_vec());
            st.blobs.insert("att-2".to_string(), vec![1, 2, 3]);
            st.blobs.insert("raw-1".to_string(), b"From: sam@outside.example\r\nSubject: Quote\r\n\r\nraw body bytes\r\n".to_vec());
        }
        (fake, "acct".to_string())
    }

    fn composed(r: Result<Composed, CliResponse>) -> Composed {
        match r {
            Ok(c) => c,
            Err(resp) => panic!("expected it to compose: {}", resp.body),
        }
    }

    fn fetch(read: &StalwartReadBackend, acct: &str) -> ForwardSource {
        read.fetch_for_forward(acct, "e1").expect("engine").expect("found")
    }

    fn out_message(c: Composed) -> OutboundMessage {
        OutboundMessage {
            from_name: Some("Bot".to_string()),
            from: format!("bot-{}@acme.dev", uuid::Uuid::new_v4().simple()),
            to: vec!["dest@else.example".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            policy_bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: c.subject,
            text_body: c.text_body,
            in_reply_to: None,
            references: None,
            reply_source: None,
            attachments: c.attachments,
            ephemeral: false,
            notify: None,
            forward_of: None,
            source_dmarc: None,
            source_check: None,
        }
    }

    fn temp_store() -> (DbOutboundStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("k2-fwd-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        (DbOutboundStore::at(dir.clone()), dir)
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn cleanup_rows(project: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_outbound WHERE owner_project_id = ?1", rusqlite::params![project]).unwrap();
    }

    /// The whole default forward against the fake engine: the original's text
    /// is fetched in full (no body limit) and never marked read; its files go
    /// back up byte for byte; From is the mailbox; nothing from the original
    /// reaches a header unclean.
    #[test]
    fn a_default_forward_round_trips_headers_text_and_files() {
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);
        let src = fetch(&read, &acct);
        let c = composed(compose(&read, &acct, &src, Some("FYI, see below."), false));

        assert_eq!(c.subject, "Fwd: Quote Bcc: evil@x.example");
        let body = &c.text_body;
        assert!(body.starts_with("FYI, see below.\n\n---------- Forwarded message ---------\n"), "{body}");
        assert!(body.contains("\nFrom: Sam X-Evil-Name: 1 <sam@outside.example>\n"), "{body}");
        assert!(body.contains("\nDate: 2026-11-01T10:00:00Z\n"), "{body}");
        assert!(body.contains("\nSubject: Quote Bcc: evil@x.example\n"), "{body}");
        assert!(body.contains("\nTo: bot@acme.dev\nCc: Pat <pat@outside.example>\n"), "{body}");
        assert!(body.ends_with("Please see the quote.\nFrom: not a real header line\n"), "{body}");

        assert_eq!(c.attachments.len(), 2);
        assert_eq!(c.attachments[0].filename, "quote__X-Evil-File: 1.pdf");
        assert_eq!(c.attachments[0].content_type, "application/pdf");
        assert_eq!(c.attachments[1].filename, "attachment-2");
        assert_eq!(c.attachments[1].content_type, "application/octet-stream");

        let m = out_message(c);
        let from = m.from.clone();
        let client = fake.client();
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let out = send::gate_and_dispatch(&store, Some(&client), Gate::On, &req, now()).expect("dispatch");
        assert!(matches!(out, SendOutcome::Submitted { .. }), "{out:?}");

        let st = fake.state.lock().unwrap();
        assert_eq!(st.uploads, vec![b"%PDF-1.".to_vec(), vec![1, 2, 3]], "the original's bytes, unchanged");
        let e = &st.created[0];
        assert_eq!(e["from"][0]["email"], from.as_str(), "the mailbox, never the original sender: {e}");
        assert!(!e.to_string().contains("sam@outside.example\"}]"), "the sender is no header: {e}");
        assert!(e.get("replyTo").is_none(), "no Reply-To by default: {e}");
        assert_eq!(e["subject"], "Fwd: Quote Bcc: evil@x.example");
        assert_eq!(st.envelopes[0]["rcptTo"], serde_json::json!([{ "email": "dest@else.example" }]));
        // Fetched whole: no maxBodyValueBytes. Not marked read: no keyword set.
        assert!(st.gets[0].get("maxBodyValueBytes").is_none(), "{}", st.gets[0]);
        assert_eq!(st.created.len(), 1, "the only Email/set is the new message");
        drop(st);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--as-eml`: the raw message unchanged, as a plain file, under its
    /// fixed name; and under approval the bytes are the ones captured, whatever
    /// happens to the original before someone approves.
    #[test]
    fn an_eml_forward_carries_the_raw_message_and_survives_the_original_going_away() {
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);
        let src = fetch(&read, &acct);
        let raw = fake.state.lock().unwrap().blobs["raw-1"].clone();
        let c = composed(compose(&read, &acct, &src, None, true));
        assert_eq!(c.text_body, "The original message is attached.");
        assert_eq!(c.attachments.len(), 1, "one file, the attachments inside it");
        assert_eq!(c.attachments[0].filename, EML_NAME);
        assert_eq!(c.attachments[0].content_type, "application/octet-stream");

        let m = out_message(c);
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Queued { id, .. } = send::gate_and_dispatch(&store, None, Gate::Approval, &req, now()).expect("queue") else {
            panic!("expected a queued message")
        };
        assert!(fake.state.lock().unwrap().uploads.is_empty(), "nothing leaves before approval");

        // The original is deleted while the message waits.
        {
            let mut st = fake.state.lock().unwrap();
            st.blobs.remove("raw-1");
            st.emails.remove("e1");
        }
        let client = fake.client();
        let mut acct_for = |_: &str| Ok("acct".to_string());
        let out = send::approve_and_submit(&store, Ok(&client), &mut acct_for, &id, "owner", None, now()).expect("approve");
        assert_eq!(out, send::ApproveOutcome::Submitted);
        let st = fake.state.lock().unwrap();
        assert_eq!(st.uploads, vec![raw], "the raw bytes, unchanged");
        assert_eq!(st.created[0]["attachments"][0]["type"], "application/octet-stream", "{}", st.created[0]);
        assert_eq!(st.created[0]["attachments"][0]["name"], EML_NAME, "{}", st.created[0]);
        drop(st);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_eml_forward_keeps_the_note_as_its_text() {
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);
        let src = fetch(&read, &acct);
        let c = composed(compose(&read, &acct, &src, Some("See attached."), true));
        assert_eq!(c.text_body, "See attached.");
    }

    /// Sizes apply to what is sent, on the server's numbers and on the bytes.
    #[test]
    fn forwards_over_the_limits_are_refused() {
        let code = |r: Result<Composed, CliResponse>| -> serde_json::Value {
            let resp = match r {
                Err(resp) => resp,
                Ok(_) => panic!("expected a refusal"),
            };
            assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
            serde_json::from_str(&resp.body).expect("JSON")
        };
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);

        // The server says the whole message is too big for an .eml.
        let mut src = fetch(&read, &acct);
        src.size = MAX_ATTACHMENT_BYTES + 1;
        let v = code(compose(&read, &acct, &src, None, true));
        assert_eq!(v["error"]["code"], "attachment_too_large", "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("the original message"), "{v}");

        // The server understated: the real bytes refuse.
        let mut src = fetch(&read, &acct);
        src.size = 1;
        fake.state.lock().unwrap().blobs.insert("raw-1".to_string(), vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1]);
        let v = code(compose(&read, &acct, &src, None, true));
        assert_eq!(v["error"]["code"], "attachment_too_large", "{v}");

        // Attachments too big by the server's numbers: no download.
        let mut src = fetch(&read, &acct);
        src.full.attachments[0].size = MAX_ATTACHMENTS_TOTAL_BYTES;
        src.full.attachments[1].size = 1;
        fake.state.lock().unwrap().blobs.clear();
        let v = code(compose(&read, &acct, &src, None, false));
        assert!(v["error"]["hint"].as_str().unwrap().contains("in total"), "{v}");

        // Understated again: the bytes refuse.
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);
        fake.state.lock().unwrap().blobs.insert("att-1".to_string(), vec![0u8; MAX_ATTACHMENTS_TOTAL_BYTES as usize]);
        let src = fetch(&read, &acct);
        let v = code(compose(&read, &acct, &src, None, false));
        assert_eq!(v["error"]["code"], "attachment_too_large", "{v}");

        // Too many files.
        let mut src = fetch(&read, &acct);
        let one = src.full.attachments[0].clone();
        src.full.attachments = vec![one; MAX_ATTACHMENTS + 1];
        let v = code(compose(&read, &acct, &src, None, false));
        assert!(v["error"]["hint"].as_str().unwrap().contains("--as-eml"), "{v}");
    }

    /// An HTML-only original still forwards as readable text.
    #[test]
    fn an_html_only_original_forwards_as_text() {
        let (fake, acct) = seeded();
        {
            let mut st = fake.state.lock().unwrap();
            let e = st.emails.get_mut("e1").unwrap();
            e["textBody"] = serde_json::json!([]);
            e["htmlBody"] = serde_json::json!([{ "partId": "2", "type": "text/html" }]);
            e["bodyValues"] = serde_json::json!({ "2": { "value": "<p>Hello <b>there</b></p><script>alert(1)</script>" } });
        }
        let read = StalwartReadBackend::new(fake.client(), None);
        let src = fetch(&read, &acct);
        let c = composed(compose(&read, &acct, &src, None, false));
        assert!(c.text_body.contains("Hello there"), "{}", c.text_body);
        assert!(!c.text_body.contains("alert"), "{}", c.text_body);
    }

    #[test]
    fn an_unknown_message_answers_none() {
        let (fake, acct) = seeded();
        let read = StalwartReadBackend::new(fake.client(), None);
        assert!(read.fetch_for_forward(&acct, "nope").expect("engine").is_none());
    }

    #[test]
    fn stored_bytes_are_not_lost_in_the_store_round_trip() {
        let (store, dir) = temp_store();
        let project = format!("P-{}", uuid::Uuid::new_v4());
        let m = out_message(Composed {
            subject: "Fwd: x".to_string(),
            text_body: "t".to_string(),
            attachments: vec![stored("a.bin".to_string(), "application/octet-stream".to_string(), vec![0, 255, 7])],
        });
        let req = OutboundRequest { project_id: &project, agent_name: "bot", account_id: "acct", message: &m, send_after: None };
        let SendOutcome::Queued { id, .. } = send::gate_and_dispatch(&store, None, Gate::Approval, &req, now()).expect("queue") else {
            panic!("expected a queued message")
        };
        let back = store.load_message_full(&id).unwrap();
        assert_eq!(back.attachments[0].to_out().unwrap().bytes, vec![0, 255, 7]);
        cleanup_rows(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
