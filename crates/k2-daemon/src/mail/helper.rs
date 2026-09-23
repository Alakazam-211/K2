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
use std::path::{Component, Path};

use super::supervisor::{
    artifact_for_arch, hardening_dropin, systemd_unit, STALWART_BIN, STALWART_CONFIG_DIR,
    STALWART_DATA_DIR, STALWART_DROPIN_DIR, STALWART_DROPIN_PATH, STALWART_LOG_DIR, STALWART_UNIT,
    STALWART_UNIT_PATH, STALWART_USER,
};

pub const HELPER_PATH: &str = "/usr/local/libexec/k2-mail-helper";
pub const SUDO_PATH: &str = "/usr/bin/sudo";

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
    [DAEMON_RELOAD, ENABLE_NOW, RESTART, DISABLE_NOW]
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

/// Stdin is the tarball, checked against [`artifact_for_arch`] for this
/// process's arch. A member hash is not the pin.
pub fn install_archive_pinned(bytes: &[u8]) -> Result<(), String> {
    let art = artifact_for_arch(std::env::consts::ARCH)?;
    if !crate::update_routes::verify_sha256(bytes, art.sha256) {
        return Err("install-bin: tarball sha256 mismatch".into());
    }
    Ok(())
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
        HelperCommand::EnsureUser | HelperCommand::Systemctl(_) => {
            Err("mail helper: no filesystem effect".into())
        }
    }
}

/// `NotFound` (sudo or the helper missing) and sudo's password / allow
/// refusals. Other stderr is not rewritten.
pub fn map_helper_failure(spawn: Option<&std::io::Error>, stderr: &str) -> Option<String> {
    if spawn.is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Some("mail helper not installed".to_string());
    }
    if stderr.contains("a password is required") || stderr.contains("not allowed to execute") {
        return Some("mail helper not installed".to_string());
    }
    if stderr.contains(HELPER_PATH)
        && (stderr.contains("command not found") || stderr.contains("No such file or directory"))
    {
        return Some("mail helper not installed".to_string());
    }
    None
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
}

/// Map a [`super::sysops`] fake recording line onto a §2 helper vector.
pub fn recorded_line_allowlisted(line: &str) -> Result<(), String> {
    if let Some(rest) = line.strip_prefix("systemctl?") {
        let _ = rest;
        return Err("systemctl query is not a helper verb".into());
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
        decide_effect, extract_stalwart_member, install_archive_pinned, write_bytes_accepted,
        FsEffect, HelperCommand, NodeKind, STALWART_BIN, STALWART_USER,
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
                install_archive_pinned(stdin)?;
                let member = extract_stalwart_member(stdin)?;
                apply_fs(cmd, STALWART_BIN, Some((&member, 0o755)))
            }
        }
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
        ];
        for v in vectors {
            assert_accept(v);
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
            &["systemctl", "stop", "stalwart"],
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

    #[test]
    fn install_bin_rejects_bytes_that_are_not_the_tarball_pin() {
        let bogus = b"not-the-stalwart-tarball-CANARY";
        let err = install_archive_pinned(bogus).expect_err("wrong tarball bytes must be rejected");
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
        let missing = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file User=root-CANARY",
        );
        let msg = map_helper_failure(Some(&missing), "").unwrap();
        assert_eq!(msg, "mail helper not installed");
        assert!(!msg.contains("CANARY"));

        for stderr in [
            "sudo: a password is required",
            "k2 is not allowed to execute /usr/local/libexec/k2-mail-helper as root on this host",
            "sudo: a password is required\nUser=root-CANARY",
        ] {
            let msg = map_helper_failure(None, stderr).unwrap();
            assert_eq!(msg, "mail helper not installed");
            assert!(!msg.contains("CANARY"), "{stderr} -> {msg}");
        }

        let gone = format!("sudo: {HELPER_PATH}: command not found");
        assert_eq!(
            map_helper_failure(None, &gone).as_deref(),
            Some("mail helper not installed")
        );
        assert!(map_helper_failure(None, "ensure-user: useradd failed: exit 1").is_none());

        let mut marked = false;
        let err: Result<(), String> = Err(msg);
        mark_step_on_ok(&err, &mut marked);
        assert!(!marked, "helper error must leave the step unmarked");
        mark_step_on_ok(&Ok(()), &mut marked);
        assert!(marked);
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
    }
}
