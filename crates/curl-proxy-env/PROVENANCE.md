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

## What was not used

curl's source code was not opened, read or copied while writing this
crate or its tests: its licence is not on our list of allowed licences.
No other proxy library's code was used either.

The code was written with an AI model (Claude). Its training data may
include public source code, curl's among it; it was asked to work only
from the documents above and the black-box results, and the tests, not
recall, are what settle each behaviour.
