# Changelog

## 0.1.0 (unreleased)

The first release.

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
- Checked against curl 8.5.0 (`tests/curl_oracle.rs`).
