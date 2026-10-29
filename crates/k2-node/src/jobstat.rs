//! What one running job uses, and the node around it (0.46.7).
//!
//! - **CE-14** peak memory and CPU time of the whole job: a cgroup's
//!   `memory.peak` / `memory.current` and `cpu.stat` (a Linux leaf or job
//!   unit), else the job's process group from `ps` (macOS).
//! - **CE-10** (D4) the stall rule: a job is stalled only when it printed
//!   nothing **and** used under 1% of one CPU for the whole stall window.
//!   The same "silent and idle" rule the health stuck watch uses
//!   ([`silent_and_idle`]), here with the job's own CPU.
//! - **CE-19** the host stamp: load, CPUs, steal and other jobs at the
//!   job's start and end.
//!
//! Readings are taken off the async runtime (`spawn_blocking`); the
//! [`ResWatch`] that folds them in is plain data, so it is tested without
//! processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::results::{cpu_source, mem_source, HostSample, JobResources};

/// Where a job's usage is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A cgroup v2 dir (a Linux leaf, or a job unit's `ControlGroup`).
    Cgroup(PathBuf),
    /// A process group (no cgroup: macOS, or Linux without delegation).
    Group(i32),
    /// Nothing readable (a unit whose cgroup an older executor didn't say).
    None,
}

/// One process of a job's group, from `ps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcUse {
    pub pid: i32,
    pub rss_bytes: u64,
    pub cpu_ms: u64,
}

/// One reading of a [`Source`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    /// `memory.peak` (Linux ≥ 5.19).
    pub mem_peak: Option<u64>,
    /// `memory.current`, or the group's summed RSS.
    pub mem_now: Option<u64>,
    /// `cpu.stat` `usage_usec`.
    pub cpu_usec: Option<u64>,
    /// The group's processes (CPU per pid), when `ps` gave them.
    pub procs: Option<Vec<ProcUse>>,
    pub cgroup: bool,
}

/// `cpu.stat` text → `usage_usec`.
pub fn parse_cpu_stat(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

fn read_u64(p: &Path) -> Option<u64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// Read a cgroup dir (blocking, a few small sysfs files).
pub fn read_cgroup(dir: &Path) -> Reading {
    Reading {
        mem_peak: read_u64(&dir.join("memory.peak")),
        mem_now: read_u64(&dir.join("memory.current")),
        cpu_usec: std::fs::read_to_string(dir.join("cpu.stat")).ok().and_then(|t| parse_cpu_stat(&t)),
        procs: None,
        cgroup: true,
    }
}

/// `[[dd-]hh:]mm:ss[.cc]` → milliseconds (macOS prints hundredths, Linux
/// procps whole seconds).
pub fn parse_cpu_time_ms(t: &str) -> Option<u64> {
    let (days, rest) = match t.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, t),
    };
    let (whole, frac) = match rest.split_once('.') {
        Some((w, f)) => (w, f),
        None => (rest, ""),
    };
    let parts: Vec<u64> = whole.split(':').map(|p| p.parse::<u64>().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    let frac_ms = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take(3).collect();
        let n: u64 = digits.parse().ok()?;
        n * 10u64.pow(3 - digits.len() as u32)
    };
    Some((days * 86400 + secs) * 1000 + frac_ms)
}

/// `ps -A -o pid=,pgid=,rss=,time=` text → the processes of group `pgid`.
pub fn parse_ps_group(text: &str, pgid: i32) -> Vec<ProcUse> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(g), Some(rss), Some(t)) = (it.next(), it.next(), it.next(), it.next()) else { continue };
        if g.parse::<i32>().ok() != Some(pgid) {
            continue;
        }
        let (Ok(pid), Ok(kb), Some(ms)) = (pid.parse::<i32>(), rss.parse::<u64>(), parse_cpu_time_ms(t)) else { continue };
        out.push(ProcUse { pid, rss_bytes: kb * 1024, cpu_ms: ms });
    }
    out
}

/// `/proc/<pid>/stat` → (pgrp, CPU ticks, RSS pages). Its own user +
/// system time only (a reaped child's time is in its parent's `cutime`
/// too, and the child was counted under its own pid while it ran).
pub fn parse_proc_pid_stat(text: &str) -> Option<(i32, u64, u64)> {
    // `comm` may hold spaces and parens: the fields start after the last ')'.
    let rest = &text[text.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] = state (field 3), so field n is f[n - 3].
    let num = |n: usize| f.get(n - 3).and_then(|x| x.parse::<u64>().ok());
    let pgrp = f.get(2)?.parse::<i32>().ok()?;
    let ticks = num(14)? + num(15)?;
    Some((pgrp, ticks, num(24)?))
}

/// A job's group from `/proc` (Linux; blocking): 10 ms CPU resolution,
/// where procps' `time` has whole seconds.
fn proc_group(pgid: i32) -> Option<Vec<ProcUse>> {
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if tck <= 0 || page <= 0 {
        return None;
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(text) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        if let Some((g, ticks, pages)) = parse_proc_pid_stat(&text) {
            if g == pgid {
                out.push(ProcUse { pid, rss_bytes: pages * page as u64, cpu_ms: ticks * 1000 / tck as u64 });
            }
        }
    }
    Some(out)
}

/// A job's group from libproc (macOS): no `ps` spawn every second, and
/// CPU in nanoseconds. The task times are Mach absolute time (ticks on
/// Apple silicon), converted with the timebase.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc's mach timebase items point at the mach2 crate; the call is the same
fn libproc_group(pgid: i32) -> Option<Vec<ProcUse>> {
    let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
    if unsafe { libc::mach_timebase_info(&mut tb) } != 0 || tb.denom == 0 {
        return None;
    }
    let mut pids: Vec<libc::pid_t> = vec![0; 4096];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    if unsafe { libc::proc_listpgrppids(pgid, pids.as_mut_ptr() as *mut libc::c_void, bytes) } < 0 {
        return None;
    }
    let mut out = Vec::new();
    // The buffer was zeroed: every pid it holds is > 0.
    for pid in pids.into_iter().filter(|p| *p > 0) {
        let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, &mut ti as *mut _ as *mut libc::c_void, size) };
        if n != size {
            continue;
        }
        let ns = (ti.pti_total_user + ti.pti_total_system) as u128 * tb.numer as u128 / tb.denom as u128;
        out.push(ProcUse { pid, rss_bytes: ti.pti_resident_size, cpu_ms: (ns / 1_000_000) as u64 });
    }
    Some(out)
}

#[cfg(not(target_os = "macos"))]
fn libproc_group(_pgid: i32) -> Option<Vec<ProcUse>> {
    None
}

/// A job's group: `/proc` on Linux, libproc on macOS, else `ps` (blocking).
pub fn ps_group(pgid: i32) -> Option<Vec<ProcUse>> {
    if cfg!(target_os = "linux") {
        if let Some(v) = proc_group(pgid) {
            return Some(v);
        }
    }
    if let Some(v) = libproc_group(pgid).filter(|v| !v.is_empty()) {
        return Some(v);
    }
    let mut c = std::process::Command::new("/bin/ps");
    c.args(["-A", "-o", "pid=,pgid=,rss=,time="]).env_clear().env("LANG", "C");
    let text = crate::util::output_limited(c, Duration::from_secs(10)).ok()?;
    Some(parse_ps_group(&text, pgid))
}

/// One reading of `src` (blocking): a cgroup's files, or the group's
/// processes from the sampler (its summed RSS when it can't list them).
pub fn read_usage(src: &Source, sampler: &dyn crate::runner::RssSampler) -> Reading {
    match src {
        Source::Cgroup(d) => read_cgroup(d),
        Source::Group(pgid) => match sampler.group_procs(*pgid) {
            Some(p) => Reading { procs: Some(p), ..Default::default() },
            None => Reading { mem_now: sampler.group_rss(*pgid), ..Default::default() },
        },
        Source::None => Reading::default(),
    }
}

/// Silent and idle for the whole window: the one stuck/stall rule (the
/// health watch's `STUCK_SECS`, the run loop's stall timeout).
pub fn silent_and_idle(quiet: u64, idle: u64, window: u64) -> bool {
    quiet >= window && idle >= window
}

/// Folds readings into the job's usage and its last busy moment.
#[derive(Debug, Clone)]
pub struct ResWatch {
    pub source: Source,
    peak_sampled: Option<u64>,
    peak_kernel: Option<u64>,
    cgroup: bool,
    /// cgroup: the last `usage_usec`.
    cpu_usec: Option<u64>,
    /// ps: the highest CPU seen per pid (a process that exits keeps its
    /// last reading, so the sum doesn't drop).
    per_pid: HashMap<i32, u64>,
    /// The CPU total and time of the previous reading (µs, ms since start).
    prev: Option<(u64, u64)>,
    /// When the job last used at least 1% of one CPU (ms since start).
    pub last_busy_ms: u64,
    /// CPU of the last interval, in tenths of a percent of one CPU.
    pub last_rate_permille: u64,
    /// Two CPU readings arrived: the stall rule may use CPU.
    pub cpu_known: bool,
}

impl ResWatch {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            peak_sampled: None,
            peak_kernel: None,
            cgroup: false,
            cpu_usec: None,
            per_pid: HashMap::new(),
            prev: None,
            last_busy_ms: 0,
            last_rate_permille: 0,
            cpu_known: false,
        }
    }

    /// The job's total CPU so far, µs.
    fn cpu_total_usec(&self) -> Option<u64> {
        if self.cgroup {
            self.cpu_usec
        } else if self.per_pid.is_empty() {
            None
        } else {
            Some(self.per_pid.values().sum::<u64>() * 1000)
        }
    }

    /// Fold one reading taken `at_ms` after the job started. Returns the
    /// memory in use now (the macOS soft cap compares it).
    pub fn fold(&mut self, at_ms: u64, r: &Reading) -> Option<u64> {
        self.cgroup |= r.cgroup;
        if let Some(p) = r.mem_peak {
            self.peak_kernel = Some(self.peak_kernel.map_or(p, |k| k.max(p)));
        }
        let now = r.mem_now.or_else(|| r.procs.as_ref().map(|ps| ps.iter().map(|p| p.rss_bytes).sum()));
        if let Some(m) = now {
            self.peak_sampled = Some(self.peak_sampled.map_or(m, |k| k.max(m)));
        }
        if let Some(u) = r.cpu_usec {
            self.cpu_usec = Some(u);
        }
        if let Some(ps) = &r.procs {
            for p in ps {
                let e = self.per_pid.entry(p.pid).or_insert(0);
                *e = (*e).max(p.cpu_ms);
            }
        }
        if let Some(total) = self.cpu_total_usec() {
            if let Some((prev, prev_ms)) = self.prev {
                let wall_ms = at_ms.saturating_sub(prev_ms).max(1);
                let used = total.saturating_sub(prev);
                // ≥ 1% of one CPU over the interval: busy.
                self.last_rate_permille = used / wall_ms;
                if used.saturating_mul(100) >= wall_ms.saturating_mul(1000) {
                    self.last_busy_ms = at_ms;
                }
                self.cpu_known = true;
            }
            self.prev = Some((total, at_ms));
        }
        now
    }

    /// Stalled: silent for `stall_ms` and (when CPU is known) idle as long.
    pub fn stalled(&self, quiet_ms: u64, now_ms: u64, stall_ms: u64) -> bool {
        let idle = if self.cpu_known { now_ms.saturating_sub(self.last_busy_ms) } else { u64::MAX };
        silent_and_idle(quiet_ms, idle, stall_ms)
    }

    /// The job's resources (CE-14).
    pub fn resources(&self, mem_limit: Option<u64>) -> JobResources {
        let (peak, msrc) = match (self.peak_kernel, self.peak_sampled) {
            (Some(k), s) => (Some(k.max(s.unwrap_or(0))), mem_source::MEMORY_PEAK),
            (None, Some(s)) if self.cgroup => (Some(s), mem_source::CGROUP_SAMPLED),
            (None, Some(s)) => (Some(s), mem_source::RSS_SAMPLED),
            (None, None) => (None, mem_source::NONE),
        };
        let cpu = self.cpu_total_usec().map(|u| u / 1000);
        let csrc = match cpu {
            None => cpu_source::NONE,
            Some(_) if self.cgroup => cpu_source::CPU_STAT,
            Some(_) => cpu_source::PS_SAMPLED,
        };
        JobResources { peak_mem_bytes: peak, mem_source: msrc.into(), cpu_ms: cpu, cpu_source: csrc.into(), mem_limit_bytes: mem_limit }
    }
}

// ── host stamp (CE-19) ────────────────────────────────────────────────

/// The 1-minute load × 1000.
pub fn load1_milli() -> u64 {
    let mut l = [0f64; 3];
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    if n < 1 || !l[0].is_finite() || l[0] < 0.0 {
        return 0;
    }
    (l[0] * 1000.0).round() as u64
}

/// `/proc/stat`'s `cpu` line → (steal, total) jiffies.
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    if v.len() < 8 {
        return None;
    }
    // user nice system idle iowait irq softirq steal [guest guest_nice]:
    // guest time is already in user/nice.
    let total: u64 = v.iter().take(8).sum();
    Some((v[7], total))
}

/// Steal over `window`, tenths of a percent (Linux; `None` elsewhere).
/// Blocking: sleeps `window`.
pub fn steal_permille(window: Duration) -> Option<u32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let read = || std::fs::read_to_string("/proc/stat").ok().and_then(|t| parse_proc_stat(&t));
    let a = read()?;
    std::thread::sleep(window);
    let b = read()?;
    steal_between(a, b)
}

/// Steal between two `/proc/stat` readings, tenths of a percent.
pub fn steal_between(a: (u64, u64), b: (u64, u64)) -> Option<u32> {
    let total = b.1.checked_sub(a.1)?;
    let steal = b.0.checked_sub(a.0)?;
    if total == 0 {
        return Some(0);
    }
    Some(((steal * 1000) / total).min(1000) as u32)
}

/// One host stamp (blocking: the steal window).
pub fn host_sample(at: i64, cpus: u32, other_jobs: u32, window: Duration) -> HostSample {
    let steal = steal_permille(window);
    HostSample { at, load1_milli: load1_milli(), cpus, steal_permille: steal, other_jobs }
}

/// The log line: `host at start: load 2.31 on 16 CPUs, steal 0.4%, 2 other K2 jobs`.
pub fn host_line(when: &str, h: &HostSample) -> String {
    let steal = h.steal_permille.map(|s| format!(", steal {}.{}%", s / 10, s % 10)).unwrap_or_default();
    let others = match h.other_jobs {
        0 => "no other K2 job".to_string(),
        1 => "1 other K2 job".to_string(),
        n => format!("{n} other K2 jobs"),
    };
    format!("host at {when}: load {}.{:02} on {} CPUs{steal}, {others}", h.load1_milli / 1000, (h.load1_milli % 1000) / 10, h.cpus)
}

/// `25.3G` for the mem_cap and log lines.
pub fn gib(bytes: u64) -> String {
    format!("{:.1}G", bytes as f64 / (1u64 << 30) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_times_parse_with_and_without_hundredths() {
        assert_eq!(parse_cpu_time_ms("0:00.03"), Some(30));
        assert_eq!(parse_cpu_time_ms("1:02.5"), Some(62_500));
        assert_eq!(parse_cpu_time_ms("01:02:03"), Some(3_723_000));
        assert_eq!(parse_cpu_time_ms("2-00:00:01"), Some(172_801_000));
        assert_eq!(parse_cpu_time_ms("x"), None);
        let ps = "  10   10  2048 0:01.50\n  11   10  1024 0:00.25\n  12   99  4096 9:00.00\n junk\n";
        let g = parse_ps_group(ps, 10);
        assert_eq!(g, vec![ProcUse { pid: 10, rss_bytes: 2048 * 1024, cpu_ms: 1500 }, ProcUse { pid: 11, rss_bytes: 1024 * 1024, cpu_ms: 250 }]);
        assert_eq!(parse_cpu_stat("usage_usec 123456\nuser_usec 1\n"), Some(123_456));
        let stat = "4242 (my (odd) cmd) S 1 4240 4240 0 -1 4194304 10 0 0 0 150 50 7 3 20 0 1 0 100 1000000 256 18446744073709551615";
        assert_eq!(parse_proc_pid_stat(stat), Some((4240, 200, 256)));
        assert_eq!(parse_proc_pid_stat("garbage"), None);
        assert_eq!(parse_cpu_stat("nothing\n"), None);
    }

    fn cg(at_cpu_usec: u64, mem: u64, peak: Option<u64>) -> Reading {
        Reading { mem_peak: peak, mem_now: Some(mem), cpu_usec: Some(at_cpu_usec), procs: None, cgroup: true }
    }

    /// D4: silent and busy is not stalled; silent and idle is; unknown
    /// CPU keeps today's output-only rule.
    #[test]
    fn the_stall_rule_needs_silence_and_idleness() {
        let mut w = ResWatch::new(Source::Cgroup("/x".into()));
        assert!(w.stalled(5_000, 5_000, 1_000), "no CPU readings yet: output-only");
        // Busy: 900 ms of CPU per second.
        w.fold(0, &cg(0, 10, None));
        w.fold(1_000, &cg(900_000, 10, None));
        w.fold(2_000, &cg(1_800_000, 10, None));
        assert!(w.cpu_known);
        assert_eq!(w.last_busy_ms, 2_000);
        assert_eq!(w.last_rate_permille, 900);
        assert!(!w.stalled(2_000, 2_000, 1_000), "busy within the window");
        // Idle from here: 0.5% of a CPU is under the 1% bar.
        w.fold(3_000, &cg(1_805_000, 10, None));
        w.fold(4_000, &cg(1_810_000, 10, None));
        assert_eq!(w.last_busy_ms, 2_000);
        assert!(w.stalled(4_000, 4_000, 2_000), "silent and idle for the window");
        assert!(!w.stalled(1_500, 4_000, 2_000), "it printed recently");
        assert!(silent_and_idle(30, 30, 30) && !silent_and_idle(29, 30, 30) && !silent_and_idle(30, 29, 30));
    }

    /// CE-14: the kernel's peak wins; else the highest sample, named by
    /// how it was read; ps CPU never drops when a process exits.
    #[test]
    fn resources_report_peak_and_cpu_with_their_source() {
        let mut w = ResWatch::new(Source::Cgroup("/x".into()));
        w.fold(0, &cg(1_000, 5 << 20, Some(7 << 20)));
        w.fold(1_000, &cg(2_501_000, 3 << 20, Some(9 << 20)));
        let r = w.resources(Some(1 << 30));
        assert_eq!((r.peak_mem_bytes, r.mem_source.as_str(), r.cpu_ms, r.cpu_source.as_str()), (Some(9 << 20), "memory.peak", Some(2_501), "cpu.stat"));
        assert_eq!(r.mem_limit_bytes, Some(1 << 30));
        let mut old = ResWatch::new(Source::Cgroup("/x".into()));
        old.fold(0, &cg(0, 4 << 20, None));
        old.fold(1, &cg(0, 6 << 20, None));
        old.fold(2, &cg(0, 2 << 20, None));
        assert_eq!((old.resources(None).peak_mem_bytes, old.resources(None).mem_source.as_str()), (Some(6 << 20), "cgroup-sampled"));
        let mut g = ResWatch::new(Source::Group(10));
        let ps = |v: Vec<ProcUse>| Reading { procs: Some(v), ..Default::default() };
        g.fold(0, &ps(vec![ProcUse { pid: 10, rss_bytes: 100, cpu_ms: 10 }, ProcUse { pid: 11, rss_bytes: 300, cpu_ms: 500 }]));
        g.fold(1_000, &ps(vec![ProcUse { pid: 10, rss_bytes: 100, cpu_ms: 20 }]));
        let r = g.resources(None);
        assert_eq!((r.peak_mem_bytes, r.mem_source.as_str(), r.cpu_ms, r.cpu_source.as_str()), (Some(400), "rss-sampled", Some(520), "ps-sampled"));
        assert!(g.cpu_known && g.last_busy_ms == 1_000, "10 ms in 1 s is 1%: busy");
        let none = ResWatch::new(Source::None).resources(None);
        assert_eq!((none.peak_mem_bytes, none.mem_source.as_str(), none.cpu_ms, none.cpu_source.as_str()), (None, "none", None, "none"));
    }

    /// The group reader sees a real busy group (this process's own: its
    /// CPU grows while it spins, its RSS is above zero). Spins until the
    /// reader sees 100 ms more CPU (a loaded node gives less CPU than wall
    /// time), at most 20 s of wall time.
    #[test]
    fn the_group_reader_sees_this_process() {
        let me = unsafe { libc::getpgrp() };
        let pid = std::process::id() as i32;
        let read = || ps_group(me).expect("readable").into_iter().find(|p| p.pid == pid).expect("this process is in its group");
        let first = read();
        assert!(first.rss_bytes > 0, "{first:?}");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut now = first;
        while now.cpu_ms < first.cpu_ms + 100 && std::time::Instant::now() < deadline {
            let spin = std::time::Instant::now();
            while spin.elapsed() < Duration::from_millis(50) {
                std::hint::black_box(0u64);
            }
            now = read();
        }
        assert!(now.cpu_ms >= first.cpu_ms + 100, "the reader saw this process's CPU grow: {first:?} → {now:?}");
    }

    #[test]
    fn host_stamp_parts() {
        let a = parse_proc_stat("cpu  100 0 50 800 10 0 0 40 0 0\ncpu0 1 2 3\n").unwrap();
        assert_eq!(a, (40, 1000));
        assert_eq!(steal_between(a, (60, 1200)), Some(100), "20 of 200 jiffies = 10.0%");
        assert_eq!(steal_between(a, a), Some(0));
        assert_eq!(steal_between((5, 10), (1, 20)), None);
        assert_eq!(parse_proc_stat("cpu 1 2\n"), None);
        let h = HostSample { at: 1, load1_milli: 2_314, cpus: 16, steal_permille: Some(4), other_jobs: 2 };
        assert_eq!(host_line("start", &h), "host at start: load 2.31 on 16 CPUs, steal 0.4%, 2 other K2 jobs");
        let mac = HostSample { steal_permille: None, other_jobs: 0, ..h };
        assert_eq!(host_line("end", &mac), "host at end: load 2.31 on 16 CPUs, no other K2 job");
        let s = host_sample(7, 4, 1, Duration::from_millis(20));
        assert_eq!((s.at, s.cpus, s.other_jobs), (7, 4, 1));
        assert_eq!(s.steal_permille.is_some(), cfg!(target_os = "linux"), "Linux reports steal, macOS null");
        assert_eq!(gib(25 * (1 << 30) + (3 << 30) / 10), "25.3G");
    }
}
