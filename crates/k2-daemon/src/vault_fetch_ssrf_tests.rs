//! Garden v3 sources (contract §10.4): no LAN, loopback, metadata or
//! redirect escape. Every refusal is proven with a counting listener (0
//! accepts) where one can exist, and DNS is pinned per test.

use super::*;
use crate::egress::test_dns::{self, Answer};
use crate::egress::test_util::AirgapEnv;
use crate::sources::fetch::tests::{http, Fake};
use std::net::{IpAddr, Ipv4Addr};

const LOOP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn src(url: &str) -> FetchSpec {
    FetchSpec {
        method: "GET".into(),
        url: url.into(),
        headers: vec![],
        body: None,
        max_bytes: 1 << 20,
        timeout: Duration::from_secs(5),
        include_headers: false,
        purpose: Purpose::Source,
    }
}

fn go(url: &str, allow: &Allowlist) -> Result<FetchResult, VaultError> {
    execute(src(url), allow, vec![], &Redactor::empty())
}

fn for_source(origin: &str, lan: bool) -> Allowlist {
    Allowlist::for_source(&[origin.to_string()], lan).expect("allowlist")
}

#[test]
fn secretless_metadata_ip_refused() {
    let _a = AirgapEnv::set(false);
    let e = Allowlist::for_source(&["http://169.254.169.254:80".into()], false).expect_err("metadata ip");
    assert_eq!(e.code, "lan_owner_only");
    // A name pinned to the metadata address is refused before any connect.
    let _p = test_dns::pin("meta.test", vec![test_dns::plain("169.254.169.254".parse().unwrap())]);
    let e = go("https://meta.test/latest/meta-data/", &for_source("https://meta.test:443", false)).expect_err("blocked");
    assert_eq!(e.code, "address_not_allowed");
}

#[test]
fn secretless_loopback_refused() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    for o in [f.origin(), "https://127.0.0.1:443".into(), "http://[::1]:80".into(), "http://0.0.0.0:80".into()] {
        assert_eq!(Allowlist::for_source(&[o.clone()], false).expect_err(&o).code, "lan_owner_only", "{o}");
    }
    // A name that resolves to loopback (no pin: the real resolver).
    let e = go(&format!("http://localhost:{}/", f.port), &for_source(&format!("http://localhost:{}", f.port), false))
        .expect_err("localhost");
    assert_eq!(e.code, "address_not_allowed");
    // An exact-IP URL can't borrow a name's allowlist.
    let e = go(&format!("{}/", f.origin()), &for_source("https://example.com:443", false)).expect_err("not listed");
    assert_eq!(e.code, "host_not_allowed");
    assert_eq!(f.conns(), 0);
}

#[test]
fn private_ranges_refused() {
    let _a = AirgapEnv::set(false);
    for o in ["http://10.0.0.5:80", "http://172.16.0.1:80", "http://192.168.1.1:80", "http://100.64.0.1:80", "http://[fd00::1]:80", "http://[fe80::1]:80"] {
        assert_eq!(Allowlist::for_source(&[o.to_string()], false).expect_err(o).code, "lan_owner_only", "{o}");
    }
}

#[test]
fn ip_encodings_refused() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    let p = f.port;
    for u in [
        format!("http://2130706433:{p}/"),
        format!("http://0x7f000001:{p}/"),
        format!("http://017700000001:{p}/"),
        format!("http://[::ffff:127.0.0.1]:{p}/"),
    ] {
        let (_, o) = Origin::parse_url(&u).expect("parses");
        assert_eq!(o.render(), format!("http://127.0.0.1:{p}"), "{u} normalises to loopback");
        let e = go(&u, &for_source("https://example.com:443", false)).expect_err(&u);
        assert_eq!(e.code, "host_not_allowed", "{u}");
        assert_eq!(Allowlist::for_source(&[o.render()], false).expect_err(&u).code, "lan_owner_only");
    }
    assert_eq!(f.conns(), 0);
}

#[test]
fn name_resolving_private_refused() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    let _p1 = test_dns::pin("loop.test", vec![test_dns::plain(LOOP)]);
    let o = format!("http://loop.test:{}", f.port);
    assert_eq!(go(&format!("{o}/"), &for_source(&o, false)).expect_err("loopback name").code, "address_not_allowed");
    drop(_p1);
    let _p2 = test_dns::pin("lan.test", vec![test_dns::plain("10.1.2.3".parse().unwrap())]);
    let o = "https://lan.test:443".to_string();
    assert_eq!(go(&format!("{o}/"), &for_source(&o, false)).expect_err("lan name").code, "address_not_allowed");
    assert_eq!(f.conns(), 0);
}

#[test]
fn own_address_refused() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    // Past the range check (as a public address would be), still this box.
    let _p = test_dns::pin("self.test", vec![Answer { ips: vec![LOOP], skip_range: true, skip_own: false }]);
    let o = format!("http://self.test:{}", f.port);
    assert_eq!(go(&format!("{o}/"), &for_source(&o, false)).expect_err("own address").code, "address_not_allowed");
    assert_eq!(f.conns(), 0);
}

#[test]
fn redirect_to_loopback_refused() {
    let _a = AirgapEnv::set(false);
    let inner = Fake::start(|_| http("200 OK", &[], b"{\"secret\":true}"));
    let target = format!("{}/admin", inner.origin());
    let outer = Fake::start(move |_| http("302 Found", &[("Location", &target)], b""));
    let _p = test_dns::pin("pub.test", vec![test_dns::public(LOOP)]);
    let o = format!("http://pub.test:{}", outer.port);
    let inj = Injection {
        name: "K".into(),
        value: k2_vault::SecretBytes::from_slice(b"k2test_secret_value_123"),
        dst: k2_vault::inject::Dst::Header("Authorization".into()),
        fmt: "Bearer {}".into(),
        allowed: vec![OriginPattern::parse(&o, false).unwrap()],
        sign: None,
    };
    let r = execute(src(&format!("{o}/start")), &for_source(&o, false), vec![inj], &Redactor::empty()).expect("3xx back");
    assert_eq!(r.status, 302);
    assert!(!r.redirected);
    assert_eq!(inner.conns(), 0, "the loopback target was never contacted (no secret re-sent)");
    assert_eq!(outer.conns(), 1);
}

#[test]
fn redirect_same_origin_rebinding_refused() {
    let _a = AirgapEnv::set(false);
    let up = Fake::start(|head| {
        if head.starts_with("GET /start") {
            http("302 Found", &[("Location", "/next")], b"")
        } else {
            http("200 OK", &[], b"{}")
        }
    });
    // First answer: a "public" address (our fake); then the name flips to a
    // private one. The second hop re-vets and is refused.
    let _p = test_dns::pin("flip.test", vec![test_dns::public(LOOP), test_dns::plain("10.0.0.1".parse().unwrap())]);
    let o = format!("http://flip.test:{}", up.port);
    let e = go(&format!("{o}/start"), &for_source(&o, false)).expect_err("rebinding");
    assert_eq!(e.code, "address_not_allowed");
    assert_eq!(up.conns(), 1, "only the first hop connected");
}

#[test]
fn allowlist_checked_before_injections() {
    let _a = AirgapEnv::set(false);
    let before = test_dns::lookups();
    let both = vec![
        OriginPattern::parse("https://a.example:443", false).unwrap(),
        OriginPattern::parse("https://b.example:443", false).unwrap(),
    ];
    let inj = Injection {
        name: "K".into(),
        value: k2_vault::SecretBytes::from_slice(b"k2test_secret_value_456"),
        dst: k2_vault::inject::Dst::Header("Authorization".into()),
        fmt: "Bearer {}".into(),
        allowed: both,
        sign: None,
    };
    let e = execute(src("https://b.example/"), &for_source("https://a.example:443", false), vec![inj], &Redactor::empty())
        .expect_err("not on the source's list");
    assert_eq!(e.code, "host_not_allowed");
    assert_eq!(test_dns::lookups(), before, "refused before any lookup");
}

#[test]
fn empty_allowlist_refused() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{}"));
    let e = go(&format!("{}/", f.origin()), &Allowlist::default()).expect_err("empty");
    assert_eq!(e.code, "no_allowlist");
    assert!(Allowlist::for_source(&[], true).unwrap().is_empty());
    assert_eq!(f.conns(), 0);
}

#[test]
fn vault_route_still_needs_secret() {
    let _a = AirgapEnv::set(false);
    let mut s = src("https://a.example/");
    s.purpose = Purpose::VaultFetch;
    let e = execute(s, &for_source("https://a.example:443", false), vec![], &Redactor::empty()).expect_err("no secret");
    assert_eq!(e.code, "no_secret");
}

#[test]
fn exact_ip_only_from_allowlist() {
    let _a = AirgapEnv::set(false);
    let f = Fake::start(|_| http("200 OK", &[], b"{\"lan\":1}"));
    let other = Fake::start(|_| http("200 OK", &[], b"{}"));
    // The Owner typed exactly this ip:port: allowed.
    let r = go(&format!("{}/", f.origin()), &for_source(&f.origin(), true)).expect("owner exact ip");
    assert_eq!(r.body, b"{\"lan\":1}");
    // Same IP, another port: not on the list.
    let e = go(&format!("{}/", other.origin()), &for_source(&f.origin(), true)).expect_err("other port");
    assert_eq!(e.code, "host_not_allowed");
    // lan for a different IP doesn't open this one.
    let e = go(&format!("{}/", f.origin()), &for_source("http://192.168.1.5:8080", true)).expect_err("other ip");
    assert_eq!(e.code, "host_not_allowed");
    assert_eq!(other.conns(), 0);
    assert_eq!(f.conns(), 1);
    // A LAN entry is honoured for its own origin only.
    let allow = for_source("http://192.168.1.5:8080", true);
    let (_, o) = Origin::parse_url("http://192.168.1.5:8080/x").unwrap();
    assert!(allow.exact_ip(&o));
    let (_, o2) = Origin::parse_url("http://192.168.1.5:8081/x").unwrap();
    assert!(!allow.exact_ip(&o2));
}

#[test]
fn proxy_env_ignored() {
    let _a = AirgapEnv::set(false);
    let proxy = Fake::start(|_| http("200 OK", &[], b"{\"via\":\"proxy\"}"));
    let up = Fake::start(|_| http("200 OK", &[], b"{\"via\":\"direct\"}"));
    let p = format!("http://127.0.0.1:{}", proxy.port);
    let _e1 = k2_core::test_env::EnvVar::set("HTTP_PROXY", &p);
    let _e2 = k2_core::test_env::EnvVar::set("HTTPS_PROXY", &p);
    let _e3 = k2_core::test_env::EnvVar::set("ALL_PROXY", &p);
    let _pin = test_dns::pin("direct.test", vec![test_dns::public(LOOP)]);
    let o = format!("http://direct.test:{}", up.port);
    let r = go(&format!("{o}/"), &for_source(&o, false)).expect("direct");
    assert_eq!(r.body, b"{\"via\":\"direct\"}");
    assert_eq!(proxy.conns(), 0);
}
