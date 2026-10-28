use std::cell::RefCell;
use std::net::IpAddr;

use super::*;
use crate::apps::custom_domains::test_util::runtime_app;

const HOST: &str = "nsi.agentnsi.com";
const BOX: &str = "203.0.113.10";

struct Fake {
    txt: RefCell<Vec<String>>,
    addrs: RefCell<Vec<IpAddr>>,
    cname: Option<String>,
}

impl Fake {
    fn new(txt: Vec<String>, addrs: &[&str]) -> Fake {
        Fake { txt: RefCell::new(txt), addrs: RefCell::new(addrs.iter().map(|a| a.parse().unwrap()).collect()), cname: None }
    }
}

impl Lookup for Fake {
    fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        assert_eq!(name, format!("_k2-domain.{HOST}"), "the proof is read at _k2-domain.<host>");
        Ok(self.txt.borrow().clone())
    }
    fn addrs(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
        Ok(self.addrs.borrow().clone())
    }
    fn cname(&self, _host: &str) -> Option<String> {
        self.cname.clone()
    }
}

fn setup() -> AppDomain {
    runtime_app("ws-nsi", "agentnsi");
    set_test_box_ips(Some(vec![BOX.parse().unwrap()]));
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::upsert_binding(&conn, "agentnsi.com", None, false).unwrap();
    domains::upsert_name(&conn, HOST, "agentnsi.com", "app:agentnsi").unwrap();
    domains::app_domain(&conn, HOST).unwrap().unwrap()
}

fn proof(d: &AppDomain) -> Vec<String> {
    vec!["v=spf1 -all".into(), format!("\"k2-verify={}\"", d.verify_token)]
}

fn row() -> AppDomain {
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::app_domain(&conn, HOST).unwrap().unwrap()
}

#[test]
fn the_check_needs_the_txt_and_every_address_at_this_box() {
    let _db = k2_core::db::scoped_for_test();
    let d = setup();
    let ips: Vec<IpAddr> = vec![BOX.parse().unwrap()];

    let o = evaluate(&d, &Fake::new(vec![], &[BOX]), &ips, "nsi");
    assert!(!o.ok && !o.txt_ok && o.here);
    assert!(o.reason.unwrap().contains(&format!("TXT _k2-domain.{HOST}")));

    let o = evaluate(&d, &Fake::new(vec!["k2-verify=someone-elses".into()], &[BOX]), &ips, "nsi");
    assert!(!o.ok && !o.txt_ok, "another box's token is not this box's proof");

    let o = evaluate(&d, &Fake::new(proof(&d), &["198.51.100.20"]), &ips, "nsi");
    assert!(!o.ok && o.txt_ok && !o.here);
    assert!(o.reason.as_ref().unwrap().contains("DNS now points at 198.51.100.20"), "{o:?}");

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX, "198.51.100.20"]), &ips, "nsi");
    assert!(!o.ok, "every address must be this box's (the CA may pick any)");

    let o = evaluate(&d, &Fake::new(proof(&d), &["104.16.1.1"]), &ips, "nsi");
    assert!(o.reason.unwrap().contains("grey cloud"), "Cloudflare proxy named");

    let o = evaluate(&d, &Fake::new(proof(&d), &[]), &ips, "nsi");
    assert!(o.reason.unwrap().contains("no A/AAAA record"));

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX]), &[], "nsi");
    assert!(!o.ok, "no known box address → never proven");

    let mut via_connect = Fake::new(proof(&d), &["5.6.7.8"]);
    via_connect.cname = Some("nsi.k2.dev".into());
    let o = evaluate(&d, &via_connect, &ips, "nsi");
    assert_eq!((o.ok, o.path), (false, Some(domains::PATH_CONNECT)));
    assert!(o.reason.unwrap().contains("path b"));

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX]), &ips, "nsi");
    assert_eq!((o.ok, o.path, o.reason), (true, Some(PATH_DIRECT), None));
    assert!(is_cloudflare_proxy(&"2606:4700::1".parse().unwrap()));
    assert!(!is_cloudflare_proxy(&BOX.parse().unwrap()));
}

/// waiting_dns → (proof) issuing + one order; TXT missing → stays
/// waiting_dns and no order (PRD test 4, at the flow level).
#[test]
fn a_proven_name_is_issued_and_an_unproven_one_waits() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    let orders = RefCell::new(Vec::<String>::new());
    let issue = |h: &str| orders.borrow_mut().push(h.to_string());

    let (r, o) = check_with(HOST, &Fake::new(vec![], &[BOX]), &issue).unwrap();
    assert_eq!(r.state, STATE_WAITING_DNS);
    assert!(r.last_error.as_deref().unwrap().contains("TXT"), "{:?}", r.last_error);
    assert!(!o.ok);
    assert!(orders.borrow().is_empty(), "no order without the proof");
    assert!(r.checked_at.is_some() && r.verified_at.is_none());

    let (r, _) = check_with(HOST, &Fake::new(proof(&d), &[BOX]), &issue).unwrap();
    assert_eq!(r.state, STATE_ISSUING);
    assert_eq!(*orders.borrow(), vec![HOST.to_string()]);
    assert!(r.verified_at.is_some());
    assert_eq!(r.path.as_deref(), Some(PATH_DIRECT));
    assert_eq!(r.last_error, None);
    // Issuing is routed to the door (TLS-ALPN-01), not served.
    assert_eq!(crate::apps::tls_door::reload().routed, vec![HOST.to_string()]);
    assert_eq!(crate::apps::tls_door::app_for_host(HOST), None);
}

/// A name with a usable certificate in the store (an upload, or one
/// still valid) goes straight to live once proven: no order.
#[test]
fn a_proven_name_with_a_usable_cert_goes_live_without_an_order() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    let pem = crate::domains::acme::test_le_pem(HOST);
    crate::domains::store::install(HOST, &pem.chain_pem, &pem.key_pem).unwrap();
    let orders = RefCell::new(0);
    let (r, _) = check_with(HOST, &Fake::new(proof(&d), &[BOX]), &|_| *orders.borrow_mut() += 1).unwrap();
    assert_eq!(r.state, STATE_LIVE);
    assert_eq!(*orders.borrow(), 0);
    assert_eq!(crate::apps::tls_door::reload().routed, vec![HOST.to_string()]);
    assert_eq!(crate::apps::tls_door::app_for_host(HOST), Some("agentnsi".to_string()));
}

/// PRD test 8: DNS moved away from a live name → it keeps serving with a
/// warning; after 7 days of failed re-checks it is `failing` with the new
/// target named, unrouted, and not renewed. Back at this box → live again.
#[test]
fn dns_moving_away_fails_after_seven_days_and_recovers() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    let pem = crate::domains::acme::test_le_pem(HOST);
    crate::domains::store::install(HOST, &pem.chain_pem, &pem.key_pem).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::set_app_state(&conn, HOST, STATE_LIVE, None).unwrap();
    }
    let moved = Fake::new(proof(&d), &["198.51.100.20"]);
    let t0 = 1_000_000;
    let o = evaluate(&row(), &moved, &[BOX.parse().unwrap()], "nsi");
    assert_eq!(apply(&row(), &o, t0), Next::Nothing);
    let r = row();
    assert_eq!(r.state, STATE_LIVE, "still served during the grace");
    assert_eq!(r.failing_since, Some(t0));
    assert!(r.last_error.as_deref().unwrap().contains("still served"), "{:?}", r.last_error);

    // Six days on: still live, same run.
    assert_eq!(apply(&row(), &o, t0 + 6 * 24 * 3600), Next::Nothing);
    assert_eq!((row().state.as_str(), row().failing_since), (STATE_LIVE, Some(t0)));

    // Seven days: failing, the new target named, unrouted, not renewed.
    assert_eq!(apply(&row(), &o, t0 + FAILING_AFTER_SECS), Next::Reroute);
    let r = row();
    assert_eq!(r.state, STATE_FAILING);
    assert!(r.last_error.as_deref().unwrap().contains("DNS now points at 198.51.100.20"), "{:?}", r.last_error);
    assert!(crate::apps::tls_door::reload().routed.is_empty(), "a failing name is not routed");
    let binding = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::get_binding(&conn, "agentnsi.com").unwrap()
    };
    let rr = crate::domains::renew::AttachedRow { hostname: HOST.into(), role: "app:agentnsi".into(), binding };
    assert!(crate::domains::renew::qualifies(&rr).is_err(), "renewal skipped while failing");

    // DNS back: live again (the cert is still usable), the run cleared.
    let back = Fake::new(proof(&d), &[BOX]);
    let o = evaluate(&row(), &back, &[BOX.parse().unwrap()], "nsi");
    assert_eq!(apply(&row(), &o, t0 + FAILING_AFTER_SECS + 60), Next::Reroute);
    let r = row();
    assert_eq!((r.state.as_str(), r.failing_since, r.last_error), (STATE_LIVE, None, None));
}

#[test]
fn checks_are_due_by_state() {
    let _db = k2_core::db::scoped_for_test();
    let mut d = setup();
    let at = 10_000_000;
    d.checked_at = None;
    assert!(due(&d, at));
    d.checked_at = Some(at - 5 * 60);
    assert!(!due(&d, at), "waiting: every 10 minutes");
    d.checked_at = Some(at - 11 * 60);
    assert!(due(&d, at));
    d.state = STATE_LIVE.into();
    assert!(!due(&d, at), "live: daily");
    d.checked_at = Some(at - 25 * 3600);
    assert!(due(&d, at));
    d.state = STATE_FAILING.into();
    d.checked_at = Some(at - 30 * 60);
    assert!(!due(&d, at));
}

/// §6: on a K2-hosted zone K2 plants the TXT itself (and replaces older
/// copies), through the DNS API fake: never a live zone in tests.
#[test]
fn k2_plants_and_removes_the_txt_on_a_k2_hosted_zone() {
    let _db = k2_core::db::scoped_for_test();
    let d = setup();
    let posted = std::rc::Rc::new(RefCell::new(Vec::<String>::new()));
    let p = posted.clone();
    let _api = crate::dns::proxy::set_fake_dns_api(move |method, path, body| {
        let ok = |b: &str| Ok(crate::dns::proxy::DnsHttpResponse { status: 200, body: b.to_string() });
        match (method, path) {
            ("GET", "/api/dns/zones/z-nsi") => ok(r#"{"records":[{"id":"r1","type":"TXT","name":"_k2-domain.nsi","content":"k2-verify=old"},{"id":"r2","type":"A","name":"nsi","content":"203.0.113.10"}]}"#),
            ("DELETE", "/api/dns/records/r1") => ok("{}"),
            ("POST", "/api/dns/zones/z-nsi/records") => {
                p.borrow_mut().push(body.unwrap_or("").to_string());
                ok(r#"{"record":{"id":"r9"}}"#)
            }
            other => panic!("unexpected DNS API call {other:?}"),
        }
    });
    let binding = k2_core::domains::DomainBinding {
        apex: "agentnsi.com".into(),
        zone_id: Some("z-nsi".into()),
        dns_write: true,
        created_at: 0,
        status: Some("active".into()),
        nameservers: vec![],
        auto_created: false,
    };
    assert_eq!(txt_relative_name(HOST, "agentnsi.com"), "_k2-domain.nsi");
    plant_txt(&d, &binding).unwrap();
    let body: J = serde_json::from_str(&posted.borrow()[0]).unwrap();
    assert_eq!(body["type"], "TXT");
    assert_eq!(body["name"], "_k2-domain.nsi");
    assert_eq!(body["content"], format!("k2-verify={}", d.verify_token));
    let calls = crate::dns::proxy::fake_dns_api_calls();
    assert!(calls.iter().any(|(m, p)| m == "DELETE" && p == "/api/dns/records/r1"), "the old proof is replaced: {calls:?}");
    assert!(!calls.iter().any(|(_, p)| p.ends_with("/r2")), "never touches other records");
}

/// Coordinator ruling on #10: a grandfathered `external` name (bound
/// before 0.46.6, served by the box's own proxy) stays an origin, is never
/// re-checked by the loop or routed to the door, and gets no warning. The
/// owner's explicit check moves it into the door flow only when the proof
/// passes; a failed check leaves it `external` (still an origin).
#[test]
fn a_grandfathered_name_stays_an_origin_until_the_owner_moves_it() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::set_app_state(&conn, HOST, STATE_EXTERNAL, None).unwrap();
    }
    let ext = row();
    assert!(ext.is_external() && ext.is_origin());
    assert!(!due(&ext, 10_000_000_000), "the loop never checks it");
    assert!(crate::apps::tls_door::reload().routed.is_empty(), "the door never routes it");
    let st = crate::apps::custom_domains::status_json(&ext, true);
    assert_eq!((st["notServedYet"].as_bool(), st["external"].as_bool()), (Some(false), Some(true)));
    assert!(st["words"].as_str().unwrap().starts_with("served by your own proxy (not by K2)"), "{st}");
    assert!(st.get("served").is_none() && st.get("warning").is_none(), "no probe, no warning spam: {st}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(domains::origin_hostnames_for_app(&conn, "agentnsi").unwrap(), vec![HOST.to_string()]);
    }

    // The owner's check, DNS not at this box: stays external, still an origin.
    let issues = RefCell::new(0);
    let (r, _) = check_with(HOST, &Fake::new(proof(&d), &["198.51.100.20"]), &|_| *issues.borrow_mut() += 1).unwrap();
    assert_eq!(r.state, STATE_EXTERNAL);
    assert!(r.is_origin());
    assert!(r.last_error.as_deref().unwrap().contains("still served by your own proxy"), "{:?}", r.last_error);
    assert_eq!(*issues.borrow(), 0);

    // Proven: into the door flow (issuing, one order).
    let (r, _) = check_with(HOST, &Fake::new(proof(&d), &[BOX]), &|_| *issues.borrow_mut() += 1).unwrap();
    assert_eq!(r.state, STATE_ISSUING);
    assert_eq!(*issues.borrow(), 1);
}

/// Coordinator ruling on #5: under air-gap the status says why a name
/// stays where it is (no DNS checks, no certificates).
#[test]
fn under_air_gap_the_status_says_why_nothing_moves() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let _air = k2_core::test_env::EnvVar::set("K2_AIRGAP", "1");
    setup();
    let w = crate::apps::custom_domains::state_words(&row());
    assert!(w.contains("Air-gap is on") && w.contains("stays waiting_dns"), "{w}");
    assert!(check_now(HOST).is_none(), "no check under air-gap");
}

// ── setup copy (0.46.6, PRD §7, PA-A11) ──────────────────────────────

#[test]
fn nat_is_the_egress_address_missing_from_every_interface() {
    let on_if: Vec<std::net::Ipv4Addr> = vec!["203.0.113.10".parse().unwrap()];
    assert_eq!(nat_verdict(Some("203.0.113.10"), &on_if), Some(false));
    assert_eq!(nat_verdict(Some("198.51.100.4"), &on_if), Some(true), "egress elsewhere = a router");
    assert_eq!(nat_verdict(Some("198.51.100.4"), &[]), Some(true), "only private interfaces = a router");
    assert_eq!(nat_verdict(None, &on_if), None, "air-gap / lookup failed: can't tell");
    assert_eq!(nat_verdict(Some("garbage"), &on_if), None);
}

/// Test 10 (PRD §11.1): behind a router → the Path B wording, Path A
/// records only under "Advanced"; a desktop → no Path A records; a
/// foreign :443 holder named with its fixes; unknown NAT → no Path B words.
#[test]
fn setup_advice_points_nat_and_desktops_at_path_b() {
    let nat = advice_for(true, Some(true), true, None);
    assert_eq!(nat["path"], "b");
    assert_eq!(nat["records"], "advanced");
    assert_eq!(nat["message"], NAT_WORDING);
    assert!(nat["advanced"].as_str().unwrap().contains("forwarded port 443"));
    let desk = advice_for(false, None, false, None);
    assert_eq!(desk["path"], "b");
    assert_eq!(desk["records"], "none");
    assert_eq!(desk["message"], DESKTOP_WORDING);
    let ok = advice_for(true, Some(false), true, None);
    assert_eq!(ok["path"], "a");
    assert!(ok["message"].is_null());
    let unknown = advice_for(true, None, true, None);
    assert_eq!(unknown["path"], "a", "unknown NAT never gets the Path B wording");
    let off = advice_for(true, Some(false), false, None);
    assert!(off["message"].as_str().unwrap().contains("k2 front enable"));
    let holder = json!({"holder": "foreign", "units": ["caddy"], "message": crate::front443::foreign_443_message(&["caddy".to_string()])});
    let foreign = advice_for(true, Some(false), false, Some(&holder));
    let m = foreign["message"].as_str().unwrap();
    assert!(m.contains("caddy") && m.contains("k2 front passthrough add") && m.contains("never takes :443"), "{m}");
    assert_eq!(foreign["port443"]["holder"], "foreign");
    // Through the seam: the add / check answer carries it.
    set_test_nat(Some(Some(true)));
    assert_eq!(behind_nat(), Some(true));
    assert_eq!(setup_advice()["path"], "b");
    set_test_nat(None);
    assert_eq!(behind_nat(), None);
}
