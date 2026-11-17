//! Live process lookup for the hook owner check
//! (prd-daemon-activity-and-thread-working-v1 DA14-2).
//!
//! [`LiveProcessTable`] answers "is pid N alive, who is its parent, and
//! when did it start" from the OS:
//! - macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` (`pbi_ppid`, `pbi_start_*`).
//! - Linux: `/proc/<pid>/stat` fields 4 (`ppid`) and 22 (`starttime`).
//! - Elsewhere (Windows): always `None`. Windows runs the conversation-only
//!   owner check instead (`OwnerCheckMode` in k2-core's agent_hooks::owner).
//!
//! The start time is only compared with itself (pid-reuse detection), so
//! its unit differs per OS and that is fine.
//!
//! [`LiveProcessTable::command_name`] answers "what is pid N called": the
//! kernel's command name (`comm`), the file name the process was started
//! from, cut to 15 bytes on Linux and 16 on macOS. It comes from the same
//! read as the facts above (`/proc/<pid>/stat`'s second field on Linux,
//! `pbi_comm` on macOS), so a process of another user that the walk can
//! see can be named too. Elsewhere it is `None`. The Rust CLI's passport
//! rule names a session's processes by it.

use crate::proc_tree::{ProcInfo, ProcessTable};

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

impl LiveProcessTable {
    /// The command name of `pid` (see the module notes), or `None` when
    /// the pid is not alive or the OS can't say.
    pub fn command_name(&self, pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        platform::command_name(pid)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ProcInfo;

    fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
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
        Some(bsd)
    }

    pub fn info(pid: i32) -> Option<ProcInfo> {
        let bsd = bsd_info(pid)?;
        Some(ProcInfo {
            pid,
            ppid: bsd.pbi_ppid as i32,
            start_time: bsd.pbi_start_tvsec.saturating_mul(1_000_000) + bsd.pbi_start_tvusec,
        })
    }

    /// `pbi_comm`, up to its first NUL (it has none when it fills the
    /// field).
    pub fn command_name(pid: i32) -> Option<String> {
        let bsd = bsd_info(pid)?;
        let bytes: Vec<u8> = bsd.pbi_comm.iter().map(|c| *c as u8).take_while(|b| *b != 0).collect();
        if bytes.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ProcInfo;

    pub fn info(pid: i32) -> Option<ProcInfo> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(pid, &raw)
    }

    pub fn command_name(pid: i32) -> Option<String> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_comm(&raw)
    }

    /// Field 2, `comm`: what is between the first `(` and the last `)`.
    pub fn parse_comm(raw: &str) -> Option<String> {
        let open = raw.find('(')?;
        let close = raw.rfind(')')?;
        (close > open).then(|| raw[open + 1..close].to_string())
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
            assert_eq!(super::parse_comm(raw).as_deref(), Some("a) b (c)"));
            assert_eq!(super::parse_comm("4242 k2-ptyd S 1"), None);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::ProcInfo;

    pub fn info(_pid: i32) -> Option<ProcInfo> {
        None
    }

    pub fn command_name(_pid: i32) -> Option<String> {
        None
    }
}
