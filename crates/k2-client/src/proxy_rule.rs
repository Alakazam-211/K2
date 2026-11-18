//! When a request to a K2 server may use the person's proxy settings.
//!
//! Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
//! HTTPS to a remote K2 may use the person's proxy (a CONNECT tunnel hides
//! the request; a company proxy that intercepts TLS is the organisation's
//! own policy).
//!
//! One rule for the desktop app and the daemon; the `k2` command's curl
//! calls follow the same rule (`_k2_direct_url` in cli/src).

/// True when a request to `url` must skip every proxy setting: plain
/// `http://`, or any scheme to a loopback host. Only `https://` to a host
/// that is not loopback may use the person's proxy. A URL that does not
/// parse skips the proxy too (such a request fails anyway).
pub fn direct(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return true;
    };
    if u.scheme() != "https" {
        return true;
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.');
            d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost")
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::direct;

    /// The shared table (tests/fixtures/proxy_rule/table.txt): the CLI's
    /// bash and Python helpers answer the same rows the same way
    /// (tests/cli/proxy_rule_parity.sh).
    #[test]
    fn every_row_of_the_shared_table() {
        let table = include_str!("../../../tests/fixtures/proxy_rule/table.txt");
        let mut rows = 0;
        for line in table.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (want, url) = line.split_once(' ').expect("a row is \"direct|proxy <url>\"");
            let want_direct = match want {
                "direct" => true,
                "proxy" => false,
                other => panic!("bad row kind {other:?} in {line:?}"),
            };
            assert_eq!(direct(url), want_direct, "{url}: want {want}");
            rows += 1;
        }
        assert!(rows >= 19, "the table lost rows: {rows}");
    }
}
