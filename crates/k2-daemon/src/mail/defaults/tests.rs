//! Tests for the 0.45.1 field-fix reconcile. Fixtures only: a recording
//! fake registry, captured Stalwart object shapes, the checked-in
//! spam-filter release excerpts. Never a real Stalwart, never the network.

use super::*;
use crate::mail::cert_names::PointsHere;
use crate::mail::cert_owner::CertMgmtApi;
use std::cell::RefCell;

// ── Fixtures ────────────────────────────────────────────────────────────

/// The `mx` route as Stalwart 0.16.10 returns it (rehearsal 2026-10-08).
fn mx_route(strategy: &str) -> Value {
    serde_json::json!({
        "id": "jinysbhmadab",
        "name": "mx",
        "@type": "Mx",
        "ipLookupStrategy": strategy,
        "maxMultihomed": 2,
        "maxMxHosts": 2,
        "description": null,
    })
}

fn local_route() -> Value {
    serde_json::json!({ "id": "jinysbhmadqb", "name": "local", "@type": "Local" })
}

fn tls(id: &str, name: &str, dane: &str) -> Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "allowInvalidCerts": name == "invalid-tls",
        "dane": dane,
        "mtaSts": "optional",
        "startTls": "optional",
        "description": null,
    })
}

fn default_tls() -> Vec<Value> {
    vec![tls("jinysbhmacab", "invalid-tls", "optional"), tls("jinysbhmacqb", "default", "optional")]
}

fn excerpt(file: &str) -> Vec<Value> {
    let raw = match file {
        "v3.0.1" => include_str!("../fixtures/spam-filter-v3.0.1-dnsbl.json"),
        "v3.0.2" => include_str!("../fixtures/spam-filter-v3.0.2-dnsbl.json"),
        other => panic!("no excerpt {other}"),
    };
    let doc: Value = serde_json::from_str(raw).expect("excerpt JSON");
    doc["SpamDnsblServer"].as_array().expect("SpamDnsblServer list").clone()
}

/// The 3 DNSBL rules as the registry stores them: the release objects
/// plus a server-set id.
fn dnsbl_from(version: &str) -> Vec<Value> {
    excerpt(version)
        .into_iter()
        .enumerate()
        .map(|(i, mut o)| {
            o["id"] = Value::String(format!("jinysczmb{i}aa"));
            o
        })
        .collect()
}

fn v(s: &str) -> Ver {
    parse_version(s).expect("version")
}

// ── Recording fake registry ─────────────────────────────────────────────

struct Fake {
    cm: RefCell<CertManagement>,
    routes: RefCell<Vec<Value>>,
    tls: RefCell<Vec<Value>>,
    dnsbl: RefCell<Vec<Value>>,
    url: RefCell<Option<String>>,
    tasks: RefCell<Vec<AcmeTask>>,
    reload_err: Option<String>,
    set_err: Option<(String, String)>,
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            cm: RefCell::new(CertManagement::Automatic {
                acme_provider_id: "acme-p1".into(),
                subject_alternative_names: vec![],
            }),
            routes: RefCell::new(vec![mx_route("v4ThenV6"), local_route()]),
            tls: RefCell::new(default_tls()),
            dnsbl: RefCell::new(dnsbl_from("v3.0.1")),
            url: RefCell::new(Some(RULES_URL_LATEST.into())),
            tasks: RefCell::new(vec![]),
            reload_err: None,
            set_err: None,
            calls: RefCell::new(vec![]),
        }
    }
    fn log(&self, s: String) {
        self.calls.borrow_mut().push(s);
    }
    fn writes(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| c.starts_with("set") || c.starts_with("reload") || c.starts_with("task"))
            .cloned()
            .collect()
    }
    fn fail(&self, what: &str, id: &str) -> Result<(), String> {
        match &self.set_err {
            Some((w, i)) if w == what && i == id => Err(format!("{what} update rejected — invalidPatch")),
            _ => Ok(()),
        }
    }
}

fn replace(list: &RefCell<Vec<Value>>, id: &str, body: &Value) {
    for o in list.borrow_mut().iter_mut() {
        if o["id"] == id {
            let name = o["name"].clone();
            *o = body.clone();
            o["id"] = Value::String(id.into());
            o["name"] = name;
        }
    }
}

impl CertMgmtApi for Fake {
    fn mail_domain_id(&self, host: &str) -> Result<Option<String>, String> {
        self.log(format!("id {host}"));
        Ok(Some("dom-1".into()))
    }
    fn get(&self, _id: &str) -> Result<CertManagement, String> {
        self.log("get-cm".into());
        Ok(self.cm.borrow().clone())
    }
    fn set_manual(&self, id: &str) -> Result<(), String> {
        self.log(format!("set-manual {id}"));
        Ok(())
    }
    fn set_locked(&self, id: &str, provider: &str, host: &str) -> Result<(), String> {
        self.log(format!("set-locked {id} {provider} {host}"));
        *self.cm.borrow_mut() = CertManagement::Automatic {
            acme_provider_id: provider.into(),
            subject_alternative_names: vec![host.into()],
        };
        Ok(())
    }
    fn acme_providers(&self) -> Result<Vec<String>, String> {
        Ok(vec!["acme-p1".into()])
    }
}

impl DefaultsApi for Fake {
    fn mta_routes(&self) -> Result<Vec<Value>, String> {
        self.log("get-routes".into());
        Ok(self.routes.borrow().clone())
    }
    fn mta_route_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.log(format!("set-route {id} {obj}"));
        self.fail("route", id)?;
        replace(&self.routes, id, &obj);
        Ok(())
    }
    fn tls_strategies(&self) -> Result<Vec<Value>, String> {
        self.log("get-tls".into());
        Ok(self.tls.borrow().clone())
    }
    fn tls_strategy_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.log(format!("set-tls {id} {}", obj["dane"]));
        self.fail("tls", id)?;
        replace(&self.tls, id, &obj);
        Ok(())
    }
    fn dnsbl_servers(&self) -> Result<Vec<Value>, String> {
        self.log("get-dnsbl".into());
        Ok(self.dnsbl.borrow().clone())
    }
    fn dnsbl_server_set(&self, id: &str, obj: Value) -> Result<(), String> {
        self.log(format!("set-dnsbl {id}"));
        self.fail("dnsbl", id)?;
        replace(&self.dnsbl, id, &obj);
        Ok(())
    }
    fn spam_rules_url(&self) -> Result<Option<String>, String> {
        self.log("get-url".into());
        Ok(self.url.borrow().clone())
    }
    fn set_spam_rules_url(&self, url: &str) -> Result<(), String> {
        self.log(format!("set-url {url}"));
        *self.url.borrow_mut() = Some(url.into());
        Ok(())
    }
    fn reload_settings(&self) -> Result<(), String> {
        self.log("reload".into());
        match &self.reload_err {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
    fn acme_tasks(&self) -> Result<Vec<AcmeTask>, String> {
        self.log("get-tasks".into());
        Ok(self.tasks.borrow().clone())
    }
    fn queue_acme_renewal(&self, domain_id: &str) -> Result<String, String> {
        self.log(format!("task-create {domain_id}"));
        Ok("task-k2-1".into())
    }
}

fn task(id: &str, domain: &str, state: &str) -> AcmeTask {
    AcmeTask {
        id: id.into(),
        domain_id: domain.into(),
        state: state.into(),
        reason: (state != "Pending").then(|| "connection refused".to_string()),
        at: Some("2026-10-07T20:00:00Z".into()),
        due: None,
    }
}

struct Inputs {
    owner: CertOwner,
    plan: Option<&'static str>,
    served: &'static str,
    points: PointsHere,
}

impl Inputs {
    fn stalwart() -> Self {
        Inputs { owner: CertOwner::StalwartAcme, plan: Some("tls-alpn"), served: "self-signed", points: PointsHere::Yes }
    }
}

fn run(api: &Fake, inp: &Inputs, installed: &str, markers: Option<Markers>, restart_ok: bool) -> (Outcome, usize) {
    let served = || inp.served.to_string();
    let points = || inp.points.clone();
    let ctx = Ctx {
        cert: CertInputs {
            host: "mail.example.com",
            owner: inp.owner,
            port_plan: inp.plan,
            served_state: &served,
            points_here: &points,
        },
        installed: Some(installed),
        now: 1_760_000_000,
    };
    let mut restarts = 0usize;
    let mut restart = || {
        restarts += 1;
        if restart_ok {
            Ok(())
        } else {
            Err("no door (mail helper missing)".to_string())
        }
    };
    let out = reconcile(api, &ctx, markers, &mut restart);
    (out, restarts)
}

// ── Versions + markers ──────────────────────────────────────────────────

#[test]
fn versions_parse_and_order() {
    assert_eq!(parse_version("0.16.10"), Some((0, 16, 10)));
    assert_eq!(parse_version("v0.16.20"), Some((0, 16, 20)));
    assert_eq!(parse_version("0.16.25-k2.1"), Some((0, 16, 25)));
    assert_eq!(parse_version("0.16"), None);
    assert_eq!(parse_version("garbage"), None);
    assert!(v("0.16.10") < V_DANE_SELF_OFF && v("0.16.20") >= V_DANE_SELF_OFF);
    assert!(v("0.16.22") < V_BIT_AND && v("0.16.23") >= V_BIT_AND);
}

#[test]
fn markers_round_trip_and_garbage_is_loud() {
    assert_eq!(parse_markers(None).expect("null"), Markers::default());
    assert_eq!(parse_markers(Some("  ")).expect("empty"), Markers::default());
    let err = parse_markers(Some("{not json")).expect_err("garbage");
    assert!(err.contains("outbound_json"), "{err}");
    let m = Markers {
        mx_route: Some(RouteMark { id: "r".into(), from: "v4ThenV6".into(), to: Some("v4Only".into()), at: 5 }),
        dane: vec![DaneMark { id: "t".into(), name: "default".into(), from: "optional".into(), at: 5 }],
        ..Default::default()
    };
    let json = serde_json::to_string(&m).expect("ser");
    assert!(json.contains("\"mxRoute\"") && json.contains("\"dane\""), "{json}");
    assert!(!json.contains("spamUrl") && !json.contains("pendingReload"), "only what changed: {json}");
    assert_eq!(parse_markers(Some(&json)).expect("back"), m);
}

// ── x:Task/get parser ───────────────────────────────────────────────────

#[test]
fn acme_task_parser_reads_pending_retry_failed_and_refuses_unknowns() {
    let reply = serde_json::json!({
        "list": [
            { "id": "t1", "@type": "AcmeRenewal", "domainId": "dom-1",
              "status": { "@type": "Pending", "createdAt": "2026-10-07T10:00:00Z", "due": "2026-10-07T10:00:00Z" } },
            { "id": "t2", "@type": "AcmeRenewal", "domainId": "dom-1",
              "status": { "@type": "Retry", "createdAt": "2026-10-07T11:00:00Z", "due": "2026-10-07T11:02:08Z",
                          "attemptNumber": 2, "failureReason": "tls-alpn-01: connection refused" } },
            { "id": "t3", "@type": "AcmeRenewal", "domainId": "dom-2",
              "status": { "@type": "Failed", "createdAt": "2026-10-06T09:00:00Z", "failedAt": "2026-10-06T09:05:00Z",
                          "failedAttemptNumber": 3, "failureReason": "rateLimited" } },
        ],
        "notFound": [],
    });
    let t = parse_acme_tasks(&reply).expect("parse");
    assert_eq!(t.len(), 3);
    assert_eq!((t[0].state.as_str(), t[0].reason.as_deref()), ("Pending", None));
    assert_eq!(t[0].due.as_deref(), Some("2026-10-07T10:00:00Z"));
    assert_eq!((t[1].state.as_str(), t[1].reason.as_deref()), ("Retry", Some("tls-alpn-01: connection refused")));
    assert!(t[1].is_live() && t[0].is_live() && !t[2].is_live());
    assert_eq!(t[2].at.as_deref(), Some("2026-10-06T09:05:00Z"), "Failed reports failedAt");
    assert_eq!(t[2].domain_id, "dom-2");

    let unknown = serde_json::json!({ "list": [
        { "id": "t9", "@type": "AcmeRenewal", "domainId": "d", "status": { "@type": "Running" } } ] });
    assert!(parse_acme_tasks(&unknown).expect_err("unknown status").contains("Running"));
    // 0.16.10 can't filter by type: other task types are skipped.
    let other = serde_json::json!({ "list": [
        { "id": "t8", "@type": "DkimManagement", "domainId": "d", "status": { "@type": "Pending" } } ] });
    assert!(parse_acme_tasks(&other).expect("skip").is_empty());
    let untyped = serde_json::json!({ "list": [ { "id": "t7", "status": { "@type": "Pending" } } ] });
    assert!(parse_acme_tasks(&untyped).expect_err("no @type").contains("t7"));
    assert!(parse_acme_tasks(&serde_json::json!({})).is_err(), "no list is loud");
    // The exact Failed task a scratch Stalwart 0.16.10 returned (2026-10-08).
    let live = serde_json::json!({ "list": [{ "domainId": "b", "status": { "createdAt": "2026-10-08T05:52:13Z",
        "failedAt": "2026-10-08T05:52:13Z", "failedAttemptNumber": 0,
        "failureReason": "Invalid request: ACME not configured for domain", "@type": "Failed" },
        "@type": "AcmeRenewal", "id": "jiotgfghkrqa" }] });
    let t = parse_acme_tasks(&live).expect("live shape");
    assert_eq!(t[0].reason.as_deref(), Some("Invalid request: ACME not configured for domain"));
    assert_eq!((t[0].state.as_str(), t[0].domain_id.as_str()), ("Failed", "b"));
}

// ── A4 ──────────────────────────────────────────────────────────────────

/// The vendored v3.0.1 bodies are byte-equal (as JSON) to the checked-in
/// release excerpt — no download in tests.
#[test]
fn vendored_v301_tags_match_the_release_excerpt() {
    let objs = excerpt("v3.0.1");
    assert_eq!(objs.len(), 3);
    for o in &objs {
        let name = o["name"].as_str().expect("name");
        assert!(REPAIR_NAMES.contains(&name), "{name}");
        assert_eq!(v301_tag(name).expect("vendored"), o["tag"], "{name}");
    }
    assert!(v301_tag("STWT_OTHER").is_none());
    // And v3.0.2 differs only in `tag` (HF1).
    for (a, b) in excerpt("v3.0.1").iter().zip(excerpt("v3.0.2").iter()) {
        assert_eq!(a["name"], b["name"]);
        assert_eq!(a["zone"], b["zone"]);
        assert_eq!(a["@type"], b["@type"]);
        assert_eq!(a["enable"], b["enable"]);
        assert_ne!(a["tag"], b["tag"]);
    }
}

#[test]
fn function_scanner_flags_bit_and_but_not_known_functions_or_literals() {
    assert_eq!(called_functions("bit_and(octets[3], 2) != 0"), vec!["bit_and"]);
    assert_eq!(called_functions("is_ip_in_cidr(ip, '10.0.0.0/8')"), vec!["is_ip_in_cidr"]);
    assert!(called_functions("'bit_and(' + value").is_empty(), "inside quotes");
    assert!(called_functions("\"bit_and(x)\"").is_empty(), "double quotes");
    assert!(called_functions("octets[3] == 16").is_empty());
    assert_eq!(called_functions("key_exists ('surbl-hashbl', host)"), vec!["key_exists"]);
    let known = known_functions(v("0.16.20"));
    let flagged = |e: &str| {
        let obj = serde_json::json!({ "tag": { "else": "false", "match": { "0": { "if": e, "then": "'X'" } } } });
        unknown_functions(&obj, &known)
    };
    assert_eq!(flagged("bit_and(x, 2)"), vec!["bit_and"]);
    assert!(flagged("is_ip_in_cidr(ip, '1.2.3.0/24')").is_empty());
    assert!(flagged("'bit_and(' == value").is_empty());
    // A list-shaped `match` is read too.
    let arr = serde_json::json!({ "else": "false", "match": [ { "if": "bit_and(a, 1)", "then": "'Y'" } ] });
    assert_eq!(unknown_functions(&arr, &known), vec!["bit_and"]);
    // From 0.16.23 bit_and is known.
    assert!(unknown_functions(&arr, &known_functions(v("0.16.23"))).is_empty());
}

/// K2's function list for the 0.16.x pin = the copy taken from Stalwart
/// 0.16.20 (`FUNCTIONS` + `ASYNC_FUNCTIONS`), same length, same names.
#[test]
fn function_list_matches_stalwart_0_16_20() {
    let copy: Vec<&str> = include_str!("../fixtures/stalwart-0.16.20-expr-functions.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(copy.len(), STALWART_0_16_FUNCTIONS.len());
    assert_eq!(copy, STALWART_0_16_FUNCTIONS);
    assert!(!copy.contains(&"bit_and"));
}

#[test]
fn spam_repair_rewrites_exactly_the_three_bit_and_rules() {
    let servers = dnsbl_from("v3.0.2");
    let plan = plan_spam_repair(&servers, v("0.16.10"));
    assert_eq!(plan.len(), 3, "{plan:?}");
    for (w, s) in plan.iter().zip(servers.iter()) {
        assert_eq!(w.id, s["id"].as_str().expect("id"));
        assert_eq!(w.body["tag"], v301_tag(&w.name).expect("tag"));
        assert!(w.body.get("id").is_none() && w.body.get("name").is_none(), "read-only id/name left out");
        assert_eq!(w.body["@type"], s["@type"], "@type rides along (HF3)");
        assert_eq!(w.body["zone"], s["zone"], "zone untouched");
        assert_eq!(w.body["enable"], s["enable"]);
    }
    assert_eq!(plan_spam_repair(&servers, v("0.16.20")).len(), 3, "0.16.20 lacks bit_and too");
    assert!(plan_spam_repair(&servers, v("0.16.23")).is_empty(), "never revert a box that has bit_and");
    assert!(plan_spam_repair(&dnsbl_from("v3.0.1"), v("0.16.10")).is_empty(), "already v3.0.1");
    // An unknown rule using bit_and is never written (the doctor names it).
    let mut odd = servers[0].clone();
    odd["name"] = "STWT_SOMETHING_NEW".into();
    odd["id"] = "x1".into();
    assert!(plan_spam_repair(&[odd.clone()], v("0.16.10")).is_empty());
    let check = spam_rules_load_check(&Ok(vec![("SpamDnsblServer".into(), odd)]), Some(v("0.16.10")));
    assert_eq!(check.status, ST_WARN);
    assert!(check.detail.contains("STWT_SOMETHING_NEW") && check.detail.contains("bit_and"), "{}", check.detail);
}

#[test]
fn spam_url_is_pinned_only_from_stalwarts_latest_default_and_once() {
    assert_eq!(plan_spam_url(Some(RULES_URL_LATEST), None), Some(RULES_URL_PINNED));
    assert_eq!(plan_spam_url(Some("https://example.com/rules.json.gz"), None), None, "a person set it");
    assert_eq!(plan_spam_url(None, None), None, "unset = no downloads; leave it");
    assert_eq!(plan_spam_url(Some(RULES_URL_PINNED), None), None);
    let mark = UrlMark { from: RULES_URL_LATEST.into(), to: RULES_URL_PINNED.into(), at: 1 };
    assert_eq!(plan_spam_url(Some(RULES_URL_LATEST), Some(&mark)), None, "K2 pinned once already");
}

#[test]
fn spam_rules_load_check_passes_clean_rules() {
    let objs: Vec<(String, Value)> = dnsbl_from("v3.0.1").into_iter().map(|o| ("SpamDnsblServer".to_string(), o)).collect();
    let c = spam_rules_load_check(&Ok(objs), Some(v("0.16.20")));
    assert_eq!(c.status, ST_PASS, "{}", c.detail);
    let bad: Vec<(String, Value)> = dnsbl_from("v3.0.2").into_iter().map(|o| ("SpamDnsblServer".to_string(), o)).collect();
    let c = spam_rules_load_check(&Ok(bad.clone()), Some(v("0.16.20")));
    assert_eq!(c.status, ST_WARN);
    for n in REPAIR_NAMES {
        assert!(c.detail.contains(n), "{}", c.detail);
    }
    assert_eq!(spam_rules_load_check(&Ok(bad), Some(v("0.16.23"))).status, ST_PASS);
    assert_eq!(spam_rules_load_check(&Err("down".into()), Some(v("0.16.20"))).status, ST_UNKNOWN);
    assert_eq!(spam_rules_load_check(&Ok(vec![]), None).status, ST_UNKNOWN);
}

// ── A1 ──────────────────────────────────────────────────────────────────

#[test]
fn mx_route_default_gets_v4only_once_as_a_whole_object() {
    let routes = vec![mx_route("v4ThenV6"), local_route()];
    let RoutePlan::Write { id, body, from } = plan_mx_route(&routes, None) else {
        panic!("expected a write");
    };
    assert_eq!((id.as_str(), from.as_str()), ("jinysbhmadab", "v4ThenV6"));
    assert_eq!(
        body,
        serde_json::json!({
            "@type": "Mx",
            "ipLookupStrategy": "v4Only",
            "maxMultihomed": 2,
            "maxMxHosts": 2,
            "description": null,
        }),
        "whole object minus id + name; enum value case-exact"
    );
    // A person's choice is recorded, never written.
    for s in ["v6ThenV4", "v4Only", "v6Only"] {
        assert_eq!(
            plan_mx_route(&[mx_route(s)], None),
            RoutePlan::Record { id: "jinysbhmadab".into(), from: s.into() }
        );
    }
    // Decided once: a later v4ThenV6 (a person's revert) is left alone.
    let mark = RouteMark { id: "jinysbhmadab".into(), from: "v4ThenV6".into(), to: Some("v4Only".into()), at: 1 };
    assert!(matches!(plan_mx_route(&routes, Some(&mark)), RoutePlan::Skip(_)));
    // A fresh Stalwart (new route id) is decided again.
    let other = RouteMark { id: "gone".into(), ..mark };
    assert!(matches!(plan_mx_route(&routes, Some(&other)), RoutePlan::Write { .. }));
    assert!(matches!(plan_mx_route(&[local_route()], None), RoutePlan::Skip(_)));
}

/// {0.16.10, 0.16.20} × {optional, disable by hand, disable by K2} →
/// the exact writes. A hand-set `disable` is never flipped.
#[test]
fn dane_rule_table() {
    let k2_mark = |id: &str, name: &str| DaneMark { id: id.into(), name: name.into(), from: "optional".into(), at: 1 };
    let writes = |p: &DanePlan| p.writes.iter().map(|w| format!("{} {}", w.name, w.to)).collect::<Vec<_>>();

    // 0.16.10 + optional → disable, marked.
    let p = plan_dane(&default_tls(), &[], v("0.16.10"), 9);
    assert_eq!(writes(&p), vec!["invalid-tls disable", "default disable"]);
    assert_eq!(p.marks.len(), 2);
    assert!(p.writes.iter().all(|w| w.body.get("id").is_none() && w.body.get("name").is_none()));
    assert_eq!(p.writes[1].body["startTls"], "optional", "the rest of the object is written back");
    // 0.16.10 + disable by hand → nothing, no marker (noir).
    let hand = vec![tls("a", "invalid-tls", "disable"), tls("b", "default", "disable")];
    let p = plan_dane(&hand, &[], v("0.16.10"), 9);
    assert!(p.writes.is_empty() && p.marks.is_empty());
    // 0.16.10 + disable by K2 → nothing, marker kept.
    let p = plan_dane(&hand, &[k2_mark("a", "invalid-tls"), k2_mark("b", "default")], v("0.16.10"), 9);
    assert!(p.writes.is_empty());
    assert_eq!(p.marks.len(), 2);
    // 0.16.10 + K2 marked but a person turned it back on → respected.
    let p = plan_dane(&default_tls(), &[k2_mark("jinysbhmacqb", "default")], v("0.16.10"), 9);
    assert_eq!(writes(&p), vec!["invalid-tls disable"]);

    // 0.16.20 + optional → nothing.
    let p = plan_dane(&default_tls(), &[], v("0.16.20"), 9);
    assert!(p.writes.is_empty() && p.marks.is_empty());
    // 0.16.20 + disable by hand → never flipped (noir after its upgrade).
    let p = plan_dane(&hand, &[], v("0.16.20"), 9);
    assert!(p.writes.is_empty());
    // 0.16.20 + disable by K2 → back to optional, markers gone.
    let p = plan_dane(&hand, &[k2_mark("a", "invalid-tls"), k2_mark("b", "default")], v("0.16.20"), 9);
    assert_eq!(writes(&p), vec!["invalid-tls optional", "default optional"]);
    assert!(p.marks.is_empty());
    // Mixed: K2 restores only its own.
    let p = plan_dane(&hand, &[k2_mark("b", "default")], v("0.16.20"), 9);
    assert_eq!(writes(&p), vec!["default optional"]);
    // A marker for a strategy that is gone is dropped.
    let p = plan_dane(&default_tls(), &[k2_mark("gone", "x")], v("0.16.10"), 9);
    assert!(p.marks.iter().all(|m| m.id != "gone"));
}

// ── A2 ──────────────────────────────────────────────────────────────────

fn cert(api: &Fake, inp: &Inputs) -> CertStep {
    let served = || inp.served.to_string();
    let points = || inp.points.clone();
    cert_step(
        api,
        &CertInputs { host: "Mail.Example.com.", owner: inp.owner, port_plan: inp.plan, served_state: &served, points_here: &points },
    )
    .expect("cert step")
}

#[test]
fn cert_lock_cases() {
    // stalwart-acme + empty list → one lock write (+ the guarded retry).
    let api = Fake::new();
    let out = cert(&api, &Inputs::stalwart());
    let CertStep::Locked { previous, retry } = out else { panic!("{out:?}") };
    assert!(previous.is_empty());
    assert_eq!(retry, RetryDecision::Queued { domain_id: "dom-1".into(), task_id: "task-k2-1".into() });
    assert_eq!(api.writes(), vec!["set-locked dom-1 acme-p1 mail.example.com", "task-create dom-1"]);
    // Locked → no write.
    assert_eq!(cert(&api, &Inputs::stalwart()), CertStep::AlreadyLocked);
    assert_eq!(api.writes().len(), 2);
    // Manual → no write.
    let manual = Fake::new();
    *manual.cm.borrow_mut() = CertManagement::Manual;
    assert_eq!(cert(&manual, &Inputs::stalwart()), CertStep::NotAutomatic);
    assert!(manual.writes().is_empty());
    // K2 owner → the Manual path is cert_owner's; not even a read here.
    let k2 = Fake::new();
    assert!(matches!(cert(&k2, &Inputs { owner: CertOwner::K2, ..Inputs::stalwart() }), CertStep::Skipped(_)));
    assert!(k2.calls.borrow().is_empty());
    let unknown = Fake::new();
    assert!(matches!(cert(&unknown, &Inputs { owner: CertOwner::Unknown, ..Inputs::stalwart() }), CertStep::Skipped(_)));
    // Not tls-alpn → Stalwart ACME doesn't run.
    let http = Fake::new();
    assert!(matches!(cert(&http, &Inputs { plan: Some("http-01"), ..Inputs::stalwart() }), CertStep::Skipped(_)));
    assert!(http.calls.borrow().is_empty());
}

/// §10 sign-off (Seoca): a valid certificate + a fresh lock → ZERO
/// AcmeRenewal tasks. The lock only narrows the next scheduled renewal.
#[test]
fn valid_cert_with_a_fresh_lock_queues_zero_acme_orders() {
    let api = Fake::new();
    let out = cert(&api, &Inputs { served: "issued", ..Inputs::stalwart() });
    let CertStep::Locked { retry: RetryDecision::NotQueued(why), .. } = out else { panic!("{out:?}") };
    assert!(why.contains("never re-ordered"), "{why}");
    assert!(!api.calls.borrow().iter().any(|c| c.starts_with("task") || c == "get-tasks"), "{:?}", api.calls.borrow());
    assert_eq!(api.writes(), vec!["set-locked dom-1 acme-p1 mail.example.com"]);
}

#[test]
fn acme_retry_guards() {
    // A record elsewhere → no task.
    let api = Fake::new();
    let out = cert(&api, &Inputs { points: PointsHere::No("mail.example.com A 198.51.100.9 is not this box".into()), ..Inputs::stalwart() });
    let CertStep::Locked { retry: RetryDecision::NotQueued(why), .. } = out else { panic!("{out:?}") };
    assert!(why.contains("add the A record") && why.contains("k2 hostmail cert renew"), "{why}");
    assert!(!api.writes().iter().any(|w| w.starts_with("task")));
    // DNS unknown → no task.
    let api = Fake::new();
    let out = cert(&api, &Inputs { points: PointsHere::Unknown("timeout".into()), ..Inputs::stalwart() });
    assert!(matches!(out, CertStep::Locked { retry: RetryDecision::NotQueued(_), .. }));
    assert!(!api.writes().iter().any(|w| w.starts_with("task")));
    // An existing Retry (or Pending) task for this Domain → no task.
    for state in ["Retry", "Pending"] {
        let api = Fake::new();
        api.tasks.borrow_mut().push(task("t-old", "dom-1", state));
        let out = cert(&api, &Inputs::stalwart());
        let CertStep::Locked { retry: RetryDecision::NotQueued(why), .. } = out else { panic!("{out:?}") };
        assert!(why.contains("t-old"), "{why}");
        assert!(!api.writes().iter().any(|w| w.starts_with("task")), "{state}");
    }
    // A Failed task, or a live task for ANOTHER Domain, doesn't block.
    let api = Fake::new();
    api.tasks.borrow_mut().push(task("t-failed", "dom-1", "Failed"));
    api.tasks.borrow_mut().push(task("t-other", "dom-9", "Retry"));
    let out = cert(&api, &Inputs::stalwart());
    assert!(matches!(out, CertStep::Locked { retry: RetryDecision::Queued { .. }, .. }), "{out:?}");
    // Missing cert counts like self-signed.
    let api = Fake::new();
    let out = cert(&api, &Inputs { served: "missing", ..Inputs::stalwart() });
    assert!(matches!(out, CertStep::Locked { retry: RetryDecision::Queued { .. }, .. }));
}

// ── The reconcile ───────────────────────────────────────────────────────

/// quillify: 0.16.10, v3.0.2 rules, defaults everywhere. A4 lands before
/// A1, then exactly one reload.
#[test]
fn reconcile_heals_a_quillify_shaped_box_in_order_with_one_reload() {
    let api = Fake::new();
    *api.dnsbl.borrow_mut() = dnsbl_from("v3.0.2");
    let (out, restarts) = run(&api, &Inputs::stalwart(), "0.16.10", Some(Markers::default()), true);
    assert_eq!(restarts, 0);
    let w = api.writes();
    let pos = |p: &str| w.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("{p} missing: {w:?}"));
    assert!(pos("set-locked") < pos("set-url"));
    assert!(pos("set-url") < pos("set-dnsbl"), "A4 before A1");
    assert!(pos("set-dnsbl") < pos("set-route"), "A4 before A1");
    assert!(pos("set-route") < pos("set-tls"));
    assert_eq!(w.iter().filter(|c| *c == "reload").count(), 1, "{w:?}");
    assert_eq!(w.last().map(String::as_str), Some("reload"));
    assert_eq!(w.iter().filter(|c| c.starts_with("set-dnsbl")).count(), 3);
    assert_eq!(w.iter().filter(|c| c.starts_with("set-tls")).count(), 2);
    assert!(w.iter().any(|c| c.starts_with("set-route jinysbhmadab") && c.contains("\"ipLookupStrategy\":\"v4Only\"")));
    assert_eq!(out.settings_written, 1 + 3 + 1 + 2);
    let m = &out.markers;
    assert_eq!(m.mx_route.as_ref().map(|r| r.to.as_deref()), Some(Some("v4Only")));
    assert_eq!(m.dane.len(), 2);
    assert_eq!(m.spam_repair.len(), 3);
    assert_eq!(m.spam_url.as_ref().map(|u| u.to.as_str()), Some(RULES_URL_PINNED));
    assert_eq!(m.acme_retry.as_ref().map(|a| a.task_id.as_str()), Some("task-k2-1"));
    assert!(m.pending_reload.is_none());

    // Second boot: nothing to do, no reload.
    api.calls.borrow_mut().clear();
    let (out2, _) = run(&api, &Inputs::stalwart(), "0.16.10", Some(out.markers.clone()), true);
    assert!(api.writes().is_empty(), "{:?}", api.writes());
    assert_eq!(out2.settings_written, 0);
    assert_eq!(out2.markers, out.markers);
}

/// noir after Sterling's hand fix: v4Only + DANE disable by hand, v3.0.1
/// rules. No A1/A4 writes; it stays off after the 0.16.20 upgrade.
#[test]
fn reconcile_never_undoes_a_hand_fix() {
    let api = Fake::new();
    *api.routes.borrow_mut() = vec![mx_route("v4Only")];
    *api.tls.borrow_mut() = vec![tls("a", "invalid-tls", "disable"), tls("b", "default", "disable")];
    *api.url.borrow_mut() = Some("https://example.com/my-rules.json.gz".into());
    let (out, _) = run(&api, &Inputs { served: "issued", ..Inputs::stalwart() }, "0.16.10", Some(Markers::default()), true);
    assert_eq!(api.writes(), vec!["set-locked dom-1 acme-p1 mail.example.com"], "lock only");
    assert_eq!(out.markers.mx_route.as_ref().map(|r| (r.from.as_str(), r.to.clone())), Some(("v4Only", None)));
    assert!(out.markers.dane.is_empty(), "hand-set = no K2 marker");
    // Upgrade to 0.16.20: still nothing touched.
    api.calls.borrow_mut().clear();
    let (_, _) = run(&api, &Inputs { served: "issued", ..Inputs::stalwart() }, "0.16.20", Some(out.markers), true);
    assert!(api.writes().is_empty(), "{:?}", api.writes());
    let st = dane_disabled_check(&Ok(api.tls.borrow().clone()), &Markers::default(), Some(v("0.16.20")));
    assert_eq!(st.status, ST_WARN, "HF19: stays visible");
    assert!(st.detail.contains("k2 hostmail outbound dane on"), "{}", st.detail);
}

/// After the 0.16.20 upgrade K2 puts back only what it changed.
#[test]
fn reconcile_restores_k2s_dane_after_the_upgrade() {
    let api = Fake::new();
    let (out, _) = run(&api, &Inputs { owner: CertOwner::K2, ..Inputs::stalwart() }, "0.16.10", Some(Markers::default()), true);
    assert_eq!(out.markers.dane.len(), 2);
    api.calls.borrow_mut().clear();
    let (out2, _) = run(&api, &Inputs { owner: CertOwner::K2, ..Inputs::stalwart() }, "0.16.20", Some(out.markers), true);
    assert_eq!(
        api.writes(),
        vec!["set-tls jinysbhmacab \"optional\"", "set-tls jinysbhmacqb \"optional\"", "reload"]
    );
    assert!(out2.markers.dane.is_empty());
    assert!(out2.markers.mx_route.is_some(), "the route decision stays recorded");
}

/// ReloadSettings refused (a bit_and rule K2 can't repair) → one restart;
/// no restart door → saved, `pendingReload`, retried next boot.
#[test]
fn refused_reload_falls_back_to_one_restart_then_pending() {
    let api = Fake { reload_err: Some("x:Action/set create rejected — validationFailed: SpamDnsblServer bit_and".into()), ..Fake::new() };
    let k2 = Inputs { owner: CertOwner::K2, ..Inputs::stalwart() };
    let (out, restarts) = run(&api, &k2, "0.16.20", Some(Markers::default()), true);
    assert_eq!(restarts, 1);
    assert!(out.markers.pending_reload.is_none());
    assert!(out.lines.iter().any(|l| l.contains("restarted it once")), "{:?}", out.lines);

    let api = Fake { reload_err: Some("validationFailed".into()), ..Fake::new() };
    let (out, restarts) = run(&api, &k2, "0.16.20", Some(Markers::default()), false);
    assert_eq!(restarts, 1);
    let p = out.markers.pending_reload.clone().expect("pending");
    assert!(p.error.contains("validationFailed") && p.error.contains("no door"), "{}", p.error);
    // Next boot with nothing new to write still retries the reload.
    let api2 = Fake::new();
    *api2.url.borrow_mut() = Some(RULES_URL_PINNED.into());
    *api2.routes.borrow_mut() = vec![mx_route("v4Only")];
    let (out2, _) = run(&api2, &k2, "0.16.20", Some(out.markers), true);
    assert_eq!(api2.writes(), vec!["reload"]);
    assert!(out2.markers.pending_reload.is_none());
}

/// One failing step never blocks the others; a failed write records
/// nothing (so it's retried).
#[test]
fn a_failed_step_is_isolated_and_unrecorded() {
    let api = Fake { set_err: Some(("route".into(), "jinysbhmadab".into())), ..Fake::new() };
    let (out, _) = run(&api, &Inputs { owner: CertOwner::K2, ..Inputs::stalwart() }, "0.16.10", Some(Markers::default()), true);
    assert!(out.markers.mx_route.is_none(), "failed route write is not recorded");
    assert_eq!(out.markers.dane.len(), 2, "DANE still ran");
    assert!(out.lines.iter().any(|l| l.contains("mx route: FAILED")), "{:?}", out.lines);
    let api = Fake { set_err: Some(("tls".into(), "jinysbhmacqb".into())), ..Fake::new() };
    let (out, _) = run(&api, &Inputs { owner: CertOwner::K2, ..Inputs::stalwart() }, "0.16.10", Some(Markers::default()), true);
    assert_eq!(out.markers.dane.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["jinysbhmacab"]);
}

/// Unreadable outbound_json: the once-only steps are skipped, never
/// redone blind; the repair and the cert lock still run.
#[test]
fn unreadable_markers_skip_the_once_only_steps() {
    let api = Fake::new();
    *api.dnsbl.borrow_mut() = dnsbl_from("v3.0.2");
    let (out, _) = run(&api, &Inputs::stalwart(), "0.16.10", None, true);
    let w = api.writes();
    assert!(!w.iter().any(|c| c.starts_with("set-route") || c.starts_with("set-tls") || c.starts_with("set-url")), "{w:?}");
    assert_eq!(w.iter().filter(|c| c.starts_with("set-dnsbl")).count(), 3);
    assert!(w.iter().any(|c| c.starts_with("set-locked")));
    assert!(out.lines[0].contains("unreadable"), "{:?}", out.lines);
}

#[test]
fn reconcile_gate_runs_on_error_but_not_disabled_or_installing() {
    assert!(gate(None, true).is_some());
    for s in ["running", "degraded", "stopped", "error"] {
        assert_eq!(gate(Some(s), true), None, "{s}");
    }
    for s in ["disabled", "installing"] {
        assert!(gate(Some(s), true).is_some(), "{s}");
    }
    assert!(gate(Some("running"), false).is_some());
}

// ── Status + doctor (pure) ──────────────────────────────────────────────

#[test]
fn acme_status_expands_the_empty_list_and_picks_the_last_task() {
    let empty = CertManagement::Automatic { acme_provider_id: "p".into(), subject_alternative_names: vec![] };
    let tasks = vec![task("t1", "dom-1", "Failed"), Task2::retry("t2", "dom-1", "2026-10-07T22:00:00Z")];
    let v = acme_status("mail.example.com", &Ok(Some(("dom-1".into(), "example.com".into(), empty))), &Ok(tasks));
    assert_eq!(v["mode"], "Automatic");
    assert_eq!(v["locked"], false);
    assert_eq!(
        v["orderNames"],
        serde_json::json!([
            "autoconfig.example.com", "autodiscover.example.com", "mail.example.com",
            "mta-sts.example.com", "ua-auto-config.example.com"
        ])
    );
    assert_eq!(v["lastTask"]["id"], "t2", "newest wins");
    let locked = CertManagement::Automatic { acme_provider_id: "p".into(), subject_alternative_names: vec!["mail.example.com".into()] };
    let v = acme_status("mail.example.com", &Ok(Some(("dom-1".into(), "example.com".into(), locked))), &Ok(vec![]));
    assert_eq!(v["orderNames"], serde_json::json!(["mail.example.com"]));
    assert_eq!(v["locked"], true);
    assert!(v["lastTask"].is_null());
    let v = acme_status("mail.example.com", &Ok(Some(("dom-1".into(), "example.com".into(), CertManagement::Manual))), &Err("down".into()));
    assert_eq!(v["mode"], "Manual");
    assert_eq!(v["lastTaskError"], "down");
    assert_eq!(order_names(&["autoconfig".into()], "example.com", "mail.example.com"), vec!["autoconfig.example.com"]);
}

struct Task2;
impl Task2 {
    fn retry(id: &str, dom: &str, at: &str) -> AcmeTask {
        AcmeTask { at: Some(at.into()), ..task(id, dom, "Retry") }
    }
}

/// `lastError` = `acme: <reason>` only when the supervisor has none, the
/// cert is not issued and the last task failed; a healthy cert → none.
#[test]
fn last_error_falls_back_to_the_acme_task() {
    let acme = serde_json::json!({ "lastTask": { "state": "Failed", "reason": "urn:ietf:params:acme:error:connection" } });
    assert_eq!(
        last_error_fallback(None, "missing", &acme).as_deref(),
        Some("acme: urn:ietf:params:acme:error:connection")
    );
    assert!(last_error_fallback(None, "issued", &acme).is_none(), "healthy cert → null");
    assert!(last_error_fallback(Some("start: boom"), "missing", &acme).is_none(), "supervisor wins");
    let pending = serde_json::json!({ "lastTask": { "state": "Pending" } });
    assert!(last_error_fallback(None, "self-signed", &pending).is_none());
    assert!(last_error_fallback(None, "missing", &serde_json::json!({ "lastTask": null })).is_none());
}

#[test]
fn next_renewal_note_only_for_a_valid_wider_cert_under_a_lock() {
    let locked = serde_json::json!({ "locked": true });
    let served = vec!["mail.example.com".to_string(), "mta-sts.example.com".to_string()];
    let note = next_renewal_note("mail.example.com", "issued", &served, &locked).expect("note");
    assert!(note.contains("mta-sts.example.com") && note.contains("covers mail.example.com only"), "{note}");
    assert!(next_renewal_note("mail.example.com", "issued", &served[..1], &locked).is_none());
    assert!(next_renewal_note("mail.example.com", "self-signed", &served, &locked).is_none());
    assert!(next_renewal_note("mail.example.com", "issued", &served, &serde_json::json!({ "locked": false })).is_none());
}

#[test]
fn outbound_status_says_who_set_what() {
    let mut m = Markers::default();
    let out = outbound_status(&Ok(vec![mx_route("v4ThenV6")]), &Ok(default_tls()), &m, None);
    assert_eq!(out["ipStrategy"], "v4ThenV6");
    assert_eq!(out["ipStrategySetBy"], "default");
    assert_eq!(out["daneSummary"], "optional");
    assert_eq!(out["daneAutoOff"], false);
    m.mx_route = Some(RouteMark { id: "jinysbhmadab".into(), from: "v4ThenV6".into(), to: Some("v4Only".into()), at: 1 });
    m.dane = vec![
        DaneMark { id: "jinysbhmacab".into(), name: "invalid-tls".into(), from: "optional".into(), at: 1 },
        DaneMark { id: "jinysbhmacqb".into(), name: "default".into(), from: "optional".into(), at: 1 },
    ];
    let off = vec![tls("jinysbhmacab", "invalid-tls", "disable"), tls("jinysbhmacqb", "default", "disable")];
    let out = outbound_status(&Ok(vec![mx_route("v4Only")]), &Ok(off.clone()), &m, Some("ok"));
    assert_eq!(out["ipStrategySetBy"], "k2");
    assert_eq!(out["daneSummary"], "off (K2, until 0.16.20)");
    assert_eq!(out["dnssec"], "ok");
    let out = outbound_status(&Ok(vec![mx_route("v4Only")]), &Ok(off), &Markers::default(), None);
    assert_eq!(out["ipStrategySetBy"], "hand");
    assert_eq!(out["daneSummary"], "off (set by hand)");
    assert_eq!(out["dane"][0]["setBy"], "hand");
    let out = outbound_status(&Err("down".into()), &Err("down".into()), &Markers::default(), None);
    assert_eq!(out["routeError"], "down");
    assert_eq!(out["daneError"], "down");
}

#[test]
fn doctor_checks_for_dane_route_queue_and_acme() {
    // HF19: warn whenever any strategy has dane: disable.
    let k2 = Markers { dane: vec![DaneMark { id: "b".into(), name: "default".into(), from: "optional".into(), at: 1 }], ..Default::default() };
    let off = vec![tls("a", "invalid-tls", "optional"), tls("b", "default", "disable")];
    let c = dane_disabled_check(&Ok(off.clone()), &k2, Some(v("0.16.10")));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("K2 turned it off") && c.detail.contains("k2 hostmail upgrade"), "{}", c.detail);
    let c = dane_disabled_check(&Ok(off), &Markers::default(), Some(v("0.16.10")));
    assert!(c.detail.contains("set by hand") && c.detail.contains("after `k2 hostmail upgrade`"), "{}", c.detail);
    assert_eq!(dane_disabled_check(&Ok(default_tls()), &k2, Some(v("0.16.10"))).status, ST_PASS);
    assert_eq!(dane_disabled_check(&Err("x".into()), &k2, None).status, ST_UNKNOWN);

    assert_eq!(outbound_route_check(&Ok(vec![mx_route("v4Only")]), Some(v("0.16.10"))).status, ST_PASS);
    assert_eq!(outbound_route_check(&Ok(vec![mx_route("v4ThenV6")]), Some(v("0.16.10"))).status, ST_WARN);
    assert_eq!(outbound_route_check(&Ok(vec![mx_route("v6ThenV4")]), Some(v("0.16.20"))).status, ST_INFO);

    use crate::mail::jmap::QueuedMessageInfo;
    let q = vec![
        QueuedMessageInfo {
            id: "jinywvj7aaaa".into(),
            next_retry: None,
            return_path: None,
            recipients: serde_json::json!({ "x@marbleuniques.com": { "status": { "errorMessage":
                "lookup error: DNS resolution error: DNS error: DNSSEC Negative Record Response for d360910b.ess.barracudanetworks.com. IN AAAA, Bogus" } } }),
        },
        QueuedMessageInfo { id: "ok1".into(), next_retry: None, return_path: None, recipients: serde_json::json!({}) },
    ];
    let c = queue_dnssec_check(&Ok(q));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("jinywvj7aaaa") && !c.detail.contains("ok1") && c.detail.contains("k2 hostmail queue retry"), "{}", c.detail);
    assert_eq!(queue_dnssec_check(&Ok(vec![])).status, ST_PASS);

    let mut c = DoctorCheck { id: "acme-cert-names".into(), label: "x".into(), status: ST_PASS, detail: "Stalwart ACME orders mail.example.com only".into(), gates_direct: false };
    acme_check_extras(&mut c, &serde_json::json!({ "lastTask": { "state": "Failed", "reason": "connection refused", "at": "2026-10-07T20:00:00Z" } }), Some("the next renewal covers mail.example.com only"));
    assert!(c.detail.contains("last ACME task Failed (2026-10-07T20:00:00Z): connection refused"), "{}", c.detail);
    assert!(c.detail.contains("next renewal"), "{}", c.detail);
    assert_eq!(c.status, ST_INFO);
}

#[test]
fn dane_on_is_refused_below_0_16_20_and_turns_every_disable_back() {
    let api = Fake::new();
    *api.tls.borrow_mut() = vec![tls("a", "invalid-tls", "disable"), tls("b", "default", "disable")];
    let mut m = Markers { dane: vec![DaneMark { id: "a".into(), name: "invalid-tls".into(), from: "optional".into(), at: 1 }], ..Default::default() };
    let mut restart = || -> Result<(), String> { panic!("no restart expected") };
    let err = dane_on(&api, Some("0.16.10"), &mut m, &mut restart).expect_err("old");
    assert!(err.contains("k2 hostmail upgrade"), "{err}");
    assert!(api.writes().is_empty());
    let changed = dane_on(&api, Some("0.16.20"), &mut m, &mut restart).expect("on");
    assert_eq!(changed, vec!["invalid-tls", "default"]);
    assert_eq!(api.writes(), vec!["set-tls a \"optional\"", "set-tls b \"optional\"", "reload"]);
    assert!(m.dane.is_empty());
}

// ── DNSSEC probe (captured wire bytes, never a socket) ──────────────────

/// `. DNSKEY` with DO over TCP from 1.1.1.1, captured 2026-10-07: 1414
/// bytes (truncated over UDP), carrying the RRSIG over the key set.
const DNSKEY_DO: &[u8] = include_bytes!("../fixtures/dnskey-root-tcp-do.bin");
/// Same question without the DO bit: keys, no RRSIG.
const DNSKEY_NO_DO: &[u8] = include_bytes!("../fixtures/dnskey-root-tcp-no-do.bin");

#[test]
fn dnskey_query_matches_the_captured_question() {
    // The capture's query bytes (id 0x4b32, RD, root DNSKEY IN, OPT 1232 DO).
    let q = dnskey_query(0x4b32);
    assert_eq!(
        q.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "4b3201000001000000000001000030000100002904d0000080000000"
    );
}

#[test]
fn dnskey_parser_reads_captured_answers() {
    let a = parse_dnskey_tcp(0x4b32, DNSKEY_DO).expect("DO answer");
    assert_eq!(a.bytes, 1414, "the root DNSKEY set no longer fits a 1232-byte UDP reply");
    assert_eq!(a.rcode, 0);
    assert!(!a.truncated);
    assert!(a.dnskeys >= 2, "{a:?}");
    assert!(a.rrsig_over_dnskey, "{a:?}");
    let b = parse_dnskey_tcp(0x4b33, DNSKEY_NO_DO).expect("no-DO answer");
    assert!(b.dnskeys >= 2 && !b.rrsig_over_dnskey, "{b:?}");
    assert!(parse_dnskey_tcp(0x4b33, DNSKEY_DO).expect_err("id").contains("id"));
    assert!(parse_dnskey_tcp(0x4b32, &DNSKEY_DO[..700]).expect_err("short").contains("short"));
    assert!(parse_dnskey_tcp(0x4b32, &[]).is_err());
}

#[test]
fn resolv_conf_parsing() {
    let text = "# managed\nnameserver 127.0.0.53\noptions edns0 trust-ad\nsearch .\nnameserver 2001:db8::53 ; v6\n#nameserver 9.9.9.9\n";
    assert_eq!(resolv_conf_nameservers(text), vec!["127.0.0.53", "2001:db8::53"]);
}

#[test]
fn dnssec_resolver_check_per_nameserver() {
    let ns = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // Answer with the captured stream, re-stamped with the query's id.
    let serve = |stream: &'static [u8]| {
        move |_ns: &str, q: &[u8]| -> Result<Vec<u8>, String> {
            let mut s = stream.to_vec();
            s[2] = q[0];
            s[3] = q[1];
            Ok(s)
        }
    };
    let ok = serve(DNSKEY_DO);
    let c = dnssec_resolver_check(&ns(&["127.0.0.53"]), &ok, Some(v("0.16.10")));
    assert_eq!(c.status, ST_PASS, "{}", c.detail);
    // Two nameservers on Stalwart < 0.16.23: the race warning (HF5).
    let c = dnssec_resolver_check(&ns(&["10.0.0.2", "10.0.0.3"]), &ok, Some(v("0.16.20")));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("0.16.23") && c.detail.contains("more than one nameserver"), "{}", c.detail);
    assert_eq!(dnssec_resolver_check(&ns(&["10.0.0.2", "10.0.0.3"]), &ok, Some(v("0.16.23"))).status, ST_PASS);
    // Signatures stripped.
    let stripped = serve(DNSKEY_NO_DO);
    let c = dnssec_resolver_check(&ns(&["127.0.0.53"]), &stripped, Some(v("0.16.10")));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("DANE can't work here") && c.detail.contains("RRSIG"), "{}", c.detail);
    // TCP/53 blocked (a timeout).
    let blocked = |_ns: &str, _q: &[u8]| -> Result<Vec<u8>, String> { Err("TCP/53 connect: timed out".into()) };
    let c = dnssec_resolver_check(&ns(&["127.0.0.53"]), &blocked, Some(v("0.16.10")));
    assert_eq!(c.status, ST_WARN);
    assert!(c.detail.contains("check TCP/53 egress") && c.detail.contains("timed out"), "{}", c.detail);
    assert_eq!(dnssec_resolver_check(&[], &ok, None).status, ST_UNKNOWN);
}

#[test]
fn mail_host_address_check_fails_a_missing_or_foreign_a() {
    use crate::mail::dns_verify::{DnsError, DnsResolver, MxHost, SrvAnswer};
    struct R(Option<Vec<std::net::Ipv4Addr>>);
    impl DnsResolver for R {
        fn mx(&self, _: &str) -> Result<Vec<MxHost>, DnsError> { Err(DnsError::NotFound) }
        fn txt(&self, _: &str) -> Result<Vec<Vec<String>>, DnsError> { Err(DnsError::NotFound) }
        fn a(&self, _: &str) -> Result<Vec<std::net::Ipv4Addr>, DnsError> { self.0.clone().ok_or(DnsError::NotFound) }
        fn ptr(&self, _: std::net::IpAddr) -> Result<Vec<String>, DnsError> { Err(DnsError::NotFound) }
        fn srv(&self, _: &str) -> Result<Vec<SrvAnswer>, DnsError> { Err(DnsError::NotFound) }
    }
    let addrs = crate::mail::cert_names::BoxAddrs::default();
    let c = mail_host_address_check(&R(None), "mail.example.com", Some("203.0.113.7"), &addrs);
    assert_eq!(c.status, crate::mail::doctor::ST_FAIL);
    assert!(c.detail.contains("mail.example.com A 203.0.113.7") && c.detail.contains("cert renew"), "{}", c.detail);
    assert!(!c.gates_direct);
    let c = mail_host_address_check(&R(Some(vec!["198.51.100.1".parse().unwrap()])), "mail.example.com", Some("203.0.113.7"), &addrs);
    assert_eq!(c.status, crate::mail::doctor::ST_FAIL);
    let c = mail_host_address_check(&R(Some(vec!["203.0.113.7".parse().unwrap()])), "mail.example.com", Some("203.0.113.7"), &addrs);
    assert_eq!(c.status, ST_PASS, "{}", c.detail);
}

// ── Against the loopback mock (exact wire bodies) ───────────────────────

mod wire {
    use super::*;
    use crate::mail::jmap::tests::{body_json, spawn_mock_server, NORMAL_SESSION_FIXTURE};

    fn reply(method: &str, args: Value) -> String {
        serde_json::json!({ "methodResponses": [[method, args, "0"]] }).to_string()
    }

    /// The PRD's canned-reply case: the default `mx` route gets exactly
    /// one update to v4Only (whole object minus id/name) plus one
    /// ReloadSettings; nothing else is written.
    #[test]
    fn default_mx_route_gets_one_v4only_update_and_one_reload() {
        let routes = serde_json::json!({ "list": [mx_route("v4ThenV6"), local_route()], "notFound": [] });
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            reply("x:SpamSettings/get", serde_json::json!({ "list": [{ "id": "singleton", "spamFilterRulesUrl": RULES_URL_PINNED }] })),
            reply("x:SpamDnsblServer/get", serde_json::json!({ "list": dnsbl_from("v3.0.1") })),
            reply("x:MtaRoute/get", routes),
            reply("x:MtaRoute/set", serde_json::json!({ "updated": { "jinysbhmadab": null } })),
            reply("x:MtaTlsStrategy/get", serde_json::json!({ "list": default_tls() })),
            reply("x:Action/set", serde_json::json!({ "created": { "k2": { "id": "act1" } } })),
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let served = || "issued".to_string();
        let points = || PointsHere::Yes;
        let ctx = Ctx {
            cert: CertInputs { host: "mail.example.com", owner: CertOwner::K2, port_plan: Some("tls-alpn"), served_state: &served, points_here: &points },
            installed: Some("0.16.20"),
            now: 7,
        };
        let mut restart = || -> Result<(), String> { panic!("no restart") };
        let out = reconcile(&client, &ctx, Some(Markers::default()), &mut restart);
        assert_eq!(out.settings_written, 1, "{:?}", out.lines);
        let _session = rx.recv().expect("session");
        let methods: Vec<Value> = (0..6).map(|_| body_json(&rx.recv().expect("call"))).collect();
        let names: Vec<&str> = methods.iter().map(|b| b["methodCalls"][0][0].as_str().expect("m")).collect();
        assert_eq!(
            names,
            vec!["x:SpamSettings/get", "x:SpamDnsblServer/get", "x:MtaRoute/get", "x:MtaRoute/set", "x:MtaTlsStrategy/get", "x:Action/set"]
        );
        let set = &methods[3]["methodCalls"][0][1]["update"];
        assert_eq!(set.as_object().expect("update").len(), 1);
        assert_eq!(
            set["jinysbhmadab"],
            serde_json::json!({ "@type": "Mx", "ipLookupStrategy": "v4Only", "maxMultihomed": 2, "maxMxHosts": 2, "description": null })
        );
        assert_eq!(methods[5]["methodCalls"][0][1]["create"]["k2"]["@type"], "ReloadSettings");
        assert!(rx.try_recv().is_err(), "nothing else was sent");
    }

    /// A `v6ThenV4` route gets no write, and with nothing written there is
    /// no reload.
    #[test]
    fn a_hand_set_route_gets_no_write_and_no_reload() {
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            reply("x:SpamSettings/get", serde_json::json!({ "list": [{ "id": "singleton", "spamFilterRulesUrl": null }] })),
            reply("x:SpamDnsblServer/get", serde_json::json!({ "list": [] })),
            reply("x:MtaRoute/get", serde_json::json!({ "list": [mx_route("v6ThenV4")] })),
            reply("x:MtaTlsStrategy/get", serde_json::json!({ "list": default_tls() })),
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let served = || "issued".to_string();
        let points = || PointsHere::Yes;
        let ctx = Ctx {
            cert: CertInputs { host: "mail.example.com", owner: CertOwner::K2, port_plan: Some("tls-alpn"), served_state: &served, points_here: &points },
            installed: Some("0.16.20"),
            now: 7,
        };
        let mut restart = || -> Result<(), String> { panic!("no restart") };
        let out = reconcile(&client, &ctx, Some(Markers::default()), &mut restart);
        assert_eq!(out.settings_written, 0);
        assert_eq!(out.markers.mx_route.as_ref().map(|r| r.to.clone()), Some(None));
        let _ = rx.recv().expect("session");
        for want in ["x:SpamSettings/get", "x:SpamDnsblServer/get", "x:MtaRoute/get", "x:MtaTlsStrategy/get"] {
            assert_eq!(body_json(&rx.recv().expect("call"))["methodCalls"][0][0], want);
        }
        assert!(rx.try_recv().is_err(), "no write, no reload");
    }

    /// `notUpdated` on a registry write is a loud Err with the server's
    /// SetError (the rehearsal's read-only `name` refusal).
    #[test]
    fn not_updated_is_a_loud_error() {
        let (port, _rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            reply("x:MtaRoute/set", serde_json::json!({ "notUpdated": { "jinysbhmadab": {
                "type": "invalidPatch", "description": "Cannot modify read-only property" } } })),
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let err = client
            .registry_update("MtaRoute", "jinysbhmadab", serde_json::json!({ "name": "mx" }))
            .expect_err("refused");
        assert!(err.contains("notUpdated") && err.contains("read-only"), "{err}");
    }

    /// An unsupported filter (live 2026-10-08: `type` instead of `@type`
    /// is refused on 0.16.10 and 0.16.20): K2 re-queries unfiltered and
    /// keeps the AcmeRenewal tasks itself.
    #[test]
    fn acme_tasks_fall_back_to_an_unfiltered_query_on_0_16_10() {
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            serde_json::json!({ "methodResponses": [["error", { "type": "unsupportedFilter", "description": "type" }, "0"]] }).to_string(),
            reply("x:Task/query", serde_json::json!({ "ids": ["t1", "t2"] })),
            reply("x:Task/get", serde_json::json!({ "list": [
                { "id": "t1", "@type": "AcmeRenewal", "domainId": "b", "status": { "@type": "Failed",
                  "createdAt": "2026-10-08T05:52:13Z", "failedAt": "2026-10-08T05:52:13Z",
                  "failedAttemptNumber": 0, "failureReason": "Invalid request: ACME not configured for domain" } },
                { "id": "t2", "@type": "DkimManagement", "domainId": "b", "status": { "@type": "Pending" } },
            ] })),
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let t = client.acme_renewal_tasks().expect("tasks");
        assert_eq!(t.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["t1"]);
        let _ = rx.recv().expect("session");
        let q1 = body_json(&rx.recv().expect("filtered"));
        assert_eq!(q1["methodCalls"][0][1]["filter"]["@type"], "AcmeRenewal");
        let q2 = body_json(&rx.recv().expect("unfiltered"));
        assert!(q2["methodCalls"][0][1].get("filter").is_none(), "{q2}");
        let g = body_json(&rx.recv().expect("get"));
        assert_eq!(g["methodCalls"][0][1]["ids"], serde_json::json!(["t1", "t2"]));
    }

    #[test]
    fn acme_tasks_query_then_get_and_spam_url_round_trip() {
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            reply("x:Task/query", serde_json::json!({ "ids": ["t1"] })),
            reply("x:Task/get", serde_json::json!({ "list": [{ "id": "t1", "@type": "AcmeRenewal", "domainId": "dom-1",
                "status": { "@type": "Retry", "createdAt": "2026-10-07T10:00:00Z", "due": "2026-10-07T10:02:00Z",
                            "attemptNumber": 1, "failureReason": "connection refused" } }] })),
            reply("x:SpamSettings/get", serde_json::json!({ "list": [{ "id": "singleton", "spamFilterRulesUrl": RULES_URL_LATEST }] })),
            reply("x:SpamSettings/set", serde_json::json!({ "updated": { "singleton": null } })),
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let t = client.acme_renewal_tasks().expect("tasks");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].reason.as_deref(), Some("connection refused"));
        assert_eq!(client.spam_filter_rules_url().expect("url").as_deref(), Some(RULES_URL_LATEST));
        client.set_spam_filter_rules_url(RULES_URL_PINNED).expect("set");
        let _ = rx.recv().expect("session");
        let q = body_json(&rx.recv().expect("query"));
        assert_eq!(q["methodCalls"][0][1]["filter"], serde_json::json!({ "@type": "AcmeRenewal" }), "live: the key is @type");
        let g = body_json(&rx.recv().expect("get"));
        assert_eq!(g["methodCalls"][0][1]["ids"], serde_json::json!(["t1"]));
        let _ = rx.recv().expect("settings get");
        let s = body_json(&rx.recv().expect("settings set"));
        assert_eq!(s["methodCalls"][0][1]["update"]["singleton"], serde_json::json!({ "spamFilterRulesUrl": RULES_URL_PINNED }));
    }
}
