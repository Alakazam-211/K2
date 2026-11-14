//! What one TCP request lets the response writers do to its answer:
//!
//! - **gzip** a JSON answer of [`GZIP_MIN_BYTES`] or more when the request
//!   came from off this machine (LAN, a local reverse proxy, the Connect
//!   tunnel) and its `Accept-Encoding` allows gzip. Never on loopback (the
//!   desktop app on this machine, the `k2` CLI), never for an App's own
//!   container, and never on the cell socket (it has its own writer).
//! - **ETag / 304** on the routes in [`ETAG_ROUTES`]: a 200 JSON answer
//!   carries a weak `ETag` (a hash of the bytes it would send), and a
//!   request whose `If-None-Match` names that tag gets `304 Not Modified`
//!   with no body. The tag is a hash of the answer itself, so it can never
//!   say "unchanged" about a list that changed, whatever changed it.
//!
//! The dispatcher notes each request ([`note`]) inside the connection's
//! [`scope`]; the writers in `routes/http.rs` call [`shape`]. Outside a
//! scope (the cell socket, tests calling a writer directly) nothing changes.

use std::cell::RefCell;
use std::future::Future;
use std::io::Write as _;

/// Routes whose 200 JSON answers carry an ETag and may answer 304.
pub const ETAG_ROUTES: &[&str] = &["/cli/projects/list"];

/// Smaller JSON bodies are sent as they are: gzip would save a few bytes
/// and cost a little time on both ends.
pub const GZIP_MIN_BYTES: usize = 1024;

/// The boot-status feature key: `GET /cli/projects/list` carries an ETag and
/// answers 304 to a matching `If-None-Match`.
pub const FEATURE_PROJECTS_ETAG: &str = "projects-etag";

/// One request's facts, read from its head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// Off this machine and `Accept-Encoding` allows gzip.
    pub gzip: bool,
    /// The request came from off this machine (answers vary by encoding).
    pub remote: bool,
    /// A GET of a route in [`ETAG_ROUTES`].
    pub etag: bool,
    /// The request's `If-None-Match`, as sent.
    pub if_none_match: Option<String>,
}

impl Request {
    /// `remote`: the request's ingress is off this machine. `head` is the
    /// request head (request line and headers).
    pub fn from_head(remote: bool, method: &str, path_and_query: &str, head: &str) -> Request {
        let path = path_and_query.split('?').next().unwrap_or("");
        let etag = method.eq_ignore_ascii_case("GET") && ETAG_ROUTES.contains(&path);
        Request {
            gzip: remote && crate::static_files::accepts(header_value(head, "accept-encoding"), "gzip"),
            remote,
            etag,
            if_none_match: if etag { header_value(head, "if-none-match").map(str::to_string) } else { None },
        }
    }
}

/// A header's value in `head` (first match, trimmed), or `None`.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").skip(1).take_while(|l| !l.is_empty()).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

tokio::task_local! {
    static CURRENT: RefCell<Request>;
}

/// Run one TCP connection with its per-request wire facts.
pub fn scope<F: Future>(f: F) -> impl Future<Output = F::Output> {
    CURRENT.scope(RefCell::new(Request::default()), f)
}

/// The dispatcher, once a request's head is parsed. A no-op outside a scope.
pub fn note(req: Request) {
    let _ = CURRENT.try_with(|c| *c.borrow_mut() = req);
}

/// Forget the last request's facts (the keep-alive loop, before the next).
pub fn reset() {
    note(Request::default());
}

/// This request's facts (the defaults outside a scope: nothing changes).
pub fn current() -> Request {
    CURRENT.try_with(|c| c.borrow().clone()).unwrap_or_default()
}

/// An answer as it goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shaped {
    /// The status line's text (`304 Not Modified` replaces a `200 OK`).
    pub status: String,
    /// Extra complete header lines (`Name: value\r\n`).
    pub headers: String,
    /// The body bytes to send.
    pub body: Vec<u8>,
    /// False for a 304: no `Content-Length` and no body.
    pub has_body: bool,
}

/// The weak ETag of a JSON answer.
pub fn etag_of(body: &str) -> String {
    let h = blake3::hash(body.as_bytes());
    format!("W/\"{}\"", &h.to_hex()[..32])
}

fn is_json(ct: &str) -> bool {
    ct.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
}

/// Shape one answer for `req`: maybe a 304, maybe gzip, else as it is.
pub fn shape(req: &Request, status: &str, ct: &str, body: &str) -> Shaped {
    let json = is_json(ct);
    let ok = status.starts_with("200");
    let mut headers = String::new();
    if req.etag && ok && json {
        let tag = etag_of(body);
        headers.push_str(&format!("ETag: {tag}\r\nCache-Control: no-cache\r\n"));
        if crate::static_files::not_modified(req.if_none_match.as_deref(), &tag) {
            if req.remote {
                headers.push_str("Vary: Accept-Encoding\r\n");
            }
            return Shaped { status: "304 Not Modified".to_string(), headers, body: Vec::new(), has_body: false };
        }
    }
    if req.remote && json {
        headers.push_str("Vary: Accept-Encoding\r\n");
    }
    if req.gzip && json && body.len() >= GZIP_MIN_BYTES {
        if let Some(gz) = gzip(body.as_bytes()) {
            headers.push_str("Content-Encoding: gzip\r\n");
            return Shaped { status: status.to_string(), headers, body: gz, has_body: true };
        }
    }
    Shaped { status: status.to_string(), headers, body: body.as_bytes().to_vec(), has_body: true }
}

/// gzip `bytes` (fast level: these answers go out once). `None` on a
/// compressor error: the answer is then sent as it is.
fn gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len() / 4), flate2::Compression::fast());
    enc.write_all(bytes).ok()?;
    enc.finish().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn head(lines: &[&str]) -> String {
        let mut h = "GET /cli/projects/list?token=t HTTP/1.1\r\nHost: x\r\n".to_string();
        for l in lines {
            h.push_str(l);
            h.push_str("\r\n");
        }
        h.push_str("\r\n");
        h
    }

    fn big_json() -> String {
        let rows: Vec<String> = (0..200).map(|i| format!(r#"{{"id":"p{i}","name":"workspace {i}"}}"#)).collect();
        format!("[{}]", rows.join(","))
    }

    fn gunzip(b: &[u8]) -> String {
        let mut s = String::new();
        flate2::read::GzDecoder::new(b).read_to_string(&mut s).expect("a gzip body");
        s
    }

    #[test]
    fn gzip_only_for_remote_requests_that_allow_it() {
        let body = big_json();
        let h = head(&["Accept-Encoding: gzip, deflate, br"]);
        let remote = Request::from_head(true, "GET", "/cli/projects/list?token=t", &h);
        let local = Request::from_head(false, "GET", "/cli/projects/list?token=t", &h);
        let s = shape(&remote, "200 OK", "application/json", &body);
        assert!(s.headers.contains("Content-Encoding: gzip\r\n"), "{:?}", s.headers);
        assert!(s.headers.contains("Vary: Accept-Encoding\r\n"));
        assert!(s.body.len() < body.len() / 2, "gzip {} of {}", s.body.len(), body.len());
        assert_eq!(gunzip(&s.body), body, "decodes to the same JSON");

        let s = shape(&local, "200 OK", "application/json", &body);
        assert!(!s.headers.contains("Content-Encoding"), "loopback stays plain: {:?}", s.headers);
        assert_eq!(s.body, body.as_bytes());

        // Remote without gzip in Accept-Encoding, or with q=0: plain.
        for ae in [&[][..], &["Accept-Encoding: br"][..], &["Accept-Encoding: gzip;q=0"][..]] {
            let r = Request::from_head(true, "GET", "/cli/projects/list", &head(ae));
            let s = shape(&r, "200 OK", "application/json", &body);
            assert!(!s.headers.contains("Content-Encoding"), "{ae:?}: {:?}", s.headers);
            assert_eq!(s.body, body.as_bytes(), "{ae:?}");
        }
    }

    #[test]
    fn gzip_skips_small_and_non_json_bodies() {
        let r = Request::from_head(true, "GET", "/cli/other", &head(&["Accept-Encoding: gzip"]));
        let small = r#"{"ok":true}"#;
        let s = shape(&r, "200 OK", "application/json", small);
        assert!(!s.headers.contains("Content-Encoding"));
        assert_eq!(s.body, small.as_bytes());
        let text = "x".repeat(4096);
        let s = shape(&r, "200 OK", "text/plain; charset=utf-8", &text);
        assert!(!s.headers.contains("Content-Encoding"));
        assert_eq!(s.body, text.as_bytes());
        // Any status: an error body over the floor is JSON too.
        let s = shape(&r, "400 Bad Request", "application/json", &big_json());
        assert!(s.headers.contains("Content-Encoding: gzip\r\n"));
    }

    #[test]
    fn etag_round_trip_answers_304_with_no_body() {
        let body = big_json();
        let first = Request::from_head(false, "GET", "/cli/projects/list?token=t", &head(&[]));
        let s = shape(&first, "200 OK", "application/json", &body);
        let tag = etag_of(&body);
        assert!(s.headers.contains(&format!("ETag: {tag}\r\n")), "{:?}", s.headers);
        assert!(s.headers.contains("Cache-Control: no-cache\r\n"));
        assert_eq!(s.body, body.as_bytes());

        let again = Request::from_head(false, "GET", "/cli/projects/list?token=t", &head(&[&format!("If-None-Match: {tag}")]));
        let s = shape(&again, "200 OK", "application/json", &body);
        assert_eq!(s.status, "304 Not Modified");
        assert!(!s.has_body);
        assert!(s.body.is_empty());
        assert!(s.headers.contains(&format!("ETag: {tag}\r\n")));

        // A changed list: a full 200 with the new tag.
        let changed = body.replace("workspace 7", "workspace seven");
        let s = shape(&again, "200 OK", "application/json", &changed);
        assert_eq!(s.status, "200 OK");
        assert_eq!(s.body, changed.as_bytes());
        assert!(s.headers.contains(&format!("ETag: {}\r\n", etag_of(&changed))));
    }

    #[test]
    fn etag_only_on_its_routes_and_on_200_get() {
        let body = big_json();
        let tag = etag_of(&body);
        let inm = format!("If-None-Match: {tag}");
        let other = Request::from_head(false, "GET", "/cli/projects/active", &head(&[&inm]));
        let s = shape(&other, "200 OK", "application/json", &body);
        assert_eq!(s.status, "200 OK");
        assert!(!s.headers.contains("ETag"));
        let post = Request::from_head(false, "POST", "/cli/projects/list", &head(&[&inm]));
        assert_eq!(shape(&post, "200 OK", "application/json", &body).status, "200 OK");
        let get = Request::from_head(false, "GET", "/cli/projects/list", &head(&[&inm]));
        let err = shape(&get, "403 Forbidden", "application/json", &body);
        assert_eq!(err.status, "403 Forbidden");
        assert!(!err.headers.contains("ETag"));
    }

    #[test]
    fn remote_304_and_gzip_together() {
        let body = big_json();
        let tag = etag_of(&body);
        let r = Request::from_head(
            true,
            "GET",
            "/cli/projects/list",
            &head(&["Accept-Encoding: gzip", &format!("If-None-Match: {tag}")]),
        );
        let s = shape(&r, "200 OK", "application/json", &body);
        assert_eq!(s.status, "304 Not Modified");
        assert!(!s.headers.contains("Content-Encoding"), "a 304 has no body to encode");
        let r = Request::from_head(true, "GET", "/cli/projects/list", &head(&["Accept-Encoding: gzip"]));
        let s = shape(&r, "200 OK", "application/json", &body);
        assert!(s.headers.contains(&format!("ETag: {tag}\r\n")), "the tag names the JSON, not its gzip bytes");
        assert_eq!(gunzip(&s.body), body);
    }

    #[test]
    fn outside_a_scope_nothing_changes() {
        assert_eq!(current(), Request::default());
        let s = shape(&current(), "200 OK", "application/json", &big_json());
        assert_eq!(s.body, big_json().as_bytes());
        assert!(s.headers.is_empty());
    }
}
