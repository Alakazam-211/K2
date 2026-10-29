//! Every limit, proven against the real worker with the hostile exporters
//! (`cargo test -p k2-export --features test-hostile`): memory (the
//! allocator; RLIMIT_AS on Linux; the footprint watchdog on macOS), time
//! (a sleeper and a spinner), output bytes (a flood), cancel (a kill),
//! a panic, an abort, an exit without a final frame, a decode error, and
//! the empty environment.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use k2_export::host::{self, Bounds, Done, Failed, RunError, Spawn};
use k2_export::proto::{Header, Kind, Limits};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-export-lim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tmp dir");
    d
}

struct Run {
    out: Vec<u8>,
    res: Result<Done, Failed>,
    took: Duration,
}

fn export(tag: &str, target: &str, input: &[u8], bounds: Bounds, cancel_after: Option<Duration>, progress: &AtomicU64) -> Run {
    let d = tmp(tag);
    let path = d.join("in");
    std::fs::write(&path, input).expect("write input");
    let header = Header::Export {
        job: tag.into(),
        kind: Kind::Format("bin".into()),
        target: target.into(),
        options: vec![],
        limits: Limits {
            max_input_bytes: 1 << 30,
            max_output_bytes: bounds.max_output_bytes,
            max_pixels: 1 << 20,
            max_pages: 10,
            max_wall_ms: bounds.wall.as_millis() as u64,
            memory_bytes: bounds.memory_bytes,
        },
        name: "x.bin".into(),
        size: input.len() as u64,
    };
    let s = Spawn { bin: PathBuf::from(env!("CARGO_BIN_EXE_k2-export")), cwd: d.clone() };
    let cancel = AtomicBool::new(false);
    let mut out = Vec::new();
    let start = Instant::now();
    let res = std::thread::scope(|scope| {
        if let Some(after) = cancel_after {
            let c = &cancel;
            scope.spawn(move || {
                std::thread::sleep(after);
                c.store(true, Ordering::Release);
            });
        }
        host::run(&s, &header, Some((&path, input.len() as u64)), bounds, &cancel, &|done, _| {
            progress.store(done, Ordering::Release);
        }, &mut out)
    });
    Run { out, res, took: start.elapsed() }
}

fn b(wall_ms: u64, memory_mib: u64, out_mib: u64) -> Bounds {
    Bounds { wall: Duration::from_millis(wall_ms), memory_bytes: memory_mib << 20, max_output_bytes: out_mib << 20 }
}

fn err(r: &Run) -> &RunError {
    match &r.res {
        Err(f) => &f.error,
        Ok(d) => panic!("want a failure, got {d:?}"),
    }
}

fn kit_id(e: &RunError) -> (&str, &serde_json::Value) {
    match e {
        RunError::Kit(v) => (v["id"].as_str().expect("id"), &v["details"]),
        other => panic!("want the kit's error, got {other:?}"),
    }
}

#[test]
fn rust_allocations_past_the_cap_are_export_limit_memory() {
    let r = export("alloc", "hostile-alloc", b"", b(60_000, 128, 64), None, &AtomicU64::new(0));
    let e = err(&r);
    let (id, details) = kit_id(e);
    assert_eq!(id, "MPC-E003", "{e:?}");
    assert_eq!(details["limit"], serde_json::json!("memory"), "{e:?}");
}

#[test]
fn native_memory_past_the_cap_is_a_memory_limit_on_this_os() {
    let r = export("malloc", "hostile-malloc", b"", b(60_000, 128, 64), None, &AtomicU64::new(0));
    let e = err(&r);
    if cfg!(target_os = "macos") {
        // RLIMIT_AS isn't enforced: the footprint watchdog kills it.
        assert_eq!(e, &RunError::Limit("memory"), "{e:?}");
    } else if cfg!(target_os = "linux") {
        // RLIMIT_AS (cap + headroom): malloc fails, the exporter says so.
        let (id, details) = kit_id(e);
        assert_eq!((id, &details["limit"]), ("MPC-E003", &serde_json::json!("memory")), "{e:?}");
    }
}

#[test]
fn a_sleeper_is_killed_at_the_wall_deadline() {
    let r = export("sleep", "hostile-sleep", b"", b(1_000, 256, 64), None, &AtomicU64::new(0));
    assert_eq!(err(&r), &RunError::Limit("time"));
    assert!(r.took < Duration::from_secs(10), "took {:?}", r.took);
}

#[test]
fn a_spinner_is_killed_at_the_wall_deadline() {
    let r = export("spin", "hostile-spin", b"", b(1_000, 256, 64), None, &AtomicU64::new(0));
    assert_eq!(err(&r), &RunError::Limit("time"));
}

#[test]
fn a_flood_past_the_output_cap_is_killed_and_nothing_past_it_is_written() {
    let r = export("flood", "hostile-flood", b"", b(60_000, 256, 1), None, &AtomicU64::new(0));
    let e = err(&r);
    // The host's count or the kit's own sink, whichever saw it first.
    match e {
        RunError::Limit("output bytes") => {}
        RunError::Kit(v) => assert_eq!((v["id"].as_str(), &v["details"]["limit"]), (Some("MPC-E003"), &serde_json::json!("output bytes"))),
        other => panic!("{other:?}"),
    }
    assert!(r.out.len() as u64 <= 1 << 20, "{} bytes written", r.out.len());
}

#[test]
fn a_cancel_kills_the_worker() {
    let progress = AtomicU64::new(0);
    let r = export("cancel", "hostile-slow", &[b'x'; 200], b(60_000, 256, 64), Some(Duration::from_millis(700)), &progress);
    assert_eq!(err(&r), &RunError::Cancelled);
    assert!(r.took < Duration::from_secs(10), "took {:?}", r.took);
    assert!(progress.load(Ordering::Acquire) > 0, "no progress frame arrived before the cancel");
}

#[test]
fn a_slow_export_reports_progress_and_finishes() {
    let progress = AtomicU64::new(0);
    let r = export("slow", "hostile-slow", b"abcd", b(60_000, 256, 64), None, &progress);
    let done = r.res.as_ref().expect("finishes");
    assert_eq!(r.out, b"abcd");
    assert_eq!(done.bytes, 4);
    assert!(progress.load(Ordering::Acquire) > 0);
}

#[test]
fn a_panic_is_caught_by_the_registry_as_export_internal() {
    let r = export("panic", "hostile-panic", b"", b(60_000, 256, 64), None, &AtomicU64::new(0));
    let e = err(&r);
    let (id, details) = kit_id(e);
    assert_eq!(id, "MPC-E007");
    assert!(details["exporter"].as_str().is_some_and(|x| x.starts_with("k2-export-hostile@")), "{e:?}");
}

#[test]
fn an_abort_is_export_internal_with_the_signal() {
    let r = export("abort", "hostile-abort", b"", b(60_000, 256, 64), None, &AtomicU64::new(0));
    match err(&r) {
        RunError::Internal { worker, .. } => {
            if cfg!(unix) {
                assert_eq!(worker, &format!("signal {}", libc::SIGABRT));
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_exit_without_a_final_frame_is_a_breach() {
    let r = export("exit", "hostile-exit", b"", b(60_000, 256, 64), None, &AtomicU64::new(0));
    match err(&r) {
        RunError::Internal { worker, message } => {
            assert_eq!(worker, "exit 0");
            assert!(message.contains("without a final frame"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_decode_error_is_export_decode() {
    let r = export("decode", "hostile-decode", b"", b(60_000, 256, 64), None, &AtomicU64::new(0));
    let (id, _) = kit_id(err(&r));
    assert_eq!(id, "MPC-E005");
}

#[test]
fn the_worker_gets_no_environment() {
    std::env::set_var("K2_HOOK_TOKEN", "must-not-leak");
    let r = export("env", "hostile-env", b"", b(60_000, 256, 64), None, &AtomicU64::new(0));
    r.res.as_ref().expect("env export");
    let names = String::from_utf8(r.out).expect("utf8");
    for bad in ["K2_HOOK_TOKEN", "HOME", "PATH", "USER"] {
        assert!(!names.lines().any(|l| l == bad), "{bad} reached the worker: {names:?}");
    }
    assert!(!names.lines().any(|l| l.starts_with("K2")), "a K2 variable reached the worker: {names:?}");
}

#[test]
fn hostile_builds_say_so_in_the_hello() {
    let d = tmp("hello");
    let s = Spawn { bin: PathBuf::from(env!("CARGO_BIN_EXE_k2-export")), cwd: d };
    let (hello, targets) = host::describe(&s, &Kind::Format("bin".into()), b(10_000, 256, 1)).expect("describe");
    assert!(hello.hostile);
    assert!(targets.iter().any(|t| t["id"] == "hostile-sleep"));
}
