# Provenance

How this crate was written, recorded so its licensing can be checked.

## Sources used

- **curl's documentation:** the `curl(1)` manual (the ENVIRONMENT
  section on `http_proxy`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`; the
  `--proxy`, `--noproxy`, `--socks4`, `--socks4a`, `--socks5`,
  `--socks5-hostname` and `--proxy-user` options; EXIT CODES) and
  libcurl's option pages for `CURLOPT_PROXY`, `CURLOPT_NOPROXY` and
  `CURLOPT_PROXYTYPE`. These say what curl does; they hold no code.
- **The protocols' own specifications:** RFC 1928 (SOCKS5), RFC 1929
  (SOCKS5 user and password), the SOCKS4 protocol description and the
  SOCKS4a extension, RFC 9110 (HTTP CONNECT), RFC 3986 (URI syntax) and
  RFC 4648 (base64).
- **Black-box tests of the curl command.** `tests/curl_oracle.rs` runs
  the curl binary (8.5.0) as a separate process against stub proxies
  and compares what it does with what this crate does. Where the
  documentation is silent, the tests decided.
- **K2's own earlier code.** The `no_proxy` matching and the CONNECT head
  started from the k5 command's transport (`crates/k5-cli`, MIT, Alakazam
  Labs), which was itself written from the same black-box testing of curl.

## What the black-box table decided

Where the documentation is silent, curl 8.5.0's behaviour in
`tests/curl_oracle.rs` decided (observed, not read from source):

- a second `@` in a proxy URL is refused (exit 5); port 0 is read, and
  the connect fails (exit 7); a percent-encoded host is decoded;
- `socks://` is SOCKS4; SOCKS4 and SOCKS5 resolve the target after the
  proxy is connected (SOCKS5 after the greeting), and a name that doesn't
  resolve fails the handshake (exit 97); SOCKS4a sends an address as
  text too;
- `no_proxy`: an IPv6 token matches only the same address written
  without brackets; no IPv6 range matched;
- the user part: a decoded byte below 0x20 (`%0a`, `%00`) makes curl drop
  the proxy and connect direct, which this crate refuses instead (the
  one kept difference, pinned by the table); a decoded DEL (`%7f`) is
  sent as it is, as curl sends it;
- an `https://` proxy's certificate is checked with the caller's config;
  curl checks it with its built-in roots, never `SSL_CERT_FILE` (the
  curl manual names that variable under `--cacert` only).

## What was not used

curl's source code was not opened, read or copied while writing this
crate or its tests: its licence is not on our list of allowed licences.
No other proxy library's code was used either.

The code was written with an AI model (Claude). Its training data may
include public source code, curl's among it; it was asked to work only
from the documents above and the black-box results, and the tests, not
recall, are what settle each behaviour.
