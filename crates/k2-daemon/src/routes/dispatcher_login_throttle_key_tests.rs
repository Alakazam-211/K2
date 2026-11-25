//! `login_throttle_key`'s tests: a child of `routes::dispatcher` (moved out
//! unchanged for the line ratchet).

use super::*;
use std::net::IpAddr;

fn ip(s: &str) -> IpAddr {
    s.parse().expect("test ip")
}

const LO: &str = "127.0.0.1";

#[test]
fn tunnel_skin_ignores_every_forwarded_header() {
    for headers in [
        "CF-Connecting-IP: 203.0.113.1\r\n",
        "X-Forwarded-For: 203.0.113.2\r\n",
        "X-Forwarded-For: 203.0.113.3, 198.51.100.4\r\nCF-Connecting-IP: 203.0.113.5\r\n",
        "",
    ] {
        assert_eq!(
            skin_login_throttle_key(Ingress::Tunnel, Some(ip(LO)), headers, &[], None),
            ThrottleKey::Uncapped,
            "tunnel skin login must not key on {headers:?}"
        );
    }
}

#[test]
fn tunnel_connect_uses_attested_ip_not_headers() {
    let h = "X-Forwarded-For: 203.0.113.9\r\nCF-Connecting-IP: 203.0.113.8\r\n";
    assert_eq!(
        login_throttle_key(Ingress::Tunnel, Some("198.51.100.20"), Some(ip(LO)), h, &[], None),
        ThrottleKey::Client("198.51.100.20".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Tunnel, Some("-"), Some(ip(LO)), h, &[], None),
        ThrottleKey::TunnelShared,
        "attestation without an IP falls to the shared tunnel bucket"
    );
    assert_eq!(
        login_throttle_key(Ingress::Tunnel, None, Some(ip(LO)), h, &[], None),
        ThrottleKey::TunnelShared,
        "unattested (any-mode) tunnel login never keys on 127.0.0.1 or a header"
    );
}

/// 0.46.3 — the address the relay saw (frpc's PROXY v2 header, via
/// the E2E listener's hop) keys tunnel logins and vouches for them; an
/// edge attestation still wins; headers never do.
#[test]
fn tunnel_relay_ip_keys_and_vouches() {
    let h = "X-Forwarded-For: 203.0.113.9\r\n";
    let relay = Some(ip("198.51.100.77"));
    let lo = Some(ip(LO));
    assert_eq!(
        login_throttle_key(Ingress::Tunnel, None, lo, h, &[], relay),
        ThrottleKey::Client("198.51.100.77".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Tunnel, Some("198.51.100.20"), lo, h, &[], relay),
        ThrottleKey::Client("198.51.100.20".into()),
        "an edge attestation wins over the relay's address"
    );
    assert_eq!(
        skin_login_throttle_key(Ingress::Tunnel, lo, h, &[], relay),
        ThrottleKey::Client("198.51.100.77".into())
    );
    assert_eq!(login_client_trust(Ingress::Tunnel, None, lo, h, &[], relay), ClientTrust::Vouched);
    assert_eq!(skin_login_client_trust(Ingress::Tunnel, lo, h, &[], relay), ClientTrust::Vouched);
    // Never outside the tunnel (the dispatcher only looks it up there,
    // and these keep their own rules even if handed one).
    assert_eq!(login_throttle_key(Ingress::Loopback, None, lo, h, &[], relay), ThrottleKey::Uncapped);
    assert_eq!(
        skin_login_throttle_key(Ingress::App, lo, h, &[], relay),
        ThrottleKey::Uncapped,
        "an App link login keeps its own buckets"
    );
    assert_eq!(skin_login_client_trust(Ingress::App, lo, h, &[], relay), ClientTrust::Anonymous);
}

/// 0.46.6 (App guest login lockout, fix 1a): a login through an App's
/// link never counts toward, or is refused by, the box-wide ceiling of
/// anonymous app logins; it has its App's own. An App-ingress login
/// with no link bound and an unattested tunnel login still do.
#[test]
fn a_link_login_is_never_under_the_box_wide_app_ceiling() {
    let lo = Some(ip(LO));
    let app = skin_login_client_trust(Ingress::App, lo, "", &[], None);
    assert_eq!(app, ClientTrust::Anonymous, "the link carries no client address");
    assert!(!skin_login_under_box_ceiling(app, true), "on a link: the App's own ceiling");
    assert!(skin_login_under_box_ceiling(app, false), "no link bound: the box's");
    let tunnel = skin_login_client_trust(Ingress::Tunnel, lo, "", &[], None);
    assert!(skin_login_under_box_ceiling(tunnel, false), "an unattested tunnel login");
    let local = skin_login_client_trust(Ingress::Loopback, lo, "", &[], None);
    assert!(!skin_login_under_box_ceiling(local, false), "a local login never is");
}

#[test]
fn loopback_skin_uses_rightmost_xff_only() {
    assert_eq!(
        skin_login_throttle_key(
            Ingress::Loopback,
            Some(ip(LO)),
            "X-Forwarded-For: 203.0.113.50, 198.51.100.7\r\n",
            &[], None
        ),
        ThrottleKey::Client("198.51.100.7".into())
    );
    assert_eq!(
        skin_login_throttle_key(
            Ingress::Loopback,
            Some(ip(LO)),
            "X-Forwarded-For: 203.0.113.50\r\nX-Forwarded-For: 198.51.100.8\r\n",
            &[], None
        ),
        ThrottleKey::Client("198.51.100.8".into()),
        "last header line wins"
    );
    assert_eq!(
        skin_login_throttle_key(
            Ingress::Loopback,
            Some(ip(LO)),
            "CF-Connecting-IP: 203.0.113.51\r\n",
            &[], None
        ),
        ThrottleKey::Uncapped,
        "CF-Connecting-IP is never trusted"
    );
    assert_eq!(
        skin_login_throttle_key(Ingress::Loopback, Some(ip(LO)), "X-Forwarded-For: junk\r\n", &[], None),
        ThrottleKey::Uncapped
    );
    assert_eq!(
        skin_login_throttle_key(Ingress::Loopback, Some(ip(LO)), "X-Forwarded-For: 127.0.0.1\r\n", &[], None),
        ThrottleKey::Uncapped,
        "a proxy that only saw the splice does not know the client"
    );
    assert_eq!(
        skin_login_throttle_key(Ingress::Loopback, Some(ip(LO)), "", &[], None),
        ThrottleKey::Uncapped
    );
}

#[test]
fn lan_uses_peer_unless_peer_is_a_trusted_proxy() {
    let h = "X-Forwarded-For: 203.0.113.60\r\nCF-Connecting-IP: 203.0.113.61\r\n";
    let peer = Some(ip("192.168.1.40"));
    assert_eq!(
        skin_login_throttle_key(Ingress::Lan, peer, h, &[], None),
        ThrottleKey::Client("192.168.1.40".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Lan, None, peer, h, &[], None),
        ThrottleKey::Client("192.168.1.40".into())
    );
    let trusted = [ip("192.168.1.40")];
    assert_eq!(
        skin_login_throttle_key(Ingress::Lan, peer, h, &trusted, None),
        ThrottleKey::Client("203.0.113.60".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Lan, None, peer, h, &trusted, None),
        ThrottleKey::Client("203.0.113.60".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Lan, None, peer, "", &trusted, None),
        ThrottleKey::Client("192.168.1.40".into()),
        "trusted proxy with no header keys on the proxy itself"
    );
}

/// LM4 — each proxy header turns a loopback connection into `Lan`
/// (header names case-insensitive); without one nothing changes; a
/// body line never counts; `Lan`/`Tunnel` are never changed.
#[test]
fn proxied_loopback_is_reclassified_as_lan() {
    let head = |h: &str| format!("POST /cli/auth/login HTTP/1.1\r\nHost: x\r\n{h}\r\n");
    for h in [
        "X-Forwarded-For: 203.0.113.1\r\n",
        "x-forwarded-for: 203.0.113.1\r\n",
        "Forwarded: for=203.0.113.1\r\n",
        "X-Real-IP: 203.0.113.1\r\n",
        "Tailscale-User-Login: someone@example.com\r\n",
        "tailscale-user-name: Someone\r\n",
    ] {
        assert_eq!(
            Ingress::Loopback.reclassify_proxied(&head(h)),
            Ingress::Lan,
            "{h:?} must reclassify"
        );
    }
    for h in ["", "X-Forwarded-Proto: https\r\n", "CF-Connecting-IP: 203.0.113.1\r\n"] {
        assert_eq!(
            Ingress::Loopback.reclassify_proxied(&head(h)),
            Ingress::Loopback,
            "{h:?} must not reclassify"
        );
    }
    let body_only = "POST /cli/auth/login HTTP/1.1\r\nHost: x\r\n\r\nX-Forwarded-For: 1.2.3.4\r\n";
    assert_eq!(Ingress::Loopback.reclassify_proxied(body_only), Ingress::Loopback);
    let xff = head("X-Forwarded-For: 203.0.113.1\r\n");
    assert_eq!(Ingress::Lan.reclassify_proxied(&xff), Ingress::Lan);
    assert_eq!(Ingress::Tunnel.reclassify_proxied(&xff), Ingress::Tunnel);
}

/// LM4 — on a reclassified local proxy the bucket is the right-most
/// hop for both doors; with no usable hop it is the peer.
#[test]
fn reclassified_local_proxy_keys_on_rightmost_hop() {
    let h = "X-Forwarded-For: 203.0.113.80, 198.51.100.81\r\nCF-Connecting-IP: 203.0.113.82\r\n";
    assert_eq!(
        login_throttle_key(Ingress::Lan, None, Some(ip(LO)), h, &[], None),
        ThrottleKey::Client("198.51.100.81".into())
    );
    assert_eq!(
        skin_login_throttle_key(Ingress::Lan, Some(ip(LO)), h, &[], None),
        ThrottleKey::Client("198.51.100.81".into())
    );
    assert_eq!(
        login_throttle_key(Ingress::Lan, None, Some(ip(LO)), "X-Real-IP: 203.0.113.83\r\n", &[], None),
        ThrottleKey::Client(LO.into()),
        "no X-Forwarded-For: the proxy's own bucket"
    );
}

#[test]
fn loopback_connect_stays_uncapped() {
    assert_eq!(
        login_throttle_key(
            Ingress::Loopback,
            None,
            Some(ip(LO)),
            "X-Forwarded-For: 203.0.113.70\r\n",
            &[], None
        ),
        ThrottleKey::Uncapped
    );
}

#[test]
fn hop_parsing_handles_ports_brackets_and_mapped_v6() {
    assert_eq!(parse_hop_ip(" 198.51.100.1:443 "), Some(ip("198.51.100.1")));
    assert_eq!(parse_hop_ip("[2001:db8::1]:443"), Some(ip("2001:db8::1")));
    assert_eq!(parse_hop_ip("[2001:db8::2]"), Some(ip("2001:db8::2")));
    assert_eq!(parse_hop_ip("\"2001:db8::3\""), Some(ip("2001:db8::3")));
    assert_eq!(parse_hop_ip("::ffff:198.51.100.2"), Some(ip("198.51.100.2")));
    assert_eq!(parse_hop_ip("unknown"), None);
    assert_eq!(parse_hop_ip(""), None);
}

/// LM4 — only logins with no trustworthy client address fall under the
/// global failed-login ceiling.
#[test]
fn global_ceiling_applies_only_to_anonymous_logins() {
    let xff = "X-Forwarded-For: 203.0.113.90\r\n";
    let lan_peer = Some(ip("192.168.1.40"));
    let lo = Some(ip(LO));

    // Connect.
    assert_eq!(login_client_trust(Ingress::Loopback, None, lo, "", &[], None), ClientTrust::Local);
    assert_eq!(
        login_client_trust(Ingress::Tunnel, Some("198.51.100.20"), lo, xff, &[], None),
        ClientTrust::Vouched,
        "an edge-attested login is vouched for by the signed ip="
    );
    assert_eq!(
        login_client_trust(Ingress::Tunnel, Some("-"), lo, xff, &[], None),
        ClientTrust::Anonymous,
        "an attestation with no usable ip names no client"
    );
    assert_eq!(
        login_client_trust(Ingress::Tunnel, None, lo, xff, &[], None),
        ClientTrust::Anonymous,
        "an unattested tunnel login is anonymous whatever its headers say"
    );
    assert_eq!(login_client_trust(Ingress::Lan, None, lan_peer, xff, &[], None), ClientTrust::Vouched);
    assert_eq!(login_client_trust(Ingress::Lan, None, None, "", &[], None), ClientTrust::Anonymous);
    assert_eq!(
        login_client_trust(Ingress::Lan, None, lo, xff, &[], None),
        ClientTrust::Anonymous,
        "a local proxy that is not a configured trusted proxy"
    );
    assert_eq!(
        login_client_trust(Ingress::Lan, None, lo, xff, &[ip(LO)], None),
        ClientTrust::Vouched,
        "a configured trusted local proxy that names the client"
    );
    assert_eq!(
        login_client_trust(Ingress::Lan, None, lo, "X-Forwarded-For: 127.0.0.1\r\n", &[ip(LO)], None),
        ClientTrust::Anonymous,
        "a trusted proxy that only saw loopback does not know the client"
    );
    assert_eq!(
        login_client_trust(Ingress::Lan, None, lo, "X-Real-IP: 203.0.113.9\r\n", &[ip(LO)], None),
        ClientTrust::Anonymous,
        "no X-Forwarded-For: the proxy names no client"
    );

    // App (skin).
    assert_eq!(skin_login_client_trust(Ingress::Loopback, lo, "", &[], None), ClientTrust::Local);
    assert_eq!(
        skin_login_client_trust(Ingress::Tunnel, lo, xff, &[], None),
        ClientTrust::Anonymous,
        "the edge never attests app logins"
    );
    assert_eq!(skin_login_client_trust(Ingress::Lan, lan_peer, xff, &[], None), ClientTrust::Vouched);
    assert_eq!(skin_login_client_trust(Ingress::Lan, lo, xff, &[], None), ClientTrust::Anonymous);

    assert!(ClientTrust::Anonymous.under_global_ceiling());
    assert!(!ClientTrust::Vouched.under_global_ceiling());
    assert!(!ClientTrust::Local.under_global_ceiling());
}
