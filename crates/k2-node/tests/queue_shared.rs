//! The node queue on a shared node (prd-compute-shared-queue-v1 §7, §16
//! tier 1): one real node, two fake servers (the owner at slot 0, another
//! at slot 1), real jobs.
//!
//! The node is shared the real way (lane S1): the node owner's
//! `servers.toml` names A (owner) and B, B's pin is on disk, and the node's
//! servers loop opens B's session; each attach sets the server's protocol
//! and runs `queue_attach` with its `Welcome.resume`. Tests that change a
//! share mid-run (weights, an end date) set it on the queue directly.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use k2_node::fairq::ShareInfo;
use k2_node::node::Tenant;
use k2_node_proto::frames::{Cancel, Frame, JobState, ResumeHint, CANCEL_IF_QUEUED, CANCEL_WITHDRAW};
use k2_node_proto::share::{codes, JobClass, ServerStatus, ServersFile, ShareLimits, ShareState, Trust};
use k2_node_proto::tiers::Tier;

fn limits() -> ShareLimits {
    ShareLimits { max_running: 4, max_queued: 10, weight: 1, ..ShareLimits::new_server(Trust::Own, false) }
}

fn share(label: &str, limits: ShareLimits, status: ServerStatus) -> Option<ShareInfo> {
    Some(ShareInfo { label: label.into(), limits, status })
}

/// One node shared by servers A (owner) and B.
struct Two {
    a: Fake,
    b: Fake,
    n: TestNode,
    ca: Conn,
    cb: Conn,
    owner: Arc<Tenant>,
    tb: Arc<Tenant>,
}

async fn two(tag: &str, policy_extra: &str) -> Two {
    let a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, tag, policy_extra, |_| {}).await;
    let key = k2_node::identity::load_or_create_key(&n.layout.key()).unwrap();
    b.register_active(&key, "node-b", "mini-1");
    let tl = n.layout.tenant(1);
    tl.ensure_state().unwrap();
    k2_node::identity::write_pin(&tl.pin(), &b.pin_for("node-b", "mini-1")).unwrap();
    // The node owner shares the node with B (S1 opens B's session).
    let mut f = ServersFile::new(&a.key.fingerprint(), "a.k2.dev", limits(), 8, k2_node::util::now(), "local:test");
    let mut e = f.servers[0].clone();
    e.slot = 1;
    e.label = "b.k2.dev".into();
    e.fp_prefix = b.key.fingerprint()[..16].to_string();
    e.trust = Trust::Own;
    e.state = ShareState::Active;
    f.servers.push(e);
    k2_node::admin::servers::write_file(&n.layout.config, &f).unwrap();
    let owner = n.node.owner();
    let mut a2 = a;
    let mut ca = a2.next_conn().await;
    ready(&mut ca).await;
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    let tb = n.node.tenant(1).expect("S1 opened B");
    for _ in 0..400 {
        if n.node.fairq.shared(0) && n.node.fairq.protocol(0) >= 3 && n.node.fairq.protocol(1) >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(n.node.fairq.shared(0) && n.node.fairq.protocol(1) >= 3, "S1 told the queue both shares and protocols");
    Two { a: a2, b, n, ca, cb, owner, tb }
}

/// A plan from server A (`node-1`) or B (`node-b`).
fn job(node_id: &str, id: &str, argv: &[&str]) -> k2_node_proto::frames::Plan {
    let mut p = plan(id, argv);
    p.node_id = node_id.into();
    p
}

/// Poll `t`'s ledger until `job`/`gen` matches `pred` (20 s).
async fn wait_row(t: &Tenant, id: &str, gen: u32, what: &str, pred: impl Fn(&k2_node::ledger::Row) -> bool) -> k2_node::ledger::Row {
    for _ in 0..400 {
        if let Some(r) = t.ledger.lock().unwrap().get(id, gen).unwrap() {
            if pred(&r) {
                return r;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{id} g{gen} on slot {} never became {what}: {:?}", t.slot, t.ledger.lock().unwrap().get(id, gen).unwrap());
}

fn started(r: &k2_node::ledger::Row) -> bool {
    r.started_at.is_some()
}

fn queued(r: &k2_node::ledger::Row) -> bool {
    r.state == JobState::Queued
}

fn ended(r: &k2_node::ledger::Row) -> bool {
    r.state.is_terminal() && r.receipt.is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_wait_in_the_queue_and_start_in_fair_order() {
    let mut w = two("q-fair", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "a1", &["/bin/sh", "-c", "sleep 1.5"]), 1)));
    wait_row(&w.owner, "a1", 1, "started", started).await;
    w.ca.send(Frame::Assign(assign(job("node-1", "a2", &["/bin/sh", "-c", "echo a2"]), 1)));
    w.cb.send(Frame::Assign(assign(job("node-b", "b1", &["/bin/sh", "-c", "echo b1"]), 1)));
    let a2 = wait_row(&w.owner, "a2", 1, "queued", queued).await;
    let b1 = wait_row(&w.tb, "b1", 1, "queued", queued).await;
    assert_eq!((a2.reason.as_deref(), b1.reason.as_deref()), (Some(codes::NODE_QUEUED), Some(codes::NODE_QUEUED)));
    assert!(a2.enqueued_at.unwrap() <= b1.enqueued_at.unwrap(), "a2 came first");
    // B is told where it stands.
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "b1" && s.queue.is_some())).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Queued, Some(codes::NODE_QUEUED)));
    // First or second, depending on whether a1's CPU was counted yet; never
    // blocked, and an ETA (a1 has no estimate yet, so maybe unknown).
    let qp = s.queue.as_ref().unwrap();
    assert!(matches!(qp.pos, 1 | 2) && qp.blocked_by.is_none(), "{qp:?}");
    // a1 ends: A has used the node, B hasn't, so b1 goes before the older a2.
    let b1 = wait_row(&w.tb, "b1", 1, "ended", ended).await;
    let a2 = wait_row(&w.owner, "a2", 1, "ended", ended).await;
    assert_eq!((b1.state, a2.state), (JobState::Done, JobState::Done));
    assert!(b1.started_at.unwrap() <= a2.started_at.unwrap(), "fair share: b1 {b1:?} a2 {a2:?}");
    assert!(w.n.node.fairq.usage(w.n.node.now()).get(&0).is_some_and(|u| *u > 0), "a1's CPU counted");
    let _ = &w.a;
    let _ = &w.b;
}

/// Every server starts at weight 1 (the owner too); the node owner sets
/// 2:1 and the starts follow it (the node half of `servers.limits
/// --weight`). One slot, equal short jobs, both servers always waiting.
#[tokio::test(flavor = "multi_thread")]
async fn weights_set_by_the_node_owner_steer_the_starts() {
    let w = two("q-weights", "max_parallel = 1\n").await;
    let owner_default = w.n.node.fairq.share(1, &w.n.node).unwrap();
    assert_eq!(owner_default.limits.weight, 1);
    w.n.node.fairq.set_share(0, None);
    assert_eq!(w.n.node.fairq.share(0, &w.n.node).unwrap().limits.weight, 1, "the owner's default share is weight 1 too");
    w.n.node.fairq.set_share(0, share("a.k2.dev", ShareLimits { weight: 2, max_running: 1, max_queued: 30, ..limits() }, ServerStatus::Active));
    w.n.node.fairq.set_share(1, share("b.k2.dev", ShareLimits { weight: 1, max_running: 1, max_queued: 30, ..limits() }, ServerStatus::Active));
    let order = w.n.dir.join("order.txt");
    let o = order.display().to_string();
    // The hold keeps the only slot until every job below is queued, so no
    // start happens while the loop still submits (under load a short hold
    // ended mid-loop and B's first jobs started before A's were in: 3 of 21
    // runs failed). It asks for the least CPU a plan can name, so the
    // queue charges A next to nothing for it however long the submitting
    // takes: the starts after it follow the weights alone.
    let mut hold = job("node-1", "hold", &["/bin/sh", "-c", "sleep 60"]);
    hold.limits.cpu_millis = Some(1);
    w.ca.send(Frame::Assign(assign(hold, 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    let n = 12;
    for i in 0..n {
        let line = format!("echo a{i} >> {o}; sleep 0.5");
        w.ca.send(Frame::Assign(assign(job("node-1", &format!("a{i}"), &["/bin/sh", "-c", &line]), 1)));
        let line = format!("echo b{i} >> {o}; sleep 0.5");
        w.cb.send(Frame::Assign(assign(job("node-b", &format!("b{i}"), &["/bin/sh", "-c", &line]), 1)));
    }
    for i in 0..n {
        wait_row(&w.owner, &format!("a{i}"), 1, "queued", queued).await;
        wait_row(&w.tb, &format!("b{i}"), 1, "queued", queued).await;
    }
    assert!(!order.exists(), "nothing started while the hold ran: {:?}", std::fs::read_to_string(&order));
    // All 24 wait; free the slot. Both servers always wait during the
    // first 12 starts, so A gets about two of every three.
    w.ca.send(Frame::Cancel(Cancel { job_id: "hold".into(), generation: 1, reason: "cancelled".into() }));
    for _ in 0..600 {
        if std::fs::read_to_string(&order).map(|s| s.lines().count()).unwrap_or(0) >= 12 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let text = std::fs::read_to_string(&order).unwrap();
    let first: Vec<&str> = text.lines().take(12).collect();
    assert_eq!(first.len(), 12, "{text}");
    let a = first.iter().filter(|l| l.starts_with('a')).count();
    assert!((7..=9).contains(&a), "2:1 weights gave A {a} of the first 12 starts: {first:?}");
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// A queued job doesn't start while its server is away; after a
/// reconnect only the rows `Welcome.resume` lists may start, the rest end
/// `superseded` and never run (V47).
#[tokio::test(flavor = "multi_thread")]
async fn queued_jobs_wait_for_their_server_and_the_resume_list_decides() {
    let mut w = two("q-away", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "hold", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "gone", &["/bin/sh", "-c", "echo gone"]), 1)));
    w.cb.send(Frame::Assign(assign(job("node-b", "kept", &["/bin/sh", "-c", "echo kept"]), 1)));
    wait_row(&w.tb, "gone", 1, "queued", queued).await;
    wait_row(&w.tb, "kept", 1, "queued", queued).await;
    // B goes away; the node redials it. The new session's attach lists
    // only "kept": S1 runs queue_attach with that resume list.
    *w.b.resume.lock().unwrap() = vec![ResumeHint { job_id: "kept".into(), generation: 1, since_seq: 0 }];
    w.cb.tx.send(Out::Close).unwrap();
    let mut cb2 = w.b.next_conn().await;
    let first = cb2.until(|f| matches!(f, Frame::Offer(_))).await;
    let g = wait_row(&w.tb, "gone", 1, "superseded", ended).await;
    assert_eq!((g.state, g.reason.as_deref(), g.started_at), (JobState::Cancelled, Some(codes::SUPERSEDED), None));
    let told = first.iter().any(|f| match f {
        Frame::State(s) => s.job_id == "gone" && s.state == JobState::Cancelled && s.reason.as_deref() == Some(codes::SUPERSEDED),
        Frame::JobsSnapshot(p) => p.jobs.iter().any(|j| j.job_id == "gone" && j.state == JobState::Cancelled),
        _ => false,
    });
    assert!(told, "B hears at once that gone was superseded: {first:?}");
    // "kept" waits for capacity (A's hold runs), then runs in this session.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(wait_row(&w.tb, "kept", 1, "still queued", queued).await.started_at.is_none(), "no capacity yet");
    w.ca.send(Frame::Cancel(Cancel { job_id: "hold".into(), generation: 1, reason: "cancelled".into() }));
    let k = wait_row(&w.tb, "kept", 1, "ended", ended).await;
    assert_eq!(k.state, JobState::Done);
}

/// Queued rows survive a k2-node restart; the started one doesn't (V47).
#[tokio::test(flavor = "multi_thread")]
async fn queued_rows_survive_a_restart() {
    let w = two("q-restart", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "hold", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    w.ca.send(Frame::Assign(assign(job("node-1", "a-q", &["/bin/echo", "x"]), 1)));
    w.cb.send(Frame::Assign(assign(job("node-b", "b-q", &["/bin/echo", "x"]), 1)));
    wait_row(&w.owner, "a-q", 1, "queued", queued).await;
    wait_row(&w.tb, "b-q", 1, "queued", queued).await;
    // Freeze this process's queue, then open a second node on the same
    // dirs, as a restarted k2-node would.
    w.n.node.fairq.set_blocked(0, Some("test_restart".into()));
    w.n.node.fairq.set_blocked(1, Some("test_restart".into()));
    let layout = w.n.layout.clone();
    let again = k2_node::node::Node::open(options(layout)).await.unwrap();
    let t1 = again.add_tenant(1).await.unwrap();
    let hold = again.owner().ledger.lock().unwrap().get("hold", 1).unwrap().unwrap();
    assert_eq!((hold.state, hold.reason.as_deref()), (JobState::Interrupted, Some("node_restart")), "the started job can't be re-adopted");
    for (t, id) in [(again.owner(), "a-q"), (t1, "b-q")] {
        let r = t.ledger.lock().unwrap().get(id, 1).unwrap().unwrap();
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Queued, Some(codes::NODE_QUEUED)), "{id} survives");
    }
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// `withdraw` and `if_queued` end only queued jobs; a started job answers
/// its real state and keeps running (V57). A withdrawn job comes back
/// under a new generation and runs (V46).
#[tokio::test(flavor = "multi_thread")]
async fn withdraw_never_touches_a_started_job_and_a_new_generation_runs() {
    let mut w = two("q-withdraw", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "hold", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    // Past the shipper's earlier frames (queued, then running).
    w.ca.until(|f| matches!(f, Frame::State(s) if s.job_id == "hold" && s.state == JobState::Running)).await;
    for reason in [CANCEL_IF_QUEUED, CANCEL_WITHDRAW] {
        w.ca.send(Frame::Cancel(Cancel { job_id: "hold".into(), generation: 1, reason: reason.into() }));
        let fr = w.ca.until(|f| matches!(f, Frame::State(s) if s.job_id == "hold")).await;
        let Some(Frame::State(s)) = fr.last() else { unreachable!() };
        assert_eq!(s.state, JobState::Running, "{reason} answers the real state");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(w.owner.ledger.lock().unwrap().get("hold", 1).unwrap().unwrap().state, JobState::Running, "still running");

    w.cb.send(Frame::Assign(assign(job("node-b", "mv", &["/bin/sh", "-c", "echo moved"]), 1)));
    wait_row(&w.tb, "mv", 1, "queued", queued).await;
    w.cb.send(Frame::Cancel(Cancel { job_id: "mv".into(), generation: 1, reason: CANCEL_WITHDRAW.into() }));
    let r = wait_row(&w.tb, "mv", 1, "withdrawn", ended).await;
    assert_eq!((r.state, r.reason.as_deref(), r.started_at), (JobState::Cancelled, Some(codes::WITHDRAWN), None));
    let fr = w.cb.until(|f| matches!(f, Frame::Receipt(x) if x.job_id == "mv")).await;
    assert!(fr.iter().any(|f| matches!(f, Frame::State(s) if s.job_id == "mv" && s.reason.as_deref() == Some(codes::WITHDRAWN))));
    // The server moves it back here under a new generation: it runs.
    w.cb.send(Frame::Assign(assign(job("node-b", "mv", &["/bin/sh", "-c", "echo moved"]), 2)));
    wait_row(&w.tb, "mv", 2, "queued", queued).await;
    w.ca.send(Frame::Cancel(Cancel { job_id: "hold".into(), generation: 1, reason: "cancelled".into() }));
    let r = wait_row(&w.tb, "mv", 2, "ended", ended).await;
    assert_eq!(r.state, JobState::Done);
    // A no-force cancel of a queued job ends it at once.
    w.ca.send(Frame::Assign(assign(job("node-1", "h2", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "h2", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "nf", &["/bin/echo", "x"]), 1)));
    wait_row(&w.tb, "nf", 1, "queued", queued).await;
    w.cb.send(Frame::Cancel(Cancel { job_id: "nf".into(), generation: 1, reason: CANCEL_IF_QUEUED.into() }));
    let r = wait_row(&w.tb, "nf", 1, "cancelled", ended).await;
    assert_eq!((r.state, r.reason.as_deref()), (JobState::Cancelled, Some("cancelled")));
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// An old (protocol 2) owner never queues: it runs only when no other
/// server has queued work, else `fair_share_wait` (V70 b).
#[tokio::test(flavor = "multi_thread")]
async fn an_old_owner_runs_only_when_nobody_else_waits() {
    let mut w = two("q-old", "max_parallel = 3\n").await;
    w.n.node.fairq.set_protocol(0, Some(2));
    w.n.node.fairq.set_share(1, share("b.k2.dev", ShareLimits { max_running: 1, ..limits() }, ServerStatus::Active));
    w.cb.send(Frame::Assign(assign(job("node-b", "b0", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.tb, "b0", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "b1", &["/bin/echo", "x"]), 1)));
    wait_row(&w.tb, "b1", 1, "queued", queued).await;
    let a = assign(job("node-1", "old", &["/bin/echo", "old"]), 1);
    w.ca.send(Frame::Assign(a.clone()));
    let fr = w.ca.until(|f| matches!(f, Frame::State(s) if s.job_id == "old")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Queued, Some(codes::FAIR_SHARE_WAIT)));
    assert!(w.owner.ledger.lock().unwrap().get("old", 1).unwrap().is_none(), "a refusal is never written");
    // Nobody else waits any more: the old owner's retry runs at once.
    w.cb.send(Frame::Cancel(Cancel { job_id: "b1".into(), generation: 1, reason: CANCEL_WITHDRAW.into() }));
    wait_row(&w.tb, "b1", 1, "withdrawn", ended).await;
    w.ca.send(Frame::Assign(a));
    let r = wait_row(&w.owner, "old", 1, "ended", ended).await;
    assert_eq!((r.state, r.enqueued_at), (JobState::Done, None), "it ran without queueing");
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// `gate` goes first but never stops a running job; one at a time.
#[tokio::test(flavor = "multi_thread")]
async fn gate_jumps_the_queue_and_never_kills() {
    let mut w = two("q-gate", "max_parallel = 1\n").await;
    w.n.node.fairq.set_share(0, share("a.k2.dev", ShareLimits { gate: true, top_class: JobClass::Gate, ..limits() }, ServerStatus::Active));
    w.cb.send(Frame::Assign(assign(job("node-b", "b0", &["/bin/sh", "-c", "sleep 1"]), 1)));
    wait_row(&w.tb, "b0", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "b1", &["/bin/echo", "b1"]), 1)));
    wait_row(&w.tb, "b1", 1, "queued", queued).await;
    let mut g = job("node-1", "g1", &["/bin/echo", "g1"]);
    g.class = Some(JobClass::Gate);
    w.ca.send(Frame::Assign(assign(g, 1)));
    wait_row(&w.owner, "g1", 1, "queued", queued).await;
    let b0 = wait_row(&w.tb, "b0", 1, "ended", ended).await;
    assert_eq!((b0.state, b0.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)), "gate never kills");
    let g1 = wait_row(&w.owner, "g1", 1, "ended", ended).await;
    let b1 = wait_row(&w.tb, "b1", 1, "ended", ended).await;
    assert!(g1.started_at.unwrap() <= b1.started_at.unwrap(), "the gate job went first");
    // A server whose share doesn't allow gate is refused at assign.
    let mut bg = job("node-b", "bg", &["/bin/echo", "x"]);
    bg.class = Some(JobClass::Gate);
    w.cb.send(Frame::Assign(assign(bg, 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "bg")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(codes::GATE_NOT_ALLOWED));
}

/// Share caps refuse at once; a full queue refuses; nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn share_caps_and_a_full_queue_refuse_without_a_row() {
    let mut w = two("q-caps", "max_parallel = 1\n").await;
    w.n.node.fairq.set_share(1, share("b.k2.dev", ShareLimits { max_queued: 1, max_cpus: 1, ..limits() }, ServerStatus::Active));
    let mut big = job("node-b", "big", &["/bin/echo", "x"]);
    big.limits.cpu_millis = Some(2000);
    w.cb.send(Frame::Assign(assign(big, 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "big")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(codes::EXCEEDS_SHARE_CAPS));
    w.ca.send(Frame::Assign(assign(job("node-1", "hold", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "q1", &["/bin/echo", "x"]), 1)));
    wait_row(&w.tb, "q1", 1, "queued", queued).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "q2", &["/bin/echo", "x"]), 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "q2")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(codes::TENANT_QUEUE_FULL));
    // A second server on an old protocol never runs here.
    w.n.node.fairq.set_protocol(1, Some(2));
    w.cb.send(Frame::Assign(assign(job("node-b", "old-b", &["/bin/echo", "x"]), 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "old-b")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(codes::SERVER_TOO_OLD));
    for id in ["big", "q2", "old-b"] {
        assert!(w.tb.ledger.lock().unwrap().get(id, 1).unwrap().is_none(), "{id}: refusals are never written");
    }
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// A share past its end date: queued jobs end `share_expired`, running
/// ones finish.
#[tokio::test(flavor = "multi_thread")]
async fn expiry_cancels_queued_jobs_and_lets_running_ones_finish() {
    let mut w = two("q-expire", "max_parallel = 1\n").await;
    w.cb.send(Frame::Assign(assign(job("node-b", "run", &["/bin/sh", "-c", "sleep 1"]), 1)));
    wait_row(&w.tb, "run", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "wait", &["/bin/echo", "x"]), 1)));
    wait_row(&w.tb, "wait", 1, "queued", queued).await;
    w.n.node.fairq.set_share(1, share("b.k2.dev", limits(), ServerStatus::Expired));
    let r = wait_row(&w.tb, "wait", 1, "expired", ended).await;
    assert_eq!((r.state, r.reason.as_deref(), r.started_at), (JobState::Cancelled, Some(codes::SHARE_EXPIRED), None));
    let r = wait_row(&w.tb, "run", 1, "ended", ended).await;
    assert_eq!(r.state, JobState::Done, "the running job finished");
    // New work from the expired server is refused.
    w.cb.send(Frame::Assign(assign(job("node-b", "late", &["/bin/echo", "x"]), 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "late")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(codes::SHARE_EXPIRED));
}

/// What each server sees: its own jobs in full, others redacted; quotes
/// for its recent job sizes.
#[tokio::test(flavor = "multi_thread")]
async fn views_redact_other_servers_and_quotes_follow_asks() {
    let w = two("q-view", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "owner-secret", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "owner-secret", 1, "started", started).await;
    let mut p = job("node-b", "b-mine", &["/bin/echo", "x"]);
    p.limits.cpu_millis = Some(1000);
    p.limits.mem_bytes = Some(1 << 30);
    w.cb.send(Frame::Assign(assign(p, 1)));
    wait_row(&w.tb, "b-mine", 1, "queued", queued).await;
    let v = w.n.node.fairq.view(&w.n.node, &w.tb, "r1");
    assert_eq!(v.request_id, "r1");
    assert_eq!(v.entries.len(), 2);
    assert!(!v.entries[0].mine && v.entries[0].job_id.is_none() && v.entries[0].server_label.is_none());
    assert!(v.entries[1].mine && v.entries[1].job_id.as_deref() == Some("b-mine"));
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("owner-secret") && !text.contains("a.k2.dev"), "{text}");
    let ov = w.n.node.fairq.view(&w.n.node, &w.owner, "r2");
    assert_eq!(ov.entries[1].server_label.as_deref(), Some("b.k2.dev"));
    assert!(ov.entries[1].job_id.is_none() && ov.totals.len() == 2);
    let q = w.n.node.fairq.quotes(&w.n.node, &w.tb);
    assert_eq!(q.len(), 1, "one distinct size asked");
    assert!(!q[0].starts_now, "the node is busy");
    assert_eq!((q[0].cpu_millis, q[0].mem_bytes), (1000, 1 << 30));

    // B6: a session asking faster than once a second gets the last view
    // again (under its own request id); a second later, a fresh one.
    let mut cb = w.cb;
    let ask = |id: &str| Frame::QueueGet(k2_node_proto::share::QueueGet { request_id: id.into() });
    let view_of = |f: Vec<Frame>| match f.last() {
        Some(Frame::QueueView(v)) => v.clone(),
        other => panic!("{other:?}"),
    };
    cb.send(ask("v1"));
    let v1 = view_of(cb.until(|f| matches!(f, Frame::QueueView(_))).await);
    w.ca.send(Frame::Assign(assign(job("node-1", "a-more", &["/bin/echo", "x"]), 1)));
    wait_row(&w.owner, "a-more", 1, "queued", queued).await;
    cb.send(ask("v2"));
    let v2 = view_of(cb.until(|f| matches!(f, Frame::QueueView(_))).await);
    assert_eq!(v2.request_id, "v2");
    assert_eq!(v2.entries.len(), v1.entries.len(), "within a second: the same view, not recomputed");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    cb.send(ask("v3"));
    let v3 = view_of(cb.until(|f| matches!(f, Frame::QueueView(_))).await);
    assert_eq!((v3.request_id.as_str(), v3.entries.len()), ("v3", v1.entries.len() + 1), "a second later: fresh");
    w.n.node.stop_all(JobState::Cancelled, "test_over");
}

/// A stopping k2-node starts nothing new: the capacity its stopped jobs
/// free doesn't start queued work (it stays queued for the next start, so
/// a server that re-placed it meanwhile never gets a double run), and new
/// assigns are refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopping_node_starts_nothing() {
    let mut w = two("q-stop", "max_parallel = 1\n").await;
    w.ca.send(Frame::Assign(assign(job("node-1", "hold", &["/bin/sh", "-c", "sleep 30"]), 1)));
    wait_row(&w.owner, "hold", 1, "started", started).await;
    w.cb.send(Frame::Assign(assign(job("node-b", "b1", &["/bin/echo", "b1"]), 1)));
    wait_row(&w.tb, "b1", 1, "queued", queued).await;
    w.n.node.begin_shutdown();
    w.n.node.stop_all(JobState::Interrupted, "node_shutdown");
    wait_row(&w.owner, "hold", 1, "ended", ended).await;
    for _ in 0..5 {
        w.n.node.fairq.tick(&w.n.node);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let b1 = w.tb.ledger.lock().unwrap().get("b1", 1).unwrap().unwrap();
    assert_eq!((b1.state, b1.started_at), (JobState::Queued, None), "nothing starts while stopping");
    w.cb.send(Frame::Assign(assign(job("node-b", "late", &["/bin/echo", "x"]), 1)));
    let fr = w.cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "late")).await;
    let Some(Frame::State(s)) = fr.last() else { unreachable!() };
    assert_eq!(s.reason.as_deref(), Some(k2_node_proto::refusal::LOCAL_STOPPED));
    assert!(w.tb.ledger.lock().unwrap().get("late", 1).unwrap().is_none());
}

/// No deadlock under contention: the queue's tick, quotes and views (every
/// offer and `queue_get`), assigns, withdraws and accounting all run at
/// once from many threads. The queue keeps one lock; this catches an order
/// inversion with the node's own locks (ledgers, jobs, hub) too. Every
/// caller must finish within the deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_queue_never_deadlocks_under_contention() {
    let w = two("q-stress", "max_parallel = 2\n").await;
    let node = w.n.node.clone();
    let deadline = Duration::from_secs(60);
    // Plain threads report on a channel: a hung one can't stall the test's
    // runtime shutdown, so a deadlock fails at the deadline (loudly).
    let (done_tx, done_rx) = std::sync::mpsc::channel::<u32>();
    let rt = tokio::runtime::Handle::current();
    for i in 0..4u32 {
        let n = node.clone();
        let (o, b) = (w.owner.clone(), w.tb.clone());
        let done = done_tx.clone();
        let rt = rt.clone();
        std::thread::spawn(move || {
            let _enter = rt.enter();
            for k in 0..150u32 {
                match (i + k) % 4 {
                    0 => n.fairq.tick(&n),
                    1 => {
                        let _ = n.fairq.quotes(&n, &b);
                    }
                    2 => {
                        let _ = n.fairq.view(&n, &o, "stress");
                    }
                    _ => {
                        let _ = n.fairq.usage(n.now());
                        let _ = n.fairq.shared(n.now());
                    }
                }
            }
            let _ = done.send(i);
        });
    }
    drop(done_tx);
    // Real traffic meanwhile: jobs in and out from both servers.
    for k in 0..30 {
        w.ca.send(Frame::Assign(assign(job("node-1", &format!("sa{k}"), &["/bin/sh", "-c", "sleep 0.05"]), 1)));
        w.cb.send(Frame::Assign(assign(job("node-b", &format!("sb{k}"), &["/bin/sh", "-c", "sleep 0.05"]), 1)));
        if k % 3 == 0 {
            w.cb.send(Frame::Cancel(Cancel { job_id: format!("sb{k}"), generation: 1, reason: CANCEL_WITHDRAW.into() }));
        }
    }
    let start = std::time::Instant::now();
    let mut finished = 0;
    while finished < 4 {
        let left = deadline.saturating_sub(start.elapsed());
        match tokio::task::block_in_place(|| done_rx.recv_timeout(left)) {
            Ok(_) => finished += 1,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A deadlocked queue also blocks the node's own tick on the
                // blocking pool, and dropping the runtime would wait for it
                // forever: end the process so the run fails, not hangs.
                eprintln!("DEADLOCK: a queue caller hung for {deadline:?} ({finished} of 4 finished)");
                std::process::abort();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("a queue caller panicked ({finished} of 4 finished)"),
        }
    }
    // Everything still ends (max_queued 10 refuses some: those never exist).
    for k in 0..30 {
        for (t, id) in [(&w.owner, format!("sa{k}")), (&w.tb, format!("sb{k}"))] {
            if t.ledger.lock().unwrap().get(&id, 1).unwrap().is_some() {
                wait_row(t, &id, 1, "ended", ended).await;
            }
        }
    }
    node.stop_all(JobState::Cancelled, "test_over");
}

// ── job tiers (prd-compute-job-tiers-v1 S1) ──────────────────────────

/// A guest server whose share has `gate` sends CI: it takes the next free
/// slot ahead of the owner's older Normal job, and never stops the
/// running one. Both servers speak protocol 5, so both are offered `tiers`.
#[tokio::test(flavor = "multi_thread")]
async fn ci_from_a_guest_with_gate_jumps_the_owners_normal_work() {
    let w = two("q-tier-ci", "max_parallel = 1\n").await;
    for t in [&w.owner, &w.tb] {
        assert!(w.n.node.session_features(t).iter().any(|f| f == k2_node_proto::tiers::FEATURE_TIERS), "slot {} speaks 5", t.slot);
    }
    w.n.node.fairq.set_share(1, share("b.k2.dev", ShareLimits { gate: true, top_class: JobClass::Gate, ..limits() }, ServerStatus::Active));
    w.ca.send(Frame::Assign(assign(job("node-1", "a0", &["/bin/sh", "-c", "sleep 1"]), 1)));
    wait_row(&w.owner, "a0", 1, "started", started).await;
    w.ca.send(Frame::Assign(assign(job("node-1", "a1", &["/bin/echo", "a1"]), 1)));
    wait_row(&w.owner, "a1", 1, "queued", queued).await;
    let mut ci = job("node-b", "bc", &["/bin/echo", "bc"]);
    ci.tier = Some(Tier::Ci);
    ci.class = Some(JobClass::Gate);
    w.cb.send(Frame::Assign(assign(ci, 1)));
    wait_row(&w.tb, "bc", 1, "queued", queued).await;
    let a0 = wait_row(&w.owner, "a0", 1, "ended", ended).await;
    assert_eq!((a0.state, a0.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)), "CI never stops a running job");
    let bc = wait_row(&w.tb, "bc", 1, "ended", ended).await;
    let a1 = wait_row(&w.owner, "a1", 1, "ended", ended).await;
    assert!(bc.started_at.unwrap() <= a1.started_at.unwrap(), "CI went first: bc {bc:?} a1 {a1:?}");
}

/// T7: a guest's Priority counts only where the node's owner says so in
/// `queue.toml`; its offer tells it (`ShareView.priority`), and the
/// servers loop picks a change up without a restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_guests_priority_word_comes_from_queue_toml() {
    let w = two("q-tier-p", "max_parallel = 1\n").await;
    let word = |t: &Tenant| w.n.node.offer(t).share.and_then(|s| s.priority);
    for _ in 0..400 {
        if word(w.tb.as_ref()).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(word(w.owner.as_ref()), Some(true), "the owner's Priority counts by default");
    assert_eq!(word(w.tb.as_ref()), Some(false), "a guest's doesn't");
    let fp = w.b.key.fingerprint();
    let prefix = &fp[..16];
    std::fs::write(w.n.layout.config.join(k2_node::queue_file::QUEUE_FILE), format!("[servers.{prefix}]\npriority = true\n")).unwrap();
    for _ in 0..800 {
        if w.n.node.fairq.priority(1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(w.n.node.fairq.priority(1), "the servers loop read queue.toml");
    assert_eq!(word(w.tb.as_ref()), Some(true));
    let listed = k2_node::admin::servers::node_list(&w.n.node).unwrap();
    assert_eq!(listed.servers.iter().map(|s| (s.slot, s.priority)).collect::<Vec<_>>(), vec![(0, Some(true)), (1, Some(true))]);
    let _ = (&w.a, &w.ca, &w.cb);
}
