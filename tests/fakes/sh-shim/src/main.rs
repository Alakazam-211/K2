//! TEST ONLY (Windows). `k2_core::test_env::write_executable(path, script)`
//! writes the sh script at `path` and a copy of this program at
//! `path.exe`; Windows (and Rust's `Command`) run `path.exe` for an
//! extensionless `path`. This program runs `path` (or `path.sh`) in Git
//! Bash with the same arguments, stdin, stdout and stderr, and exits with
//! the script's code.
//!
//! It first puts itself in a Job Object that kills on close, so the bash
//! it starts (and that script's own children: `sleep`, `cat`) die when this
//! process is killed, as a Unix script's process would. Git Bash is
//! `K2_TEST_BASH`, else `Git\bin\bash.exe` under Program Files; none is a
//! loud failure (exit 127).

fn main() {
    #[cfg(windows)]
    std::process::exit(win::run());
    #[cfg(not(windows))]
    {
        eprintln!("k2-test-sh-shim: a Windows test helper; Unix runs the script itself");
        std::process::exit(2);
    }
}

#[cfg(windows)]
mod win {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE};

    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    fn bash() -> Option<PathBuf> {
        if let Some(b) = std::env::var_os("K2_TEST_BASH").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(b));
        }
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|r| PathBuf::from(r).join("Git").join("bin").join("bash.exe"))
            .find(|p| p.is_file())
    }

    /// Kill-on-close job holding this process and everything it starts.
    fn contain_self() {
        // SAFETY: plain Win32 calls; the job handle is deliberately kept
        // open for the life of the process (closing it kills the tree).
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            AssignProcessToJobObject(job, GetCurrentProcess());
        }
    }

    pub fn run() -> i32 {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("k2-test-sh-shim: current_exe: {e}");
                return 127;
            }
        };
        let plain = exe.with_extension("");
        let script = if plain.is_file() { plain } else { exe.with_extension("sh") };
        if !script.is_file() {
            eprintln!("k2-test-sh-shim: no script next to {} (looked for {})", exe.display(), script.display());
            return 127;
        }
        let Some(bash) = bash() else {
            eprintln!("k2-test-sh-shim: Git Bash not found (install Git for Windows or set K2_TEST_BASH)");
            return 127;
        };
        contain_self();
        let mut cmd = Command::new(&bash);
        cmd.arg(&script).args(std::env::args_os().skip(1));
        let (relay_out, relay_err) = (is_disk_file(STD_OUTPUT_HANDLE), is_disk_file(STD_ERROR_HANDLE));
        if relay_out {
            cmd.stdout(Stdio::piped());
        }
        if relay_err {
            cmd.stderr(Stdio::piped());
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("k2-test-sh-shim: run {}: {e}", bash.display());
                return 127;
            }
        };
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let mut relays = 0;
        if let Some(out) = child.stdout.take() {
            relay(out, std::io::stdout(), done_tx.clone());
            relays += 1;
        }
        if let Some(err) = child.stderr.take() {
            relay(err, std::io::stderr(), done_tx.clone());
            relays += 1;
        }
        let code = match child.wait() {
            Ok(st) => st.code().unwrap_or(1),
            Err(e) => {
                eprintln!("k2-test-sh-shim: wait for {}: {e}", bash.display());
                127
            }
        };
        // The script's own leftovers (a backgrounded `sleep`) can hold a
        // pipe open; they die with this process's job, so don't wait on them.
        for _ in 0..relays {
            if done_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                break;
            }
        }
        code
    }

    /// The handle is a file on disk. Git Bash can't write to one opened
    /// append-only (`FILE_APPEND_DATA` without `FILE_WRITE_DATA`, which is
    /// what Rust's `append(true)` gives, as for K2's `frpc.log`); a real
    /// Windows program can. So for a file, the shim relays the script's
    /// output through a pipe; a console or pipe is handed straight down.
    fn is_disk_file(which: STD_HANDLE) -> bool {
        // SAFETY: plain queries of this process's own standard handles.
        unsafe {
            let h = GetStdHandle(which);
            !h.is_null() && h != INVALID_HANDLE_VALUE && GetFileType(h) == FILE_TYPE_DISK
        }
    }

    fn relay(mut from: impl Read + Send + 'static, mut to: impl Write + Send + 'static, done: mpsc::Sender<()>) {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match from.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if to.write_all(&buf[..n]).and_then(|_| to.flush()).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = done.send(());
        });
    }
}
