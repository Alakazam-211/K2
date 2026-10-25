//! Compute secret-env on the node, without root (prd-compute-secret-env-v1
//! §10 "Node"): the `job-exec` shim takes the secret env from stdin and
//! nothing else does; an unwalled node never offers `secret-env` and
//! refuses a secret plan at assign with no ledger row (SE16, SE17).
//! The walled path (executor, unit, redaction, SE26) is in
//! `job_wall_linux.rs` (root). Fake secrets only.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use common::*;
use k2_node_proto::frames::{Frame, JobState};
use k2_node_proto::secret::{codes, FEATURE_SECRET_ENV, PIPE_MAGIC};

const VALUE: &str = "k2test-secret-SHIM-q8Zr3Lx0Pw";

fn spec_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("k2n-secret-{tag}-{}", k2_node_proto::crypto::random_id().unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run `k2-node job-exec <spec>` with `stdin` on its stdin.
fn shim(spec: &k2_node::wall::ExecSpec, stdin: &[u8], tag: &str) -> (i32, String, String, std::path::PathBuf) {
    let d = spec_dir(tag);
    // Written by hand (write_exec_spec also hands the dir to the node group, which
    // a test under the job wall may not do); the shim reads and deletes it.
    let p = d.join("exec.json");
    std::fs::write(&p, serde_json::to_vec(spec).unwrap()).unwrap();
    let body = std::fs::read_to_string(&p).unwrap();
    assert!(!body.contains(VALUE), "exec.json carries the value: {body}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_k2-node"))
        .args(["job-exec", p.to_str().unwrap()])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A shim that stops reading early (over the cap) closes the pipe: EPIPE here is fine.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into(), d)
}

fn spec(script: &str, d: &std::path::Path) -> k2_node::wall::ExecSpec {
    k2_node::wall::ExecSpec {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        cwd: d.to_path_buf(),
        stdin_env: true,
    }
}

fn pipe_body(json: &str) -> Vec<u8> {
    [PIPE_MAGIC, json.as_bytes()].concat()
}

#[test]
fn the_shim_takes_the_secret_env_from_stdin_and_closes_it() {
    let d = spec_dir("ok");
    // The job sees the value; its stdin is /dev/null afterwards.
    let s = spec("printf 'v=%s\\n' \"$K2T_SECRET\"; if [ -z \"$(cat)\" ]; then echo stdin-empty; fi", &d);
    let (code, out, err, dir) = shim(&s, &pipe_body(&format!(r#"{{"env":{{"K2T_SECRET":"{VALUE}"}}}}"#)), "ok");
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, format!("v={VALUE}\nstdin-empty\n"));
    assert!(!dir.join("exec.json").exists(), "the spec is read once, then gone");
}

#[test]
fn the_shim_refuses_a_bad_secret_env_and_never_prints_the_value() {
    let d = spec_dir("bad");
    let s = spec("echo ran", &d);
    for (tag, body, want) in [
        ("nomagic", format!(r#"{{"env":{{"K2T_SECRET":"{VALUE}"}}}}"#).into_bytes(), "no secret env on stdin"),
        ("empty", Vec::new(), "no secret env on stdin"),
        ("reserved", pipe_body(&format!(r#"{{"env":{{"K2_TOKEN":"{VALUE}"}}}}"#)), "K2_TOKEN"),
        ("loader", pipe_body(&format!(r#"{{"env":{{"LD_PRELOAD":"{VALUE}"}}}}"#)), "LD_PRELOAD"),
        ("overlap", pipe_body(&format!(r#"{{"env":{{"PATH":"{VALUE}"}}}}"#)), "PATH"),
        ("big", pipe_body(&format!(r#"{{"env":{{"K2T_BIG":"{}"}}}}"#, "a".repeat(70 * 1024))), "over"),
        ("garbage", pipe_body("{not json"), "doesn't parse"),
    ] {
        let (code, out, err, _) = shim(&s, &body, tag);
        assert_ne!(code, 0, "{tag}: ran anyway: {out}");
        assert!(!out.contains("ran"), "{tag}: the job ran: {out}");
        assert!(err.contains(want), "{tag}: {err}");
        assert!(!err.contains(VALUE) && !out.contains(VALUE), "{tag}: a refusal printed the value: {err}");
    }
    // A name the job's env already sets.
    let mut s2 = spec("echo ran", &d);
    s2.env.push(("K2T_SECRET".into(), "plain".into()));
    let (code, _, err, _) = shim(&s2, &pipe_body(&format!(r#"{{"env":{{"K2T_SECRET":"{VALUE}"}}}}"#)), "dup");
    assert_ne!(code, 0);
    assert!(err.contains("also in the job's env") && !err.contains(VALUE), "{err}");
}

/// SE16/SE17 on a node without the job wall: no `secret-env` in the offer,
/// and a secret plan fails at assign (`secret_needs_wall`) before any
/// ledger row; a plain plan still runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_unwalled_node_never_takes_a_secret_job() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "secret-unwalled", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let offer = n.node.offer(&n.node.owner());
    assert!(!offer.features.iter().any(|f| f == FEATURE_SECRET_ENV), "{:?}", offer.features);
    let mut p = plan("s1", &["/bin/sh", "-c", "echo should-not-run"]);
    p.secret_env = vec!["K2T_SECRET".into()];
    c.send(Frame::Assign(assign(p, 1)));
    let fr = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "s1" && s.state.is_terminal())).await;
    let s = states_of(&fr, "s1").into_iter().last().unwrap();
    assert_eq!(s.state, JobState::Failed);
    let want = if cfg!(target_os = "linux") { codes::NEEDS_WALL } else { codes::LINUX_ONLY };
    assert_eq!(s.reason.as_deref(), Some(want), "{s:?}");
    assert!(n.node.owner().ledger.lock().unwrap().get("s1", 1).unwrap().is_none(), "no ledger row");
    assert!(!fr.iter().any(|f| matches!(f, Frame::SecretNeed(_))), "never asks for a value");
    c.send(Frame::Assign(assign(plan("p1", &["/bin/sh", "-c", "echo plain"]), 1)));
    let fr = run_to_receipt(&mut c, "p1").await;
    assert_eq!(text(&logs_of(&fr, "p1"), k2_node_proto::frames::LogStream::Out), "plain\n");
}
