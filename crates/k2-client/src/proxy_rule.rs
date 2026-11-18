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

    #[test]
    fn loopback_and_plain_http_skip_the_proxy() {
        for url in [
            "http://127.0.0.1:4646/cli/x",
            "http://127.9.9.9:1",
            "http://[::1]:4646/",
            "http://localhost:4646",
            "https://127.0.0.1:4646/",
            "https://[::1]/",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://app.localhost/",
            "http://k2.example.com/cli/x",
            "ws://127.0.0.1:4646/events",
            "not a url",
        ] {
            assert!(direct(url), "{url} must skip the proxy");
        }
    }

    #[test]
    fn https_to_a_remote_host_may_use_the_proxy() {
        for url in ["https://k2.example.com/cli/x", "https://acme.k2.dev", "https://10.0.0.5:4646/", "https://[2001:db8::1]/"] {
            assert!(!direct(url), "{url} may use the person's proxy");
        }
    }
}
