//! Host facts (protocol 7, prd-compute-self-serve-environments-v1 §4):
//! what each node is, beyond `os` and `arch`. The distro and its version,
//! the package manager, the kernel, the C library and the GPU.
//!
//! - The node reads them from files (never a shell) at start and every 10
//!   minutes with the tools probe.
//! - They ride the offer as labels ([`HOST_LABEL_KEYS`]) and as one
//!   [`HostFacts`] object, only to a server that said protocol ≥
//!   [`PROTOCOL_HOST`]. An older server sees exactly its own release's
//!   offer (the solo goldens hold it).
//! - Owner labels can't shadow these keys: a pin with such a label is
//!   refused ([`shadows_host_label`]).
//! - Kernel, libc and GPU go to every server on the node. A tenant's job
//!   can run `uname` anyway, so nothing new leaks (Rosson, Q7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The first server protocol that gets host facts, the package status and
/// the package request frames.
pub const PROTOCOL_HOST: u32 = 7;

/// Offer feature: the node sends [`HostFacts`] and the host labels.
pub const FEATURE_HOST_FACTS: &str = "host-facts";

/// Labels the node sets itself. `os`, `arch`, `vm` and `name` are older;
/// the rest come with protocol 7.
pub const HOST_LABEL_KEYS: &[&str] = &["os", "arch", "vm", "name", "distro", "distro_version", "pkg", "libc", "gpu"];

/// Is `key` a label the node sets itself (so an owner label may not use it)?
pub fn shadows_host_label(key: &str) -> bool {
    HOST_LABEL_KEYS.contains(&key)
}

/// What a node is (§4.1). Every field is a short display string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// `linux` | `macos`.
    pub os: String,
    /// `x86_64` | `arm64` | `aarch64`.
    pub arch: String,
    /// `/etc/os-release` `ID` (`ubuntu`, `arch`, `debian`, `fedora`), or
    /// `macos`.
    pub distro: String,
    /// `ID_LIKE` (`debian` for Ubuntu). Empty on macOS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distro_like: Vec<String>,
    /// `VERSION_ID`, or `rolling` when the distro has none (Arch).
    pub distro_version: String,
    /// `PRETTY_NAME` (`Ubuntu 24.04.1 LTS`), or `macOS 15.5`.
    pub distro_name: String,
    /// A known wrapper on top of the distro (`omarchy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_flavor: Option<String>,
    /// `apt` | `dnf` | `pacman` | `zypper` | `brew`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg: Option<String>,
    /// `uname -r`.
    pub kernel: String,
    /// `glibc 2.39`, `musl`, `apple`.
    pub libc: String,
    /// `nvidia 550.120`, `amd`, `intel`, `apple`; absent = none found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// A kernel update waits for a reboot (§10).
    #[serde(default)]
    pub reboot_required: bool,
}

impl HostFacts {
    /// One short column: `ubuntu24.04-x86_64`, `arch-x86_64` (rolling),
    /// `macos15.5-arm64`. Falls back to `<os>-<arch>` with no distro.
    pub fn short(&self) -> String {
        let d = if self.distro.is_empty() { self.os.as_str() } else { self.distro.as_str() };
        let v = if self.distro_version.is_empty() || self.distro_version == "rolling" { "" } else { self.distro_version.as_str() };
        format!("{d}{v}-{}", self.arch)
    }

    /// The `host:` line (`k2 compute info`, the job log header):
    /// `Ubuntu 24.04.1 LTS · apt · kernel 6.8.0-48-generic · glibc 2.39 · x86_64 · no GPU`.
    pub fn line(&self) -> String {
        let mut parts = vec![if self.distro_name.is_empty() { self.short() } else { self.distro_name.clone() }];
        if let Some(f) = &self.distro_flavor {
            parts[0] = format!("{} ({f})", parts[0]);
        }
        if let Some(p) = &self.pkg {
            parts.push(p.clone());
        }
        if !self.kernel.is_empty() {
            parts.push(format!("kernel {}", self.kernel));
        }
        if !self.libc.is_empty() {
            parts.push(self.libc.clone());
        }
        parts.push(self.arch.clone());
        parts.push(self.gpu.as_ref().map(|g| format!("GPU {g}")).unwrap_or_else(|| "no GPU".into()));
        if self.reboot_required {
            parts.push("reboot pending".into());
        }
        parts.join(" · ")
    }

    /// The protocol 7 labels (`distro`, `distro_version`, `pkg`, `libc`,
    /// `gpu`). `libc` is its family (`glibc`), `gpu` its vendor.
    pub fn labels(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        if !self.distro.is_empty() {
            m.insert("distro".into(), self.distro.clone());
        }
        if !self.distro_version.is_empty() {
            m.insert("distro_version".into(), self.distro_version.clone());
        }
        if let Some(p) = &self.pkg {
            m.insert("pkg".into(), p.clone());
        }
        if let Some(f) = self.libc.split_whitespace().next() {
            m.insert("libc".into(), f.to_string());
        }
        if let Some(v) = self.gpu.as_deref().and_then(|g| g.split_whitespace().next()) {
            m.insert("gpu".into(), v.to_string());
        }
        m
    }

    /// `ID` or `ID_LIKE` is `d` (so Ubuntu counts as Debian-like).
    pub fn is_like(&self, d: &str) -> bool {
        self.distro == d || self.distro_like.iter().any(|x| x == d)
    }

    /// `--distro` against these facts: `ubuntu`, `debian` (ID or ID_LIKE),
    /// `ubuntu@24.04` (the version matches on a dot prefix and only for the
    /// distro's own ID: `debian@12` never matches Ubuntu).
    pub fn matches(&self, spec: &DistroSpec) -> bool {
        match &spec.version {
            None => self.is_like(&spec.distro),
            Some(v) => self.distro == spec.distro && version_prefix(&self.distro_version, v),
        }
    }
}

/// `24.04.1` matches `24` and `24.04`, on a dot.
pub fn version_prefix(have: &str, want: &str) -> bool {
    have == want || have.strip_prefix(want).is_some_and(|r| r.starts_with('.'))
}

/// A parsed `--distro` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistroSpec {
    pub distro: String,
    pub version: Option<String>,
}

/// `ubuntu`, `ubuntu@24.04`, `arch`, `debian@12`. Lower-case `a-z 0-9 . _ -`.
pub fn parse_distro(s: &str) -> Result<DistroSpec, String> {
    let s = s.trim().to_ascii_lowercase();
    let (d, v) = match s.split_once('@') {
        Some((d, v)) => (d.to_string(), Some(v.to_string())),
        None => (s.clone(), None),
    };
    let ok = |x: &str| !x.is_empty() && x.len() <= 32 && x.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !ok(&d) || v.as_deref().is_some_and(|v| !ok(v)) {
        return Err(format!("--distro {s:?}: a distro id, optionally @version (ubuntu, ubuntu@24.04, debian, arch, fedora)"));
    }
    Ok(DistroSpec { distro: d, version: v })
}

/// The `/etc/os-release` keys K2 reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: Vec<String>,
    pub version_id: Option<String>,
    pub pretty_name: Option<String>,
}

/// Parse `/etc/os-release` (`KEY=value`, values optionally quoted).
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut o = OsRelease::default();
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else { continue };
        let v = v.trim();
        let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(v);
        let v: String = v.chars().filter(|c| !c.is_control()).take(80).collect();
        match k.trim() {
            "ID" => o.id = v.to_ascii_lowercase(),
            "ID_LIKE" => o.id_like = v.split_whitespace().map(|x| x.to_ascii_lowercase()).collect(),
            "VERSION_ID" if !v.is_empty() => o.version_id = Some(v),
            "PRETTY_NAME" if !v.is_empty() => o.pretty_name = Some(v),
            _ => {}
        }
    }
    o
}

/// The GPU vendor from a PCI vendor id (`/sys/class/drm/card*/device/vendor`).
pub fn gpu_vendor(pci_vendor: &str) -> Option<&'static str> {
    match pci_vendor.trim().to_ascii_lowercase().as_str() {
        "0x10de" => Some("nvidia"),
        "0x1002" => Some("amd"),
        "0x8086" => Some("intel"),
        _ => None,
    }
}

/// The driver version from `/proc/driver/nvidia/version`
/// (`NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.120  …`).
pub fn nvidia_driver(text: &str) -> Option<String> {
    let l = text.lines().next()?;
    l.split_whitespace().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.')).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ubuntu() -> HostFacts {
        HostFacts {
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: "ubuntu".into(),
            distro_like: vec!["debian".into()],
            distro_version: "24.04".into(),
            distro_name: "Ubuntu 24.04.1 LTS".into(),
            distro_flavor: None,
            pkg: Some("apt".into()),
            kernel: "6.8.0-48-generic".into(),
            libc: "glibc 2.39".into(),
            gpu: None,
            reboot_required: false,
        }
    }

    #[test]
    fn os_release_parses_quoted_and_bare_values() {
        let o = parse_os_release("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n");
        assert_eq!(o.id, "ubuntu");
        assert_eq!(o.id_like, vec!["debian"]);
        assert_eq!(o.version_id.as_deref(), Some("24.04"));
        assert_eq!(o.pretty_name.as_deref(), Some("Ubuntu 24.04.1 LTS"));
        let a = parse_os_release("NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\nPRETTY_NAME=\"Arch Linux\"\n");
        assert_eq!(a.id, "arch");
        assert_eq!(a.version_id, None, "Arch has no VERSION_ID");
        let f = parse_os_release("ID=fedora\nVERSION_ID=41\nID_LIKE=\"rhel centos\"\n");
        assert_eq!(f.id_like, vec!["rhel", "centos"]);
    }

    #[test]
    fn short_and_line_read_as_the_prd_says() {
        let u = ubuntu();
        assert_eq!(u.short(), "ubuntu24.04-x86_64");
        assert_eq!(u.line(), "Ubuntu 24.04.1 LTS · apt · kernel 6.8.0-48-generic · glibc 2.39 · x86_64 · no GPU");
        let mut a = ubuntu();
        a.distro = "arch".into();
        a.distro_version = "rolling".into();
        assert_eq!(a.short(), "arch-x86_64");
        let mac = HostFacts { os: "macos".into(), arch: "arm64".into(), distro: "macos".into(), distro_version: "15.5".into(), ..Default::default() };
        assert_eq!(mac.short(), "macos15.5-arm64");
    }

    #[test]
    fn distro_filter_uses_id_like_and_a_dot_prefix() {
        let u = ubuntu();
        assert!(u.matches(&parse_distro("ubuntu").unwrap()));
        assert!(u.matches(&parse_distro("debian").unwrap()), "Ubuntu is Debian-like");
        assert!(u.matches(&parse_distro("ubuntu@24.04").unwrap()));
        assert!(u.matches(&parse_distro("ubuntu@24").unwrap()));
        assert!(!u.matches(&parse_distro("ubuntu@24.0").unwrap()), "a dot prefix, not a string prefix");
        assert!(!u.matches(&parse_distro("debian@12").unwrap()), "a version names the distro's own id");
        assert!(!u.matches(&parse_distro("arch").unwrap()));
        assert!(parse_distro("Ubuntu@24.04").is_ok(), "case folds");
        assert!(parse_distro("ubuntu;rm").is_err());
        assert!(parse_distro("").is_err());
    }

    #[test]
    fn labels_carry_families_not_versions() {
        let mut u = ubuntu();
        u.gpu = Some("nvidia 550.120".into());
        let l = u.labels();
        assert_eq!(l.get("libc").map(String::as_str), Some("glibc"));
        assert_eq!(l.get("gpu").map(String::as_str), Some("nvidia"));
        assert_eq!(l.get("distro").map(String::as_str), Some("ubuntu"));
        assert!(l.keys().all(|k| shadows_host_label(k)), "every host label is reserved");
        assert!(!shadows_host_label("team"));
    }

    #[test]
    fn gpu_facts_parse() {
        assert_eq!(gpu_vendor("0x10de\n"), Some("nvidia"));
        assert_eq!(gpu_vendor("0x1af4"), None, "a virtio card is no GPU");
        assert_eq!(
            nvidia_driver("NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.120  Fri Sep 13 10:10:01 UTC 2024\n").as_deref(),
            Some("550.120")
        );
    }
}
