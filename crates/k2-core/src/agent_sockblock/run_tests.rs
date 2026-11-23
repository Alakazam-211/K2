//! The block on this machine's kernel, in-process: the test binary runs
//! itself as the exec helper (`role_helper`: prepares, forks, supervises)
//! and as the session's program (`role_client`: tries each row and prints
//! `R <row> <result>`). No Landlock here (the helper's fence is the kernel
//! rows' job); everything else is the real filter and supervisor.
//!
//! Needs a kernel with seccomp user notification and `pidfd_getfd` (5.19+)
//! and a test process allowed to use them (a container's seccomp profile
//! may not be): the preflight says which, loudly.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::supervisor::{self, HookRule, Log, Prepared, Rules, TmpRule};
use super::{sys, Mode};

const ROLE: &str = "K2_SB_ROLE";

fn role_is(r: &str) -> bool {
    std::env::var(ROLE).as_deref() == Ok(r)
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} is set by the outer test"))
}

/// This test binary running one of the role tests below.
fn me(test: &str) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("the test binary"));
    c.args([test, "--exact", "--nocapture", "--test-threads=1", "--ignored"]);
    c
}

// ── the outer side ───────────────────────────────────────────────────

struct World {
    root: PathBuf,
    tcp: u16,
    abs: String,
}

fn serve_unix(l: UnixListener, reply: &'static [u8]) {
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let _ = s.write_all(reply);
        }
    });
}

fn world(tag: &str) -> World {
    let root = crate::test_env::short_tmp(format!("k2sbr{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["t", "cells", "out"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let root = std::fs::canonicalize(&root).unwrap();
    serve_unix(UnixListener::bind(root.join("cells/hook.sock")).unwrap(), b"hi");
    serve_unix(UnixListener::bind(root.join("out/other.sock")).unwrap(), b"other");
    std::os::unix::fs::symlink(root.join("out/other.sock"), root.join("t/link")).unwrap();
    let abs = format!("k2sb-abs-{tag}-{}", std::process::id());
    {
        use std::os::linux::net::SocketAddrExt;
        let a = std::os::unix::net::SocketAddr::from_abstract_name(abs.as_bytes()).unwrap();
        serve_unix(UnixListener::bind_addr(&a).unwrap(), b"abs");
    }
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = tcp.local_addr().unwrap().port();
    std::thread::spawn(move || for _s in tcp.incoming() {});
    World { root, tcp: port, abs }
}

/// Run `client` (a role test's name) under the supervisor in `mode`, or
/// directly when `mode` is `None`. Its status and `R` lines.
fn run(w: &World, mode: Option<Mode>, client: &str) -> (i32, HashMap<String, String>, String) {
    let mut c = match mode {
        Some(m) => {
            let mut c = me("agent_sockblock::run_tests::role_helper");
            c.env(ROLE, "helper").env("K2_SB_MODE", m.as_str()).env("K2_SB_CLIENT", client);
            c
        }
        None => {
            let mut c = me(client);
            c.env(ROLE, "client").env("TMPDIR", w.root.join("t"));
            c
        }
    };
    c.env("K2_SB_ROOT", &w.root).env("K2_SB_TCP", w.tcp.to_string()).env("K2_SB_ABS", &w.abs).env("K2_SB_MODE_SEEN", mode.map_or("none", Mode::as_str));
    let out = c.stdin(Stdio::null()).stderr(Stdio::inherit()).output().expect("run the role");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("──── {client} ({}) ────\n{text}", mode.map_or("no block", Mode::as_str));
    let rows = text.lines().filter_map(|l| l.strip_prefix("R ")).filter_map(|l| l.split_once(' ')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    (out.status.code().unwrap_or(-1), rows, text)
}

fn preflight() {
    if let Err(why) = super::support() {
        panic!("this kernel can't run the block: {why}");
    }
    // A child that installs a listener filter: a container profile may refuse.
    let prog = super::filter::program(super::filter::Arch::native().unwrap(), Mode::Enforce);
    // SAFETY: the forked child only makes raw calls and _exits.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: plain prctl.
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        let rc = match sys::seccomp_install(&prog, super::filter::FILTER_FLAG_NEW_LISTENER | super::filter::FILTER_FLAG_WAIT_KILLABLE_RECV) {
            Ok(_) => 0,
            Err(e) => e.raw_os_error().unwrap_or(255),
        };
        // SAFETY: never returns.
        unsafe { libc::_exit(rc) };
    }
    let mut st = 0;
    // SAFETY: our child.
    unsafe { libc::waitpid(pid, &mut st, 0) };
    let rc = libc::WEXITSTATUS(st);
    assert_eq!(rc, 0, "this test process can't install a seccomp listener filter here (errno {rc}): {}", std::io::Error::from_raw_os_error(rc));
}

const REFUSED: &str = "err 111";

/// Enforce: every row of the review's section 7 that a process can check
/// for itself, through the real filter and supervisor.
#[test]
fn a_session_under_the_block_enforced() {
    preflight();
    let w = world("e");
    let (status, r, text) = run(&w, Some(Mode::Enforce), "agent_sockblock::run_tests::role_client");
    let get = |k: &str| r.get(k).map(String::as_str).unwrap_or_else(|| panic!("row {k} missing:\n{text}"));
    assert_eq!(get("fds"), "none", "the program holds no descriptor above 2 (row 9)");
    assert_eq!(get("hook"), "ok hi", "its own hook socket");
    assert_eq!(get("tmp"), "ok", "a socket it made in its own temp folder");
    // Row 4: both allowed connects above went through the supervisor's
    // `/proc/self/fd/<n>` route. The same name from the session itself is
    // not under its temp folder: refused.
    assert_eq!(get("tmp_proc_fd_by_name"), REFUSED, "a /proc/self/fd name from the session");
    for row in ["other", "link", "dotdot", "abstract", "relative"] {
        assert_eq!(get(row), REFUSED, "{row}: refused (row 3)");
    }
    assert_eq!(get("tcp"), "ok", "a blocking TCP connect (the pool)");
    assert_eq!(get("tcp_nonblock"), "ok", "a non-blocking TCP connect (EINPROGRESS passed back)");
    assert_eq!(get("dgram"), "err 1", "Unix datagram sockets can't be made (row 5)");
    assert_eq!(get("dgram_pair"), "err 1", "nor a datagram socketpair");
    assert_eq!(get("stream_pair"), "ok", "a stream socketpair is fine");
    assert_eq!(get("uring"), "err 38", "io_uring_setup: ENOSYS (row 2)");
    assert_eq!(get("listener"), "err 1", "its own notification listener: refused (row 1)");
    assert_eq!(get("plain_filter"), "ok", "a filter without a listener is still allowed");
    assert_eq!(get("doublefork"), "0", "a double-forked child still connects (row 7)");
    assert_eq!(get("nondumpable"), "1", "a non-dumpable caller's connect is refused (EPERM: it can't be read)");
    assert_eq!(get("ptrace"), "err 1", "the supervisor can't be traced (row 8)");
    assert_eq!(get("vm_read"), "err 1", "nor its memory read");
    assert!(get("environ").starts_with("err"), "nor its environment: {}", get("environ"));
    assert_eq!(get("signals"), "0", "connects with signals arriving: no 'already connected' (row 6)");
    assert_eq!(get("prlimit"), "ok", "the supervisor's open-file limit set to zero");
    assert_eq!(get("after_prlimit_hook"), "err 1", "then even the hook socket is refused, nothing opens (row 8)");
    assert_eq!(get("after_prlimit_tcp"), "err 1", "and TCP");
    assert_eq!(status, 0, "the helper exits with the program's status");
    let log = std::fs::read_to_string(w.root.join("log")).unwrap_or_default();
    println!("──── log ────\n{log}");
    assert!(log.contains("\"path\":\"") && log.contains("other.sock") && log.contains("\"applied\":\"refused\""), "refusals are logged: {log}");
    assert!(!log.contains("\"verdict\":\"allow\""), "enforce logs refusals only");
    let _ = std::fs::remove_dir_all(&w.root);
}

/// Record: the same session decides as enforce would, logs it, and lets
/// it through.
#[test]
fn a_session_under_the_block_recorded() {
    preflight();
    let w = world("r");
    let (status, r, text) = run(&w, Some(Mode::Record), "agent_sockblock::run_tests::role_client");
    let get = |k: &str| r.get(k).map(String::as_str).unwrap_or_else(|| panic!("row {k} missing:\n{text}"));
    assert_eq!(get("hook"), "ok hi");
    assert_eq!(get("other"), "ok other", "record lets it through");
    assert_eq!(get("link"), "ok other");
    assert_eq!(get("abstract"), "ok abs");
    assert_eq!(get("dgram"), "ok");
    assert_eq!(get("listener"), "ok", "record lets its own listener through (in a child that exits)");
    assert_eq!(get("tcp"), "ok");
    assert_eq!(status, 0);
    let log = std::fs::read_to_string(w.root.join("log")).unwrap();
    println!("──── log ────\n{log}");
    let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}"))).collect();
    assert_eq!(lines[0]["event"], "start");
    let find = |path_part: &str| lines.iter().find(|l| l["path"].as_str().is_some_and(|p| p.contains(path_part))).unwrap_or_else(|| panic!("no line for {path_part}"));
    let other = find("out/other.sock");
    assert_eq!((other["verdict"].as_str(), other["applied"].as_str(), other["mode"].as_str()), (Some("refuse"), Some("allowed"), Some("record")));
    assert!(other["pid"].as_u64().is_some() && other["t"].as_str().is_some() && other["reason"].as_str().is_some());
    assert_eq!(find("hook.sock")["verdict"], "allow");
    assert_eq!(find("@k2sb-abs")["verdict"], "refuse");
    assert!(lines.iter().any(|l| l["call"] == "socket" && l["verdict"] == "refuse"), "the datagram socket");
    assert!(lines.iter().any(|l| l["call"] == "io_uring_setup"), "io_uring");
    assert!(lines.iter().any(|l| l["family"] == "inet" && l["verdict"] == "allow"), "TCP");
    let _ = std::fs::remove_dir_all(&w.root);
}

// ── the roles (run only as the outer tests' children) ────────────────

#[test]
#[ignore = "a role for the in-process block tests; does nothing on its own"]
fn role_helper() {
    if !role_is("helper") {
        return;
    }
    let root = PathBuf::from(env("K2_SB_ROOT"));
    let mode = if env("K2_SB_MODE") == "enforce" { Mode::Enforce } else { Mode::Record };
    let t = root.join("t");
    let c = std::ffi::CString::new(t.to_str().unwrap()).unwrap();
    let dir = sys::openat2(libc::AT_FDCWD, &c, libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC, sys::RESOLVE_NO_SYMLINKS).unwrap();
    let dev = sys::fstat(dir.as_raw_fd()).unwrap().st_dev;
    // SAFETY: no preconditions.
    let uid = unsafe { libc::geteuid() };
    let prep = Prepared {
        mode,
        rules: Rules {
            hook: Some(HookRule { path: root.join("cells/hook.sock").to_str().unwrap().as_bytes().to_vec(), uid }),
            tmp: Some(TmpRule { prefix: t.to_str().unwrap().as_bytes().to_vec(), dir, dev }),
        },
        log: Log::open(&root.join("log")).unwrap(),
    };
    let mut cmd = me(&env("K2_SB_CLIENT"));
    cmd.env(ROLE, "client").env("TMPDIR", &t).env_remove("K2_SB_CLIENT");
    std::process::exit(supervisor::run(prep, cmd));
}

fn res<T>(r: std::io::Result<T>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => format!("err {}", e.raw_os_error().unwrap_or(-1)),
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

fn talk(p: &Path) -> String {
    match UnixStream::connect(p) {
        Ok(mut s) => {
            let mut b = String::new();
            let _ = s.read_to_string(&mut b);
            format!("ok {b}")
        }
        Err(e) => format!("err {}", e.raw_os_error().unwrap_or(-1)),
    }
}

/// `connect()` with raw calls only (safe after fork): 0 or the errno.
fn raw_connect(addr: &[u8]) -> i32 {
    // SAFETY: raw socket calls on a new descriptor; addr is a valid buffer.
    unsafe {
        let s = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if s < 0 {
            return errno();
        }
        let r = libc::connect(s, addr.as_ptr().cast(), addr.len() as libc::socklen_t);
        let e = if r == 0 { 0 } else { errno() };
        libc::close(s);
        e
    }
}

/// Run `f` in a forked child; its exit code (raw calls only in `f`).
fn in_child(f: impl FnOnce() -> i32) -> i32 {
    // SAFETY: the child runs only raw calls, then _exits.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        let rc = f();
        // SAFETY: never returns.
        unsafe { libc::_exit(rc) };
    }
    let mut st = 0;
    // SAFETY: our child.
    unsafe { libc::waitpid(pid, &mut st, 0) };
    libc::WEXITSTATUS(st)
}

fn out(k: &str, v: impl std::fmt::Display) {
    println!("R {k} {v}");
}

extern "C" fn on_usr1(_: libc::c_int) {}

#[test]
#[ignore = "a role for the in-process block tests; does nothing on its own"]
fn role_client() {
    if !role_is("client") {
        return;
    }
    let root = PathBuf::from(env("K2_SB_ROOT"));
    let enforced = env("K2_SB_MODE_SEEN") == "enforce";
    // Row 9: nothing above 2 came with the program.
    // SAFETY: F_GETFD only asks.
    let open: Vec<i32> = (3..1024).filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } != -1).collect();
    out("fds", if open.is_empty() { "none".to_string() } else { format!("{open:?}") });
    let hook = root.join("cells/hook.sock");
    out("hook", talk(&hook));
    let tmp = PathBuf::from(env("TMPDIR"));
    let own = tmp.join("own.sock");
    let _ = std::fs::remove_file(&own);
    let l = UnixListener::bind(&own).unwrap();
    std::thread::spawn(move || for _s in l.incoming() {});
    out("tmp", res(UnixStream::connect(&own)));
    // The supervisor connects through /proc/self/fd/<n>; the same name
    // from here, on a handle to its own socket, is under /proc, not its
    // temp folder.
    let c = std::ffi::CString::new(own.to_str().unwrap()).unwrap();
    // SAFETY: O_PATH open of a NUL-terminated path.
    let h = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    let via = format!("/proc/self/fd/{h}");
    let r = res(UnixStream::connect(&via));
    out("tmp_proc_fd_by_name", &r);
    out("other", talk(&root.join("out/other.sock")));
    out("link", talk(&tmp.join("link")));
    out("dotdot", talk(&tmp.join("../out/other.sock")));
    {
        use std::os::linux::net::SocketAddrExt;
        let a = std::os::unix::net::SocketAddr::from_abstract_name(env("K2_SB_ABS").as_bytes()).unwrap();
        out("abstract", match UnixStream::connect_addr(&a) {
            Ok(mut s) => {
                let mut b = String::new();
                let _ = s.read_to_string(&mut b);
                format!("ok {b}")
            }
            Err(e) => format!("err {}", e.raw_os_error().unwrap_or(-1)),
        });
    }
    std::env::set_current_dir(root.join("out")).unwrap();
    out("relative", talk(Path::new("other.sock")));
    let tcp: u16 = env("K2_SB_TCP").parse().unwrap();
    out("tcp", res(std::net::TcpStream::connect(("127.0.0.1", tcp))));
    out("tcp_nonblock", nonblocking_tcp(tcp));
    // SAFETY: raw socket calls; descriptors closed.
    unsafe {
        let s = libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        out("dgram", if s >= 0 { libc::close(s); "ok".to_string() } else { format!("err {}", errno()) });
        let mut sv = [0; 2];
        let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, sv.as_mut_ptr());
        out("dgram_pair", if r == 0 { "ok".to_string() } else { format!("err {}", errno()) });
        let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr());
        out("stream_pair", if r == 0 { "ok".to_string() } else { format!("err {}", errno()) });
        let mut params = [0u8; 120];
        let r = libc::syscall(libc::SYS_io_uring_setup, 1u32, params.as_mut_ptr());
        out("uring", if r >= 0 { libc::close(r as i32); "ok".to_string() } else { format!("err {}", errno()) });
    }
    // Row 1: its own listener filter, in a child that exits right after.
    let allow = [super::filter::SockFilter { code: super::filter::RET_K, jt: 0, jf: 0, k: super::filter::RET_ALLOW }];
    let rc = in_child(|| match sys::seccomp_install(&allow, super::filter::FILTER_FLAG_NEW_LISTENER) {
        Ok(_) => 0,
        Err(e) => e.raw_os_error().unwrap_or(255),
    });
    out("listener", if rc == 0 { "ok".to_string() } else { format!("err {rc}") });
    let rc = in_child(|| match sys::seccomp_install(&allow, 0) {
        Ok(_) => 0,
        Err(e) => e.raw_os_error().unwrap_or(255),
    });
    out("plain_filter", if rc == 0 { "ok".to_string() } else { format!("err {rc}") });
    // Row 7: a double-forked child (reparented to the supervisor) connects.
    let hook_addr = sys::unix_addr(hook.to_str().unwrap().as_bytes());
    let mut p = [0; 2];
    // SAFETY: a two-element out-array.
    unsafe { libc::pipe(p.as_mut_ptr()) };
    let rc = in_child(|| {
        // SAFETY: raw calls only; the grandchild writes its result and _exits.
        unsafe {
            if libc::fork() == 0 {
                libc::usleep(100_000);
                let e = raw_connect(&hook_addr) as u8;
                libc::write(p[1], (&e as *const u8).cast(), 1);
                libc::_exit(0);
            }
        }
        0
    });
    let mut b = [255u8; 1];
    // SAFETY: reads one byte into b.
    unsafe {
        libc::close(p[1]);
        libc::read(p[0], b.as_mut_ptr().cast(), 1);
        libc::close(p[0]);
    }
    out("doublefork", if rc == 0 { b[0].to_string() } else { format!("child {rc}") });
    // A caller that made itself non-dumpable can't be read: refused.
    let rc = in_child(|| {
        // SAFETY: plain prctl.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
        raw_connect(&hook_addr)
    });
    out("nondumpable", rc);
    // Row 8: the supervisor (this program's parent) is non-dumpable.
    // SAFETY: getppid has no failure mode.
    let sup = unsafe { libc::getppid() };
    // SAFETY: ptrace attach to a pid; detached right away on success.
    let r = unsafe { libc::ptrace(libc::PTRACE_ATTACH, sup, 0, 0) };
    out("ptrace", if r == 0 { unsafe { libc::ptrace(libc::PTRACE_DETACH, sup, 0, 0) }; "ok".to_string() } else { format!("err {}", errno()) });
    out("vm_read", res(sys::read_mem(sup, 0x400000, 8)));
    out("environ", res(std::fs::read(format!("/proc/{sup}/environ"))));
    // Row 6: connects while signals keep arriving (handled, so they
    // interrupt): with the wait-killable flag none fails "already connected".
    // SAFETY: a handler that does nothing.
    unsafe { libc::signal(libc::SIGUSR1, on_usr1 as *const () as libc::sighandler_t) };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    // SAFETY: getpid has no failure mode.
    let me_pid = unsafe { libc::getpid() };
    let pinger = std::thread::spawn(move || {
        while !s2.load(std::sync::atomic::Ordering::Relaxed) {
            // SAFETY: a signal to this process, which handles it.
            unsafe { libc::kill(me_pid, libc::SIGUSR1) };
            std::thread::sleep(Duration::from_micros(50));
        }
    });
    let mut bad = 0;
    for _ in 0..2000 {
        match std::net::TcpStream::connect(("127.0.0.1", tcp)) {
            Ok(s) => linger0(&s),
            Err(e) => {
                bad += 1;
                eprintln!("connect with signals: {e}");
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = pinger.join();
    out("signals", bad);
    if enforced {
        // Row 8: the supervisor's open-file limit set to zero (allowed for
        // the same user): from then on every connect is refused.
        let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: a valid limit for another process of this user.
        let r = unsafe { libc::prlimit(sup, libc::RLIMIT_NOFILE, &zero, std::ptr::null_mut()) };
        out("prlimit", if r == 0 { "ok".to_string() } else { format!("err {}", errno()) });
        out("after_prlimit_hook", res(UnixStream::connect(&hook)));
        out("after_prlimit_tcp", res(std::net::TcpStream::connect(("127.0.0.1", tcp))));
    }
    out("done", 1);
}

fn linger0(s: &std::net::TcpStream) {
    let l = libc::linger { l_onoff: 1, l_linger: 0 };
    // SAFETY: a sized option on our socket.
    unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_LINGER, (&l as *const libc::linger).cast(), std::mem::size_of::<libc::linger>() as libc::socklen_t) };
}

fn nonblocking_tcp(port: u16) -> String {
    // SAFETY: raw socket calls on a new descriptor, closed at the end.
    unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0);
        let mut a: libc::sockaddr_in = std::mem::zeroed();
        a.sin_family = libc::AF_INET as libc::sa_family_t;
        a.sin_port = port.to_be();
        a.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        let r = libc::connect(s, (&a as *const libc::sockaddr_in).cast(), std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t);
        let e = if r == 0 { 0 } else { errno() };
        let res = if r == 0 || e == libc::EINPROGRESS {
            let mut p = libc::pollfd { fd: s, events: libc::POLLOUT, revents: 0 };
            libc::poll(&mut p, 1, 5000);
            let mut err: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            libc::getsockopt(s, libc::SOL_SOCKET, libc::SO_ERROR, (&mut err as *mut libc::c_int).cast(), &mut len);
            if err == 0 { "ok".to_string() } else { format!("err {err}") }
        } else {
            format!("err {e}")
        };
        libc::close(s);
        res
    }
}

// ── the cost bench (review section 4) ────────────────────────────────

fn pct(v: &mut [u64], p: f64) -> u64 {
    v.sort_unstable();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn timed(n: usize, threads: usize, f: impl Fn() + Sync) -> Vec<u64> {
    let per = n / threads;
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    (0..per)
                        .map(|_| {
                            let t = Instant::now();
                            f();
                            t.elapsed().as_nanos() as u64
                        })
                        .collect::<Vec<u64>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

#[test]
#[ignore = "a role for the cost bench; does nothing on its own"]
fn role_bench() {
    if !role_is("client") {
        return;
    }
    let root = PathBuf::from(env("K2_SB_ROOT"));
    let tcp: u16 = env("K2_SB_TCP").parse().unwrap();
    let hook = root.join("cells/hook.sock");
    for threads in [1usize, 8] {
        let mut v = timed(10_000, threads, || {
            let s = std::net::TcpStream::connect(("127.0.0.1", tcp)).expect("tcp connect");
            linger0(&s);
        });
        let (m, p, x) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 1.0));
        println!("R tcp{threads} {m} {p} {x}");
        let mut v = timed(10_000, threads, || {
            UnixStream::connect(&hook).expect("hook connect");
        });
        let (m, p, x) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 1.0));
        println!("R hook{threads} {m} {p} {x}");
    }
    let mut f = std::fs::OpenOptions::new().write(true).open("/dev/null").unwrap();
    let t = Instant::now();
    for _ in 0..1_000_000 {
        f.write_all(b"x").unwrap();
    }
    println!("R write_loop {} 0 0", t.elapsed().as_nanos() / 1_000_000);
}

/// Section 4's rows: 10k local TCP connects and 10k hook-socket connects,
/// one connector and 8 in parallel, with and without the block (enforce);
/// a tight write loop. Prints the median and p99 added; the bar is 50 µs
/// at the median and no stall over 1 ms at p99.
#[test]
#[ignore = "the cost bench: cargo nextest run -p k2-core --run-ignored only -E 'test(cost_bench)' --no-capture"]
fn cost_bench() {
    preflight();
    let w = world("b");
    // The hook stand-in answers nothing here: connects only.
    let (s0, base, t0) = run(&w, None, "agent_sockblock::run_tests::role_bench");
    let (s1, with, t1) = run(&w, Some(Mode::Enforce), "agent_sockblock::run_tests::role_bench");
    assert!(s0 == 0 && s1 == 0, "bench runs failed:\n{t0}\n{t1}");
    println!("{:<12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12}", "row", "median off", "median on", "added", "p99 off", "p99 on", "max on");
    for row in ["tcp1", "tcp8", "hook1", "hook8", "write_loop"] {
        let n = |m: &HashMap<String, String>| -> Vec<i64> { m[row].split_whitespace().map(|x| x.parse().unwrap()).collect() };
        let (a, b) = (n(&base), n(&with));
        let added = b[0] - a[0];
        let bar = if row == "write_loop" { String::new() } else { format!("  bar: added {} 50µs, p99 {} 1ms", if added <= 50_000 { "<=" } else { "OVER" }, if b[1] <= 1_000_000 { "<=" } else { "OVER" }) };
        println!("{row:<12} {:>10}ns {:>10}ns {:>10}ns {:>10}ns {:>10}ns {:>10}ns{bar}", a[0], b[0], added, a[1], b[1], b[2]);
    }
    let _ = std::fs::remove_dir_all(&w.root);
}
