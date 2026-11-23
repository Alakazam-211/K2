//! A job's whole process tree: found and ended at the job's end, and the
//! node's check for processes no live job owns (0.46.16).
//!
//! Each job starts in its own session and process group (`setsid`, in
//! [`crate::runner::spawn`] and the holder), and a stop or an end signals
//! that group. A process can still leave it: `setpgid` (it stays in the
//! session) or `setsid` (it leaves both), often with a double fork, so it
//! is reparented to init/launchd. Linux's cgroup (the unit, or the leaf)
//! holds those; macOS has no cgroup, and a Linux node without a delegated
//! cgroup has none either. A bench job's child daemons outlived a cancel
//! by ~27 hours on a Mac mini that way (11-18), and the node showed
//! nothing running.
//!
//! So after the group is gone the node sweeps the processes of its own
//! uid (the job user without the Linux wall) that are the job's by any of:
//!
//! - its process group or session (`setpgid` escapes), while the group's
//!   leader is gone or still the job's (a reused id is never the job's);
//! - its working directory, executable, an argv entry or an env value
//!   under the job's folder ([`crate::strays::belongs`]): every job
//!   process inherits `HOME` and `TMPDIR` there unless it clears its env,
//!   and a test harness that strips `K2_*` keeps them.
//!
//! `K2_COMPUTE_JOB=<id>` alone never marks a process for a kill: an id is
//! the controller's, and two nodes on one machine (tests, nested nodes)
//! may run the same one. The folder is this node's own.
//!
//! Each is ended with SIGTERM, a grace period, then SIGKILL, re-checked by
//! its start time right before each signal (a reused pid is never hit).
//! Never touched: the node itself and its process group, `sccache` (the
//! slot's shared compile server), any `k2-node` process (holders, the
//! egress proxy), and other users' processes.
//!
//! [`leftovers`] is the node's check: processes of the job user that look
//! like job processes and that no live job owns. Reported in the node's
//! health and `k2 compute nodes`; never killed by that check.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::strays::{self, Stray};

/// The env name every job process inherits (its value: the job id).
pub const JOB_ENV: &str = "K2_COMPUTE_JOB";

/// One process, as read from the process table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub pgid: i32,
    pub sid: i32,
    /// [`crate::util::process_start_time`]'s units.
    pub start: u64,
    /// Seconds since it started, when known.
    pub age_secs: Option<u64>,
    pub cwd: Option<String>,
    pub exe: Option<String>,
    pub argv: Vec<String>,
    /// `NAME=value` entries (empty when unreadable).
    pub env: Vec<String>,
}

impl ProcRow {
    /// Its command line for a log, cut short.
    pub fn cmd(&self) -> String {
        let mut cmd = if self.argv.is_empty() { self.exe.clone().unwrap_or_default() } else { self.argv.join(" ") };
        if cmd.len() > 200 {
            let mut cut = 200;
            while !cmd.is_char_boundary(cut) {
                cut -= 1;
            }
            cmd.truncate(cut);
            cmd.push('…');
        }
        cmd
    }

    /// The job id in its environment, when set.
    pub fn env_job(&self) -> Option<&str> {
        self.env.iter().find_map(|e| e.strip_prefix(JOB_ENV).and_then(|r| r.strip_prefix('=')))
    }

    fn stray(&self) -> Stray {
        Stray { pid: self.pid, start: self.start, cmd: self.cmd() }
    }
}

/// What marks one job's processes.
#[derive(Debug, Clone, Default)]
pub struct Mark {
    pub job_id: String,
    /// The job's folder (`jobs/<id>-g<gen>`).
    pub root: PathBuf,
    /// Its process group and session id (= the leader's pid); 0 when the
    /// node has none (a unit's job, an orphan's row).
    pub pgid: i32,
    /// The leader's start time: a live leader with another one means the
    /// id was reused, and the group rule is off.
    pub leader_start: Option<u64>,
}

/// Is this program never ended here (shared beyond one job, or the node's
/// own: holders and the egress proxy run the `k2-node` binary)?
pub fn spared(p: &ProcRow) -> bool {
    let name = |s: &str| Path::new(s).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    strays::spared(p.exe.as_deref(), &p.argv) || p.exe.as_deref().map(name).is_some_and(|n| n == "k2-node")
}

/// The group rule may be used: the leader is gone, or still the job's.
pub fn group_valid(m: &Mark, table: &[ProcRow]) -> bool {
    if m.pgid <= 1 {
        return false;
    }
    match (table.iter().find(|p| p.pid == m.pgid), m.leader_start) {
        (Some(leader), Some(want)) => leader.start == want,
        // A live leader whose start the node never recorded: only if the
        // kernel can't tell (the table says what it says).
        (Some(_), None) => true,
        (None, _) => true,
    }
}

/// Does `p` belong to the job `m`? Pure, for tests (`group_ok`: see
/// [`group_valid`]; `by_id`: its `K2_COMPUTE_JOB` counts too, for the
/// report's live jobs only, never for a kill).
pub fn of_job(p: &ProcRow, m: &Mark, roots: &[String], group_ok: bool, by_id: bool) -> bool {
    if group_ok && m.pgid > 1 && (p.pgid == m.pgid || p.sid == m.pgid) {
        return true;
    }
    if by_id && !m.job_id.is_empty() && p.env_job() == Some(m.job_id.as_str()) {
        return true;
    }
    let mut words = p.argv.clone();
    words.extend(p.env.iter().cloned());
    strays::belongs(p.cwd.as_deref(), p.exe.as_deref(), &words, roots)
}

/// This process's own (pid, process group); never a target.
fn own() -> (i32, i32) {
    // SAFETY: plain getters.
    unsafe { (libc::getpid(), libc::getpgrp()) }
}

/// The processes of `uid` that are the job's (the node itself, its group
/// and spared programs left out).
pub fn find_in(table: &[ProcRow], m: &Mark, uid: u32) -> Vec<ProcRow> {
    let roots = strays::roots(&m.root);
    let group_ok = group_valid(m, table);
    let (me, my_group) = own();
    table
        .iter()
        .filter(|p| p.uid == uid && p.pid > 1 && p.pid != me && p.pgid != my_group && !spared(p))
        .filter(|p| of_job(p, m, &roots, group_ok, false))
        .cloned()
        .collect()
}

/// Find and end the job's processes: SIGTERM, up to `grace`, SIGKILL.
/// Returns the lines for its log (empty: nothing was left).
pub fn sweep(m: &Mark, grace: Duration) -> Vec<String> {
    // SAFETY: a plain getter.
    let uid = unsafe { libc::geteuid() };
    let found = find_in(&snapshot(uid), m, uid);
    if found.is_empty() {
        return Vec::new();
    }
    let strays: Vec<Stray> = found.iter().map(ProcRow::stray).collect();
    strays::end(&strays, grace)
}

/// The job record's note for what a sweep ended (`None`: nothing).
pub fn note(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    // A note is at most 300 characters (`Outputs::capped`): as many whole
    // lines as fit, then how many more.
    let head = format!("killed {} leftover process(es) the job left running: ", lines.len());
    let mut shown: Vec<String> = Vec::new();
    for l in lines {
        let more = lines.len() - shown.len() - 1;
        let tail = if more > 0 { format!("; and {more} more (the job's log has all)") } else { String::new() };
        let cand = [shown.clone(), vec![l.clone()]].concat().join("; ");
        if shown.is_empty() || head.len() + cand.len() + tail.len() <= 300 {
            shown.push(l.chars().take(120).collect());
        } else {
            break;
        }
    }
    let more = lines.len() - shown.len();
    let tail = if more > 0 { format!("; and {more} more (the job's log has all)") } else { String::new() };
    Some(format!("{head}{}{tail}", shown.join("; ")))
}

/// A process the node check found (see [`leftovers`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub pid: i32,
    pub age_secs: Option<u64>,
    pub cmd: String,
    /// The `K2_COMPUTE_JOB` it carries, when it does.
    pub job_id: Option<String>,
}

/// Younger than this, a process may be a job's that is still starting.
pub const LEFTOVER_MIN_AGE_SECS: u64 = 120;

/// The node check: processes of `uid` that look like job processes (they
/// carry `K2_COMPUTE_JOB`, or refer to a path under one of `roots`, the
/// node's job-facing roots) and that none of `live` owns. Never the node
/// itself, its group or its descendants, spared programs, or a process
/// younger than `min_age_secs` (the node: [`LEFTOVER_MIN_AGE_SECS`]).
/// Pure over `table`, for tests.
pub fn leftovers_in(table: &[ProcRow], uid: u32, roots: &[PathBuf], live: &[Mark], me: (i32, i32), min_age_secs: u64) -> Vec<Leftover> {
    let node_roots: Vec<String> = roots.iter().flat_map(|r| strays::roots(r)).collect();
    let live: Vec<(Mark, Vec<String>, bool)> = live.iter().map(|m| (m.clone(), strays::roots(&m.root), group_valid(m, table))).collect();
    // The node's descendants (its git, toolchain installs, a Local job's
    // own tree while it runs).
    let mut mine: std::collections::HashSet<i32> = std::collections::HashSet::from([me.0]);
    loop {
        let before = mine.len();
        for p in table {
            if mine.contains(&p.ppid) {
                mine.insert(p.pid);
            }
        }
        if mine.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    for p in table {
        if p.uid != uid || p.pid <= 1 || mine.contains(&p.pid) || p.pgid == me.1 || spared(p) {
            continue;
        }
        if p.age_secs.is_none_or(|a| a < min_age_secs) {
            continue;
        }
        let mut words = p.argv.clone();
        words.extend(p.env.iter().cloned());
        let looks = p.env_job().is_some() || strays::belongs(p.cwd.as_deref(), p.exe.as_deref(), &words, &node_roots);
        if !looks || live.iter().any(|(m, r, g)| of_job(p, m, r, *g, true)) {
            continue;
        }
        out.push(Leftover { pid: p.pid, age_secs: p.age_secs, cmd: p.cmd(), job_id: p.env_job().map(str::to_string) });
    }
    out.sort_by_key(|l| std::cmp::Reverse(l.age_secs));
    out
}

/// [`leftovers_in`] over this machine's process table, for this uid.
pub fn leftovers(roots: &[PathBuf], live: &[Mark], min_age_secs: u64) -> Vec<Leftover> {
    // SAFETY: a plain getter.
    let uid = unsafe { libc::geteuid() };
    leftovers_in(&snapshot(uid), uid, roots, live, own(), min_age_secs)
}

// ── the process table ────────────────────────────────────────────────

/// Every process; cwd, exe, argv and env read only for `uid`'s (the rest
/// keep the ids, for the group rule's leader check).
pub fn snapshot(uid: u32) -> Vec<ProcRow> {
    imp::snapshot(uid)
}

/// `KERN_PROCARGS2` bytes → (argv, env): `argc`, the exec path, NUL
/// padding, `argc` strings, then the environment (and Apple's own
/// `key=value` strings after it; harmless here).
pub fn parse_procargs2_env(b: &[u8]) -> (Vec<String>, Vec<String>) {
    let argv = strays::parse_procargs2(b);
    if b.len() < 4 {
        return (argv, Vec::new());
    }
    let mut i = 4;
    while i < b.len() && b[i] != 0 {
        i += 1;
    }
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    let mut n = 0;
    while n < argv.len() && i < b.len() {
        let end = b[i..].iter().position(|c| *c == 0).map(|p| i + p).unwrap_or(b.len());
        i = end + 1;
        n += 1;
    }
    let mut env = Vec::new();
    while i < b.len() {
        let end = b[i..].iter().position(|c| *c == 0).map(|p| i + p).unwrap_or(b.len());
        if end > i {
            env.push(String::from_utf8_lossy(&b[i..end]).into_owned());
        }
        i = end + 1;
    }
    (argv, env)
}

/// A NUL-separated list (`/proc/<pid>/cmdline`, `environ`).
pub fn split_nul(b: &[u8]) -> Vec<String> {
    b.split(|c| *c == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// `/proc/<pid>/stat` → (ppid, pgid, sid, starttime).
pub fn parse_linux_stat(s: &str) -> Option<(i32, i32, i32, u64)> {
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some((f.get(1)?.parse().ok()?, f.get(2)?.parse().ok()?, f.get(3)?.parse().ok()?, f.get(19)?.parse().ok()?))
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn snapshot(uid: u32) -> Vec<ProcRow> {
        // SAFETY: a plain getter.
        let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let uptime: Option<f64> = std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next()?.parse().ok());
        let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
            let dir = e.path();
            let Ok(meta) = std::fs::metadata(&dir) else { continue };
            let Some((ppid, pgid, sid, start)) = std::fs::read_to_string(dir.join("stat")).ok().as_deref().and_then(parse_linux_stat) else { continue };
            let age_secs = uptime.map(|u| (u - start as f64 / tck as f64).max(0.0) as u64);
            let mut row = ProcRow { pid, ppid, uid: meta.uid(), pgid, sid, start, age_secs, ..Default::default() };
            if row.uid == uid {
                row.cwd = std::fs::read_link(dir.join("cwd")).ok().map(|p| p.display().to_string());
                row.exe = std::fs::read_link(dir.join("exe")).ok().map(|p| p.display().to_string().trim_end_matches(" (deleted)").to_string());
                row.argv = std::fs::read(dir.join("cmdline")).map(|b| split_nul(&b)).unwrap_or_default();
                row.env = std::fs::read(dir.join("environ")).map(|b| split_nul(&b)).unwrap_or_default();
            }
            out.push(row);
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::CStr;

    fn all_pids() -> Vec<i32> {
        let mut pids: Vec<libc::c_int> = vec![0; 16384];
        let bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
        // SAFETY: a buffer of `bytes` bytes.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, bytes) };
        if n <= 0 {
            return Vec::new();
        }
        pids.truncate(n as usize);
        pids.retain(|p| *p > 0);
        pids
    }

    fn cwd(pid: i32) -> Option<String> {
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        // SAFETY: a zeroed out-struct of the size we pass.
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
        if n != size {
            return None;
        }
        // SAFETY: vip_path is MAXPATHLEN bytes, NUL-terminated by the kernel.
        let p = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) };
        Some(p.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
    }

    fn exe(pid: i32) -> Option<String> {
        let mut buf = vec![0u8; 4096];
        // SAFETY: a buffer of the size we pass.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    fn procargs(pid: i32) -> Vec<u8> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a size query (null buffer).
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size];
        // SAFETY: a buffer of `size` bytes.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) } != 0 {
            return Vec::new();
        }
        buf.truncate(size);
        buf
    }

    pub fn snapshot(uid: u32) -> Vec<ProcRow> {
        let now = crate::util::now();
        let mut out = Vec::new();
        for pid in all_pids() {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            // SAFETY: a zeroed out-struct of the size we pass.
            let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
            if n != size {
                continue;
            }
            // SAFETY: a plain query; -1 when the pid is gone.
            let sid = unsafe { libc::getsid(pid) };
            let start = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec;
            let age_secs = Some((now - info.pbi_start_tvsec as i64).max(0) as u64);
            let mut row = ProcRow { pid, ppid: info.pbi_ppid as i32, uid: info.pbi_uid, pgid: info.pbi_pgid as i32, sid, start, age_secs, ..Default::default() };
            if row.uid == uid {
                row.cwd = cwd(pid);
                row.exe = exe(pid);
                let (argv, env) = parse_procargs2_env(&procargs(pid));
                row.argv = argv;
                row.env = env;
            }
            out.push(row);
        }
        out
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use super::*;
    pub fn snapshot(_uid: u32) -> Vec<ProcRow> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: i32, pgid: i32, sid: i32) -> ProcRow {
        ProcRow { pid, ppid: 1, uid: 501, pgid, sid, start: 10, age_secs: Some(600), exe: Some("/bin/sleep".into()), argv: vec!["sleep".into(), "3600".into()], ..Default::default() }
    }

    fn mark(pgid: i32) -> Mark {
        Mark { job_id: "j-1".into(), root: PathBuf::from("/var/k2node/jobs/j-1-g1"), pgid, leader_start: Some(10) }
    }

    /// The ways a process is the job's, and a neighbour that isn't.
    #[test]
    fn a_process_is_the_jobs_by_group_session_or_path() {
        let m = mark(500);
        let roots = strays::roots(&m.root);
        assert!(of_job(&row(501, 500, 500), &m, &roots, true, false), "its group");
        assert!(of_job(&row(502, 502, 500), &m, &roots, true, false), "setpgid: still its session");
        assert!(!of_job(&row(502, 502, 500), &m, &roots, false, false), "a reused group id: never by id");
        let mut env = row(503, 503, 503);
        env.env = vec!["PATH=/bin".into(), "K2_COMPUTE_JOB=j-1".into()];
        assert!(!of_job(&env, &m, &roots, true, false), "the id alone never marks a kill (another node may run it)");
        assert!(of_job(&env, &m, &roots, true, true), "but a live job's report owns it");
        env.env = vec!["K2_COMPUTE_JOB=j-10".into()];
        assert!(!of_job(&env, &m, &roots, true, true), "another job's id (a prefix) isn't this one");
        let mut home = row(504, 504, 504);
        home.env = vec!["HOME=/var/k2node/jobs/j-1-g1/home".into()];
        assert!(of_job(&home, &m, &roots, true, false), "setsid, K2_* stripped, HOME still the job's");
        let mut cwd = row(505, 505, 505);
        cwd.cwd = Some("/var/k2node/jobs/j-1-g1/src".into());
        assert!(of_job(&cwd, &m, &roots, true, false));
        assert!(!of_job(&row(506, 506, 506), &m, &roots, true, true), "elsewhere, other ids");
    }

    #[test]
    fn a_reused_leader_turns_the_group_rule_off() {
        let m = mark(500);
        assert!(group_valid(&m, &[row(1, 1, 1)]), "leader gone: the group can only be the job's");
        assert!(group_valid(&m, &[row(500, 500, 500)]), "leader alive with its own start");
        let mut other = row(500, 500, 500);
        other.start = 99;
        assert!(!group_valid(&m, &[other]), "another process with that pid");
        assert!(!group_valid(&mark(0), &[]), "no group recorded");
        assert!(!group_valid(&mark(1), &[]));
    }

    #[test]
    fn spared_programs_and_the_node_are_never_targets() {
        let mut s = row(600, 500, 500);
        s.exe = Some("/var/k2node/toolchains/bin/sccache".into());
        assert!(spared(&s));
        s.exe = Some("/usr/local/bin/k2-node".into());
        assert!(spared(&s), "holders and the egress proxy");
        assert!(!spared(&row(601, 500, 500)));
        let table = vec![s, row(601, 500, 500)];
        let found: Vec<i32> = find_in(&table, &mark(500), 501).iter().map(|p| p.pid).collect();
        assert_eq!(found, vec![601]);
        assert!(find_in(&table, &mark(500), 502).is_empty(), "another uid");
    }

    #[test]
    fn leftovers_skip_live_jobs_young_and_the_nodes_own() {
        let roots = vec![PathBuf::from("/var/k2node")];
        let me = (100, 100);
        let mut orphan = row(700, 700, 700);
        orphan.env = vec!["K2_COMPUTE_JOB=dead-job".into()];
        let mut live_one = row(701, 701, 701);
        live_one.env = vec!["K2_COMPUTE_JOB=j-1".into()];
        let mut young = orphan.clone();
        young.pid = 702;
        young.age_secs = Some(5);
        let mut child = orphan.clone();
        child.pid = 703;
        child.ppid = 100;
        let mut grandchild = orphan.clone();
        grandchild.pid = 704;
        grandchild.ppid = 703;
        let mut by_path = row(705, 705, 705);
        by_path.cwd = Some("/var/k2node/cache/pk/slots/0/target".into());
        let unrelated = row(706, 706, 706);
        let table = vec![orphan, live_one, young, child, grandchild, by_path, unrelated];
        let found = leftovers_in(&table, 501, &roots, &[mark(9000)], me, LEFTOVER_MIN_AGE_SECS);
        let pids: Vec<i32> = found.iter().map(|l| l.pid).collect();
        assert_eq!(pids, vec![700, 705], "{found:?}");
        assert_eq!(found[0].job_id.as_deref(), Some("dead-job"));
        assert_eq!(found[1].job_id, None);
    }

    #[test]
    fn procargs2_env_follows_argv() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/bin/sh\0\0\0/bin/sh\0-c\0K2_COMPUTE_JOB=j-1\0HOME=/h\0\0executable_path=/bin/sh\0");
        let (argv, env) = parse_procargs2_env(&b);
        assert_eq!(argv, vec!["/bin/sh", "-c"]);
        assert_eq!(env[..2], ["K2_COMPUTE_JOB=j-1".to_string(), "HOME=/h".to_string()]);
        let p = ProcRow { env, ..Default::default() };
        assert_eq!(p.env_job(), Some("j-1"));
    }

    #[test]
    fn linux_stat_reads_ids_after_the_comm() {
        let s = "4242 (a (b) c) S 1 4240 4239 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 987654 0 0";
        assert_eq!(parse_linux_stat(s), Some((1, 4240, 4239, 987654)));
        assert_eq!(split_nul(b"a\0b\0\0"), vec!["a", "b"]);
    }

    #[test]
    fn the_note_counts_and_cuts() {
        assert_eq!(note(&[]), None);
        let lines: Vec<String> = (0..5).map(|i| format!("pid {i} (TERM): sleep 3600")).collect();
        let n = note(&lines).unwrap();
        assert!(n.starts_with("killed 5 leftover process(es) the job left running: pid 0 (TERM): sleep 3600; pid 1"), "{n}");
        assert!(n.len() <= 300, "{n}");
        let long: Vec<String> = (0..9).map(|i| format!("pid {i} (KILL): {}", "x".repeat(100))).collect();
        let n = note(&long).unwrap();
        assert!(n.len() <= 300 && n.contains("more (the job's log has all)"), "{} {n}", n.len());
    }

    /// The real table: this test's own child is found by pid, group,
    /// session and start time; on Linux also by env, and a sweep of a job
    /// whose folder its TMPDIR names ends it although it left the group.
    ///
    /// Known macOS limit: `KERN_PROCARGS2` hides an Apple platform binary's
    /// environment, so the `/bin/sleep` child's env reads empty and the
    /// sweep can't tie it to the job: asserted as today's behaviour. The
    /// planned fix is tracking descendants by parent pid while a job runs;
    /// when it lands the macOS asserts fail and become the Linux ones.
    #[test]
    fn the_live_table_has_our_child() {
        use std::os::unix::process::CommandExt;
        let root = crate::util::temp_dir("tree").join("jobs").join("j-g1");
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let mut c = std::process::Command::new("/bin/sleep");
        c.arg("30").current_dir("/").env(JOB_ENV, "tree-test-job").env("TMPDIR", root.join("tmp"));
        // SAFETY: setsid only.
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = c.spawn().unwrap();
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(200));
        let uid = unsafe { libc::geteuid() };
        let t = snapshot(uid);
        let p = t.iter().find(|p| p.pid == pid).unwrap_or_else(|| panic!("pid {pid} not in the table"));
        let ids = (p.pgid, p.sid, p.uid);
        let start = (Some(p.start), crate::util::process_start_time(pid as u32));
        let env_job = p.env_job().map(str::to_string);
        let env = p.env.clone();
        let m = Mark { job_id: "tree-test-job".into(), root: root.clone(), pgid: 0, leader_start: None };
        let lines = sweep(&m, Duration::from_millis(500));
        let swept = lines.iter().any(|l| l.starts_with(&format!("pid {pid} (")));
        // Never leak the child (the sweep may have ended it already).
        let _ = child.kill();
        assert!(child.wait().is_ok());
        assert_eq!(ids, (pid, pid, uid));
        assert_eq!(start.0, start.1);
        #[cfg(target_os = "linux")]
        {
            assert_eq!(env_job.as_deref(), Some("tree-test-job"), "{env:?}");
            assert!(swept, "{lines:?}");
            assert!(lines.iter().any(|l| l.starts_with(&format!("pid {pid} (TERM)"))), "{lines:?}");
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(env_job, None, "known macOS limit: /bin/sleep's env is hidden (if this fails the limit is fixed: use the Linux assertions): {env:?}");
            assert!(!swept, "known macOS limit: the sweep can't tie it to the job: {lines:?}");
        }
    }
}
