//! The ONE lock for process-environment mutation in tests (quiet-gate
//! PRD §5.3, 0.45.1).
//!
//! `$HOME`, `$PATH`, `$SHELL` and the `K2_*` / `K2SO_*` knobs are process
//! globals. Cargo runs a test binary's tests on many threads, so two tests
//! that change the same variable under DIFFERENT locks (or none) stomp each
//! other: one test's store lands under another test's (then deleted) temp
//! home. Before 0.45.1 k2-core had four separate HOME/PATH locks and
//! k2-daemon three more, which is exactly how the `skin::`, `sql::`,
//! `federation_routes::` and `subscription_usage::` flakes happened.
//!
//! Rules (also in `docs/testing.md`):
//!
//! - Every test that sets or removes an env var takes [`lock`] first, or
//!   uses a guard here that takes it ([`EnvVar`], [`TempHome`]).
//! - Every test that READS `$HOME` (directly, or through `dirs::home_dir()`
//!   / a store under `~/.k2`) and needs it stable holds [`lock`] (a
//!   [`TempHome`] does) for the whole time it depends on that value.
//! - Never call `std::env::set_var` / `remove_var` for `HOME`, `PATH` or
//!   `SHELL` outside this module: a source-walk ratchet test fails on it.
//! - k2-daemon re-uses this exact lock (through `test-util`), so both
//!   crates' code in one test binary serialize on the same mutex.
//!
//! The lock is RE-ENTRANT on the same thread, so a test holding it may
//! call a helper that takes it again. It does not poison: a panicking
//! test releases it and the next test runs normally. Guards restore the
//! previous value on drop (LIFO), including on panic.
//!
//! Async tests: the guard is `!Send`, which is fine for `#[tokio::test]`
//! (the test body runs on the test thread). Code inside spawned tasks that
//! reads `$HOME` sees the test's value only while the guard is alive.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

static ENV_LOCK: ReentrantMutex<()> = parking_lot::const_reentrant_mutex(());

/// Guard for the crate-wide test env lock.
pub type EnvLock = ReentrantMutexGuard<'static, ()>;

/// Take the crate-wide (and, through `test-util`, cross-crate) env lock.
pub fn lock() -> EnvLock {
    ENV_LOCK.lock()
}

/// A module test lock taken AFTER the env lock: `_inner` is the module's
/// guard, `_env` the env lock (fields drop in order: module first).
///
/// Use [`serial`] for every test-only module lock (a singleton DB row, a
/// job map, a mock slot) whose tests may also change env. Because the env
/// lock is always taken first, "module lock, then `TempHome`" in one test
/// and "`EnvVar`, then module lock" in another can never deadlock (the env
/// lock is re-entrant on the holder's thread).
#[must_use = "bind the guard for the test's duration"]
pub struct EnvSerial<G> {
    _inner: G,
    _env: EnvLock,
}

/// [`EnvSerial`] over a `std` mutex guard.
pub type SerialGuard = EnvSerial<std::sync::MutexGuard<'static, ()>>;

/// Take the env lock, then `module_lock` (poison recovered: a panicking
/// test must not fail the next one).
pub fn serial(module_lock: &'static std::sync::Mutex<()>) -> SerialGuard {
    let env = lock();
    let inner = module_lock.lock().unwrap_or_else(|p| p.into_inner());
    EnvSerial { _inner: inner, _env: env }
}

/// Take the env lock, then build the module guard(s) with `f` (for a module
/// lock that comes with its own `TempHome`, a tuple, …).
pub fn serial_with<G>(f: impl FnOnce() -> G) -> EnvSerial<G> {
    let env = lock();
    EnvSerial { _inner: f(), _env: env }
}

/// A zero-sized handle whose `.lock()` takes [`lock`]. Old per-module lock
/// statics (`themes::HOME_LOCK`, …) are declared as this type, so any code
/// still written against them (including branches merged later) serializes
/// on the ONE env lock instead of a private one.
pub struct SharedEnvLock;

impl SharedEnvLock {
    /// Same as [`lock`].
    pub fn lock(&self) -> EnvLock {
        lock()
    }
}

/// Set or remove one env var for the life of the guard, then restore the
/// previous value. Holds [`lock`] the whole time.
#[must_use = "the variable is restored when the guard drops"]
pub struct EnvVar {
    key: OsString,
    prev: Option<OsString>,
    _lock: EnvLock,
}

impl EnvVar {
    /// Set `key` to `value` until the guard drops.
    pub fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::set_var(&key, value);
        Self { key, prev, _lock: lock }
    }

    /// Remove `key` until the guard drops.
    pub fn remove(key: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::remove_var(&key);
        Self { key, prev, _lock: lock }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

/// A fresh, unique temp dir path (not created). Uses a uuid, never the pid,
/// a `ThreadId` or a timestamp: those repeat across the several test
/// binaries of one `cargo test` run and across runs.
pub fn unique_temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("k2-{tag}-{}", uuid::Uuid::new_v4().simple()))
}

/// RAII temp `$HOME`: holds [`lock`], points `$HOME` at a fresh temp dir
/// (with `.k2/` pre-created so writers that stage a tmp file and rename it
/// into `~/.k2/…` find the parent), checks the test cannot reach the
/// production daemon, and on drop restores the previous `$HOME` and removes
/// the dir.
#[must_use = "HOME is restored when the guard drops"]
pub struct TempHome {
    prev: Option<OsString>,
    home: PathBuf,
    _lock: EnvLock,
}

impl TempHome {
    /// A temp home under the system temp dir.
    pub fn new() -> Self {
        Self::at(unique_temp_path("test-home"))
    }

    /// A temp home with a SHORT path (`/tmp/k2t-<8 hex>`), for tests that
    /// bind Unix sockets under `$HOME`: a socket path must fit `SUN_LEN`
    /// (~104 bytes on macOS, 108 on Linux), and the macOS per-user temp dir
    /// alone is ~50 bytes.
    #[cfg(unix)]
    pub fn short() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self::at(PathBuf::from(format!("/tmp/k2t-{}", &id[..8])))
    }

    fn at(home: PathBuf) -> Self {
        let lock = lock();
        std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        crate::test_isolation::assert_isolated_from_prod();
        Self { prev, home, _lock: lock }
    }

    /// The temp `$HOME`.
    pub fn path(&self) -> &Path {
        &self.home
    }
}

impl Default for TempHome {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Write an executable script (mode 0755) that a test will exec.
///
/// Never `fs::write` + `chmod` + exec from a multi-threaded test binary on
/// Linux: while this thread holds the file open for writing, another test
/// thread may `fork`, and the child keeps a copy of that write fd until it
/// `exec`s. An `exec` of the script inside that window fails with ETXTBSY
/// ("Text file busy"), which surfaced as a 3/200 flake. The bytes are
/// written by a short-lived `/bin/sh` child instead, so the write fd only
/// ever exists in that child, never in this process.
pub fn write_executable(path: &Path, contents: &str) {
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }
    #[cfg(unix)]
    write_executable_unix(path, contents);
}

#[cfg(unix)]
fn write_executable_unix(path: &Path, contents: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn /bin/sh to write {}: {e}", path.display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(contents.as_bytes())
        .unwrap_or_else(|e| panic!("pipe script for {}: {e}", path.display()));
    let status = child.wait().expect("wait for script writer");
    assert!(status.success(), "writing {} failed: {status}", path.display());
}

/// RAII agent-CLI shims: a temp dir holding a `#!/bin/sh\nexec cat`
/// executable for every [`crate::terminal::agent_spawn_guard::AGENT_CLIS`]
/// name, registered as `K2_TEST_AGENT_SHIM_DIR` (prepended to any existing
/// list) while the guard lives. Holds [`lock`]. Any test that makes the
/// daemon spawn an agent (an explicit `claude`, or a workspace default
/// agent) needs one: test builds refuse real agent CLIs.
#[cfg(unix)]
#[must_use = "the shims are unregistered when the guard drops"]
pub struct AgentShim {
    dir: PathBuf,
    _var: EnvVar,
}

#[cfg(unix)]
impl AgentShim {
    /// Install the default `exec cat` shims.
    pub fn install() -> Self {
        Self::with_body("exec cat")
    }

    /// Install shims whose script body (after `#!/bin/sh`) is `body`.
    pub fn with_body(body: &str) -> Self {
        use crate::terminal::agent_spawn_guard::{AGENT_CLIS, SHIM_DIR_ENV};
        let dir = unique_temp_path("agent-shim");
        std::fs::create_dir_all(&dir).expect("create agent shim dir");
        for name in AGENT_CLIS {
            write_executable(&dir.join(name), &format!("#!/bin/sh\n{body}\n"));
        }
        let list = match std::env::var_os(SHIM_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(prev) => {
                let mut dirs = vec![dir.clone()];
                dirs.extend(std::env::split_paths(&prev));
                std::env::join_paths(dirs).expect("join shim dirs")
            }
            None => dir.clone().into_os_string(),
        };
        let var = EnvVar::set(SHIM_DIR_ENV, list);
        Self { dir, _var: var }
    }

    /// The shim dir (holds one executable per agent CLI name).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(unix)]
impl Drop for AgentShim {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A loopback HTTP server that answers every request with one fixed
/// error status, for tests that need an "unreachable" upstream (control
/// plane, broker, peer). Use it instead of a well-known dead port such as
/// `127.0.0.1:9` or `:1`: on a host whose firewall drops loopback RSTs
/// (leftover K2 sandbox nft tables did, on the build box) a connect to a
/// closed port HANGS instead of being refused. Binds `127.0.0.1:0`; stops
/// on drop.
pub struct ErrorHttpServer {
    addr: std::net::SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ErrorHttpServer {
    /// Answer every request with `status` (e.g. 503) and an empty body.
    pub fn start(status: u16) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_t = stop.clone();
        let response = format!(
            "HTTP/1.1 {status} Test Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_t.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                // Read until the end of the request headers (bodies are
                // ignored; the connection closes after the answer).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { addr, stop, thread: Some(thread) }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The bound address.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }
}

impl Drop for ErrorHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Unblock `accept` so the thread sees the flag.
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ── Source-walk ratchet (quiet-gate PRD §5.3) ────────────────────────

/// Env vars that only this module may set or remove (test code uses
/// [`TempHome`] / [`EnvVar`]). Production call sites are listed per crate.
pub const PROTECTED_VARS: &[&str] = &["HOME", "PATH", "SHELL"];

/// One file's raw env-mutation call counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvMutationCount {
    /// Every `set_var(` / `remove_var(` call.
    pub all: usize,
    /// Calls whose first argument is a literal in [`PROTECTED_VARS`].
    pub protected: usize,
}

/// Count raw `set_var(` / `remove_var(` calls in Rust source text (comment
/// lines and `fn set_var` definitions excluded; a call split over lines
/// still counts).
pub fn count_env_mutations(src: &str) -> EnvMutationCount {
    // Blank out `//` comment lines so prose about set_var never counts.
    let code: String = src
        .lines()
        .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = code.as_bytes();
    let mut out = EnvMutationCount::default();
    for needle in ["set_var(", "remove_var("] {
        let mut from = 0;
        while let Some(i) = code[from..].find(needle) {
            let at = from + i;
            from = at + needle.len();
            let prev = if at == 0 { b' ' } else { bytes[at - 1] };
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue; // e.g. `my_set_var(`
            }
            if code[..at].trim_end().ends_with("fn") {
                continue; // a definition, not a call
            }
            out.all += 1;
            let rest = code[from..].trim_start();
            if PROTECTED_VARS
                .iter()
                .any(|v| rest.starts_with(&format!("\"{v}\"")))
            {
                out.protected += 1;
            }
        }
    }
    out
}

/// Walk `src_root` and compare every `.rs` file's env-mutation counts with
/// `baseline` (path relative to `src_root`, `/`-separated, then
/// `(max all, max protected)`; files not listed allow 0). Returns one line
/// per file over its limit; empty = pass. Lower counts always pass, so the
/// baseline only ever needs lowering (never raising) when sites are fixed.
pub fn env_mutation_violations(
    src_root: &Path,
    baseline: &[(&str, usize, usize)],
) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![src_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(src_root)
            .expect("under src root")
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let c = count_env_mutations(&src);
        let (max_all, max_protected) = baseline
            .iter()
            .find(|(f, _, _)| *f == rel)
            .map(|(_, a, p)| (*a, *p))
            .unwrap_or((0, 0));
        if c.protected > max_protected {
            out.push(format!(
                "{rel}: {} set_var/remove_var of HOME/PATH/SHELL (allowed {max_protected}): \
                 use k2_core::test_env::TempHome / EnvVar",
                c.protected
            ));
        }
        if c.all > max_all {
            out.push(format!(
                "{rel}: {} raw set_var/remove_var calls (allowed {max_all}): tests change env \
                 only through k2_core::test_env::EnvVar (holds the one env lock, restores on drop)",
                c.all
            ));
        }
    }
    out
}

/// Lines under `src_root` (non-comment) that use `thread::current().id()`.
/// Temp paths named from a `ThreadId` (or pid + nanos) repeat across the
/// test binaries of one run and across runs (the 0.45.0 gate leaked
/// `k2-update-test-boot-<pid>-ThreadId(N)` homes); use
/// [`unique_temp_path`]. Nothing in `src/` needs a `ThreadId` today, so the
/// rule is zero-tolerance.
pub fn thread_id_name_violations(src_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![src_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (n, line) in src.lines().enumerate() {
                // concat! so this line never matches itself.
                if !line.trim_start().starts_with("//")
                    && line.contains(concat!("current()", ".id()"))
                {
                    out.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    out.sort();
    out
}

/// A loopback TCP port that is RESERVED but not listening: connects to it
/// are refused at once, and no other test can take it while the guard
/// lives. Use it for "nothing listens there" tests instead of binding a
/// listener and dropping it, which frees the port for any parallel test to
/// re-bind (`local_port_reachable_false_for_closed_port` saw a closed port
/// answer under load). [`ClosedPort::new`] reserves `127.0.0.1:<port>`;
/// [`ClosedPort::dual`] also reserves `[::1]:<port>` when the host has IPv6
/// loopback, for tests of code that tries both.
#[cfg(unix)]
pub struct ClosedPort {
    port: u16,
    _fds: Vec<std::os::fd::OwnedFd>,
}

#[cfg(unix)]
impl ClosedPort {
    /// Reserve a closed `127.0.0.1` port.
    pub fn new() -> Self {
        let (fd, port) = reserve(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
        Self { port, _fds: vec![fd] }
    }

    /// Reserve the same closed port on `127.0.0.1` and (when IPv6
    /// loopback exists) `::1`.
    pub fn dual() -> Self {
        for _ in 0..64 {
            let (v4, port) = reserve(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
            match reserve(true, port) {
                Ok((v6, _)) => return Self { port, _fds: vec![v4, v6] },
                // No IPv6 loopback on this host: nothing can listen there.
                Err(e) if e.raw_os_error() == Some(libc::EAFNOSUPPORT)
                    || e.raw_os_error() == Some(libc::EADDRNOTAVAIL) =>
                {
                    return Self { port, _fds: vec![v4] }
                }
                // Someone holds [::1]:port; pick another port.
                Err(_) => continue,
            }
        }
        panic!("could not reserve one closed port on both loopbacks");
    }

    /// The reserved port.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A bound, NOT listening TCP socket on loopback (`port` 0 = any).
#[cfg(unix)]
fn reserve(v6: bool, port: u16) -> std::io::Result<(std::os::fd::OwnedFd, u16)> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
    // SAFETY: plain socket syscalls on a fresh fd we own; every pointer
    // passed is to a correctly sized, zero-initialized sockaddr.
    unsafe {
        let raw = libc::socket(family, libc::SOCK_STREAM, 0);
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(raw);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        let mut storage: libc::sockaddr_storage = std::mem::zeroed();
        let len = if v6 {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in6);
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                a.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
            }
            a.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            a.sin6_port = port.to_be();
            a.sin6_addr.s6_addr = std::net::Ipv6Addr::LOCALHOST.octets();
            std::mem::size_of::<libc::sockaddr_in6>()
        } else {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in);
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                a.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            }
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            std::mem::size_of::<libc::sockaddr_in>()
        } as libc::socklen_t;
        if libc::bind(fd.as_raw_fd(), &storage as *const _ as *const libc::sockaddr, len) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut out: libc::sockaddr_storage = std::mem::zeroed();
        let mut out_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd.as_raw_fd(), &mut out as *mut _ as *mut libc::sockaddr, &mut out_len) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let bound = if v6 {
            (*(&out as *const _ as *const libc::sockaddr_in6)).sin6_port
        } else {
            (*(&out as *const _ as *const libc::sockaddr_in)).sin_port
        };
        Ok((fd, u16::from_be(bound)))
    }
}

/// Run `f` with a fresh temp `$HOME` (see [`TempHome`]).
pub fn with_temp_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let home = TempHome::new();
    f(home.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_guard_restores_previous_value_lifo() {
        const KEY: &str = "K2_TEST_ENV_GUARD_PROBE";
        let _l = lock();
        assert_eq!(std::env::var_os(KEY), None);
        {
            let _a = EnvVar::set(KEY, "a");
            assert_eq!(std::env::var(KEY).unwrap(), "a");
            {
                let _b = EnvVar::set(KEY, "b");
                assert_eq!(std::env::var(KEY).unwrap(), "b");
                {
                    let _c = EnvVar::remove(KEY);
                    assert_eq!(std::env::var_os(KEY), None);
                }
                assert_eq!(std::env::var(KEY).unwrap(), "b");
            }
            assert_eq!(std::env::var(KEY).unwrap(), "a");
        }
        assert_eq!(std::env::var_os(KEY), None);
    }

    #[test]
    fn temp_home_is_unique_restored_and_removed() {
        let _l = lock();
        let before = std::env::var_os("HOME");
        let dir;
        {
            let a = TempHome::new();
            dir = a.path().to_path_buf();
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
            assert!(dir.join(".k2").is_dir());
            // Re-entrant: a nested guard on the same thread does not deadlock.
            let b = TempHome::new();
            assert_ne!(a.path(), b.path());
            drop(b);
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
        }
        assert_eq!(std::env::var_os("HOME"), before);
        assert!(!dir.exists(), "temp home removed on drop");
    }

    #[test]
    fn lock_survives_a_panicking_holder() {
        let t = std::thread::spawn(|| {
            let _g = lock();
            panic!("holder panics");
        });
        assert!(t.join().is_err());
        // Not poisoned: the next taker proceeds.
        let _g = lock();
    }

    #[cfg(unix)]
    #[test]
    fn agent_shim_registers_and_unregisters() {
        use crate::terminal::agent_spawn_guard::{resolve_program, GuardEnv, SHIM_DIR_ENV};
        let _l = lock();
        let before = std::env::var_os(SHIM_DIR_ENV);
        let dir;
        {
            let shim = AgentShim::install();
            dir = shim.dir().to_path_buf();
            let resolved = resolve_program("claude", "/usr/bin", &GuardEnv::from_process())
                .expect("claude resolves to the shim");
            assert_eq!(resolved, dir.join("claude").to_string_lossy());
            let body = std::fs::read_to_string(dir.join("codex")).unwrap();
            assert_eq!(body, "#!/bin/sh\nexec cat\n");
        }
        assert_eq!(std::env::var_os(SHIM_DIR_ENV), before);
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn closed_port_refuses_at_once_and_stays_reserved() {
        let closed = ClosedPort::dual();
        let t = std::time::Instant::now();
        let err = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], closed.port())),
            std::time::Duration::from_secs(5),
        )
        .expect_err("nothing listens on a reserved port");
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err:?}");
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
        // Reserved: nobody else can listen on it meanwhile.
        assert!(std::net::TcpListener::bind(("127.0.0.1", closed.port())).is_err());
    }

    #[test]
    fn error_http_server_answers_its_status_fast() {
        use std::io::{Read, Write};
        let srv = ErrorHttpServer::start(503);
        assert!(srv.url().starts_with("http://127.0.0.1:"));
        let t = std::time::Instant::now();
        let mut c = std::net::TcpStream::connect(srv.addr()).expect("connect");
        c.write_all(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n").unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 503 "), "{out:?}");
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn env_mutation_counter_sees_calls_not_prose() {
        let src = "// std::env::set_var(\"HOME\", x) in a comment\n\
                   fn set_var(k: &str) {}\n\
                   std::env::set_var(\"HOME\", h);\n\
                   std::env::remove_var(\n    \"PATH\");\n\
                   env::set_var(\"K2_X\", \"1\");\n\
                   my_set_var(\"HOME\");\n";
        assert_eq!(count_env_mutations(src), EnvMutationCount { all: 3, protected: 2 });
    }

    /// RATCHET: no new raw env mutation in k2-core. HOME/PATH/SHELL change
    /// only through this module; other vars only in the files (and counts)
    /// listed in `K2_CORE_ENV_BASELINE`. Lower a count when you convert
    /// sites; never raise one.
    #[test]
    fn no_thread_id_temp_names_in_k2_core() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let v = thread_id_name_violations(&src);
        assert!(v.is_empty(), "use test_env::unique_temp_path, not a ThreadId:\n{}", v.join("\n"));
    }

    #[test]
    fn no_new_raw_env_mutation_in_k2_core() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let violations = env_mutation_violations(&src, K2_CORE_ENV_BASELINE);
        assert!(
            violations.is_empty(),
            "raw env mutation over the ratchet:\n{}",
            violations.join("\n")
        );
    }
}

/// `(file, max raw calls, max HOME/PATH/SHELL calls)` for k2-core `src/`.
/// `test_env.rs` is the sanctioned home of the raw calls (its guards, plus
/// the counter test's sample text); `lib.rs` is production
/// (`enrich_path_from_login_shell` adopts the login-shell PATH).
#[cfg(test)]
const K2_CORE_ENV_BASELINE: &[(&str, usize, usize)] = &[
    ("lib.rs", 1, 1),
    ("test_env.rs", 13, 3),
];
