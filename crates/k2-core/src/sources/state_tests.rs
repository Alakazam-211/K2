use super::*;
use serde_json::json;

fn fleet(n: usize) -> SourceSpec {
    let targets: Vec<_> =
        (0..n).map(|i| json!({"key": format!("b{i}"), "url": format!("https://b{i}.example.com/boot-status")})).collect();
    serde_json::from_value(json!({
        "name": "fleet", "label": "Fleet", "fetch": {"targets": targets}, "everySecs": 60,
        "extract": {"items": {"itemFields": {"version": "/version", "phase": "/phase"}},
                    "aggregates": {"n": {"op": "count"}}, "trend": ["n"]},
        "rules": [{"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}}]
    }))
    .unwrap()
}

fn single() -> SourceSpec {
    serde_json::from_value(json!({
        "name": "st", "label": "Status", "fetch": {"url": "https://status.example.com/api/v2/summary.json"}, "everySecs": 60,
        "extract": {"fields": {"indicator": "/status/indicator", "open": "/open"},
                    "items": {"pointer": "/components", "itemKey": "/name", "itemFields": {"status": "/status"}},
                    "trend": ["open"]},
        "rules": [{"field": "status", "crit": {"eq": "major_outage"}, "warn": {"eq": "degraded"}, "ok": {"eq": "operational"}}]
    }))
    .unwrap()
}

fn ok(body: serde_json::Value) -> FetchOutcome {
    FetchOutcome::Ok { status: 200, body, not_modified: false, etag: Some("\"e1\"".into()), last_modified: None, elapsed_ms: 5 }
}

fn fail(r: Reason) -> FetchOutcome {
    FetchOutcome::Err { reason: r, http_status: None, retry_after_secs: None }
}

fn ready() -> FetchOutcome {
    ok(json!({"version": "0.45.6", "phase": "ready"}))
}

struct Run {
    spec: SourceSpec,
    st: SourceState,
    proj: Option<Projection>,
    now: i64,
}

impl Run {
    fn new(spec: SourceSpec) -> Run {
        Run { spec, st: SourceState::new("s1"), proj: None, now: 1_000_000 }
    }
    fn poll(&mut self, outcomes: Vec<(Option<String>, FetchOutcome)>) -> Vec<Transition> {
        self.now += 60;
        let a = apply_poll(
            &self.st,
            PollInput { spec: &self.spec, outcomes, now: self.now, simulated: false, prev_projection: self.proj.as_ref() },
        );
        if a.projection.is_some() {
            self.proj = a.projection;
        }
        self.st = a.state;
        a.transitions
    }
    fn fan(&mut self, f: impl Fn(usize) -> FetchOutcome, n: usize) -> Vec<Transition> {
        self.poll((0..n).map(|i| (Some(format!("b{i}")), f(i))).collect())
    }
}

fn hard(ts: &[Transition]) -> Vec<(Option<&str>, TransitionKind)> {
    ts.iter().filter(|t| t.hard).map(|t| (t.scope_item.as_deref(), t.kind)).collect()
}

#[test]
fn one_down_target_is_a_red_item_after_fail_after_and_the_source_stays_live() {
    let mut r = Run::new(fleet(3));
    let ts = r.fan(|_| ready(), 3);
    assert_eq!(r.st.phase, Phase::Live);
    assert_eq!(r.st.health, Health::Ok);
    assert!(hard(&ts).contains(&(None, TransitionKind::WentOk)));
    assert_eq!(r.st.counts, Counts { ok: 3, warn: 0, crit: 0, unknown: 0 });

    let down = |i: usize| if i == 2 { fail(Reason::Timeout) } else { ready() };
    for n in 1..=2 {
        let ts = r.fan(down, 3);
        assert!(hard(&ts).is_empty(), "soft {n}/3 makes no hard change");
        assert_eq!(r.st.items["b2"].soft_count, n);
        assert_eq!(r.st.items["b2"].health, Health::Ok);
        assert!(r.st.has_soft());
    }
    let ts = r.fan(down, 3);
    assert_eq!(hard(&ts), vec![(Some("b2"), TransitionKind::WentCrit), (None, TransitionKind::WentCrit)]);
    let item = &r.st.items["b2"];
    assert_eq!((item.health, item.reason), (Health::Crit, Some(Reason::Timeout)));
    assert!(item.incident_id.is_some());
    assert_eq!(r.st.phase, Phase::Live, "one box down never stales the source");
    assert_eq!(r.st.health, Health::Crit);
    assert_eq!(r.st.reason, Some(Reason::Timeout));
    assert_eq!(r.st.counts, Counts { ok: 2, warn: 0, crit: 1, unknown: 0 });
    assert!(r.proj.as_ref().unwrap().item("b2").unwrap().fields.contains_key("version"), "last good fields kept");

    // Still down: no new transition, same incident.
    let inc = r.st.items["b2"].incident_id.clone();
    assert!(hard(&r.fan(down, 3)).is_empty());
    assert_eq!(r.st.items["b2"].incident_id, inc);

    // Recovery applies at once and closes the incident.
    let ts = r.fan(|_| ready(), 3);
    assert_eq!(hard(&ts), vec![(Some("b2"), TransitionKind::Recovered), (None, TransitionKind::Recovered)]);
    let rec = ts.iter().find(|t| t.scope_item.is_some()).unwrap();
    assert_eq!(rec.incident_id, inc, "recovery carries the closed incident");
    assert!(r.st.items["b2"].incident_id.is_none());
    assert_eq!(r.st.items["b2"].recovered_at, Some(r.now));
}

#[test]
fn unknown_target_error_mode_and_unknown_counted_apart() {
    let mut spec = fleet(3);
    spec.on_target_error = OnTargetError::Unknown;
    spec.fail_after = 1;
    let mut r = Run::new(spec);
    r.fan(|_| ready(), 3);
    r.fan(|i| if i == 0 { fail(Reason::Dns) } else { ready() }, 3);
    assert_eq!(r.st.items["b0"].health, Health::Unknown);
    assert_eq!(r.st.health, Health::Ok, "18 ok + 2 unknown is ok");
    assert_eq!(r.st.counts.unknown, 1);
}

#[test]
fn every_target_down_goes_stale_after_the_window_and_fail_after() {
    let mut r = Run::new(fleet(2));
    r.fan(|_| ready(), 2);
    let mut went_stale = false;
    for _ in 0..4 {
        let ts = r.fan(|_| fail(Reason::Connect), 2);
        went_stale |= ts.iter().any(|t| t.kind == TransitionKind::WentStale);
    }
    // 4 × 60 s since the last success >= max(180, 180).
    assert!(went_stale);
    assert_eq!(r.st.phase, Phase::Stale);
    assert_eq!(r.st.fetch_failures, 4);
    assert_eq!(r.st.backoff_k, 1);
    let ts = r.fan(|_| ready(), 2);
    assert!(ts.iter().any(|t| t.kind == TransitionKind::WentLive));
    assert_eq!(r.st.phase, Phase::Live);
}

#[test]
fn a_never_reached_source_shows_stale_not_pending_after_fail_after() {
    let mut spec = single();
    spec.fail_after = 2;
    let mut r = Run::new(spec);
    r.poll(vec![(None, fail(Reason::Dns))]);
    assert_eq!(r.st.phase, Phase::Pending);
    r.poll(vec![(None, fail(Reason::Dns))]);
    assert_eq!(r.st.phase, Phase::Stale);
    assert_eq!(r.st.health, Health::Crit);
    assert_eq!(r.st.reason, Some(Reason::Dns));
}

#[test]
fn non_ok_readings_go_through_the_soft_count_and_escalation_keeps_the_incident() {
    let mut r = Run::new(single());
    let body = |s: &str| ok(json!({"status": {"indicator": "x"}, "open": 2, "components": [{"name": "api", "status": s}]}));
    r.poll(vec![(None, body("operational"))]);
    assert_eq!(r.st.items["api"].health, Health::Ok);
    r.poll(vec![(None, body("degraded"))]);
    r.poll(vec![(None, body("degraded"))]);
    assert_eq!(r.st.items["api"].health, Health::Ok, "warn is soft until failAfter");
    let ts = r.poll(vec![(None, body("degraded"))]);
    assert!(hard(&ts).contains(&(Some("api"), TransitionKind::WentWarn)));
    let inc = r.st.items["api"].incident_id.clone().expect("incident opened");
    for _ in 0..3 {
        r.poll(vec![(None, body("major_outage"))]);
    }
    assert_eq!(r.st.items["api"].health, Health::Crit);
    assert_eq!(r.st.items["api"].incident_id.as_deref(), Some(inc.as_str()), "warn → crit keeps the incident");
    assert_eq!(r.st.incident_id.is_some(), true);

    // failAfter = 1: readings apply at once.
    let mut s = single();
    s.fail_after = 1;
    let mut r = Run::new(s);
    r.poll(vec![(None, body("operational"))]);
    let ts = r.poll(vec![(None, body("degraded"))]);
    assert!(hard(&ts).contains(&(Some("api"), TransitionKind::WentWarn)));
}

#[test]
fn single_url_failures_rate_the_source_and_keep_the_projection() {
    let mut r = Run::new(single());
    r.poll(vec![(None, ok(json!({"status": {"indicator": "none"}, "open": 1, "components": []})))]);
    assert_eq!(r.st.health, Health::Ok);
    for _ in 0..3 {
        r.poll(vec![(None, FetchOutcome::Err { reason: Reason::Http5xx, http_status: Some(503), retry_after_secs: None })]);
    }
    assert_eq!(r.st.health, Health::Crit);
    assert_eq!((r.st.reason, r.st.http_status), (Some(Reason::Http5xx), Some(503)));
    assert_eq!(r.proj.as_ref().unwrap().fields["indicator"], json!("none"), "last good value kept");
}

#[test]
fn not_modified_keeps_data_and_counts_as_success() {
    let mut r = Run::new(single());
    r.poll(vec![(None, ok(json!({"status": {"indicator": "none"}, "open": 4, "components": [{"name": "api", "status": "operational"}]})))]);
    assert_eq!(r.st.etag.as_deref(), Some("\"e1\""));
    let before = r.st.updated_at;
    let nm = FetchOutcome::Ok { status: 304, body: Json::Null, not_modified: true, etag: None, last_modified: None, elapsed_ms: 1 };
    let a = apply_poll(&r.st, PollInput { spec: &r.spec, outcomes: vec![(None, nm)], now: r.now + 60, simulated: false, prev_projection: r.proj.as_ref() });
    assert_eq!(a.state.updated_at, Some(r.now + 60));
    assert!(a.state.updated_at > before);
    assert_eq!(a.projection.unwrap().fields["open"], json!(4));
    assert_eq!(a.state.etag.as_deref(), Some("\"e1\""), "etag kept on 304");
    assert_eq!(a.points, vec![("open".to_string(), 4.0)]);
}

#[test]
fn keep_states_stores_keys_only_and_no_points() {
    let mut spec = single();
    spec.keep = Keep::States;
    let mut r = Run::new(spec);
    let a = apply_poll(&r.st, PollInput {
        spec: &r.spec,
        outcomes: vec![(None, ok(json!({"status": {"indicator": "major"}, "open": 9, "components": [{"name": "api", "status": "major_outage"}]})))],
        now: r.now, simulated: false, prev_projection: None,
    });
    let p = a.projection.unwrap();
    assert!(p.fields.is_empty());
    assert_eq!(p.items.len(), 1);
    assert!(p.items[0].fields.is_empty());
    assert!(a.points.is_empty());
    r.st = a.state;
    assert!(r.st.items.contains_key("api"));
}

#[test]
fn gaps_pause_and_the_next_poll_ends_them_with_incidents_carried_over() {
    let mut spec = fleet(1);
    spec.fail_after = 1;
    let mut r = Run::new(spec);
    r.fan(|_| fail(Reason::Timeout), 1);
    let inc = r.st.items["b0"].incident_id.clone().expect("open incident");
    r.st = apply_gap(&r.st, GapKind::DaemonRestarted, r.now, r.now + 600);
    assert!(r.st.in_gap());
    assert_eq!(r.st.phase, Phase::Paused);
    assert!(check_stale(&r.st, &r.spec, r.now + 10_000).is_none(), "a gapped source never goes stale on the clock");
    r.now += 600;
    let ts = r.fan(|_| fail(Reason::Timeout), 1);
    let health_changes: Vec<_> = hard(&ts).into_iter().filter(|(_, k)| *k != TransitionKind::WentStale).collect();
    assert!(health_changes.is_empty(), "still red: no new health transition across the gap: {health_changes:?}");
    assert_eq!(r.st.items["b0"].incident_id, Some(inc), "the incident carries over");
    assert!(!r.st.in_gap());
    assert!(r.st.gap_kind.is_none());
}

#[test]
fn check_stale_is_time_only_and_carries_an_episode_id() {
    let mut r = Run::new(single());
    r.poll(vec![(None, ok(json!({"components": []})))]);
    let u = r.st.updated_at.unwrap();
    assert!(check_stale(&r.st, &r.spec, u + 179).is_none());
    let (s, t) = check_stale(&r.st, &r.spec, u + 180).expect("stale");
    assert_eq!(s.phase, Phase::Stale);
    assert!(t.hard && t.kind == TransitionKind::WentStale);
    assert_eq!(t.incident_id, Some(stale_incident("s1", Some(u))));
    assert_eq!(stale_since(&s, &r.spec), Some(u + 180));
    assert!(check_stale(&s, &r.spec, u + 999).is_none(), "already stale");
}

#[test]
fn removed_targets_drop_their_items() {
    let mut r = Run::new(fleet(3));
    r.fan(|_| ready(), 3);
    r.spec = fleet(2);
    r.fan(|_| ready(), 2);
    assert_eq!(r.st.items.len(), 2);
}
