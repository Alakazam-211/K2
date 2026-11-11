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
        JobResources { peak_mem_bytes: peak, mem_source: msrc.into(), cpu_ms: cpu, cpu_source: csrc.into(), mem_limit_bytes: mem_limit, gpu: None }
    }
}

// ── GPU use (GPU jobs S4) ─────────────────────────────────────────────

/// One reading of a GPU's sysfs counters (each `None` where the driver
/// has no such file).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuReading {
    pub busy_pct: Option<u32>,
    pub vram_used: Option<u64>,
    pub gtt_used: Option<u64>,
}

/// The GPU at `pci` under `sys` now.
pub fn read_gpu(sys: &Path, pci: &str) -> GpuReading {
    if !crate::gpu::valid_pci(pci) {
        return GpuReading::default();
    }
    let dev = sys.join("bus/pci/devices").join(pci);
    let n = |f: &str| std::fs::read_to_string(dev.join(f)).ok().and_then(|s| s.trim().parse::<u64>().ok());
    GpuReading { busy_pct: n("gpu_busy_percent").map(|b| b.min(100) as u32), vram_used: n("mem_info_vram_used"), gtt_used: n("mem_info_gtt_used") }
}

/// Folds a GPU job's readings: busy average and max, memory peaks over
/// the start (the whole device).
#[derive(Debug, Clone)]
pub struct GpuWatch {
    pub pci: String,
    start: GpuReading,
    busy_sum: u64,
    busy_n: u32,
    busy_max: Option<u32>,
    vram_max: Option<u64>,
    gtt_max: Option<u64>,
    samples: u32,
}

impl GpuWatch {
    /// `start`: the reading just before the job's command ran.
    pub fn new(pci: &str, start: GpuReading) -> Self {
        Self { pci: pci.to_string(), start, busy_sum: 0, busy_n: 0, busy_max: None, vram_max: None, gtt_max: None, samples: 0 }
    }

    pub fn fold(&mut self, r: &GpuReading) {
        self.samples += 1;
        if let Some(b) = r.busy_pct {
            self.busy_sum += u64::from(b);
            self.busy_n += 1;
            self.busy_max = Some(self.busy_max.map_or(b, |m| m.max(b)));
        }
        if let Some(v) = r.vram_used {
            self.vram_max = Some(self.vram_max.map_or(v, |m| m.max(v)));
        }
        if let Some(g) = r.gtt_used {
            self.gtt_max = Some(self.gtt_max.map_or(g, |m| m.max(g)));
        }
    }

    /// What the job's GPU did (`model` from the node's facts).
    pub fn usage(&self, model: &str) -> k2_node_proto::results::GpuUse {
        let over = |max: Option<u64>, start: Option<u64>| max.map(|m| m.saturating_sub(start.unwrap_or(0)));
        k2_node_proto::results::GpuUse {
            model: if model.is_empty() { self.pci.clone() } else { model.to_string() },
            pci: self.pci.clone(),
            busy_avg_pct: (self.busy_n > 0).then(|| ((self.busy_sum + u64::from(self.busy_n) / 2) / u64::from(self.busy_n)) as u32),
            busy_max_pct: self.busy_max,
            vram_peak_bytes: over(self.vram_max, self.start.vram_used),
            gtt_peak_bytes: over(self.gtt_max, self.start.gtt_used),
            samples: self.samples,
            source: k2_node_proto::results::gpu_source::SYSFS_DEVICE.into(),
            job_mem_peak_bytes: None,
            mem_limit_bytes: None,
        }
    }
}

/// GPU jobs S6: a GPU job's **own** GPU memory (root's `gpu_usage`, the
/// DRM fdinfo of its unit's processes) against its budget (`--gpu-mem`,
/// or the default on a unified device). The node stops the job once a
/// reading is over the budget by more than 10% (`failed/gpu_mem_cap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuMemWatch {
    pub cap: u64,
    pub peak: Option<u64>,
    pub samples: u32,
}

impl GpuMemWatch {
    pub fn new(cap: u64) -> Self {
        Self { cap, peak: None, samples: 0 }
    }

    /// One reading. `Some(detail)`: over the budget; stop the job.
    pub fn fold(&mut self, u: &k2_node_proto::gpu::GpuMemUsage) -> Option<String> {
        self.samples += 1;
        self.peak = Some(self.peak.map_or(u.bytes, |p| p.max(u.bytes)));
        k2_node_proto::gpu::over_cap(self.cap, u.bytes).then(|| k2_node_proto::gpu::gpu_mem_cap_detail(self.cap, u.bytes))
    }

    /// Into the footer's GPU use (a peak only when one was read).
    pub fn fill(&self, g: &mut k2_node_proto::results::GpuUse) {
        g.job_mem_peak_bytes = self.peak;
        g.mem_limit_bytes = Some(self.cap);
    }
}

/// The footer line: `GPU AMD Radeon Test APU: busy avg 37% (max 98%), VRAM
/// +0.5G, GTT +3.2G (the whole device, the host's own use included)`.
pub fn gpu_line(g: &k2_node_proto::results::GpuUse) -> String {
    let mut parts = Vec::new();
    match (g.busy_avg_pct, g.busy_max_pct) {
        (Some(a), Some(m)) => parts.push(format!("busy avg {a}% (max {m}%)")),
        _ => parts.push("busy % not reported by this driver".into()),
    }
    if let Some(v) = g.vram_peak_bytes {
        parts.push(format!("VRAM +{}", gib(v)));
    }
    if let Some(t) = g.gtt_peak_bytes {
        parts.push(format!("GTT +{}", gib(t)));
    }
    let mut line = format!("GPU {}: {} (the whole device over {} sample(s), the host's own use included)", g.model, parts.join(", "), g.samples);
    // S6: the job's own GPU memory, against its budget.
    match (g.job_mem_peak_bytes, g.mem_limit_bytes) {
        (Some(p), Some(c)) => line.push_str(&format!("; this job's own GPU memory: peak {} of --gpu-mem {}", gib(p), gib(c))),
        (None, Some(c)) => line.push_str(&format!("; this job's own GPU memory: not read (--gpu-mem {})", gib(c))),
        _ => {}
    }
    line
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

/// What else a host stamp looks at (GPU jobs S2): a foreign lock held,
/// and the job's GPU (its PCI address) for `gpu_busy_percent`.
#[derive(Debug, Clone, Default)]
pub struct HostExtra {
    pub foreign_lock: bool,
    pub gpu_pci: Option<String>,
}

/// One host stamp (blocking: the steal window).
pub fn host_sample(at: i64, cpus: u32, other_jobs: u32, extra: &HostExtra, window: Duration) -> HostSample {
    let steal = steal_permille(window);
    let gpu = extra.gpu_pci.as_deref().and_then(|p| gpu_busy_pct(Path::new(crate::gpu::SYS), p));
    let load = load1_milli();
    let quiet = k2_node_proto::results::quiet_why(load, cpus, other_jobs, extra.foreign_lock, gpu).is_none();
    HostSample { at, load1_milli: load, cpus, steal_permille: steal, other_jobs, foreign_lock: extra.foreign_lock, gpu_busy_pct: gpu, quiet: Some(quiet) }
}

/// The log line: `host at start: load 2.31 on 16 CPUs, steal 0.4%, 2 other
/// K2 jobs, quiet host: no (2 other K2 jobs)`. A GPU job's line also says
/// how busy its GPU was (the whole device).
pub fn host_line(when: &str, h: &HostSample) -> String {
    let steal = h.steal_permille.map(|s| format!(", steal {}.{}%", s / 10, s % 10)).unwrap_or_default();
    let others = match h.other_jobs {
        0 => "no other K2 job".to_string(),
        1 => "1 other K2 job".to_string(),
        n => format!("{n} other K2 jobs"),
    };
    let gpu = h.gpu_busy_pct.map(|b| format!(", GPU {b}% busy")).unwrap_or_default();
    let quiet = match h.quiet {
        None => String::new(),
        Some(_) => match k2_node_proto::results::quiet_why(h.load1_milli, h.cpus, h.other_jobs, h.foreign_lock, h.gpu_busy_pct) {
            None => ", quiet host: yes".into(),
            Some(w) => format!(", quiet host: no ({w})"),
        },
    };
    format!("host at {when}: load {}.{:02} on {} CPUs{steal}, {others}{gpu}{quiet}", h.load1_milli / 1000, (h.load1_milli % 1000) / 10, h.cpus)
}

/// `gpu_busy_percent` of the GPU at `pci` under `sys` (amdgpu reports it;
/// Intel's i915/xe don't: `None`).
pub fn gpu_busy_pct(sys: &Path, pci: &str) -> Option<u32> {
    if !crate::gpu::valid_pci(pci) {
        return None;
    }
    std::fs::read_to_string(sys.join("bus/pci/devices").join(pci).join("gpu_busy_percent")).ok()?.trim().parse().ok()
}

/// How long `--exclusive --gpu` waits for a quiet host before it starts
/// anyway (GPU jobs S2).
pub const QUIET_WAIT: Duration = Duration::from_secs(300);
/// How often it looks.
pub const QUIET_POLL: Duration = Duration::from_secs(5);

/// The outcome of [`wait_quiet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietWait {
    /// Why it still wasn't quiet at the end (`None`: it was).
    pub not_quiet: Option<String>,
    /// How long it waited.
    pub waited: Duration,
}

/// `--exclusive --gpu` (GPU jobs S2): wait up to `max` for the load under
/// 0.25 × `cpus` and every GPU under 5% busy, looking every `poll`. Never
/// a hang: at `max` it returns why the host still isn't quiet. `read`
/// gives (load1 × 1000, the GPUs' busy percents); other K2 jobs and locks
/// aren't asked here (an exclusive job already has the node to itself).
pub async fn wait_quiet(mut read: impl FnMut() -> (u64, Vec<u32>), cpus: u32, max: Duration, poll: Duration) -> QuietWait {
    let t0 = tokio::time::Instant::now();
    loop {
        let (load, gpus) = read();
        let busiest = gpus.iter().copied().max();
        let why = k2_node_proto::results::quiet_why(load, cpus, 0, false, busiest);
        let waited = t0.elapsed();
        if why.is_none() || waited >= max {
            return QuietWait { not_quiet: why, waited };
        }
        tokio::time::sleep(poll.min(max.saturating_sub(waited)).max(Duration::from_millis(1))).await;
    }
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
        let h = HostSample { at: 1, load1_milli: 2_314, cpus: 16, steal_permille: Some(4), other_jobs: 2, ..Default::default() };
        assert_eq!(host_line("start", &h), "host at start: load 2.31 on 16 CPUs, steal 0.4%, 2 other K2 jobs", "an older node's stamp (no quiet field)");
        let mac = HostSample { steal_permille: None, other_jobs: 0, ..h.clone() };
        assert_eq!(host_line("end", &mac), "host at end: load 2.31 on 16 CPUs, no other K2 job");
        let s = host_sample(7, 4, 1, &HostExtra::default(), Duration::from_millis(20));
        assert_eq!((s.at, s.cpus, s.other_jobs, s.quiet), (7, 4, 1, Some(false)), "another K2 job: never quiet");
        assert_eq!(s.steal_permille.is_some(), cfg!(target_os = "linux"), "Linux reports steal, macOS null");
        assert_eq!(gib(25 * (1 << 30) + (3 << 30) / 10), "25.3G");
    }

    /// GPU jobs S2: the stamp says whether the host was quiet and why not
    /// (K2 jobs, a foreign lock, load ≥ 0.25 × CPUs, the GPU ≥ 5% busy).
    #[test]
    fn host_line_says_quiet_host() {
        let quiet = HostSample { at: 1, load1_milli: 1_000, cpus: 16, steal_permille: Some(0), other_jobs: 0, quiet: Some(true), ..Default::default() };
        assert_eq!(host_line("start", &quiet), "host at start: load 1.00 on 16 CPUs, steal 0.0%, no other K2 job, quiet host: yes");
        let gpu = HostSample { gpu_busy_pct: Some(2), ..quiet.clone() };
        assert_eq!(host_line("start", &gpu), "host at start: load 1.00 on 16 CPUs, steal 0.0%, no other K2 job, GPU 2% busy, quiet host: yes");
        let loud = HostSample { load1_milli: 4_000, other_jobs: 1, foreign_lock: true, gpu_busy_pct: Some(41), quiet: Some(false), ..quiet };
        assert_eq!(
            host_line("end", &loud),
            "host at end: load 4.00 on 16 CPUs, steal 0.0%, 1 other K2 job, GPU 41% busy, quiet host: no (1 other K2 job, a foreign lock is held, load 4.00 ≥ 4.00, GPU 41% busy)"
        );
        let j = serde_json::to_string(&HostSample { at: 1, load1_milli: 1, cpus: 1, steal_permille: None, other_jobs: 0, ..Default::default() }).unwrap();
        assert!(!j.contains("quiet") && !j.contains("gpu") && !j.contains("foreign"), "an older reader's JSON: {j}");
    }

    /// GPU jobs S4: busy average and max, and memory peaks over the value
    /// at the start (the whole device), folded from sysfs readings.
    #[test]
    fn gpu_watch_folds_busy_and_memory_over_the_start() {
        let d = crate::util::temp_dir("gpu-use");
        let dev = d.join("bus/pci/devices/0000:c4:00.0");
        std::fs::create_dir_all(&dev).unwrap();
        let write = |busy: u64, vram: u64, gtt: u64| {
            std::fs::write(dev.join("gpu_busy_percent"), format!("{busy}\n")).unwrap();
            std::fs::write(dev.join("mem_info_vram_used"), format!("{vram}\n")).unwrap();
            std::fs::write(dev.join("mem_info_gtt_used"), format!("{gtt}\n")).unwrap();
        };
        write(3, 100 << 20, 1 << 30);
        let mut w = GpuWatch::new("0000:c4:00.0", read_gpu(&d, "0000:c4:00.0"));
        for (b, v, g) in [(40, 300 << 20, 3 << 30), (98, 612 << 20, 4 << 30), (10, 200 << 20, 2 << 30)] {
            write(b, v, g);
            w.fold(&read_gpu(&d, "0000:c4:00.0"));
        }
        let u = w.usage("AMD Radeon Test APU");
        assert_eq!((u.busy_avg_pct, u.busy_max_pct, u.samples), (Some(49), Some(98), 3));
        assert_eq!((u.vram_peak_bytes, u.gtt_peak_bytes), (Some(512 << 20), Some(3 << 30)), "peak minus the start");
        assert_eq!(u.source, "sysfs-device");
        assert_eq!(gpu_line(&u), "GPU AMD Radeon Test APU: busy avg 49% (max 98%), VRAM +0.5G, GTT +3.0G (the whole device over 3 sample(s), the host's own use included)");
        // Intel: no counters, still a line that says so.
        let mut i = GpuWatch::new("0000:00:02.0", read_gpu(&d, "0000:00:02.0"));
        i.fold(&read_gpu(&d, "0000:00:02.0"));
        let u = i.usage("");
        assert_eq!((u.model.as_str(), u.busy_avg_pct, u.vram_peak_bytes), ("0000:00:02.0", None, None));
        assert!(gpu_line(&u).contains("busy % not reported by this driver"), "{}", gpu_line(&u));
        // An older reader's JSON: no gpu key without a GPU.
        let r = ResWatch::new(Source::None).resources(Some(1));
        assert!(!serde_json::to_string(&r).unwrap().contains("gpu"));
    }

    /// S6: the watch keeps the peak, says nothing within the budget plus
    /// 10% and stops the job above it; the footer names the job's own peak.
    #[test]
    fn gpu_mem_watch_stops_only_over_the_budget() {
        use k2_node_proto::gpu::GpuMemUsage;
        let cap = 2u64 << 30;
        let mut w = GpuMemWatch::new(cap);
        let at = |b: u64| GpuMemUsage { bytes: b, ..Default::default() };
        assert_eq!(w.fold(&at(1 << 30)), None);
        assert_eq!(w.fold(&at(cap + cap / 10)), None, "within the 10%");
        assert_eq!(w.peak, Some(cap + cap / 10));
        let d = w.fold(&at(3 << 30)).expect("over the budget: stop");
        assert!(d.contains("3.0G") && d.contains("--gpu-mem is 2.0G"), "{d}");
        assert_eq!((w.peak, w.samples), (Some(3 << 30), 3));
        let mut g = GpuWatch::new("0000:c4:00.0", GpuReading::default()).usage("AMD Radeon Test APU");
        w.fill(&mut g);
        assert_eq!((g.job_mem_peak_bytes, g.mem_limit_bytes), (Some(3 << 30), Some(cap)));
        assert!(gpu_line(&g).ends_with("; this job's own GPU memory: peak 3.0G of --gpu-mem 2.0G"), "{}", gpu_line(&g));
        g.job_mem_peak_bytes = None;
        assert!(gpu_line(&g).ends_with("not read (--gpu-mem 2.0G)"), "{}", gpu_line(&g));
    }

    #[test]
    fn gpu_busy_reads_sysfs() {
        let d = crate::util::temp_dir("gpu-busy");
        let dev = d.join("bus/pci/devices/0000:c4:00.0");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("gpu_busy_percent"), "37\n").unwrap();
        assert_eq!(gpu_busy_pct(&d, "0000:c4:00.0"), Some(37));
        assert_eq!(gpu_busy_pct(&d, "0000:c5:00.0"), None, "Intel: no file");
        assert_eq!(gpu_busy_pct(&d, "../../etc"), None);
    }

    /// GPU jobs S2: `--exclusive --gpu` waits for a quiet host, and at the
    /// limit starts anyway with the reason (never a hang).
    #[tokio::test]
    async fn wait_quiet_waits_then_gives_up_with_why() {
        let mut n = 0;
        let w = wait_quiet(
            || {
                n += 1;
                if n < 3 { (8_000, vec![60]) } else { (500, vec![1]) }
            },
            16,
            Duration::from_secs(5),
            Duration::from_millis(5),
        )
        .await;
        assert_eq!(w.not_quiet, None, "quiet on the third look");
        assert_eq!(n, 3);
        let w = wait_quiet(|| (8_000, vec![60]), 16, Duration::from_millis(40), Duration::from_millis(5)).await;
        assert_eq!(w.not_quiet.as_deref(), Some("load 8.00 ≥ 4.00, GPU 60% busy"));
        assert!(w.waited >= Duration::from_millis(40) && w.waited < Duration::from_secs(5), "{:?}", w.waited);
    }
}
