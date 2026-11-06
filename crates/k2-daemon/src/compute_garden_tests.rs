//! `compute.nodes` shaping (catalog v49): what the Compute Garden's load
//! tile reads, from facts built here (no live node), plus the closed shape
//! against the catalog's schema.

use serde_json::{json, Value as J};

use k2_core::compute::proto::frames::{self, Control, JobState, Offer};
use k2_core::compute::proto::share::{self as pshare, QueueEntry, QueueView, ServerStatus, ShareView};
use k2_core::compute::proto::tiers::Tier;
use k2_core::compute::store::{JobRow, NodeRow};

use super::*;

const GB: u64 = 1 << 30;
const NOW: i64 = 1_000_000;

fn row(name: &str) -> NodeRow {
    NodeRow {
        id: format!("id-{name}"),
        name: name.into(),
        fingerprint: String::new(),
        public_key_pem: String::new(),
        os: Some("linux".into()),
        arch: Some("x86_64".into()),
        labels: Default::default(),
        state: "active".into(),
        enrolled_via: "code".into(),
        enrolled_by: None,
        enrolled_at: 1,
        sas: None,
        confirmed_by: None,
        confirmed_at: None,
        revoked_by: None,
        revoked_at: None,
        controller_pause: None,
        controller_pause_by: None,
        controller_pause_at: None,
        last_seen: Some(NOW - 5),
        last_route: None,
        protocol: Some(6),
        node_version: Some("0.46.8".into()),
        routes: vec![],
        last_offer: None,
        role: "owner".into(),
        shared: false,
        owner_label: None,
        share: None,
        share_at: None,
    }
}

/// 8 CPUs and 32 GB under K2's caps, `running` of `slots` busy.
fn offer(running: u32, slots: u32) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 8000, mem_bytes: 32 * GB, disk_budget_bytes: 100 * GB, hard: true },
        free: frames::Free { cpu_millis: 8000 - u64::from(running) * 2000, mem_bytes: (32 - 4 * u64::from(running)) * GB, disk_free_bytes: 500 * GB },
        slots: vec![],
        running,
        parallel_max: slots,
        refusal: None,
        labels: Default::default(),
        tools: Default::default(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.8".into(),
        features: vec![], host: None, pkg: None,
    }
}

fn job(id: &str, ws: &str, state: &str, started: Option<i64>) -> JobRow {
    JobRow {
        id: id.into(),
        workspace_id: ws.into(),
        state: state.into(),
        argv: vec!["cargo".into(), "test".into(), "--secret-flag".into()],
        session_id: Some("sess-secret".into()),
        created_at: NOW - 300,
        started_at: started,
        tier: "normal".into(),
        ..Default::default()
    }
}

fn facts(row: NodeRow, online: bool, offer: Option<Offer>) -> NodeFacts {
    NodeFacts { row, online, offer, active: vec![], queued: vec![], load: None, idle_cpu_millis: None, queue: None }
}

/// Workspace ids `wsid-<x>` read as handles `agent-<x>` (the daemon's
/// handle lookup, in tests).
fn h(ws: &str) -> String {
    ws.replace("wsid-", "agent-")
}

fn shape(f: &NodeFacts) -> J {
    node(f, &h, NOW)
}

fn shared_view(servers: u32, state: ServerStatus) -> ShareView {
    ShareView {
        role: pshare::Role::Owner,
        slot: 0,
        shared: true,
        servers,
        owner_label: None,
        state,
        limits: pshare::ShareLimits::new_server(pshare::Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        quotes: vec![],
        priority: None,
    }
}

fn entry(pos: u32, mine: bool, state: JobState, job_id: Option<&str>, server: Option<&str>, blocked: Option<&str>) -> QueueEntry {
    QueueEntry {
        pos,
        mine,
        class: pshare::JobClass::Interactive,
        tier: Some(Tier::Normal),
        state,
        cpu_millis: 2000,
        mem_bytes: 4 * GB,
        eta_secs: None,
        eta_p90_secs: None,
        blocked_by: blocked.map(str::to_string),
        job_id: job_id.map(str::to_string),
        generation: None,
        workspace_id: None,
        session_id: None,
        server_label: server.map(str::to_string),
    }
}

/// A node nobody shares: this server's jobs, its offer's slots, free CPU
/// and memory, whose jobs they are, and nothing of a job but its state,
/// owner, tier and age.
#[test]
fn a_solo_node_shows_this_servers_jobs_and_free_room() {
    let mut f = facts(row("k2n12-vin"), true, Some(offer(1, 3)));
    f.active = vec![job("j-run", "wsid-a", "running", Some(NOW - 120))];
    let mut q = job("j-wait", "wsid-b", "queued", None);
    q.reason = Some("waiting_for_room".into());
    f.queued = vec![q];
    f.load = Some([1.5, 1.0, 0.5]);
    f.idle_cpu_millis = Some(6500);
    let v = shape(&f);
    assert_eq!(v["state"], "active", "{v}");
    assert_eq!(v["why"], J::Null);
    assert_eq!((v["slots"].as_u64(), v["running"].as_u64(), v["queued"].as_u64()), (Some(3), Some(1), Some(1)), "{v}");
    assert_eq!(v["queueFrom"], "server");
    assert_eq!(v["cpu"], json!({"freeMillis": 6000, "totalMillis": 8000, "idleMillis": 6500}));
    assert_eq!(v["mem"], json!({"freeBytes": 28 * GB, "totalBytes": 32 * GB}));
    assert_eq!(v["load"], json!([1.5, 1.0, 0.5]));
    assert_eq!(v["who"], json!([
        {"label": "agent-a", "kind": "workspace", "running": 1, "queued": 0},
        {"label": "agent-b", "kind": "workspace", "running": 0, "queued": 1},
    ]));
    assert_eq!(v["jobs"][0], json!({"state": "running", "workspace": "agent-a", "server": null, "tier": "normal", "elapsedSecs": 120, "blockedBy": null}));
    assert_eq!(v["jobs"][1]["state"], "queued");
    assert_eq!(v["jobs"][1]["elapsedSecs"], 300, "a queued job's wait");
    assert_eq!(v["jobs"][1]["blockedBy"], "waiting_for_room");
    let all = v.to_string();
    for secret in ["j-run", "j-wait", "cargo", "--secret-flag", "sess-secret", "wsid-", "id-k2n12"] {
        assert!(!all.contains(secret), "no job id, command, session, workspace id or node id: {secret} in {all}");
    }
}

/// A shared node that answered with its own queue: every server's jobs
/// count (queueFrom node), another server's jobs carry its name (or
/// "another server"), and this server's carry the agent's handle.
#[test]
fn a_shared_node_counts_every_servers_jobs_from_its_own_queue() {
    let mut r = row("k2n10-vin");
    r.shared = true;
    r.share = Some(shared_view(2, ServerStatus::Active));
    let mut f = facts(r, true, Some(offer(2, 3)));
    f.active = vec![job("mine-1", "wsid-a", "running", Some(NOW - 60))];
    f.queue = Some(Ok(QueueView {
        request_id: "r".into(),
        at: NOW,
        entries: vec![
            entry(1, true, JobState::Running, Some("mine-1"), None, None),
            entry(2, false, JobState::Running, None, Some("other.k2.dev"), None),
            entry(3, false, JobState::Queued, None, None, Some("waiting_for_room")),
            entry(4, false, JobState::Queued, None, Some("other.k2.dev"), Some("exclusive")),
        ],
        totals: vec![],
    }));
    let v = shape(&f);
    assert_eq!(v["queueFrom"], "node");
    assert_eq!((v["running"].as_u64(), v["queued"].as_u64()), (Some(2), Some(2)), "{v}");
    assert_eq!((v["shared"].as_bool(), v["servers"].as_u64()), (Some(true), Some(2)));
    assert_eq!(v["who"], json!([
        {"label": "agent-a", "kind": "workspace", "running": 1, "queued": 0},
        {"label": "another server", "kind": "server", "running": 0, "queued": 1},
        {"label": "other.k2.dev", "kind": "server", "running": 1, "queued": 1},
    ]));
    assert_eq!(v["jobs"][0]["workspace"], "agent-a");
    assert_eq!(v["jobs"][0]["elapsedSecs"], 60);
    assert_eq!(v["jobs"][1], json!({"state": "running", "workspace": null, "server": "other.k2.dev", "tier": "normal", "elapsedSecs": null, "blockedBy": null}));
    assert_eq!(v["jobs"][3]["blockedBy"], "exclusive");
    assert_eq!(v["queueError"], J::Null);
}

/// A shared node that didn't answer: this server's jobs, the offer's
/// node-wide running count, and why.
#[test]
fn a_shared_node_that_doesnt_answer_falls_back_to_this_servers_jobs() {
    let mut r = row("k2n11-vin");
    r.shared = true;
    r.share = Some(shared_view(2, ServerStatus::Active));
    let mut f = facts(r, true, Some(offer(2, 2)));
    f.queued = vec![job("q1", "wsid-a", "queued", None)];
    f.queue = Some(Err("the node didn't answer with its queue in time; try again".into()));
    let v = shape(&f);
    assert_eq!(v["queueFrom"], "server");
    assert_eq!(v["running"], 2, "the offer's node-wide count: {v}");
    assert_eq!(v["queued"], 1);
    assert!(v["queueError"].as_str().is_some_and(|e| e.contains("didn't answer")), "{v}");
}

/// Offline, paused on this server, paused or draining on the node, an
/// unavailable offer, a held queue and an ended share each say so.
#[test]
fn states_say_why_a_node_isnt_taking_jobs() {
    let v = shape(&facts(row("off"), false, Some(offer(0, 2))));
    assert_eq!((v["state"].as_str(), v["online"].as_bool(), v["slots"].as_u64()), (Some("offline"), Some(false), Some(2)), "{v}");
    assert_eq!(v["lastSeenAt"], NOW - 5);

    let mut r = row("held");
    r.controller_pause = Some("draining".into());
    r.controller_pause_by = Some("owner".into());
    let v = shape(&facts(r, true, Some(offer(0, 2))));
    assert_eq!(v["state"], "draining");
    assert!(v["why"].as_str().is_some_and(|w| w.contains("this server")), "{v}");

    let mut o = offer(0, 2);
    o.control = Control::Paused;
    o.control_by = Some("z3thon".into());
    let v = shape(&facts(row("p"), true, Some(o)));
    assert_eq!(v["state"], "paused");
    assert!(v["why"].as_str().is_some_and(|w| w.contains("on the node")), "{v}");

    let mut o = offer(0, 2);
    o.availability = frames::Availability { ok: false, reasons: vec!["on_battery".into()] };
    let v = shape(&facts(row("u"), true, Some(o)));
    assert_eq!((v["state"].as_str(), v["why"].as_str()), (Some("unavailable"), Some("on battery")), "{v}");

    let mut o = offer(0, 2);
    o.refusal = Some(frames::Refusal::new("foreign_lock", "sew-ci holds a lock"));
    let v = shape(&facts(row("l"), true, Some(o)));
    assert_eq!((v["state"].as_str(), v["why"].as_str()), (Some("active"), Some("sew-ci holds a lock")), "{v}");

    let mut r = row("s");
    r.share = Some(shared_view(2, ServerStatus::Paused));
    let v = shape(&facts(r, true, Some(offer(0, 2))));
    assert_eq!(v["state"], "unavailable");
    assert!(v["why"].as_str().is_some_and(|w| w.contains("paused")), "{v}");

    let v = shape(&facts(row("new"), true, None));
    assert_eq!((v["state"].as_str(), v["slots"].as_u64()), (Some("unavailable"), Some(0)), "{v}");
    assert_eq!(v["cpu"], json!({"freeMillis": null, "totalMillis": null, "idleMillis": null}));
}

/// Only nodes this server may use: never a pending enrolment or a share
/// that ended.
#[test]
fn only_usable_nodes_are_listed() {
    assert!(usable(&row("a")));
    let mut p = row("p");
    p.state = "pending".into();
    assert!(!usable(&p));
    for (st, ok) in [(ServerStatus::Active, true), (ServerStatus::Paused, true), (ServerStatus::Revoked, false), (ServerStatus::Expired, false)] {
        let mut r = row("s");
        r.share = Some(shared_view(2, st));
        assert_eq!(usable(&r), ok, "{st:?}");
    }
}

/// At most MAX_JOBS listed; the counts keep every job.
#[test]
fn a_long_queue_lists_twelve_and_counts_all() {
    let mut f = facts(row("busy"), true, Some(offer(0, 1)));
    f.queued = (0..20).map(|i| job(&format!("q{i}"), "wsid-a", "queued", None)).collect();
    let v = shape(&f);
    assert_eq!(v["queued"], 20);
    assert_eq!(v["jobs"].as_array().map(Vec::len), Some(MAX_JOBS));
}

/// The answer is exactly the catalog's closed shape: every required key,
/// no other key, the right types, at every depth.
#[test]
fn the_answer_is_the_catalogs_closed_shape() {
    let mut a = facts(row("b-shared"), true, Some(offer(2, 3)));
    a.row.shared = true;
    a.row.share = Some(shared_view(3, ServerStatus::Active));
    a.active = vec![job("mine-1", "wsid-a", "running", Some(NOW - 60))];
    a.load = Some([0.5, 0.5, 0.5]);
    a.queue = Some(Ok(QueueView {
        request_id: "r".into(),
        at: NOW,
        entries: vec![entry(1, true, JobState::Running, Some("mine-1"), None, None), entry(2, false, JobState::Queued, None, None, Some("exclusive"))],
        totals: vec![],
    }));
    let b = facts(row("a-offline"), false, None);
    let v = card(&[a, b], Some("rosson.k2.dev".into()), NOW);
    assert_eq!(v["nodes"][0]["name"], "a-offline", "by name");
    assert_eq!(v["controller"], "rosson.k2.dev");
    let row = k2_core::contract::catalog().verb(VERB).expect("the compute.nodes row");
    assert_eq!(row.cap.as_deref(), Some(CAP));
    assert_eq!(row.feature, FEATURE);
    let mut errs = Vec::new();
    check(&row.response, &v, "$", &mut errs);
    assert!(errs.is_empty(), "{errs:#?}\n{v:#}");
}

fn check(s: &J, v: &J, at: &str, errs: &mut Vec<String>) {
    let types: Vec<&str> = match &s["type"] {
        J::String(t) => vec![t.as_str()],
        J::Array(a) => a.iter().filter_map(J::as_str).collect(),
        _ => vec![],
    };
    if let Some(e) = s.get("enum").and_then(J::as_array) {
        if !e.contains(v) {
            errs.push(format!("{at}: {v} isn't one of {e:?}"));
        }
        return;
    }
    let kind = match v {
        J::Null => "null",
        J::Bool(_) => "boolean",
        J::Number(n) if n.is_i64() || n.is_u64() => "integer",
        J::Number(_) => "number",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    };
    if !types.contains(&kind) && !(kind == "integer" && types.contains(&"number")) {
        errs.push(format!("{at}: {kind}, want {types:?}"));
        return;
    }
    match v {
        J::Object(o) => {
            let props = s["properties"].as_object().expect("an object schema lists properties");
            for k in o.keys() {
                if !props.contains_key(k) {
                    errs.push(format!("{at}.{k}: not in the schema"));
                }
            }
            for r in s["required"].as_array().expect("required").iter().filter_map(J::as_str) {
                if !o.contains_key(r) {
                    errs.push(format!("{at}.{r}: missing"));
                }
            }
            for (k, x) in o {
                if let Some(p) = props.get(k) {
                    check(p, x, &format!("{at}.{k}"), errs);
                }
            }
        }
        J::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                check(&s["items"], x, &format!("{at}[{i}]"), errs);
            }
        }
        _ => {}
    }
}
