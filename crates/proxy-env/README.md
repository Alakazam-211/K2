# proxy-env

Picks the proxy for a request from the environment (`http_proxy`,
`https_proxy`, `all_proxy`, `no_proxy` and their upper-case forms),
following curl's proxy environment rules, and opens a tunnel through it:
HTTP CONNECT, an `https://` proxy (TLS to the proxy itself), SOCKS4,
SOCKS4a, SOCKS5 and SOCKS5h with a user and password. A program that used
to shell out to curl can keep its users' proxy settings working the same
way without carrying curl. It is not affiliated with the curl project.

Its behaviour is checked against curl 8.5.0, run as a separate process on
the same settings (`tests/curl_oracle.rs`): the proxy picked, the bytes
sent to it and the exit code curl gives when it fails. The table matched
curl 8.5.0 on all 122 of its rows.

## Example

```rust,no_run
use proxy_env::{connect, process_env, proxy_for, Options};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The proxy curl would use for https://example.com, if any.
    let proxy = proxy_for("https", "example.com", process_env)?;
    let opts = Options::new()
        .user_agent("my-tool/1.0")
        .timeout(Duration::from_secs(30));
    // Through the tunnel (or direct, with no proxy): speak TLS to
    // example.com on this stream.
    let stream = connect(proxy.as_ref(), "example.com", 443, &opts)?;
    let _ = stream;
    Ok(())
}
```

An `https://` proxy needs a TLS config for the proxy's certificate:
`Options::proxy_tls(Arc<rustls::ClientConfig>)`. The crate compiles in no
crypto provider; the config brings its own (ring or aws-lc-rs). The
certificate is always checked against the config's roots, for the
proxy's own name: there is no option to turn the check off.

## What it reads (curl's rules)

- **Which variable:** `<scheme>_proxy`, then `<SCHEME>_PROXY`, then
  `all_proxy`, then `ALL_PROXY`; the lower-case one wins. `HTTP_PROXY` is
  never read for `http`. An empty value counts as unset.
- **`no_proxy`, else `NO_PROXY`:** exactly `*` matches every host.
  Otherwise tokens separated by commas or blanks: a name matches itself or
  any name under it at a dot, in any case, with a leading or trailing dot
  ignored; an address matches itself or a CIDR range (`10.0.0.0/8`,
  `2001:db8::/32`). A listed host goes direct.
- **The proxy URL:** `[scheme://][user[:password]@]host[:port]`. Schemes
  `http` (also no scheme), `https`, `socks4`, `socks4a`, `socks5`,
  `socks5h`; anything else fails, as it does in curl. The port defaults
  to 443 for `https` and 1080 for the rest. The user and password are
  percent-decoded. IPv6 hosts go in brackets.
- **A proxy that fails is an error, never a direct connection.** Each
  error carries curl's exit code for the same failure
  (`Error::curl_exit_code`).

Where curl versions differ, the crate does what curl 8.5.0 does.

## The one deliberate difference from curl

A proxy URL whose user or password holds a control byte once
percent-decoded (`%0a`, `%00`, any byte below 0x20): curl 8.5.0 drops that
proxy and connects direct. This crate refuses instead
(`ConfigError::ControlInCredentials`, curl's exit 5), because a proxy that
is set must never be skipped. The table checks both sides of it.

## Features

| Feature | Default | What it turns on |
|---|---|---|
| `rustls` | yes | `https://` proxies (TLS to the proxy) with the caller's `rustls::ClientConfig`. Without it, an `https://` proxy is `Error::TlsNotConfigured`. |

## Platforms and MSRV

Rust 1.80 or later. Pure Rust on `std`, no `unsafe` (`#![forbid(unsafe_code)]`).
Tested on Linux (x86-64) and macOS. It uses only `std::net`, so it builds
anywhere `std` does.

## Licence

MIT OR Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)). How it was written:
[PROVENANCE.md](PROVENANCE.md).

## Security

Please report vulnerabilities privately, never in a public issue: see
[SECURITY.md](https://github.com/Alakazam-211/K2/blob/main/SECURITY.md)
(GitHub's private vulnerability reporting on the Security tab of
[Alakazam-211/K2](https://github.com/Alakazam-211/K2/security)).

## Issues

A bug, a setting read differently from curl, or a question:
[open an issue](https://github.com/Alakazam-211/K2/issues/new?labels=proxy-env&template=proxy-env.yml)
with the variables you set (with any password removed), the curl version
you compared with, and what each did.
