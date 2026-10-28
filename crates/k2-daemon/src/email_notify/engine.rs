//! The seams Email Notifications talks to the world through: the local
//! Stalwart (hosted senders), an SMTP relay, the vault (relay passwords)
//! and DNS (DMARC). Production uses the real clients; tests install fakes
//! for their own thread ([`set_test_engine`] and friends), so no test ever
//! reaches a mail server, a relay, a vault file or a resolver.

use std::sync::Arc;

use crate::mail::dns_verify::DnsResolver;
use crate::mail::jmap::StalwartClient;
use k2_core::email_notify::Relay;

use super::{fail, Fail};

/// One hosted submission (E8): what the JMAP request carries.
#[derive(Debug, Clone)]
pub struct HostedMail<'a> {
    pub account_id: &'a str,
    /// The identity and envelope MAIL FROM (the sender address, E9).
    pub from: &'a str,
    /// Every envelope recipient (to + cc + bcc).
    pub rcpt: &'a [String],
    /// The RFC 8621 Email object (no `mailboxIds`).
    pub email: serde_json::Value,
    /// `mailFrom.parameters` (`RET`, `ENVID`).
    pub params: serde_json::Value,
    /// `onSuccessDestroyEmail` (no stored copy).
    pub destroy_copy: bool,
}

/// What the Stalwart side of Email Notifications needs.
pub trait MailEngine: Send + Sync {
    /// E3: a sender account (no usable credential, logins off). Returns
    /// the account id.
    fn create_sender_account(&self, local: &str, stalwart_domain_id: &str) -> Result<String, String>;
    /// E26: rename-retire the account (mail to it bounces; data kept).
    fn retire_account(&self, account_id: &str) -> Result<(), String>;
    /// E2: is `address` any Stalwart principal's name or alias, a mailing
    /// list, or the domain's catch-all destination?
    fn address_taken(&self, address: &str, stalwart_domain_id: &str) -> Result<bool, String>;
    /// E8: submit by delegation.
    fn submit(&self, m: &HostedMail<'_>) -> Result<(), String>;
    /// E5: destroy messages older than `before_unix`; `(destroyed, DSNs)`.
    fn sweep_account(&self, account_id: &str, before_unix: i64) -> Result<(u64, u64), String>;
    /// N6: the delivery and complaint reports waiting in the account's
    /// Inbox and Junk (the report part only, never a returned body).
    fn fetch_reports(&self, account_id: &str, limit: usize) -> Result<Vec<RawReport>, String>;
    /// N6: destroy the reports the pass recorded.
    fn destroy_emails(&self, account_id: &str, ids: &[String]) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// RFC 3464 `message/delivery-status`.
    Dsn,
    /// RFC 5965 `message/feedback-report`.
    Arf,
}

/// One report read from a sender's mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawReport {
    pub email_id: String,
    pub kind: ReportKind,
    /// The report part's text.
    pub report: String,
    /// A complaint's original header block (headers only; the rest is
    /// cut before it is kept).
    pub original_headers: Option<String>,
}

/// Keep only a message's header block (up to the first blank line).
pub fn header_block(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(&raw[..raw.len().min(64 * 1024)]).replace("\r\n", "\n");
    s.split("\n\n").next().unwrap_or("").to_string()
}

impl MailEngine for StalwartClient {
    fn create_sender_account(&self, local: &str, stalwart_domain_id: &str) -> Result<String, String> {
        self.sender_account_create(local, stalwart_domain_id)
    }

    fn retire_account(&self, account_id: &str) -> Result<(), String> {
        self.account_disable(account_id)
    }

    fn address_taken(&self, address: &str, stalwart_domain_id: &str) -> Result<bool, String> {
        let local = address.split('@').next().unwrap_or("").to_ascii_lowercase();
        let users = self.account_list_users()?;
        if crate::mail::jmap::alias_collides("", &local, stalwart_domain_id, &users) {
            return Ok(true);
        }
        let lists = self.mailing_list_query(None)?;
        if !lists.is_empty() {
            for l in self.mailing_list_get(&lists)? {
                let by_address = l.email_address.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(address));
                let by_name = !l.name.is_empty() && l.name.eq_ignore_ascii_case(&local) && (l.domain_id.is_empty() || l.domain_id == stalwart_domain_id);
                if by_address || by_name {
                    return Ok(true);
                }
            }
        }
        Ok(self.domain_get_catchall(stalwart_domain_id)?.is_some_and(|d| d.eq_ignore_ascii_case(address)))
    }

    fn submit(&self, m: &HostedMail<'_>) -> Result<(), String> {
        self.submission_send_envelope(m.account_id, m.from, m.rcpt, m.email.clone(), Some(&m.params), m.destroy_copy)
    }

    fn sweep_account(&self, account_id: &str, before_unix: i64) -> Result<(u64, u64), String> {
        let before = chrono::DateTime::<chrono::Utc>::from_timestamp(before_unix, 0)
            .ok_or_else(|| "sweep: bad time".to_string())?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.email_sweep_older_than(account_id, &before)
    }

    fn fetch_reports(&self, account_id: &str, limit: usize) -> Result<Vec<RawReport>, String> {
        let mut out = Vec::new();
        for (email_id, parts) in self.email_report_parts(account_id, limit)? {
            let find = |pred: &dyn Fn(&str) -> bool| parts.iter().find(|(t, _)| pred(t)).map(|(t, b)| (t.clone(), b.clone()));
            let (kind, (ty, blob)) = if let Some(p) = find(&|t| t.ends_with("delivery-status")) {
                (ReportKind::Dsn, p)
            } else if let Some(p) = find(&|t| t == "message/feedback-report") {
                (ReportKind::Arf, p)
            } else {
                continue;
            };
            let bytes = self.blob_download(account_id, &blob, "report", &ty)?;
            let report = String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]).into_owned();
            let original_headers = if kind == ReportKind::Arf {
                match find(&|t| t == "text/rfc822-headers" || t == "message/rfc822" || t == "message/global") {
                    Some((t, b)) => Some(header_block(&self.blob_download(account_id, &b, "original", &t)?)),
                    None => None,
                }
            } else {
                None
            };
            out.push(RawReport { email_id, kind, report, original_headers });
        }
        Ok(out)
    }

    fn destroy_emails(&self, account_id: &str, ids: &[String]) -> Result<(), String> {
        self.email_destroy(account_id, ids).map(|_| ())
    }
}

/// The SMTP relay transport (N3).
pub trait RelayTransport: Send + Sync {
    fn submit(&self, relay: &Relay, password: &str, from: &str, rcpt: &[String], rfc822: &[u8]) -> Result<(), String>;
}

/// lettre over rustls (certificate verification always on).
pub struct RealRelay;

impl RelayTransport for RealRelay {
    fn submit(&self, relay: &Relay, password: &str, from: &str, rcpt: &[String], rfc822: &[u8]) -> Result<(), String> {
        use crate::mail::external_smtp::{RealSmtpOps, SmtpAuth, SmtpOps, SmtpRoute, SmtpSecurity};
        let route = SmtpRoute {
            host: relay.host.clone(),
            port: relay.port,
            security: if relay.tls == "starttls" { SmtpSecurity::StartTls } else { SmtpSecurity::ImplicitTls },
        };
        RealSmtpOps.submit(&route, &relay.username, &SmtpAuth::Password(password.to_string()), from, rcpt, rfc822)
    }
}

/// Why a relay password couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredError {
    /// The vault isn't open: `503 vault_closed` (never a file fallback, E12).
    VaultClosed,
    /// The vault refused (its code only).
    Vault(String),
}

impl CredError {
    pub fn to_fail(&self) -> Fail {
        match self {
            CredError::VaultClosed => fail(503, "vault_closed", "the vault isn't open yet (K2 is still starting); try again shortly"),
            CredError::Vault(c) => fail(500, "failed", format!("the vault refused ({c})")),
        }
    }
}

/// Where relay passwords live (E12): the vault's system namespace, scope
/// `engine:email-notify`, name `relay-<id>`. Never the 0600 file store.
pub trait RelayCreds: Send + Sync {
    fn put(&self, relay_id: &str, password: &[u8]) -> Result<(), CredError>;
    fn get(&self, relay_id: &str) -> Result<Option<String>, CredError>;
    fn destroy(&self, relay_id: &str) -> Result<(), CredError>;
}

pub const VAULT_ENGINE: &str = "email-notify";
pub const VAULT_SUBSYSTEM: &str = "email-notify";

pub fn vault_name(relay_id: &str) -> String {
    format!("relay-{relay_id}")
}

/// The real vault.
pub struct VaultCreds;

impl RelayCreds for VaultCreds {
    fn put(&self, relay_id: &str, password: &[u8]) -> Result<(), CredError> {
        use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Scope, SecretBytes, SecretStore};
        let v = crate::vault::get().map_err(|_| CredError::VaultClosed)?;
        let mut r = PutRequest::new(Namespace::System, Scope::engine(VAULT_ENGINE), vault_name(relay_id), Kind::Generic);
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::None);
        r.origin = format!("system:{VAULT_SUBSYSTEM}");
        r.description = "An SMTP relay login for Email Notifications; daemon-only.".to_string();
        r.mode = PutMode::Upsert;
        r.idempotent = true;
        v.put(r, SecretBytes::from_slice(password), &Actor::System(VAULT_SUBSYSTEM.into())).map(|_| ()).map_err(|e| CredError::Vault(e.code.to_string()))
    }

    fn get(&self, relay_id: &str) -> Result<Option<String>, CredError> {
        use k2_vault::{Requester, Scope, SecretStore, Selector, Use};
        let v = crate::vault::get().map_err(|_| CredError::VaultClosed)?;
        match v.resolve(&Selector::system(Scope::engine(VAULT_ENGINE), vault_name(relay_id)), Use::System, &Requester::System(VAULT_SUBSYSTEM.into())) {
            Ok(r) => String::from_utf8(r.value.expose().to_vec()).map(Some).map_err(|_| CredError::Vault("not_utf8".into())),
            Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
            Err(e) => Err(CredError::Vault(e.code.to_string())),
        }
    }

    fn destroy(&self, relay_id: &str) -> Result<(), CredError> {
        use k2_vault::{Actor, Scope, SecretStore, Selector};
        let v = crate::vault::get().map_err(|_| CredError::VaultClosed)?;
        match v.destroy(&Selector::system(Scope::engine(VAULT_ENGINE), vault_name(relay_id)), &Actor::System(VAULT_SUBSYSTEM.into())) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 || e.code == "secret_destroyed" => Ok(()),
            Err(e) => Err(CredError::Vault(e.code.to_string())),
        }
    }
}

// ── the seams ────────────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    static TEST_ENGINE: std::cell::RefCell<Option<Arc<dyn MailEngine>>> = const { std::cell::RefCell::new(None) };
    static TEST_RELAY: std::cell::RefCell<Option<Arc<dyn RelayTransport>>> = const { std::cell::RefCell::new(None) };
    static TEST_CREDS: std::cell::RefCell<Option<Arc<dyn RelayCreds>>> = const { std::cell::RefCell::new(None) };
    static TEST_DNS: std::cell::RefCell<Option<Arc<dyn DnsResolver>>> = const { std::cell::RefCell::new(None) };
    static TEST_HOSTED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
pub(crate) fn set_test_engine(e: Option<Arc<dyn MailEngine>>) {
    TEST_ENGINE.with(|c| *c.borrow_mut() = e);
}
#[cfg(test)]
pub(crate) fn set_test_relay(r: Option<Arc<dyn RelayTransport>>) {
    TEST_RELAY.with(|c| *c.borrow_mut() = r);
}
#[cfg(test)]
pub(crate) fn set_test_creds(c: Option<Arc<dyn RelayCreds>>) {
    TEST_CREDS.with(|x| *x.borrow_mut() = c);
}
#[cfg(test)]
pub(crate) fn set_test_dns(d: Option<Arc<dyn DnsResolver>>) {
    TEST_DNS.with(|c| *c.borrow_mut() = d);
}
/// Tests: is this thread's box Linux-with-hosted-mail (default yes)?
#[cfg(test)]
pub(crate) fn set_test_hosted_supported(on: bool) {
    TEST_HOSTED.with(|c| c.set(on));
}
#[cfg(test)]
pub(crate) fn test_hosted_supported() -> bool {
    TEST_HOSTED.with(|c| c.get())
}

/// The local Stalwart (`not_ready` while it isn't installed and running).
pub fn mail_engine() -> Result<Arc<dyn MailEngine>, Fail> {
    #[cfg(test)]
    {
        if let Some(e) = TEST_ENGINE.with(|c| c.borrow().clone()) {
            return Ok(e);
        }
        Err(fail(409, "domain_not_ready", "tests: no fake mail engine installed"))
    }
    #[cfg(not(test))]
    {
        match crate::mail::domains::engine_from_db() {
            Ok((client, _)) => Ok(Arc::new(client)),
            Err(hint) => Err(fail(409, "domain_not_ready", hint)),
        }
    }
}

pub fn relay_transport() -> Arc<dyn RelayTransport> {
    #[cfg(test)]
    {
        TEST_RELAY.with(|c| c.borrow().clone()).expect("tests: install a fake relay transport (set_test_relay) before sending through a relay")
    }
    #[cfg(not(test))]
    {
        if let Some(dir) = sink_dir() {
            return Arc::new(SinkRelay(dir));
        }
        Arc::new(RealRelay)
    }
}

/// Test hook (DEBUG builds only; a release build ignores it):
/// `K2_TEST_EMAIL_SINK_DIR=<dir>` makes every relay hand-off write the
/// message to `<dir>/<n>.eml` instead of dialing the relay. The headless
/// curl script (`scripts/email-notifications-headless.sh`) uses it so a
/// daemon-only run never sends mail.
#[cfg(not(test))]
fn sink_dir() -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("K2_TEST_EMAIL_SINK_DIR").map(std::path::PathBuf::from).filter(|p| p.is_absolute())
}

/// The sink [`RelayTransport`] (see `sink_dir`): the envelope as two
/// `X-K2-Sink-*` lines, then the message. Never the password.
pub struct SinkRelay(pub std::path::PathBuf);

impl RelayTransport for SinkRelay {
    fn submit(&self, _relay: &Relay, _password: &str, from: &str, rcpt: &[String], rfc822: &[u8]) -> Result<(), String> {
        std::fs::create_dir_all(&self.0).map_err(|e| format!("sink: {e}"))?;
        let n = std::fs::read_dir(&self.0).map(|d| d.count()).unwrap_or(0) + 1;
        let mut out = format!("X-K2-Sink-From: {from}\r\nX-K2-Sink-To: {}\r\n", rcpt.join(",")).into_bytes();
        out.extend_from_slice(rfc822);
        std::fs::write(self.0.join(format!("{n}.eml")), out).map_err(|e| format!("sink: {e}"))
    }
}

pub fn relay_creds() -> Arc<dyn RelayCreds> {
    #[cfg(test)]
    {
        TEST_CREDS.with(|c| c.borrow().clone()).expect("tests: install fake relay creds (set_test_creds)")
    }
    #[cfg(not(test))]
    {
        Arc::new(VaultCreds)
    }
}

pub fn dns() -> Result<Arc<dyn DnsResolver>, Fail> {
    #[cfg(test)]
    {
        TEST_DNS.with(|c| c.borrow().clone()).ok_or_else(|| fail(409, "dmarc_missing", "tests: no fake resolver installed"))
    }
    #[cfg(not(test))]
    {
        crate::mail::dns_verify::SystemResolver::new()
            .map(|r| Arc::new(r) as Arc<dyn DnsResolver>)
            .map_err(|e| fail(503, "failed", format!("no DNS resolver: {e}")))
    }
}
