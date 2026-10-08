//! Who may frame a K2 web page (prd-app-frame-ancestors-v1).
//!
//! Clickjacking guard. Every K2 response says, by header, which origins may
//! put it in a frame:
//!
//! ```text
//! Content-Security-Policy: frame-ancestors <sources>
//! X-Frame-Options: SAMEORIGIN            (DENY when <sources> is 'none')
//! ```
//!
//! Two fixed policies plus one per-App setting:
//!
//! - **Connect** (the daemon's own pages and every daemon response, and the
//!   Connect web client Worker on `*.app.k2.dev`): `'self'` only. Nothing
//!   frames them (Q1).
//! - **App default** (the `--skin` gateway helper and the Caddy front door):
//!   `'self'` plus K2's desktop windows, `tauri://localhost` (macOS, Linux)
//!   and `http://tauri.localhost` (Windows WebView2). A host-source must
//!   carry its scheme: a bare `tauri.localhost` would take the page's own
//!   scheme (`https`) and never match. `'self'` never matches either Tauri
//!   origin.
//! - **Per App** (`published_services.frame_ancestors`, migration 0143,
//!   `k2 publish frame`): `''` = the App default, `none` = refuse every
//!   frame including K2's windows, otherwise a space-separated list of
//!   validated extra origins added to the App default.
//!
//! This module is the only place the policy text lives. The daemon, the
//! helper and the Caddy renderer read it from here. The Connect Worker
//! (k2-connect `edge/app-web/router.worker.js`) keeps a copy of
//! [`CONNECT_CSP`] and [`XFO_SAMEORIGIN`]; its `test-router.mjs` and the
//! `connect_worker_copy_is_pinned` test below hold the two equal.
//!
//! `X-Frame-Options` cannot name the Tauri origins (`ALLOW-FROM` is dead).
//! The HTML standard ignores XFO when an enforced CSP carries
//! `frame-ancestors`, so `SAMEORIGIN` is kept for old browsers and
//! scanners, even beside an allow-list ([`KEEP_XFO_WITH_ALLOW_LIST`],
//! FA19: proven by the positive embed test in `tests/frame/`).

use std::fmt;

/// K2's desktop window origin on macOS (WKWebView) and Linux (WebKitGTK).
pub const TAURI_ORIGIN: &str = "tauri://localhost";
/// K2's desktop window origin on Windows (WebView2, `useHttpsScheme` unset).
pub const TAURI_WINDOWS_ORIGIN: &str = "http://tauri.localhost";

/// `frame-ancestors` sources for a published App with no owner setting.
pub const APP_DEFAULT_SOURCES: &str = "'self' tauri://localhost http://tauri.localhost";
/// `frame-ancestors` sources for Connect surfaces (daemon, web client).
pub const CONNECT_SOURCES: &str = "'self'";
/// `frame-ancestors` sources that refuse every frame.
pub const NONE_SOURCES: &str = "'none'";

/// The CSP value the Connect Worker sets on every `*.app.k2.dev` response.
/// Copied by hand into k2-connect; pinned on both sides.
pub const CONNECT_CSP: &str = "frame-ancestors 'self'";

pub const XFO_SAMEORIGIN: &str = "SAMEORIGIN";
pub const XFO_DENY: &str = "DENY";

/// The two header lines (each ending `\r\n`) on every daemon response.
/// A literal so raw `format!` writers can splice it in; pinned to
/// [`FramePolicy::connect`] by a test.
pub const CONNECT_HEADER_LINES: &str =
    "Content-Security-Policy: frame-ancestors 'self'\r\nX-Frame-Options: SAMEORIGIN\r\n";

/// The App default as header lines. Pinned to [`FramePolicy::app_default`].
pub const APP_DEFAULT_HEADER_LINES: &str = "Content-Security-Policy: frame-ancestors 'self' tauri://localhost http://tauri.localhost\r\nX-Frame-Options: SAMEORIGIN\r\n";

/// Stored value for "refuse every frame" (`k2 publish frame <name> --none`).
pub const STORED_NONE: &str = "none";
/// Stored value for "the App default" (`--reset`).
pub const STORED_DEFAULT: &str = "";

/// At most this many extra origins per App (the default is not counted).
pub const MAX_EXTRA_ORIGINS: usize = 8;

/// FA19: keep `X-Frame-Options: SAMEORIGIN` beside an owner allow-list.
/// True because every engine we test ignores XFO once `frame-ancestors`
/// is present (`tests/frame/frame-ancestors.spec.ts`, Chromium + WebKit,
/// "allowed origin really embeds"). If a signed-build check finds an
/// engine that applies XFO anyway, flip this and the helper drops XFO for
/// any App whose list is wider than `'self'`.
pub const KEEP_XFO_WITH_ALLOW_LIST: bool = true;

/// Why an origin or policy was refused. Every variant is exit 2 at the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// One `--allow` value is not an acceptable origin.
    BadOrigin { origin: String, why: &'static str },
    /// More than [`MAX_EXTRA_ORIGINS`] extras.
    TooMany { count: usize },
    /// A stored value or helper `--frame-ancestors` argument that this
    /// module would never have written.
    BadPolicy(String),
}

impl FrameError {
    /// Stable machine code (`error.code` on the wire).
    pub fn code(&self) -> &'static str {
        match self {
            FrameError::BadOrigin { .. } => "bad_frame_origin",
            FrameError::TooMany { .. } => "frame_too_many",
            FrameError::BadPolicy(_) => "bad_frame_policy",
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::BadOrigin { origin, why } => {
                write!(f, "not an allowed frame origin: {origin:?}: {why}")
            }
            FrameError::TooMany { count } => write!(
                f,
                "too many frame origins ({count}); at most {MAX_EXTRA_ORIGINS} besides the default"
            ),
            FrameError::BadPolicy(v) => write!(f, "bad frame policy: {v:?}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// One response's framing policy: the `frame-ancestors` source list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePolicy {
    sources: String,
}

impl FramePolicy {
    /// `'self' tauri://localhost http://tauri.localhost`.
    pub fn app_default() -> Self {
        Self {
            sources: APP_DEFAULT_SOURCES.to_string(),
        }
    }

    /// `'self'` (daemon and Connect web client).
    pub fn connect() -> Self {
        Self {
            sources: CONNECT_SOURCES.to_string(),
        }
    }

    /// `'none'` + `DENY`.
    pub fn deny_all() -> Self {
        Self {
            sources: NONE_SOURCES.to_string(),
        }
    }

    /// The effective App policy for a stored `published_services.frame_ancestors`.
    pub fn from_stored(stored: &str) -> Result<Self, FrameError> {
        if stored == STORED_NONE {
            return Ok(Self::deny_all());
        }
        let extras = extras_of(stored)?;
        let mut sources = APP_DEFAULT_SOURCES.to_string();
        for e in extras {
            sources.push(' ');
            sources.push_str(&e);
        }
        Ok(Self { sources })
    }

    /// Parse the helper's `--frame-ancestors` value. Accepts exactly what
    /// [`FramePolicy::sources`] of an App policy can produce: `'none'`, or
    /// the App default followed by validated, distinct extras. Anything
    /// else is refused (the helper then refuses to start).
    pub fn from_sources(arg: &str) -> Result<Self, FrameError> {
        if arg == NONE_SOURCES {
            return Ok(Self::deny_all());
        }
        let Some(rest) = arg.strip_prefix(APP_DEFAULT_SOURCES) else {
            return Err(FrameError::BadPolicy(arg.to_string()));
        };
        if rest.is_empty() {
            return Ok(Self::app_default());
        }
        let Some(list) = rest.strip_prefix(' ') else {
            return Err(FrameError::BadPolicy(arg.to_string()));
        };
        // Same rules as a stored list; `extras_of` re-validates each one.
        let policy = Self::from_stored(list)?;
        if policy.sources != arg {
            return Err(FrameError::BadPolicy(arg.to_string()));
        }
        Ok(policy)
    }

    /// The `frame-ancestors` source list.
    pub fn sources(&self) -> &str {
        &self.sources
    }

    pub fn is_deny_all(&self) -> bool {
        self.sources == NONE_SOURCES
    }

    /// `frame-ancestors <sources>`.
    pub fn csp_value(&self) -> String {
        format!("frame-ancestors {}", self.sources)
    }

    /// `DENY` for `'none'`, otherwise `SAMEORIGIN`; `None` when the
    /// allow-list is wider than `'self'` and [`KEEP_XFO_WITH_ALLOW_LIST`]
    /// is off.
    pub fn xfo_value(&self) -> Option<&'static str> {
        if self.is_deny_all() {
            return Some(XFO_DENY);
        }
        if !KEEP_XFO_WITH_ALLOW_LIST && self.sources != CONNECT_SOURCES {
            return None;
        }
        Some(XFO_SAMEORIGIN)
    }

    /// `Content-Security-Policy: …\r\nX-Frame-Options: …\r\n`.
    pub fn header_lines(&self) -> String {
        let mut out = format!("Content-Security-Policy: {}\r\n", self.csp_value());
        if let Some(xfo) = self.xfo_value() {
            out.push_str("X-Frame-Options: ");
            out.push_str(xfo);
            out.push_str("\r\n");
        }
        out
    }
}

/// The extras in a stored value. `''` and `none` have none. Every entry
/// must already be in normalized form, distinct, and at most
/// [`MAX_EXTRA_ORIGINS`] long.
pub fn extras_of(stored: &str) -> Result<Vec<String>, FrameError> {
    if stored == STORED_DEFAULT || stored == STORED_NONE {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    for tok in stored.split(' ') {
        let norm = normalize_origin(tok)?;
        if norm != tok || out.contains(&norm) {
            return Err(FrameError::BadPolicy(stored.to_string()));
        }
        out.push(norm);
    }
    if out.len() > MAX_EXTRA_ORIGINS {
        return Err(FrameError::TooMany { count: out.len() });
    }
    Ok(out)
}

/// `--allow`: add origins to a stored value. Additive and de-duplicated;
/// after `none` it starts again from the default. Returns the new stored
/// value. Fails on the first bad origin or past the cap.
pub fn stored_with_allow(stored: &str, adds: &[String]) -> Result<String, FrameError> {
    let mut list = extras_of(stored)?;
    for raw in adds {
        let norm = normalize_origin(raw)?;
        if !list.contains(&norm) {
            list.push(norm);
        }
    }
    if list.len() > MAX_EXTRA_ORIGINS {
        return Err(FrameError::TooMany { count: list.len() });
    }
    Ok(list.join(" "))
}

/// Validate and normalize one owner-supplied origin.
///
/// - `https://host[:port]`; `http://` only for `localhost` / `127.0.0.1`.
/// - No path (one trailing `/` is dropped), query, fragment, userinfo,
///   quotes, `;`, `,`, whitespace or control characters (header injection).
/// - No `*` anywhere, no bare scheme, no IP-literal brackets, no non-ASCII.
/// - Not K2's own window origins (already in the default).
/// - Lowercased; a default port (`:443` / `:80`) is dropped.
pub fn normalize_origin(raw: &str) -> Result<String, FrameError> {
    let bad = |why: &'static str| FrameError::BadOrigin {
        origin: raw.to_string(),
        why,
    };
    if raw.is_empty() {
        return Err(bad("empty"));
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(bad("whitespace or control characters"));
    }
    if !raw.is_ascii() {
        return Err(bad("use the punycode (xn--) form of the host"));
    }
    if raw.contains('*') {
        return Err(bad("wildcards are not allowed; name each origin"));
    }
    if raw.chars().any(|c| matches!(c, '\'' | '"' | ';' | ',' | '\\' | '`' | '<' | '>')) {
        return Err(bad("quotes, ';' and ',' are not allowed"));
    }
    let lower = raw.to_ascii_lowercase();
    let (scheme, rest) = match lower.split_once("://") {
        Some((s, r)) => (s.to_string(), r.to_string()),
        None => return Err(bad("write the full origin, e.g. https://partner.example")),
    };
    if scheme != "https" && scheme != "http" {
        return Err(bad("only https:// origins (http:// only for localhost)"));
    }
    let rest = rest.strip_suffix('/').unwrap_or(&rest).to_string();
    if rest.is_empty() {
        return Err(bad("missing host"));
    }
    if rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err(bad("an origin has no path, query or fragment"));
    }
    if rest.contains('@') {
        return Err(bad("no user name or password"));
    }
    if rest.contains('[') || rest.contains(']') {
        return Err(bad("IP-literal hosts are not supported"));
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), Some(p.to_string())),
        None => (rest.clone(), None),
    };
    if host.is_empty() {
        return Err(bad("missing host"));
    }
    let labels_ok = host.split('.').all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    if !labels_ok {
        return Err(bad("not a valid host name"));
    }
    let port = match port {
        None => None,
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("port must be 1-65535"));
            }
            let n: u32 = p.parse().map_err(|_| bad("port must be 1-65535"))?;
            if n == 0 || n > 65_535 {
                return Err(bad("port must be 1-65535"));
            }
            Some(n)
        }
    };
    let loopback_host = host == "localhost" || host == "127.0.0.1";
    if scheme == "http" && !loopback_host {
        return Err(bad("http:// is only for localhost or 127.0.0.1 (dev); use https://"));
    }
    if host == "tauri.localhost" || host.ends_with(".tauri.localhost") {
        return Err(bad("K2's own windows are already allowed"));
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    let mut out = format!("{scheme}://{host}");
    if let Some(p) = port {
        if p != default_port {
            out.push(':');
            out.push_str(&p.to_string());
        }
    }
    Ok(out)
}

/// True when a `Content-Security-Policy` header value carries a
/// `frame-ancestors` directive (any sources). Used by the `--cmd` App
/// probe: such an App protects itself.
pub fn csp_has_frame_ancestors(value: &str) -> bool {
    value.split(';').any(|d| {
        d.trim()
            .split_ascii_whitespace()
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_strings_are_exact() {
        assert_eq!(
            FramePolicy::app_default().csp_value(),
            "frame-ancestors 'self' tauri://localhost http://tauri.localhost"
        );
        assert_eq!(FramePolicy::app_default().xfo_value(), Some("SAMEORIGIN"));
        assert_eq!(FramePolicy::connect().csp_value(), "frame-ancestors 'self'");
        assert_eq!(FramePolicy::connect().xfo_value(), Some("SAMEORIGIN"));
        assert_eq!(FramePolicy::deny_all().csp_value(), "frame-ancestors 'none'");
        assert_eq!(FramePolicy::deny_all().xfo_value(), Some("DENY"));
        assert_eq!(
            APP_DEFAULT_SOURCES,
            format!("'self' {TAURI_ORIGIN} {TAURI_WINDOWS_ORIGIN}")
        );
    }

    #[test]
    fn header_line_literals_match_the_policies() {
        assert_eq!(CONNECT_HEADER_LINES, FramePolicy::connect().header_lines());
        assert_eq!(APP_DEFAULT_HEADER_LINES, FramePolicy::app_default().header_lines());
        assert_eq!(
            FramePolicy::deny_all().header_lines(),
            "Content-Security-Policy: frame-ancestors 'none'\r\nX-Frame-Options: DENY\r\n"
        );
    }

    /// The Connect Worker (k2-connect `edge/app-web/router.worker.js`)
    /// hard-codes these two strings. Changing either here means changing
    /// the Worker and its `test-router.mjs` pin in the same release.
    #[test]
    fn connect_worker_copy_is_pinned() {
        assert_eq!(CONNECT_CSP, "frame-ancestors 'self'");
        assert_eq!(XFO_SAMEORIGIN, "SAMEORIGIN");
        assert_eq!(CONNECT_CSP, FramePolicy::connect().csp_value());
    }

    #[test]
    fn stored_values_map_to_policies() {
        assert_eq!(FramePolicy::from_stored("").unwrap(), FramePolicy::app_default());
        assert_eq!(FramePolicy::from_stored("none").unwrap(), FramePolicy::deny_all());
        let p = FramePolicy::from_stored("https://a.example https://b.example:8443").unwrap();
        assert_eq!(
            p.sources(),
            "'self' tauri://localhost http://tauri.localhost https://a.example https://b.example:8443"
        );
        assert_eq!(p.xfo_value(), Some("SAMEORIGIN"));
        // Not normalized, duplicate, or junk: refused.
        for bad in [
            "HTTPS://A.example",
            "https://a.example https://a.example",
            "https://a.example  https://b.example",
            " https://a.example",
            "https://a.example/",
            "'none'",
            "NONE",
        ] {
            assert!(FramePolicy::from_stored(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn helper_argument_round_trips_and_refuses_anything_else() {
        for stored in ["", "none", "https://a.example", "https://a.example http://localhost:5173"] {
            let p = FramePolicy::from_stored(stored).unwrap();
            assert_eq!(FramePolicy::from_sources(p.sources()).unwrap(), p, "{stored:?}");
        }
        for bad in [
            "",
            "'self'",
            "*",
            "'self' tauri://localhost http://tauri.localhost ",
            "'self' tauri://localhost http://tauri.localhost https://*.k2.dev",
            "'self' tauri://localhost http://tauri.localhost\r\nSet-Cookie: x=1",
            "'self' tauri://localhost http://tauri.localhost https://A.example",
            "'self' tauri://localhost http://tauri.localhost 'none'",
            "https://a.example",
            "'none' https://a.example",
        ] {
            assert!(FramePolicy::from_sources(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn origin_validation_refuses_with_named_errors() {
        let cases: &[(&str, &str)] = &[
            ("", "empty"),
            ("https://a.example\r\nX-Evil: 1", "whitespace or control characters"),
            ("https://a.example\n", "whitespace or control characters"),
            ("https://a .example", "whitespace or control characters"),
            ("*", "wildcards are not allowed; name each origin"),
            ("https://*.k2.dev", "wildcards are not allowed; name each origin"),
            ("https://*.example", "wildcards are not allowed; name each origin"),
            ("https:", "write the full origin, e.g. https://partner.example"),
            ("partner.example", "write the full origin, e.g. https://partner.example"),
            ("'self'", "quotes, ';' and ',' are not allowed"),
            ("https://a.example;frame-ancestors *", "whitespace or control characters"),
            ("https://a.example;x", "quotes, ';' and ',' are not allowed"),
            ("https://a.example,https://b.example", "quotes, ';' and ',' are not allowed"),
            ("ftp://a.example", "only https:// origins (http:// only for localhost)"),
            ("tauri://localhost", "only https:// origins (http:// only for localhost)"),
            ("http://partner.example", "http:// is only for localhost or 127.0.0.1 (dev); use https://"),
            ("http://tauri.localhost", "http:// is only for localhost or 127.0.0.1 (dev); use https://"),
            ("https://tauri.localhost", "K2's own windows are already allowed"),
            ("https://a.example/path", "an origin has no path, query or fragment"),
            ("https://a.example?x=1", "an origin has no path, query or fragment"),
            ("https://a.example#f", "an origin has no path, query or fragment"),
            ("https://user@a.example", "no user name or password"),
            ("https://[::1]:8080", "IP-literal hosts are not supported"),
            ("https://a..example", "not a valid host name"),
            ("https://-a.example", "not a valid host name"),
            ("https://a_b.example", "not a valid host name"),
            ("https://a.example:0", "port must be 1-65535"),
            ("https://a.example:70000", "port must be 1-65535"),
            ("https://a.example:", "port must be 1-65535"),
            ("https://a.example:8x", "port must be 1-65535"),
            ("https://bücher.example", "use the punycode (xn--) form of the host"),
            ("https://", "missing host"),
        ];
        for (input, why) in cases {
            match normalize_origin(input) {
                Err(FrameError::BadOrigin { why: got, .. }) => {
                    assert_eq!(got, *why, "{input:?}")
                }
                other => panic!("{input:?}: expected BadOrigin({why}), got {other:?}"),
            }
            assert_eq!(
                normalize_origin(input).unwrap_err().code(),
                "bad_frame_origin",
                "{input:?}"
            );
        }
    }

    #[test]
    fn origin_normalization() {
        let ok: &[(&str, &str)] = &[
            ("https://partner.example", "https://partner.example"),
            ("HTTPS://Partner.Example/", "https://partner.example"),
            ("https://partner.example:443", "https://partner.example"),
            ("https://partner.example:8443", "https://partner.example:8443"),
            ("http://localhost:5173", "http://localhost:5173"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://localhost:80", "http://localhost"),
            ("https://other.rosson.k2.dev", "https://other.rosson.k2.dev"),
            ("https://xn--bcher-kva.example", "https://xn--bcher-kva.example"),
        ];
        for (input, want) in ok {
            assert_eq!(normalize_origin(input).as_deref(), Ok(*want), "{input:?}");
        }
    }

    #[test]
    fn allow_is_additive_dedups_restarts_after_none_and_caps() {
        let s = stored_with_allow("", &["https://a.example".into()]).unwrap();
        assert_eq!(s, "https://a.example");
        let s = stored_with_allow(&s, &["HTTPS://A.EXAMPLE/".into(), "https://b.example".into()])
            .unwrap();
        assert_eq!(s, "https://a.example https://b.example");
        let s = stored_with_allow(STORED_NONE, &["https://c.example".into()]).unwrap();
        assert_eq!(s, "https://c.example", "after --none, --allow starts from the default");
        let eight: Vec<String> = (1..=8).map(|i| format!("https://o{i}.example")).collect();
        let s = stored_with_allow("", &eight).unwrap();
        assert_eq!(extras_of(&s).unwrap().len(), 8);
        assert_eq!(
            stored_with_allow(&s, &["https://o9.example".into()]),
            Err(FrameError::TooMany { count: 9 })
        );
        assert_eq!(
            stored_with_allow(&s, &["https://o9.example".into()]).unwrap_err().code(),
            "frame_too_many"
        );
        // A re-add of an existing one does not count against the cap.
        assert_eq!(stored_with_allow(&s, &["https://o1.example".into()]).unwrap(), s);
        // One bad origin refuses the whole call.
        assert!(stored_with_allow("", &["https://ok.example".into(), "*".into()]).is_err());
    }

    #[test]
    fn csp_frame_ancestors_detection() {
        assert!(csp_has_frame_ancestors("frame-ancestors 'self'"));
        assert!(csp_has_frame_ancestors("default-src 'self'; Frame-Ancestors https://a"));
        assert!(!csp_has_frame_ancestors("default-src 'self'"));
        assert!(!csp_has_frame_ancestors("default-src frame-ancestors"));
        assert!(!csp_has_frame_ancestors(""));
    }
}
