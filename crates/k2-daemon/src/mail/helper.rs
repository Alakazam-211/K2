//! Allowlist for `/usr/local/libexec/k2-mail-helper`.
//!
//! User `k2` may `sudo -n` that one path. This module is the allowlist:
//! exact argv, canned unit bytes, and the tarball pin. The helper binary
//! calls the executor through the library. `main.rs` compiles this same
//! file into the daemon and does not call the executor, so those items
//! are unused in that build. `release.sh` checks with `-D warnings`.
//!
//! [`super::sysops::RealSystemOps`] calls the path constants. Tests do
//! not exec the binary, spawn sudo, or write `/etc`.
#![allow(dead_code)]

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::supervisor::{
    artifact_for_arch, hardening_dropin, previous_artifact_for_arch, systemd_unit, STALWART_BIN,
    STALWART_BIN_STAGING,
    STALWART_CONFIG_DIR, STALWART_DATA_DIR, STALWART_DROPIN_DIR, STALWART_DROPIN_PATH,
    STALWART_LOG_DIR, STALWART_PINNED_VERSION, STALWART_PREVIOUS_VERSION, STALWART_SNAPSHOT_DIR,
    STALWART_UNIT, STALWART_UNIT_PATH, STALWART_USER,
};

pub const HELPER_PATH: &str = "/usr/local/libexec/k2-mail-helper";
pub const SUDO_PATH: &str = "/usr/bin/sudo";

/// The helper's verb protocol, printed by `version`. Bump on any verb or
/// policy change the daemon has to know about before it calls.
/// - 1: the original verbs (no `version`; an old helper answers it with
///   "refused arguments").
/// - 2: calendars S1 upgrade — `version`, `usage`, `snapshot-data`,
///   `restore-data`, `systemctl stop stalwart`; `install-bin` accepts the
///   pin OR the previous version's tarball and installs atomically.
pub const HELPER_PROTOCOL: u32 = 2;

/// `k2 hostmail upgrade` refuses below this protocol.
pub const UPGRADE_MIN_PROTOCOL: u32 = 2;

/// Free space a snapshot needs beyond the bytes it copies (CAL14).
pub const SNAPSHOT_FREE_MARGIN: u64 = 1 << 30;

/// RocksDB's lock file inside the store (`{data}/data/LOCK`); a process
/// that still has it locked still has the store open.
pub const STALWART_STORE_LOCK: &str = "/var/lib/stalwart/data/LOCK";

const RECOVERY_ENV_PREFIX: &str = "Environment=STALWART_RECOVERY_ADMIN=admin:";
/// Official Stalwart binary upper bound. The pin check runs first.
const MAX_STALWART_MEMBER: u64 = 256 * 1024 * 1024;

const MKDIR_PATHS: &[&str] = &[
    STALWART_CONFIG_DIR,
    STALWART_DATA_DIR,
    STALWART_LOG_DIR,
    STALWART_DROPIN_DIR,
];
const CHOWN_PATHS: &[&str] = &[STALWART_CONFIG_DIR, STALWART_DATA_DIR, STALWART_LOG_DIR];
const WRITE_PATHS: &[&str] = &[STALWART_UNIT_PATH, STALWART_DROPIN_PATH];
const REMOVE_PATHS: &[&str] = &[
    STALWART_UNIT_PATH,
    STALWART_DROPIN_DIR,
    STALWART_BIN,
    STALWART_DATA_DIR,
    STALWART_LOG_DIR,
    STALWART_CONFIG_DIR,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperCommand {
    EnsureUser,
    Mkdir(&'static str),
    Chown(&'static str),
    Write(&'static str),
    InstallBin,
    Systemctl(&'static [&'static str]),
    Remove(&'static str),
    /// Prints [`version_json`]: protocol + the Stalwart table this helper
    /// was built with. Read-only.
    Version,
    /// Prints [`DataLayout`] sizes + free space as JSON. Read-only.
    Usage,
    /// Copies the data + config dirs to [`STALWART_SNAPSHOT_DIR`]. No
    /// argument: the paths are compiled in. Refuses unless Stalwart is
    /// stopped.
    SnapshotData,
    /// Replaces the data + config dirs with the complete snapshot. No
    /// argument. Refuses unless Stalwart is stopped.
    RestoreData,
}

impl HelperCommand {
    pub fn reads_stdin(self) -> bool {
        matches!(self, Self::Write(_) | Self::InstallBin)
    }

    pub fn argv(self) -> Vec<&'static str> {
        match self {
            Self::EnsureUser => vec!["ensure-user"],
            Self::Mkdir(p) => vec!["mkdir", p],
            Self::Chown(p) => vec!["chown", p],
            Self::Write(p) => vec!["write", p],
            Self::InstallBin => vec!["install-bin"],
            Self::Systemctl(args) => {
                let mut v = Vec::with_capacity(1 + args.len());
                v.push("systemctl");
                v.extend_from_slice(args);
                v
            }
            Self::Remove(p) => vec!["remove", p],
            Self::Version => vec!["version"],
            Self::Usage => vec!["usage"],
            Self::SnapshotData => vec!["snapshot-data"],
            Self::RestoreData => vec!["restore-data"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Missing,
    Symlink,
    Directory,
    File,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEffect {
    CreateDir { mode: u32 },
    DirAlready { mode: u32 },
    ChownDir,
    WriteFile { mode: u32 },
    InstallFile { mode: u32 },
    RemoveFile,
    RemoveDir,
    RemoveAbsent,
}

fn arg_has_meta(arg: &str) -> bool {
    arg.bytes()
        .any(|b| matches!(b, b' ' | b';' | b'|' | b'\n' | b'\r' | b'\0'))
}

fn static_path<'a>(allowed: &[&'a str], path: &str) -> Option<&'a str> {
    allowed.iter().copied().find(|p| *p == path)
}

fn systemctl_vector(args: &[&str]) -> Option<&'static [&'static str]> {
    const DAEMON_RELOAD: &[&str] = &["daemon-reload"];
    const ENABLE_NOW: &[&str] = &["enable", "--now", STALWART_UNIT];
    const RESTART: &[&str] = &["restart", STALWART_UNIT];
    const DISABLE_NOW: &[&str] = &["disable", "--now", STALWART_UNIT];
    // Protocol 2 (upgrade): stop for the snapshot window WITHOUT touching
    // the unit's boot enablement (never `disable --now`, which is the
    // hostmail-disable path). Start is the existing `restart` door.
    const STOP: &[&str] = &["stop", STALWART_UNIT];
    [DAEMON_RELOAD, ENABLE_NOW, RESTART, DISABLE_NOW, STOP]
        .into_iter()
        .find(|v| *v == args)
}

/// Exact argv or a hard error. Nothing is stripped.
pub fn parse_argv(args: &[&str]) -> Result<HelperCommand, String> {
    if args.is_empty() {
        return Err("mail helper: empty argv".into());
    }
    if args.iter().any(|a| a.is_empty() || arg_has_meta(a)) {
        return Err("mail helper: refused argv".into());
    }
    match args {
        ["ensure-user"] => Ok(HelperCommand::EnsureUser),
        ["install-bin"] => Ok(HelperCommand::InstallBin),
        ["version"] => Ok(HelperCommand::Version),
        ["usage"] => Ok(HelperCommand::Usage),
        ["snapshot-data"] => Ok(HelperCommand::SnapshotData),
        ["restore-data"] => Ok(HelperCommand::RestoreData),
        ["mkdir", path] => static_path(MKDIR_PATHS, path)
            .map(HelperCommand::Mkdir)
            .ok_or_else(|| "mail helper: refused arguments".into()),
        ["chown", path] => static_path(CHOWN_PATHS, path)
            .map(HelperCommand::Chown)
            .ok_or_else(|| "mail helper: refused arguments".into()),
        ["write", path] => static_path(WRITE_PATHS, path)
            .map(HelperCommand::Write)
            .ok_or_else(|| "mail helper: refused arguments".into()),
        ["remove", path] => static_path(REMOVE_PATHS, path)
            .map(HelperCommand::Remove)
            .ok_or_else(|| "mail helper: refused arguments".into()),
        ["systemctl", rest @ ..] => systemctl_vector(rest)
            .map(HelperCommand::Systemctl)
            .ok_or_else(|| "mail helper: refused arguments".into()),
        _ => Err("mail helper: refused arguments".into()),
    }
}

pub fn require_stalwart_user(user: &str) -> Result<(), String> {
    if user == STALWART_USER {
        Ok(())
    } else {
        Err("mail helper: refused user".into())
    }
}

/// Mail only ever extracts this one member. Any other dest/mode is refused
/// so a caller cannot choose `04755` or a path under `/tmp`.
pub fn extract_request_allowed(member: &str, dest: &str, mode: u32) -> Result<(), String> {
    if member == "stalwart" && dest == STALWART_BIN && mode == 0o755 {
        Ok(())
    } else {
        Err("extract refused".into())
    }
}

/// Forced mode for an allowlisted command. There is no requested-mode argument.
pub fn forced_mode(cmd: HelperCommand) -> Option<u32> {
    match cmd {
        HelperCommand::Mkdir(_) => Some(0o755),
        HelperCommand::Write(STALWART_UNIT_PATH) => Some(0o600),
        HelperCommand::Write(STALWART_DROPIN_PATH) => Some(0o644),
        HelperCommand::InstallBin => Some(0o755),
        _ => None,
    }
}

fn recovery_pw_ok(pw: &str) -> bool {
    pw.len() == 64 && pw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn unit_bytes_match(stdin: &[u8]) -> bool {
    if stdin == systemd_unit(None).as_bytes() {
        return true;
    }
    let Ok(text) = std::str::from_utf8(stdin) else {
        return false;
    };
    let Some(start) = text.find(RECOVERY_ENV_PREFIX) else {
        return false;
    };
    let after = &text[start + RECOVERY_ENV_PREFIX.len()..];
    if after.contains(RECOVERY_ENV_PREFIX) {
        return false;
    }
    let Some(nl) = after.find('\n') else {
        return false;
    };
    let pw = &after[..nl];
    if !recovery_pw_ok(pw) {
        return false;
    }
    text.as_bytes() == systemd_unit(Some(pw)).as_bytes()
}

/// Accept stdin only when it is one of the two unit templates or the canned
/// drop-in, on the matching path. The error is static — caller bytes stay
/// out of logs.
pub fn write_bytes_accepted(path: &str, stdin: &[u8]) -> Result<u32, String> {
    if path == STALWART_UNIT_PATH && unit_bytes_match(stdin) {
        return Ok(0o600);
    }
    if path == STALWART_DROPIN_PATH && stdin == hardening_dropin().as_bytes() {
        return Ok(0o644);
    }
    Err("write refused".into())
}

/// Stdin is the tarball, checked against EXACTLY two compiled-in entries
/// for this process's arch: the pin ([`artifact_for_arch`]) and the
/// previous version ([`previous_artifact_for_arch`], the upgrade
/// rollback). Returns which version matched. A member hash is not a pin.
pub fn install_archive_allowed(bytes: &[u8]) -> Result<&'static str, String> {
    install_archive_allowed_for(bytes, std::env::consts::ARCH)
}

pub fn install_archive_allowed_for(bytes: &[u8], arch: &str) -> Result<&'static str, String> {
    match_archive(
        bytes,
        artifact_for_arch(arch)?,
        previous_artifact_for_arch(arch)?,
    )
}

/// The two-entry check itself: the pin first, then the previous version.
/// Nothing else is ever accepted.
pub fn match_archive(
    bytes: &[u8],
    pin: &super::supervisor::StalwartArtifact,
    previous: &super::supervisor::StalwartArtifact,
) -> Result<&'static str, String> {
    if crate::update_routes::verify_sha256(bytes, pin.sha256) {
        return Ok(STALWART_PINNED_VERSION);
    }
    if crate::update_routes::verify_sha256(bytes, previous.sha256) {
        return Ok(STALWART_PREVIOUS_VERSION);
    }
    Err("install-bin: tarball sha256 mismatch".into())
}

/// What `version` prints: the protocol and the exact Stalwart table this
/// helper build installs, so the daemon refuses before any effect when
/// the helper on the box was built for another pin.
pub fn version_json() -> serde_json::Value {
    version_json_for(std::env::consts::ARCH)
}

pub fn version_json_for(arch: &str) -> serde_json::Value {
    serde_json::json!({
        "helper": "k2-mail-helper",
        "protocol": HELPER_PROTOCOL,
        "k2Version": env!("CARGO_PKG_VERSION"),
        "stalwartPin": STALWART_PINNED_VERSION,
        "stalwartPinSha256": artifact_for_arch(arch).map(|a| a.sha256).ok(),
        "stalwartPrevious": STALWART_PREVIOUS_VERSION,
        "stalwartPreviousSha256": previous_artifact_for_arch(arch).map(|a| a.sha256).ok(),
    })
}

/// The fixed paths `snapshot-data`, `restore-data` and `usage` act on.
/// Production is [`DataLayout::fixed`]; nothing on argv can choose a path.
/// Tests build one over a temp tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    pub data: PathBuf,
    pub config: PathBuf,
    pub snapshot: PathBuf,
    pub store_lock: PathBuf,
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

impl DataLayout {
    pub fn fixed() -> Self {
        Self {
            data: PathBuf::from(STALWART_DATA_DIR),
            config: PathBuf::from(STALWART_CONFIG_DIR),
            snapshot: PathBuf::from(STALWART_SNAPSHOT_DIR),
            store_lock: PathBuf::from(STALWART_STORE_LOCK),
        }
    }
    /// The snapshot is built here and renamed onto `snapshot` when done.
    pub fn snapshot_tmp(&self) -> PathBuf {
        with_suffix(&self.snapshot, ".tmp")
    }
    pub fn snapshot_data(&self) -> PathBuf {
        self.snapshot.join("data")
    }
    pub fn snapshot_config(&self) -> PathBuf {
        self.snapshot.join("config")
    }
    pub fn snapshot_marker(&self) -> PathBuf {
        self.snapshot
            .join(super::supervisor::STALWART_SNAPSHOT_MARKER)
    }
    pub fn data_restore_tmp(&self) -> PathBuf {
        with_suffix(&self.data, ".k2-restore.tmp")
    }
    pub fn data_aside(&self) -> PathBuf {
        with_suffix(&self.data, ".k2-failed")
    }
    pub fn config_restore_tmp(&self) -> PathBuf {
        with_suffix(&self.config, ".k2-restore.tmp")
    }
    pub fn config_aside(&self) -> PathBuf {
        with_suffix(&self.config, ".k2-failed")
    }
}

fn exact_stalwart_name(path: &Path) -> bool {
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(name)), None) => name == "stalwart",
        _ => false,
    }
}

/// Read member `stalwart` only. Other names, links, and `..` are not written.
pub fn extract_stalwart_member(archive: &[u8]) -> Result<Vec<u8>, String> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(dec);
    let entries = tar
        .entries()
        .map_err(|_| "install-bin: archive unreadable".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "install-bin: archive entry unreadable".to_string())?;
        let path = entry
            .path()
            .map_err(|_| "install-bin: archive path unreadable".to_string())?;
        if !exact_stalwart_name(&path) {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err("install-bin: stalwart member is not a regular file".into());
        }
        let mut buf = Vec::new();
        entry
            .take(MAX_STALWART_MEMBER.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|_| "install-bin: read member failed".to_string())?;
        if buf.len() as u64 > MAX_STALWART_MEMBER {
            return Err("install-bin: stalwart member too large".into());
        }
        if buf.is_empty() {
            return Err("install-bin: stalwart member empty".into());
        }
        return Ok(buf);
    }
    Err("install-bin: stalwart member missing".into())
}

pub fn decide_effect(cmd: HelperCommand, kind: NodeKind) -> Result<FsEffect, String> {
    match cmd {
        HelperCommand::Mkdir(_) => match kind {
            NodeKind::Missing => Ok(FsEffect::CreateDir { mode: 0o755 }),
            NodeKind::Directory => Ok(FsEffect::DirAlready { mode: 0o755 }),
            NodeKind::Symlink => Err("mkdir refused: symlink".into()),
            _ => Err("mkdir refused".into()),
        },
        HelperCommand::Chown(_) => match kind {
            NodeKind::Directory => Ok(FsEffect::ChownDir),
            NodeKind::Symlink => Err("chown refused: symlink".into()),
            NodeKind::Missing => Err("chown refused: missing".into()),
            _ => Err("chown refused".into()),
        },
        HelperCommand::Remove(_) => match kind {
            NodeKind::Missing => Ok(FsEffect::RemoveAbsent),
            NodeKind::Symlink => Err("remove refused: symlink".into()),
            NodeKind::Directory => Ok(FsEffect::RemoveDir),
            NodeKind::File => Ok(FsEffect::RemoveFile),
            NodeKind::Other => Err("remove refused".into()),
        },
        HelperCommand::Write(_) => match kind {
            NodeKind::Symlink => Err("write refused".into()),
            NodeKind::Directory | NodeKind::Other => Err("write refused".into()),
            NodeKind::Missing | NodeKind::File => {
                let mode = forced_mode(cmd).ok_or_else(|| "write refused".to_string())?;
                Ok(FsEffect::WriteFile { mode })
            }
        },
        HelperCommand::InstallBin => match kind {
            NodeKind::Symlink | NodeKind::Directory | NodeKind::Other => {
                Err("install-bin refused".into())
            }
            NodeKind::Missing | NodeKind::File => Ok(FsEffect::InstallFile { mode: 0o755 }),
        },
        HelperCommand::EnsureUser
        | HelperCommand::Systemctl(_)
        | HelperCommand::Version
        | HelperCommand::Usage
        | HelperCommand::SnapshotData
        | HelperCommand::RestoreData => Err("mail helper: no filesystem effect".into()),
    }
}

/// Where the root installer and the helper assets live. One release
/// per tag: `install-mail-helper.sh`, `k2-mail-helper-linux-<arch>`
/// (+ `.sig` minisign, `.sha256`). Uploaded by `daemon-binaries.yml`.
pub const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/Alakazam-211/K2/releases/download";

/// Is the root door usable by this daemon user right now?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperState {
    /// Present and `sudo -n` may run it.
    Installed,
    /// `/usr/local/libexec/k2-mail-helper` (or sudo itself) is absent.
    Missing,
    /// Present, but sudoers does not let this user run it without a password.
    NotAllowed,
}

impl HelperState {
    /// The preflight / status word: `installed | missing | not allowed by sudoers`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Missing => "missing",
            Self::NotAllowed => "not allowed by sudoers",
        }
    }
}

/// The one-line root fix for `version`. The installer and the helper
/// binary are release assets of the same tag, so the helper argv always
/// matches the daemon that asks for it.
pub fn install_command_for(version: &str) -> String {
    format!(
        "curl -fsSL {RELEASE_DOWNLOAD_BASE}/v{version}/install-mail-helper.sh | sudo bash -s -- --version {version}"
    )
}

/// [`install_command_for`] this build's version.
pub fn install_command() -> String {
    install_command_for(env!("CARGO_PKG_VERSION"))
}

/// The error every helper-dependent path returns when the door is shut.
/// Names the exact fix. Never includes sudo's stderr. `None` when installed.
pub fn unavailable_message(state: HelperState) -> Option<String> {
    let what = match state {
        HelperState::Installed => return None,
        HelperState::Missing => format!("mail helper not installed ({HELPER_PATH} is missing)"),
        HelperState::NotAllowed => format!(
            "mail helper not allowed by sudoers (this daemon user may not `sudo -n {HELPER_PATH}`)"
        ),
    };
    Some(format!("{what} — run as root: {}", install_command()))
}

/// Classify a failed `sudo -n <helper>`. `NotFound` (sudo missing) and
/// "command not found" for the helper path are [`HelperState::Missing`].
/// sudo's password / allow refusals are [`HelperState::NotAllowed`] when
/// the helper file exists, else Missing (sudo refuses an unlisted command
/// before it looks for the file). Other stderr is not a door problem.
pub fn classify_helper_failure(
    spawn: Option<&std::io::Error>,
    stderr: &str,
    helper_present: bool,
) -> Option<HelperState> {
    if spawn.is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Some(HelperState::Missing);
    }
    if stderr.contains(HELPER_PATH)
        && (stderr.contains("command not found") || stderr.contains("No such file or directory"))
    {
        return Some(HelperState::Missing);
    }
    if stderr.contains("a password is required") || stderr.contains("not allowed to execute") {
        return Some(if helper_present {
            HelperState::NotAllowed
        } else {
            HelperState::Missing
        });
    }
    None
}

/// [`classify_helper_failure`] mapped to the teaching error (with the fix).
pub fn map_helper_failure_with(
    spawn: Option<&std::io::Error>,
    stderr: &str,
    helper_present: bool,
) -> Option<String> {
    classify_helper_failure(spawn, stderr, helper_present).and_then(unavailable_message)
}

/// Production mapper: checks the helper path on disk.
pub fn map_helper_failure(spawn: Option<&std::io::Error>, stderr: &str) -> Option<String> {
    map_helper_failure_with(spawn, stderr, Path::new(HELPER_PATH).exists())
}

/// Probe the door without running a verb: the file, then
/// `sudo -n -l <helper>` (exit 0 = allowed without a password). Never runs
/// the helper. Production only; tests use the [`super::sysops`] fake.
pub fn probe_state() -> HelperState {
    if !Path::new(HELPER_PATH).exists() {
        return HelperState::Missing;
    }
    match std::process::Command::new(SUDO_PATH)
        .args(["-n", "-l", HELPER_PATH])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(st) if st.success() => HelperState::Installed,
        Ok(_) => HelperState::NotAllowed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HelperState::Missing,
        Err(_) => HelperState::NotAllowed,
    }
}

/// Enable marks a step only after the privileged call returns `Ok`.
pub fn mark_step_on_ok(result: &Result<(), String>, marked: &mut bool) {
    if result.is_ok() {
        *marked = true;
    }
}

pub fn is_privileged_recording(line: &str) -> bool {
    line.starts_with("mkdir ")
        || line.starts_with("chown ")
        || line.starts_with("write ")
        || line.starts_with("rm ")
        || line.starts_with("systemctl ")
        || line.starts_with("systemctl?")
        || line == "snapshot-data"
        || line == "restore-data"
        || line.starts_with("helper ")
}

/// Map a [`super::sysops`] fake recording line onto a §2 helper vector.
pub fn recorded_line_allowlisted(line: &str) -> Result<(), String> {
    if let Some(rest) = line.strip_prefix("systemctl?") {
        let _ = rest;
        return Err("systemctl query is not a helper verb".into());
    }
    if line == "snapshot-data" || line == "restore-data" {
        return parse_argv(&[line]).map(|_| ());
    }
    if let Some(verb) = line.strip_prefix("helper ") {
        // Read-only queries (`version`, `usage`) the fake records.
        if verb.contains(' ') {
            return Err("helper recording has extra fields".into());
        }
        return match parse_argv(&[verb])? {
            HelperCommand::Version | HelperCommand::Usage => Ok(()),
            _ => Err("helper recording is not a query verb".into()),
        };
    }
    if let Some(path) = line.strip_prefix("mkdir ") {
        if path.contains(' ') {
            return Err("mkdir recording has extra fields".into());
        }
        return parse_argv(&["mkdir", path]).map(|_| ());
    }
    if let Some(rest) = line.strip_prefix("chown ") {
        let Some((user, path)) = rest.split_once(' ') else {
            return Err("chown recording missing path".into());
        };
        if path.contains(' ') {
            return Err("chown recording has extra fields".into());
        }
        require_stalwart_user(user)?;
        return parse_argv(&["chown", path]).map(|_| ());
    }
    if let Some(rest) = line.strip_prefix("write ") {
        let Some((path, meta)) = rest.split_once(" (") else {
            return Err("write recording missing meta".into());
        };
        let cmd = parse_argv(&["write", path])?;
        let Some(mode) = meta.split("mode ").nth(1) else {
            return Err("write recording missing mode".into());
        };
        let mode = mode.trim_end_matches(')');
        let expected =
            forced_mode(cmd).ok_or_else(|| "write recording has no forced mode".to_string())?;
        if mode != format!("{expected:o}") {
            return Err("write recording mode is not the forced mode".into());
        }
        return Ok(());
    }
    if let Some(path) = line.strip_prefix("rm ") {
        if path.contains(' ') {
            return Err("rm recording has extra fields".into());
        }
        return parse_argv(&["remove", path]).map(|_| ());
    }
    if let Some(rest) = line.strip_prefix("systemctl ") {
        let mut argv = vec!["systemctl"];
        if rest.is_empty() || rest.split(' ').any(|p| p.is_empty()) {
            return Err("systemctl recording is empty".into());
        }
        argv.extend(rest.split(' '));
        return parse_argv(&argv).map(|_| ());
    }
    Err("not a helper recording".into())
}

pub fn execute(cmd: HelperCommand, stdin: &[u8]) -> Result<(), String> {
    let argv = cmd.argv();
    let cmd = parse_argv(&argv)?;
    #[cfg(unix)]
    {
        unix_io::perform(cmd, stdin)
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, stdin);
        Err("mail helper: unix only".into())
    }
}

#[cfg(unix)]
mod unix_io {
    use super::{
        data_ops, decide_effect, extract_stalwart_member, install_archive_allowed, version_json,
        write_bytes_accepted, DataLayout, FsEffect, HelperCommand, NodeKind, STALWART_BIN,
        STALWART_UNIT, STALWART_USER,
    };
    use std::ffi::CString;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub fn perform(cmd: HelperCommand, stdin: &[u8]) -> Result<(), String> {
        match cmd {
            HelperCommand::EnsureUser => ensure_user(),
            HelperCommand::Systemctl(args) => systemctl(args),
            HelperCommand::Mkdir(path)
            | HelperCommand::Chown(path)
            | HelperCommand::Remove(path) => apply_fs(cmd, path, None),
            HelperCommand::Write(path) => {
                let mode = write_bytes_accepted(path, stdin)?;
                apply_fs(cmd, path, Some((stdin, mode)))
            }
            HelperCommand::InstallBin => {
                install_archive_allowed(stdin)?;
                let member = extract_stalwart_member(stdin)?;
                // Refuse a symlink / directory at the final path (same
                // policy as before), then stage + rename: never a
                // truncate-in-place of the binary systemd runs.
                decide_effect(cmd, node_kind(STALWART_BIN)?)?;
                data_ops::install_file_atomic(
                    Path::new(super::STALWART_BIN_STAGING),
                    Path::new(STALWART_BIN),
                    &member,
                    0o755,
                    Some((0, 0)),
                )
            }
            HelperCommand::Version => {
                println!("{}", version_json());
                Ok(())
            }
            HelperCommand::Usage => {
                let v = data_ops::usage_with(&DataLayout::fixed(), &data_ops::free_bytes)?;
                println!("{v}");
                Ok(())
            }
            HelperCommand::SnapshotData => {
                let v = data_ops::snapshot_with(
                    &DataLayout::fixed(),
                    &unit_state,
                    &data_ops::free_bytes,
                    super::SNAPSHOT_FREE_MARGIN,
                )?;
                println!("{v}");
                Ok(())
            }
            HelperCommand::RestoreData => {
                let v = data_ops::restore_with(
                    &DataLayout::fixed(),
                    &unit_state,
                    &data_ops::free_bytes,
                    super::SNAPSHOT_FREE_MARGIN,
                )?;
                println!("{v}");
                Ok(())
            }
        }
    }

    /// `systemctl is-active stalwart` as root: a non-zero exit is an answer
    /// (`inactive` exits 3); no output at all reads as unknown and the
    /// data verbs refuse on it.
    fn unit_state() -> String {
        Command::new("/usr/bin/systemctl")
            .args(["is-active", STALWART_UNIT])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn apply_fs(
        cmd: HelperCommand,
        path: &str,
        payload: Option<(&[u8], u32)>,
    ) -> Result<(), String> {
        let kind = node_kind(path)?;
        match decide_effect(cmd, kind)? {
            FsEffect::CreateDir { mode } => {
                match std::fs::create_dir(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(format!("mkdir failed: {e}")),
                }
                if !matches!(node_kind(path)?, NodeKind::Directory) {
                    return Err("mkdir refused: symlink".into());
                }
                fchmod_nofollow(path, mode, true)
            }
            FsEffect::DirAlready { mode } => {
                if !matches!(node_kind(path)?, NodeKind::Directory) {
                    return Err("mkdir refused: symlink".into());
                }
                fchmod_nofollow(path, mode, true)
            }
            FsEffect::ChownDir => chown_dir(path),
            FsEffect::WriteFile { mode } | FsEffect::InstallFile { mode } => {
                let bytes = payload.map(|(b, _)| b).unwrap_or(b"");
                write_nofollow(path, bytes, mode)
            }
            FsEffect::RemoveAbsent => Ok(()),
            FsEffect::RemoveFile => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("remove failed: {e}")),
            },
            FsEffect::RemoveDir => match node_kind(path)? {
                NodeKind::Missing => Ok(()),
                NodeKind::Symlink => Err("remove refused: symlink".into()),
                NodeKind::Directory => match std::fs::remove_dir_all(path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(format!("remove failed: {e}")),
                },
                _ => Err("remove refused".into()),
            },
        }
    }

    fn node_kind(path: &str) -> Result<NodeKind, String> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => Ok(NodeKind::Symlink),
            Ok(meta) if meta.is_dir() => Ok(NodeKind::Directory),
            Ok(meta) if meta.is_file() => Ok(NodeKind::File),
            Ok(_) => Ok(NodeKind::Other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(NodeKind::Missing),
            Err(e) => Err(format!("stat failed: {e}")),
        }
    }

    fn fchmod_nofollow(path: &str, mode: u32, directory: bool) -> Result<(), String> {
        let c = CString::new(path).map_err(|_| "mail helper: bad path".to_string())?;
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        if directory {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: `c` is a NUL-terminated path kept alive for the call.
        // The fd is closed on every return path below.
        let fd = unsafe { libc::open(c.as_ptr(), flags) };
        if fd < 0 {
            return Err(format!(
                "mail helper: open failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: `fd` is the descriptor just opened. `fchmod` does not follow.
        let rc = unsafe { libc::fchmod(fd, mode as libc::mode_t) };
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        if rc != 0 {
            return Err(format!("mail helper: chmod failed: {err}"));
        }
        Ok(())
    }

    fn write_nofollow(path: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        opts.mode(mode);
        let mut file = opts.open(path).map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                "write refused".to_string()
            } else {
                format!("write failed: {e}")
            }
        })?;
        file.write_all(bytes)
            .map_err(|_| "write failed".to_string())?;
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|_| "write failed".to_string())?;
        // SAFETY: the fd belongs to `file` and stays open for this call.
        let rc = unsafe { libc::fchown(file.as_raw_fd(), 0, 0) };
        if rc != 0 {
            return Err(format!("write failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn stalwart_ids() -> Result<(libc::uid_t, libc::gid_t), String> {
        let name = CString::new(STALWART_USER).map_err(|_| "chown refused".to_string())?;
        // SAFETY: `name` is NUL-terminated and kept alive. `getpwnam` returns
        // a pointer into a static buffer or null; uid and gid are copied out
        // before any other NSS call.
        let pw = unsafe { libc::getpwnam(name.as_ptr()) };
        if pw.is_null() {
            return Err("chown refused: user stalwart missing".into());
        }
        let uid = unsafe { (*pw).pw_uid };
        let gid = unsafe { (*pw).pw_gid };
        if uid == 0 {
            return Err("chown refused: stalwart uid is 0".into());
        }
        Ok((uid, gid))
    }

    fn lchown_path(path: &Path, uid: libc::uid_t, gid: libc::gid_t) -> Result<(), String> {
        let c =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| "chown refused".to_string())?;
        // SAFETY: `c` is NUL-terminated and kept alive. `lchown` does not follow.
        let rc = unsafe { libc::lchown(c.as_ptr(), uid, gid) };
        if rc != 0 {
            return Err(format!("chown failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn chown_tree(path: &Path, uid: libc::uid_t, gid: libc::gid_t) -> Result<(), String> {
        let meta = std::fs::symlink_metadata(path).map_err(|e| format!("chown failed: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err("chown refused: symlink".into());
        }
        lchown_path(path, uid, gid)?;
        if meta.is_dir() {
            for ent in std::fs::read_dir(path).map_err(|e| format!("chown failed: {e}"))? {
                let ent = ent.map_err(|e| format!("chown failed: {e}"))?;
                chown_tree(&ent.path(), uid, gid)?;
            }
        }
        Ok(())
    }

    fn chown_dir(path: &str) -> Result<(), String> {
        let (uid, gid) = stalwart_ids()?;
        chown_tree(Path::new(path), uid, gid)
    }

    fn ensure_user() -> Result<(), String> {
        let id = Command::new("/usr/bin/id")
            .args(["-u", STALWART_USER])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("ensure-user: id: {e}"))?;
        if id.success() {
            return Ok(());
        }
        let out = Command::new("/usr/sbin/useradd")
            .args([
                "--system",
                "--no-create-home",
                "--shell",
                "/usr/sbin/nologin",
                STALWART_USER,
            ])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("ensure-user: useradd: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("ensure-user: useradd failed: {}", err.trim()));
        }
        Ok(())
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let out = Command::new("/usr/bin/systemctl")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("systemctl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("systemctl {}: {}", args.join(" "), err.trim()));
        }
        Ok(())
    }
}

/// Root-side data operations for `k2 hostmail upgrade` (CAL14): the
/// snapshot, the restore, `usage`, and the atomic binary install. Every
/// function takes a [`DataLayout`] so tests run them over a temp tree;
/// the helper only ever passes [`DataLayout::fixed`].
///
/// Rules: never follow a symlink (a symlink where a dir is expected is a
/// refusal), refuse sockets/FIFOs/devices, keep owner + mode + times,
/// build into a temp sibling and rename only when complete, and refuse
/// unless Stalwart is stopped (systemd `inactive`/`failed` AND nobody
/// holds the RocksDB lock) — checked again after the copy.
#[cfg(unix)]
pub mod data_ops {
    use super::{DataLayout, HELPER_PROTOCOL};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    fn io_err(what: &str, path: &Path, e: std::io::Error) -> String {
        format!("{what} {}: {e}", path.display())
    }

    /// Stalwart must be stopped: systemd says `inactive` or `failed`, and
    /// no process holds the store's RocksDB lock (a Stalwart started by
    /// hand outside systemd still has it).
    pub fn require_stopped(unit_state: &str, layout: &DataLayout) -> Result<(), String> {
        match unit_state.trim() {
            "inactive" | "failed" => {}
            "" => {
                return Err(
                    "refused: could not read the stalwart unit state — Stalwart must be \
                     stopped first"
                        .into(),
                )
            }
            other => {
                return Err(format!(
                    "refused: Stalwart is not stopped (systemd: {other}) — the data dir is \
                     only copied while Stalwart is stopped"
                ))
            }
        }
        if store_lock_held(&layout.store_lock)? {
            return Err(format!(
                "refused: a process still holds the Stalwart store lock {} — Stalwart is \
                 running outside systemd; stop it first",
                layout.store_lock.display()
            ));
        }
        Ok(())
    }

    /// POSIX `F_GETLK` on RocksDB's LOCK file (RocksDB takes an `F_SETLK`
    /// write lock on it while the store is open). Missing file = no store
    /// open. Any other open failure fails closed.
    pub fn store_lock_held(path: &Path) -> Result<bool, String> {
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io_err("refused: cannot check the store lock", path, e)),
        };
        // SAFETY: zeroed `flock` is a valid all-fields-zero C struct; the
        // fields set below are the ones F_GETLK reads.
        let mut fl: libc::flock = unsafe { std::mem::zeroed() };
        fl.l_type = libc::F_WRLCK as _;
        fl.l_whence = libc::SEEK_SET as _;
        fl.l_start = 0;
        fl.l_len = 0;
        // SAFETY: `file` keeps the fd open for the call; `fl` is a valid
        // `flock` the kernel fills in.
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut fl) };
        if rc != 0 {
            return Err(format!(
                "refused: cannot check the store lock {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(i64::from(fl.l_type) != i64::from(libc::F_UNLCK))
    }

    /// Apparent bytes of every regular file under `path` (symlinks count 0,
    /// never followed). Missing = 0.
    pub fn tree_bytes(path: &Path) -> Result<u64, String> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(io_err("stat", path, e)),
        };
        let ft = meta.file_type();
        if ft.is_symlink() {
            return Ok(0);
        }
        if ft.is_file() {
            return Ok(meta.len());
        }
        if !ft.is_dir() {
            return Ok(0);
        }
        let mut total = 0u64;
        for ent in fs::read_dir(path).map_err(|e| io_err("read dir", path, e))? {
            let ent = ent.map_err(|e| io_err("read dir", path, e))?;
            total = total.saturating_add(tree_bytes(&ent.path())?);
        }
        Ok(total)
    }

    /// Bytes an unprivileged writer may still use on `path`'s filesystem.
    pub fn free_bytes(path: &Path) -> Result<u64, String> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "statvfs: bad path".to_string())?;
        // SAFETY: zeroed statvfs is a valid out-param; `c` is NUL-terminated.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(format!(
                "statvfs {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        #[allow(clippy::unnecessary_cast)]
        Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }

    fn parent_of(path: &Path) -> Result<&Path, String> {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| format!("no parent directory for {}", path.display()))
    }

    /// A real directory (not a symlink to one), or a refusal naming `what`.
    fn require_real_dir(path: &Path, what: &str) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => Err(format!(
                "refused: {what} {} is a symlink",
                path.display()
            )),
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(format!(
                "refused: {what} {} is not a directory",
                path.display()
            )),
            Err(e) => Err(io_err(&format!("refused: {what}"), path, e)),
        }
    }

    /// Remove a directory tree we own (a temp/aside sibling). Missing is
    /// fine; a symlink or a plain file at that path is a refusal (never
    /// delete through a link, never guess).
    pub fn remove_real_dir(path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("stat", path, e)),
            Ok(m) if m.file_type().is_symlink() => {
                Err(format!("refused: {} is a symlink", path.display()))
            }
            Ok(m) if m.is_dir() => {
                fs::remove_dir_all(path).map_err(|e| io_err("remove", path, e))
            }
            Ok(_) => Err(format!(
                "refused: {} is not a directory",
                path.display()
            )),
        }
    }

    fn fsync_dir(path: &Path) -> Result<(), String> {
        let dir = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| io_err("open dir", path, e))?;
        dir.sync_all().map_err(|e| io_err("fsync dir", path, e))
    }

    fn rename(from: &Path, to: &Path) -> Result<(), String> {
        fs::rename(from, to).map_err(|e| {
            format!("rename {} -> {}: {e}", from.display(), to.display())
        })
    }

    fn path_cstring(path: &Path) -> Result<std::ffi::CString, String> {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("bad path {}", path.display()))
    }

    fn lchown(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
        let c = path_cstring(path)?;
        // SAFETY: NUL-terminated path; lchown never follows a link.
        let rc = unsafe { libc::lchown(c.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
        if rc != 0 {
            return Err(format!(
                "chown {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn times_of(meta: &fs::Metadata) -> [libc::timespec; 2] {
        // SAFETY: zeroed timespec is valid; fields set right after.
        let mut ts: [libc::timespec; 2] = unsafe { std::mem::zeroed() };
        ts[0].tv_sec = meta.atime() as _;
        ts[0].tv_nsec = meta.atime_nsec() as _;
        ts[1].tv_sec = meta.mtime() as _;
        ts[1].tv_nsec = meta.mtime_nsec() as _;
        ts
    }

    fn set_times_nofollow(path: &Path, meta: &fs::Metadata) -> Result<(), String> {
        let c = path_cstring(path)?;
        let ts = times_of(meta);
        // SAFETY: NUL-terminated path, two valid timespecs.
        let rc = unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c.as_ptr(),
                ts.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(format!(
                "set times {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Copy `src` to `dst` (which must NOT exist), keeping uid/gid, mode
    /// (incl. setgid/sticky bits) and atime/mtime. Directories recurse;
    /// symlinks are re-created as symlinks (never followed); regular files
    /// are copied and fsync'd; anything else is a refusal. Hard links are
    /// copied as separate files; xattrs/ACLs are not copied.
    pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
        let meta = fs::symlink_metadata(src).map_err(|e| io_err("stat", src, e))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            let target = fs::read_link(src).map_err(|e| io_err("readlink", src, e))?;
            std::os::unix::fs::symlink(&target, dst).map_err(|e| io_err("symlink", dst, e))?;
            return lchown(dst, meta.uid(), meta.gid());
        }
        if ft.is_dir() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dst)
                .map_err(|e| io_err("mkdir", dst, e))?;
            for ent in fs::read_dir(src).map_err(|e| io_err("read dir", src, e))? {
                let ent = ent.map_err(|e| io_err("read dir", src, e))?;
                copy_tree(&ent.path(), &dst.join(ent.file_name()))?;
            }
            // Owner first (chown clears set-id bits), then mode, then times
            // (after the children, whose creation bumped mtime).
            lchown(dst, meta.uid(), meta.gid())?;
            fs::set_permissions(dst, fs::Permissions::from_mode(meta.mode() & 0o7777))
                .map_err(|e| io_err("chmod", dst, e))?;
            return set_times_nofollow(dst, &meta);
        }
        if ft.is_file() {
            return copy_file(src, dst, &meta);
        }
        Err(format!(
            "refused: {} is not a regular file, directory or symlink",
            src.display()
        ))
    }

    fn copy_file(src: &Path, dst: &Path, meta: &fs::Metadata) -> Result<(), String> {
        let mut from = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(src)
            .map_err(|e| io_err("open", src, e))?;
        let mut to = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dst)
            .map_err(|e| io_err("create", dst, e))?;
        let copied = std::io::copy(&mut from, &mut to).map_err(|e| io_err("copy", src, e))?;
        if copied != meta.len() {
            return Err(format!(
                "copy {}: {copied} of {} bytes (file changed during the copy)",
                src.display(),
                meta.len()
            ));
        }
        to.sync_all().map_err(|e| io_err("fsync", dst, e))?;
        let fd = to.as_raw_fd();
        // SAFETY: `fd` belongs to `to`, open for this whole block.
        if unsafe { libc::fchown(fd, meta.uid() as libc::uid_t, meta.gid() as libc::gid_t) } != 0
        {
            return Err(format!(
                "chown {}: {}",
                dst.display(),
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: as above.
        if unsafe { libc::fchmod(fd, (meta.mode() & 0o7777) as libc::mode_t) } != 0 {
            return Err(format!(
                "chmod {}: {}",
                dst.display(),
                std::io::Error::last_os_error()
            ));
        }
        let ts = times_of(meta);
        // SAFETY: as above; two valid timespecs.
        if unsafe { libc::futimens(fd, ts.as_ptr()) } != 0 {
            return Err(format!(
                "set times {}: {}",
                dst.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// `usage`: what a snapshot would copy, the snapshot already there,
    /// and free space where the snapshot lives. Read-only.
    pub fn usage_with(
        layout: &DataLayout,
        free_of: &dyn Fn(&Path) -> Result<u64, String>,
    ) -> Result<serde_json::Value, String> {
        let complete = snapshot_complete(layout);
        Ok(serde_json::json!({
            "dataDir": layout.data.display().to_string(),
            "configDir": layout.config.display().to_string(),
            "snapshotDir": layout.snapshot.display().to_string(),
            "dataBytes": tree_bytes(&layout.data)?,
            "configBytes": tree_bytes(&layout.config)?,
            "snapshotBytes": tree_bytes(&layout.snapshot)?,
            "snapshotComplete": complete,
            "freeBytes": free_of(parent_of(&layout.snapshot)?)?,
        }))
    }

    /// A snapshot is complete only with its marker AND both copies.
    pub fn snapshot_complete(layout: &DataLayout) -> bool {
        let real_dir = |p: &Path| {
            fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        };
        real_dir(&layout.snapshot)
            && fs::symlink_metadata(layout.snapshot_marker())
                .is_ok_and(|m| m.file_type().is_file())
            && real_dir(&layout.snapshot_data())
            && real_dir(&layout.snapshot_config())
    }

    /// `snapshot-data`: copy data + config into `snapshot.tmp`, write the
    /// marker last, rename onto `snapshot`. The previous snapshot (if any)
    /// is removed first — counted as free space — so one snapshot exists
    /// at a time. On any failure the temp copy is removed and the live
    /// dirs are untouched (this verb only ever reads them).
    pub fn snapshot_with(
        layout: &DataLayout,
        unit_state: &dyn Fn() -> String,
        free_of: &dyn Fn(&Path) -> Result<u64, String>,
        margin: u64,
    ) -> Result<serde_json::Value, String> {
        require_stopped(&unit_state(), layout)?;
        require_real_dir(&layout.data, "Stalwart data dir")?;
        require_real_dir(&layout.config, "Stalwart config dir")?;
        let tmp = layout.snapshot_tmp();
        remove_real_dir(&tmp)?;

        let data_bytes = tree_bytes(&layout.data)?;
        let config_bytes = tree_bytes(&layout.config)?;
        let old_snapshot = tree_bytes(&layout.snapshot)?;
        let parent = parent_of(&layout.snapshot)?;
        let free = free_of(parent)?;
        let need = data_bytes
            .saturating_add(config_bytes)
            .saturating_add(margin);
        let have = free.saturating_add(old_snapshot);
        if have < need {
            return Err(format!(
                "refused: not enough free space for the snapshot on {} — need {need} bytes \
                 (data {data_bytes} + config {config_bytes} + {margin} margin), have {have} \
                 (free {free} + old snapshot {old_snapshot})",
                parent.display()
            ));
        }
        remove_real_dir(&layout.snapshot)?;

        let result = (|| -> Result<serde_json::Value, String> {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&tmp)
                .map_err(|e| io_err("mkdir", &tmp, e))?;
            copy_tree(&layout.data, &tmp.join("data"))?;
            copy_tree(&layout.config, &tmp.join("config"))?;
            // Stalwart must not have started while we copied (a TLS-reload
            // restart, a human): that copy would not be a consistent store.
            require_stopped(&unit_state(), layout)
                .map_err(|e| format!("Stalwart started during the snapshot copy — {e}"))?;
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let marker = serde_json::json!({
                "createdAt": created_at,
                "dataBytes": data_bytes,
                "configBytes": config_bytes,
                "helperProtocol": HELPER_PROTOCOL,
                "dataDir": layout.data.display().to_string(),
                "configDir": layout.config.display().to_string(),
            });
            let marker_path = tmp.join(super::super::supervisor::STALWART_SNAPSHOT_MARKER);
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&marker_path)
                .map_err(|e| io_err("create", &marker_path, e))?;
            f.write_all(marker.to_string().as_bytes())
                .map_err(|e| io_err("write", &marker_path, e))?;
            f.sync_all().map_err(|e| io_err("fsync", &marker_path, e))?;
            fsync_dir(&tmp)?;
            rename(&tmp, &layout.snapshot)?;
            fsync_dir(parent)?;
            let mut out = marker;
            out["snapshotDir"] = serde_json::json!(layout.snapshot.display().to_string());
            Ok(out)
        })();
        if result.is_err() {
            let _ = remove_real_dir(&tmp);
        }
        result
    }

    /// `restore-data`: replace the live data + config dirs with the
    /// complete snapshot. The snapshot itself is only read (it survives,
    /// for a second attempt). With room for a full copy beside the live
    /// data, the copy is made first and swapped in; without room the
    /// (failed-upgrade) live data dir is moved aside and deleted first —
    /// the snapshot is the copy that matters.
    pub fn restore_with(
        layout: &DataLayout,
        unit_state: &dyn Fn() -> String,
        free_of: &dyn Fn(&Path) -> Result<u64, String>,
        margin: u64,
    ) -> Result<serde_json::Value, String> {
        require_stopped(&unit_state(), layout)?;
        if !snapshot_complete(layout) {
            return Err(format!(
                "refused: no complete snapshot at {} (marker {} missing or a copy missing) — \
                 nothing was restored",
                layout.snapshot.display(),
                layout.snapshot_marker().display()
            ));
        }
        let data_tmp = layout.data_restore_tmp();
        let data_aside = layout.data_aside();
        let config_tmp = layout.config_restore_tmp();
        let config_aside = layout.config_aside();
        for p in [&data_tmp, &data_aside, &config_tmp, &config_aside] {
            remove_real_dir(p)?;
        }
        // A live dir that is a symlink is never moved or deleted.
        for (p, what) in [
            (&layout.data, "Stalwart data dir"),
            (&layout.config, "Stalwart config dir"),
        ] {
            if fs::symlink_metadata(p).is_ok() {
                require_real_dir(p, what)?;
            }
        }

        // Config first (small): staged copy, nothing live touched yet.
        if let Err(e) = copy_tree(&layout.snapshot_config(), &config_tmp) {
            let _ = remove_real_dir(&config_tmp);
            return Err(format!("restore: nothing changed — {e}"));
        }

        let mut leftovers: Vec<String> = Vec::new();
        let snap_data_bytes = tree_bytes(&layout.snapshot_data())?;
        let data_parent = parent_of(&layout.data)?;
        let free = free_of(data_parent)?;
        let copy_first = free >= snap_data_bytes.saturating_add(margin);
        let lost = |e: String| {
            format!(
                "restore FAILED after the live data dir was removed — {e}. The snapshot at {} \
                 is intact: copy {} to {} by hand (cp -a) with Stalwart stopped",
                layout.snapshot.display(),
                layout.snapshot_data().display(),
                layout.data.display()
            )
        };
        if copy_first {
            if let Err(e) = copy_tree(&layout.snapshot_data(), &data_tmp) {
                let _ = remove_real_dir(&data_tmp);
                let _ = remove_real_dir(&config_tmp);
                return Err(format!("restore: nothing changed — {e}"));
            }
            if fs::symlink_metadata(&layout.data).is_ok() {
                rename(&layout.data, &data_aside).map_err(|e| {
                    let _ = remove_real_dir(&data_tmp);
                    let _ = remove_real_dir(&config_tmp);
                    format!("restore: nothing changed — {e}")
                })?;
            }
            rename(&data_tmp, &layout.data).map_err(lost)?;
            if let Err(e) = remove_real_dir(&data_aside) {
                leftovers.push(format!("{} ({e})", data_aside.display()));
            }
        } else {
            if fs::symlink_metadata(&layout.data).is_ok() {
                rename(&layout.data, &data_aside).map_err(|e| {
                    let _ = remove_real_dir(&config_tmp);
                    format!("restore: nothing changed — {e}")
                })?;
            }
            remove_real_dir(&data_aside).map_err(lost)?;
            if let Err(e) = copy_tree(&layout.snapshot_data(), &data_tmp) {
                let _ = remove_real_dir(&data_tmp);
                return Err(lost(e));
            }
            rename(&data_tmp, &layout.data).map_err(lost)?;
        }
        fsync_dir(data_parent)?;

        if fs::symlink_metadata(&layout.config).is_ok() {
            rename(&layout.config, &config_aside)?;
        }
        rename(&config_tmp, &layout.config)?;
        if let Err(e) = remove_real_dir(&config_aside) {
            leftovers.push(format!("{} ({e})", config_aside.display()));
        }
        fsync_dir(parent_of(&layout.config)?)?;
        Ok(serde_json::json!({
            "restoredFrom": layout.snapshot.display().to_string(),
            "dataBytes": snap_data_bytes,
            "copyFirst": copy_first,
            // Old (failed-upgrade) dirs that could not be deleted after the
            // swap. The restore itself succeeded; remove these by hand.
            "leftovers": leftovers,
        }))
    }

    /// Write `bytes` to `staging` (fresh, never through a link), set
    /// owner + mode, fsync, then rename onto `dest`. A crash leaves the old
    /// `dest` or the new one, never a half-written binary. `owner` is
    /// `Some((0, 0))` in the helper; tests pass `None`.
    pub fn install_file_atomic(
        staging: &Path,
        dest: &Path,
        bytes: &[u8],
        mode: u32,
        owner: Option<(u32, u32)>,
    ) -> Result<(), String> {
        match fs::symlink_metadata(staging) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("install-bin: stat", staging, e)),
            Ok(m) if m.file_type().is_file() => {
                fs::remove_file(staging).map_err(|e| io_err("install-bin: remove", staging, e))?
            }
            Ok(_) => {
                return Err(format!(
                    "install-bin refused: staging path {} is not a regular file",
                    staging.display()
                ))
            }
        }
        let result = (|| -> Result<(), String> {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(staging)
                .map_err(|e| io_err("install-bin: create", staging, e))?;
            f.write_all(bytes)
                .map_err(|e| io_err("install-bin: write", staging, e))?;
            if let Some((uid, gid)) = owner {
                // SAFETY: the fd belongs to `f`, open for this call.
                if unsafe { libc::fchown(f.as_raw_fd(), uid as libc::uid_t, gid as libc::gid_t) }
                    != 0
                {
                    return Err(format!(
                        "install-bin: chown {}: {}",
                        staging.display(),
                        std::io::Error::last_os_error()
                    ));
                }
            }
            f.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|e| io_err("install-bin: chmod", staging, e))?;
            f.sync_all()
                .map_err(|e| io_err("install-bin: fsync", staging, e))?;
            rename(staging, dest)?;
            fsync_dir(parent_of(dest)?)
        })();
        if result.is_err() {
            let _ = fs::remove_file(staging);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_accept(args: &[&str]) {
        let cmd = parse_argv(args).unwrap_or_else(|e| panic!("accept {args:?}: {e}"));
        let again = parse_argv(&cmd.argv()).unwrap_or_else(|e| panic!("round-trip {args:?}: {e}"));
        assert_eq!(cmd, again, "{args:?}");
    }

    fn assert_reject(args: &[&str]) {
        assert!(parse_argv(args).is_err(), "expected reject, got {args:?}");
    }

    #[test]
    fn allowlist_accepts_section2_vectors() {
        assert_eq!(STALWART_BIN, "/usr/local/bin/stalwart");
        assert_eq!(STALWART_CONFIG_DIR, "/etc/stalwart");
        assert_eq!(STALWART_DATA_DIR, "/var/lib/stalwart");
        assert_eq!(STALWART_LOG_DIR, "/var/log/stalwart");
        assert_eq!(STALWART_USER, "stalwart");
        assert_eq!(STALWART_UNIT, "stalwart");
        assert_eq!(STALWART_UNIT_PATH, "/etc/systemd/system/stalwart.service");
        assert_eq!(
            STALWART_DROPIN_DIR,
            "/etc/systemd/system/stalwart.service.d"
        );
        assert_eq!(
            STALWART_DROPIN_PATH,
            "/etc/systemd/system/stalwart.service.d/k2-hardening.conf"
        );

        let vectors: &[&[&str]] = &[
            &["ensure-user"],
            &["mkdir", "/etc/stalwart"],
            &["mkdir", "/var/lib/stalwart"],
            &["mkdir", "/var/log/stalwart"],
            &["mkdir", "/etc/systemd/system/stalwart.service.d"],
            &["chown", "/etc/stalwart"],
            &["chown", "/var/lib/stalwart"],
            &["chown", "/var/log/stalwart"],
            &["write", "/etc/systemd/system/stalwart.service"],
            &[
                "write",
                "/etc/systemd/system/stalwart.service.d/k2-hardening.conf",
            ],
            &["install-bin"],
            &["systemctl", "daemon-reload"],
            &["systemctl", "enable", "--now", "stalwart"],
            &["systemctl", "restart", "stalwart"],
            &["systemctl", "disable", "--now", "stalwart"],
            &["remove", "/etc/systemd/system/stalwart.service"],
            &["remove", "/etc/systemd/system/stalwart.service.d"],
            &["remove", "/usr/local/bin/stalwart"],
            &["remove", "/var/lib/stalwart"],
            &["remove", "/var/log/stalwart"],
            &["remove", "/etc/stalwart"],
            // Protocol 2 (calendars S1 upgrade): no path arguments at all.
            &["version"],
            &["usage"],
            &["snapshot-data"],
            &["restore-data"],
            &["systemctl", "stop", "stalwart"],
        ];
        for v in vectors {
            assert_accept(v);
        }
        assert_eq!(parse_argv(&["version"]), Ok(HelperCommand::Version));
        assert_eq!(parse_argv(&["usage"]), Ok(HelperCommand::Usage));
        assert_eq!(parse_argv(&["snapshot-data"]), Ok(HelperCommand::SnapshotData));
        assert_eq!(parse_argv(&["restore-data"]), Ok(HelperCommand::RestoreData));
        for cmd in [
            HelperCommand::Version,
            HelperCommand::Usage,
            HelperCommand::SnapshotData,
            HelperCommand::RestoreData,
        ] {
            assert!(!cmd.reads_stdin(), "{cmd:?} takes no stdin");
            assert!(decide_effect(cmd, NodeKind::Missing).is_err(), "{cmd:?}");
        }
        // The data verbs act on compiled-in paths only.
        let fixed = DataLayout::fixed();
        assert_eq!(fixed.data, Path::new(STALWART_DATA_DIR));
        assert_eq!(fixed.config, Path::new(STALWART_CONFIG_DIR));
        assert_eq!(fixed.snapshot, Path::new("/var/lib/stalwart.k2-snap"));
        assert_eq!(fixed.store_lock, Path::new("/var/lib/stalwart/data/LOCK"));
        assert_eq!(fixed.snapshot_tmp(), Path::new("/var/lib/stalwart.k2-snap.tmp"));
        assert_eq!(
            fixed.snapshot_marker(),
            Path::new("/var/lib/stalwart.k2-snap/K2-SNAPSHOT.json")
        );
        assert_eq!(
            fixed.data_restore_tmp(),
            Path::new("/var/lib/stalwart.k2-restore.tmp")
        );
        assert_eq!(fixed.data_aside(), Path::new("/var/lib/stalwart.k2-failed"));
        assert_eq!(
            fixed.config_restore_tmp(),
            Path::new("/etc/stalwart.k2-restore.tmp")
        );
        assert_eq!(fixed.config_aside(), Path::new("/etc/stalwart.k2-failed"));
    }

    /// Protocol 2 path fixing: the data verbs take no argument, so no
    /// caller can point a root copy / restore / delete anywhere.
    #[test]
    fn data_verbs_refuse_any_argument() {
        for v in [
            &["snapshot-data", "/tmp/x"][..],
            &["snapshot-data", "/var/lib/stalwart"],
            &["restore-data", "/var/lib/stalwart.k2-snap"],
            &["restore-data", "/etc"],
            &["usage", "/"],
            &["version", "--json"],
            &["snapshot-data", "--to", "/root"],
            &["systemctl", "stop", "k2-daemon"],
            &["systemctl", "stop", "stalwart", "--force"],
            &["systemctl", "start", "stalwart"],
            &["systemctl", "kill", "stalwart"],
        ] {
            assert_reject(v);
        }
    }

    #[test]
    fn allowlist_rejects_attacks() {
        let rejects: &[&[&str]] = &[
            &[],
            &["ensure-user", "root"],
            &["ensure-user", "stalwart"],
            &["ensure-user", "stalwart", "--shell", "/bin/bash"],
            &["mkdir", "/etc/stalwart-evil"],
            &["mkdir", "/etc/stalwart/"],
            &["mkdir", "/etc/stalwart/../../etc"],
            &["mkdir", "/tmp/k2-mail-extract-1"],
            &["mkdir", "/"],
            &["mkdir", "/etc/stalwart", "/var/lib/stalwart"],
            &["chown", "/"],
            &["chown", "/etc/systemd/system/stalwart.service.d"],
            &["chown", "/var/lib/stalwart/data"],
            &["chown", "/etc/stalwart", "stalwart"],
            &["remove", "/etc/stalwart/config.json"],
            &["remove", "/var/lib/stalwart/data"],
            &["remove", "/etc/sudoers.d/k2-mail-helper"],
            &["write", "/etc/sudoers.d/k2-mail-helper"],
            &["write", "/etc/stalwart/config.json"],
            &["install-bin", "/tmp/x"],
            &["systemctl", "restart", "k2-daemon"],
            &["systemctl", "restart", "stalwart.service"],
            &["systemctl", "enable", "stalwart"],
            &["systemctl", "is-active", "stalwart"],
            &["systemctl", "daemon-reexec"],
            &["systemctl", "edit", "stalwart"],
            &["systemctl", "enable", "--now", "stalwart", "--now"],
            &["systemctl", "isolate", "multi-user.target"],
            &["useradd", "stalwart"],
            &["rm", "/etc/stalwart"],
            &["bash", "-c", "id"],
            &["mkdir"],
            &["systemctl"],
        ];
        for v in rejects {
            assert_reject(v);
        }
        assert_reject(&["mkdir", "/etc/stalwart;rm -rf /"]);
        assert_reject(&["systemctl", "restart stalwart"]);
        assert_reject(&["systemctl", "restart", "stalwart|sh"]);
        assert_reject(&["mkdir", "/etc/stalwart\n"]);
        assert_reject(&[""]);
        assert_reject(&["ensure-user\n"]);
    }

    #[test]
    fn unit_and_dropin_compare_hides_canary() {
        let none = systemd_unit(None);
        assert_eq!(
            write_bytes_accepted(STALWART_UNIT_PATH, none.as_bytes()).unwrap(),
            0o600
        );
        let pw = "ab".repeat(32);
        assert_eq!(pw.len(), 64);
        let with_pw = systemd_unit(Some(&pw));
        assert_eq!(
            write_bytes_accepted(STALWART_UNIT_PATH, with_pw.as_bytes()).unwrap(),
            0o600
        );
        let zeros = "0".repeat(64);
        assert_eq!(
            write_bytes_accepted(STALWART_UNIT_PATH, systemd_unit(Some(&zeros)).as_bytes())
                .unwrap(),
            0o600
        );
        assert_eq!(
            write_bytes_accepted(STALWART_DROPIN_PATH, hardening_dropin().as_bytes()).unwrap(),
            0o644
        );

        let reject = |bytes: &[u8], path: &str| {
            let err = write_bytes_accepted(path, bytes).expect_err("must refuse");
            assert_eq!(err, "write refused");
            assert!(!err.contains("CANARY"), "{err}");
            assert!(!err.contains("root-CANARY"), "{err}");
            assert!(!bytes.is_empty() || err == "write refused");
        };

        reject(hardening_dropin().as_bytes(), STALWART_UNIT_PATH);
        reject(none.as_bytes(), STALWART_DROPIN_PATH);
        reject(with_pw.as_bytes(), STALWART_DROPIN_PATH);

        let user_root = none.replace("User=stalwart", "User=root-CANARY");
        assert!(user_root.contains("User=root-CANARY"));
        reject(user_root.as_bytes(), STALWART_UNIT_PATH);
        reject(b"User=root-CANARY\n", STALWART_UNIT_PATH);

        let mut second_exec = none.clone();
        second_exec.push_str("ExecStart=/bin/sh\n");
        reject(second_exec.as_bytes(), STALWART_UNIT_PATH);

        reject(
            systemd_unit(Some(&"ab".repeat(31))).as_bytes(),
            STALWART_UNIT_PATH,
        );
        reject(
            systemd_unit(Some(&"AB".repeat(32))).as_bytes(),
            STALWART_UNIT_PATH,
        );
        reject(
            systemd_unit(Some(&format!("{}\n{}", "ab".repeat(16), "cd".repeat(16)))).as_bytes(),
            STALWART_UNIT_PATH,
        );

        let mut extra = hardening_dropin().to_string();
        extra.push_str("User=root-CANARY\n");
        reject(extra.as_bytes(), STALWART_DROPIN_PATH);
        reject(b"", STALWART_UNIT_PATH);
        reject(b"", STALWART_DROPIN_PATH);
    }

    /// CAL14: install-bin accepts exactly {pin, previous} — the upgrade
    /// target and the rollback binary — and nothing else.
    #[test]
    fn install_bin_accepts_only_the_pin_or_the_previous_tarball() {
        use super::super::supervisor::StalwartArtifact;
        use sha2::{Digest, Sha256};
        let hex = |b: &[u8]| -> &'static str {
            let h: String = Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect();
            Box::leak(h.into_boxed_str())
        };
        let pin = StalwartArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            sha256: hex(b"pin-tarball"),
        };
        let prev = StalwartArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            sha256: hex(b"previous-tarball"),
        };
        assert_eq!(
            match_archive(b"pin-tarball", &pin, &prev),
            Ok(STALWART_PINNED_VERSION)
        );
        assert_eq!(
            match_archive(b"previous-tarball", &pin, &prev),
            Ok(STALWART_PREVIOUS_VERSION)
        );
        let err = match_archive(b"some-other-tarball-CANARY", &pin, &prev).expect_err("other");
        assert_eq!(err, "install-bin: tarball sha256 mismatch");
        // The real tables: both arches, two distinct real checksums each.
        for arch in ["x86_64", "aarch64"] {
            let p = artifact_for_arch(arch).expect("pin");
            let q = previous_artifact_for_arch(arch).expect("previous");
            assert_ne!(p.sha256, q.sha256, "{arch}");
            assert!(install_archive_allowed_for(b"bogus", arch).is_err());
        }
        assert!(install_archive_allowed_for(b"bogus", "riscv64")
            .expect_err("arch")
            .contains("riscv64"));
    }

    #[test]
    fn version_json_names_the_protocol_and_both_checksums() {
        let v = version_json_for("x86_64");
        assert_eq!(v["helper"], "k2-mail-helper");
        assert_eq!(v["protocol"], HELPER_PROTOCOL);
        assert!(HELPER_PROTOCOL >= UPGRADE_MIN_PROTOCOL);
        assert_eq!(v["stalwartPin"], STALWART_PINNED_VERSION);
        assert_eq!(v["stalwartPrevious"], STALWART_PREVIOUS_VERSION);
        assert_eq!(
            v["stalwartPinSha256"],
            artifact_for_arch("x86_64").unwrap().sha256
        );
        assert_eq!(
            v["stalwartPreviousSha256"],
            previous_artifact_for_arch("x86_64").unwrap().sha256
        );
        assert_eq!(v["k2Version"], env!("CARGO_PKG_VERSION"));
        assert!(version_json_for("riscv64")["stalwartPinSha256"].is_null());
    }

    #[test]
    fn install_bin_rejects_bytes_that_are_not_the_tarball_pin() {
        let bogus = b"not-the-stalwart-tarball-CANARY";
        let err = install_archive_allowed(bogus).expect_err("wrong tarball bytes must be rejected");
        assert!(
            err.contains("sha256 mismatch") || err.contains("unsupported CPU architecture"),
            "arch {}: {err}",
            std::env::consts::ARCH
        );
        assert!(!err.contains("CANARY"), "{err}");
        assert!(!err.contains("not-the-stalwart"), "{err}");
        assert!(extract_request_allowed("stalwart", STALWART_BIN, 0o755).is_ok());
        assert!(extract_request_allowed("stalwart", STALWART_BIN, 0o4755).is_err());
        assert!(extract_request_allowed("stalwart", "/tmp/stalwart", 0o755).is_err());
        assert!(extract_request_allowed("other", STALWART_BIN, 0o755).is_err());
    }

    #[test]
    fn extract_member_is_exact_stalwart_file() {
        fn pack(files: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
            let mut raw = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut raw);
                for (name, data, kind) in files {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(*kind);
                    header.set_size(data.len() as u64);
                    header.set_mode(0o755);
                    if *kind == tar::EntryType::Symlink {
                        header.set_link_name("evil").unwrap();
                        header.set_size(0);
                    }
                    builder
                        .append_data(&mut header, *name, *data)
                        .expect("append");
                }
                builder.finish().unwrap();
            }
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut gz, &raw).unwrap();
            gz.finish().unwrap()
        }

        let archive = pack(&[
            ("README", b"nope", tar::EntryType::Regular),
            ("stalwart", b"member-bytes", tar::EntryType::Regular),
        ]);
        assert_eq!(
            extract_stalwart_member(&archive).unwrap(),
            b"member-bytes".to_vec()
        );
        let nested = pack(&[("bin/stalwart", b"nope", tar::EntryType::Regular)]);
        assert!(extract_stalwart_member(&nested).is_err());
        // `./stalwart` is the same member after the tar reader normalizes it.
        let dotted = pack(&[("./stalwart", b"dot-member", tar::EntryType::Regular)]);
        assert_eq!(
            extract_stalwart_member(&dotted).unwrap(),
            b"dot-member".to_vec()
        );
        // The safe builder refuses `..`. Plant the name in the header bytes.
        let parent = {
            let mut raw = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut raw);
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(6);
                header.set_mode(0o644);
                let name = b"../stalwart";
                header.as_mut_bytes()[..name.len()].copy_from_slice(name);
                header.set_cksum();
                builder.append(&header, &b"escape"[..]).unwrap();
                builder.finish().unwrap();
            }
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut gz, &raw).unwrap();
            gz.finish().unwrap()
        };
        let err = extract_stalwart_member(&parent).expect_err(".. member");
        assert!(!err.contains("escape"), "{err}");
        let link = pack(&[("stalwart", b"", tar::EntryType::Symlink)]);
        let err = extract_stalwart_member(&link).expect_err("symlink member");
        assert!(!err.contains("evil"), "{err}");
    }

    #[test]
    fn mode_and_symlink_policy_is_data() {
        let cmds = [
            HelperCommand::Mkdir(STALWART_CONFIG_DIR),
            HelperCommand::Mkdir(STALWART_DATA_DIR),
            HelperCommand::Mkdir(STALWART_LOG_DIR),
            HelperCommand::Mkdir(STALWART_DROPIN_DIR),
            HelperCommand::Chown(STALWART_CONFIG_DIR),
            HelperCommand::Chown(STALWART_DATA_DIR),
            HelperCommand::Chown(STALWART_LOG_DIR),
            HelperCommand::Write(STALWART_UNIT_PATH),
            HelperCommand::Write(STALWART_DROPIN_PATH),
            HelperCommand::InstallBin,
            HelperCommand::Remove(STALWART_UNIT_PATH),
            HelperCommand::Remove(STALWART_DROPIN_DIR),
            HelperCommand::Remove(STALWART_BIN),
            HelperCommand::Remove(STALWART_DATA_DIR),
            HelperCommand::Remove(STALWART_LOG_DIR),
            HelperCommand::Remove(STALWART_CONFIG_DIR),
        ];
        for cmd in cmds {
            assert!(
                decide_effect(cmd, NodeKind::Symlink).is_err(),
                "symlink must refuse {cmd:?}"
            );
            if let Some(mode) = forced_mode(cmd) {
                assert_ne!(mode, 0o4755);
                assert!(matches!(mode, 0o600 | 0o644 | 0o755), "{mode:o}");
            }
        }
        assert_eq!(
            decide_effect(HelperCommand::Mkdir(STALWART_CONFIG_DIR), NodeKind::Missing).unwrap(),
            FsEffect::CreateDir { mode: 0o755 }
        );
        assert_eq!(
            decide_effect(
                HelperCommand::Mkdir(STALWART_CONFIG_DIR),
                NodeKind::Directory
            )
            .unwrap(),
            FsEffect::DirAlready { mode: 0o755 }
        );
        assert!(decide_effect(HelperCommand::Chown(STALWART_DATA_DIR), NodeKind::Missing).is_err());
        assert_eq!(
            decide_effect(HelperCommand::Chown(STALWART_DATA_DIR), NodeKind::Directory).unwrap(),
            FsEffect::ChownDir
        );
        assert_eq!(
            decide_effect(HelperCommand::Remove(STALWART_BIN), NodeKind::Missing).unwrap(),
            FsEffect::RemoveAbsent
        );
        assert_eq!(
            decide_effect(HelperCommand::Remove(STALWART_DATA_DIR), NodeKind::Missing).unwrap(),
            FsEffect::RemoveAbsent
        );
        assert_eq!(
            forced_mode(HelperCommand::Write(STALWART_UNIT_PATH)),
            Some(0o600)
        );
        assert_eq!(
            forced_mode(HelperCommand::Write(STALWART_DROPIN_PATH)),
            Some(0o644)
        );
        assert_eq!(forced_mode(HelperCommand::InstallBin), Some(0o755));
        assert_eq!(
            forced_mode(HelperCommand::Mkdir(STALWART_LOG_DIR)),
            Some(0o755)
        );
        assert_eq!(
            decide_effect(HelperCommand::InstallBin, NodeKind::Missing).unwrap(),
            FsEffect::InstallFile { mode: 0o755 }
        );
        assert_eq!(
            decide_effect(HelperCommand::Write(STALWART_UNIT_PATH), NodeKind::File).unwrap(),
            FsEffect::WriteFile { mode: 0o600 }
        );
    }

    #[test]
    fn missing_helper_mapper_and_unmarked_step() {
        let fix = install_command();
        let missing = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file User=root-CANARY",
        );
        let msg = map_helper_failure_with(Some(&missing), "", true).unwrap();
        assert!(msg.starts_with("mail helper not installed"), "{msg}");
        assert!(msg.contains(&fix), "{msg}");
        assert!(!msg.contains("CANARY"));

        // Password / allow refusal with the file absent = not installed.
        for stderr in [
            "sudo: a password is required",
            "k2 is not allowed to execute /usr/local/libexec/k2-mail-helper as root on this host",
            "sudo: a password is required\nUser=root-CANARY",
        ] {
            let msg = map_helper_failure_with(None, stderr, false).unwrap();
            assert!(
                msg.starts_with("mail helper not installed"),
                "{stderr} -> {msg}"
            );
            assert!(msg.contains(&fix), "{stderr} -> {msg}");
            assert!(!msg.contains("CANARY"), "{stderr} -> {msg}");
        }
        // Same refusal with the file present = sudoers does not allow it.
        for stderr in [
            "sudo: a password is required",
            "k2 is not allowed to execute /usr/local/libexec/k2-mail-helper as root on this host",
        ] {
            assert_eq!(
                classify_helper_failure(None, stderr, true),
                Some(HelperState::NotAllowed),
                "{stderr}"
            );
            let msg = map_helper_failure_with(None, stderr, true).unwrap();
            assert!(
                msg.starts_with("mail helper not allowed by sudoers"),
                "{msg}"
            );
            assert!(msg.contains(&fix), "{msg}");
        }

        let gone = format!("sudo: {HELPER_PATH}: command not found");
        assert_eq!(
            classify_helper_failure(None, &gone, true),
            Some(HelperState::Missing)
        );
        assert!(map_helper_failure_with(None, &gone, true)
            .unwrap()
            .starts_with("mail helper not installed"));
        assert!(
            map_helper_failure_with(None, "ensure-user: useradd failed: exit 1", true).is_none()
        );
        assert!(
            classify_helper_failure(None, "systemctl restart stalwart: failed", false).is_none()
        );

        let mut marked = false;
        let err: Result<(), String> = Err(msg);
        mark_step_on_ok(&err, &mut marked);
        assert!(!marked, "helper error must leave the step unmarked");
        mark_step_on_ok(&Ok(()), &mut marked);
        assert!(marked);
    }

    /// The installer the hint points at must agree with the daemon: same
    /// updater pubkey as the self-updater, same helper path, the exact
    /// two sudoers lines (visudo-checked), and the per-arch asset name
    /// daemon-binaries.yml uploads.
    #[test]
    fn installer_script_matches_daemon_constants() {
        let script = include_str!("../../../../scripts/install-mail-helper.sh");
        assert!(
            script.contains(&format!(
                "K2_HELPER_PUBKEY=\"{}\"",
                crate::update_routes::UPDATER_PUBKEY_B64
            )),
            "installer pubkey must equal update_routes::UPDATER_PUBKEY_B64"
        );
        assert!(script.contains(&format!("HELPER_PATH=\"{HELPER_PATH}\"")));
        assert!(
            script.contains(r#"printf '%s ALL=(root) NOPASSWD: %s\nDefaults:%s !requiretty\n'"#)
        );
        assert!(script.contains("RUN_USER=\"${K2_RUN_USER:-k2}\""));
        assert!(script.contains("visudo -cf \"$CANDIDATE\""));
        assert!(script.contains("ASSET=\"k2-mail-helper-linux-${ARCH}\""));
        assert!(script.contains(&format!(
            "RELEASE_BASE=\"${{K2_RELEASE_BASE:-{RELEASE_DOWNLOAD_BASE}}}\""
        )));
        let workflow = include_str!("../../../../.github/workflows/daemon-binaries.yml");
        for asset in [
            "helper_asset: k2-mail-helper-linux-x86_64",
            "helper_asset: k2-mail-helper-linux-aarch64",
            "--bin k2-mail-helper",
            "gh release upload \"$TAG\" scripts/install-mail-helper.sh --clobber",
        ] {
            assert!(
                workflow.contains(asset),
                "daemon-binaries.yml lacks {asset}"
            );
        }
    }

    #[test]
    fn install_hint_names_the_release_installer_for_this_version() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(
            install_command(),
            format!(
                "curl -fsSL https://github.com/Alakazam-211/K2/releases/download/v{v}/install-mail-helper.sh | sudo bash -s -- --version {v}"
            )
        );
        assert_eq!(unavailable_message(HelperState::Installed), None);
        let missing = unavailable_message(HelperState::Missing).unwrap();
        assert_eq!(
            missing,
            format!(
                "mail helper not installed (/usr/local/libexec/k2-mail-helper is missing) — run as root: {}",
                install_command()
            )
        );
        let denied = unavailable_message(HelperState::NotAllowed).unwrap();
        assert!(
            denied.starts_with("mail helper not allowed by sudoers"),
            "{denied}"
        );
        assert!(denied.ends_with(&install_command()), "{denied}");
        assert_eq!(HelperState::Installed.as_str(), "installed");
        assert_eq!(HelperState::Missing.as_str(), "missing");
        assert_eq!(HelperState::NotAllowed.as_str(), "not allowed by sudoers");
    }

    #[test]
    fn recorded_lines_map_onto_helper_vectors() {
        assert!(recorded_line_allowlisted("mkdir /etc/stalwart").is_ok());
        assert!(recorded_line_allowlisted("chown stalwart /var/lib/stalwart").is_ok());
        assert!(recorded_line_allowlisted("chown root /etc/stalwart").is_err());
        let unit = systemd_unit(None);
        let line = format!(
            "write {STALWART_UNIT_PATH} ({} bytes, mode 600)",
            unit.len()
        );
        assert!(recorded_line_allowlisted(&line).is_ok());
        let bad_mode = format!(
            "write {STALWART_UNIT_PATH} ({} bytes, mode 4755)",
            unit.len()
        );
        assert!(recorded_line_allowlisted(&bad_mode).is_err());
        let dropin = format!(
            "write {STALWART_DROPIN_PATH} ({} bytes, mode 644)",
            hardening_dropin().len()
        );
        assert!(recorded_line_allowlisted(&dropin).is_ok());
        assert!(recorded_line_allowlisted("rm /etc/stalwart").is_ok());
        assert!(recorded_line_allowlisted("rm /usr/local/bin/stalwart").is_ok());
        assert!(recorded_line_allowlisted("rm /etc/stalwart/config.json").is_err());
        assert!(recorded_line_allowlisted("systemctl daemon-reload").is_ok());
        assert!(recorded_line_allowlisted("systemctl enable --now stalwart").is_ok());
        assert!(recorded_line_allowlisted("systemctl restart stalwart").is_ok());
        assert!(recorded_line_allowlisted("systemctl disable --now stalwart").is_ok());
        assert!(recorded_line_allowlisted("systemctl restart k2-daemon").is_err());
        let query = recorded_line_allowlisted("systemctl? is-active stalwart").unwrap_err();
        assert!(query.contains("not a helper"), "{query}");
        assert!(is_privileged_recording("systemctl? is-active stalwart"));
        assert!(!is_privileged_recording("useradd stalwart"));
        assert!(!is_privileged_recording(
            "extract stalwart -> /usr/local/bin/stalwart (mode 755)"
        ));
        // Protocol 2 recordings.
        assert!(recorded_line_allowlisted("systemctl stop stalwart").is_ok());
        assert!(recorded_line_allowlisted("systemctl stop k2-daemon").is_err());
        assert!(recorded_line_allowlisted("snapshot-data").is_ok());
        assert!(recorded_line_allowlisted("restore-data").is_ok());
        assert!(recorded_line_allowlisted("helper version").is_ok());
        assert!(recorded_line_allowlisted("helper usage").is_ok());
        assert!(recorded_line_allowlisted("helper snapshot-data").is_err());
        assert!(recorded_line_allowlisted("helper version extra").is_err());
        assert!(is_privileged_recording("snapshot-data"));
        assert!(is_privileged_recording("restore-data"));
    }
}

/// The root data operations over a temp tree (never the real paths).
#[cfg(all(test, unix))]
mod data_ops_tests {
    use super::data_ops::*;
    use super::DataLayout;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    const MARGIN: u64 = 1024;

    struct Tree {
        root: PathBuf,
        layout: DataLayout,
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // Make anything chmod'ed 000 removable again.
            let _ = std::process::Command::new("chmod")
                .args(["-R", "u+rwx"])
                .arg(&self.root)
                .status();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn tree(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!(
            "k2-mail-upgrade-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = root.join("var/lib/stalwart");
        let config = root.join("etc/stalwart");
        fs::create_dir_all(data.join("data")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(data.join("data/000123.sst"), b"rocksdb table bytes").unwrap();
        fs::write(data.join("data/CURRENT"), b"MANIFEST-000001\n").unwrap();
        fs::write(data.join("data/LOCK"), b"").unwrap();
        fs::set_permissions(
            data.join("data/000123.sst"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        fs::write(config.join("config.json"), br#"{"@type":"RocksDb"}"#).unwrap();
        fs::set_permissions(config.join("config.json"), fs::Permissions::from_mode(0o600))
            .unwrap();
        std::os::unix::fs::symlink("data/CURRENT", data.join("current-link")).unwrap();
        let layout = DataLayout {
            data: data.clone(),
            config,
            snapshot: root.join("var/lib/stalwart.k2-snap"),
            store_lock: data.join("data/LOCK"),
        };
        Tree { root, layout }
    }

    fn stopped() -> String {
        "inactive".into()
    }
    fn plenty(_: &Path) -> Result<u64, String> {
        Ok(u64::MAX / 4)
    }

    #[test]
    fn snapshot_copies_data_and_config_with_modes_times_and_links() {
        let t = tree("snap");
        let src_meta = fs::metadata(t.layout.data.join("data/000123.sst")).unwrap();
        let out = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
        assert_eq!(out["helperProtocol"], super::HELPER_PROTOCOL);
        assert!(snapshot_complete(&t.layout));
        let snap = &t.layout.snapshot;
        assert_eq!(
            fs::read(snap.join("data/data/000123.sst")).unwrap(),
            b"rocksdb table bytes"
        );
        assert_eq!(
            fs::read(snap.join("config/config.json")).unwrap(),
            br#"{"@type":"RocksDb"}"#
        );
        let copy_meta = fs::metadata(snap.join("data/data/000123.sst")).unwrap();
        assert_eq!(copy_meta.mode() & 0o7777, 0o640);
        assert_eq!(copy_meta.mtime(), src_meta.mtime());
        assert_eq!(copy_meta.uid(), src_meta.uid());
        assert_eq!(
            fs::metadata(snap.join("config/config.json")).unwrap().mode() & 0o7777,
            0o600
        );
        let link = snap.join("data/current-link");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("data/CURRENT"));
        assert!(!t.layout.snapshot_tmp().exists(), "tmp renamed away");
        assert_eq!(
            fs::metadata(snap).unwrap().mode() & 0o777,
            0o700,
            "snapshot root is private"
        );
        // usage sees it.
        let u = usage_with(&t.layout, &plenty).expect("usage");
        assert_eq!(u["snapshotComplete"], true);
        assert!(u["dataBytes"].as_u64().unwrap() > 0);
        assert!(u["snapshotBytes"].as_u64().unwrap() >= u["dataBytes"].as_u64().unwrap());
        // A second snapshot replaces the first (one snapshot at a time).
        fs::write(t.layout.data.join("data/000124.sst"), b"newer").unwrap();
        snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("second snapshot");
        assert_eq!(fs::read(snap.join("data/data/000124.sst")).unwrap(), b"newer");
    }

    #[test]
    fn snapshot_and_restore_refuse_unless_stalwart_is_stopped() {
        let t = tree("running");
        for state in ["active", "activating", "deactivating", "reloading", ""] {
            let st = move || state.to_string();
            let err = snapshot_with(&t.layout, &st, &plenty, MARGIN).expect_err(state);
            assert!(err.starts_with("refused"), "{state}: {err}");
            assert!(!t.layout.snapshot.exists(), "{state}");
            assert!(!t.layout.snapshot_tmp().exists(), "{state}");
            let err = restore_with(&t.layout, &st, &plenty, MARGIN).expect_err(state);
            assert!(err.starts_with("refused"), "{state}: {err}");
        }
        // `failed` is a stopped unit.
        let failed = || "failed".to_string();
        snapshot_with(&t.layout, &failed, &plenty, MARGIN).expect("failed unit = stopped");
    }

    #[test]
    fn snapshot_fails_closed_if_stalwart_starts_during_the_copy() {
        let t = tree("started");
        let calls = std::cell::Cell::new(0);
        let st = || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 { "inactive" } else { "active" }.to_string()
        };
        let err = snapshot_with(&t.layout, &st, &plenty, MARGIN).expect_err("started");
        assert!(err.contains("started during the snapshot copy"), "{err}");
        assert!(!t.layout.snapshot.exists());
        assert!(!t.layout.snapshot_tmp().exists(), "partial copy removed");
        assert!(t.layout.data.join("data/000123.sst").exists(), "live data untouched");
    }

    #[test]
    fn snapshot_refuses_symlinks_special_files_and_low_space() {
        // Live data dir is a symlink → refused.
        let t = tree("symlink");
        let real = t.root.join("elsewhere");
        fs::rename(&t.layout.data, &real).unwrap();
        std::os::unix::fs::symlink(&real, &t.layout.data).unwrap();
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("symlink");
        assert!(err.contains("symlink"), "{err}");

        // A FIFO inside the store → refused, tmp cleaned up.
        let t = tree("fifo");
        let fifo = t.layout.data.join("data/pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: NUL-terminated path in a temp dir we own.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("fifo");
        assert!(err.contains("not a regular file"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists());
        assert!(!t.layout.snapshot.exists());

        // Space: data + config + margin must fit in free + old snapshot.
        let t = tree("space");
        let tight = |_: &Path| -> Result<u64, String> { Ok(10) };
        let err = snapshot_with(&t.layout, &stopped, &tight, MARGIN).expect_err("space");
        assert!(err.contains("not enough free space"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists());
    }

    #[test]
    fn snapshot_cleans_up_a_partial_copy_on_a_read_error() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads mode-000 files; this case needs a non-root run
        }
        let t = tree("unreadable");
        let f = t.layout.data.join("data/000123.sst");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o000)).unwrap();
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("unreadable");
        assert!(err.contains("open"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists(), "partial copy removed");
        assert!(!snapshot_complete(&t.layout));
        fs::set_permissions(&f, fs::Permissions::from_mode(0o640)).unwrap();
    }

    #[test]
    fn restore_puts_the_snapshot_back_on_both_space_paths() {
        for (tag, free) in [("copy-first", u64::MAX / 4), ("delete-first", 0u64)] {
            let t = tree(tag);
            snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
            // The failed upgrade rewrote the store and the config.
            fs::write(t.layout.data.join("data/000123.sst"), b"migrated by 0.16.20").unwrap();
            fs::write(t.layout.data.join("data/999999.sst"), b"new file").unwrap();
            fs::write(t.layout.config.join("config.json"), b"{}").unwrap();
            let free_of = move |_: &Path| -> Result<u64, String> { Ok(free) };
            let out = restore_with(&t.layout, &stopped, &free_of, MARGIN).expect(tag);
            assert_eq!(out["copyFirst"], tag == "copy-first", "{tag}");
            assert!(out["leftovers"].as_array().unwrap().is_empty(), "{tag}: {out}");
            assert_eq!(
                fs::read(t.layout.data.join("data/000123.sst")).unwrap(),
                b"rocksdb table bytes",
                "{tag}"
            );
            assert!(!t.layout.data.join("data/999999.sst").exists(), "{tag}");
            assert_eq!(
                fs::read(t.layout.config.join("config.json")).unwrap(),
                br#"{"@type":"RocksDb"}"#,
                "{tag}"
            );
            assert_eq!(
                fs::metadata(t.layout.data.join("data/000123.sst")).unwrap().mode() & 0o7777,
                0o640,
                "{tag}"
            );
            for p in [
                t.layout.data_restore_tmp(),
                t.layout.data_aside(),
                t.layout.config_restore_tmp(),
                t.layout.config_aside(),
            ] {
                assert!(!p.exists(), "{tag}: {} left behind", p.display());
            }
            // The snapshot survives a restore (a second attempt can use it).
            assert!(snapshot_complete(&t.layout), "{tag}");
        }
    }

    #[test]
    fn restore_refuses_an_incomplete_snapshot_and_changes_nothing() {
        let t = tree("incomplete");
        let err = restore_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("none");
        assert!(err.contains("no complete snapshot"), "{err}");
        snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
        fs::remove_file(t.layout.snapshot_marker()).unwrap();
        fs::write(t.layout.data.join("data/000123.sst"), b"live").unwrap();
        let err = restore_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("no marker");
        assert!(err.contains("no complete snapshot"), "{err}");
        assert_eq!(
            fs::read(t.layout.data.join("data/000123.sst")).unwrap(),
            b"live",
            "live data untouched"
        );
    }

    #[test]
    fn install_file_atomic_replaces_and_refuses_a_staging_link() {
        let t = tree("install");
        let bin_dir = t.root.join("usr/local/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let dest = bin_dir.join("stalwart");
        let staging = bin_dir.join(".stalwart.k2-new");
        fs::write(&dest, b"old binary").unwrap();
        fs::write(&staging, b"stale staging").unwrap();
        install_file_atomic(&staging, &dest, b"new binary", 0o755, None).expect("install");
        assert_eq!(fs::read(&dest).unwrap(), b"new binary");
        assert_eq!(fs::metadata(&dest).unwrap().mode() & 0o7777, 0o755);
        assert!(!staging.exists());
        std::os::unix::fs::symlink(t.root.join("evil"), &staging).unwrap();
        let err = install_file_atomic(&staging, &dest, b"x", 0o755, None).expect_err("link");
        assert!(err.contains("not a regular file"), "{err}");
        assert_eq!(fs::read(&dest).unwrap(), b"new binary", "dest untouched");
        assert!(!t.root.join("evil").exists(), "never written through the link");
    }

    #[test]
    fn store_lock_is_free_when_missing_or_unlocked() {
        let t = tree("lock");
        assert!(!store_lock_held(&t.layout.store_lock).expect("unlocked"));
        assert!(!store_lock_held(&t.root.join("nope/LOCK")).expect("missing"));
    }

    /// A store some OTHER process holds open (RocksDB's `F_SETLK` write
    /// lock on LOCK — e.g. a Stalwart started by hand) refuses the data
    /// verbs even when systemd says `inactive`.
    #[test]
    fn a_lock_held_by_another_process_refuses_snapshot_and_restore() {
        let t = tree("held");
        let path = std::ffi::CString::new(t.layout.store_lock.to_str().unwrap()).unwrap();
        let mut fds = [0i32; 2];
        // SAFETY: plain pipe/fork; the child only makes async-signal-safe
        // calls (open, fcntl, write, sleep, _exit) and never returns.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork");
        if pid == 0 {
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_RDWR);
                let mut fl: libc::flock = std::mem::zeroed();
                fl.l_type = libc::F_WRLCK as _;
                fl.l_whence = libc::SEEK_SET as _;
                let ok = fd >= 0 && libc::fcntl(fd, libc::F_SETLK, &fl) == 0;
                let byte: u8 = if ok { b'1' } else { b'0' };
                libc::write(fds[1], (&byte as *const u8).cast(), 1);
                libc::sleep(30);
                libc::_exit(0);
            }
        }
        let mut got = 0u8;
        // SAFETY: reading one byte from our pipe end.
        let n = unsafe { libc::read(fds[0], (&mut got as *mut u8).cast(), 1) };
        assert_eq!(n, 1, "child reported");
        assert_eq!(got, b'1', "child took the lock");
        let held = store_lock_held(&t.layout.store_lock);
        let snap = snapshot_with(&t.layout, &stopped, &plenty, MARGIN);
        let restore = restore_with(&t.layout, &stopped, &plenty, MARGIN);
        // SAFETY: our own child.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        assert!(held.expect("lock check"), "another process holds the lock");
        let err = snap.expect_err("held lock refuses the snapshot");
        assert!(err.contains("store lock"), "{err}");
        assert!(!t.layout.snapshot.exists());
        let err = restore.expect_err("held lock refuses the restore");
        assert!(err.contains("store lock"), "{err}");
        assert!(
            !store_lock_held(&t.layout.store_lock).expect("released"),
            "the lock dies with the process"
        );
    }
}
