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

mod socket;
mod tunnel;

use socket::uds_target;
pub use socket::{is_socket, uds_eligible};

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
/// `_compute_send GET`'s `--max-time 60` (`cli/src/compute-lib.sh`).
pub const COMPUTE_GET_MAX_TIME: Duration = Duration::from_secs(60);
/// `_compute_send POST`'s `--max-time "${K2_CP_MAX_TIME:-120}"`.
pub const COMPUTE_POST_MAX_TIME: Duration = Duration::from_secs(120);
/// `_vault_sock_call`'s `--max-time 180` (`cli/src/vault.sh`).
pub const VAULT_SOCK_MAX_TIME: Duration = Duration::from_secs(180);
/// `_migrate_send`'s `--max-time 600` on the vault socket
/// (`cli/src/migrate.sh`).
pub const MIGRATE_SOCK_MAX_TIME: Duration = Duration::from_secs(600);
/// The per-verb header `_vault_sock_call` sends: `1` when stdin and
/// stdout, or stdin and stderr, are terminals, else `0`.
pub const TTY_HEADER: &str = "X-K2-Tty";
/// `_cli_use_passport_project`'s `--max-time 15` (`cli/src/cell-socket.sh`).
pub const PASSPORT_PROJECT_MAX_TIME: Duration = Duration::from_secs(15);
/// Where `_permission_probe` asks whether permission cards are on
/// (`cli/src/permission.sh`).
pub const CARDS_ENDPOINT: &str = "/cli/permission/enabled";
/// `_permission_probe`'s `--connect-timeout 1 --max-time 1`, one try.
pub const CARDS_TIME: Duration = Duration::from_secs(1);
/// Where `_sf_agents_off_probe` reads whether agents may report
/// (`cli/src/send-feedback.sh`).
pub const FEEDBACK_SETTINGS_ENDPOINT: &str = "/cli/product-feedback/settings";
/// `_sf_agents_off_probe`'s `--connect-timeout 1`.
pub const FEEDBACK_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// `_sf_agents_off_probe`'s `--max-time 1`, one try.
pub const FEEDBACK_MAX_TIME: Duration = Duration::from_secs(1);
/// `k2 update --cli`'s `--connect-timeout 2` (`_update_daemon_version`,
/// `cli/src/update.sh`).
pub const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// `k2 update --cli`'s `--max-time 4`.
pub const UPDATE_MAX_TIME: Duration = Duration::from_secs(4);
/// `k2 api status`'s `/boot-status` read: at most 10 s.
pub const BOOT_STATUS_MAX_TIME: Duration = Duration::from_secs(10);
/// Where `_k2_send_once_supported` asks whether the daemon drops a
/// repeated send (`cli/src/cell-socket.sh`).
pub const SEND_ONCE_ENDPOINT: &str = "/boot-status";
/// The word in that answer's `features` that says it does.
pub const SEND_ONCE_FEATURE: &str = r#""send-once""#;
/// `_connect_curl_daemon`'s route: the daemon makes the Connect control
/// plane's call with the tunnel token it holds.
pub const CONNECT_PROXY_ENDPOINT: &str = "/cli/connect/proxy";
/// `_connect_curl_daemon`'s `--max-time 45`.
pub const CONNECT_PROXY_MAX_TIME: Duration = Duration::from_secs(45);
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
/// The whole body of that refusal from the daemon's auth gate (TCP and
/// the cell socket), sent before any route's handler runs.
pub const GATE_REFUSAL: &str = r#"{"error":"Invalid or missing auth token"}"#;

/// An answer from the daemon's auth gate refusing the credential: status
/// 401 or 403 and exactly [`GATE_REFUSAL`]. No handler ran, so nothing
/// was done. A handler's own answer (a 200, a 409 conflict) may hold
/// [`PASSPORT_REFUSAL`] too, in a document's text; that is not one.
pub fn gate_refusal(status: u16, body: &[u8]) -> bool {
    matches!(status, 401 | 403) && body == GATE_REFUSAL.as_bytes()
}

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
/// A Codex call's conversation: the root thread of the conversation
/// (`CODEX_SESSION_ID`; a subagent's call names its parent's).
pub const CONVERSATION_HEADER: &str = "X-K2-Conversation";
/// A Codex call's own thread (`CODEX_THREAD_ID`).
pub const THREAD_HEADER: &str = "X-K2-Thread";
/// The switch that turns the conversation headers off: one of `0`,
/// `off`, `false` or `no`, as written.
pub const CODEX_BIND_SWITCH: &str = "K2_CODEX_PASSPORT_BIND";

/// The shell's `_k2_codex_headers` (`cli/src/cli-tools-policy.sh`): a
/// Codex call's conversation headers, sent on every daemon request.
/// None unless `thread` (`CODEX_THREAD_ID`, which Codex sets on every
/// tool call) is set and not empty and `switch`
/// ([`CODEX_BIND_SWITCH`]) is not one of `0`, `off`, `false` or `no`
/// (`_cli_codex_bind_on`). Then [`CONVERSATION_HEADER`] with `root`
/// (`CODEX_SESSION_ID`) and [`THREAD_HEADER`] with `thread`, in that
/// order, each only when its value is 1 to 128 of ASCII letters, digits,
/// `-` and `_` (`_K2_CODEX_ID_RE`); one that is not is left out on its
/// own. So a value never needs escaping, and never holds a line break.
pub fn codex_headers(
    thread: Option<&std::ffi::OsStr>,
    root: Option<&std::ffi::OsStr>,
    switch: Option<&std::ffi::OsStr>,
) -> Vec<(String, String)> {
    if thread.is_none_or(|t| t.is_empty()) {
        return Vec::new();
    }
    if matches!(
        switch.and_then(|s| s.to_str()),
        Some("0" | "off" | "false" | "no")
    ) {
        return Vec::new();
    }
    let id = |v: Option<&std::ffi::OsStr>| {
        v.and_then(|v| v.to_str())
            .filter(|v| {
                (1..=128).contains(&v.len())
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .map(str::to_string)
    };
    let mut headers = Vec::new();
    if let Some(root) = id(root) {
        headers.push((CONVERSATION_HEADER.to_string(), root));
    }
    if let Some(thread) = id(thread) {
        headers.push((THREAD_HEADER.to_string(), thread));
    }
    headers
}

/// [`codex_headers`] from this process's environment.
pub fn codex_headers_from_env() -> Vec<(String, String)> {
    let var = std::env::var_os;
    codex_headers(
        var("CODEX_THREAD_ID").as_deref(),
        var("CODEX_SESSION_ID").as_deref(),
        var(CODEX_BIND_SWITCH).as_deref(),
    )
}

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
    /// call is [`Error::NoCredential`], never sent, unless the client's
    /// [`Client::no_token`] says to send it with none.
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
    /// `cli_post` / `cli_post_uds` (`cli/src/usage.sh`): a POST with an
    /// empty body and the pairs in the query, after `project=` over TCP
    /// and alone on the cell socket ([`Kind::QueryPost`]). Through
    /// [`Client::send`] it is `cli_send cli_post`: sent once, as any POST.
    QueryPost(&'a [&'a str]),
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
    /// `_compute_send GET` (`cli/src/compute-lib.sh`): `cli_send_json
    /// GET`'s request, at most 60 s, and nothing else: no heal, no line,
    /// no note (`_compute_split` notes the body itself).
    ComputeGet,
    /// `_compute_send POST`: `cli_send_json POST`'s request, at most
    /// `K2_CP_MAX_TIME` or 120 s; likewise nothing else.
    ComputePost,
    /// `_sidecar_send POST`: `cli_send_json POST`'s request, at most 120 s,
    /// and likewise nothing else.
    SidecarPost,
    /// `_vault_sock_call GET <route>` (`cli/src/vault.sh`): `_k2_curl "" -s
    /// -G --unix-socket <K2 home>/vault.sock http://localhost/cli/vault/<route>
    /// -H "X-K2-Tty: <0|1>"`, connect 5 s, at most 180 s. No credential
    /// (the socket is the person's channel), `_k2_curl`'s tries for a
    /// refused connect; no heal, no line, no note (`_vault_fail` notes a
    /// refusal itself).
    VaultSockGet,
    /// `_vault_sock_call POST <route> <json>`: the same with `-X POST`, the
    /// `X-K2-Tty` header, `Content-Type: application/json` and the body
    /// from stdin (`--data-binary @-`), so never tried twice.
    VaultSockPost,
    /// `_cli_use_passport_project`'s `/cli/whoami` (`cli/src/cell-socket.sh`):
    /// `cli_send_json GET`'s request with no query of the caller's, at
    /// most 15 s; no heal, no line, no note.
    PassportProject,
    /// `_permission_probe` (`cli/src/permission.sh`): a GET of
    /// [`CARDS_ENDPOINT`] with `-G` and no data of the caller's, on the
    /// cell socket (the passport as Bearer) or over TCP (the token as
    /// Bearer), never both. Connect 1 s, at
    /// most 1 s; `_K2_CURL_TRIES=1`: one try (a refused connect is not
    /// tried again); no heal, no line, no note.
    Cards,
    /// `_sf_agents_off_probe` (`cli/src/send-feedback.sh`): the request of
    /// [`Kind::Cards`] for [`FEEDBACK_SETTINGS_ENDPOINT`], connect 1 s, at
    /// most 1 s, one try.
    FeedbackOptin,
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
    /// `cli_post` (`cli/src/usage.sh`): `_k2_curl` with a config `url` of
    /// `<endpoint>?project=<it>&<pairs>` and the token as Bearer, `-s -X
    /// POST --data-raw ""`, connect 5 s, at most 30 s, over TCP. Only
    /// `_k2_curl`'s tries (a refused connect, a moved port); no status read,
    /// no 503 tries, no heal, no line. The answer gets `_permission_note`.
    /// On the cell socket it is `cli_post_uds`: the passport as Bearer and
    /// the pairs alone in the query (no `project=`).
    QueryPost,
    /// `_k2_curl` with a config the caller writes itself
    /// (`cli/src/federation.sh`): see [`OwnCurl`]. Over TCP, the path as
    /// it is (no `project=`), the caller's credential or none; for a POST
    /// a JSON body. Only `_k2_curl`'s tries (a refused connect, a moved
    /// port); no 503 tries, no heal, no line, and `_permission_note` only
    /// when `note`.
    Own {
        post: bool,
        note: bool,
    },
    /// `k2 api status`'s `_k2_curl` of `${BASE_URL}/boot-status`
    /// (`cli/src/talk-and-workspace.sh`): a GET of the path as it is, over
    /// TCP, `$TOKEN` as Bearer, `-s --connect-timeout 5 --max-time 10`.
    /// Only `_k2_curl`'s tries (a refused connect, a moved port); no heal,
    /// no line, no note.
    BootStatus,
    /// `_k2_send_once_supported` (`cli/src/cell-socket.sh`):
    /// `_K2_CURL_TRIES=1 _k2_curl "$_K2_CFG" -s -G --connect-timeout 3
    /// --max-time 5 "${BASE_URL}/boot-status"`, the token as Bearer when
    /// there is one. Over TCP, one try, no query; no heal, no line, no
    /// note.
    SendOnce,
    /// `_connect_curl_daemon` (`cli/src/connect.sh`): `_k2_curl` POSTs of
    /// a JSON body to [`CONNECT_PROXY_ENDPOINT`] as it is (no `project=`),
    /// `-sS -X POST -H "Content-Type: application/json"`, connect 5 s, at
    /// most 45 s; the passport as Bearer on the cell socket, `$TOKEN` over
    /// TCP. Only `_k2_curl`'s tries (a refused connect, a moved port); no
    /// status read, no heal, no line, no note.
    ConnectProxy,
    /// `_connect_curl`'s own call to the Connect control plane, when no
    /// daemon made it: `_K2_CURL_NOT_DAEMON=1 _k2_curl` with the tunnel
    /// token as the config's `Authorization: Bearer` line, `-sS -X
    /// <method> <base><path>`, a JSON body with its `Content-Type` when
    /// there is one, connect 5 s, at most 30 s. No hand-over headers (it
    /// is not the daemon), no `project=`; only `_k2_curl`'s tries; no
    /// heal, no line, no note. The token is only ever that header.
    ControlPlane {
        method: Method,
    },
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
                | Kind::ComputeGet
                | Kind::ComputePost
                | Kind::SidecarPost
                | Kind::VaultSockGet
                | Kind::VaultSockPost
                | Kind::PassportProject
                | Kind::Cards
                | Kind::FeedbackOptin
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
                | Kind::QueryPost
                | Kind::Own { .. }
                | Kind::BootStatus
                | Kind::SendOnce
                | Kind::ConnectProxy
                | Kind::ControlPlane { .. }
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
                | Kind::ComputeGet
                | Kind::ComputePost
                | Kind::SidecarPost
                | Kind::VaultSockGet
                | Kind::VaultSockPost
                | Kind::PassportProject
                | Kind::Cards
                | Kind::FeedbackOptin
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
                | Kind::QueryPost
                | Kind::Own { .. }
                | Kind::BootStatus
                | Kind::SendOnce
                | Kind::ConnectProxy
                | Kind::ControlPlane { .. }
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
                | Kind::ComputeGet
                | Kind::ComputePost
                | Kind::SidecarPost
                | Kind::VaultSockGet
                | Kind::VaultSockPost
                | Kind::PassportProject
                | Kind::Cards
                | Kind::FeedbackOptin
                | Kind::UpdateVersion
                | Kind::SetupGet
                | Kind::SetupPost
                | Kind::Own { note: false, .. }
                | Kind::BootStatus
                | Kind::SendOnce
                | Kind::ConnectProxy
                | Kind::ControlPlane { .. }
        )
    }
    /// `--connect-timeout`.
    fn connect_timeout(self) -> Duration {
        match self {
            Kind::Probe => PROBE_CONNECT_TIMEOUT,
            Kind::Cards => CARDS_TIME,
            Kind::FeedbackOptin => FEEDBACK_CONNECT_TIMEOUT,
            Kind::UpdateVersion => UPDATE_CONNECT_TIMEOUT,
            Kind::SendOnce => PROBE_CONNECT_TIMEOUT,
            _ => CONNECT_TIMEOUT,
        }
    }
    /// Nothing is ever tried twice: the body is read from stdin (the
    /// shell's `@-` rule is an argument that is exactly `@-`, never one in
    /// a URL), or `_K2_CURL_TRIES=1`.
    fn once(self) -> bool {
        matches!(
            self,
            Kind::PostJsonStdin
                | Kind::UpdateVersion
                | Kind::VaultSockPost
                | Kind::SendOnce
                | Kind::Cards
                | Kind::FeedbackOptin
        )
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
    /// ([`Auth::None`]) and no [`Client::no_token`].
    NoCredential { endpoint: String },
    /// A socket-first call (`cli_send`, `cli_send_json`, `_sidecar_send`)
    /// in a session with no passport, whose TCP credential would be the
    /// owner token from disk: nothing is sent ([`NO_OWNER_IN_SESSION`],
    /// exit 1; `_k2_session_tcp_token`, `cli/src/cli-tools-policy.sh`).
    NoOwnerInSession { endpoint: String },
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

/// `_K2_NO_OWNER_WRITE_LINE` (`cli/src/cli-tools-policy.sh`): a
/// socket-first call in a session with no passport sends nothing, not
/// even with the owner token from disk, and ends the command with 1.
pub const NO_OWNER_IN_SESSION: &str = concat!(
    "Error: in a session, ",
    k2_client::product_name!(),
    " doesn't act as the owner; this session has no passport, so nothing was sent."
);

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
            Error::NoOwnerInSession { .. } => f.write_str(NO_OWNER_IN_SESSION),
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
            | Error::NoOwnerInSession { .. }
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
    /// A Codex call's conversation headers ([`codex_headers`]), on every
    /// request to the daemon, on either channel; never on a call to
    /// something else (the Connect control plane). [`Client::new`] reads
    /// them from the environment.
    pub conversation: Vec<(String, String)>,
    /// `HOOK_SOCK` is set: a session. With no passport, a socket-first
    /// call refuses to send the owner token from disk
    /// ([`Error::NoOwnerInSession`]).
    pub session: bool,
    /// `$TOKEN` is empty on purpose: with [`Auth::None`], a TCP call goes
    /// out with no `Authorization` header (`_k2_cfg_auth` adds none), as
    /// the shell's open verbs that send a session's passport do in a
    /// session that has none (`_cli_verb_sends_session_passport`).
    pub no_token: bool,
    /// For `User-Agent: k5/<version>` and `X-K5-Cli-Version`, and the
    /// version a server's is compared with ([`skew`]).
    pub version: String,
    /// `_cli_skip_uds`: a remote host was applied; never the cell socket.
    pub force_tcp: bool,
    /// `BASE_URL` is `https://…`: TLS over the TCP connection
    /// ([`super::tls`]).
    pub tls: bool,
    /// The person's proxy for a remote `https://` base
    /// ([`super::proxy`]). Only [`super::remote::apply`] sets it; it is
    /// used only with [`Client::tls`], and a new client has none.
    pub(in crate::transport) via: Option<super::proxy::Proxy>,
    /// `K2_HOST=https://<name>` with a name or a proxy setting the binary
    /// doesn't read as curl does ([`super::remote::daemon_https`]): the
    /// URL the shell would send to. Nothing goes over TCP then.
    pub(in crate::transport) unsendable: Option<String>,
    /// The `Host` header curl writes for the `BASE_URL`, when it is not
    /// `<HOST>:<PORT>` (a remote URL with the scheme's own port leaves the
    /// port out).
    pub host_header: Option<String>,
    /// `_K2_POST_MAX_TIME`: `cli_post_json` and `cli_post_form` only.
    pub post_max_time: Option<Duration>,
    /// `_K2_SEND_MAX_TIME`: `cli_send_json` only.
    pub send_max_time: Option<Duration>,
    /// `K2_CP_MAX_TIME`: `_compute_send POST` only.
    pub compute_post_max_time: Option<Duration>,
    /// `_vault_sock_call`'s fourth word, its `--max-time` (the phone
    /// confirm waits up to 330 s): the vault socket only, not
    /// `_migrate_send`'s.
    pub vault_sock_max_time: Option<Duration>,
    /// Between tries. [`PAUSE`]; a test may shorten it.
    pub pause: Duration,
    /// `daemon.port` in the K2 home: a refused loopback connect tries the
    /// port published there when it differs (`_k2_follow_port_move`).
    pub port_file: Option<PathBuf>,
    /// Where a refused passport may have been minted again.
    pub healer: Option<Box<dyn Healer>>,
    /// The one heal on a refusal only when the answer is the auth gate's
    /// ([`gate_refusal`]), not on any answer that holds its words. Off by
    /// default (the shell's rule: the words anywhere in the answer).
    pub heal_on_gate_only: bool,
    healed: bool,
    /// Refused connects `_k2_curl` tried again before the last answer.
    refused: std::cell::Cell<u32>,
    /// The vault socket and the `X-K2-Tty` value of the call under way
    /// ([`Client::vault_sock`]).
    vault_sock: Option<(PathBuf, bool)>,
    /// The vault socket call under way is `_migrate_send`'s
    /// ([`Client::migrate_sock`]): its `--max-time` and, for a GET,
    /// `_K2_CURL_TRIES=1`.
    migrate_sock: bool,
    /// The credential and `--max-time` of the [`Kind::Own`] call being
    /// made ([`Client::own_curl`]).
    own: Option<(Option<String>, Duration)>,
    /// The [`Kind::Own`] call under way is sent once
    /// ([`Client::own_curl_once`]).
    own_once: bool,
    /// The [`Kind::Own`] POST under way carries no data at all
    /// ([`Client::own_curl_bare`]).
    own_bare: bool,
    /// The `--connect-timeout` of the [`Kind::Own`] call under way, when
    /// it is not the default ([`Client::own_curl_bare`]).
    own_connect: Option<Duration>,
    /// The cell socket of the call under way that has no passport to
    /// send ([`Client::post_form_uds_bare`], [`Client::send_bare`]).
    bare_sock: Option<PathBuf>,
}

/// How [`Client::own_curl`] sends: `_k2_curl` with the caller's config
/// (`url`, `_k2_cfg_auth <credential>`, a `data-raw` body), `-s`, connect
/// 5 s.
#[derive(Debug, Clone, Copy)]
pub struct OwnCurl<'a> {
    /// `-X POST` with the body and `Content-Type: application/json`; else
    /// a GET with no body.
    pub post: bool,
    /// The `Authorization: Bearer` line; `None` or an empty one: no line.
    pub credential: Option<&'a str>,
    /// `--max-time`.
    pub max_time: Duration,
    /// `_permission_note` runs on the answer.
    pub note: bool,
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
            // The unit tests' clients start with none, whatever the
            // environment that runs them holds.
            conversation: if cfg!(test) {
                Vec::new()
            } else {
                codex_headers_from_env()
            },
            session: false,
            no_token: false,
            version: crate::embedded::version().to_string(),
            force_tcp: false,
            tls: false,
            via: None,
            unsendable: None,
            host_header: None,
            post_max_time: None,
            send_max_time: None,
            compute_post_max_time: None,
            vault_sock_max_time: None,
            pause: PAUSE,
            port_file: None,
            healer: None,
            heal_on_gate_only: false,
            healed: false,
            refused: std::cell::Cell::new(0),
            vault_sock: None,
            migrate_sock: false,
            own: None,
            own_once: false,
            own_bare: false,
            own_connect: None,
            bare_sock: None,
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

    /// `_sf_agents_off_probe`'s request ([`Kind::FeedbackOptin`]): the
    /// GET of [`FEEDBACK_SETTINGS_ENDPOINT`], on the cell socket when
    /// [`Client::has_cell_socket`], else over TCP, as
    /// [`Client::permission_enabled`] sends its own. The caller reads the
    /// answer. Needs a passport.
    pub fn feedback_settings(&mut self) -> Result<Response, Error> {
        let channel = if self.has_cell_socket() {
            Channel::Uds
        } else {
            Channel::Tcp
        };
        self.run(
            Kind::FeedbackOptin,
            channel,
            FEEDBACK_SETTINGS_ENDPOINT,
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

    /// A GET of `path` as it is, over TCP, with no credential and no query
    /// of ours: no `project=`, no Bearer. For what the daemon
    /// shows anyone on this machine (`/boot-status`). A refused or timed
    /// out connect is tried again as `cli_request`'s is; no heal, no note.
    pub fn get_public(&mut self, path: &str) -> Result<Response, Error> {
        self.run(Kind::Public, Channel::Tcp, path, &[], &[], Instant::now())
            .map_err(|f| f.error)
    }

    /// `_k2_send_once_supported`: one GET of [`SEND_ONCE_ENDPOINT`]
    /// ([`Kind::SendOnce`]). True when the answer names
    /// [`SEND_ONCE_FEATURE`]: the daemon answers a repeat of a send's
    /// idempotency key with its first answer. A failed call is false.
    pub fn send_once_supported(&mut self) -> bool {
        match self.run(
            Kind::SendOnce,
            Channel::Tcp,
            SEND_ONCE_ENDPOINT,
            &[],
            &[],
            Instant::now(),
        ) {
            Ok(r) => contains(&r.body, SEND_ONCE_FEATURE),
            Err(_) => false,
        }
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

    /// `k2 api status`'s GET of `path` ([`Kind::BootStatus`]): over TCP,
    /// this client's credential as Bearer, no query; connect 5 s, at most
    /// 10 s; a refused connect is tried again; no heal, no line, no note.
    pub fn get_boot_status(&mut self, path: &str) -> Result<Response, Error> {
        self.run(
            Kind::BootStatus,
            Channel::Tcp,
            path,
            &[],
            &[],
            Instant::now(),
        )
        .map_err(|f| f.error)
    }

    /// `_setup_send <GET|POST> <ep> <as_json> [<body>]` (`cli/src/setup.sh`):
    /// `ep`, then `?format=text` when `text`, the token as Bearer (in a
    /// session with no passport, none: [`Client::session`]), over TCP; a POST
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

    /// A `_k2_curl` of the caller's own ([`OwnCurl`], [`Kind::Own`]):
    /// `endpoint` as it is, over TCP, its credential or none. The body
    /// comes back whatever the status.
    pub fn own_curl(
        &mut self,
        endpoint: &str,
        how: OwnCurl<'_>,
        body: &[u8],
    ) -> Result<Response, Error> {
        self.own = Some((how.credential.map(str::to_string), how.max_time));
        let kind = Kind::Own {
            post: how.post,
            note: how.note,
        };
        let r = self.run(kind, Channel::Tcp, endpoint, &[], body, Instant::now());
        self.own = None;
        r.map_err(|f| f.error)
    }

    /// [`Client::own_curl`] with `_K2_CURL_TRIES=1`: a refused connect is
    /// not tried again.
    pub fn own_curl_once(
        &mut self,
        endpoint: &str,
        how: OwnCurl<'_>,
        body: &[u8],
    ) -> Result<Response, Error> {
        self.own_once = true;
        let r = self.own_curl(endpoint, how, body);
        self.own_once = false;
        r
    }

    /// [`Client::own_curl`] as `_daemon_local_post` and `_daemon_wait_ready`
    /// send it (`cli/src/daemon.sh`): `--connect-timeout` is `connect`, and
    /// a POST carries no data at all (`-X POST` alone: no `Content-Type`,
    /// no `Content-Length`, no body).
    pub fn own_curl_bare(
        &mut self,
        endpoint: &str,
        how: OwnCurl<'_>,
        connect: Duration,
    ) -> Result<Response, Error> {
        self.own_bare = how.post;
        self.own_connect = Some(connect);
        let r = self.own_curl(endpoint, how, &[]);
        self.own_bare = false;
        self.own_connect = None;
        r
    }

    /// The credential this client's TCP requests carry (the shell's
    /// `$TOKEN`), if any: for a caller that sends it in a call of its own
    /// ([`Client::own_curl`]).
    pub fn tcp_credential(&self) -> Option<String> {
        self.tcp_token().map(str::to_string)
    }

    /// `cli_post <endpoint> <pairs>` ([`Kind::QueryPost`]): a POST with an
    /// empty body and the pairs in the query after `project=`, over TCP.
    /// The body comes back whatever the status; no heal, no no-answer
    /// line. A refused connect is tried again (nothing went out).
    pub fn post_query(&mut self, endpoint: &str, args: &[&str]) -> Result<Response, Error> {
        self.run(
            Kind::QueryPost,
            Channel::Tcp,
            endpoint,
            args,
            &[],
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
            Call::QueryPost(args) => self.run(Kind::QueryPost, channel, endpoint, args, &[], t0),
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
        if kind.heals()
            && contains(&answer.body, PASSPORT_REFUSAL)
            && (!self.heal_on_gate_only || gate_refusal(answer.status, &answer.body))
            && self.heal()
        {
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
            | Kind::SetupPost
            | Kind::QueryPost => MAX_TIME,
            Kind::Probe | Kind::SendOnce => PROBE_MAX_TIME,
            Kind::SidecarGet | Kind::SidecarPost => SIDECAR_MAX_TIME,
            Kind::ComputeGet => COMPUTE_GET_MAX_TIME,
            Kind::ComputePost => self.compute_post_max_time.unwrap_or(COMPUTE_POST_MAX_TIME),
            Kind::VaultSockGet | Kind::VaultSockPost if self.migrate_sock => MIGRATE_SOCK_MAX_TIME,
            Kind::VaultSockGet | Kind::VaultSockPost => {
                self.vault_sock_max_time.unwrap_or(VAULT_SOCK_MAX_TIME)
            }
            Kind::PassportProject => PASSPORT_PROJECT_MAX_TIME,
            Kind::Cards => CARDS_TIME,
            Kind::FeedbackOptin => FEEDBACK_MAX_TIME,
            Kind::UpdateVersion => UPDATE_MAX_TIME,
            Kind::Own { .. } => self.own.as_ref().map_or(MAX_TIME, |o| o.1),
            Kind::BootStatus => BOOT_STATUS_MAX_TIME,
            Kind::ConnectProxy => CONNECT_PROXY_MAX_TIME,
            Kind::ControlPlane { .. } => MAX_TIME,
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
        let connect = match (kind, self.own_connect) {
            (Kind::Own { .. }, Some(c)) => c,
            _ => kind.connect_timeout(),
        };
        loop {
            let deadline = Instant::now() + max;
            match wire::connect(&target, connect, deadline) {
                Ok(mut conn) => {
                    let r = wire::exchange(&mut conn, &request, deadline);
                    // Every answer, a retried one too: the compare is on
                    // what came back, never a request of its own.
                    if let Ok(a) = &r {
                        super::brand_headers::note_answer(&self.version, a, &target);
                    }
                    return r;
                }
                Err(Failure::Connect(e)) => {
                    let own_once = self.own_once && matches!(kind, Kind::Own { .. });
                    if kind.once() || own_once || self.migrate_sock || tries >= TRIES {
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
        // A port as the shell's rule has it (`_k2_follow_port_move`).
        let p = crate::port::parse(digits.as_bytes())?;
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
        let vault_sock = matches!(kind, Kind::VaultSockGet | Kind::VaultSockPost);
        let (target, host) = match channel {
            // The person's channel: no credential of ours.
            _ if vault_sock => {
                let Some((sock, _)) = &self.vault_sock else {
                    return Err(Error::NoCellSocket {
                        endpoint: endpoint.to_string(),
                    });
                };
                (uds_target(sock, endpoint)?, "localhost".to_string())
            }
            Channel::Tcp if self.unsendable.is_some() => {
                return Err(Error::Broken {
                    endpoint: endpoint.to_string(),
                    why: format!(
                        "won't send to {}: a host or proxy setting this program doesn't read",
                        self.unsendable.as_deref().unwrap_or_default()
                    ),
                    curl: wire::curl::URL_MALFORMAT,
                });
            }
            Channel::Tcp => {
                // The credential is the first header, an `Authorization:
                // Bearer` line of the shell's curl config (`_k2_cfg_auth`),
                // read before the command line's `-H`s. Never the query.
                let credential = match kind {
                    Kind::Public | Kind::UpdateVersion => None,
                    // `_k2_cfg_auth "${TOKEN:-}"`: no header for no token.
                    Kind::SendOnce if self.tcp_token().is_none() => None,
                    // `_setup_token` (0.46.17): in a session (`HOOK_SOCK`
                    // set) the passport, and with none no credential at
                    // all, never the owner token.
                    Kind::SetupGet | Kind::SetupPost
                        if self.session && self.passport().is_none() =>
                    {
                        None
                    }
                    Kind::Probe | Kind::BearerForm => Some(self.passport().map(Passport::as_str)),
                    Kind::Own { .. } => match self.own.as_ref().and_then(|o| o.0.as_deref()) {
                        Some(t) if !t.is_empty() => Some(Some(t)),
                        _ => None,
                    },
                    // `$TOKEN` empty on purpose: no header.
                    _ if self.no_token && self.auth == Auth::None => None,
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
                let (host, port) = (self.host.clone(), self.port);
                let header = self
                    .host_header
                    .clone()
                    .unwrap_or_else(|| format!("{host}:{port}"));
                let target = if self.tls {
                    // The proxy rides only here: `via` is set by
                    // `remote::apply` alone, for a host that is not this
                    // computer.
                    let via = self.via.clone().map(|proxy| {
                        Box::new(wire::Tunnel {
                            proxy,
                            user_agent: format!("k5/{}", self.version),
                        })
                    });
                    Target::Tls { host, port, via }
                } else {
                    Target::Tcp { host, port }
                };
                (target, header)
            }
            // No passport: no credential line ([`Client::post_form_uds_bare`],
            // [`Client::send_bare`]).
            Channel::Uds if self.bare_sock.is_some() && self.passport().is_none() => {
                let sock = self.bare_sock.as_deref().unwrap_or(Path::new(""));
                (uds_target(sock, endpoint)?, "localhost".to_string())
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
        let daemon = !matches!(kind, Kind::ControlPlane { .. });
        // A Codex call's conversation headers: `_k2_curl` adds them to
        // every daemon request (not a `_K2_CURL_NOT_DAEMON` one) as lines
        // of its curl config, which curl reads before the command line's
        // `-H`s: right after the credential. A body on stdin with no
        // config of the caller's (`cli_post_json_stdin`, the vault
        // socket's POST) gets them from a config file of its own, put
        // after the hand-over headers.
        let stdin_body = matches!(kind, Kind::PostJsonStdin | Kind::VaultSockPost);
        if daemon && !stdin_body {
            headers.extend(self.conversation.iter().cloned());
        }
        if self.would_refuse && daemon {
            headers.push((WOULD_REFUSE_HEADER.into(), "1".into()));
        }
        if daemon {
            headers.push((VERSION_HEADER.into(), self.version.clone()));
            headers.push((VIA_HEADER.into(), VIA_NATIVE.into()));
        }
        if daemon
            && self.owner_from_disk
            && matches!(self.auth, Auth::Owner(_))
            && kind != Kind::Public
        {
            headers.push((OWNER_SOURCE_HEADER.into(), OWNER_SOURCE_DISK.into()));
        }
        if daemon && stdin_body {
            headers.extend(self.conversation.iter().cloned());
        }
        headers.extend(late_credential);
        if let Some((_, tty)) = self.vault_sock.as_ref().filter(|_| vault_sock) {
            headers.push((TTY_HEADER.into(), if *tty { "1" } else { "0" }.into()));
        }

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
            Kind::QueryPost => {
                // `cli_post_uds`: no `project=`, and no `?` with no pairs.
                let q = if tcp {
                    let mut q = format!("{endpoint}?{}", with_project());
                    for a in args {
                        q.push('&');
                        q.push_str(a);
                    }
                    q
                } else if joined.is_empty() {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}?{joined}")
                };
                (
                    Method::Post,
                    q,
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
            Kind::SendJsonGet
            | Kind::SidecarGet
            | Kind::ComputeGet
            | Kind::PassportProject
            | Kind::Cards
            | Kind::FeedbackOptin => {
                let payload = String::from_utf8_lossy(body);
                let path = if payload.is_empty() {
                    endpoint.to_string()
                } else {
                    wire::append_get_query(endpoint, &payload)
                };
                (Method::Get, path, None)
            }
            Kind::Own { post: true, .. } if self.own_bare => {
                (Method::Post, endpoint.to_string(), None)
            }
            Kind::SendJsonPost
            | Kind::ComputePost
            | Kind::SidecarPost
            | Kind::VaultSockPost
            | Kind::ConnectProxy
            | Kind::Own { post: true, .. } => {
                (Method::Post, endpoint.to_string(), json(&mut headers))
            }
            Kind::ControlPlane { method } => {
                let body = if body.is_empty() {
                    None
                } else {
                    json(&mut headers)
                };
                (method, endpoint.to_string(), body)
            }
            Kind::VaultSockGet | Kind::Own { post: false, .. } => {
                (Method::Get, endpoint.to_string(), None)
            }
            Kind::Public
            | Kind::Probe
            | Kind::UpdateVersion
            | Kind::BootStatus
            | Kind::SendOnce => (Method::Get, endpoint.to_string(), None),
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
        Failure::Tunnel(why) => Error::Broken {
            endpoint,
            why,
            curl: wire::curl::RECV_ERROR,
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
        .map_or_else(|| PathBuf::from(crate::offline::default_tmpdir()), PathBuf::from);
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
        let rows: [(Error, i32); 12] = [
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
            (Error::NoOwnerInSession { endpoint: ep() }, 1),
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
            Kind::FeedbackOptin,
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
            (ComputeGet, false, false, false, false, false, false, false),
            (ComputePost, false, false, false, false, false, false, false),
            (SidecarPost, false, false, false, false, false, false, false),
            (
                VaultSockGet,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            ),
            (
                VaultSockPost,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
            ),
            (
                Own {
                    post: true,
                    note: true,
                },
                false,
                false,
                false,
                false,
                false,
                true,
                false,
            ),
            (
                Own {
                    post: false,
                    note: false,
                },
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            ),
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
            (Cards, false, false, false, false, false, false, true),
            (
                FeedbackOptin,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
            ),
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
            (SendOnce, false, false, false, false, false, false, true),
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
        assert_eq!(Kind::Cards.connect_timeout(), Duration::from_secs(1));
        assert_eq!(
            Kind::FeedbackOptin.connect_timeout(),
            Duration::from_secs(1)
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
            (Kind::ComputeGet, 60),
            (Kind::ComputePost, 120),
            (Kind::SidecarPost, 120),
            (Kind::VaultSockGet, 180),
            (Kind::VaultSockPost, 180),
            (Kind::PassportProject, 15),
            (Kind::Cards, 1),
            (Kind::FeedbackOptin, 1),
            (Kind::UpdateVersion, 4),
            (Kind::SetupGet, 30),
            (Kind::SetupPost, 30),
            (Kind::QueryPost, 30),
            (Kind::BootStatus, 10),
            (Kind::SendOnce, 5),
            (Kind::ConnectProxy, 45),
            (
                Kind::ControlPlane {
                    method: Method::Put,
                },
                30,
            ),
        ] {
            assert_eq!(c.max_time(k), Duration::from_secs(want), "{k:?}");
        }
        // K2_CP_MAX_TIME: the compute POST only.
        c.compute_post_max_time = Some(Duration::from_secs(3700));
        assert_eq!(c.max_time(Kind::ComputePost), Duration::from_secs(3700));
        assert_eq!(c.max_time(Kind::ComputeGet), Duration::from_secs(60));
        assert_eq!(c.max_time(Kind::SendJsonPost), Duration::from_secs(330));
        // _vault_sock_call's fourth word: the vault socket only.
        c.vault_sock_max_time = Some(Duration::from_secs(330));
        assert_eq!(c.max_time(Kind::VaultSockPost), Duration::from_secs(330));
        assert_eq!(c.max_time(Kind::VaultSockGet), Duration::from_secs(330));
        c.migrate_sock = true;
        assert_eq!(c.max_time(Kind::VaultSockPost), Duration::from_secs(600));
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

    /// `no_token`: a request goes out over TCP with no `Authorization`
    /// header and no owner-source header (the shell's `$TOKEN` empty);
    /// the probe still needs a passport. Without it, nothing is sent.
    #[test]
    fn no_token_sends_no_credential() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::None);
        c.owner_from_disk = true;
        assert!(matches!(
            c.build(Kind::Request, Channel::Tcp, "/cli/x", &[], &[]),
            Err(Error::NoCredential { .. })
        ));
        c.no_token = true;
        for kind in [Kind::Request, Kind::RequestPost] {
            let (_, r) = c
                .build(kind, Channel::Tcp, "/cli/x", &["a=1"], &[])
                .unwrap();
            let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, [VERSION_HEADER, VIA_HEADER], "{kind:?}");
            assert!(r.target.ends_with("&a=1"), "{}", r.target);
        }
        assert!(matches!(
            c.build(Kind::Probe, Channel::Tcp, "/cli/whoami", &[], &[]),
            Err(Error::NoCredential { .. })
        ));
        // An owner token is sent as before.
        c.auth = Auth::Owner("o".into());
        let (_, r) = c
            .build(Kind::Request, Channel::Tcp, "/cli/x", &[], &[])
            .unwrap();
        assert_eq!(r.headers[0], ("Authorization".into(), "Bearer o".into()));
    }

    /// `_setup_token`: in a session with no passport, setup's requests
    /// carry no credential at all (never the owner token), and the
    /// owner's source header still goes; outside a session, the owner
    /// token as before.
    #[test]
    fn setup_in_a_session_with_no_passport_sends_no_credential() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        c.owner_from_disk = true;
        let names = |c: &Client, kind| {
            let (_, r) = c
                .build(kind, Channel::Tcp, "/cli/first-run/status", &[], &[])
                .unwrap();
            r.headers.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
        };
        for kind in [Kind::SetupGet, Kind::SetupPost] {
            assert_eq!(
                names(&c, kind)[0],
                "Authorization",
                "{kind:?} outside a session"
            );
        }
        c.session = true;
        for kind in [Kind::SetupGet, Kind::SetupPost] {
            let n = names(&c, kind);
            assert!(!n.iter().any(|h| h == "Authorization"), "{kind:?}: {n:?}");
            assert!(
                n.iter().any(|h| h == OWNER_SOURCE_HEADER),
                "{kind:?}: {n:?}"
            );
        }
        // Every other kind keeps the token (the socket-first ones refuse
        // instead, before they build).
        assert_eq!(names(&c, Kind::Request)[0], "Authorization");
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

    /// `_k2_send_once_supported`'s GET: the token as Bearer (none with no
    /// token), the binary's headers, no query; connect 3 s.
    #[test]
    fn the_send_once_get() {
        assert_eq!(Kind::SendOnce.connect_timeout(), Duration::from_secs(3));
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        c.owner_from_disk = true;
        let (_, r) = c
            .build(Kind::SendOnce, Channel::Tcp, SEND_ONCE_ENDPOINT, &[], &[])
            .unwrap();
        assert_eq!((r.method, r.target.as_str()), (Method::Get, "/boot-status"));
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
        assert_eq!(r.headers[0].1, "Bearer o");
        let none = Client::new("127.0.0.1", 9, "/p", Auth::None);
        let (_, r) = none
            .build(Kind::SendOnce, Channel::Tcp, SEND_ONCE_ENDPOINT, &[], &[])
            .unwrap();
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [VERSION_HEADER, VIA_HEADER]);
        let policy = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cell-socket.sh"),
        )
        .unwrap();
        assert!(
            policy.contains(r#"body="$(_K2_CURL_TRIES=1 _k2_curl "$_K2_CFG" -s -G --connect-timeout 3 --max-time 5 "${BASE_URL}/boot-status" 2>/dev/null)" || body="""#),
            "_k2_send_once_supported's request moved"
        );
        assert!(policy.contains(r#"*'"send-once"'*) _K2_SEND_ONCE=1 ;;"#));
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
                ComputeGet,
                ComputePost,
                SidecarPost,
                PassportProject,
                Cards,
                FeedbackOptin,
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
        // `_compute_send`: no `project=`, the payload as the query or the
        // JSON body.
        let (_, r) = owner
            .build(ComputeGet, Channel::Tcp, "/cli/x", &[], b"job=a")
            .unwrap();
        assert_eq!((r.method, r.target.as_str()), (Method::Get, "/cli/x?job=a"));
        let (_, r) = owner
            .build(ComputePost, Channel::Tcp, "/cli/x", &[], b"{}")
            .unwrap();
        assert_eq!((r.method, r.target.as_str()), (Method::Post, "/cli/x"));
        assert_eq!(
            r.headers.last(),
            Some(&("Content-Type".into(), "application/json".into()))
        );
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

    /// `_k2_curl` with the caller's config: its credential (none for an
    /// empty one), the path as it is, a JSON body for a POST, its time.
    #[test]
    fn an_own_curl_sends_its_caller_s_credential() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        c.own = Some((Some("f.t".into()), Duration::from_secs(60)));
        let post = Kind::Own {
            post: true,
            note: true,
        };
        let (_, r) = c
            .build(post, Channel::Tcp, "/cli/federation/send", &[], b"{}")
            .unwrap();
        assert_eq!(
            (r.method, r.target.as_str()),
            (Method::Post, "/cli/federation/send")
        );
        assert_eq!(r.headers[0], ("Authorization".into(), "Bearer f.t".into()));
        assert_eq!(
            r.headers.last(),
            Some(&("Content-Type".into(), "application/json".into()))
        );
        assert_eq!(c.max_time(post), Duration::from_secs(60));
        for none in [None, Some(String::new())] {
            c.own = Some((none, Duration::from_secs(30)));
            let get = Kind::Own {
                post: false,
                note: false,
            };
            let (_, r) = c.build(get, Channel::Tcp, "/cli/x", &[], &[]).unwrap();
            assert_eq!((r.method, r.target.as_str()), (Method::Get, "/cli/x"));
            assert!(r.headers.iter().all(|(n, _)| n != "Authorization"));
            assert!(r.body.is_none());
        }
    }

    /// `_daemon_local_post`'s `-X POST` with no data: no body, so no
    /// `Content-Type` and no `Content-Length`; a GET is as before.
    #[test]
    fn a_bare_own_post_carries_no_data() {
        let mut c = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        c.own = Some((Some("d.t".into()), Duration::from_secs(30)));
        c.own_bare = true;
        let post = Kind::Own {
            post: true,
            note: false,
        };
        let (_, r) = c
            .build(post, Channel::Tcp, "/cli/daemon/stop", &[], &[])
            .unwrap();
        assert_eq!(
            (r.method, r.target.as_str()),
            (Method::Post, "/cli/daemon/stop")
        );
        assert_eq!(r.headers[0], ("Authorization".into(), "Bearer d.t".into()));
        assert!(r.headers.iter().all(|(n, _)| n != "Content-Type"));
        assert!(r.body.is_none());
        let text = String::from_utf8(r.encode()).unwrap();
        assert!(!text.contains("Content-Length"), "{text}");
        c.own_bare = false;
        let (_, r) = c
            .build(post, Channel::Tcp, "/cli/daemon/stop", &[], &[])
            .unwrap();
        assert!(r.body.is_some());
    }

    /// `_K2_CURL_TRIES=1`: [`Client::own_curl_once`] connects once where
    /// [`Client::own_curl`] tries a refused connect again.
    #[test]
    fn an_own_curl_once_is_not_tried_again() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut c = Client::new("127.0.0.1", port, "/p", Auth::None);
        c.pause = Duration::ZERO;
        let how = OwnCurl {
            post: true,
            credential: None,
            max_time: Duration::from_secs(5),
            note: false,
        };
        assert!(c.own_curl_once("/cli/x", how, b"{}").is_err());
        assert_eq!(c.refused_before_answer(), 0);
        assert!(!c.own_once);
        assert!(c.own_curl("/cli/x", how, b"{}").is_err());
        assert_eq!(c.refused_before_answer(), TRIES - 1);
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

    /// `cli_post` over TCP: `project=` first, then the pairs, the token as
    /// Bearer. `cli_post_uds` on the cell socket: the pairs alone (no `?`
    /// with none), the passport as Bearer. Both an empty POST body.
    #[cfg(unix)]
    #[test]
    fn the_query_post_request() {
        let p = Client::new(
            "127.0.0.1",
            4000,
            "/w s",
            Auth::Passport {
                passport: Passport::new("s.x").unwrap(),
                sock: Some(PathBuf::from("/tmp/k5-qp.sock")),
            },
        );
        let pairs = ["label=acme", "check=1"];
        let shape = |p: &Client, channel, args: &[&str]| {
            let (_, r) = p
                .build(
                    Kind::QueryPost,
                    channel,
                    "/cli/publish/subdomain/claim",
                    args,
                    &[],
                )
                .unwrap();
            let body = r.body.as_ref().expect("a body");
            assert!(body.bytes.is_empty() && body.curl_form_type);
            assert_eq!(r.method, Method::Post);
            assert_eq!(r.headers[0], ("Authorization".into(), "Bearer s.x".into()));
            r.target
        };
        assert_eq!(
            shape(&p, Channel::Tcp, &pairs),
            "/cli/publish/subdomain/claim?project=%2Fw%20s&label=acme&check=1"
        );
        assert_eq!(
            shape(&p, Channel::Uds, &pairs),
            "/cli/publish/subdomain/claim?label=acme&check=1"
        );
        assert_eq!(shape(&p, Channel::Uds, &[]), "/cli/publish/subdomain/claim");
        assert_eq!(
            shape(&p, Channel::Tcp, &[]),
            "/cli/publish/subdomain/claim?project=%2Fw%20s"
        );
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
    fn only_the_auth_gate_s_answer_is_its_refusal() {
        assert!(gate_refusal(403, GATE_REFUSAL.as_bytes()));
        assert!(gate_refusal(401, GATE_REFUSAL.as_bytes()));
        // The words in a handler's answer: a document's text, a conflict.
        let text = format!(r#"{{"stdout":"{PASSPORT_REFUSAL}\n","exit":0}}"#);
        assert!(!gate_refusal(200, text.as_bytes()));
        assert!(!gate_refusal(409, GATE_REFUSAL.as_bytes()));
        assert!(!gate_refusal(403, text.as_bytes()));
        assert!(!gate_refusal(403, b""));
        assert!(!gate_refusal(403, format!("{GATE_REFUSAL}\n").as_bytes()));
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
        #[cfg(not(unix))]
        std::fs::remove_file(&file).unwrap();
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
            ("65536\n", "127.0.0.1", None),
            ("0\n", "127.0.0.1", None),
            ("04001\n", "127.0.0.1", None),
        ] {
            std::fs::write(&file, content).unwrap();
            assert_eq!(c.moved_port(host, 4000), want, "{content:?} {host}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// No credential ever follows a redirect: a 3xx is the answer, on the
    /// transport's calls and the urllib adapter's alike. A first server
    /// answers 302 with a `Location` at a second one, which must get
    /// nothing (the clone engine's redirect rule).
    #[test]
    fn a_redirect_is_the_answer_and_never_followed() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let second = TcpListener::bind("127.0.0.1:0").unwrap();
        let p2 = second.local_addr().unwrap().port();
        second.set_nonblocking(true).unwrap();
        let first = TcpListener::bind("127.0.0.1:0").unwrap();
        let p1 = first.local_addr().unwrap().port();
        let answer = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{p2}/cli/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let served = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut c, _) = first.accept().unwrap();
                c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 1024];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                c.write_all(answer.as_bytes()).unwrap();
            }
        });
        let mut c = Client::new("127.0.0.1", p1, "/p", Auth::Owner("owner-tok".into()));
        c.pause = Duration::from_millis(1);
        let r = c
            .call(Channel::Tcp, "/cli/x", Call::Request(&[]))
            .expect("the 302 is the answer");
        assert_eq!(r.status, 302);
        let opener = super::super::urllib::Opener::from_client(&c);
        let req = super::super::urllib::Request::new(
            super::super::urllib::Method::Get,
            "/cli/x",
            Duration::from_secs(5),
        );
        assert_eq!(opener.open(&req).status(), Some(302));
        served.join().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        match second.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok((_, from)) => panic!("the redirect was followed: a connection from {from}"),
            Err(e) => panic!("accept: {e}"),
        }
    }

    /// `_k2_codex_headers`: both headers only with `CODEX_THREAD_ID` set
    /// and not empty and the switch not off; each id checked on its own.
    #[test]
    fn codex_headers_follow_the_shell_s_rules() {
        use std::ffi::OsStr;
        let h = |t: Option<&str>, r: Option<&str>, s: Option<&str>| {
            codex_headers(t.map(OsStr::new), r.map(OsStr::new), s.map(OsStr::new))
        };
        let both = vec![
            (CONVERSATION_HEADER.to_string(), "root-1".to_string()),
            (THREAD_HEADER.to_string(), "thread-1".to_string()),
        ];
        assert_eq!(h(Some("thread-1"), Some("root-1"), None), both);
        // No thread, or an empty one: none, the root's either.
        assert_eq!(h(None, Some("root-1"), None), vec![]);
        assert_eq!(h(Some(""), Some("root-1"), None), vec![]);
        // The switch: exactly 0, off, false or no turns them off.
        for off in ["0", "off", "false", "no"] {
            assert_eq!(
                h(Some("thread-1"), Some("root-1"), Some(off)),
                vec![],
                "{off}"
            );
        }
        for on in ["", "1", "on", "OFF", "No", " 0", "0 "] {
            assert_eq!(
                h(Some("thread-1"), Some("root-1"), Some(on)),
                both,
                "{on:?}"
            );
        }
        // Only the thread: its header alone.
        assert_eq!(
            h(Some("thread-1"), None, None),
            vec![(THREAD_HEADER.to_string(), "thread-1".to_string())]
        );
        // An id that is not 1 to 128 of [A-Za-z0-9_-] is left out alone.
        let long = "a".repeat(129);
        let max = "Z9_-".repeat(32);
        for bad in ["", "a b", "a\nb", "a.b", "a/b", "\u{e9}", long.as_str()] {
            assert_eq!(
                h(Some("thread-1"), Some(bad), None),
                vec![(THREAD_HEADER.to_string(), "thread-1".to_string())],
                "{bad:?}"
            );
        }
        assert_eq!(
            h(Some("bad id"), Some("root-1"), None),
            vec![(CONVERSATION_HEADER.to_string(), "root-1".to_string())]
        );
        assert_eq!(
            h(Some(max.as_str()), Some(max.as_str()), None),
            vec![
                (CONVERSATION_HEADER.to_string(), max.clone()),
                (THREAD_HEADER.to_string(), max.clone()),
            ]
        );
    }

    /// Not UTF-8: a thread still turns the switch on (the shell's `-n`),
    /// but no id that is not ASCII is sent; a switch that is not UTF-8
    /// is not off.
    #[cfg(unix)]
    #[test]
    fn codex_headers_of_bytes_that_are_not_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bad = OsStr::from_bytes(b"t\xff");
        assert_eq!(
            codex_headers(Some(bad), Some(OsStr::new("root-1")), None),
            vec![(CONVERSATION_HEADER.to_string(), "root-1".to_string())]
        );
        assert_eq!(
            codex_headers(
                Some(OsStr::new("thread-1")),
                None,
                Some(OsStr::from_bytes(b"0\xff"))
            ),
            vec![(THREAD_HEADER.to_string(), "thread-1".to_string())]
        );
    }

    /// Where `_k2_curl` puts them: right after the credential (its curl
    /// config is read first), on every daemon kind and both channels;
    /// after the hand-over headers for a body on stdin with no config
    /// (`cli_post_json_stdin`, the vault socket's POST); never to the
    /// Connect control plane. The embedded Python's come after its
    /// hand-over headers.
    #[test]
    fn codex_headers_go_where_k2_curl_puts_them() {
        let conv = vec![
            (CONVERSATION_HEADER.to_string(), "root-1".to_string()),
            (THREAD_HEADER.to_string(), "thread-1".to_string()),
        ];
        let names =
            |r: &Request| -> Vec<String> { r.headers.iter().map(|(n, _)| n.clone()).collect() };
        let mut owner = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        owner.conversation = conv.clone();
        owner.owner_from_disk = true;
        owner.would_refuse = true;
        let first = [
            "Authorization",
            CONVERSATION_HEADER,
            THREAD_HEADER,
            WOULD_REFUSE_HEADER,
            VERSION_HEADER,
            VIA_HEADER,
            OWNER_SOURCE_HEADER,
        ];
        for kind in [
            Kind::Request,
            Kind::RequestPost,
            Kind::SendJsonGet,
            Kind::QueryPost,
        ] {
            let (_, r) = owner.build(kind, Channel::Tcp, "/cli/x", &[], &[]).unwrap();
            assert_eq!(names(&r), first, "{kind:?}");
            assert_eq!(r.headers[1].1, "root-1");
            assert_eq!(r.headers[2].1, "thread-1");
        }
        let (_, r) = owner
            .build(Kind::PostJson, Channel::Tcp, "/cli/x", &[], b"{}")
            .unwrap();
        assert_eq!(names(&r), [&first[..], &["Content-Type"]].concat());
        // A body on stdin: after the hand-over headers, before its Bearer.
        let (_, r) = owner
            .build(Kind::PostJsonStdin, Channel::Tcp, "/cli/x", &[], b"{}")
            .unwrap();
        assert_eq!(
            names(&r),
            [
                WOULD_REFUSE_HEADER,
                VERSION_HEADER,
                VIA_HEADER,
                OWNER_SOURCE_HEADER,
                CONVERSATION_HEADER,
                THREAD_HEADER,
                "Authorization",
                "Content-Type"
            ]
        );
        // No credential (the public GET): first.
        let (_, r) = owner
            .build(Kind::Public, Channel::Tcp, "/boot-status", &[], &[])
            .unwrap();
        assert_eq!(
            names(&r),
            [
                CONVERSATION_HEADER,
                THREAD_HEADER,
                WOULD_REFUSE_HEADER,
                VERSION_HEADER,
                VIA_HEADER
            ]
        );
        // The cell socket and the vault socket: socket.rs's
        // codex_headers_on_the_cell_and_vault_sockets (Unix only).
        // The Connect control plane is not the daemon: none.
        let (_, r) = owner
            .build(
                Kind::ControlPlane {
                    method: Method::Post,
                },
                Channel::Tcp,
                "/v1/x",
                &[],
                b"{}",
            )
            .unwrap();
        assert_eq!(names(&r), ["Authorization", "Content-Type"]);
        // With none set, nothing new on any of them.
        let plain = Client::new("127.0.0.1", 9, "/p", Auth::Owner("o".into()));
        let (_, r) = plain
            .build(Kind::Request, Channel::Tcp, "/cli/x", &[], &[])
            .unwrap();
        assert_eq!(names(&r), ["Authorization", VERSION_HEADER, VIA_HEADER]);
        // The embedded Python: after its hand-over headers.
        let opener = super::super::urllib::Opener::from_client(&owner);
        let hand: Vec<&str> = opener.handover.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            hand,
            [
                WOULD_REFUSE_HEADER,
                VERSION_HEADER,
                VIA_HEADER,
                OWNER_SOURCE_HEADER,
                CONVERSATION_HEADER,
                THREAD_HEADER
            ]
        );
        let opener = super::super::urllib::Opener::from_client(&plain);
        assert!(opener.handover.iter().all(|(n, _)| !n.starts_with("X-K2-")));
    }
}
