//! The toolchain layer (compute polish v1): whatever a job needs to
//! compile code and run tests, installed on demand and cached.
//!
//! Three kinds of need ([`k2_node_proto::needs`]):
//!
//! - **User-space toolchains** (Node, Python, Go, Java, Bun, …) through
//!   [mise](https://mise.jdx.dev) (MIT, one static binary, reads the
//!   same files developers already commit). Everything lives in the node
//!   home (`toolchains/mise`), installs run as the node user, no root.
//!   The mise binary itself is a pinned release whose sha256 is compiled
//!   in here; it is fetched once and checked before it is ever run.
//! - **Rust** through rustup in `toolchains/{cargo,rustup}` (mise only
//!   wraps rustup for Rust, so we call rustup directly), plus the
//!   prebuilt `cargo-nextest`.
//! - **System packages** (`sys:libssl-dev`): root. Linux installs them
//!   only from the machine owner's approved list
//!   ([`crate::packages`]); macOS reports the exact `brew install` line.
//!
//! What a job needs = its explicit `--needs` (required: a failure fails
//! the job before it starts) plus what its repo declares (`mise.toml`,
//! `.mise.toml`, `.tool-versions`, `.nvmrc`, `.node-version`,
//! `.python-version`, `rust-toolchain(.toml)`, `.k2/compute.toml`), which
//! is best effort (a failure is a warning in the job log). An explicit
//! need beats a repo file for the same tool.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::needs::{self, Need};

use crate::util::{run_cmd, CmdOut};

/// Pinned mise release (2026-09-20). Bump = new version + four sha256s.
pub const MISE_VERSION: &str = "2026.9.12";

/// `(os, arch) → (asset, sha256)` for [`MISE_VERSION`].
const MISE_ASSETS: &[(&str, &str, &str, &str)] = &[
    ("linux", "x86_64", "mise-v2026.9.12-linux-x64-musl.tar.gz", "1b2051d8d3efab4d9c85ceb59849a3eb052db24ad406a3901fd6b45374c56ebe"),
    ("linux", "aarch64", "mise-v2026.9.12-linux-arm64-musl.tar.gz", "22e7ecd3c6c84ef36e4d0c78c4eef49a79e3fb83aa88e091e2993fd9c9a3f8a1"),
    ("macos", "aarch64", "mise-v2026.9.12-macos-arm64.tar.gz", "0f1c7f3e74d8c9ae82976e6990058f2bc68821acc6b4c37a68c206c836692419"),
    ("macos", "x86_64", "mise-v2026.9.12-macos-x64.tar.gz", "22be38ec60632913143bf6a96b0aacfd895fdfe4ce4e8958e942f7bdb5a9185b"),
];

const MISE_BASE_URL: &str = "https://github.com/jdx/mise/releases/download";
const RUSTUP_INIT_URL: &str = "https://sh.rustup.rs";

/// Time limits for one install step.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(10 * 60);
const INSTALL_LIMIT: Duration = Duration::from_secs(20 * 60);

/// Stable failure codes (the job's `reason`).
pub mod codes {
    pub const NEED_INVALID: &str = k2_node_proto::needs::NEED_INVALID;
    pub const TOOL_UNSUPPORTED: &str = k2_node_proto::needs::TOOL_UNSUPPORTED;
    pub const TOOL_INSTALL_FAILED: &str = "tool_install_failed";
    pub const PACKAGE_NOT_APPROVED: &str = "package_not_approved";
    pub const PACKAGE_INSTALL_FAILED: &str = "package_install_failed";
    pub const PACKAGE_MISSING: &str = "package_missing";
    pub const RUST_TARGET_UNSUPPORTED: &str = k2_node_proto::needs::RUST_TARGET_UNSUPPORTED;
}

pub type Fail = (&'static str, String);

/// Toolchain resolves in progress, by toolchains root, since when.
static INSTALLING: std::sync::Mutex<BTreeMap<PathBuf, Vec<std::time::Instant>>> = std::sync::Mutex::new(BTreeMap::new());

/// A resolve this long is installing something (an already-installed
/// toolchain resolves in well under it).
pub const INSTALLING_AFTER: Duration = Duration::from_secs(2);

/// Held for one `resolve` (prd-compute-shared-queue-v1 §7, V76: the node
/// queue starts nothing new while a toolchain install runs, `installing_tools`).
pub struct Installing {
    root: PathBuf,
    at: std::time::Instant,
}

impl Installing {
    pub fn start(root: &Path) -> Self {
        let at = std::time::Instant::now();
        INSTALLING.lock().unwrap().entry(root.to_path_buf()).or_default().push(at);
        Self { root: root.to_path_buf(), at }
    }
}

impl Drop for Installing {
    fn drop(&mut self) {
        let mut m = INSTALLING.lock().unwrap();
        if let Some(v) = m.get_mut(&self.root) {
            v.retain(|t| *t != self.at);
            if v.is_empty() {
                m.remove(&self.root);
            }
        }
    }
}

/// Is a toolchain install running under `root` (a resolve held at least
/// [`INSTALLING_AFTER`])?
pub fn installing(root: &Path) -> bool {
    installing_after(root, INSTALLING_AFTER)
}

fn installing_after(root: &Path, after: Duration) -> bool {
    INSTALLING.lock().unwrap().get(root).is_some_and(|v| v.iter().any(|t| t.elapsed() >= after))
}

/// Where everything lives, and the env the installers run with.
#[derive(Debug, Clone)]
pub struct Toolchains {
    /// `<home>/toolchains`.
    pub root: PathBuf,
    /// The env every installer starts from (node's base env).
    pub base_env: Vec<(String, String)>,
    pub os: &'static str,
    pub arch: &'static str,
    /// The approved-packages file (root-owned; see [`crate::packages`]).
    pub packages_file: PathBuf,
    /// Rust channels a running job uses now (update continuity, Rosson
    /// 2026-11-04): a moving channel (`stable`, `beta`) is never moved to a
    /// newer release under them; the next job after they end gets it.
    pub busy_channels: Vec<String>,
    /// The owner's package policy file (root-owned; protocol 7).
    pub policy_file: PathBuf,
    /// Whose policy the job's packages fall under (its server).
    pub server: crate::packages::RequestServer,
    /// The root package installer is set up here.
    pub pkg_installer: bool,
    /// Protocol 7 (`pkg-map`): the plan's system names, profiles expanded
    /// by the server. Empty from an older server.
    pub plan_packages: Vec<String>,
}

/// Rust channels that move to a new release over time.
pub const MOVING_CHANNELS: &[&str] = &["stable", "beta"];
/// A moving channel is brought to its newest release at most this often,
/// and after every k2-node update (new jobs after an update get the newest).
pub const RUST_REFRESH_SECS: i64 = 24 * 3600;

/// What a job gets after resolving.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Bin dirs to put first on the job's PATH.
    pub path_prepend: Vec<PathBuf>,
    /// Extra job env (`RUSTUP_TOOLCHAIN`, `MISE_*`).
    pub env: Vec<(String, String)>,
    /// Lines for the job log.
    pub notes: Vec<String>,
    /// Anything installed now (re-probe tools afterwards).
    pub installed_any: bool,
}

impl Toolchains {
    pub fn new(home: &Path, base_env: Vec<(String, String)>) -> Self {
        Self {
            root: home.join("toolchains"),
            base_env,
            os: crate::sysinfo::os_label(),
            arch: std::env::consts::ARCH,
            packages_file: PathBuf::from(crate::packages::APPROVED_FILE),
            busy_channels: Vec::new(),
            policy_file: PathBuf::from(crate::packages::POLICY_FILE),
            server: crate::packages::RequestServer::owner(),
            pkg_installer: crate::packages::installer_present(),
            plan_packages: Vec::new(),
        }
    }

    fn refresh_mark(&self, channel: &str) -> PathBuf {
        self.root.join(format!("rust-refreshed-{channel}"))
    }

    /// Is a moving channel due for its newest release? Never refreshed,
    /// refreshed by another k2-node build (this one was installed since),
    /// or over [`RUST_REFRESH_SECS`] ago.
    pub fn rust_refresh_due(&self, channel: &str, now: i64) -> bool {
        if !MOVING_CHANNELS.contains(&channel) {
            return false;
        }
        let mark = std::fs::read_to_string(self.refresh_mark(channel)).unwrap_or_default();
        let mut w = mark.split_whitespace();
        match (w.next(), w.next().and_then(|t| t.parse::<i64>().ok())) {
            (Some(v), Some(at)) => v != crate::PRODUCT_VERSION || now - at >= RUST_REFRESH_SECS,
            _ => true,
        }
    }

    fn mark_refreshed(&self, channel: &str, now: i64) {
        let _ = crate::util::atomic_write(&self.refresh_mark(channel), format!("{} {now}\n", crate::PRODUCT_VERSION).as_bytes(), 0o644);
    }

    pub fn mise_dir(&self) -> PathBuf {
        self.root.join("mise-bin")
    }
    /// The extracted release keeps its own layout: `mise/bin/mise`.
    pub fn mise_bin(&self) -> PathBuf {
        self.mise_dir().join("mise/bin/mise")
    }
    pub fn mise_data(&self) -> PathBuf {
        self.root.join("mise")
    }
    pub fn cargo_home(&self) -> PathBuf {
        self.root.join("cargo")
    }
    pub fn rustup_home(&self) -> PathBuf {
        self.root.join("rustup")
    }

    /// mise's own dirs, all inside the node home; the config dir has no
    /// config, so mise never reads a file we didn't parse ourselves.
    pub fn mise_env(&self) -> Vec<(String, String)> {
        let d = |p: PathBuf| p.display().to_string();
        vec![
            ("MISE_DATA_DIR".into(), d(self.mise_data())),
            ("MISE_CACHE_DIR".into(), d(self.root.join("mise-cache"))),
            ("MISE_CONFIG_DIR".into(), d(self.root.join("mise-config"))),
            ("MISE_STATE_DIR".into(), d(self.root.join("mise-state"))),
            ("MISE_GLOBAL_CONFIG_FILE".into(), d(self.root.join("mise-config/config.toml"))),
            ("MISE_YES".into(), "1".into()),
            ("MISE_HIDE_UPDATE_WARNING".into(), "1".into()),
            ("MISE_PYTHON_COMPILE".into(), "0".into()),
            // Never asdf/vfox plugins (shell scripts from third-party
            // repos): registry names like `cmake` resolve to their aqua
            // backend (prebuilt release, checksum-checked) or fail.
            ("MISE_DISABLE_BACKENDS".into(), "asdf,vfox".into()),
        ]
    }

    fn env_with(&self, extra: &[(String, String)]) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self.base_env.clone();
        for (k, v) in extra {
            env.retain(|(ek, _)| ek != k);
            env.push((k.clone(), v.clone()));
        }
        env
    }

    fn installer_env(&self) -> Vec<(String, String)> {
        let mut extra = self.mise_env();
        extra.push(("CARGO_HOME".into(), self.cargo_home().display().to_string()));
        extra.push(("RUSTUP_HOME".into(), self.rustup_home().display().to_string()));
        self.env_with(&extra)
    }

    /// Run a program from a directory the node user can always read (the
    /// toolchains root), never the caller's cwd (the old Rust installer
    /// failed when started from a 0700 home).
    async fn run(&self, program: &str, args: &[&str], limit: Duration) -> Result<CmdOut, String> {
        let _ = std::fs::create_dir_all(&self.root);
        run_cmd(program, args, &self.installer_env(), Some(&self.root), limit).await
    }

    // ── mise ─────────────────────────────────────────────────────────

    fn mise_asset(&self) -> Option<(&'static str, &'static str)> {
        MISE_ASSETS.iter().find(|(o, a, _, _)| *o == self.os && *a == self.arch).map(|(_, _, n, s)| (*n, *s))
    }

    /// The installed mise is the pinned one (`mise --version` starts with it).
    pub async fn mise_ready(&self) -> bool {
        let bin = self.mise_bin();
        if !bin.is_file() {
            return false;
        }
        match self.run(&bin.display().to_string(), &["--version"], Duration::from_secs(20)).await {
            Ok(o) => o.ok && o.stdout.trim_start().starts_with(MISE_VERSION),
            Err(_) => false,
        }
    }

    /// Fetch the pinned mise, check its sha256, unpack it. Idempotent.
    pub async fn ensure_mise(&self, log: &mut (dyn FnMut(&str) + Send)) -> Result<PathBuf, String> {
        if self.mise_ready().await {
            return Ok(self.mise_bin());
        }
        let (asset, sha) = self
            .mise_asset()
            .ok_or_else(|| format!("no pinned mise build for {}-{}", self.os, self.arch))?;
        log(&format!("installing mise {MISE_VERSION} (pinned, sha256 checked) into {}", self.mise_dir().display()));
        let stage = self.root.join(format!(".mise-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        crate::util::mkdir_mode(&stage, 0o755)?;
        let tgz = stage.join(asset);
        let url = format!("{MISE_BASE_URL}/v{MISE_VERSION}/{asset}");
        let r = download(self, &url, &tgz).await;
        let r = r.and_then(|_| check_sha256(&tgz, sha));
        if let Err(e) = r {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
        let x = self
            .run("/usr/bin/tar", &["-xzf", &tgz.display().to_string(), "-C", &stage.display().to_string()], Duration::from_secs(120))
            .await?;
        if !x.ok {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!("unpack mise: {}", x.stderr.trim()));
        }
        let _ = std::fs::remove_file(&tgz);
        let dest = self.mise_dir();
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&stage, &dest).map_err(|e| format!("place mise: {e}"))?;
        if !self.mise_ready().await {
            return Err(format!("mise at {} doesn't report version {MISE_VERSION}", self.mise_bin().display()));
        }
        Ok(self.mise_bin())
    }

    /// `mise install tool@version`, then its bin dirs.
    pub async fn ensure_tool(&self, tool: &str, version: &str, log: &mut (dyn FnMut(&str) + Send)) -> Result<(Vec<PathBuf>, bool), String> {
        let mise = self.ensure_mise(log).await?.display().to_string();
        let spec = format!("{tool}@{version}");
        // Already there? `bin-paths` answers only for installed versions.
        let before = self.bin_paths(&mise, &spec).await;
        let mut installed = false;
        if before.is_empty() {
            log(&format!("installing {spec} (mise)"));
            let o = self.run(&mise, &["install", &spec], INSTALL_LIMIT).await?;
            if !o.ok {
                return Err(format!("mise install {spec} failed: {}", last_lines(&o.stderr, 6)));
            }
            installed = true;
        }
        let dirs = self.bin_paths(&mise, &spec).await;
        if dirs.is_empty() {
            return Err(format!("mise installed {spec} but reports no bin dir"));
        }
        // The commands this tool must provide (cmake → `cmake`): checked
        // now, so the job fails here instead of a later `command not found`.
        for want in needs::expected_bins(tool) {
            if !dirs.iter().any(|d| is_executable(&d.join(want))) {
                return Err(format!(
                    "mise installed {spec} but there is no `{want}` in {}",
                    dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        Ok((dirs, installed))
    }

    async fn bin_paths(&self, mise: &str, spec: &str) -> Vec<PathBuf> {
        match self.run(mise, &["bin-paths", spec], Duration::from_secs(60)).await {
            Ok(o) if o.ok => o
                .stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .collect(),
            _ => Vec::new(),
        }
    }

    // ── rust ─────────────────────────────────────────────────────────

    fn rustup(&self) -> PathBuf {
        self.cargo_home().join("bin/rustup")
    }

    /// How this node gets Rust (offer `tools`
    /// [`needs::TOOLS_RUST_INSTALLER`]): `rustup` when its own rustup is
    /// there (or a rustup is on the job PATH); `system` when the job PATH
    /// has a `rustc` from outside the node's toolchains (a distro
    /// package) and no rustup anywhere: then no compile target can be
    /// added here (`rust_target_unsupported`). `None`: no Rust yet (the
    /// first Rust need installs rustup).
    pub fn rust_installer(&self) -> Option<&'static str> {
        if self.rustup().is_file() || crate::sysinfo::which("rustup", &self.base_env).is_some() {
            return Some("rustup");
        }
        let rustc = crate::sysinfo::which("rustc", &self.base_env)?;
        if Path::new(&rustc).starts_with(&self.root) {
            return None;
        }
        Some("system")
    }

    /// rustup itself (no toolchain), via the official rustup-init script.
    pub async fn ensure_rustup(&self, log: &mut (dyn FnMut(&str) + Send)) -> Result<(), String> {
        if self.rustup().is_file() {
            return Ok(());
        }
        log(&format!("installing rustup into {}", self.cargo_home().display()));
        for d in [self.cargo_home(), self.rustup_home()] {
            std::fs::create_dir_all(&d).map_err(|e| format!("create {}: {e}", d.display()))?;
        }
        let script = self.root.join(format!(".rustup-init-{}.sh", std::process::id()));
        download(self, RUSTUP_INIT_URL, &script).await?;
        let o = self
            .run(
                "/bin/sh",
                &[&script.display().to_string(), "-y", "--no-modify-path", "--profile", "minimal", "--default-toolchain", "none"],
                INSTALL_LIMIT,
            )
            .await;
        let _ = std::fs::remove_file(&script);
        let o = o?;
        if !o.ok || !self.rustup().is_file() {
            return Err(format!("rustup-init failed: {}", last_lines(&o.stderr, 6)));
        }
        Ok(())
    }

    /// `rustup toolchain install <channel>` (+ components / targets).
    /// Returns true when something was installed. A toolchain that's
    /// already there only gets its missing components and targets
    /// (`rustup component add` / `target add`, CE-A10): never a reinstall,
    /// which would also move `stable` to a newer release under the jobs.
    pub async fn ensure_rust(&self, rt: &RustToolchain, log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, String> {
        self.ensure_rustup(log).await?;
        let rustup = self.rustup().display().to_string();
        let list = self.run(&rustup, &["toolchain", "list"], Duration::from_secs(60)).await?;
        let have = list.stdout.lines().any(|l| {
            let name = l.split_whitespace().next().unwrap_or("");
            name == rt.channel || name.starts_with(&format!("{}-", rt.channel))
        });
        // A moving channel gets its newest release for new jobs (after a
        // k2-node update, or once a day), never while a running job uses it:
        // `rustup toolchain install` replaces it in place.
        if have && self.rust_refresh_due(&rt.channel, unix_now()) {
            if self.busy_channels.iter().any(|c| *c == rt.channel) {
                log(&format!("Rust {} stays at its installed release while a running job uses it; the next job after it ends gets the newest", rt.channel));
            } else {
                log(&format!("updating Rust {} to its newest release (rustup)", rt.channel));
                let mut args: Vec<String> =
                    vec!["toolchain".into(), "install".into(), rt.channel.clone(), "--profile".into(), "minimal".into(), "--no-self-update".into()];
                for c in &rt.components {
                    args.push("--component".into());
                    args.push(rustup_component_name(c).to_string());
                }
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let o = self.run(&rustup, &argv, INSTALL_LIMIT).await?;
                if !o.ok {
                    // The installed release still works: the job goes on with it.
                    log(&format!("updating Rust {} failed (the installed release is used): {}", rt.channel, last_lines(&o.stderr, 4)));
                } else {
                    self.mark_refreshed(&rt.channel, unix_now());
                    return Ok(true);
                }
            }
        }
        if have && rt.components.is_empty() && rt.targets.is_empty() {
            return Ok(false);
        }
        if have {
            let dir = self.toolchain_dir(&rt.channel);
            let installed = dir.as_deref().map(toolchain_components).unwrap_or_default();
            let missing_c: Vec<&String> = rt.components.iter().filter(|c| !installed.iter().any(|i| i == needs::canonical_component(c))).collect();
            let missing_t: Vec<&String> = rt.targets.iter().filter(|t| !dir.as_ref().is_some_and(|d| d.join("lib/rustlib").join(t.as_str()).is_dir())).collect();
            if missing_c.is_empty() && missing_t.is_empty() {
                return Ok(false);
            }
            if !missing_c.is_empty() {
                let names: Vec<String> = missing_c.iter().map(|c| rustup_component_name(c).to_string()).collect();
                log(&format!("adding Rust components {} to {} (rustup)", names.join(", "), rt.channel));
                let mut args: Vec<&str> = vec!["component", "add", "--toolchain", &rt.channel];
                args.extend(names.iter().map(String::as_str));
                let o = self.run(&rustup, &args, INSTALL_LIMIT).await?;
                if !o.ok {
                    return Err(format!("rustup component add {} failed: {}", names.join(" "), last_lines(&o.stderr, 6)));
                }
            }
            if !missing_t.is_empty() {
                log(&format!("adding Rust targets {} to {} (rustup)", missing_t.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "), rt.channel));
                let mut args: Vec<&str> = vec!["target", "add", "--toolchain", &rt.channel];
                args.extend(missing_t.iter().map(|t| t.as_str()));
                let o = self.run(&rustup, &args, INSTALL_LIMIT).await?;
                if !o.ok {
                    return Err(format!("rustup target add failed: {}", last_lines(&o.stderr, 6)));
                }
            }
            return Ok(true);
        }
        log(&format!("installing Rust {} (rustup)", rt.channel));
        let mut args: Vec<String> =
            vec!["toolchain".into(), "install".into(), rt.channel.clone(), "--profile".into(), "minimal".into(), "--no-self-update".into()];
        for c in &rt.components {
            args.push("--component".into());
            args.push(rustup_component_name(c).to_string());
        }
        for t in &rt.targets {
            args.push("--target".into());
            args.push(t.clone());
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = self.run(&rustup, &argv, INSTALL_LIMIT).await?;
        if !o.ok {
            return Err(format!("rustup toolchain install {} failed: {}", rt.channel, last_lines(&o.stderr, 6)));
        }
        if MOVING_CHANNELS.contains(&rt.channel.as_str()) {
            self.mark_refreshed(&rt.channel, unix_now());
        }
        Ok(!have)
    }

    /// The installed toolchain dir a channel names (`stable` →
    /// `toolchains/rustup/toolchains/stable-aarch64-apple-darwin`).
    pub fn toolchain_dir(&self, channel: &str) -> Option<PathBuf> {
        let rd = std::fs::read_dir(self.rustup_home().join("toolchains")).ok()?;
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let name = names.into_iter().find(|n| n == channel || n.starts_with(&format!("{channel}-")))?;
        Some(self.rustup_home().join("toolchains").join(name))
    }

    /// The prebuilt `cargo-nextest` into `toolchains/cargo/bin`.
    pub async fn ensure_nextest(&self, log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, String> {
        let dest = self.cargo_home().join("bin/cargo-nextest");
        if dest.is_file() {
            return Ok(false);
        }
        let which = match (self.os, self.arch) {
            ("macos", _) => "mac",
            ("linux", "x86_64") => "linux",
            ("linux", "aarch64") => "linux-arm",
            (o, a) => return Err(format!("no prebuilt cargo-nextest for {o}-{a}")),
        };
        log("installing cargo-nextest (prebuilt)");
        let bin = self.cargo_home().join("bin");
        std::fs::create_dir_all(&bin).map_err(|e| format!("create {}: {e}", bin.display()))?;
        let tgz = self.root.join(format!(".nextest-{}.tar.gz", std::process::id()));
        download(self, &format!("https://get.nexte.st/latest/{which}"), &tgz).await?;
        let o = self
            .run("/usr/bin/tar", &["-xzf", &tgz.display().to_string(), "-C", &bin.display().to_string(), "cargo-nextest"], Duration::from_secs(120))
            .await;
        let _ = std::fs::remove_file(&tgz);
        let o = o?;
        if !o.ok || !dest.is_file() {
            return Err(format!("unpack cargo-nextest failed: {}", last_lines(&o.stderr, 4)));
        }
        Ok(true)
    }

    // ── native tools: the Android NDK and cargo-ndk (pinned) ────────

    /// `toolchains/android-ndk/<release>` (`r27d`).
    pub fn ndk_dir(&self, release: &str) -> PathBuf {
        self.root.join(needs::ANDROID_NDK).join(release)
    }
    /// `toolchains/cargo-ndk/<version>`.
    pub fn cargo_ndk_dir(&self) -> PathBuf {
        self.root.join(needs::CARGO_NDK).join(needs::CARGO_NDK_VERSION)
    }

    /// The native tools run on Linux x86_64 only.
    pub fn native_host(&self, tool: &str) -> Result<(), String> {
        if (self.os, self.arch) == needs::NATIVE_TOOLS_HOST {
            return Ok(());
        }
        Err(format!(
            "{tool} runs on Linux x86_64 nodes only (the Android NDK ships a linux-x86_64 host and no linux-aarch64; on macOS it's a .dmg); this node is {}-{}: place the job on a Linux x86_64 node",
            self.os, self.arch
        ))
    }

    /// The pinned NDK in `toolchains/android-ndk/<release>`: its folder,
    /// and whether it was installed now.
    pub async fn ensure_android_ndk(&self, version: &str, log: &mut (dyn FnMut(&str) + Send)) -> Result<(PathBuf, bool), String> {
        self.native_host(needs::ANDROID_NDK)?;
        let pin = needs::ndk_pin(version).ok_or_else(|| needs::check_native(needs::ANDROID_NDK, version).err().unwrap_or_default())?;
        let dest = self.ndk_dir(pin.release);
        if ndk_revision(&dest).as_deref() == Some(pin.revision) {
            return Ok((dest, false));
        }
        log(&format!("installing the Android NDK {} ({}, {} MB, sha256 checked) into {}", pin.release, pin.revision, pin.bytes >> 20, dest.display()));
        let stage = self.stage(needs::ANDROID_NDK)?;
        let zip = stage.join(pin.url.rsplit('/').next().unwrap_or("ndk.zip"));
        if let Err(e) = download_within(self, pin.url, &zip, NDK_DOWNLOAD_LIMIT).await {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
        self.place_ndk(&zip, pin, &stage, &dest).await.map(|d| (d, true))
    }

    /// Check the zip's sha256, unpack it in `stage`, check its revision,
    /// and rename it in as `dest`: a job never sees a half-unpacked NDK.
    /// `stage` is removed either way.
    async fn place_ndk(&self, zip: &Path, pin: &needs::NdkPin, stage: &Path, dest: &Path) -> Result<PathBuf, String> {
        let r = async {
            check_sha256(zip, pin.sha256)?;
            unzip(self, zip, stage).await?;
            let _ = std::fs::remove_file(zip);
            let top = stage.join(format!("android-ndk-{}", pin.release));
            match ndk_revision(&top) {
                Some(r) if r == pin.revision => {}
                got => return Err(format!("the unpacked NDK says revision {got:?}, expected {}", pin.revision)),
            }
            swap_in(&top, dest, stage)
        }
        .await;
        let _ = std::fs::remove_dir_all(stage);
        r.map(|_| dest.to_path_buf())
    }

    /// The pinned prebuilt cargo-ndk in `toolchains/cargo-ndk/<version>`.
    pub async fn ensure_cargo_ndk(&self, log: &mut (dyn FnMut(&str) + Send)) -> Result<(PathBuf, bool), String> {
        self.native_host(needs::CARGO_NDK)?;
        let dest = self.cargo_ndk_dir();
        if is_executable(&dest.join("cargo-ndk")) {
            return Ok((dest, false));
        }
        log(&format!("installing cargo-ndk {} (prebuilt, sha256 checked)", needs::CARGO_NDK_VERSION));
        let stage = self.stage(needs::CARGO_NDK)?;
        let tgz = stage.join("cargo-ndk.tgz");
        if let Err(e) = download(self, needs::CARGO_NDK_URL, &tgz).await {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
        let top = format!("cargo-ndk-x86_64-unknown-linux-musl-v{}", needs::CARGO_NDK_VERSION);
        self.place_cargo_ndk(&tgz, needs::CARGO_NDK_SHA256, &top, &stage, &dest).await.map(|d| (d, true))
    }

    async fn place_cargo_ndk(&self, tgz: &Path, sha: &str, top: &str, stage: &Path, dest: &Path) -> Result<PathBuf, String> {
        let r = async {
            check_sha256(tgz, sha)?;
            let o = self.run("/usr/bin/tar", &["-xzf", &tgz.display().to_string(), "-C", &stage.display().to_string()], Duration::from_secs(120)).await?;
            if !o.ok {
                return Err(format!("unpack cargo-ndk: {}", last_lines(&o.stderr, 4)));
            }
            let top = stage.join(top);
            if !is_executable(&top.join("cargo-ndk")) {
                return Err(format!("no executable cargo-ndk in {}", top.display()));
            }
            swap_in(&top, dest, stage)
        }
        .await;
        let _ = std::fs::remove_dir_all(stage);
        r.map(|_| dest.to_path_buf())
    }

    /// A fresh `toolchains/.<what>-stage-<pid>` (same filesystem as the
    /// destination, so the final rename is atomic).
    fn stage(&self, what: &str) -> Result<PathBuf, String> {
        let stage = self.root.join(format!(".{what}-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&self.root).map_err(|e| format!("create {}: {e}", self.root.display()))?;
        crate::util::mkdir_mode(&stage, 0o755)?;
        Ok(stage)
    }

    /// `mise:node@22.23.4` / `android-ndk:r27d` / `cargo-ndk:4.1.2` for a
    /// path inside an install (the tool-cache LRU's key).
    fn install_key(&self, p: &Path) -> Option<String> {
        self.mise_key(p).or_else(|| {
            needs::NATIVE_TOOLS.iter().find_map(|t| {
                let rel = p.strip_prefix(self.root.join(t)).ok()?;
                let v = rel.components().next()?.as_os_str().to_string_lossy().into_owned();
                Some(format!("{t}:{v}"))
            })
        })
    }

    // ── resolve ──────────────────────────────────────────────────────

    /// Everything the job needs, installed. `explicit` failures fail the
    /// job; repo-declared ones become notes.
    pub async fn resolve(&self, explicit: &[Need], repo: &[(Need, String)], rust_file: Option<&RustToolchain>, log: &mut (dyn FnMut(&str) + Send)) -> Result<Resolved, Fail> {
        let _installing = Installing::start(&self.root);
        let mut out = Resolved::default();
        // Profiles (protocol 7): their tools join the explicit needs (an
        // explicit need for the same tool wins), their system names the
        // package list. This node's own copy decides what a profile holds.
        let mut explicit: Vec<Need> = explicit.to_vec();
        let mut profile_sys: Vec<String> = Vec::new();
        for n in explicit.clone() {
            if let Need::Profile { name, version } = &n {
                let p = k2_node_proto::profiles::get(name, *version)
                    .ok_or_else(|| (k2_node_proto::profiles::PROFILE_UNKNOWN, k2_node_proto::profiles::resolve(&n.token()["profile:".len()..]).err().unwrap_or_default()))?;
                profile_sys.extend(p.all_sys());
                for t in p.all_tools() {
                    let tn = needs::parse(&t).map_err(|e| (codes::TOOL_INSTALL_FAILED, format!("{}: {e}", n.token())))?;
                    if !explicit.iter().any(|x| x.key() == tn.key()) {
                        explicit.push(tn);
                    }
                }
            }
        }
        let explicit = &explicit[..];
        let explicit_keys: Vec<String> = explicit.iter().map(Need::key).collect();
        let mut work: Vec<(Need, Option<String>)> = explicit.iter().cloned().map(|n| (n, None)).collect();
        for (n, src) in repo {
            if !explicit_keys.contains(&n.key()) && !work.iter().any(|(w, _)| w.key() == n.key()) {
                work.push((n.clone(), Some(src.clone())));
            }
        }
        // System packages first, all at once (one root call).
        let mut sys: Vec<String> = work
            .iter()
            .filter_map(|(n, src)| match n {
                Need::System { package } if src.is_none() => Some(package.clone()),
                _ => None,
            })
            .collect();
        // The plan's list (the server expanded the profiles) and this
        // node's own expansion: every name still goes through this node's
        // policy and root's guards.
        for p in self.plan_packages.iter().chain(profile_sys.iter()) {
            if !sys.contains(p) {
                sys.push(p.clone());
            }
        }
        if !sys.is_empty() {
            let did = crate::packages::ensure(self, &sys, log).await?;
            out.installed_any |= did;
        }
        // The native tools (Android NDK, cargo-ndk) asked for explicitly on
        // a node that can't run them: tool_unsupported, not an install failure.
        for (n, src) in &work {
            if let (Need::Tool { tool, .. }, None) = (n, src) {
                if needs::NATIVE_TOOLS.contains(&tool.as_str()) {
                    self.native_host(tool).map_err(|e| (codes::TOOL_UNSUPPORTED, format!("{}: {e}", n.token())))?;
                }
            }
        }
        let mut rust_env: Option<String> = None;
        for (n, src) in &work {
            let required = src.is_none();
            let r: Result<bool, String> = match n {
                Need::System { package } => {
                    // Repo-declared system packages are never installed:
                    // only an explicit need may ask for root.
                    if !crate::packages::installed(self.os, package) {
                        out.notes.push(format!("{} (from {}) isn't installed; ask for it with --needs sys:{package}", package, src.clone().unwrap_or_default()));
                    }
                    Ok(false)
                }
                Need::Tool { tool, version } if tool == needs::ANDROID_NDK => match self.ensure_android_ndk(version, log).await {
                    Ok((dir, did)) => {
                        let (bin, env) = ndk_env(&dir, needs::ANDROID_DEFAULT_API);
                        if !out.path_prepend.contains(&bin) {
                            out.path_prepend.push(bin);
                        }
                        out.env.extend(env);
                        out.notes.push(format!(
                            "Android NDK {}: ANDROID_NDK_HOME and the aarch64-linux-android{api} linker are set (API {api}; your own --env CC_/CARGO_TARGET_…_LINKER/CARGO_NDK_PLATFORM wins)",
                            dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            api = needs::ANDROID_DEFAULT_API
                        ));
                        Ok(did)
                    }
                    Err(e) => Err(e),
                },
                Need::Tool { tool, .. } if tool == needs::CARGO_NDK => match self.ensure_cargo_ndk(log).await {
                    Ok((dir, did)) => {
                        if !out.path_prepend.contains(&dir) {
                            out.path_prepend.push(dir);
                        }
                        Ok(did)
                    }
                    Err(e) => Err(e),
                },
                Need::Tool { tool, version } => match self.ensure_tool(tool, version, log).await {
                    Ok((dirs, did)) => {
                        for d in dirs {
                            if !out.path_prepend.contains(&d) {
                                out.path_prepend.push(d);
                            }
                        }
                        Ok(did)
                    }
                    Err(e) => Err(e),
                },
                Need::Rust { channel, components, targets } => {
                    // A distro Rust with no rustup can't add a target: a
                    // named refusal, never a half-made install.
                    if !targets.is_empty() && self.rust_installer() == Some("system") {
                        let why = system_rust_refusal(&n.token());
                        if required {
                            return Err((codes::RUST_TARGET_UNSUPPORTED, why));
                        }
                        out.notes.push(format!("warning: {why} (from {})", src.clone().unwrap_or_default()));
                        continue;
                    }
                    // Protocol 7 (§8): clippy and rustfmt come by default;
                    // `+minimal` opts out.
                    let mut comps: Vec<String> = components.iter().filter(|c| *c != needs::RUST_MINIMAL).cloned().collect();
                    if n.rust_wants_defaults() {
                        for d in needs::DEFAULT_RUST_COMPONENTS {
                            if !comps.iter().any(|c| c == d) {
                                comps.push(d.to_string());
                            }
                        }
                    }
                    let rt = RustToolchain { channel: channel.clone(), components: comps, targets: targets.clone() };
                    let r = self.ensure_rust(&rt, log).await;
                    if r.is_ok() && required {
                        rust_env = Some(channel.clone());
                    }
                    // CE-16: llvm-symbolizer, llvm-profdata, llvm-cov on
                    // the job's PATH, and the sanitizers' symbolizer.
                    if r.is_ok() && components.iter().any(|c| c == "llvm-tools") {
                        match self.toolchain_dir(channel).and_then(|d| llvm_bin_dir(&d)) {
                            Some(bin) => {
                                let sym = bin.join("llvm-symbolizer");
                                if is_executable(&sym) {
                                    out.env.push(("ASAN_SYMBOLIZER_PATH".into(), sym.display().to_string()));
                                }
                                if !out.path_prepend.contains(&bin) {
                                    out.path_prepend.push(bin);
                                }
                            }
                            None => out.notes.push(format!("rust@{channel}+llvm-tools: no LLVM tools folder in the toolchain")),
                        }
                    }
                    r
                }
                Need::Nextest => self.ensure_nextest(log).await,
                // Expanded above.
                Need::Profile { .. } => Ok(false),
            };
            match r {
                Ok(did) => out.installed_any |= did,
                Err(e) if required => {
                    let code = codes::TOOL_INSTALL_FAILED;
                    return Err((code, format!("{}: {e}", n.token())));
                }
                Err(e) => out.notes.push(format!("warning: {} (from {}): {e}", n.token(), src.clone().unwrap_or_default())),
            }
        }
        // rust-toolchain(.toml): rustup honours it by itself; install it
        // up front so the first cargo call isn't a silent download (and
        // works with rustup versions that no longer auto-install).
        if let Some(rt) = rust_file {
            if rust_env.is_none() && !explicit_keys.iter().any(|k| k == "rust") {
                match self.ensure_rust(rt, log).await {
                    Ok(did) => out.installed_any |= did,
                    Err(e) => out.notes.push(format!("warning: rust-toolchain file ({}): {e}", rt.channel)),
                }
            }
        }
        // CE-18: what this job used, for the tool-cache cap's LRU.
        let mut used: Vec<String> = out.path_prepend.iter().filter_map(|p| self.install_key(p)).collect();
        for ch in rust_env.iter().chain(rust_file.map(|r| &r.channel)) {
            if let Some(name) = self.toolchain_dir(ch).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())) {
                used.push(format!("rust:{name}"));
            }
        }
        self.mark_used(&used, unix_now());
        if let Some(c) = rust_env {
            out.env.push(("RUSTUP_TOOLCHAIN".into(), c));
        }
        if self.mise_bin().is_file() {
            let bin = self.mise_bin().parent().map(Path::to_path_buf).unwrap_or_default();
            if !out.path_prepend.contains(&bin) {
                out.path_prepend.push(bin);
            }
            out.env.extend(self.mise_env());
        }
        Ok(out)
    }

    // ── the tool-cache cap (CE-18) ───────────────────────────────────

    fn last_used_path(&self) -> PathBuf {
        self.root.join(LAST_USED_FILE)
    }

    fn read_last_used(&self) -> BTreeMap<String, i64> {
        small_read(&self.last_used_path()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Note that a job used these versions now (the LRU's clock).
    pub fn mark_used(&self, keys: &[String], now: i64) {
        if keys.is_empty() {
            return;
        }
        let mut m = self.read_last_used();
        for k in keys {
            m.insert(k.clone(), now);
        }
        // Forget versions that are gone (the file stays small).
        let have: Vec<String> = self.installed_versions_shallow().into_iter().map(|(k, _)| k).collect();
        m.retain(|k, _| have.contains(k) || keys.contains(k));
        if let Ok(text) = serde_json::to_string(&m) {
            let tmp = self.root.join(format!(".{LAST_USED_FILE}.{}", std::process::id()));
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, self.last_used_path());
            }
        }
    }

    /// `mise:node@22.23.4` for a path inside a mise install.
    fn mise_key(&self, p: &Path) -> Option<String> {
        let rel = p.strip_prefix(self.mise_data().join("installs")).ok()?;
        let mut it = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
        let (tool, ver) = (it.next()?, it.next()?);
        Some(format!("mise:{tool}@{ver}"))
    }

    /// Every installed version's key and folder (no sizes): mise's
    /// `installs/<tool>/<version>` (real dirs, not its alias links) and
    /// rustup's `toolchains/<name>`.
    fn installed_versions_shallow(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let real_dirs = |p: &Path| -> Vec<(String, PathBuf)> {
            std::fs::read_dir(p)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                        .filter(|(n, _)| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default()
        };
        for (tool, tdir) in real_dirs(&self.mise_data().join("installs")) {
            for (ver, vdir) in real_dirs(&tdir) {
                out.push((format!("mise:{tool}@{ver}"), vdir));
            }
        }
        for (tc, dir) in real_dirs(&self.rustup_home().join("toolchains")) {
            out.push((format!("rust:{tc}"), dir));
        }
        for t in needs::NATIVE_TOOLS {
            for (v, dir) in real_dirs(&self.root.join(t)) {
                out.push((format!("{t}:{v}"), dir));
            }
        }
        out.sort();
        out
    }

    /// Every installed version with its size and last use (blocking: walks
    /// the folders).
    pub fn installed_versions(&self) -> Vec<ToolVersion> {
        let used = self.read_last_used();
        self.installed_versions_shallow()
            .into_iter()
            .map(|(key, path)| {
                let last_used = used.get(&key).copied().unwrap_or_else(|| mtime_secs(&path));
                ToolVersion { bytes: tree_bytes(&path), key, path, last_used }
            })
            .collect()
    }

    /// rustup's default toolchain (jobs without a Rust need use it).
    fn default_rust(&self) -> Option<String> {
        let s = small_read(&self.rustup_home().join("settings.toml"))?;
        let t: toml::Table = s.parse().ok()?;
        t.get("default_toolchain").and_then(|v| v.as_str()).map(str::to_string)
    }

    /// CE-18: over `budget`, remove the least recently used versions that
    /// no need in `in_use` (running and queued jobs') asks for, that aren't
    /// rustup's default toolchain, and that weren't used in the last hour.
    /// Returns what went (blocking).
    pub fn gc_to_budget(&self, budget: u64, in_use: &[Need], now: i64) -> Vec<ToolVersion> {
        let versions = self.installed_versions();
        let default_rust = self.default_rust();
        let keep = |v: &ToolVersion| {
            now - v.last_used < 3600
                || version_needed(&v.key, in_use)
                || default_rust.as_deref().is_some_and(|d| v.key == format!("rust:{d}") || v.key.starts_with(&format!("rust:{d}-")))
        };
        let mut removed = Vec::new();
        for i in lru_victims(&versions, budget, keep) {
            let v = &versions[i];
            if std::fs::remove_dir_all(&v.path).is_ok() {
                if let Some(tc) = v.key.strip_prefix("rust:") {
                    let _ = std::fs::remove_file(self.rustup_home().join("update-hashes").join(tc));
                }
                removed.push(v.clone());
            }
        }
        removed
    }

    /// Installed mise tools for the offer (`mise:node` → `22.23.3`).
    pub fn installed_tools(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if self.mise_bin().is_file() {
            out.insert("mise".into(), MISE_VERSION.into());
        }
        let installs = self.mise_data().join("installs");
        if let Ok(rd) = std::fs::read_dir(&installs) {
            for e in rd.flatten() {
                let tool = e.file_name().to_string_lossy().into_owned();
                let mut versions: Vec<String> = std::fs::read_dir(e.path())
                    .map(|vs| {
                        vs.flatten()
                            .filter(|v| v.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false))
                            .map(|v| v.file_name().to_string_lossy().into_owned())
                            .filter(|v| !v.starts_with('.'))
                            .collect()
                    })
                    .unwrap_or_default();
                versions.sort();
                if !versions.is_empty() {
                    out.insert(format!("mise:{tool}"), versions.join(","));
                }
            }
        }
        if let Some(how) = self.rust_installer() {
            out.insert(needs::TOOLS_RUST_INSTALLER.into(), how.into());
        }
        // The native tools: the NDK by its revision (`27.3.13750724`, what
        // `android-ndk@27` matches), cargo-ndk by its version.
        let dirs_of = |t: &str| -> Vec<PathBuf> {
            let mut v: Vec<PathBuf> = std::fs::read_dir(self.root.join(t))
                .map(|rd| rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink())).map(|e| e.path()).collect())
                .unwrap_or_default();
            v.sort();
            v
        };
        let ndks: Vec<String> = dirs_of(needs::ANDROID_NDK).iter().filter_map(|d| ndk_revision(d)).collect();
        if !ndks.is_empty() {
            out.insert(needs::ANDROID_NDK.into(), ndks.join(","));
        }
        let cndk: Vec<String> = dirs_of(needs::CARGO_NDK)
            .iter()
            .filter(|d| is_executable(&d.join("cargo-ndk")))
            .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if !cndk.is_empty() {
            out.insert(needs::CARGO_NDK.into(), cndk.join(","));
        }
        if let Ok(rd) = std::fs::read_dir(self.rustup_home().join("toolchains")) {
            let mut tcs: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            tcs.sort();
            if !tcs.is_empty() {
                out.insert("rust-toolchains".into(), tcs.join(","));
            }
            // CE-A10 / CE-1: per toolchain, its components (the ones a need
            // may name) and its rustc version, read from the toolchain's
            // own files (no rustup run).
            for tc in &tcs {
                let dir = self.rustup_home().join("toolchains").join(tc);
                let comps = toolchain_components(&dir);
                if !comps.is_empty() {
                    out.insert(format!("rust-components:{tc}"), comps.join(","));
                }
                // Its compile targets (the host's included).
                let targets = toolchain_targets(&dir);
                if !targets.is_empty() {
                    out.insert(format!("rust-targets:{tc}"), targets.join(","));
                }
                if let Some(v) = toolchain_rustc_version(&dir) {
                    out.insert(format!("rustc:{tc}"), v);
                }
            }
        }
        out
    }
}

/// The name rustup installs a need's component under: `llvm-tools` is
/// `llvm-tools-preview` (rustup renames it on newer toolchains, and older
/// ones know only that name).
pub fn rustup_component_name(c: &str) -> &str {
    match c {
        "llvm-tools" => "llvm-tools-preview",
        other => other,
    }
}

/// The components a toolchain has installed, as a need names them
/// ([`needs::RUST_COMPONENTS`] only, sorted), from its
/// `lib/rustlib/components` list (`clippy-preview-x86_64-unknown-linux-gnu`
/// → `clippy`).
pub fn toolchain_components(dir: &Path) -> Vec<String> {
    let Some(text) = small_read(&dir.join("lib/rustlib/components")) else { return Vec::new() };
    components_from_list(&text)
}

/// The compile targets a toolchain has (sorted): each `rust-std-<triple>`
/// in its `lib/rustlib/components` list, the host's included
/// (`rust-std-wasm32-unknown-unknown` → `wasm32-unknown-unknown`).
pub fn toolchain_targets(dir: &Path) -> Vec<String> {
    let Some(text) = small_read(&dir.join("lib/rustlib/components")) else { return Vec::new() };
    targets_from_list(&text)
}

/// [`toolchain_targets`] on the file's text.
pub fn targets_from_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> =
        text.lines().filter_map(|l| l.trim().strip_prefix("rust-std-")).filter(|t| needs::valid_target(t)).map(str::to_string).collect();
    out.sort();
    out.dedup();
    out
}

/// The refusal for a Rust target on a node whose Rust is a system package.
pub fn system_rust_refusal(token: &str) -> String {
    format!(
        "{token}: this node's Rust is a system package with no rustup, so k2-node can't add compile targets here. \
         Send the job without a node name (K2 places it on a node with rustup), or name another node"
    )
}

/// [`toolchain_components`] on the file's text.
pub fn components_from_list(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // The host triple: `rustc-<host>` is in every toolchain.
    let host = lines.iter().find_map(|l| l.strip_prefix("rustc-")).unwrap_or("");
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        let base = if host.is_empty() { l } else { l.strip_suffix(&format!("-{host}")).unwrap_or(l) };
        let c = needs::canonical_component(base);
        if needs::RUST_COMPONENTS.contains(&c) && !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    }
    out.sort();
    out
}

/// `1.99.1` from a toolchain's channel manifest (`[pkg.rustc]` `version =
/// "1.99.1 (abc 2026-09-01)"`); a text scan, the file is large.
pub fn toolchain_rustc_version(dir: &Path) -> Option<String> {
    let p = dir.join("lib/rustlib/multirust-channel-manifest.toml");
    let m = std::fs::symlink_metadata(&p).ok()?;
    if !m.is_file() || m.len() > 8 << 20 {
        return None;
    }
    rustc_version_from_manifest(&std::fs::read_to_string(p).ok()?)
}

/// [`toolchain_rustc_version`] on the manifest's text.
pub fn rustc_version_from_manifest(text: &str) -> Option<String> {
    let mut in_rustc = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_rustc = l == "[pkg.rustc]";
            continue;
        }
        if in_rustc {
            if let Some(v) = l.strip_prefix("version = \"") {
                let v = v.split([' ', '"']).next()?;
                return plain_word(v).then(|| v.to_string());
            }
        }
    }
    None
}

/// The toolchain's LLVM tools folder (`lib/rustlib/<host>/bin`), if any.
pub fn llvm_bin_dir(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir.join("lib/rustlib")).ok()?;
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path().join("bin"))
        .filter(|b| b.join("llvm-profdata").is_file() || b.join("llvm-cov").is_file() || b.join("llvm-symbolizer").is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// What one probe needs: the program, its args, how to read the version.
struct Probe {
    key: String,
    prog: String,
    args: Vec<&'static str>,
}

/// CE-4: what the job's PATH really resolves to, run with the job's env in
/// its working directory (so a `rust-toolchain.toml` override counts),
/// 5 s per tool, after the toolchains are resolved and before the command
/// starts. Never installs anything (`RUSTUP_AUTO_INSTALL=0`). A tool not on
/// the PATH, or one whose probe fails, is absent.
pub async fn tools_used(env: &[(String, String)], cwd: &Path, explicit: &[Need]) -> BTreeMap<String, String> {
    let limit = Duration::from_secs(5);
    let mut env: Vec<(String, String)> = env.to_vec();
    env.retain(|(k, _)| k != "RUSTUP_AUTO_INSTALL");
    env.push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    let cwd = cwd.is_dir().then_some(cwd);
    let mut probes = vec![
        Probe { key: "rustc".into(), prog: "rustc".into(), args: vec!["-V"] },
        Probe { key: "cargo".into(), prog: "cargo".into(), args: vec!["-V"] },
    ];
    if crate::sysinfo::which("cargo-nextest", &env).is_some() {
        probes.push(Probe { key: "cargo-nextest".into(), prog: "cargo".into(), args: vec!["nextest", "--version"] });
    }
    for n in explicit {
        if let Need::Tool { tool, .. } = n {
            if let Some(bin) = needs::expected_bins(tool).first() {
                let args = if *tool == "go" { vec!["version"] } else { vec!["--version"] };
                probes.push(Probe { key: tool.clone(), prog: bin.to_string(), args });
            }
        }
    }
    let mut out = BTreeMap::new();
    for p in probes {
        let Some(path) = crate::sysinfo::which(&p.prog, &env) else { continue };
        if let Ok(o) = run_cmd(&path, &p.args, &env, cwd, limit).await {
            if o.ok {
                if let Some(v) = tool_version(&o.stdout).or_else(|| tool_version(&o.stderr)) {
                    out.insert(p.key, v);
                }
            }
        }
    }
    // The active Rust toolchain and its components.
    if let Some(rustup) = crate::sysinfo::which("rustup", &env) {
        if let Ok(o) = run_cmd(&rustup, &["show", "active-toolchain"], &env, cwd, limit).await {
            if let Some(tc) = o.stdout.split_whitespace().next().filter(|t| o.ok && plain_word(t)) {
                out.insert("rust-toolchain".into(), tc.to_string());
                if let Some(home) = env.iter().rev().find(|(k, _)| k == "RUSTUP_HOME").map(|(_, v)| PathBuf::from(v)) {
                    let comps = toolchain_components(&home.join("toolchains").join(tc));
                    if !comps.is_empty() {
                        out.insert("rust-components".into(), comps.join(","));
                    }
                    let targets = toolchain_targets(&home.join("toolchains").join(tc));
                    if !targets.is_empty() {
                        out.insert("rust-targets".into(), targets.join(","));
                    }
                }
            }
        }
    }
    out
}

/// `rustc 1.99.0 (abc 2026-09-01)` → `1.99.0`; `go version go1.23.1 linux/amd64`
/// → `1.23.1`; `Python 3.12.1` → `3.12.1`; else the first line, short.
pub fn tool_version(s: &str) -> Option<String> {
    for w in s.split_whitespace() {
        let w = w.trim_start_matches("go").trim_start_matches('v').trim_end_matches(',');
        if w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.') {
            return Some(w.chars().take(64).collect());
        }
    }
    s.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(80).collect())
}

/// One line for the job log: `tools: rustc 1.99.0, cargo 1.99.0, …`.
pub fn tools_line(t: &BTreeMap<String, String>) -> String {
    let order = ["rustc", "cargo", "cargo-nextest", "rust-toolchain", "rust-components", "rust-targets"];
    let mut parts: Vec<String> = order.iter().filter_map(|k| t.get(*k).map(|v| format!("{k} {v}"))).collect();
    parts.extend(t.iter().filter(|(k, _)| !order.contains(&k.as_str())).map(|(k, v)| format!("{k} {v}")));
    if parts.is_empty() {
        "tools: none found on the job's PATH".into()
    } else {
        format!("tools: {}", parts.join(", "))
    }
}

/// Commands a job may call that a node doesn't always have, and the
/// `--needs` that puts each on the job's PATH.
const HINTED_TOOLS: &[(&str, &str)] = &[
    ("python3", "python@3.12"),
    ("python", "python@3.12"),
    ("pip", "python@3.12"),
    ("pip3", "python@3.12"),
    ("uv", "uv"),
    ("uvx", "uv"),
    ("node", "node@22"),
    ("npm", "node@22"),
    ("npx", "node@22"),
    ("pnpm", "pnpm"),
    ("bun", "bun"),
    ("bunx", "bun"),
    ("deno", "deno"),
    ("go", "go"),
    ("java", "java"),
    ("cmake", "cmake@3"),
    ("cargo", "rust@stable"),
    ("rustc", "rust@stable"),
];

/// Words that start (or end) a simple command without being its program.
const PREFIX_WORDS: &[&str] = &["env", "time", "nice", "nohup", "exec", "then", "else", "elif", "do", "!", "if", "while", "until", "done", "fi", "esac"];

/// The programs a job's command plainly calls, in order: `argv[0]`, or,
/// for `sh`/`bash`/`zsh`/`dash -c SCRIPT`, the first word of each simple
/// command in SCRIPT (split at `; & | ( ) { }`, newlines and backticks),
/// past `VAR=value`, [`PREFIX_WORDS`] and bare numbers (`2>&1` splits at
/// the `&`). A word with a `/` is an explicit
/// path, not a PATH lookup, and is left out. A rough reading on purpose:
/// it only decides whether to print a hint.
pub fn commands_called(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else { return Vec::new() };
    let base = first.rsplit('/').next().unwrap_or(first);
    let script = if matches!(base, "sh" | "bash" | "zsh" | "dash" | "ksh") {
        argv.iter().position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c')).and_then(|i| argv.get(i + 1)).cloned()
    } else {
        None
    };
    let text = script.unwrap_or_else(|| argv.join(" "));
    let mut out = Vec::new();
    for seg in text.split(|c: char| matches!(c, ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`' | '\n')) {
        let word = seg
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| c == '"' || c == '\''))
            .filter(|w| !w.is_empty() && *w != "$")
            .find(|w| !PREFIX_WORDS.contains(w) && !is_assignment(w) && !w.starts_with('-') && !w.bytes().all(|b| b.is_ascii_digit()));
        if let Some(w) = word {
            if !w.contains('/') && !w.starts_with('$') && !w.starts_with('<') && !w.starts_with('>') {
                out.push(w.to_string());
            }
        }
    }
    out
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()))
}

/// Does `--needs` already ask for what `need` (a [`HINTED_TOOLS`] value)
/// names?
fn need_asked(need: &str, explicit: &[Need]) -> bool {
    let tool = need.split('@').next().unwrap_or(need);
    explicit.iter().any(|n| match n {
        Need::Tool { tool: t, .. } => t == tool,
        Need::Rust { .. } => tool == "rust",
        _ => false,
    })
}

/// One line per tool the job's command calls that isn't on the job's PATH
/// and wasn't asked for in `--needs` ("python3 isn't on this node's PATH
/// for jobs; add --needs python@3.12"), each need named once. Printed at
/// job start, before the command fails with `command not found`.
pub fn missing_tool_hints(argv: &[String], env: &[(String, String)], explicit: &[Need]) -> Vec<String> {
    let mut named: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for cmd in commands_called(argv) {
        let Some((_, need)) = HINTED_TOOLS.iter().find(|(t, _)| *t == cmd) else { continue };
        if named.contains(need) || need_asked(need, explicit) || crate::sysinfo::which(&cmd, env).is_some() {
            continue;
        }
        named.push(*need);
        out.push(format!("{cmd} isn't on this node's PATH for jobs; add --needs {need}"));
    }
    out
}

/// The LRU clock of the tool-cache cap, in the toolchains root.
const LAST_USED_FILE: &str = "k2-last-used.json";

/// One installed tool version (CE-18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolVersion {
    /// `mise:node@22.23.4` or `rust:stable-x86_64-unknown-linux-gnu`.
    pub key: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Unix seconds (a job's last use, else the folder's mtime).
    pub last_used: i64,
}

/// Does a need ask for this version? `node@22` keeps `node@22.23.4`;
/// `latest`/`lts`, a backend tool and an unknown form keep every version
/// of that tool (doubt keeps); `rust@stable` keeps `stable-<host>`.
pub fn version_needed(key: &str, needs: &[Need]) -> bool {
    needs.iter().any(|n| match n {
        Need::Rust { channel, .. } => key.strip_prefix("rust:").is_some_and(|tc| tc == channel || tc.starts_with(&format!("{channel}-"))),
        Need::Tool { tool, version } if tool == needs::ANDROID_NDK => {
            key.strip_prefix("android-ndk:").is_some_and(|r| needs::ndk_pin(version).is_some_and(|p| p.release == r))
        }
        Need::Tool { tool, version } if tool == needs::CARGO_NDK => key.strip_prefix("cargo-ndk:").is_some_and(|v| needs::cargo_ndk_matches(version) && v == needs::CARGO_NDK_VERSION),
        Need::Tool { tool, version } => {
            let Some(rest) = key.strip_prefix("mise:") else { return false };
            let Some((t, v)) = rest.split_once('@') else { return false };
            if tool.contains(':') {
                // Backend tools' folder names aren't the need's: keep any
                // folder that could be it.
                let flat: String = tool.chars().map(|c| if c == ':' || c == '/' { '-' } else { c }).collect();
                return t == flat || t.ends_with(tool.rsplit('/').next().unwrap_or(tool));
            }
            t == tool && (matches!(version.as_str(), "latest" | "lts") || v == version || v.starts_with(&format!("{version}.")))
        }
        _ => false,
    })
}

/// Which versions go (indexes, oldest use first) to get `versions` under
/// `budget`, never one `keep` says to keep.
pub fn lru_victims(versions: &[ToolVersion], budget: u64, keep: impl Fn(&ToolVersion) -> bool) -> Vec<usize> {
    let mut total: u64 = versions.iter().map(|v| v.bytes).sum();
    if total <= budget {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..versions.len()).filter(|i| !keep(&versions[*i])).collect();
    order.sort_by_key(|i| (versions[*i].last_used, versions[*i].key.clone()));
    let mut out = Vec::new();
    for i in order {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(versions[i].bytes);
        out.push(i);
    }
    out
}

/// Bytes under `p` (files' lengths, no link followed).
fn tree_bytes(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| tree_bytes(&e.path())).sum()).unwrap_or(0)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn mtime_secs(p: &Path) -> i64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn download(tc: &Toolchains, url: &str, dest: &Path) -> Result<(), String> {
    let o = tc
        .run(
            "/usr/bin/curl",
            &["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "2", "-o", &dest.display().to_string(), url],
            DOWNLOAD_LIMIT,
        )
        .await?;
    if !o.ok {
        return Err(format!("download {url} failed: {}", last_lines(&o.stderr, 3)));
    }
    Ok(())
}

/// The NDK is ~650 MB: more time than the other downloads get.
const NDK_DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);

async fn download_within(tc: &Toolchains, url: &str, dest: &Path, limit: Duration) -> Result<(), String> {
    let o = tc.run("/usr/bin/curl", &["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "2", "-o", &dest.display().to_string(), url], limit).await?;
    if !o.ok {
        return Err(format!("download {url} failed: {}", last_lines(&o.stderr, 3)));
    }
    Ok(())
}

/// Unpack a zip keeping its symlinks and modes (the NDK has both):
/// `unzip`, else `bsdtar` (libarchive; on Arch with pacman).
async fn unzip(tc: &Toolchains, zip: &Path, into: &Path) -> Result<(), String> {
    let (z, d) = (zip.display().to_string(), into.display().to_string());
    let tries: [(&str, Vec<&str>); 2] = [("unzip", vec!["-q", "-o", &z, "-d", &d]), ("bsdtar", vec!["-xf", &z, "-C", &d])];
    for (prog, args) in tries {
        let Some(bin) = crate::sysinfo::which(prog, &tc.base_env) else { continue };
        let o = tc.run(&bin, &args, INSTALL_LIMIT).await?;
        if o.ok {
            return Ok(());
        }
        return Err(format!("{prog} {}: {}", zip.display(), last_lines(&o.stderr, 4)));
    }
    Err("unpacking it needs `unzip` or `bsdtar` on this node: add --needs sys:unzip (or profile:build-base)".into())
}

/// `Pkg.Revision` from an NDK's `source.properties` (`27.3.13750724`).
pub fn ndk_revision(ndk: &Path) -> Option<String> {
    let s = small_read(&ndk.join("source.properties"))?;
    s.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "Pkg.Revision").then(|| v.trim().to_string())
    })
}

/// Rename `top` in as `dest`; a stale `dest` (an older or broken unpack)
/// is moved into `stage` first, so `dest` is never half there.
fn swap_in(top: &Path, dest: &Path, stage: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    if std::fs::symlink_metadata(dest).is_ok() {
        std::fs::rename(dest, stage.join(".old")).map_err(|e| format!("move the old {} aside: {e}", dest.display()))?;
    }
    std::fs::rename(top, dest).map_err(|e| format!("place {}: {e}", dest.display()))
}

/// The job env for an NDK at `ndk`: its LLVM bin dir (first on PATH) and
/// the variables cargo, the cc crate and cargo-ndk read, for
/// aarch64-linux-android at API level `api`. The job's own `--env` comes
/// after these, so it wins.
pub fn ndk_env(ndk: &Path, api: u32) -> (PathBuf, Vec<(String, String)>) {
    let bin = ndk.join("toolchains/llvm/prebuilt/linux-x86_64/bin");
    let d = ndk.display().to_string();
    let b = |n: &str| bin.join(n).display().to_string();
    let clang = b(&format!("aarch64-linux-android{api}-clang"));
    let env = vec![
        ("ANDROID_NDK_HOME".to_string(), d.clone()),
        ("ANDROID_NDK_ROOT".to_string(), d.clone()),
        ("ANDROID_NDK".to_string(), d.clone()),
        ("NDK_HOME".to_string(), d),
        ("CC_aarch64_linux_android".to_string(), clang.clone()),
        ("CXX_aarch64_linux_android".to_string(), format!("{clang}++")),
        ("AR_aarch64_linux_android".to_string(), b("llvm-ar")),
        ("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER".to_string(), clang),
        ("CARGO_NDK_PLATFORM".to_string(), api.to_string()),
    ];
    (bin, env)
}

fn check_sha256(p: &Path, want: &str) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| format!("open {}: {e}", p.display()))?;
    let mut h = k2_node_proto::crypto::Sha256Stream::default();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read {}: {e}", p.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let got = h.finish_hex();
    if got != want {
        return Err(format!("{} has sha256 {got}, expected {want}; refusing to run it", p.display()));
    }
    Ok(())
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

// ── what a repo declares ─────────────────────────────────────────────

/// `rust-toolchain(.toml)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustToolchain {
    pub channel: String,
    pub components: Vec<String>,
    pub targets: Vec<String>,
}

fn small_read(p: &Path) -> Option<String> {
    let m = std::fs::symlink_metadata(p).ok()?;
    if !m.is_file() || m.len() > 64 * 1024 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

fn first_line(s: &str) -> Option<String> {
    s.lines().map(|l| l.split('#').next().unwrap_or("").trim()).find(|l| !l.is_empty()).map(str::to_string)
}

fn plain_word(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `rust-toolchain.toml` / `rust-toolchain` at the repo root.
pub fn rust_toolchain_file(root: &Path) -> Option<RustToolchain> {
    if let Some(s) = small_read(&root.join("rust-toolchain.toml")) {
        let t: toml::Table = s.parse().ok()?;
        let tc = t.get("toolchain")?.as_table()?;
        let channel = tc.get("channel")?.as_str()?.to_string();
        let list = |k: &str| -> Vec<String> {
            tc.get(k)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| plain_word(x)).map(str::to_string).collect())
                .unwrap_or_default()
        };
        return plain_word(&channel).then(|| RustToolchain { channel, components: list("components"), targets: list("targets") });
    }
    let s = small_read(&root.join("rust-toolchain"))?;
    let ch = first_line(&s)?;
    plain_word(&ch).then(|| RustToolchain { channel: ch, components: vec![], targets: vec![] })
}

fn node_version_file(s: &str) -> Option<String> {
    let v = first_line(s)?;
    let v = v.trim_start_matches('v');
    Some(match v {
        "node" | "stable" | "current" => "latest".to_string(),
        x if x.starts_with("lts") => "lts".to_string(),
        x => x.to_string(),
    })
}

/// Needs a repo declares, with the file each came from. Bad entries are
/// skipped (they become notes via the `skipped` list).
pub fn declared_in_repo(root: &Path) -> (Vec<(Need, String)>, Vec<String>) {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    // .k2/compute.toml: needs = ["node@22", "sys:pkg-config"]
    if let Some(s) = small_read(&root.join(".k2/compute.toml")) {
        match s.parse::<toml::Table>() {
            Ok(t) => {
                for v in t.get("needs").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    if let Some(x) = v.as_str() {
                        found.push((x.to_string(), ".k2/compute.toml".into()));
                    }
                }
            }
            Err(e) => skipped.push(format!(".k2/compute.toml: {e}")),
        }
    }
    for f in ["mise.toml", ".mise.toml"] {
        let Some(s) = small_read(&root.join(f)) else { continue };
        match s.parse::<toml::Table>() {
            Ok(t) => {
                if let Some(tools) = t.get("tools").and_then(|v| v.as_table()) {
                    for (tool, v) in tools {
                        let ver = match v {
                            toml::Value::String(s) => Some(s.clone()),
                            toml::Value::Array(a) => a.iter().find_map(|x| x.as_str().map(str::to_string)),
                            toml::Value::Table(t) => t.get("version").and_then(|x| x.as_str()).map(str::to_string),
                            _ => None,
                        };
                        match ver {
                            Some(v) => found.push((format!("{tool}@{v}"), f.into())),
                            None => skipped.push(format!("{f}: {tool} has no version string")),
                        }
                    }
                }
            }
            Err(e) => skipped.push(format!("{f}: {e}")),
        }
    }
    if let Some(s) = small_read(&root.join(".tool-versions")) {
        for line in s.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut it = line.split_whitespace();
            if let (Some(tool), Some(ver)) = (it.next(), it.next()) {
                found.push((format!("{tool}@{ver}"), ".tool-versions".into()));
            }
        }
    }
    for f in [".nvmrc", ".node-version"] {
        if let Some(v) = small_read(&root.join(f)).and_then(|s| node_version_file(&s)) {
            found.push((format!("node@{v}"), f.into()));
        }
    }
    if let Some(v) = small_read(&root.join(".python-version")).and_then(|s| first_line(&s)) {
        found.push((format!("python@{v}"), ".python-version".into()));
    }
    let mut out: Vec<(Need, String)> = Vec::new();
    for (tok, src) in found {
        match needs::parse(&tok).and_then(|n| needs::check_supported(&n).map(|_| n)) {
            // rust-toolchain files are handled separately; a `rust` entry
            // in mise.toml/.tool-versions is honoured like any need. A
            // tool the node can't provide is skipped with a note.
            Ok(n) => {
                if !out.iter().any(|(o, _)| o.key() == n.key()) {
                    out.push((n, src));
                }
            }
            Err(e) => skipped.push(format!("{src}: {e}")),
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The commands a job plainly calls: the program itself, or each
    /// simple command of a `sh -c` script, past `VAR=x` and `env`/`time`.
    #[test]
    fn commands_called_reads_argv_and_shell_scripts() {
        assert_eq!(commands_called(&argv(&["python3", "x.py"])), vec!["python3"]);
        assert_eq!(commands_called(&argv(&["env", "A=1", "uv", "run", "t"])), vec!["uv"]);
        let script = "set -o pipefail; CARGO_BUILD_JOBS=4 cargo test 2>&1 | tail -5 && (cd web && bun install) ; for f in a b; do python3 \"$f\"; done";
        assert_eq!(commands_called(&argv(&["bash", "-c", script])), vec!["set", "cargo", "tail", "cd", "bun", "for", "python3"]);
        assert_eq!(commands_called(&argv(&["sh", "-ec", "time npx vitest run"])), vec!["npx"]);
        // An explicit path isn't a PATH lookup; `command -v x` only asks.
        assert!(commands_called(&argv(&["bash", "-c", "/opt/py/bin/python3 x; command -v uv"])).iter().all(|c| c != "python3" && c != "uv"));
        assert!(commands_called(&argv(&[])).is_empty());
    }

    /// A tool the command calls that the job's PATH lacks and `--needs`
    /// didn't ask for gets one line naming the need; a present or asked
    /// tool gets none, and one need is named once.
    #[test]
    fn missing_tool_hints_name_the_need() {
        let d = crate::util::temp_dir("hints");
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for t in ["cargo", "sh"] {
            let p = bin.join(t);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = vec![("PATH".to_string(), bin.display().to_string())];
        let cmd = argv(&["sh", "-c", "cargo build && python3 -m pip install x; pip3 list; uv sync; npx vitest; bunx tsc; node -v"]);
        assert_eq!(
            missing_tool_hints(&cmd, &env, &[]),
            vec![
                "python3 isn't on this node's PATH for jobs; add --needs python@3.12".to_string(),
                "uv isn't on this node's PATH for jobs; add --needs uv".to_string(),
                "npx isn't on this node's PATH for jobs; add --needs node@22".to_string(),
                "bunx isn't on this node's PATH for jobs; add --needs bun".to_string(),
            ]
        );
        // Asked for in --needs: no hint (the install puts it on the PATH).
        let needs = needs::parse_all(&["python@3.12".to_string(), "uv".to_string(), "node@22".to_string(), "bun".to_string()]).unwrap();
        assert!(missing_tool_hints(&cmd, &env, &needs).is_empty());
        // cargo is missing on this PATH: rust@stable.
        let no_cargo = vec![("PATH".to_string(), d.join("none").display().to_string())];
        assert_eq!(missing_tool_hints(&argv(&["cargo", "test"]), &no_cargo, &[]), vec!["cargo isn't on this node's PATH for jobs; add --needs rust@stable".to_string()]);
        assert!(missing_tool_hints(&argv(&["cargo", "test"]), &no_cargo, &needs::parse_all(&["rust@stable".to_string()]).unwrap()).is_empty());
    }

    /// V76: a resolve in progress (long enough to be an install) holds the
    /// node queue; per toolchains root; gone when the resolve ends.
    #[test]
    fn a_long_resolve_counts_as_installing() {
        let d = crate::util::temp_dir("installing");
        let (a, b) = (d.join("a/toolchains"), d.join("b/toolchains"));
        assert!(!installing_after(&a, Duration::ZERO));
        let g1 = Installing::start(&a);
        let g2 = Installing::start(&a);
        assert!(installing_after(&a, Duration::ZERO) && !installing_after(&b, Duration::ZERO));
        assert!(!installing(&a), "a quick resolve isn't an install yet");
        drop(g1);
        assert!(installing_after(&a, Duration::ZERO), "the other resolve still runs");
        drop(g2);
        assert!(!installing_after(&a, Duration::ZERO));
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn repo_files_become_needs() {
        let d = crate::util::temp_dir("tc-repo");
        write(&d, "mise.toml", "[tools]\nnode = \"22\"\npython = [\"3.12\", \"3.11\"]\ngo = { version = \"1.23\" }\n");
        write(&d, ".tool-versions", "nodejs 20.1.0 # ignored: mise.toml came first\njava 21\nterraform 1.9\n");
        write(&d, ".nvmrc", "lts/iron\n");
        write(&d, ".python-version", "3.10\n");
        write(&d, ".k2/compute.toml", "needs = [\"sys:pkg-config\", \"bun@1.2\"]\n");
        let (n, skipped) = declared_in_repo(&d);
        let mut toks: Vec<String> = n.iter().map(|(x, s)| format!("{}<{s}", x.token())).collect();
        toks.sort();
        assert_eq!(
            toks,
            vec![
                "bun@1.2<.k2/compute.toml",
                "go@1.23<mise.toml",
                "java@21<.tool-versions",
                "node@22<mise.toml",
                "python@3.12<mise.toml",
                "sys:pkg-config<.k2/compute.toml",
            ],
            "mise.toml beats .tool-versions/.nvmrc/.python-version for the same tool"
        );
        // A tool the node can't provide is skipped with a note, not tried.
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].starts_with(".tool-versions: terraform@1.9: k2-node doesn't install"), "{skipped:?}");
    }

    #[test]
    fn hostile_repo_files_are_skipped_not_run() {
        let d = crate::util::temp_dir("tc-hostile");
        write(&d, ".tool-versions", "node $(reboot)\npython 3.12;rm\n");
        write(&d, ".nvmrc", "--force\n");
        let (n, skipped) = declared_in_repo(&d);
        assert!(n.is_empty(), "{n:?}");
        assert_eq!(skipped.len(), 3, "{skipped:?}");
    }

    #[test]
    fn rust_toolchain_files() {
        let d = crate::util::temp_dir("tc-rust");
        assert_eq!(rust_toolchain_file(&d), None);
        write(&d, "rust-toolchain", "1.99.0\n");
        assert_eq!(rust_toolchain_file(&d).unwrap().channel, "1.99.0");
        write(&d, "rust-toolchain.toml", "[toolchain]\nchannel = \"nightly-2026-09-01\"\ncomponents = [\"clippy\", \"rustfmt\", \"bad;x\"]\ntargets = [\"wasm32-unknown-unknown\"]\n");
        let rt = rust_toolchain_file(&d).unwrap();
        assert_eq!(rt.channel, "nightly-2026-09-01");
        assert_eq!(rt.components, vec!["clippy", "rustfmt"]);
        assert_eq!(rt.targets, vec!["wasm32-unknown-unknown"]);
    }

    /// CE-A10: a toolchain's `components` list → the names a need uses
    /// (`-preview` and the host suffix gone, only the ones a need may name).
    #[test]
    fn toolchain_components_read_like_needs() {
        let list = "cargo-x86_64-unknown-linux-gnu\nclippy-preview-x86_64-unknown-linux-gnu\nrust-std-x86_64-unknown-linux-gnu\nrustc-x86_64-unknown-linux-gnu\nrustfmt-preview-x86_64-unknown-linux-gnu\nllvm-tools-preview-x86_64-unknown-linux-gnu\nrust-src\n";
        assert_eq!(components_from_list(list), vec!["clippy", "llvm-tools", "rust-src", "rustfmt"]);
        // Newer names, macOS host.
        let mac = "rustc-aarch64-apple-darwin\nclippy-aarch64-apple-darwin\nllvm-tools-aarch64-apple-darwin\nrust-std-aarch64-apple-darwin\n";
        assert_eq!(components_from_list(mac), vec!["clippy", "llvm-tools"]);
        assert!(components_from_list("").is_empty());
        let d = crate::util::temp_dir("tc-comps");
        write(&d, "lib/rustlib/components", mac);
        assert_eq!(toolchain_components(&d), vec!["clippy", "llvm-tools"]);
        assert_eq!(rustup_component_name("llvm-tools"), "llvm-tools-preview");
        assert_eq!(rustup_component_name("clippy"), "clippy");
    }

    #[test]
    fn rustc_version_from_the_channel_manifest() {
        let m = "manifest-version = \"2\"\n[pkg.cargo]\nversion = \"1.99.0 (aaa 2026-09-01)\"\n[pkg.rustc]\nversion = \"1.99.1 (bbb 2026-09-20)\"\n[pkg.rustc.target.x]\nversion = \"9\"\n";
        assert_eq!(rustc_version_from_manifest(m).as_deref(), Some("1.99.1"));
        let nightly = "[pkg.rustc]\nversion = \"1.101.0-nightly (ccc 2026-10-01)\"\n";
        assert_eq!(rustc_version_from_manifest(nightly).as_deref(), Some("1.101.0-nightly"));
        assert_eq!(rustc_version_from_manifest("[pkg.cargo]\nversion = \"1\"\n"), None);
        let d = crate::util::temp_dir("tc-ver");
        write(&d, "lib/rustlib/multirust-channel-manifest.toml", m);
        assert_eq!(toolchain_rustc_version(&d).as_deref(), Some("1.99.1"));
    }

    #[test]
    fn tool_versions_parse() {
        assert_eq!(tool_version("rustc 1.99.0 (59807616e 2026-09-14)").as_deref(), Some("1.99.0"));
        assert_eq!(tool_version("cargo-nextest 0.9.150 (abc 2026-10-01)").as_deref(), Some("0.9.150"));
        assert_eq!(tool_version("go version go1.23.1 linux/amd64").as_deref(), Some("1.23.1"));
        assert_eq!(tool_version("v22.23.4\n").as_deref(), Some("22.23.4"));
        assert_eq!(tool_version("Python 3.12.1").as_deref(), Some("3.12.1"));
        assert_eq!(tool_version("openjdk 21.0.1 2023-10-17").as_deref(), Some("21.0.1"));
        assert_eq!(tool_version("weird tool\n").as_deref(), Some("weird tool"));
        assert_eq!(tool_version("  \n"), None);
        let mut t = BTreeMap::new();
        t.insert("node".to_string(), "22.23.4".to_string());
        t.insert("rustc".to_string(), "1.99.0".to_string());
        assert_eq!(tools_line(&t), "tools: rustc 1.99.0, node 22.23.4");
        assert_eq!(tools_line(&BTreeMap::new()), "tools: none found on the job's PATH");
    }

    /// CE-4: the probe runs the job's PATH in its env (a fake `rustc`
    /// and `node` first on PATH win), never installs, skips what's absent.
    #[tokio::test]
    async fn tools_used_reads_the_jobs_own_path() {
        let d = crate::util::temp_dir("tc-used");
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, body) in [
            ("rustc", "#!/bin/sh\n[ \"$RUSTUP_AUTO_INSTALL\" = 0 ] || exit 3\necho \"rustc 1.99.0 (abc 2026-09-01)\"\n"),
            ("node", "#!/bin/sh\necho v22.23.4\n"),
            ("python3", "#!/bin/sh\nexit 1\n"),
        ] {
            crate::util::write_executable(&bin.join(name), body);
        }
        let env = vec![("PATH".to_string(), format!("{}:/usr/bin:/bin", bin.display()))];
        let explicit = needs::parse_all(&["node@22", "python@3.12"]).unwrap();
        let t = tools_used(&env, &d, &explicit).await;
        assert_eq!(t.get("rustc").map(String::as_str), Some("1.99.0"), "{t:?}");
        assert_eq!(t.get("node").map(String::as_str), Some("22.23.4"), "{t:?}");
        assert!(!t.contains_key("python"), "a failing probe is absent: {t:?}");
        assert!(!t.contains_key("cargo-nextest"), "{t:?}");
    }

    /// CE-18: over the cap, the least recently used versions no job needs
    /// go first, until under it; needed, default and fresh ones stay.
    #[test]
    fn the_tool_cache_cap_removes_the_least_recently_used() {
        let v = |key: &str, bytes: u64, last_used: i64| ToolVersion { key: key.into(), path: PathBuf::from(key), bytes, last_used };
        let gb = 1u64 << 30;
        let vs = vec![v("mise:node@20.1.0", gb, 100), v("mise:node@22.23.4", gb, 300), v("mise:node@18.0.0", gb, 200)];
        // Cap 1 GB, two versions in, a third pushed it over: the LRU goes.
        assert_eq!(lru_victims(&vs[..2], gb * 2, |_| false), Vec::<usize>::new());
        assert_eq!(lru_victims(&vs, gb * 2, |_| false), vec![0], "node 20 (oldest use) goes; 2 GB fits");
        assert_eq!(lru_victims(&vs, gb, |_| false), vec![0, 2]);
        let needs = needs::parse_all(&["node@20"]).unwrap();
        assert_eq!(lru_victims(&vs, gb * 2, |x| version_needed(&x.key, &needs)), vec![2], "a needed version stays");
        assert!(version_needed("mise:node@22.23.4", &needs::parse_all(&["node@22"]).unwrap()));
        assert!(!version_needed("mise:node@220.1", &needs::parse_all(&["node@22"]).unwrap()));
        assert!(version_needed("mise:node@18.0.0", &needs::parse_all(&["node@lts"]).unwrap()));
        assert!(version_needed("rust:stable-x86_64-unknown-linux-gnu", &needs::parse_all(&["rust@stable+clippy"]).unwrap()));
        assert!(!version_needed("rust:1.99.0-x86_64-unknown-linux-gnu", &needs::parse_all(&["rust@stable"]).unwrap()));
        assert!(version_needed("mise:aqua-cli-cli@2.1.0", &needs::parse_all(&["aqua:cli/cli@2"]).unwrap()));
    }

    /// The fixture of CE-18 acceptance 3 on disk: three installs over a
    /// 1-byte cap; the one used longest ago goes, the default toolchain
    /// and a needed one stay; the LRU clock comes from `mark_used`.
    #[test]
    fn gc_to_budget_on_disk() {
        let home = crate::util::temp_dir("tc-gc");
        let tc = Toolchains::new(&home, vec![]);
        let inst = tc.mise_data().join("installs");
        write(&inst, "node/20.1.0/bin/node", "x".repeat(100).as_str());
        write(&inst, "node/22.23.4/bin/node", "x".repeat(100).as_str());
        write(&inst, "python/3.12.1/bin/python3", "x".repeat(100).as_str());
        write(&tc.rustup_home(), "toolchains/stable-x/lib/x", "x".repeat(100).as_str());
        write(&tc.rustup_home(), "settings.toml", "default_toolchain = \"stable-x\"\n");
        tc.mark_used(&["mise:node@20.1.0".into()], 1_000);
        tc.mark_used(&["mise:python@3.12.1".into()], 2_000);
        tc.mark_used(&["mise:node@22.23.4".into()], 3_000);
        let all = tc.installed_versions();
        assert_eq!(all.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), vec!["mise:node@20.1.0", "mise:node@22.23.4", "mise:python@3.12.1", "rust:stable-x"]);
        assert!(all.iter().all(|v| v.bytes >= 100), "{all:?}");
        let gone = tc.gc_to_budget(250, &needs::parse_all(&["python@3.12"]).unwrap(), 10_000);
        assert_eq!(gone.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), vec!["mise:node@20.1.0", "mise:node@22.23.4"]);
        assert!(!inst.join("node/20.1.0").exists() && inst.join("python/3.12.1").exists());
        assert!(tc.rustup_home().join("toolchains/stable-x").exists(), "the default toolchain stays");
        // Under the cap: nothing more goes.
        assert!(tc.gc_to_budget(1 << 30, &[], 10_000).is_empty());
    }

    // ── the native tools (android-ndk, cargo-ndk) ────────────────────

    fn linux_x86(home: &Path) -> Toolchains {
        let mut tc = Toolchains::new(home, vec![("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), home.display().to_string())]);
        tc.os = "linux";
        tc.arch = "x86_64";
        tc
    }

    /// A small stand-in for the NDK zip: `<top>/source.properties`, an
    /// executable clang wrapper, and a symlink (the real one has both).
    fn fake_ndk_zip(path: &Path, top: &str, revision: &str) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.add_directory(format!("{top}/"), o).unwrap();
        z.start_file(format!("{top}/source.properties"), o.unix_permissions(0o644)).unwrap();
        z.write_all(format!("Pkg.Desc = Android NDK\nPkg.Revision = {revision}\n").as_bytes()).unwrap();
        let bin = format!("{top}/toolchains/llvm/prebuilt/linux-x86_64/bin");
        z.start_file(format!("{bin}/clang-18"), o.unix_permissions(0o755)).unwrap();
        z.write_all(b"#!/bin/sh\necho clang\n").unwrap();
        z.add_symlink(format!("{bin}/clang"), "clang-18", o).unwrap();
        z.finish().unwrap();
    }

    fn sha_of(p: &Path) -> String {
        k2_node_proto::crypto::sha256_hex(&std::fs::read(p).unwrap())
    }

    fn have_unzipper() -> bool {
        ["unzip", "bsdtar"].iter().any(|p| crate::sysinfo::which(p, &[("PATH".into(), "/usr/bin:/bin".into())]).is_some())
    }

    #[test]
    fn ndk_env_is_the_aarch64_android28_toolchain_and_the_ndk_home() {
        let (bin, env) = ndk_env(Path::new("/n/android-ndk/r27d"), needs::ANDROID_DEFAULT_API);
        assert_eq!(bin, PathBuf::from("/n/android-ndk/r27d/toolchains/llvm/prebuilt/linux-x86_64/bin"));
        let get = |k: &str| env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("{k}"));
        for k in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK", "NDK_HOME"] {
            assert_eq!(get(k), "/n/android-ndk/r27d");
        }
        let clang = format!("{}/aarch64-linux-android28-clang", bin.display());
        assert_eq!(get("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"), clang);
        assert_eq!(get("CC_aarch64_linux_android"), clang);
        assert_eq!(get("CXX_aarch64_linux_android"), format!("{clang}++"));
        assert_eq!(get("AR_aarch64_linux_android"), format!("{}/llvm-ar", bin.display()));
        assert_eq!(get("CARGO_NDK_PLATFORM"), "28", "Sew's minSdk");
    }

    /// The job's own `--env` comes after the NDK's, so it wins (another API level).
    #[test]
    fn the_jobs_env_wins_over_the_ndk_env() {
        let home = crate::util::temp_dir("tc-ndk-env");
        let (_, tool_env) = ndk_env(Path::new("/n/ndk"), 28);
        let extra: BTreeMap<String, String> = [("CARGO_NDK_PLATFORM".to_string(), "31".to_string())].into();
        let (job_home, tmp, target) = (home.join("h"), home.join("t"), home.join("target"));
        let env = crate::runner::job_env(&crate::runner::EnvInputs {
            job_home: &job_home,
            job_tmp: &tmp,
            node_home: &home,
            cache_dir: &home,
            target_dir: &target,
            tool_paths: &[],
            tool_env: &tool_env,
            extra: &extra,
            job_id: "j",
            user: "u",
            sccache_socket: None,
            job_cache: None,
            src_dir: None,
        })
        .unwrap();
        let last = env.iter().rev().find(|(k, _)| k == "CARGO_NDK_PLATFORM").unwrap();
        assert_eq!(last.1, "31", "the last value is what the job gets");
    }

    #[tokio::test]
    async fn the_ndk_is_checked_unpacked_and_swapped_in_whole() {
        if !have_unzipper() {
            eprintln!("skipped: no unzip or bsdtar here");
            return;
        }
        let home = crate::util::temp_dir("tc-ndk");
        let tc = linux_x86(&home);
        let src = home.join("ndk.zip");
        fake_ndk_zip(&src, "android-ndk-rtest", "27.9.1");
        let sha: &'static str = Box::leak(sha_of(&src).into_boxed_str());
        let pin = needs::NdkPin { major: "27", release: "rtest", revision: "27.9.1", url: "https://x/android-ndk-rtest-linux.zip", sha256: sha, sha1: "", bytes: 0 };
        let dest = tc.ndk_dir("rtest");
        // A stale half-unpacked folder from before is replaced, not merged.
        write(&dest, "junk", "x");
        let stage = tc.stage("android-ndk").unwrap();
        let zip = stage.join("android-ndk-rtest-linux.zip");
        std::fs::copy(&src, &zip).unwrap();
        assert_eq!(tc.place_ndk(&zip, &pin, &stage, &dest).await.unwrap(), dest);
        assert_eq!(ndk_revision(&dest).as_deref(), Some("27.9.1"));
        assert!(!dest.join("junk").exists(), "the old folder went whole");
        let bin = dest.join("toolchains/llvm/prebuilt/linux-x86_64/bin");
        assert!(is_executable(&bin.join("clang-18")), "modes kept");
        assert_eq!(std::fs::read_link(bin.join("clang")).unwrap(), PathBuf::from("clang-18"), "symlinks kept");
        assert!(!stage.exists(), "the stage is gone");
        // Facts, the LRU key, and what a need keeps.
        assert_eq!(tc.installed_tools().get("android-ndk").map(String::as_str), Some("27.9.1"));
        assert_eq!(tc.install_key(&bin).as_deref(), Some("android-ndk:rtest"));
        assert!(tc.installed_versions_shallow().iter().any(|(k, _)| k == "android-ndk:rtest"));
        assert!(version_needed("android-ndk:r27d", &needs::parse_all(&["android-ndk@27"]).unwrap()));
        assert!(!version_needed("android-ndk:r26c", &needs::parse_all(&["android-ndk@27"]).unwrap()));
    }

    #[tokio::test]
    async fn a_wrong_sha_or_revision_leaves_nothing_behind() {
        if !have_unzipper() {
            eprintln!("skipped: no unzip or bsdtar here");
            return;
        }
        let home = crate::util::temp_dir("tc-ndk-bad");
        let tc = linux_x86(&home);
        let src = home.join("ndk.zip");
        fake_ndk_zip(&src, "android-ndk-rtest", "27.9.1");
        let good: &'static str = Box::leak(sha_of(&src).into_boxed_str());
        let zero: &'static str = Box::leak("0".repeat(64).into_boxed_str());
        for (sha, rev, why) in [(zero, "27.9.1", "refusing"), (good, "27.9.2", "expected 27.9.2")] {
            let pin = needs::NdkPin { major: "27", release: "rtest", revision: rev, url: "https://x/z.zip", sha256: sha, sha1: "", bytes: 0 };
            let stage = tc.stage("android-ndk").unwrap();
            let zip = stage.join("z.zip");
            std::fs::copy(&src, &zip).unwrap();
            let e = tc.place_ndk(&zip, &pin, &stage, &tc.ndk_dir("rtest")).await.unwrap_err();
            assert!(e.contains(why), "{e}");
            assert!(!stage.exists() && !tc.ndk_dir("rtest").exists(), "nothing left: {e}");
        }
    }

    #[tokio::test]
    async fn cargo_ndk_is_checked_and_placed() {
        let home = crate::util::temp_dir("tc-cargo-ndk");
        let tc = linux_x86(&home);
        let top = "cargo-ndk-x86_64-unknown-linux-musl-v4.1.2";
        write(&home.join("src"), &format!("{top}/cargo-ndk"), "#!/bin/sh\n");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(home.join("src").join(top).join("cargo-ndk"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tgz = home.join("c.tgz");
        assert!(std::process::Command::new("/usr/bin/tar").args(["-czf", tgz.to_str().unwrap(), "-C", home.join("src").to_str().unwrap(), top]).status().unwrap().success());
        let stage = tc.stage("cargo-ndk").unwrap();
        let staged = stage.join("c.tgz");
        std::fs::copy(&tgz, &staged).unwrap();
        let dest = tc.cargo_ndk_dir();
        tc.place_cargo_ndk(&staged, &sha_of(&tgz), top, &stage, &dest).await.unwrap();
        assert!(is_executable(&dest.join("cargo-ndk")) && !stage.exists());
        assert_eq!(tc.installed_tools().get("cargo-ndk").map(String::as_str), Some("4.1.2"));
        // Installed: resolve puts it on PATH without a download.
        let r = tc.resolve(&needs::parse_all(&["cargo-ndk@4"]).unwrap(), &[], None, &mut |_| {}).await.unwrap();
        assert_eq!(r.path_prepend, vec![dest]);
        assert!(!r.installed_any);
    }

    /// An installed NDK resolves without a download: its bin first on PATH,
    /// its env and a note in the job's log.
    #[tokio::test]
    async fn resolve_gives_an_installed_ndk_its_env() {
        let home = crate::util::temp_dir("tc-ndk-resolve");
        let tc = linux_x86(&home);
        let pin = needs::ndk_pin("27").unwrap();
        write(&tc.ndk_dir(pin.release), "source.properties", &format!("Pkg.Revision = {}\n", pin.revision));
        let r = tc.resolve(&needs::parse_all(&["ndk@27"]).unwrap(), &[], None, &mut |_| {}).await.unwrap();
        assert_eq!(r.path_prepend, vec![tc.ndk_dir("r27d").join("toolchains/llvm/prebuilt/linux-x86_64/bin")]);
        assert!(r.env.iter().any(|(k, v)| k == "ANDROID_NDK_HOME" && Path::new(v) == tc.ndk_dir("r27d")), "{:?}", r.env);
        assert!(r.notes.iter().any(|n| n.contains("aarch64-linux-android28 linker")), "{:?}", r.notes);
        assert!(!r.installed_any);
    }

    /// Elsewhere (a Mac, arm64 Linux): an explicit need is tool_unsupported
    /// before anything is fetched; a repo-declared one is a note.
    #[tokio::test]
    async fn the_ndk_is_refused_off_linux_x86_64() {
        let home = crate::util::temp_dir("tc-ndk-mac");
        let mut tc = linux_x86(&home);
        for (os, arch) in [("macos", "aarch64"), ("linux", "aarch64")] {
            tc.os = os;
            tc.arch = arch;
            let (code, msg) = tc.resolve(&needs::parse_all(&["android-ndk@27"]).unwrap(), &[], None, &mut |_| {}).await.unwrap_err();
            assert_eq!(code, codes::TOOL_UNSUPPORTED, "{msg}");
            assert!(msg.contains("Linux x86_64 nodes only") && msg.contains(&format!("{os}-{arch}")), "{msg}");
            let repo = vec![(needs::parse("android-ndk@27").unwrap(), "mise.toml".to_string())];
            let r = tc.resolve(&[], &repo, None, &mut |_| {}).await.unwrap();
            assert!(r.notes.iter().any(|n| n.contains("Linux x86_64 nodes only")), "{:?}", r.notes);
        }
        assert!(!tc.root.join("android-ndk").exists(), "nothing fetched");
    }

    #[test]
    fn mise_pins_cover_both_oses() {
        for (os, arch) in [("linux", "x86_64"), ("linux", "aarch64"), ("macos", "aarch64"), ("macos", "x86_64")] {
            let (asset, sha) = MISE_ASSETS.iter().find(|(o, a, _, _)| *o == os && *a == arch).map(|(_, _, n, s)| (*n, *s)).unwrap();
            assert!(asset.contains(MISE_VERSION), "{asset}");
            assert_eq!(sha.len(), 64);
        }
    }

    #[test]
    fn sha_mismatch_is_refused() {
        let d = crate::util::temp_dir("tc-sha");
        let p = d.join("x");
        std::fs::write(&p, b"hello").unwrap();
        assert!(check_sha256(&p, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824").is_ok());
        let e = check_sha256(&p, &"0".repeat(64)).unwrap_err();
        assert!(e.contains("refusing"), "{e}");
    }

    /// `rust@<channel>+target:<triple>` with a fake `rustup`: a
    /// toolchain that's there gets only its missing targets (`rustup
    /// target add`, never a reinstall); a new one installs with
    /// `--target`; the offer lists each toolchain's targets and how the
    /// node gets Rust. No network.
    #[tokio::test]
    async fn rust_targets_install_with_a_fake_rustup() {
        use std::os::unix::fs::PermissionsExt;
        let home = crate::util::temp_dir("tc-targets");
        let tc = Toolchains::new(&home, vec![("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), home.display().to_string())]);
        let rustup = tc.cargo_home().join("bin/rustup");
        std::fs::create_dir_all(rustup.parent().unwrap()).unwrap();
        let script = r#"#!/bin/sh
R="$RUSTUP_HOME"; H=x86_64-unknown-linux-gnu
echo "$*" >> "$R/calls.log"
case "$1 $2" in
  "toolchain list") ls "$R/toolchains" 2>/dev/null ;;
  "target add") [ "$3" = --toolchain ] || exit 8; d="$R/toolchains/$4-$H"; shift 4
     for t in "$@"; do mkdir -p "$d/lib/rustlib/$t/lib"; echo "rust-std-$t" >> "$d/lib/rustlib/components"; done ;;
  "toolchain install") d="$R/toolchains/$3-$H"; mkdir -p "$d/lib/rustlib/$H/lib"; printf 'rustc-%s\nrust-std-%s\n' $H $H > "$d/lib/rustlib/components"
     shift 3; while [ $# -gt 0 ]; do [ "$1" = --target ] && { mkdir -p "$d/lib/rustlib/$2/lib"; echo "rust-std-$2" >> "$d/lib/rustlib/components"; }; shift; done ;;
  *) exit 9 ;;
esac
"#;
        std::fs::write(&rustup, script).unwrap();
        std::fs::set_permissions(&rustup, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(tc.rustup_home()).unwrap();
        assert_eq!(tc.rust_installer(), Some("rustup"));
        let calls = || std::fs::read_to_string(tc.rustup_home().join("calls.log")).unwrap_or_default();
        // `+minimal`: this test is about targets (the default clippy and
        // rustfmt have their own tests). A new toolchain: installed with its
        // target in one go.
        let need = needs::parse_all(&["rust@1.95.0+minimal+target:wasm32-unknown-unknown"]).unwrap();
        let r = tc.resolve(&need, &[], None, &mut |_| {}).await.unwrap();
        assert!(r.installed_any);
        assert!(calls().contains("toolchain install 1.95.0 --profile minimal --no-self-update --target wasm32-unknown-unknown"), "{}", calls());
        assert!(r.env.contains(&("RUSTUP_TOOLCHAIN".to_string(), "1.95.0".to_string())));
        // Installed: nothing more happens.
        let before = calls();
        let r = tc.resolve(&need, &[], None, &mut |_| {}).await.unwrap();
        assert!(!r.installed_any, "{}", calls());
        assert_eq!(calls().lines().filter(|l| !l.starts_with("toolchain list")).count(), before.lines().filter(|l| !l.starts_with("toolchain list")).count());
        // A second target on the same toolchain: `target add` for it only.
        let two = needs::parse_all(&["rust@1.95.0+minimal+target:wasm32-unknown-unknown+target:wasm32-wasip1"]).unwrap();
        let r = tc.resolve(&two, &[], None, &mut |_| {}).await.unwrap();
        assert!(r.installed_any);
        assert!(calls().lines().any(|l| l == "target add --toolchain 1.95.0 wasm32-wasip1"), "{}", calls());
        assert!(!calls().contains("toolchain install 1.95.0 --profile minimal --no-self-update --target wasm32-unknown-unknown --target"), "never a reinstall");
        let tools = tc.installed_tools();
        assert_eq!(tools.get("rust-targets:1.95.0-x86_64-unknown-linux-gnu").map(String::as_str), Some("wasm32-unknown-unknown,wasm32-wasip1,x86_64-unknown-linux-gnu"), "{tools:?}");
        assert_eq!(tools.get(needs::TOOLS_RUST_INSTALLER).map(String::as_str), Some("rustup"));
        assert_eq!(targets_from_list("rustc-x86_64-unknown-linux-gnu\nrust-std-x86_64-unknown-linux-gnu\nclippy-preview-x86_64-unknown-linux-gnu\nrust-std-wasm32-unknown-unknown\nrust-std-../x\n"), vec!["wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"]);
        let mut used = BTreeMap::new();
        used.insert("rust-targets".to_string(), "wasm32-unknown-unknown".to_string());
        used.insert("rustc".to_string(), "1.95.0".to_string());
        assert_eq!(tools_line(&used), "tools: rustc 1.95.0, rust-targets wasm32-unknown-unknown");
    }

    /// A node whose Rust is a distro package (a `rustc` on the job
    /// PATH outside the node's toolchains, no rustup) says so in its offer
    /// and refuses a target with `rust_target_unsupported`, naming why and
    /// what to do; it never tries to install rustup for it.
    #[tokio::test]
    async fn a_system_rust_node_refuses_targets_by_name() {
        use std::os::unix::fs::PermissionsExt;
        let home = crate::util::temp_dir("tc-sysrust");
        let sys = home.join("usr-bin");
        std::fs::create_dir_all(&sys).unwrap();
        let rustc = sys.join("rustc");
        std::fs::write(&rustc, "#!/bin/sh\necho 'rustc 1.95.0 (arch)'\n").unwrap();
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tc = Toolchains::new(&home, vec![("PATH".into(), format!("{}:/nonexistent", sys.display())), ("HOME".into(), home.display().to_string())]);
        assert_eq!(tc.rust_installer(), Some("system"));
        assert_eq!(tc.installed_tools().get(needs::TOOLS_RUST_INSTALLER).map(String::as_str), Some("system"));
        let need = needs::parse_all(&["rust@1.95.0+target:wasm32-unknown-unknown"]).unwrap();
        let e = tc.resolve(&need, &[], None, &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::RUST_TARGET_UNSUPPORTED);
        assert!(e.1.contains("rust@1.95.0+target:wasm32-unknown-unknown") && e.1.contains("system package with no rustup") && e.1.contains("without a node name"), "{}", e.1);
        assert!(!tc.cargo_home().join("bin/rustup").exists(), "no rustup install was tried");
        // A repo-declared one is only a warning.
        let ok = tc.resolve(&[], &[(need[0].clone(), ".k2/compute.toml".into())], None, &mut |_| {}).await.unwrap();
        assert!(ok.notes.iter().any(|n| n.contains("warning") && n.contains("no rustup")), "{:?}", ok.notes);
        // Nothing on the PATH: no Rust yet (the first need installs rustup).
        let bare = Toolchains::new(&home, vec![("PATH".into(), "/nonexistent".into())]);
        assert_eq!(bare.rust_installer(), None);
    }

    /// A fake `mise` (shell) stands in for the pinned binary: `resolve`
    /// installs what's missing, reuses what's there, and puts the bin
    /// dirs first. No network.
    #[tokio::test]
    async fn resolve_with_a_fake_mise() {
        let home = crate::util::temp_dir("tc-fake");
        let tc = Toolchains::new(&home, vec![("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), home.display().to_string())]);
        let bin = tc.mise_bin();
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        let script = format!(
            r#"#!/bin/sh
D="$MISE_DATA_DIR/installs"
case "$1" in
  --version) echo "{MISE_VERSION} fake" ;;
  install) t="${{2%@*}}"; v="${{2#*@}}"; mkdir -p "$D/$t/$v/bin"; echo "$2" >> "$MISE_DATA_DIR/install.log"
     [ "$MISE_DISABLE_BACKENDS" = "asdf,vfox" ] || {{ echo "plugin backends not disabled" >&2; exit 7; }}
     b="$t"; [ "$t" = python ] && b=python3
     [ -f "$MISE_DATA_DIR/empty-installs" ] || {{ printf '#!/bin/sh\n' > "$D/$t/$v/bin/$b"; chmod 755 "$D/$t/$v/bin/$b"; }} ;;
  bin-paths) t="${{2%@*}}"; v="${{2#*@}}"; [ -d "$D/$t/$v/bin" ] && echo "$D/$t/$v/bin" ;;
  *) exit 9 ;;
esac
"#
        );
        crate::util::write_executable(&bin, &script);

        let explicit = needs::parse_all(&["node@22"]).unwrap();
        let repo = vec![(needs::parse("python@3.12").unwrap(), "mise.toml".to_string()), (needs::parse("node@20").unwrap(), ".nvmrc".to_string())];
        let mut logs = Vec::new();
        let r = tc.resolve(&explicit, &repo, None, &mut |l| logs.push(l.to_string())).await.unwrap();
        assert!(r.installed_any);
        let data = tc.mise_data().join("installs");
        assert_eq!(r.path_prepend[0], data.join("node/22/bin"), "explicit node@22 beats .nvmrc's 20");
        assert_eq!(r.path_prepend[1], data.join("python/3.12/bin"));
        assert!(!data.join("node/20").exists(), ".nvmrc lost to the explicit need");
        assert!(r.env.iter().any(|(k, _)| k == "MISE_DATA_DIR"));
        let log1 = std::fs::read_to_string(tc.mise_data().join("install.log")).unwrap();
        assert_eq!(log1, "node@22\npython@3.12\n");
        // Second run: cached, nothing installed.
        let r2 = tc.resolve(&explicit, &repo, None, &mut |_| {}).await.unwrap();
        assert!(!r2.installed_any);
        assert_eq!(std::fs::read_to_string(tc.mise_data().join("install.log")).unwrap(), log1);
        let tools = tc.installed_tools();
        assert_eq!(tools.get("mise:node").map(String::as_str), Some("22"));
        assert_eq!(tools.get("mise:python").map(String::as_str), Some("3.12"));
        // cmake (Rosson 2026-10-13: K2's own build shells out to it).
        let r3 = tc.resolve(&needs::parse_all(&["cmake@3"]).unwrap(), &[], None, &mut |_| {}).await.unwrap();
        assert_eq!(r3.path_prepend[0], data.join("cmake/3/bin"));
        assert!(is_executable(&data.join("cmake/3/bin/cmake")));
        // "Installed" without the command it must provide: the job fails
        // now (tool_install_failed), never a later `cmake: command not found`.
        std::fs::write(tc.mise_data().join("empty-installs"), b"").unwrap();
        let e = tc.resolve(&needs::parse_all(&["ninja@1"]).unwrap(), &[], None, &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::TOOL_INSTALL_FAILED);
        assert!(e.1.contains("ninja@1") && e.1.contains("no `ninja`"), "{}", e.1);
        std::fs::remove_file(tc.mise_data().join("empty-installs")).unwrap();
        // A failing explicit install fails the job with a stable code; a
        // failing repo-declared one is only a note.
        crate::util::write_executable(&bin, &format!("#!/bin/sh\ncase \"$1\" in --version) echo {MISE_VERSION};; install) echo 'no such tool' >&2; exit 1;; *) exit 0;; esac\n"));
        let e = tc.resolve(&needs::parse_all(&["zig@0.13"]).unwrap(), &[], None, &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::TOOL_INSTALL_FAILED);
        assert!(e.1.contains("zig@0.13") && e.1.contains("no such tool"), "{}", e.1);
        let ok = tc.resolve(&[], &[(needs::parse("zig@0.13").unwrap(), ".tool-versions".into())], None, &mut |_| {}).await.unwrap();
        assert!(ok.notes.iter().any(|n| n.contains("warning") && n.contains(".tool-versions")), "{:?}", ok.notes);
    }

    /// Rosson 2026-11-04: new jobs after a k2-node update get the newest
    /// `rust@stable`, while running jobs keep theirs. A fake rustup records
    /// each install: the first job installs; a job right after doesn't; a
    /// new k2-node build (another version in the mark) brings stable to its
    /// newest for the next job, but never while a running job uses stable.
    #[tokio::test]
    async fn stable_moves_forward_for_new_jobs_never_under_a_running_one() {
        let home = crate::util::temp_dir("tc-rust-refresh");
        let mut tc = Toolchains::new(&home, vec![("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), home.display().to_string())]);
        let rustup = tc.cargo_home().join("bin/rustup");
        std::fs::create_dir_all(rustup.parent().unwrap()).unwrap();
        let log = home.join("rustup.log");
        let script = format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  'toolchain list') [ -f '{h}/installed' ] && echo 'stable-x86_64-unknown-linux-gnu (default)' ;;\n  'toolchain install') echo \"$*\" >> '{l}'; touch '{h}/installed' ;;\n  *) exit 9 ;;\nesac\n",
            h = home.display(),
            l = log.display()
        );
        crate::util::write_executable(&rustup, &script);
        let stable = RustToolchain { channel: "stable".into(), components: vec![], targets: vec![] };
        let installs = || std::fs::read_to_string(&log).unwrap_or_default().lines().count();
        let mut notes = Vec::new();
        assert!(tc.ensure_rust(&stable, &mut |l| notes.push(l.to_string())).await.unwrap(), "the first job installs stable");
        assert_eq!(installs(), 1);
        assert!(!tc.ensure_rust(&stable, &mut |_| {}).await.unwrap(), "the next job uses it as it is");
        assert_eq!(installs(), 1);
        // k2-node was updated since stable last moved (another version in the mark).
        std::fs::write(tc.refresh_mark("stable"), format!("0.0.1 {}\n", unix_now())).unwrap();
        tc.busy_channels = vec!["stable".into()];
        let mut notes = Vec::new();
        assert!(!tc.ensure_rust(&stable, &mut |l| notes.push(l.to_string())).await.unwrap());
        assert_eq!(installs(), 1, "never moved under a running job");
        assert!(notes.iter().any(|n| n.contains("stays at its installed release while a running job uses it")), "{notes:?}");
        tc.busy_channels.clear();
        assert!(tc.ensure_rust(&stable, &mut |_| {}).await.unwrap(), "the next job after it ends gets the newest");
        assert_eq!(installs(), 2);
        assert!(!tc.rust_refresh_due("stable", unix_now()), "fresh for this build");
        assert!(tc.rust_refresh_due("stable", unix_now() + RUST_REFRESH_SECS), "and again a day later");
        assert!(!tc.rust_refresh_due("1.82.0", unix_now()), "a pinned release never moves");
    }
}
