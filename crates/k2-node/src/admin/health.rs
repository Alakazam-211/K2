//! Health facts (prd-k2-node-admin-v1 §10.1, S6), collected by the
//! unprivileged node; none need root. Parsers are pure and tested; the
//! collectors run fixed programs with absolute paths and a clean env.
//! A fact that can't be read is `None` plus an entry in `errors`.

use std::path::Path;
use std::time::Duration;

use k2_node_proto::health::{DiskFact, ExecutorFact, Health, OsFact, Power, SecurityFact, Sleep, Thermal, Updates};

use crate::node::Node;

/// Pending-update lists are cached this long (they hit the network).
pub const UPDATES_EVERY_SECS: i64 = 24 * 3600;

// ── pure parsers ─────────────────────────────────────────────────────

/// `pmset -g` → sleep settings.
pub fn parse_pmset(text: &str) -> Sleep {
    let mut s = Sleep::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        let Ok(v) = v.parse::<i64>() else { continue };
        match k {
            "sleep" => s.sleep = Some(v),
            "disksleep" => s.disksleep = Some(v),
            "displaysleep" => s.displaysleep = Some(v),
            "womp" => s.womp = Some(v),
            "autorestart" => s.autorestart = Some(v),
            "powernap" => s.powernap = Some(v),
            "standby" => s.standby = Some(v),
            _ => {}
        }
    }
    s
}

/// `pmset -g therm` → CPU speed limit (100 when no line says otherwise).
pub fn parse_therm(text: &str) -> Option<u32> {
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "CPU_Speed_Limit" {
                return v.trim().parse().ok();
            }
        }
    }
    if text.contains("No thermal warning level") || text.contains("No CPU power status") {
        return Some(100);
    }
    None
}

/// `apt-get -s upgrade` → (pending, security).
pub fn parse_apt_sim(text: &str) -> (u32, u32) {
    let inst: Vec<&str> = text.lines().filter(|l| l.starts_with("Inst ")).collect();
    let sec = inst.iter().filter(|l| l.contains("-security")).count();
    (inst.len() as u32, sec as u32)
}

/// `softwareupdate --list` → number of updates (`* Label:` lines).
pub fn parse_softwareupdate(text: &str) -> u32 {
    text.lines().filter(|l| l.trim_start().starts_with("* Label:")).count() as u32
}

/// `/proc/pressure/memory` → normal | warn | critical (avg10 of "some").
pub fn parse_psi(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.starts_with("some "))?;
    let avg10 = line.split_whitespace().find_map(|f| f.strip_prefix("avg10="))?;
    let v: f64 = avg10.parse().ok()?;
    Some(if v >= 40.0 { "critical" } else if v >= 10.0 { "warn" } else { "normal" }.into())
}

/// `/proc/meminfo` → swap used bytes.
pub fn parse_meminfo_swap(text: &str) -> Option<u64> {
    let kb = |k: &str| -> Option<u64> {
        text.lines().find(|l| l.starts_with(k))?.split_whitespace().nth(1)?.parse().ok()
    };
    Some(kb("SwapTotal:")?.saturating_sub(kb("SwapFree:")?) * 1024)
}

/// `/etc/os-release` → (name, version).
pub fn parse_os_release(text: &str) -> (String, String) {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(|v| v.trim_matches('"').to_string())
            .unwrap_or_default()
    };
    (get("NAME"), get("VERSION_ID"))
}

/// macOS memory pressure level sysctl value → word.
pub fn pressure_word(level: i64) -> Option<String> {
    match level {
        1 => Some("normal".into()),
        2 => Some("warn".into()),
        4 => Some("critical".into()),
        _ => None,
    }
}

// ── collectors ───────────────────────────────────────────────────────

fn run(program: &str, args: &[&str], secs: u64) -> Option<String> {
    let mut c = std::process::Command::new(program);
    c.args(args).env_clear().env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin").env("LANG", "C").current_dir("/").stdin(std::process::Stdio::null());
    let out = crate::util::output_limited(c, Duration::from_secs(secs)).ok()?;
    Some(out)
}

#[cfg(target_os = "macos")]
fn sysctl_raw(name: &str, buf: &mut [u8]) -> Option<usize> {
    let c = std::ffi::CString::new(name).ok()?;
    let mut len = buf.len();
    // SAFETY: valid name and buffer.
    let rc = unsafe { libc::sysctlbyname(c.as_ptr(), buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    (rc == 0).then_some(len)
}

pub fn uptime_secs() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let t = std::fs::read_to_string("/proc/uptime").ok()?;
        return t.split_whitespace().next()?.split('.').next()?.parse().ok();
    }
    #[cfg(target_os = "macos")]
    {
        let mut buf = [0u8; 16];
        sysctl_raw("kern.boottime", &mut buf)?;
        let secs = i64::from_ne_bytes(buf[0..8].try_into().ok()?);
        let now = crate::util::now();
        return (now >= secs).then(|| (now - secs) as u64);
    }
    #[allow(unreachable_code)]
    None
}

fn load_milli() -> Option<[u64; 3]> {
    let mut l = [0f64; 3];
    // SAFETY: valid 3-element buffer.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    (n == 3).then(|| [(l[0] * 1000.0) as u64, (l[1] * 1000.0) as u64, (l[2] * 1000.0) as u64])
}

fn swap_and_pressure() -> (Option<u64>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        let swap = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| parse_meminfo_swap(&t));
        let p = std::fs::read_to_string("/proc/pressure/memory").ok().and_then(|t| parse_psi(&t));
        return (swap, p);
    }
    #[cfg(target_os = "macos")]
    {
        // struct xsw_usage { u64 total, avail, used; u32 pagesize; bool encrypted }
        let mut buf = [0u8; 32];
        let swap = sysctl_raw("vm.swapusage", &mut buf).and_then(|_| Some(u64::from_ne_bytes(buf[16..24].try_into().ok()?)));
        let mut lb = [0u8; 4];
        let p = sysctl_raw("kern.memorystatus_vm_pressure_level", &mut lb).and_then(|_| pressure_word(i32::from_ne_bytes(lb) as i64));
        return (swap, p);
    }
    #[allow(unreachable_code)]
    (None, None)
}

fn os_fact() -> OsFact {
    let kernel = run("/usr/bin/uname", &["-r"], 5).or_else(|| run("/bin/uname", &["-r"], 5)).unwrap_or_default().trim().to_string();
    let (name, version) = if cfg!(target_os = "macos") {
        ("macOS".to_string(), run("/usr/bin/sw_vers", &["-productVersion"], 5).unwrap_or_default().trim().to_string())
    } else {
        std::fs::read_to_string("/etc/os-release").map(|t| parse_os_release(&t)).unwrap_or_default()
    };
    OsFact { name, version, kernel, arch: std::env::consts::ARCH.to_string() }
}

fn sleep_fact() -> Option<Sleep> {
    if cfg!(target_os = "macos") {
        return run("/usr/bin/pmset", &["-g"], 10).map(|t| parse_pmset(&t));
    }
    let out = run("/usr/bin/systemctl", &["is-enabled", "sleep.target", "suspend.target"], 10)
        .or_else(|| run("/bin/systemctl", &["is-enabled", "sleep.target", "suspend.target"], 10))?;
    let masked = out.lines().filter(|l| l.trim() == "masked").count() == 2;
    Some(Sleep { targets_masked: Some(masked), ..Default::default() })
}

fn thermal_fact() -> Option<Thermal> {
    if cfg!(target_os = "macos") {
        return run("/usr/bin/pmset", &["-g", "therm"], 10).map(|t| Thermal { cpu_speed_limit: parse_therm(&t), max_zone_milli_c: None });
    }
    let mut max: Option<i64> = None;
    for e in std::fs::read_dir("/sys/class/thermal").ok()?.flatten() {
        if !e.file_name().to_string_lossy().starts_with("thermal_zone") {
            continue;
        }
        if let Some(v) = std::fs::read_to_string(e.path().join("temp")).ok().and_then(|t| t.trim().parse::<i64>().ok()) {
            max = Some(max.map_or(v, |m| m.max(v)));
        }
    }
    max.map(|m| Thermal { cpu_speed_limit: None, max_zone_milli_c: Some(m) })
}

fn encryption_fact() -> Option<String> {
    if cfg!(target_os = "macos") {
        let t = run("/usr/bin/fdesetup", &["status"], 10)?;
        return Some(if t.contains("FileVault is On") { "on" } else if t.contains("FileVault is Off") { "off" } else { "unknown" }.into());
    }
    let t = std::fs::read_to_string("/etc/crypttab").unwrap_or_default();
    Some(if t.lines().any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')) { "luks" } else { "off" }.into())
}

fn updates_cache(home: &Path) -> std::path::PathBuf {
    home.join("run").join("updates.json")
}

/// The cached pending-updates list (never runs anything).
pub fn updates_cached(home: &Path) -> Option<Updates> {
    std::fs::read(updates_cache(home)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Refresh the list off the session's path when it's older than a day.
pub fn refresh_updates_in_background(node: &std::sync::Arc<Node>) {
    use std::sync::atomic::Ordering;
    let stale = updates_cached(&node.layout().home).is_none_or(|u| node.now() - u.checked_at >= UPDATES_EVERY_SECS);
    if !stale || node.updates_refreshing.swap(true, Ordering::SeqCst) {
        return;
    }
    let n = node.clone();
    tokio::task::spawn_blocking(move || {
        let _ = updates_fact(&n.layout().home, n.now());
        n.updates_refreshing.store(false, Ordering::SeqCst);
    });
}

/// Check pending updates now (slow, network) and cache the answer.
pub fn updates_fact(home: &Path, now: i64) -> Option<Updates> {
    let cache = updates_cache(home);
    let u = if cfg!(target_os = "macos") {
        let t = run("/usr/sbin/softwareupdate", &["--list"], 300)?;
        Updates { pending: Some(parse_softwareupdate(&t)), security: None, reboot_required: None, checked_at: now, source: "softwareupdate".into() }
    } else {
        let reboot = Some(Path::new("/var/run/reboot-required").exists());
        if Path::new("/usr/bin/apt-get").exists() {
            let t = run("/usr/bin/apt-get", &["-s", "-o", "Debug::NoLocking=1", "upgrade"], 120)?;
            let (p, s) = parse_apt_sim(&t);
            Updates { pending: Some(p), security: Some(s), reboot_required: reboot, checked_at: now, source: "apt".into() }
        } else if Path::new("/usr/bin/checkupdates").exists() {
            let t = run("/usr/bin/checkupdates", &[], 120).unwrap_or_default();
            Updates { pending: Some(t.lines().filter(|l| !l.trim().is_empty()).count() as u32), security: None, reboot_required: reboot, checked_at: now, source: "pacman".into() }
        } else if Path::new("/usr/bin/dnf").exists() {
            let t = run("/usr/bin/dnf", &["-q", "check-update"], 120).unwrap_or_default();
            Updates { pending: Some(t.lines().filter(|l| l.split_whitespace().count() == 3).count() as u32), security: None, reboot_required: reboot, checked_at: now, source: "dnf".into() }
        } else {
            Updates { pending: None, security: None, reboot_required: reboot, checked_at: now, source: "unknown".into() }
        }
    };
    if let Ok(b) = serde_json::to_vec(&u) {
        let _ = crate::util::atomic_write(&cache, &b, 0o644);
    }
    Some(u)
}

/// What the executor says (unreachable is a fact, not an error).
async fn executor_fact(node: &Node) -> ExecutorFact {
    match super::courier::ask(node, super::exec::LocalRequest::Status, None, Duration::from_secs(5)).await {
        Ok(super::exec::LocalResponse::Status(s)) => ExecutorFact {
            reachable: true,
            version: Some(s.version),
            remote_admin: Some(s.remote_admin),
            approval_key_fps: s.approval_key_fps,
            auto_update: Some(s.auto_update),
            last_update: s.last_update.and_then(|v| v.get("phase").and_then(|p| p.as_str()).map(|p| format!("{p} {}", v.get("to").and_then(|t| t.as_str()).unwrap_or("")))),
            wall_restart_pending: s.wall_restart_pending,
        },
        _ => ExecutorFact { reachable: false, remote_admin: Some(super::courier::local_level(node).as_str().into()), ..Default::default() },
    }
}

/// Everything (blocking collectors run off the async runtime).
pub async fn collect(node: &std::sync::Arc<Node>, refresh_updates: bool) -> Health {
    let home = node.layout().home.clone();
    let now = node.now();
    let blocking = tokio::task::spawn_blocking(move || {
        let (swap, pressure) = swap_and_pressure();
        let updates = if refresh_updates { updates_fact(&home, now) } else { updates_cached(&home) };
        (uptime_secs(), load_milli(), swap, pressure, os_fact(), sleep_fact(), thermal_fact(), encryption_fact(), updates)
    })
    .await;
    let (uptime, load, swap, pressure, os, sleep, thermal, enc, updates) = blocking.unwrap_or_default();
    let mut disks = Vec::new();
    for (name, p) in [("root", Path::new("/").to_path_buf()), ("node_home", node.layout().home.clone())] {
        let (free, total) = crate::sysinfo::disk(&p);
        disks.push(DiskFact { name: name.into(), path: p.display().to_string(), total_bytes: total, free_bytes: free });
    }
    let (has_battery, on_battery) = crate::sysinfo::power();
    let mut errors = std::collections::BTreeMap::new();
    if uptime.is_none() {
        errors.insert("uptime".into(), "unreadable".into());
    }
    if updates.is_none() {
        errors.insert("updates".into(), if refresh_updates { "check failed" } else { "not checked yet" }.into());
    }
    if sleep.is_none() {
        errors.insert("sleep".into(), "unreadable".into());
    }
    let executor = executor_fact(node).await;
    let mut h = Health {
        at: now,
        boot_id: node.opts.clock.boot_id(),
        uptime_secs: uptime,
        load_milli: load,
        cores: node.facts.cores,
        mem_total_bytes: node.facts.mem_bytes,
        mem_available_bytes: crate::sysinfo::mem_available(),
        swap_used_bytes: swap,
        memory_pressure: pressure,
        disks,
        updates,
        sleep,
        power: Power { has_battery, on_battery },
        thermal,
        disk_encryption: enc,
        os,
        security: Some(security_fact(node, executor.wall_restart_pending)),
        executor,
        stuck_jobs: node.stuck_jobs(),
        errors,
    };
    if !h.fits() {
        h.stuck_jobs.truncate(16);
        h.errors.clear();
    }
    h
}

/// The job wall and the open home folders (0.46.3, V15/V16): the node
/// doctor's warnings. Readable without root (homes are listed, never read).
pub fn security_fact(node: &Node, wall_restart_owed: bool) -> SecurityFact {
    let open: Vec<String> = crate::wall::open_homes_in(crate::wall::homes_root()).into_iter().map(|(p, m)| format!("{} ({m:o})", p.display())).collect();
    security_fact_with(&node.wall, &node.opts.user, open, wall_restart_owed)
}

/// What the owner reads when root's owed-restart flag is set (round 5, item 7).
pub const WALL_RESTART_OWED: &str = "a restart onto the job wall is owed; run wall.enable again (or approve the held restart)";

/// `wall_restart_owed`: root's flag (the executor's status): the wall's
/// files are in place, so the advice is the restart, not a setup run.
pub fn security_fact_with(wall: &crate::wall::Wall, node_user: &str, open_homes: Vec<String>, wall_restart_owed: bool) -> SecurityFact {
    use crate::wall::Wall;
    let mut notes = Vec::new();
    let job_user = match wall {
        Wall::JobUser { user } => Some(user.clone()),
        _ => Some(node_user.to_string()),
    };
    match wall {
        Wall::None { why } if wall_restart_owed => notes.push(format!("no job wall yet ({why}): jobs run as {node_user} until then; {WALL_RESTART_OWED}")),
        Wall::None { why } => notes.push(format!(
            "no job wall ({why}): jobs run as {node_user} and can read the node's key and state; run sudo k2-node setup (k2-node 0.46.3+) to add it"
        )),
        Wall::Seatbelt => notes.push("Seatbelt walls files and network; jobs share the node's user (a separate job user is later work)".into()),
        Wall::JobUser { .. } => {}
    }
    if !open_homes.is_empty() {
        notes.push(format!(
            "{} home folder(s) others can read{}: fix at the machine with sudo k2-node admin homes --fix, or approve homes.private",
            open_homes.len(),
            if wall.is_walled() { " (the wall keeps jobs out; other local users still get in)" } else { " (jobs can read them)" }
        ));
    }
    SecurityFact { wall: wall.label().to_string(), job_user, network_none: wall.network_none(), open_homes, notes }
}

/// The parts that count as "changed" (not the clock-driven ones).
pub fn same_facts(a: &Health, b: &Health) -> bool {
    let strip = |h: &Health| {
        let mut x = h.clone();
        x.at = 0;
        x.uptime_secs = None;
        x.load_milli = None;
        x.mem_available_bytes = None;
        x.swap_used_bytes = None;
        for d in &mut x.disks {
            d.free_bytes /= 1 << 30; // only GiB moves are news
        }
        x.thermal = None;
        x
    };
    strip(a) == strip(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The doctor's warnings: no wall says the key is readable; open homes
    /// say how to fix them, never fix them.
    #[test]
    fn security_fact_warns_about_no_wall_and_open_homes() {
        use crate::wall::Wall;
        let none = security_fact_with(&Wall::None { why: "no k2job".into() }, "k2node", vec!["/home/a (755)".into()], false);
        assert_eq!((none.wall.as_str(), none.network_none), ("none", false));
        assert!(none.notes.iter().any(|n| n.contains("can read the node's key") && n.contains("no k2job")), "{:?}", none.notes);
        assert!(none.notes.iter().any(|n| n.contains("admin homes --fix") && n.contains("jobs can read them")), "{:?}", none.notes);
        let walled = security_fact_with(&Wall::JobUser { user: "k2job".into() }, "k2node", vec![], false);
        assert_eq!((walled.wall.as_str(), walled.job_user.as_deref(), walled.network_none), ("job_user", Some("k2job"), true));
        assert!(walled.notes.is_empty(), "{:?}", walled.notes);
        let sb = security_fact_with(&Wall::Seatbelt, "_k2node", vec!["/Users/rosson (755)".into()], false);
        assert!(sb.notes.iter().any(|n| n.contains("the wall keeps jobs out")), "{:?}", sb.notes);
    }

    #[test]
    fn pmset_and_therm() {
        let t = "System-wide power settings:\nCurrently in use:\n standby              0\n Sleep On Power Button 1\n womp                 1\n autorestart          1\n powernap             0\n disksleep            10\n sleep                0 (sleep prevented by sharingd)\n displaysleep         10\n";
        let s = parse_pmset(t);
        assert_eq!((s.sleep, s.disksleep, s.womp, s.autorestart, s.powernap, s.standby, s.displaysleep), (Some(0), Some(10), Some(1), Some(1), Some(0), Some(0), Some(10)));
        assert_eq!(parse_therm("Note: No thermal warning level has been recorded\n"), Some(100));
        assert_eq!(parse_therm("CPU Power notify\n\tCPU_Scheduler_Limit \t= 100\n\tCPU_Available_CPUs \t= 8\n\tCPU_Speed_Limit \t= 72\n"), Some(72));
        assert_eq!(parse_therm("garbage"), None);
    }

    #[test]
    fn linux_parsers() {
        let apt = "Inst libssl3 [3.0.2-0ubuntu1.15] (3.0.2-0ubuntu1.16 Ubuntu:22.04/jammy-security [amd64])\nInst curl [7.81] (7.81.1 Ubuntu:22.04/jammy-updates [amd64])\nConf libssl3\n";
        assert_eq!(parse_apt_sim(apt), (2, 1));
        assert_eq!(parse_psi("some avg10=12.50 avg60=1.00 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").as_deref(), Some("warn"));
        assert_eq!(parse_psi("some avg10=0.00 avg60=0 avg300=0 total=0\n").as_deref(), Some("normal"));
        assert_eq!(parse_meminfo_swap("SwapTotal:  2048 kB\nSwapFree:   1024 kB\n"), Some(1024 * 1024));
        assert_eq!(parse_os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n"), ("Ubuntu".into(), "24.04".into()));
        assert_eq!(parse_softwareupdate("Software Update found:\n* Label: Command Line Tools for Xcode-16.0\n\tTitle: CLT\n* Label: Safari18.1\n"), 2);
        assert_eq!(pressure_word(4).as_deref(), Some("critical"));
        assert_eq!(pressure_word(9), None);
    }

    #[test]
    fn same_facts_ignores_clocks_but_not_settings() {
        let a = Health { at: 1, uptime_secs: Some(5), boot_id: "b".into(), sleep: Some(Sleep { sleep: Some(0), ..Default::default() }), ..Default::default() };
        let mut b = a.clone();
        b.at = 99;
        b.uptime_secs = Some(500);
        assert!(same_facts(&a, &b));
        b.sleep = Some(Sleep { sleep: Some(10), ..Default::default() });
        assert!(!same_facts(&a, &b), "sleep drift is news");
        let mut c = a.clone();
        c.boot_id = "rebooted".into();
        assert!(!same_facts(&a, &c));
    }

    #[test]
    fn this_machine_reports_real_facts() {
        assert!(uptime_secs().is_some_and(|u| u > 0), "uptime must read on Linux and macOS");
        assert!(load_milli().is_some());
        assert!(!os_fact().arch.is_empty());
    }
}
