//! Per-peer dial URL (id vs route) + air-gap / LAN policy.
//!
//! F2: fingerprint is identity; the **base URL** (scheme+host+port) is the
//! dial target. If a hint is already `http(s)://…`, dial it. Never append
//! `.k2.dev` when air-gap is on.
//! F6: LAN v1 is HTTP; HTTPS to RFC1918 is a teaching error.
//! F7: air-gap may dial RFC1918 / Tailscale / explicit `http://`; refuse
//! `*.k2.dev` / Connect. No SYN on a refused URL.

/// Teaching copy when air-gap would hit the Connect zone.
pub const AIRGAP_CONNECT_REFUSE: &str =
    "Air-gap is on (K2_AIRGAP=1). This daemon will not dial *.k2.dev or K2 Connect.";

/// Teaching copy for HTTPS to a private LAN address (F6).
pub const LAN_HTTPS_REFUSE: &str =
    "LAN federation uses HTTP (air-gap D2). HTTPS to a private RFC1918 address is not supported in v1.";

/// Trim trailing slashes from a base URL.
pub fn trim_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// True iff `s` is already an absolute HTTP(S) URL.
pub fn is_absolute_http_url(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 8
        && (t[..7].eq_ignore_ascii_case("http://") || t[..8].eq_ignore_ascii_case("https://"))
}

/// Host[:port] of a base URL or bare host hint (no scheme/path).
pub fn host_port(url_or_host: &str) -> String {
    let s = url_or_host.trim().trim_end_matches('/');
    let lower = s.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        &s[s.len() - r.len()..]
    } else if let Some(r) = lower.strip_prefix("http://") {
        &s[s.len() - r.len()..]
    } else {
        s
    };
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// Hostname without port (IPv6 out of v1).
pub fn hostname_only(url_or_host: &str) -> String {
    let hp = host_port(url_or_host);
    match hp.rfind(':') {
        Some(i) if hp[..i].chars().all(|c| c.is_ascii_digit() || c == '.') => hp[..i].to_string(),
        _ => hp,
    }
}

fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let h = hostname_only(host);
    let mut out = [0u8; 4];
    let mut n = 0usize;
    for part in h.split('.') {
        if n == 4 {
            return None;
        }
        if part.is_empty() || part.len() > 3 || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let v: u8 = part.parse().ok()?;
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        out[n] = v;
        n += 1;
    }
    if n == 4 {
        Some(out)
    } else {
        None
    }
}

/// RFC1918: 10/8, 172.16/12, 192.168/16.
pub fn is_rfc1918_host(host: &str) -> bool {
    match parse_ipv4(host) {
        Some([10, _, _, _]) => true,
        Some([172, b, _, _]) if (16..=31).contains(&b) => true,
        Some([192, 168, _, _]) => true,
        _ => false,
    }
}

/// Tailscale CGNAT 100.64/10.
pub fn is_tailscale_ip_host(host: &str) -> bool {
    match parse_ipv4(host) {
        Some([100, b, _, _]) if (64..128).contains(&b) => true,
        _ => false,
    }
}

pub fn is_loopback_host(host: &str) -> bool {
    match parse_ipv4(host) {
        Some([127, _, _, _]) => true,
        _ => {
            let h = host_port(host).to_ascii_lowercase();
            h == "localhost" || h.starts_with("localhost:")
        }
    }
}

pub fn is_ts_net_host(host: &str) -> bool {
    let h = hostname_only(host).to_ascii_lowercase();
    h == "ts.net" || h.ends_with(".ts.net")
}

/// `k2.dev` or `*.k2.dev` (Connect zone).
pub fn is_k2_dev_host(host: &str) -> bool {
    let h = hostname_only(host).to_ascii_lowercase();
    h == "k2.dev" || h.ends_with(".k2.dev")
}

/// Bare Connect routing label: no dot, no colon, not a URL (e.g. `"rpm"`).
pub fn is_connect_subdomain_label(hint: &str) -> bool {
    let h = hint.trim();
    if h.is_empty() || is_absolute_http_url(h) {
        return false;
    }
    !h.contains('.') && !h.contains(':') && !h.contains('/')
}

fn scheme_of(url: &str) -> Option<&'static str> {
    let t = url.trim();
    if t.len() >= 8 && t[..8].eq_ignore_ascii_case("https://") {
        Some("https")
    } else if t.len() >= 7 && t[..7].eq_ignore_ascii_case("http://") {
        Some("http")
    } else {
        None
    }
}

/// Normalize a pair `--url` / `baseUrl`. Rejects empty, missing host, or
/// non-http(s) schemes.
pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let s = trim_base(raw);
    if s.is_empty() {
        return Err("empty federation base URL".into());
    }
    if !is_absolute_http_url(&s) {
        return Err(format!(
            "federation URL must be http(s)://host[:port] (got {raw:?})"
        ));
    }
    let host = host_port(&s);
    if host.is_empty() {
        return Err("federation URL is missing a host".into());
    }
    Ok(s)
}

/// F6: HTTPS to RFC1918 is a teaching error (LAN v1 is HTTP).
pub fn assert_lan_http(url: &str) -> Result<(), String> {
    if scheme_of(url) == Some("https") && is_rfc1918_host(url) {
        return Err(LAN_HTTPS_REFUSE.into());
    }
    Ok(())
}

/// Whether air-gap is allowed to dial `url` (already a base URL).
///
/// Allow: RFC1918, Tailscale (CGNAT / `*.ts.net`), explicit `http://` that
/// is not the Connect zone, loopback HTTP (tests / local Caddy).
/// Refuse: `*.k2.dev` / Connect. HTTPS+RFC1918 is refused even off air-gap.
pub fn assert_may_dial(url: &str) -> Result<(), String> {
    let url = trim_base(url);
    if url.is_empty() {
        return Err(if crate::airgap::enabled() {
            AIRGAP_CONNECT_REFUSE.to_string()
        } else {
            "federation peer has no dial URL".into()
        });
    }
    assert_lan_http(&url)?;
    if !crate::airgap::enabled() {
        return Ok(());
    }
    if is_k2_dev_host(&url) {
        return Err(AIRGAP_CONNECT_REFUSE.into());
    }
    let http = scheme_of(&url) == Some("http");
    let host_ok = is_rfc1918_host(&url)
        || is_tailscale_ip_host(&url)
        || is_ts_net_host(&url)
        || is_loopback_host(&url);
    if http || host_ok {
        return Ok(());
    }
    Err(AIRGAP_CONNECT_REFUSE.into())
}

/// Resolve the dial base URL from a peer row (no env override).
///
/// Prefer `base_url`. If the subdomain hint is already `http(s)://…`, use
/// it. Air-gap never concatenates `.k2.dev`; Connect labels stay unroutable
/// until an explicit LAN/Tailscale URL is stored.
pub fn resolve_peer_base_url(subdomain: &str, base_url: &str) -> String {
    let stored = base_url.trim();
    if !stored.is_empty() {
        return trim_base(stored);
    }
    let hint = subdomain.trim();
    if is_absolute_http_url(hint) {
        return trim_base(hint);
    }
    if crate::airgap::enabled() {
        if hint.is_empty() || is_connect_subdomain_label(hint) || is_k2_dev_host(hint) {
            return String::new();
        }
        // Already a host:port / RFC1918 / Tailscale MagicDNS — HTTP LAN v1.
        return format!("http://{}", hint.trim_end_matches('/'));
    }
    let n = crate::connections::normalize_remote_host(hint);
    if n.ends_with(".k2.dev") {
        format!("https://{n}")
    } else if n.is_empty() {
        n
    } else {
        format!("https://{n}.k2.dev")
    }
}

/// Host string for `agent::<host>` (F5): saved host:port, not `name::lan`.
pub fn peer_address_host(subdomain: &str, base_url: &str) -> String {
    let base = resolve_peer_base_url(subdomain, base_url);
    if !base.is_empty() {
        return crate::connections::normalize_remote_host(&host_port(&base));
    }
    crate::connections::normalize_remote_host(subdomain)
}

/// Operator override for this daemon's advertised pair-back URL.
/// Distinct from `K2_FEDERATION_INBOUND_BASE` (test dial-target override).
pub const ADVERTISE_URL_ENV: &str = "K2_FEDERATION_ADVERTISE_URL";

/// Optional advertised TCP port when [`ADVERTISE_URL_ENV`] is unset.
pub const ADVERTISE_PORT_ENV: &str = "K2_FEDERATION_ADVERTISE_PORT";

/// Air-gap Caddy LAN front (daemon stays loopback).
pub const AIRGAP_ADVERTISE_PORT: u16 = 38471;

/// This daemon's LAN/Tailscale URL for pair-back. Never loopback / localhost.
/// Empty when nothing routable is known.
pub fn advertised_federation_base() -> String {
    advertised_federation_base_from(
        std::env::var(ADVERTISE_URL_ENV).ok().as_deref(),
        parse_advertise_port_env(std::env::var(ADVERTISE_PORT_ENV).ok().as_deref()),
        crate::airgap::enabled(),
        crate::listen::lan_bound(),
        daemon_http_port(),
        &local_ipv4_addrs(),
    )
}

fn parse_advertise_port_env(raw: Option<&str>) -> Option<u16> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    match s.parse::<u16>() {
        Ok(p) if p != 0 => Some(p),
        _ => None,
    }
}

fn daemon_http_port() -> Option<u16> {
    crate::port_claim::read_port_file(&crate::paths::k2_home().join("daemon.port"))
}

/// Pick among already-collected IPv4s (unit-tested with a fake iface list).
/// Prefers RFC1918, then Tailscale CGNAT, then any other non-loopback.
pub fn pick_advertise_ipv4(addrs: &[std::net::Ipv4Addr]) -> Option<std::net::Ipv4Addr> {
    fn usable(ip: std::net::Ipv4Addr) -> bool {
        !ip.is_loopback()
            && !ip.is_unspecified()
            && !ip.is_broadcast()
            && !ip.is_multicast()
            && !ip.is_link_local()
    }
    fn rank(ip: std::net::Ipv4Addr) -> u8 {
        let s = ip.to_string();
        if is_rfc1918_host(&s) {
            0
        } else if is_tailscale_ip_host(&s) {
            1
        } else {
            2
        }
    }
    addrs
        .iter()
        .copied()
        .filter(|ip| usable(*ip))
        .min_by_key(|ip| rank(*ip))
}

fn sanitize_advertise_url(raw: &str) -> Option<String> {
    let url = normalize_base_url(raw).ok()?;
    if is_loopback_host(&url) {
        return None;
    }
    if assert_lan_http(&url).is_err() {
        return None;
    }
    Some(url)
}

pub(crate) fn advertised_federation_base_from(
    env_url: Option<&str>,
    env_port: Option<u16>,
    airgap: bool,
    lan_bound: bool,
    daemon_port: Option<u16>,
    ifaces: &[std::net::Ipv4Addr],
) -> String {
    if let Some(raw) = env_url {
        if let Some(url) = sanitize_advertise_url(raw) {
            return url;
        }
    }
    let port = if let Some(p) = env_port.filter(|p| *p != 0) {
        p
    } else if airgap {
        AIRGAP_ADVERTISE_PORT
    } else if lan_bound {
        match daemon_port.filter(|p| *p != 0) {
            Some(p) => p,
            None => return String::new(),
        }
    } else {
        return String::new();
    };
    match pick_advertise_ipv4(ifaces) {
        Some(ip) => format!("http://{ip}:{port}"),
        None => String::new(),
    }
}

fn local_ipv4_addrs() -> Vec<std::net::Ipv4Addr> {
    #[cfg(unix)]
    {
        unix_ipv4_addrs()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(unix)]
fn unix_ipv4_addrs() -> Vec<std::net::Ipv4Addr> {
    use std::net::Ipv4Addr;
    let mut out = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return out;
        }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            let up = (ifa.ifa_flags as u32) & (libc::IFF_UP as u32) != 0;
            if up && !ifa.ifa_addr.is_null() {
                let family = (*ifa.ifa_addr).sa_family as u32;
                if family == libc::AF_INET as u32 {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    out.push(Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes()));
                }
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::HOME_LOCK;

    struct AirgapEnv(Option<std::ffi::OsString>);
    impl AirgapEnv {
        fn set(val: Option<&str>) -> Self {
            let prev = std::env::var_os(crate::airgap::ENV_VAR);
            match val {
                Some(v) => std::env::set_var(crate::airgap::ENV_VAR, v),
                None => std::env::remove_var(crate::airgap::ENV_VAR),
            }
            crate::airgap::set_setting_enabled(false);
            Self(prev)
        }
    }
    impl Drop for AirgapEnv {
        fn drop(&mut self) {
            match &self.0 {
                Some(p) => std::env::set_var(crate::airgap::ENV_VAR, p),
                None => std::env::remove_var(crate::airgap::ENV_VAR),
            }
            crate::airgap::set_setting_enabled(false);
        }
    }

    #[test]
    fn host_port_strips_scheme_and_path() {
        assert_eq!(
            host_port("http://192.168.1.50:38471/cli/x"),
            "192.168.1.50:38471"
        );
        assert_eq!(host_port("https://rpm.k2.dev"), "rpm.k2.dev");
        assert_eq!(host_port("box.ts.net"), "box.ts.net");
    }

    #[test]
    fn rfc1918_and_tailscale_and_zone() {
        assert!(is_rfc1918_host("192.168.1.50:38471"));
        assert!(is_rfc1918_host("10.0.0.4"));
        assert!(is_rfc1918_host("172.16.9.1"));
        assert!(!is_rfc1918_host("172.15.0.1"));
        assert!(is_tailscale_ip_host("100.64.0.1"));
        assert!(is_ts_net_host("box.ts.net"));
        assert!(is_k2_dev_host("foo.k2.dev"));
        assert!(is_k2_dev_host("https://foo.k2.dev"));
        assert!(!is_k2_dev_host("192.168.1.50:38471"));
        assert!(is_connect_subdomain_label("rpm"));
        assert!(!is_connect_subdomain_label("192.168.1.50:38471"));
        assert!(!is_connect_subdomain_label("http://x"));
        assert!(is_loopback_host("127.0.0.1:38471"));
        assert!(is_loopback_host("http://localhost:38471"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("192.168.1.40:38471"));
    }

    #[test]
    fn https_rfc1918_is_teaching_error() {
        let err = assert_lan_http("https://192.168.1.50:38471").expect_err("https lan");
        assert!(err.contains("HTTP"), "got {err}");
        assert_lan_http("http://192.168.1.50:38471").expect("http lan ok");
        assert_lan_http("https://rpm.k2.dev").expect("connect https ok");
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn resolve_connect_appends_zone_when_not_airgap() {
        let _lock = HOME_LOCK.lock();
        let _env = AirgapEnv::set(Some("0"));
        assert_eq!(resolve_peer_base_url("rpm", ""), "https://rpm.k2.dev");
        assert_eq!(
            resolve_peer_base_url("rpm.k2.dev", ""),
            "https://rpm.k2.dev"
        );
    }

    #[test]
    fn resolve_absolute_hint_is_used_as_is() {
        assert_eq!(
            resolve_peer_base_url("", "http://192.168.1.50:38471/"),
            "http://192.168.1.50:38471"
        );
        assert_eq!(
            resolve_peer_base_url("http://10.0.0.5:38471", ""),
            "http://10.0.0.5:38471"
        );
    }

    #[test]
    fn airgap_never_concatenates_k2_dev() {
        let _lock = HOME_LOCK.lock();
        let _env = AirgapEnv::set(Some("1"));
        let url = resolve_peer_base_url("rpm", "");
        assert!(
            !url.contains("k2.dev"),
            "air-gap must not append .k2.dev; got {url:?}"
        );
        assert!(
            url.is_empty(),
            "Connect label is unroutable under air-gap without a LAN URL; got {url:?}"
        );
        let lan = resolve_peer_base_url("192.168.1.50:38471", "");
        assert_eq!(lan, "http://192.168.1.50:38471");
        assert!(!lan.contains("k2.dev"));
        let stored = resolve_peer_base_url("rpm", "http://192.168.1.50:38471");
        assert_eq!(stored, "http://192.168.1.50:38471");
        assert!(!stored.contains("k2.dev"));
    }

    #[test]
    fn airgap_may_dial_lan_http_refuses_k2_dev() {
        let _lock = HOME_LOCK.lock();
        let _env = AirgapEnv::set(Some("1"));
        assert_may_dial("http://192.168.1.50:38471").expect("lan http");
        assert_may_dial("http://100.64.1.2:38471").expect("tailscale http");
        assert_may_dial("http://box.ts.net:38471").expect("ts.net");
        assert_may_dial("http://127.0.0.1:9").expect("loopback http tests");
        let err = assert_may_dial("https://foo.k2.dev").expect_err("connect");
        assert!(
            err.contains("k2.dev") || err.contains("Air-gap"),
            "got {err}"
        );
        assert_may_dial("").expect_err("empty");
    }

    #[test]
    fn peer_address_host_keeps_lan_host_port() {
        let _lock = HOME_LOCK.lock();
        let _env = AirgapEnv::set(Some("1"));
        assert_eq!(
            peer_address_host("", "http://192.168.1.50:38471"),
            "192.168.1.50:38471"
        );
        assert_ne!(peer_address_host("", "http://192.168.1.50:38471"), "lan");
    }

    #[test]
    fn pick_advertise_ipv4_skips_loopback_prefers_rfc1918() {
        use std::net::Ipv4Addr;
        let loopback = Ipv4Addr::new(127, 0, 0, 1);
        let link_local = Ipv4Addr::new(169, 254, 1, 1);
        let lan = Ipv4Addr::new(192, 168, 1, 40);
        let ts = Ipv4Addr::new(100, 64, 1, 2);
        let public = Ipv4Addr::new(8, 8, 8, 8);
        assert_eq!(pick_advertise_ipv4(&[loopback]), None);
        assert_eq!(pick_advertise_ipv4(&[loopback, link_local]), None);
        assert_eq!(pick_advertise_ipv4(&[loopback, lan]), Some(lan));
        assert_eq!(pick_advertise_ipv4(&[public, lan]), Some(lan));
        assert_eq!(pick_advertise_ipv4(&[public, ts]), Some(ts));
        assert_eq!(pick_advertise_ipv4(&[ts, lan]), Some(lan));
        assert_eq!(
            pick_advertise_ipv4(&[Ipv4Addr::new(10, 0, 0, 5), lan]),
            Some(Ipv4Addr::new(10, 0, 0, 5)),
            "first RFC1918 wins among equals"
        );
    }

    #[test]
    fn advertised_base_env_url_round_trips_skips_loopback() {
        use std::net::Ipv4Addr;
        let lan = [Ipv4Addr::new(192, 168, 1, 40)];
        assert_eq!(
            advertised_federation_base_from(
                Some("http://10.0.0.9:38471/"),
                None,
                false,
                false,
                None,
                &lan,
            ),
            "http://10.0.0.9:38471"
        );
        assert!(
            !advertised_federation_base_from(
                Some("http://127.0.0.1:38471"),
                None,
                false,
                false,
                None,
                &[],
            )
            .contains("127.0.0.1")
        );
        assert!(
            advertised_federation_base_from(
                Some("http://localhost:38471"),
                None,
                false,
                false,
                None,
                &[],
            )
            .is_empty()
        );
        assert!(
            advertised_federation_base_from(
                Some("https://192.168.1.40:38471"),
                None,
                true,
                false,
                None,
                &[],
            )
            .is_empty(),
            "HTTPS+RFC1918 must not be advertised"
        );
    }

    #[test]
    fn advertised_base_airgap_uses_caddy_port_on_rfc1918() {
        use std::net::Ipv4Addr;
        let ifaces = [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(192, 168, 1, 40),
        ];
        assert_eq!(
            advertised_federation_base_from(None, None, true, false, Some(60710), &ifaces),
            "http://192.168.1.40:38471"
        );
        assert_eq!(
            advertised_federation_base_from(None, Some(9999), true, false, Some(60710), &ifaces),
            "http://192.168.1.40:9999"
        );
    }

    #[test]
    fn advertised_base_lan_bound_uses_daemon_port_loopback_skipped() {
        use std::net::Ipv4Addr;
        let ifaces = [Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(10, 1, 2, 3)];
        assert_eq!(
            advertised_federation_base_from(None, None, false, true, Some(60710), &ifaces),
            "http://10.1.2.3:60710"
        );
        assert!(
            advertised_federation_base_from(
                None,
                None,
                false,
                false,
                Some(60710),
                &ifaces,
            )
            .is_empty(),
            "loopback-only non-airgap without LAN listen / env stays empty"
        );
        assert!(advertised_federation_base_from(
            None,
            None,
            true,
            false,
            None,
            &[Ipv4Addr::LOCALHOST],
        )
        .is_empty());
    }

    #[test]
    fn advertised_federation_base_env_round_trips_never_loopback() {
        let _lock = HOME_LOCK.lock();
        let prev_url = std::env::var_os(ADVERTISE_URL_ENV);
        let prev_port = std::env::var_os(ADVERTISE_PORT_ENV);
        std::env::set_var(ADVERTISE_URL_ENV, "http://192.168.1.40:38471");
        std::env::remove_var(ADVERTISE_PORT_ENV);
        let got = advertised_federation_base();
        std::env::set_var(ADVERTISE_URL_ENV, "http://127.0.0.1:38471");
        let skipped = advertised_federation_base();
        match prev_url {
            Some(p) => std::env::set_var(ADVERTISE_URL_ENV, p),
            None => std::env::remove_var(ADVERTISE_URL_ENV),
        }
        match prev_port {
            Some(p) => std::env::set_var(ADVERTISE_PORT_ENV, p),
            None => std::env::remove_var(ADVERTISE_PORT_ENV),
        }
        assert_eq!(got, "http://192.168.1.40:38471");
        assert!(
            !skipped.contains("127.0.0.1") && !skipped.to_ascii_lowercase().contains("localhost"),
            "must never advertise loopback; got {skipped:?}"
        );
    }
}
