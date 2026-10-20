//! Opaque token prefixes and shapes (C3, C6, gap G26).
//!
//! Tokens are not ids. After the prefix, bodies are base64url without padding: a session
//! selector is 16 bytes (22 chars) and every secret is 32 bytes (43 chars). Generation lives in
//! the server (this crate has no RNG).

use std::fmt;

/// Session token prefix: `sews_<selector>.<secret>`.
pub const SESSION_PREFIX: &str = "sews_";
/// Mgmt service token prefix: `sewsvc_<secret>`.
pub const SERVICE_PREFIX: &str = "sewsvc_";
/// Upload capability prefix: `sewup_<secret>`.
pub const UPLOAD_PREFIX: &str = "sewup_";
/// Gateway ticket prefix: `sewtk_<secret>`.
pub const TICKET_PREFIX: &str = "sewtk_";
/// Standalone one-time password setup token prefix: `sewpw_<secret>` (S8 gap S8-G7).
pub const SETUP_PREFIX: &str = "sewpw_";

/// Length of a base64url 16-byte selector.
pub const SELECTOR_LEN: usize = 22;
/// Length of a base64url 32-byte secret.
pub const SECRET_LEN: usize = 43;

/// Whether every byte is in the base64url alphabet (`A-Z a-z 0-9 - _`).
pub fn is_base64url(s: &str) -> bool {
    s.bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Which kind of opaque token a string is, by prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `sews_`
    Session,
    /// `sewsvc_`
    Service,
    /// `sewup_`
    Upload,
    /// `sewtk_`
    Ticket,
    /// `sewpw_` (standalone password setup link, S8)
    Setup,
}

impl TokenKind {
    /// Classifies by prefix. `sewsvc_` is checked before `sews_`.
    pub fn classify(s: &str) -> Option<TokenKind> {
        if s.starts_with(SERVICE_PREFIX) {
            Some(TokenKind::Service)
        } else if s.starts_with(SESSION_PREFIX) {
            Some(TokenKind::Session)
        } else if s.starts_with(UPLOAD_PREFIX) {
            Some(TokenKind::Upload)
        } else if s.starts_with(TICKET_PREFIX) {
            Some(TokenKind::Ticket)
        } else if s.starts_with(SETUP_PREFIX) {
            Some(TokenKind::Setup)
        } else {
            None
        }
    }

    /// The prefix for this kind.
    pub const fn prefix(self) -> &'static str {
        match self {
            TokenKind::Session => SESSION_PREFIX,
            TokenKind::Service => SERVICE_PREFIX,
            TokenKind::Upload => UPLOAD_PREFIX,
            TokenKind::Ticket => TICKET_PREFIX,
            TokenKind::Setup => SETUP_PREFIX,
        }
    }
}

/// Whether `s` is `<prefix><43 base64url chars>`, the shape of service, upload, ticket and setup
/// tokens.
pub fn is_secret_token(s: &str, kind: TokenKind) -> bool {
    match s.strip_prefix(kind.prefix()) {
        Some(body) => body.len() == SECRET_LEN && is_base64url(body),
        None => false,
    }
}

/// A parsed session token `sews_<selector>.<secret>`. `Debug` redacts the secret.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionToken {
    selector: String,
    secret: String,
}

impl SessionToken {
    /// Checks prefix, `.`, lengths and the base64url alphabet. Does not decode.
    pub fn parse(s: &str) -> Option<SessionToken> {
        let body = s.strip_prefix(SESSION_PREFIX)?;
        let (selector, secret) = body.split_once('.')?;
        if selector.len() != SELECTOR_LEN
            || secret.len() != SECRET_LEN
            || !is_base64url(selector)
            || !is_base64url(secret)
        {
            return None;
        }
        Some(SessionToken {
            selector: selector.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The lookup selector (not secret).
    pub fn selector(&self) -> &str {
        &self.selector
    }

    /// The secret part. Store only its SHA-256.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// The full token string.
    pub fn expose(&self) -> String {
        format!("{SESSION_PREFIX}{}.{}", self.selector, self.secret)
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SESSION_PREFIX}{}.***", self.selector)
    }
}

/// Redacts a token-like string for `Debug`: keeps a known prefix and hides the rest.
pub(crate) fn redact(s: &str) -> String {
    match TokenKind::classify(s) {
        Some(kind) => {
            let body = &s[kind.prefix().len()..];
            match (kind, body.split_once('.')) {
                (TokenKind::Session, Some((selector, _))) => {
                    format!("{SESSION_PREFIX}{selector}.***")
                }
                _ => format!("{}***", kind.prefix()),
            }
        }
        None => "***".to_owned(),
    }
}
