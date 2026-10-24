//! Vault errors. Every error carries a stable machine code (what routes
//! and the CLI print) and an HTTP status. Messages never contain a value.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultError {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

pub type Result<T> = std::result::Result<T, VaultError>;

/// Code of [`VaultError::needs_newer`].
pub const NEEDS_NEWER: &str = "vault_needs_newer_k2";

impl VaultError {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> Self {
        Self { code, status, hint: hint.into() }
    }

    pub fn bad_request(code: &'static str, hint: impl Into<String>) -> Self {
        Self::new(code, 400, hint)
    }

    pub fn forbidden(code: &'static str, hint: impl Into<String>) -> Self {
        Self::new(code, 403, hint)
    }

    pub fn not_found(hint: impl Into<String>) -> Self {
        Self::new("not_found", 404, hint)
    }

    pub fn conflict(code: &'static str, hint: impl Into<String>) -> Self {
        Self::new(code, 409, hint)
    }

    pub fn internal(hint: impl Into<String>) -> Self {
        Self::new("vault_internal", 500, hint)
    }

    /// A row or header failed its integrity check. Fails closed.
    pub fn tamper(hint: impl Into<String>) -> Self {
        Self::new("vault_tamper", 500, hint)
    }

    /// A row written by a newer K2: it names a kind, delivery, consumer or
    /// inject destination this build doesn't know (PRD vault-gaps R2).
    /// Readers that scan many rows skip and count it; a direct lookup
    /// answers with this. Not tamper: the row is intact, just newer.
    pub fn needs_newer(hint: impl Into<String>) -> Self {
        Self::new(NEEDS_NEWER, 409, hint)
    }

    pub fn is_needs_newer(&self) -> bool {
        self.code == NEEDS_NEWER
    }

    /// A tricked-agent rule (§11.2): ask the owner instead.
    pub fn owner_confirm_required(hint: impl Into<String>) -> Self {
        Self::new(
            "vault_owner_confirm_required",
            403,
            format!("{}; ask the owner to change it in Settings → Vault", hint.into()),
        )
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

impl std::error::Error for VaultError {}

impl From<rusqlite::Error> for VaultError {
    fn from(e: rusqlite::Error) -> Self {
        // SQLite errors never include bound parameter values.
        VaultError::internal(format!("vault.db: {e}"))
    }
}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError::internal(format!("io: {e}"))
    }
}
