//! The mgmt client: HTTP over the Unix socket with the service token, and act-as (S25 §12, C9).
//!
//! Every mgmt `POST` sends an `Idempotency-Key` (C9: required), generated per call unless the
//! caller passes one. Error mapping, retries and meta are the client API's (§6–§8).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sew_proto::clock::{Clock, SystemClock};
use sew_proto::mgmt::{ActAsTarget, SewVia};

use crate::api::{Client, service_client};
use crate::config::REQUEST_TIMEOUT;
use crate::error::ClientError;
use crate::retry::RetryPolicy;
use crate::secret::SecretString;

mod compliance;
mod exports;
mod people;
mod routes;

pub use routes::LONG_POLL_SLACK;

/// The default mgmt socket.
pub const DEFAULT_SOCKET: &str = "/run/sew/mgmt.sock";
/// The default service-token file.
pub const DEFAULT_TOKEN_FILE: &str = "/etc/sew/mgmt.token";
/// The service-token prefix.
pub const SERVICE_TOKEN_PREFIX: &str = "sewsvc_";

/// A mgmt API client. Cheap to clone.
#[derive(Clone)]
pub struct MgmtClient {
    socket: PathBuf,
    token: SecretString,
    retry: RetryPolicy,
    request_timeout: Duration,
    clock: Arc<dyn Clock>,
    client: Client,
}

impl fmt::Debug for MgmtClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MgmtClient")
            .field("socket", &self.socket)
            .field("token", &"***")
            .field("retry", &self.retry)
            .finish()
    }
}

impl MgmtClient {
    /// A client for `socket` sending `Authorization: Bearer <token>` (`sewsvc_…`). K2 uses this
    /// with the token from its vault.
    pub fn new(socket: impl Into<PathBuf>, token: impl Into<SecretString>) -> MgmtClient {
        MgmtClient::assemble(
            socket.into(),
            token.into(),
            RetryPolicy::default(),
            REQUEST_TIMEOUT,
            Arc::new(SystemClock),
        )
    }

    fn assemble(
        socket: PathBuf,
        token: SecretString,
        retry: RetryPolicy,
        request_timeout: Duration,
        clock: Arc<dyn Clock>,
    ) -> MgmtClient {
        let client = service_client(
            socket.clone(),
            token.clone(),
            None,
            retry.clone(),
            request_timeout,
            clock.clone(),
        );
        MgmtClient {
            socket,
            token,
            retry,
            request_timeout,
            clock,
            client,
        }
    }

    /// The defaults: `$SEW_MGMT_SOCKET` or `/run/sew/mgmt.sock`; the token from
    /// `$SEW_MGMT_TOKEN_FILE` or `/etc/sew/mgmt.token` (checked as [`MgmtClient::from_token_file`]).
    /// The only place the library reads the environment.
    pub fn from_env() -> Result<MgmtClient, ClientError> {
        let socket = std::env::var("SEW_MGMT_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_owned());
        let file =
            std::env::var("SEW_MGMT_TOKEN_FILE").unwrap_or_else(|_| DEFAULT_TOKEN_FILE.to_owned());
        MgmtClient::from_token_file(socket, Path::new(&file))
    }

    /// Reads the token from `token_file`: refuses a file readable by "other"
    /// (`mode & 0o007 != 0`), trims one trailing newline, requires the `sewsvc_` prefix.
    pub fn from_token_file(
        socket: impl Into<PathBuf>,
        token_file: &Path,
    ) -> Result<MgmtClient, ClientError> {
        let token = read_token_file(token_file)?;
        Ok(MgmtClient::new(socket, token))
    }

    /// A normal [`Client`] over the mgmt socket acting as an agent: every request carries
    /// `Authorization: Bearer sewsvc_…`, `Sew-Act-As` and `Sew-Via`. All typed `/api/v1/*`
    /// methods work on it; the gateway and every `auth_*` call return `Unsupported` without
    /// sending. Idempotency keys are scoped per acted-as agent on the server: K2 keys must be
    /// unique per agent and purpose (`k2-<purpose>-<row id>`).
    pub fn act_as(&self, target: ActAsTarget, via: SewVia) -> Client {
        service_client(
            self.socket.clone(),
            self.token.clone(),
            Some((target, via)),
            self.retry.clone(),
            self.request_timeout,
            self.clock.clone(),
        )
    }

    /// The same client with another retry policy (act-as clients made afterwards inherit it).
    pub fn with_retry(self, r: RetryPolicy) -> Self {
        MgmtClient::assemble(self.socket, self.token, r, self.request_timeout, self.clock)
    }

    /// The same client with another default request timeout.
    pub fn with_request_timeout(self, d: Duration) -> Self {
        MgmtClient::assemble(self.socket, self.token, self.retry, d, self.clock)
    }

    /// The socket path.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The service client (no act-as), for raw requests: `mgmt.client().request(…)`.
    pub fn client(&self) -> &Client {
        &self.client
    }
}

/// Reads and checks a service-token file (see [`MgmtClient::from_token_file`]).
pub fn read_token_file(path: &Path) -> Result<SecretString, ClientError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let meta = std::fs::metadata(path)
            .map_err(|e| ClientError::Config(format!("{}: {e}", path.display())))?;
        let mode = meta.permissions().mode();
        if mode & 0o007 != 0 {
            return Err(ClientError::Config(format!(
                "{} is readable by others (mode {:o}); run: chmod o-rwx {}",
                path.display(),
                mode & 0o777,
                path.display()
            )));
        }
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| ClientError::Config(format!("{}: {e}", path.display())))?;
    let token = text.strip_suffix('\n').unwrap_or(&text);
    let token = token.strip_suffix('\r').unwrap_or(token);
    if !token.starts_with(SERVICE_TOKEN_PREFIX) || token.len() <= SERVICE_TOKEN_PREFIX.len() {
        return Err(ClientError::Config(format!(
            "{} does not hold a service token (sewsvc_…)",
            path.display()
        )));
    }
    Ok(SecretString::new(token))
}
