//! Shared response shape used by every `/cli/*` route handler.
//!
//! Extracted into its own module so lib-side handler modules
//! (`terminal_routes`, future `companion_routes`, etc.) can share
//! the type with `main.rs`'s dispatch without needing `cli.rs` —
//! which contains `crate::handle_*` references scoped to main.rs
//! and therefore can't move into the lib.

/// Final HTTP response body + status line the dispatch caller
/// emits. Owned struct so the caller attaches the
/// `Content-Length` / `Connection: close` boilerplate once at the
/// top of the dispatch.
pub struct CliResponse {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: String,
}

impl CliResponse {
    pub fn ok_json(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            body,
        }
    }
    pub fn ok_text(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/plain; charset=utf-8",
            body,
        }
    }
    pub fn bad_request(err: impl std::fmt::Display) -> Self {
        Self {
            status: "400 Bad Request",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
    pub fn internal_error(err: impl std::fmt::Display) -> Self {
        Self {
            status: "500 Internal Server Error",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
    pub fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            content_type: "application/json",
            body: r#"{"error":"route not found"}"#.to_string(),
        }
    }
    pub fn forbidden() -> Self {
        Self {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
        }
    }
    /// 0.39.0f Phase 2.1b: `/cli/work/*` routes are retired but the
    /// route entries remain so external callers get a clear HTTP-410
    /// signal (rather than a silent 404 on the catch-all). The body
    /// must point them at the replacement and `help-deprecated`.
    pub fn gone(err: impl std::fmt::Display) -> Self {
        Self {
            status: "410 Gone",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — response constructors
// ──────────────────────────────────────────────────────────────────────
//
// Every `/cli/*` route returns a `CliResponse`; these tests pin the
// status + content-type + body contract that dispatch callers depend
// on. They are pure-function tests and intentionally don't go through
// the HTTP layer.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_json_has_200_status_and_json_content_type() {
        let r = CliResponse::ok_json(r#"{"ok":true}"#.to_string());
        assert_eq!(r.status, "200 OK");
        assert_eq!(r.content_type, "application/json");
        assert_eq!(r.body, r#"{"ok":true}"#);
    }

    #[test]
    fn ok_text_has_200_status_and_utf8_text_content_type() {
        let r = CliResponse::ok_text("plain reply".to_string());
        assert_eq!(r.status, "200 OK");
        assert_eq!(r.content_type, "text/plain; charset=utf-8");
        assert_eq!(r.body, "plain reply");
    }

    #[test]
    fn bad_request_wraps_error_in_json_body() {
        let r = CliResponse::bad_request("missing 'name'");
        assert_eq!(r.status, "400 Bad Request");
        assert_eq!(r.content_type, "application/json");
        // Body must be valid JSON that includes the error message.
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("bad_request body is valid JSON");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("missing 'name'"),
            "error field missing or wrong: {}",
            r.body,
        );
    }

    #[test]
    fn internal_error_uses_500_status() {
        let r = CliResponse::internal_error("boom");
        assert_eq!(r.status, "500 Internal Server Error");
        assert_eq!(r.content_type, "application/json");
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("body is valid JSON");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("boom"),
        );
    }

    #[test]
    fn not_found_has_404_status_and_route_not_found_message() {
        let r = CliResponse::not_found();
        assert_eq!(r.status, "404 Not Found");
        assert_eq!(r.content_type, "application/json");
        // Stable wire shape — renderer key off this exact string.
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("body is valid JSON");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("route not found"),
        );
    }

    #[test]
    fn forbidden_has_403_status_and_auth_error_message() {
        let r = CliResponse::forbidden();
        assert_eq!(r.status, "403 Forbidden");
        assert_eq!(r.content_type, "application/json");
        // Auth gate response shape — CLI key off this prefix.
        assert!(
            r.body.contains("Invalid or missing auth token"),
            "403 body missing expected message: {}",
            r.body,
        );
    }

    #[test]
    fn gone_has_410_status_and_carries_error_message() {
        let r = CliResponse::gone("use `k2so workspace launch` instead");
        assert_eq!(r.status, "410 Gone");
        assert_eq!(r.content_type, "application/json");
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("body is valid JSON");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("use `k2so workspace launch` instead"),
        );
    }

    #[test]
    fn bad_request_escapes_quote_in_error_message() {
        // Regression guard: error strings containing JSON-special
        // characters must not break the body's JSON validity. If a
        // caller passes `unclosed "string`, the body must still parse.
        let r = CliResponse::bad_request(r#"unexpected "quote" in body"#);
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("escaped JSON parses");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some(r#"unexpected "quote" in body"#),
        );
    }
}
