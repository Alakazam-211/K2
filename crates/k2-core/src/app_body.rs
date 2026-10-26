//! Which body encodings the App gateway link takes on a write (0.46.4,
//! field report: an App moved onto the runtime posted form-encoded to
//! `POST /cli/thread/post`, the link answered `415 json_required`, and the
//! App's UI silently did nothing; the old daemon path took the form).
//!
//! The link forwards a write to the same handler the daemon serves on its
//! main port, so it takes what that handler reads: JSON on every write,
//! and `application/x-www-form-urlencoded` on the writes whose handler
//! also parses a form body ([`FORM_POSTS`], audited route by route against
//! the handlers in 0.46.4). Every other write reads JSON only, and the
//! link keeps refusing a form body there with `415 json_required` and a
//! hint that names the route as JSON-only (a form body would reach the
//! handler as a bare 400 at best, or be silently ignored at worst).
//!
//! `k2 app warnings` reads the same list to flag an App's form-encoded
//! calls to K2 before a move (`app_grace::scan`).

/// The App-facing writes whose daemon handler parses a form body as well
/// as JSON (the handler's own parser: `overlay_routes::merge_body`,
/// `skin_routes::parse_form_pairs`, the resources arm's
/// `routes::http::parse_form_body`). `/cli/skin/logout` reads no body.
pub const FORM_POSTS: &[&str] = &[
    "/cli/skin/login",
    "/cli/skin/logout",
    "/cli/thread/post",
    "/cli/thread/answer",
    "/cli/thread/void",
    "/cli/workspace/resources/add",
    "/cli/workspace/resources/remove",
];

/// How a request's `Content-Type` encodes its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Json,
    Form,
    /// Anything else (or no `Content-Type` at all).
    Other,
}

/// The encoding a `Content-Type` value names (parameters such as
/// `; charset=utf-8` ignored; case-insensitive).
pub fn body_kind(content_type: Option<&str>) -> BodyKind {
    let Some(ct) = content_type else { return BodyKind::Other };
    let media = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match media.as_str() {
        "application/json" => BodyKind::Json,
        "application/x-www-form-urlencoded" => BodyKind::Form,
        _ if media.starts_with("application/") && media.ends_with("+json") => BodyKind::Json,
        _ => BodyKind::Other,
    }
}

/// Whether the write at `path` takes a form-encoded body on the link.
pub fn accepts_form(path: &str) -> bool {
    FORM_POSTS.contains(&path)
}

/// Whether a write with this body encoding is admitted at `path`.
pub fn admits(path: &str, kind: BodyKind) -> bool {
    match kind {
        BodyKind::Json => true,
        BodyKind::Form => accepts_form(path),
        BodyKind::Other => false,
    }
}

/// The `415 json_required` hint for a refused write at `path`.
pub fn refusal_hint(path: &str, kind: BodyKind) -> String {
    if kind == BodyKind::Form {
        format!(
            "{path} reads JSON only: send the fields as a JSON object with Content-Type: application/json \
             (form-encoded bodies work only on {})",
            FORM_POSTS.join(", ")
        )
    } else if accepts_form(path) {
        format!("{path} reads JSON (Content-Type: application/json) or a form (application/x-www-form-urlencoded)")
    } else {
        format!("{path} reads JSON only: send Content-Type: application/json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_post_is_a_post_the_link_serves() {
        for p in FORM_POSTS {
            assert!(crate::app_surface::link_allows_http("POST", p), "{p} isn't a POST on the link");
        }
    }

    #[test]
    fn content_types_classify() {
        assert_eq!(body_kind(Some("application/json")), BodyKind::Json);
        assert_eq!(body_kind(Some("Application/JSON; charset=utf-8")), BodyKind::Json);
        assert_eq!(body_kind(Some("application/merge-patch+json")), BodyKind::Json);
        assert_eq!(body_kind(Some("application/x-www-form-urlencoded")), BodyKind::Form);
        assert_eq!(body_kind(Some("application/x-www-form-urlencoded;charset=UTF-8")), BodyKind::Form);
        assert_eq!(body_kind(Some("multipart/form-data; boundary=x")), BodyKind::Other);
        assert_eq!(body_kind(Some("text/plain")), BodyKind::Other);
        assert_eq!(body_kind(Some("application/jsonx")), BodyKind::Other);
        assert_eq!(body_kind(None), BodyKind::Other);
    }

    #[test]
    fn form_is_admitted_only_where_the_handler_reads_it() {
        assert!(admits("/cli/thread/post", BodyKind::Form));
        assert!(admits("/cli/workspace/resources/add", BodyKind::Form));
        assert!(!admits("/cli/feedback/create", BodyKind::Form));
        assert!(admits("/cli/feedback/create", BodyKind::Json));
        assert!(!admits("/cli/thread/post", BodyKind::Other));
        assert!(refusal_hint("/cli/feedback/create", BodyKind::Form).starts_with("/cli/feedback/create reads JSON only"));
        assert!(refusal_hint("/cli/thread/post", BodyKind::Other).contains("x-www-form-urlencoded"));
    }
}
