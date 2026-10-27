//! Windows process facts K2 reads elsewhere from `/proc` or `ps`: is a pid
//! alive, when did it start (so a recycled pid is never mistaken for the
//! process K2 recorded), and its command line.

use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};

/// `GetExitCodeProcess` while the process runs (STATUS_PENDING).
const STILL_ACTIVE: u32 = 259;

struct Proc(HANDLE);

impl Proc {
    fn open(pid: u32) -> Option<Proc> {
        if pid == 0 {
            return None;
        }
        // SAFETY: plain OpenProcess; a null handle is checked.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!h.is_null()).then_some(Proc(h))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // SAFETY: a handle OpenProcess gave us, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Proc {
    /// Still running. An exited process stays openable while anyone (its
    /// parent's `Child`, say) holds a handle, so opening is not enough.
    fn running(&self) -> bool {
        let mut code = 0u32;
        // SAFETY: a live handle and our own out-param.
        let ok = unsafe { GetExitCodeProcess(self.0, &mut code) };
        ok != 0 && code == STILL_ACTIVE
    }
}

/// The process exists and has not exited.
pub fn pid_alive(pid: u32) -> bool {
    Proc::open(pid).is_some_and(|p| p.running())
}

/// The process's creation time (100 ns since 1601), a stable identity for a
/// live pid. `None` when the process has exited or can't be opened.
pub fn start_key(pid: u32) -> Option<u64> {
    let p = Proc::open(pid).filter(Proc::running)?;
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: a live handle and four out-params we own.
    let ok = unsafe { GetProcessTimes(p.0, &mut created, &mut exited, &mut kernel, &mut user) };
    (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// End the process at once (TerminateProcess, exit code 1); Windows has no
/// SIGTERM for a console process K2 didn't start in its own console.
/// `false` when it can't be opened (already gone, or not ours).
pub fn terminate(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: OpenProcess / TerminateProcess / CloseHandle on a handle we
    // own; a null handle is checked.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// The process's command line as Windows recorded it (one string, the
/// program usually quoted), read through CIM (`Win32_Process`). Slow
/// (PowerShell starts): for a one-time check such as adoption at boot.
pub fn command_line(pid: u32) -> Option<String> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').CommandLine"),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_child_is_alive_with_a_start_and_a_command_line_then_gone() {
        let (prog, args) = crate::test_env::sleep_command(30);
        let mut child = Command::new(&prog).args(&args).stdout(Stdio::null()).spawn().expect("spawn");
        let pid = child.id();
        assert!(pid_alive(pid));
        let start = start_key(pid).expect("start time");
        assert!(start > 0);
        assert_eq!(start_key(pid), Some(start), "stable while it lives");
        let cl = command_line(pid).expect("command line");
        assert!(cl.to_ascii_lowercase().contains("ping") && cl.contains("127.0.0.1"), "{cl}");
        assert!(terminate(pid));
        child.wait().expect("wait");
        assert!(!pid_alive(pid));
        assert_eq!(start_key(pid), None);
    }
}
