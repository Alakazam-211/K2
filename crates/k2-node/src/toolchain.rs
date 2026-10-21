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
}

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
        }
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
    /// Returns true when something was installed.
    pub async fn ensure_rust(&self, rt: &RustToolchain, log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, String> {
        self.ensure_rustup(log).await?;
        let rustup = self.rustup().display().to_string();
        let list = self.run(&rustup, &["toolchain", "list"], Duration::from_secs(60)).await?;
        let have = list.stdout.lines().any(|l| {
            let name = l.split_whitespace().next().unwrap_or("");
            name == rt.channel || name.starts_with(&format!("{}-", rt.channel))
        });
        if have && rt.components.is_empty() && rt.targets.is_empty() {
            return Ok(false);
        }
        log(&format!("installing Rust {} (rustup)", rt.channel));
        let mut args: Vec<String> =
            vec!["toolchain".into(), "install".into(), rt.channel.clone(), "--profile".into(), "minimal".into(), "--no-self-update".into()];
        for c in &rt.components {
            args.push("--component".into());
            args.push(c.clone());
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
        Ok(!have)
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

    // ── resolve ──────────────────────────────────────────────────────

    /// Everything the job needs, installed. `explicit` failures fail the
    /// job; repo-declared ones become notes.
    pub async fn resolve(&self, explicit: &[Need], repo: &[(Need, String)], rust_file: Option<&RustToolchain>, log: &mut (dyn FnMut(&str) + Send)) -> Result<Resolved, Fail> {
        let _installing = Installing::start(&self.root);
        let mut out = Resolved::default();
        let explicit_keys: Vec<String> = explicit.iter().map(Need::key).collect();
        let mut work: Vec<(Need, Option<String>)> = explicit.iter().cloned().map(|n| (n, None)).collect();
        for (n, src) in repo {
            if !explicit_keys.contains(&n.key()) && !work.iter().any(|(w, _)| w.key() == n.key()) {
                work.push((n.clone(), Some(src.clone())));
            }
        }
        // System packages first, all at once (one root call).
        let sys: Vec<String> = work
            .iter()
            .filter_map(|(n, src)| match n {
                Need::System { package } if src.is_none() => Some(package.clone()),
                _ => None,
            })
            .collect();
        if !sys.is_empty() {
            let did = crate::packages::ensure(self, &sys, log).await?;
            out.installed_any |= did;
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
                Need::Rust { channel } => {
                    let rt = RustToolchain { channel: channel.clone(), components: vec![], targets: vec![] };
                    let r = self.ensure_rust(&rt, log).await;
                    if r.is_ok() && required {
                        rust_env = Some(channel.clone());
                    }
                    r
                }
                Need::Nextest => self.ensure_nextest(log).await,
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
        if let Ok(rd) = std::fs::read_dir(self.rustup_home().join("toolchains")) {
            let mut tcs: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            tcs.sort();
            if !tcs.is_empty() {
                out.insert("rust-toolchains".into(), tcs.join(","));
            }
        }
        out
    }
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

    /// A fake `mise` (shell) stands in for the pinned binary: `resolve`
    /// installs what's missing, reuses what's there, and puts the bin
    /// dirs first. No network.
    #[tokio::test]
    async fn resolve_with_a_fake_mise() {
        use std::os::unix::fs::PermissionsExt;
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
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

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
        std::fs::write(&bin, format!("#!/bin/sh\ncase \"$1\" in --version) echo {MISE_VERSION};; install) echo 'no such tool' >&2; exit 1;; *) exit 0;; esac\n")).unwrap();
        let e = tc.resolve(&needs::parse_all(&["zig@0.13"]).unwrap(), &[], None, &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::TOOL_INSTALL_FAILED);
        assert!(e.1.contains("zig@0.13") && e.1.contains("no such tool"), "{}", e.1);
        let ok = tc.resolve(&[], &[(needs::parse("zig@0.13").unwrap(), ".tool-versions".into())], None, &mut |_| {}).await.unwrap();
        assert!(ok.notes.iter().any(|n| n.contains("warning") && n.contains(".tool-versions")), "{:?}", ok.notes);
    }
}
