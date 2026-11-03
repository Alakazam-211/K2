//! The executor's accept loop with the process really out of descriptors.
//! The child half lowers `RLIMIT_NOFILE`, which is process-wide, so it runs in
//! a process of its own; the parent reads what it logged.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

use std::fs::File;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use k2_node::admin::exec::{ExecPaths, Executor, Platform, SystemPlatform};
use k2_node::admin::server::{self, PeerRule};

const CHILD: &str = "child_serves_while_out_of_descriptors";
/// How long the child keeps the descriptor table full.
const SHORTAGE: Duration = Duration::from_millis(1500);
/// Every try the pauses allow in [`SHORTAGE`] (50 + 100 + … ms), with room for
/// a loaded machine. A loop that doesn't pause makes thousands.
const MOST_TRIES: u64 = 10;

fn cpu_time() -> Duration {
    // SAFETY: rusage is plain integers, for which all zeroes is valid.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer for this process's usage.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) }, 0);
    let t = |v: libc::timeval| Duration::new(v.tv_sec as u64, v.tv_usec as u32 * 1000);
    t(u.ru_utime) + t(u.ru_stime)
}

/// Every descriptor but one taken: the caller's next `socket` fits, then
/// nothing does. The limit comes down so this takes a few opens, not a million.
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
/// `an_executor_out_of_descriptors_pauses_logs_once_and_recovers` gives it.
#[test]
#[ignore = "run by an_executor_out_of_descriptors_pauses_logs_once_and_recovers, in its own process"]
fn child_serves_while_out_of_descriptors() {
    let dir = k2_node::util::temp_dir("accept-fds");
    let paths = ExecPaths { config_dir: dir.join("etc"), state_dir: dir.join("state"), home: dir.join("home"), binary: dir.join("bin/k2-node") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    k2_node::admin::exec::ensure_state_dir(&paths.state_dir).unwrap();
    let exec = Arc::new(Executor::new(paths, Arc::new(SystemPlatform) as Arc<dyn Platform>));
    // A socket path has a short limit, and a temp dir can be long.
    let sock = std::path::PathBuf::from(format!("/tmp/k2na-fds-{}.sock", std::process::id()));
    let l = server::bind(&sock, None).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (e2, s2) = (exec.clone(), stop.clone());
    // SAFETY: geteuid has no failure mode.
    let me = unsafe { libc::geteuid() };
    let srv = std::thread::spawn(move || {
        let rule = PeerRule { allowed_uids: vec![me] };
        server::serve(&e2, &rule, l, &|| s2.load(Ordering::SeqCst)).unwrap();
    });

    let filler = fill_descriptor_table();
    // Queued, and nothing is free to accept it into.
    let mut c = UnixStream::connect(&sock).unwrap();
    assert_eq!(File::open("/dev/null").unwrap_err().raw_os_error(), Some(libc::EMFILE), "the table is full");

    let cpu = cpu_time();
    std::thread::sleep(SHORTAGE);
    let burned = cpu_time() - cpu;
    assert!(burned < Duration::from_millis(500), "out of descriptors the loop spun: {burned:?} of CPU in {SHORTAGE:?}");

    // Descriptors free again: the same listener serves the connection that waited.
    drop(filler);
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    server::write_frame(&mut c, b"not a request").unwrap();
    let answer = server::read_frame(&mut c, 1 << 20).unwrap_or_else(|e| panic!("the waiting connection got no answer: {e}"));
    let text = String::from_utf8_lossy(&answer);
    assert!(text.contains(k2_node_proto::admin::code::MALFORMED), "{text}");
    UnixStream::connect(&sock).expect("the listener stays up");

    stop.store(true, Ordering::SeqCst);
    srv.join().unwrap();
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn an_executor_out_of_descriptors_pauses_logs_once_and_recovers() {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe).args(["--exact", CHILD, "--ignored", "--nocapture", "--test-threads=1"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let sample: String = stderr.lines().take(5).collect::<Vec<_>>().join("\n");
    let accept_lines: Vec<&str> = stderr.lines().filter(|l| l.contains("admin: accept")).collect();
    assert!(
        accept_lines.len() == 2,
        "one line for the shortage and one for the recovery, not {}. First lines:\n{sample}\nstdout:\n{stdout}",
        accept_lines.len(),
    );
    assert!(out.status.success(), "the child failed: {}\nstderr:\n{sample}\nstdout:\n{stdout}", out.status);
    assert!(accept_lines[0].contains("admin: accept error: ") && accept_lines[0].contains("the listener stays up"), "{}", accept_lines[0]);
    let tries: u64 = accept_lines[1]
        .split("accepting again after ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in: {}", accept_lines[1]));
    assert!((2..=MOST_TRIES).contains(&tries), "tries in {SHORTAGE:?}: {tries}");
}
