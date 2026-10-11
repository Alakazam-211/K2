//! `mail` namespace (§7.2): today's protocol-2 verbs, same behaviour,
//! named targets instead of path arguments (RH1), render verbs instead of
//! byte compares (RH4), destructive/credential verbs daemon-only (RH2,
//! RH25 — enforced by the class table before any of this runs).

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Component, Path};

use k2_root_proto::paths::*;
use k2_root_proto::stalwart::{
    artifact_for_arch, previous_artifact_for_arch, StalwartArtifact, STALWART_PINNED_VERSION,
    STALWART_PREVIOUS_VERSION,
};
use k2_root_proto::templates::{hardening_dropin, recovery_pw_ok, systemd_unit};
use k2_root_proto::verbs::{ChownTarget, MailSystemctl, MkdirTarget, RemoveTarget};

use crate::ctx::{Ctx, Fail, HResult, KindStr};
use crate::fsx::{Dir, Kind};
use crate::mail_data::{self, DataLayout, SNAPSHOT_FREE_MARGIN};

/// Official Stalwart binary upper bound. The pin check runs first.
const MAX_STALWART_MEMBER: u64 = 256 * 1024 * 1024;

/// Split a compiled-in absolute path into (parent, file name).
pub(crate) fn split(abs: &'static str) -> (&'static str, &'static str) {
    let i = abs.rfind('/').expect("absolute");
    (if i == 0 { "/" } else { &abs[..i] }, &abs[i + 1..])
}

/// The parent directory (opened without following links) and the name.
pub(crate) fn parent_dir(ctx: &Ctx, abs: &'static str) -> HResult<(Dir, &'static OsStr)> {
    let (parent, name) = split(abs);
    let d = ctx
        .dir(parent)
        .map_err(|e| Fail::io_err("open parent", e))?;
    Ok((d, OsStr::new(name)))
}

fn mkdir_path(t: MkdirTarget) -> &'static str {
    match t {
        MkdirTarget::Config => STALWART_CONFIG_DIR,
        MkdirTarget::Data => STALWART_DATA_DIR,
        MkdirTarget::Log => STALWART_LOG_DIR,
        MkdirTarget::DropinDir => STALWART_DROPIN_DIR,
    }
}

fn chown_path(t: ChownTarget) -> &'static str {
    match t {
        ChownTarget::Config => STALWART_CONFIG_DIR,
        ChownTarget::Data => STALWART_DATA_DIR,
        ChownTarget::Log => STALWART_LOG_DIR,
    }
}

fn remove_path(t: RemoveTarget) -> &'static str {
    match t {
        RemoveTarget::Unit => STALWART_UNIT_PATH,
        RemoveTarget::DropinDir => STALWART_DROPIN_DIR,
        RemoveTarget::Bin => STALWART_BIN,
        RemoveTarget::Data => STALWART_DATA_DIR,
        RemoveTarget::Log => STALWART_LOG_DIR,
        RemoveTarget::Config => STALWART_CONFIG_DIR,
    }
}

pub fn ensure_user(ctx: &Ctx) -> HResult<()> {
    let id = ctx
        .exec
        .run(ID, &["-u".into(), STALWART_USER.into()], None)
        .map_err(|e| Fail::io(format!("id: {}", e.kind_str())))?;
    if id.status == 0 {
        return Ok(());
    }
    ctx.run_ok(
        USERADD,
        &[
            "--system",
            "--no-create-home",
            "--shell",
            "/usr/sbin/nologin",
            STALWART_USER,
        ],
        None,
    )?;
    Ok(())
}

/// 0755 root; a symlink (or a file) at the path is a refusal.
pub fn mkdir(ctx: &Ctx, t: MkdirTarget) -> HResult<()> {
    let (d, name) = parent_dir(ctx, mkdir_path(t))?;
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => d.mkdir(name, 0o755).or_else(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(Fail::io_err("mkdir", e))
            }
        })?,
        Kind::Dir => {}
        Kind::Symlink => return Err(Fail::state("mkdir refused: symlink")),
        _ => return Err(Fail::state("mkdir refused")),
    }
    let sub = d.sub(name).map_err(|e| Fail::io_err("mkdir refused", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        sub.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
    }
    sub.fchmod(0o755).map_err(|e| Fail::io_err("chmod", e))
}

fn chown_tree(dir: &Dir, uid: u32, gid: u32) -> HResult<()> {
    for name in dir.entries().map_err(|e| Fail::io_err("read dir", e))? {
        match dir.kind(&name).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Symlink => return Err(Fail::state("chown refused: symlink")),
            Kind::Dir => {
                dir.chown_at(&name, uid, gid)
                    .map_err(|e| Fail::io_err("chown", e))?;
                let sub = dir.sub(&name).map_err(|e| Fail::io_err("chown", e))?;
                chown_tree(&sub, uid, gid)?;
            }
            _ => dir
                .chown_at(&name, uid, gid)
                .map_err(|e| Fail::io_err("chown", e))?,
        }
    }
    Ok(())
}

/// → `stalwart:stalwart`, recursively, never through a link.
pub fn chown(ctx: &Ctx, t: ChownTarget) -> HResult<()> {
    let (d, name) = parent_dir(ctx, chown_path(t))?;
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Dir => {}
        Kind::Symlink => return Err(Fail::state("chown refused: symlink")),
        Kind::Missing => return Err(Fail::state("chown refused: missing")),
        _ => return Err(Fail::state("chown refused")),
    }
    let (uid, gid) = if ctx.real_owners {
        let (u, g) = crate::sys::ids_of_user(STALWART_USER)
            .ok_or_else(|| Fail::state("chown refused: user stalwart missing"))?;
        if u == 0 {
            return Err(Fail::state("chown refused: stalwart uid is 0"));
        }
        (u, g)
    } else {
        // Tests: chown to ourselves (exercises the walk, never root).
        // SAFETY: no preconditions.
        unsafe { (libc::getuid(), libc::getgid()) }
    };
    let sub = d.sub(name).map_err(|e| Fail::io_err("chown refused", e))?;
    sub.fchown(uid, gid).map_err(|e| Fail::io_err("chown", e))?;
    chown_tree(&sub, uid, gid)
}

/// Stdin: empty, or exactly the 64-hex recovery admin password. The helper
/// renders its own unit (0600) — the daemon never sends unit bytes.
pub fn render_unit(ctx: &Ctx, stdin: &[u8]) -> HResult<()> {
    let pw = match stdin.len() {
        0 => None,
        64 => {
            let s = std::str::from_utf8(stdin)
                .map_err(|_| Fail::argv("render-unit refused: password"))?;
            if !recovery_pw_ok(s) {
                return Err(Fail::argv("render-unit refused: password"));
            }
            Some(s)
        }
        _ => return Err(Fail::argv("render-unit refused: password")),
    };
    let (d, name) = parent_dir(ctx, STALWART_UNIT_PATH)?;
    d.write_atomic(name, systemd_unit(pw).as_bytes(), 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("write unit", e))
}

/// The helper's hardening drop-in, 0644. The drop-in dir must exist
/// (`mail mkdir dropin-dir` first, as before).
pub fn render_dropin(ctx: &Ctx) -> HResult<()> {
    let (d, name) = parent_dir(ctx, STALWART_DROPIN_PATH)?;
    d.write_atomic(name, hardening_dropin().as_bytes(), 0o644, ctx.root_owner())
        .map_err(|e| Fail::io_err("write drop-in", e))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The two-entry check: the pin first, then the previous version.
pub fn match_archive(
    bytes: &[u8],
    pin: &StalwartArtifact,
    previous: &StalwartArtifact,
) -> HResult<&'static str> {
    let got = sha256_hex(bytes);
    if got.eq_ignore_ascii_case(pin.sha256) {
        return Ok(STALWART_PINNED_VERSION);
    }
    if got.eq_ignore_ascii_case(previous.sha256) {
        return Ok(STALWART_PREVIOUS_VERSION);
    }
    Err(Fail::argv("install-bin: tarball sha256 mismatch"))
}

fn exact_stalwart_name(path: &Path) -> bool {
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(name)), None) => name == "stalwart",
        _ => false,
    }
}

/// Read member `stalwart` only. Other names, links, and `..` are not written.
pub fn extract_stalwart_member(archive: &[u8]) -> HResult<Vec<u8>> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(dec);
    let entries = tar
        .entries()
        .map_err(|_| Fail::argv("install-bin: archive unreadable"))?;
    for entry in entries {
        let entry = entry.map_err(|_| Fail::argv("install-bin: archive entry unreadable"))?;
        let path = entry
            .path()
            .map_err(|_| Fail::argv("install-bin: archive path unreadable"))?;
        if !exact_stalwart_name(&path) {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(Fail::argv(
                "install-bin: stalwart member is not a regular file",
            ));
        }
        let mut buf = Vec::new();
        entry
            .take(MAX_STALWART_MEMBER.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|_| Fail::argv("install-bin: read member failed"))?;
        if buf.len() as u64 > MAX_STALWART_MEMBER {
            return Err(Fail::argv("install-bin: stalwart member too large"));
        }
        if buf.is_empty() {
            return Err(Fail::argv("install-bin: stalwart member empty"));
        }
        return Ok(buf);
    }
    Err(Fail::argv("install-bin: stalwart member missing"))
}

/// Stdin tarball; sha must equal this build's pin or previous; extracts
/// member `stalwart` only; stage + rename (never truncate in place).
pub fn install_bin(ctx: &Ctx, stdin: &[u8]) -> HResult<serde_json::Value> {
    let pin = artifact_for_arch(ctx.arch).map_err(Fail::argv)?;
    let prev = previous_artifact_for_arch(ctx.arch).map_err(Fail::argv)?;
    let version = match_archive(stdin, pin, prev)?;
    let member = extract_stalwart_member(stdin)?;
    let (d, name) = parent_dir(ctx, STALWART_BIN)?;
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing | Kind::File => {}
        _ => return Err(Fail::state("install-bin refused")),
    }
    let (_, stage) = split(STALWART_BIN_STAGING);
    match d
        .kind(OsStr::new(stage))
        .map_err(|e| Fail::io_err("stat", e))?
    {
        Kind::Missing => {}
        Kind::File => d
            .unlink(OsStr::new(stage))
            .map_err(|e| Fail::io_err("install-bin: remove stage", e))?,
        _ => {
            return Err(Fail::state(
                "install-bin refused: staging path is not a regular file",
            ))
        }
    }
    let res = (|| -> std::io::Result<()> {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let mut f = d.create_excl(OsStr::new(stage))?;
        f.write_all(&member)?;
        if let Some((u, g)) = ctx.root_owner() {
            // SAFETY: fd owned by f.
            if unsafe { libc::fchown(f.as_raw_fd(), u, g) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: fd owned by f.
        if unsafe { libc::fchmod(f.as_raw_fd(), 0o755) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        f.sync_all()?;
        d.rename(OsStr::new(stage), name)?;
        d.fsync()
    })();
    if let Err(e) = res {
        let _ = d.unlink(OsStr::new(stage));
        return Err(Fail::io_err("install-bin", e));
    }
    Ok(serde_json::json!({ "installed": version }))
}

pub fn systemctl(ctx: &Ctx, which: MailSystemctl) -> HResult<()> {
    let args: &[&str] = match which {
        MailSystemctl::DaemonReload => &["daemon-reload"],
        MailSystemctl::EnableNow => &["enable", "--now", STALWART_UNIT],
        MailSystemctl::Restart => &["restart", STALWART_UNIT],
        // Stop for the snapshot window WITHOUT touching boot enablement.
        MailSystemctl::Stop => &["stop", STALWART_UNIT],
        MailSystemctl::DisableNow => &["disable", "--now", STALWART_UNIT],
    };
    ctx.run_ok(SYSTEMCTL, args, None).map(|_| ())
}

/// Uninstall / purge one named target. Missing is fine; a symlink is a
/// refusal; directories are removed without following links inside.
pub fn remove(ctx: &Ctx, t: RemoveTarget) -> HResult<()> {
    let (d, name) = parent_dir(ctx, remove_path(t))?;
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(()),
        Kind::Symlink => Err(Fail::state("remove refused: symlink")),
        Kind::Dir | Kind::File => d.remove_tree(name).map_err(|e| Fail::io_err("remove", e)),
        Kind::Other => Err(Fail::state("remove refused")),
    }
}

/// `systemctl is-active stalwart` as root: a non-zero exit is an answer
/// (`inactive` exits 3); no output reads as unknown and the data verbs
/// refuse on it.
fn unit_state(ctx: &Ctx) -> String {
    ctx.exec
        .run(SYSTEMCTL, &["is-active".into(), STALWART_UNIT.into()], None)
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn usage(ctx: &Ctx) -> HResult<serde_json::Value> {
    mail_data::usage_with(&DataLayout::under(&ctx.root), &mail_data::free_bytes)
        .map_err(Fail::state)
}

pub fn snapshot(ctx: &Ctx) -> HResult<serde_json::Value> {
    mail_data::snapshot_with(
        &DataLayout::under(&ctx.root),
        &|| unit_state(ctx),
        &mail_data::free_bytes,
        SNAPSHOT_FREE_MARGIN,
    )
    .map_err(Fail::state)
}

pub fn restore(ctx: &Ctx) -> HResult<serde_json::Value> {
    mail_data::restore_with(
        &DataLayout::under(&ctx.root),
        &|| unit_state(ctx),
        &mail_data::free_bytes,
        SNAPSHOT_FREE_MARGIN,
    )
    .map_err(Fail::state)
}
