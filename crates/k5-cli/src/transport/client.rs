//! The calls: the shell CLI's request functions, each with the request it
//! sends, its tries and its time limits (the shapes are in
//! [`crate::transport`]).

use super::encode::quote;
use super::permission::{self, Note};
use super::skew;
use super::wire::{self, Answer, Body, Failure, Method, Request, Target};
use k2_client::brand::PRODUCT_NAME;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--connect-timeout 5`, every call but the passport probe.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// `--max-time 30`, and the default of `_K2_POST_MAX_TIME`.
pub const MAX_TIME: Duration = Duration::from_secs(30);
/// The passport probe's `--connect-timeout 3` (`_cli_scoped_token_rejected`).
pub const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The passport probe's `--max-time 5`.
pub const PROBE_MAX_TIME: Duration = Duration::from_secs(5);
/// Where the passport probe asks.
pub const PROBE_ENDPOINT: &str = "/cli/whoami";
/// `cli_send_json`'s default, `_K2_SEND_MAX_TIME`.
pub const SEND_JSON_MAX_TIME: Duration = Duration::from_secs(60);
/// `_sidecar_send`'s `--max-time 120` (`cli/src/sidecar.sh`).
pub const SIDECAR_MAX_TIME: Duration = Duration::from_secs(120);
/// `_cli_use_passport_project`'s `--max-time 15` (`cli/src/cell-socket.sh`).
pub const PASSPORT_PROJECT_MAX_TIME: Duration = Duration::from_secs(15);
/// Where `_permission_probe` asks whether permission cards are on
/// (`cli/src/permission.sh`).
pub const CARDS_ENDPOINT: &str = "/cli/permission/enabled";
/// `_permission_probe`'s `--connect-timeout 1 --max-time 1`.
pub const CARDS_TIME: Duration = Duration::from_secs(1);
/// `k2 update --cli`'s `--connect-timeout 2` (`_update_daemon_version`,
/// `cli/src/update.sh`).
pub const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// `k2 update --cli`'s `--max-time 4`.
pub const UPDATE_MAX_TIME: Duration = Duration::from_secs(4);
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
/// Set to `disk` on a call made as the owner with the token read from
/// the K2 home's files, never with a passport (slice 10).
pub const OWNER_SOURCE_HEADER: &str = "X-K5-Owner-Source";
/// Its one value.
pub const OWNER_SOURCE_DISK: &str = "disk";

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
    /// No credential: only [`Client::get_public`] goes out. Every other
    /// call is [`Error::NoCredential`], never sent.
    None,
    /// No passport: the owner token, over TCP only.
    Owner(String),
    /// A passport: the Bearer on the cell socket at `sock`, and over TCP.
    Passport {
        passport: Passport,
        sock: Option<PathBuf>,
    },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::None => f.write_str("None"),
            Auth::Owner(_) => f.write_str("Owner(..)"),
            Auth::Passport { sock, .. } => f
                .debug_struct("Passport")
                .field("sock", sock)
                .finish_non_exhaustive(),
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
            && session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !secret.is_empty()
            && secret.bytes().all(|b| b.is_ascii_alphanumeric());
        let same_session = cur.split('.').next() == Some(session);
        (shaped && t != cur && same_session).then(|| Passport(t.to_string()))
    }
}

/// The channel a request went on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Loopback TCP, the credential as Bearer (never in the query).
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

impl Call<'_> {
    /// A POST: it may change something, so once any of it went out on the
    /// cell socket it is never sent again over TCP.
    fn is_post(self) -> bool {
        !matches!(self, Call::Request(_))
    }
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
    /// A GET with no credential and no query of ours, over TCP: what
    /// anyone may read (`/boot-status`).
    Public,
    /// `_cli_scoped_token_rejected`: a GET of the endpoint as it is, over
    /// TCP, the passport as Bearer and no query of ours. Only `_k2_curl`'s
    /// tries (a refused connect); nothing else.
    Probe,
    /// `k2 respond`'s TCP arm (`cli/src/respond.sh`): a form POST over
    /// TCP, the passport as Bearer, no query of ours. Only `_k2_curl`'s
    /// tries; nothing read back, so no heal, no line, no note.
    BearerForm,
    /// `_sidecar_send GET` (`cli/src/sidecar.sh`): `cli_send_json GET`'s
    /// request, at most 120 s, and nothing else: no heal, no line, and no
    /// note (the caller notes a refusal itself).
    SidecarGet,
    /// `_cli_use_passport_project`'s `/cli/whoami` (`cli/src/cell-socket.sh`):
    /// `cli_send_json GET`'s request with no query of the caller's, at
    /// most 15 s; no heal, no line, no note.
    PassportProject,
    /// `_permission_probe` (`cli/src/permission.sh`): a GET of
    /// [`CARDS_ENDPOINT`] with `-G` and no data of the caller's, on the
    /// cell socket (the passport as Bearer) or over TCP (the token as
    /// Bearer), never both. Connect 1 s, at
    /// most 1 s; only `_k2_curl`'s tries (a refused connect, and a moved
    /// port over TCP); no heal, no line, no note.
    Cards,
    /// `_update_daemon_version` (`cli/src/update.sh`): `_K2_CURL_TRIES=1
    /// _k2_curl "" -s --connect-timeout 2 --max-time 4` of
    /// `http://127.0.0.1:$PORT/boot-status`. A GET with no credential and
    /// no query, over TCP, one try (a refused connect is not tried again),
    /// with the hand-over headers `_k2_curl` adds (the owner's source
    /// too); no heal, no line, no note.
    UpdateVersion,
    /// `_setup_send GET` (`cli/src/setup.sh`): `_k2_curl` with a config
    /// `url` of `<ep>[?format=text]` and the token as Bearer, `-s
    /// --connect-timeout 5 --max-time 30`, over TCP. One exchange with
    /// `_k2_curl`'s tries (a refused connect, a moved port); the 503
    /// "migrating" tries and the one heal are `_setup_send`'s own loop
    /// ([`Client::setup_send`]); no line, no note (the caller notes the
    /// answer as the shell's `_setup_render` holds it).
    SetupGet,
    /// `_setup_send POST`: the same, `-X POST`, the config's `data-raw`
    /// body and `Content-Type: application/json`.
    SetupPost,
}

impl Kind {
    /// The 503 "migrating" answer is asked again.
    fn retries_migrating(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// A connect that timed out is tried again (`_cli_curl`).
    fn retries_connect_timeout(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost | Kind::Public)
    }
    /// A failure goes through `_cli_curl`'s local check
    /// ([`local_failure`]): the two functions that run curl through it.
    fn checks_local_denial(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// The status is read, so a 5xx with an empty body is "no answer".
    fn reads_status(self) -> bool {
        matches!(self, Kind::Request | Kind::RequestPost)
    }
    /// `_cli_no_answer` runs.
    fn says_no_answer(self) -> bool {
        !matches!(
            self,
            Kind::PostJsonStdin
                | Kind::SendJsonGet
                | Kind::SendJsonPost
                | Kind::Probe
                | Kind::BearerForm
                | Kind::SidecarGet
                | Kind::PassportProject
                | Kind::Cards
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
        )
    }
    /// A refused passport is tried again with a healed one, on either
    /// channel. Not a body from stdin, a call with no credential, nor the
    /// probe.
    fn heals(self) -> bool {
        !matches!(
            self,
            Kind::PostJsonStdin
                | Kind::Public
                | Kind::Probe
                | Kind::BearerForm
                | Kind::SidecarGet
                | Kind::PassportProject
                | Kind::Cards
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
        )
    }
    /// `_permission_note` runs on the answer. Not a body from stdin, a
    /// call with no credential, nor the probe.
    fn notes_permission(self) -> bool {
        !matches!(
            self,
            Kind::PostJsonStdin
                | Kind::Public
                | Kind::Probe
                | Kind::BearerForm
                | Kind::SidecarGet
                | Kind::PassportProject
                | Kind::Cards
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
        )
    }
    /// `--connect-timeout`.
    fn connect_timeout(self) -> Duration {
        match self {
            Kind::Probe => PROBE_CONNECT_TIMEOUT,
            Kind::Cards => CARDS_TIME,
            Kind::UpdateVersion => UPDATE_CONNECT_TIMEOUT,
            _ => CONNECT_TIMEOUT,
        }
    }
    /// Nothing is ever tried twice: the body is read from stdin (the
    /// shell's `@-` rule is an argument that is exactly `@-`, never one in
    /// a URL), or `_K2_CURL_TRIES=1`.
    fn once(self) -> bool {
        matches!(self, Kind::PostJsonStdin | Kind::UpdateVersion)
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
    /// The daemon's version from the answer's head, when it named one
    /// shaped like a version ([`skew::clean`]). The client has already
    /// compared it with its own ([`skew::note`]).
    pub server_version: Option<String>,
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
            write!(f, "Error: {PRODUCT_NAME}'s server gave an empty answer to {} after {} s; run k2 daemon status", self.endpoint, self.waited)
        } else {
            write!(
                f,
                "Error: {PRODUCT_NAME}'s server didn't answer {} within {} s; run k2 daemon status",
                self.endpoint, self.waited
            )
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
    /// The exchange broke some other way; `curl` is curl's exit code for
    /// it ([`wire::curl`]).
    Broken {
        endpoint: String,
        why: String,
        curl: i32,
    },
    /// A target curl would not send as it is (a space, a control byte or
    /// a `#`).
    BadTarget { target: String, byte: u8 },
    /// A cell-socket call without a passport and its socket.
    NoCellSocket { endpoint: String },
    /// A call that needs a credential, on a client with none
    /// ([`Auth::None`]).
    NoCredential { endpoint: String },
    /// A POST went out on the cell socket and no usable answer came back
    /// (no answer in time, a connection closed or reset before the answer
    /// ended, or a 5xx with an empty body). The daemon may have run it,
    /// so it is not sent again over TCP.
    MaybeApplied { endpoint: String, waited: u64 },
    /// `_cli_curl`'s local refusal: the call failed and this machine, not
    /// the daemon, is the likely cause (the OS refused the connect, or
    /// `TMPDIR` can't be used), so the shell returns `code` in place of
    /// curl's ([`LOCAL_TRANSPORT_DENIED`] or [`TMPDIR_UNWRITABLE`]) and no
    /// no-answer line. See [`local_failure`].
    Denied { endpoint: String, code: i32 },
}

/// `_CLI_LOCAL_TRANSPORT_DENIED` (`cli/src/cli-tools-policy.sh`): the code
/// `_cli_curl` returns in place of curl's 7 when the OS refused the
/// connect on this machine. Nothing was sent.
pub const LOCAL_TRANSPORT_DENIED: i32 = 75;

/// `_CLI_TMPDIR_UNWRITABLE`: the code `_cli_curl` returns in place of
/// curl's when it failed while `TMPDIR` can't be used. The request may
/// have gone out.
pub const TMPDIR_UNWRITABLE: i32 = 76;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoAnswer(n) => n.fmt(f),
            Error::Unreachable { endpoint, err } => write!(f, "can't reach {PRODUCT_NAME}'s server for {endpoint}: {err}"),
            Error::TimedOut { endpoint } => write!(f, "{PRODUCT_NAME}'s server didn't answer {endpoint} in time"),
            Error::Empty { endpoint } => write!(f, "{PRODUCT_NAME}'s server gave an empty answer to {endpoint}"),
            Error::Broken { endpoint, why, .. } => write!(f, "the request to {endpoint} failed: {why}"),
            Error::BadTarget { target, byte } => write!(f, "won't send {target:?}: byte 0x{byte:02x} can't be in a URL"),
            Error::NoCellSocket { endpoint } => write!(f, "no session passport and cell socket for {endpoint}"),
            Error::NoCredential { endpoint } => write!(f, "no owner token or session passport for {endpoint}"),
            Error::MaybeApplied { endpoint, waited } => write!(
                f,
                "Error: {PRODUCT_NAME}'s server didn't answer {endpoint} ({waited} s); the request may have been applied. Check before running it again."
            ),
            Error::Denied { endpoint, .. } => write!(f, "this machine refused the request to {endpoint} (the OS, or TMPDIR)"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The code the shell's call returns for the same failure: curl's own
    /// (a call whose last step is curl ends with it, under `set -e`), or
    /// [`LOCAL_TRANSPORT_DENIED`]. A failure with a no-answer line ends
    /// the command with exit 1 after it; its code here is curl's (28 for
    /// no answer in time, 52 for an empty one). The ones that never reach
    /// curl are 1.
    pub fn curl_code(&self) -> i32 {
        match self {
            Error::NoAnswer(n) if n.empty => wire::curl::GOT_NOTHING,
            Error::NoAnswer(_) | Error::TimedOut { .. } => wire::curl::OPERATION_TIMEDOUT,
            Error::Unreachable { .. } => wire::curl::COULDNT_CONNECT,
            Error::Empty { .. } => wire::curl::GOT_NOTHING,
            Error::Broken { curl, .. } => *curl,
            Error::BadTarget { .. } => wire::curl::URL_MALFORMAT,
            Error::Denied { code, .. } => *code,
            Error::NoCellSocket { .. }
            | Error::NoCredential { .. }
            | Error::MaybeApplied { .. } => 1,
        }
    }
}

/// A call that failed. Whether a POST on the cell socket goes on to TCP
/// is read from its curl code, as the shell's `cli_send` reads `$rc`.
#[derive(Debug)]
struct Failed {
    error: Error,
}

impl Failed {
    fn unsent(error: Error) -> Failed {
        Failed { error }
    }
}

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
    const UNDER: &[&str] = &[
        "/cli/inbox/",
        "/cli/review-checklist/",
        "/cli/dns/",
        "/cli/certs/",
        "/cli/publish/",
        "/cli/heartbeat/",
        "/cli/thread/",
    ];
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
    /// The owner token came from the K2 home's files:
    /// `X-K5-Owner-Source: disk` on every call made with it. Never sent
    /// with a passport or no credential.
    pub owner_from_disk: bool,
    /// For `User-Agent: k5/<version>` and `X-K5-Cli-Version`, and the
    /// version a server's is compared with ([`skew`]).
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
    /// Refused connects `_k2_curl` tried again before the last answer.
    refused: std::cell::Cell<u32>,
}

impl Client {
    /// A client for `host:port` with the shell's defaults.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        project: impl Into<String>,
        auth: Auth,
    ) -> Client {
        Client {
            host: host.into(),
            port,
            project: project.into(),
            auth,
            would_refuse: false,
            owner_from_disk: false,
            version: crate::embedded::version().to_string(),
            force_tcp: false,
            post_max_time: None,
            send_max_time: None,
            pause: PAUSE,
            port_file: None,
            healer: None,
            healed: false,
            refused: std::cell::Cell::new(0),
        }
    }

    /// `_cli_use_healed_passport`: switch to the passport the daemon minted
    /// again for this session, when [`Client::healer`] has one. The shell
    /// does it as it starts (before any request) and again before the
    /// probe; neither counts as the one heal on a refusal. False when
    /// there is none to switch to.
    pub fn use_healed_passport(&mut self) -> bool {
        let Some(healer) = &self.healer else {
            return false;
        };
        let Auth::Passport { passport, .. } = &mut self.auth else {
            return false;
        };
        match healer.healed(passport) {
            Some(new) => {
                *passport = new;
                true
            }
            None => false,
        }
    }

    /// `_cli_scoped_token_rejected`: one GET of [`PROBE_ENDPOINT`] over
    /// TCP with the passport as Bearer (no query, connect 3 s, at most
    /// 5 s; a refused connect is tried again as `_k2_curl` does). True
    /// only when the answer holds the daemon's passport refusal; false
    /// with no passport, or when the probe failed.
    pub fn passport_rejected(&mut self) -> bool {
        if self.passport().is_none() {
            return false;
        }
        match self.run(
            Kind::Probe,
            Channel::Tcp,
            PROBE_ENDPOINT,
            &[],
            &[],
            Instant::now(),
        ) {
            Ok(r) => contains(&r.body, PASSPORT_REFUSAL),
            Err(_) => false,
        }
    }

    /// `_permission_probe "$SCOPED_TOKEN"`'s request ([`Kind::Cards`]):
    /// on the cell socket when [`Client::has_cell_socket`], else over TCP.
    /// The caller reads the answer (the shell's Python: a 200 whose body
    /// is a JSON object with `"enabled": true`). Needs a passport.
    pub fn permission_enabled(&mut self) -> Result<Response, Error> {
        let channel = if self.has_cell_socket() {
            Channel::Uds
        } else {
            Channel::Tcp
        };
        self.run(
            Kind::Cards,
            channel,
            CARDS_ENDPOINT,
            &[],
            &[],
            Instant::now(),
        )
        .map_err(|f| f.error)
    }

    /// How many refused connects the last exchange tried again before it
    /// got through (`_k2_curl`'s loop). A call whose shell reads curl's
    /// `-w` output from `$(…)` sees one `-w` line per try, the failed ones
    /// first.
    pub fn refused_before_answer(&self) -> u32 {
        self.refused.get()
    }

    /// There is a passport, its cell socket is a socket (`[ -S ]`) and no
    /// remote host forces TCP.
    pub fn has_cell_socket(&self) -> bool {
        self.cell_socket().is_some()
    }

    /// The passport the calls use now (a healed one after a heal).
    pub fn passport(&self) -> Option<&Passport> {
        match &self.auth {
            Auth::Passport { passport, .. } => Some(passport),
            Auth::Owner(_) | Auth::None => None,
        }
    }

    /// One call on one channel: `cli_request`, `cli_request_uds`, and the
    /// others of [`Call`].
    pub fn call(
        &mut self,
        channel: Channel,
        endpoint: &str,
        call: Call<'_>,
    ) -> Result<Response, Error> {
        self.call_from(channel, endpoint, call, Instant::now())
            .map_err(|f| f.error)
    }

    /// `cli_send`: the cell socket when there is a passport with a live
    /// socket and the route is one it serves; its answer when it has one
    /// (a body of only newlines and NULs is none: the shell holds it in
    /// `$(…)`) that is not "not served on cell socket". Else a GET goes on
    /// to TCP, and a POST only when the connect failed (curl 7, or 75 when
    /// the OS refused it) or the daemon said "not served on cell socket"
    /// (no handler ran). Any other end of a POST there (28, 52, 56, an
    /// empty answer, 76, ...) may have run it: [`Error::MaybeApplied`],
    /// never sent again.
    pub fn send(&mut self, endpoint: &str, call: Call<'_>) -> Result<Response, Error> {
        let t0 = Instant::now();
        if self.cell_socket().is_some() && uds_eligible(endpoint) {
            match self.call_from(Channel::Uds, endpoint, call, t0) {
                Ok(r) if contains(&r.body, NOT_SERVED) => {}
                Ok(r) if r.body.iter().any(|b| !matches!(b, b'\n' | 0)) => return Ok(r),
                Ok(_) if call.is_post() => return Err(maybe_applied(endpoint, t0)),
                Ok(_) => {}
                Err(f)
                    if call.is_post()
                        && !matches!(
                            f.error.curl_code(),
                            wire::curl::COULDNT_CONNECT | LOCAL_TRANSPORT_DENIED
                        ) =>
                {
                    return Err(maybe_applied(endpoint, t0))
                }
                Err(_) => {}
            }
        }
        self.call_from(Channel::Tcp, endpoint, call, t0)
            .map_err(|f| f.error)
    }

    /// A GET of `path` as it is, over TCP, with no credential and no query
    /// of ours: no `project=`, no Bearer. For what the daemon
    /// shows anyone on this machine (`/boot-status`). A refused or timed
    /// out connect is tried again as `cli_request`'s is; no heal, no note.
    pub fn get_public(&mut self, path: &str) -> Result<Response, Error> {
        self.run(Kind::Public, Channel::Tcp, path, &[], &[], Instant::now())
            .map_err(|f| f.error)
    }

    /// `k2 update --cli`'s GET of `path` ([`Kind::UpdateVersion`]): over
    /// TCP to this client's host and port, no credential, no query, one
    /// try, connect 2 s, at most 4 s; no heal, no line, no note.
    pub fn get_update_version(&mut self, path: &str) -> Result<Response, Error> {
        self.run(
            Kind::UpdateVersion,
            Channel::Tcp,
            path,
            &[],
            &[],
            Instant::now(),
        )
        .map_err(|f| f.error)
    }

    /// `_setup_send <GET|POST> <ep> <as_json> [<body>]` (`cli/src/setup.sh`):
    /// `ep`, then `?format=text` when `text`, the token as Bearer, over TCP; a POST
    /// carries `body` as JSON. Each exchange is `_k2_curl`'s (a refused
    /// connect is tried again, [`Client::refused_before_answer`] counts
    /// those of the last one). Then `_setup_send`'s loop: a GET whose
    /// answer is a 503 holding `"state":"migrating"` is asked again, a
    /// second apart, at most [`TRIES`] tries in all; an answer holding the
    /// passport refusal is asked again once per client with a healed
    /// passport (the migrating count goes on, as the shell's does). Any
    /// status is an answer; nothing is noted and no line is printed.
    pub fn setup_send(
        &mut self,
        post: bool,
        endpoint: &str,
        text: bool,
        body: &[u8],
    ) -> Result<Response, Error> {
        let kind = if post {
            Kind::SetupPost
        } else {
            Kind::SetupGet
        };
        let args: &[&str] = if text { &["format=text"] } else { &[] };
        let t0 = Instant::now();
        let mut tries = 1;
        loop {
            let r = self
                .run(kind, Channel::Tcp, endpoint, args, body, t0)
                .map_err(|f| f.error)?;
            if !post && r.status == 503 && contains(&r.body, MIGRATING) && tries < TRIES {
                tries += 1;
                std::thread::sleep(self.pause);
                continue;
            }
            if contains(&r.body, PASSPORT_REFUSAL) && self.heal() {
                continue;
            }
            return Ok(r);
        }
    }

    /// `cli_post_json_stdin`: a JSON body that was read from stdin, over
    /// TCP. Sent once: no tries, no heal, no line, no note.
    pub fn post_json_stdin(&mut self, endpoint: &str, body: &[u8]) -> Result<Response, Error> {
        self.run(
            Kind::PostJsonStdin,
            Channel::Tcp,
            endpoint,
            &[],
            body,
            Instant::now(),
        )
        .map_err(|f| f.error)
    }

    /// `k2 respond`'s TCP arm: `args` joined by `&` as a form body, POSTed
    /// to `endpoint` as it is over TCP with the passport as Bearer (a
    /// config line, before the shell's `-H`s), no `project=`. `_k2_curl` with `-s -X POST`, connect 5 s, at most
    /// 30 s; only a refused connect is tried again. Nothing is read back:
    /// no heal, no no-answer line, no note. Needs a passport.
    pub fn post_form_bearer(&mut self, endpoint: &str, args: &[&str]) -> Result<Response, Error> {
        self.run(
            Kind::BearerForm,
            Channel::Tcp,
            endpoint,
            args,
            &[],
            Instant::now(),
        )
        .map_err(|f| f.error)
    }

    /// `cli_send_json METHOD PATH [payload]`: GET with `payload` as a
    /// pre-encoded query, or POST with it as a JSON body. The cell socket
    /// when there is a passport with a live socket (any route), then TCP
    /// when that failed, said "not served on cell socket" or gave status
    /// 0. The shell prints `<body>\n<status>`; here [`Response`] holds both
    /// and an error stands for its `000`. A POST that went out on the cell
    /// socket and failed is [`Error::MaybeApplied`], never sent again over
    /// TCP; a GET goes to TCP, as the shell's does.
    pub fn send_json(&mut self, post: bool, path: &str, payload: &str) -> Result<Response, Error> {
        let kind = if post {
            Kind::SendJsonPost
        } else {
            Kind::SendJsonGet
        };
        self.send_json_as(kind, path, payload)
    }

    /// `_sidecar_send GET <path> <query>`: `cli_send_json GET`'s requests
    /// and channels (the cell socket, then TCP when that failed, said "not
    /// served on cell socket" or gave status 0), at most 120 s each, with
    /// no heal and no note. The shell prints `<body>\n<status>`; an error
    /// here stands for its `000`.
    pub fn sidecar_get(&mut self, path: &str, payload: &str) -> Result<Response, Error> {
        self.send_json_as(Kind::SidecarGet, path, payload)
    }

    /// `_cli_use_passport_project`'s request: `/cli/whoami` with the
    /// passport, on the cell socket when there is a live one, then over
    /// TCP (the passport as Bearer) when that failed or its answer, as
    /// `$(…)` holds it, is empty or says "not served on cell socket".
    /// At most 15 s each; no heal, no line, no note. What `out` holds
    /// after it: the answer's bytes with the NULs and trailing newlines
    /// dropped, or nothing when it failed. Needs a passport.
    pub fn passport_project(&mut self) -> Vec<u8> {
        let held = |r: &Response| {
            let mut b: Vec<u8> = r.body.iter().copied().filter(|b| *b != 0).collect();
            while b.last() == Some(&b'\n') {
                b.pop();
            }
            b
        };
        let t0 = Instant::now();
        let kind = Kind::PassportProject;
        if self.cell_socket().is_some() {
            if let Ok(r) = self.run(kind, Channel::Uds, PROBE_ENDPOINT, &[], &[], t0) {
                let out = held(&r);
                if !out.is_empty() && !contains(&out, NOT_SERVED) {
                    return out;
                }
            }
        }
        match self.run(kind, Channel::Tcp, PROBE_ENDPOINT, &[], &[], t0) {
            Ok(r) => held(&r),
            Err(_) => Vec::new(),
        }
    }

    fn send_json_as(&mut self, kind: Kind, path: &str, payload: &str) -> Result<Response, Error> {
        let post = kind == Kind::SendJsonPost;
        let t0 = Instant::now();
        if self.cell_socket().is_some() {
            // A POST goes on to TCP only after a failed connect (curl 7) or
            // "not served on cell socket", as `cli_send`'s.
            match self.run(kind, Channel::Uds, path, &[], payload.as_bytes(), t0) {
                Ok(r) if contains(&r.body, NOT_SERVED) => {}
                Ok(r) if r.status != 0 => return Ok(r),
                Ok(_) if post => return Err(maybe_applied(path, t0)),
                Ok(_) => {}
                Err(f) if post && f.error.curl_code() != wire::curl::COULDNT_CONNECT => {
                    return Err(maybe_applied(path, t0))
                }
                Err(_) => {}
            }
        }
        self.run(kind, Channel::Tcp, path, &[], payload.as_bytes(), t0)
            .map_err(|f| f.error)
    }

    /// The cell socket, when there is a passport, the socket is a socket
    /// (`[ -S ]`) and no remote host forces TCP.
    fn cell_socket(&self) -> Option<&Path> {
        if self.force_tcp {
            return None;
        }
        match &self.auth {
            Auth::Passport {
                sock: Some(sock), ..
            } if is_socket(sock) => Some(sock),
            _ => None,
        }
    }

    fn call_from(
        &mut self,
        channel: Channel,
        endpoint: &str,
        call: Call<'_>,
        t0: Instant,
    ) -> Result<Response, Failed> {
        match call {
            Call::Request(args) => self.run(Kind::Request, channel, endpoint, args, &[], t0),
            Call::RequestPost(args) => {
                self.run(Kind::RequestPost, channel, endpoint, args, &[], t0)
            }
            Call::PostForm(args) => self.run(Kind::PostForm, channel, endpoint, args, &[], t0),
            Call::PostJson(body) => self.run(Kind::PostJson, channel, endpoint, &[], body, t0),
        }
    }

    /// One shell request function, all its tries included.
    fn run(
        &mut self,
        kind: Kind,
        channel: Channel,
        endpoint: &str,
        args: &[&str],
        body: &[u8],
        t0: Instant,
    ) -> Result<Response, Failed> {
        let (target, request) = match self.build(kind, channel, endpoint, args, body) {
            Ok(built) => built,
            // The shell hands curl such a URL, and curl refuses it (3)
            // before it connects: `_cli_curl`'s check still runs.
            Err(Error::BadTarget { .. }) if kind.checks_local_denial() && tmpdir_unusable() => {
                return Err(Failed::unsent(Error::Denied {
                    endpoint: endpoint.to_string(),
                    code: TMPDIR_UNWRITABLE,
                }));
            }
            Err(e) => return Err(Failed::unsent(e)),
        };
        let max = self.max_time(kind);
        let mut tries = 1;
        let result = loop {
            let r = self.attempt(kind, &target, &request, max);
            if let Ok(a) = &r {
                if kind.retries_migrating()
                    && a.status == 503
                    && contains(&a.body, MIGRATING)
                    && tries < TRIES
                {
                    tries += 1;
                    std::thread::sleep(self.pause);
                    continue;
                }
            }
            break r;
        };
        let answer = match result {
            Ok(a) => {
                if kind.says_no_answer()
                    && kind.reads_status()
                    && a.body.is_empty()
                    && (500..600).contains(&a.status)
                {
                    return Err(Failed {
                        error: Error::NoAnswer(no_answer(endpoint, t0, true)),
                    });
                }
                a
            }
            Err(f) => {
                if let Some(code) = kind
                    .checks_local_denial()
                    .then(|| local_failure(&f, &self.host))
                    .flatten()
                {
                    return Err(Failed {
                        error: Error::Denied {
                            endpoint: endpoint.to_string(),
                            code,
                        },
                    });
                }
                return Err(Failed {
                    error: failure(kind, endpoint, f, t0),
                });
            }
        };
        if kind == Kind::RequestPost
            && channel == Channel::Tcp
            && contains(&answer.body, METHOD_NOT_ALLOWED)
        {
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
        let server_version = answer
            .server_version
            .as_deref()
            .and_then(skew::clean)
            .map(str::to_string);
        Ok(Response {
            status: answer.status,
            body: answer.body,
            channel,
            permission,
            server_version,
        })
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
            Kind::SendJsonGet | Kind::SendJsonPost => {
                self.send_max_time.unwrap_or(SEND_JSON_MAX_TIME)
            }
            Kind::Request
            | Kind::RequestPost
            | Kind::PostJsonStdin
            | Kind::Public
            | Kind::BearerForm
            | Kind::SetupGet
            | Kind::SetupPost => MAX_TIME,
            Kind::Probe => PROBE_MAX_TIME,
            Kind::SidecarGet => SIDECAR_MAX_TIME,
            Kind::PassportProject => PASSPORT_PROJECT_MAX_TIME,
            Kind::Cards => CARDS_TIME,
            Kind::UpdateVersion => UPDATE_MAX_TIME,
        }
    }

    /// `_cli_curl`'s loop: a connect that timed out, again (its kinds).
    fn attempt(
        &self,
        kind: Kind,
        target: &Target,
        request: &Request,
        max: Duration,
    ) -> Result<Answer, Failure> {
        let mut tries = 1;
        loop {
            let r = self.curl(kind, target, request, max);
            if matches!(r, Err(Failure::ConnectTimedOut))
                && kind.retries_connect_timeout()
                && tries < TRIES
            {
                tries += 1;
                std::thread::sleep(self.pause);
                continue;
            }
            return r;
        }
    }

    /// `_k2_curl`'s loop: a refused connect, again, following a moved
    /// port; a body from stdin never.
    fn curl(
        &self,
        kind: Kind,
        target: &Target,
        request: &Request,
        max: Duration,
    ) -> Result<Answer, Failure> {
        let mut target = target.clone();
        let mut request = request.clone();
        let mut tries = 1;
        self.refused.set(0);
        loop {
            let deadline = Instant::now() + max;
            match wire::connect(&target, kind.connect_timeout(), deadline) {
                Ok(mut conn) => {
                    let r = wire::exchange(&mut conn, &request, deadline);
                    // Every answer, a retried one too: the compare is on
                    // what came back, never a request of its own.
                    if let Ok(a) = &r {
                        skew::note(&self.version, a.server_version.as_deref());
                    }
                    return r;
                }
                Err(Failure::Connect(e)) => {
                    if kind.once() || tries >= TRIES {
                        return Err(Failure::Connect(e));
                    }
                    tries += 1;
                    self.refused.set(self.refused.get() + 1);
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

    /// The TCP token. Only [`Kind::Public`] goes out with none.
    fn tcp_token(&self) -> Option<&str> {
        match &self.auth {
            Auth::None => None,
            Auth::Owner(t) => Some(t.as_str()),
            Auth::Passport { passport, .. } => Some(passport.as_str()),
        }
    }

    /// The request, byte for byte the shell's for this kind and channel.
    fn build(
        &self,
        kind: Kind,
        channel: Channel,
        endpoint: &str,
        args: &[&str],
        body: &[u8],
    ) -> Result<(Target, Request), Error> {
        let mut headers: Vec<(String, String)> = Vec::new();
        // `cli_post_json_stdin` reads its config with `-K /dev/fd/3`, after
        // the `-H`s `_k2_curl` puts first: its Bearer comes after the
        // hand-over headers. Every other call's comes before them.
        let mut late_credential = None;
        let (target, host) = match channel {
            Channel::Tcp => {
                // The credential is the first header, an `Authorization:
                // Bearer` line of the shell's curl config (`_k2_cfg_auth`),
                // read before the command line's `-H`s. Never the query.
                let credential = match kind {
                    Kind::Public | Kind::UpdateVersion => None,
                    Kind::Probe | Kind::BearerForm => Some(self.passport().map(Passport::as_str)),
                    _ => Some(self.tcp_token()),
                };
                if let Some(credential) = credential {
                    let Some(token) = credential else {
                        return Err(Error::NoCredential {
                            endpoint: endpoint.to_string(),
                        });
                    };
                    let line = ("Authorization".to_string(), format!("Bearer {token}"));
                    if kind == Kind::PostJsonStdin {
                        late_credential = Some(line);
                    } else {
                        headers.push(line);
                    }
                }
                (
                    Target::Tcp {
                        host: self.host.clone(),
                        port: self.port,
                    },
                    format!("{}:{}", self.host, self.port),
                )
            }
            Channel::Uds => {
                let no = || Error::NoCellSocket {
                    endpoint: endpoint.to_string(),
                };
                let Auth::Passport { passport, sock } = &self.auth else {
                    return Err(no());
                };
                let sock = sock.as_ref().ok_or_else(no)?;
                headers.push((
                    "Authorization".into(),
                    format!("Bearer {}", passport.as_str()),
                ));
                (uds_target(sock, endpoint)?, "localhost".to_string())
            }
        };
        if self.would_refuse {
            headers.push((WOULD_REFUSE_HEADER.into(), "1".into()));
        }
        headers.push((VERSION_HEADER.into(), self.version.clone()));
        headers.push((VIA_HEADER.into(), VIA_NATIVE.into()));
        if self.owner_from_disk && matches!(self.auth, Auth::Owner(_)) && kind != Kind::Public {
            headers.push((OWNER_SOURCE_HEADER.into(), OWNER_SOURCE_DISK.into()));
        }
        headers.extend(late_credential);

        let tcp = channel == Channel::Tcp;
        let with_project = || format!("project={}", quote(&self.project));
        let joined = args.join("&");
        let json = |headers: &mut Vec<(String, String)>| {
            headers.push(("Content-Type".into(), "application/json".into()));
            Some(Body {
                bytes: body.to_vec(),
                curl_form_type: false,
            })
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
                (
                    Method::Post,
                    path,
                    Some(Body {
                        bytes: Vec::new(),
                        curl_form_type: true,
                    }),
                )
            }
            Kind::PostForm => {
                let path = if tcp {
                    format!("{endpoint}?{}", with_project())
                } else {
                    endpoint.to_string()
                };
                headers.push(("Content-Type".into(), wire::FORM_TYPE.into()));
                (
                    Method::Post,
                    path,
                    Some(Body {
                        bytes: joined.into_bytes(),
                        curl_form_type: false,
                    }),
                )
            }
            Kind::PostJson | Kind::PostJsonStdin => {
                (Method::Post, endpoint.to_string(), json(&mut headers))
            }
            Kind::SendJsonGet | Kind::SidecarGet | Kind::PassportProject | Kind::Cards => {
                let payload = String::from_utf8_lossy(body);
                let path = if payload.is_empty() {
                    endpoint.to_string()
                } else {
                    wire::append_get_query(endpoint, &payload)
                };
                (Method::Get, path, None)
            }
            Kind::SendJsonPost => (Method::Post, endpoint.to_string(), json(&mut headers)),
            Kind::Public | Kind::Probe | Kind::UpdateVersion => {
                (Method::Get, endpoint.to_string(), None)
            }
            Kind::SetupGet | Kind::SetupPost => {
                // `${path}${q}`: `?format=text` or nothing.
                let path = if joined.is_empty() {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}?{joined}")
                };
                if kind == Kind::SetupPost {
                    (Method::Post, path, json(&mut headers))
                } else {
                    (Method::Get, path, None)
                }
            }
            Kind::BearerForm => {
                headers.push(("Content-Type".into(), wire::FORM_TYPE.into()));
                (
                    Method::Post,
                    endpoint.to_string(),
                    Some(Body {
                        bytes: joined.into_bytes(),
                        curl_form_type: false,
                    }),
                )
            }
        };
        wire::check_target(&path).map_err(|byte| Error::BadTarget {
            target: path.clone(),
            byte,
        })?;
        let request = Request {
            method,
            target: path,
            host,
            user_agent: format!("k5/{}", self.version),
            headers,
            body,
        };
        Ok((target, request))
    }
}

#[cfg(unix)]
fn uds_target(sock: &Path, _endpoint: &str) -> Result<Target, Error> {
    Ok(Target::Unix(sock.to_path_buf()))
}

#[cfg(not(unix))]
fn uds_target(_sock: &Path, endpoint: &str) -> Result<Target, Error> {
    Err(Error::NoCellSocket {
        endpoint: endpoint.to_string(),
    })
}

/// `[ -S path ]`: a socket, through any links.
#[cfg(unix)]
pub fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// `[ -S path ]`: never, where there are no Unix sockets.
#[cfg(not(unix))]
pub fn is_socket(_path: &Path) -> bool {
    false
}

/// The error for a failed exchange.
fn failure(kind: Kind, endpoint: &str, f: Failure, t0: Instant) -> Error {
    let endpoint = endpoint.to_string();
    match f {
        Failure::TimedOut | Failure::ConnectTimedOut if kind.says_no_answer() => {
            Error::NoAnswer(no_answer(&endpoint, t0, false))
        }
        Failure::Empty if kind.says_no_answer() => Error::NoAnswer(no_answer(&endpoint, t0, true)),
        Failure::TimedOut | Failure::ConnectTimedOut => Error::TimedOut { endpoint },
        Failure::Empty => Error::Empty { endpoint },
        Failure::Connect(err) => Error::Unreachable { endpoint, err },
        Failure::NotSent(why) => Error::Broken {
            endpoint,
            why,
            curl: wire::curl::SEND_ERROR,
        },
        Failure::Broken { curl, why } => Error::Broken {
            endpoint,
            why,
            curl,
        },
    }
}

/// `_cli_transport_failure`: the code `_cli_curl` gives a failed call
/// that is this machine's doing, as the shell reads curl's `-v` trace. A
/// refused connect never is. The OS refusing the connect (EPERM, EACCES),
/// an address family it doesn't have, or no route to `::1` is
/// [`LOCAL_TRANSPORT_DENIED`]. Any other failure is [`TMPDIR_UNWRITABLE`]
/// when `TMPDIR` (else `/tmp`) is not a folder this user may write in
/// ([`tmpdir_unusable`]).
pub fn local_failure(f: &Failure, host: &str) -> Option<i32> {
    if let Failure::Connect(e) = f {
        if e.kind() == io::ErrorKind::ConnectionRefused {
            return None;
        }
        if os_denied(e, host) {
            return Some(LOCAL_TRANSPORT_DENIED);
        }
    }
    tmpdir_unusable().then_some(TMPDIR_UNWRITABLE)
}

#[cfg(unix)]
fn os_denied(e: &io::Error, host: &str) -> bool {
    match e.raw_os_error() {
        Some(libc::EPERM | libc::EACCES | libc::EAFNOSUPPORT | libc::EPFNOSUPPORT) => true,
        Some(libc::ENETUNREACH | libc::EHOSTUNREACH) => host.contains("::1"),
        _ => false,
    }
}

#[cfg(not(unix))]
fn os_denied(e: &io::Error, _host: &str) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

/// `[ ! -d "$td" ] || [ ! -w "$td" ]` with `td=${TMPDIR:-/tmp}`.
pub fn tmpdir_unusable() -> bool {
    let td = std::env::var_os("TMPDIR")
        .filter(|t| !t.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    !td.is_dir() || !writable(&td)
}

#[cfg(unix)]
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a NUL-terminated path; access() only reads it.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn writable(dir: &Path) -> bool {
    std::fs::metadata(dir).is_ok_and(|m| !m.permissions().readonly())
}

fn maybe_applied(endpoint: &str, t0: Instant) -> Error {
    Error::MaybeApplied {
        endpoint: endpoint.to_string(),
        waited: t0.elapsed().as_secs(),
    }
}

fn no_answer(endpoint: &str, t0: Instant, empty: bool) -> NoAnswer {
    NoAnswer {
        endpoint: endpoint.to_string(),
        waited: t0.elapsed().as_secs(),
        empty,
    }
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
        assert_eq!(
            Passport::new("s.x").map(|p| p.as_str().to_string()),
            Some("s.x".to_string())
        );
        // Never printed.
        assert_eq!(
            format!("{:?}", Passport::new("s.secret").unwrap()),
            "Passport(..)"
        );
        assert_eq!(
            format!("{:?}", Auth::Owner("owner-secret".into())),
            "Owner(..)"
        );
        assert_eq!(format!("{:?}", Auth::None), "None");
    }

    #[test]
    fn the_no_answer_lines_are_the_shell_s() {
        let n = NoAnswer {
            endpoint: "/cli/inbox/list".into(),
            waited: 30,
            empty: false,
        };
        assert_eq!(
            n.to_string(),
            "Error: K2's server didn't answer /cli/inbox/list within 30 s; run k2 daemon status"
        );
        let n = NoAnswer {
            endpoint: "/cli/inbox/list".into(),
            waited: 0,
            empty: true,
        };
        assert_eq!(n.to_string(), "Error: K2's server gave an empty answer to /cli/inbox/list after 0 s; run k2 daemon status");
    }

    #[test]
    fn each_failure_has_the_shell_s_code() {
        let ep = || "/cli/x".to_string();
        let rows: [(Error, i32); 11] = [
            (
                Error::NoAnswer(NoAnswer {
                    endpoint: ep(),
                    waited: 30,
                    empty: false,
                }),
                28,
            ),
            (
                Error::NoAnswer(NoAnswer {
                    endpoint: ep(),
                    waited: 0,
                    empty: true,
                }),
                52,
            ),
            (
                Error::Unreachable {
                    endpoint: ep(),
                    err: io::Error::from(io::ErrorKind::ConnectionRefused),
                },
                7,
            ),
            (Error::TimedOut { endpoint: ep() }, 28),
            (Error::Empty { endpoint: ep() }, 52),
            (
                Error::Broken {
                    endpoint: ep(),
                    why: "read".into(),
                    curl: 56,
                },
                56,
            ),
            (
                Error::BadTarget {
                    target: "/a b".into(),
                    byte: b' ',
                },
                3,
            ),
            (
                Error::Denied {
                    endpoint: ep(),
                    code: 75,
                },
                75,
            ),
            (
                Error::Denied {
                    endpoint: ep(),
                    code: 76,
                },
                76,
            ),
            (
                Error::MaybeApplied {
                    endpoint: ep(),
                    waited: 1,
                },
                1,
            ),
            (Error::NoCredential { endpoint: ep() }, 1),
        ];
        for (e, want) in rows {
            assert_eq!(e.curl_code(), want, "{e:?}");
        }
        assert!(matches!(
            failure(
                Kind::PostForm,
                "/x",
                Failure::NotSent("send".into()),
                Instant::now()
            ),
            Error::Broken { curl: 55, .. }
        ));
    }

    /// `_cli_transport_failure`'s reading of a failed connect: a refused
    /// one never is the machine's; the OS refusing it is, 75.
    #[cfg(unix)]
    #[test]
    fn the_os_refusing_a_connect_is_local() {
        let e = |n: i32| io::Error::from_raw_os_error(n);
        assert_eq!(
            local_failure(
                &Failure::Connect(io::Error::from(io::ErrorKind::ConnectionRefused)),
                "127.0.0.1"
            ),
            None
        );
        for n in [libc::EPERM, libc::EACCES, libc::EAFNOSUPPORT] {
            assert_eq!(
                local_failure(&Failure::Connect(e(n)), "127.0.0.1"),
                Some(LOCAL_TRANSPORT_DENIED),
                "{n}"
            );
        }
        assert!(os_denied(&e(libc::ENETUNREACH), "::1"));
        assert!(!os_denied(&e(libc::ENETUNREACH), "127.0.0.1"));
        assert!(!os_denied(&e(libc::ENOENT), "127.0.0.1"));
        assert!(Kind::Request.checks_local_denial() && Kind::RequestPost.checks_local_denial());
        for k in [
            Kind::PostForm,
            Kind::PostJson,
            Kind::PostJsonStdin,
            Kind::SendJsonGet,
            Kind::SendJsonPost,
            Kind::Public,
            Kind::Probe,
            Kind::SidecarGet,
            Kind::PassportProject,
            Kind::Cards,
        ] {
            assert!(!k.checks_local_denial(), "{k:?}");
        }
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
            (
                PostJsonStdin,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
            ),
            (SendJsonGet, false, false, false, false, true, true, false),
            (SendJsonPost, false, false, false, false, true, true, false),
            (Public, false, true, false, true, false, false, false),
            (Probe, false, false, false, false, false, false, false),
            (BearerForm, false, false, false, false, false, false, false),
            (SidecarGet, false, false, false, false, false, false, false),
            (
                PassportProject,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            ),
            (Cards, false, false, false, false, false, false, false),
            (
                UpdateVersion,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
            ),
            (SetupGet, false, false, false, false, false, false, false),
            (SetupPost, false, false, false, false, false, false, false),
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
        assert_eq!(
            Kind::UpdateVersion.connect_timeout(),
            Duration::from_secs(2)
        );
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
            (Kind::Public, 30),
            (Kind::Probe, 5),
            (Kind::BearerForm, 30),
            (Kind::SidecarGet, 120),
            (Kind::PassportProject, 15),
            (Kind::UpdateVersion, 4),
            (Kind::SetupGet, 30),
            (Kind::SetupPost, 30),
        ] {
            assert_eq!(c.max_time(k), Duration::from_secs(want), "{k:?}");
        }
    }

    #[test]
    fn the_owner_has_no_cell_socket_arm() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(
            c.call(Channel::Uds, "/cli/inbox/list", Call::Request(&[])),
            Err(Error::NoCellSocket { .. })
        ));
        let mut c = Client::new(
            "127.0.0.1",
            1,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: None,
            },
        );
        assert!(matches!(
            c.call(Channel::Uds, "/cli/inbox/list", Call::Request(&[])),
            Err(Error::NoCellSocket { .. })
        ));
    }

    /// With no credential only the public GET is built; every other call
    /// is refused before anything is sent (port 1: nothing listens).
    #[test]
    fn no_credential_sends_only_the_public_get() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::None);
        for call in [
            Call::Request(&[]),
            Call::RequestPost(&[]),
            Call::PostForm(&["a=b"]),
            Call::PostJson(b"{}"),
        ] {
            assert!(
                matches!(
                    c.call(Channel::Tcp, "/cli/x", call),
                    Err(Error::NoCredential { .. })
                ),
                "{call:?}"
            );
        }
        assert!(matches!(
            c.post_json_stdin("/cli/x", b"{}"),
            Err(Error::NoCredential { .. })
        ));
        assert!(matches!(
            c.send_json(false, "/cli/x", ""),
            Err(Error::NoCredential { .. })
        ));
        let (_, r) = c
            .build(Kind::Public, Channel::Tcp, "/boot-status", &[], &[])
            .unwrap();
        assert_eq!(r.target, "/boot-status");
        assert_eq!(r.method, Method::Get);
        assert!(r.body.is_none());
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [VERSION_HEADER, VIA_HEADER]);
    }

    /// `k2 update --cli`'s GET: the path as it is, no token, the
    /// binary's headers and the owner's source as `_k5_headers` adds it
    /// (the owner token from disk, no passport), never with a passport.
    #[test]
    fn the_update_version_get() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o t+".into()));
        c.owner_from_disk = true;
        let (target, r) = c
            .build(Kind::UpdateVersion, Channel::Tcp, "/boot-status", &[], &[])
            .unwrap();
        assert_eq!(
            target,
            Target::Tcp {
                host: "127.0.0.1".into(),
                port: 9
            }
        );
        assert_eq!((r.method, r.target.as_str()), (Method::Get, "/boot-status"));
        assert!(r.body.is_none());
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [VERSION_HEADER, VIA_HEADER, OWNER_SOURCE_HEADER]);
        let p = Client::new(
            "127.0.0.1",
            9,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.t").unwrap(),
                sock: None,
            },
        );
        let (_, r) = p
            .build(Kind::UpdateVersion, Channel::Tcp, "/boot-status", &[], &[])
            .unwrap();
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [VERSION_HEADER, VIA_HEADER]);
        assert!(!r.target.contains("token"));
    }

    /// `_setup_send`'s two: the token as the Bearer header, then
    /// `?format=text` when asked; the POST's body as JSON; the binary's
    /// headers and the owner's source; over TCP to the host and port.
    #[test]
    fn the_setup_requests() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("t.k/+= z".into()));
        c.owner_from_disk = true;
        let (target, r) = c
            .build(
                Kind::SetupGet,
                Channel::Tcp,
                "/cli/first-run/status",
                &["format=text"],
                &[],
            )
            .unwrap();
        assert!(matches!(target, Target::Tcp { port: 9, .. }));
        assert_eq!(
            (r.method, r.target.as_str(), r.host.as_str()),
            (
                Method::Get,
                "/cli/first-run/status?format=text",
                "127.0.0.1:9"
            )
        );
        assert!(r.body.is_none());
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "Authorization",
                VERSION_HEADER,
                VIA_HEADER,
                OWNER_SOURCE_HEADER
            ]
        );
        assert_eq!(r.headers[0].1, "Bearer t.k/+= z");
        let (_, r) = c
            .build(
                Kind::SetupPost,
                Channel::Tcp,
                "/cli/first-run/skip",
                &[],
                br#"{"row": "x"}"#,
            )
            .unwrap();
        assert_eq!(
            (r.method, r.target.as_str()),
            (Method::Post, "/cli/first-run/skip")
        );
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "Authorization",
                VERSION_HEADER,
                VIA_HEADER,
                OWNER_SOURCE_HEADER,
                "Content-Type"
            ]
        );
        assert_eq!(r.body.as_ref().unwrap().bytes, br#"{"row": "x"}"#);
        let p = Client::new(
            "127.0.0.1",
            9,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.t").unwrap(),
                sock: None,
            },
        );
        let (_, r) = p
            .build(
                Kind::SetupGet,
                Channel::Tcp,
                "/cli/first-run/status",
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(r.target, "/cli/first-run/status");
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Authorization", VERSION_HEADER, VIA_HEADER]);
        assert_eq!(r.headers[0].1, "Bearer s.t");
    }

    /// Over TCP every call but the public GETs carries its credential as
    /// the first header, `Authorization: Bearer <T>` (the shell's
    /// `_k2_cfg_auth`, a curl config line read before the `-H`s), and
    /// never in the query: the owner token, or the passport when there is
    /// one. `cli_request`'s query starts with `project=`.
    #[test]
    fn the_tcp_credential_is_a_bearer_header() {
        let owner = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o t+".into()));
        let passport = Client::new(
            "127.0.0.1",
            9,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.t").unwrap(),
                sock: None,
            },
        );
        use Kind::*;
        for (c, want) in [(&owner, "Bearer o t+"), (&passport, "Bearer s.t")] {
            for k in [
                Request,
                RequestPost,
                PostForm,
                PostJson,
                SendJsonGet,
                SendJsonPost,
                SidecarGet,
                PassportProject,
                Cards,
                SetupGet,
                SetupPost,
            ] {
                let (_, r) = c.build(k, Channel::Tcp, "/cli/x", &[], &[]).unwrap();
                assert_eq!(
                    r.headers.first().map(|(n, v)| (n.as_str(), v.as_str())),
                    Some(("Authorization", want)),
                    "{k:?}"
                );
                assert!(!r.target.contains("token"), "{k:?}: {}", r.target);
            }
        }
        // cli_post_json_stdin's comes after the hand-over headers.
        let (_, r) = owner
            .build(PostJsonStdin, Channel::Tcp, "/cli/x", &[], &[])
            .unwrap();
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [VERSION_HEADER, VIA_HEADER, "Authorization", "Content-Type"]
        );
        assert_eq!(r.target, "/cli/x");
        for k in [Probe, BearerForm] {
            let (_, r) = passport.build(k, Channel::Tcp, "/cli/x", &[], &[]).unwrap();
            assert_eq!(r.headers[0], ("Authorization".into(), "Bearer s.t".into()));
            assert!(matches!(
                owner.build(k, Channel::Tcp, "/cli/x", &[], &[]),
                Err(Error::NoCredential { .. })
            ));
        }
        for k in [Public, UpdateVersion] {
            let (_, r) = owner.build(k, Channel::Tcp, "/cli/x", &[], &[]).unwrap();
            assert!(r.headers.iter().all(|(n, _)| n != "Authorization"), "{k:?}");
        }
        let (_, r) = owner
            .build(Request, Channel::Tcp, "/cli/x", &["a=1"], &[])
            .unwrap();
        assert_eq!(r.target, "/cli/x?project=%2Fp&a=1");
        let (_, r) = owner
            .build(SendJsonGet, Channel::Tcp, "/cli/x", &[], b"a=1")
            .unwrap();
        assert_eq!(r.target, "/cli/x?a=1");
        let (_, r) = owner
            .build(PostForm, Channel::Tcp, "/cli/x", &["a=1"], &[])
            .unwrap();
        assert_eq!(r.target, "/cli/x?project=%2Fp");
        let none = Client::new("127.0.0.1", 9, "/p", Auth::None);
        assert!(matches!(
            none.build(Request, Channel::Tcp, "/cli/x", &[], &[]),
            Err(Error::NoCredential { .. })
        ));
    }

    /// `X-K5-Owner-Source: disk` after the Via, only on an owner's call
    /// whose token came from disk, never on the public GET.
    #[test]
    fn the_owner_source_header() {
        let names = |c: &Client, kind: Kind| -> Vec<(String, String)> {
            c.build(kind, Channel::Tcp, "/cli/x", &[], &[])
                .unwrap()
                .1
                .headers
        };
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(names(&c, Kind::Request)
            .iter()
            .all(|(n, _)| n != OWNER_SOURCE_HEADER));
        c.owner_from_disk = true;
        c.would_refuse = true;
        let got = names(&c, Kind::Request);
        let order: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            order,
            [
                "Authorization",
                WOULD_REFUSE_HEADER,
                VERSION_HEADER,
                VIA_HEADER,
                OWNER_SOURCE_HEADER
            ]
        );
        assert_eq!(got[4].1, "disk");
        assert!(names(&c, Kind::Public)
            .iter()
            .all(|(n, _)| n != OWNER_SOURCE_HEADER));
        let mut p = Client::new(
            "127.0.0.1",
            1,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: None,
            },
        );
        p.owner_from_disk = true;
        assert!(
            names(&p, Kind::Request)
                .iter()
                .all(|(n, _)| n != OWNER_SOURCE_HEADER),
            "never with a passport"
        );
    }

    /// The probe: the endpoint as it is, the passport as Bearer before
    /// the custom headers, no token in the query and no Owner-Source.
    #[test]
    fn the_probe_request() {
        let mut p = Client::new(
            "127.0.0.1",
            4000,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: None,
            },
        );
        p.would_refuse = true;
        p.owner_from_disk = true;
        let (target, r) = p
            .build(Kind::Probe, Channel::Tcp, PROBE_ENDPOINT, &[], &[])
            .unwrap();
        assert!(matches!(target, Target::Tcp { ref host, port: 4000 } if host == "127.0.0.1"));
        assert_eq!(
            (r.method, r.target.as_str(), r.host.as_str()),
            (Method::Get, "/cli/whoami", "127.0.0.1:4000")
        );
        assert!(r.body.is_none());
        let names: Vec<(&str, &str)> = r
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(names[0], ("Authorization", "Bearer s.x"));
        let order: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            order,
            [
                "Authorization",
                WOULD_REFUSE_HEADER,
                VERSION_HEADER,
                VIA_HEADER
            ]
        );
        assert_eq!(Kind::Probe.connect_timeout(), Duration::from_secs(3));
        assert_eq!(Kind::Request.connect_timeout(), Duration::from_secs(5));
        // No passport, no probe: nothing is sent (port 1: nothing listens).
        let mut o = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(
            o.build(Kind::Probe, Channel::Tcp, PROBE_ENDPOINT, &[], &[]),
            Err(Error::NoCredential { .. })
        ));
        assert!(!o.passport_rejected());
    }

    /// `k2 respond`'s TCP arm: the passport as Bearer first, the form
    /// body, no query of ours, never the owner-source line.
    #[test]
    fn the_bearer_form_request() {
        let mut p = Client::new(
            "127.0.0.1",
            4000,
            "/p",
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: None,
            },
        );
        p.would_refuse = true;
        p.owner_from_disk = true;
        let (target, r) = p
            .build(
                Kind::BearerForm,
                Channel::Tcp,
                "/cli/respond",
                &["message=hi%20there", "final=1"],
                &[],
            )
            .unwrap();
        assert!(matches!(target, Target::Tcp { ref host, port: 4000 } if host == "127.0.0.1"));
        assert_eq!(
            (r.method, r.target.as_str(), r.host.as_str()),
            (Method::Post, "/cli/respond", "127.0.0.1:4000")
        );
        let body = r.body.as_ref().expect("a body");
        assert_eq!(
            (body.bytes.as_slice(), body.curl_form_type),
            (&b"message=hi%20there&final=1"[..], false)
        );
        let names: Vec<(&str, &str)> = r
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("Authorization", "Bearer s.x"),
                (WOULD_REFUSE_HEADER, "1"),
                (VERSION_HEADER, p.version.as_str()),
                (VIA_HEADER, VIA_NATIVE),
                ("Content-Type", wire::FORM_TYPE)
            ]
        );
        let mut o = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(
            o.post_form_bearer("/cli/respond", &["message=x"]),
            Err(Error::NoCredential { .. })
        ));
    }

    #[test]
    fn a_target_curl_refuses_is_not_sent() {
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner("o".into()));
        assert!(matches!(
            c.call(Channel::Tcp, "/cli/x", Call::Request(&["a=b c"])),
            Err(Error::BadTarget { byte: b' ', .. })
        ));
        assert!(matches!(
            c.call(Channel::Tcp, "/cli/x#y", Call::PostJson(b"{}")),
            Err(Error::BadTarget { byte: b'#', .. })
        ));
    }

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k5-client-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
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
            assert_eq!(
                healer.healed(&cur).map(|p| p.as_str().to_string()),
                want.map(str::to_string),
                "{content:?}"
            );
        }
        // An owner-shaped token (no dot) never heals.
        std::fs::write(&file, "sessA.newsecret\n").unwrap();
        assert_eq!(healer.healed(&Passport::new("ownertoken").unwrap()), None);
        // A dash in the session part is fine.
        std::fs::write(&file, "s-1.b2\n").unwrap();
        assert_eq!(
            healer
                .healed(&Passport::new("s-1.a1").unwrap())
                .map(|p| p.as_str().to_string()),
            Some("s-1.b2".to_string())
        );
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
        assert_eq!(
            SecretsDir(PathBuf::new()).healed(&cur),
            None,
            "no folder set"
        );
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
