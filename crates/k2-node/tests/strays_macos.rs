//! macOS: a process a job left running outside its process group (a test's
//! `setsid` fake daemon, the frpc stubs that piled up on the LAN minis) is
//! ended by the node at job end and named in the job's log; see
//! `k2_node::strays`.
#![cfg(target_os = "macos")]

mod common;

use common::*;
use k2_node_proto::frames::{Frame, JobState, LogStream};

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setsid_stray_left_by_a_job_is_ended_and_logged() {
    let mut fake = Fake::start().await;
    let _n = start_node(&fake, "e2e-strays", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let _ = c.until(|f| matches!(f, Frame::Offer(_))).await;

    // The job forks a child that leaves its group (setsid), ignores SIGTERM
    // and keeps the job's folder as its cwd; the job itself exits 0.
    let perl = r#"use POSIX; my $p = fork; if ($p) { print "stray $p\n"; exit 0 } POSIX::setsid(); open STDOUT, ">/dev/null"; open STDERR, ">/dev/null"; exec "/bin/sh", "-c", "trap \"\" TERM; while :; do sleep 1; done""#;
    c.send(Frame::Assign(assign(plan("s1", &["/usr/bin/perl", "-e", perl]), 1)));
    let fr = run_to_receipt(&mut c, "s1").await;
    let end = states_of(&fr, "s1").into_iter().last().unwrap();
    assert_eq!(end.state, JobState::Done);
    let logs = logs_of(&fr, "s1");
    let out = text(&logs, LogStream::Out);
    let pid: i32 = out.lines().find_map(|l| l.strip_prefix("stray ")).and_then(|p| p.trim().parse().ok()).unwrap_or_else(|| panic!("no stray pid: {out}"));
    let sys = text(&logs, LogStream::Sys);
    // The stray shell and, when caught mid-loop, its `sleep` (same cwd).
    assert!(sys.contains(" process(es) the job left running outside its group: "), "{sys}");
    assert!(sys.contains(&format!("pid {pid} (KILL)")), "SIGTERM is ignored, so KILL: {sys}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if alive(pid) {
        // SAFETY: the test's own child; don't leak it either way.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        panic!("stray {pid} still alive after the job ended");
    }
}
