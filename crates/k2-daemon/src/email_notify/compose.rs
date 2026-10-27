//! What one notification may say (prd-email-notifications-core-v1 E19,
//! §3.8) and how it is put on the wire: the RFC 8621 Email object for a
//! hosted send (E8) and the RFC 5322 message for a relay (E12).
//!
//! Limits: at most [`MAX_RECIPIENTS`] envelope recipients (to + cc + bcc);
//! subject ≤ 998 bytes with no control characters; text ≤ 256 KB, html ≤
//! 512 KB, the whole message ≤ 1 MB; at most [`MAX_HEADERS`] custom
//! headers from a short named set (the App's own `X-App-<name>` namespace
//! and `X-Entity-Ref-ID`, see [`header_name_allowed`]) with a
//! printable-ASCII value ≤ 998 bytes; `replyTo` ≤ 3 (a structured field,
//! never a header); tags ≤ 10 × `[A-Za-z0-9_-]{1,64}`; an idempotency key
//! ≤ 256 characters; `sendAt` ≤ 7 days ahead. Every other header is
//! refused (`header_not_allowed`): K2's own (`From`, `Bcc`, `Reply-To`,
//! `DKIM-*`, `Content-*`…) and every relay provider's control header
//! (`X-SMTPAPI`, `X-MC-*`, `X-Mailgun-*`, `X-SES-*`, `X-PM-*`…, security
//! review H2), on both the hosted and the relay path.
//!
//! Every address (to, cc, bcc, replyTo) is a plain `dot-atom@domain`
//! (security review M4): no control characters, spaces, quotes or `<>`,
//! local part ≤ 64, address ≤ 254. Refused, never cleaned up.

use std::collections::BTreeMap;

use serde_json::{json, Value as J};

use super::{fail, R};

pub const MAX_RECIPIENTS: usize = 50;
pub const MAX_SUBJECT_BYTES: usize = 998;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_HTML_BYTES: usize = 512 * 1024;
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;
pub const MAX_HEADERS: usize = 10;
pub const MAX_HEADER_VALUE_BYTES: usize = 998;
pub const MAX_REPLY_TO: usize = 3;
pub const MAX_TAGS: usize = 10;
pub const MAX_IDEMPOTENCY_KEY: usize = 256;
/// The furthest `sendAt` may be (7 days; security review M1).
pub const MAX_SCHEDULE_SECS: i64 = 7 * 86_400;

/// The request keys `email.send` takes (the catalog's
/// `additionalProperties: false`).
pub const REQUEST_KEYS: &[&str] = &["from", "fromName", "to", "cc", "bcc", "subject", "text", "html", "replyTo", "headers", "tags", "idempotencyKey", "sendAt"];

/// One parsed, checked `email.send` request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub from: Option<String>,
    pub from_name: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub reply_to: Vec<String>,
    pub headers: BTreeMap<String, String>,
    pub tags: Vec<String>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    pub send_at: Option<i64>,
}

impl Request {
    /// Every envelope recipient, to + cc + bcc, each once.
    pub fn envelope(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
        out
    }

    /// The SHA-256 of what was asked (the idempotency comparison, E19):
    /// everything but the key itself.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let canon = serde_json::to_vec(self).unwrap_or_default();
        Sha256::digest(&canon).iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn body_bytes(&self) -> usize {
        self.subject.len()
            + self.text.as_deref().map(str::len).unwrap_or(0)
            + self.html.as_deref().map(str::len).unwrap_or(0)
            + self.headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum::<usize>()
    }
}

fn usage(hint: impl Into<String>) -> super::Fail {
    fail(400, "usage", hint)
}

fn header_refused(hint: impl Into<String>) -> super::Fail {
    fail(400, "header_not_allowed", hint)
}

/// A list field: one string or an array of strings.
fn list(v: &J, key: &str) -> R<Vec<String>> {
    // M4: a control character is refused, never trimmed away.
    let bad = |s: &str| s.chars().any(char::is_control);
    match v.get(key) {
        Some(J::String(s)) if bad(s) => return Err(usage(format!("{key} can't hold line breaks or other control characters"))),
        Some(J::Array(a)) if a.iter().any(|x| x.as_str().is_some_and(bad)) => {
            return Err(usage(format!("{key} can't hold line breaks or other control characters")))
        }
        _ => {}
    }
    match v.get(key) {
        None | Some(J::Null) => Ok(Vec::new()),
        Some(J::String(s)) => Ok(if s.trim().is_empty() { Vec::new() } else { vec![s.trim().to_string()] }),
        Some(J::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| usage(format!("{key} is a list of strings"))))
            .collect(),
        Some(_) => Err(usage(format!("{key} is a string or a list of strings"))),
    }
}

fn opt_str(v: &J, key: &str) -> R<Option<String>> {
    match v.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(J::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(usage(format!("{key} is a string"))),
    }
}

fn addresses(raw: &[String], key: &str) -> R<Vec<String>> {
    let list = crate::mail::send::normalize_recipients(raw).map_err(|e| match e {
        crate::mail::send::SendError::Usage(h) => usage(format!("{key}: {h}")),
        _ => usage(format!("{key}: an address isn't valid")),
    })?;
    for a in &list {
        if let Some(why) = mailbox_problem(a) {
            return Err(usage(format!("{key}: {why}")));
        }
    }
    Ok(list)
}

/// RFC 5322 `atext` (a dot-atom's characters, besides `.`).
fn atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~".contains(c)
}

/// Why a (normalized) address isn't a plain `dot-atom@domain` a
/// notification may name (security review M4), if it isn't.
pub fn mailbox_problem(address: &str) -> Option<String> {
    if address.len() > crate::mail::addresses::MAX_ADDRESS_BYTES {
        return Some("an address is at most 254 characters".into());
    }
    let Some((local, domain)) = address.rsplit_once('@') else { return Some("an address is local@domain".into()) };
    if local.is_empty() || local.len() > crate::mail::addresses::MAX_LOCAL_PART_BYTES {
        return Some("an address's local part is 1–64 characters".into());
    }
    if !local.chars().all(|c| c == '.' || atext(c)) || local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return Some(
            "an address's local part is plain letters, digits and !#$%&'*+-/=?^_`{|}~ with single dots between \
             (no spaces, quotes, < >, line breaks or other control characters)"
                .into(),
        );
    }
    if domain.is_empty() || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Some("an address's domain isn't valid".into());
    }
    None
}

/// One address for K2's own mail (`send_fixed`, the owner's address,
/// a sender's default Reply-To): normalized, then the same strict check.
pub fn strict_address(raw: &str) -> R<String> {
    let a = crate::mail::addresses::normalize_address(raw).map_err(|_| usage("the address isn't valid"))?;
    match mailbox_problem(&a) {
        Some(why) => Err(usage(why)),
        None => Ok(a),
    }
}

/// The App's own header namespace (security review H2).
pub const APP_HEADER_PREFIX: &str = "X-App-";

/// The few named headers outside the namespace an App may set.
pub const NAMED_HEADERS: &[&str] = &["X-Entity-Ref-ID"];

/// Is `name` an allowed custom header name (E19, §3.8, review H2)? Only
/// `X-App-<letters, digits, ->` (at most 64 after the prefix) or one of
/// [`NAMED_HEADERS`]. No relay provider acts on either, so an App can
/// never reach `X-SMTPAPI`, `X-MC-BccAddress`, `X-Mailgun-*`, `X-SES-*`,
/// `X-PM-*` and the like, on either path.
pub fn header_name_allowed(name: &str) -> bool {
    if NAMED_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name)) {
        return true;
    }
    if name.len() <= APP_HEADER_PREFIX.len() || !name[..APP_HEADER_PREFIX.len()].eq_ignore_ascii_case(APP_HEADER_PREFIX) {
        return false;
    }
    let rest = &name[APP_HEADER_PREFIX.len()..];
    rest.len() <= 64 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A display name may not carry an RFC 2047 encoded word (`=?…?=`): a
/// client decodes it after K2's checks (security review L6).
pub fn encoded_word(name: &str) -> bool {
    name.contains("=?")
}

/// A display name folded for comparison: lowercase, no whitespace.
pub fn fold_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// Re-check a kept request (the scheduled flusher, security review L12):
/// it must still parse to itself under today's rules.
pub fn recheck(req: &Request, now: i64) -> R<()> {
    let mut v = serde_json::to_value(req).map_err(|e| usage(format!("the kept email doesn't read back: {e}")))?;
    if let Some(o) = v.as_object_mut() {
        o.remove("sendAt");
        o.retain(|_, x| !x.is_null());
    }
    let mut again = parse(&v, now)?;
    again.send_at = req.send_at;
    again.idempotency_key = req.idempotency_key.clone();
    if &again != req {
        return Err(usage("the kept email changed since it was accepted"));
    }
    Ok(())
}

fn header_value_ok(value: &str) -> bool {
    value.len() <= MAX_HEADER_VALUE_BYTES && value.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fn tag_ok(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `sendAt`: unix seconds or an RFC 3339 time. `None` = now. A time in
/// the past (beyond a minute of skew) or past 7 days is refused.
fn send_at(v: &J, now: i64) -> R<Option<i64>> {
    let t = match v.get("sendAt") {
        None | Some(J::Null) => return Ok(None),
        Some(J::Number(n)) => n.as_i64().ok_or_else(|| usage("sendAt is whole seconds since the epoch, or an RFC 3339 time"))?,
        Some(J::String(s)) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .map(|d| d.timestamp())
            .map_err(|_| usage("sendAt is whole seconds since the epoch, or an RFC 3339 time"))?,
        Some(_) => return Err(usage("sendAt is whole seconds since the epoch, or an RFC 3339 time")),
    };
    if t < now - 60 {
        return Err(usage("sendAt is in the past"));
    }
    if t > now + MAX_SCHEDULE_SECS {
        return Err(usage("sendAt is at most 7 days ahead"));
    }
    Ok((t > now).then_some(t))
}

/// Parse and check one `email.send` body (E19). Nothing here decides who
/// may send or to whom: the pipeline does.
pub fn parse(v: &J, now: i64) -> R<Request> {
    let obj = v.as_object().ok_or_else(|| usage("the body is a JSON object"))?;
    if let Some(k) = obj.keys().find(|k| !REQUEST_KEYS.contains(&k.as_str())) {
        if k.eq_ignore_ascii_case("attachments") {
            return Err(usage("attachments aren't supported yet"));
        }
        return Err(usage(format!("unknown field {k} (email.send takes {})", REQUEST_KEYS.join(", "))));
    }
    let to = addresses(&list(v, "to")?, "to")?;
    let cc = addresses(&list(v, "cc")?, "cc")?;
    let bcc = addresses(&list(v, "bcc")?, "bcc")?;
    let mut req = Request {
        from: opt_str(v, "from")?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        from_name: None,
        to,
        cc,
        bcc,
        subject: String::new(),
        text: opt_str(v, "text")?.filter(|s| !s.is_empty()),
        html: opt_str(v, "html")?.filter(|s| !s.is_empty()),
        reply_to: Vec::new(),
        headers: BTreeMap::new(),
        tags: Vec::new(),
        idempotency_key: None,
        send_at: send_at(v, now)?,
    };
    if req.to.is_empty() && req.cc.is_empty() && req.bcc.is_empty() {
        return Err(usage("name at least one recipient (to)"));
    }
    if req.envelope().len() > MAX_RECIPIENTS {
        return Err(fail(400, "too_many_recipients", format!("at most {MAX_RECIPIENTS} recipients in one email (to, cc and bcc together)")));
    }
    // Subject.
    let subject = opt_str(v, "subject")?.unwrap_or_default();
    if subject.trim().is_empty() {
        return Err(usage("subject is required"));
    }
    if subject.chars().any(char::is_control) {
        return Err(usage("subject can't hold line breaks or other control characters"));
    }
    if subject.len() > MAX_SUBJECT_BYTES {
        return Err(usage(format!("subject is at most {MAX_SUBJECT_BYTES} bytes")));
    }
    req.subject = subject;
    // Bodies.
    if req.text.is_none() && req.html.is_none() {
        return Err(usage("send text, html or both"));
    }
    if req.text.as_deref().is_some_and(|t| t.len() > MAX_TEXT_BYTES) {
        return Err(fail(413, "message_too_large", "text is at most 256 KB"));
    }
    if req.html.as_deref().is_some_and(|t| t.len() > MAX_HTML_BYTES) {
        return Err(fail(413, "message_too_large", "html is at most 512 KB"));
    }
    // fromName (E20 adds the system-name rule in the pipeline).
    if let Some(n) = opt_str(v, "fromName")? {
        if encoded_word(&n) {
            return Err(usage("fromName can't hold an encoded word (=?…?=): send the name as plain text"));
        }
        req.from_name = Some(crate::mail::send::validate_from_name(&n).map_err(|e| match e {
            crate::mail::send::SendError::Usage(h) => usage(h),
            _ => usage("fromName isn't valid"),
        })?);
    }
    // replyTo (who may be named is the pipeline's rule).
    let reply_to = addresses(&list(v, "replyTo")?, "replyTo")?;
    if reply_to.len() > MAX_REPLY_TO {
        return Err(usage(format!("replyTo names at most {MAX_REPLY_TO} addresses")));
    }
    req.reply_to = reply_to;
    // Headers.
    match v.get("headers") {
        None | Some(J::Null) => {}
        Some(J::Object(m)) => {
            if m.len() > MAX_HEADERS {
                return Err(header_refused(format!("at most {MAX_HEADERS} custom headers")));
            }
            for (k, x) in m {
                let name = k.trim();
                if !header_name_allowed(name) {
                    return Err(header_refused(format!(
                        "header {} isn't allowed: only your App's own X-App-… headers (letters, digits and -) and \
                         X-Entity-Ref-ID. From, To, Reply-To (use replyTo), Bcc (use bcc), DKIM, Content-* and the like \
                         are K2's, and mail providers' control headers (X-SMTPAPI, X-Mailgun-…) are never passed on",
                        name.chars().filter(|c| !c.is_control()).take(80).collect::<String>()
                    )));
                }
                let Some(val) = x.as_str() else { return Err(header_refused(format!("header {name}'s value is a string"))) };
                if !header_value_ok(val) {
                    return Err(header_refused(format!(
                        "header {name}'s value is printable ASCII with no line breaks, at most {MAX_HEADER_VALUE_BYTES} bytes"
                    )));
                }
                if req.headers.keys().any(|h| h.eq_ignore_ascii_case(name)) {
                    return Err(header_refused(format!("header {name} is given twice")));
                }
                req.headers.insert(name.to_string(), val.to_string());
            }
        }
        Some(_) => return Err(header_refused("headers is an object of name → value")),
    }
    // Tags.
    let tags = list(v, "tags")?;
    if tags.len() > MAX_TAGS {
        return Err(usage(format!("at most {MAX_TAGS} tags")));
    }
    if let Some(bad) = tags.iter().find(|t| !tag_ok(t)) {
        return Err(usage(format!(
            "tag {} isn't valid: letters, digits, _ and -, at most 64",
            bad.chars().filter(|c| !c.is_control()).take(80).collect::<String>()
        )));
    }
    req.tags = tags;
    // Idempotency key.
    if let Some(k) = opt_str(v, "idempotencyKey")? {
        let k = k.trim().to_string();
        if k.is_empty() || k.chars().count() > MAX_IDEMPOTENCY_KEY || k.chars().any(char::is_control) {
            return Err(usage(format!("idempotencyKey is 1–{MAX_IDEMPOTENCY_KEY} characters, no control characters")));
        }
        req.idempotency_key = Some(k);
    }
    if req.body_bytes() > MAX_TOTAL_BYTES {
        return Err(fail(413, "message_too_large", "a notification is at most 1 MB"));
    }
    Ok(req)
}

/// What goes on the wire once every check passed.
#[derive(Debug, Clone)]
pub struct Message<'a> {
    pub from_name: &'a str,
    pub from: &'a str,
    pub to: &'a [String],
    pub cc: &'a [String],
    pub subject: &'a str,
    pub text: Option<&'a str>,
    pub html: Option<&'a str>,
    pub reply_to: &'a [String],
    pub headers: &'a BTreeMap<String, String>,
    pub message_id: &'a str,
}

/// E8: the RFC 8621 Email object for `Email/set create` (no
/// `mailboxIds`: the JMAP layer adds it). Bcc never appears: it rides the
/// envelope only.
pub fn email_json(m: &Message<'_>) -> J {
    let addr = |list: &[String]| -> Vec<J> { list.iter().map(|e| json!({ "email": e })).collect() };
    let mut body_values = serde_json::Map::new();
    let mut v = json!({
        "from": [{ "name": m.from_name, "email": m.from }],
        "messageId": [m.message_id],
        "subject": m.subject,
        "keywords": { "$seen": true },
    });
    if !m.to.is_empty() {
        v["to"] = J::Array(addr(m.to));
    }
    if !m.cc.is_empty() {
        v["cc"] = J::Array(addr(m.cc));
    }
    if !m.reply_to.is_empty() {
        v["replyTo"] = J::Array(addr(m.reply_to));
    }
    if let Some(t) = m.text {
        body_values.insert("k2text".into(), json!({ "value": t }));
        v["textBody"] = json!([{ "partId": "k2text", "type": "text/plain" }]);
    }
    if let Some(h) = m.html {
        body_values.insert("k2html".into(), json!({ "value": h }));
        v["htmlBody"] = json!([{ "partId": "k2html", "type": "text/html" }]);
    }
    v["bodyValues"] = J::Object(body_values);
    for (k, val) in m.headers {
        v[format!("header:{k}:asText")] = json!(val);
    }
    v
}

/// The envelope `mailFrom.parameters` (E8): headers-only DSNs, K2's log
/// id as the envelope id.
pub fn envelope_params(log_id: &str) -> J {
    json!({ "RET": "HDRS", "ENVID": log_id })
}

/// E12: the RFC 5322 message for a relay: `multipart/alternative` when
/// there are both parts, else one part; Reply-To, K2's Message-ID and the
/// allowed headers. Bcc never appears (envelope only).
pub fn rfc822(m: &Message<'_>, date: std::time::SystemTime) -> Result<Vec<u8>, String> {
    use lettre::message::{Mailbox, MultiPart, SinglePart};
    let mb = |name: Option<&str>, email: &str| -> Result<Mailbox, String> {
        let a: lettre::Address = email.parse().map_err(|e| format!("address: {e}"))?;
        Ok(Mailbox::new(name.map(str::to_string), a))
    };
    let mut b = lettre::Message::builder()
        .from(mb(Some(m.from_name), m.from)?)
        .date(date)
        .subject(m.subject)
        .message_id(Some(format!("<{}>", m.message_id.trim_matches(|c| c == '<' || c == '>'))));
    for a in m.to {
        b = b.to(mb(None, a)?);
    }
    for a in m.cc {
        b = b.cc(mb(None, a)?);
    }
    for a in m.reply_to {
        b = b.reply_to(mb(None, a)?);
    }
    let msg = match (m.text, m.html) {
        (Some(t), Some(h)) => b.multipart(MultiPart::alternative_plain_html(t.to_string(), h.to_string())),
        (Some(t), None) => b.singlepart(SinglePart::plain(t.to_string())),
        (None, Some(h)) => b.singlepart(SinglePart::html(h.to_string())),
        (None, None) => return Err("compose: no body".to_string()),
    }
    .map_err(|e| format!("compose: {e}"))?;
    let formatted = msg.formatted();
    if m.headers.is_empty() {
        return Ok(formatted);
    }
    // The allowed X- headers (names and values checked by `parse`: ASCII,
    // no line breaks) go at the top of the header block.
    let mut out = Vec::with_capacity(formatted.len() + 256);
    for (k, v) in m.headers {
        if !header_name_allowed(k) || !header_value_ok(v) {
            return Err("compose: a header failed its check".to_string());
        }
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(&formatted);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(v: J) -> Request {
        parse(&v, 1_000_000).expect("valid")
    }

    fn code(v: J) -> &'static str {
        parse(&v, 1_000_000).expect_err("refused").code
    }

    #[test]
    fn a_full_request_parses_and_bcc_rides_the_envelope_only() {
        let r = ok(json!({
            "from": "noreply@brand.test", "fromName": "Brand", "to": ["A@Example.test", "a@example.test"], "cc": "c@example.test",
            "bcc": ["b@example.test", "c@example.test"], "subject": "Hello", "text": "hi", "html": "<p>hi</p>",
            "replyTo": "help@brand.test", "headers": {"X-App-Order": "123"}, "tags": ["welcome"], "idempotencyKey": "k1",
            "sendAt": 1_000_600,
        }));
        assert_eq!(r.to, vec!["a@example.test"], "normalized and de-duplicated");
        assert_eq!(r.envelope(), vec!["a@example.test", "c@example.test", "b@example.test"]);
        assert_eq!(r.send_at, Some(1_000_600));
        assert_eq!(r.idempotency_key.as_deref(), Some("k1"));
        let mid = "abc@brand.test".to_string();
        let e = email_json(&Message {
            from_name: "Brand",
            from: "noreply@brand.test",
            to: &r.to,
            cc: &r.cc,
            subject: &r.subject,
            text: r.text.as_deref(),
            html: r.html.as_deref(),
            reply_to: &r.reply_to,
            headers: &r.headers,
            message_id: &mid,
        });
        assert_eq!(e["replyTo"], json!([{"email": "help@brand.test"}]));
        assert_eq!(e["header:X-App-Order:asText"], "123");
        assert_eq!(e["textBody"][0]["type"], "text/plain");
        assert_eq!(e["htmlBody"][0]["type"], "text/html");
        assert_eq!(e["bodyValues"]["k2html"]["value"], "<p>hi</p>");
        assert!(e.get("bcc").is_none() && !e.to_string().contains("b@example.test"), "bcc never in the Email object: {e}");
        assert_eq!(envelope_params("enl_1"), json!({"RET": "HDRS", "ENVID": "enl_1"}));
    }

    #[test]
    fn headers_outside_the_allowlist_are_refused() {
        let one = |name: &str, val: &str| -> J {
            let mut m = serde_json::Map::new();
            m.insert(name.to_string(), json!(val));
            J::Object(m)
        };
        for h in ["Bcc", "From", "Reply-To", "X-K2-Thing", "x-k2-other", "List-Unsubscribe", "DKIM-Signature", "X-", "X-has space", "Content-Type"] {
            assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": one(h, "v")})), "header_not_allowed", "{h}");
        }
        for v in ["a\r\nBcc: evil@x.test", "a\nb", "tab\there", "é"] {
            assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": one("X-App-Ok", v)})), "header_not_allowed", "{v:?}");
        }
        let many: serde_json::Map<String, J> = (0..11).map(|i| (format!("X-App-H{i}"), json!("v"))).collect();
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": many})), "header_not_allowed");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": {"X-App-A": "1", "x-app-a": "2"}})), "header_not_allowed");
        assert!(header_name_allowed("X-App-Order-Id") && !header_name_allowed(&format!("X-App-{}", "a".repeat(65))));
    }

    #[test]
    fn composition_limits() {
        let tos: Vec<String> = (0..51).map(|i| format!("u{i}@x.test")).collect();
        assert_eq!(code(json!({"to": tos, "subject": "s", "text": "t"})), "too_many_recipients");
        let tos: Vec<String> = (0..30).map(|i| format!("u{i}@x.test")).collect();
        let bcc: Vec<String> = (30..51).map(|i| format!("u{i}@x.test")).collect();
        assert_eq!(code(json!({"to": tos, "bcc": bcc, "subject": "s", "text": "t"})), "too_many_recipients", "to + bcc together");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "a\r\nBcc: b@x.test", "text": "t"})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s".repeat(999), "text": "t"})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s"})), "usage", "a body is required");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "x".repeat(MAX_TEXT_BYTES + 1)})), "message_too_large");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "html": "x".repeat(MAX_HTML_BYTES + 1)})), "message_too_large");
        // The largest parts together still fit the 1 MB total.
        let big = parse(
            &json!({"to": "a@x.test", "subject": "s", "text": "x".repeat(MAX_TEXT_BYTES), "html": "y".repeat(MAX_HTML_BYTES), "headers": (0..10).map(|i| (format!("X-App-H{i}"), json!("z".repeat(998)))).collect::<serde_json::Map<String, J>>()}),
            1_000_000,
        )
        .expect("at the limits");
        assert!(big.body_bytes() <= MAX_TOTAL_BYTES);
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "replyTo": ["a@x.test", "b@x.test", "c@x.test", "d@x.test"]})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "tags": ["has space"]})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "idempotencyKey": "k".repeat(257)})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "fromName": "ceo@bank.test"})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "attachments": []})), "usage");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "sendAt": 900_000})), "usage", "the past");
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "sendAt": 1_000_000 + 8 * 86_400})), "usage");
        assert_eq!(code(json!({"to": "not-an-address", "subject": "s", "text": "t"})), "usage");
        assert!(parse(&json!({"to": "a@x.test", "subject": "s", "text": "t", "sendAt": 1_000_000 + 7 * 86_400}), 1_000_000).is_ok(), "7 days is the edge");
    }

    #[test]
    fn relay_provider_control_headers_are_refused() {
        // Security review H2: every provider keyword, any case, on both paths
        // (parse refuses; rfc822 re-checks before writing the relay message).
        let one = |name: &str| -> J {
            let mut m = serde_json::Map::new();
            m.insert(name.to_string(), json!("{\"to\":[\"victim@x.test\"]}"));
            json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": J::Object(m)})
        };
        for h in [
            "X-SMTPAPI", "x-smtpapi", "X-MC-BccAddress", "X-MC-Tags", "X-Mailgun-Dkim", "X-Mailgun-Sending-Ip", "X-Mailgun-Drop-Message",
            "X-SES-CONFIGURATION-SET", "X-SES-SOURCE-ARN", "X-SES-FROM-ARN", "X-PM-Message-Stream", "X-Postmark-Server-Token",
            "X-SG-EID", "X-MSYS-API", "X-MJ-CustomID", "X-Mailjet-Campaign", "X-Mailin-custom", "X-Order", "X-Priority", "X-App-",
        ] {
            assert_eq!(code(one(h)), "header_not_allowed", "{h}");
            assert!(!header_name_allowed(h), "{h}");
            let headers: BTreeMap<String, String> = [(h.to_string(), "1".to_string())].into();
            let raw = rfc822(
                &Message {
                    from_name: "B",
                    from: "noreply@brand.test",
                    to: &["a@x.test".to_string()],
                    cc: &[],
                    subject: "s",
                    text: Some("t"),
                    html: None,
                    reply_to: &[],
                    headers: &headers,
                    message_id: "m@brand.test",
                },
                std::time::SystemTime::UNIX_EPOCH,
            );
            assert!(raw.is_err(), "the relay path refuses {h} too");
        }
        for ok in ["X-App-Order", "x-app-order-id", "X-Entity-Ref-ID", "x-entity-ref-id"] {
            assert!(header_name_allowed(ok), "{ok}");
        }
    }

    #[test]
    fn every_address_is_a_plain_dot_atom() {
        // Security review M4: refused, never cleaned up.
        for bad in [
            "a\r\nX-Injected: y@victim.test",
            "a\n@victim.test",
            "a\u{0}b@victim.test",
            "a b@victim.test",
            "\"a b\"@victim.test",
            "<a>@victim.test",
            "a<b@victim.test",
            "a(b)@victim.test",
            "a,b@victim.test",
            ".a@victim.test",
            "a.@victim.test",
            "a..b@victim.test",
            "a@victim.test\r\n",
            "é@victim.test",
        ] {
            for key in ["to", "cc", "bcc", "replyTo"] {
                let mut v = json!({"to": "ok@x.test", "subject": "s", "text": "t"});
                v[key] = json!(bad);
                assert_eq!(code(v), "usage", "{key}: {bad:?}");
            }
            assert!(strict_address(bad).is_err(), "{bad:?}");
        }
        let long_local = format!("{}@x.test", "a".repeat(65));
        assert_eq!(code(json!({"to": long_local, "subject": "s", "text": "t"})), "usage", "local part ≤ 64");
        let long = format!("a@{}.test", "b".repeat(250));
        assert_eq!(code(json!({"to": long, "subject": "s", "text": "t"})), "usage", "address ≤ 254");
        assert_eq!(strict_address(" Dana.O'Neil+news@X.test ").expect("plain"), "dana.o'neil+news@x.test");
    }

    #[test]
    fn display_names_refuse_encoded_words_and_fold_for_comparison() {
        // Security review L6.
        assert_eq!(code(json!({"to": "a@x.test", "subject": "s", "text": "t", "fromName": "=?utf-8?B?c2VjdXJpdHlAYmFuay5jb20=?="})), "usage");
        assert_eq!(fold_name(" K 2  Team "), fold_name("k2team"));
        assert_ne!(fold_name("K2"), fold_name("K3"));
    }

    #[test]
    fn a_kept_request_rechecks_to_itself() {
        let r = ok(json!({"to": ["a@x.test"], "bcc": "b@x.test", "subject": "s", "html": "<p>h</p>", "headers": {"X-App-Id": "1"}, "tags": ["t"], "sendAt": 1_000_600}));
        recheck(&r, 1_000_700).expect("still valid at flush time");
        let mut tampered = r.clone();
        tampered.headers.insert("X-SMTPAPI".into(), "{}".into());
        assert_eq!(recheck(&tampered, 1_000_700).expect_err("refused").code, "header_not_allowed");
        let mut tampered = r;
        tampered.to = vec!["a\r\nb@x.test".into()];
        assert_eq!(recheck(&tampered, 1_000_700).expect_err("refused").code, "usage");
    }

    #[test]
    fn the_relay_message_is_alternative_with_reply_to_and_k2s_message_id() {
        let headers: BTreeMap<String, String> = [("X-App-Order".to_string(), "123".to_string())].into();
        let raw = rfc822(
            &Message {
                from_name: "Brand",
                from: "noreply@brand.test",
                to: &["a@x.test".to_string()],
                cc: &[],
                subject: "Hello",
                text: Some("plain body"),
                html: Some("<p>html body</p>"),
                reply_to: &["help@brand.test".to_string()],
                headers: &headers,
                message_id: "mid-1@brand.test",
            },
            std::time::SystemTime::UNIX_EPOCH,
        )
        .expect("compose");
        let s = String::from_utf8_lossy(&raw);
        assert!(s.starts_with("X-App-Order: 123\r\n"), "{s}");
        assert!(s.contains("multipart/alternative"), "{s}");
        assert!(s.contains("Reply-To: help@brand.test"), "{s}");
        assert!(s.contains("Message-ID: <mid-1@brand.test>"), "{s}");
        assert!(!s.to_ascii_lowercase().contains("\r\nbcc:"), "{s}");
    }
}
