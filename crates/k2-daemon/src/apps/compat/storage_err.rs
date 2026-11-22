//! Storage errors in Supabase Storage's own shape (PRD §10, R0).
//!
//! Supabase answers almost every refusal with HTTP **400** and puts the
//! real status in the body as a string:
//!
//! ```json
//! {"statusCode": "409", "code": "KeyAlreadyExists", "error": "Duplicate",
//!  "message": "The resource already exists"}
//! ```
//!
//! Only a 5xx keeps its own HTTP status, and a few refusals carry an
//! explicit one (a role the door won't take is 403, a range past the end
//! 416). supabase-js and the Dart client read `statusCode`, `error` and
//! `message` from the body; the phone app's offline queue depends on the
//! duplicate answer being exactly this.

use serde_json::{json, Value};

/// One storage refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageError {
    /// The status in the body (`"404"`, `"409"` …).
    pub status: u16,
    /// Supabase's machine code (`NoSuchKey`, `KeyAlreadyExists` …).
    pub code: &'static str,
    /// The legacy `error` field (`not_found`, `Duplicate` …).
    pub error: String,
    pub message: String,
    /// The HTTP status when it isn't the default (400 below 500).
    pub http: Option<u16>,
}

impl StorageError {
    pub fn new(status: u16, code: &'static str, error: impl Into<String>, message: impl Into<String>) -> Self {
        StorageError { status, code, error: error.into(), message: message.into(), http: None }
    }

    /// The HTTP status of the answer.
    pub fn http_status(&self) -> u16 {
        match self.http {
            Some(h) => h,
            None if self.status >= 500 => self.status,
            None => 400,
        }
    }

    pub fn with_http(mut self, h: u16) -> Self {
        self.http = Some(h);
        self
    }

    pub fn body(&self) -> Value {
        json!({
            "statusCode": self.status.to_string(),
            "code": self.code,
            "error": self.error,
            "message": self.message,
        })
    }

    // ── the ones the door gives ──────────────────────────────────────

    pub fn not_found() -> Self {
        Self::new(404, "NoSuchKey", "not_found", "Object not found")
    }
    pub fn no_bucket() -> Self {
        Self::new(404, "NoSuchBucket", "Bucket not found", "Bucket not found")
    }
    /// An upload over a live object without upsert. The phone app's
    /// offline queue reads this as "already there, done".
    pub fn duplicate() -> Self {
        Self::new(409, "KeyAlreadyExists", "Duplicate", "The resource already exists")
    }
    /// A bucket id or name that is taken.
    pub fn bucket_exists() -> Self {
        Self::new(409, "ResourceAlreadyExists", "Duplicate", "The resource already exists")
    }
    /// A storage policy said no (Postgres 42501).
    pub fn rls() -> Self {
        Self::new(403, "AccessDenied", "Unauthorized", "new row violates row-level security policy")
    }
    pub fn access_denied(msg: impl Into<String>) -> Self {
        Self::new(403, "AccessDenied", "Unauthorized", msg)
    }
    pub fn invalid_jwt(msg: impl Into<String>) -> Self {
        Self::new(400, "InvalidJWT", "InvalidJWT", msg)
    }
    pub fn invalid_signature(msg: impl Into<String>) -> Self {
        Self::new(400, "InvalidSignature", "InvalidSignature", msg)
    }
    pub fn expired_signature() -> Self {
        Self::new(400, "ExpiredToken", "ExpiredToken", "The provided token has expired.")
    }
    pub fn invalid_key(key: &str) -> Self {
        Self::new(400, "InvalidKey", "InvalidKey", format!("Invalid key: {key}"))
    }
    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(400, "InvalidRequest", "InvalidRequest", msg)
    }
    pub fn too_large(limit: u64) -> Self {
        Self::new(413, "EntityTooLarge", "Payload too large", format!("The object exceeded the maximum allowed size ({limit} bytes)"))
    }
    pub fn mime(mime: &str) -> Self {
        Self::new(415, "InvalidMimeType", "invalid_mime_type", format!("mime type {mime} is not supported"))
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(500, "InternalError", "internal", msg)
    }
    /// The door isn't on for this App, or its database can't be reached.
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::new(503, "DatabaseError", "unavailable", msg)
    }
    /// K2's own refusal for a part of the API v1 doesn't carry (a 400 with
    /// a code naming the part, never a wrong answer: PRD §7.2's rule).
    pub fn unsupported(part: &str) -> Self {
        Self::new(400, "NotSupported", "k2_unsupported", format!("{part} is not supported by K2's storage door in v1"))
    }

    /// Map a Postgres error the way Supabase Storage does.
    pub fn from_pg(e: &tokio_postgres::Error) -> Self {
        let Some(db) = e.as_db_error() else {
            return Self::unavailable(format!("database error: {e}"));
        };
        let code = db.code().code();
        match code {
            "42501" if db.message().contains("row-level security") => Self::rls(),
            "42501" => Self::access_denied(db.message().to_string()),
            "23505" => Self::duplicate(),
            "23503" => Self::new(404, "RelatedResourceNotFound", "not_found", "The related resource does not exist"),
            "57014" => Self::new(544, "DatabaseTimeout", "DatabaseTimeout", "The connection to the database timed out").with_http(544),
            "22P02" | "22007" | "22008" => Self::invalid_request(db.message().to_string()),
            "42703" | "42P01" => Self::new(
                503,
                "DatabaseSchemaMismatch",
                "DatabaseSchemaMismatch",
                format!("There is a database schema mismatch in a trigger or RLS policy: {}", db.message()),
            ),
            other => Self::new(500, "DatabaseError", "DatabaseError", format!("database error, code: {other}")),
        }
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.message)
    }
}

pub type SResult<T> = Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_is_supabases_exact_answer() {
        let e = StorageError::duplicate();
        assert_eq!(e.http_status(), 400, "Supabase answers a duplicate with HTTP 400");
        assert_eq!(
            e.body(),
            json!({"statusCode": "409", "code": "KeyAlreadyExists", "error": "Duplicate", "message": "The resource already exists"})
        );
    }

    #[test]
    fn statuses() {
        assert_eq!(StorageError::not_found().http_status(), 400);
        assert_eq!(StorageError::internal("x").http_status(), 500);
        assert_eq!(StorageError::unavailable("x").http_status(), 503);
        assert_eq!(StorageError::access_denied("x").with_http(403).http_status(), 403);
        assert_eq!(StorageError::rls().body()["message"], "new row violates row-level security policy");
    }
}
