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
    command_lines(&[pid]).into_iter().next().map(|(_, c)| c)
}

/// The command lines of `pids` in one CIM query (one PowerShell start for
/// them all). A pid that is gone, or whose line can't be read, is left out.
pub fn command_lines(pids: &[u32]) -> Vec<(u32, String)> {
    use std::os::windows::process::CommandExt;
    /// No console window flashes up from the (windowless) daemon.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if pids.is_empty() {
        return Vec::new();
    }
    let filter = pids.iter().map(|p| format!("ProcessId={p}")).collect::<Vec<_>>().join(" OR ");
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
                 Get-CimInstance Win32_Process -Filter '{filter}' | \
                 ForEach-Object {{ [string]$_.ProcessId + [char]9 + $_.CommandLine }}"
            ),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, cmd) = l.split_once('\t')?;
            let cmd = cmd.trim();
            (!cmd.is_empty()).then(|| Some((pid.trim().parse().ok()?, cmd.to_string())))?
        })
        .collect()
}

/// Pids of the running processes whose program file name starts with
/// `prefix` (ASCII case-insensitive), from a Toolhelp snapshot: cheap, so a
/// caller can skip the slow `command_line` when nothing matches.
pub fn pids_named(prefix: &str) -> Vec<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: a snapshot handle checked for failure and closed once; the
    // entry is sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix) {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// Pids of the running processes whose parent is `parent` (Toolhelp's
/// parent pid; a parent that exited can leave stale children listed by a
/// recycled pid, so callers that care check start times).
pub fn children_of(parent: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: as in pids_named.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            if e.th32ParentProcessID == parent && e.th32ProcessID != 0 {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// End `pid` and every process descended from it (Windows' answer to a
/// Unix `killpg` of the child's group). The tree is read first, each child
/// kept only when it started after its parent (a recycled pid that merely
/// names the old parent is never touched); then the root goes first, so it
/// can't start more, then the descendants. Returns how many ended.
pub fn terminate_tree(pid: u32) -> usize {
    let Some(root_start) = start_key(pid) else { return 0 };
    let mut tree = vec![pid];
    let mut frontier = vec![(pid, root_start)];
    while let Some((parent, parent_start)) = frontier.pop() {
        for child in children_of(parent) {
            if tree.contains(&child) {
                continue;
            }
            if let Some(started) = start_key(child).filter(|s| *s >= parent_start) {
                tree.push(child);
                frontier.push((child, started));
            }
        }
    }
    tree.into_iter().filter(|p| terminate(*p)).count()
}

/// `cmdline` split into argv the way Windows programs parse their own
/// (CommandLineToArgvW): quotes and backslashes as the C runtime reads them.
pub fn split_command_line(cmdline: &str) -> Vec<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
    if cmdline.trim().is_empty() {
        // The API answers an empty line with this program's own path.
        return Vec::new();
    }
    let wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let mut n = 0i32;
    // SAFETY: a NUL-terminated wide string; the returned block holds `n`
    // NUL-terminated strings and is freed once with LocalFree.
    unsafe {
        let argv = CommandLineToArgvW(wide.as_ptr(), &mut n);
        if argv.is_null() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(n.max(0) as usize);
        for i in 0..n.max(0) as usize {
            let p = *argv.add(i);
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
        }
        LocalFree(argv as _);
        out
    }
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
        let both = command_lines(&[pid, std::process::id()]);
        assert_eq!(both.len(), 2, "{both:?}");
        assert_eq!(both.iter().find(|(p, _)| *p == pid).map(|(_, c)| c.as_str()), Some(cl.as_str()));
        assert!(terminate(pid));
        child.wait().expect("wait");
        assert!(!pid_alive(pid));
        assert_eq!(start_key(pid), None);
    }

    #[test]
    fn pids_by_name_and_a_command_line_split_as_windows_does() {
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_name().unwrap().to_string_lossy().to_ascii_uppercase();
        assert!(pids_named(&stem[..4]).contains(&me), "this test binary is found by its name prefix");
        assert!(!pids_named("no-such-program-k2").contains(&me));

        assert_eq!(
            split_command_line(r#""C:\Program Files\K2\frpc.exe" -c "C:\Users\a b\.k2\frpc.toml""#),
            vec![r"C:\Program Files\K2\frpc.exe", "-c", r"C:\Users\a b\.k2\frpc.toml"]
        );
        assert_eq!(split_command_line(r"D:\k2\frpc -c D:\k2\frpc.toml"), vec![r"D:\k2\frpc", "-c", r"D:\k2\frpc.toml"]);
        assert!(split_command_line("  ").is_empty());
    }
}
