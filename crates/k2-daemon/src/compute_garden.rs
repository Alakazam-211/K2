//! `compute.nodes` (catalog v49, feature `compute-nodes-v1`, cap
//! `compute:nodes`): the Compute Garden's node load tile (Rosson
//! 2026-11-03). Garden-local and read only, through `POST /cli/zen/call`.
//!
//! The same facts `k2 compute status` and `k2 compute queue <node>` give
//! the owner, shaped for a tile: per node its state, jobs running against
//! its slots, jobs queued, free CPU and memory, and whose jobs they are.
//!
//! - **Which nodes.** Every node this server may use: confirmed (`active`)
//!   rows only, never one still pending its code or revoked, and never a
//!   shared node whose share to this server ended (revoked or expired).
//! - **Node-wide counts.** On a node several servers share, the node's own
//!   queue (`queue_get`, what `k2 compute queue` asks for) gives every
//!   server's running and queued jobs (`queueFrom: "node"`). Those asks
//!   run in parallel, wait at most [`QUEUE_WAIT`], and each node's answer
//!   is kept for [`QUEUE_KEEP`], so a tile on the desktop and one on a
//!   phone never send a node more than one ask per window. A node that
//!   doesn't answer (or one nobody shares) shows this server's own jobs
//!   (`queueFrom: "server"`), with `queueError` saying why.
//! - **What a widget sees.** A widget's code is not the person: agent
//!   handles and other servers' names, never a job's command, id, folder,
//!   session or environment (the response schema is closed, and the answer
//!   is projected onto it).

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use k2_core::compute::proto::frames::{Control, Offer};
use k2_core::compute::proto::share::{QueueView, ServerStatus};
use k2_core::compute::proto::tiers::Tier;
use k2_core::compute::{self as compute, share, store};

use crate::compute_routes::ws_handle;
use crate::compute_ws;

/// The `/boot-status` and frame feature key that brings `compute.nodes`.
pub const FEATURE: &str = "compute-nodes-v1";
/// The verb and the cap (catalog v49).
pub const VERB: &str = "compute.nodes";
pub const CAP: &str = "compute:nodes";
/// At most this many jobs listed per node (running first, then the queue).
pub const MAX_JOBS: usize = 12;
/// How long one node's queue ask may take.
pub const QUEUE_WAIT: Duration = Duration::from_secs(4);
/// How long a node's queue answer (or its failure) is reused.
pub const QUEUE_KEEP: Duration = Duration::from_secs(10);
/// What a node another server owns calls "the other server" when it
/// doesn't name it.
pub const ANOTHER_SERVER: &str = "another server";

/// What K2 knows about one node, gathered before shaping (so the shaping is
/// a pure function the tests drive).
#[derive(Debug, Clone)]
pub struct NodeFacts {
    pub row: store::NodeRow,
    pub online: bool,
    /// The live offer, else the last one stored.
    pub offer: Option<Offer>,
    /// This server's jobs on the node: started (`ACTIVE_STATES`, minus
    /// those waiting in the node's own queue) and waiting (this server's
    /// queue plus the node-queued ones).
    pub active: Vec<store::JobRow>,
    pub queued: Vec<store::JobRow>,
    /// 1/5/15-minute load (never for a node another server owns).
    pub load: Option<[f64; 3]>,
    pub idle_cpu_millis: Option<u64>,
    /// The node's own queue (a shared node, online): the answer or why not.
    pub queue: Option<Result<QueueView, String>>,
}

/// The answer to `compute.nodes`.
pub fn answer() -> J {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let rows: Vec<store::NodeRow> = rows.into_iter().filter(usable).collect();
    let facts = gather(rows);
    card(&facts, Some(compute_ws::controller_label()), compute::now())
}

/// A node this server may use: confirmed, and (for a shared node) a share
/// that hasn't ended.
pub fn usable(n: &store::NodeRow) -> bool {
    n.state == "active" && !n.share.as_ref().is_some_and(|s| matches!(s.state, ServerStatus::Revoked | ServerStatus::Expired))
}

fn gather(rows: Vec<store::NodeRow>) -> Vec<NodeFacts> {
    let mut facts: Vec<NodeFacts> = rows
        .into_iter()
        .map(|row| {
            let (online, _, live) = compute_ws::live_view(&row.id);
            let offer = live.or_else(|| row.last_offer.clone());
            let (active, queued, health) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                (
                    store::jobs_in_states(&conn, &row.id, store::ACTIVE_STATES),
                    store::jobs_in_states(&conn, &row.id, &["queued"]),
                    compute::admin::health(&conn, &row.id),
                )
            };
            // A shared node's queued jobs stay `assigned` here: queued, not running.
            let (node_queued, active): (Vec<_>, Vec<_>) = active.into_iter().partition(|r| r.node_queued);
            let mut waiting = node_queued;
            waiting.extend(queued);
            let load = if row.is_tenant() {
                None
            } else {
                health.as_ref().filter(|(at, _)| compute::now() - at <= 900).and_then(|(_, h)| h.load_milli).map(|l| l.map(|x| x as f64 / 1000.0))
            };
            let idle_cpu_millis = crate::compute_views::idle_cpu_millis(&row.id, offer.as_ref());
            NodeFacts { row, online, offer, active, queued: waiting, load, idle_cpu_millis, queue: None }
        })
        .collect();
    // The node's own queue, for every shared node online, in parallel.
    let ask: Vec<usize> =
        facts.iter().enumerate().filter(|(_, f)| f.online && f.offer.as_ref().is_some_and(share::is_shared)).map(|(i, _)| i).collect();
    let answers: Vec<(usize, Result<QueueView, String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = ask
            .iter()
            .map(|&i| {
                let id = facts[i].row.id.clone();
                (i, s.spawn(move || node_queue(&id)))
            })
            .collect();
        handles.into_iter().map(|(i, h)| (i, h.join().unwrap_or_else(|_| Err("the queue read stopped".to_string())))).collect()
    });
    for (i, q) in answers {
        facts[i].queue = Some(q);
    }
    facts
}

/// Kept queue answers, per node id.
static QUEUES: LazyLock<Mutex<HashMap<String, (Instant, Result<QueueView, String>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// One node's queue, reused for [`QUEUE_KEEP`].
fn node_queue(node_id: &str) -> Result<QueueView, String> {
    if let Some((at, q)) = QUEUES.lock().unwrap_or_else(|p| p.into_inner()).get(node_id) {
        if at.elapsed() < QUEUE_KEEP {
            return q.clone();
        }
    }
    let q = compute_ws::queue_get(node_id, QUEUE_WAIT).map_err(|(_, hint)| hint);
    QUEUES.lock().unwrap_or_else(|p| p.into_inner()).insert(node_id.to_string(), (Instant::now(), q.clone()));
    q
}

/// The answer from gathered facts, nodes by name.
pub fn card(facts: &[NodeFacts], controller: Option<String>, now: i64) -> J {
    let mut nodes: Vec<&NodeFacts> = facts.iter().collect();
    nodes.sort_by(|a, b| a.row.name.cmp(&b.row.name));
    let nodes: Vec<J> = nodes.into_iter().map(|f| node(f, &ws_handle, now)).collect();
    let v = json!({ "at": now, "controller": controller, "nodes": nodes });
    match k2_core::contract::catalog().verb(VERB) {
        Some(row) => crate::garden_data::person_rows::project(&row.response, &v, &BTreeMap::new()),
        None => v,
    }
}

/// One job as the tile lists it.
struct Job {
    running: bool,
    workspace: Option<String>,
    server: Option<String>,
    tier: String,
    elapsed: Option<i64>,
    blocked_by: Option<String>,
}

/// One node's row. `handle` names a workspace id (an agent's handle here).
pub fn node(f: &NodeFacts, handle: &dyn Fn(&str) -> String, now: i64) -> J {
    let n = &f.row;
    let offer = f.offer.as_ref();
    let label = |k: &str| n.labels.get(k).cloned().or_else(|| offer.and_then(|o| o.labels.get(k).cloned()));
    // This server's own jobs, as `k2 compute queue` lists a node nobody shares.
    let own: Vec<Job> = f
        .active
        .iter()
        .map(|r| Job {
            running: true,
            workspace: Some(handle(&r.workspace_id)),
            server: None,
            tier: r.tier().as_str().to_string(),
            elapsed: r.started_at.or(r.assigned_at).map(|t| (now - t).max(0)),
            blocked_by: None,
        })
        .chain(f.queued.iter().map(|r| Job {
            running: false,
            workspace: Some(handle(&r.workspace_id)),
            server: None,
            tier: r.tier().as_str().to_string(),
            elapsed: Some((now - if r.ctl.queued_at > 0 { r.ctl.queued_at } else { r.created_at }).max(0)),
            blocked_by: r.node_blocked_by.clone().or_else(|| r.reason.clone().filter(|x| x != "node_queued")),
        }))
        .collect();
    // The node's own queue, when it answered: every server's jobs.
    let (jobs, from, queue_error) = match &f.queue {
        Some(Ok(view)) => {
            let ids: HashMap<&str, &store::JobRow> = f.active.iter().chain(f.queued.iter()).map(|r| (r.id.as_str(), r)).collect();
            let jobs: Vec<Job> = view
                .entries
                .iter()
                .map(|e| {
                    let mine = e.mine.then(|| e.job_id.as_deref().and_then(|id| ids.get(id).copied())).flatten();
                    let running = e.state.as_str() == "running";
                    Job {
                        running,
                        workspace: mine.map(|r| handle(&r.workspace_id)).or_else(|| if e.mine { e.workspace_id.as_deref().map(|w| handle(w)) } else { None }),
                        server: if e.mine { None } else { Some(e.server_label.clone().unwrap_or_else(|| ANOTHER_SERVER.to_string())) },
                        // A node older than 0.46.5 sends no tier: its class says.
                        tier: e.tier.unwrap_or_else(|| Tier::from_class(e.class)).as_str().to_string(),
                        elapsed: mine.and_then(|r| if running { r.started_at.or(r.assigned_at) } else { Some(r.created_at) }).map(|t| (now - t).max(0)),
                        blocked_by: if running { None } else { e.blocked_by.clone() },
                    }
                })
                .collect();
            (jobs, "node", None)
        }
        Some(Err(why)) => (own, "server", Some(why.clone())),
        None => (own, "server", None),
    };
    let running = jobs.iter().filter(|j| j.running).count();
    let queued = jobs.len() - running;
    // Whose they are: per agent here, per other server.
    let mut who: BTreeMap<(u8, String), (usize, usize)> = BTreeMap::new();
    for j in &jobs {
        let key = match (&j.workspace, &j.server) {
            (Some(w), _) => (0, w.clone()),
            (None, Some(s)) => (1, s.clone()),
            (None, None) => (1, ANOTHER_SERVER.to_string()),
        };
        let e = who.entry(key).or_default();
        if j.running {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    let who: Vec<J> = who
        .into_iter()
        .map(|((kind, label), (r, q))| json!({ "label": label, "kind": if kind == 0 { "workspace" } else { "server" }, "running": r, "queued": q }))
        .collect();
    let (state, why) = state_of(f);
    let listed: Vec<J> = jobs
        .iter()
        .take(MAX_JOBS)
        .map(|j| {
            json!({
                "state": if j.running { "running" } else { "queued" },
                "workspace": j.workspace,
                "server": j.server,
                "tier": j.tier,
                "elapsedSecs": j.elapsed,
                "blockedBy": j.blocked_by,
            })
        })
        .collect();
    json!({
        "name": n.name,
        "os": n.os.clone().or_else(|| label("os")),
        "arch": n.arch.clone().or_else(|| label("arch")),
        "version": n.node_version.clone().or_else(|| offer.map(|o| o.node_version.clone()).filter(|v| !v.is_empty())),
        "online": f.online,
        "state": state,
        "why": why,
        "slots": offer.map(|o| o.parallel_max).unwrap_or(0),
        // A node that didn't answer with its queue: its offer still says how
        // many jobs run there (every server's).
        "running": if from == "server" && f.online { offer.map(|o| o.running as usize).unwrap_or(running).max(running) } else { running },
        "queued": queued,
        "queueFrom": from,
        "cpu": {
            "freeMillis": offer.map(|o| o.free.cpu_millis),
            "totalMillis": offer.map(|o| o.caps.cpu_millis),
            "idleMillis": f.idle_cpu_millis,
        },
        "mem": {
            "freeBytes": offer.map(|o| o.free.mem_bytes),
            "totalBytes": offer.map(|o| o.caps.mem_bytes),
        },
        "load": f.load.map(|l| l.to_vec()),
        "lastSeenAt": n.last_seen,
        "shared": n.shared,
        "servers": n.share.as_ref().filter(|_| n.shared).map(|s| s.servers),
        "who": who,
        "jobs": listed,
        "queueError": queue_error,
    })
}

/// The node's state and, when it isn't simply taking jobs, why.
fn state_of(f: &NodeFacts) -> (&'static str, Option<String>) {
    let n = &f.row;
    if !f.online {
        return ("offline", None);
    }
    match n.controller_pause.as_deref() {
        Some("draining") => return ("draining", n.controller_pause_by.as_ref().map(|b| format!("draining on this server (by {b})"))),
        Some(_) => return ("paused", n.controller_pause_by.as_ref().map(|b| format!("paused on this server (by {b})"))),
        None => {}
    }
    let Some(o) = f.offer.as_ref() else { return ("unavailable", Some("K2 hasn't heard what it can take yet".to_string())) };
    match o.control {
        Control::Active => {}
        c => return (c.as_str(), o.control_by.as_ref().map(|b| format!("{} on the node (by {b})", c.as_str()))),
    }
    if let Some(s) = n.share.as_ref().filter(|s| s.state != ServerStatus::Active) {
        return ("unavailable", Some(format!("this server's share is {}", s.state.as_str())));
    }
    if !o.availability.ok {
        let why = o.availability.reasons.iter().map(|r| r.replace('_', " ")).collect::<Vec<_>>().join(", ");
        return ("unavailable", Some(if why.is_empty() { "not taking jobs now".to_string() } else { why }));
    }
    // Taking jobs, but something holds the queue (a foreign lock, low disk).
    ("active", o.refusal.as_ref().map(|r| if r.message.is_empty() { share::blocked_words(&r.code) } else { r.message.clone() }))
}

#[cfg(test)]
#[path = "compute_garden_tests.rs"]
mod tests;
