//! Every limit the host puts on the worker, proven with a `test-hostile`
//! build (`cargo test -p k2-deckrender --features test-hostile`): the worker
//! sleeps, spins, allocates through its allocator and around it, aborts, tries
//! the network and reports its environment, and each ends typed.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_deckrender::host::{self, Limits, Request};
use k2_deckrender::proto::{FailureCode, SlideRef};

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_k2-deckrender"))
}

fn hostile(what: &str, limits: &Limits) -> host::Failure {
    let req = Request::new(b"not read", SlideRef::Index(0), 64);
    let started = Instant::now();
    let f = host::render_with(bin(), &req, limits, Some(what)).expect_err("a hostile run never renders");
    eprintln!("[deckrender] hostile {what}: {} after {} ms: {} | stderr: {}", f.code, started.elapsed().as_millis(), f.detail, f.stderr.trim());
    f
}

fn short() -> Limits {
    Limits { wall: Duration::from_secs(2), memory_bytes: 256 << 20, ..Limits::default() }
}

#[test]
fn the_build_says_it_is_a_test_build() {
    let out = std::process::Command::new(bin()).arg("--version").output().expect("run");
    let v = String::from_utf8_lossy(&out.stdout);
    assert!(v.contains("K2_DECKRENDER_TEST_HOSTILE_ENABLED"), "{v}");
}

#[test]
fn a_sleeping_or_spinning_worker_is_killed_at_the_wall_deadline() {
    for what in ["sleep", "spin"] {
        let started = Instant::now();
        let f = hostile(what, &short());
        assert_eq!(f.code, FailureCode::Timeout, "{what}");
        assert!(started.elapsed() < Duration::from_secs(10), "{what} took {:?}", started.elapsed());
    }
}

#[test]
fn memory_past_the_cap_is_too_large_through_the_allocator_and_around_it() {
    let f = hostile("alloc", &short());
    assert_eq!(f.code, FailureCode::TooLarge);
    assert!(f.detail.contains("memory"), "{}", f.detail);
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        // malloc + touch, never counted: RLIMIT_AS (Linux), the watchdog on
        // RSS / the footprint (Linux, macOS) and the Job Object's memory cap
        // (Windows) stop it.
        let l = Limits { wall: Duration::from_secs(30), ..short() };
        assert_eq!(hostile("alloc_raw", &l).code, FailureCode::TooLarge);
    }
}

/// Sixteen spinning threads use up the CPU time long before the wall
/// deadline: RLIMIT_CPU (SIGXCPU) on unix, the Job Object's CPU time on
/// Windows.
#[test]
fn a_spin_on_many_threads_runs_out_of_cpu_time() {
    let l = Limits { wall: Duration::from_secs(60), ..short() };
    let started = Instant::now();
    let f = hostile("spin_threads", &l);
    assert_eq!(f.code, FailureCode::Timeout, "{}", f.detail);
    assert!(f.detail.contains("CPU limit"), "the CPU cap, not the wall: {}", f.detail);
    assert!(started.elapsed() < Duration::from_secs(50), "before the wall deadline: {:?}", started.elapsed());
}

/// Windows only: the Job Object allows the worker no child process (unix
/// has no per-process limit for that; its seccomp and sandbox profile are
/// about the network and the files).
#[cfg(windows)]
#[test]
fn the_worker_cannot_start_a_child_on_windows() {
    let f = hostile("child", &short());
    assert_eq!(f.code, FailureCode::RendererError);
    assert!(f.detail.starts_with("child: refused"), "{}", f.detail);
}

#[test]
fn a_crash_is_a_renderer_error() {
    // A write to an unmapped address. (Not `abort`: on Windows its fast-fail
    // goes to Windows Error Reporting, which can't start its helper inside the
    // worker's no-children job, so an aborting worker waits for the wall
    // deadline there. A crash in the renderer's code is an access violation,
    // which the job ends at once.)
    assert_eq!(hostile("crash", &short()).code, FailureCode::RendererError);
}

#[test]
fn the_worker_has_no_network() {
    let f = hostile("net", &short());
    assert_eq!(f.code, FailureCode::RendererError);
    if cfg!(target_os = "linux") {
        assert!(f.detail.contains("net: blocked"), "seccomp blocks socket(2): {}", f.detail);
    }
    if cfg!(target_os = "macos") {
        // Best effort: when the sandbox is on, the network is closed.
        if f.detail.contains("sandbox: on") {
            assert!(f.detail.contains("net: blocked"), "{}", f.detail);
        } else {
            eprintln!("[deckrender] macOS: the no-network sandbox is off here: {}", f.detail);
        }
    }
}

#[test]
fn the_worker_inherits_nothing_and_runs_in_a_private_folder_that_goes_away() {
    std::env::set_var("K2_DECKRENDER_SECRET_PROBE", "s3cr3t");
    let f = hostile("env", &short());
    let detail = f.detail;
    let env = detail.split("; cwd: ").next().unwrap_or("");
    // macOS adds __CF_USER_TEXT_ENCODING (the user's text encoding, no secret)
    // to every process it starts; nothing else may be there.
    let names: Vec<&str> = env.trim_start_matches("env: [").trim_end_matches(']').split(',').filter(|n| !n.is_empty()).collect();
    let extra: Vec<&&str> = names.iter().filter(|n| !(cfg!(target_os = "macos") && **n == "__CF_USER_TEXT_ENCODING")).collect();
    assert!(extra.is_empty(), "an empty environment: {detail}");
    assert!(!detail.contains("K2_DECKRENDER_SECRET_PROBE"));
    let cwd = detail.split("; cwd: ").nth(1).unwrap_or("");
    assert!(cwd.contains("k2-deckrender-"), "a fresh folder: {detail}");
    assert!(!Path::new(cwd).exists(), "removed afterwards: {cwd}");
}

#[test]
fn the_limits_are_on_before_the_first_input_byte_is_read() {
    // 1. A worker the host started: what was in force just before it read its
    //    first byte of stdin (taken there, answered by the `limits` mode).
    let limits = short();
    let f = hostile("limits", &limits);
    assert_eq!(f.code, FailureCode::RendererError);
    let seen: std::collections::HashMap<&str, &str> =
        f.detail.trim_start_matches("limits: ").split("; sandbox:").next().unwrap_or("").split(';').filter_map(|kv| kv.split_once('=')).collect();
    let cpu = limits.wall.as_secs() + 5;
    let want = |k: &str, v: String| assert_eq!(seen.get(k).copied(), Some(v.as_str()), "{k} in {}", f.detail);
    want("alloc_cap", limits.memory_bytes.to_string());
    if cfg!(unix) {
        want("core", "0/0".into());
        want("fsize", "0/0".into());
        want("nofile", format!("{0}/{0}", k2_deckrender::sandbox::NOFILE));
        want("cpu", format!("{cpu}/{}", cpu + 1));
    }
    // An attribute, not `if cfg!`: libc has no SIGKILL on Windows.
    #[cfg(target_os = "linux")]
    {
        let as_cap = limits.memory_bytes + host::ADDRESS_SPACE_HEADROOM;
        want("as", format!("{as_cap}/{as_cap}"));
        want("no_new_privs", "1".into());
        want("seccomp", "2".into());
        want("pdeathsig", libc::SIGKILL.to_string());
        // Our filter's own answer to socket(2): EACCES (13).
        want("socket", "errno13".into());
    }

    // 2. A step that fails stops the worker before it reads anything: a parent
    //    that isn't ours, with stdin left open and never written (a worker that
    //    read it would wait here until the deadline).
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::process::{Command, Stdio};
        let not_parent = k2_deckrender::sandbox::SelfLimits { parent: 1, cpu_secs: 10, memory_bytes: 256 << 20, address_space: 0 };
        let mut child =
            Command::new(bin()).args(not_parent.to_args()).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("spawn");
        let _stdin_kept_open = child.stdin.take();
        let started = Instant::now();
        let mut out = String::new();
        child.stdout.take().expect("stdout").read_to_string(&mut out).expect("read");
        let st = child.wait().expect("wait");
        assert!(started.elapsed() < Duration::from_secs(10), "it waited for input");
        assert_eq!(st.code(), Some(k2_deckrender::proto::EXIT_FAILED), "{out}");
        assert!(out.contains("\"renderer_error\"") && out.contains("parent"), "{out}");
    }
}
