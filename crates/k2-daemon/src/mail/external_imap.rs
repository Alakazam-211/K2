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

fn login(inbox: &MailExternalInbox, password: &str) -> Result<ImapSession, String> {
    let client = connect(inbox)?;
    // The error path carries the SERVER's text (never the password —
    // imap's ValidateError exposes only an offending char).
    client
        .login(&inbox.username, password)
        .map_err(|(e, _client)| imap_err("login", e))
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

/// The single management dispatch on an open session.
fn manage_on_session(session: &mut ImapSession, op: &ManageOp) -> Result<ManageOutcome, ListError> {
    match op {
        ManageOp::FolderList => Ok(ManageOutcome {
            folders: manage_list_names(session)?,
            ..Default::default()
        }),
        ManageOp::FolderCreate { name } => {
            session.create(name).map_err(|e| ListError::Engine(imap_err("CREATE", e)))?;
            Ok(ManageOutcome::default())
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
}
