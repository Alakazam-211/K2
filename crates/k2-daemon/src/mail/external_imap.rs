//! S9 production [`ImapOps`] — the ONLY module that speaks IMAP wire
//! protocol (PRD §17.5). Everything above talks to the
//! [`crate::mail::external::ImapOps`] trait; everything below is the
//! `imap` 3.x crate over rustls (TLS verification ALWAYS on — there is
//! no code path that relaxes it; the schema's `tls` CHECK admits only
//! `implicit-tls`/`starttls`, and the test-only plaintext constructor
//! is `#[cfg(test)]` for the loopback mock).
//!
//! ⚠ LIVE-BOX functions (genuinely-uncertain live-server behavior —
//! verify against real providers on a live box before trusting edge
//! cases):
//!   1. [`survey_folders`] — whether SPECIAL-USE attributes ride a
//!      plain `LIST` varies (Gmail: yes; some Dovecot configs: only
//!      with `RETURN (SPECIAL-USE)`), and non-ASCII folder names
//!      arrive modified-UTF-7-encoded (we match them verbatim, no
//!      decode in V1 — the classic Gmail "[Gmail]/Drafts" naming is
//!      covered by the fallback list either way).
//!   2. [`build_search_query`] — non-ASCII SEARCH strings ride as
//!      UTF-8 inside a quoted string (RFC 6855 servers accept it;
//!      strict RFC 3501 servers may error — the route surfaces that
//!      error verbatim rather than silently dropping the filter).
//!   3. S11 management ops ([`manage_on_session`]) — move/archive/delete
//!      use RFC 6851 `UID MOVE`; a server WITHOUT the MOVE extension
//!      errors loudly (NO COPY+EXPUNGE fallback, so K2 NEVER expunges —
//!      DELETE is always a move to Trash). Archive/Trash SPECIAL-USE
//!      detection shares the UTF-7 / SPECIAL-USE-variance caveats of #1.
//!   4. O2 Gmail [`xoauth2_authenticate`] — SASL `XOAUTH2` login for
//!      `auth_kind='oauth'` rows. Google does NOT report a rejected
//!      token as a normal `NO`: it sends a base64-JSON error blob on a
//!      `+` continuation, expects an empty response, THEN sends the
//!      tagged `NO`. This one function isolates the AUTHENTICATE
//!      conversation AND that error classification, mapping a
//!      token-rejection to a clear "re-link needed" so a stale token
//!      never reads as "connected". The token / SASL string is NEVER
//!      logged or placed in any error.
//!
//! Session hygiene: one short-lived connection per operation (connect
//! → LOGIN → work → LOGOUT). External inboxes are polled at human
//! cadence (a `wait` loop is 2 s against the LOCAL server but external
//! reads are request-scoped), so connection reuse is a V2 concern.
//! Error strings surface transport/server text only — never
//! credentials, never message bodies (pre-mortem #16).

use crate::mail::external::{
    self, ConnectCheck, ImapOps, ManageOp, RawEmail, TLS_IMPLICIT, TLS_STARTTLS,
};
use crate::mail::jmap::{EmailSummary, MailAddr};
use crate::mail::messages::{ListError, ListFilter, MailFolder, ManageOutcome};
use crate::mail::oauth::{self, OauthProvider, ReqwestHttp};
use crate::mail::secrets::FileSecretStore;
use imap::types::{Flag, Name};
use imap::{ClientBuilder, ConnectionMode, TlsKind};
use imap_proto::types::NameAttribute;
use k2_core::db::schema::MailExternalInbox;
use mail_parser::MessageParser;

type ImapSession = imap::Session<imap::Connection>;

pub struct RealImapOps;

fn imap_err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

/// Connect per the row's TLS kind. rustls (`TlsKind::Rust`) with full
/// certificate verification — `danger_skip_tls_verify` is never
/// called, and there is no plaintext arm (the 0074 CHECK makes any
/// other value unreachable; fail loud if the impossible happens).
fn connect(inbox: &MailExternalInbox) -> Result<imap::Client<imap::Connection>, String> {
    let mode = match inbox.tls.as_str() {
        TLS_IMPLICIT => ConnectionMode::Tls,
        TLS_STARTTLS => ConnectionMode::StartTls,
        other => return Err(format!("unsupported tls kind '{other}' (schema drift?)")),
    };
    ClientBuilder::new(inbox.host.as_str(), inbox.port as u16)
        .mode(mode)
        .tls_kind(TlsKind::Rust)
        .connect()
        .map_err(|e| imap_err("connect", e))
}

/// Test-only plaintext connect for the scripted loopback mock — the
/// mock can't terminate TLS, and the point of the test is the IMAP
/// conversation, not the handshake. Never compiled into production.
#[cfg(test)]
fn connect_plaintext_for_test(
    host: &str,
    port: u16,
) -> Result<imap::Client<imap::Connection>, String> {
    ClientBuilder::new(host, port)
        .mode(ConnectionMode::Plaintext)
        .connect()
        .map_err(|e| imap_err("connect", e))
}

/// The SINGLE auth point for every read/draft/manage op. Two branches
/// (O2): an `auth_kind='oauth'` row authenticates with SASL XOAUTH2
/// against a freshly-minted access token; every other row keeps the
/// EXACT app-password `LOGIN` path, byte-for-byte unchanged. The oauth
/// columns live off the [`MailExternalInbox`] struct, so one raw-SQL
/// read decides the branch.
fn login(inbox: &MailExternalInbox, password: &str) -> Result<ImapSession, String> {
    let auth = external::read_oauth_fields(&inbox.id)?;
    if auth.auth_kind == external::AUTH_OAUTH {
        return oauth_login(inbox, &auth);
    }
    let client = connect(inbox)?;
    // The error path carries the SERVER's text (never the password —
    // imap's ValidateError exposes only an offending char).
    client
        .login(&inbox.username, password)
        .map_err(|(e, _client)| imap_err("login", e))
}

/// The OAuth (Gmail XOAUTH2) login branch. Mint/refresh a fresh access
/// token via the O1 engine, persist the (possibly refreshed) expiry back
/// to the row, then authenticate with SASL XOAUTH2. Only Gmail rides the
/// IMAP transport — Microsoft is Graph-only (O3), so a `microsoft`/other
/// provider here is a loud config error, never a silent password
/// fallthrough. The token is fetched fresh EVERY login (the O1 engine
/// reuses it when still valid) and NEVER logged.
fn oauth_login(inbox: &MailExternalInbox, auth: &external::OauthFields) -> Result<ImapSession, String> {
    let provider_str = auth.provider.as_deref().unwrap_or("");
    let provider = OauthProvider::from_str(provider_str).ok_or_else(|| {
        format!(
            "inbox {} is oauth but its provider '{provider_str}' is not recognized \
             — re-link this inbox",
            inbox.id
        )
    })?;
    if provider != OauthProvider::Gmail {
        // Microsoft (and any future non-IMAP vendor) does NOT use this
        // transport — Graph is a peer backend (O3). Fail loud rather
        // than attempt an IMAP login the vendor won't honor.
        return Err(format!(
            "provider '{provider_str}' does not use IMAP — Microsoft mail is served over \
             Graph, not this XOAUTH2 path"
        ));
    }

    // O1 engine: fresh access token (refreshing within the 60s skew),
    // via the production vault + HTTP client. `now` is injected into the
    // engine; here it is the wall clock.
    let secrets = FileSecretStore::default();
    let http = ReqwestHttp::default();
    let now = chrono::Utc::now().timestamp();
    let fresh = oauth::access_token_for(
        &secrets,
        &inbox.id,
        provider,
        auth.token_expires_at,
        now,
        None,
        None,
        &http,
    )
    .map_err(|e| format!("oauth token for inbox {}: {e}", inbox.id))?;

    // Persist the new expiry so the NEXT login reuses the token instead
    // of refreshing again (the refresh already re-vaulted the bundle).
    if fresh.refreshed {
        external::persist_token_expiry(&inbox.id, fresh.token_expires_at);
    }

    let client = connect(inbox)?;
    xoauth2_authenticate(client, &inbox.username, &fresh.access_token)
}

// ── ⚠ LIVE-BOX #4: SASL XOAUTH2 auth + Google continuation-error class ──

/// The exact SASL XOAUTH2 initial-response STRING (pre-base64):
/// `user=<user>\x01auth=Bearer <token>\x01\x01` (`\x01` = ^A, `0x01`).
/// The `imap` crate base64-encodes the [`imap::Authenticator::process`]
/// return, so this raw string is what the encoder receives — matching
/// `base64("user=" + user + ^A + "auth=Bearer " + token + ^A + ^A)`
/// (prd §5a). NEVER logged.
fn sasl_xoauth2_init(username: &str, access_token: &str) -> String {
    format!("user={username}\x01auth=Bearer {access_token}\x01\x01")
}

/// A stateful SASL XOAUTH2 authenticator. The `imap` crate calls
/// [`process`](imap::Authenticator::process) once per server `+`
/// continuation with the base64-DECODED challenge and base64-encodes the
/// return. XOAUTH2 has exactly one client message — the init string — so
/// the FIRST call yields it. Google's twist (⚠ LIVE-BOX): on a REJECTED
/// token it does not answer `NO`; it sends the error as a base64-JSON
/// blob on a SECOND `+` continuation and expects an empty response
/// before the tagged `NO`. Any call after the first captures that blob
/// (no token in it) for classification and answers empty.
struct XOAuth2Authenticator {
    init: String,
    sent_init: std::cell::Cell<bool>,
    error_challenge: std::cell::RefCell<Option<Vec<u8>>>,
}

impl XOAuth2Authenticator {
    fn new(username: &str, access_token: &str) -> Self {
        Self {
            init: sasl_xoauth2_init(username, access_token),
            sent_init: std::cell::Cell::new(false),
            error_challenge: std::cell::RefCell::new(None),
        }
    }
}

impl imap::Authenticator for XOAuth2Authenticator {
    type Response = Vec<u8>;
    fn process(&self, challenge: &[u8]) -> Vec<u8> {
        if !self.sent_init.get() {
            self.sent_init.set(true);
            self.init.clone().into_bytes()
        } else {
            // A continuation AFTER the init = Google's XOAUTH2 error blob
            // (base64-JSON, e.g. {"status":"400",...} — carries no
            // token). Capture it, answer empty; the server then sends the
            // tagged NO which the crate surfaces as an error.
            *self.error_challenge.borrow_mut() = Some(challenge.to_vec());
            Vec::new()
        }
    }
}

/// Authenticate an already-connected client with SASL XOAUTH2 and
/// classify Google's continuation-error surface. On a token rejection
/// (the error blob captured above) return a clear RE-LINK message so a
/// stale/revoked token surfaces as an error, never as "connected". The
/// access token and SASL string never appear in the returned error.
fn xoauth2_authenticate(
    client: imap::Client<imap::Connection>,
    username: &str,
    access_token: &str,
) -> Result<ImapSession, String> {
    let auth = XOAuth2Authenticator::new(username, access_token);
    match client.authenticate("XOAUTH2", &auth) {
        Ok(session) => Ok(session),
        Err((e, _client)) => {
            if let Some(blob) = auth.error_challenge.borrow().as_ref() {
                Err(xoauth2_rejection_message(blob))
            } else {
                // No error blob → a transport/protocol failure, not a
                // token rejection. Surface the server/transport text
                // (never a token — the imap error carries neither the
                // SASL string nor the token).
                Err(imap_err("XOAUTH2", e))
            }
        }
    }
}

/// Turn Google's base64-JSON XOAUTH2 error blob (already base64-DECODED
/// by the imap crate) into a re-link instruction. The blob carries a
/// `status` (e.g. `"400"`) but NO token; we surface only the status for
/// diagnostics and a fixed re-link hint.
fn xoauth2_rejection_message(blob: &[u8]) -> String {
    let status = std::str::from_utf8(blob)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("status")
                .and_then(|s| s.as_str().map(str::to_string).or_else(|| s.as_i64().map(|n| n.to_string())))
        });
    match status {
        Some(code) => format!(
            "Gmail rejected the OAuth access token (XOAUTH2 status {code}) — the inbox must be \
             re-linked (the token likely expired or was revoked)"
        ),
        None => "Gmail rejected the OAuth access token — the inbox must be re-linked (the token \
             likely expired or was revoked)"
            .to_string(),
    }
}

// ── ⚠ LIVE-BOX #1: folder survey (SPECIAL-USE via plain LIST) ───────────

/// `LIST "" *` → `(name, has \Drafts special-use)` pairs, feeding the
/// pure [`external::pick_drafts_folder`]. See the module-header
/// LIVE-BOX note on SPECIAL-USE and UTF-7 variance.
fn survey_folders(session: &mut ImapSession) -> Result<Vec<(String, bool)>, String> {
    let names = session
        .list(Some(""), Some("*"))
        .map_err(|e| imap_err("LIST", e))?;
    Ok(names
        .iter()
        .map(|n: &Name| {
            let drafts = n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::Drafts));
            (n.name().to_string(), drafts)
        })
        .collect())
}

/// `LIST "" *` → `(name, has \Junk special-use)` pairs, feeding the
/// pure [`external::pick_junk_folder`] and the `--folder` name match.
/// Same LIVE-BOX caveats as [`survey_folders`] (SPECIAL-USE + UTF-7).
fn survey_read_folders(session: &mut ImapSession) -> Result<Vec<(String, bool)>, String> {
    let names = session
        .list(Some(""), Some("*"))
        .map_err(|e| imap_err("LIST", e))?;
    Ok(names
        .iter()
        .map(|n: &Name| {
            let junk = n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::Junk));
            (n.name().to_string(), junk)
        })
        .collect())
}

/// Resolve `filter.folder` to the mailbox name to SELECT. Inbox is the
/// literal `INBOX` (no LIST round trip). Junk/Named do ONE LIST and
/// match on `\Junk` special-use / name (case-insensitive); a miss is
/// [`ListError::UnknownFolder`] listing the real folders so the agent
/// can retry — never a silent fall-through to INBOX.
fn resolve_read_folder(
    session: &mut ImapSession,
    folder: &MailFolder,
) -> Result<String, ListError> {
    if let MailFolder::Inbox = folder {
        return Ok("INBOX".to_string());
    }
    let folders = survey_read_folders(session)?;
    let names = || folders.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    match folder {
        MailFolder::Inbox => unreachable!(),
        MailFolder::Junk => external::pick_junk_folder(&folders).ok_or_else(|| {
            ListError::UnknownFolder { requested: "junk".to_string(), available: names() }
        }),
        MailFolder::Named(want) => folders
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(want))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| ListError::UnknownFolder {
                requested: want.clone(),
                available: names(),
            }),
    }
}

/// Resolve the drafts folder: the configured override must EXIST
/// (typo protection at add time); otherwise autodetect.
fn resolve_drafts_folder(
    session: &mut ImapSession,
    inbox: &MailExternalInbox,
) -> Result<Option<String>, String> {
    let folders = survey_folders(session)?;
    if let Some(want) = inbox.drafts_folder.as_deref() {
        return folders
            .iter()
            .find(|(name, _)| name == want)
            .map(|(name, _)| Some(name.clone()))
            .ok_or_else(|| {
                format!(
                    "the configured drafts folder '{want}' does not exist on the server \
                     (folders seen: {})",
                    folders
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
    }
    Ok(external::pick_drafts_folder(&folders))
}

// ── ⚠ LIVE-BOX #2: SEARCH query building ────────────────────────────────

/// Escape into an IMAP quoted string: CR/LF stripped (command
/// injection), `\` and `"` escaped. Non-ASCII rides as UTF-8 — see the
/// module-header LIVE-BOX note.
fn imap_quote(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    format!("\"{}\"", cleaned.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Unix seconds → the RFC 3501 date form (`8-Jul-2026`, English month
/// names — chrono's %b is locale-independent).
fn imap_date(unix: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
        .unwrap_or_default()
        .format("%-d-%b-%Y")
        .to_string()
}

/// [`ListFilter`] → an RFC 3501 `UID SEARCH` query. `since` is
/// day-granular on the wire (SEARCH SINCE ignores the time) — the
/// route layer's `summary_matches` re-check keeps second precision,
/// same as it does over the JMAP backend.
pub fn build_search_query(filter: &ListFilter) -> String {
    let mut parts: Vec<String> = Vec::new();
    if filter.unread_only {
        parts.push("UNSEEN".to_string());
    }
    // SINCE is INTERNALDATE >= that DAY (RFC 3501 is day-granular — the
    // route's `summary_matches` keeps second precision). The wait grace
    // (`since_unix`) and the `--since` window collapse to the tighter
    // (later) lower bound.
    if let Some(after) = filter.since_unix.into_iter().chain(filter.after_unix).max() {
        parts.push(format!("SINCE {}", imap_date(after)));
    }
    // BEFORE is INTERNALDATE < that day — the `--before` upper bound.
    if let Some(before) = filter.before_unix {
        parts.push(format!("BEFORE {}", imap_date(before)));
    }
    if let Some(from) = filter.from.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("FROM {}", imap_quote(from)));
    }
    if let Some(q) = filter.query.as_deref().filter(|s| !s.is_empty()) {
        // §11.1.8: query matches subject AND body → subject-OR-body.
        parts.push(format!("OR SUBJECT {} BODY {}", imap_quote(q), imap_quote(q)));
    }
    if parts.is_empty() {
        "ALL".to_string()
    } else {
        parts.join(" ")
    }
}

// ── Summary shaping (headers-only fetch → EmailSummary) ─────────────────

/// One summary out of a `UID FETCH (UID FLAGS INTERNALDATE
/// BODY.PEEK[HEADER])` item. Header bytes parse through mail-parser
/// (RFC 2047 display names decode for free). `has_attachment` is the
/// root Content-Type heuristic (multipart/mixed) — headers-only can't
/// know for sure; the full `read` is exact.
fn summary_from_fetch(
    uidvalidity: u32,
    fetch: &imap::types::Fetch<'_>,
) -> Option<EmailSummary> {
    let uid = fetch.uid?;
    let header_bytes = fetch.header()?;
    let msg = MessageParser::default().parse(header_bytes)?;
    let addr = |a: Option<&mail_parser::Address<'_>>| -> Vec<MailAddr> {
        a.map(|a| {
            a.iter()
                .filter_map(|x| {
                    let email = x.address.as_deref()?.trim().to_string();
                    if email.is_empty() {
                        return None;
                    }
                    Some(MailAddr {
                        name: x.name.as_deref().map(str::to_string),
                        email,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    };
    let received_at = fetch
        .internal_date()
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    let root_ct = msg
        .root_part()
        .headers()
        .iter()
        .find(|h| h.name == mail_parser::HeaderName::ContentType)
        .and_then(|h| h.value.as_content_type());
    let has_attachment = root_ct
        .map(|ct| {
            ct.ctype().eq_ignore_ascii_case("multipart")
                && ct
                    .subtype()
                    .map(|s| s.eq_ignore_ascii_case("mixed"))
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    Some(EmailSummary {
        id: external::encode_uid_token(uidvalidity, uid),
        thread_id: None,
        from: addr(msg.from()),
        to: addr(msg.to()),
        subject: msg.subject().unwrap_or_default().to_string(),
        received_at,
        unread: !fetch.flags().iter().any(|f| matches!(f, Flag::Seen)),
        has_attachment,
    })
}

// ── The trait impl ──────────────────────────────────────────────────────

impl ImapOps for RealImapOps {
    fn check_connect(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
    ) -> Result<ConnectCheck, String> {
        let mut session = login(inbox, password)?;
        let drafts_folder = resolve_drafts_folder(&mut session, inbox)?;
        let _ = session.logout();
        Ok(ConnectCheck { drafts_folder })
    }

    fn list_inbox(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        filter: &ListFilter,
        limit: usize,
    ) -> Result<Vec<EmailSummary>, ListError> {
        let mut session = login(inbox, password)?;
        let target = resolve_read_folder(&mut session, &filter.folder)?;
        let mailbox = session.select(&target).map_err(|e| imap_err("SELECT", e))?;
        let uidvalidity = mailbox.uid_validity.unwrap_or(0);
        let mut uids: Vec<u32> = session
            .uid_search(build_search_query(filter))
            .map_err(|e| imap_err("SEARCH", e))?
            .into_iter()
            .collect();
        // Newest first ≈ highest UID first (UIDs ascend with arrival
        // within one UIDVALIDITY generation); the route re-sorts the
        // merged list by date anyway. `offset` is the pagination cursor:
        // skip that many of the newest before taking a page.
        uids.sort_unstable_by(|a, b| b.cmp(a));
        let uids: Vec<u32> = uids.into_iter().skip(filter.offset).take(limit).collect();
        if uids.is_empty() {
            let _ = session.logout();
            return Ok(Vec::new());
        }
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let fetches = session
            .uid_fetch(set, "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])")
            .map_err(|e| imap_err("FETCH", e))?;
        let mut out: Vec<EmailSummary> = fetches
            .iter()
            .filter_map(|f| summary_from_fetch(uidvalidity, f))
            .collect();
        out.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        let _ = session.logout();
        Ok(out)
    }

    fn fetch_raw(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        uid_token: &str,
    ) -> Result<Option<RawEmail>, String> {
        let Some((want_validity, uid)) = external::parse_uid_token(uid_token) else {
            return Ok(None); // malformed token = unknown message
        };
        let mut session = login(inbox, password)?;
        let mailbox = session.select("INBOX").map_err(|e| imap_err("SELECT INBOX", e))?;
        if mailbox.uid_validity.unwrap_or(0) != want_validity {
            // The mailbox was rebuilt — the old UID may now be a
            // DIFFERENT message. Stale ids answer not-found, never the
            // wrong mail.
            let _ = session.logout();
            return Ok(None);
        }
        let fetches = session
            .uid_fetch(uid.to_string(), "(UID FLAGS INTERNALDATE BODY.PEEK[])")
            .map_err(|e| imap_err("FETCH", e))?;
        let raw = fetches.iter().find(|f| f.uid == Some(uid)).and_then(|f| {
            Some(RawEmail {
                received_at_iso: f
                    .internal_date()
                    .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                    .unwrap_or_default(),
                unread: !f.flags().iter().any(|fl| matches!(fl, Flag::Seen)),
                raw: f.body()?.to_vec(),
            })
        });
        let _ = session.logout();
        Ok(raw)
    }

    fn mark_seen(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        uid_token: &str,
    ) -> Result<(), String> {
        let Some((want_validity, uid)) = external::parse_uid_token(uid_token) else {
            return Err(format!("malformed message token '{uid_token}'"));
        };
        let mut session = login(inbox, password)?;
        let mailbox = session.select("INBOX").map_err(|e| imap_err("SELECT INBOX", e))?;
        if mailbox.uid_validity.unwrap_or(0) != want_validity {
            let _ = session.logout();
            return Ok(()); // stale token — nothing to mark
        }
        session
            .uid_store(uid.to_string(), "+FLAGS (\\Seen)")
            .map_err(|e| imap_err("STORE", e))?;
        let _ = session.logout();
        Ok(())
    }

    fn append_draft(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        folder: &str,
        rfc822: &[u8],
    ) -> Result<(), String> {
        let mut session = login(inbox, password)?;
        let result = append_draft_to(&mut session, folder, rfc822);
        let _ = session.logout();
        result
    }

    fn manage(
        &self,
        inbox: &MailExternalInbox,
        password: &str,
        op: &ManageOp,
    ) -> Result<ManageOutcome, ListError> {
        let mut session = login(inbox, password).map_err(ListError::Engine)?;
        let result = manage_on_session(&mut session, op);
        let _ = session.logout();
        result
    }
}

// ── ⚠ LIVE-BOX #3: S11 management ops (MOVE / STORE / CREATE / RENAME) ───
//
// DELETE is ALWAYS a MOVE to Trash (never an EXPUNGE) — there is no
// permanent-deletion code path here. Move/archive/delete use RFC 6851
// `UID MOVE`; a server WITHOUT the MOVE extension surfaces the server's
// error verbatim (we deliberately do NOT fall back to COPY + STORE
// \Deleted + EXPUNGE, so K2 can never expunge a message). Archive/Trash
// resolution reuses the `pick_archive_folder`/`pick_trash_folder`
// SPECIAL-USE patterns (same UTF-7 / SPECIAL-USE-variance caveats as
// [`survey_folders`]). All message ops SELECT INBOX and verify
// UIDVALIDITY first (a rebuilt mailbox → a loud "no longer in the inbox"
// error, never the wrong message).

/// LIST "" * → the folder NAMES (for Move-Named / rename / list).
fn manage_list_names(session: &mut ImapSession) -> Result<Vec<String>, ListError> {
    let names = session.list(Some(""), Some("*")).map_err(|e| ListError::Engine(imap_err("LIST", e)))?;
    Ok(names.iter().map(|n: &Name| n.name().to_string()).collect())
}

/// LIST "" * → `(name, has <attr> special-use)` for a chosen SPECIAL-USE
/// attribute (feeds the pure Junk/Archive/Trash pickers).
fn manage_survey_special(
    session: &mut ImapSession,
    matches: impl Fn(&NameAttribute) -> bool,
) -> Result<Vec<(String, bool)>, ListError> {
    let names = session.list(Some(""), Some("*")).map_err(|e| ListError::Engine(imap_err("LIST", e)))?;
    Ok(names
        .iter()
        .map(|n: &Name| {
            let special = n.attributes().iter().any(&matches);
            (n.name().to_string(), special)
        })
        .collect())
}

/// SELECT INBOX, verify the token's UIDVALIDITY, return the UID. A
/// malformed token or a rebuilt mailbox is a loud error — never a MOVE
/// of the WRONG message.
fn select_inbox_for_uid(session: &mut ImapSession, uid_token: &str) -> Result<u32, ListError> {
    let Some((want_validity, uid)) = external::parse_uid_token(uid_token) else {
        return Err(ListError::Engine(format!("malformed message token '{uid_token}'")));
    };
    let mailbox = session.select("INBOX").map_err(|e| ListError::Engine(imap_err("SELECT INBOX", e)))?;
    if mailbox.uid_validity.unwrap_or(0) != want_validity {
        return Err(ListError::Engine(
            "this message is no longer in the inbox (the mailbox was rebuilt) — re-list with \
             'k2 mail messages'"
                .to_string(),
        ));
    }
    Ok(uid)
}

/// UID STORE +/-FLAGS for one flag (`\Seen` / `\Flagged`).
fn store_flag(session: &mut ImapSession, uid: u32, flag: &str, on: bool) -> Result<(), ListError> {
    let op = if on { "+FLAGS" } else { "-FLAGS" };
    session
        .uid_store(uid.to_string(), format!("{op} ({flag})"))
        .map(|_| ())
        .map_err(|e| ListError::Engine(imap_err("STORE", e)))
}

/// Resolve a Move destination folder name (Inbox/Junk/Named) against the
/// live LIST. A mistyped Named/Junk is [`ListError::UnknownFolder`].
fn resolve_move_dest(session: &mut ImapSession, dest: &MailFolder) -> Result<String, ListError> {
    match dest {
        MailFolder::Inbox => Ok("INBOX".to_string()),
        MailFolder::Junk => {
            let folders = manage_survey_special(session, |a| matches!(a, NameAttribute::Junk))?;
            external::pick_junk_folder(&folders).ok_or_else(|| ListError::UnknownFolder {
                requested: "junk".to_string(),
                available: folders.into_iter().map(|(n, _)| n).collect(),
            })
        }
        MailFolder::Named(want) => {
            let names = manage_list_names(session)?;
            names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(want))
                .cloned()
                .ok_or_else(|| ListError::UnknownFolder { requested: want.clone(), available: names })
        }
    }
}

/// Decide the mailbox name to hand to CREATE so a BARE name lands in the
/// server's PERSONAL namespace. Dovecot-style servers root user folders
/// under `INBOX<delim>` (e.g. `INBOX.Work`) and reject a bare `Work`;
/// Gmail (`[Gmail]/…`) and root-namespace servers take it as-is. We only
/// prepend `INBOX<delim>` when the LISTed hierarchy actually shows an
/// INBOX-rooted namespace AND the caller has not already supplied a path.
/// Pure (no I/O) so it is unit-testable off the LIST result alone.
fn namespaced_folder_name(names: &[String], delimiter: Option<&str>, want: &str) -> String {
    let Some(delim) = delimiter.filter(|d| !d.is_empty()) else {
        return want.to_string();
    };
    // Caller already gave a hierarchy path (nested or prefixed) — respect it.
    if want.contains(delim) {
        return want.to_string();
    }
    let prefix = format!("INBOX{delim}");
    // INBOX-rooted server: at least one existing folder lives under it.
    let inbox_rooted = names.iter().any(|n| n.starts_with(&prefix));
    if inbox_rooted {
        format!("{prefix}{want}")
    } else {
        want.to_string()
    }
}

/// Extract a namespace prefix a server SUGGESTS in a CREATE error, e.g.
/// `… nonexistent namespace (Mailbox name should probably be prefixed
/// with: INBOX.)` → `INBOX.`. `None` when the error carries no such hint.
fn suggested_create_prefix(err: &str) -> Option<String> {
    let lower = err.to_ascii_lowercase();
    let at = lower.find("prefixed with:")?;
    let tail = err[at + "prefixed with:".len()..].trim_start();
    let prefix: String = tail
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ')' && *c != '"' && *c != '\'')
        .collect();
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

/// The single management dispatch on an open session.
fn manage_on_session(session: &mut ImapSession, op: &ManageOp) -> Result<ManageOutcome, ListError> {
    match op {
        ManageOp::FolderList => Ok(ManageOutcome {
            folders: manage_list_names(session)?,
            ..Default::default()
        }),
        ManageOp::FolderCreate { name } => {
            // Derive the server's hierarchy from LIST so a BARE folder name
            // lands in the personal namespace. Dovecot-style servers root
            // user folders at `INBOX.<x>` and reject a bare `Work` with a
            // "nonexistent namespace" error; Gmail (`[Gmail]/…`) and
            // root-namespace servers take the bare name. Detect, don't assume.
            let listed = session
                .list(Some(""), Some("*"))
                .map_err(|e| ListError::Engine(imap_err("LIST", e)))?;
            let delimiter = listed.iter().find_map(|n| n.delimiter()).map(str::to_string);
            let folder_names: Vec<String> = listed.iter().map(|n| n.name().to_string()).collect();
            let target = namespaced_folder_name(&folder_names, delimiter.as_deref(), name);
            match session.create(&target) {
                Ok(()) => Ok(ManageOutcome::default()),
                Err(e) => {
                    // The server may itself SUGGEST the prefix in its error
                    // ("… prefixed with: INBOX."). Retry ONCE with it — but
                    // only if it differs from what we already tried.
                    let msg = imap_err("CREATE", e);
                    if let Some(prefix) = suggested_create_prefix(&msg) {
                        let retried = format!("{prefix}{name}");
                        if retried != target {
                            session
                                .create(&retried)
                                .map_err(|e2| ListError::Engine(imap_err("CREATE", e2)))?;
                            return Ok(ManageOutcome::default());
                        }
                    }
                    Err(ListError::Engine(msg))
                }
            }
        }
        ManageOp::FolderRename { from, to } => {
            let names = manage_list_names(session)?;
            let real = names.iter().find(|n| n.eq_ignore_ascii_case(from)).cloned().ok_or_else(
                || ListError::UnknownFolder { requested: from.to_string(), available: names.clone() },
            )?;
            session
                .rename(&real, to)
                .map_err(|e| ListError::Engine(imap_err("RENAME", e)))?;
            Ok(ManageOutcome::default())
        }
        ManageOp::Flags { uid_token, read, flagged } => {
            let uid = select_inbox_for_uid(session, uid_token)?;
            if let Some(r) = read {
                store_flag(session, uid, "\\Seen", *r)?;
            }
            if let Some(f) = flagged {
                store_flag(session, uid, "\\Flagged", *f)?;
            }
            Ok(ManageOutcome::default())
        }
        ManageOp::Move { uid_token, dest } => {
            let folder = resolve_move_dest(session, dest)?;
            let uid = select_inbox_for_uid(session, uid_token)?;
            session
                .uid_mv(uid.to_string(), &folder)
                .map_err(|e| ListError::Engine(imap_err("UID MOVE", e)))?;
            Ok(ManageOutcome { folder: Some(folder), ..Default::default() })
        }
        ManageOp::Archive { uid_token } => {
            let folders = manage_survey_special(session, |a| matches!(a, NameAttribute::Archive))?;
            let folder = external::pick_archive_folder(&folders).ok_or_else(|| {
                ListError::UnknownFolder {
                    requested: "archive".to_string(),
                    available: folders.into_iter().map(|(n, _)| n).collect(),
                }
            })?;
            let uid = select_inbox_for_uid(session, uid_token)?;
            session
                .uid_mv(uid.to_string(), &folder)
                .map_err(|e| ListError::Engine(imap_err("UID MOVE", e)))?;
            Ok(ManageOutcome { folder: Some(folder), ..Default::default() })
        }
        ManageOp::Trash { uid_token } => {
            // DELETE = move to Trash. NEVER an EXPUNGE.
            let folders = manage_survey_special(session, |a| matches!(a, NameAttribute::Trash))?;
            let folder = external::pick_trash_folder(&folders).ok_or_else(|| {
                ListError::UnknownFolder {
                    requested: "trash".to_string(),
                    available: folders.into_iter().map(|(n, _)| n).collect(),
                }
            })?;
            let uid = select_inbox_for_uid(session, uid_token)?;
            session
                .uid_mv(uid.to_string(), &folder)
                .map_err(|e| ListError::Engine(imap_err("UID MOVE", e)))?;
            Ok(ManageOutcome { folder: Some(folder), ..Default::default() })
        }
    }
}

/// APPEND with `\Draft` (session-level so the loopback mock exercises
/// the exact production conversation).
fn append_draft_to(session: &mut ImapSession, folder: &str, rfc822: &[u8]) -> Result<(), String> {
    session
        .append(folder, rfc822)
        .flag(Flag::Draft)
        .finish()
        .map(|_| ())
        .map_err(|e| imap_err("APPEND", e))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — pure builders + a SCRIPTED LOOPBACK MOCK for the
// connect/LIST/APPEND conversation (127.0.0.1 ephemeral port, no real
// network, no TLS — the production path's TLS is rustls-with-
// verification and is deliberately not mockable).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    // ── pure builders ──

    #[test]
    fn search_query_covers_the_listfilter_surface_and_escapes() {
        assert_eq!(build_search_query(&ListFilter::default()), "ALL");
        let f = ListFilter {
            unread_only: true,
            query: Some("verify \"now\"".to_string()),
            from: Some("git\\hub".to_string()),
            since_unix: Some(1_783_000_000), // 2026-07-02
            ..Default::default()
        };
        let q = build_search_query(&f);
        assert!(q.starts_with("UNSEEN SINCE "), "{q}");
        assert!(q.contains("-2026 FROM \"git\\\\hub\""), "{q}");
        assert!(q.contains("OR SUBJECT \"verify \\\"now\\\"\" BODY \"verify \\\"now\\\"\""), "{q}");
        // CR/LF can never smuggle a second command.
        let f = ListFilter { from: Some("a\r\nA2 DELETE INBOX".to_string()), ..Default::default() };
        let q = build_search_query(&f);
        assert!(!q.contains('\r') && !q.contains('\n'), "{q}");
    }

    #[test]
    fn imap_date_is_english_and_unpadded() {
        // 2026-07-08 12:00 UTC.
        assert_eq!(imap_date(1_783_512_000), "8-Jul-2026");
    }

    #[test]
    fn search_query_emits_since_and_before_date_window() {
        let f = ListFilter {
            after_unix: Some(1_783_512_000),  // 2026-07-08
            before_unix: Some(1_783_598_400), // 2026-07-09
            ..Default::default()
        };
        let q = build_search_query(&f);
        assert!(q.contains("SINCE 8-Jul-2026"), "{q}");
        assert!(q.contains("BEFORE 9-Jul-2026"), "{q}");
        // The wait grace and --since window collapse to the later SINCE.
        let f = ListFilter { since_unix: Some(1), after_unix: Some(1_783_512_000), ..Default::default() };
        let q = build_search_query(&f);
        assert!(q.contains("SINCE 8-Jul-2026") && !q.contains("SINCE 1-Jan-1970"), "{q}");
    }

    // ── the scripted loopback mock ──

    /// A minimal scripted IMAP server: greets, then answers each
    /// tagged command by verb. Panics (test failure) on anything
    /// unexpected — the script IS the assertion.
    fn spawn_mock(expect_append_body: Vec<u8>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
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
            w.write_all(b"* OK mock IMAP4rev1 ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end().to_string();
                seen.push(line.clone());
                let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                let verb = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_ascii_uppercase();
                match verb.as_str() {
                    "LOGIN" => {
                        // Never OK a wrong password — the mock knows one.
                        if line.contains("\"app-password\"") {
                            w.write_all(format!("{tag} OK LOGIN done\r\n").as_bytes()).unwrap();
                        } else {
                            w.write_all(
                                format!("{tag} NO [AUTHENTICATIONFAILED] bad credentials\r\n")
                                    .as_bytes(),
                            )
                            .unwrap();
                        }
                    }
                    "LIST" => {
                        w.write_all(
                            b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                              * LIST (\\HasNoChildren \\Drafts) \"/\" \"Entwuerfe\"\r\n",
                        )
                        .unwrap();
                        w.write_all(format!("{tag} OK LIST done\r\n").as_bytes()).unwrap();
                    }
                    "APPEND" => {
                        // `... APPEND "folder" (\Draft) {N}` → go-ahead,
                        // read exactly N bytes + CRLF, then OK.
                        assert!(line.contains("(\\Draft)"), "draft flag required: {line}");
                        assert!(line.contains("\"Entwuerfe\""), "wrong folder: {line}");
                        let n: usize = line
                            .rsplit('{')
                            .next()
                            .and_then(|s| s.strip_suffix('}'))
                            .and_then(|s| s.parse().ok())
                            .expect("literal size");
                        w.write_all(b"+ go ahead\r\n").unwrap();
                        let mut buf = vec![0u8; n + 2];
                        reader.read_exact(&mut buf).expect("literal bytes");
                        assert_eq!(&buf[..n], expect_append_body.as_slice(), "APPEND literal");
                        assert_eq!(&buf[n..], b"\r\n");
                        w.write_all(format!("{tag} OK APPEND completed\r\n").as_bytes()).unwrap();
                    }
                    "LOGOUT" => {
                        w.write_all(b"* BYE mock done\r\n").unwrap();
                        w.write_all(format!("{tag} OK LOGOUT done\r\n").as_bytes()).unwrap();
                        break;
                    }
                    other => panic!("mock got unexpected command '{other}': {line}"),
                }
            }
            seen
        });
        (port, handle)
    }

    fn plaintext_session(port: u16, password: &str) -> Result<ImapSession, String> {
        let client = connect_plaintext_for_test("127.0.0.1", port)?;
        client
            .login("rosson@example.com", password)
            .map_err(|(e, _)| imap_err("login", e))
    }

    /// The connect → LOGIN → LIST(SPECIAL-USE) → APPEND-\Draft →
    /// LOGOUT conversation against the scripted mock: the exact
    /// production helpers, minus TLS.
    #[test]
    fn loopback_mock_login_list_detect_and_append_draft() {
        let draft = b"Subject: Re: hi\r\n\r\ndGVzdA==\r\n".to_vec();
        let (port, handle) = spawn_mock(draft.clone());

        let mut session = plaintext_session(port, "app-password").expect("login");
        // Folder survey sees the SPECIAL-USE \Drafts attribute.
        let folders = survey_folders(&mut session).expect("LIST");
        assert_eq!(
            folders,
            vec![("INBOX".to_string(), false), ("Entwuerfe".to_string(), true)]
        );
        assert_eq!(
            external::pick_drafts_folder(&folders).as_deref(),
            Some("Entwuerfe"),
            "SPECIAL-USE wins over name matching"
        );
        // APPEND with \Draft — the mock asserts flag + folder + bytes.
        append_draft_to(&mut session, "Entwuerfe", &draft).expect("APPEND");
        session.logout().expect("LOGOUT");

        let seen = handle.join().expect("mock thread");
        assert!(seen.iter().any(|l| l.contains("LOGIN")), "{seen:?}");
        // The password never appears anywhere BUT the LOGIN line (no
        // logging surface in this test, but the invariant is cheap to
        // pin: nothing else echoes it).
        assert_eq!(
            seen.iter().filter(|l| l.contains("app-password")).count(),
            1,
            "{seen:?}"
        );
    }

    // ── folder-create namespace prefix (#31.3) ──

    #[test]
    fn namespaced_folder_name_prefixes_only_inbox_rooted_servers() {
        // Dovecot: user folders live under `INBOX.` → prefix a bare name.
        let dovecot = vec!["INBOX".to_string(), "INBOX.Archive".to_string()];
        assert_eq!(
            namespaced_folder_name(&dovecot, Some("."), "Work"),
            "INBOX.Work"
        );
        // …but respect a path the caller already prefixed / nested.
        assert_eq!(
            namespaced_folder_name(&dovecot, Some("."), "INBOX.Work"),
            "INBOX.Work"
        );
        // Gmail: `[Gmail]/…`, delimiter '/', NOT INBOX-rooted → leave bare.
        let gmail = vec![
            "INBOX".to_string(),
            "[Gmail]/Sent Mail".to_string(),
            "[Gmail]/Trash".to_string(),
        ];
        assert_eq!(namespaced_folder_name(&gmail, Some("/"), "Work"), "Work");
        // Root-namespace server (flat, no INBOX.-rooted siblings) → bare.
        let flat = vec!["INBOX".to_string(), "Archive".to_string()];
        assert_eq!(namespaced_folder_name(&flat, Some("."), "Work"), "Work");
        // No delimiter reported → never guess.
        assert_eq!(namespaced_folder_name(&dovecot, None, "Work"), "Work");
    }

    #[test]
    fn suggested_create_prefix_reads_the_servers_hint() {
        let err = "CREATE failed: Client tried to access nonexistent namespace. \
                   (Mailbox name should probably be prefixed with: INBOX.)";
        assert_eq!(suggested_create_prefix(err).as_deref(), Some("INBOX."));
        // No hint → nothing to retry with.
        assert_eq!(suggested_create_prefix("CREATE failed: over quota"), None);
    }

    /// A scripted mock for folder-create: greets, LOGIN, answers LIST with
    /// a fixed body, and either accepts CREATE or rejects a BARE name once
    /// (with the server's namespace hint) so the retry path is exercised.
    fn spawn_folder_mock(
        list_body: &'static [u8],
        reject: Option<(&'static str, &'static str)>,
    ) -> (u16, std::thread::JoinHandle<Vec<String>>) {
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
            w.write_all(b"* OK mock IMAP4rev1 ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end().to_string();
                seen.push(line.clone());
                let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                let verb = line.split_whitespace().nth(1).unwrap_or("").to_ascii_uppercase();
                match verb.as_str() {
                    "LOGIN" => {
                        w.write_all(format!("{tag} OK LOGIN done\r\n").as_bytes()).unwrap();
                    }
                    "LIST" => {
                        w.write_all(list_body).unwrap();
                        w.write_all(format!("{tag} OK LIST done\r\n").as_bytes()).unwrap();
                    }
                    "CREATE" => {
                        let bare_fails = reject
                            .map(|(bad, _)| line.contains(bad) && !line.contains("INBOX."))
                            .unwrap_or(false);
                        if bare_fails {
                            let hint = reject.unwrap().1;
                            w.write_all(format!("{tag} NO {hint}\r\n").as_bytes()).unwrap();
                        } else {
                            w.write_all(format!("{tag} OK CREATE done\r\n").as_bytes()).unwrap();
                        }
                    }
                    "LOGOUT" => {
                        w.write_all(b"* BYE mock done\r\n").unwrap();
                        w.write_all(format!("{tag} OK LOGOUT done\r\n").as_bytes()).unwrap();
                        break;
                    }
                    other => panic!("mock got unexpected command '{other}': {line}"),
                }
            }
            seen
        });
        (port, handle)
    }

    /// Dovecot-style: LIST shows `INBOX.` children → a bare `Work` is
    /// auto-prefixed to `INBOX.Work` before CREATE (no error round-trip).
    #[test]
    fn folder_create_auto_prefixes_from_inbox_rooted_list() {
        let (port, handle) = spawn_folder_mock(
            b"* LIST (\\HasNoChildren) \".\" \"INBOX\"\r\n\
              * LIST (\\HasNoChildren) \".\" \"INBOX.Archive\"\r\n",
            None,
        );
        let mut session = plaintext_session(port, "app-password").expect("login");
        manage_on_session(&mut session, &ManageOp::FolderCreate { name: "Work" }).expect("create");
        session.logout().ok();
        let seen = handle.join().expect("mock thread");
        let create = seen.iter().find(|l| l.contains("CREATE")).expect("a CREATE was sent");
        assert!(create.contains("INBOX.Work"), "auto-prefixed: {create}");
    }

    /// Empty account (LIST reveals only INBOX, so auto-detect can't fire):
    /// the bare CREATE is rejected with the server's hint, and we RETRY
    /// with the suggested `INBOX.` prefix.
    #[test]
    fn folder_create_retries_on_server_namespace_hint() {
        let (port, handle) = spawn_folder_mock(
            b"* LIST (\\HasNoChildren) \".\" \"INBOX\"\r\n",
            Some((
                "Work",
                "[CANNOT] Client tried to access nonexistent namespace. \
                 (Mailbox name should probably be prefixed with: INBOX.)",
            )),
        );
        let mut session = plaintext_session(port, "app-password").expect("login");
        manage_on_session(&mut session, &ManageOp::FolderCreate { name: "Work" }).expect("create");
        session.logout().ok();
        let seen = handle.join().expect("mock thread");
        let creates: Vec<&String> = seen.iter().filter(|l| l.contains("CREATE")).collect();
        assert_eq!(creates.len(), 2, "bare then prefixed: {creates:?}");
        assert!(creates[0].contains("Work") && !creates[0].contains("INBOX."), "{:?}", creates[0]);
        assert!(creates[1].contains("INBOX.Work"), "retry prefixed: {:?}", creates[1]);
    }

    /// Gmail-style (`[Gmail]/…`, '/' delimiter, NOT INBOX-rooted): a bare
    /// name is NOT prefixed — we never blindly prepend `INBOX.`.
    #[test]
    fn folder_create_leaves_gmail_style_names_bare() {
        let (port, handle) = spawn_folder_mock(
            b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
              * LIST (\\HasChildren \\Noselect) \"/\" \"[Gmail]\"\r\n\
              * LIST (\\HasNoChildren \\Sent) \"/\" \"[Gmail]/Sent Mail\"\r\n",
            None,
        );
        let mut session = plaintext_session(port, "app-password").expect("login");
        manage_on_session(&mut session, &ManageOp::FolderCreate { name: "Work" }).expect("create");
        session.logout().ok();
        let seen = handle.join().expect("mock thread");
        let create = seen.iter().find(|l| l.contains("CREATE")).expect("a CREATE was sent");
        assert!(create.contains("Work") && !create.contains("INBOX"), "left bare: {create}");
    }

    /// Wrong credentials fail loudly with the SERVER's text and no
    /// password in the error.
    #[test]
    fn loopback_mock_rejects_bad_credentials() {
        let (port, handle) = spawn_mock(Vec::new());
        let err = plaintext_session(port, "wrong-password").expect_err("must fail");
        assert!(err.contains("AUTHENTICATIONFAILED") || err.contains("bad credentials"), "{err}");
        assert!(!err.contains("wrong-password"), "the password never rides an error: {err}");
        // The client bails before LOGOUT; drop the listener thread.
        drop(handle);
    }

    /// The configured-override branch of drafts resolution: a typo'd
    /// folder is refused WITH the server's real folder list.
    #[test]
    fn loopback_mock_verifies_configured_drafts_folder_exists() {
        let (port, handle) = spawn_mock(Vec::new());
        let mut session = plaintext_session(port, "app-password").expect("login");
        let mut inbox = crate::mail::external::tests::test_inbox(
            "X1",
            "p1",
            "rosson@example.com",
        );
        inbox.drafts_folder = Some("Draftz".to_string());
        let err = resolve_drafts_folder(&mut session, &inbox).expect_err("typo refused");
        assert!(err.contains("'Draftz' does not exist"), "{err}");
        assert!(err.contains("Entwuerfe"), "names the real folders: {err}");
        // And the existing override passes through verbatim.
        inbox.drafts_folder = Some("Entwuerfe".to_string());
        let got = resolve_drafts_folder(&mut session, &inbox).expect("exists");
        assert_eq!(got.as_deref(), Some("Entwuerfe"));
        session.logout().expect("LOGOUT");
        let _ = handle.join();
    }

    /// Production `connect()` refuses a schema-drifted TLS kind loudly
    /// — there is no plaintext arm to fall into.
    #[test]
    fn connect_has_no_plaintext_arm() {
        let mut inbox =
            crate::mail::external::tests::test_inbox("X1", "p1", "rosson@example.com");
        inbox.tls = "plaintext".to_string();
        let err = connect(&inbox).expect_err("refused");
        assert!(err.contains("unsupported tls kind"), "{err}");
    }

    /// #31.6: archiving a Gmail message resolves to the provider's
    /// `[Gmail]/All Mail` (vs `Archive` / `INBOX.Archive` elsewhere), and
    /// that resolved destination rides back in `ManageOutcome.folder` so
    /// the CLI success line can say `archived → [Gmail]/All Mail`.
    /// Scripted over the loopback mock: LIST → SELECT INBOX → UID MOVE,
    /// never an EXPUNGE. No real Gmail, no network.
    #[test]
    fn archive_resolves_and_returns_gmail_all_mail_destination() {
        let (port, handle) = spawn_gmail_manage_mock();
        let mut session = plaintext_session(port, "app-password").expect("login");
        let tok = external::encode_uid_token(777, 42);
        let out = manage_on_session(&mut session, &ManageOp::Archive { uid_token: &tok })
            .expect("archive");
        assert_eq!(
            out.folder.as_deref(),
            Some("[Gmail]/All Mail"),
            "Gmail archive lands in All Mail and that destination is surfaced",
        );
        session.logout().expect("LOGOUT");
        let seen = handle.join().expect("mock thread");
        // Archive was a MOVE to All Mail — never an EXPUNGE.
        assert!(
            seen.iter().any(|l| {
                let u = l.to_ascii_uppercase();
                u.contains("UID MOVE") && l.contains("[Gmail]/All Mail")
            }),
            "{seen:?}",
        );
        assert!(
            !seen.iter().any(|l| l.to_ascii_uppercase().contains("EXPUNGE")),
            "{seen:?}",
        );
    }

    /// A Gmail-shaped scripted IMAP: LIST advertises `[Gmail]/All Mail`
    /// (SPECIAL-USE `\All`, NOT `\Archive` — the pick falls to the common
    /// name), SELECT INBOX pins UIDVALIDITY 777, UID MOVE completes.
    /// Returns the command lines it saw.
    fn spawn_gmail_manage_mock() -> (u16, std::thread::JoinHandle<Vec<String>>) {
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
            w.write_all(b"* OK mock Gmail IMAP4rev1 ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end().to_string();
                seen.push(line.clone());
                let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                let verb = line.split_whitespace().nth(1).unwrap_or("").to_ascii_uppercase();
                match verb.as_str() {
                    "LOGIN" => {
                        w.write_all(format!("{tag} OK LOGIN done\r\n").as_bytes()).unwrap();
                    }
                    "LIST" => {
                        w.write_all(
                            b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                              * LIST (\\HasNoChildren \\All) \"/\" \"[Gmail]/All Mail\"\r\n\
                              * LIST (\\HasNoChildren \\Trash) \"/\" \"[Gmail]/Trash\"\r\n",
                        )
                        .unwrap();
                        w.write_all(format!("{tag} OK LIST done\r\n").as_bytes()).unwrap();
                    }
                    "SELECT" => {
                        w.write_all(
                            b"* 3 EXISTS\r\n\
                              * 0 RECENT\r\n\
                              * OK [UIDVALIDITY 777] UIDs valid\r\n\
                              * OK [UIDNEXT 99] Predicted next UID\r\n",
                        )
                        .unwrap();
                        w.write_all(
                            format!("{tag} OK [READ-WRITE] SELECT done\r\n").as_bytes(),
                        )
                        .unwrap();
                    }
                    "UID" => {
                        // `... UID MOVE <uid> "<folder>"` → completed.
                        w.write_all(format!("{tag} OK MOVE completed\r\n").as_bytes()).unwrap();
                    }
                    "LOGOUT" => {
                        w.write_all(b"* BYE mock done\r\n").unwrap();
                        w.write_all(format!("{tag} OK LOGOUT done\r\n").as_bytes()).unwrap();
                        break;
                    }
                    other => panic!("gmail mock got unexpected command '{other}': {line}"),
                }
            }
            seen
        });
        (port, handle)
    }

    // ── O2: SASL XOAUTH2 (no real network, no real Gmail) ───────────────

    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;

    const TEST_ACCESS_TOKEN: &str = "ya29.SENTINEL-ACCESS-TOKEN-do-not-leak";

    /// The SASL init string is byte-exact per prd §5a:
    /// `user=<user>^Aauth=Bearer <token>^A^A`.
    #[test]
    fn sasl_xoauth2_init_is_byte_exact() {
        let s = sasl_xoauth2_init("me@gmail.com", "TOK");
        assert_eq!(s, "user=me@gmail.com\x01auth=Bearer TOK\x01\x01");
        // Exactly three 0x01 bytes, in the right places; no trailing junk.
        assert_eq!(s.bytes().filter(|b| *b == 0x01).count(), 3);
        assert!(s.starts_with("user=me@gmail.com\u{1}auth=Bearer "));
        assert!(s.ends_with("\u{1}\u{1}"));
    }

    /// A scripted IMAP server speaking the XOAUTH2 AUTHENTICATE
    /// conversation. Captures the base64-DECODED SASL response for the
    /// test to assert. `reject=true` replays Google's error surface: the
    /// rejection rides a base64-JSON blob on a `+` continuation, NOT a
    /// plain `NO`.
    fn spawn_xoauth2_mock(reject: bool) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .expect("read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut w = stream;
            let mut captured_sasl: Vec<u8> = Vec::new();
            w.write_all(b"* OK mock IMAP4rev1 ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let trimmed = line.trim_end().to_string();
                let mut parts = trimmed.split_whitespace();
                let tag = parts.next().unwrap_or("*").to_string();
                let verb = parts.next().unwrap_or("").to_ascii_uppercase();
                match verb.as_str() {
                    "AUTHENTICATE" => {
                        assert_eq!(parts.next(), Some("XOAUTH2"), "wrong mechanism: {trimmed}");
                        // Empty challenge (regex `^\+ (.*)\r\n` → "").
                        w.write_all(b"+ \r\n").unwrap();
                        // The base64(SASL) response line.
                        let mut resp = String::new();
                        reader.read_line(&mut resp).expect("sasl line");
                        captured_sasl = B64.decode(resp.trim_end()).expect("base64 sasl");
                        if reject {
                            // Google's rejected-token surface: base64-JSON
                            // on a continuation, then the tagged NO.
                            let blob = br#"{"status":"400","schemes":"Bearer","scope":"https://mail.google.com/"}"#;
                            w.write_all(format!("+ {}\r\n", B64.encode(blob)).as_bytes()).unwrap();
                            // The client answers empty; consume it.
                            let mut empty = String::new();
                            reader.read_line(&mut empty).expect("empty response");
                            w.write_all(
                                format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n")
                                    .as_bytes(),
                            )
                            .unwrap();
                        } else {
                            w.write_all(format!("{tag} OK authenticated\r\n").as_bytes()).unwrap();
                        }
                        break;
                    }
                    other => panic!("mock got unexpected command '{other}': {trimmed}"),
                }
            }
            captured_sasl
        });
        (port, handle)
    }

    /// The success path: `xoauth2_authenticate` speaks XOAUTH2 and the
    /// server sees EXACTLY the byte-exact SASL string with our token.
    #[test]
    fn xoauth2_authenticate_sends_exact_sasl_and_succeeds() {
        let (port, handle) = spawn_xoauth2_mock(false);
        let client = connect_plaintext_for_test("127.0.0.1", port).expect("connect");
        let session = xoauth2_authenticate(client, "rosson@gmail.com", TEST_ACCESS_TOKEN)
            .expect("XOAUTH2 login");
        drop(session);
        let sasl = handle.join().expect("mock thread");
        assert_eq!(
            sasl,
            format!("user=rosson@gmail.com\x01auth=Bearer {TEST_ACCESS_TOKEN}\x01\x01").into_bytes(),
            "the server must see the byte-exact XOAUTH2 SASL string"
        );
    }

    /// The rejection path: Google's base64-JSON error on a continuation
    /// (NOT a `NO`) maps to a clear re-link message — and the access
    /// token NEVER appears in that error.
    #[test]
    fn xoauth2_authenticate_classifies_token_rejection_without_leaking_token() {
        let (port, handle) = spawn_xoauth2_mock(true);
        let client = connect_plaintext_for_test("127.0.0.1", port).expect("connect");
        let err = xoauth2_authenticate(client, "rosson@gmail.com", TEST_ACCESS_TOKEN)
            .expect_err("token must be rejected");
        assert!(err.contains("re-linked"), "must instruct a re-link: {err}");
        assert!(err.contains("400"), "surfaces the XOAUTH2 status: {err}");
        assert!(
            !err.contains(TEST_ACCESS_TOKEN) && !err.contains("Bearer"),
            "the token/SASL must never ride the error: {err}"
        );
        // The server still saw the byte-exact SASL (auth was attempted).
        let sasl = handle.join().expect("mock thread");
        assert_eq!(
            sasl,
            format!("user=rosson@gmail.com\x01auth=Bearer {TEST_ACCESS_TOKEN}\x01\x01").into_bytes()
        );
    }

    // ── O2: the token fetch/refresh → SASL wiring (O1 mock HTTP) ─────────

    /// In-memory vault fake (no filesystem).
    #[derive(Default)]
    struct MemStore {
        map: Mutex<std::collections::HashMap<String, String>>,
    }
    impl crate::mail::secrets::SecretStore for MemStore {
        fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
            let key = format!("mem_{kind}_{}", self.map.lock().unwrap().len());
            self.map.lock().unwrap().insert(key.clone(), secret.to_string());
            Ok(key)
        }
        fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
            Ok(self.map.lock().unwrap().get(sref).cloned())
        }
        fn delete(&self, sref: &str) -> Result<(), String> {
            self.map.lock().unwrap().remove(sref);
            Ok(())
        }
        fn store_exact(&self, key: &str, secret: &str) -> Result<(), String> {
            self.map.lock().unwrap().insert(key.to_string(), secret.to_string());
            Ok(())
        }
    }

    /// One-shot HTTP fake returning a canned refresh reply (no network).
    struct OneShotHttp {
        body: String,
        calls: Mutex<usize>,
    }
    impl oauth::HttpClient for OneShotHttp {
        fn post_form(
            &self,
            _url: &str,
            _form: &[(&str, &str)],
        ) -> Result<oauth::HttpResponse, oauth::OauthError> {
            *self.calls.lock().unwrap() += 1;
            Ok(oauth::HttpResponse { status: 200, body: self.body.clone() })
        }
    }

    /// Drive the O1 engine's refresh path through a mock token endpoint,
    /// then feed the REFRESHED access token into the SASL builder — the
    /// end-to-end shape `oauth_login` uses, minus DB + real network. The
    /// refreshed token (not the stale one) lands in the SASL string.
    #[test]
    fn refresh_then_sasl_uses_the_fresh_token() {
        let store = MemStore::default();
        // Seed a stale bundle (expiry in the past → force a refresh).
        let seed = oauth::Tokens {
            access_token: "STALE-access".into(),
            refresh_token: Some("RT-1".into()),
            scope: Some("https://mail.google.com/".into()),
            token_type: "Bearer".into(),
            expires_in: 3600,
        };
        oauth::store_tokens(&store, "rowA", &seed, 0).expect("seed");

        let http = OneShotHttp {
            body: r#"{"access_token":"FRESH-access","expires_in":3600,"token_type":"Bearer"}"#
                .into(),
            calls: Mutex::new(0),
        };
        // now=10_000, stored expiry(=3600) is long past → refresh.
        let fresh = oauth::access_token_for(
            &store,
            "rowA",
            OauthProvider::Gmail,
            Some(3_600),
            10_000,
            None,
            None,
            &http,
        )
        .expect("access token");
        assert!(fresh.refreshed, "expiry in the past must refresh");
        assert_eq!(fresh.access_token, "FRESH-access");
        assert_eq!(*http.calls.lock().unwrap(), 1, "exactly one refresh call");

        let sasl = sasl_xoauth2_init("rosson@gmail.com", &fresh.access_token);
        assert_eq!(sasl, "user=rosson@gmail.com\x01auth=Bearer FRESH-access\x01\x01");
        assert!(!sasl.contains("STALE-access"), "the stale token must not ride the SASL");
    }
}
