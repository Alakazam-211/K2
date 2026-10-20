//! Everything with a side effect goes through [`Ctx`] (§14): the root the
//! fixed paths resolve under, child processes ([`Exec`]), the process table
//! ([`Procs`], for the caller walk), randomness, and the audit sink.
//!
//! [`Ctx::production`] is the only constructor the binary uses: root `/`,
//! real owners, real processes. There is no env var or argv that changes
//! any of it. Tests build a `Ctx` over a temp dir with fakes.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

use k2_root_proto::paths::{CHILD_PATH, HELPER_PATH};
use k2_root_proto::verbs::exit;

use crate::fsx::Dir;

/// A refusal or failure: exit code + one short line. Never caller bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub code: i32,
    pub msg: String,
}

impl Fail {
    pub fn argv(msg: impl Into<String>) -> Self {
        Self {
            code: exit::ARGV,
            msg: msg.into(),
        }
    }
    pub fn state(msg: impl Into<String>) -> Self {
        Self {
            code: exit::STATE,
            msg: msg.into(),
        }
    }
    pub fn sig(msg: impl Into<String>) -> Self {
        Self {
            code: exit::SIGNATURE,
            msg: msg.into(),
        }
    }
    pub fn io(msg: impl Into<String>) -> Self {
        Self {
            code: exit::IO,
            msg: msg.into(),
        }
    }
    pub fn io_err(what: &str, e: io::Error) -> Self {
        Self::io(format!("{what}: {}", e.kind_str()))
    }
    pub fn not_daemon() -> Self {
        Self {
            code: exit::NOT_DAEMON,
            msg: "caller_not_daemon".into(),
        }
    }
}

/// io::Error → a short, static-ish string (no paths from the caller ever
/// reach an io::Error here; this just keeps lines short).
pub trait KindStr {
    fn kind_str(&self) -> String;
}
impl KindStr for io::Error {
    fn kind_str(&self) -> String {
        match self.raw_os_error() {
            Some(libc::ELOOP) => "refused: symlink".into(),
            Some(libc::ENOTDIR) => "refused: not a directory".into(),
            Some(libc::ENOSPC) => "no space left on device".into(),
            _ => self.to_string(),
        }
    }
}

pub type HResult<T> = Result<T, Fail>;

/// Output of a child process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOut {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Children are spawned by absolute path with `env_clear()`,
/// `PATH=/usr/sbin:/usr/bin:/sbin:/bin`, `LC_ALL=C`, stdin null unless the
/// verb pipes data. Never a shell.
pub trait Exec {
    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> io::Result<ExecOut>;
}

pub struct RealExec;

impl Exec for RealExec {
    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> io::Result<ExecOut> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if !prog.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "program must be absolute",
            ));
        }
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .env_clear()
            .env("PATH", CHILD_PATH)
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn()?;
        // Feed stdin from a thread while wait_with_output drains stdout and
        // stderr: a child (psql) that prints more than a pipe buffer while
        // we are still writing would otherwise deadlock (security review #9).
        let writer = match stdin {
            Some(bytes) => {
                let mut sin = child
                    .stdin
                    .take()
                    .ok_or_else(|| io::Error::other("stdin"))?;
                let bytes = bytes.to_vec();
                // A child that exits early (psql on a syntax error) closes
                // the pipe; that is its answer, not ours.
                Some(std::thread::spawn(move || {
                    let _ = sin.write_all(&bytes);
                }))
            }
            None => None,
        };
        let out = child.wait_with_output()?;
        if let Some(w) = writer {
            let _ = w.join();
        }
        Ok(ExecOut {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

/// The process table, for the caller walk (§6.2).
pub trait Procs {
    fn parent_pid(&self) -> i32;
    /// A handle that later tells whether this exact process still lives
    /// (a pidfd on Linux). `None` when it is already gone.
    fn hold(&self, pid: i32) -> Option<Box<dyn ProcHandle>>;
    /// `/proc/<pid>/exe` target and that file's owner uid.
    fn exe(&self, pid: i32) -> Option<(PathBuf, u32)>;
    fn ppid_of(&self, pid: i32) -> Option<i32>;
    /// Real uid.
    fn ruid_of(&self, pid: i32) -> Option<u32>;
    /// Session id (`/proc/<pid>/stat` field 6; no ptrace needed).
    fn sid_of(&self, pid: i32) -> Option<i32>;
    /// This process's own session id (`getsid(0)`).
    fn own_sid(&self) -> Option<i32>;
    fn uid_of_user(&self, user: &str) -> Option<u32>;
}

pub trait ProcHandle {
    fn alive(&self) -> bool;
}

pub trait Random {
    fn fill(&self, buf: &mut [u8]) -> io::Result<()>;
}

/// One journal line per call (§12).
pub trait AuditSink {
    fn line(&self, json: &str);
}

pub struct Ctx {
    /// `/` in production; a temp dir in tests.
    pub root: PathBuf,
    /// True in production: chown files to root, chown Stalwart trees, check
    /// owners. False in tests (they never run as root).
    pub real_owners: bool,
    pub euid: u32,
    pub arch: &'static str,
    /// The running binary (`/proc/self/exe`).
    pub self_exe: PathBuf,
    /// This build trusts the TEST key (`--features test-keys`).
    pub test_keys: bool,
    /// This build pins a real Sew release (`sew::sew_release_pinned`); while
    /// false the Sew install verbs refuse `sew_not_released`. Tests flip it
    /// to check the release-build refusal.
    pub sew_pinned: bool,
    pub exec: Box<dyn Exec>,
    pub procs: Box<dyn Procs>,
    pub rand: Box<dyn Random>,
    pub audit: Box<dyn AuditSink>,
    /// Bytes for stdout (JSON, or the 32-byte KEK).
    pub out: RefCell<Vec<u8>>,
    /// Lock wait (30 s in production).
    pub lock_timeout_ms: u64,
    /// Bytes free on a path's filesystem (statvfs in production; tests
    /// pick an answer). Box prep's disk floor reads it (0.46.3 F5).
    pub free_bytes: fn(&Path) -> Result<u64, String>,
}

impl Ctx {
    /// The binary's context. Nothing here can be redirected at runtime.
    pub fn production() -> Self {
        Ctx {
            root: PathBuf::from("/"),
            real_owners: true,
            // SAFETY: geteuid has no preconditions.
            euid: unsafe { libc::geteuid() },
            arch: std::env::consts::ARCH,
            self_exe: crate::sys::self_exe(),
            test_keys: cfg!(feature = "test-keys"),
            sew_pinned: k2_root_proto::sew::sew_release_pinned(),
            exec: Box::new(RealExec),
            procs: Box::new(crate::sys::RealProcs),
            rand: Box::new(crate::sys::RealRandom),
            audit: Box::new(crate::sys::SyslogAudit),
            out: RefCell::new(Vec::new()),
            lock_timeout_ms: 30_000,
            free_bytes: crate::mail_data::free_bytes,
        }
    }

    /// A fixed absolute path under the root.
    pub fn p(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }

    /// The directory handle for a fixed absolute directory.
    pub fn dir(&self, abs: &str) -> io::Result<Dir> {
        Dir::open_beneath(&self.root, abs)
    }

    /// Owner for root-owned files (`None` in tests).
    pub fn root_owner(&self) -> Option<(u32, u32)> {
        self.real_owners.then_some((0, 0))
    }

    pub fn print(&self, bytes: &[u8]) {
        self.out.borrow_mut().extend_from_slice(bytes);
    }

    pub fn print_json(&self, v: &serde_json::Value) {
        self.print(v.to_string().as_bytes());
        self.print(b"\n");
    }

    /// Is the running binary the self-updatable path (not package-owned)?
    pub fn is_canonical_path(&self) -> bool {
        self.self_exe == self.p(HELPER_PATH)
    }

    /// Run a fixed program; non-zero exit is an `Err` with its stderr
    /// (the child's words, trimmed; never our stdin).
    pub fn run_ok(&self, prog: &str, args: &[&str], stdin: Option<&[u8]>) -> HResult<ExecOut> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let out = self
            .exec
            .run(prog, &args, stdin)
            .map_err(|e| Fail::io(format!("{prog}: {}", e.kind_str())))?;
        if out.status != 0 {
            let err = String::from_utf8_lossy(&out.stderr);
            let err: String = err.trim().chars().take(400).collect();
            return Err(Fail::io(format!("{prog} exit {}: {err}", out.status)));
        }
        Ok(out)
    }
}

/// Root path helper for tests and fixtures.
pub fn under(root: &Path, abs: &str) -> PathBuf {
    root.join(abs.trim_start_matches('/'))
}
