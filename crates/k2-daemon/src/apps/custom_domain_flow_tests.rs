use std::cell::RefCell;
use std::net::IpAddr;

use super::*;
use crate::apps::custom_domains::test_util::runtime_app;

const HOST: &str = "initech.agentinitech.com";
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
    fn addrs(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        // This box's own k2.dev name: the relay it is on (nothing here,
        // unless a test says so).
        if host.ends_with(".k2.dev") {
            return Ok(Vec::new());
        }
        Ok(self.addrs.borrow().clone())
    }
    fn cname(&self, host: &str) -> Option<String> {
        // One hop: the name itself, never its target.
        if host != HOST {
            return None;
        }
        self.cname.clone()
    }
}

fn setup() -> AppDomain {
    runtime_app("ws-initech", "agentinitech");
    set_test_box_ips(Some(vec![BOX.parse().unwrap()]));
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::upsert_binding(&conn, "agentinitech.com", None, false).unwrap();
    domains::upsert_name(&conn, HOST, "agentinitech.com", "app:agentinitech").unwrap();
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

    let o = evaluate(&d, &Fake::new(vec![], &[BOX]), &ips, "initech");
    assert!(!o.ok && !o.txt_ok && o.here);
    assert!(o.reason.unwrap().contains(&format!("TXT _k2-domain.{HOST}")));

    let o = evaluate(&d, &Fake::new(vec!["k2-verify=someone-elses".into()], &[BOX]), &ips, "initech");
    assert!(!o.ok && !o.txt_ok, "another box's token is not this box's proof");

    let o = evaluate(&d, &Fake::new(proof(&d), &["198.51.100.20"]), &ips, "initech");
    assert!(!o.ok && o.txt_ok && !o.here);
    assert!(o.reason.as_ref().unwrap().contains("DNS now points at 198.51.100.20"), "{o:?}");

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX, "198.51.100.20"]), &ips, "initech");
    assert!(!o.ok, "every address must be this box's (the CA may pick any)");

    let o = evaluate(&d, &Fake::new(proof(&d), &["104.16.1.1"]), &ips, "initech");
    assert!(o.reason.unwrap().contains("grey cloud"), "Cloudflare proxy named");

    let o = evaluate(&d, &Fake::new(proof(&d), &[]), &ips, "initech");
    assert!(o.reason.unwrap().contains("no A/AAAA record"));

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX]), &[], "initech");
    assert!(!o.ok, "no known box address → never proven");

    // A CNAME to this box's k2.dev name is Path B (0.46.8).
    let mut via_connect = Fake::new(proof(&d), &["5.6.7.8"]);
    via_connect.cname = Some("initech.k2.dev".into());
    let o = evaluate(&d, &via_connect, &ips, "initech");
    assert_eq!((o.ok, o.path), (true, Some(domains::PATH_CONNECT)));

    let o = evaluate(&d, &Fake::new(proof(&d), &[BOX]), &ips, "initech");
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
    assert_eq!(crate::apps::tls_door::app_for_host(HOST), Some("agentinitech".to_string()));
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
    let o = evaluate(&row(), &moved, &[BOX.parse().unwrap()], "initech");
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
        domains::get_binding(&conn, "agentinitech.com").unwrap()
    };
    let rr = crate::domains::renew::AttachedRow { hostname: HOST.into(), role: "app:agentinitech".into(), binding };
    assert!(crate::domains::renew::qualifies(&rr).is_err(), "renewal skipped while failing");

    // DNS back: live again (the cert is still usable), the run cleared.
    let back = Fake::new(proof(&d), &[BOX]);
    let o = evaluate(&row(), &back, &[BOX.parse().unwrap()], "initech");
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
            ("GET", "/api/dns/zones/z-initech") => ok(r#"{"records":[{"id":"r1","type":"TXT","name":"_k2-domain.initech","content":"k2-verify=old"},{"id":"r2","type":"A","name":"initech","content":"203.0.113.10"}]}"#),
            ("DELETE", "/api/dns/records/r1") => ok("{}"),
            ("POST", "/api/dns/zones/z-initech/records") => {
                p.borrow_mut().push(body.unwrap_or("").to_string());
                ok(r#"{"record":{"id":"r9"}}"#)
            }
            other => panic!("unexpected DNS API call {other:?}"),
        }
    });
    let binding = k2_core::domains::DomainBinding {
        apex: "agentinitech.com".into(),
        zone_id: Some("z-initech".into()),
        dns_write: true,
        created_at: 0,
        status: Some("active".into()),
        nameservers: vec![],
        auto_created: false,
    };
    assert_eq!(txt_relative_name(HOST, "agentinitech.com"), "_k2-domain.initech");
    plant_txt(&d, &binding).unwrap();
    let body: J = serde_json::from_str(&posted.borrow()[0]).unwrap();
    assert_eq!(body["type"], "TXT");
    assert_eq!(body["name"], "_k2-domain.initech");
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
    assert!(st["words"].as_str().unwrap().starts_with("served by your own web server (not by K2's front)"), "{st}");
    assert!(st.get("served").is_none() && st.get("warning").is_none(), "no probe, no warning spam: {st}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(domains::origin_hostnames_for_app(&conn, "agentinitech").unwrap(), vec![HOST.to_string()]);
    }

    // The owner's check, DNS not at this box: stays external, still an origin.
    let issues = RefCell::new(0);
    let (r, _) = check_with(HOST, &Fake::new(proof(&d), &["198.51.100.20"]), &|_| *issues.borrow_mut() += 1).unwrap();
    assert_eq!(r.state, STATE_EXTERNAL);
    assert!(r.is_origin());
    assert!(r.last_error.as_deref().unwrap().contains("still served by your own web server"), "{:?}", r.last_error);
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

fn path_b(open: bool) -> PathB {
    PathB { ready: Ok(()), pro: Ok(open), used: 3 }
}

/// Test 10 (PRD §11.1), 0.46.8: behind a router or on a desktop, Path B
/// when it is open to the box (its records shown), else the Pro words (Path
/// A records only under "Advanced" behind a router, none on a desktop); a
/// foreign :443 holder named with its fixes, and Path B offered beside
/// them; unknown NAT → no Path B wording.
#[test]
fn setup_advice_points_nat_and_desktops_at_path_b() {
    let _db = k2_core::db::scoped_for_test();
    let closed = path_b(false);
    let open = path_b(true);
    let nat = advice_for(true, Some(true), true, None, &closed);
    assert_eq!(nat["path"], "b");
    assert_eq!(nat["records"], "advanced");
    let m = nat["message"].as_str().unwrap();
    assert!(m.contains("home or office") && m.contains("Pro") && m.contains(super::super::relay_claims::UPGRADE_URL), "{m}");
    assert!(nat["advanced"].as_str().unwrap().contains("forwarded port 443"));
    let nat = advice_for(true, Some(true), true, None, &open);
    assert_eq!((nat["records"].as_str(), nat["message"].as_str()), (Some("shown"), Some(NAT_WORDING)));
    assert_eq!(nat["pathB"]["available"], true);
    assert_eq!(nat["pathB"]["meter"], "3 of 20 names through K2's relays");
    let desk = advice_for(false, None, false, None, &closed);
    assert_eq!((desk["path"].as_str(), desk["records"].as_str(), desk["pathA"].as_bool()), (Some("b"), Some("none"), Some(false)));
    assert!(desk["message"].as_str().unwrap().contains("Pro"), "{desk}");
    assert_eq!(desk["pathB"]["upgrade"], super::super::relay_claims::UPGRADE_URL);
    let desk = advice_for(false, None, false, None, &open);
    assert_eq!((desk["records"].as_str(), desk["message"].as_str()), (Some("shown"), Some(DESKTOP_WORDING)));
    // Can't read the plan: said so, never a silent pass.
    let unknown_plan = PathB { ready: Ok(()), pro: Err("K2 can't check this server's plan right now".into()), used: 0 };
    assert!(!unknown_plan.open());
    assert_eq!(advice_for(false, None, false, None, &unknown_plan)["pathB"]["why"], "K2 can't check this server's plan right now");
    let ok = advice_for(true, Some(false), true, None, &open);
    assert_eq!(ok["path"], "a", "K2's front serves: Path A, Path B beside it");
    assert!(ok["message"].is_null());
    assert_eq!(ok["pathB"]["available"], true);
    let unknown = advice_for(true, None, true, None, &closed);
    assert_eq!(unknown["path"], "a", "unknown NAT never gets the Path B wording");
    let off = advice_for(true, Some(false), false, None, &closed);
    assert!(off["message"].as_str().unwrap().contains("k2 front enable"));
    let holder = json!({"holder": "foreign", "units": ["caddy"], "message": crate::front443::foreign_443_message(&["caddy".to_string()])});
    let foreign = advice_for(true, Some(false), false, Some(&holder), &closed);
    let m = foreign["message"].as_str().unwrap();
    assert!(m.contains("caddy") && m.contains("k2 front passthrough add") && m.contains("never takes :443"), "{m}");
    assert_eq!(foreign["port443"]["holder"], "foreign");
    // The field case: another web server on :443, Pro behind K2 Connect.
    let foreign = advice_for(true, Some(false), false, Some(&holder), &open);
    assert_eq!((foreign["path"].as_str(), foreign["records"].as_str()), (Some("b"), Some("shown")));
    let m = foreign["message"].as_str().unwrap();
    assert!(m.contains("Another web server holds port 443") && m.contains("no sudo"), "{m}");
    assert_eq!(foreign["port443"]["holder"], "foreign");
    // Through the seam: the add / check answer carries it.
    set_test_nat(Some(Some(true)));
    assert_eq!(behind_nat(), Some(true));
    assert_eq!(setup_advice()["path"], "b");
    set_test_nat(None);
    assert_eq!(behind_nat(), None);
}

// ── Path B (0.46.8, prd-custom-domains-path-b-0467-v1) ───────────────

use super::super::relay_claims::{self, ClaimAnswer, CpError, RelayApi};
use std::rc::Rc;

/// The control plane, scripted: what `claim` and `verify` answer, every call seen.
#[derive(Default)]
struct FakeCp {
    claim: RefCell<Option<Result<ClaimAnswer, CpError>>>,
    verify: RefCell<Option<Result<ClaimAnswer, CpError>>>,
    list: RefCell<Vec<ClaimAnswer>>,
    list_down: RefCell<bool>,
    release_err: RefCell<Option<CpError>>,
    calls: RefCell<Vec<String>>,
}

/// The proof token the control plane picks in these tests.
const PICKED: &str = "relay-picked-token-0123456789";

fn answer(status: &str) -> ClaimAnswer {
    ClaimAnswer { hostname: HOST.into(), status: status.into(), verify_token: Some(PICKED.into()), last_error: None }
}

impl RelayApi for FakeCp {
    fn http_redirect(&self, hostname: Option<&str>, enabled: bool) -> Result<(), CpError> {
        self.calls.borrow_mut().push(format!("http-redirect {hostname:?} {enabled}"));
        Ok(())
    }
    fn claim(&self, hostname: &str, http_redirect: Option<bool>) -> Result<ClaimAnswer, CpError> {
        self.calls.borrow_mut().push(format!("claim {hostname} {http_redirect:?}"));
        self.claim.borrow().clone().unwrap_or_else(|| Ok(answer("pending")))
    }
    fn verify(&self, hostname: &str) -> Result<ClaimAnswer, CpError> {
        self.calls.borrow_mut().push(format!("verify {hostname}"));
        self.verify.borrow().clone().unwrap_or_else(|| Ok(answer("verified")))
    }
    fn release(&self, hostname: &str) -> Result<(), CpError> {
        self.calls.borrow_mut().push(format!("release {hostname}"));
        match self.release_err.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
        self.calls.borrow_mut().push("list".into());
        if *self.list_down.borrow() {
            return Err(CpError::Unreachable("HTTP 502".into()));
        }
        Ok(self.list.borrow().clone())
    }
}

struct Seams;
impl Drop for Seams {
    fn drop(&mut self) {
        relay_claims::set_test_api(None);
        relay_claims::set_test_pro(None);
        relay_claims::set_test_ready(None);
    }
}

/// Pro, behind K2 Connect, on the fake control plane.
fn path_b_box() -> (Rc<FakeCp>, Seams) {
    relay_claims::reset_missing_for_test();
    let cp = Rc::new(FakeCp::default());
    relay_claims::set_test_api(Some(cp.clone()));
    relay_claims::set_test_pro(Some(Ok(true)));
    relay_claims::set_test_ready(Some(Ok(())));
    (cp, Seams)
}

/// DNS: a CNAME to this App's name on this box, and the TXT K2 Connect
/// picked. The box's label comes from tunnel.json, so the test names it.
fn cname_here(_: &AppDomain) -> Fake {
    let mut f = Fake::new(vec![format!("k2-verify={PICKED}")], &["192.0.2.50"]);
    f.cname = Some(format!("agentinitech.{}.k2.dev", label()));
    f
}

fn label() -> String {
    crate::apps::origins::box_subdomain()
}

fn set_box_label(sub: &str) {
    let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
    cfg.subdomain = sub.into();
    k2_core::tunnel::config::save(&cfg).unwrap();
    k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap { primary: sub.into(), targets: Default::default() });
}

/// PRD test 7 (ruling 11-01: no A records offered, but the check still
/// recognises them): a CNAME chain to the box's name passes; an ALIAS or
/// flattened apex answering only relay addresses passes; relay addresses
/// mixed with anything else fail; a proxied answer is named; another K2
/// box's name is named; Path A is unchanged.
#[test]
fn the_dns_check_recognises_path_b() {
    let _db = k2_core::db::scoped_for_test();
    let d = setup();
    let ips: Vec<IpAddr> = vec![BOX.parse().unwrap()];
    let relay: IpAddr = "192.0.2.50".parse().unwrap();

    // CNAME to the App's own name, and to the box's name.
    for target in ["agentinitech.initech.k2.dev", "initech.k2.dev", "INITECH.k2.dev."] {
        let mut f = Fake::new(proof(&d), &["192.0.2.50"]);
        f.cname = Some(target.into());
        let o = evaluate_with(&d, &f, &ips, "initech", &[relay]);
        assert_eq!((o.ok, o.here, o.path), (true, true, Some(domains::PATH_CONNECT)), "{target}");
    }
    // TXT missing: DNS is right, still not proven.
    let mut f = Fake::new(vec![], &["192.0.2.50"]);
    f.cname = Some("agentinitech.initech.k2.dev".into());
    let o = evaluate_with(&d, &f, &ips, "initech", &[relay]);
    assert_eq!((o.ok, o.here, o.path), (false, true, Some(domains::PATH_CONNECT)));
    assert!(o.reason.unwrap().contains("TXT"));
    // ALIAS / flattening: no CNAME, only relay addresses.
    let o = evaluate_with(&d, &Fake::new(proof(&d), &["192.0.2.50"]), &ips, "initech", &[relay]);
    assert_eq!((o.ok, o.path), (true, Some(domains::PATH_CONNECT)));
    // Mixed.
    let o = evaluate_with(&d, &Fake::new(proof(&d), &["192.0.2.50", "198.51.100.20"]), &ips, "initech", &[relay]);
    assert!(!o.ok);
    let r = o.reason.unwrap();
    assert!(r.contains("part of your DNS points elsewhere (198.51.100.20)") && r.contains("agentinitech.initech.k2.dev"), "{r}");
    // Cloudflare's proxy is named, as on Path A.
    let o = evaluate_with(&d, &Fake::new(proof(&d), &["104.16.1.1"]), &ips, "initech", &[relay]);
    assert!(o.reason.unwrap().contains("grey cloud"));
    // Another server's K2 name.
    let mut f = Fake::new(proof(&d), &["192.0.2.50"]);
    f.cname = Some("other.k2.dev".into());
    let o = evaluate_with(&d, &f, &ips, "initech", &[relay]);
    assert!(!o.ok && o.path.is_none());
    assert!(o.reason.unwrap().contains("isn't this server's K2 name: point it at agentinitech.initech.k2.dev"));
    // Off K2 Connect a relay address is just somewhere else.
    let o = evaluate_with(&d, &Fake::new(proof(&d), &["192.0.2.50"]), &ips, "", &[relay]);
    assert!(!o.ok && o.path.is_none(), "{o:?}");
    // Path A is untouched.
    let o = evaluate_with(&d, &Fake::new(proof(&d), &[BOX]), &ips, "initech", &[relay]);
    assert_eq!((o.ok, o.path), (true, Some(PATH_DIRECT)));
}

/// PB-5, rulings 1 and K2 Web 11-01: a CNAME to the App's own name, an
/// ALIAS at an apex, the TXT; never an `A` record at a relay.
#[test]
fn path_b_records_are_a_cname_or_alias_and_never_a_relay_address() {
    let _db = k2_core::db::scoped_for_test();
    let mut d = setup();
    d.relay_state = Some(domains::RELAY_CLAIMED.into());
    let r = records_connect(&d, "initech");
    assert_eq!(r.len(), 2);
    assert_eq!((r[0]["type"].as_str(), r[0]["name"].as_str(), r[0]["value"].as_str()), (Some("CNAME"), Some(HOST), Some("agentinitech.initech.k2.dev")));
    assert!(r[0]["note"].as_str().unwrap().contains("DNS-only"));
    assert_eq!(r[1]["type"], "TXT");
    let mut apex = d.clone();
    apex.hostname = "agentinitech.com".into();
    let r = records_connect(&apex, "initech");
    assert_eq!((r[0]["type"].as_str(), r[0]["value"].as_str()), (Some("ALIAS"), Some("agentinitech.initech.k2.dev")));
    let note = r[0]["note"].as_str().unwrap();
    assert!(note.contains("ALIAS, ANAME or CNAME flattening") && note.contains("DNS-only"), "{note}");
    for rec in records_connect(&d, "initech").iter().chain(r.iter()) {
        assert!(!matches!(rec["type"].as_str(), Some("A") | Some("AAAA")), "no relay address is ever offered: {rec}");
    }
    assert!(records_connect(&d, "").is_empty(), "off K2 Connect there is nothing to point at");
    // Which path's records show.
    let advice_b = json!({"path": "b", "pathA": true, "pathB": {"available": true}});
    let advice_a = json!({"path": "a", "pathA": true, "pathB": {"available": true}});
    assert_eq!(shown_path(&d, &advice_b), domains::PATH_CONNECT);
    assert_eq!(shown_path(&d, &advice_a), PATH_DIRECT);
    let mut dns_b = d.clone();
    dns_b.path = Some(domains::PATH_CONNECT.into());
    assert_eq!(shown_path(&dns_b, &advice_a), domains::PATH_CONNECT, "DNS decides");
    let mut dns_a = d.clone();
    dns_a.path = Some(PATH_DIRECT.into());
    assert_eq!(shown_path(&dns_a, &advice_b), PATH_DIRECT, "DNS decides");
}

/// PRD test 5: claim + verify → issuing, one order, the tunnel's custom
/// proxy carries it, and TLS-ALPN-01 is the proof (no front443; PB-A14 needs
/// the E2E listener up, asserted in `via_relay`). The body names the box's
/// token. A second check doesn't claim again.
#[test]
fn a_verified_claim_issues_and_the_tunnel_carries_the_name() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let orders = RefCell::new(Vec::<String>::new());
    let issue = |h: &str| orders.borrow_mut().push(h.to_string());
    let (r, o) = check_with(HOST, &cname_here(&d), &issue).unwrap();
    assert!(o.ok, "{o:?}");
    assert_eq!(r.state, STATE_ISSUING);
    assert_eq!(r.path.as_deref(), Some(domains::PATH_CONNECT));
    assert_eq!(r.relay_state.as_deref(), Some(domains::RELAY_VERIFIED));
    assert!(r.via_relay());
    assert_eq!(*orders.borrow(), vec![HOST.to_string()]);
    // The claim comes first and carries no token; the row now holds K2
    // Connect's, and the box's own is gone.
    assert_eq!(*cp.calls.borrow(), vec![format!("claim {HOST} None"), format!("verify {HOST}")]);
    assert_eq!(r.verify_token, PICKED);
    assert_ne!(d.verify_token, PICKED);
    assert_eq!(r.txt(), (format!("_k2-domain.{HOST}"), format!("k2-verify={PICKED}")));
    assert!(crate::domains::acme::app_name_via_relay(HOST));
    let rl = crate::apps::custom_names::reload();
    assert_eq!(rl.tunnel, vec![HOST.to_string()], "issuing names ride the custom proxy");
    assert_eq!(k2_core::tunnel::custom_names::current(), vec![HOST.to_string()]);
    let _ = k2_core::tunnel::custom_names::set(Vec::new());

    // A re-check of a verified name calls nobody.
    cp.calls.borrow_mut().clear();
    let (r, _) = check_with(HOST, &cname_here(&row()), &issue).unwrap();
    assert_eq!(r.state, STATE_ISSUING);
    assert!(cp.calls.borrow().is_empty(), "{:?}", cp.calls.borrow());

    // A Path A name never rides the custom proxy.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::record_app_check(&conn, HOST, &CheckRecord { checked_at: 1, verified_at: Some(1), path: Some(PATH_DIRECT.into()), failing_since: None }).unwrap();
    }
    assert!(crate::apps::custom_names::reload().tunnel.is_empty());
    let _ = k2_core::tunnel::custom_names::set(Vec::new());
}

/// K2 Web's review: a TXT carrying the box's own old token proves nothing. K2
/// claims, learns K2 Connect's value, and waits for that one; never verifies.
#[test]
fn the_boxs_own_token_is_not_the_proof_on_path_b() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let mut f = Fake::new(proof(&d), &["192.0.2.50"]);
    f.cname = Some("agentinitech.initech.k2.dev".into());
    let (r, o) = check_with(HOST, &f, &|h| panic!("no order for {h}")).unwrap();
    assert!(!o.ok && !o.txt_ok, "{o:?}");
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_WAITING_DNS, Some(domains::RELAY_CLAIMED)));
    assert!(r.last_error.as_deref().unwrap().contains(&format!("k2-verify={PICKED}")), "{:?}", r.last_error);
    assert_eq!(*cp.calls.borrow(), vec![format!("claim {HOST} None")], "no verify before the TXT shows");
    // No claim yet: the CNAME alone, and a claim that fails says why.
    let bare = {
        let mut b = d.clone();
        b.relay_state = None;
        b
    };
    assert_eq!(records_connect(&bare, "initech").len(), 1);
    let mut held = d.clone();
    held.relay_state = Some(domains::RELAY_CLAIMED.into());
    held.verify_token = PICKED.into();
    assert_eq!(records_connect(&held, "initech")[1]["value"], format!("k2-verify={PICKED}"));
}

/// The owner turned the :80 redirect off: the claim says so; otherwise it is
/// left out (a held name keeps its switch, a new one starts on).
#[test]
fn the_claim_sends_http_redirect_only_when_the_owner_chose() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let mut cfg = k2_core::front443::load();
    cfg.http = Some(false);
    k2_core::front443::save(&cfg).unwrap();
    let _ = check_with(HOST, &cname_here(&d), &|_| {}).unwrap();
    assert_eq!(cp.calls.borrow()[0], format!("claim {HOST} Some(false)"));
}

/// PRD test 5: each refusal maps to words and a state; pending waits.
#[test]
fn control_plane_refusals_are_plain_words_and_never_issue() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let no_order = |h: &str| panic!("no order for {h}");
    for (code, want) in [
        ("pro_required", "isn't Pro"),
        ("cap_reached", "already holds"),
        ("hostname_claimed_elsewhere", "another K2 server or account"),
        ("capability_missing", "update K2"),
    ] {
        *cp.claim.borrow_mut() = Some(Err(CpError::Refused { code: code.into(), words: relay_claims::words_for(code, HOST, &J::Null) }));
        let (r, o) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
        assert!(!o.ok, "{code}");
        assert_eq!(r.state, STATE_WAITING_DNS, "{code}");
        assert_eq!(r.relay_state.as_deref(), Some(domains::RELAY_REFUSED), "{code}");
        assert!(r.last_error.as_deref().unwrap().contains(want), "{code}: {:?}", r.last_error);
    }
    // Pending at the control plane: claimed, waiting; then verified.
    *cp.claim.borrow_mut() = None;
    *cp.verify.borrow_mut() = Some(Ok(answer("pending")));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_WAITING_DNS, Some(domains::RELAY_CLAIMED)));
    assert!(r.last_error.as_deref().unwrap().contains("can't see the TXT record yet"), "{:?}", r.last_error);
    // Its own TXT check fails: claimed, waiting, K2 Connect's words.
    *cp.verify.borrow_mut() = Some(Err(CpError::Refused {
        code: "verification_failed".into(),
        words: relay_claims::words_for("verification_failed", HOST, &json!({"detail": "TXT does not hold it"})),
    }));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_WAITING_DNS, Some(domains::RELAY_CLAIMED)));
    assert!(r.last_error.as_deref().unwrap().contains("TXT does not hold it"), "{:?}", r.last_error);
    // Its rate limit: said, nothing sticks.
    *cp.verify.borrow_mut() = Some(Err(CpError::Retry(relay_claims::words_for("rate_limited", HOST, &J::Null))));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!(r.relay_state.as_deref(), Some(domains::RELAY_CLAIMED));
    assert!(r.last_error.as_deref().unwrap().contains("5 new names an hour"), "{:?}", r.last_error);
    // It dropped the old claim: this box holds none now, and the next check
    // claims again.
    *cp.verify.borrow_mut() = Some(Err(CpError::Refused { code: "not_claimed".into(), words: "no claim".into() }));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!(r.relay_state, None);
    assert_eq!(r.relay_error.as_deref(), Some("no claim"));
    cp.calls.borrow_mut().clear();
    *cp.verify.borrow_mut() = Some(Ok(answer("pending")));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!(cp.calls.borrow()[0], format!("claim {HOST} None"));
    assert_eq!(r.relay_state.as_deref(), Some(domains::RELAY_CLAIMED));
    // No answer at all: says so, tries again, no state change at the plane.
    *cp.verify.borrow_mut() = Some(Err(CpError::Unreachable("HTTP 502".into())));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!(r.relay_state.as_deref(), Some(domains::RELAY_CLAIMED));
    assert!(r.last_error.as_deref().unwrap().contains("K2 Connect didn't answer"), "{:?}", r.last_error);
}

/// PRD test 6, PB-4: not Pro, the plan unreadable, off K2 Connect, or the
/// 21st name: refused on the box before any call to the control plane.
#[test]
fn the_box_refuses_before_any_claim_when_it_cant_be_path_b() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let no_order = |h: &str| panic!("no order for {h}");

    relay_claims::set_test_pro(Some(Ok(false)));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert_eq!(r.state, STATE_WAITING_DNS);
    let e = r.last_error.as_deref().unwrap();
    assert!(e.contains("needs K2 Connect Pro") && e.contains(relay_claims::UPGRADE_URL), "{e}");
    assert_eq!(r.relay_state, None, "nothing was claimed");

    relay_claims::set_test_pro(Some(Err("K2 can't check this server's plan right now".into())));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert!(r.last_error.as_deref().unwrap().contains("can't check this server's plan"), "never a silent pass");

    relay_claims::set_test_pro(Some(Ok(true)));
    relay_claims::set_test_ready(Some(Err("this server isn't on K2 Connect yet".into())));
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    assert!(r.last_error.as_deref().unwrap().contains("isn't on K2 Connect"), "{:?}", r.last_error);

    relay_claims::set_test_ready(Some(Ok(())));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for i in 0..k2_core::tunnel::custom_names::MAX_PER_BOX {
            let h = format!("n{i}.agentinitech.com");
            domains::upsert_name(&conn, &h, "agentinitech.com", "app:agentinitech").unwrap();
            domains::set_relay_state(&conn, &h, Some(domains::RELAY_VERIFIED), None, 1).unwrap();
        }
    }
    let (r, _) = check_with(HOST, &cname_here(&d), &no_order).unwrap();
    let e = r.last_error.as_deref().unwrap();
    assert!(e.contains("already has 20 of 20") && e.contains("k2 app domain rm"), "{e}");
    assert!(cp.calls.borrow().is_empty(), "the box said no first: {:?}", cp.calls.borrow());
}

/// PB-A15: for a name K2 Connect routes, the box's failed check only informs
/// (never the 7-day run), and the control plane's `released` wins: `failing`,
/// its reason shown, off the tunnel. `verified` keeps it.
#[test]
fn the_control_plane_decides_a_relayed_names_fate() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let d = setup();
    set_box_label("initech");
    let (cp, _s) = path_b_box();
    let pem = crate::domains::acme::test_le_pem(HOST);
    crate::domains::store::install(HOST, &pem.chain_pem, &pem.key_pem).unwrap();
    let (r, _) = check_with(HOST, &cname_here(&d), &|_| {}).unwrap();
    assert_eq!(r.state, STATE_LIVE, "a usable cert goes live at once");
    assert!(crate::apps::custom_domains::state_words(&r).contains("through K2 Connect"));

    // DNS moved: 30 days of failed box checks change nothing but the words.
    let moved = Fake::new(proof(&d), &["198.51.100.20"]);
    let o = evaluate_with(&row(), &moved, &[BOX.parse().unwrap()], "initech", &["192.0.2.50".parse().unwrap()]);
    assert!(!o.ok);
    assert_eq!(apply(&row(), &o, 1_000_000 + 30 * 24 * 3600), Next::Nothing);
    let r = row();
    assert_eq!((r.state.as_str(), r.failing_since), (STATE_LIVE, None));
    assert!(r.last_error.as_deref().unwrap().contains("K2 Connect still routes it"), "{:?}", r.last_error);

    // The list says verified: nothing changes.
    *cp.list.borrow_mut() = vec![answer("verified")];
    assert_eq!(relay_claims::with_api(relay_claims::sync), Ok(vec![]));
    assert_eq!(row().state, STATE_LIVE);
    // The list says released: failing, wins over everything, off the tunnel.
    *cp.list.borrow_mut() = vec![ClaimAnswer { hostname: HOST.into(), status: "released".into(), verify_token: Some(PICKED.into()), last_error: Some("TXT gone for 7 days".into()) }];
    assert_eq!(relay_claims::with_api(relay_claims::sync), Ok(vec![HOST.to_string()]));
    let r = row();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_FAILING, Some(domains::RELAY_RELEASED)));
    assert!(r.last_error.as_deref().unwrap().contains("K2 Connect released it: TXT gone for 7 days"), "{:?}", r.last_error);
    assert!(!r.via_relay());
    assert!(crate::apps::custom_names::reload().tunnel.is_empty());

    // Fixed: a fresh claim brings it back.
    let (r, o) = check_with(HOST, &cname_here(&row()), &|_| {}).unwrap();
    assert!(o.ok, "{o:?}");
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_LIVE, Some(domains::RELAY_VERIFIED)));
    let _ = k2_core::tunnel::custom_names::set(Vec::new());
}

// ── a name missing from K2 Connect's list (one wrong answer stops nothing) ──

const OTHER: &str = "other.agentinitech.com";
const MIN: i64 = 60;

/// A live name K2 Connect routes, and a second held name so a list can be
/// non-empty without it.
fn live_pair() -> (Rc<FakeCp>, Seams) {
    let d = setup();
    set_box_label("initech");
    let (cp, s) = path_b_box();
    let pem = crate::domains::acme::test_le_pem(HOST);
    crate::domains::store::install(HOST, &pem.chain_pem, &pem.key_pem).unwrap();
    let (r, _) = check_with(HOST, &cname_here(&d), &|_| {}).unwrap();
    assert_eq!(r.state, STATE_LIVE);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::upsert_name(&conn, OTHER, "agentinitech.com", "app:agentinitech").unwrap();
        domains::set_relay_state(&conn, OTHER, Some(domains::RELAY_VERIFIED), None, 1).unwrap();
    }
    cp.calls.borrow_mut().clear();
    (cp, s)
}

fn listed(host: &str) -> ClaimAnswer {
    ClaimAnswer { hostname: host.into(), status: "verified".into(), verify_token: Some(PICKED.into()), last_error: None }
}

fn still_live() {
    let r = row();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_LIVE, Some(domains::RELAY_VERIFIED)));
}

/// A whole list that comes back empty is held: logged, nothing changes, as
/// long as the verify doesn't say `not_claimed`. Then the list is back and
/// nothing was counted, however often it happens.
#[test]
fn an_empty_list_once_then_back_changes_nothing() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let (cp, _s) = live_pair();
    let t0 = 1_000_000;
    cp.list.borrow_mut().clear();
    for i in 0..5 {
        assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + i * 60 * MIN), Ok(vec![]), "empty list {i}");
        still_live();
    }
    assert!(cp.calls.borrow().iter().any(|c| c.starts_with("verify ")), "it asked: {:?}", cp.calls.borrow());
    // A verify that fails for another reason doesn't confirm it either.
    *cp.verify.borrow_mut() = Some(Err(CpError::Refused { code: "pro_required".into(), words: "x".into() }));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 6 * 60 * MIN), Ok(vec![]));
    *cp.verify.borrow_mut() = Some(Err(CpError::Retry("x".into())));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 7 * 60 * MIN), Ok(vec![]));
    still_live();
    // Back: and still nothing against it.
    *cp.list.borrow_mut() = vec![listed(HOST), listed(OTHER)];
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 8 * 60 * MIN), Ok(vec![]));
    still_live();
}

/// An empty list the verify confirms (`not_claimed`) counts like any other,
/// and still takes three lists over 30 minutes.
#[test]
fn an_empty_list_a_verify_confirms_counts_but_still_takes_three() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let (cp, _s) = live_pair();
    let t0 = 1_000_000;
    cp.list.borrow_mut().clear();
    *cp.verify.borrow_mut() = Some(Err(CpError::Refused { code: "not_claimed".into(), words: "x".into() }));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0), Ok(vec![]));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 15 * MIN), Ok(vec![]));
    still_live();
    let released = relay_claims::sync_at(cp.as_ref(), t0 + 30 * MIN).unwrap();
    assert!(released.contains(&HOST.to_string()), "{released:?}");
    let r = row();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_FAILING, Some(domains::RELAY_RELEASED)));
}

/// Missing from three successful lists over at least 30 minutes: released,
/// with K2 Connect's words. Fewer, or sooner, or listed again in between: no.
/// A failed fetch neither counts nor resets.
#[test]
fn missing_three_times_over_half_an_hour_releases_and_nothing_less_does() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let (cp, _s) = live_pair();
    let t0 = 1_000_000;
    let only_other = vec![listed(OTHER)];
    *cp.list.borrow_mut() = only_other.clone();
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0), Ok(vec![]));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 60 * MIN), Ok(vec![]));
    still_live();
    // Listed again: the run starts over.
    *cp.list.borrow_mut() = vec![listed(HOST), listed(OTHER)];
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t0 + 120 * MIN), Ok(vec![]));
    *cp.list.borrow_mut() = only_other.clone();
    let t1 = t0 + 180 * MIN;
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t1), Ok(vec![]));
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t1 + 5 * MIN), Ok(vec![]));
    // A failed fetch: not a count, not a reset.
    *cp.list_down.borrow_mut() = true;
    assert!(relay_claims::sync_at(cp.as_ref(), t1 + 6 * MIN).is_err());
    *cp.list_down.borrow_mut() = false;
    // The third list, too soon: 10 minutes after the first. Still served.
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t1 + 10 * MIN), Ok(vec![]));
    still_live();
    // A later list, past 30 minutes: released.
    assert_eq!(relay_claims::sync_at(cp.as_ref(), t1 + 30 * MIN), Ok(vec![HOST.to_string()]));
    let r = row();
    assert_eq!((r.state.as_str(), r.relay_state.as_deref()), (STATE_FAILING, Some(domains::RELAY_RELEASED)));
    assert!(r.last_error.as_deref().unwrap().contains("no longer lists"), "{:?}", r.last_error);
    assert_eq!(relay_claims::MISSING_LISTS_TO_RELEASE, 3);
    assert_eq!(relay_claims::MISSING_MIN_SPAN_SECS, 30 * 60);
}
