//! A job's whole process tree ends with it (0.46.16, `k2_node::jobtree`):
//! on a cancel and at a normal end, on Linux and macOS, for a job the node
//! starts itself and for one a `job-hold` process holds (the macOS node's
//! way). A bench job's child daemons outlived a cancel by ~27 hours on a
//! Mac mini; these are its regression tests.
//!
//! - `sh -c 'sleep 3600 & … sleep 3600'` cancelled: no `sleep 3600` of
//!   the job is left within a few seconds.
//! - The same with a child that calls `setsid` (and leaves for `/`), and
//!   one that calls `setpgid` with an empty environment: both outside the
//!   job's group, so only the session and folder rules find them.
//! - A job that backgrounds a `setsid` child and exits 0: the child is
//!   killed and the job's record says so ("killed 1 leftover process(es)").
//! - The node check: a process with a dead job's `K2_COMPUTE_JOB` under
//!   the node's folder is reported, a live job's isn't, and the check
//!   kills nothing.
//!
//! Each job prints its children's pids (`kid <pid>`), so the test checks
//! those exact processes (pid + start time) and, beside them, every
//! `sleep 3600` of this uid that names the node's folder.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use k2_node_proto::crypto;
use k2_node_proto::frames::{Cancel, Frame, JobState, LogStream};

fn uid() -> u32 {
    // SAFETY: a plain getter.
    unsafe { libc::geteuid() }
}

/// A child that leaves the job's session and group, and its folder.
const SETSID_KID: &str = r#"/usr/bin/perl -MPOSIX -e 'POSIX::setsid() or die "setsid: $!"; chdir "/"; exec "/bin/sleep", "3600"' </dev/null >/dev/null 2>&1 & echo "kid $!""#;
/// A child that leaves the job's group (not its session) with an empty
/// environment, in `/`.
const SETPGID_KID: &str = r#"env -i /usr/bin/perl -e 'setpgrp(0, 0) or die "setpgrp: $!"; chdir "/"; exec "/bin/sleep", "3600"' </dev/null >/dev/null 2>&1 & echo "kid $!""#;

async fn start(fake: &Fake, tag: &str, held: bool) -> TestNode {
    start_node(fake, tag, "", |o| {
        if held {
            o.job_holder = Some(PathBuf::from(env!("CARGO_BIN_EXE_k2-node")));
        }
    })
    .await
}

/// `kid <pid>` lines → (pid, start time). A kid that's already gone has
/// no start time: a failure (the job's setup didn't work).
fn kids(out: &str) -> Vec<(i32, u64)> {
    out.lines()
        .filter_map(|l| l.strip_prefix("kid "))
        .map(|p| {
            let pid: i32 = p.trim().parse().unwrap_or_else(|_| panic!("bad kid line {p:?} in {out}"));
            let start = k2_node::util::process_start_time(pid as u32).unwrap_or_else(|| panic!("kid {pid} is already gone: {out}"));
            (pid, start)
        })
        .collect()
}

fn alive(k: &(i32, u64)) -> bool {
    k2_node::util::process_start_time(k.0 as u32) == Some(k.1)
}

fn is_sleep_3600(p: &k2_node::jobtree::ProcRow) -> bool {
    let first = p.argv.first().map(|a| Path::new(a).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    first.as_deref() == Some("sleep") && p.argv.get(1).map(String::as_str) == Some("3600")
}

/// What's left: the kids still alive, and every `sleep 3600` of this uid
/// that names `dir` (cwd, argv, env).
fn left(dir: &Path, kids: &[(i32, u64)]) -> Vec<String> {
    let roots = k2_node::strays::roots(dir);
    let mut out: Vec<String> = kids.iter().filter(|k| alive(k)).map(|k| format!("kid {}", k.0)).collect();
    for p in k2_node::jobtree::snapshot(uid()) {
        if p.uid != uid() || !is_sleep_3600(&p) || kids.iter().any(|k| k.0 == p.pid) {
            continue;
        }
        let mut words = p.argv.clone();
        words.extend(p.env.iter().cloned());
        if k2_node::strays::belongs(p.cwd.as_deref(), p.exe.as_deref(), &words, &roots) {
            out.push(format!("pid {} {}", p.pid, p.cmd()));
        }
    }
    out
}

/// Poll until nothing is left or `within` passes; what's left then.
async fn left_after(dir: &Path, kids: &[(i32, u64)], within: Duration) -> Vec<String> {
    let deadline = Instant::now() + within;
    loop {
        let l = left(dir, kids);
        if l.is_empty() || Instant::now() >= deadline {
            return l;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until a planted kid has left this test's session (its `setsid`
/// ran): before that it is in this process's group, never a leftover.
async fn left_our_session(k: &(i32, u64)) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = k2_node::jobtree::snapshot(uid()).into_iter().find(|p| p.pid == k.0);
        if row.as_ref().is_some_and(|p| p.sid == p.pid && is_sleep_3600(p)) {
            return;
        }
        assert!(Instant::now() < deadline && alive(k), "kid {} never left this session for its sleep: {row:?}", k.0);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Kill what a failed test left (never leak a sleep either way).
fn cleanup(kids: &[(i32, u64)]) {
    for k in kids.iter().filter(|k| alive(k)) {
        // SAFETY: re-checked by its start time just above.
        unsafe { libc::kill(k.0, libc::SIGKILL) };
    }
}

/// Frames until the job printed `ready`, and its stdout so far.
async fn until_ready(c: &mut Conn, job: &str) -> String {
    let mut out = String::new();
    let job = job.to_string();
    c.until(|f| match f {
        Frame::Log(l) if l.job_id == job && l.stream == LogStream::Out => {
            out.push_str(&String::from_utf8_lossy(&crypto::unb64(&l.data).unwrap()));
            out.contains("ready\n")
        }
        Frame::State(s) if s.job_id == job && s.state.is_terminal() => panic!("{job} ended before it was ready: {s:?}; out: {out}"),
        _ => false,
    })
    .await;
    out
}

/// Start `script`, wait for `ready`, check its kids run, cancel it, and
/// assert nothing of it is left within 5 s.
async fn cancel_leaves_nothing(held: bool, job: &str, script: &str, want_kids: usize) {
    let mut fake = Fake::start().await;
    let n = start(&fake, job, held).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan(job, &["/bin/sh", "-c", script]), 1)));
    let out = until_ready(&mut c, job).await;
    let kids = kids(&out);
    assert_eq!(kids.len(), want_kids, "{out}");
    // The kids have left (setsid/setpgid) before the cancel.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let running = left(&n.dir, &kids);
    assert!(kids.iter().all(alive), "every kid runs before the cancel: {running:?}");
    c.send(Frame::Cancel(Cancel { job_id: job.into(), generation: 1, reason: "cancelled".into() }));
    let fr = run_to_receipt(&mut c, job).await;
    let end = states_of(&fr, job).into_iter().last().unwrap();
    assert_eq!(end.state, JobState::Cancelled, "{end:?}");
    let still = left_after(&n.dir, &kids, Duration::from_secs(5)).await;
    cleanup(&kids);
    assert!(still.is_empty(), "held={held}: left after the cancel: {still:?}; sys log: {}", text(&logs_of(&fr, job), LogStream::Sys));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_job_leaves_no_background_sleep() {
    // `sleep 3600 & sleep 3600`, printing both pids (the second is the
    // job's own process once the shell execs it).
    let script = r#"sleep 3600 </dev/null >/dev/null 2>&1 & echo "kid $!"; echo "kid $$"; echo ready; exec sleep 3600"#;
    cancel_leaves_nothing(false, "tk-bg-local", script, 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_held_job_leaves_no_background_sleep() {
    let script = r#"sleep 3600 </dev/null >/dev/null 2>&1 & echo "kid $!"; echo "kid $$"; echo ready; exec sleep 3600"#;
    cancel_leaves_nothing(true, "tk-bg-held", script, 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_job_leaves_no_setsid_or_setpgid_child() {
    let script = format!("{SETSID_KID}; {SETPGID_KID}; sleep 3600 </dev/null >/dev/null 2>&1 & echo \"kid $!\"; echo ready; wait");
    cancel_leaves_nothing(false, "tk-sid-local", &script, 3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_held_job_leaves_no_setsid_or_setpgid_child() {
    let script = format!("{SETSID_KID}; {SETPGID_KID}; sleep 3600 </dev/null >/dev/null 2>&1 & echo \"kid $!\"; echo ready; wait");
    cancel_leaves_nothing(true, "tk-sid-held", &script, 3).await;
}

/// A job that starts a `setsid` daemon and exits 0: the daemon is killed
/// at its end, and its log and record say so.
async fn normal_end_kills_the_daemon(held: bool, job: &str) {
    let mut fake = Fake::start().await;
    let n = start(&fake, job, held).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let script = format!("{SETSID_KID}; sleep 0.5; echo ready");
    c.send(Frame::Assign(assign(plan(job, &["/bin/sh", "-c", &script]), 1)));
    let fr = run_to_receipt(&mut c, job).await;
    let logs = logs_of(&fr, job);
    let out = text(&logs, LogStream::Out);
    let kid: Vec<(i32, u64)> = out
        .lines()
        .filter_map(|l| l.strip_prefix("kid "))
        .map(|p| (p.trim().parse::<i32>().unwrap(), 0))
        .collect();
    assert_eq!(kid.len(), 1, "{out}");
    let end = states_of(&fr, job).into_iter().last().unwrap();
    assert_eq!(end.state, JobState::Done, "{end:?}");
    let sys = text(&logs, LogStream::Sys);
    assert!(sys.contains("killed 1 leftover process(es) the job left running outside its group: "), "{sys}");
    assert!(sys.contains(&format!("pid {} (", kid[0].0)), "names the pid: {sys}");
    let notes = end.outputs.as_ref().map(|o| o.notes.clone()).unwrap_or_default();
    assert!(notes.iter().any(|n| n.starts_with("killed 1 leftover process(es) the job left running: ")), "the job record says so: {notes:?}");
    // The pid is gone (a reused pid is never a `sleep 3600` naming the node's folder).
    let still = left_after(&n.dir, &[], Duration::from_secs(5)).await;
    assert!(still.is_empty(), "held={held}: left after a normal end: {still:?}");
    // SAFETY: signal 0 only checks.
    let pid_alive = unsafe { libc::kill(kid[0].0, 0) } == 0;
    if pid_alive {
        let p = k2_node::jobtree::snapshot(uid()).into_iter().find(|p| p.pid == kid[0].0);
        assert!(!p.as_ref().is_some_and(is_sleep_3600), "the kid {} still runs: {p:?}", kid[0].0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_exits_leaves_no_setsid_daemon() {
    normal_end_kills_the_daemon(false, "tk-end-local").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_job_that_exits_leaves_no_setsid_daemon() {
    normal_end_kills_the_daemon(true, "tk-end-held").await;
}

/// The node check: a dead job's process under the node's folder is
/// reported (with its job id), a live held job's isn't, and the check
/// kills neither.
#[tokio::test(flavor = "multi_thread")]
async fn the_node_check_reports_leftovers_and_kills_nothing() {
    let mut fake = Fake::start().await;
    let n = start(&fake, "tk-check", true).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    // A leftover: out of this test's group and parentage, a dead job's id,
    // its cwd under the node's jobs folder.
    let jobs = n.layout.home.join("jobs");
    let o = std::process::Command::new("/bin/sh")
        .args(["-c", SETSID_KID.replace("chdir \"/\"; ", "").as_str()])
        .current_dir(&jobs)
        .env("K2_COMPUTE_JOB", "gone-job")
        .output()
        .unwrap();
    let planted = kids(&String::from_utf8_lossy(&o.stdout));
    assert_eq!(planted.len(), 1, "{o:?}");
    left_our_session(&planted[0]).await;
    // A live job (held: no descendant of this process).
    c.send(Frame::Assign(assign(plan("tk-live", &["/bin/sh", "-c", r#"echo "kid $$"; echo ready; exec sleep 3600"#]), 1)));
    let live = kids(&until_ready(&mut c, "tk-live").await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (roots, marks) = n.node.leftover_inputs();
    assert!(marks.iter().any(|m| m.job_id == "tk-live"), "{marks:?}");
    let found = k2_node::jobtree::leftovers(&roots, &marks, 0);
    let pids: Vec<i32> = found.iter().map(|l| l.pid).collect();
    let mine = found.iter().find(|l| l.pid == planted[0].0);
    // The health fact (this test's leftover only: inside a compute job
    // every process of the outer job carries its K2_COMPUTE_JOB too).
    let fact = k2_node::admin::health::leftovers_fact(found.iter().filter(|l| l.job_id.as_deref() == Some("gone-job")).cloned().collect());
    cleanup(&planted);
    c.send(Frame::Cancel(Cancel { job_id: "tk-live".into(), generation: 1, reason: "cancelled".into() }));
    let _ = run_to_receipt(&mut c, "tk-live").await;
    let mine = mine.unwrap_or_else(|| panic!("the planted leftover {} isn't reported: {found:?}", planted[0].0));
    assert_eq!(mine.job_id.as_deref(), Some("gone-job"));
    assert!(mine.cmd.contains("sleep 3600"), "{mine:?}");
    assert!(!pids.contains(&live[0].0), "a live job's process is never a leftover: {found:?}");
    assert_eq!(fact.count, 1, "{fact:?}");
    assert!(fact.procs.iter().any(|p| p.pid == planted[0].0 && p.job_id.as_deref() == Some("gone-job") && p.cmd.contains("sleep 3600")), "{fact:?}");
    // The check killed nothing: both were alive after it (cleanup and the
    // cancel ended them only afterwards, and the cancel's own sweep had the
    // live one).
    assert!(left_after(&n.dir, &live, Duration::from_secs(5)).await.is_empty());
}

/// The check itself kills nothing: run it twice, the leftover lives on.
#[tokio::test(flavor = "multi_thread")]
async fn the_node_check_never_kills() {
    let dir = k2_node::util::temp_dir("tk-never");
    let jobs = dir.join("home").join("jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    let o = std::process::Command::new("/bin/sh")
        .args(["-c", SETSID_KID.replace("chdir \"/\"; ", "").as_str()])
        .current_dir(&jobs)
        .env("K2_COMPUTE_JOB", "gone-job-2")
        .output()
        .unwrap();
    let planted = kids(&String::from_utf8_lossy(&o.stdout));
    assert_eq!(planted.len(), 1, "{o:?}");
    left_our_session(&planted[0]).await;
    let roots = vec![dir.join("home")];
    for _ in 0..2 {
        let found = k2_node::jobtree::leftovers(&roots, &[], 0);
        assert!(found.iter().any(|l| l.pid == planted[0].0), "{found:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let alive_after = alive(&planted[0]);
    cleanup(&planted);
    assert!(alive_after, "the check never kills");
}
