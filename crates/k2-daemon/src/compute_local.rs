//! "Make this computer a compute node" from the machine's own K2
//! (compute polish v1): Settings → Compute and `k2 compute local enable`.
//!
//! 1. **The binary.** `k2-node` ships next to the daemon (Linux release
//!    assets, the Arch package, K2.app on macOS). A standalone Linux box
//!    without it downloads `k2-node-linux-<arch>` for this exact version
//!    and checks its minisign signature with the **root-helper key**
//!    (`k2_root_proto::node_build`, prd-k2-node-admin-v1 §8.1: k2-node
//!    has a root mode, so never the app-updater key) before anything runs
//!    it.
//! 2. **Root, once.** In order: the root helper's `node install` verb
//!    (feature-checked: only when the installed helper reports the `node`
//!    namespace), passwordless sudo for exactly `k2-node setup` (boxes
//!    provisioned that way), the macOS admin prompt (Settings only), or
//!    one owner command to copy (`sudo …/k2-node setup …`).
//! 3. **Enroll as the node.** With an enroll code the setup run enrolls
//!    directly; otherwise (a one-click ticket, or a node that's already
//!    installed) this daemon writes `enroll-request.toml` and the running
//!    node enrolls itself within seconds.
//!
//! The private-home choice is never silent: when the human's home is
//! readable by others the request must say `privateHome` true or false.
//!
//! Remote admin (prd-compute-update-continuity-v1 Q4, Rosson 2026-10-24):
//! the same setup run sets the node's remote-admin level, `policy` unless
//! the request says otherwise ("Let K2 keep this node updated and healthy"
//! in Settings), so there is no second password prompt. A reinstall over
//! an earlier choice sets none unless the request names one. `full` is an
//! explicit opt-in. Changing it later (`local/remote-admin`) goes through
//! the same root path: `k2-node admin level <off|policy|full>`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use k2_core::compute::{self as compute, local};
use k2_core::log_debug;

/// The root helper's binary and the first protocol with a `node`
/// namespace (designed in `.k2/prds/compute-polish-v1-notes.md`; the
/// helper on `hold/root-helper` is protocol 4 and has none yet).
pub const ROOT_HELPER: &str = "/usr/local/libexec/k2-root-helper";
pub const ROOT_HELPER_NODE_MIN_PROTOCOL: u64 = 5;

/// Where `k2-node setup` installs the binary.
pub fn installed_binary_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/K2/node/k2-node")
    } else {
        PathBuf::from("/usr/local/libexec/k2-node")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: &'static str,
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The k2-node this machine can install from, best source first.
pub fn find_binary() -> Option<Found> {
    let mut cands: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(p) = std::env::var_os("K2_NODE_BINARY").filter(|v| !v.is_empty()) {
        cands.push((PathBuf::from(p), "env"));
    }
    // Next to the daemon; on macOS the daemon sits in the nested
    // `K2 Daemon.app` and its sidecars stay in K2.app/Contents/MacOS.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| k2_core::daemon_lifecycle::bundle_sidecar_dir(&e)) {
        cands.push((dir.join("k2-node"), "bundled"));
    }
    if cfg!(target_os = "macos") {
        cands.push((PathBuf::from("/Applications/K2.app/Contents/MacOS/k2-node"), "bundled"));
    }
    cands.push((PathBuf::from("/usr/lib/k2/k2-node"), "package"));
    cands.push((PathBuf::from("/usr/bin/k2-node"), "package"));
    cands.push((k2_core::paths::k2_home().join("bin/k2-node"), "downloaded"));
    cands.push((installed_binary_path(), "installed"));
    cands.into_iter().find(|(p, _)| is_exec(p)).map(|(path, source)| Found { path, source })
}

/// Run a program with a time limit; `(success, stdout+stderr)`.
fn run_limited(mut cmd: Command, stdin: Option<Vec<u8>>, limit: Duration) -> Result<(bool, String), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).current_dir("/");
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        std::thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    let mut text = t_out.join().unwrap_or_default();
    text.push_str(&t_err.join().unwrap_or_default());
    Ok((status.success(), text))
}

pub fn binary_version(p: &Path) -> Option<String> {
    let mut c = Command::new(p);
    c.arg("version").env_clear().env("PATH", "/usr/bin:/bin");
    match run_limited(c, None, Duration::from_secs(10)) {
        Ok((true, out)) => out.lines().next().map(|l| l.trim().to_string()),
        _ => None,
    }
}

/// [`binary_version`], re-run only when the file changes (status is polled).
fn cached_version(p: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<HashMap<PathBuf, (Option<std::time::SystemTime>, Option<String>)>>;
    static C: OnceLock<Cache> = OnceLock::new();
    let mtime = std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((m, v)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(p) {
        if *m == mtime {
            return v.clone();
        }
    }
    let v = binary_version(p);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(p.to_path_buf(), (mtime, v.clone()));
    v
}

/// The check on a downloaded `k2-node-linux-<arch>` (§8.1): a minisign
/// signature by one of `keys` (the root-helper keys; never the updater
/// key) whose trusted comment names exactly this asset, a 64-bit ELF for
/// `arch`, one build marker equal to the comment, and that version is
/// `want_version` (this daemon's own). Fail closed: any doubt is an error.
pub fn check_node_download(bin: &[u8], sig: &str, keys: &[k2_root_proto::keys::TrustedKey], arch: &str, want_version: &str) -> Result<(), String> {
    let asset = k2_root_proto::node_build::asset_name("linux", arch);
    let v = k2_root_proto::node_build::verify_node_build(bin, sig, keys, "linux", arch)
        .map_err(|m| format!("{asset} signature check failed (root-helper key), refusing it: {m}"))?;
    if v.version.to_string() != want_version {
        return Err(format!("{asset} is signed as v{}, this daemon is v{want_version}; refusing it", v.version));
    }
    Ok(())
}

/// Linux: `k2-node-linux-<arch>` for this daemon's version, signature
/// checked with the root-helper key ([`check_node_download`]), into
/// `~/.k2/bin/k2-node`.
#[cfg(not(feature = "airgap"))]
pub fn download_binary() -> Result<PathBuf, String> {
    if !cfg!(target_os = "linux") {
        return Err("k2-node ships inside K2.app on macOS; update K2".into());
    }
    if k2_core::airgap::enabled() {
        return Err("air-gapped: copy k2-node next to the daemon by hand (the release asset k2-node-linux-<arch>)".into());
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        a => return Err(format!("no k2-node release for {a}")),
    };
    let base = std::env::var("K2_NODE_RELEASE_BASE").unwrap_or_else(|_| "https://github.com/Alakazam-211/K2/releases/download".into());
    let url = format!("{}/v{}/k2-node-linux-{arch}", base.trim_end_matches('/'), env!("CARGO_PKG_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let get = |u: &str| -> Result<Vec<u8>, String> {
        let r = client.get(u).send().map_err(|e| format!("{u}: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("{u}: http {}", r.status()));
        }
        r.bytes().map(|b| b.to_vec()).map_err(|e| format!("{u}: {e}"))
    };
    let bin = get(&url)?;
    let sig = String::from_utf8(get(&format!("{url}.sig"))?).map_err(|_| "signature isn't text".to_string())?;
    check_node_download(&bin, &sig, &k2_root_proto::keys::trusted_keys(false), arch, env!("CARGO_PKG_VERSION"))?;
    let dir = k2_core::paths::k2_home().join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".k2-node.{}", std::process::id()));
    std::fs::write(&tmp, &bin).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let dest = dir.join("k2-node");
    std::fs::rename(&tmp, &dest).map_err(|e| format!("place {}: {e}", dest.display()))?;
    binary_version(&dest).ok_or("the downloaded k2-node doesn't run on this machine")?;
    Ok(dest)
}

#[cfg(feature = "airgap")]
pub fn download_binary() -> Result<PathBuf, String> {
    Err("air-gapped build: copy k2-node next to the daemon by hand".into())
}

/// How root gets done on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// The root helper's `node` namespace (no password, no prompt).
    Helper,
    /// sudo without a password for exactly this binary.
    Sudo,
    /// The macOS admin prompt (Settings only).
    Prompt,
    /// One command for the owner to run.
    Command,
}

impl Elevation {
    pub fn as_str(self) -> &'static str {
        match self {
            Elevation::Helper => "helper",
            Elevation::Sudo => "sudo",
            Elevation::Prompt => "prompt",
            Elevation::Command => "command",
        }
    }
}

/// `helper version` (no root needed): does it carry the `node` namespace?
pub fn root_helper_node() -> serde_json::Value {
    let present = Path::new(ROOT_HELPER).is_file();
    if !present {
        return serde_json::json!({"present": false, "node": false});
    }
    let mut c = Command::new(ROOT_HELPER);
    c.args(["helper", "version"]).env_clear().env("PATH", "/usr/bin:/bin");
    let v: serde_json::Value = match run_limited(c, None, Duration::from_secs(10)) {
        Ok((true, out)) => serde_json::from_str(out.trim()).unwrap_or_default(),
        _ => serde_json::Value::Null,
    };
    let protocol = v.get("protocol").and_then(|p| p.as_u64()).unwrap_or(0);
    let node = protocol >= ROOT_HELPER_NODE_MIN_PROTOCOL && v.pointer("/namespaces/node").and_then(|n| n.as_bool()) == Some(true);
    serde_json::json!({"present": true, "protocol": protocol, "node": node})
}

/// sudo would run exactly `<bin> <verb> <args>` without a password
/// (`sudo -l <command>` checks the whole command line against sudoers).
fn sudo_allows(bin: &Path, verb: &str, args: &[String]) -> bool {
    if !Path::new("/usr/bin/sudo").is_file() {
        return false;
    }
    let mut c = Command::new("/usr/bin/sudo");
    c.arg("-n").arg("-l").arg(bin).arg(verb).args(args);
    matches!(run_limited(c, None, Duration::from_secs(10)), Ok((true, _)))
}

pub fn elevation(bin: &Path, verb: &str, args: &[String], allow_prompt: bool) -> Elevation {
    elevation_with(bin, verb, args, allow_prompt, true)
}

/// `helper_ok`: the root helper's `node` namespace knows only install and
/// uninstall; anything else goes through sudo, the prompt or a command.
pub fn elevation_with(bin: &Path, verb: &str, args: &[String], allow_prompt: bool, helper_ok: bool) -> Elevation {
    if helper_ok && cfg!(target_os = "linux") && root_helper_node()["node"] == true {
        return Elevation::Helper;
    }
    if sudo_allows(bin, verb, args) {
        return Elevation::Sudo;
    }
    if cfg!(target_os = "macos") && allow_prompt {
        return Elevation::Prompt;
    }
    Elevation::Command
}

/// POSIX shell single-quoting.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn command_line(bin: &Path, verb: &str, args: &[String]) -> String {
    let mut parts = vec!["sudo".to_string(), shell_quote(&bin.display().to_string()), verb.to_string()];
    parts.extend(args.iter().map(|a| shell_quote(a)));
    parts.join(" ")
}

/// AppleScript `do shell script "…" with administrator privileges`.
pub fn applescript_for(shell_cmd: &str) -> String {
    let esc = shell_cmd.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{esc}\" with administrator privileges")
}

/// Remote admin levels (`k2-node admin level`).
pub const REMOTE_ADMIN_LEVELS: &[&str] = &["off", "policy", "full"];
/// What "Make this computer a compute node" sets unless asked otherwise:
/// K2 keeps the node updated and healthy (Rosson 2026-10-24).
pub const DEFAULT_REMOTE_ADMIN: &str = "policy";

/// A request's `remoteAdmin`: absent or null = `default` (when there is
/// one), else one of [`REMOTE_ADMIN_LEVELS`].
pub fn remote_admin_arg(v: Option<&serde_json::Value>, default: Option<&str>) -> Result<String, Refused> {
    let bad = || ("bad_request", 400, "remoteAdmin is off, policy or full".to_string());
    match v {
        None | Some(serde_json::Value::Null) => default.map(str::to_string).ok_or_else(bad),
        Some(serde_json::Value::String(s)) if REMOTE_ADMIN_LEVELS.contains(&s.as_str()) => Ok(s.clone()),
        Some(_) => Err(bad()),
    }
}

/// The installer argv (`k2-node setup …`).
pub fn setup_args(human: &str, private_home: Option<bool>, enroll: Option<(&str, &str, &str)>, remote_admin: Option<&str>) -> Vec<String> {
    let mut a = vec!["--human".to_string(), human.to_string()];
    match private_home {
        Some(true) => a.push("--private-home".into()),
        Some(false) => a.push("--no-private-home".into()),
        None => {}
    }
    if let Some(l) = remote_admin {
        a.extend(["--remote-admin".to_string(), l.to_string()]);
    }
    if let Some((controller, code, name)) = enroll {
        a.extend(["--controller".into(), controller.into(), "--enroll".into(), code.into(), "--name".into(), name.into()]);
    }
    a
}

/// Run `<bin> <verb> args` as root through `el`.
fn run_root(el: Elevation, bin: &Path, verb: &str, args: &[String]) -> Result<String, String> {
    let limit = Duration::from_secs(300);
    let (ok, out) = match el {
        Elevation::Helper => {
            // Designed verb (see the notes): the helper takes the binary on
            // stdin, checks its release signature itself and runs the
            // embedded installer; only fixed tokens on argv.
            let hverb = match verb {
                "setup" => "install",
                "uninstall" => "uninstall",
                v => return Err(format!("the root helper has no node verb for {v}")),
            };
            let bytes = std::fs::read(bin).map_err(|e| format!("read {}: {e}", bin.display()))?;
            let mut c = Command::new("/usr/bin/sudo");
            c.arg("-n").arg(ROOT_HELPER).arg("node").arg(hverb).args(args);
            run_limited(c, Some(bytes), limit)?
        }
        Elevation::Sudo => {
            let mut c = Command::new("/usr/bin/sudo");
            c.arg("-n").arg(bin).arg(verb).args(args);
            run_limited(c, None, limit)?
        }
        Elevation::Prompt => {
            let mut parts = vec![shell_quote(&bin.display().to_string()), verb.to_string()];
            parts.extend(args.iter().map(|a| shell_quote(a)));
            let mut c = Command::new("/usr/bin/osascript");
            c.arg("-e").arg(applescript_for(&parts.join(" ")));
            run_limited(c, None, limit)?
        }
        Elevation::Command => return Err("needs the owner to run the command".into()),
    };
    if ok {
        Ok(out)
    } else {
        let tail: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(6)..].join(" | ");
        if tail.contains("User canceled") || tail.contains("-128") {
            return Err("the admin prompt was cancelled".into());
        }
        Err(tail)
    }
}

/// The daemon user's name and home.
pub fn current_user() -> (String, PathBuf) {
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() {
            return (std::env::var("USER").unwrap_or_default(), k2_core::paths::home_dir().unwrap_or_default());
        }
        let name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        (name, PathBuf::from(dir))
    }
}

fn approved_packages() -> serde_json::Value {
    let manager = [("apt", "/usr/bin/apt-get"), ("dnf", "/usr/bin/dnf"), ("pacman", "/usr/bin/pacman"), ("zypper", "/usr/bin/zypper")]
        .iter()
        .find(|(_, p)| Path::new(p).is_file())
        .map(|(n, _)| *n);
    let approved = local::approved_packages();
    serde_json::json!({
        "manager": if cfg!(target_os = "macos") { Some("brew") } else { manager },
        "approved": approved,
        // Jobs can install approved packages (the root path unit is in).
        "installer": Path::new("/etc/systemd/system/k2-node-packages.path").is_file(),
    })
}

/// `local/status` plus what the enable sheet needs.
pub fn status() -> serde_json::Value {
    let mut v = local::status();
    let found = find_binary();
    let (_, home) = current_user();
    v["binary"] = match &found {
        Some(f) => serde_json::json!({"path": f.path.display().to_string(), "source": f.source, "version": cached_version(&f.path)}),
        None => serde_json::Value::Null,
    };
    v["installedBinary"] = installed_binary_path().is_file().into();
    let rh = root_helper_node();
    // Never probe sudo here (this is polled; a failed `sudo -n` can be
    // logged or mailed to root). `enable` tries passwordless sudo itself.
    v["elevation"] = if rh["node"] == true {
        "helper"
    } else if cfg!(target_os = "macos") {
        "prompt"
    } else {
        "command"
    }
    .into();
    v["rootHelper"] = rh;
    v["homeCheck"] = local::home_check(&home).unwrap_or(serde_json::Value::Null);
    v["packages"] = approved_packages();
    // Remote admin off (or unset): K2 can't update this node. One command
    // turns it on (the toggle in Settings runs the same through root).
    if v["installed"] == true && !matches!(v["remoteAdmin"].as_str(), Some("policy") | Some("full")) {
        v["remoteAdminCommand"] = command_line(&installed_binary_path(), "admin", &["level".into(), DEFAULT_REMOTE_ADMIN.into()]).into();
    }
    v["ticketSupported"] = compute::ticket::supported().into();
    v
}

/// A refusal for the route (code, http status, hint).
pub type Refused = (&'static str, u16, String);

/// What `local/enable` was asked for.
#[derive(Debug, Clone)]
pub struct EnableReq {
    pub controller: Option<String>,
    pub enroll: Option<String>,
    pub ticket: Option<String>,
    pub name: Option<String>,
    pub private_home: Option<bool>,
    /// Settings may pop the macOS admin prompt; the CLI never does.
    pub allow_prompt: bool,
    /// `off` | `policy` | `full`, set by the same setup run. `None`: the
    /// request didn't name one, so [`setup_remote_admin`] decides.
    pub remote_admin: Option<String>,
}

/// The level a setup run sets: the one the request named; else `policy`
/// on a machine with no remote-admin choice yet; else nothing, so the
/// owner's earlier choice (an explicit `off` included) stands on a
/// reinstall (security reviews A-L4 / B-L6).
pub fn setup_remote_admin(named: Option<&str>, admin_toml_exists: bool) -> Option<String> {
    match named {
        Some(l) => Some(l.to_string()),
        None if admin_toml_exists => None,
        None => Some(DEFAULT_REMOTE_ADMIN.to_string()),
    }
}

/// Is there a remote-admin choice on this machine already?
fn admin_toml_exists() -> bool {
    local::config_dir().join("admin.toml").is_file()
}

impl Default for EnableReq {
    fn default() -> Self {
        Self {
            controller: None,
            enroll: None,
            ticket: None,
            name: None,
            private_home: None,
            allow_prompt: false,
            remote_admin: None,
        }
    }
}

fn node_installed() -> bool {
    // A dev/test node (`K2_NODE_CONFIG_DIR`) has no system install.
    let dev = std::env::var_os("K2_NODE_CONFIG_DIR").is_some_and(|v| !v.is_empty());
    local::config_dir().is_dir() && (dev || installed_binary_path().is_file())
}

fn wait_for<T>(limit: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(t) = f() {
            return Some(t);
        }
        if start.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn node_sas() -> Option<String> {
    local::status()["node"]["sas"].as_str().map(str::to_string)
}

/// `POST /cli/compute/local/enable`.
pub fn enable(req: &EnableReq, by: &str) -> Result<serde_json::Value, Refused> {
    let bad = |m: &str| -> Refused { ("bad_request", 400, m.to_string()) };
    if req.remote_admin.as_deref().is_some_and(|l| !REMOTE_ADMIN_LEVELS.contains(&l)) {
        return Err(bad("remoteAdmin is off, policy or full"));
    }
    if req.enroll.is_some() && req.ticket.is_some() {
        return Err(bad("pass an enroll code or a ticket, not both"));
    }
    if let Some(e) = &req.enroll {
        if compute::proto::pairing::EnrollString::parse(e).is_none() {
            return Err(bad("the enroll string looks like ABCDE-FGHJK.0123456789abcdef"));
        }
    }
    if let Some(t) = &req.ticket {
        compute::proto::ticket::Ticket::decode(t).map_err(|e| bad(&format!("ticket: {e}")))?;
    }
    let has_cred = req.enroll.is_some() || req.ticket.is_some();
    if has_cred {
        let c = req.controller.as_deref().ok_or_else(|| bad("pass --controller https://<server>"))?;
        if !(c.starts_with("https://") || c.starts_with("http://")) {
            return Err(bad("controller must be an http(s) URL"));
        }
        let n = req.name.as_deref().ok_or_else(|| bad("pass --name"))?;
        if !compute::enroll::valid_name(n) {
            return Err(bad("node names are 1–32 of a-z, 0-9 and '-'"));
        }
    }
    let (human, home) = current_user();
    let mut installed_now = false;
    let mut enrolled_by_setup = false;
    if !node_installed() {
        // The private-home choice is shown and asked, never silent.
        let check = local::home_check(&home);
        let open = check.as_ref().is_some_and(|c| c["private"] == false);
        if open && req.private_home.is_none() {
            return Err((
                "private_home_choice",
                400,
                format!(
                    "{} is readable by other users, so jobs (as k2node) could read files there. Choose: --private-home (chmod 700) or --no-private-home",
                    home.display()
                ),
            ));
        }
        let found = match find_binary() {
            Some(f) => f,
            None => Found {
                path: download_binary().map_err(|e| ("no_binary", 409, format!("no k2-node on this machine and the download failed: {e}")))?,
                source: "downloaded",
            },
        };
        let code_enroll = match (&req.controller, &req.enroll, &req.name) {
            (Some(c), Some(e), Some(n)) => Some((c.as_str(), e.as_str(), n.as_str())),
            _ => None,
        };
        let level = setup_remote_admin(req.remote_admin.as_deref(), admin_toml_exists());
        let args = setup_args(&human, req.private_home, code_enroll, level.as_deref());
        let el = elevation(&found.path, "setup", &args, req.allow_prompt);
        if el == Elevation::Command {
            return Ok(serde_json::json!({
                "ok": true,
                "step": "needs_root",
                "mode": "command",
                "rootNeeded": true,
                "command": command_line(&found.path, "setup", &args),
                "binary": found.path.display().to_string(),
                "message": "Run this once as an admin (it asks for your password). It creates the hidden k2node user and the service, and enrolls the node. Over SSH: ssh <host> 'sudo -S -p \"\" …' < password-file.",
            }));
        }
        let out = run_root(el, &found.path, "setup", &args).map_err(|e| ("setup_failed", 409, format!("k2-node setup ({}) failed: {e}", el.as_str())))?;
        log_debug!("[compute] k2-node setup via {}: {}", el.as_str(), out.lines().last().unwrap_or(""));
        installed_now = true;
        enrolled_by_setup = code_enroll.is_some();
        audit(by, "local_install", serde_json::json!({"via": el.as_str(), "binary": found.source, "remoteAdmin": level}));
    }
    if enrolled_by_setup {
        let sas = wait_for(Duration::from_secs(10), node_sas);
        return Ok(serde_json::json!({
            "ok": true, "step": "enrolling", "mode": "installed", "sas": sas,
            "message": match &sas {
                Some(s) => format!("Installed and enrolled. Confirm code {s} on the controller."),
                None => "Installed and enrolled; the confirmation code shows in Settings → Compute and `k2-node status`.".into(),
            },
        }));
    }
    if !has_cred {
        return Ok(serde_json::json!({
            "ok": true, "step": if installed_now { "installed" } else { "ready" },
            "nodeFp": wait_for(Duration::from_secs(10), || local::status()["node"]["node_fp"].as_str().map(str::to_string)),
            "message": "This computer is a compute node (not enrolled yet). Enroll it with a code or one-click.",
        }));
    }
    let id = local::write_enroll_request(
        req.controller.as_deref().unwrap_or_default(),
        req.enroll.as_deref(),
        req.ticket.as_deref(),
        req.name.as_deref().unwrap_or_default(),
        by,
    )
    .map_err(|e| {
        let hint = if e.contains("permission") {
            format!("{e}. K2 can't write the node's config yet: run `sudo k2-node setup` once more (it gives you access), or log out and back in")
        } else {
            e
        };
        ("local_write", 409, hint)
    })?;
    audit(by, "local_enroll_request", serde_json::json!({"id": id, "via": if req.ticket.is_some() { "ticket" } else { "code" }}));
    let result = wait_for(Duration::from_secs(20), || local::enroll_result().filter(|r| r["id"] == id.as_str()));
    match result {
        Some(r) if r["ok"] == true => {
            local::clear_enroll_request();
            Ok(serde_json::json!({"ok": true, "step": "enrolling", "sas": r["sas"], "message": r["message"]}))
        }
        Some(r) => {
            local::clear_enroll_request();
            Err(("enroll_failed", 409, r["message"].as_str().unwrap_or("the node couldn't enroll").to_string()))
        }
        None => Ok(serde_json::json!({
            "ok": true, "step": "waiting", "requestId": id,
            "message": "Asked the node to enroll; it answers within a few seconds (Settings → Compute shows the code).",
        })),
    }
}

/// `POST /cli/compute/local/remote-admin {level}`: the toggles under
/// "Make this computer a compute node", later. Root runs
/// `k2-node admin level <level>` the same way setup ran (passwordless
/// sudo, the macOS prompt from Settings, or one command to copy). Only the
/// owner reaches it: this machine's own K2, or the owner signed in over
/// K2 Connect from anywhere (then a Mac shows its admin prompt on its own
/// screen). Agents, App passes and Admin logins never do; an agent's
/// controller can only lower the level (A-L5 / B-L5).
pub fn set_remote_admin(level: &str, by: &str, allow_prompt: bool) -> Result<serde_json::Value, Refused> {
    if !REMOTE_ADMIN_LEVELS.contains(&level) {
        return Err(("bad_request", 400, "level is off, policy or full".into()));
    }
    let bin = installed_binary_path();
    if !local::config_dir().is_dir() || !bin.is_file() {
        return Err(("not_a_node", 409, "this computer isn't a compute node yet (k2 compute local enable)".into()));
    }
    let args = vec!["level".to_string(), level.to_string()];
    let el = elevation_with(&bin, "admin", &args, allow_prompt, false);
    if el == Elevation::Command {
        return Ok(serde_json::json!({
            "ok": true, "done": false, "rootNeeded": true, "mode": "command",
            "command": command_line(&bin, "admin", &args),
            "message": "Run this once as an admin; this page updates by itself.",
        }));
    }
    run_root(el, &bin, "admin", &args).map_err(|e| ("remote_admin_failed", 409, format!("k2-node admin level ({}) failed: {e}", el.as_str())))?;
    audit(by, "local_remote_admin", serde_json::json!({"level": level, "via": el.as_str()}));
    Ok(serde_json::json!({
        "ok": true, "done": true, "mode": el.as_str(), "remoteAdmin": level,
        "message": match level {
            "off" => "Remote admin is off: K2 can't update this node or keep it healthy.",
            "policy" => "K2 keeps this node updated and healthy (remote admin: policy).",
            _ => "K2 may also run admin actions as root here, each one approved by you (remote admin: full).",
        },
    }))
}

/// `POST /cli/compute/local/disable`.
pub fn disable(by: &str, allow_prompt: bool) -> Result<serde_json::Value, Refused> {
    if !node_installed() && !local::config_dir().is_dir() {
        return Ok(serde_json::json!({"ok": true, "done": true, "message": "this computer isn't a compute node"}));
    }
    let bin = if installed_binary_path().is_file() { installed_binary_path() } else { find_binary().map(|f| f.path).unwrap_or_else(installed_binary_path) };
    let el = elevation(&bin, "uninstall", &[], allow_prompt);
    if el == Elevation::Command {
        return Ok(serde_json::json!({
            "ok": true, "done": false, "rootNeeded": true, "mode": "command",
            "command": command_line(&bin, "uninstall", &[]),
            "message": "Run this once as an admin: it stops the node's jobs and removes the service, the hidden user and its home. Remove the node on its controller too (k2 compute node remove <name>).",
        }));
    }
    run_root(el, &bin, "uninstall", &[]).map_err(|e| ("uninstall_failed", 409, e))?;
    audit(by, "local_uninstall", serde_json::json!({"via": el.as_str()}));
    Ok(serde_json::json!({"ok": true, "done": true, "mode": el.as_str(), "message": "This computer is no longer a compute node."}))
}

fn audit(by: &str, kind: &str, detail: serde_json::Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    compute::store::record_event(&conn, by, kind, None, None, None, detail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_hostile_values() {
        assert_eq!(shell_quote("mini-1"), "mini-1");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("x'y"), "'x'\\''y'");
        assert_eq!(shell_quote("$(reboot)"), "'$(reboot)'");
        let line = command_line(Path::new("/Applications/K2.app/Contents/MacOS/k2-node"), "setup", &["--human".into(), "ro sson".into()]);
        assert_eq!(line, "sudo /Applications/K2.app/Contents/MacOS/k2-node setup --human 'ro sson'");
        let a = applescript_for("'/x/k2-node' setup --name \"q\"\\");
        assert_eq!(a, "do shell script \"'/x/k2-node' setup --name \\\"q\\\"\\\\\" with administrator privileges");
    }

    #[test]
    fn setup_args_carry_the_choice_and_the_code() {
        assert_eq!(setup_args("rosson", Some(true), None, Some("policy")), vec!["--human", "rosson", "--private-home", "--remote-admin", "policy"]);
        assert_eq!(setup_args("rosson", Some(false), None, Some("off")), vec!["--human", "rosson", "--no-private-home", "--remote-admin", "off"]);
        assert_eq!(setup_args("rosson", None, None, None), vec!["--human", "rosson"], "no level: the installer keeps the owner's");
        let a = setup_args("r", None, Some(("https://rosson.k2.dev", "ABCDE-FGHJK.0123456789abcdef", "mini-1")), Some("full"));
        assert_eq!(
            a,
            vec!["--human", "r", "--remote-admin", "full", "--controller", "https://rosson.k2.dev", "--enroll", "ABCDE-FGHJK.0123456789abcdef", "--name", "mini-1"]
        );
    }

    /// Rosson 2026-10-24: "Let K2 keep this node updated and healthy" is on
    /// unless the request says otherwise; `full` only when asked; anything
    /// else is refused before root runs.
    #[test]
    fn remote_admin_defaults_to_policy_and_takes_only_the_three_levels() {
        use serde_json::json;
        assert_eq!(remote_admin_arg(None, Some(DEFAULT_REMOTE_ADMIN)).unwrap(), "policy");
        assert_eq!(remote_admin_arg(Some(&json!(null)), Some(DEFAULT_REMOTE_ADMIN)).unwrap(), "policy");
        for l in ["off", "policy", "full"] {
            assert_eq!(remote_admin_arg(Some(&json!(l)), Some(DEFAULT_REMOTE_ADMIN)).unwrap(), l);
        }
        for bad in [json!("root"), json!(true), json!(2)] {
            assert_eq!(remote_admin_arg(Some(&bad), Some(DEFAULT_REMOTE_ADMIN)).unwrap_err().1, 400, "{bad}");
        }
        assert_eq!(remote_admin_arg(None, None).unwrap_err().0, "bad_request", "the later change names a level");
        assert_eq!(set_remote_admin("root", "test", false).unwrap_err().1, 400);
        // A-L4 / B-L6: a reinstall over an earlier choice passes none.
        assert_eq!(setup_remote_admin(None, false).as_deref(), Some("policy"), "a first install: policy");
        assert_eq!(setup_remote_admin(None, true), None, "an earlier choice (off included) stands");
        assert_eq!(setup_remote_admin(Some("off"), false).as_deref(), Some("off"));
        assert_eq!(setup_remote_admin(Some("full"), true).as_deref(), Some("full"), "a named level is the owner's word");
        let r = EnableReq { remote_admin: Some("sudo".into()), ..Default::default() };
        assert_eq!(enable(&r, "test").unwrap_err().0, "bad_request", "checked first, before any install step");
    }

    #[test]
    fn requests_are_validated_before_anything_runs() {
        let bad = |r: EnableReq| enable(&r, "test").unwrap_err();
        let both = EnableReq { enroll: Some("ABCDE-FGHJK.0123456789abcdef".into()), ticket: Some("x".into()), ..Default::default() };
        assert_eq!(bad(both).0, "bad_request");
        let junk = EnableReq { enroll: Some("nope".into()), ..Default::default() };
        assert_eq!(bad(junk).0, "bad_request");
        let no_ctrl = EnableReq { enroll: Some("ABCDE-FGHJK.0123456789abcdef".into()), name: Some("n".into()), ..Default::default() };
        assert!(bad(no_ctrl).2.contains("--controller"));
        let bad_name = EnableReq {
            enroll: Some("ABCDE-FGHJK.0123456789abcdef".into()),
            controller: Some("https://x.k2.dev".into()),
            name: Some("Bad Name".into()),
            ..Default::default()
        };
        assert_eq!(bad(bad_name).0, "bad_request");
        let bad_ticket = EnableReq { ticket: Some("garbage!".into()), controller: Some("https://x.k2.dev".into()), name: Some("n".into()), ..Default::default() };
        assert!(bad(bad_ticket).2.starts_with("ticket:"));
    }

    #[test]
    fn node_downloads_need_the_root_helper_key_and_this_version() {
        use k2_root_proto::keys::{trusted_keys, TEST_KEY};
        use k2_root_proto::node_build::{format_marker, trusted_comment};
        use k2_root_proto::testsign::{fake_elf, sign, sign_with_test_key};
        let keys = [TEST_KEY];
        let elf = fake_elf("x86_64", &format_marker("0.45.5", 2, "linux", "x86_64"));
        let good = sign_with_test_key(&elf, &trusted_comment("linux", "x86_64", "0.45.5", 2));
        check_node_download(&elf, &good, &keys, "x86_64", "0.45.5").unwrap();
        // Production trusts only RELEASE_KEYS: a test-key signature is refused.
        assert!(check_node_download(&elf, &good, &trusted_keys(false), "x86_64", "0.45.5").is_err());
        // Another key (e.g. the app-updater key) is refused even with a perfect comment.
        let other = sign(&[9u8; 32], [1, 2, 3, 4, 5, 6, 7, 8], &elf, &trusted_comment("linux", "x86_64", "0.45.5", 2));
        let e = check_node_download(&elf, &other, &keys, "x86_64", "0.45.5").unwrap_err();
        assert!(e.contains("not valid for any trusted key"), "{e}");
        // The real updater-key signature format (tauri's wrapped text) is not even readable here.
        assert!(check_node_download(&elf, "dW50cnVzdGVkIGNvbW1lbnQ6", &keys, "x86_64", "0.45.5").is_err());
        // Wrong trusted comment: the root helper's own file name, or another arch.
        let helper = sign_with_test_key(&elf, &k2_root_proto::sig::trusted_comment("x86_64", "0.45.5", 2));
        assert!(check_node_download(&elf, &helper, &keys, "x86_64", "0.45.5").unwrap_err().contains("names another file"));
        let arm = sign_with_test_key(&elf, &trusted_comment("linux", "aarch64", "0.45.5", 2));
        assert!(check_node_download(&elf, &arm, &keys, "x86_64", "0.45.5").is_err());
        // A good build of another release is refused (this daemon's version only).
        let e = check_node_download(&elf, &good, &keys, "x86_64", "0.45.6").unwrap_err();
        assert!(e.contains("this daemon is v0.45.6"), "{e}");
        // Tampered bytes.
        let mut bad = elf.clone();
        bad[70] ^= 1;
        assert!(check_node_download(&bad, &good, &keys, "x86_64", "0.45.5").is_err());
    }

    #[test]
    fn root_helper_is_only_used_with_a_node_namespace() {
        // No helper on dev machines / CI: reported, never chosen.
        if !Path::new(ROOT_HELPER).exists() {
            assert_eq!(root_helper_node(), serde_json::json!({"present": false, "node": false}));
        } else {
            assert_eq!(root_helper_node()["present"], true);
        }
    }
}
