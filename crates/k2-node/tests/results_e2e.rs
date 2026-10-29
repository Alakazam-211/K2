//! Results back on the real node against the loopback fake controller:
//! `--artifact` globs are kept and hashed after the command exits, JUnit
//! among them becomes a test summary in the terminal `state`, the signed
//! receipt lists the files, and `artifact_get` streams one back.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use common::*;
use k2_node_proto::crypto;
use k2_node_proto::frames::{ArtifactGet, Frame, JobState};
use k2_node_proto::results;

const JUNIT: &str = r#"<testsuites><testsuite name="k2-core"><testcase name="ok_one" classname="k2-core"/><testcase name="bad_one" classname="k2-core"><failure type="test failure">thread bad_one panicked at src/x.rs:1:1:
boom</failure></testcase><testcase name="skip_one"><skipped/></testcase></testsuite></testsuites>"#;

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_and_junit_come_back_with_the_result() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-results", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let offer = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offer.last() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == results::FEATURE_ARTIFACTS), "the offer says artifacts: {:?}", o.features);

    let script = format!("mkdir -p out/sub && printf '%s' '{JUNIT}' > out/junit.xml && printf 'bin' > out/sub/tool.bin && exit 100");
    let mut p = plan("r1", &["/bin/sh", "-c", &script]);
    p.artifacts = vec!["out/*.xml".into(), "out/**/*.bin".into(), "missing/*".into()];
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "r1").await;
    let end = states_of(&fr, "r1").into_iter().last().unwrap();
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(100)));
    let o = end.outputs.expect("the terminal state carries the outputs");
    let names: Vec<&str> = o.artifacts.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["out/junit.xml", "out/sub/tool.bin"]);
    assert_eq!(o.artifacts[1].sha256, crypto::sha256_hex(b"bin"));
    let t = o.tests.expect("JUnit parsed on the node");
    assert_eq!((t.total, t.passed, t.failed, t.skipped), (3, 1, 1, 1));
    assert_eq!(t.failing[0].name, "bad_one");
    assert!(t.failing[0].message.contains("panicked at src/x.rs"), "{}", t.failing[0].message);
    // CE-23: the note names the folder searched (the job's cwd, as a
    // job-relative path) and that globs are relative to it.
    assert!(
        o.notes.iter().any(|n| n.starts_with("missing/* matched nothing under <job>/src") || n.starts_with("missing/* matched nothing under <job> (")),
        "{:?}",
        o.notes
    );
    // CE-4, CE-14, CE-19: every result carries the tools, usage and host.
    assert!(o.resources.is_some() && o.host.as_ref().is_some_and(|h| h.start.is_some() && h.end.is_some()), "{:?} {:?}", o.resources, o.host);

    let Frame::Receipt(r) = fr.last().unwrap() else { unreachable!() };
    let receipt = r.receipt.verify(n.node.key.spki_der()).unwrap();
    assert_eq!(receipt.artifacts, o.artifacts, "the signed receipt lists the same files");
    let logs = logs_of(&fr, "r1");
    let sys = text(&logs, k2_node_proto::frames::LogStream::Sys);
    assert!(sys.contains("[k2-node] tests: 1 passed, 1 failed, 0 errors, 1 skipped"), "{sys}");

    // Pull one back.
    c.send(Frame::ArtifactGet(ArtifactGet { request_id: "q1".into(), job_id: "r1".into(), generation: 1, name: "out/junit.xml".into() }));
    let got = c.until(|f| matches!(f, Frame::ArtifactChunk(a) if a.request_id == "q1" && a.last)).await;
    let mut bytes = Vec::new();
    for f in &got {
        if let Frame::ArtifactChunk(a) = f {
            bytes.extend(crypto::unb64(&a.data).unwrap());
        }
    }
    assert_eq!(String::from_utf8(bytes).unwrap(), JUNIT);
    c.send(Frame::ArtifactGet(ArtifactGet { request_id: "q2".into(), job_id: "r1".into(), generation: 1, name: "../../state/node-key.pem".into() }));
    let got = c.until(|f| matches!(f, Frame::ArtifactFailed(a) if a.request_id == "q2")).await;
    let Some(Frame::ArtifactFailed(a)) = got.last() else { unreachable!() };
    assert_eq!(a.code, "artifact_not_found", "only names the job reported, never a path");
    drop(n);
}

/// A job that prints nothing for `stall_secs` ends as `timeout/stalled`
/// (its whole group is stopped); one that keeps printing runs to its end.
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_job_stalls_out_and_a_chatty_one_does_not() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-stall", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let offer = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offer.last() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == k2_node_proto::frames::FEATURE_STALL), "{:?}", o.features);

    let mut p = plan("s1", &["/bin/sh", "-c", "echo hi; sleep 30"]);
    p.stall_secs = Some(1);
    let t = std::time::Instant::now();
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "s1").await;
    let end = states_of(&fr, "s1").into_iter().last().unwrap();
    assert_eq!((end.state, end.reason.as_deref(), end.detail.as_deref()), (JobState::Timeout, Some("stalled"), Some("no output for 1s")));
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "stopped long before the 30 s sleep: {:?}", t.elapsed());
    let sys = text(&logs_of(&fr, "s1"), k2_node_proto::frames::LogStream::Sys);
    assert!(sys.contains("[k2-node] stopping job: stalled"), "{sys}");

    let mut p = plan("s2", &["/bin/sh", "-c", "for i in 1 2 3 4 5 6 7; do echo $i; sleep 0.3; done"]);
    p.stall_secs = Some(1);
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "s2").await;
    let end = states_of(&fr, "s2").into_iter().last().unwrap();
    assert_eq!((end.state, end.exit.and_then(|e| e.code)), (JobState::Done, Some(0)), "output kept it alive for over 2 s");
    drop(n);
}

/// CE-10 (D4, `stall-cpu`): a job that prints nothing but burns CPU isn't
/// stalled; it runs to its wall clock (`timeout`, not `stalled`), and the
/// log says it's quiet but busy. Its CPU time is in the result (CE-14).
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_busy_job_is_not_stalled() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-busy", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let offer = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offer.last() else { unreachable!() };
    for f in [
        k2_node_proto::frames::FEATURE_STALL_CPU,
        results::FEATURE_TOOLS_USED,
        results::FEATURE_JOB_RESOURCES,
        k2_node_proto::needs::FEATURE_RUST_COMPONENTS,
        k2_node_proto::needs::FEATURE_TOOLCHAIN_BUDGET,
        k2_node_proto::env::FEATURE_CPU_HINTS,
        k2_node_proto::env::FEATURE_JOB_PORTS,
    ] {
        assert!(o.features.iter().any(|x| x == f), "the offer says {f}: {:?}", o.features);
    }

    let mut p = plan("b1", &["/bin/sh", "-c", "echo start; while :; do :; done"]);
    p.stall_secs = Some(1);
    p.limits.max_secs = 4;
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "b1").await;
    let end = states_of(&fr, "b1").into_iter().last().unwrap();
    assert_eq!((end.state, end.reason.as_deref()), (JobState::Timeout, Some("timeout")), "busy: the wall clock ended it, not the stall watch");
    let sys = text(&logs_of(&fr, "b1"), k2_node_proto::frames::LogStream::Sys);
    assert!(sys.contains("[k2-node] quiet but busy: no output for"), "{sys}");
    assert_eq!(sys.matches("quiet but busy").count(), 1, "once per 30 min, not every tick: {sys}");
    let r = end.outputs.as_ref().and_then(|o| o.resources.clone()).expect("resources on every end");
    assert!(r.cpu_ms.is_some_and(|c| c >= 1_000), "about 4 s of one CPU: {r:?}");
    assert_ne!(r.cpu_source, "none", "{r:?}");
    drop(n);
}

/// CE-15 + loopback ports: the job's CPUs as thread hints unless its env
/// sets them, `K2_JOB_CPUS`, and its own port block; CE-4 the tools line
/// and the signed `tools_used`; CE-14 a peak memory; CE-19 the host stamps.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_gets_its_cpus_ports_tools_usage_and_host_stamp() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-hints", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let script = "echo cpus=$K2_JOB_CPUS cargo=$CARGO_BUILD_JOBS omp=$OMP_NUM_THREADS nextest=$NEXTEST_TEST_THREADS rtt=$RUST_TEST_THREADS base=$K2_JOB_PORT_BASE count=$K2_JOB_PORT_COUNT; \
                  x=$(head -c 120000000 /dev/zero | tr '\\0' a); sleep 2; echo ${#x}";
    let mut p = plan("h1", &["/bin/sh", "-c", script]);
    p.limits.cpu_millis = Some(2500);
    p.env.insert("RUST_TEST_THREADS".into(), "7".into());
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "h1").await;
    let out = text(&logs_of(&fr, "h1"), k2_node_proto::frames::LogStream::Out);
    assert!(out.starts_with("cpus=3 cargo=3 omp=3 nextest=3 rtt=7 base=20000 count=100\n"), "--cpus 2.5 rounds up; the job's own env wins: {out}");
    assert!(out.contains("120000000"), "{out}");
    let sys = text(&logs_of(&fr, "h1"), k2_node_proto::frames::LogStream::Sys);
    assert!(sys.contains("[k2-node] cpus: 3 (K2_JOB_CPUS;") && sys.contains("ports 20000-20099"), "{sys}");
    assert!(sys.contains("[k2-node] tools: "), "{sys}");
    assert!(sys.contains("[k2-node] host at start: load ") && sys.contains("[k2-node] host at end: load "), "{sys}");
    assert!(sys.contains("[k2-node] used: peak memory "), "{sys}");
    let end = states_of(&fr, "h1").into_iter().last().unwrap();
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(0)));
    let o = end.outputs.expect("outputs on every end, artifacts or not");
    let r = o.resources.clone().expect("resources");
    assert!(r.peak_mem_bytes.is_some_and(|m| m >= 100_000_000), "a 120 MB string: {r:?}");
    let h = o.host.clone().expect("host stamps");
    let (s, e) = (h.start.expect("start"), h.end.expect("end"));
    assert!(s.cpus >= 1 && e.at >= s.at, "{s:?} {e:?}");
    assert_eq!(s.steal_permille.is_some(), cfg!(target_os = "linux"), "steal on Linux, null on macOS: {s:?}");
    let Frame::Receipt(rf) = fr.last().unwrap() else { unreachable!() };
    let receipt = rf.receipt.verify(n.node.key.spki_der()).unwrap();
    assert_eq!(receipt.tools_used, o.tools_used, "the signed receipt carries the same tools");
    drop(n);
}
