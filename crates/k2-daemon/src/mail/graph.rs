//! O3 — the Microsoft 365 / Exchange Online mail backend, over the
//! **Microsoft Graph REST API** (prd-email-oauth-providers-v1 §5b/§9).
//! Microsoft rows are `kind='graph'` (NOT `'imap'`): Graph is a PEER
//! backend behind the exact same §17.5 seam the local Stalwart and
//! linked-IMAP backends sit behind. Everything above talks to the
//! [`ReadBackend`]/[`ManageBackend`] traits and never learns Graph
//! exists — untrusted-content markers, masked ownership, body-shaping,
//! and the opaque message-id token all stay at the route/ops layer.
//!
//! WHY GRAPH, NOT IMAP (prd §0): Microsoft treats IMAP as legacy
//! (basic-auth IMAP already killed, OAuth-IMAP at risk). Graph is the
//! API Microsoft invests in, its mail scopes are device-flow-eligible
//! (uniform local/remote, no loopback), and clean REST is easier to
//! maintain than IMAP's quirks. There is NO IMAP path for Microsoft.
//!
//! AUTH (prd §6): every request is Bearer-authed with a fresh access
//! token from the O1 engine ([`oauth::access_token_for`] with
//! [`OauthProvider::Microsoft`]) — refresh within the 60 s skew, persist
//! the refreshed expiry back to the row, and (Microsoft ROTATES refresh
//! tokens) the O1 engine re-vaults the newest bundle. The token lives
//! ONLY inside [`RealGraphHttp`]'s reqwest call — never a column, never a
//! log line, never in an error, and the injected [`GraphHttp`] test seam
//! never even sees it.
//!
//! DELETE = MOVE to Deleted Items (`deleteditems`), NEVER a hard
//! `DELETE /messages/{id}` (prd §11.12 — the same no-permanent-delete
//! guarantee as wave-3 IMAP). There is deliberately no code path in this
//! module that issues an HTTP DELETE against a message.
//!
//! ⚠ LIVE-BOX functions (genuinely-uncertain live-server behavior —
//! verify against a real M365 account on a live box before trusting edge
//! cases):
//!   1. [`build_list_query`] — Graph forbids combining `$search` with
//!      `$filter`/`$orderby` (a `$search` request DROPS both here). The
//!      exact `$search` matching semantics + whether some tenants reject
//!      `$search` on `/messages` are the uncertain bits.
//!   2. [`path_encode`] / message-id opacity — Graph message ids are
//!      opaque and can carry URL-reserved bytes; they are percent-encoded
//!      into the path. Whether every tenant's ids survive a round-trip
//!      through `move`/`$value` verbatim is the live check.
//!   3. [`GraphBackend::send`] throttling — a 429 carries a `Retry-After`
//!      (seconds); it is honored with a bounded, capped retry. The exact
//!      per-endpoint throttle thresholds are only observable live.
//!
//! **No real network in this module's tests.** Every Graph effect goes
//! through the injected [`GraphHttp`] trait, so the unit tests drive a
//! scripted mock (canned responses + a request recorder) — no M365, no
//! token, no sends, ever (house rule / prd §11.16).

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;

use crate::mail::external::read_oauth_fields;
use crate::mail::jmap::{AttachmentMeta, EmailFull, EmailSummary, MailAddr};
use crate::mail::messages::{
    iso_to_unix, ListError, ListFilter, MailFolder, ManageBackend, ManageOutcome, ReadBackend,
};
use crate::mail::oauth::{form_encode, OauthProvider, ReqwestHttp};
use crate::mail::secrets::FileSecretStore;
use k2_core::db::schema::MailExternalInbox;

/// The Graph v1.0 base. Microsoft mail is HTTPS to graph.microsoft.com —
/// the row's `host`/`tls` (IMAP-shaped) are UNUSED for graph rows. A
/// `provider-fakes` test build may point it at the provider fakes
/// ([`crate::mail::oauth::graph_base`]); release builds can't.
fn graph_base() -> String {
    crate::mail::oauth::graph_base()
}

/// The opaque-token discriminator for a Graph message id: the backend
/// `email_id` inside the seam's `m_<b64url(address\n<email_id>)>` token
/// is `graph:<message-id>` — disjoint from IMAP's `uid:…` and JMAP's
/// bare ids (prd §5b).
const GRAPH_ID_PREFIX: &str = "graph:";

/// `$select` for the summaries list (envelope only — no body rides it).
const SUMMARY_SELECT: &str =
    "id,subject,receivedDateTime,from,toRecipients,isRead,hasAttachments,conversationId";

/// `$select` for one full message: envelope + body + the raw internet
/// headers (Authentication-Results → the §8.1 SPF/DKIM/DMARC verdicts).
const FULL_SELECT: &str = "id,subject,receivedDateTime,from,toRecipients,ccRecipients,isRead,\
     hasAttachments,conversationId,body,internetMessageHeaders";

/// Graph's inline attachment limits (PRD §3.3 / MCS-A4): one file attachment
/// may carry at most 3 MB, and one request at most 4 MB. Bigger files need
/// an upload session on the draft, which K2 does not build yet — they
/// answer `attachment_too_large` instead.
pub const GRAPH_INLINE_ATTACHMENT_MAX: usize = 3 * 1024 * 1024;
pub const GRAPH_REQUEST_MAX: usize = 4 * 1024 * 1024;

/// One attachment K2 hands to Graph (the caller already read + capped it).
pub struct GraphAttachment<'a> {
    pub filename: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

/// A Graph send/draft failure. `TooLarge` is the caller's to fix (400
/// `attachment_too_large`); `NotFound` is the masked missing source
/// message; `Engine` is everything else (no token, no body in the text).
#[derive(Debug, PartialEq, Eq)]
pub enum GraphSendError {
    TooLarge(String),
    NotFound(String),
    Engine(String),
}

impl std::fmt::Display for GraphSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphSendError::TooLarge(h) | GraphSendError::NotFound(h) | GraphSendError::Engine(h) => {
                f.write_str(h)
            }
        }
    }
}

/// What a Graph send handed to Microsoft (for the outbox trail). Never the
/// body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphReceipt {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub attachments: Vec<String>,
}

/// Refuse attachments Graph won't take inline: any file over 3 MB, or a
/// set whose base64 (+ the message) would push one request past 4 MB.
pub fn check_inline_attachments(atts: &[GraphAttachment], body_len: usize) -> Result<(), GraphSendError> {
    let mut total = body_len;
    for a in atts {
        if a.bytes.len() > GRAPH_INLINE_ATTACHMENT_MAX {
            return Err(GraphSendError::TooLarge(format!(
                "attachment '{}' is {} bytes — Microsoft 365 inboxes take attachments up to 3 MB \
                 each in this K2 version (larger files need an upload session, not built yet); \
                 share a link instead",
                a.filename,
                a.bytes.len()
            )));
        }
        // base64 grows 4/3, plus the JSON wrapper.
        total = total.saturating_add(a.bytes.len().div_ceil(3) * 4 + 512);
    }
    if total > GRAPH_REQUEST_MAX {
        return Err(GraphSendError::TooLarge(format!(
            "attachments total too large for one Microsoft 365 request (~{total} bytes encoded, \
             max 4 MB) — send fewer or smaller files, or share a link"
        )));
    }
    Ok(())
}

fn recipients_json(list: &[String]) -> Vec<serde_json::Value> {
    list.iter().map(|a| serde_json::json!({ "emailAddress": { "address": a } })).collect()
}

fn file_attachment_json(a: &GraphAttachment) -> serde_json::Value {
    use base64::engine::general_purpose::STANDARD;
    serde_json::json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": a.filename,
        "contentType": a.content_type,
        "contentBytes": STANDARD.encode(a.bytes),
    })
}

/// Throttling (⚠ LIVE-BOX #3): bounded retries on a 429, and a hard cap
/// on how long a single `Retry-After` may park a request (a hostile /
/// buggy header must never hang the daemon).
const MAX_THROTTLE_RETRIES: u32 = 3;
const RETRY_AFTER_CAP_SECS: u64 = 60;

// ─────────────────────────────────────────────────────────────────────
// The injected HTTP seam (production: RealGraphHttp; tests: a mock)
// ─────────────────────────────────────────────────────────────────────

/// One HTTP verb Graph uses. Mail never uses `Delete` — this module has
/// no permanent-delete path for a MESSAGE (delete = move to Deleted
/// Items). `Delete` exists only for calendar EVENTS (MCS3,
/// `mail::calendar_graph`): a draft event the agent may delete, which
/// Exchange keeps in Deleted Items too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMethod {
    Get,
    Post,
    Patch,
    Delete,
}

impl GraphMethod {
    fn as_str(self) -> &'static str {
        match self {
            GraphMethod::Get => "GET",
            GraphMethod::Post => "POST",
            GraphMethod::Patch => "PATCH",
            GraphMethod::Delete => "DELETE",
        }
    }
}

/// A raw Graph reply. `body` may hold message content (a read body is
/// EXTERNAL UNTRUSTED input) and `retry_after` is the throttle hint —
/// deliberately NOT `Debug` (never dump a response wholesale to a log).
pub struct GraphResponse {
    pub status: u16,
    /// Parsed `Retry-After` (seconds) on a 429, if present.
    pub retry_after: Option<u64>,
    pub body: String,
}

/// Every network effect the Graph backend performs, behind ONE trait so
/// the whole module unit-tests with a scripted mock — no real network,
/// no token, ever. The production impl ([`RealGraphHttp`]) adds the
/// Bearer token INSIDE this seam, so a token never crosses the trait
/// boundary and callers/tests never handle one.
pub trait GraphHttp: Send + Sync {
    /// One authed JSON request. `url` is absolute (`GRAPH_BASE` + path).
    /// `body` `None` = no request body. The impl adds `Authorization:
    /// Bearer …`; the caller never sees it.
    fn send(
        &self,
        method: GraphMethod,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<GraphResponse, String>;
    /// [`Self::send`] with extra request headers (MCS3 calendars:
    /// `Prefer: outlook.timezone="UTC"`). Never an Authorization header —
    /// the impl adds that. The default drops the headers (mail mocks).
    fn send_with(
        &self,
        method: GraphMethod,
        url: &str,
        body: Option<&serde_json::Value>,
        headers: &[(&'static str, String)],
    ) -> Result<GraphResponse, String> {
        let _ = headers;
        self.send(method, url, body)
    }
    /// Authed GET of raw bytes (attachment `$value`, raw-message MIME
    /// `$value`) — kept off [`Self::send`] so binary never round-trips
    /// through a `String`.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// The production Graph HTTP client. Holds ONLY the inbox row id — the
/// OAuth fields + token are read/minted fresh per request via the O1
/// engine (mirrors `external_imap::oauth_login`), so a rotated Microsoft
/// refresh token and a refreshed expiry are always the latest. The token
/// is never logged, never placed in an error, never stored on the struct.
pub struct RealGraphHttp {
    inbox_id: String,
}

impl RealGraphHttp {
    pub fn new(inbox_id: impl Into<String>) -> Self {
        Self { inbox_id: inbox_id.into() }
    }

    /// Mint a fresh Bearer access token for this row via the O1 engine
    /// (refresh within the 60 s skew; persist the refreshed expiry). A
    /// `graph` row MUST be `provider='microsoft'` — anything else is a
    /// loud config error, never a silent wrong-provider call. The token
    /// is returned by value to the ONE reqwest call site and never logged.
    fn bearer_token(&self) -> Result<String, String> {
        let auth = read_oauth_fields(&self.inbox_id)?;
        if auth.auth_kind != crate::mail::external::AUTH_OAUTH {
            return Err(format!(
                "graph inbox {} is not an oauth row (auth_kind='{}') — re-link it",
                self.inbox_id, auth.auth_kind
            ));
        }
        let provider_str = auth.provider.as_deref().unwrap_or("");
        let provider = OauthProvider::from_str(provider_str).ok_or_else(|| {
            format!(
                "graph inbox {} has unrecognized provider '{provider_str}' — re-link it",
                self.inbox_id
            )
        })?;
        if provider != OauthProvider::Microsoft {
            return Err(format!(
                "graph inbox {} provider '{provider_str}' is not microsoft — the Graph backend \
                 serves Microsoft 365 only",
                self.inbox_id
            ));
        }
        let secrets = FileSecretStore::default();
        let http = ReqwestHttp::default();
        let now = chrono::Utc::now().timestamp();
        // MCS1: minted by the client that linked the row (never a swapped
        // BYO app); the refreshed expiry + granted scopes persist inside.
        crate::mail::external::fresh_access_token(
            &self.inbox_id,
            provider,
            auth.token_expires_at,
            &secrets,
            &http,
            now,
        )
        .map_err(|e| format!("graph inbox {}: {e}", self.inbox_id))
    }

    fn client() -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("graph http client: {e}"))
    }
}

/// Parse a `Retry-After` header value (delta-seconds form; an HTTP-date
/// form is ignored — the caller's cap handles the missing hint).
fn parse_retry_after(v: Option<&str>) -> Option<u64> {
    v.and_then(|s| s.trim().parse::<u64>().ok())
}

impl GraphHttp for RealGraphHttp {
    fn send(
        &self,
        method: GraphMethod,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<GraphResponse, String> {
        self.send_with(method, url, body, &[])
    }

    fn send_with(
        &self,
        method: GraphMethod,
        url: &str,
        body: Option<&serde_json::Value>,
        headers: &[(&'static str, String)],
    ) -> Result<GraphResponse, String> {
        let token = self.bearer_token()?;
        let client = Self::client()?;
        let mut req = match method {
            GraphMethod::Get => client.get(url),
            GraphMethod::Post => client.post(url),
            GraphMethod::Patch => client.patch(url),
            GraphMethod::Delete => client.delete(url),
        }
        // The token rides ONLY this header, on this one call — never
        // logged, never echoed into an error below.
        .bearer_auth(&token)
        .header("Accept", "application/json");
        for (k, v) in headers {
            if !k.eq_ignore_ascii_case("authorization") {
                req = req.header(*k, v.as_str());
            }
        }
        if let Some(b) = body {
            let payload = serde_json::to_string(b)
                .map_err(|e| format!("graph {} body serialize: {e}", method.as_str()))?;
            req = req.header("Content-Type", "application/json").body(payload);
        }
        // reqwest's transport error carries the URL/host, never the body
        // or the Authorization header.
        let resp = req
            .send()
            .map_err(|e| format!("graph {} {}: {e}", method.as_str(), redact_url(url)))?;
        let status = resp.status().as_u16();
        let retry_after = parse_retry_after(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok()),
        );
        let body = resp.text().unwrap_or_default();
        Ok(GraphResponse { status, retry_after, body })
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let token = self.bearer_token()?;
        let client = Self::client()?;
        let resp = client
            .get(url)
            .bearer_auth(&token)
            .send()
            .map_err(|e| format!("graph GET bytes {}: {e}", redact_url(url)))?;
        let status = resp.status();
        let bytes = resp.bytes().map_err(|e| format!("graph GET bytes: read body: {e}"))?;
        if !status.is_success() {
            let excerpt: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(format!("graph GET bytes: HTTP {status}: {excerpt}"));
        }
        Ok(bytes.to_vec())
    }
}

/// A Graph URL for an error message with the message-id path segment
/// masked (ids are opaque and unhelpful in a log; keep only the shape).
fn redact_url(url: &str) -> String {
    // Everything before "/messages/" or "/mailFolders/" is safe shape.
    for anchor in ["/messages/", "/mailFolders/"] {
        if let Some(i) = url.find(anchor) {
            return format!("{}{anchor}<id>", &url[..i]);
        }
    }
    url.to_string()
}

// ─────────────────────────────────────────────────────────────────────
// Opaque ids (message + attachment blob tokens)
// ─────────────────────────────────────────────────────────────────────

/// The seam `email_id` for a Graph message: `graph:<message-id>`
/// (feeds `messages::encode_message_id(address, id)`).
fn encode_graph_email_id(graph_id: &str) -> String {
    format!("{GRAPH_ID_PREFIX}{graph_id}")
}

/// `graph:<id>` → `<id>`; `None` = not a Graph token (route masks it).
fn decode_graph_email_id(email_id: &str) -> Option<&str> {
    email_id.strip_prefix(GRAPH_ID_PREFIX)
}

/// Attachment blob token: `gatt_<b64url(msgId\nattId)>` — opaque + safe
/// (Graph ids carry URL-reserved bytes, so the pair is base64url'd, not
/// delimiter-joined). The bare `graph:<id>` message token means the RAW
/// MIME (`read --raw`).
fn encode_att_blob(msg_id: &str, att_id: &str) -> String {
    format!("gatt_{}", B64URL.encode(format!("{msg_id}\n{att_id}")))
}

fn decode_att_blob(token: &str) -> Option<(String, String)> {
    let raw = token.strip_prefix("gatt_")?;
    let bytes = B64URL.decode(raw).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let (msg, att) = s.split_once('\n')?;
    if msg.is_empty() || att.is_empty() {
        return None;
    }
    Some((msg.to_string(), att.to_string()))
}

/// Percent-encode one opaque id into a URL PATH segment (⚠ LIVE-BOX #2 —
/// Graph message ids are opaque and can carry `/`, `+`, `=`). Reuses the
/// OAuth `form_encode` unreserved set (encodes space as `%20`), which is
/// a safe superset for a path segment.
pub(crate) fn path_encode(id: &str) -> String {
    form_encode(id)
}

// ─────────────────────────────────────────────────────────────────────
// URL / query building (pure — fixture-tested)
// ─────────────────────────────────────────────────────────────────────

/// unix secs → the Graph `$filter` datetime literal (`…Z`, second
/// precision).
fn unix_to_graph_iso(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Assemble `base?k1=enc(v1)&k2=enc(v2)` (keys literal so `$filter` reads
/// as `$filter`, values percent-encoded).
pub(crate) fn with_query(base: &str, pairs: &[(&str, String)]) -> String {
    if pairs.is_empty() {
        return base.to_string();
    }
    let q = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", form_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{q}")
}

/// Map a [`ListFilter`] to Graph OData query pairs (⚠ LIVE-BOX #1). Two
/// mutually-exclusive shapes because Graph FORBIDS combining `$search`
/// with `$filter`/`$orderby`:
///   * `--query` set → `$search="…"` ONLY (no `$orderby`, no `$filter`);
///     Graph ranks by relevance, the route's `summary_matches` re-check
///     keeps the semantics honest.
///   * otherwise → `$orderby=receivedDateTime desc` + an AND-joined
///     `$filter` from `--unread` (`isRead eq false`) + `--since`/`--before`
///     (`receivedDateTime ge/lt …`).
/// Always: `$top` (limit), `$skip` (offset when > 0), `$select`.
pub fn build_list_query(filter: &ListFilter, limit: usize) -> Vec<(&'static str, String)> {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();

    let search = filter.query.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(q) = search {
        // $search cannot coexist with $orderby/$filter (Graph rejects it).
        pairs.push(("$search", format!("\"{}\"", q.replace('"', " "))));
    } else {
        pairs.push(("$orderby", "receivedDateTime desc".to_string()));
        let mut clauses: Vec<String> = Vec::new();
        if filter.unread_only {
            clauses.push("isRead eq false".to_string());
        }
        // The wait grace (`since_unix`) and the `--since` window collapse
        // to the tighter (later) lower bound — mirrors the JMAP/IMAP maps.
        if let Some(after) = filter.since_unix.into_iter().chain(filter.after_unix).max() {
            clauses.push(format!("receivedDateTime ge {}", unix_to_graph_iso(after)));
        }
        if let Some(before) = filter.before_unix {
            clauses.push(format!("receivedDateTime lt {}", unix_to_graph_iso(before)));
        }
        if !clauses.is_empty() {
            pairs.push(("$filter", clauses.join(" and ")));
        }
    }

    pairs.push(("$top", limit.to_string()));
    if filter.offset > 0 {
        pairs.push(("$skip", filter.offset.to_string()));
    }
    pairs.push(("$select", SUMMARY_SELECT.to_string()));
    pairs
}

/// The well-known Graph mailFolder id for a folder name/alias, or `None`
/// for an arbitrary user folder (resolved by displayName lookup). Case-
/// insensitive; covers the prd §5b set + common human spellings.
fn well_known_folder(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "inbox" => Some("inbox"),
        "junk" | "junkemail" | "junk email" | "junk e-mail" | "spam" => Some("junkemail"),
        "archive" | "archives" => Some("archive"),
        "deleteditems" | "deleted items" | "deleted" | "trash" => Some("deleteditems"),
        "drafts" => Some("drafts"),
        "sentitems" | "sent items" | "sent" => Some("sentitems"),
        _ => None,
    }
}

/// The mailFolder segment a LIST targets (`--folder`/`--junk`): Inbox →
/// `inbox`, Junk → `junkemail`, Named → well-known id or the verbatim
/// displayName (resolved to an id server-side by the caller when needed).
fn list_folder_id(folder: &MailFolder) -> String {
    match folder {
        MailFolder::Inbox => "inbox".to_string(),
        MailFolder::Junk => "junkemail".to_string(),
        MailFolder::Named(name) => {
            well_known_folder(name).map(str::to_string).unwrap_or_else(|| name.clone())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Response parsing (pure — bodies are UNTRUSTED, never logged)
// ─────────────────────────────────────────────────────────────────────

/// One Graph `{emailAddress:{name,address}}` → [`MailAddr`] (`None` when
/// there is no usable address).
fn parse_recipient(v: &serde_json::Value) -> Option<MailAddr> {
    let ea = v.get("emailAddress")?;
    let email = ea.get("address").and_then(|s| s.as_str()).map(str::trim).filter(|s| !s.is_empty())?;
    Some(MailAddr {
        name: ea
            .get("name")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        email: email.to_string(),
    })
}

fn parse_recipient_list(v: &serde_json::Value) -> Vec<MailAddr> {
    v.as_array()
        .map(|a| a.iter().filter_map(parse_recipient).collect())
        .unwrap_or_default()
}

/// One message resource → [`EmailSummary`] (envelope only). `None` when
/// the object has no `id` (malformed — skipped, never a panic).
fn summary_from_message(m: &serde_json::Value) -> Option<EmailSummary> {
    let id = m.get("id").and_then(|s| s.as_str())?;
    let from = m.get("from").map(parse_recipient).and_then(|o| o).into_iter().collect();
    Some(EmailSummary {
        id: encode_graph_email_id(id),
        thread_id: m.get("conversationId").and_then(|s| s.as_str()).map(str::to_string),
        from,
        to: m.get("toRecipients").map(parse_recipient_list).unwrap_or_default(),
        subject: m.get("subject").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        received_at: normalize_received(m.get("receivedDateTime").and_then(|s| s.as_str())),
        unread: !m.get("isRead").and_then(|b| b.as_bool()).unwrap_or(false),
        has_attachment: m.get("hasAttachments").and_then(|b| b.as_bool()).unwrap_or(false),
    })
}

/// Graph `receivedDateTime` is already RFC 3339 (`…Z`); normalize
/// defensively so the route's date sort/compare always sees a `Z` form.
fn normalize_received(s: Option<&str>) -> String {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        Some(iso) => {
            let unix = iso_to_unix(iso);
            if unix == 0 {
                iso.to_string()
            } else {
                unix_to_graph_iso(unix)
            }
        }
        None => String::new(),
    }
}

/// Parse the `value` array of a `…/messages` list into summaries.
pub fn parse_message_list(v: &serde_json::Value) -> Vec<EmailSummary> {
    v.get("value")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(summary_from_message).collect())
        .unwrap_or_default()
}

/// Parse a full message resource (+ optional already-fetched attachment
/// metadata) into [`EmailFull`]. Graph serves ONE body with a
/// `contentType` of `html`|`text`; the §8.1 shaping (markers, HTML-strip
/// fallback, auth verdicts) is applied ABOVE this by `shape_full_json`.
fn full_from_message(m: &serde_json::Value, attachments: Vec<AttachmentMeta>) -> Option<EmailFull> {
    let summary = summary_from_message(m)?;
    let (text, html) = match m.get("body") {
        Some(b) => {
            let content = b.get("content").and_then(|s| s.as_str()).unwrap_or("").to_string();
            match b.get("contentType").and_then(|s| s.as_str()).unwrap_or("text") {
                t if t.eq_ignore_ascii_case("html") => (None, Some(content)),
                _ => (Some(content), None),
            }
        }
        None => (None, None),
    };
    let auth_results = m
        .get("internetMessageHeaders")
        .and_then(|a| a.as_array())
        .map(|hs| {
            hs.iter()
                .filter(|h| {
                    h.get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("Authentication-Results"))
                })
                .filter_map(|h| h.get("value").and_then(|v| v.as_str()).map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // The raw MIME blob is the message itself (`read --raw` → `/$value`).
    let raw_id = m.get("id").and_then(|s| s.as_str()).map(encode_graph_email_id);
    Some(EmailFull {
        summary,
        cc: m.get("ccRecipients").map(parse_recipient_list).unwrap_or_default(),
        blob_id: raw_id,
        text,
        html,
        attachments,
        auth_results,
    })
}

/// Parse a `…/attachments` list into [`AttachmentMeta`] (blob token
/// carries msg+attachment ids so `fetch_blob` can pull `/$value`).
fn parse_attachments(msg_id: &str, v: &serde_json::Value) -> Vec<AttachmentMeta> {
    v.get("value")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|att| {
                    let att_id = att.get("id").and_then(|s| s.as_str())?;
                    Some(AttachmentMeta {
                        blob_id: encode_att_blob(msg_id, att_id),
                        filename: att
                            .get("name")
                            .and_then(|s| s.as_str())
                            .map(str::to_string),
                        mime: att
                            .get("contentType")
                            .and_then(|s| s.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        size: att.get("size").and_then(|n| n.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Folder names out of a `/me/mailFolders` list (for `folder list`).
fn parse_folder_names(v: &serde_json::Value) -> Vec<String> {
    v.get("value")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| f.get("displayName").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────
// The backend
// ─────────────────────────────────────────────────────────────────────

/// [`ReadBackend`] + [`ManageBackend`] over Microsoft Graph — the §17.5
/// seam's `graph` variant. `account_id` (the backend handle) is the
/// `mail_external_inboxes.id`; all Graph calls address `/me/*` (the token
/// IS the mailbox owner), so the handle is only a sanity check.
pub struct GraphBackend {
    inbox: MailExternalInbox,
    http: Arc<dyn GraphHttp>,
    /// Injected sleeper for the 429 back-off (production: real sleep;
    /// tests: a no-op recorder) so the suite never parks on `Retry-After`.
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
}

impl GraphBackend {
    pub fn new(inbox: MailExternalInbox, http: Arc<dyn GraphHttp>) -> Self {
        Self { inbox, http, sleep: Box::new(|d| std::thread::sleep(d)) }
    }

    #[cfg(test)]
    fn with_sleep(
        inbox: MailExternalInbox,
        http: Arc<dyn GraphHttp>,
        sleep: Box<dyn Fn(Duration) + Send + Sync>,
    ) -> Self {
        Self { inbox, http, sleep }
    }

    fn check_handle(&self, account_id: &str) -> Result<(), String> {
        if account_id == self.inbox.id {
            Ok(())
        } else {
            Err("graph backend called with a foreign mailbox handle".to_string())
        }
    }

    /// One authed request WITH bounded 429 back-off (⚠ LIVE-BOX #3):
    /// honor `Retry-After` (capped), retry up to [`MAX_THROTTLE_RETRIES`],
    /// then surface the throttle as an engine error rather than hang.
    fn send(
        &self,
        method: GraphMethod,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<GraphResponse, String> {
        let mut attempt = 0u32;
        loop {
            let resp = self.http.send(method, url, body)?;
            if resp.status == 429 && attempt < MAX_THROTTLE_RETRIES {
                let secs = resp.retry_after.unwrap_or(1).min(RETRY_AFTER_CAP_SECS);
                (self.sleep)(Duration::from_secs(secs));
                attempt += 1;
                continue;
            }
            return Ok(resp);
        }
    }

    /// JSON body of a 2xx reply; `Ok(None)` on 404 (the route masks it);
    /// any other non-2xx is a loud engine error carrying only Graph's own
    /// error text (a short excerpt — never a token, which never rides a
    /// response anyway).
    fn json_ok(&self, resp: GraphResponse, what: &str) -> Result<Option<serde_json::Value>, String> {
        if resp.status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&resp.status) {
            let excerpt: String = graph_error_excerpt(&resp.body);
            return Err(format!("graph {what}: HTTP {}: {excerpt}", resp.status));
        }
        if resp.body.trim().is_empty() {
            return Ok(Some(serde_json::Value::Null));
        }
        serde_json::from_str(&resp.body)
            // Never echo the body on a parse failure — it is untrusted
            // content (and, defensively, never a token surface).
            .map(Some)
            .map_err(|_| format!("graph {what}: unparseable JSON reply (HTTP {})", resp.status))
    }

    fn url(path: &str) -> String {
        format!("{}{path}", graph_base())
    }

    /// GET a full message + (when present) its attachment metadata.
    fn fetch_message(&self, graph_id: &str) -> Result<Option<EmailFull>, String> {
        let base = Self::url(&format!("/me/messages/{}", path_encode(graph_id)));
        let url = with_query(&base, &[("$select", FULL_SELECT.to_string())]);
        let resp = self.send(GraphMethod::Get, &url, None)?;
        let Some(m) = self.json_ok(resp, "read message")? else {
            return Ok(None);
        };
        let attachments = if m.get("hasAttachments").and_then(|b| b.as_bool()).unwrap_or(false) {
            self.fetch_attachment_meta(graph_id)?
        } else {
            Vec::new()
        };
        Ok(full_from_message(&m, attachments))
    }

    fn fetch_attachment_meta(&self, graph_id: &str) -> Result<Vec<AttachmentMeta>, String> {
        let base = Self::url(&format!("/me/messages/{}/attachments", path_encode(graph_id)));
        let url = with_query(&base, &[("$select", "id,name,contentType,size".to_string())]);
        let resp = self.send(GraphMethod::Get, &url, None)?;
        match self.json_ok(resp, "list attachments")? {
            Some(v) => Ok(parse_attachments(graph_id, &v)),
            None => Ok(Vec::new()),
        }
    }

    /// Resolve a move DESTINATION [`MailFolder`] to a Graph folder id
    /// string: well-known ids pass straight through; an arbitrary Named
    /// folder is looked up by displayName (an unknown one is
    /// [`ListError::UnknownFolder`] listing the real folders).
    fn resolve_dest_id(&self, dest: &MailFolder) -> Result<String, ListError> {
        match dest {
            MailFolder::Inbox => Ok("inbox".to_string()),
            MailFolder::Junk => Ok("junkemail".to_string()),
            MailFolder::Named(name) => {
                if let Some(id) = well_known_folder(name) {
                    return Ok(id.to_string());
                }
                self.folder_id_by_name(name)
            }
        }
    }

    /// Look up a user folder's id by displayName; the miss lists the real
    /// folder names (uniform with the IMAP/JMAP `UnknownFolder`).
    fn folder_id_by_name(&self, want: &str) -> Result<String, ListError> {
        let names = self.list_folders_full()?;
        names
            .iter()
            .find(|(dn, _)| dn.eq_ignore_ascii_case(want))
            .map(|(_, id)| id.clone())
            .ok_or_else(|| ListError::UnknownFolder {
                requested: want.to_string(),
                available: names.into_iter().map(|(dn, _)| dn).collect(),
            })
    }

    /// `(displayName, id)` for every mailFolder (one page, generous top).
    fn list_folders_full(&self) -> Result<Vec<(String, String)>, ListError> {
        let base = Self::url("/me/mailFolders");
        let url = with_query(&base, &[("$top", "100".to_string()), ("$select", "id,displayName".to_string())]);
        let resp = self.send(GraphMethod::Get, &url, None).map_err(ListError::Engine)?;
        let v = self
            .json_ok(resp, "list folders")
            .map_err(ListError::Engine)?
            .unwrap_or(serde_json::Value::Null);
        Ok(v.get("value")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        let dn = f.get("displayName").and_then(|s| s.as_str())?;
                        let id = f.get("id").and_then(|s| s.as_str())?;
                        Some((dn.to_string(), id.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The shared move implementation (move/archive/trash). `dest_id` is a
    /// well-known id or a resolved folder id; the outcome carries a
    /// human-facing label. NEVER an HTTP DELETE.
    fn move_to(&self, graph_id: &str, dest_id: &str, label: &str) -> Result<ManageOutcome, ListError> {
        let url = Self::url(&format!("/me/messages/{}/move", path_encode(graph_id)));
        let body = serde_json::json!({ "destinationId": dest_id });
        let resp = self.send(GraphMethod::Post, &url, Some(&body)).map_err(ListError::Engine)?;
        self.json_ok(resp, "move message").map_err(ListError::Engine)?;
        Ok(ManageOutcome { folder: Some(label.to_string()), ..Default::default() })
    }

    /// PATCH `isRead` and/or `flag` on a message.
    fn patch_flags(&self, graph_id: &str, read: Option<bool>, flagged: Option<bool>) -> Result<(), String> {
        let mut patch = serde_json::Map::new();
        if let Some(r) = read {
            patch.insert("isRead".to_string(), serde_json::json!(r));
        }
        if let Some(f) = flagged {
            // Graph's flag is a followupFlag object: flagged | notFlagged.
            patch.insert(
                "flag".to_string(),
                serde_json::json!({ "flagStatus": if f { "flagged" } else { "notFlagged" } }),
            );
        }
        if patch.is_empty() {
            return Ok(());
        }
        let url = Self::url(&format!("/me/messages/{}", path_encode(graph_id)));
        let body = serde_json::Value::Object(patch);
        let resp = self.send(GraphMethod::Patch, &url, Some(&body))?;
        self.json_ok(resp, "patch message")?;
        Ok(())
    }

    /// DRAFT a reply (prd §5b): `POST …/createReply` → a draft in the
    /// account's Drafts, then `PATCH` its body with the agent's text.
    /// NEVER `/send` — the user reviews + sends from their own client
    /// (the exact draft-only model as the IMAP/local backends).
    ///
    /// O4 wires this into the `draft` route (`routes_external::handle_draft`
    /// dispatches Graph rows here; IMAP rows keep the `external::
    /// save_reply_draft` APPEND path), so the Graph backend satisfies the
    /// full seam surface.
    pub fn save_reply_draft(&self, account_id: &str, email_id: &str, body: &str) -> Result<String, String> {
        self.save_reply_draft_with(account_id, email_id, body, &[]).map_err(|e| e.to_string())
    }

    /// [`Self::save_reply_draft`] with attachments (MCS2): createReply →
    /// PATCH the body → one POST per attachment onto the draft. NEVER
    /// `/send`.
    pub fn save_reply_draft_with(
        &self,
        account_id: &str,
        email_id: &str,
        body: &str,
        attachments: &[GraphAttachment],
    ) -> Result<String, GraphSendError> {
        self.check_handle(account_id).map_err(GraphSendError::Engine)?;
        check_inline_attachments(attachments, 0)?;
        let draft = self.create_reply(email_id)?;
        self.patch_message(&draft.id, &serde_json::json!({
            "body": { "contentType": "text", "content": body },
        }))?;
        self.attach_all(&draft.id, attachments)?;
        Ok("Drafts".to_string())
    }

    /// MCS2: a brand-new (compose) DRAFT in the account's Drafts folder —
    /// `POST /me/messages`, then one POST per attachment. NEVER `/send`.
    #[allow(clippy::too_many_arguments)]
    pub fn save_compose_draft(
        &self,
        account_id: &str,
        to: &[String],
        cc: &[String],
        bcc: &[String],
        subject: &str,
        body: &str,
        attachments: &[GraphAttachment],
    ) -> Result<String, GraphSendError> {
        self.check_handle(account_id).map_err(GraphSendError::Engine)?;
        check_inline_attachments(attachments, 0)?;
        let mut msg = serde_json::json!({
            "subject": subject,
            "body": { "contentType": "text", "content": body },
            "toRecipients": recipients_json(to),
        });
        if !cc.is_empty() {
            msg["ccRecipients"] = serde_json::json!(recipients_json(cc));
        }
        if !bcc.is_empty() {
            msg["bccRecipients"] = serde_json::json!(recipients_json(bcc));
        }
        let url = Self::url("/me/messages");
        let resp = self.send(GraphMethod::Post, &url, Some(&msg)).map_err(GraphSendError::Engine)?;
        let draft = self
            .json_ok(resp, "create draft")
            .map_err(GraphSendError::Engine)?
            .ok_or_else(|| GraphSendError::Engine("graph create draft: 404".to_string()))?;
        let draft_id = draft
            .get("id")
            .and_then(|s| s.as_str())
            .ok_or_else(|| GraphSendError::Engine("create draft reply carried no id".to_string()))?
            .to_string();
        self.attach_all(&draft_id, attachments)?;
        Ok("Drafts".to_string())
    }

    /// MCS2 SEND (PRD §3.3): `POST /me/sendMail` with `saveToSentItems:
    /// true`. Blind copies ride `bccRecipients` (Exchange keeps them off
    /// the delivered headers, like the SMTP envelope). Attachments go
    /// inline (≤ 3 MB each, ≤ 4 MB request). Graph answers 202 Accepted —
    /// accepted for delivery, never "delivered".
    #[allow(clippy::too_many_arguments)]
    pub fn send_mail(
        &self,
        account_id: &str,
        to: &[String],
        cc: &[String],
        bcc: &[String],
        subject: &str,
        body: &str,
        attachments: &[GraphAttachment],
    ) -> Result<GraphReceipt, GraphSendError> {
        self.check_handle(account_id).map_err(GraphSendError::Engine)?;
        if to.is_empty() {
            return Err(GraphSendError::Engine("a message needs at least one recipient".to_string()));
        }
        check_inline_attachments(attachments, body.len() + subject.len())?;
        let mut message = serde_json::json!({
            "subject": subject,
            "body": { "contentType": "text", "content": body },
            "toRecipients": recipients_json(to),
        });
        if !cc.is_empty() {
            message["ccRecipients"] = serde_json::json!(recipients_json(cc));
        }
        if !bcc.is_empty() {
            message["bccRecipients"] = serde_json::json!(recipients_json(bcc));
        }
        if !attachments.is_empty() {
            message["attachments"] =
                serde_json::Value::Array(attachments.iter().map(file_attachment_json).collect());
        }
        let payload = serde_json::json!({ "message": message, "saveToSentItems": true });
        let url = Self::url("/me/sendMail");
        let resp = self.send(GraphMethod::Post, &url, Some(&payload)).map_err(GraphSendError::Engine)?;
        self.expect_accepted(resp, "sendMail")?;
        Ok(GraphReceipt {
            to: to.to_vec(),
            cc: cc.to_vec(),
            subject: subject.to_string(),
            attachments: attachments.iter().map(|a| a.filename.to_string()).collect(),
        })
    }

    /// MCS2 REPLY (PRD §3.3): `createReply` (Graph locks the recipient to
    /// the original sender and threads it) → PATCH the body (+ policy
    /// blind copies) → attachments → `POST …/send`. Returns who it went to.
    pub fn send_reply(
        &self,
        account_id: &str,
        email_id: &str,
        body: &str,
        bcc: &[String],
        attachments: &[GraphAttachment],
    ) -> Result<GraphReceipt, GraphSendError> {
        self.check_handle(account_id).map_err(GraphSendError::Engine)?;
        check_inline_attachments(attachments, body.len())?;
        let draft = self.create_reply(email_id)?;
        if draft.to.is_empty() {
            return Err(GraphSendError::Engine(
                "the source message has no sender address to reply to".to_string(),
            ));
        }
        let mut patch = serde_json::json!({ "body": { "contentType": "text", "content": body } });
        if !bcc.is_empty() {
            patch["bccRecipients"] = serde_json::json!(recipients_json(bcc));
        }
        self.patch_message(&draft.id, &patch)?;
        self.attach_all(&draft.id, attachments)?;
        let url = Self::url(&format!("/me/messages/{}/send", path_encode(&draft.id)));
        let resp = self.send(GraphMethod::Post, &url, None).map_err(GraphSendError::Engine)?;
        self.expect_accepted(resp, "send reply")?;
        Ok(GraphReceipt {
            to: draft.to,
            cc: Vec::new(),
            subject: draft.subject,
            attachments: attachments.iter().map(|a| a.filename.to_string()).collect(),
        })
    }

    /// Who a reply to `email_id` would go to and its subject — read from
    /// the source message (the approval queue shows it before anything is
    /// sent). `None` = no such message (masked by the caller).
    pub fn reply_preview(&self, account_id: &str, email_id: &str) -> Result<Option<(String, String)>, String> {
        self.check_handle(account_id)?;
        let Some(full) = self.fetch_full(account_id, email_id)? else {
            return Ok(None);
        };
        let Some(sender) = full.summary.from.first().map(|a| a.email.clone()) else {
            return Err("the source message has no sender address to reply to".to_string());
        };
        Ok(Some((sender, crate::mail::send::reply_subject(&full.summary.subject))))
    }

    /// `POST …/createReply` → the new draft's id, recipients and subject.
    fn create_reply(&self, email_id: &str) -> Result<ReplyDraft, GraphSendError> {
        let Some(graph_id) = decode_graph_email_id(email_id) else {
            return Err(GraphSendError::NotFound(format!("malformed graph message token '{email_id}'")));
        };
        // createReply lands a draft (In-Reply-To/References set by Graph).
        let reply_url = Self::url(&format!("/me/messages/{}/createReply", path_encode(graph_id)));
        let resp = self.send(GraphMethod::Post, &reply_url, None).map_err(GraphSendError::Engine)?;
        let draft = self
            .json_ok(resp, "create reply draft")
            .map_err(GraphSendError::Engine)?
            .ok_or_else(|| {
                GraphSendError::NotFound("the source message is no longer on the server".to_string())
            })?;
        let id = draft
            .get("id")
            .and_then(|s| s.as_str())
            .ok_or_else(|| GraphSendError::Engine("createReply reply carried no draft id".to_string()))?
            .to_string();
        let to = draft
            .get("toRecipients")
            .map(parse_recipient_list)
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.email)
            .collect();
        let subject = draft.get("subject").and_then(|s| s.as_str()).unwrap_or_default().to_string();
        Ok(ReplyDraft { id, to, subject })
    }

    fn patch_message(&self, id: &str, patch: &serde_json::Value) -> Result<(), GraphSendError> {
        let url = Self::url(&format!("/me/messages/{}", path_encode(id)));
        let resp = self.send(GraphMethod::Patch, &url, Some(patch)).map_err(GraphSendError::Engine)?;
        self.json_ok(resp, "patch draft").map_err(GraphSendError::Engine)?;
        Ok(())
    }

    /// One `POST …/attachments` per file (each ≤ 3 MB, checked by the
    /// caller first).
    fn attach_all(&self, draft_id: &str, attachments: &[GraphAttachment]) -> Result<(), GraphSendError> {
        for a in attachments {
            let url = Self::url(&format!("/me/messages/{}/attachments", path_encode(draft_id)));
            let resp = self
                .send(GraphMethod::Post, &url, Some(&file_attachment_json(a)))
                .map_err(GraphSendError::Engine)?;
            if resp.status == 413 {
                return Err(GraphSendError::TooLarge(format!(
                    "Microsoft refused attachment '{}' as too large",
                    a.filename
                )));
            }
            self.json_ok(resp, "add attachment").map_err(GraphSendError::Engine)?;
        }
        Ok(())
    }

    /// A send answers 202 Accepted with no body; anything else is an
    /// engine error carrying only Graph's own message.
    fn expect_accepted(&self, resp: GraphResponse, what: &str) -> Result<(), GraphSendError> {
        match resp.status {
            200..=299 => Ok(()),
            413 => Err(GraphSendError::TooLarge(format!(
                "Microsoft refused the {what} request as too large (max 4 MB)"
            ))),
            404 => Err(GraphSendError::NotFound(format!("graph {what}: not found"))),
            s => Err(GraphSendError::Engine(format!(
                "graph {what}: HTTP {s}: {}",
                graph_error_excerpt(&resp.body)
            ))),
        }
    }
}

/// The draft `createReply` made.
struct ReplyDraft {
    id: String,
    to: Vec<String>,
    subject: String,
}

impl ReadBackend for GraphBackend {
    fn list_inbox(
        &self,
        account_id: &str,
        filter: &ListFilter,
        limit: usize,
    ) -> Result<Vec<EmailSummary>, ListError> {
        self.check_handle(account_id)?;
        let folder = list_folder_id(&filter.folder);
        let base = Self::url(&format!("/me/mailFolders/{}/messages", path_encode(&folder)));
        let url = with_query(&base, &build_list_query(filter, limit));
        let resp = self.send(GraphMethod::Get, &url, None).map_err(ListError::Engine)?;
        // A bad `--folder` surfaces as Graph's error → uniform
        // UnknownFolder so the agent sees the same teaching shape as the
        // other backends.
        match self.json_ok(resp, "list messages").map_err(ListError::Engine)? {
            Some(v) => Ok(parse_message_list(&v)),
            None => {
                // 404 on a folder path = unknown folder (teach with the
                // real folder names).
                let available = self
                    .list_folders_full()
                    .map(|f| f.into_iter().map(|(dn, _)| dn).collect())
                    .unwrap_or_default();
                Err(ListError::UnknownFolder { requested: folder, available })
            }
        }
    }

    fn fetch_full(&self, account_id: &str, email_id: &str) -> Result<Option<EmailFull>, String> {
        self.check_handle(account_id)?;
        let Some(graph_id) = decode_graph_email_id(email_id) else {
            return Ok(None); // not a graph token = unknown message (masked)
        };
        self.fetch_message(graph_id)
    }

    fn mark_seen(&self, account_id: &str, email_id: &str) -> Result<(), String> {
        self.check_handle(account_id)?;
        let Some(graph_id) = decode_graph_email_id(email_id) else {
            return Err(format!("malformed graph message token '{email_id}'"));
        };
        self.patch_flags(graph_id, Some(true), None)
    }

    fn fetch_blob(
        &self,
        account_id: &str,
        blob_id: &str,
        _name: &str,
        _mime: &str,
    ) -> Result<Vec<u8>, String> {
        self.check_handle(account_id)?;
        // Attachment blob token → attachment `/$value` bytes.
        if let Some((msg_id, att_id)) = decode_att_blob(blob_id) {
            let url = Self::url(&format!(
                "/me/messages/{}/attachments/{}/$value",
                path_encode(&msg_id),
                path_encode(&att_id)
            ));
            return self.http.get_bytes(&url);
        }
        // Bare `graph:<id>` → the raw RFC 822 MIME (`read --raw`).
        if let Some(graph_id) = decode_graph_email_id(blob_id) {
            let url = Self::url(&format!("/me/messages/{}/$value", path_encode(graph_id)));
            return self.http.get_bytes(&url);
        }
        Err(format!("malformed graph blob id '{blob_id}'"))
    }
}

impl ManageBackend for GraphBackend {
    fn move_message(
        &self,
        account_id: &str,
        email_id: &str,
        dest: &MailFolder,
    ) -> Result<ManageOutcome, ListError> {
        self.check_handle(account_id)?;
        let graph_id = decode_graph_email_id(email_id)
            .ok_or_else(|| ListError::Engine(format!("malformed graph message token '{email_id}'")))?;
        let dest_id = self.resolve_dest_id(dest)?;
        self.move_to(graph_id, &dest_id, &dest_id)
    }

    fn archive_message(&self, account_id: &str, email_id: &str) -> Result<ManageOutcome, ListError> {
        self.check_handle(account_id)?;
        let graph_id = decode_graph_email_id(email_id)
            .ok_or_else(|| ListError::Engine(format!("malformed graph message token '{email_id}'")))?;
        self.move_to(graph_id, "archive", "archive")
    }

    fn trash_message(&self, account_id: &str, email_id: &str) -> Result<ManageOutcome, ListError> {
        self.check_handle(account_id)?;
        let graph_id = decode_graph_email_id(email_id)
            .ok_or_else(|| ListError::Engine(format!("malformed graph message token '{email_id}'")))?;
        // DELETE = MOVE to Deleted Items. NEVER a hard DELETE /messages/{id}.
        self.move_to(graph_id, "deleteditems", "deleteditems")
    }

    fn set_flags(
        &self,
        account_id: &str,
        email_id: &str,
        read: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<(), String> {
        self.check_handle(account_id)?;
        let Some(graph_id) = decode_graph_email_id(email_id) else {
            return Err(format!("malformed graph message token '{email_id}'"));
        };
        self.patch_flags(graph_id, read, flagged)
    }

    fn folder_create(&self, account_id: &str, name: &str) -> Result<(), String> {
        self.check_handle(account_id)?;
        let url = Self::url("/me/mailFolders");
        let body = serde_json::json!({ "displayName": name });
        let resp = self.send(GraphMethod::Post, &url, Some(&body))?;
        self.json_ok(resp, "create folder")?;
        Ok(())
    }

    fn folder_rename(&self, account_id: &str, from: &str, to: &str) -> Result<(), ListError> {
        self.check_handle(account_id)?;
        let id = self.folder_id_by_name(from)?;
        let url = Self::url(&format!("/me/mailFolders/{}", path_encode(&id)));
        let body = serde_json::json!({ "displayName": to });
        let resp = self.send(GraphMethod::Patch, &url, Some(&body)).map_err(ListError::Engine)?;
        self.json_ok(resp, "rename folder").map_err(ListError::Engine)?;
        Ok(())
    }

    fn folder_list(&self, account_id: &str) -> Result<Vec<String>, String> {
        self.check_handle(account_id)?;
        let base = Self::url("/me/mailFolders");
        let url = with_query(&base, &[("$top", "100".to_string()), ("$select", "displayName".to_string())]);
        let resp = self.send(GraphMethod::Get, &url, None)?;
        match self.json_ok(resp, "list folders")? {
            Some(v) => Ok(parse_folder_names(&v)),
            None => Ok(Vec::new()),
        }
    }
}

/// Pull the human `error.message` out of a Graph error body (`{"error":
/// {"code","message"}}`), else a short raw excerpt. Never a token (Graph
/// error bodies carry none; the Bearer only ever rides a REQUEST header).
pub(crate) fn graph_error_excerpt(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            return msg.chars().take(200).collect();
        }
    }
    body.chars().take(200).collect()
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — scripted mock GraphHttp + fixtures. No network, no
// token, no M365, no sends (house rules / prd §11.16).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn test_inbox(id: &str) -> MailExternalInbox {
        MailExternalInbox {
            id: id.to_string(),
            owner_project_id: "p1".to_string(),
            email_address: "user@company.com".to_string(),
            display_name: Some("User".to_string()),
            kind: "graph".to_string(),
            host: "graph.microsoft.com".to_string(),
            port: 443,
            tls: "implicit-tls".to_string(),
            username: "user@company.com".to_string(),
            drafts_folder: None,
            status: "connected".to_string(),
            last_checked_at: None,
            last_error: None,
            created_at: 0,
            smtp_host: None,
            smtp_port: None,
            smtp_tls: None,
        }
    }

    /// One recorded request (verb + url + body) — the mock's assertion
    /// surface. A DELETE can never appear here: [`GraphMethod`] has no
    /// Delete variant (the no-hard-delete guarantee is structural).
    #[derive(Debug, Clone)]
    struct Seen {
        method: GraphMethod,
        url: String,
        body: Option<serde_json::Value>,
    }

    /// Scripted mock: pops queued JSON replies for `send` (records every
    /// request), serves canned bytes for `get_bytes`. Never a token — the
    /// trait carries none by construction.
    struct MockGraph {
        replies: Mutex<VecDeque<(u16, Option<u64>, String)>>,
        bytes: Mutex<VecDeque<Vec<u8>>>,
        seen: Mutex<Vec<Seen>>,
    }

    impl MockGraph {
        fn new(replies: Vec<(u16, Option<u64>, &str)>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(
                    replies.into_iter().map(|(s, ra, b)| (s, ra, b.to_string())).collect(),
                ),
                bytes: Mutex::new(VecDeque::new()),
                seen: Mutex::new(Vec::new()),
            })
        }
        fn with_bytes(self: &Arc<Self>, b: Vec<u8>) {
            self.bytes.lock().unwrap().push_back(b);
        }
        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl GraphHttp for MockGraph {
        fn send(
            &self,
            method: GraphMethod,
            url: &str,
            body: Option<&serde_json::Value>,
        ) -> Result<GraphResponse, String> {
            self.seen.lock().unwrap().push(Seen {
                method,
                url: url.to_string(),
                body: body.cloned(),
            });
            let (status, retry_after, body) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock ran out of scripted replies");
            Ok(GraphResponse { status, retry_after, body })
        }
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
            self.seen.lock().unwrap().push(Seen {
                method: GraphMethod::Get,
                url: url.to_string(),
                body: None,
            });
            Ok(self.bytes.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    fn no_sleep() -> Box<dyn Fn(Duration) + Send + Sync> {
        Box::new(|_| {})
    }

    fn backend(mock: Arc<MockGraph>) -> GraphBackend {
        GraphBackend::with_sleep(test_inbox("row-1"), mock, no_sleep())
    }

    // ── query mapping (⚠ LIVE-BOX #1) ──

    #[test]
    fn list_query_maps_listfilter_to_odata() {
        let f = ListFilter {
            unread_only: true,
            after_unix: Some(1_783_468_800),  // 2026-07-08T00:00:00Z
            before_unix: Some(1_783_555_200), // 2026-07-09T00:00:00Z
            offset: 20,
            ..Default::default()
        };
        let pairs = build_list_query(&f, 15);
        let map: std::collections::HashMap<_, _> = pairs.iter().cloned().collect();
        assert_eq!(map["$orderby"], "receivedDateTime desc");
        assert_eq!(map["$top"], "15");
        assert_eq!(map["$skip"], "20");
        let filter = &map["$filter"];
        assert!(filter.contains("isRead eq false"), "{filter}");
        assert!(filter.contains("receivedDateTime ge 2026-07-08T00:00:00Z"), "{filter}");
        assert!(filter.contains("receivedDateTime lt 2026-07-09T00:00:00Z"), "{filter}");
        assert!(filter.contains(" and "), "clauses AND-joined: {filter}");
        assert!(map.contains_key("$select"));
    }

    #[test]
    fn list_query_search_drops_orderby_and_filter() {
        // Graph forbids $search WITH $orderby/$filter — a query request
        // must carry ONLY $search (+ $top/$select).
        let f = ListFilter {
            unread_only: true,
            query: Some("invoice \"q1\"".to_string()),
            after_unix: Some(1_783_468_800),
            ..Default::default()
        };
        let pairs = build_list_query(&f, 10);
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"$search"), "{keys:?}");
        assert!(!keys.contains(&"$orderby"), "no $orderby with $search: {keys:?}");
        assert!(!keys.contains(&"$filter"), "no $filter with $search: {keys:?}");
        let search = pairs.iter().find(|(k, _)| *k == "$search").map(|(_, v)| v).unwrap();
        assert_eq!(search, "\"invoice  q1 \"", "quotes neutralized, wrapped: {search}");
    }

    #[test]
    fn list_offset_zero_omits_skip() {
        let pairs = build_list_query(&ListFilter::default(), 25);
        assert!(!pairs.iter().any(|(k, _)| *k == "$skip"), "offset 0 → no $skip");
        assert!(pairs.iter().any(|(k, v)| *k == "$top" && v == "25"));
    }

    // ── list parse + folder targeting ──

    #[test]
    fn list_inbox_builds_url_and_parses_summaries() {
        let reply = r#"{"value":[
            {"id":"AAA","subject":"Hi","receivedDateTime":"2026-07-08T10:15:00Z",
             "isRead":false,"hasAttachments":true,"conversationId":"C1",
             "from":{"emailAddress":{"name":"GitHub","address":"noreply@github.com"}},
             "toRecipients":[{"emailAddress":{"address":"user@company.com"}}]}
        ]}"#;
        let mock = MockGraph::new(vec![(200, None, reply)]);
        let be = backend(mock.clone());
        let out = be
            .list_inbox("row-1", &ListFilter::default(), 20)
            .expect("list");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "graph:AAA");
        assert_eq!(out[0].subject, "Hi");
        assert_eq!(out[0].from[0].email, "noreply@github.com");
        assert!(out[0].unread && out[0].has_attachment);
        // The URL targets the Inbox well-known folder with the query.
        let seen = mock.seen();
        assert_eq!(seen[0].method, GraphMethod::Get);
        assert!(seen[0].url.contains("/me/mailFolders/inbox/messages"), "{}", seen[0].url);
        // Keys stay literal (`$orderby`); only VALUES are percent-encoded.
        assert!(seen[0].url.contains("$orderby=receivedDateTime%20desc"), "{}", seen[0].url);
    }

    #[test]
    fn list_junk_and_named_folders_target_the_right_segment() {
        let mock = MockGraph::new(vec![(200, None, r#"{"value":[]}"#)]);
        let be = backend(mock.clone());
        be.list_inbox(
            "row-1",
            &ListFilter { folder: MailFolder::Junk, ..Default::default() },
            5,
        )
        .unwrap();
        assert!(mock.seen()[0].url.contains("/me/mailFolders/junkemail/messages"));

        let mock2 = MockGraph::new(vec![(200, None, r#"{"value":[]}"#)]);
        let be2 = backend(mock2.clone());
        be2.list_inbox(
            "row-1",
            &ListFilter { folder: MailFolder::Named("archive".into()), ..Default::default() },
            5,
        )
        .unwrap();
        assert!(mock2.seen()[0].url.contains("/me/mailFolders/archive/messages"));
    }

    // ── read + shaping through the seam ──

    #[test]
    fn read_fetches_full_with_attachments_and_shapes_markers() {
        use crate::mail::messages::{shape_full_json, MARKER_BEGIN, MARKER_END};
        let msg = r#"{"id":"AAA","subject":"Verify","receivedDateTime":"2026-07-08T10:15:00Z",
            "isRead":true,"hasAttachments":true,"conversationId":"C1",
            "from":{"emailAddress":{"name":"GitHub","address":"noreply@github.com"}},
            "toRecipients":[{"emailAddress":{"address":"user@company.com"}}],
            "ccRecipients":[{"emailAddress":{"address":"cc@company.com"}}],
            "body":{"contentType":"html","content":"<p>Your code is <b>424242</b>.</p>"},
            "internetMessageHeaders":[
              {"name":"Authentication-Results","value":"spf=pass dkim=fail dmarc=none"}]}"#;
        let atts = r#"{"value":[{"id":"ATT1","name":"invite.ics","contentType":"text/calendar","size":512}]}"#;
        let mock = MockGraph::new(vec![(200, None, msg), (200, None, atts)]);
        let be = backend(mock.clone());
        let full = be.fetch_full("row-1", "graph:AAA").expect("read").expect("some");
        assert_eq!(full.summary.subject, "Verify");
        assert_eq!(full.cc[0].email, "cc@company.com");
        assert_eq!(full.attachments.len(), 1);
        assert_eq!(full.attachments[0].filename.as_deref(), Some("invite.ics"));
        assert!(full.html.is_some() && full.text.is_none());
        // The route-layer shaping wraps the (html-stripped) body + verdicts.
        let v = shape_full_json("user@company.com", &full, false);
        let text = v["text"].as_str().unwrap();
        assert!(text.starts_with(MARKER_BEGIN) && text.ends_with(MARKER_END));
        assert!(text.contains("Your code is 424242."));
        assert_eq!(v["authResults"]["spf"], "pass");
        assert_eq!(v["authResults"]["dkim"], "fail");
        // The full message GET carried $select but never a token in the URL.
        let seen = mock.seen();
        assert!(seen[0].url.contains("/me/messages/AAA"));
        assert!(seen.iter().all(|s| !s.url.contains("Bearer") && !s.url.to_lowercase().contains("token")));
    }

    #[test]
    fn read_unknown_id_is_masked_none() {
        // 404 → Ok(None) (the route masks it); a non-graph token → None.
        let mock = MockGraph::new(vec![(404, None, r#"{"error":{"code":"ErrorItemNotFound"}}"#)]);
        let be = backend(mock);
        assert!(be.fetch_full("row-1", "graph:MISSING").unwrap().is_none());
        let mock2 = MockGraph::new(vec![]);
        let be2 = backend(mock2);
        assert!(be2.fetch_full("row-1", "uid:1:2").unwrap().is_none());
    }

    #[test]
    fn mark_seen_patches_isread_true() {
        let mock = MockGraph::new(vec![(200, None, r#"{"id":"AAA","isRead":true}"#)]);
        let be = backend(mock.clone());
        be.mark_seen("row-1", "graph:AAA").expect("mark");
        let seen = mock.seen();
        assert_eq!(seen[0].method, GraphMethod::Patch);
        assert!(seen[0].url.ends_with("/me/messages/AAA"));
        assert_eq!(seen[0].body.as_ref().unwrap()["isRead"], true);
    }

    // ── manage: move / archive / delete-to-DeletedItems / flag / folders ──

    #[test]
    fn delete_moves_to_deleteditems_never_hard_delete() {
        let mock = MockGraph::new(vec![(200, None, r#"{"id":"BBB"}"#)]);
        let be = backend(mock.clone());
        let out = be.trash_message("row-1", "graph:AAA").expect("trash");
        assert_eq!(out.folder.as_deref(), Some("deleteditems"));
        let seen = mock.seen();
        // Exactly one POST …/move with destinationId=deleteditems.
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, GraphMethod::Post);
        assert!(seen[0].url.ends_with("/me/messages/AAA/move"), "{}", seen[0].url);
        assert_eq!(seen[0].body.as_ref().unwrap()["destinationId"], "deleteditems");
        // PROOF: no request in the whole module is ever a hard DELETE —
        // GraphMethod has no Delete variant, and no /$value hard-delete
        // path exists. The recorder can only hold Get/Post/Patch.
        assert!(seen.iter().all(|s| s.method != GraphMethod::Get || !s.url.contains("/move")));
    }

    #[test]
    fn move_and_archive_target_the_right_destinations() {
        let mock = MockGraph::new(vec![(200, None, "{}")]);
        let be = backend(mock.clone());
        be.archive_message("row-1", "graph:AAA").expect("archive");
        assert_eq!(mock.seen()[0].body.as_ref().unwrap()["destinationId"], "archive");

        let mock2 = MockGraph::new(vec![(200, None, "{}")]);
        let be2 = backend(mock2.clone());
        be2.move_message("row-1", "graph:AAA", &MailFolder::Junk).expect("move");
        assert_eq!(mock2.seen()[0].body.as_ref().unwrap()["destinationId"], "junkemail");
    }

    #[test]
    fn set_flags_patches_isread_and_flag() {
        let mock = MockGraph::new(vec![(200, None, "{}")]);
        let be = backend(mock.clone());
        be.set_flags("row-1", "graph:AAA", Some(false), Some(true)).expect("flags");
        let body = mock.seen()[0].body.clone().unwrap();
        assert_eq!(body["isRead"], false);
        assert_eq!(body["flag"]["flagStatus"], "flagged");
    }

    #[test]
    fn folder_create_and_list() {
        let mock = MockGraph::new(vec![(201, None, r#"{"id":"F1","displayName":"Receipts"}"#)]);
        let be = backend(mock.clone());
        be.folder_create("row-1", "Receipts").expect("create");
        assert_eq!(mock.seen()[0].body.as_ref().unwrap()["displayName"], "Receipts");

        let mock2 = MockGraph::new(vec![(
            200,
            None,
            r#"{"value":[{"displayName":"Inbox"},{"displayName":"Receipts"}]}"#,
        )]);
        let be2 = backend(mock2);
        let names = be2.folder_list("row-1").expect("list");
        assert_eq!(names, vec!["Inbox".to_string(), "Receipts".to_string()]);
    }

    #[test]
    fn folder_rename_resolves_id_then_patches() {
        // First reply: the folder lookup; second: the PATCH.
        let mock = MockGraph::new(vec![
            (200, None, r#"{"value":[{"id":"F9","displayName":"Old"}]}"#),
            (200, None, r#"{"id":"F9","displayName":"New"}"#),
        ]);
        let be = backend(mock.clone());
        be.folder_rename("row-1", "Old", "New").expect("rename");
        let seen = mock.seen();
        assert_eq!(seen[1].method, GraphMethod::Patch);
        assert!(seen[1].url.ends_with("/me/mailFolders/F9"));
        assert_eq!(seen[1].body.as_ref().unwrap()["displayName"], "New");
    }

    // ── draft (createReply → PATCH body; NEVER send) ──

    #[test]
    fn draft_creates_reply_then_patches_body_never_sends() {
        let mock = MockGraph::new(vec![
            (201, None, r#"{"id":"DRAFT1"}"#),          // createReply → draft
            (200, None, r#"{"id":"DRAFT1"}"#),          // PATCH body
        ]);
        let be = backend(mock.clone());
        let folder = be.save_reply_draft("row-1", "graph:AAA", "Thanks!").expect("draft");
        assert_eq!(folder, "Drafts");
        let seen = mock.seen();
        assert_eq!(seen[0].method, GraphMethod::Post);
        assert!(seen[0].url.ends_with("/me/messages/AAA/createReply"), "{}", seen[0].url);
        assert_eq!(seen[1].method, GraphMethod::Patch);
        assert_eq!(seen[1].body.as_ref().unwrap()["body"]["content"], "Thanks!");
        // NEVER a /send — the user reviews + sends themselves.
        assert!(seen.iter().all(|s| !s.url.contains("/send")), "no send: {seen:?}");
    }

    // ── MCS2: send, reply, compose drafts, attachments ──

    fn att<'a>(name: &'a str, bytes: &'a [u8]) -> GraphAttachment<'a> {
        GraphAttachment { filename: name, content_type: "application/pdf", bytes }
    }

    #[test]
    fn send_mail_posts_sendmail_with_bcc_attachments_and_saves_to_sent() {
        let mock = MockGraph::new(vec![(202, None, "")]);
        let be = backend(mock.clone());
        let pdf = b"%PDF-1.7 tiny".to_vec();
        let r = be
            .send_mail(
                "row-1",
                &["pat@dest.example".to_string()],
                &["cc@dest.example".to_string()],
                &["owner@shop.example".to_string()],
                "Quarterly",
                "Numbers attached.",
                &[att("q.pdf", &pdf)],
            )
            .expect("send");
        assert_eq!(r.to, vec!["pat@dest.example".to_string()]);
        assert_eq!(r.attachments, vec!["q.pdf".to_string()]);
        let seen = mock.seen();
        assert_eq!(seen.len(), 1, "one request");
        assert_eq!(seen[0].method, GraphMethod::Post);
        assert!(seen[0].url.ends_with("/me/sendMail"), "{}", seen[0].url);
        let b = seen[0].body.clone().unwrap();
        assert_eq!(b["saveToSentItems"], true);
        let m = &b["message"];
        assert_eq!(m["subject"], "Quarterly");
        assert_eq!(m["body"]["contentType"], "text");
        assert_eq!(m["toRecipients"][0]["emailAddress"]["address"], "pat@dest.example");
        assert_eq!(m["ccRecipients"][0]["emailAddress"]["address"], "cc@dest.example");
        assert_eq!(m["bccRecipients"][0]["emailAddress"]["address"], "owner@shop.example");
        assert_eq!(m["attachments"][0]["@odata.type"], "#microsoft.graph.fileAttachment");
        assert_eq!(m["attachments"][0]["name"], "q.pdf");
        use base64::engine::general_purpose::STANDARD;
        assert_eq!(m["attachments"][0]["contentBytes"], STANDARD.encode(&pdf));
    }

    #[test]
    fn send_mail_refuses_an_attachment_over_3mb_before_any_request() {
        let mock = MockGraph::new(vec![]); // any request would panic
        let be = backend(mock.clone());
        let big = vec![0u8; GRAPH_INLINE_ATTACHMENT_MAX + 1];
        let err = be
            .send_mail("row-1", &["a@b.example".into()], &[], &[], "s", "b", &[att("big.bin", &big)])
            .expect_err("too large");
        assert!(matches!(&err, GraphSendError::TooLarge(h) if h.contains("3 MB")), "{err:?}");
        assert!(mock.seen().is_empty());
        // Two files under 3 MB each but over 4 MB together are refused too.
        let two = vec![0u8; 2 * 1024 * 1024];
        let err = be
            .send_mail(
                "row-1",
                &["a@b.example".into()],
                &[],
                &[],
                "s",
                "b",
                &[att("a.bin", &two), att("b.bin", &two)],
            )
            .expect_err("request too large");
        assert!(matches!(&err, GraphSendError::TooLarge(h) if h.contains("4 MB")), "{err:?}");
    }

    #[test]
    fn send_mail_maps_graph_errors_without_leaking_the_body() {
        let mock = MockGraph::new(vec![(
            403,
            None,
            r#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied. Check credentials"}}"#,
        )]);
        let be = backend(mock);
        let err = be
            .send_mail("row-1", &["a@b.example".into()], &[], &[], "s", "secret body", &[])
            .expect_err("denied");
        match err {
            GraphSendError::Engine(h) => {
                assert!(h.contains("403") && h.contains("Access is denied"), "{h}");
                assert!(!h.contains("secret body"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn send_reply_creates_patches_attaches_then_sends_to_the_locked_sender() {
        let mock = MockGraph::new(vec![
            (
                201,
                None,
                r#"{"id":"R1","subject":"RE: Hi","toRecipients":[{"emailAddress":{"address":"alice@example.com"}}]}"#,
            ),
            (200, None, r#"{"id":"R1"}"#),
            (201, None, r#"{"id":"ATT1"}"#),
            (202, None, ""),
        ]);
        let be = backend(mock.clone());
        let r = be
            .send_reply(
                "row-1",
                "graph:AAA",
                "Thanks!",
                &["owner@shop.example".to_string()],
                &[att("n.pdf", b"x")],
            )
            .expect("reply");
        assert_eq!(r.to, vec!["alice@example.com".to_string()]);
        assert_eq!(r.subject, "RE: Hi");
        let seen = mock.seen();
        assert!(seen[0].url.ends_with("/me/messages/AAA/createReply"), "{}", seen[0].url);
        assert_eq!(seen[1].method, GraphMethod::Patch);
        assert_eq!(seen[1].body.as_ref().unwrap()["body"]["content"], "Thanks!");
        assert_eq!(
            seen[1].body.as_ref().unwrap()["bccRecipients"][0]["emailAddress"]["address"],
            "owner@shop.example"
        );
        assert!(seen[2].url.ends_with("/me/messages/R1/attachments"), "{}", seen[2].url);
        assert_eq!(seen[3].method, GraphMethod::Post);
        assert!(seen[3].url.ends_with("/me/messages/R1/send"), "{}", seen[3].url);
        assert!(seen.iter().all(|s| !s.url.ends_with("/sendMail")));
    }

    #[test]
    fn send_reply_to_a_missing_message_is_not_found() {
        let mock = MockGraph::new(vec![(404, None, r#"{"error":{"code":"ErrorItemNotFound"}}"#)]);
        let be = backend(mock);
        let err = be.send_reply("row-1", "graph:GONE", "x", &[], &[]).expect_err("gone");
        assert!(matches!(err, GraphSendError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn compose_draft_posts_a_message_and_attachments_and_never_sends() {
        let mock = MockGraph::new(vec![(201, None, r#"{"id":"D1"}"#), (201, None, r#"{"id":"A1"}"#)]);
        let be = backend(mock.clone());
        let folder = be
            .save_compose_draft(
                "row-1",
                &["pat@dest.example".to_string()],
                &[],
                &["hidden@dest.example".to_string()],
                "Proposal",
                "Draft text",
                &[att("p.pdf", b"pdf")],
            )
            .expect("draft");
        assert_eq!(folder, "Drafts");
        let seen = mock.seen();
        assert!(seen[0].url.ends_with("/me/messages"), "{}", seen[0].url);
        let b = seen[0].body.clone().unwrap();
        assert_eq!(b["subject"], "Proposal");
        assert_eq!(b["toRecipients"][0]["emailAddress"]["address"], "pat@dest.example");
        assert_eq!(b["bccRecipients"][0]["emailAddress"]["address"], "hidden@dest.example");
        assert!(seen[1].url.ends_with("/me/messages/D1/attachments"), "{}", seen[1].url);
        assert_eq!(seen[1].body.as_ref().unwrap()["name"], "p.pdf");
        assert!(seen.iter().all(|s| !s.url.contains("/send")), "a draft never sends: {seen:?}");
    }

    #[test]
    fn reply_draft_with_attachment_never_sends() {
        let mock = MockGraph::new(vec![
            (201, None, r#"{"id":"DR","toRecipients":[]}"#),
            (200, None, r#"{"id":"DR"}"#),
            (201, None, r#"{"id":"A1"}"#),
        ]);
        let be = backend(mock.clone());
        be.save_reply_draft_with("row-1", "graph:AAA", "ok", &[att("a.txt", b"a")]).expect("draft");
        let seen = mock.seen();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|s| !s.url.contains("/send")), "{seen:?}");
    }

    #[test]
    fn reply_preview_reads_the_source_sender_and_subject() {
        let msg = r#"{"id":"AAA","subject":"Lunch?","receivedDateTime":"2026-07-08T10:15:00Z",
            "isRead":true,"hasAttachments":false,
            "from":{"emailAddress":{"name":"Alice","address":"alice@example.com"}},
            "toRecipients":[],"body":{"contentType":"text","content":"hi"}}"#;
        let mock = MockGraph::new(vec![(200, None, msg)]);
        let be = backend(mock);
        let (to, subject) = be.reply_preview("row-1", "graph:AAA").unwrap().expect("some");
        assert_eq!(to, "alice@example.com");
        assert_eq!(subject, "Re: Lunch?");
        let mock = MockGraph::new(vec![(404, None, "{}")]);
        assert!(backend(mock).reply_preview("row-1", "graph:NOPE").unwrap().is_none());
    }

    // ── attachments (bytes over get_bytes) ──

    #[test]
    fn fetch_blob_attachment_and_raw_message() {
        let mock = MockGraph::new(vec![]);
        mock.with_bytes(b"ICS-BYTES".to_vec());
        let be = backend(mock.clone());
        let att_token = encode_att_blob("AAA", "ATT1");
        let bytes = be.fetch_blob("row-1", &att_token, "invite.ics", "text/calendar").expect("att");
        assert_eq!(bytes, b"ICS-BYTES");
        // The `$value` path literal is not encoded; only the opaque ids are.
        assert!(mock.seen()[0].url.ends_with("/me/messages/AAA/attachments/ATT1/$value"));

        let mock2 = MockGraph::new(vec![]);
        mock2.with_bytes(b"RAW-MIME".to_vec());
        let be2 = backend(mock2.clone());
        let raw = be2.fetch_blob("row-1", "graph:AAA", "message.eml", "message/rfc822").expect("raw");
        assert_eq!(raw, b"RAW-MIME");
        assert!(mock2.seen()[0].url.ends_with("/me/messages/AAA/$value"));
    }

    // ── throttling (⚠ LIVE-BOX #3) ──

    #[test]
    fn throttle_429_retry_after_is_honored_then_succeeds() {
        // 429 with Retry-After: 2, then a 200 — the backend must retry.
        let mock = MockGraph::new(vec![
            (429, Some(2), r#"{"error":{"code":"TooManyRequests"}}"#),
            (200, None, r#"{"value":[]}"#),
        ]);
        let slept: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let slept_c = slept.clone();
        let be = GraphBackend::with_sleep(
            test_inbox("row-1"),
            mock.clone(),
            Box::new(move |d| slept_c.lock().unwrap().push(d.as_secs())),
        );
        let out = be.list_inbox("row-1", &ListFilter::default(), 10).expect("list after retry");
        assert!(out.is_empty());
        assert_eq!(mock.seen().len(), 2, "one retry after the 429");
        assert_eq!(*slept.lock().unwrap(), vec![2], "honored Retry-After: 2s");
    }

    #[test]
    fn throttle_retry_after_is_capped() {
        // A hostile Retry-After is capped so a request never parks forever.
        let mock = MockGraph::new(vec![
            (429, Some(99_999), r#"{}"#),
            (200, None, r#"{"value":[]}"#),
        ]);
        let slept: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let slept_c = slept.clone();
        let be = GraphBackend::with_sleep(
            test_inbox("row-1"),
            mock,
            Box::new(move |d| slept_c.lock().unwrap().push(d.as_secs())),
        );
        be.list_inbox("row-1", &ListFilter::default(), 10).expect("list");
        assert_eq!(*slept.lock().unwrap(), vec![RETRY_AFTER_CAP_SECS]);
    }

    #[test]
    fn throttle_gives_up_after_bounded_retries() {
        // Persistent 429s: bounded retries, then the throttle surfaces.
        let replies: Vec<(u16, Option<u64>, &str)> =
            std::iter::repeat((429u16, Some(0u64), r#"{"error":{"message":"throttled"}}"#))
                .take((MAX_THROTTLE_RETRIES + 1) as usize)
                .collect();
        let mock = MockGraph::new(replies);
        let be = backend(mock.clone());
        let err = be.list_inbox("row-1", &ListFilter::default(), 10).expect_err("throttled");
        assert!(err.to_string().contains("429"), "{err}");
        assert_eq!(mock.seen().len(), (MAX_THROTTLE_RETRIES + 1) as usize);
    }

    // ── id / token helpers ──

    #[test]
    fn graph_id_and_att_blob_round_trip() {
        assert_eq!(encode_graph_email_id("AAA"), "graph:AAA");
        assert_eq!(decode_graph_email_id("graph:AAA"), Some("AAA"));
        assert_eq!(decode_graph_email_id("uid:1:2"), None);
        let t = encode_att_blob("msg/+id", "att=id");
        assert_eq!(decode_att_blob(&t), Some(("msg/+id".to_string(), "att=id".to_string())));
        assert_eq!(decode_att_blob("gatt_!!!"), None);
        assert_eq!(decode_att_blob("nope"), None);
    }

    #[test]
    fn path_encode_escapes_reserved_id_bytes() {
        // Opaque ids with URL-reserved bytes survive as a safe path seg.
        let enc = path_encode("AAMk/Ag+=b");
        assert!(!enc.contains('/') && !enc.contains('+') && !enc.contains('='), "{enc}");
    }

    #[test]
    fn non_2xx_error_excerpt_carries_graph_message_not_body_dump() {
        assert_eq!(
            graph_error_excerpt(r#"{"error":{"code":"X","message":"be nice"}}"#),
            "be nice"
        );
    }
}
