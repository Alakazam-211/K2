# Changelog

## 0.1.0 (unreleased)

The first release.

- MSRV: Rust 1.85 for the library, on both feature sets (rustls's
  `zeroize` needs Cargo's edition 2024; checked with
  `cargo +1.85 check --locked --lib`). The tests need 1.88, for the
  dev-dependency `rcgen` and its `time`; `time` is not in the library's
  dependency tree.

- `proxy_for`: the proxy curl would use for a URL's scheme and host,
  from the environment (`http_proxy`, `https_proxy`, `all_proxy`,
  `no_proxy` and their upper-case forms), or a `ConfigError` for a
  setting curl refuses.
- `ProxyUrl::parse` and `NoProxy::parse`: the proxy URL and the
  `no_proxy` list on their own, as bytes.
- `connect`: the connection through the proxy (HTTP CONNECT, an
  `https://` proxy with the caller's rustls config, SOCKS4, SOCKS4a,
  SOCKS5 and SOCKS5h with RFC 1929 user and password), or direct when
  there is none.
- Every `Error` names curl's exit code for the same failure.
- An `https://` proxy's certificate is always checked against the
  caller's roots, for the proxy's own name; there is no option to turn
  the check off.
- One deliberate difference from curl: a user or password with a control
  byte once decoded is refused (`ConfigError::ControlInCredentials`) where
  curl connects direct.
- Checked against curl 8.5.0 (`tests/curl_oracle.rs`, 122 rows, all
  matching), and fuzzed: the variables, and the CONNECT answer and SOCKS4
  and SOCKS5 replies (`fuzz/`).
