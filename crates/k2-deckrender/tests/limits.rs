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
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        // malloc + touch, never counted: RLIMIT_AS (Linux) and the watchdog on
        // RSS / the footprint (Linux, macOS) stop it.
        let l = Limits { wall: Duration::from_secs(30), ..short() };
        assert_eq!(hostile("alloc_raw", &l).code, FailureCode::TooLarge);
    }
}

#[test]
fn a_crash_is_a_renderer_error() {
    assert_eq!(hostile("abort", &short()).code, FailureCode::RendererError);
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
