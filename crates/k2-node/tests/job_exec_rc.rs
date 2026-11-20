//! The `job-exec` shim's rc file (2026-11-16, job 265ae46f): with
//! `rc_file` in its spec the shim runs the job's command as its child,
//! records how it ended, and ends the same way, so systemd and the rc file
//! agree and a k2-node that adopted the job after an update can prove the
//! exit even when systemd no longer records the unit. Runs the real binary
//! (the shim changes its own signal handling; never in the test process).
#![cfg(unix)]

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_node::wall::{read_rc, write_exec_spec, ExecSpec, EXEC_RC, EXEC_SPEC};
use k2_node_proto::frames::ExitInfo;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-node"))
}

fn root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-job-exec-rc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn spec(root: &Path, script: &str, rc: bool) -> PathBuf {
    let s = ExecSpec {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        cwd: root.to_path_buf(),
        stdin_env: false,
        rc_file: rc.then(|| root.join(EXEC_RC)),
    };
    write_exec_spec(root, &s).unwrap()
}

/// Run the shim like a unit does, in its own process group; `during` gets
/// the group id once the command has said it started (its stdout).
fn run(root: &Path, spec: &Path, during: impl FnOnce(i32)) -> std::process::ExitStatus {
    let out = root.join("out.txt");
    let f = std::fs::File::create(&out).unwrap();
    let mut c = std::process::Command::new(bin()).arg("job-exec").arg(spec).stdout(f).stderr(std::process::Stdio::inherit()).process_group(0).spawn().unwrap();
    let pgid = c.id() as i32;
    let t0 = Instant::now();
    while !std::fs::read_to_string(&out).unwrap().contains("started") {
        if let Some(s) = c.try_wait().unwrap() {
            return s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "the job never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    during(pgid);
    let t0 = Instant::now();
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "the shim never ended");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_shim_records_a_nonzero_exit_and_exits_the_same() {
    let d = root("code");
    let p = spec(&d, "echo started; exit 7", true);
    let s = run(&d, &p, |_| {});
    assert_eq!(s.code(), Some(7), "{s:?}");
    assert!(!p.exists(), "the spec is read once, then gone");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(7), signal: None }));
}

#[test]
fn the_shim_records_exit_zero() {
    let d = root("zero");
    let s = run(&d, &spec(&d, "echo started; exit 0", true), |_| {});
    assert_eq!(s.code(), Some(0), "{s:?}");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(0), signal: None }));
}

/// A stop signals the whole unit: the shim outlives its child (it ignores
/// the stop itself), records the child's signal and ends by the same
/// signal. The child got the default disposition back (else it would
/// ignore the stop and this test would time out).
#[test]
fn a_stop_of_the_whole_group_is_recorded_as_the_childs_signal() {
    let d = root("stop");
    let s = run(&d, &spec(&d, "echo started; sleep 30; echo never", true), |pgid| {
        // SAFETY: signalling the process group this test made.
        assert_eq!(unsafe { libc::kill(-pgid, libc::SIGTERM) }, 0);
    });
    assert_eq!(s.signal(), Some(libc::SIGTERM), "{s:?}");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: None, signal: Some(libc::SIGTERM) }));
    assert!(!std::fs::read_to_string(d.join("out.txt")).unwrap().contains("never"));
}

#[test]
fn a_command_killed_by_a_signal_is_recorded_so() {
    let d = root("kill");
    let s = run(&d, &spec(&d, "echo started; kill -KILL $$", true), |_| {});
    assert_eq!(s.signal(), Some(libc::SIGKILL), "{s:?}");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: None, signal: Some(libc::SIGKILL) }));
}

#[test]
fn a_command_that_cant_start_is_127_and_recorded() {
    let d = root("missing");
    let s = ExecSpec { argv: vec!["/nonexistent/k2-test-program".into()], env: vec![], cwd: d.clone(), stdin_env: false, rc_file: Some(d.join(EXEC_RC)) };
    let p = write_exec_spec(&d, &s).unwrap();
    let st = std::process::Command::new(bin()).arg("job-exec").arg(&p).stderr(std::process::Stdio::null()).status().unwrap();
    assert_eq!(st.code(), Some(127));
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(127), signal: None }));
}

/// A job unit started a second time from outside k2-node (needrestart's
/// `systemctl restart` after an apt upgrade): the first run took the spec,
/// so the second runs nothing, exits 66 (not 127, "command not found")
/// and says why on stderr (the unit's journal), and leaves the first
/// run's rc file as it was.
#[test]
fn a_second_start_of_the_same_unit_runs_nothing_and_says_why() {
    let d = root("restart");
    let p = spec(&d, "echo started; exit 3", true);
    let first = run(&d, &p, |_| {});
    assert_eq!(first.code(), Some(3));
    let out = std::process::Command::new(bin()).arg("job-exec").arg(&p).output().unwrap();
    assert_eq!(out.status.code(), Some(i32::from(k2_node::wall::SPEC_GONE_EXIT)), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("this job's folder is gone") && err.contains("restarted outside k2-node"), "{err}");
    assert_eq!(read_rc(&d.join(EXEC_RC)), Some(ExitInfo { code: Some(3), signal: None }), "the first run's end stands");
}

/// Without `rc_file` (a 0.46.13 spec) the shim execs as before: same exit,
/// no rc file.
#[test]
fn without_an_rc_file_the_shim_execs_as_before() {
    let d = root("exec");
    let s = run(&d, &spec(&d, "echo started; exit 5", false), |_| {});
    assert_eq!(s.code(), Some(5), "{s:?}");
    assert!(!d.join(EXEC_RC).exists());
    assert!(!d.join(EXEC_SPEC).exists());
}
