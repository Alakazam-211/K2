//! The engine's accept loop with the process really out of descriptors. The
//! child half lowers `RLIMIT_NOFILE`, which is process-wide, so it runs in a
//! process of its own; the parent reads what it logged.
//!
//! Linux only, as the engine is, and for a Linux reason: `accept` takes its
//! descriptor before it waits. A loop already blocked in `accept` holds one and
//! never sees the shortage, so the table fills before the engine's first
//! `accept`, which then fails at once with or without a connection waiting.
#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::{Duration, Instant};

use k2_app_engine::server::{serve, Engine};

const CHILD: &str = "child_serves_while_out_of_descriptors";
/// How long the child keeps the descriptor table full.
const SHORTAGE: Duration = Duration::from_millis(1500);
/// Every try the pauses allow in [`SHORTAGE`] (50 + 100 + … ms), with room for
/// a loaded machine. A loop that doesn't pause makes millions.
const MOST_TRIES: u64 = 10;

fn cpu_time() -> Duration {
    // SAFETY: rusage is plain integers, for which all zeroes is valid.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer for this process's usage.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) }, 0);
    let t = |v: libc::timeval| Duration::new(v.tv_sec as u64, v.tv_usec as u32 * 1000);
    t(u.ru_utime) + t(u.ru_stime)
}

/// Every descriptor but one taken: the engine's listener fits, then nothing
/// does. The limit comes down so this takes a few opens, not a million.
fn fill_descriptor_table() -> Vec<File> {
    let lowest_free = {
        use std::os::fd::AsRawFd;
        File::open("/dev/null").unwrap().as_raw_fd() as u64
    };
    let lim = libc::rlimit { rlim_cur: lowest_free + 16, rlim_max: lowest_free + 16 };
    // SAFETY: a valid rlimit; this process exists only for this test.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) }, 0);
    let mut filler = Vec::new();
    loop {
        match File::open("/dev/null") {
            Ok(f) => filler.push(f),
            Err(e) => {
                assert_eq!(e.raw_os_error(), Some(libc::EMFILE), "{e}");
                break;
            }
        }
    }
    filler.pop();
    filler
}

/// The child half. Ignored in a normal run: it needs its own process, which
/// `the_engine_out_of_descriptors_pauses_logs_once_and_recovers` gives it.
/// `serve` runs forever, so its thread ends with this process.
#[test]
#[ignore = "run by the_engine_out_of_descriptors_pauses_logs_once_and_recovers, in its own process"]
fn child_serves_while_out_of_descriptors() {
    let d = tempfile::tempdir().unwrap();
    let sock = d.path().join("engine.sock");
    let (e, s2) = (Engine::new(d.path().join("home"), "/tmp".into()), sock.clone());

    let filler = fill_descriptor_table();
    std::thread::spawn(move || serve(e, &s2).unwrap());
    // Bound means the listener took the last descriptor.
    let started = Instant::now();
    while !sock.exists() {
        assert!(started.elapsed() < Duration::from_secs(20), "the engine never bound");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(File::open("/dev/null").unwrap_err().raw_os_error(), Some(libc::EMFILE), "the table is full");

    let cpu = cpu_time();
    std::thread::sleep(SHORTAGE);
    let burned = cpu_time() - cpu;
    assert!(burned < Duration::from_millis(500), "out of descriptors the loop spun: {burned:?} of CPU in {SHORTAGE:?}");

    // Descriptors free again: the same listener answers a connection (this
    // test isn't the daemon, so it answers caller_not_daemon).
    drop(filler);
    let mut c = UnixStream::connect(&sock).unwrap_or_else(|e| panic!("the listener went away: {e}"));
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap_or_else(|e| panic!("the connection got no answer: {e}"));
    assert!(answer.contains("caller_not_daemon"), "{answer}");
}

#[test]
fn the_engine_out_of_descriptors_pauses_logs_once_and_recovers() {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe).args(["--exact", CHILD, "--ignored", "--nocapture", "--test-threads=1"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the child failed: {}\nstderr:\n{stderr}\nstdout:\n{stdout}", out.status);
    let lines: Vec<&str> = stderr.lines().filter(|l| l.contains("k2-app-engine: accept")).collect();
    assert_eq!(lines.len(), 2, "one line for the shortage and one for the recovery:\n{stderr}");
    assert!(lines[0].contains("accept error: ") && lines[0].contains("the listener stays up"), "{}", lines[0]);
    let tries: u64 = lines[1]
        .split("accepting again after ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in: {}", lines[1]));
    assert!((2..=MOST_TRIES).contains(&tries), "tries in {SHORTAGE:?}: {tries}");
}
