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
    use std::path::PathBuf;
    use std::process::Command;

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
        match Command::new(&bash).arg(&script).args(std::env::args_os().skip(1)).status() {
            Ok(st) => st.code().unwrap_or(1),
            Err(e) => {
                eprintln!("k2-test-sh-shim: run {}: {e}", bash.display());
                127
            }
        }
    }
}
