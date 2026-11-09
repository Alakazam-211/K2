//! K2-shipped package profiles (protocol 7,
//! prd-compute-self-serve-environments-v1 §7).
//!
//! - A profile is a named, versioned bundle of canonical `sys:` names
//!   ([`crate::pkgmap`]) plus tool needs. K2 writes and reviews them like
//!   code, and they never hold a package that starts a service (Rosson, Q6;
//!   a test checks every name against the deny list).
//! - A job names one as a need: `--needs profile:tauri-linux` (the newest
//!   version) or `profile:tauri-linux@1`. It counts as one need against the
//!   32-need cap; its system names travel in `Plan.packages` (at most
//!   [`MAX_PLAN_PACKAGES`]) to nodes offering `pkg-map`.
//! - A node owner at the `profiles` policy level turns profiles on per
//!   server, by version (`tauri-linux@1`). A later version that adds names
//!   covers them only once the owner allows that version: until then they
//!   need a card like any other name (§7.4).
//! - Every version stays in [`PROFILES`], so a node still knows what an
//!   owner allowed.
//!
//! Not to be confused with `--preset` (your own saved flags in
//! `.k2/compute.toml`) or `k2 compute test --profile` (nextest's profile).

/// Most system names one plan carries (profiles expanded plus `sys:`).
pub const MAX_PLAN_PACKAGES: usize = 128;

/// Refusal code: a profile name or version this node (or server) doesn't
/// know.
pub const PROFILE_UNKNOWN: &str = "profile_unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub version: u32,
    /// Profiles whose names this one includes (same K2 release).
    pub includes: &'static [&'static str],
    /// Canonical system names.
    pub sys: &'static [&'static str],
    /// Tool needs (`rust@stable+clippy+rustfmt`, `cargo-nextest`).
    pub tools: &'static [&'static str],
    /// One line for `k2 compute profiles` and the docs.
    pub about: &'static str,
}

const BUILD_BASE: &[&str] = &["build-essential", "pkg-config", "cmake", "clang", "lld", "git", "curl", "ca-certificates", "unzip", "libssl-dev"];

/// Every version of every profile, oldest first per name.
pub const PROFILES: &[Profile] = &[
    Profile { name: "build-base", version: 1, includes: &[], sys: BUILD_BASE, tools: &[], about: "compilers and the common build tools" },
    Profile {
        name: "tauri-linux",
        version: 1,
        includes: &["build-base"],
        sys: &[
            // Tauri's own Linux list.
            "libwebkit2gtk-4.1-dev",
            "libgtk-3-dev",
            "libayatana-appindicator3-dev",
            "librsvg2-dev",
            "libsoup-3.0-dev",
            "libjavascriptcoregtk-4.1-dev",
            "libxdo-dev",
            // The GUI CI set a Tauri app's Rust tests and headless runs need.
            "llvm-dev",
            "libclang-dev",
            "python3",
            "libx11-dev",
            "libxcb1-dev",
            "libxkbcommon-dev",
            "libxkbcommon-x11-dev",
            "libwayland-dev",
            "libegl1-mesa-dev",
            "libgl1-mesa-dev",
            "libgles2-mesa-dev",
            "libfontconfig1-dev",
            "libfreetype6-dev",
            "libharfbuzz-dev",
            "libdbus-1-dev",
            "libudev-dev",
            "libunwind-dev",
            "libasound2-dev",
            "libgl1-mesa-dri",
            "mesa-vulkan-drivers",
            "xvfb",
            "libxtst6",
            "libx11-6",
            "libegl1",
            "libasound2t64",
            "libfontconfig1",
        ],
        tools: &[],
        about: "build-base plus WebKitGTK, GTK and the GUI test set for Tauri apps on Linux",
    },
    Profile {
        name: "rust-gui-ci",
        version: 1,
        includes: &["build-base"],
        sys: &[
            "llvm-dev",
            "libclang-dev",
            "libx11-dev",
            "libxcb1-dev",
            "libxkbcommon-dev",
            "libxkbcommon-x11-dev",
            "libwayland-dev",
            "libegl1-mesa-dev",
            "libgl1-mesa-dev",
            "libgles2-mesa-dev",
            "libfontconfig1-dev",
            "libfreetype6-dev",
            "libharfbuzz-dev",
            "libdbus-1-dev",
            "libudev-dev",
            "libunwind-dev",
            "libasound2-dev",
            "mesa-vulkan-drivers",
            "xvfb",
        ],
        tools: &["rust@stable+clippy+rustfmt", "cargo-nextest"],
        about: "build-base plus the X11/Wayland/GL/font/audio dev set, Rust with clippy and rustfmt, and nextest",
    },
    Profile {
        name: "gpu-headless",
        version: 1,
        includes: &[],
        sys: &["mesa-utils", "vulkan-tools", "mesa-vulkan-drivers", "libvulkan1", "libegl1", "libgbm1", "weston", "xwayland", "xvfb"],
        tools: &[],
        about: "GPU tests without a display (with --gpu): EGL and GBM, the Vulkan loader and Mesa's GPU drivers, eglinfo and vulkaninfo, a headless Weston with Xwayland, and Xvfb (CPU-only)",
    },
    Profile {
        name: "k2-workspace",
        version: 1,
        includes: &["build-base"],
        sys: &["libasound2-dev", "libdbus-1-dev", "libudev-dev", "libgtk-3-dev", "libwebkit2gtk-4.1-dev", "libsoup-3.0-dev", "libjavascriptcoregtk-4.1-dev", "libayatana-appindicator3-dev", "librsvg2-dev", "libxdo-dev"],
        tools: &[],
        about: "what K2's own cargo --workspace build needs",
    },
];

/// The newest version of `name`.
pub fn latest(name: &str) -> Option<&'static Profile> {
    PROFILES.iter().filter(|p| p.name == name).max_by_key(|p| p.version)
}

/// `name` at `version` (`None` = the newest).
pub fn get(name: &str, version: Option<u32>) -> Option<&'static Profile> {
    match version {
        None => latest(name),
        Some(v) => PROFILES.iter().find(|p| p.name == name && p.version == v),
    }
}

/// Parse `tauri-linux` / `tauri-linux@1` (no `profile:` prefix).
pub fn parse_ref(s: &str) -> Result<(String, Option<u32>), String> {
    let (n, v) = match s.split_once('@') {
        Some((n, v)) => (n, Some(v.parse::<u32>().map_err(|_| format!("profile:{s}: the version is a whole number (profile:tauri-linux@1)"))?)),
        None => (s, None),
    };
    if n.is_empty() || n.len() > 40 || !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("profile:{s}: a profile is profile:<name>[@<version>] (a-z 0-9 -)"));
    }
    Ok((n.to_string(), v))
}

/// The profile a ref names, or `profile_unknown` words.
pub fn resolve(s: &str) -> Result<&'static Profile, String> {
    let (n, v) = parse_ref(s)?;
    get(&n, v).ok_or_else(|| {
        let names: Vec<String> = names().into_iter().map(|n| format!("profile:{n}")).collect();
        match v {
            Some(v) if latest(&n).is_some() => format!("{PROFILE_UNKNOWN}: profile:{n}@{v} isn't known here (newest: @{}); update k2-node, or leave the version off", latest(&n).map(|p| p.version).unwrap_or(1)),
            _ => format!("{PROFILE_UNKNOWN}: no profile {n:?}. K2 ships: {}", names.join(", ")),
        }
    })
}

/// Profile names, newest version each.
pub fn names() -> Vec<&'static str> {
    let mut v: Vec<&str> = PROFILES.iter().map(|p| p.name).collect();
    v.dedup();
    v
}

impl Profile {
    /// `tauri-linux@1`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Every canonical system name, includes first, sorted and unique.
    pub fn all_sys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for inc in self.includes {
            if let Some(p) = latest(inc) {
                out.extend(p.all_sys());
            }
        }
        out.extend(self.sys.iter().map(|s| s.to_string()));
        out.sort();
        out.dedup();
        out
    }

    /// Tool needs, includes first, unique.
    pub fn all_tools(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for inc in self.includes {
            if let Some(p) = latest(inc) {
                out.extend(p.all_tools());
            }
        }
        for t in self.tools {
            if !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tauri_linux_covers_the_requested_list() {
        let want = "build-essential clang lld llvm-dev libclang-dev cmake pkg-config python3 libx11-dev libxcb1-dev libxkbcommon-dev \
                    libxkbcommon-x11-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libgles2-mesa-dev libfontconfig1-dev \
                    libfreetype6-dev libharfbuzz-dev libssl-dev libdbus-1-dev libudev-dev libunwind-dev libasound2-dev \
                    libgl1-mesa-dri mesa-vulkan-drivers xvfb libxtst6 libx11-6 libegl1 libasound2t64 libfontconfig1";
        let have = latest("tauri-linux").unwrap().all_sys();
        for w in want.split_whitespace() {
            assert!(have.iter().any(|h| h == w), "tauri-linux lacks {w}");
        }
    }

    #[test]
    fn every_profile_name_is_mapped_valid_and_never_a_service() {
        for p in PROFILES {
            for s in p.all_sys() {
                assert!(crate::needs::valid_package(&s), "{}: {s}", p.id());
                assert!(crate::pkgmap::known(&s), "{}: {s} has no map row", p.id());
                assert!(crate::pkgpolicy::denied(&s).is_none(), "{}: {s} is on the deny list", p.id());
                for m in crate::pkgmap::Mgr::ALL {
                    if let Some(natives) = crate::pkgmap::native(&s, m, "", "") {
                        for n in natives {
                            assert!(crate::pkgpolicy::denied(&n).is_none(), "{}: {s} → {} {n} is denied", p.id(), m.name());
                        }
                    }
                }
            }
            for t in p.all_tools() {
                assert!(crate::needs::parse(t.split('+').next().unwrap()).is_ok(), "{}: tool {t}", p.id());
            }
            assert!(p.all_sys().len() <= MAX_PLAN_PACKAGES);
        }
    }

    /// GPU jobs S3 (0.46.11): `profile:gpu-headless@1` is the PRD's set,
    /// and on Arch it installs the GPU Vulkan drivers, not only lavapipe.
    #[test]
    fn gpu_headless_is_the_headless_gpu_set() {
        let p = resolve("gpu-headless@1").unwrap();
        assert_eq!(
            p.all_sys(),
            vec!["libegl1", "libgbm1", "libvulkan1", "mesa-utils", "mesa-vulkan-drivers", "vulkan-tools", "weston", "xvfb", "xwayland"]
        );
        let arch: Vec<String> = p.all_sys().iter().flat_map(|s| crate::pkgmap::native(s, crate::pkgmap::Mgr::Pacman, "arch", "rolling").unwrap()).collect();
        for want in ["vulkan-radeon", "vulkan-intel", "mesa-utils", "vulkan-tools", "weston", "xorg-xwayland", "vulkan-icd-loader"] {
            assert!(arch.iter().any(|n| n == want), "Arch lacks {want}: {arch:?}");
        }
        assert!(names().contains(&"gpu-headless"));
    }

    #[test]
    fn refs_resolve_by_version() {
        assert_eq!(resolve("tauri-linux").unwrap().version, 1);
        assert_eq!(resolve("tauri-linux@1").unwrap().name, "tauri-linux");
        let e = resolve("tauri-linux@9").unwrap_err();
        assert!(e.starts_with(PROFILE_UNKNOWN) && e.contains("update k2-node"), "{e}");
        let e = resolve("nope").unwrap_err();
        assert!(e.contains("profile:tauri-linux"), "{e}");
        assert!(parse_ref("Tauri").is_err());
        assert!(parse_ref("tauri@x").is_err());
    }
}
