//! `k2 publish run --dev` (prd-publish-visibility-and-dev-mode-v1 §8): the
//! parts of a dev server that don't touch the publish supervisor's live map:
//! flag parsing, the dev environment, caps (DM-5), the bind check (PV-4,
//! PA-5), the inotify warning (DM-7) and the idle clock (DM-6, PA-11).
//!
//! A dev service is an ordinary published service on the process lane. It
//! runs in the live workspace folder, never a copy (PA-7), and it isn't walled
//! (PA-8), which is why an agent's first `--dev` run waits for the owner.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// `--idle` default: 60 minutes (P-D7).
pub const IDLE_DEFAULT_SECS: i64 = 60 * 60;
/// `--idle` bounds: 5 minutes to 24 hours.
pub const IDLE_MIN_SECS: i64 = 5 * 60;
pub const IDLE_MAX_SECS: i64 = 24 * 60 * 60;
/// `--mem` default: 4 GiB.
pub const MEM_DEFAULT_MB: i64 = 4096;
/// `--mem` bounds: 256 MiB to 1 TiB.
const MEM_MIN_MB: i64 = 256;
const MEM_MAX_MB: i64 = 1024 * 1024;
/// `--cpus` default.
pub const CPUS_DEFAULT: f64 = 2.0;
/// `--cpus` bounds.
const CPUS_MIN: f64 = 0.1;
const CPUS_MAX: f64 = 1024.0;

/// DM-7: below this, a big `node_modules` tree can run out of watches and hot
/// reload silently stops.
pub const INOTIFY_MIN_WATCHES: u64 = 65_536;

/// What `ps` says about where a dev service runs (PA-7).
pub const LANE_LABEL: &str = "dev: live folder, not walled";

/// DM-4: printed when a dev server starts and again when it is shared.
pub const DEV_WARNING: &str = "A dev server shows more than a normal site: source maps, error pages with file paths, and anything the dev server evaluates. Whoever can open this site can see them.";

/// How a dev service was asked to run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DevSpec {
    /// Idle limit in seconds; `None` = `--idle off`.
    pub idle_secs: Option<i64>,
    pub mem_mb: i64,
    pub cpus: f64,
}

impl Default for DevSpec {
    fn default() -> Self {
        Self { idle_secs: Some(IDLE_DEFAULT_SECS), mem_mb: MEM_DEFAULT_MB, cpus: CPUS_DEFAULT }
    }
}

/// Parse `--idle`: `off`, or `30m`, `2h`, `90s`, `1h30m`, or bare seconds,
/// between 5 minutes and 24 hours.
pub fn parse_idle(raw: &str) -> Result<Option<i64>, String> {
    let t = raw.trim().to_ascii_lowercase();
    if t == "off" {
        return Ok(None);
    }
    let bad = || format!("--idle must be a duration from 5m to 24h (like 30m or 2h), or off; got '{}'", raw.trim());
    if t.is_empty() {
        return Err(bad());
    }
    let mut total: i64 = 0;
    if t.bytes().all(|b| b.is_ascii_digit()) {
        total = t.parse::<i64>().map_err(|_| bad())?;
    } else {
        let mut num = String::new();
        for c in t.chars() {
            if c.is_ascii_digit() {
                num.push(c);
                continue;
            }
            let n: i64 = num.parse().map_err(|_| bad())?;
            num.clear();
            let unit = match c {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                _ => return Err(bad()),
            };
            total = n.checked_mul(unit).and_then(|v| total.checked_add(v)).ok_or_else(bad)?;
        }
        if !num.is_empty() {
            return Err(bad());
        }
    }
    if !(IDLE_MIN_SECS..=IDLE_MAX_SECS).contains(&total) {
        return Err(bad());
    }
    Ok(Some(total))
}

/// Parse `--mem` to MiB: `4G`, `4GiB`, `512M`, `512MiB`.
pub fn parse_mem(raw: &str) -> Result<i64, String> {
    let t = raw.trim().to_ascii_lowercase();
    let bad = || format!("--mem must be an amount from 256M to 1024G (like 4G or 512M); got '{}'", raw.trim());
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(digits_end);
    let n: i64 = num.parse().map_err(|_| bad())?;
    let mb = match unit {
        "g" | "gb" | "gib" => n.checked_mul(1024).ok_or_else(bad)?,
        "m" | "mb" | "mib" => n,
        _ => return Err(bad()),
    };
    if !(MEM_MIN_MB..=MEM_MAX_MB).contains(&mb) {
        return Err(bad());
    }
    Ok(mb)
}

/// Parse `--cpus`: a number of CPUs (`2`, `0.5`).
pub fn parse_cpus(raw: &str) -> Result<f64, String> {
    let bad = || format!("--cpus must be a number of CPUs from 0.1 to 1024 (like 2 or 0.5); got '{}'", raw.trim());
    let n: f64 = raw.trim().parse().map_err(|_| bad())?;
    if !n.is_finite() || !(CPUS_MIN..=CPUS_MAX).contains(&n) {
        return Err(bad());
    }
    Ok(n)
}

/// DM-1: what a dev server gets on top of a service's environment. Its bind
/// address is loopback, so the gate is the only way in (the bind check still
/// verifies it, PA-5), and its hot-reload client gets the public address.
pub fn env_pairs(port: u16, public_host: Option<&str>) -> Vec<(&'static str, String)> {
    let mut out = vec![("HOST", "127.0.0.1".to_string()), ("PORT", port.to_string())];
    if let Some(h) = public_host.filter(|h| !h.is_empty()) {
        out.push(("K2_DEV_PUBLIC_HOST", h.to_string()));
        out.push(("K2_DEV_PUBLIC_PORT", "443".to_string()));
        out.push(("K2_DEV_PUBLIC_PROTOCOL", "wss".to_string()));
    }
    out
}

/// A dev server's systemd scope name. New each start: a stopping scope can
/// still hold the old name for a moment.
pub fn scope_unit(label: &str) -> String {
    format!("k2-dev-{label}-{}.scope", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// The `systemd-run` arguments that put `sh -c <cmd>` in a capped user scope.
/// `--scope` runs the command in place, so its pid (and process group) are the
/// child's own: `stop` and the live map work as for any service.
pub fn scope_args(unit: &str, mem_mb: i64, cpus: f64, cmd: &str) -> Vec<String> {
    vec![
        "--user".into(),
        "--scope".into(),
        "--quiet".into(),
        "--collect".into(),
        format!("--unit={unit}"),
        format!("--property=MemoryMax={mem_mb}M"),
        format!("--property=CPUQuota={}%", (cpus * 100.0).round() as i64),
        "--".into(),
        "sh".into(),
        "-c".into(),
        cmd.to_string(),
    ]
}

/// The environment `systemd-run --user` needs to reach this user's manager.
#[cfg(unix)]
pub fn user_manager_env() -> Vec<(&'static str, String)> {
    // SAFETY: getuid has no failure mode and touches no memory.
    let uid = unsafe { libc::getuid() };
    vec![
        ("XDG_RUNTIME_DIR", format!("/run/user/{uid}")),
        ("DBUS_SESSION_BUS_ADDRESS", format!("unix:path=/run/user/{uid}/bus")),
    ]
}

/// `systemd-run` when this box's user can have capped scopes, else why not.
/// It starts a real scope around `true`: the only check that proves it, as a
/// daemon under a system unit may have no user manager (PA-9).
#[cfg(target_os = "linux")]
pub fn user_scopes() -> Result<PathBuf, String> {
    let exe = ["/usr/bin/systemd-run", "/bin/systemd-run"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .ok_or_else(|| "systemd-run isn't on this box".to_string())?;
    let mut c = k2_core::bg_proc::command(&exe);
    c.args(["--user", "--scope", "--quiet", "--collect", "--", "true"])
        .envs(user_manager_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = c.spawn().map_err(|e| format!("systemd-run: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(exe),
            Ok(Some(_)) => return Err("this user has no systemd user manager (loginctl enable-linger turns it on)".into()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("systemd-run didn't answer in 5 seconds".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("systemd-run: {e}")),
        }
    }
}

/// Caps read back from a running process's cgroup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapsRead {
    pub mem_bytes: Option<u64>,
    pub cpus: Option<f64>,
}

/// The cgroup path of a process (`/proc/<pid>/cgroup`, cgroup v2's `0::` line).
pub fn parse_cgroup_path(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(|p| p.trim().to_string()).filter(|p| !p.is_empty())
}

/// `memory.max`'s value; `max` = no cap.
pub fn parse_memory_max(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// `cpu.max`'s `<quota> <period>`; `max …` = no cap.
pub fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut parts = text.split_whitespace();
    let quota: f64 = parts.next()?.parse().ok()?;
    let period: f64 = parts.next()?.parse().ok()?;
    (period > 0.0).then_some(quota / period)
}

/// `4 GiB, 2 CPUs`, what `ps` shows for caps K2 read back.
pub fn caps_label(r: CapsRead) -> String {
    let mem = r.mem_bytes.map(|b| {
        let mib = b / (1024 * 1024);
        if mib >= 1024 && mib % 1024 == 0 {
            format!("{} GiB", mib / 1024)
        } else {
            format!("{mib} MiB")
        }
    });
    let cpus = r.cpus.map(|c| {
        let shown = if (c - c.round()).abs() < 0.005 { format!("{}", c.round() as i64) } else { format!("{c:.2}") };
        format!("{shown} {}", if shown == "1" { "CPU" } else { "CPUs" })
    });
    match (mem, cpus) {
        (Some(m), Some(c)) => format!("{m}, {c}"),
        (Some(m), None) => format!("{m}, no CPU cap"),
        (None, Some(c)) => format!("no memory cap, {c}"),
        (None, None) => no_caps("the box's user scope doesn't apply them"),
    }
}

fn no_caps(why: &str) -> String {
    format!("no caps ({why})")
}

/// DM-5: what `ps` says about a dev process's caps. Only what the process's
/// own cgroup shows: never what was asked for.
pub fn caps_of(pid: Option<i64>) -> String {
    let Some(pid) = pid.filter(|p| *p > 0) else {
        return "not running".to_string();
    };
    #[cfg(target_os = "linux")]
    {
        let Some(path) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().as_deref().and_then(parse_cgroup_path) else {
            return no_caps("K2 can't read this process's cgroup");
        };
        if !path.contains("/k2-dev-") {
            return no_caps("user scopes aren't available on this box");
        }
        let dir = PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok();
        caps_label(CapsRead {
            mem_bytes: read("memory.max").as_deref().and_then(parse_memory_max),
            cpus: read("cpu.max").as_deref().and_then(parse_cpu_max),
        })
    }
    #[cfg(target_os = "macos")]
    {
        let _ = pid;
        no_caps("macOS has none")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        no_caps("this OS has none")
    }
}

/// DM-7: the warning when the box's inotify limit is too small.
pub fn inotify_warning_for(max_user_watches: u64) -> Option<String> {
    (max_user_watches < INOTIFY_MIN_WATCHES).then(|| {
        format!(
            "this box allows {max_user_watches} file watches (fs.inotify.max_user_watches); a big node_modules can use more, and then hot reload silently stops. \
             The owner can raise it: sudo sysctl fs.inotify.max_user_watches=524288"
        )
    })
}

/// DM-7 on this box. Linux only: `None` elsewhere, or when it can't be read.
pub fn inotify_warning() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let v = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok()?;
        return inotify_warning_for(v.trim().parse().ok()?);
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

// ── the bind check (PV-4, PA-5) ─────────────────────────────────────────

/// One listening socket: its address as shown to a person, and its IP
/// (`None` = a wildcard lsof printed as `*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub shown: String,
    pub ip: Option<IpAddr>,
    pub port: u16,
    pub inode: u64,
}

impl Listener {
    /// Reachable from outside this machine, past the gate.
    pub fn exposed(&self) -> bool {
        match self.ip {
            None => true,
            Some(ip) => !is_loopback(ip),
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|m| m.is_loopback()),
    }
}

fn show(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

/// The LISTEN rows of `/proc/net/tcp` or `/proc/net/tcp6`.
pub fn parse_proc_net_tcp(text: &str) -> Vec<Listener> {
    const LISTEN: &str = "0A";
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[3] != LISTEN {
            continue;
        }
        let Some((addr, port)) = f[1].split_once(':') else { continue };
        let Ok(port) = u16::from_str_radix(port, 16) else { continue };
        let Ok(inode) = f[9].parse::<u64>() else { continue };
        let Some(ip) = parse_proc_ip(addr) else { continue };
        out.push(Listener { shown: show(ip, port), ip: Some(ip), port, inode });
    }
    out
}

/// `/proc/net/tcp` writes each 32-bit word in host byte order (little-endian
/// on every platform K2 ships).
fn parse_proc_ip(hex: &str) -> Option<IpAddr> {
    let word = |s: &str| u32::from_str_radix(s, 16).ok().map(u32::swap_bytes);
    match hex.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(hex)?))),
        32 => {
            let mut o = [0u8; 16];
            for i in 0..4 {
                o[i * 4..i * 4 + 4].copy_from_slice(&word(&hex[i * 8..i * 8 + 8])?.to_be_bytes());
            }
            Some(IpAddr::V6(Ipv6Addr::from(o)))
        }
        _ => None,
    }
}

/// `lsof -F n`'s name lines (`n*:5173`, `n127.0.0.1:5173`, `n[::1]:5173`).
pub fn parse_lsof_names(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(name) = line.strip_prefix('n') else { continue };
        let Some((host, port)) = name.rsplit_once(':') else { continue };
        let Ok(port) = port.parse::<u16>() else { continue };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let ip = if host == "*" { None } else { host.parse::<IpAddr>().ok() };
        if host != "*" && ip.is_none() {
            continue;
        }
        out.push(Listener { shown: name.to_string(), ip, port, inode: 0 });
    }
    out
}

/// The listeners of a process group, plus whatever listens on `port`. `None`:
/// K2 couldn't look (never read as "nothing is exposed").
pub fn listeners(pgid: Option<i32>, port: u16) -> Option<Vec<Listener>> {
    #[cfg(target_os = "linux")]
    {
        let mut all = parse_proc_net_tcp(&std::fs::read_to_string("/proc/net/tcp").ok()?);
        if let Ok(v6) = std::fs::read_to_string("/proc/net/tcp6") {
            all.extend(parse_proc_net_tcp(&v6));
        }
        let inodes = pgid.map(group_socket_inodes).unwrap_or_default();
        return Some(all.into_iter().filter(|l| l.port == port || inodes.contains(&l.inode)).collect());
    }
    #[cfg(target_os = "macos")]
    {
        const LSOF: &str = "/usr/sbin/lsof";
        let run = |args: &[String]| -> Option<String> {
            let o = k2_core::bg_proc::command(LSOF).args(args).stdin(std::process::Stdio::null()).output().ok()?;
            // lsof exits 1 when nothing matched: that is an answer, not a failure.
            (o.status.success() || o.status.code() == Some(1)).then(|| String::from_utf8_lossy(&o.stdout).into_owned())
        };
        let mut out = parse_lsof_names(&run(&["-nP".into(), format!("-iTCP:{port}"), "-sTCP:LISTEN".into(), "-Fn".into()])?);
        if let Some(g) = pgid {
            out.extend(parse_lsof_names(&run(&["-nP".into(), "-a".into(), "-g".into(), g.to_string(), "-iTCP".into(), "-sTCP:LISTEN".into(), "-Fn".into()])?));
        }
        return Some(out);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (pgid, port);
        None
    }
}

/// The socket inodes every process of `pgid` holds open.
#[cfg(target_os = "linux")]
fn group_socket_inodes(pgid: i32) -> std::collections::HashSet<u64> {
    let mut out = std::collections::HashSet::new();
    let Ok(procs) = std::fs::read_dir("/proc") else { return out };
    for p in procs.flatten() {
        let name = p.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        if parse_stat_pgrp(&stat) != Some(pgid) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for fd in fds.flatten() {
            if let Some(i) = std::fs::read_link(fd.path()).ok().and_then(|t| parse_socket_link(&t.to_string_lossy())) {
                out.insert(i);
            }
        }
    }
    out
}

/// `/proc/<pid>/stat`'s process group: the third field after the `)` that
/// ends the (possibly spaced) command name.
pub fn parse_stat_pgrp(stat: &str) -> Option<i32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(2)?.parse().ok()
}

/// A `/proc/<pid>/fd/<n>` link `socket:[12345]`.
pub fn parse_socket_link(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

/// The exposed addresses (deduplicated, sorted), for `ps` and `visibility`.
pub fn exposed_addrs(ls: &[Listener]) -> Vec<String> {
    let mut v: Vec<String> = ls.iter().filter(|l| l.exposed()).map(|l| l.shown.clone()).collect();
    v.sort();
    v.dedup();
    v
}

/// The warning for a gated site whose program listens beyond loopback (PA-5).
pub fn bind_warning(label: &str, addrs: &[String]) -> String {
    format!(
        "{label} listens on {}: anyone who can reach this machine on that port gets the site without K2's check. Bind it to 127.0.0.1 and restart it.",
        addrs.join(", ")
    )
}

// ── the idle clock (DM-6, PA-11) ────────────────────────────────────────

fn clock() -> &'static Mutex<HashMap<String, Instant>> {
    static CLOCK: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    CLOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Something came through the front door for `label` just now.
pub fn touch(label: &str) {
    clock().lock().unwrap_or_else(|e| e.into_inner()).insert(label.to_string(), Instant::now());
}

/// When the front door last saw `label`, in this daemon's life.
pub fn last_touch(label: &str) -> Option<Instant> {
    clock().lock().unwrap_or_else(|e| e.into_inner()).get(label).copied()
}

/// Forget `label` (a service that stopped, or is no longer dev).
pub fn forget(label: &str) {
    clock().lock().unwrap_or_else(|e| e.into_inner()).remove(label);
}

/// Has a process that has been up since `since`, last seen at `last`, been
/// idle for `limit`? The process's own start counts as activity, so a server
/// nobody has visited yet still gets the whole limit.
pub fn idle_due(since: Instant, last: Option<Instant>, limit: Duration, now: Instant) -> bool {
    let from = match last {
        Some(l) if l > since => l,
        _ => since,
    };
    now.saturating_duration_since(from) >= limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_flags_take_durations_or_off_inside_the_bounds() {
        assert_eq!(parse_idle("off").unwrap(), None);
        assert_eq!(parse_idle(" OFF ").unwrap(), None);
        assert_eq!(parse_idle("60m").unwrap(), Some(3600));
        assert_eq!(parse_idle("5m").unwrap(), Some(300));
        assert_eq!(parse_idle("24h").unwrap(), Some(86_400));
        assert_eq!(parse_idle("1h30m").unwrap(), Some(5400));
        assert_eq!(parse_idle("600").unwrap(), Some(600));
        for bad in ["", "4m", "25h", "soon", "10x", "m", "5m3"] {
            let e = parse_idle(bad).expect_err(bad);
            assert!(e.contains("--idle"), "{e}");
        }
    }

    #[test]
    fn mem_and_cpu_flags_parse_and_refuse_what_is_out_of_range() {
        assert_eq!(parse_mem("4G").unwrap(), 4096);
        assert_eq!(parse_mem("4gib").unwrap(), 4096);
        assert_eq!(parse_mem("512M").unwrap(), 512);
        for bad in ["", "4", "100M", "2000G", "4T", "G"] {
            assert!(parse_mem(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_cpus("2").unwrap(), 2.0);
        assert_eq!(parse_cpus("0.5").unwrap(), 0.5);
        for bad in ["", "0", "-1", "NaN", "inf", "2000", "two"] {
            assert!(parse_cpus(bad).is_err(), "{bad}");
        }
    }

    /// DM-1: loopback always; the public address only when K2 knows it.
    #[test]
    fn the_dev_env_binds_loopback_and_names_the_public_address() {
        let e = env_pairs(5173, Some("web.rosson.k2.dev"));
        assert_eq!(
            e,
            vec![
                ("HOST", "127.0.0.1".to_string()),
                ("PORT", "5173".to_string()),
                ("K2_DEV_PUBLIC_HOST", "web.rosson.k2.dev".to_string()),
                ("K2_DEV_PUBLIC_PORT", "443".to_string()),
                ("K2_DEV_PUBLIC_PROTOCOL", "wss".to_string()),
            ]
        );
        assert_eq!(env_pairs(5173, None), vec![("HOST", "127.0.0.1".to_string()), ("PORT", "5173".to_string())]);
    }

    /// DM-5: the scope wraps the same `sh -c` a service runs, with both caps.
    #[test]
    fn the_scope_args_cap_memory_and_cpu_around_the_shell() {
        let a = scope_args("k2-dev-web-ab.scope", 4096, 1.5, "npm run dev");
        assert_eq!(&a[..5], ["--user", "--scope", "--quiet", "--collect", "--unit=k2-dev-web-ab.scope"]);
        assert!(a.contains(&"--property=MemoryMax=4096M".to_string()), "{a:?}");
        assert!(a.contains(&"--property=CPUQuota=150%".to_string()), "{a:?}");
        assert_eq!(&a[a.len() - 4..], ["--", "sh", "-c", "npm run dev"]);
        assert!(scope_unit("web").starts_with("k2-dev-web-"));
        assert_ne!(scope_unit("web"), scope_unit("web"), "a stopping scope can hold the old name");
    }

    #[test]
    fn caps_are_read_from_the_cgroup_files() {
        assert_eq!(parse_cgroup_path("0::/user.slice/app.slice/k2-dev-web-ab.scope\n").as_deref(), Some("/user.slice/app.slice/k2-dev-web-ab.scope"));
        assert_eq!(parse_cgroup_path("1:name=systemd:/x\n"), None);
        assert_eq!(parse_memory_max("4294967296\n"), Some(4_294_967_296));
        assert_eq!(parse_memory_max("max\n"), None);
        assert_eq!(parse_cpu_max("200000 100000\n"), Some(2.0));
        assert_eq!(parse_cpu_max("max 100000\n"), None);
        assert_eq!(caps_label(CapsRead { mem_bytes: Some(4 << 30), cpus: Some(2.0) }), "4 GiB, 2 CPUs");
        assert_eq!(caps_label(CapsRead { mem_bytes: Some(512 << 20), cpus: Some(1.0) }), "512 MiB, 1 CPU");
        assert_eq!(caps_label(CapsRead { mem_bytes: None, cpus: Some(0.5) }), "no memory cap, 0.50 CPUs");
        assert!(caps_label(CapsRead { mem_bytes: None, cpus: None }).starts_with("no caps"));
        assert_eq!(caps_of(None), "not running");
    }

    /// macOS says "no caps"; Linux, for a process outside a dev scope (this
    /// test binary), says the same rather than inventing any.
    #[test]
    fn a_process_outside_a_dev_scope_has_no_caps() {
        let c = caps_of(Some(std::process::id() as i64));
        assert!(c.starts_with("no caps"), "{c}");
    }

    #[test]
    fn the_inotify_warning_fires_below_the_limit_with_the_fix() {
        assert!(inotify_warning_for(65_536).is_none());
        let w = inotify_warning_for(8192).expect("warn");
        assert!(w.contains("8192") && w.contains("sysctl fs.inotify.max_user_watches"), "{w}");
    }

    const PROC_TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0\n\
   1: 00000000:1436 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1 0 100 0 0 10 0\n\
   2: 0100007F:1437 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 333 1 0 100 0 0 10 0\n";

    /// PA-5: a wildcard or LAN bind is exposed; loopback isn't; an established
    /// connection isn't a listener.
    #[test]
    fn proc_net_tcp_finds_listeners_and_which_are_exposed() {
        let ls = parse_proc_net_tcp(PROC_TCP);
        assert_eq!(ls.len(), 2, "{ls:?}");
        assert_eq!((ls[0].shown.as_str(), ls[0].port, ls[0].inode, ls[0].exposed()), ("127.0.0.1:5173", 5173, 111, false));
        assert_eq!((ls[1].shown.as_str(), ls[1].port, ls[1].exposed()), ("0.0.0.0:5174", 5174, true));
        assert_eq!(exposed_addrs(&ls), vec!["0.0.0.0:5174".to_string()]);
    }

    #[test]
    fn proc_net_tcp6_reads_loopback_wildcard_and_mapped_addresses() {
        let text = "header\n\
   0: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 11 1\n\
   1: 00000000000000000000000000000000:1436 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 22 1\n\
   2: 0000000000000000FFFF00000100007F:1437 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 33 1\n";
        let ls = parse_proc_net_tcp(text);
        assert_eq!(ls.len(), 3, "{ls:?}");
        assert_eq!((ls[0].shown.as_str(), ls[0].exposed()), ("[::1]:5173", false));
        assert_eq!((ls[1].shown.as_str(), ls[1].exposed()), ("[::]:5174", true));
        assert!(!ls[2].exposed(), "v4-mapped loopback: {:?}", ls[2]);
    }

    #[test]
    fn lsof_names_read_wildcards_and_addresses() {
        let ls = parse_lsof_names("p123\nf10\nn*:5173\nf11\nn127.0.0.1:5174\nf12\nn[::1]:5175\nf13\nn[::]:5176\n");
        assert_eq!(ls.len(), 4, "{ls:?}");
        assert!(ls[0].exposed(), "wildcard");
        assert!(!ls[1].exposed());
        assert!(!ls[2].exposed());
        assert!(ls[3].exposed());
        assert_eq!(exposed_addrs(&ls), vec!["*:5173".to_string(), "[::]:5176".to_string()]);
    }

    #[test]
    fn stat_and_fd_links_parse() {
        assert_eq!(parse_stat_pgrp("123 (node (dev) x) S 1 777 777 0"), Some(777));
        assert_eq!(parse_socket_link("socket:[12345]"), Some(12345));
        assert_eq!(parse_socket_link("/dev/null"), None);
    }

    /// PV-4's accept: a real program bound to 0.0.0.0 is reported, and a
    /// loopback one isn't, through the same call `ps` uses.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_program_bound_to_every_address_is_reported_and_loopback_is_not() {
        let wide = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("bind wide");
        let port = wide.local_addr().expect("addr").port();
        let got = listeners(None, port).expect("the check must be able to look");
        let exposed = exposed_addrs(&got);
        assert_eq!(exposed.len(), 1, "{got:?}");
        assert!(exposed[0].ends_with(&format!(":{port}")), "{exposed:?}");
        drop(wide);

        let local = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind local");
        let port = local.local_addr().expect("addr").port();
        let got = listeners(None, port).expect("the check must be able to look");
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(exposed_addrs(&got).is_empty(), "{got:?}");
    }

    /// DM-6: the process's start is activity; a visit moves the clock on.
    #[test]
    fn idle_counts_from_the_later_of_the_start_and_the_last_visit() {
        let t0 = Instant::now();
        let limit = Duration::from_secs(60);
        assert!(!idle_due(t0, None, limit, t0 + Duration::from_secs(59)));
        assert!(idle_due(t0, None, limit, t0 + Duration::from_secs(60)));
        let visit = t0 + Duration::from_secs(50);
        assert!(!idle_due(t0, Some(visit), limit, t0 + Duration::from_secs(100)));
        assert!(idle_due(t0, Some(visit), limit, t0 + Duration::from_secs(110)));
        // A visit before this process started (the last run) counts for nothing.
        let t1 = t0 + Duration::from_secs(500);
        assert!(!idle_due(t1, Some(visit), limit, t1 + Duration::from_secs(10)));
    }

    #[test]
    fn the_clock_remembers_touches_until_forgotten() {
        let label = format!("clock-{}", uuid::Uuid::new_v4().simple());
        assert!(last_touch(&label).is_none());
        touch(&label);
        assert!(last_touch(&label).is_some());
        forget(&label);
        assert!(last_touch(&label).is_none());
    }
}
