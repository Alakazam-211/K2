#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod connect;
mod error;
mod no_proxy;
mod resolve;
mod stream;
mod url;

pub use connect::{connect, Options, DEFAULT_CONNECT_TIMEOUT};
pub use error::{exit, ConfigError, Error};
pub use no_proxy::NoProxy;
pub use resolve::{no_proxy_setting, process_env, proxy_for, proxy_setting};
pub use stream::ProxyStream;
pub use url::{Credentials, Host, ProxyUrl, Scheme};

/// The curl release this crate's behaviour is checked against
/// (`tests/curl_oracle.rs` runs that curl beside the crate).
pub const CURL_VERSION: &str = "8.5.0";
