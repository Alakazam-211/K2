//! `POST /cli/mail/profile` — an Apple setup profile for one hosted
//! mailbox (calendars S6, prd-hostmail-calendars-v1 §4 S6, CAL24, CAL25,
//! CAL38, CAL45, IT11, IT12).
//!
//! Builds an UNSIGNED `.mobileconfig` (XML property list) with:
//! - `com.apple.mail.managed`: IMAP `<mail host>:993` (implicit TLS) and
//!   SMTP submission `<mail host>:465` (implicit TLS), both password auth;
//! - `com.apple.caldav.account` / `com.apple.carddav.account`: `<mail
//!   host>:443` over TLS, principal URL `https://<mail host>/dav/cal` and
//!   `…/dav/card` — only while the owner's DAV policy has calendars on AND
//!   a client can reach CalDAV on this box (`dav::calendar_reach`).
//!
//! The mail host is `mail_hostname_for_apex(<address's domain>)` — the
//! name on the box's mail certificate. Never `*.k2.dev` (that is the
//! control plane, not this box): refused.
//!
//! Credential: every call mints a FRESH app password on the mailbox
//! (description `k2-profile-<YYYY-MM-DD>`). A stored mailbox secret is
//! never read back or reused (IT12). The secret appears only inside the
//! returned profile — never in an error, never logged. The daemon does
//! not log `/cli/*` request or response bodies (same as
//! `POST /cli/mail/app-password`, which also returns its secret once).
//!
//! Owner/admin only: in `is_owner_level_mutation`, NOT the "agents manage
//! hosted mail" toggle surface (it mints a credential for a person's
//! mailbox). POST only.

use serde::Deserialize;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::app_password::AppPasswordEngine;
use crate::mail::dav;
use crate::mail::hosted::{addr_err, err_json};

/// IMAP over implicit TLS (`imap_listeners::IMAPS_IMPLICIT`).
pub const IMAP_PORT: u16 = 993;
/// SMTP submission over implicit TLS (`submissions` listener).
pub const SMTP_PORT: u16 = 465;
/// CalDAV / CardDAV ride Stalwart's HTTPS listener on the mail host.
pub const DAV_PORT: u16 = 443;

/// Stalwart's DAV collection bases (`groupware::DavResourceName::
/// base_path`: `/dav/cal`, `/dav/card`; `/.well-known/caldav` 307s to
/// `/dav/cal`). A PROPFIND on the base answers `current-user-principal`
/// for the signed-in account, so clients discover `/dav/pal/<name>/` and
/// the homes from there — no need to guess the account's principal name.
pub const CALDAV_PATH: &str = "/dav/cal";
pub const CARDDAV_PATH: &str = "/dav/card";

// ── Plist writer ────────────────────────────────────────────────────────

/// The property-list values a profile needs.
#[derive(Debug, Clone)]
enum P {
    S(String),
    I(i64),
    B(bool),
    A(Vec<P>),
    D(Vec<(&'static str, P)>),
}

fn s(v: impl Into<String>) -> P {
    P::S(v.into())
}

/// XML text escaping for element content.
pub fn xml_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most C0 controls even escaped; drop them.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn write_value(out: &mut String, v: &P, depth: usize) {
    let pad = "\t".repeat(depth);
    match v {
        P::S(t) => {
            out.push_str(&pad);
            out.push_str("<string>");
            out.push_str(&xml_escape(t));
            out.push_str("</string>\n");
        }
        P::I(n) => out.push_str(&format!("{pad}<integer>{n}</integer>\n")),
        P::B(true) => out.push_str(&format!("{pad}<true/>\n")),
        P::B(false) => out.push_str(&format!("{pad}<false/>\n")),
        P::A(items) => {
            out.push_str(&format!("{pad}<array>\n"));
            for it in items {
                write_value(out, it, depth + 1);
            }
            out.push_str(&format!("{pad}</array>\n"));
        }
        P::D(entries) => {
            out.push_str(&format!("{pad}<dict>\n"));
            for (k, it) in entries {
                out.push_str(&format!("{pad}\t<key>{}</key>\n", xml_escape(k)));
                write_value(out, it, depth + 1);
            }
            out.push_str(&format!("{pad}</dict>\n"));
        }
    }
}

fn plist_document(root: &P) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n",
    );
    write_value(&mut out, root, 0);
    out.push_str("</plist>\n");
    out
}

// ── Profile ─────────────────────────────────────────────────────────────

/// Everything the profile is built from. `password` is the freshly
/// minted app-password secret.
pub struct ProfileSpec<'a> {
    pub address: &'a str,
    pub mail_host: &'a str,
    pub password: &'a str,
    pub include_dav: bool,
    /// The app password's description (shown in the profile text so the
    /// person can match it to `k2 hostmail app-password list`).
    pub label: &'a str,
}

/// `example.com` → `com.example`; characters outside `[A-Za-z0-9-]` in a
/// label become `-`.
fn reverse_dns(domain: &str) -> String {
    domain
        .trim_end_matches('.')
        .split('.')
        .filter(|l| !l.is_empty())
        .rev()
        .map(ident_label)
        .collect::<Vec<_>>()
        .join(".")
}

fn ident_label(raw: &str) -> String {
    let out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() {
        "x".to_string()
    } else {
        out
    }
}

/// Top-level `PayloadIdentifier`: `<reverse mail domain>.k2mail.<local>`.
/// Stable per address, so installing a newer profile for the same
/// mailbox replaces the old one on the device.
pub fn profile_identifier(address: &str) -> String {
    let (local, domain) = address.split_once('@').unwrap_or((address, ""));
    let rev = reverse_dns(domain);
    if rev.is_empty() {
        format!("k2mail.{}", ident_label(local))
    } else {
        format!("{rev}.k2mail.{}", ident_label(local))
    }
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string().to_ascii_uppercase()
}

/// Which payloads a profile carries (`mail` always).
pub fn includes(include_dav: bool) -> Vec<&'static str> {
    if include_dav {
        vec!["mail", "caldav", "carddav"]
    } else {
        vec!["mail"]
    }
}

/// The unsigned `.mobileconfig` XML.
pub fn build_mobileconfig(spec: &ProfileSpec<'_>) -> String {
    let display = format!("{} (K2 Mail)", spec.address);
    let id = profile_identifier(spec.address);
    let host = spec.mail_host.trim().trim_end_matches('.');

    let mail = P::D(vec![
        ("PayloadType", s("com.apple.mail.managed")),
        ("PayloadVersion", P::I(1)),
        ("PayloadIdentifier", s(format!("{id}.mail"))),
        ("PayloadUUID", s(new_uuid())),
        ("PayloadDisplayName", s(format!("{} — Mail", spec.address))),
        ("EmailAccountDescription", s(display.clone())),
        ("EmailAccountName", s(spec.address)),
        ("EmailAccountType", s("EmailTypeIMAP")),
        ("EmailAddress", s(spec.address)),
        ("IncomingMailServerAuthentication", s("EmailAuthPassword")),
        ("IncomingMailServerHostName", s(host)),
        ("IncomingMailServerPortNumber", P::I(i64::from(IMAP_PORT))),
        ("IncomingMailServerUseSSL", P::B(true)),
        ("IncomingMailServerUsername", s(spec.address)),
        ("IncomingPassword", s(spec.password)),
        ("OutgoingMailServerAuthentication", s("EmailAuthPassword")),
        ("OutgoingMailServerHostName", s(host)),
        ("OutgoingMailServerPortNumber", P::I(i64::from(SMTP_PORT))),
        ("OutgoingMailServerUseSSL", P::B(true)),
        ("OutgoingMailServerUsername", s(spec.address)),
        ("OutgoingPassword", s(spec.password)),
    ]);
    let mut content = vec![mail];
    if spec.include_dav {
        content.push(P::D(vec![
            ("PayloadType", s("com.apple.caldav.account")),
            ("PayloadVersion", P::I(1)),
            ("PayloadIdentifier", s(format!("{id}.caldav"))),
            ("PayloadUUID", s(new_uuid())),
            ("PayloadDisplayName", s(format!("{} — Calendars", spec.address))),
            ("CalDAVAccountDescription", s(display.clone())),
            ("CalDAVHostName", s(host)),
            ("CalDAVPort", P::I(i64::from(DAV_PORT))),
            ("CalDAVUseSSL", P::B(true)),
            ("CalDAVPrincipalURL", s(format!("https://{host}{CALDAV_PATH}"))),
            ("CalDAVUsername", s(spec.address)),
            ("CalDAVPassword", s(spec.password)),
        ]));
        content.push(P::D(vec![
            ("PayloadType", s("com.apple.carddav.account")),
            ("PayloadVersion", P::I(1)),
            ("PayloadIdentifier", s(format!("{id}.carddav"))),
            ("PayloadUUID", s(new_uuid())),
            ("PayloadDisplayName", s(format!("{} — Contacts", spec.address))),
            ("CardDAVAccountDescription", s(display.clone())),
            ("CardDAVHostName", s(host)),
            ("CardDAVPort", P::I(i64::from(DAV_PORT))),
            ("CardDAVUseSSL", P::B(true)),
            ("CardDAVPrincipalURL", s(format!("https://{host}{CARDDAV_PATH}"))),
            ("CardDAVUsername", s(spec.address)),
            ("CardDAVPassword", s(spec.password)),
        ]));
    }
    let what = if spec.include_dav {
        "Mail, Calendars and Contacts"
    } else {
        "Mail"
    };
    let root = P::D(vec![
        ("PayloadType", s("Configuration")),
        ("PayloadVersion", P::I(1)),
        ("PayloadIdentifier", s(id)),
        ("PayloadUUID", s(new_uuid())),
        ("PayloadDisplayName", s(display)),
        (
            "PayloadDescription",
            s(format!(
                "{what} for {} on {host}. Signs in with the app password \"{}\" \
                 (revoke it with k2 hostmail app-password revoke). Unsigned profile made by K2.",
                spec.address, spec.label
            )),
        ),
        ("PayloadRemovalDisallowed", P::B(false)),
        ("PayloadContent", P::A(content)),
    ]);
    plist_document(&root)
}

// ── Gates (pure, so the refusals test without a box) ────────────────────

fn is_k2_dev(host: &str) -> bool {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    lower == "k2.dev" || lower.ends_with(".k2.dev")
}

/// The profile's server name: the apex's mail host (the name on the mail
/// certificate). Missing or `*.k2.dev` → refused.
pub fn pick_mail_host(apex: &str, host: Option<String>) -> Result<String, CliResponse> {
    let Some(host) = host
        .map(|h| h.trim().trim_end_matches('.').to_string())
        .filter(|h| !h.is_empty())
    else {
        return Err(err_json(
            "409 Conflict",
            "not_ready",
            format!(
                "no mail hostname for {apex} — set one first (k2 hostmail enable --hostname \
                 mail.{apex}, or attach it with k2 domain)"
            ),
        ));
    };
    if is_k2_dev(&host) {
        return Err(err_json(
            "409 Conflict",
            "not_ready",
            format!(
                "the mail host for {apex} is {host}, a k2.dev name that points at the K2 \
                 control plane, not this box — Mail, Calendar and Contacts can't sign in \
                 there. Put a mail hostname on your own domain (e.g. mail.{apex}) with a \
                 certificate first (k2 hostmail ptr set / k2 domain), then make the profile"
            ),
        ));
    }
    Ok(host)
}

/// Hosted mail must be running (`running` or `degraded`, the same states
/// every Stalwart call accepts).
pub fn require_running(status: Option<&str>) -> Result<(), CliResponse> {
    match status {
        None => Err(err_json(
            "503 Service Unavailable",
            "not_ready",
            "hosted mail is not installed — enable it first (k2 hostmail enable)".to_string(),
        )),
        Some("running") | Some("degraded") => Ok(()),
        Some(other) => Err(err_json(
            "503 Service Unavailable",
            "not_ready",
            format!("hosted mail is {other} — start it (k2 hostmail enable) before making a profile"),
        )),
    }
}

/// Whether the CalDAV/CardDAV payloads go in, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavInclusion {
    pub include: bool,
    pub reason: Option<String>,
}

/// Pure: calendars on (policy) AND reachable (`dav::calendar_reach`).
pub fn dav_inclusion(
    policy: &Result<Option<dav::DavPolicy>, String>,
    port_plan: Option<&str>,
    host: &str,
    caddy_mail_site: bool,
) -> DavInclusion {
    let enabled = match policy {
        Ok(None) => true,
        Ok(Some(p)) => p.calendars == dav::OnOff::On,
        Err(e) => {
            return DavInclusion {
                include: false,
                reason: Some(format!("the calendar policy is unreadable ({e})")),
            }
        }
    };
    if !enabled {
        return DavInclusion {
            include: false,
            reason: Some(
                "calendars are off for hosted addresses (k2 hostmail calendar enable)".to_string(),
            ),
        };
    }
    let (reachable, reason) = dav::calendar_reach(true, port_plan, Some(host), caddy_mail_site);
    if !reachable {
        return DavInclusion {
            include: false,
            reason: Some(format!(
                "calendar clients can't reach CalDAV on this box yet: {}",
                reason.unwrap_or_else(|| "not reachable".to_string())
            )),
        };
    }
    DavInclusion {
        include: true,
        reason: None,
    }
}

/// `k2-profile-<YYYY-MM-DD>` (UTC).
pub fn profile_label(today: &str) -> String {
    format!("k2-profile-{today}")
}

/// Mint + build. The secret goes into the profile and nowhere else.
pub fn profile_on(
    engine: &dyn AppPasswordEngine,
    address: &str,
    account_id: &str,
    mail_host: &str,
    dav: &DavInclusion,
    today: &str,
) -> CliResponse {
    let label = profile_label(today);
    // Permissions: `Inherit` (StalwartClient::app_password_create) — the
    // app password can do everything the mailbox password can.
    // TODO(S0.6): scope it with `Replace` to IMAP + SMTP + davCal*/
    // davCard*/davPrincipal*/davSyncCollection once the S0.6 Apple smoke
    // test proves such a password still signs in. Not before: a profile
    // whose password can't log in makes the device retry and get the
    // person's network IP auth-banned (IT11, CAL38).
    let created = match engine.create(account_id, &label) {
        Ok(c) => c,
        // The engine error is Stalwart's SetError text — no secret exists
        // yet, and our request args are never echoed.
        Err(e) => return err_json("502 Bad Gateway", "engine", e),
    };
    let xml = build_mobileconfig(&ProfileSpec {
        address,
        mail_host,
        password: &created.secret,
        include_dav: dav.include,
        label: &label,
    });
    let mut notes: Vec<String> = Vec::new();
    if let Some(r) = &dav.reason {
        notes.push(format!("mail only — {r}"));
    }
    notes.push(
        "a device signing in with a stale or wrong password gets its network's IP banned — \
         see k2 hostmail bans list / k2 hostmail allowlist"
            .to_string(),
    );
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "address": address,
            "format": "apple",
            "mailHost": mail_host,
            "includes": includes(dav.include),
            "appPasswordId": created.id,
            "description": label,
            "filename": format!("{address}.mobileconfig"),
            "signed": false,
            "profile": xml,
            "notes": notes,
            "revoke": format!("k2 hostmail app-password revoke {address} {}", created.id),
        })
        .to_string(),
    )
}

// ── Route ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProfileBody {
    address: String,
    apple: bool,
}

fn linked_inbox_exists(address: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT 1 FROM mail_external_inboxes WHERE LOWER(email_address) = LOWER(?1)",
        rusqlite::params![address],
        |_| Ok(()),
    )
    .is_ok()
}

/// The hosted row for `raw`, refusing linked, retired, unknown and
/// unminted addresses. Pure DB — no Stalwart call.
pub(crate) fn resolve_hosted(raw: &str) -> Result<(String, String), CliResponse> {
    let address = addresses::normalize_address(raw).map_err(addr_err)?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    };
    let Some(row) = row else {
        if linked_inbox_exists(&address) {
            return Err(err_json(
                "400 Bad Request",
                "usage",
                format!(
                    "'{address}' is a linked inbox (another provider's account), not hosted \
                     mail on this box — set it up with that provider's own instructions"
                ),
            ));
        }
        return Err(addr_err(AddrError::NotFound(format!(
            "no hosted address '{address}'"
        ))));
    };
    if row.status != "active" {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("'{address}' is retired — a profile needs an active hosted mailbox"),
        ));
    }
    let account_id = row
        .stalwart_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("address '{address}' has no mail-server account"),
            )
        })?;
    Ok((row.address, account_id))
}

fn server_state() -> Option<(String, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT status, port_plan FROM mail_server WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// `POST /cli/mail/profile` `{address, apple: true}`.
pub fn handle_profile(body: &[u8]) -> CliResponse {
    let b: ProfileBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    if b.address.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — which hosted mailbox?".to_string(),
        );
    }
    if !b.apple {
        return err_json(
            "400 Bad Request",
            "usage",
            "pass apple:true (--apple) — the Apple .mobileconfig is the only profile format".to_string(),
        );
    }
    let (address, account_id) = match resolve_hosted(&b.address) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let state = server_state();
    if let Err(r) = require_running(state.as_ref().map(|(s, _)| s.as_str())) {
        return r;
    }
    let port_plan = state.and_then(|(_, p)| p);
    let apex = address.split_once('@').map(|(_, d)| d).unwrap_or("");
    let host = match pick_mail_host(apex, crate::mail::autoconfig::mail_hostname_for_apex(apex)) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let caddy = port_plan.as_deref() != Some("tls-alpn") && k2_core::skin_door::mail_site_live(&host);
    let dav = dav_inclusion(&dav::load_policy(), port_plan.as_deref(), &host, caddy);
    let (engine, _) = match crate::mail::domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return err_json("503 Service Unavailable", "not_ready", hint),
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    profile_on(&engine, &address, &account_id, &host, &dav, &today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::jmap::{AppPasswordInfo, CreatedAppPassword};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ── A strict little plist reader (parse-back check) ──────────────────

    #[derive(Debug, Clone, PartialEq)]
    enum V {
        S(String),
        I(i64),
        B(bool),
        A(Vec<V>),
        D(Vec<(String, V)>),
    }

    impl V {
        fn get(&self, k: &str) -> &V {
            match self {
                V::D(e) => {
                    let hits: Vec<&V> = e.iter().filter(|(n, _)| n == k).map(|(_, v)| v).collect();
                    assert_eq!(hits.len(), 1, "key {k} must appear exactly once");
                    hits[0]
                }
                other => panic!("not a dict: {other:?}"),
            }
        }
        fn keys(&self) -> Vec<String> {
            match self {
                V::D(e) => e.iter().map(|(k, _)| k.clone()).collect(),
                other => panic!("not a dict: {other:?}"),
            }
        }
        fn str(&self) -> &str {
            match self {
                V::S(s) => s,
                other => panic!("not a string: {other:?}"),
            }
        }
        fn int(&self) -> i64 {
            match self {
                V::I(n) => *n,
                other => panic!("not an integer: {other:?}"),
            }
        }
        fn boolean(&self) -> bool {
            match self {
                V::B(b) => *b,
                other => panic!("not a bool: {other:?}"),
            }
        }
        fn arr(&self) -> &Vec<V> {
            match self {
                V::A(a) => a,
                other => panic!("not an array: {other:?}"),
            }
        }
    }

    fn unescape(t: &str) -> String {
        assert!(
            !t.contains('<') && !t.contains('>'),
            "raw markup inside text: {t}"
        );
        let mut out = String::new();
        let mut rest = t;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            let end = tail.find(';').expect("unterminated entity");
            out.push(match &tail[..=end] {
                "&amp;" => '&',
                "&lt;" => '<',
                "&gt;" => '>',
                "&quot;" => '"',
                "&apos;" => '\'',
                other => panic!("unknown entity {other}"),
            });
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        out
    }

    struct Reader<'a> {
        s: &'a str,
    }

    impl<'a> Reader<'a> {
        fn skip_ws(&mut self) {
            self.s = self.s.trim_start();
        }
        fn tag(&mut self) -> String {
            self.skip_ws();
            assert!(self.s.starts_with('<'), "expected a tag at: {:.40}", self.s);
            let end = self.s.find('>').expect("unterminated tag");
            let t = self.s[1..end].to_string();
            self.s = &self.s[end + 1..];
            t
        }
        fn text_until(&mut self, close: &str) -> String {
            let end = self.s.find(close).unwrap_or_else(|| panic!("missing {close}"));
            let t = unescape(&self.s[..end]);
            self.s = &self.s[end + close.len()..];
            t
        }
        fn value(&mut self) -> V {
            let t = self.tag();
            match t.as_str() {
                "string" => V::S(self.text_until("</string>")),
                "integer" => V::I(self.text_until("</integer>").parse().expect("integer")),
                "true/" => V::B(true),
                "false/" => V::B(false),
                "array" => {
                    let mut items = Vec::new();
                    loop {
                        self.skip_ws();
                        if self.s.starts_with("</array>") {
                            self.s = &self.s["</array>".len()..];
                            break;
                        }
                        items.push(self.value());
                    }
                    V::A(items)
                }
                "dict" => {
                    let mut entries = Vec::new();
                    loop {
                        self.skip_ws();
                        if self.s.starts_with("</dict>") {
                            self.s = &self.s["</dict>".len()..];
                            break;
                        }
                        assert_eq!(self.tag(), "key", "dict entries start with <key>");
                        let k = self.text_until("</key>");
                        entries.push((k, self.value()));
                    }
                    V::D(entries)
                }
                other => panic!("unexpected element <{other}>"),
            }
        }
    }

    fn parse_plist(xml: &str) -> V {
        let prolog = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \
                      \"-//Apple//DTD PLIST 1.0//EN\" \
                      \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";
        let body = xml.strip_prefix(prolog).expect("XML prolog + plist DOCTYPE");
        let mut r = Reader { s: body };
        assert_eq!(r.tag(), "plist version=\"1.0\"");
        let v = r.value();
        assert_eq!(r.tag(), "/plist");
        r.skip_ws();
        assert!(r.s.is_empty(), "trailing content after </plist>: {:?}", r.s);
        v
    }

    // ── Fake engine ──────────────────────────────────────────────────────

    struct FakeEngine {
        calls: Mutex<Vec<(String, String)>>,
        fail: Option<String>,
    }

    impl FakeEngine {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail: None,
            }
        }
    }

    const SECRET: &str = "app_Fixture<&>\"'Secret-9";

    impl AppPasswordEngine for FakeEngine {
        fn create(&self, account_id: &str, description: &str) -> Result<CreatedAppPassword, String> {
            self.calls
                .lock()
                .unwrap()
                .push((account_id.to_string(), description.to_string()));
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            Ok(CreatedAppPassword {
                id: "ap-7".to_string(),
                secret: SECRET.to_string(),
            })
        }
        fn list(&self, _: &str) -> Result<Vec<AppPasswordInfo>, String> {
            panic!("a profile never lists app passwords")
        }
        fn query_ids(&self, _: &str) -> Result<Vec<String>, String> {
            panic!("a profile never queries app passwords")
        }
        fn destroy(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("a profile never destroys app passwords")
        }
    }

    const ADDR: &str = "alice@example.com";
    const HOST: &str = "mail.example.com";

    fn dav_on() -> DavInclusion {
        DavInclusion {
            include: true,
            reason: None,
        }
    }

    fn make(dav: &DavInclusion) -> (serde_json::Value, V, FakeEngine) {
        let engine = FakeEngine::new();
        let r = profile_on(&engine, ADDR, "acc-1", HOST, dav, "2026-10-06");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let plist = parse_plist(v["profile"].as_str().unwrap());
        (v, plist, engine)
    }

    fn by_type<'a>(root: &'a V, t: &str) -> Vec<&'a V> {
        root.get("PayloadContent")
            .arr()
            .iter()
            .filter(|p| p.get("PayloadType").str() == t)
            .collect()
    }

    /// Collect every string value in the tree.
    fn strings(v: &V, out: &mut Vec<(String, String)>, key: &str) {
        match v {
            V::S(s) => out.push((key.to_string(), s.clone())),
            V::A(a) => a.iter().for_each(|x| strings(x, out, key)),
            V::D(e) => e.iter().for_each(|(k, x)| strings(x, out, k)),
            _ => {}
        }
    }

    #[test]
    fn plist_parses_back_with_mail_caldav_carddav() {
        let (v, root, engine) = make(&dav_on());
        assert_eq!(root.get("PayloadType").str(), "Configuration");
        assert_eq!(root.get("PayloadVersion").int(), 1);
        assert_eq!(root.get("PayloadDisplayName").str(), "alice@example.com (K2 Mail)");
        assert_eq!(root.get("PayloadIdentifier").str(), "com.example.k2mail.alice");
        assert!(!root.get("PayloadRemovalDisallowed").boolean());
        assert_eq!(root.get("PayloadContent").arr().len(), 3);

        let mail = by_type(&root, "com.apple.mail.managed");
        assert_eq!(mail.len(), 1);
        let m = mail[0];
        assert_eq!(m.get("PayloadIdentifier").str(), "com.example.k2mail.alice.mail");
        assert_eq!(m.get("EmailAccountType").str(), "EmailTypeIMAP");
        assert_eq!(m.get("EmailAddress").str(), ADDR);
        assert_eq!(m.get("IncomingMailServerHostName").str(), HOST);
        assert_eq!(m.get("IncomingMailServerPortNumber").int(), 993);
        assert!(m.get("IncomingMailServerUseSSL").boolean());
        assert_eq!(m.get("IncomingMailServerAuthentication").str(), "EmailAuthPassword");
        assert_eq!(m.get("IncomingMailServerUsername").str(), ADDR);
        assert_eq!(m.get("IncomingPassword").str(), SECRET);
        assert_eq!(m.get("OutgoingMailServerHostName").str(), HOST);
        assert_eq!(m.get("OutgoingMailServerPortNumber").int(), 465);
        assert!(m.get("OutgoingMailServerUseSSL").boolean());
        assert_eq!(m.get("OutgoingMailServerAuthentication").str(), "EmailAuthPassword");
        assert_eq!(m.get("OutgoingMailServerUsername").str(), ADDR);
        assert_eq!(m.get("OutgoingPassword").str(), SECRET);

        let cal = by_type(&root, "com.apple.caldav.account");
        assert_eq!(cal.len(), 1);
        let c = cal[0];
        assert_eq!(c.get("CalDAVHostName").str(), HOST);
        assert_eq!(c.get("CalDAVPort").int(), 443);
        assert!(c.get("CalDAVUseSSL").boolean());
        assert_eq!(c.get("CalDAVPrincipalURL").str(), "https://mail.example.com/dav/cal");
        assert_eq!(c.get("CalDAVUsername").str(), ADDR);
        assert_eq!(c.get("CalDAVPassword").str(), SECRET);

        let card = by_type(&root, "com.apple.carddav.account");
        assert_eq!(card.len(), 1);
        let k = card[0];
        assert_eq!(k.get("CardDAVHostName").str(), HOST);
        assert_eq!(k.get("CardDAVPort").int(), 443);
        assert!(k.get("CardDAVUseSSL").boolean());
        assert_eq!(k.get("CardDAVPrincipalURL").str(), "https://mail.example.com/dav/card");
        assert_eq!(k.get("CardDAVUsername").str(), ADDR);
        assert_eq!(k.get("CardDAVPassword").str(), SECRET);

        // Every payload: its own upper-case UUID, all distinct.
        let mut uuids = vec![root.get("PayloadUUID").str().to_string()];
        for p in root.get("PayloadContent").arr() {
            assert_eq!(p.get("PayloadVersion").int(), 1);
            uuids.push(p.get("PayloadUUID").str().to_string());
        }
        for u in &uuids {
            let parsed = uuid::Uuid::parse_str(u).expect("PayloadUUID is a UUID");
            assert_eq!(parsed.get_version_num(), 4);
            assert_eq!(u, &u.to_ascii_uppercase());
        }
        let mut dedup = uuids.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), uuids.len(), "PayloadUUIDs must be unique");

        assert_eq!(v["includes"], serde_json::json!(["mail", "caldav", "carddav"]));
        assert_eq!(v["mailHost"], HOST);
        assert_eq!(v["appPasswordId"], "ap-7");
        assert_eq!(v["description"], "k2-profile-2026-10-06");
        assert_eq!(v["signed"], false);
        assert_eq!(
            v["revoke"],
            "k2 hostmail app-password revoke alice@example.com ap-7"
        );
        assert_eq!(engine.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn exactly_one_fresh_app_password_minted_with_dated_description() {
        let (v, root, engine) = make(&dav_on());
        let calls = engine.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![("acc-1".to_string(), "k2-profile-2026-10-06".to_string())],
            "one mint, on the mailbox account, dated description"
        );
        // Every password field carries that one secret; nothing else does.
        let mut all = Vec::new();
        strings(&root, &mut all, "");
        let pw_keys = [
            "IncomingPassword",
            "OutgoingPassword",
            "CalDAVPassword",
            "CardDAVPassword",
        ];
        let carrying: Vec<&String> = all
            .iter()
            .filter(|(_, val)| val.contains(SECRET))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(carrying.len(), 4, "{carrying:?}");
        for k in carrying {
            assert!(pw_keys.contains(&k.as_str()), "secret leaked into {k}");
        }
        for (k, val) in &all {
            if pw_keys.contains(&k.as_str()) {
                assert_eq!(val, SECRET, "{k} must be the fresh app password");
            }
        }
        // The secret is only inside the profile: strip it, nothing left.
        let mut outside = v.clone();
        outside["profile"] = serde_json::json!("");
        assert!(!outside.to_string().contains("Fixture"), "{outside}");
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        let (v, _, _) = make(&dav_on());
        let xml = v["profile"].as_str().unwrap();
        assert!(!xml.contains(SECRET), "raw secret with <&>\"' must be escaped");
        assert!(xml.contains("app_Fixture&lt;&amp;&gt;&quot;&apos;Secret-9"));
        assert_eq!(xml_escape("a<b>&\"c'\u{1}"), "a&lt;b&gt;&amp;&quot;c&apos;");
    }

    #[test]
    fn mail_only_when_dav_not_included() {
        let dav = DavInclusion {
            include: false,
            reason: Some("calendars are off for hosted addresses".to_string()),
        };
        let (v, root, _) = make(&dav);
        assert_eq!(root.get("PayloadContent").arr().len(), 1);
        assert!(by_type(&root, "com.apple.caldav.account").is_empty());
        assert!(by_type(&root, "com.apple.carddav.account").is_empty());
        assert_eq!(v["includes"], serde_json::json!(["mail"]));
        let notes = v["notes"].to_string();
        assert!(notes.contains("mail only"), "{notes}");
        assert!(notes.contains("calendars are off"), "{notes}");
        assert!(notes.contains("k2 hostmail bans"), "{notes}");
        // The secret appears in the two mail fields only.
        assert_eq!(v["profile"].as_str().unwrap().matches("app_Fixture").count(), 2);
    }

    #[test]
    fn dav_included_only_when_enabled_and_reachable() {
        // Never applied (Stalwart default, on) + tls-alpn → included.
        let d = dav_inclusion(&Ok(None), Some("tls-alpn"), HOST, false);
        assert!(d.include, "{d:?}");
        // Caddy fronts the mail host → included.
        let d = dav_inclusion(&Ok(None), Some("http-01"), HOST, true);
        assert!(d.include, "{d:?}");
        // A foreign :443 → not reachable → mail only, reason names the paths.
        let d = dav_inclusion(&Ok(None), Some("http-01"), HOST, false);
        assert!(!d.include);
        assert!(d.reason.unwrap().contains("/.well-known/caldav"));
        // Owner turned calendars off → mail only even when reachable.
        let off = dav::parse_policy_json(
            r#"{"calendars":"off","files":"off","appliedAt":1}"#,
        )
        .expect("policy fixture");
        let d = dav_inclusion(&Ok(Some(off)), Some("tls-alpn"), HOST, false);
        assert!(!d.include);
        assert!(d.reason.unwrap().contains("calendar enable"));
        let on = dav::parse_policy_json(r#"{"calendars":"on","files":"off","appliedAt":1}"#)
            .expect("policy fixture");
        assert!(dav_inclusion(&Ok(Some(on)), Some("tls-alpn"), HOST, false).include);
        // Unreadable policy → not included.
        let d = dav_inclusion(&Err("bad json".to_string()), Some("tls-alpn"), HOST, false);
        assert!(!d.include);
        assert!(d.reason.unwrap().contains("unreadable"));
    }

    #[test]
    fn k2_dev_mail_host_is_refused() {
        for h in ["mail.box.k2.dev", "K2.DEV", "box.k2.dev."] {
            let r = pick_mail_host("example.com", Some(h.to_string())).unwrap_err();
            assert_eq!(r.status, "409 Conflict", "{h}: {}", r.body);
            assert!(r.body.contains("k2.dev"), "{}", r.body);
            assert!(r.body.contains("not_ready"), "{}", r.body);
        }
        let r = pick_mail_host("example.com", None).unwrap_err();
        assert_eq!(r.status, "409 Conflict");
        assert!(r.body.contains("no mail hostname"), "{}", r.body);
        match pick_mail_host("example.com", Some("mail.example.com.".to_string())) {
            Ok(h) => assert_eq!(h, "mail.example.com"),
            Err(r) => panic!("own-domain mail host refused: {}", r.body),
        }
    }

    #[test]
    fn not_running_is_refused() {
        let r = require_running(None).unwrap_err();
        assert_eq!(r.status, "503 Service Unavailable");
        assert!(r.body.contains("not installed"), "{}", r.body);
        for st in ["stopped", "error", "provisioning"] {
            let r = require_running(Some(st)).unwrap_err();
            assert_eq!(r.status, "503 Service Unavailable");
            assert!(r.body.contains(st), "{}", r.body);
        }
        assert!(require_running(Some("running")).is_ok());
        assert!(require_running(Some("degraded")).is_ok());
    }

    #[test]
    fn engine_failure_is_502_and_carries_no_secret() {
        let mut engine = FakeEngine::new();
        engine.fail = Some("x:AppPassword/set: forbidden".to_string());
        let r = profile_on(&engine, ADDR, "acc-1", HOST, &dav_on(), "2026-10-06");
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        assert!(!r.body.contains("Fixture"));
        assert!(!r.body.contains("profile\""), "{}", r.body);
    }

    /// End to end over the real Stalwart client (loopback mock): the wire
    /// create is `Inherit` with the dated description on the mailbox
    /// account, and the minted secret lands in the profile.
    #[test]
    fn real_client_mints_inherit_app_password_with_dated_description() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server};
        let session = r#"{"apiUrl": "/jmap/", "accounts": {"svc": {}}, "primaryAccounts": {"urn:stalwart:jmap": "svc"}}"#;
        let created = serde_json::json!({
            "methodResponses": [["x:AppPassword/set", {
                "accountId": "mbox-1",
                "created": { "k2": { "id": "ap9", "secret": "app_wire-fixture" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session.to_string(), created]);
        let client = crate::mail::jmap::StalwartClient::new(
            format!("http://127.0.0.1:{port}"),
            "k2-test-key",
        );
        let r = profile_on(&client, ADDR, "mbox-1", HOST, &dav_on(), "2026-10-06");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let _session = rx.recv().expect("session");
        let req = body_json(&rx.recv().expect("set"));
        assert_eq!(req["methodCalls"][0][0], "x:AppPassword/set");
        let args = &req["methodCalls"][0][1];
        assert_eq!(args["accountId"], "mbox-1");
        let create = &args["create"]["k2"];
        assert_eq!(create["description"], "k2-profile-2026-10-06");
        assert_eq!(create["permissions"]["@type"], "Inherit");
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["appPasswordId"], "ap9");
        let root = parse_plist(v["profile"].as_str().unwrap());
        let m = by_type(&root, "com.apple.mail.managed")[0];
        assert_eq!(m.get("IncomingPassword").str(), "app_wire-fixture");
    }

    #[test]
    fn identifier_is_reverse_dns_of_the_mail_domain() {
        assert_eq!(profile_identifier("alice@example.com"), "com.example.k2mail.alice");
        assert_eq!(
            profile_identifier("first.last+x@sub.example.co.uk"),
            "uk.co.example.sub.k2mail.first-last-x"
        );
        assert_eq!(profile_label("2026-10-06"), "k2-profile-2026-10-06");
    }

    // ── Route-level refusals (DB fixtures; no Stalwart) ──────────────────

    fn unique(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4().simple())
    }

    fn seed_domain(domain: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, created_at) \
             VALUES (?1, ?2, 'stw-d', 'pending', 100)",
            rusqlite::params![id, domain],
        )
        .expect("seed domain");
        id
    }

    fn seed_address(address: &str, domain_id: &str, account: Option<&str>, status: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, ?3, ?4, 'proj', ?5, 100)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                address,
                domain_id,
                account,
                status
            ],
        )
        .expect("seed address");
    }

    fn seed_linked(address: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at) VALUES (?1, 'proj', ?2, 'imap.example.net', 993, ?2, 100)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), address],
        )
        .expect("seed linked inbox");
    }

    fn cleanup(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM mail_addresses WHERE domain_id IN \
             (SELECT id FROM mail_domains WHERE domain = ?1)",
            rusqlite::params![domain],
        )
        .expect("cleanup addresses");
        conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![domain])
            .expect("cleanup domain");
        conn.execute(
            "DELETE FROM mail_external_inboxes WHERE email_address LIKE ?1",
            rusqlite::params![format!("%@{domain}")],
        )
        .expect("cleanup linked");
    }

    fn post(body: serde_json::Value) -> CliResponse {
        handle_profile(body.to_string().as_bytes())
    }

    #[test]
    fn refuses_linked_retired_unknown_and_unminted() {
        let domain = unique("prof") + ".example.com";
        let domain_id = seed_domain(&domain);

        let linked = format!("linked@{domain}");
        seed_linked(&linked);
        let r = post(serde_json::json!({ "address": linked, "apple": true }));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("linked inbox"), "{}", r.body);

        let retired = format!("gone@{domain}");
        seed_address(&retired, &domain_id, Some("acc-gone"), "retired");
        let r = post(serde_json::json!({ "address": retired, "apple": true }));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(r.body.contains("retired"), "{}", r.body);

        let r = post(serde_json::json!({ "address": format!("ghost@{domain}"), "apple": true }));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(r.body.contains("no hosted address"), "{}", r.body);

        let unminted = format!("new@{domain}");
        seed_address(&unminted, &domain_id, None, "active");
        let r = post(serde_json::json!({ "address": unminted, "apple": true }));
        assert_eq!(r.status, "409 Conflict", "{}", r.body);

        // An active, minted address passes the address gate.
        let ok = format!("bob@{domain}");
        seed_address(&ok, &domain_id, Some("acc-bob"), "active");
        let (a, acc) = match resolve_hosted(&ok.to_ascii_uppercase()) {
            Ok(v) => v,
            Err(r) => panic!("active hosted address refused: {}", r.body),
        };
        assert_eq!(a, ok);
        assert_eq!(acc, "acc-bob");
        cleanup(&domain);
    }

    #[test]
    fn usage_errors() {
        let r = handle_profile(b"not json");
        assert_eq!(r.status, "400 Bad Request");
        let r = post(serde_json::json!({ "apple": true }));
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
        let r = post(serde_json::json!({ "address": "a@example.com" }));
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("--apple"), "{}", r.body);
        let r = post(serde_json::json!({ "address": "no-at-sign", "apple": true }));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn json_keys_stable() {
        let (v, _, _) = make(&dav_on());
        let keys: BTreeMap<String, ()> = v
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), ()))
            .collect();
        for k in [
            "ok",
            "address",
            "format",
            "mailHost",
            "includes",
            "appPasswordId",
            "description",
            "filename",
            "signed",
            "profile",
            "notes",
            "revoke",
        ] {
            assert!(keys.contains_key(k), "missing {k}");
        }
        assert!(!keys.contains_key("secret"), "no top-level secret field");
        // The plist keys of the mail payload, exactly (Apple's names).
        let (_, root, _) = make(&dav_on());
        let m = by_type(&root, "com.apple.mail.managed")[0];
        let mut got = m.keys();
        got.sort();
        let mut want = vec![
            "PayloadType",
            "PayloadVersion",
            "PayloadIdentifier",
            "PayloadUUID",
            "PayloadDisplayName",
            "EmailAccountDescription",
            "EmailAccountName",
            "EmailAccountType",
            "EmailAddress",
            "IncomingMailServerAuthentication",
            "IncomingMailServerHostName",
            "IncomingMailServerPortNumber",
            "IncomingMailServerUseSSL",
            "IncomingMailServerUsername",
            "IncomingPassword",
            "OutgoingMailServerAuthentication",
            "OutgoingMailServerHostName",
            "OutgoingMailServerPortNumber",
            "OutgoingMailServerUseSSL",
            "OutgoingMailServerUsername",
            "OutgoingPassword",
        ];
        want.sort();
        assert_eq!(got, want);
    }
}
