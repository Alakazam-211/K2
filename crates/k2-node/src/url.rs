//! Base URL → WebSocket URL. `https` becomes `wss` (web PKI). Plain
//! `http`/`ws` is allowed only to hosts that can't be on the open internet
//! (loopback, RFC 1918, CGNAT/tailnet 100.64/10, link-local, IPv6 ULA):
//! every frame is signed anyway, but job argv and logs would be readable.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn ws_url(base: &str, path: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    let (scheme, rest) = base.split_once("://").ok_or_else(|| format!("{base:?} has no scheme (https://…)"))?;
    let host_port = rest.split('/').next().unwrap_or("");
    if host_port.is_empty() || host_port.contains('@') {
        return Err(format!("{base:?} has no usable host"));
    }
    let ws_scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        "http" | "ws" => {
            let host = host_of(host_port);
            if !plaintext_ok(&host) {
                return Err(format!(
                    "plain {scheme}:// is only allowed on a private network (loopback, LAN, tailnet); use https:// for {host}"
                ));
            }
            "ws"
        }
        other => return Err(format!("unsupported scheme {other:?}")),
    };
    Ok(format!("{ws_scheme}://{host_port}{path}"))
}

fn host_of(host_port: &str) -> String {
    if let Some(rest) = host_port.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_string();
    }
    match host_port.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => host_port.to_string(),
    }
}

pub fn plaintext_ok(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4_private(v4),
        Ok(IpAddr::V6(v6)) => v6_private(v6),
        Err(_) => false,
    }
}

fn v4_private(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
}

fn v6_private(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    ip.is_loopback() || (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_becomes_wss() {
        assert_eq!(ws_url("https://rosson.k2.dev/", "/cli/compute/attach").unwrap(), "wss://rosson.k2.dev/cli/compute/attach");
        assert_eq!(ws_url("wss://a.b:8443", "/x").unwrap(), "wss://a.b:8443/x");
    }

    #[test]
    fn plaintext_only_on_private_hosts() {
        for ok in ["http://127.0.0.1:4000", "http://localhost:1", "ws://192.168.1.20:9", "http://10.0.0.1", "http://100.100.1.1:3", "http://[::1]:5", "http://[fd00::1]:5", "http://169.254.2.2"] {
            assert!(ws_url(ok, "/p").unwrap().starts_with("ws://"), "{ok}");
        }
        for bad in ["http://rosson.k2.dev", "http://8.8.8.8", "http://100.200.0.1", "http://[2001:db8::1]", "ftp://x", "rosson.k2.dev", "http://user@10.0.0.1"] {
            assert!(ws_url(bad, "/p").is_err(), "{bad}");
        }
    }
}
