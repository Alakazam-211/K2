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
//!   say "unchanged" about a list that changed, whatever changed it. The
//!   tag may also come as the `if_none_match` query parameter: a header a
//!   web page sets itself makes the browser ask first (a CORS preflight,
//!   one more round trip), a query parameter does not.
//! - **Icon ETag / 304** on the routes in [`ICON_ETAG_ROUTES`]
//!   (`project-icons-rev-v1`): a 200 JSON answer with an `iconRev` carries
//!   the strong `ETag: "<iconRev>"`, so a phone that holds the list's
//!   `iconRev` already knows the tag, and a matching `If-None-Match` gets
//!   `304 Not Modified` with no body. No icon (`iconRev` null): no tag.
//! - **Repeated query keys** ([`Request::repeated`]): the parsed params keep
//!   one value per key, so a handler that refuses a repeated key asks here.
//!
//! The dispatcher notes each request ([`note`]) inside the connection's
//! [`scope`]; the writers in `routes/http.rs` call [`shape`]. Outside a
//! scope (the cell socket, tests calling a writer directly) nothing changes.

use std::cell::RefCell;
use std::future::Future;
use std::io::Write as _;

/// Routes whose 200 JSON answers carry an ETag and may answer 304.
pub const ETAG_ROUTES: &[&str] = &["/cli/projects/list"];

/// Routes whose 200 JSON answers carry `ETag: "<iconRev>"` and may answer
/// 304 (`project-icons-rev-v1`).
pub const ICON_ETAG_ROUTES: &[&str] = &["/cli/projects/get-icon", "/cli/project-group/icon"];

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
    /// A GET of a route in [`ICON_ETAG_ROUTES`].
    pub icon_etag: bool,
    /// The request's `If-None-Match` header, else its `if_none_match`
    /// query parameter.
    pub if_none_match: Option<String>,
    /// A `/cli/*` request on this machine: its answer says the daemon's
    /// version (`X-K5-Server-Version`, R1 slice 10). Never off the machine,
    /// where a ghost `/boot-status` hides the version from strangers.
    pub server_version: bool,
    /// Query keys sent more than once, in first-seen order (raw, as the
    /// params map keys them).
    pub repeated: Vec<String>,
}

impl Request {
    /// `remote`: the request's ingress is off this machine. `head` is the
    /// request head (request line and headers).
    pub fn from_head(remote: bool, method: &str, path_and_query: &str, head: &str) -> Request {
        let path = path_and_query.split('?').next().unwrap_or("");
        let get = method.eq_ignore_ascii_case("GET");
        let etag = get && ETAG_ROUTES.contains(&path);
        let icon_etag = get && ICON_ETAG_ROUTES.contains(&path);
        Request {
            gzip: remote && crate::static_files::accepts(header_value(head, "accept-encoding"), "gzip"),
            remote,
            etag,
            icon_etag,
            if_none_match: if etag || icon_etag {
                header_value(head, "if-none-match").map(str::to_string).or_else(|| {
                    k2_core::agent_hooks::parse_query_params(path_and_query)
                        .remove("if_none_match")
                        .filter(|v| !v.is_empty())
                })
            } else {
                None
            },
            server_version: !remote && path.starts_with("/cli/"),
            repeated: repeated_keys(path_and_query),
        }
    }

    /// Was the query key `key` sent more than once?
    pub fn repeats(&self, key: &str) -> bool {
        self.repeated.iter().any(|k| k == key)
    }

    /// Keep the version header only when `on_this_machine` (the request
    /// came over loopback, not through a proxy, the LAN, the tunnel or an
    /// App's link).
    pub fn server_version(mut self, on_this_machine: bool) -> Request {
        self.server_version &= on_this_machine;
        self
    }
}

/// The keys of `path_and_query`'s query that appear more than once (a
/// pair with no `=` is not a key, as in the params parser).
fn repeated_keys(path_and_query: &str) -> Vec<String> {
    let Some((_, query)) = path_and_query.split_once('?') else { return Vec::new() };
    let mut seen: Vec<&str> = Vec::new();
    let mut repeated: Vec<String> = Vec::new();
    for key in query.split('&').filter_map(|pair| pair.split_once('=').map(|(k, _)| k)) {
        if seen.contains(&key) {
            if !repeated.iter().any(|r| r == key) {
                repeated.push(key.to_string());
            }
        } else {
            seen.push(key);
        }
    }
    repeated
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

/// The weak ETag of a JSON answer: the first 32 hex digits of the SHA-256
/// of its bytes. A web client works out the same tag from the list it holds
/// (`src/renderer/lib/projects-list-cache.ts`, `projectsListTag`), so it
/// needs no response header to ask "changed?" next time.
pub fn etag_of(body: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(body.as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// The strong ETag of an icon answer: `"<iconRev>"` from the body's
/// top-level `iconRev`; `None` when it is null, empty or absent.
pub fn icon_etag_of(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let rev = v.get("iconRev")?.as_str().filter(|r| !r.is_empty())?;
    Some(format!("\"{rev}\""))
}

/// Does `If-None-Match` name the icon tag `tag`? Each comma-separated entry
/// may be the bare rev (`abc`), quoted (`"abc"`) or weak (`W/"abc"`); `*`
/// matches any tag.
pub fn icon_not_modified(if_none_match: Option<&str>, tag: &str) -> bool {
    let Some(inm) = if_none_match else { return false };
    let bare = |t: &str| t.trim().trim_start_matches("W/").trim_matches('"').to_string();
    let want = bare(tag);
    !want.is_empty() && inm.split(',').any(|t| t.trim() == "*" || bare(t) == want)
}

fn is_json(ct: &str) -> bool {
    ct.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
}

/// Shape one answer for `req`: maybe a 304, maybe gzip, else as it is.
pub fn shape(req: &Request, status: &str, ct: &str, body: &str) -> Shaped {
    let json = is_json(ct);
    let ok = status.starts_with("200");
    let mut headers = String::new();
    if req.server_version {
        headers.push_str(crate::cli_usage::SERVER_VERSION_HEADER);
    }
    let tag = match (ok && json, req.etag, req.icon_etag) {
        (true, true, _) => Some((etag_of(body), false)),
        (true, _, true) => icon_etag_of(body).map(|t| (t, true)),
        _ => None,
    };
    if let Some((tag, icon)) = tag {
        headers.push_str(&format!("ETag: {tag}\r\nCache-Control: no-cache\r\n"));
        let inm = req.if_none_match.as_deref();
        let unchanged =
            if icon { icon_not_modified(inm, &tag) } else { crate::static_files::not_modified(inm, &tag) };
        if unchanged {
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

    /// A web page sends the tag as a query parameter (no preflight).
    #[test]
    fn etag_from_the_query_parameter() {
        let body = big_json();
        let tag = etag_of(&body);
        let enc: String = tag.bytes().map(|b| format!("%{b:02X}")).collect();
        let pq = format!("/cli/projects/list?token=t&if_none_match={enc}");
        let r = Request::from_head(false, "GET", &pq, &head(&[]));
        assert_eq!(r.if_none_match.as_deref(), Some(tag.as_str()));
        assert_eq!(shape(&r, "200 OK", "application/json", &body).status, "304 Not Modified");
        let stale = Request::from_head(false, "GET", "/cli/projects/list?if_none_match=W%2F%22old%22", &head(&[]));
        assert_eq!(shape(&stale, "200 OK", "application/json", &body).status, "200 OK");
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

    /// The renderer's `projectsListTag` test holds the same fixture and tag:
    /// the two must agree, or a web client never gets a 304.
    #[test]
    fn etag_matches_the_renderer_fixture() {
        let body = "[{\"id\":\"p1\",\"name\":\"Caf\u{e9} \\\"one\\\"\",\"tabOrder\":3,\"iconUrl\":null,\"workspaces\":[]}]";
        assert_eq!(etag_of(body), "W/\"a9de4e57e561bbb8bdc197b3faee8112\"");
    }

    /// R1 slice 10: a `/cli/*` answer on this machine names the daemon's
    /// version, whatever its status or type; nothing else does.
    #[test]
    fn a_cli_answer_on_this_machine_says_the_daemon_version() {
        let line = format!("X-K5-Server-Version: {}\r\n", env!("CARGO_PKG_VERSION"));
        let cli = Request::from_head(false, "GET", "/cli/inbox/list?token=t", &head(&[])).server_version(true);
        for (status, ct, body) in
            [("200 OK", "application/json", "{}"), ("403 Forbidden", "application/json", "{}"), ("200 OK", "text/plain", "x")]
        {
            let s = shape(&cli, status, ct, body);
            assert!(s.headers.contains(&line), "{status} {ct}: {:?}", s.headers);
            assert_eq!(s.headers.matches("X-K5-Server-Version").count(), 1);
        }
        // A 304 too.
        let body = big_json();
        let tag = etag_of(&body);
        let again = Request::from_head(false, "GET", "/cli/projects/list", &head(&[&format!("If-None-Match: {tag}")]))
            .server_version(true);
        let s = shape(&again, "200 OK", "application/json", &body);
        assert_eq!(s.status, "304 Not Modified");
        assert!(s.headers.contains(&line), "{:?}", s.headers);
        // Not a /cli/* route; off this machine (LAN or tunnel, or an App's
        // link: loopback but not this machine's CLI); outside a scope (the
        // cell socket writes its own).
        let not_cli = Request::from_head(false, "GET", "/boot-status", &head(&[])).server_version(true);
        let remote = Request::from_head(true, "GET", "/cli/inbox/list", &head(&[])).server_version(true);
        let app_link = Request::from_head(false, "GET", "/cli/inbox/list", &head(&[])).server_version(false);
        for r in [not_cli, remote, app_link, Request::default()] {
            let s = shape(&r, "200 OK", "application/json", "{}");
            assert!(!s.headers.contains("X-K5-Server-Version"), "{r:?}: {:?}", s.headers);
        }
    }

    /// `project-icons-rev-v1`: an icon answer's tag is its `iconRev`; a
    /// matching `If-None-Match` (header or query) is a 304 with no body, a
    /// stale one the full answer.
    #[test]
    fn icon_routes_tag_their_rev_and_answer_304() {
        let body = r#"{"found":true,"dataUrl":"data:image/png;base64,AAAA","iconRev":"0123456789abcdef"}"#;
        let inm = "If-None-Match: \"0123456789abcdef\"";
        for path in ICON_ETAG_ROUTES {
            let first = Request::from_head(false, "GET", &format!("{path}?token=t"), &head(&[]));
            assert!(first.icon_etag && !first.etag, "{path}");
            let s = shape(&first, "200 OK", "application/json", body);
            assert!(s.headers.contains("ETag: \"0123456789abcdef\"\r\n"), "{path}: {:?}", s.headers);
            assert!(s.headers.contains("Cache-Control: no-cache\r\n"));
            assert_eq!(s.body, body.as_bytes());

            let again = Request::from_head(false, "GET", &format!("{path}?token=t"), &head(&[inm]));
            let s = shape(&again, "200 OK", "application/json", body);
            assert_eq!(s.status, "304 Not Modified", "{path}");
            assert!(!s.has_body);
            assert!(s.body.is_empty());
            assert!(s.headers.contains("ETag: \"0123456789abcdef\"\r\n"));

            let q = Request::from_head(false, "GET", &format!("{path}?if_none_match=%220123456789abcdef%22"), &head(&[]));
            assert_eq!(shape(&q, "200 OK", "application/json", body).status, "304 Not Modified", "{path}");

            let stale = Request::from_head(false, "GET", path, &head(&["If-None-Match: \"ffffffffffffffff\""]));
            let s = shape(&stale, "200 OK", "application/json", body);
            assert_eq!(s.status, "200 OK", "{path}");
            assert_eq!(s.body, body.as_bytes());

            // No icon: no tag, and an If-None-Match never makes it a 304.
            let none = r#"{"found":false,"dataUrl":null,"iconRev":null}"#;
            let s = shape(&again, "200 OK", "application/json", none);
            assert_eq!(s.status, "200 OK");
            assert!(!s.headers.contains("ETag"), "{:?}", s.headers);
            // A POST or an error answer: never.
            let post = Request::from_head(false, "POST", path, &head(&[inm]));
            assert_eq!(shape(&post, "200 OK", "application/json", body).status, "200 OK");
            let err = shape(&again, "400 Bad Request", "application/json", body);
            assert_eq!(err.status, "400 Bad Request");
            assert!(!err.headers.contains("ETag"));
        }
        // The list keeps its own whole-body tag.
        let list = Request::from_head(false, "GET", "/cli/projects/list", &head(&[inm]));
        assert!(list.etag && !list.icon_etag);
        assert_eq!(shape(&list, "200 OK", "application/json", body).status, "200 OK");
        assert_eq!(icon_etag_of(r#"{"iconRev":""}"#), None);
        assert_eq!(icon_etag_of("not json"), None);
    }

    /// The phone sends the rev bare, quoted, weak, or in a list.
    #[test]
    fn icon_if_none_match_takes_bare_quoted_weak_and_lists() {
        let body = r#"{"found":true,"dataUrl":"data:image/png;base64,AAAA","iconRev":"abc0123456789def"}"#;
        for inm in [
            "abc0123456789def",
            "\"abc0123456789def\"",
            "W/\"abc0123456789def\"",
            "\"old\", abc0123456789def",
            "W/\"old\" , W/\"abc0123456789def\"",
            "x,\"y\",\"abc0123456789def\"",
            "*",
        ] {
            for path in ICON_ETAG_ROUTES {
                let r = Request::from_head(false, "GET", path, &head(&[&format!("If-None-Match: {inm}")]));
                let s = shape(&r, "200 OK", "application/json", body);
                assert_eq!(s.status, "304 Not Modified", "{path} If-None-Match: {inm}");
                assert!(s.body.is_empty() && !s.has_body, "{inm}");
            }
        }
        for inm in ["abc0123456789de", "\"abc0123456789deff\"", "W/\"other\"", "\"\"", ""] {
            let r = Request::from_head(false, "GET", ICON_ETAG_ROUTES[0], &head(&[&format!("If-None-Match: {inm}")]));
            let s = shape(&r, "200 OK", "application/json", body);
            assert_eq!(s.status, "200 OK", "If-None-Match: {inm}");
            assert_eq!(s.body, body.as_bytes(), "{inm}");
        }
        assert!(icon_not_modified(Some("abc"), "\"abc\""));
        assert!(!icon_not_modified(None, "\"abc\""));
        assert!(!icon_not_modified(Some("*"), "\"\""), "no rev, no match");
    }

    #[test]
    fn repeated_query_keys_are_noted() {
        let r = Request::from_head(false, "GET", "/cli/projects/list?token=t&icons=rev&a=1&icons=rev&a=2&icons=x", &head(&[]));
        assert_eq!(r.repeated, vec!["icons".to_string(), "a".to_string()]);
        assert!(r.repeats("icons"));
        assert!(!r.repeats("token"));
        let once = Request::from_head(false, "GET", "/cli/projects/list?token=t&icons=rev&iconsx=1&flag", &head(&[]));
        assert!(once.repeated.is_empty(), "{:?}", once.repeated);
        assert!(Request::from_head(false, "GET", "/cli/projects/list", &head(&[])).repeated.is_empty());
    }

    #[test]
    fn outside_a_scope_nothing_changes() {
        assert_eq!(current(), Request::default());
        let s = shape(&current(), "200 OK", "application/json", &big_json());
        assert_eq!(s.body, big_json().as_bytes());
        assert!(s.headers.is_empty());
    }
}
