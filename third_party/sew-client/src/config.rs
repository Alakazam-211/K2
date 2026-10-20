//! `ClientBuilder` and per-request options (S25 §6.1).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use sew_proto::clock::{Clock, SystemClock};
use sew_proto::http::IdempotencyKey;

use crate::api::Client;
use crate::error::ClientError;
use crate::retry::RetryPolicy;
use crate::secret::SecretString;
use crate::session::PassSource;

/// Default connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Default per-request timeout (S8 name kept).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Options for one call. Build with [`RequestOptions::new`] and the setters.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestOptions {
    /// A caller-chosen idempotency key; else one is generated for every `POST`. K2 passes its
    /// own for durable work (`k2-<purpose>-<outbox row id>`, unique per agent).
    pub idempotency_key: Option<IdempotencyKey>,
    /// BFFs only: sets `X-Forwarded-For` (contracts rev 9).
    pub forwarded_for: Option<IpAddr>,
    /// Overrides the client's request timeout.
    pub timeout: Option<Duration>,
    /// One attempt only.
    pub no_retry: bool,
}

impl RequestOptions {
    /// Defaults: generated key, the client's timeout, retries on.
    pub fn new() -> Self {
        RequestOptions::default()
    }

    /// Sends this idempotency key (reused on every retry of the call).
    pub fn idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    /// Sets `X-Forwarded-For`.
    pub fn forwarded_for(mut self, ip: IpAddr) -> Self {
        self.forwarded_for = Some(ip);
        self
    }

    /// Overrides the request timeout.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    /// Disables retries.
    pub fn no_retry(mut self) -> Self {
        self.no_retry = true;
        self
    }
}

/// Builds a [`Client`].
pub struct ClientBuilder {
    pub(crate) base: String,
    pub(crate) token: Option<SecretString>,
    pub(crate) pass_source: Option<Arc<dyn PassSource>>,
    pub(crate) retry: RetryPolicy,
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Duration,
    pub(crate) user_agent: String,
    pub(crate) clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("pass_source", &self.pass_source.is_some())
            .field("retry", &self.retry)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

/// `sew-client/<version>`.
pub(crate) fn default_user_agent() -> String {
    format!("sew-client/{}", env!("CARGO_PKG_VERSION"))
}

impl ClientBuilder {
    pub(crate) fn new(base_url: &str) -> ClientBuilder {
        ClientBuilder {
            base: base_url.to_owned(),
            token: None,
            pass_source: None,
            retry: RetryPolicy::default(),
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            user_agent: default_user_agent(),
            clock: Arc::new(SystemClock),
        }
    }

    /// A session token.
    pub fn token(mut self, t: impl Into<SecretString>) -> Self {
        self.token = Some(t.into());
        self
    }

    /// A K2 pass source for login, renew and re-login (S25 §11).
    pub fn pass_source(mut self, p: Arc<dyn PassSource>) -> Self {
        self.pass_source = Some(p);
        self
    }

    /// The retry policy (default §8.1).
    pub fn retry(mut self, r: RetryPolicy) -> Self {
        self.retry = r;
        self
    }

    /// Connect timeout (default 5 s).
    pub fn connect_timeout(mut self, d: Duration) -> Self {
        self.connect_timeout = d;
        self
    }

    /// Request timeout (default 30 s).
    pub fn request_timeout(mut self, d: Duration) -> Self {
        self.request_timeout = d;
        self
    }

    /// `User-Agent: <product> sew-client/<version>`.
    pub fn user_agent(mut self, product: &str) -> Self {
        self.user_agent = format!("{product} {}", default_user_agent());
        self
    }

    /// The time source for session expiry (default the system clock; tests pass the kit's).
    pub fn clock(mut self, c: Arc<dyn Clock>) -> Self {
        self.clock = c;
        self
    }

    /// The client.
    pub fn build(self) -> Result<Client, ClientError> {
        Client::from_builder(self)
    }
}
