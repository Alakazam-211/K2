//! The calls: the shell CLI's request functions, each with the request it
//! sends, its tries and its time limits (the shapes are in
//! [`crate::transport`]).

use super::encode::quote;
use super::permission::{self, Note};
use super::wire::{self, Answer, Body, Failure, Method, Request, Target};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--connect-timeout 5`, every call.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// `--max-time 30`, and the default of `_K2_POST_MAX_TIME`.
pub const MAX_TIME: Duration = Duration::from_secs(30);
/// `cli_send_json`'s default, `_K2_SEND_MAX_TIME`.
pub const SEND_JSON_MAX_TIME: Duration = Duration::from_secs(60);
/// `_K2_CURL_TRIES`: each retry loop's tries, the first included.
pub const TRIES: u32 = 4;
/// The `sleep 1` between tries.
pub const PAUSE: Duration = Duration::from_secs(1);

/// The daemon's pre-ready answer (with status 503): no handler ran.
pub const MIGRATING: &str = r#""state":"migrating""#;
/// The daemon's answer on the cell socket for a route it serves only on
/// TCP.
pub const NOT_SERVED: &str = "not served on cell socket";
/// A pre-0.44.4 daemon's answer to a POST on a GET route.
pub const METHOD_NOT_ALLOWED: &str = "method not allowed for this route";
/// The daemon's refusal of a passport it doesn't know.
pub const PASSPORT_REFUSAL: &str = "Invalid or missing auth token";

/// The header that says which CLI sent the request.
pub const VIA_HEADER: &str = "X-K5-Cli-Via";
/// Its value on the binary's own requests (the shell says `fallthrough`).
pub const VIA_NATIVE: &str = "native";
/// The binary's version, on its own requests.
pub const VERSION_HEADER: &str = "X-K5-Cli-Version";
/// Set to `1` when the passport rule would refuse the call (slice 7).
pub const WOULD_REFUSE_HEADER: &str = "X-K5-Would-Refuse";

/// A session passport. Never empty, so an empty Bearer is never sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Passport(String);

impl Passport {
    /// `None` for an empty token: an empty passport is no passport.
    pub fn new(token: impl Into<String>) -> Option<Passport> {
        let token = token.into();
        (!token.is_empty()).then_some(Passport(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Passport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passport(..)")
    }
}

/// Who the calls are made as.
///
/// With a passport there is no owner token at all, so none is ever sent
/// on any channel (PRD 13.9 #4): a refusal is the passport's answer.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// No passport: the owner token, over TCP only.
    Owner(String),
    /// A passport: the Bearer on the cell socket at `sock`, and `token=`
    /// over TCP.
    Passport { passport: Passport, sock: Option<PathBuf> },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::Owner(_) => f.write_str("Owner(..)"),
            Auth::Passport { sock, .. } => f.debug_struct("Passport").field("sock", sock).finish_non_exhaustive(),
        }
    }
}

/// Where a passport the daemon refused may have been minted again. The
/// shell reads `$K2_SECRETS_DIR/.k2-passport` ([`SecretsDir`]).
pub trait Healer {
    /// The new passport for this session, if there is one.
    fn healed(&self, current: &Passport) -> Option<Passport>;
}

/// The shell's rule (`_cli_healed_passport`, `cli/src/prelude.sh`): the
/// first line of `<dir>/.k2-passport`, a regular file and not a link,
/// shaped `<session>.<secret>` (`[A-Za-z0-9-]+` dot `[A-Za-z0-9]+`), not
/// the current one and for the same session (the part before the first
/// dot). The current one must have a dot (an owner token has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretsDir(pub PathBuf);

impl Healer for SecretsDir {
    fn healed(&self, current: &Passport) -> Option<Passport> {
        let cur = current.as_str();
        if self.0.as_os_str().is_empty() || !cur.contains('.') {
            return None;
        }
        let file = self.0.join(".k2-passport");
        let meta = std::fs::symlink_metadata(&file).ok()?;
        if !meta.file_type().is_file() {
            return None;
        }
        let bytes = std::fs::read(&file).ok()?;
        let line = bytes.split(|b| *b == b'\n').next().unwrap_or(&[]);
        let t = std::str::from_utf8(line).ok()?;
        let (session, secret) = t.split_once('.')?;
        let shaped = !session.is_empty()
            && session.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !secret.is_empty()
            && secret.bytes().all(|b| b.is_ascii_alphanumeric());
        let same_session = cur.split('.').next() == Some(session);
        (shaped && t != cur && same_session).then(|| Passport(t.to_string()))
    }
}

/// The channel a request went on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Loopback TCP, `token=` in the query.
    Tcp,
    /// The cell socket, the passport as Bearer.
    Uds,
}

/// A call and what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call<'a> {
    /// `cli_request` / `cli_request_uds`: a GET; pre-encoded `key=value`
    /// pairs in the query.
    Request(&'a [&'a str]),
    /// `cli_request_post` / `cli_request_post_uds`: the same, as a POST
    /// with an empty body.
    RequestPost(&'a [&'a str]),
    /// `cli_post_form` / `cli_post_form_uds`: the pairs as a form body.
    PostForm(&'a [&'a str]),
    /// `cli_post_json` / `cli_post_json_uds`: a JSON body. The shell reads
    /// an empty one from stdin with `$(cat)`, which drops its trailing
    /// newlines; that is the caller's.
    PostJson(&'a [u8]),
}

/// The internal kinds: the [`Call`]s, `cli_post_json_stdin` and
/// `cli_send_json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Request,
    RequestPost,
    PostForm,
    PostJson,
    PostJsonStdin,
    SendJsonGet,
    SendJsonPost,
}

impl Kind {
    /// The 503 "migrating" answer is asked again.
    fn retries_migrating(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// A connect that timed out is tried again (`_cli_curl`).
    fn retries_connect_timeout(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// The status is read, so a 5xx with an empty body is "no answer".
    fn reads_status(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// `_cli_no_answer` runs.
    fn says_no_answer(self) -> bool {
        !matches!(self, Kind::PostJsonStdin | Kind::SendJsonGet | Kind::SendJsonPost)
    }
    /// A refused passport is tried again with a healed one, on either
    /// channel. Not a body from stdin.
    fn heals(self) -> bool {
        self != Kind::PostJsonStdin
    }
    /// `_permission_note` runs on the answer. Not a body from stdin.
    fn notes_permission(self) -> bool {
        self != Kind::PostJsonStdin
    }
    /// The body is read from stdin: nothing is ever tried twice.
    fn once(self) -> bool {
        self == Kind::PostJsonStdin
    }
}

/// An answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub channel: Channel,
    /// The refusal's closing line, when the shell would note one for this
    /// call ([`permission::note`]). Printing it is the caller's
    /// ([`Note::print`]).
    pub permission: Option<Note>,
}

/// A request the daemon never answered: the shell's one line for it
/// (`_cli_no_answer`), after which the command ends with exit 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoAnswer {
    pub endpoint: String,
    /// Whole seconds since the call began.
    pub waited: u64,
    /// True for an empty answer (the connection closed with nothing, or
    /// a 5xx with an empty body); false for a timeout.
    pub empty: bool,
}

impl fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.empty {
            write!(f, "Error: K2's server gave an empty answer to {} after {} s; run k2 daemon status", self.endpoint, self.waited)
        } else {
            write!(f, "Error: K2's server didn't answer {} within {} s; run k2 daemon status", self.endpoint, self.waited)
        }
    }
}

/// Why a call failed.
#[derive(Debug)]
pub enum Error {
    /// The daemon never answered (see [`NoAnswer`]).
    NoAnswer(NoAnswer),
    /// Nothing accepted the connection, after the tries (curl exit 7).
    Unreachable { endpoint: String, err: io::Error },
    /// No answer in time, on a call that prints no line for it (curl
    /// exit 28: `cli_post_json_stdin`, `cli_send_json`).
    TimedOut { endpoint: String },
    /// The connection closed with nothing, likewise (curl exit 52).
    Empty { endpoint: String },
    /// The exchange broke some other way.
    Broken { endpoint: String, why: String },
    /// A target curl would not send as it is (a space, a control byte or
    /// a `#`).
    BadTarget { target: String, byte: u8 },
    /// A cell-socket call without a passport and its socket.
    NoCellSocket { endpoint: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoAnswer(n) => n.fmt(f),
            Error::Unreachable { endpoint, err } => write!(f, "can't reach K2's server for {endpoint}: {err}"),
            Error::TimedOut { endpoint } => write!(f, "K2's server didn't answer {endpoint} in time"),
            Error::Empty { endpoint } => write!(f, "K2's server gave an empty answer to {endpoint}"),
            Error::Broken { endpoint, why } => write!(f, "the request to {endpoint} failed: {why}"),
            Error::BadTarget { target, byte } => write!(f, "won't send {target:?}: byte 0x{byte:02x} can't be in a URL"),
            Error::NoCellSocket { endpoint } => write!(f, "no session passport and cell socket for {endpoint}"),
        }
    }
}

impl std::error::Error for Error {}

/// The routes the cell socket serves (`_uds_eligible`,
/// `cli/src/cell-socket.sh`). Anything else goes over TCP.
pub fn uds_eligible(endpoint: &str) -> bool {
    const EXACT: &[&str] = &[
        "/cli/workspace/msg",
        "/cli/awareness/publish",
        "/cli/folder-trust/status",
        "/cli/folder-trust/request",
        "/cli/terminal/read",
        "/cli/domains",
        "/cli/domains/names",
        "/cli/domains/names/remove",
        "/cli/certs",
        "/cli/connections",
        "/cli/federation/peers",
        "/cli/federation/peer-roster",
        "/cli/federation/peer-rosters",
        "/cli/federation/send",
        "/cli/heartbeat-log",
        "/cli/thread",
        "/cli/browser/notes",
        "/cli/browser/notes/redeliver",
    ];
    // Owner-only heartbeat routes, before `/cli/heartbeat/*`.
    const NOT: &[&str] = &[
        "/cli/heartbeat/install-launchd",
        "/cli/heartbeat/uninstall-launchd",
        "/cli/heartbeat/apply-wake-scheduler",
        "/cli/heartbeat/list-all",
        "/cli/heartbeat/fires-list-all",
        "/cli/heartbeat/set-show-sessions",
    ];
    const UNDER: &[&str] =
        &["/cli/inbox/", "/cli/review-checklist/", "/cli/dns/", "/cli/certs/", "/cli/publish/", "/cli/heartbeat/", "/cli/thread/"];
    if NOT.contains(&endpoint) {
        return false;
    }
    EXACT.contains(&endpoint) || UNDER.iter().any(|p| endpoint.starts_with(p))
}

/// The client: the shell's connection settings and its per-run state.
pub struct Client {
    /// `HOST` (default `127.0.0.1`).
    pub host: String,
    /// `PORT`.
    pub port: u16,
    /// `PROJECT`, sent escaped as `project=`.
    pub project: String,
    pub auth: Auth,
    /// The passport rule would refuse the call: `X-K5-Would-Refuse: 1`.
    pub would_refuse: bool,
    /// For `User-Agent: k5/<version>` and `X-K5-Cli-Version`.
    pub version: String,
    /// `_cli_skip_uds`: a remote host was applied; never the cell socket.
    pub force_tcp: bool,
    /// `_K2_POST_MAX_TIME`: `cli_post_json` and `cli_post_form` only.
    pub post_max_time: Option<Duration>,
    /// `_K2_SEND_MAX_TIME`: `cli_send_json` only.
    pub send_max_time: Option<Duration>,
    /// Between tries. [`PAUSE`]; a test may shorten it.
    pub pause: Duration,
    /// `daemon.port` in the K2 home: a refused loopback connect tries the
    /// port published there when it differs (`_k2_follow_port_move`).
    pub port_file: Option<PathBuf>,
    /// Where a refused passport may have been minted again.
    pub healer: Option<Box<dyn Healer>>,
    healed: bool,
}

impl Client {
    /// A client for `host:port` with the shell's defaults.
    pub fn new(host: impl Into<String>, port: u16, project: impl Into<String>, auth: Auth) -> Client {
        Client {
            host: host.into(),
            port,
            project: project.into(),
            auth,
            would_refuse: false,
            version: crate::embedded::version().to_string(),
            force_tcp: false,
            post_max_time: None,
            send_max_time: None,
            pause: PAUSE,
            port_file: None,
            healer: None,
            healed: false,
        }
    }

    /// The passport the calls use now (a healed one after a heal).
    pub fn passport(&self) -> Option<&Passport> {
        match &self.auth {
            Auth::Passport { passport, .. } => Some(passport),
            Auth::Owner(_) => None,
        }
    }

    /// One call on one channel: `cli_request`, `cli_request_uds`, and the
    /// others of [`Call`].
    pub fn call(&mut self, channel: Channel, endpoint: &str, call: Call<'_>) -> Result<Response, Error> {
        self.call_from(channel, endpoint, call, Instant::now())
    }

    /// `cli_send`: the cell socket when there is a passport with a live
    /// socket and the route is one it serves, then TCP when that gave no
    /// usable answer (an error, an empty body, or "not served on cell
    /// socket").
    pub fn send(&mut self, endpoint: &str, call: Call<'_>) -> Result<Response, Error> {
        let t0 = Instant::now();
        if self.cell_socket().is_some() && uds_eligible(endpoint) {
            if let Ok(r) = self.call_from(Channel::Uds, endpoint, call, t0) {
                if !r.body.is_empty() && !contains(&r.body, NOT_SERVED) {
                    return Ok(r);
                }
            }
        }
        self.call_from(Channel::Tcp, endpoint, call, t0)
    }

    /// `cli_post_json_stdin`: a JSON body that was read from stdin, over
    /// TCP. Sent once: no tries, no heal, no line, no note.
    pub fn post_json_stdin(&mut self, endpoint: &str, body: &[u8]) -> Result<Response, Error> {
        self.run(Kind::PostJsonStdin, Channel::Tcp, endpoint, &[], body, Instant::now())
    }

    /// `cli_send_json METHOD PATH [payload]`: GET with `payload` as a
    /// pre-encoded query, or POST with it as a JSON body. The cell socket
    /// when there is a passport with a live socket (any route), then TCP
    /// when that failed, said "not served on cell socket" or gave status
    /// 0. The shell prints `<body>\n<status>`; here [`Response`] holds both
    /// and an error stands for its `000`.
    pub fn send_json(&mut self, post: bool, path: &str, payload: &str) -> Result<Response, Error> {
        let kind = if post { Kind::SendJsonPost } else { Kind::SendJsonGet };
        let t0 = Instant::now();
        if self.cell_socket().is_some() {
            if let Ok(r) = self.run(kind, Channel::Uds, path, &[], payload.as_bytes(), t0) {
                if !contains(&r.body, NOT_SERVED) && r.status != 0 {
                    return Ok(r);
                }
            }
        }
        self.run(kind, Channel::Tcp, path, &[], payload.as_bytes(), t0)
    }

    /// The cell socket, when there is a passport, the socket is a socket
    /// (`[ -S ]`) and no remote host forces TCP.
    fn cell_socket(&self) -> Option<&Path> {
        if self.force_tcp {
            return None;
        }
        match &self.auth {
            Auth::Passport { sock: Some(sock), .. } if is_socket(sock) => Some(sock),
            _ => None,
        }
    }

    fn call_from(&mut self, channel: Channel, endpoint: &str, call: Call<'_>, t0: Instant) -> Result<Response, Error> {
        match call {
            Call::Request(args) => self.run(Kind::Request, channel, endpoint, args, &[], t0),
            Call::RequestPost(args) => self.run(Kind::RequestPost, channel, endpoint, args, &[], t0),
            Call::PostForm(args) => self.run(Kind::PostForm, channel, endpoint, args, &[], t0),
            Call::PostJson(body) => self.run(Kind::PostJson, channel, endpoint, &[], body, t0),
        }
    }

    /// One shell request function, all its tries included.
    fn run(&mut self, kind: Kind, channel: Channel, endpoint: &str, args: &[&str], body: &[u8], t0: Instant) -> Result<Response, Error> {
        let (target, request) = self.build(kind, channel, endpoint, args, body)?;
        let max = self.max_time(kind);
        let mut tries = 1;
        let result = loop {
            let r = self.attempt(kind, &target, &request, max);
            if let Ok(a) = &r {
                if kind.retries_migrating() && a.status == 503 && contains(&a.body, MIGRATING) && tries < TRIES {
                    tries += 1;
                    std::thread::sleep(self.pause);
                    continue;
                }
            }
            break r;
        };
        let answer = match result {
            Ok(a) => {
                if kind.says_no_answer() && kind.reads_status() && a.body.is_empty() && (500..600).contains(&a.status) {
                    return Err(Error::NoAnswer(no_answer(endpoint, t0, true)));
                }
                a
            }
            Err(f) => return Err(failure(kind, endpoint, f, t0)),
        };
        if kind == Kind::RequestPost && channel == Channel::Tcp && contains(&answer.body, METHOD_NOT_ALLOWED) {
            return self.run(Kind::Request, Channel::Tcp, endpoint, args, &[], t0);
        }
        if kind.heals() && contains(&answer.body, PASSPORT_REFUSAL) && self.heal() {
            return self.run(kind, channel, endpoint, args, body, t0);
        }
        let permission = if kind.notes_permission() {
            if matches!(kind, Kind::SendJsonGet | Kind::SendJsonPost) {
                let mut text = answer.body.clone();
                text.extend_from_slice(format!("\n{:03}", answer.status).as_bytes());
                permission::note(&text)
            } else {
                permission::note(&answer.body)
            }
        } else {
            None
        };
        Ok(Response { status: answer.status, body: answer.body, channel, permission })
    }

    /// Switches to a healed passport, once per client.
    fn heal(&mut self) -> bool {
        if self.healed {
            return false;
        }
        let Some(healer) = &self.healer else {
            return false;
        };
        let Auth::Passport { passport, .. } = &mut self.auth else {
            return false;
        };
        match healer.healed(passport) {
            Some(new) => {
                *passport = new;
                self.healed = true;
                true
            }
            None => false,
        }
    }

    fn max_time(&self, kind: Kind) -> Duration {
        match kind {
            Kind::PostForm | Kind::PostJson => self.post_max_time.unwrap_or(MAX_TIME),
            Kind::SendJsonGet | Kind::SendJsonPost => self.send_max_time.unwrap_or(SEND_JSON_MAX_TIME),
            Kind::Request | Kind::RequestPost | Kind::PostJsonStdin => MAX_TIME,
        }
    }

    /// `_cli_curl`'s loop: a connect that timed out, again (its kinds).
    fn attempt(&self, kind: Kind, target: &Target, request: &Request, max: Duration) -> Result<Answer, Failure> {
        let mut tries = 1;
        loop {
            let r = self.curl(kind, target, request, max);
            if matches!(r, Err(Failure::ConnectTimedOut)) && kind.retries_connect_timeout() && tries < TRIES {
                tries += 1;
                std::thread::sleep(self.pause);
                continue;
            }
            return r;
        }
    }

    /// `_k2_curl`'s loop: a refused connect, again, following a moved
    /// port; a body from stdin never.
    fn curl(&self, kind: Kind, target: &Target, request: &Request, max: Duration) -> Result<Answer, Failure> {
        let mut target = target.clone();
        let mut request = request.clone();
        let mut tries = 1;
        loop {
            let deadline = Instant::now() + max;
            match wire::connect(&target, CONNECT_TIMEOUT, deadline) {
                Ok(mut conn) => return wire::exchange(&mut conn, &request, deadline),
                Err(Failure::Connect(e)) => {
                    if kind.once() || tries >= TRIES {
                        return Err(Failure::Connect(e));
                    }
                    tries += 1;
                    std::thread::sleep(self.pause);
                    if let Target::Tcp { host, port } = &mut target {
                        if let Some(moved) = self.moved_port(host, *port) {
                            *port = moved;
                            request.host = format!("{host}:{moved}");
                        }
                    }
                }
                Err(f) => return Err(f),
            }
        }
    }

    /// The port in [`Client::port_file`], when the host is loopback and it
    /// differs from `port`.
    fn moved_port(&self, host: &str, port: u16) -> Option<u16> {
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return None;
        }
        let file = self.port_file.as_ref()?;
        if !file.is_file() {
            return None;
        }
        let text = std::fs::read_to_string(file).ok()?;
        let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let p: u16 = digits.parse().ok()?;
        (p != port).then_some(p)
    }

    fn tcp_token(&self) -> &str {
        match &self.auth {
            Auth::Owner(t) => t,
            Auth::Passport { passport, .. } => passport.as_str(),
        }
    }

    /// The request, byte for byte the shell's for this kind and channel.
    fn build(&self, kind: Kind, channel: Channel, endpoint: &str, args: &[&str], body: &[u8]) -> Result<(Target, Request), Error> {
        let mut headers: Vec<(String, String)> = Vec::new();
        let (target, host) = match channel {
            Channel::Tcp => (Target::Tcp { host: self.host.clone(), port: self.port }, format!("{}:{}", self.host, self.port)),
            Channel::Uds => {
                let no = || Error::NoCellSocket { endpoint: endpoint.to_string() };
                let Auth::Passport { passport, sock } = &self.auth else {
                    return Err(no());
                };
                let sock = sock.as_ref().ok_or_else(no)?;
                headers.push(("Authorization".into(), format!("Bearer {}", passport.as_str())));
                (uds_target(sock, endpoint)?, "localhost".to_string())
            }
        };
        if self.would_refuse {
            headers.push((WOULD_REFUSE_HEADER.into(), "1".into()));
        }
        headers.push((VERSION_HEADER.into(), self.version.clone()));
        headers.push((VIA_HEADER.into(), VIA_NATIVE.into()));

        let tcp = channel == Channel::Tcp;
        let token = self.tcp_token();
        let with_project = || format!("token={token}&project={}", quote(&self.project));
        let joined = args.join("&");
        let json = |headers: &mut Vec<(String, String)>| {
            headers.push(("Content-Type".into(), "application/json".into()));
            Some(Body { bytes: body.to_vec(), curl_form_type: false })
        };
        let (method, path, body) = match kind {
            Kind::Request => {
                let path = if tcp {
                    let mut data = with_project();
                    for a in args {
                        data.push('&');
                        data.push_str(a);
                    }
                    wire::append_get_query(endpoint, &data)
                } else if joined.is_empty() {
                    endpoint.to_string()
                } else {
                    wire::append_get_query(endpoint, &joined)
                };
                (Method::Get, path, None)
            }
            Kind::RequestPost => {
                // After `&` when the endpoint has a query already.
                let sep = if endpoint.contains('?') { '&' } else { '?' };
                let path = if tcp {
                    let mut q = format!("{endpoint}{sep}{}", with_project());
                    for a in args {
                        q.push('&');
                        q.push_str(a);
                    }
                    q
                } else if joined.is_empty() {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}{sep}{joined}")
                };
                (Method::Post, path, Some(Body { bytes: Vec::new(), curl_form_type: true }))
            }
            Kind::PostForm => {
                let path = if tcp { format!("{endpoint}?{}", with_project()) } else { endpoint.to_string() };
                headers.push(("Content-Type".into(), wire::FORM_TYPE.into()));
                (Method::Post, path, Some(Body { bytes: joined.into_bytes(), curl_form_type: false }))
            }
            Kind::PostJson | Kind::PostJsonStdin => {
                let path = if tcp { format!("{endpoint}?token={token}") } else { endpoint.to_string() };
                (Method::Post, path, json(&mut headers))
            }
            Kind::SendJsonGet => {
                let payload = String::from_utf8_lossy(body);
                let path = if tcp {
                    let mut data = wire::curl_data_urlencode("token", token);
                    if !payload.is_empty() {
                        data.push('&');
                        data.push_str(&payload);
                    }
                    wire::append_get_query(endpoint, &data)
                } else if payload.is_empty() {
                    endpoint.to_string()
                } else {
                    wire::append_get_query(endpoint, &payload)
                };
                (Method::Get, path, None)
            }
            Kind::SendJsonPost => {
                let path = if tcp { format!("{endpoint}?token={}", quote(token)) } else { endpoint.to_string() };
                (Method::Post, path, json(&mut headers))
            }
        };
        wire::check_target(&path).map_err(|byte| Error::BadTarget { target: path.clone(), byte })?;
        let request = Request { method, target: path, host, user_agent: format!("k5/{}", self.version), headers, body };
        Ok((target, request))
    }
}

#[cfg(unix)]
fn uds_target(sock: &Path, _endpoint: &str) -> Result<Target, Error> {
    Ok(Target::Unix(sock.to_path_buf()))
}

#[cfg(not(unix))]
fn uds_target(_sock: &Path, endpoint: &str) -> Result<Target, Error> {
    Err(Error::NoCellSocket { endpoint: endpoint.to_string() })
}

#[cfg(unix)]
fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

#[cfg(not(unix))]
fn is_socket(_path: &Path) -> bool {
    false
}

/// The error for a failed exchange.
fn failure(kind: Kind, endpoint: &str, f: Failure, t0: Instant) -> Error {
    let endpoint = endpoint.to_string();
    match f {
        Failure::TimedOut | Failure::ConnectTimedOut if kind.says_no_answer() => Error::NoAnswer(no_answer(&endpoint, t0, false)),
        Failure::Empty if kind.says_no_answer() => Error::NoAnswer(no_answer(&endpoint, t0, true)),
        Failure::TimedOut | Failure::ConnectTimedOut => Error::TimedOut { endpoint },
        Failure::Empty => Error::Empty { endpoint },
        Failure::Connect(err) => Error::Unreachable { endpoint, err },
        Failure::Broken(why) => Error::Broken { endpoint, why },
    }
}

fn no_answer(endpoint: &str, t0: Instant, empty: bool) -> NoAnswer {
    NoAnswer { endpoint: endpoint.to_string(), waited: t0.elapsed().as_secs(), empty }
}

fn contains(hay: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_passport_is_none() {
        assert!(Passport::new("").is_none());
        assert_eq!(Passport::new("s.x").map(|p| p.as_str().to_string()), Some("s.x".to_string()));
        // Never printed.
        assert_eq!(format!("{:?}", Passport::new("s.secret").unwrap()), "Passport(..)");
        assert_eq!(format!("{:?}", Auth::Owner("owner-secret".into())), "Owner(..)");
    }

    #[test]
    fn the_no_answer_lines_are_the_shell_s() {
        let n = NoAnswer { endpoint: "/cli/inbox/list".into(), waited: 30, empty: false };
        assert_eq!(n.to_string(), "Error: K2's server didn't answer /cli/inbox/list within 30 s; run k2 daemon status");
        let n = NoAnswer { endpoint: "/cli/inbox/list".into(), waited: 0, empty: true };
        assert_eq!(n.to_string(), "Error: K2's server gave an empty answer to /cli/inbox/list after 0 s; run k2 daemon status");
    }

    #[test]
    fn the_kinds_behave_as_their_shell_functions() {
        use Kind::*;
        // (kind, migrating, connect timeout, status, no-answer line,
        // heals, note, once)
        let rows = [
            (Request, true, true, true, true, true, true, false),
            (RequestPost, true, true, true, true, true, true, false),
            (PostForm, false, false, false, true, true, true, false),
            (PostJson, false, false, false, true, true, true, false),
            (PostJsonStdin, false, false, false, false, false, false, true),
            (SendJsonGet, false, false, false, false, true, true, false),
            (SendJsonPost, false, false, false, false, true, true, false),
        ];
        for (k, m, c, s, n, h, p, o) in rows {
            assert_eq!(
                (
                    k.retries_migrating(),
                    k.retries_connect_timeout(),
                    k.reads_status(),
                    k.says_no_answer(),
                    k.heals(),
                    k.notes_permission(),
                    k.once()
                ),
                (m, c, s, n, h, p, o),
                "{k:?}"
            );
        }
    }

    #[test]
    fn the_time_limits() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert_eq!(c.max_time(Kind::Request), Duration::from_secs(30));
        assert_eq!(c.max_time(Kind::PostJson), Duration::from_secs(30));
        assert_eq!(c.max_time(Kind::SendJsonGet), Duration::from_secs(60));
        c.post_max_time = Some(Duration::from_secs(150));
        c.send_max_time = Some(Duration::from_secs(330));
        for (k, want) in [
            (Kind::Request, 30),
            (Kind::RequestPost, 30),
            (Kind::PostForm, 150),
            (Kind::PostJson, 150),
            (Kind::PostJsonStdin, 30),
            (Kind::SendJsonGet, 330),
            (Kind::SendJsonPost, 330),
        ] {
            assert_eq!(c.max_time(k), Duration::from_secs(want), "{k:?}");
        }
    }

    #[test]
    fn the_owner_has_no_cell_socket_arm() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(c.call(Channel::Uds, "/cli/inbox/list", Call::Request(&[])), Err(Error::NoCellSocket { .. })));
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Passport { passport: Passport::new("s.x").unwrap(), sock: None });
        assert!(matches!(c.call(Channel::Uds, "/cli/inbox/list", Call::Request(&[])), Err(Error::NoCellSocket { .. })));
    }

    #[test]
    fn a_target_curl_refuses_is_not_sent() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(c.call(Channel::Tcp, "/cli/x", Call::Request(&["a=b c"])), Err(Error::BadTarget { byte: b' ', .. })));
        assert!(matches!(c.call(Channel::Tcp, "/cli/x#y", Call::PostJson(b"{}")), Err(Error::BadTarget { byte: b'#', .. })));
    }

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("k5-client-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_healed_passport_rule() {
        let dir = scratch("heal");
        let healer = SecretsDir(dir.clone());
        let cur = Passport::new("sessA.oldsecret").unwrap();
        let file = dir.join(".k2-passport");
        assert_eq!(healer.healed(&cur), None, "no file");
        for (content, want) in [
            ("sessA.newsecret\n", Some("sessA.newsecret")),
            ("sessA.newsecret", Some("sessA.newsecret")),
            ("sessA.newsecret\nignored\n", Some("sessA.newsecret")),
            ("sessA.oldsecret\n", None),
            ("sessB.newsecret\n", None),
            ("sessA.new-secret\n", None),
            ("sessA.new.secret\n", None),
            ("sessA.\n", None),
            (".newsecret\n", None),
            ("sessA.newsecret\r\n", None),
            (" sessA.newsecret\n", None),
            ("", None),
            ("ses-sA.x\n", None),
        ] {
            std::fs::write(&file, content).unwrap();
            assert_eq!(healer.healed(&cur).map(|p| p.as_str().to_string()), want.map(str::to_string), "{content:?}");
        }
        // An owner-shaped token (no dot) never heals.
        std::fs::write(&file, "sessA.newsecret\n").unwrap();
        assert_eq!(healer.healed(&Passport::new("ownertoken").unwrap()), None);
        // A dash in the session part is fine.
        std::fs::write(&file, "s-1.b2\n").unwrap();
        assert_eq!(healer.healed(&Passport::new("s-1.a1").unwrap()).map(|p| p.as_str().to_string()), Some("s-1.b2".to_string()));
        // A link is not followed; a folder is not a file.
        #[cfg(unix)]
        {
            let real = dir.join("real");
            std::fs::write(&real, "sessA.newsecret\n").unwrap();
            std::fs::remove_file(&file).unwrap();
            std::os::unix::fs::symlink(&real, &file).unwrap();
            assert_eq!(healer.healed(&cur), None, "a link");
            std::fs::remove_file(&file).unwrap();
        }
        std::fs::create_dir(&file).unwrap();
        assert_eq!(healer.healed(&cur), None, "a folder");
        assert_eq!(SecretsDir(PathBuf::new()).healed(&cur), None, "no folder set");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_moved_port_rule() {
        let dir = scratch("port");
        let file = dir.join("daemon.port");
        let mut c = Client::new("127.0.0.1", 4000, "/p", Auth::Owner("o".into()));
        assert_eq!(c.moved_port("127.0.0.1", 4000), None, "no file set");
        c.port_file = Some(file.clone());
        assert_eq!(c.moved_port("127.0.0.1", 4000), None, "no file");
        for (content, host, want) in [
            ("4001\n", "127.0.0.1", Some(4001)),
            (" 40 01 \n", "localhost", Some(4001)),
            ("4001", "::1", Some(4001)),
            ("4000\n", "127.0.0.1", None),
            ("4001\n", "10.0.0.2", None),
            ("", "127.0.0.1", None),
            ("40a1", "127.0.0.1", None),
            ("-4001", "127.0.0.1", None),
            ("99999", "127.0.0.1", None),
        ] {
            std::fs::write(&file, content).unwrap();
            assert_eq!(c.moved_port(host, 4000), want, "{content:?} {host}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
