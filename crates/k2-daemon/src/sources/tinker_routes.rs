//! `try` and `simulate` handlers (contract §4.2, §4.3; PRD §9.1, §9.2).
//! Owner: B2.
//!
//! `crate::sources_routes::handle` has already classified the caller and
//! refused app passes and invalid tokens; both transports refuse a GET to a
//! POST-only path with 405.
//!
//! **Try** fetches once (a draft, or a saved source by name) and answers
//! with the status, timing and fixed reason, the shape tree of the
//! (redacted) body, and the card view this one answer would make. Nothing
//! is saved and the tree is never stored. Fan-out drafts try their first 3
//! targets. Agents may try secretless sources (samples shown); naming a
//! secret makes it person-only; LAN and plain http are the owner's
//! (`validate`) [GV54]. Try has its own per-origin bucket at the poller's
//! rate (1 every 2 s, burst 4); 429 `rate_limited` past it.
//!
//! **Simulate** sets a fake state for testing cards (`ok`, `warn`, `crit`,
//! `stale`, `unknown`, `slow`; `off` clears it), 1 h by default. Every card
//! shows TEST; triggers never fire for it (they read `simulate_state`);
//! history isn't written (the poller skips points while it's on). Agents
//! up to 1 h. Allowed on pinned sources and ones a locked Garden reads: it
//! changes nothing lasting and is clearly marked [GV65]. Turning it off
//! re-judges open incidents (`wake::handle_unmute`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use k2_core::sources::spec::{FetchTargets, SimulateState, SourceStatus, Validated};
use k2_core::sources::state::{apply_poll, FetchOutcome, PollInput, SourceState};
use k2_core::sources::view::{self, Viewer};
use k2_core::sources::{lint, presets, store, try_tree, FetchPlan, SourceRow, SourceSpec, SpecError, UrlRef};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::compute_routes::Who;

use super::approve_routes::util::*;
use super::budget::{Budget, Take};
use super::SourceErr;

pub fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    let _ = params;
    match path {
        "/cli/sources/try" => try_route(who, body),
        "/cli/sources/simulate" => simulate(who, body),
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    }
}

/// Targets a fan-out Try fetches.
pub const TRY_TARGETS: usize = 3;

fn spec_err(e: SpecError) -> SourceErr {
    match e.code {
        "lan_owner_only" => SourceErr::new("lan_owner_only", 403, e.hint).with("field", e.field),
        _ => SourceErr::new("bad_spec", 400, e.hint).with("field", e.field).with("specCode", e.code),
    }
}

fn viewer(who: &Who) -> Viewer {
    if is_person(who) {
        Viewer::Person
    } else {
        Viewer::Agent
    }
}

// ── try ──────────────────────────────────────────────────────────────

/// What a Try runs: the spec, where it may go, and the row the card view is
/// built from (a saved source's own row, or a draft's stand-in).
struct Subject {
    spec: SourceSpec,
    row: SourceRow,
    warnings: Vec<k2_core::sources::SpecWarning>,
    saved: bool,
}

/// A draft's stand-in row (never stored).
fn draft_row(v: &Validated, now: i64) -> SourceRow {
    let s = &v.spec;
    SourceRow {
        id: String::new(),
        name: s.name.clone(),
        label: s.label.clone(),
        kind: s.kind,
        preset: s.preset.clone(),
        spec_json: s.canonical_json(),
        spec_hash: s.hash(),
        origins: v.origins.clone(),
        lan: v.lan,
        secret_name: s.secret.clone(),
        url_secret_name: match &s.fetch {
            FetchTargets::UrlSecret { name, .. } => Some(name.clone()),
            _ => None,
        },
        secret_bound: false,
        every_secs: s.every_secs,
        timeout_secs: s.timeout_secs,
        max_bytes: s.max_bytes,
        fail_after: s.fail_after,
        on_target_error: s.on_target_error,
        link: s.link.clone(),
        viewers: s.viewers.unwrap_or(k2_core::sources::Viewers::Member),
        agent_access: s.agent_access.unwrap_or(k2_core::sources::AgentAccess::Values),
        keep: s.keep,
        pinned: false,
        pinned_by: None,
        pinned_at: None,
        status: SourceStatus::Proposed,
        pause_reason: None,
        made_by: "owner".into(),
        made_by_project: None,
        approved_by: None,
        approved_at: None,
        approved_spec_hash: None,
        simulate_state: None,
        simulate_until: None,
        simulate_by: None,
        mute_until: None,
        mute_by: None,
        mute_reason: None,
        rev: 0,
        created_at: now,
        updated_at: now,
        removed_at: None,
    }
}

fn subject(who: &Who, body: &[u8], now: i64) -> Result<Subject, SourceErr> {
    let v: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(body).map_err(|e| bad_request(format!("bad JSON body: {e}")))?
    };
    let obj = v.as_object().ok_or_else(|| bad_request("the body is a JSON object"))?;
    // `{"name": "fleet"}` alone: try the saved source.
    if obj.len() == 1 {
        if let Some(name) = obj.get("name").and_then(Value::as_str) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = source_by_name(&conn, name)?;
            check_viewers(who, &row)?;
            let spec = store::spec_of(&row).map_err(|e| SourceErr::new("db_error", 500, e))?;
            return Ok(Subject { spec, row, warnings: Vec::new(), saved: true });
        }
    }
    let mut draft = v.clone();
    if let Value::Object(m) = &mut draft {
        m.entry("name").or_insert_with(|| json!("try"));
        m.entry("label").or_insert_with(|| json!("Try"));
        m.entry("everySecs").or_insert_with(|| json!(60));
    }
    let spec = presets::expand(&draft).map_err(spec_err)?;
    let validated = spec.validate(who.is_owner()).map_err(spec_err)?;
    let row = draft_row(&validated, now);
    Ok(Subject { spec: validated.spec, row, warnings: validated.warnings, saved: false })
}

/// The requests a Try makes: one, or the first 3 fan-out targets.
fn plans(s: &Subject) -> Result<Vec<(Option<String>, FetchPlan)>, SourceErr> {
    let spec = &s.spec;
    let plan = |url: UrlRef, origin: String, key: Option<String>| FetchPlan {
        source_id: s.saved.then(|| s.row.id.clone()),
        target_key: key,
        url,
        origin,
        lan_exact_ip: s.row.lan,
        headers: spec.headers.clone(),
        secret: spec.secret.clone(),
        timeout_secs: spec.timeout_secs,
        max_bytes: spec.max_bytes,
    };
    let origin = |u: &str| lint::origin_of(u, "fetch", true).map(|p| p.origin).map_err(spec_err);
    Ok(match &spec.fetch {
        FetchTargets::Url(u) => vec![(None, plan(UrlRef::Plain(u.clone()), origin(u)?, None))],
        FetchTargets::UrlSecret { name, origin: o } => {
            let o = lint::origin_of(o, "fetch.urlSecret.origin", true).map(|p| p.origin).unwrap_or_else(|_| o.clone());
            vec![(None, plan(UrlRef::Secret { name: name.clone() }, o, None))]
        }
        FetchTargets::Targets(ts) => ts
            .iter()
            .take(TRY_TARGETS)
            .map(|t| Ok((Some(t.key.clone()), plan(UrlRef::Plain(t.url.clone()), origin(&t.url)?, Some(t.key.clone())))))
            .collect::<Result<Vec<_>, SourceErr>>()?,
    })
}

// TODO(sources 0.45.7): share the engine's per-origin budget instead of a
// second bucket. Contract §4.3 says Try "uses the same per-origin budget";
// today Try and the poller each get 1 request / 2 s per origin, so a Try
// burst can briefly double what K2 sends to one origin.
static TRY_BUDGET: Mutex<Option<Budget>> = Mutex::new(None);

/// One token per origin, or 429 with the wait.
fn take_budget(origins: &[String]) -> Result<(), SourceErr> {
    #[cfg(test)]
    if test_seams::no_budget() {
        return Ok(());
    }
    let mut g = TRY_BUDGET.lock().unwrap_or_else(|e| e.into_inner());
    let b = g.get_or_insert_with(Budget::new);
    let now = Instant::now();
    for o in origins {
        match b.try_take(o, now) {
            Take::Go => {}
            Take::Wait(at) => {
                let secs = at.saturating_duration_since(now).as_secs().max(1);
                return Err(SourceErr::new("rate_limited", 429, format!("K2 is holding requests to {o}; try again in {secs} s"))
                    .with("retryAfterSecs", secs));
            }
            Take::Parked(p) => {
                let secs = p.until.saturating_duration_since(now).as_secs().max(1);
                return Err(SourceErr::new("rate_limited", 429, format!("{o} asked K2 to slow down; try again later"))
                    .with("retryAfterSecs", secs));
            }
        }
    }
    Ok(())
}

/// Air-gap (a per-thread override in tests: other tests flip the process
/// flag).
fn airgap() -> bool {
    #[cfg(test)]
    if let Some(on) = test_seams::airgap() {
        return on;
    }
    k2_core::airgap::enabled()
}

fn fetch(plan: &FetchPlan) -> FetchOutcome {
    #[cfg(test)]
    if let Some(f) = test_seams::fetcher() {
        return f(plan);
    }
    super::fetch::fetch_once(plan)
}

fn try_route(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let now = now();
    let s = subject(who, body, now)?;
    let named_secret = s.spec.secret.is_some() || matches!(s.spec.fetch, FetchTargets::UrlSecret { .. });
    if named_secret && !is_person(who) {
        return Err(SourceErr::new(
            "person_only",
            403,
            "Try with a secret is for a person: ask the person to Try this in Settings → Gardens → Data Sources.",
        ));
    }
    if s.row.lan && !who.is_owner() {
        return Err(SourceErr::new("lan_owner_only", 403, "LAN addresses and plain http are the owner's"));
    }
    let plans = plans(&s)?;
    let airgapped = airgap() && !s.row.lan;
    let mut outcomes: Vec<(Option<String>, FetchOutcome)> = Vec::new();
    let mut targets = Vec::new();
    let mut elapsed_total: u32 = 0;
    if airgapped {
        for (key, _) in &plans {
            outcomes.push((key.clone(), FetchOutcome::Err { reason: k2_core::sources::Reason::Airgap, http_status: None, retry_after_secs: None }));
        }
    } else {
        let mut origins: Vec<String> = plans.iter().map(|(_, p)| p.origin.clone()).collect();
        origins.sort();
        origins.dedup();
        take_budget(&origins)?;
        for (key, plan) in &plans {
            let started = Instant::now();
            let out = fetch(plan);
            let ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
            let ms = match &out {
                FetchOutcome::Ok { elapsed_ms, .. } => *elapsed_ms,
                _ => ms,
            };
            elapsed_total = elapsed_total.saturating_add(ms);
            outcomes.push((key.clone(), out));
        }
    }
    let fanout = matches!(s.spec.fetch, FetchTargets::Targets(_));
    for (key, out) in &outcomes {
        if let Some(k) = key {
            let (status, reason, ms) = match out {
                FetchOutcome::Ok { status, elapsed_ms, .. } => (Some(*status), None, *elapsed_ms),
                FetchOutcome::Err { reason, http_status, .. } => (*http_status, Some(*reason), 0),
            };
            targets.push(json!({"key": k, "status": status, "reason": reason, "elapsedMs": ms}));
        }
    }
    // The first answer that parsed: its status and shape.
    let first_ok = outcomes.iter().find_map(|(_, o)| match o {
        FetchOutcome::Ok { status, body, .. } => Some((*status, body)),
        _ => None,
    });
    let first = outcomes.first().map(|(_, o)| o);
    let (ok, status, reason) = match (first_ok, first) {
        (Some((st, _)), _) => (true, Some(st), None),
        (None, Some(FetchOutcome::Err { reason, http_status, .. })) => (false, *http_status, Some(*reason)),
        _ => (false, None, None),
    };
    let truncated = matches!(reason, Some(k2_core::sources::Reason::TooLarge));
    let (tree, tree_truncated) = match first_ok {
        Some((_, body)) => try_tree::build(body),
        None => (Vec::new(), false),
    };
    // The card view this one answer makes (hard at once: one answer).
    let mut once = s.spec.clone();
    once.fail_after = 1;
    let applied = apply_poll(
        &SourceState::new(&s.row.id),
        PollInput { spec: &once, outcomes, now, simulated: false, prev_projection: None },
    );
    let mut row = s.row.clone();
    row.fail_after = 1;
    row.simulate_state = None;
    row.simulate_until = None;
    row.mute_until = None;
    row.mute_by = None;
    if row.status == SourceStatus::Paused {
        row.status = SourceStatus::Active;
    }
    let card = view::build(&row, &applied.state, applied.projection.as_ref(), viewer(who), now);
    let root_key = match (&s.spec.secret, is_person(who)) {
        (Some(name), true) => super::secrets::root_key_hint_for(name).ok().flatten(),
        _ => None,
    };
    let mut out = json!({
        "ok": ok,
        "status": status,
        "elapsedMs": elapsed_total,
        "reason": reason,
        "truncated": truncated,
        "tree": tree,
        "treeTruncated": tree_truncated,
        "view": card,
        "warnings": s.warnings,
        "rootKey": root_key,
    });
    if fanout {
        out["targets"] = Value::Array(targets);
    }
    Ok(out)
}

// ── simulate ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SimulateBody {
    name: String,
    state: String,
    #[serde(default)]
    for_secs: Option<u32>,
}

fn simulate(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: SimulateBody = parse(body)?;
    let state: Option<SimulateState> = match b.state.as_str() {
        "off" => None,
        s => Some(
            serde_json::from_value(Value::String(s.to_string()))
                .map_err(|_| bad_request("state is ok, warn, crit, stale, unknown, slow or off"))?,
        ),
    };
    let secs = b.for_secs.unwrap_or(k2_core::sources::SIMULATE_DEFAULT_SECS);
    let max = if is_person(who) { k2_core::sources::PERSON_MUTE_MAX_SECS } else { k2_core::sources::AGENT_SIMULATE_MAX_SECS };
    if state.is_some() && (secs == 0 || secs > max) {
        return Err(SourceErr::new(
            "too_long",
            403,
            if is_person(who) { "simulate for up to a day" } else { "an agent simulates for up to 1 hour" },
        ));
    }
    let now = now();
    let (actor, _) = actor(who);
    let (after, rev) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = source_by_name(&conn, &b.name)?;
        check_viewers(who, &row)?;
        let until = state.map(|_| now + i64::from(secs));
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let rev = store::set_simulate(&tx, &row.id, state, until, Some(&actor), now).map_err(db_err)?;
        audit(
            &tx,
            who,
            if state.is_some() { "simulate" } else { "simulate_off" },
            Some(&row.id),
            None,
            json!({"name": row.name, "state": state, "forSecs": state.map(|_| secs)}),
        );
        tx.commit().map_err(db_err)?;
        let after = store::get_by_id(&conn, &row.id).map_err(db_err)?.ok_or_else(|| SourceErr::new("source_not_found", 404, "gone"))?;
        (after, rev)
    };
    super::notify_changed(rev);
    if state.is_none() {
        super::wake::handle_unmute(&after, now);
    }
    let simulated = match (after.simulate_state, after.simulate_until) {
        (Some(st), Some(until)) => json!({"state": st, "until": until, "by": after.simulate_by}),
        _ => Value::Null,
    };
    Ok(json!({"name": after.name, "simulated": simulated}))
}

/// Test seams: a fake fetcher (never the network in tests) and no budget.
#[cfg(test)]
pub mod test_seams {
    use std::cell::{Cell, RefCell};

    use k2_core::sources::{FetchOutcome, FetchPlan};

    pub type Fetch = fn(&FetchPlan) -> FetchOutcome;

    thread_local! {
        static FETCH: RefCell<Option<Fetch>> = const { RefCell::new(None) };
        static NO_BUDGET: Cell<bool> = const { Cell::new(false) };
        static AIRGAP: Cell<Option<bool>> = const { Cell::new(None) };
    }

    pub fn set_airgap(on: Option<bool>) {
        AIRGAP.with(|c| c.set(on));
    }

    pub(super) fn airgap() -> Option<bool> {
        AIRGAP.with(|c| c.get())
    }

    pub fn set_fetcher(f: Option<Fetch>) {
        FETCH.with(|c| *c.borrow_mut() = f);
    }

    pub(super) fn fetcher() -> Option<Fetch> {
        FETCH.with(|c| *c.borrow())
    }

    pub fn set_no_budget(on: bool) {
        NO_BUDGET.with(|c| c.set(on));
    }

    pub(super) fn no_budget() -> bool {
        NO_BUDGET.with(|c| c.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::approve_routes::tests::{admin, agent, env, insert_source};
    use k2_core::sources::Reason;

    fn call(path: &str, who: &Who, body: Value) -> Result<Value, SourceErr> {
        handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    }

    fn deployments(plan: &FetchPlan) -> FetchOutcome {
        if plan.origin.contains("down.") {
            return FetchOutcome::Err { reason: Reason::Timeout, http_status: None, retry_after_secs: None };
        }
        FetchOutcome::Ok {
            status: 200,
            body: json!({"deployments": [
                {"uid": "d1", "state": "READY"},
                {"uid": "d2", "state": "ERROR"},
                {"uid": "d3", "state": "READY"},
                {"uid": "d4", "state": "READY"}
            ], "version": "0.45.6"}),
            not_modified: false,
            etag: None,
            last_modified: None,
            elapsed_ms: 12,
        }
    }

    struct Guard(#[allow(dead_code)] crate::sources::approve_routes::tests::Env);
    impl Drop for Guard {
        fn drop(&mut self) {
            test_seams::set_fetcher(None);
            test_seams::set_no_budget(false);
            test_seams::set_airgap(None);
        }
    }
    fn setup() -> Guard {
        let e = env();
        test_seams::set_fetcher(Some(deployments));
        test_seams::set_no_budget(true);
        test_seams::set_airgap(Some(false));
        Guard(e)
    }

    fn count_sources() -> i64 {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn try_a_draft_shows_the_tree_and_the_card_and_saves_nothing() {
        let _g = setup();
        let before = count_sources();
        let r = call(
            "/cli/sources/try",
            &agent("p1"),
            json!({
                "fetch": {"url": "https://api.example.com/v6/deployments?limit=5"},
                "everySecs": 180,
                "extract": {"items": {"pointer": "/deployments", "itemKey": "/uid", "itemFields": {"state": "/state"}}},
                "rules": [{"field": "state", "crit": {"eq": "ERROR"}, "ok": {"eq": "READY"}}]
            }),
        )
        .unwrap();
        assert_eq!((r["ok"].as_bool(), r["status"].as_u64(), r["elapsedMs"].as_u64()), (Some(true), Some(200), Some(12)));
        let ptrs: Vec<&str> = r["tree"].as_array().unwrap().iter().map(|n| n["pointer"].as_str().unwrap()).collect();
        assert!(ptrs.contains(&"/deployments/0/state"));
        assert!(!ptrs.contains(&"/deployments/3"), "arrays show 3");
        let sample = r["tree"].as_array().unwrap().iter().find(|n| n["pointer"] == "/version").unwrap()["sample"].clone();
        assert_eq!(sample, "0.45.6", "agents see samples of secretless answers");
        assert_eq!(r["view"]["phase"], "live");
        assert_eq!(r["view"]["health"], "crit", "d2 is ERROR");
        assert_eq!(r["view"]["counts"]["crit"], 1);
        assert!(r.get("targets").is_none());
        assert_eq!(count_sources(), before, "Try saves nothing");
    }

    #[test]
    fn fan_out_tries_the_first_three_targets() {
        let _g = setup();
        let targets: Vec<Value> = (0..5)
            .map(|i| {
                let host = if i == 1 { "down.example.com".to_string() } else { format!("box{i}.example.com") };
                json!({"key": format!("box{i}"), "url": format!("https://{host}/boot-status")})
            })
            .collect();
        let r = call(
            "/cli/sources/try",
            &admin(),
            json!({"fetch": {"targets": targets}, "extract": {"items": {"itemFields": {"version": "/version"}}}}),
        )
        .unwrap();
        let t = r["targets"].as_array().unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[1]["reason"], "timeout");
        assert_eq!(r["view"]["items"].as_array().unwrap().iter().filter(|i| i["health"] == "crit").count(), 1);
        assert_eq!(r["view"]["phase"], "live", "one down box is a red item, not a stale source");
    }

    #[test]
    fn agents_cant_try_with_a_secret_or_a_lan_address() {
        let _g = setup();
        let e = call("/cli/sources/try", &agent("p1"), json!({"fetch": {"url": "https://api.stripe.com/v1/x"}, "secret": "STRIPE_RK"}))
            .unwrap_err();
        assert_eq!((e.code, e.status), ("person_only", 403));
        let e = call("/cli/sources/try", &agent("p1"), json!({"fetch": {"url": "http://192.168.1.5:8080/status"}})).unwrap_err();
        assert_eq!((e.code, e.status), ("lan_owner_only", 403));
        let e = call("/cli/sources/try", &admin(), json!({"fetch": {"url": "http://192.168.1.5:8080/status"}})).unwrap_err();
        assert_eq!(e.code, "lan_owner_only", "an Admin isn't the owner either");
        let e = call("/cli/sources/try", &admin(), json!({"fetch": {"url": "http://example.com/x"}})).unwrap_err();
        assert_eq!((e.code, e.extra["specCode"].as_str()), ("bad_spec", Some("https_only")));
    }

    #[test]
    fn try_a_saved_source_by_name() {
        let _g = setup();
        insert_source("fleet", "active", None, false, "member");
        {
            let h = k2_core::db::shared();
            let c = h.lock();
            c.execute(
                "UPDATE sources SET spec_json = '{\"fetch\":{\"url\":\"https://api.example.com/x\"},\"extract\":{\"fields\":{\"version\":\"/version\"}}}' WHERE name = 'fleet'",
                [],
            )
            .unwrap();
        }
        let r = call("/cli/sources/try", &agent("p1"), json!({"name": "fleet"})).unwrap();
        assert_eq!(r["view"]["name"], "fleet");
        assert_eq!(r["view"]["fields"]["version"], "0.45.6");
        let e = call("/cli/sources/try", &agent("p1"), json!({"name": "ghost"})).unwrap_err();
        assert_eq!(e.code, "source_not_found");
    }

    #[test]
    fn try_shares_a_per_origin_budget() {
        let _g = setup();
        test_seams::set_no_budget(false);
        let body = json!({"fetch": {"url": "https://budget-test-origin.example.com/x"}});
        for _ in 0..4 {
            call("/cli/sources/try", &admin(), body.clone()).unwrap();
        }
        let e = call("/cli/sources/try", &admin(), body).unwrap_err();
        assert_eq!((e.code, e.status), ("rate_limited", 429));
        assert!(e.extra["retryAfterSecs"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn simulate_sets_and_clears_with_agent_limits() {
        let _g = setup();
        insert_source("fleet", "active", None, false, "member");
        {
            let h = k2_core::db::shared();
            let c = h.lock();
            c.execute("UPDATE sources SET pinned = 1, pinned_by = 'owner', pinned_at = 1 WHERE name = 'fleet'", []).unwrap();
        }
        let r = call("/cli/sources/simulate", &agent("p1"), json!({"name": "fleet", "state": "crit"})).unwrap();
        assert_eq!(r["simulated"]["state"], "crit", "allowed on a pinned source");
        assert_eq!(r["simulated"]["by"], "agent:p1");
        let until = r["simulated"]["until"].as_i64().unwrap();
        assert!((until - now() - 3600).abs() <= 2, "1 h by default");
        let e = call("/cli/sources/simulate", &agent("p1"), json!({"name": "fleet", "state": "warn", "forSecs": 7200})).unwrap_err();
        assert_eq!((e.code, e.status), ("too_long", 403));
        let r = call("/cli/sources/simulate", &admin(), json!({"name": "fleet", "state": "stale", "forSecs": 7200})).unwrap();
        assert_eq!(r["simulated"]["state"], "stale");
        let e = call("/cli/sources/simulate", &admin(), json!({"name": "fleet", "state": "purple"})).unwrap_err();
        assert_eq!(e.code, "bad_request");
        let r = call("/cli/sources/simulate", &admin(), json!({"name": "fleet", "state": "off"})).unwrap();
        assert_eq!(r["simulated"], Value::Null);
        let h = k2_core::db::shared();
        let c = h.lock();
        let acts: Vec<String> = c
            .prepare("SELECT action FROM source_audit ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(acts, vec!["simulate", "simulate", "simulate_off"]);
    }
}
