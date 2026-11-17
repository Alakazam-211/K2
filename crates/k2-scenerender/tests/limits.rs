//! Every limit on the worker, proven with a `test-hostile` build
//! (`cargo test -p k2-scenerender --features test-hostile`): the worker
//! sleeps, spins, allocates through its allocator and around it, aborts,
//! tries the network, writes a file, reports its environment and the limits
//! in force when it read its first byte, and each ends typed.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_scenerender::camera::{Framing, ViewSpec};
use k2_scenerender::host::{self, Limits, SceneRequest};
use k2_scenerender::proto::{Ao, FailureCode, View};
use k2_scenerender::raster::BAND_PX;
use k2_scenerender::render::{AO_BYTES_PER_PX, IDS_BYTES_PER_PX, IDS_KEPT_PER_PX, IDS_TABLE_BYTES};
use k2_scenerender::scene::encode;
use k2_scenerender::testgen;

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_k2-scenerender"))
}

fn hostile(what: &str, limits: &Limits) -> host::Failure {
    let req = SceneRequest::new(b"not read", vec![View::new(ViewSpec::Auto, 64)]);
    let started = Instant::now();
    let f = host::render_with(bin(), &req, limits, Some(what)).expect_err("a hostile run never renders");
    eprintln!("[scenerender] hostile {what}: {} after {} ms: {} | stderr: {}", f.code, started.elapsed().as_millis(), f.detail, f.stderr.trim());
    f
}

fn short() -> Limits {
    Limits { wall: Duration::from_secs(2), memory_bytes: 256 << 20, ..Limits::thumbnail() }
}

#[test]
fn the_build_says_it_is_a_test_build() {
    let out = std::process::Command::new(bin()).arg("--version").output().expect("run");
    let v = String::from_utf8_lossy(&out.stdout);
    assert!(v.contains("K2_SCENERENDER_TEST_HOSTILE_ENABLED"), "{v}");
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

/// Ambient occlusion's buffers (a depth image and a factor per pixel) are in
/// the worker's up-front estimate: under a cap between the plain still's
/// estimate and the occluded one's, the plain still draws and the occluded
/// one is refused before any drawing; at a cap of its own estimate it draws,
/// so what it allocates stays inside it.
#[test]
fn ambient_occlusion_buffers_are_in_the_memory_estimate() {
    let input = encode(&testgen::interior_scene());
    let plain = SceneRequest::new(&input, vec![View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 1920) }]);
    let ao = SceneRequest { ao: Some(Ao::default()), ..plain.clone() };
    let l = Limits { wall: Duration::from_secs(60), ..Limits::still() };
    let a = host::render(bin(), &plain, &l).unwrap_or_else(|f| panic!("plain: {} {}", f.code, f.detail));
    let b = host::render(bin(), &ao, &l).unwrap_or_else(|f| panic!("ao: {} {}", f.code, f.detail));
    let (w, h) = (b.views[0].meta.width as u64, b.views[0].meta.height as u64);
    assert_eq!(b.estimate_bytes - a.estimate_bytes, AO_BYTES_PER_PX * w * h);
    eprintln!("[scenerender] estimate at {w} x {h}: {} MiB plain, {} MiB with occlusion", a.estimate_bytes >> 20, b.estimate_bytes >> 20);
    let between = Limits { memory_bytes: a.estimate_bytes + (b.estimate_bytes - a.estimate_bytes) / 2, ..l };
    assert!(host::render(bin(), &plain, &between).is_ok());
    let f = host::render(bin(), &ao, &between).expect_err("over the cap");
    assert_eq!(f.code, FailureCode::TooLarge, "{}", f.detail);
    assert!(f.detail.contains("memory"), "{}", f.detail);
    let own = Limits { memory_bytes: b.estimate_bytes, ..l };
    let r = host::render(bin(), &ao, &own).unwrap_or_else(|f| panic!("at its own estimate: {} {} | {}", f.code, f.detail, f.stderr));
    assert_eq!(r.views[0].image, b.views[0].image);
}

/// The id image's buffers (the image, its encoding, a u32 per band sample,
/// the tables) are in the estimate too: under a cap between the plain still's
/// estimate and the one with ids, the plain still draws and the one with ids
/// is refused before any drawing; at its own estimate it draws.
#[test]
fn id_buffers_are_in_the_memory_estimate() {
    let input = encode(&testgen::interior_scene());
    let plain = SceneRequest::new(&input, vec![View { framing: Framing::Contents, ..View::new(ViewSpec::Auto, 1920) }]);
    let ids = SceneRequest { ids: true, ..plain.clone() };
    let l = Limits { wall: Duration::from_secs(60), ..Limits::still() };
    let a = host::render(bin(), &plain, &l).unwrap_or_else(|f| panic!("plain: {} {}", f.code, f.detail));
    let b = host::render(bin(), &ids, &l).unwrap_or_else(|f| panic!("ids: {} {}", f.code, f.detail));
    let m = &b.views[0].meta;
    let (w, h, s) = (m.width as u64, m.height as u64, m.ssaa as u64);
    let bands = l.threads as u64 * w * s * BAND_PX as u64 * s * 4;
    assert_eq!(b.estimate_bytes - a.estimate_bytes, (IDS_BYTES_PER_PX + IDS_KEPT_PER_PX) * w * h + bands + IDS_TABLE_BYTES);
    eprintln!("[scenerender] estimate at {w} x {h}: {} MiB plain, {} MiB with ids", a.estimate_bytes >> 20, b.estimate_bytes >> 20);
    let between = Limits { memory_bytes: a.estimate_bytes + (b.estimate_bytes - a.estimate_bytes) / 2, ..l };
    assert!(host::render(bin(), &plain, &between).is_ok());
    let f = host::render(bin(), &ids, &between).expect_err("over the cap");
    assert_eq!(f.code, FailureCode::TooLarge, "{}", f.detail);
    assert!(f.detail.contains("memory"), "{}", f.detail);
    let own = Limits { memory_bytes: b.estimate_bytes, ..l };
    let r = host::render(bin(), &ids, &own).unwrap_or_else(|f| panic!("at its own estimate: {} {} | {}", f.code, f.detail, f.stderr));
    assert_eq!(r.views[0].image, a.views[0].image);
    assert_eq!(r.views[0].ids, b.views[0].ids);
    assert!(r.views[0].ids.is_some() && a.views[0].ids.is_none());
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
            eprintln!("[scenerender] macOS: the no-network sandbox is off here: {}", f.detail);
        }
    }
}

#[test]
fn the_worker_writes_no_file() {
    let f = hostile("write", &short());
    assert_eq!(f.code, FailureCode::RendererError);
    if cfg!(unix) {
        assert!(f.detail.contains("write: blocked"), "RLIMIT_FSIZE 0: {}", f.detail);
    }
}

#[test]
fn the_worker_inherits_nothing_and_runs_in_a_private_folder_that_goes_away() {
    std::env::set_var("K2_SCENERENDER_SECRET_PROBE", "s3cr3t");
    let f = hostile("env", &short());
    let detail = f.detail;
    let env = detail.split("; cwd: ").next().unwrap_or("");
    // macOS adds __CF_USER_TEXT_ENCODING (the user's text encoding, no secret)
    // to every process it starts; nothing else may be there.
    let names: Vec<&str> = env.trim_start_matches("env: [").trim_end_matches(']').split(',').filter(|n| !n.is_empty()).collect();
    let extra: Vec<&&str> = names.iter().filter(|n| !(cfg!(target_os = "macos") && **n == "__CF_USER_TEXT_ENCODING")).collect();
    assert!(extra.is_empty(), "an empty environment: {detail}");
    assert!(!detail.contains("K2_SCENERENDER_SECRET_PROBE"));
    let cwd = detail.split("; cwd: ").nth(1).unwrap_or("");
    assert!(cwd.contains("k2-scenerender-"), "a fresh folder: {detail}");
    assert!(!Path::new(cwd).exists(), "removed afterwards: {cwd}");
}

/// The limits are in force when the first byte of stdin is read: the worker
/// records them just before that read, and reports the record.
#[test]
fn the_limits_are_in_force_at_the_first_read() {
    let l = short();
    let f = hostile("limits", &l);
    assert_eq!(f.code, FailureCode::RendererError);
    let d = f.detail.clone();
    let field = |k: &str| -> String {
        d.split(&[' ', ';'][..]).find_map(|p| p.strip_prefix(&format!("{k}="))).unwrap_or_else(|| panic!("no {k} in {d}")).to_string()
    };
    if cfg!(unix) {
        assert_eq!(field("core"), "0/0", "{d}");
        assert_eq!(field("fsize"), "0/0", "{d}");
        assert_eq!(field("nofile"), "64/64", "{d}");
        let cpu = l.args(1).limits.cpu_secs;
        assert_eq!(field("cpu"), format!("{cpu}/{}", cpu + 1), "{d}");
    }
    if cfg!(target_os = "linux") {
        let as_cap = l.memory_bytes + host::ADDRESS_SPACE_HEADROOM;
        assert_eq!(field("as"), format!("{as_cap}/{as_cap}"), "{d}");
        assert_eq!(field("no_new_privs"), "1", "{d}");
        assert_eq!(field("seccomp"), "2", "a filter: {d}");
        assert_eq!(field("pdeathsig"), "9", "SIGKILL: {d}");
        assert_eq!(field("open_fds"), "0", "nothing inherited past stdio: {d}");
    }
}

/// A worker started by anything but its host (another parent) limits
/// nothing, reads nothing and says why.
#[test]
fn a_worker_whose_parent_isnt_the_host_reads_nothing() {
    let mut args = Limits::thumbnail().args(1).to_args();
    // pid 1 is never this test.
    args[1] = "parent=1".into();
    let out = std::process::Command::new(bin())
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|c| c.wait_with_output())
        .expect("run");
    let line = String::from_utf8_lossy(&out.stdout);
    if cfg!(unix) {
        assert!(line.contains(r#""code":"renderer_error""#) && line.contains("parent"), "{line}");
        assert_eq!(out.status.code(), Some(k2_scenerender::proto::EXIT_FAILED));
    }
}
