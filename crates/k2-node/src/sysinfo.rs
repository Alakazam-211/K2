//! Machine facts: boot id, cores, memory, disk, battery, VM, tools.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// This boot's id (Linux `/proc/sys/kernel/random/boot_id`, macOS
/// `kern.bootsessionuuid`). Falls back to the boot time.
pub fn boot_id() -> String {
    if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    for key in ["kern.bootsessionuuid", "kern.boottime"] {
        if let Ok(out) = std::process::Command::new("/usr/sbin/sysctl").args(["-n", key]).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !s.is_empty() {
                return s;
            }
        }
    }
    "unknown-boot".to_string()
}

pub fn cores() -> u64 {
    std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(1)
}

fn sysctl_u64(key: &str) -> Option<u64> {
    let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", key]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn meminfo(field: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    s.lines().find(|l| l.starts_with(field)).and_then(|l| {
        l.split_whitespace().nth(1).and_then(|n| n.parse::<u64>().ok()).map(|kb| kb * 1024)
    })
}

pub fn mem_total() -> u64 {
    meminfo("MemTotal:").or_else(|| sysctl_u64("hw.memsize")).unwrap_or(0)
}

/// Memory the system says is available, when it can say (Linux).
pub fn mem_available() -> Option<u64> {
    meminfo("MemAvailable:")
}

/// `(free, total)` bytes of the filesystem holding `path`.
pub fn disk(path: &Path) -> (u64, u64) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()) else {
        return (0, 0);
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (0, 0);
    }
    let frsize = st.f_frsize as u64;
    (st.f_bavail as u64 * frsize, st.f_blocks as u64 * frsize)
}

/// `(has_battery, on_battery)`.
pub fn power() -> (bool, bool) {
    if cfg!(target_os = "macos") {
        let Ok(out) = std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]).output() else {
            return (false, false);
        };
        let s = String::from_utf8_lossy(&out.stdout);
        return (s.contains("InternalBattery"), s.contains("'Battery Power'"));
    }
    let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
        return (false, false);
    };
    let (mut has_batt, mut ac_online, mut has_ac) = (false, false, false);
    for e in dir.flatten() {
        let p = e.path();
        let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        match ty.trim() {
            "Battery" => has_batt = true,
            "Mains" | "USB" => {
                has_ac = true;
                if std::fs::read_to_string(p.join("online")).unwrap_or_default().trim() == "1" {
                    ac_online = true;
                }
            }
            _ => {}
        }
    }
    (has_batt, has_batt && has_ac && !ac_online)
}

pub fn is_vm() -> bool {
    if cfg!(target_os = "macos") {
        return sysctl_u64("kern.hv_vmm_present") == Some(1);
    }
    let product = std::fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default().to_ascii_lowercase();
    ["qemu", "virtualbox", "vmware", "kvm", "virtual machine", "utm", "parallels"].iter().any(|v| product.contains(v))
        || std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default().contains(" hypervisor")
}

pub fn os_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn arch_label() -> &'static str {
    match (cfg!(target_os = "macos"), std::env::consts::ARCH) {
        (true, "aarch64") => "arm64",
        (_, a) => a,
    }
}

/// The running kernel's release (`uname -r`, through libc, no shell).
pub fn kernel_release() -> String {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return String::new();
    }
    let raw: Vec<u8> = u.release.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&raw).into_owned()
}

/// `glibc 2.39`, `musl`, `apple`.
pub fn libc_label() -> String {
    if cfg!(target_os = "macos") {
        return "apple".into();
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn gnu_get_libc_version() -> *const libc::c_char;
        }
        let p = unsafe { gnu_get_libc_version() };
        if !p.is_null() {
            let v = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
            return format!("glibc {v}");
        }
    }
    if cfg!(target_env = "musl") {
        return "musl".into();
    }
    String::new()
}

/// The GPU, from sysfs (Linux) or the chip (Apple silicon).
pub fn gpu_label() -> Option<String> {
    if cfg!(target_os = "macos") {
        return (std::env::consts::ARCH == "aarch64").then(|| "apple".into());
    }
    let mut vendors: Vec<&str> = Vec::new();
    for e in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with("card") || n.contains('-') {
            continue;
        }
        if let Some(v) = std::fs::read_to_string(e.path().join("device/vendor")).ok().and_then(|v| k2_node_proto::host::gpu_vendor(&v)) {
            if !vendors.contains(&v) {
                vendors.push(v);
            }
        }
    }
    // A discrete GPU wins over the integrated one.
    let v = ["nvidia", "amd", "intel"].into_iter().find(|x| vendors.contains(x))?;
    if v == "nvidia" {
        if let Some(d) = std::fs::read_to_string("/proc/driver/nvidia/version").ok().and_then(|t| k2_node_proto::host::nvidia_driver(&t)) {
            return Some(format!("nvidia {d}"));
        }
    }
    Some(v.into())
}

/// A kernel update waits for a reboot: Debian's flag file, or the running
/// kernel's modules folder is gone (Arch removes it on upgrade).
pub fn reboot_required() -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    if std::path::Path::new("/var/run/reboot-required").exists() {
        return true;
    }
    let k = kernel_release();
    // Only when the folder is readable here: a user that can't list it
    // can't tell (z13flow's job users can't), and a guess would cry wolf.
    let Ok(rd) = std::fs::read_dir("/usr/lib/modules") else { return false };
    let names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    !k.is_empty() && !names.is_empty() && !names.iter().any(|n| *n == k) && !std::path::Path::new("/lib/modules").join(&k).is_dir()
}

/// What this node is (protocol 7, §4.1), from files and libc, never a
/// shell. `macos_version` is the tools probe's `sw_vers` answer.
pub fn host_facts(macos_version: Option<&str>) -> k2_node_proto::host::HostFacts {
    use k2_node_proto::host::HostFacts;
    let mut h = HostFacts {
        os: os_label().into(),
        arch: arch_label().into(),
        kernel: kernel_release(),
        libc: libc_label(),
        gpu: gpu_label(),
        reboot_required: reboot_required(),
        ..Default::default()
    };
    if cfg!(target_os = "macos") {
        let v = macos_version.unwrap_or("").to_string();
        h.distro = "macos".into();
        h.distro_name = if v.is_empty() { "macOS".into() } else { format!("macOS {v}") };
        h.distro_version = v;
        h.pkg = std::path::Path::new("/opt/homebrew/bin/brew").is_file().then(|| "brew".into()).or_else(|| std::path::Path::new("/usr/local/bin/brew").is_file().then(|| "brew".into()));
        return h;
    }
    let o = k2_node_proto::host::parse_os_release(&std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default());
    h.distro = if o.id.is_empty() { "linux".into() } else { o.id.clone() };
    h.distro_like = o.id_like.clone();
    h.distro_version = o.version_id.clone().unwrap_or_else(|| "rolling".into());
    h.distro_name = o.pretty_name.clone().unwrap_or_else(|| h.distro.clone());
    h.distro_flavor = crate::pkgsys::flavor();
    h.pkg = crate::pkgsys::detect().map(|m| m.name().to_string());
    // GPU jobs (0.46.11): every GPU with a render node (`jobs` is set per
    // session in the offer).
    h.gpus = crate::gpu::facts(std::path::Path::new(crate::gpu::SYS));
    h
}

fn first_version(s: &str) -> Option<String> {
    s.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(|w| w.trim_end_matches(',').to_string())
}

/// Probe tool versions with the job env (never the node's own env).
pub async fn probe_tools(env: &[(String, String)]) -> BTreeMap<String, String> {
    let limit = Duration::from_secs(20);
    let mut out = BTreeMap::new();
    let probes: &[(&str, &str, &[&str])] = &[
        ("rustc", "rustc", &["--version"]),
        ("cargo", "cargo", &["--version"]),
        ("git", "git", &["--version"]),
        ("bun", "bun", &["--version"]),
        ("cargo-nextest", "cargo", &["nextest", "--version"]),
    ];
    for (name, prog, args) in probes {
        if let Some(path) = which(prog, env) {
            if let Ok(o) = crate::util::run_cmd(&path, args, env, None, limit).await {
                if o.ok {
                    if let Some(v) = first_version(&o.stdout).or_else(|| first_version(&o.stderr)) {
                        out.insert(name.to_string(), v);
                    }
                }
            }
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(o) = crate::util::run_cmd("/usr/bin/sw_vers", &["-productVersion"], env, None, limit).await {
            if o.ok {
                out.insert("macos".into(), o.stdout.trim().to_string());
            }
        }
        if let Ok(o) =
            crate::util::run_cmd("/usr/sbin/pkgutil", &["--pkg-info=com.apple.pkg.CLTools_Executables"], env, None, limit).await
        {
            if o.ok {
                if let Some(v) = o.stdout.lines().find_map(|l| l.strip_prefix("version: ")) {
                    out.insert("xcode-clt".into(), v.trim().to_string());
                }
            }
        }
    }
    out
}

/// Resolve `prog` on the env's PATH.
pub fn which(prog: &str, env: &[(String, String)]) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_str()).unwrap_or("/usr/bin:/bin");
    path.split(':').map(|d| Path::new(d).join(prog)).find(|p| {
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }).map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(first_version("rustc 1.95.0 (59807616e 2026-04-14)").as_deref(), Some("1.95.0"));
        assert_eq!(first_version("git version 2.50.0").as_deref(), Some("2.50.0"));
        assert_eq!(first_version("1.4.2").as_deref(), Some("1.4.2"));
        assert_eq!(first_version("nothing here"), None);
    }

    #[test]
    fn facts_are_sane() {
        assert!(cores() >= 1);
        assert!(mem_total() > 0);
        let (free, total) = disk(&std::env::temp_dir());
        assert!(total > 0 && free <= total);
        assert!(!boot_id().is_empty());
    }

    /// Protocol 7: the host facts this machine reports are filled in.
    #[test]
    fn host_facts_are_filled_in() {
        let h = host_facts(Some("15.5"));
        assert_eq!(h.os, os_label());
        assert_eq!(h.arch, arch_label());
        assert!(!h.kernel.is_empty(), "uname -r");
        assert!(!h.distro.is_empty() && !h.distro_version.is_empty() && !h.distro_name.is_empty(), "{h:?}");
        if cfg!(target_os = "macos") {
            assert_eq!((h.distro.as_str(), h.distro_version.as_str(), h.libc.as_str()), ("macos", "15.5", "apple"));
        } else {
            assert!(h.libc.starts_with("glibc ") || h.libc == "musl", "{}", h.libc);
            assert!(h.pkg.is_some(), "a Linux test node has a package manager: {h:?}");
            assert_ne!(h.distro, "linux", "/etc/os-release names the distro");
        }
        assert!(!h.line().is_empty() && !h.short().is_empty());
    }

    #[tokio::test]
    async fn git_is_found_with_a_plain_path() {
        let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let tools = probe_tools(&env).await;
        assert!(tools.contains_key("git"), "{tools:?}");
    }
}
