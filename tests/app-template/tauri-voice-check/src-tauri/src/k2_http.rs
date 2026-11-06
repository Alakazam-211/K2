//! One HTTP client for this app's Rust code. `k2 app init --template tauri`
//! wrote this file; it is yours to change.
//!
//! Each reqwest client keeps its own pool of connections. A client built per
//! request pays a new TCP and TLS handshake every time, and Tauri's HTTP
//! plugin does that today. So build one and share it: `k2_http::client()`.
//!
//! Wire it once:
//!   src-tauri/src/lib.rs:  mod k2_http;
//!   src-tauri/Cargo.toml:  reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
//!
//! The settings suit an App behind K2 Connect: HTTP/1.1 only (the tunnel
//! offers no HTTP/2), an idle connection closes after 50 s (K2 closes it at
//! 60 s), and up to 6 idle connections per host for calls in parallel.
//! From your page, use the webview's own fetch() for your App's host: it
//! keeps its connections too.
#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::Duration;

/// The app's one client. Clones share its connections.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| builder().build().expect("the HTTP client builds"))
}

/// The same settings, for a client that needs more (a proxy, a timeout).
/// Build it once too.
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .http1_only()
        .pool_idle_timeout(Duration::from_secs(50))
        .pool_max_idle_per_host(6)
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(15))
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_client_for_the_whole_app() {
        assert!(std::ptr::eq(super::client(), super::client()));
    }
}
