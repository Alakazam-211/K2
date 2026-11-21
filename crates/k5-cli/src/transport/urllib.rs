//! The shell CLI's embedded Python HTTP clients, as one adapter: what
//! `urllib.request.urlopen(k5_request(req), timeout=t)` sends and what it
//! hands back, for the families whose shell side is a `python3` block
//! (`_db_py`, `_cal_py`, `_mail_py`, `_project_py`, `_preset_py`,
//! feedback's, agent's and clone's `call`). K2's ruling D4: no new crate,
//! the binary's own loopback transport ([`super::wire`]'s address rules),
//! with Python's request and Python's exceptions.
//!
//! # The request
//!
//! Always TCP to the shell's `BASE_URL` (urllib has no cell socket):
//! `http://HOST:PORT`, never a proxy (`_K2_PY_PROXY_RULE`: plain http is
//! direct), or, for `HOST=https://<name>`, TLS to `<name>` on the port
//! ([`super::tls`], K2's ruling D5), as the shell's Python opens its
//! `https://` `BASE_URL`. Where that Python would go through the person's
//! proxy, or the shared transport doesn't read the URL, nothing is sent
//! ([`Opener::open`]): never a plain or direct request in its place.
//! Written as Python 3's `http.client` writes it for urllib:
//!
//! ```text
//! <METHOD> <route>[?<urlencode(query)>] HTTP/1.1
//! Accept-Encoding: identity
//! Content-Length: <n>                         (a body only)
//! Host: <host>:<port>
//! User-Agent: <ua>                            (Python-urllib/3.x in the shell)
//! <the caller's headers, in the order its Python adds them>
//! <the hand-over headers>
//! Connection: close
//!
//! <body>
//! ```
//!
//! A POST with no data (`data=None`) gets http.client's own
//! `Content-Length: 0`, right after `Accept-Encoding`.
//!
//! A body with no `Content-Type` of the caller's gets urllib's own
//! `Content-Type: application/x-www-form-urlencoded`, just before
//! `Content-Length` (`agent`'s `data=b""` POSTs). The hand-over headers
//! are the native ones, in the shell's place (`k5_request()` adds them
//! last): `X-K5-Would-Refuse: 1` when the passport rule would refuse,
//! `X-K5-Cli-Version`, `X-K5-Cli-Via: native`, and `X-K5-Owner-Source:
//! disk` on a call made with the owner token from disk, then a Codex
//! call's `X-K2-Conversation` and `X-K2-Thread`
//! ([`super::client::codex_headers`]). The shell's `X-K5-Cli-Noun` is
//! the shell's only, as for curl's calls.
//!
//! The credential is the caller's to add (`if token:
//! req.add_header("Authorization", "Bearer " + token)`), from
//! [`Opener::token`]: the TCP token of the client it was made from (the
//! passport, else the owner token, else none), which is the shell's
//! `$TOKEN` after `_cli_apply_auth_for_verb` for an `id` tool.
//!
//! # The answer
//!
//! Read as `http.client` reads it: a `100 Continue` head skipped, the
//! status line, the headers, then the body by `Transfer-Encoding:
//! chunked`, else `Content-Length`, else to the end of the stream (none
//! for 1xx, 204 and 304). A status in 200..=299 is [`Outcome::Answer`]
//! (`urlopen` returned); any other is [`Outcome::HttpError`] (urlopen
//! raised `HTTPError`; `e.code`, `e.read()`). The body is read after the
//! head, as `r.read()` and `e.read()` read it, so a failure there is the
//! body's ([`Body`]), not urlopen's.
//!
//! # The exceptions
//!
//! What fails is a [`Raised`]: its Python class, `str(e)` and the kind
//! the shell's `_is_timeout` and `_restart_kind` walk to:
//!
//! | what happened | raised | `str(e)` | kind |
//! |---|---|---|---|
//! | nothing accepted the connect | `URLError(ConnectionRefusedError)` | `<urlopen error [Errno 111] Connection refused>` | refused |
//! | no connect in time | `URLError(TimeoutError)` | `<urlopen error timed out>` | timeout |
//! | the name doesn't resolve | `URLError(gaierror)` | `<urlopen error [Errno -2] Name or service not known>` | other |
//! | the request couldn't be written | `URLError(OSError)` | `<urlopen error [Errno 32] Broken pipe>` | dropped |
//! | closed with nothing | `RemoteDisconnected` | `Remote end closed connection without response` | dropped |
//! | reset | `ConnectionResetError` | `[Errno 104] Connection reset by peer` | dropped |
//! | no answer in time | `TimeoutError` | `timed out` | timeout |
//! | not HTTP | `BadStatusLine` | the first line as it came | other |
//! | a body cut short | `IncompleteRead` | `IncompleteRead(5 bytes read, 10 more expected)` | dropped |
//!
//! The errno numbers and texts are this system's (`ECONNREFUSED` is 111 on
//! Linux and 61 on macOS), as Python prints them.
//!
//! # Time
//!
//! `timeout` is Python's socket timeout: the connect, and each send and
//! each read on its own, not the whole exchange.
//!
//! # Tries
//!
//! [`Opener::open`] is one `urlopen`: no tries, no heal, no permission
//! note (each Python prints its own). [`Opener::open_retrying`] is the
//! loop `_mail_py` and `_cal_py` put around it (k2-ptyd S5).
//!
//! # Where this differs from Python, on purpose
//!
//! - [`Opener::open`] and [`Opener::open_retrying`] hand a redirect back
//!   as an [`Outcome::HttpError`]; plain urllib would follow it.
//!
//! # Redirects
//!
//! Every shell block runs `_K2_PY_PROXY_RULE` first, whose
//! `_K2NoRedirect` follows no redirect: a 3xx urllib's handler would
//! follow is refused with one line on stderr and `SystemExit(3)`, and
//! nothing goes to the place it names. [`Opener::open_ruled`] and
//! [`Opener::open_retrying_ruled`] are that opener: such an answer is a
//! [`Refused`] ([`redirect_refusal`] has the rule and the line), its body
//! never read. The caller writes the line where its block's stderr goes
//! ([`Refused::write`]) and ends with [`Refused::EXIT`]. Every block that
//! calls the daemon runs the rule, so a family moves to the `_ruled` pair
//! as it is checked against it.
//! - A version of the daemon that is not the binary's prints the binary's
//!   one line ([`super::skew`]), as every native call does.

use super::client::{Auth, Client, OWNER_SOURCE_DISK, OWNER_SOURCE_HEADER, VERSION_HEADER};
use super::client::{VIA_HEADER, VIA_NATIVE, WOULD_REFUSE_HEADER};
use super::encode::quote_bytes;
use super::skew;
use super::wire::SERVER_VERSION_HEADER;
use crate::exit::Exit;
use std::convert::Infallible;
use std::io::{self, BufRead, BufReader, Read as _, Write as _};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The daemon's pre-ready answer (with a 503) that `_mail_py` and
/// `_cal_py` ask again.
pub const MIGRATING: &str = r#""state":"migrating""#;

/// http.client's `_MAXLINE`: a longer status or header line is
/// `LineTooLong`.
const MAX_LINE: usize = 65536;
/// http.client's `_MAXHEADERS`.
const MAX_HEADERS: usize = 100;

/// `urllib.parse.urlencode(pairs)`: each key and value through
/// `quote_plus(v, safe='')` (letters, digits and `_ . - ~` as they are, a
/// space as `+`, every other UTF-8 byte as `%XX`), joined by `=` and `&`.
pub fn urlencode<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k.as_ref()), quote_plus(v.as_ref())))
        .collect::<Vec<_>>()
        .join("&")
}

/// `urllib.parse.quote_plus(s, safe='')`.
pub fn quote_plus(s: &str) -> String {
    s.split(' ')
        .map(|piece| quote_bytes(piece.as_bytes()))
        .collect::<Vec<_>>()
        .join("+")
}

/// `raw.decode("utf-8", "replace")`.
pub fn decode(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// The method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One `urllib.request.Request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    /// The route, `/cli/...`, as the Python writes it after `base`.
    pub route: String,
    /// The pairs `urlencode` writes after `?` (none: no `?`).
    pub query: Vec<(String, String)>,
    /// `data`: `None` sends no body and no `Content-Length`.
    pub data: Option<Vec<u8>>,
    /// The headers the Python gives the Request and adds to it, in that
    /// order (`Content-Type`, `Authorization`, ...), spelled as they go
    /// out (urllib's `.capitalize()` then `.title()`: `Content-Type`).
    pub headers: Vec<(String, String)>,
    /// `urlopen(..., timeout=t)`.
    pub timeout: Duration,
}

impl Request {
    /// A request with no query, body or headers.
    pub fn new(method: Method, route: impl Into<String>, timeout: Duration) -> Request {
        Request {
            method,
            route: route.into(),
            query: Vec::new(),
            data: None,
            headers: Vec::new(),
            timeout,
        }
    }

    /// The query pairs.
    pub fn query<K: Into<String>, V: Into<String>>(
        mut self,
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Request {
        self.query = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// The body.
    pub fn data(mut self, data: impl Into<Vec<u8>>) -> Request {
        self.data = Some(data.into());
        self
    }

    /// One more header, after the ones before.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Request {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// `req.selector`: the route and the query.
    pub fn selector(&self) -> String {
        let qs = urlencode(&self.query);
        if qs.is_empty() {
            self.route.clone()
        } else {
            format!("{}?{qs}", self.route)
        }
    }
}

/// What the shell's `_is_timeout` and `_restart_kind` make of an exception
/// (walking `.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `TimeoutError` (or `socket.timeout`).
    Timeout,
    /// `ConnectionRefusedError`: nothing accepted the connect.
    Refused,
    /// `ConnectionResetError` (`RemoteDisconnected` is one),
    /// `BrokenPipeError` or `http.client.IncompleteRead`: closed before
    /// the answer ended.
    Dropped,
    /// Anything else (`BadStatusLine`, `LineTooLong`, `gaierror`, ...).
    Other,
}

/// The exception's class, as a traceback's last line names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `urllib.error.URLError`, from the connect or the send.
    UrlError,
    TimeoutError,
    ConnectionResetError,
    BrokenPipeError,
    /// Another `OSError` subclass Python picks by errno; its name.
    OsError(&'static str),
    RemoteDisconnected,
    IncompleteRead,
    BadStatusLine,
    LineTooLong,
    HttpException,
}

impl Class {
    /// The name a traceback prints before `: str(e)`.
    pub fn name(self) -> &'static str {
        match self {
            Class::UrlError => "urllib.error.URLError",
            Class::TimeoutError => "TimeoutError",
            Class::ConnectionResetError => "ConnectionResetError",
            Class::BrokenPipeError => "BrokenPipeError",
            Class::OsError(n) => n,
            Class::RemoteDisconnected => "http.client.RemoteDisconnected",
            Class::IncompleteRead => "http.client.IncompleteRead",
            Class::BadStatusLine => "http.client.BadStatusLine",
            Class::LineTooLong => "http.client.LineTooLong",
            Class::HttpException => "http.client.HTTPException",
        }
    }
}

/// An exception urllib or http.client raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raised {
    pub class: Class,
    pub kind: Kind,
    /// `str(reason)` for a `URLError`; `str(e)` for anything else.
    pub reason: String,
}

impl Raised {
    fn url_error(reason: Raised) -> Raised {
        Raised {
            class: Class::UrlError,
            kind: reason.kind,
            reason: reason.reason,
        }
    }

    /// `str(e)` (`"%s" % e`).
    pub fn text(&self) -> String {
        match self.class {
            Class::UrlError => format!("<urlopen error {}>", self.reason),
            _ => self.reason.clone(),
        }
    }

    /// `getattr(e, "reason", e)` as `%s` prints it: the reason of a
    /// `URLError`, else `str(e)`.
    pub fn reason_text(&self) -> &str {
        &self.reason
    }

    /// The last line of the traceback it ends a run with.
    pub fn traceback_line(&self) -> String {
        format!("{}: {}", self.class.name(), self.text())
    }

    pub fn is_timeout(&self) -> bool {
        self.kind == Kind::Timeout
    }
}

/// `r.read()` or `e.read()`: the body, or what reading it raised.
pub type Body = Result<Vec<u8>, Raised>;

/// What one `urlopen` came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It returned: a status in 200..=299 (`r.status`) and `r.read()`.
    Answer { status: u16, body: Body },
    /// It raised `HTTPError`: `e.code` and `e.read()`.
    HttpError { code: u16, body: Body },
    /// It raised anything else.
    Raised(Raised),
}

impl Outcome {
    /// `r.status` or `e.code`; none when something else was raised.
    pub fn status(&self) -> Option<u16> {
        match self {
            Outcome::Answer { status, .. } => Some(*status),
            Outcome::HttpError { code, .. } => Some(*code),
            Outcome::Raised(_) => None,
        }
    }
}

/// The settings of the shell's Python block: `ABASE` and `ATOKEN`, and
/// the hand-over headers `_k5_py_headers` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opener {
    pub host: String,
    pub port: u16,
    /// The `Host` header: `ABASE`'s `host:port` as the shell typed it
    /// (urllib sends the URL's netloc as it is, so a port setting of
    /// `04100` goes out as `04100`). [`Opener::from_client`] writes the
    /// port as a number; a family that read the port as text sets it.
    pub netloc: String,
    token: Option<String>,
    /// The hand-over headers, in their order.
    pub handover: Vec<(String, String)>,
    pub user_agent: String,
    /// Between tries in [`Opener::open_retrying`] (`time.sleep(1)`); a
    /// test may shorten it.
    pub pause: Duration,
    /// The binary's version, for the daemon's ([`skew`]).
    version: String,
    /// `HOST=https://<name>`: TLS to [`Opener::host`].
    tls: bool,
    /// An `https://` URL the binary won't open as the shell's Python
    /// would: through the person's proxy, or one the shared transport
    /// doesn't read ([`super::remote::daemon_https`]). Nothing is sent.
    unsendable: Option<String>,
}

impl Opener {
    /// The opener for `client`'s settings: its host, port and TCP token,
    /// and the hand-over headers it puts on its own calls.
    pub fn from_client(client: &Client) -> Opener {
        let token = match &client.auth {
            Auth::None => None,
            Auth::Owner(t) => Some(t.clone()),
            Auth::Passport { passport, .. } => Some(passport.as_str().to_string()),
        };
        let mut handover = Vec::new();
        if client.would_refuse {
            handover.push((WOULD_REFUSE_HEADER.to_string(), "1".to_string()));
        }
        handover.push((VERSION_HEADER.to_string(), client.version.clone()));
        handover.push((VIA_HEADER.to_string(), VIA_NATIVE.to_string()));
        if client.owner_from_disk && matches!(client.auth, Auth::Owner(_)) {
            handover.push((
                OWNER_SOURCE_HEADER.to_string(),
                OWNER_SOURCE_DISK.to_string(),
            ));
        }
        // `_k5_py_headers` prints a Codex call's conversation headers
        // after its hand-over headers.
        handover.extend(client.conversation.iter().cloned());
        // The daemon's client over TLS through the person's proxy: the
        // shell's Python tunnels as http.client does, which the binary
        // doesn't copy.
        let unsendable = match (&client.unsendable, client.tls && client.via.is_some()) {
            (Some(url), _) => Some(url.clone()),
            (None, true) => Some(format!("https://{}:{}", client.host, client.port)),
            (None, false) => None,
        };
        Opener {
            host: client.host.clone(),
            port: client.port,
            netloc: format!("{}:{}", client.host, client.port),
            token,
            handover,
            user_agent: format!("k5/{}", client.version),
            pause: Duration::from_secs(1),
            version: client.version.clone(),
            tls: client.tls,
            unsendable,
        }
    }

    /// The `Host` header: [`Opener::netloc`], with the `https://` (or
    /// `HTTPS://`) a family's own `HOST` text carries taken off, and the
    /// name's one trailing `/`, as `_k2_daemon_base` writes `BASE_URL`
    /// and urllib sends that URL's netloc.
    fn host_header(&self) -> String {
        let n = self.netloc.as_str();
        if !self.tls {
            return n.to_string();
        }
        let Some(rest) = n
            .strip_prefix("https://")
            .or_else(|| n.strip_prefix("HTTPS://"))
        else {
            return n.to_string();
        };
        match rest.rsplit_once(':') {
            Some((name, port)) => format!("{}:{port}", name.strip_suffix('/').unwrap_or(name)),
            None => rest.to_string(),
        }
    }

    /// `ATOKEN`: the token the Python sends as `Bearer`, or none
    /// (`if token:` adds no header).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|t| !t.is_empty())
    }

    /// `ABASE`: `http://HOST:PORT` as the shell typed it, or
    /// `https://<name>:PORT` for `HOST=https://<name>`.
    pub fn base(&self) -> String {
        if self.tls {
            return format!("https://{}", self.host_header());
        }
        format!("http://{}", self.netloc)
    }

    /// The request's bytes, as http.client writes them.
    pub fn encode(&self, req: &Request) -> Vec<u8> {
        let mut head = format!(
            "{} {} HTTP/1.1\r\nAccept-Encoding: identity\r\n",
            req.method.as_str(),
            req.selector()
        );
        // http.client's own `Content-Length: 0` for a POST with no data,
        // before the headers urllib hands it.
        if req.data.is_none() && req.method == Method::Post {
            head.push_str("Content-Length: 0\r\n");
        }
        let has = |name: &str| {
            req.headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case(name))
        };
        if let Some(data) = &req.data {
            if !has("Content-Type") {
                head.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
            }
            head.push_str(&format!("Content-Length: {}\r\n", data.len()));
        }
        head.push_str(&format!("Host: {}\r\n", self.host_header()));
        head.push_str(&format!("User-Agent: {}\r\n", self.user_agent));
        for (name, value) in req.headers.iter().chain(&self.handover) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        let mut out = head.into_bytes();
        if let Some(data) = &req.data {
            out.extend_from_slice(data);
        }
        out
    }

    /// One `urllib.request.urlopen(k5_request(req), timeout=...)`, and the
    /// body read after it. A redirect is the answer (an
    /// [`Outcome::HttpError`]); [`Opener::open_ruled`] refuses it.
    pub fn open(&self, req: &Request) -> Outcome {
        let Ok(out) = self.exchange(req, |_| Ok::<(), Infallible>(()));
        out
    }

    /// [`Opener::open`] under `_K2_PY_PROXY_RULE`: a redirect urllib would
    /// follow is a [`Refused`], before its body is read. The request's URL
    /// is [`Opener::base`]'s, as urllib sends it.
    pub fn open_ruled(&self, req: &Request) -> Result<Outcome, Refused> {
        self.exchange(req, |head| {
            let to = head.location.as_deref().or(head.uri.as_deref());
            match redirect_refusal(&self.base(), head.status, to) {
                Some(r) => Err(r),
                None => Ok(()),
            }
        })
    }

    /// The exchange: connect, send, the head (`check` may end it there),
    /// then the body.
    fn exchange<E>(
        &self,
        req: &Request,
        check: impl FnOnce(&Head) -> Result<(), E>,
    ) -> Result<Outcome, E> {
        if let Some(url) = &self.unsendable {
            return Ok(Outcome::Raised(Raised {
                class: Class::UrlError,
                kind: Kind::Other,
                reason: format!("won't send to {url}: a proxy or URL this command doesn't carry"),
            }));
        }
        let tls = self.tls.then_some(self.host.as_str());
        let mut reader = match start(&self.host, self.port, tls, &self.encode(req), req.timeout) {
            Ok(r) => r,
            Err(r) => return Ok(Outcome::Raised(r)),
        };
        let head = match read_head(&mut reader) {
            Ok(h) => h,
            Err(r) => return Ok(Outcome::Raised(r)),
        };
        skew::note(&self.version, head.server_version.as_deref());
        check(&head)?;
        let body = read_body(&mut reader, &head);
        Ok(if (200..300).contains(&head.status) {
            Outcome::Answer {
                status: head.status,
                body,
            }
        } else {
            Outcome::HttpError {
                code: head.status,
                body,
            }
        })
    }

    /// `_mail_py`'s and `_cal_py`'s loop around [`Opener::open`] (k2-ptyd
    /// S5): at most 4 tries, [`Opener::pause`] apart. Tried again: a 503
    /// whose body holds [`MIGRATING`]; a refused connect; and, once, a GET
    /// dropped before its answer ended. A timeout ends it at once (the
    /// shell's `_is_timeout` comes first), as does anything else. The
    /// last outcome is the caller's.
    pub fn open_retrying(&self, req: &Request) -> Outcome {
        let Ok(out) = self.retrying(req, |r| Ok::<_, Infallible>(self.open(r)));
        out
    }

    /// [`Opener::open_retrying`] under `_K2_PY_PROXY_RULE`: a refused
    /// redirect ends the loop at once (`SystemExit` is no exception the
    /// loop catches).
    pub fn open_retrying_ruled(&self, req: &Request) -> Result<Outcome, Refused> {
        self.retrying(req, |r| self.open_ruled(r))
    }

    fn retrying<E>(
        &self,
        req: &Request,
        open: impl Fn(&Request) -> Result<Outcome, E>,
    ) -> Result<Outcome, E> {
        let mut dropped = false;
        let mut attempt = 0;
        loop {
            let out = open(req)?;
            let last = attempt >= 3;
            let again = match &out {
                Outcome::HttpError {
                    code: 503,
                    body: Ok(b),
                } => !last && decode(b).contains(MIGRATING),
                Outcome::HttpError { .. } | Outcome::Answer { body: Ok(_), .. } => false,
                Outcome::Answer { body: Err(r), .. } | Outcome::Raised(r) => match r.kind {
                    Kind::Timeout | Kind::Other => false,
                    Kind::Refused => !last,
                    Kind::Dropped => {
                        let once = req.method == Method::Get && !dropped && !last;
                        dropped |= once;
                        once
                    }
                },
            };
            if !again {
                return Ok(out);
            }
            std::thread::sleep(self.pause);
            attempt += 1;
        }
    }
}

/// A redirect `_K2NoRedirect` refused: the line `k2_redirect_refused`
/// writes before `SystemExit(3)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub line: String,
}

impl Refused {
    /// The block's exit after the line.
    pub const EXIT: Exit = Exit::Refused;

    /// The line on stderr now, flushed, for a family that writes its
    /// stderr as it goes. The exit, [`Refused::EXIT`].
    pub fn write(&self) -> Exit {
        crate::out::stderr_line(&self.line);
        Refused::EXIT
    }
}

/// What `_K2NoRedirect` does with an answer `code` to a request to
/// `url` whose `Location` (else `URI`) is `to`. urllib's handler takes
/// 301, 302, 303, 307 and 308 with one of them, to `http`, `https`, `ftp`
/// or no scheme (any other is its own `HTTPError`, and none here); it
/// quotes the new URL (`quote(newurl, encoding="iso-8859-1",
/// safe=string.punctuation)`) and joins it to the request's, and the
/// refusal names both places by their netloc less any user part, or the
/// whole new URL when it has none. Quoted, every character is printable,
/// so the rule's `?` for one that isn't never shows.
pub fn redirect_refusal(url: &str, code: u16, to: Option<&str>) -> Option<Refused> {
    if !matches!(code, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let to = to?;
    if !matches!(split_place(to).0.as_str(), "http" | "https" | "ftp" | "") {
        return None;
    }
    let quoted: String = to
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() {
                c.to_string()
            } else {
                // A header is read as ISO-8859-1: one byte a character.
                format!("%{:02X}", u32::from(c) & 0xff)
            }
        })
        .collect();
    let after_at = |s: &str| s.rsplit_once('@').map_or(s, |(_, h)| h).to_string();
    let (base_scheme, netloc) = split_place(url);
    // urljoin onto the request's URL: another scheme keeps its own URL;
    // the same or none takes the new netloc, else the old one.
    let (scheme, place) = split_place(&quoted);
    let there = if place.is_empty() && (scheme.is_empty() || scheme == base_scheme) {
        after_at(&netloc)
    } else {
        Some(after_at(&place))
            .filter(|p| !p.is_empty())
            .unwrap_or(quoted)
    };
    let here = after_at(&netloc);
    let p = k2_client::product_name!();
    Some(Refused {
        line: format!(
            "refused: {here} answered {code}, sending {p} to {there}; {p} does not follow a redirect with your token or password, so nothing was sent there"
        ),
    })
}

/// `urlsplit(url)`'s scheme (in lower case) and netloc.
fn split_place(url: &str) -> (String, String) {
    let mut rest = url;
    let mut scheme = String::new();
    if let Some((s, after)) = url.split_once(':') {
        let ok = s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        if ok {
            scheme = s.to_ascii_lowercase();
            rest = after;
        }
    }
    let netloc = rest.strip_prefix("//").map_or(String::new(), |after| {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        after[..end].to_string()
    });
    (scheme, netloc)
}

/// The text of an `OSError` as Python prints it: `[Errno N] strerror`.
fn errno_text(code: i32) -> String {
    let full = io::Error::from_raw_os_error(code).to_string();
    let text = full
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&full);
    format!("[Errno {code}] {text}")
}

#[cfg(unix)]
fn errno_class(code: i32) -> (Class, Kind) {
    match code {
        libc::ECONNREFUSED => (Class::OsError("ConnectionRefusedError"), Kind::Refused),
        libc::ECONNRESET => (Class::ConnectionResetError, Kind::Dropped),
        libc::EPIPE => (Class::BrokenPipeError, Kind::Dropped),
        libc::ECONNABORTED => (Class::OsError("ConnectionAbortedError"), Kind::Other),
        libc::ETIMEDOUT => (Class::TimeoutError, Kind::Timeout),
        _ => (Class::OsError("OSError"), Kind::Other),
    }
}

#[cfg(not(unix))]
fn errno_class(_code: i32) -> (Class, Kind) {
    (Class::OsError("OSError"), Kind::Other)
}

/// A socket timeout: `TimeoutError('timed out')`.
fn timed_out() -> Raised {
    Raised {
        class: Class::TimeoutError,
        kind: Kind::Timeout,
        reason: "timed out".to_string(),
    }
}

/// An I/O error as the `OSError` Python raises for it (public: `app
/// shell`'s socket raises the same).
pub fn os_error(e: &io::Error) -> Raised {
    if matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) && e.raw_os_error().is_none_or(is_timeout_errno)
    {
        return timed_out();
    }
    match e.raw_os_error() {
        Some(code) => {
            let (class, kind) = errno_class(code);
            Raised {
                class,
                kind,
                reason: errno_text(code),
            }
        }
        None => Raised {
            class: Class::OsError("OSError"),
            kind: Kind::Other,
            reason: e.to_string(),
        },
    }
}

/// An errno a socket timeout reports (`SO_RCVTIMEO` and `SO_SNDTIMEO` end
/// with `EAGAIN`); Python's own timeout is `timed out`, with no errno.
#[cfg(unix)]
fn is_timeout_errno(code: i32) -> bool {
    code == libc::EAGAIN || code == libc::EWOULDBLOCK
}

#[cfg(not(unix))]
fn is_timeout_errno(_code: i32) -> bool {
    true
}

/// `socket.create_connection((host, port), timeout)`: each address in
/// turn; the first error when none connects. The prelude's plain-http
/// step runs first, once ([`crate::prelude::plain_http`]).
pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, Raised> {
    crate::prelude::plain_http::run();
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| gai_error(&e))?
        .collect();
    let mut first = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => {
                let r = if e.kind() == io::ErrorKind::TimedOut && e.raw_os_error().is_none() {
                    timed_out()
                } else {
                    os_error(&e)
                };
                first.get_or_insert(r);
            }
        }
    }
    Err(first.unwrap_or_else(|| Raised {
        class: Class::OsError("OSError"),
        kind: Kind::Other,
        reason: "getaddrinfo returns an empty list".to_string(),
    }))
}

/// A name that doesn't resolve: `socket.gaierror`, `[Errno N] text` with
/// getaddrinfo's own number and text.
fn gai_error(e: &io::Error) -> Raised {
    let full = e.to_string();
    let text = full
        .split_once("information: ")
        .map_or(full.as_str(), |(_, t)| t)
        .to_string();
    let code = match text.as_str() {
        "Name or service not known" => Some(-2),
        "Temporary failure in name resolution" => Some(-3),
        "nodename nor servname provided, or not known" => Some(8),
        _ => None,
    };
    Raised {
        class: Class::OsError("socket.gaierror"),
        kind: Kind::Other,
        reason: match code {
            Some(n) => format!("[Errno {n}] {text}"),
            None => text,
        },
    }
}

/// `sock.sendall(request)` with the socket's timeout.
/// `urlopen`'s connection: [`connect`], TLS over it to the name `tls`
/// gives ([`super::tls`]), each send and read on `timeout`, and `bytes`
/// sent. What fails is a `URLError`, nothing of the request sent past a
/// failed handshake.
fn start(
    host: &str,
    port: u16,
    tls: Option<&str>,
    bytes: &[u8],
    timeout: Duration,
) -> Result<BufReader<super::wire::Conn>, Raised> {
    use super::wire::Conn;
    let tcp = connect(host, port, timeout).map_err(Raised::url_error)?;
    let mut conn = match tls {
        Some(name) => {
            let limit = std::time::Instant::now() + timeout;
            match super::tls::connect(tcp, name, limit) {
                Ok(s) => Conn::Tls(Box::new(s)),
                Err(f) => return Err(tls_raised(&f)),
            }
        }
        None => Conn::Tcp(tcp),
    };
    let timeouts = match &conn {
        Conn::Tcp(s) => s
            .set_write_timeout(Some(timeout))
            .and_then(|()| s.set_read_timeout(Some(timeout))),
        Conn::Tls(s) => super::tls::stream_timeouts(s, timeout),
        #[cfg(unix)]
        Conn::Unix(_) => Ok(()),
    };
    timeouts
        .and_then(|()| conn.write_all(bytes))
        .and_then(|()| conn.flush())
        .map_err(|e| Raised::url_error(os_error(&e)))?;
    Ok(BufReader::with_capacity(8192, conn))
}

/// The answer's head as http.client keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    status: u16,
    chunked: bool,
    /// `None`: read to the end of the stream.
    length: Option<u64>,
    server_version: Option<String>,
    /// The `Location` header, as http.client keeps it.
    location: Option<String>,
    /// The `URI` header (urllib's redirect handler reads it when there is
    /// no `Location`).
    uri: Option<String>,
}

/// `fp.readline(_MAXLINE + 1)`: a line with its end, or less at the end
/// of the stream.
fn read_line(r: &mut impl BufRead) -> Result<Vec<u8>, Raised> {
    let mut line = Vec::new();
    let mut limited = r.take(MAX_LINE as u64 + 1);
    match limited.read_until(b'\n', &mut line) {
        Ok(_) => Ok(line),
        Err(e) => Err(os_error(&e)),
    }
}

fn line_too_long(what: &str) -> Raised {
    Raised {
        class: Class::LineTooLong,
        kind: Kind::Other,
        reason: format!("got more than {MAX_LINE} bytes when reading {what}"),
    }
}

/// `str(line, "iso-8859-1")`.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Python's `str.split()` with no separator: runs of whitespace.
fn py_split(s: &str, max: usize) -> Vec<&str> {
    let ws = |c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c);
    let mut parts = Vec::new();
    let mut rest = s.trim_start_matches(ws);
    while !rest.is_empty() {
        if parts.len() == max {
            parts.push(rest);
            break;
        }
        let end = rest.find(ws).unwrap_or(rest.len());
        parts.push(&rest[..end]);
        rest = rest[end..].trim_start_matches(ws);
    }
    parts
}

/// Python's `int(text)` for a status or a length: optional spaces and
/// sign, digits with single `_` between them.
fn py_int(text: &str) -> Option<i64> {
    let t = text.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let n: i64 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -n } else { n })
}

/// `HTTPResponse._read_status`.
fn read_status(r: &mut impl BufRead) -> Result<u16, Raised> {
    let raw = read_line(r)?;
    if raw.len() > MAX_LINE {
        return Err(line_too_long("status line"));
    }
    if raw.is_empty() {
        return Err(Raised {
            class: Class::RemoteDisconnected,
            kind: Kind::Dropped,
            reason: "Remote end closed connection without response".to_string(),
        });
    }
    let line = latin1(&raw);
    let bad = || Raised {
        class: Class::BadStatusLine,
        kind: Kind::Other,
        reason: line.clone(),
    };
    let parts = py_split(&line, 2);
    let (version, status) = match parts.as_slice() {
        [v, s, ..] => (*v, *s),
        _ => ("", ""),
    };
    if !version.starts_with("HTTP/") {
        return Err(bad());
    }
    match py_int(status) {
        Some(n @ 100..=999) => u16::try_from(n).map_err(|_| bad()),
        _ => Err(bad()),
    }
}

/// `_read_headers`: the header lines up to the blank one (or the end of
/// the stream).
fn read_headers(r: &mut impl BufRead) -> Result<Vec<String>, Raised> {
    let mut lines = Vec::new();
    loop {
        let raw = read_line(r)?;
        if raw.len() > MAX_LINE {
            return Err(line_too_long("header line"));
        }
        if raw == b"\r\n" || raw == b"\n" || raw.is_empty() {
            return Ok(lines);
        }
        lines.push(latin1(&raw));
        if lines.len() > MAX_HEADERS {
            return Err(Raised {
                class: Class::HttpException,
                kind: Kind::Other,
                reason: format!("got more than {MAX_HEADERS} headers"),
            });
        }
    }
}

/// `msg.get(name)`: the first header of that name, its value with the
/// spaces after the colon and the line's end taken off.
fn header<'a>(lines: &'a [String], name: &str) -> Option<&'a str> {
    lines.iter().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.eq_ignore_ascii_case(name).then(|| {
            v.trim_start_matches([' ', '\t'])
                .trim_end_matches(['\r', '\n'])
        })
    })
}

/// `HTTPResponse.begin`.
fn read_head(r: &mut impl BufRead) -> Result<Head, Raised> {
    let status = loop {
        let status = read_status(r)?;
        if status != 100 {
            break status;
        }
        read_headers(r)?;
    };
    let lines = read_headers(r)?;
    let chunked =
        header(&lines, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    let mut length = match header(&lines, "content-length") {
        Some(v) if !v.is_empty() && !chunked => py_int(v).and_then(|n| u64::try_from(n).ok()),
        _ => None,
    };
    if status == 204 || status == 304 || (100..200).contains(&status) {
        length = Some(0);
    }
    Ok(Head {
        status,
        chunked,
        length,
        server_version: header(&lines, SERVER_VERSION_HEADER).map(|v| v.trim().to_string()),
        location: header(&lines, "location").map(str::to_string),
        uri: header(&lines, "uri").map(str::to_string),
    })
}

/// One answer read after a request ([`read_answer`]): what urlopen came
/// to, and the answer's `Location` header (a redirect's new place).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub outcome: Outcome,
    pub location: Option<String>,
    /// The daemon's version header, for [`skew::note`].
    pub server_version: Option<String>,
}

/// The answer on `r` as http.client reads it after a request was sent
/// (the head, then the body, as [`Opener::open`] reads them), with its
/// `Location` header. For a caller that writes the request itself (a
/// Unix socket, or a stream it watches for a stop).
pub fn read_answer(r: &mut impl BufRead) -> Answered {
    let head = match read_head(r) {
        Ok(h) => h,
        Err(raised) => {
            return Answered {
                outcome: Outcome::Raised(raised),
                location: None,
                server_version: None,
            }
        }
    };
    let body = read_body(r, &head);
    let outcome = if (200..300).contains(&head.status) {
        Outcome::Answer {
            status: head.status,
            body,
        }
    } else {
        Outcome::HttpError {
            code: head.status,
            body,
        }
    };
    Answered {
        outcome,
        location: head.location,
        server_version: head.server_version,
    }
}

/// The bytes Python's `http.client.HTTPConnection.request(method, url,
/// body, headers)` writes with no urllib around it (`_UConn`, a Unix
/// socket): `Host: <host>` and `Accept-Encoding: identity` first,
/// `Content-Length` (0 for a POST with no body), then the caller's
/// headers in their order. No `User-Agent`, no `Connection`.
pub fn encode_http_client(host: &str, req: &Request) -> Vec<u8> {
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: identity\r\n",
        req.method.as_str(),
        req.selector()
    );
    match &req.data {
        Some(data) => head.push_str(&format!("Content-Length: {}\r\n", data.len())),
        None if req.method == Method::Post => head.push_str("Content-Length: 0\r\n"),
        None => {}
    }
    for (name, value) in &req.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(data) = &req.data {
        out.extend_from_slice(data);
    }
    out
}

fn incomplete(got: usize, expected: Option<u64>) -> Raised {
    Raised {
        class: Class::IncompleteRead,
        kind: Kind::Dropped,
        reason: match expected {
            Some(n) => format!("IncompleteRead({got} bytes read, {n} more expected)"),
            None => format!("IncompleteRead({got} bytes read)"),
        },
    }
}

/// `fp.read(n)`: up to `n` bytes, fewer only at the end of the stream.
fn read_exact_or_less(r: &mut impl BufRead, n: u64, out: &mut Vec<u8>) -> Result<(), Raised> {
    r.take(n)
        .read_to_end(out)
        .map(|_| ())
        .map_err(|e| os_error(&e))
}

/// `_safe_read(n)`.
fn safe_read(r: &mut impl BufRead, n: u64) -> Result<Vec<u8>, Raised> {
    let mut data = Vec::new();
    read_exact_or_less(r, n, &mut data)?;
    let got = data.len() as u64;
    if got < n {
        return Err(incomplete(data.len(), Some(n - got)));
    }
    Ok(data)
}

/// `HTTPResponse.read()`.
fn read_body(r: &mut impl BufRead, head: &Head) -> Body {
    if head.chunked {
        return read_chunked(r);
    }
    match head.length {
        Some(n) => safe_read(r, n),
        None => {
            let mut all = Vec::new();
            r.read_to_end(&mut all).map_err(|e| os_error(&e))?;
            Ok(all)
        }
    }
}

/// `_read_chunked()`: every chunk; a broken one is `IncompleteRead` of
/// what came before it.
fn read_chunked(r: &mut impl BufRead) -> Body {
    let mut value = Vec::new();
    let mut first = true;
    loop {
        let step = (|| -> Result<Option<u64>, Raised> {
            if !first {
                safe_read(r, 2)?;
            }
            let raw = read_line(r)?;
            if raw.len() > MAX_LINE {
                return Err(line_too_long("chunk size"));
            }
            let size = raw.split(|&b| b == b';').next().unwrap_or_default();
            let text = latin1(size);
            let t = text.trim();
            let hex = t
                .strip_prefix("0x")
                .or_else(|| t.strip_prefix("0X"))
                .unwrap_or(t);
            let n = (!hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit() || b == b'_'))
                .then(|| u64::from_str_radix(&hex.replace('_', ""), 16).ok())
                .flatten()
                .ok_or_else(|| incomplete(0, None))?;
            if n == 0 {
                // The trailer, up to its blank line or the end.
                loop {
                    let l = read_line(r)?;
                    if l == b"\r\n" || l == b"\n" || l.is_empty() {
                        break;
                    }
                }
                return Ok(None);
            }
            Ok(Some(n))
        })();
        first = false;
        match step {
            Ok(None) => return Ok(value),
            Ok(Some(n)) => match safe_read(r, n) {
                Ok(chunk) => value.extend_from_slice(&chunk),
                Err(e) if e.class == Class::IncompleteRead => {
                    return Err(incomplete(value.len(), None))
                }
                Err(e) => return Err(e),
            },
            Err(e) if e.class == Class::IncompleteRead => {
                return Err(incomplete(value.len(), None))
            }
            Err(e) => return Err(e),
        }
    }
}

/// Where [`open_url`] goes: a full URL's host and port, its netloc as
/// written (urllib's `Host` header) and whether it is `https://`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub host: String,
    pub port: u16,
    pub netloc: String,
    pub tls: bool,
}

/// The bytes http.client writes for a Python block whose `Request` names
/// every header itself, `User-Agent` among them, and that sends no
/// hand-over headers (`_K5_PY_HEADERS=""`: a server that is not the
/// daemon): http.client's `Accept-Encoding`, its `Content-Length: 0` for
/// a POST with no data, urllib's form type for data with no type of the
/// caller's, `Content-Length` and `Host`, then the request's headers in
/// their order, then `Connection: close`.
pub fn encode_plain(netloc: &str, req: &Request) -> Vec<u8> {
    let mut head = format!(
        "{} {} HTTP/1.1\r\nAccept-Encoding: identity\r\n",
        req.method.as_str(),
        req.selector()
    );
    if req.data.is_none() && req.method == Method::Post {
        head.push_str("Content-Length: 0\r\n");
    }
    let has = |name: &str| {
        req.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    if let Some(data) = &req.data {
        if !has("Content-Type") {
            head.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n", data.len()));
    }
    head.push_str(&format!("Host: {netloc}\r\n"));
    for (name, value) in &req.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut out = head.into_bytes();
    if let Some(data) = &req.data {
        out.extend_from_slice(data);
    }
    out
}

/// What a failed TLS handshake raises: a `URLError`, its reason the
/// binary's own words. Python's `ssl` texts (`[SSL:
/// CERTIFICATE_VERIFY_FAILED] ...`) are OpenSSL's and not copied (the
/// named difference python-tls-error-text); like Python's, nothing of the
/// request was sent.
fn tls_raised(f: &super::wire::Failure) -> Raised {
    use super::wire::Failure;
    let (kind, reason) = match f {
        Failure::ConnectTimedOut => (
            Kind::Timeout,
            "_ssl.c: The handshake operation timed out".to_string(),
        ),
        Failure::Broken { why, .. } => (Kind::Other, format!("[SSL] {why}")),
        other => (Kind::Other, format!("[SSL] {other:?}")),
    };
    Raised {
        class: Class::UrlError,
        kind,
        reason,
    }
}

/// One `urllib.request.urlopen(req, timeout=...)` of `place` and the body
/// read after it, for a Python block that talks to a server that is not
/// the daemon (the Connect account server): plain TCP, or TLS over it for
/// `https://` ([`super::tls`], K2's ruling D5), with
/// [`encode_plain`]'s request. Never a proxy: a caller hands a URL the
/// person's proxy would carry to the shell. The answer and the
/// exceptions are [`Opener::open`]'s.
pub fn open_url(place: &Place, req: &Request) -> Outcome {
    let tls = place.tls.then_some(place.host.as_str());
    let bytes = encode_plain(&place.netloc, req);
    let mut reader = match start(&place.host, place.port, tls, &bytes, req.timeout) {
        Ok(r) => r,
        Err(r) => return Outcome::Raised(r),
    };
    let head = match read_head(&mut reader) {
        Ok(h) => h,
        Err(r) => return Outcome::Raised(r),
    };
    let body = read_body(&mut reader, &head);
    if (200..300).contains(&head.status) {
        Outcome::Answer {
            status: head.status,
            body,
        }
    } else {
        Outcome::HttpError {
            code: head.status,
            body,
        }
    }
}

#[cfg(test)]
mod tests;
