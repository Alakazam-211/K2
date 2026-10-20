//! Transports: HTTP over TCP (reqwest, rustls) and HTTP over the mgmt Unix socket (hyper)
//! (S25 §6.2). One attempt at a time; the retry loop lives in `api`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::Method;
use sew_proto::id::Id;
use sew_proto::mgmt::{ActAsTarget, SewVia};

use crate::error::TransportError;
use crate::secret::SecretString;
use crate::session::TokenCell;

pub(crate) mod tcp;
#[cfg(all(feature = "mgmt", unix))]
pub(crate) mod uds;

/// Where requests go.
#[derive(Debug)]
pub(crate) enum Transport {
    /// `http(s)://host[:port][/prefix]`; `base` has no trailing `/`.
    Tcp { base: String, http: reqwest::Client },
    /// The mgmt socket.
    #[cfg(all(feature = "mgmt", unix))]
    Uds { socket: std::path::PathBuf },
}

/// How requests authenticate.
#[derive(Debug, Clone)]
pub(crate) enum Auth {
    /// A session token cell (possibly empty).
    Session(Arc<TokenCell>),
    /// The mgmt service token, optionally acting as an agent.
    #[cfg_attr(not(all(feature = "mgmt", unix)), allow(dead_code))]
    Service {
        token: SecretString,
        act_as: Option<(ActAsTarget, SewVia)>,
        /// Filled after the first successful `me()` under act-as.
        member: Arc<Mutex<Option<Id>>>,
    },
}

/// One attempt.
#[derive(Debug)]
pub(crate) struct Wire {
    pub method: Method,
    /// Path and query, starting with `/`.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
    pub timeout: Duration,
}

/// What came back.
#[derive(Debug)]
pub(crate) struct WireResp {
    pub status: u16,
    /// Names lowercase, in order.
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Transport {
    pub(crate) async fn send(&self, w: &Wire) -> Result<WireResp, TransportError> {
        match self {
            Transport::Tcp { base, http } => tcp::send(http, base, w).await,
            #[cfg(all(feature = "mgmt", unix))]
            Transport::Uds { socket } => uds::send(socket, w).await,
        }
    }

    /// The base URL (TCP) or `unix:<path>`.
    pub(crate) fn describe(&self) -> String {
        match self {
            Transport::Tcp { base, .. } => base.clone(),
            #[cfg(all(feature = "mgmt", unix))]
            Transport::Uds { socket } => format!("unix:{}", socket.display()),
        }
    }
}
