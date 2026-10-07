//! Live process lookup for the hook owner check
//! (prd-daemon-activity-and-thread-working-v1 DA14-2).
//!
//! [`LiveProcessTable`] answers "is pid N alive, who is its parent, and
//! when did it start" from the OS:
//! - macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` (`pbi_ppid`, `pbi_start_*`).
//! - Linux: `/proc/<pid>/stat` fields 4 (`ppid`) and 22 (`starttime`).
//! - Elsewhere (Windows): always `None`. Windows runs the conversation-only
//!   owner check instead ([`crate::agent_hooks::owner::OwnerCheckMode`]).
//!
//! The start time is only compared with itself (pid-reuse detection), so
//! its unit differs per OS and that is fine.

use crate::agent_hooks::owner::{ProcInfo, ProcessTable};

/// The OS process table.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveProcessTable;

impl ProcessTable for LiveProcessTable {
    fn info(&self, pid: i32) -> Option<ProcInfo> {
        if pid <= 0 {
            return None;
        }
        platform::info(pid)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ProcInfo;

    pub fn info(pid: i32) -> Option<ProcInfo> {
        // SAFETY: `proc_bsdinfo` is plain old data; `proc_pidinfo` writes at
        // most `size` bytes into it and returns how many it wrote.
        let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut bsd as *mut libc::proc_bsdinfo as *mut libc::c_void,
                size,
            )
        };
        if written != size {
            return None;
        }
        Some(ProcInfo {
            pid,
            ppid: bsd.pbi_ppid as i32,
            start_time: bsd.pbi_start_tvsec.saturating_mul(1_000_000) + bsd.pbi_start_tvusec,
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ProcInfo;

    pub fn info(pid: i32) -> Option<ProcInfo> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(pid, &raw)
    }

    /// Fields after the parenthesised `comm` (which may hold spaces or
    /// parens itself): index 0 is field 3 (`state`), so `ppid` (field 4) is
    /// index 1 and `starttime` (field 22) is index 19.
    pub fn parse_stat(pid: i32, raw: &str) -> Option<ProcInfo> {
        let after = &raw[raw.rfind(')')? + 1..];
        let fields: Vec<&str> = after.split_whitespace().collect();
        Some(ProcInfo {
            pid,
            ppid: fields.get(1)?.parse().ok()?,
            start_time: fields.get(19)?.parse().ok()?,
        })
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn parse_stat_survives_parens_in_comm() {
            let raw = "4242 (a) b (c)) S 77 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
            let info = super::parse_stat(4242, raw).expect("parses");
            assert_eq!(info.ppid, 77);
            assert_eq!(info.start_time, 987654);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::ProcInfo;

    pub fn info(_pid: i32) -> Option<ProcInfo> {
        None
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use crate::agent_hooks::owner::{ancestry, MAX_ANCESTRY_DEPTH};

    #[test]
    fn own_process_resolves_with_the_real_parent() {
        let me = std::process::id() as i32;
        let info = LiveProcessTable.info(me).expect("this process is alive");
        // SAFETY: getppid has no preconditions.
        let parent = unsafe { libc::getppid() };
        assert_eq!(info.ppid, parent);
        assert!(info.start_time > 0);
        // Stable across reads (the pid-reuse check depends on it).
        assert_eq!(LiveProcessTable.info(me).expect("again").start_time, info.start_time);
    }

    #[test]
    fn a_child_process_chain_reaches_this_process() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        let info = LiveProcessTable.info(pid).expect("child is alive");
        let chain = ancestry(&LiveProcessTable, info, MAX_ANCESTRY_DEPTH);
        let me = std::process::id() as i32;
        assert!(chain.iter().any(|p| p.pid == me), "chain {chain:?} misses {me}");
        child.kill().expect("kill sleep");
        child.wait().expect("reap sleep");
        assert!(LiveProcessTable.info(pid).is_none(), "a reaped pid is gone");
    }

    #[test]
    fn nonsense_pids_are_none() {
        assert!(LiveProcessTable.info(0).is_none());
        assert!(LiveProcessTable.info(-5).is_none());
    }
}
