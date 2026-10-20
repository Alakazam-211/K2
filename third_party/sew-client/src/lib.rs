//! Typed async client for Sew (S25): the client HTTP API, the mgmt socket with act-as, the
//! websocket gateway, and the sync position model. Tests, `sewc`, the `sew` admin CLI, the load
//! harness and K2 all use it.
//!
//! Builds on macOS and Linux (K2 depends on it, contracts rev 16). K2 uses
//! `default-features = false, features = ["mgmt", "sync"]`.
//!
//! ```text
//! let c = sew_client::Client::new("http://127.0.0.1:8470")?;
//! c.auth_k2(&pass).await?;              // the token is kept in the client's token cell
//! let me = c.me().await?;
//! let mgmt = sew_client::MgmtClient::from_env()?;
//! let agent = mgmt.act_as(target, via); // a normal Client over the mgmt socket
//! ```
//!
//! Rules (S25 §16): the library never creates a runtime, blocks, or spawns; every public future
//! is `Send`; it reads the environment only in `MgmtClient::from_env`; it logs only through
//! `tracing`; secrets are [`SecretString`] and never appear in `Debug` or `Display`.

pub use sew_proto as proto;

mod api;
mod calls;
mod compliance;
mod config;
mod error;
mod exports;
mod extras;
pub mod files;
mod media;
mod messaging;
mod meta;
mod notify;
mod page;
mod people;
mod read_state;
mod recording;
mod recording_audio;
mod retry;
mod roles;
mod search;
mod secret;
mod session;
mod structure;
mod transport;
mod voice;

#[cfg(feature = "gateway")]
pub mod gateway;
#[cfg(all(feature = "mgmt", unix))]
pub mod mgmt;
#[cfg(feature = "sync")]
pub mod sync;

pub use api::Client;
pub use config::{CONNECT_TIMEOUT, ClientBuilder, REQUEST_TIMEOUT, RequestOptions};
pub use error::{
    ApiFailure, ClientError, GatewayError, ResyncInfo, SNIPPET_MAX, TransportError,
    TransportErrorKind,
};
pub use meta::{RateLimitInfo, RawResponse, Response, ResponseMeta, header};
pub use page::{Direction, HasId, paginate_ids};
pub use retry::{Backoff, RetryPolicy, idempotency_key_from, new_idempotency_key, rate_limit_wait};
pub use secret::{SecretString, redact_url};
pub use session::{PassSource, Provider, RENEW_BEFORE};

#[cfg(all(feature = "mgmt", unix))]
pub use mgmt::MgmtClient;
