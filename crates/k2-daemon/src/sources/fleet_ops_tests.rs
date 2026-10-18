//! Fleet Ops end to end (0.46.1): the live Fleet Ops Garden's sources, as
//! the 0.46.1 edit files leave them (`fixtures/fleet_ops/`, made-up hosts),
//! fed K2-Web-shaped `/api/ops/fleet` bodies (the shapes the live sources
//! read on 2026-10-16) through validation, the store, the scheduler and the
//! card view the cards and widgets read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_core::sources::spec::SourceSpec;
use k2_core::sources::store::{self, NewSource};
use k2_core::sources::{FetchOutcome, FetchPlan, SourceStatus, UrlRef};
use serde_json::{json, Value};

use super::fetch::Fetched;
use super::test_engine::Harness;
use crate::compute_routes::Who;
use crate::egress::test_util::AirgapEnv;

const T0: i64 = 1_792_195_800;

const FIXTURES: &[(&str, &str)] = &[
    ("k2-ops-boxes", include_str!("fixtures/fleet_ops/k2-ops-boxes.json")),
    ("k2-ops-billing", include_str!("fixtures/fleet_ops/k2-ops-billing.json")),
    ("k2-ops-edge", include_str!("fixtures/fleet_ops/k2-ops-edge.json")),
    ("k2-ops-certs", include_str!("fixtures/fleet_ops/k2-ops-certs.json")),
    ("k2-ops-backups", include_str!("fixtures/fleet_ops/k2-ops-backups.json")),
    ("k2-ops-watch", include_str!("fixtures/fleet_ops/k2-ops-watch.json")),
    ("k2-dev-site", include_str!("fixtures/fleet_ops/k2-dev-site.json")),
    ("k2-connect", include_str!("fixtures/fleet_ops/k2-connect.json")),
];

fn spec(name: &str) -> SourceSpec {
    let raw = FIXTURES.iter().find(|(n, _)| *n == name).map(|(_, r)| *r).unwrap_or_else(|| panic!("no fixture {name}"));
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// What K2 Web answers, by `?part=` (the live shapes), with knobs the
/// test turns: the block age, ohana's version, luzz online, a relay's push
/// age, the past-due / canceled rows.
#[derive(Clone)]
struct Web {
    now: i64,
    ts_age: i64,
    ohana: &'static str,
    luzz_online: bool,
    k2e02_push_age: i64,
    canceled: Vec<&'static str>,
    cert_days: i64,
}

impl Web {
    fn body(&self, part: &str) -> Value {
        let ts = self.now - self.ts_age;
        match part {
            "boxes" => json!({"boxes": {"ts": ts, "error": null, "target_version": "0.46.1", "items": [
                {"label": "akzm", "customer": false, "needs_go": null, "online": true, "version": "0.46.1", "phase": "ready",
                 "last_ok": ts, "down_since": null, "tier": "pro", "stripe_status": "active"},
                {"label": "ohana", "customer": true, "needs_go": "rosson", "online": true, "version": self.ohana, "phase": "ready",
                 "last_ok": ts, "down_since": null, "tier": "pro", "stripe_status": "active"},
                {"label": "luzz", "customer": true, "needs_go": "rosson", "online": self.luzz_online, "version": "0.46.0",
                 "phase": "ready", "last_ok": ts, "down_since": if self.luzz_online { Value::Null } else { json!(ts - 60) },
                 "tier": "ded-s", "stripe_status": "active"}
            ]}}),
            "billing" => json!({"billing": {"ts": ts, "error": null,
                "subdomains": {"by_stripe_status": {"active": 53, "past_due": 2, "canceled": 1}, "suspended": 3, "missing_period_end": 0},
                "servers": {"by_stripe_status": {"active": 11, "past_due": 1}},
                "past_due": [{"label": "edf", "keep_live": false}, {"label": "noir", "keep_live": true}],
                "canceled_but_routing": self.canceled.iter().map(|l| json!({"label": l})).collect::<Vec<_>>(),
                "reconcile": {"ts": self.now - 3300, "ok": true, "error": null, "write_mode": false,
                              "would_cancel": 0, "would_restore": 0, "date_diffs": 0, "status_diffs": 2}}}),
            // One fetch, three blocks (`?part=relays,dns,deploys`).
            "relays,dns,deploys" => json!({
                "relays": {"ts": ts, "items": [
                    {"name": "k2e-01", "role": "primary", "tunnels": 46, "cp": "active", "frps": "active", "caddy": "active",
                     "haproxy": "active", "pdns": "active", "cp_sha": "fa7baeacddd3306a", "ts": self.now - 20},
                    {"name": "k2e-02", "role": "standby", "tunnels": 0, "cp": "active", "frps": "active", "caddy": "active",
                     "haproxy": "active", "pdns": "active", "cp_sha": "fa7baeacddd3306a", "ts": self.now - self.k2e02_push_age}]},
                "dns": {"ts": ts, "wildcard_a": "primary", "failover_armed": true, "failback": "manual"},
                "deploys": {"ts": ts, "web": {"sha": "a174d7a"}, "cp": {"k2e-01": "fa7baeacddd3306a", "k2e-02": "fa7baeacddd3306a"}}
            }),
            "certs" => json!({"certs": {"ts": ts, "error": null, "items": [
                {"name": "k2.dev", "days": 78}, {"name": "connect.k2.dev on k2e-02", "days": self.cert_days}]}}),
            "backups" => json!({"backups": {"ts": ts, "error": null, "items": [{"name": "k2e-01", "age_h": 20.8}, {"name": "k2e-02", "age_h": 20.5}]}}),
            "watch" => json!({"watch": {"ts": ts, "error": null, "ok": 32, "total": 32, "muted": false, "failing": []}}),
            other => panic!("no K2 Web part {other}"),
        }
    }
}

fn answer(web: &Web, plan: &FetchPlan) -> Fetched {
    let url = match &plan.url {
        UrlRef::Plain(u) => u.clone(),
        UrlRef::Secret { name } => panic!("no url secret here: {name}"),
    };
    let outcome = if let Some(part) = url.split("part=").nth(1) {
        FetchOutcome::Ok { status: 200, body: web.body(part), not_modified: false, etag: None, last_modified: None, elapsed_ms: 3 }
    } else if url.contains("connect.example.test") {
        // Status-only: a 401 login page is the healthy answer (fetch.rs turns
        // a listed status into Ok with a null body).
        FetchOutcome::Ok { status: 401, body: Value::Null, not_modified: false, etag: None, last_modified: None, elapsed_ms: 3 }
    } else {
        FetchOutcome::Ok { status: 200, body: Value::Null, not_modified: false, etag: None, last_modified: None, elapsed_ms: 3 }
    };
    Fetched { outcome, park_secs: None }
}

fn read(name: &str) -> Value {
    let p: HashMap<String, String> = [("name".to_string(), name.to_string())].into();
    let r = crate::sources_routes::handle("/cli/sources/read", Who::Owner, b"", &p);
    assert!(r.status.starts_with("200"), "{name}: {} {}", r.status, r.body);
    serde_json::from_str(&r.body).expect("view json")
}

fn item<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["items"].as_array().expect("items").iter().find(|i| i["key"] == key).unwrap_or_else(|| panic!("no item {key} in {v}"))
}

#[test]
fn every_fleet_ops_source_validates_and_reads_k2_webs_shapes_end_to_end() {
    let _a = AirgapEnv::set(false);
    let _home = crate::test_support::TempHome::new();
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    for (name, _) in FIXTURES {
        let v = spec(name).validate(false).unwrap_or_else(|e| panic!("{name} validates: {e:?}"));
        store::insert_source(
            &conn,
            &NewSource { v: &v, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: T0 },
        )
        .unwrap_or_else(|e| panic!("{name} stores: {e}"));
    }
    // Which opt in to per-rule thresholds (APPLY.md §0).
    let opted: Vec<&str> = FIXTURES.iter().map(|(n, _)| *n).filter(|n| spec(n).uses_rule_thresholds()).collect();
    assert_eq!(opted, ["k2-ops-boxes", "k2-ops-billing", "k2-ops-edge"]);

    let web = Arc::new(Mutex::new(Web {
        now: T0,
        ts_age: 10,
        ohana: "0.44.4",
        luzz_online: true,
        k2e02_push_age: 20,
        canceled: vec!["quillify"],
        cert_days: 10,
    }));
    let w2 = web.clone();
    let mut h = Harness::new(move |p| answer(&w2.lock().unwrap(), p), T0);
    // Time moves in 5 s ticks: the six ops sources share one origin (K2's
    // per-origin budget, a request every 2 s), so they stagger as live.
    let tick = |h: &mut Harness| {
        h.advance(5);
        web.lock().unwrap().now = h.wall;
        let conn = db.lock();
        h.step(&conn);
    };
    let step = |h: &mut Harness, secs: u64| {
        for _ in 0..secs.div_ceil(5) {
            tick(h);
        }
    };
    let checked = |name: &str| -> Option<i64> {
        let conn = db.lock();
        let row = store::get_by_name(&conn, name).unwrap().unwrap();
        store::load_state(&conn, &row.id).unwrap().and_then(|(st, _)| st.checked_at)
    };
    // Exactly one more check of `name` (polls come every 60 s ± 10 %).
    let check = |h: &mut Harness, name: &str| {
        let before = checked(name);
        for _ in 0..30 {
            tick(h);
            if checked(name) != before {
                return;
            }
        }
        panic!("{name} wasn't checked in 150 s");
    };
    drop(conn);
    web.lock().unwrap().now = h.wall;
    {
        let conn = db.lock();
        h.step(&conn);
    }
    step(&mut h, 60); // check 1 of every source (first checks land within 60 s)

    // Boxes: the target from K2 Web, behind / far behind counts, whose go.
    let boxes = read("k2-ops-boxes");
    assert_eq!(boxes["phase"], "live");
    assert_eq!(boxes["fields"]["target_version"], "0.46.1");
    assert_eq!((boxes["fields"]["behind"].clone(), boxes["fields"]["far_behind"].clone()), (json!(2), json!(1)));
    assert_eq!(item(&boxes, "ohana")["fields"]["needs_go"], "rosson");
    assert_eq!(item(&boxes, "ohana")["fields"]["customer"], true);
    // ohana is a minor behind: warn after 3 checks (warnAfter 3), so it
    // waits; a box never rated yet reads unknown meanwhile (as before 0.46.1).
    assert_eq!(item(&boxes, "ohana")["health"], "unknown");
    assert_eq!(item(&boxes, "ohana")["soft"], json!({"count": 1, "of": 3, "level": "warn"}));
    assert_eq!(item(&boxes, "luzz")["health"], "ok", "a patch behind stays ok");

    // Billing: keep_live warns at once (opted in), canceled-but-routing is a
    // list item and a count, the old first-row fields stay for the widgets.
    let billing = read("k2-ops-billing");
    assert_eq!(item(&billing, "edf")["health"], "warn");
    let canceled = item(&billing, "canceled/quillify");
    assert_eq!((canceled["list"].clone(), canceled["health"].clone()), (json!("canceled"), json!("crit")));
    assert_eq!(billing["fields"]["canceled_routing"], 1);
    assert_eq!(billing["fields"]["canceled_routing_first_label"], "quillify", "alerts.js and edge.js read it");
    assert_eq!(billing["fields"]["past_due"], 2);
    assert_eq!(billing["fields"]["past_due_not_kept"], 1);
    assert_eq!(billing["health"], "crit");

    // Edge: one fetch, three blocks.
    let edge = read("k2-ops-edge");
    assert_eq!(edge["fields"]["wildcard_a"], "primary");
    assert_eq!(edge["fields"]["web_sha"], "a174d7a");
    assert_eq!(edge["fields"]["tunnels_total"], 46);
    assert_eq!(item(&edge, "k2e-01")["fields"]["role"], "primary");
    assert_eq!(edge["health"], "ok");

    // Certs isn't opted in: a 10-day cert waits failAfter (3) as before.
    let certs = read("k2-ops-certs");
    let cert = item(&certs, "connect.k2.dev on k2e-02");
    assert_eq!((cert["health"].clone(), cert["soft"]["count"].clone(), cert["soft"]["of"].clone()), (json!("unknown"), json!(1), json!(3)), "never rated yet: unknown while it waits");

    // Status-only: connect's 401 is healthy, the site's 200 too.
    let connect = read("k2-connect");
    assert_eq!((connect["health"].clone(), connect["httpStatus"].clone()), (json!("ok"), json!(401)));
    assert_eq!(read("k2-dev-site")["health"], "ok");
    assert_eq!(read("k2-ops-watch")["health"], "ok");
    assert_eq!(read("k2-ops-backups")["health"], "ok");

    check(&mut h, "k2-ops-boxes"); // check 2
    assert_eq!(item(&read("k2-ops-boxes"), "ohana")["soft"]["count"], 2);
    // Check 3: luzz goes offline, k2e-02 stops pushing.
    {
        let mut w = web.lock().unwrap();
        w.luzz_online = false;
        w.k2e02_push_age = 1000;
    }
    check(&mut h, "k2-ops-boxes");
    let boxes = read("k2-ops-boxes");
    assert_eq!(item(&boxes, "ohana")["health"], "warn", "a minor behind warns on its 3rd check");
    assert_eq!(item(&boxes, "luzz")["health"], "warn", "offline warns at once…");
    assert_eq!(item(&boxes, "luzz")["soft"], json!({"count": 1, "of": 3, "level": "crit"}));
    // A relay 1000 s quiet: warn, then crit on its 2nd check (critAfter 2).
    let k2e02 = |n: &str| item(&read("k2-ops-edge"), "k2e-02")[n].clone();
    for _ in 0..3 {
        if k2e02("health") != json!("ok") {
            break;
        }
        check(&mut h, "k2-ops-edge");
    }
    assert_eq!(k2e02("health"), "warn");
    check(&mut h, "k2-ops-edge");
    assert_eq!(k2e02("health"), "crit");
    // Certs isn't opted in: its waiting warn turns hard on the 3rd check.
    let cert = || item(&read("k2-ops-certs"), "connect.k2.dev on k2e-02")["health"].clone();
    for _ in 0..3 {
        if cert() == json!("warn") {
            break;
        }
        check(&mut h, "k2-ops-certs");
    }
    assert_eq!(cert(), "warn", "certs' legacy wait ends on the 3rd check");
    // The boxes block goes old while K2 Web still serves it.
    web.lock().unwrap().ts_age = 1000;
    let luzz_before = item(&read("k2-ops-boxes"), "luzz")["soft"]["count"].clone();
    check(&mut h, "k2-ops-boxes");
    let boxes = read("k2-ops-boxes");
    assert_eq!(boxes["health"], "crit", "the block's own ts is 1000 s old");
    assert_eq!(boxes["phase"], "live", "an old block is crit on its ts, not a stale fetch");
    if luzz_before == json!(1) {
        assert_eq!(item(&boxes, "luzz")["health"], "warn", "2 checks offline: still warn");
        check(&mut h, "k2-ops-boxes");
    }
    assert_eq!(item(&read("k2-ops-boxes"), "luzz")["health"], "crit", "…and crit after 3 checks offline");
    let events = {
        let conn = db.lock();
        let row = store::get_by_name(&conn, "k2-ops-boxes").unwrap().unwrap();
        store::events(&conn, &row.id, 50).unwrap()
    };
    assert!(events.iter().any(|e| e.kind == "went_crit" && e.item_key.as_deref() == Some("luzz")));
    assert!(events.iter().any(|e| e.kind == "went_warn" && e.item_key.as_deref() == Some("ohana")));
}
