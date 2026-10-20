//! Authentication bodies (C6).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Handle;
use crate::api::members::Member;
use crate::error::{Validate, ValidationError};
use crate::limits::{PASSWORD_MAX_BYTES, PASSWORD_MIN_CHARS};
use crate::time::Timestamp;
use crate::token::redact;

/// Longest K2 pass accepted, in bytes.
pub const PASS_MAX_BYTES: usize = 8_192;

fn check_pass(pass: &str) -> Result<(), ValidationError> {
    if pass.is_empty() || pass.len() > PASS_MAX_BYTES {
        return Err(ValidationError::invalid(
            "pass",
            format!("pass must be 1–{PASS_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/auth/k2`: exchange a K2 chat pass for a session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct K2PassRequest {
    /// The K2 chat pass (a compact JWS).
    pub pass: String,
}

impl fmt::Debug for K2PassRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("K2PassRequest")
            .field("pass", &"***")
            .finish()
    }
}

impl Validate for K2PassRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_pass(&self.pass)
    }
}

/// Body of `POST /api/v1/auth/renew`: a fresh pass for the same subject.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenewRequest {
    /// The K2 chat pass.
    pub pass: String,
}

impl fmt::Debug for RenewRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenewRequest")
            .field("pass", &"***")
            .finish()
    }
}

impl Validate for RenewRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_pass(&self.pass)
    }
}

/// Body of `POST /api/v1/auth/login` (standalone installs only).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// The handle.
    pub username: Handle,
    /// The password, 1–1024 bytes.
    pub password: String,
}

impl fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

impl Validate for LoginRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.password.is_empty() || self.password.len() > PASSWORD_MAX_BYTES {
            return Err(ValidationError::invalid(
                "password",
                format!("password must be 1–{PASSWORD_MAX_BYTES} bytes"),
            ));
        }
        Ok(())
    }
}

/// Longest setup token accepted, in bytes (the real ones are 49).
pub const SETUP_TOKEN_MAX_BYTES: usize = 256;

/// Body of `POST /api/v1/auth/password/set` (standalone installs only, S8 gap S8-G7): sets the
/// member's password with a one-time setup token from a setup link.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordSetRequest {
    /// The setup token, `sewpw_<secret>` (from the link's `#token=` fragment).
    pub token: String,
    /// The new password: 12–1024 bytes and at least 12 characters. The server also refuses a
    /// password equal to the username.
    pub password: String,
}

impl fmt::Debug for PasswordSetRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordSetRequest")
            .field("token", &redact(&self.token))
            .field("password", &"***")
            .finish()
    }
}

impl Validate for PasswordSetRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.token.is_empty() || self.token.len() > SETUP_TOKEN_MAX_BYTES {
            return Err(ValidationError::invalid(
                "token",
                format!("token must be 1–{SETUP_TOKEN_MAX_BYTES} bytes"),
            ));
        }
        if self.password.len() > PASSWORD_MAX_BYTES
            || self.password.chars().count() < PASSWORD_MIN_CHARS
        {
            return Err(ValidationError::invalid(
                "password",
                format!(
                    "password must be at least {PASSWORD_MIN_CHARS} characters and at most {PASSWORD_MAX_BYTES} bytes"
                ),
            ));
        }
        Ok(())
    }
}

/// Response of the three session routes: the session token and the signed-in member.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResponse {
    /// The session token, `sews_<selector>.<secret>`.
    pub token: String,
    /// When the session expires.
    pub expires_at: Timestamp,
    /// The signed-in member.
    pub member: Member,
}

impl fmt::Debug for SessionResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionResponse")
            .field("token", &redact(&self.token))
            .field("expires_at", &self.expires_at)
            .field("member", &self.member)
            .finish()
    }
}

/// Response of `POST /api/v1/gateway/ticket`: a single-use ticket bound to the session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayTicketResponse {
    /// The ticket, `sewtk_<secret>`.
    pub ticket: String,
    /// The gateway URL to open.
    pub url: String,
    /// Milliseconds until the ticket expires (gap G21).
    pub expires_in_ms: u64,
}

impl fmt::Debug for GatewayTicketResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayTicketResponse")
            .field("ticket", &redact(&self.ticket))
            .field("url", &self.url)
            .field("expires_in_ms", &self.expires_in_ms)
            .finish()
    }
}
