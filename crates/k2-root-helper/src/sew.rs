//! `engine sew` namespace (protocol 7, Sew S29 K11; notices protocol 8):
//! install, run, upgrade and remove Sew through Sew's own ops CLI, as root.
//!
//! Rules on top of the helper's (§5.3):
//! - **Pins.** A tarball is staged only when its sha256 is this build's pin
//!   for the arch AND its single binary member hashes to the pinned binary
//!   sha (`k2_root_proto::sew`). Nothing is downloaded here.
//! - **Notices** (Sew SW1 B1, 0.2.0 on). The pinned tarballs carry
//!   `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES.md` beside the binary;
//!   the helper puts them under `/usr/share/doc/sew` (`install-bin`, before
//!   the binary; a successful `upgrade`) and `/usr/share/doc/sew-media` (a
//!   successful `media install`), root 0644, and removes them on a
//!   successful `purge` / `media uninstall`. They follow the binary on a
//!   rollback: an upgrade keeps the old set in `/usr/share/doc/sew/previous`
//!   (beside Sew's `sew.prev`), a rollback swaps the two sets as Sew swaps
//!   the binaries. The binary member is still the only thing ever executed.
//! - **Known binary.** Before any root exec of `/usr/local/bin/sew` the file
//!   must be a regular, root-owned, not group/other-writable file whose
//!   sha256 is a release this build knows (S29 §7.3).
//! - **Inputs.** URLs and keys arrive on stdin (`InstallRequest`,
//!   `ConfigRequest`, `MediaRequest`, key text), validated here before
//!   anything runs. Sew reads JSON configs from fixed files under
//!   `/var/lib/k2-root-helper/sew` (0700 root, removed after the call);
//!   the master key goes to Sew on its stdin only, never to a file.
//! - **Sew's exit codes are data.** `install`, `upgrade`, `rollback`,
//!   `purge`, `restore`, … answer `{"sewExit": n, "sew": <Sew's JSON>,
//!   "stderrTail": "…"}` with helper exit 0; the daemon maps them (S29 §7.5).
//!   A non-zero exit always carries a non-empty `reason` (Sew's `--json`
//!   error message, else its stderr, else its stdout, else the bare exit),
//!   bounded and stripped of control characters, so a refusal never reaches
//!   the daemon without its words.
//! - **Upgrade = `--binary`** (Sew's contract, 2026-10-17). The INSTALLED
//!   Sew runs `sew upgrade --binary <staged sew member> --sha256 <pin
//!   binary_sha256>`: Sew 0.1.x refuses a tarball with more than one member
//!   and 0.2.0's carries the notices. The staged copy is root 0700 (Sew runs
//!   its `version`) and is removed after the call. The voice engine follows
//!   in a separate verb, `media upgrade`, run by the NEW core once the
//!   daemon has checked it (the sidecar pins are baked into `sew`).
//! - **Rollback mirrors it** (Sew, 2026-10-17): after the core rollback
//!   and the daemon's check of the old core, `media rollback` has the OLD
//!   core reinstall its own release's engine (`sew media upgrade --tarball
//!   <kept> --now`; Sew 0.1.x checks only pin + bytes, so this downgrades).
//!   The helper KEEPS engine tarballs for that under [`SEW_KEEP_DIR`]: the
//!   pin's after a successful media install / upgrade, and an older
//!   release's when the daemon stages it. Never Sew's `sew-media.prev`.
//!   Media notices are REPLACED by the reinstalled engine's set (removed
//!   when that release shipped none).
//! - **No groups, no firewall, no edits of Sew's files.** The daemon user
//!   never joins `sew-mgmt`: its uid rides `sew install`'s JSON as
//!   `mgmt_allow_uids` and Sew writes `[mgmt] allow_uids` (SW5 + SW7); the
//!   daemon reads the mgmt token through `engine sew token`. The helper
//!   opens no ports.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Component, Path};

use k2_root_proto::paths::{STATE_DIR, SYSTEMCTL};
use k2_root_proto::sew::*;
use k2_root_proto::verbs::{SewSystemctl, SewVerb};

use crate::caller::Caller;
use crate::ctx::{Ctx, Fail, HResult, KindStr};
use crate::fsx::{Dir, Kind};

type J = serde_json::Value;
use serde_json::json;

/// Sew's stderr kept in an answer (it never holds our stdin).
const STDERR_TAIL: usize = 1200;

pub fn run(ctx: &Ctx, verb: &SewVerb, stdin: &[u8], who: &Caller) -> HResult<J> {
    // A release without a published Sew pin installs nothing: refuse the
    // install verbs before any effect (the daemon refuses first).
    if !ctx.sew_pinned && installs(verb) {
        return Err(Fail::state(format!("{SEW_NOT_RELEASED}: {SEW_NOT_RELEASED_MESSAGE}")));
    }
    match verb {
        SewVerb::Status => Ok(status(ctx)),
        SewVerb::Stage => stage(ctx, stdin, false),
        SewVerb::StageMedia => stage(ctx, stdin, true),
        SewVerb::Unstage => unstage(ctx),
        SewVerb::InstallBin => install_bin(ctx),
        SewVerb::Install => install(ctx, stdin, who),
        SewVerb::Config => config(ctx, stdin),
        SewVerb::Systemctl(op) => systemctl(ctx, *op),
        SewVerb::Upgrade => upgrade(ctx),
        SewVerb::Rollback => {
            require_known(ctx)?;
            let mut out = run_sew(ctx, &["upgrade", "--rollback"], None)?;
            // Sew swapped sew and sew.prev: the notices follow the binary.
            if out["sewExit"] == 0 {
                out["notices"] = notices_or_error(rollback_notices(ctx));
            }
            Ok(out)
        }
        SewVerb::Uninstall => {
            require_known(ctx)?;
            run_sew(ctx, &["uninstall"], None)
        }
        SewVerb::Purge { install_id, keep_backups } => {
            require_known(ctx)?;
            let confirm = format!("--yes-delete-all-data={install_id}");
            let mut args = vec!["uninstall", "--purge", confirm.as_str()];
            if *keep_backups {
                args.push("--keep-backups");
            }
            let mut out = run_sew(ctx, &args, None)?;
            if out["sewExit"] == 0 {
                out["noticesRemoved"] = notices_or_error(remove_all_notices(ctx));
                out["keptRemoved"] = notices_or_error(remove_kept(ctx));
            }
            Ok(out)
        }
        SewVerb::Doctor => {
            require_known(ctx)?;
            run_sew(ctx, &["doctor"], None)
        }
        SewVerb::RotateMaster => rotate_master(ctx, stdin),
        SewVerb::DropPrev => {
            require_known(ctx)?;
            run_sew(ctx, &["keys", "drop-prev"], None)
        }
        SewVerb::Restore(id) => restore(ctx, id),
        SewVerb::Token => token(ctx),
        SewVerb::MediaInstall => media_install(ctx, stdin),
        SewVerb::MediaUpgrade => media_upgrade(ctx),
        SewVerb::MediaRollback => media_rollback(ctx),
        SewVerb::MediaUninstall => {
            require_known(ctx)?;
            let mut out = run_sew(ctx, &["media", "uninstall"], None)?;
            if out["sewExit"] == 0 {
                out["noticesRemoved"] = notices_or_error(remove_notices(ctx, SEW_MEDIA_DOC_DIR).map(|a| json!(a)));
            }
            Ok(out)
        }
        SewVerb::MediaReconfigure { restart_now } => media_reconfigure(ctx, stdin, *restart_now),
        SewVerb::MediaTurnCert => media_turn_cert(ctx, stdin),
        SewVerb::MediaStatus => {
            require_known(ctx)?;
            run_sew(ctx, &["media", "status"], None)
        }
        SewVerb::MediaEnable => {
            require_known(ctx)?;
            run_sew(ctx, &["media", "enable"], None)
        }
        SewVerb::MediaDisable => {
            require_known(ctx)?;
            run_sew(ctx, &["media", "disable"], None)
        }
        SewVerb::MediaProbeRecord { udp, tcp, tls } => {
            require_known(ctx)?;
            let r = |b: &bool| if *b { "ok" } else { "fail" };
            run_sew(
                ctx,
                &["media", "probe-record", "--udp", r(udp), "--tcp", r(tcp), "--turn-tls", r(tls), "--from", "k2"],
                None,
            )
        }
    }
}

// ── small helpers ───────────────────────────────────────────────────────

/// The verbs that put Sew bytes on the box or run Sew's installer.
fn installs(verb: &SewVerb) -> bool {
    matches!(
        verb,
        SewVerb::Stage
            | SewVerb::StageMedia
            | SewVerb::InstallBin
            | SewVerb::Install
            | SewVerb::Upgrade
            | SewVerb::MediaInstall
            | SewVerb::MediaUpgrade
            | SewVerb::MediaRollback
    )
}

fn split(abs: &str) -> (&str, &str) {
    let i = abs.rfind('/').expect("absolute");
    (if i == 0 { "/" } else { &abs[..i] }, &abs[i + 1..])
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The directory of a fixed path, `None` when it (or a parent) is missing.
fn open_parent(ctx: &Ctx, abs: &str) -> HResult<Option<(Dir, String)>> {
    let (parent, name) = split(abs);
    match ctx.dir(parent) {
        Ok(d) => Ok(Some((d, name.to_string()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Fail::io_err("open", e)),
    }
}

fn exists(ctx: &Ctx, abs: &str) -> bool {
    open_parent(ctx, abs)
        .ok()
        .flatten()
        .and_then(|(d, n)| d.kind(OsStr::new(&n)).ok())
        .is_some_and(|k| k != Kind::Missing)
}

/// `/var/lib/k2-root-helper/sew`, created 0700 root (its parent 0755).
fn stage_dir(ctx: &Ctx) -> HResult<Dir> {
    let (gp, state) = split(STATE_DIR);
    let base = ctx.dir(gp).map_err(|e| Fail::io_err("open /var/lib", e))?;
    let state_dir = sub_dir(ctx, &base, state, 0o755)?;
    let (_, name) = split(SEW_STAGE_DIR);
    sub_dir(ctx, &state_dir, name, 0o700)
}

fn sub_dir(ctx: &Ctx, base: &Dir, name: &str, mode: u32) -> HResult<Dir> {
    let n = OsStr::new(name);
    match base.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => base.mkdir(n, mode).map_err(|e| Fail::io_err("mkdir", e))?,
        Kind::Dir => {}
        _ => return Err(Fail::state("refused: a link or a file where K2 expects a directory")),
    }
    let d = base.sub(n).map_err(|e| Fail::io_err("open", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        d.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
    }
    d.fchmod(mode).map_err(|e| Fail::io_err("chmod", e))?;
    Ok(d)
}

fn read_staged(ctx: &Ctx, abs: &str) -> HResult<Option<Vec<u8>>> {
    let Some((d, n)) = open_parent(ctx, abs)? else { return Ok(None) };
    d.read_file(OsStr::new(&n), MAX_TARBALL).map_err(|e| Fail::io_err("read staged tarball", e))
}

fn remove_file(ctx: &Ctx, abs: &str) -> HResult<bool> {
    let Some((d, n)) = open_parent(ctx, abs)? else { return Ok(false) };
    match d.kind(OsStr::new(&n)).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(false),
        Kind::File | Kind::Symlink => {
            d.unlink(OsStr::new(&n)).map_err(|e| Fail::io_err("remove", e))?;
            Ok(true)
        }
        _ => Err(Fail::state("refused: not a regular file")),
    }
}

fn exact_name(path: &Path, want: &str) -> bool {
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(name)), None) => name == want,
        _ => false,
    }
}

/// The one member `name` of a gzip tarball: a regular file, top level,
/// never through `..`, a link or a nested path.
pub fn extract_member(archive: &[u8], name: &str) -> HResult<Vec<u8>> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(dec);
    let entries = tar.entries().map_err(|_| Fail::argv("sew tarball unreadable"))?;
    for entry in entries {
        let entry = entry.map_err(|_| Fail::argv("sew tarball entry unreadable"))?;
        let path = entry.path().map_err(|_| Fail::argv("sew tarball path unreadable"))?;
        if !exact_name(&path, name) {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(Fail::argv("sew tarball: binary member is not a regular file"));
        }
        let mut buf = Vec::new();
        entry
            .take(MAX_MEMBER + 1)
            .read_to_end(&mut buf)
            .map_err(|_| Fail::argv("sew tarball: read member failed"))?;
        if buf.len() as u64 > MAX_MEMBER {
            return Err(Fail::argv("sew tarball: binary member too large"));
        }
        if buf.is_empty() {
            return Err(Fail::argv("sew tarball: binary member empty"));
        }
        return Ok(buf);
    }
    Err(Fail::argv("sew tarball: binary member missing"))
}

/// The licence members of a gzip tarball (`LICENSE`, `NOTICE`,
/// `THIRD_PARTY_NOTICES.md`; Sew 0.2.0 on), in [`NOTICE_MEMBERS`] order.
/// Top level only, regular files only, each at most [`MAX_NOTICE`], none
/// twice. An older tarball has none: an empty list.
pub fn extract_notices(archive: &[u8]) -> HResult<Vec<(&'static str, Vec<u8>)>> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(dec);
    let entries = tar.entries().map_err(|_| Fail::argv("sew tarball unreadable"))?;
    let mut found: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| Fail::argv("sew tarball entry unreadable"))?;
        let path = entry.path().map_err(|_| Fail::argv("sew tarball path unreadable"))?;
        let Some(name) = NOTICE_MEMBERS.iter().copied().find(|n| exact_name(&path, n)) else {
            continue;
        };
        if !entry.header().entry_type().is_file() {
            return Err(Fail::argv("sew tarball: a notice member is not a regular file"));
        }
        if found.iter().any(|(n, _)| *n == name) {
            return Err(Fail::argv("sew tarball: a notice member is there twice"));
        }
        let mut buf = Vec::new();
        entry
            .take(MAX_NOTICE + 1)
            .read_to_end(&mut buf)
            .map_err(|_| Fail::argv("sew tarball: read notice failed"))?;
        if buf.len() as u64 > MAX_NOTICE {
            return Err(Fail::argv("sew tarball: notice member too large"));
        }
        found.push((name, buf));
    }
    found.sort_by_key(|(n, _)| NOTICE_MEMBERS.iter().position(|m| m == n));
    Ok(found)
}

/// A directory under an existing one: created `mode` root when missing,
/// opened as is when present (never re-moded: `/usr/share/doc` is the
/// distro's), refused when something else is there.
fn ensure_dir(ctx: &Ctx, base: &Dir, name: &str, mode: u32) -> HResult<Dir> {
    let n = OsStr::new(name);
    match base.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {
            base.mkdir(n, mode).map_err(|e| Fail::io_err("mkdir", e))?;
            let d = base.sub(n).map_err(|e| Fail::io_err("open", e))?;
            if let Some((u, g)) = ctx.root_owner() {
                d.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
            }
            d.fchmod(mode).map_err(|e| Fail::io_err("chmod", e))?;
            Ok(d)
        }
        Kind::Dir => base.sub(n).map_err(|e| Fail::io_err("open", e)),
        _ => Err(Fail::state("refused: a link or a file where K2 expects a directory")),
    }
}

/// Put a pinned tarball's licence files under `doc_dir` (Sew SW1 B1): dir
/// root 0755, files root 0644, written atomically. Answers what it wrote;
/// a tarball without notices (before 0.2.0) writes nothing.
pub(crate) fn install_notices(ctx: &Ctx, archive: &[u8], doc_dir: &str) -> HResult<J> {
    let notices = extract_notices(archive)?;
    if notices.is_empty() {
        return Ok(json!({"dir": J::Null, "files": []}));
    }
    let d = open_doc_dir(ctx, doc_dir)?;
    let files = write_notice_set(ctx, &d, &notices)?;
    Ok(json!({"dir": doc_dir, "files": files}))
}

type Notices = Vec<(&'static str, Vec<u8>)>;

/// `/usr/share/doc/<leaf>`, created root 0755 when missing (and
/// `/usr/share/doc` only when missing, never re-moded).
fn open_doc_dir(ctx: &Ctx, doc_dir: &str) -> HResult<Dir> {
    let (parent, leaf) = split(doc_dir);
    let (grand, doc) = split(parent);
    let share = ctx.dir(grand).map_err(|e| Fail::io_err("open /usr/share", e))?;
    let doc_root = ensure_dir(ctx, &share, doc, 0o755)?;
    sub_dir(ctx, &doc_root, leaf, 0o755)
}

/// Make `d` hold exactly `set` of the notice files: each written
/// atomically (root 0644), any other notice file removed. Answers the
/// names written.
fn write_notice_set(ctx: &Ctx, d: &Dir, set: &Notices) -> HResult<Vec<&'static str>> {
    let mut files = Vec::new();
    for name in NOTICE_MEMBERS {
        let n = OsStr::new(name);
        match set.iter().find(|(m, _)| *m == name) {
            Some((_, bytes)) => {
                d.write_atomic(n, bytes, 0o644, ctx.root_owner()).map_err(|e| Fail::io_err("write notice", e))?;
                files.push(name);
            }
            None => match d.kind(n).map_err(|e| Fail::io_err("stat", e))? {
                Kind::Missing => {}
                Kind::File => d.unlink(n).map_err(|e| Fail::io_err("remove notice", e))?,
                _ => return Err(Fail::state("refused: a notice path is not a regular file")),
            },
        }
    }
    Ok(files)
}

/// The notice files in `abs_dir` (none when it is missing).
fn read_notice_dir(ctx: &Ctx, abs_dir: &str) -> HResult<Notices> {
    let mut out = Vec::new();
    for name in NOTICE_MEMBERS {
        let Some((d, n)) = open_parent(ctx, &format!("{abs_dir}/{name}"))? else { continue };
        let n = OsStr::new(&n);
        match d.kind(n).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing => {}
            Kind::File => {
                let b = d
                    .read_file(n, MAX_NOTICE as usize)
                    .map_err(|e| Fail::io_err("read notice", e))?
                    .unwrap_or_default();
                out.push((name, b));
            }
            _ => return Err(Fail::state("refused: a notice path is not a regular file")),
        }
    }
    Ok(out)
}

/// A successful upgrade: the notices of the binary Sew kept for rollback
/// (`/usr/local/lib/sew/sew.prev`) go to [`SEW_DOC_PREV_DIR`], the new
/// pin's into [`SEW_DOC_DIR`]. So a rollback can swap them back with the
/// binary.
pub(crate) fn upgrade_notices(ctx: &Ctx, archive: &[u8]) -> HResult<J> {
    let new = extract_notices(archive)?;
    let current = read_notice_dir(ctx, SEW_DOC_DIR)?;
    if new.is_empty() && current.is_empty() {
        return Ok(json!({"dir": J::Null, "files": [], "previous": []}));
    }
    let d = open_doc_dir(ctx, SEW_DOC_DIR)?;
    let prev = sub_dir(ctx, &d, split(SEW_DOC_PREV_DIR).1, 0o755)?;
    let kept = write_notice_set(ctx, &prev, &current)?;
    let files = write_notice_set(ctx, &d, &new)?;
    Ok(json!({"dir": SEW_DOC_DIR, "files": files, "previous": kept}))
}

/// A successful rollback: Sew swapped `sew` and `sew.prev`; the notices
/// swap with them (current ↔ previous).
pub(crate) fn rollback_notices(ctx: &Ctx) -> HResult<J> {
    let current = read_notice_dir(ctx, SEW_DOC_DIR)?;
    let previous = read_notice_dir(ctx, SEW_DOC_PREV_DIR)?;
    if current.is_empty() && previous.is_empty() {
        return Ok(json!({"dir": J::Null, "files": [], "previous": []}));
    }
    let d = open_doc_dir(ctx, SEW_DOC_DIR)?;
    let prev = sub_dir(ctx, &d, split(SEW_DOC_PREV_DIR).1, 0o755)?;
    let files = write_notice_set(ctx, &d, &previous)?;
    let kept = write_notice_set(ctx, &prev, &current)?;
    Ok(json!({"dir": SEW_DOC_DIR, "files": files, "previous": kept}))
}

/// Remove `doc_dir`'s licence files and the directory when that leaves it
/// empty. Missing is fine.
pub(crate) fn remove_notices(ctx: &Ctx, doc_dir: &str) -> HResult<Vec<&'static str>> {
    let mut removed = Vec::new();
    for name in NOTICE_MEMBERS {
        if remove_file(ctx, &format!("{doc_dir}/{name}"))? {
            removed.push(name);
        }
    }
    if let Some((parent, leaf)) = open_parent(ctx, doc_dir)? {
        let n = OsStr::new(&leaf);
        if parent.kind(n).map_err(|e| Fail::io_err("stat", e))? == Kind::Dir {
            let d = parent.sub(n).map_err(|e| Fail::io_err("open", e))?;
            if d.entries().map_err(|e| Fail::io_err("list", e))?.is_empty() {
                parent.remove_tree(n).map_err(|e| Fail::io_err("rmdir", e))?;
            }
        }
    }
    Ok(removed)
}

/// After a purge: both notice directories go.
pub(crate) fn remove_all_notices(ctx: &Ctx) -> HResult<J> {
    remove_notices(ctx, SEW_DOC_PREV_DIR)?;
    let sew = remove_notices(ctx, SEW_DOC_DIR)?;
    let media = remove_notices(ctx, SEW_MEDIA_DOC_DIR)?;
    Ok(json!({"sew": sew, "sewMedia": media}))
}

/// What is at `/usr/local/bin/sew`: `None` = absent; `Ok(Some((sha,
/// Some(version))))` = a release this build knows; `(sha, None)` unknown.
/// A link, a non-file, or (on a real box) a non-root or writable file is a
/// refusal.
fn on_disk(ctx: &Ctx) -> HResult<Option<(String, Option<&'static str>)>> {
    let Some((d, n)) = open_parent(ctx, SEW_BIN)? else { return Ok(None) };
    let name = OsStr::new(&n);
    let Some(st) = d.lstat(name).map_err(|e| Fail::io_err("stat sew", e))? else { return Ok(None) };
    if crate::fsx::kind_of(st.st_mode as u32) != Kind::File {
        return Err(Fail::state("sew helper: /usr/local/bin/sew is not a regular file"));
    }
    if ctx.real_owners && (st.st_uid != 0 || (st.st_mode as u32) & 0o022 != 0) {
        return Err(Fail::state("sew helper: /usr/local/bin/sew is not root-owned or is writable by others"));
    }
    let bytes = d
        .read_file(name, MAX_MEMBER as usize)
        .map_err(|e| Fail::io_err("read sew", e))?
        .unwrap_or_default();
    let sha = sha256_hex(&bytes);
    let known = known_binary(&sha, ctx.arch);
    Ok(Some((sha, known)))
}

/// The installed binary's version; refuses absent or unknown binaries
/// before anything runs as root.
fn require_known(ctx: &Ctx) -> HResult<&'static str> {
    match on_disk(ctx)? {
        None => Err(Fail::state("sew is not installed")),
        Some((_, None)) => Err(Fail::state("sew helper: unknown sew binary")),
        Some((_, Some(v))) => Ok(v),
    }
}

/// The last `n` characters of `text`, trimmed, every control character
/// (ANSI escapes, CR, NUL, …) but newline and tab turned into a space.
pub(crate) fn tail_clean(text: &str, n: usize) -> String {
    let t: String = text.trim().chars().rev().take(n).collect::<Vec<_>>().into_iter().rev().collect();
    t.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// Why Sew exited non-zero, never empty: its `--json` error message
/// (`{"ok":false,"error":{"exit":n,"message":"…"}}`), else its stderr,
/// else its stdout, else the bare exit code.
pub(crate) fn sew_reason(status: i32, parsed: &J, stderr: &str, stdout: &str) -> String {
    let from_json = parsed
        .pointer("/error/message")
        .or_else(|| parsed.get("message"))
        .and_then(J::as_str)
        .map(|m| tail_clean(m, REASON_MAX))
        .filter(|m| !m.is_empty());
    from_json
        .or_else(|| Some(tail_clean(stderr, REASON_MAX)).filter(|m| !m.is_empty()))
        .or_else(|| Some(tail_clean(stdout, REASON_MAX)).filter(|m| !m.is_empty()))
        .unwrap_or_else(|| format!("sew exited {status} and printed nothing"))
}

/// The longest `reason` an answer carries.
const REASON_MAX: usize = 600;

/// Run `/usr/local/bin/sew <args> --json`. A non-zero exit is data, with
/// its `reason`.
fn run_sew(ctx: &Ctx, args: &[&str], stdin: Option<&[u8]>) -> HResult<J> {
    let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    a.push("--json".into());
    let out = ctx
        .exec
        .run(SEW_BIN, &a, stdin)
        .map_err(|e| Fail::io(format!("sew: {}", e.kind_str())))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let parsed: J = serde_json::from_str(text.trim()).unwrap_or(J::Null);
    let err = String::from_utf8_lossy(&out.stderr);
    let mut v = json!({"sewExit": out.status, "sew": parsed, "stderrTail": tail_clean(&err, STDERR_TAIL)});
    if parsed.is_null() && !text.trim().is_empty() {
        // Not JSON (a panic, a usage error): keep its words too.
        v["stdoutTail"] = json!(tail_clean(&text, STDERR_TAIL));
    }
    if out.status != 0 {
        v["reason"] = json!(sew_reason(out.status, &parsed, &err, &text));
    }
    Ok(v)
}

fn unit_state(ctx: &Ctx, unit: &str) -> String {
    ctx.exec
        .run(SYSTEMCTL, &["is-active".into(), unit.into()], None)
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

// ── status ──────────────────────────────────────────────────────────────

fn staged_version(ctx: &Ctx, abs: &str, media: bool) -> Option<&'static str> {
    let bytes = read_staged(ctx, abs).ok().flatten()?;
    let sha = sha256_hex(&bytes);
    if media {
        let (v, a) = pinned_media_artifact(ctx.arch).ok()?;
        (a.tarball_sha256 == sha).then_some(v)
    } else {
        let a = pinned_artifact(ctx.arch).ok()?;
        (a.tarball_sha256 == sha).then_some(pinned().version)
    }
}

pub fn status(ctx: &Ctx) -> J {
    let pin = pinned_artifact(ctx.arch);
    let media = pinned_media_artifact(ctx.arch);
    let binary = match on_disk(ctx) {
        Ok(None) => json!({"present": false}),
        Ok(Some((sha, v))) => json!({"present": true, "sha256": sha, "version": v, "known": v.is_some()}),
        Err(f) => json!({"present": true, "known": false, "error": f.msg}),
    };
    json!({
        "pin": {
            "version": pinned().version,
            "tarballSha256": pin.as_ref().ok().map(|a| a.tarball_sha256),
            "binarySha256": pin.as_ref().ok().map(|a| a.binary_sha256),
            "placeholder": pin.is_err(),
            "error": pin.err(),
            "media": media.as_ref().ok().map(|(v, a)| json!({"version": v, "tarballSha256": a.tarball_sha256})),
        },
        "binary": binary,
        "staged": {
            "core": staged_version(ctx, SEW_STAGE_TARBALL, false),
            "media": staged_version(ctx, SEW_STAGE_MEDIA_TARBALL, true),
            "keptMedia": kept_media_versions(ctx),
        },
        "units": {
            "sewDb": unit_state(ctx, UNIT_DB),
            "sew": unit_state(ctx, UNIT_SEW),
            "sewMedia": unit_state(ctx, UNIT_MEDIA),
        },
        "config": exists(ctx, SEW_CONFIG),
        "data": exists(ctx, SEW_DATA_DIR),
        "mgmtToken": exists(ctx, SEW_MGMT_TOKEN),
        "mediaSysctl": exists(ctx, SEW_MEDIA_SYSCTL),
        "notices": {
            "sew": exists(ctx, &format!("{SEW_DOC_DIR}/LICENSE")),
            "sewMedia": exists(ctx, &format!("{SEW_MEDIA_DOC_DIR}/LICENSE")),
        },
    })
}

// ── stage / install-bin ─────────────────────────────────────────────────

fn stage(ctx: &Ctx, tarball: &[u8], media: bool) -> HResult<J> {
    if media {
        let sha = sha256_hex(tarball);
        let pin = pinned_media_artifact(ctx.arch).map_err(Fail::state)?;
        if !sha.eq_ignore_ascii_case(pin.1.tarball_sha256) {
            // Not the pin: an older release's engine is kept for a rollback.
            let Some(v) = known_media_tarball(&sha, ctx.arch) else {
                return Err(Fail::argv("stage: tarball sha256 mismatch (not a sew-media release this build knows)"));
            };
            keep_media(ctx, v, tarball)?;
            return Ok(json!({"kept": v, "kind": "media-previous", "tarballSha256": sha}));
        }
    }
    let (version, art, member, dest) = if media {
        let (v, a) = pinned_media_artifact(ctx.arch).map_err(Fail::state)?;
        (v, a, "sew-media", SEW_STAGE_MEDIA_TARBALL)
    } else {
        let a = pinned_artifact(ctx.arch).map_err(Fail::state)?;
        (pinned().version, a, "sew", SEW_STAGE_TARBALL)
    };
    let sha = sha256_hex(tarball);
    if !sha.eq_ignore_ascii_case(art.tarball_sha256) {
        return Err(Fail::argv("stage: tarball sha256 mismatch (not this build's Sew pin)"));
    }
    let bin = extract_member(tarball, member)?;
    if !sha256_hex(&bin).eq_ignore_ascii_case(art.binary_sha256) {
        return Err(Fail::argv("stage: binary sha256 mismatch (not this build's Sew pin)"));
    }
    let d = stage_dir(ctx)?;
    let (_, name) = split(dest);
    d.write_atomic(OsStr::new(name), tarball, 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("stage", e))?;
    Ok(json!({"staged": version, "kind": if media { "media" } else { "core" }, "tarballSha256": sha}))
}

fn unstage(ctx: &Ctx) -> HResult<J> {
    let mut removed = Vec::new();
    for abs in [SEW_STAGE_TARBALL, SEW_STAGE_MEDIA_TARBALL, SEW_STAGE_INSTALL_JSON, SEW_STAGE_MEDIA_JSON, SEW_STAGE_UPGRADE_BIN] {
        if remove_file(ctx, abs)? {
            removed.push(abs);
        }
    }
    Ok(json!({"removed": removed}))
}

/// The staged core tarball, re-checked against the pin.
fn staged_core(ctx: &Ctx) -> HResult<(Vec<u8>, &'static SewArtifact)> {
    let art = pinned_artifact(ctx.arch).map_err(Fail::state)?;
    let bytes = read_staged(ctx, SEW_STAGE_TARBALL)?.ok_or_else(|| Fail::state("no staged Sew tarball (engine sew stage first)"))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(art.tarball_sha256) {
        return Err(Fail::state("the staged Sew tarball is not this build's pin"));
    }
    Ok((bytes, art))
}

fn install_bin(ctx: &Ctx) -> HResult<J> {
    let (bytes, art) = staged_core(ctx)?;
    match on_disk(ctx)? {
        Some((_, Some(v))) if v == pinned().version => {
            // A re-run heals notices a crash left behind.
            let notices = install_notices(ctx, &bytes, SEW_DOC_DIR)?;
            return Ok(json!({"installed": v, "changed": false, "notices": notices}));
        }
        Some((_, Some(v))) => {
            return Err(Fail::state(format!("install-bin refused: Sew {v} is installed; use engine sew upgrade")));
        }
        Some((_, None)) => return Err(Fail::state("sew helper: unknown sew binary")),
        None => {}
    }
    let bin = extract_member(&bytes, "sew")?;
    if !sha256_hex(&bin).eq_ignore_ascii_case(art.binary_sha256) {
        return Err(Fail::argv("install-bin: binary sha256 mismatch"));
    }
    // Sew SW1 B1: the licence files go first, so a binary is never on the
    // box without its attribution (a failure here changes nothing else).
    let notices = install_notices(ctx, &bytes, SEW_DOC_DIR)?;
    let (parent, name) = split(SEW_BIN);
    let d = ctx.dir(parent).map_err(|e| Fail::io_err("open /usr/local/bin", e))?;
    let (_, stage) = split(SEW_BIN_STAGING);
    let stage = OsStr::new(stage);
    match d.kind(stage).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => {}
        Kind::File => d.unlink(stage).map_err(|e| Fail::io_err("remove stage", e))?,
        _ => return Err(Fail::state("install-bin refused: staging path is not a regular file")),
    }
    let res = (|| -> std::io::Result<()> {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let mut f = d.create_excl(stage)?;
        f.write_all(&bin)?;
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
        d.rename(stage, OsStr::new(name))?;
        d.fsync()
    })();
    if let Err(e) = res {
        let _ = d.unlink(stage);
        return Err(Fail::io_err("install-bin", e));
    }
    Ok(json!({"installed": pinned().version, "changed": true, "notices": notices}))
}

// ── install / config / keys ─────────────────────────────────────────────

/// Write a transient Sew config JSON (0600 root) and return its real path.
fn put_json(ctx: &Ctx, abs: &str, v: &J) -> HResult<String> {
    let d = stage_dir(ctx)?;
    let (_, name) = split(abs);
    let bytes = serde_json::to_vec(v).map_err(|_| Fail::io("encode json"))?;
    d.write_atomic(OsStr::new(name), &bytes, 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("write sew input", e))?;
    Ok(ctx.p(abs).display().to_string())
}

fn parse_json<T: serde::de::DeserializeOwned>(stdin: &[u8], what: &'static str) -> HResult<T> {
    serde_json::from_slice(stdin).map_err(|_| Fail::argv(what))
}

fn install(ctx: &Ctx, stdin: &[u8], who: &Caller) -> HResult<J> {
    let req: InstallRequest = parse_json(stdin, "install refused: input")?;
    req.validate().map_err(Fail::argv)?;
    let uid = who.uid.ok_or_else(Fail::not_daemon)?;
    if uid == 0 {
        return Err(Fail::state("install refused: the daemon must not run as root"));
    }
    let v = require_known(ctx)?;
    if v != pinned().version {
        return Err(Fail::state(format!("install refused: Sew {v} is installed, the pin is {}", pinned().version)));
    }
    // SW5 + SW7: the daemon's uid (from the caller check, never from
    // input) rides Sew's own install JSON; `sew install` writes `[mgmt]
    // allow_uids` itself. K2 never edits sew.toml.
    let path = put_json(ctx, SEW_STAGE_INSTALL_JSON, &req.sew_inputs(uid))?;
    // Sew reads allow_uids at start: a sew.service that is already up
    // (a re-install over a running server) must restart to apply it.
    let was_active = unit_state(ctx, UNIT_SEW) == "active";
    let mut key = req.master_key.trim_end_matches('\n').as_bytes().to_vec();
    key.push(b'\n');
    let res = run_sew(ctx, &["install", "--config-json", &path, "--master-key-from", "-", "--no-start"], Some(&key));
    key.iter_mut().for_each(|b| *b = 0);
    let _ = remove_file(ctx, SEW_STAGE_INSTALL_JSON);
    let mut out = res?;
    if out["sewExit"] == 0 {
        out["allowUid"] = json!({"uid": uid, "restartNeeded": was_active});
    }
    Ok(out)
}

fn config(ctx: &Ctx, stdin: &[u8]) -> HResult<J> {
    let req: ConfigRequest = parse_json(stdin, "config refused: input")?;
    req.validate().map_err(Fail::argv)?;
    if let Some(p) = &req.mirror_path {
        // An existing directory, never a link (the walk stays under root).
        let real = ctx.p(p);
        match std::fs::symlink_metadata(&real) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(Fail::state("config refused: keys.mirror.path is not a directory")),
            Err(_) => return Err(Fail::state("config refused: keys.mirror.path does not exist")),
        }
    }
    require_known(ctx)?;
    let mut results = Vec::new();
    for (k, v) in req.pairs() {
        let r = run_sew(ctx, &["config", "set", k, v], None)?;
        if r["sewExit"] != 0 {
            let (exit, reason) = (r["sewExit"].clone(), format!("sew config set {k}: {}", r["reason"].as_str().unwrap_or("")));
            results.push(json!({"key": k, "result": r}));
            return Ok(json!({"sewExit": exit, "reason": reason, "results": results}));
        }
        results.push(json!({"key": k, "result": r}));
    }
    Ok(json!({"sewExit": 0, "results": results}))
}

fn rotate_master(ctx: &Ctx, stdin: &[u8]) -> HResult<J> {
    let text = std::str::from_utf8(stdin).map_err(|_| Fail::argv("rotate-master refused: key"))?;
    if !master_key_text_ok(text) {
        return Err(Fail::argv("rotate-master refused: key"));
    }
    require_known(ctx)?;
    run_sew(ctx, &["keys", "rotate-master", "--new-from", "-"], Some(stdin))
}

fn token(ctx: &Ctx) -> HResult<J> {
    let (d, n) = open_parent(ctx, SEW_MGMT_TOKEN)?.ok_or_else(|| Fail::state("no Sew mgmt token (sew install did not run)"))?;
    let b = d
        .read_file(OsStr::new(&n), 256)
        .map_err(|e| Fail::io_err("read mgmt token", e))?
        .ok_or_else(|| Fail::state("no Sew mgmt token (sew install did not run)"))?;
    let t = String::from_utf8(b).map_err(|_| Fail::state("Sew mgmt token is malformed"))?;
    let t = t.strip_suffix('\n').unwrap_or(&t);
    if !mgmt_token_ok(t) {
        return Err(Fail::state("Sew mgmt token is malformed"));
    }
    Ok(json!({"token": t}))
}

// ── units, upgrade, restore, media ──────────────────────────────────────

fn systemctl(ctx: &Ctx, op: SewSystemctl) -> HResult<J> {
    let runs: Vec<Vec<&str>> = match op {
        SewSystemctl::Enable => vec![vec!["enable", UNIT_DB, UNIT_SEW, UNIT_BACKUP_TIMER, UNIT_DRILL_TIMER]],
        SewSystemctl::StartDb => vec![vec!["start", UNIT_DB]],
        SewSystemctl::Migrate => vec![vec!["start", UNIT_MIGRATE]],
        SewSystemctl::Start => vec![vec!["start", UNIT_SEW, UNIT_BACKUP_TIMER, UNIT_DRILL_TIMER]],
        SewSystemctl::Restart => vec![vec!["restart", UNIT_SEW]],
        SewSystemctl::Stop => vec![vec!["stop", UNIT_SEW]],
        SewSystemctl::DisableNow => vec![vec!["disable", "--now", UNIT_SEW, UNIT_DB, UNIT_BACKUP_TIMER, UNIT_DRILL_TIMER]],
    };
    // Every op past `enable` runs units `sew install` wrote: refuse an
    // unknown binary first (S29 §7.3 applies to what systemd will exec).
    if !matches!(op, SewSystemctl::Stop | SewSystemctl::DisableNow) {
        require_known(ctx)?;
    }
    for args in runs {
        ctx.run_ok(SYSTEMCTL, &args, None)?;
    }
    Ok(json!({"op": op.token(), "ok": true}))
}

fn upgrade(ctx: &Ctx) -> HResult<J> {
    let from = require_known(ctx)?;
    let (bytes, art) = staged_core(ctx)?;
    if from == pinned().version {
        return Ok(json!({"sewExit": 0, "sew": {"state": "unchanged", "message": "already the pin"}, "stderrTail": ""}));
    }
    if version_cmp(from, pinned().version) != Some(std::cmp::Ordering::Less) {
        return Err(Fail::state("upgrade refused: the installed Sew is newer than the pin (a downgrade is a restore)"));
    }
    // Sew's contract: the installed (older) Sew gets the bare binary, and
    // `--sha256` is the sha256 of THAT file (the pin's binary_sha256).
    let bin = extract_member(&bytes, "sew")?;
    if !sha256_hex(&bin).eq_ignore_ascii_case(art.binary_sha256) {
        return Err(Fail::argv("upgrade: the staged tarball's sew member does not match the pin's binary sha256"));
    }
    let staged_bin = put_upgrade_bin(ctx, &bin)?;
    let res = run_sew(ctx, &["upgrade", "--binary", &staged_bin, "--sha256", art.binary_sha256], None);
    // Sew copied it (or refused): the staged copy never outlives the call.
    let _ = remove_file(ctx, SEW_STAGE_UPGRADE_BIN);
    let mut out = res?;
    out["from"] = json!(from);
    out["to"] = json!(pinned().version);
    // The new binary is in place: its notices follow it. Sew's upgrade
    // already happened, so a notice failure is reported, not an exit.
    if out["sewExit"] == 0 {
        out["notices"] = notices_or_error(upgrade_notices(ctx, &bytes));
    }
    Ok(out)
}

/// Write the pin's `sew` member to [`SEW_STAGE_UPGRADE_BIN`], root 0700
/// (the installed Sew runs its `version` before it swaps), and return its
/// real path.
fn put_upgrade_bin(ctx: &Ctx, bin: &[u8]) -> HResult<String> {
    let d = stage_dir(ctx)?;
    let (_, name) = split(SEW_STAGE_UPGRADE_BIN);
    d.write_atomic(OsStr::new(name), bin, 0o700, ctx.root_owner())
        .map_err(|e| Fail::io_err("write staged sew binary", e))?;
    Ok(ctx.p(SEW_STAGE_UPGRADE_BIN).display().to_string())
}

/// `sew media upgrade --tarball <staged media pin> --now`, run by the
/// installed core, which must already BE the pin (its baked sidecar pins
/// are the ones the media tarball is checked against). `--now`: the person
/// asked for the upgrade and the core upgrade just restarted chat; calls
/// drain with a reconnect. On success the media notices are replaced from
/// the same tarball (Sew SW1 B1).
fn media_upgrade(ctx: &Ctx) -> HResult<J> {
    let installed = require_known(ctx)?;
    if installed != pinned().version {
        return Err(Fail::state(format!(
            "media upgrade refused: the installed Sew is {installed}, the pin is {}; the core is upgraded first (engine sew upgrade)",
            pinned().version
        )));
    }
    let (_, art) = pinned_media_artifact(ctx.arch).map_err(Fail::state)?;
    let bytes = read_staged(ctx, SEW_STAGE_MEDIA_TARBALL)?
        .ok_or_else(|| Fail::state("no staged sew-media tarball (engine sew stage-media first)"))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(art.tarball_sha256) {
        return Err(Fail::state("the staged sew-media tarball is not this build's pin"));
    }
    let tarball = ctx.p(SEW_STAGE_MEDIA_TARBALL).display().to_string();
    let mut out = run_sew(ctx, &["media", "upgrade", "--tarball", &tarball, "--now"], None)?;
    if out["sewExit"] == 0 {
        out["notices"] = notices_or_error(replace_notices(ctx, &bytes, SEW_MEDIA_DOC_DIR));
        out["kept"] = notices_or_error(keep_media(ctx, pinned_media_artifact(ctx.arch).map_err(Fail::state)?.0, &bytes));
    }
    Ok(out)
}

/// The reverse of [`media_upgrade`]: the installed (rolled-back) core
/// reinstalls ITS release's engine from the kept tarball. Refused before
/// anything runs when the core is the pin (roll the core back first) or
/// the tarball is not kept.
fn media_rollback(ctx: &Ctx) -> HResult<J> {
    let installed = require_known(ctx)?;
    if installed == pinned().version {
        return Err(Fail::state(format!(
            "media rollback refused: the installed Sew is the pin ({installed}); the core is rolled back first (engine sew rollback)"
        )));
    }
    let (mv, art) = media_artifact_of(installed, ctx.arch).map_err(|e| Fail::state(format!("media rollback refused: {e}")))?;
    let path = kept_media_path(mv).ok_or_else(|| Fail::state("media rollback refused: bad sew-media version"))?;
    let missing = || {
        Fail::state(format!(
            "media rollback refused: the sew-media {mv} tarball (Sew {installed}'s engine) is not kept on this box; \
             stage it first: k2 hostchat rollback --media-tarball sew-media-{mv}-{}.tar.gz",
            art.triple
        ))
    };
    let bytes = read_staged(ctx, &path)?.ok_or_else(missing)?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(art.tarball_sha256) {
        return Err(Fail::state(format!("media rollback refused: the kept sew-media {mv} tarball is not Sew {installed}'s pin")));
    }
    let tarball = ctx.p(&path).display().to_string();
    let mut out = run_sew(ctx, &["media", "upgrade", "--tarball", &tarball, "--now"], None)?;
    out["to"] = json!(mv);
    if out["sewExit"] == 0 {
        out["notices"] = notices_or_error(replace_notices(ctx, &bytes, SEW_MEDIA_DOC_DIR));
    }
    Ok(out)
}

/// The engine's notices become exactly `archive`'s set: written when it
/// has one, removed when that release shipped none (0.1.x cores), so a
/// rollback never leaves the newer release's files behind.
pub(crate) fn replace_notices(ctx: &Ctx, archive: &[u8], doc_dir: &str) -> HResult<J> {
    if extract_notices(archive)?.is_empty() {
        let removed = remove_notices(ctx, doc_dir)?;
        return Ok(json!({"dir": J::Null, "files": [], "removed": removed}));
    }
    install_notices(ctx, archive, doc_dir)
}

/// The kept-tarball dir (root 0700), created when missing.
fn keep_dir(ctx: &Ctx) -> HResult<Dir> {
    let base = stage_dir(ctx)?;
    let (_, leaf) = split(SEW_KEEP_DIR);
    sub_dir(ctx, &base, leaf, 0o700)
}

/// Keep sew-media `version`'s tarball (0600 root), then drop the oldest
/// beyond [`SEW_KEEP_MAX`].
fn keep_media(ctx: &Ctx, version: &str, tarball: &[u8]) -> HResult<J> {
    let path = kept_media_path(version).ok_or_else(|| Fail::state("keep: bad sew-media version"))?;
    let d = keep_dir(ctx)?;
    let (_, name) = split(&path);
    d.write_atomic(OsStr::new(name), tarball, 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("keep sew-media tarball", e))?;
    let mut have = kept_media_versions(ctx);
    let mut dropped = Vec::new();
    while have.len() > SEW_KEEP_MAX {
        let old = have.remove(0);
        if old != version {
            if let Some(p) = kept_media_path(&old) {
                let _ = remove_file(ctx, &p);
            }
            dropped.push(old);
        }
    }
    Ok(json!({"media": version, "dropped": dropped}))
}

/// Kept sew-media versions, oldest first.
fn kept_media_versions(ctx: &Ctx) -> Vec<String> {
    let mut v: Vec<String> = releases()
        .iter()
        .filter_map(|r| r.media.as_ref().map(|m| m.version))
        .filter(|mv| kept_media_path(mv).is_some_and(|p| exists(ctx, &p)))
        .map(str::to_string)
        .collect();
    v.sort_by(|a, b| version_cmp(a, b).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup();
    v
}

/// After a purge: the kept engine tarballs go too.
fn remove_kept(ctx: &Ctx) -> HResult<J> {
    let mut removed = Vec::new();
    for v in kept_media_versions(ctx) {
        if let Some(p) = kept_media_path(&v) {
            if remove_file(ctx, &p)? {
                removed.push(v);
            }
        }
    }
    Ok(json!({"media": removed}))
}

/// A notice step after Sew already acted: what it wrote, or `{error}`.
fn notices_or_error(r: HResult<J>) -> J {
    r.unwrap_or_else(|f| json!({"error": f.msg}))
}

fn restore(ctx: &Ctx, id: &str) -> HResult<J> {
    require_known(ctx)?;
    // Sew restores only with sew.service stopped and sew-db active; it
    // takes its own safety bundle first (K2 never passes
    // --no-safety-backup).
    ctx.run_ok(SYSTEMCTL, &["stop", UNIT_SEW], None)?;
    let res = run_sew(ctx, &["restore", id, "--replace"], None);
    // Serve again whatever the data is now (restored, or untouched).
    let started = ctx.run_ok(SYSTEMCTL, &["start", UNIT_SEW], None).is_ok();
    let mut out = res?;
    out["restarted"] = json!(started);
    Ok(out)
}

/// Write the TURN chain + key PEM text to the helper's fixed staging files
/// (0600 root) and return their real paths. Validated by the caller.
fn put_turn_tls(ctx: &Ctx, cert_pem: &str, key_pem: &str) -> HResult<(String, String)> {
    let d = stage_dir(ctx)?;
    for (abs, body) in [(SEW_STAGE_TURN_CERT, cert_pem), (SEW_STAGE_TURN_KEY, key_pem)] {
        let (_, name) = split(abs);
        d.write_atomic(OsStr::new(name), body.as_bytes(), 0o600, ctx.root_owner())
            .map_err(|e| Fail::io_err("write turn tls", e))?;
    }
    Ok((ctx.p(SEW_STAGE_TURN_CERT).display().to_string(), ctx.p(SEW_STAGE_TURN_KEY).display().to_string()))
}

fn drop_turn_tls(ctx: &Ctx) {
    let _ = remove_file(ctx, SEW_STAGE_TURN_CERT);
    let _ = remove_file(ctx, SEW_STAGE_TURN_KEY);
}

/// `sew media install --config-json <staged>` (install: the staged pin
/// tarball; reconfigure: `--reconfigure`, no tarball). The TURN key never
/// outlives the call on the helper's side.
fn media_run(ctx: &Ctx, req: &MediaRequest, tarball: Option<&str>, reconfigure: bool, restart_now: bool) -> HResult<J> {
    let turn = match (&req.turn_cert_pem, &req.turn_key_pem) {
        (Some(c), Some(k)) => Some(put_turn_tls(ctx, c, k)?),
        _ => None,
    };
    let inputs = req.sew_inputs(tarball, turn.as_ref().map(|(c, k)| (c.as_str(), k.as_str())));
    let path = match put_json(ctx, SEW_STAGE_MEDIA_JSON, &inputs) {
        Ok(p) => p,
        Err(e) => {
            drop_turn_tls(ctx);
            return Err(e);
        }
    };
    let mut args = vec!["media", "install"];
    if reconfigure {
        args.push("--reconfigure");
        if restart_now {
            args.extend(["--restart", "--now"]);
        }
    }
    args.extend(["--config-json", path.as_str()]);
    let res = run_sew(ctx, &args, None);
    let _ = remove_file(ctx, SEW_STAGE_MEDIA_JSON);
    drop_turn_tls(ctx);
    res
}

fn media_install(ctx: &Ctx, stdin: &[u8]) -> HResult<J> {
    let req: MediaRequest = parse_json(stdin, "media refused: input")?;
    req.validate().map_err(Fail::argv)?;
    require_known(ctx)?;
    let (_, art) = pinned_media_artifact(ctx.arch).map_err(Fail::state)?;
    let bytes = read_staged(ctx, SEW_STAGE_MEDIA_TARBALL)?
        .ok_or_else(|| Fail::state("no staged sew-media tarball (engine sew stage-media first)"))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(art.tarball_sha256) {
        return Err(Fail::state("the staged sew-media tarball is not this build's pin"));
    }
    let tarball = ctx.p(SEW_STAGE_MEDIA_TARBALL).display().to_string();
    let mut out = media_run(ctx, &req, Some(&tarball), false, false)?;
    // `sew media install` put sew-media in place from the pinned tarball;
    // its licence files follow it (Sew SW1 B1).
    // TODO(K-M11): when `k2 hostchat recording enable` installs
    // sew-recorder and sew-transcriber (pinned since 0.2.0, not installed
    // yet), put each one's notices from its own pinned tarball under
    // /usr/share/doc/sew-recorder and /usr/share/doc/sew-transcriber, the
    // same way, and remove them with the binaries.
    if out["sewExit"] == 0 {
        out["notices"] = notices_or_error(install_notices(ctx, &bytes, SEW_MEDIA_DOC_DIR));
        let v = pinned_media_artifact(ctx.arch).map_err(Fail::state)?.0;
        out["kept"] = notices_or_error(keep_media(ctx, v, &bytes));
    }
    Ok(out)
}

fn media_reconfigure(ctx: &Ctx, stdin: &[u8], restart_now: bool) -> HResult<J> {
    let req: MediaRequest = parse_json(stdin, "media refused: input")?;
    req.validate_reconfigure().map_err(Fail::argv)?;
    require_known(ctx)?;
    media_run(ctx, &req, None, true, restart_now)
}

/// `sew media turn-cert --cert <staged> --key <staged> --when-idle`: a
/// renewed chain (same key: Sew reloads it without an engine restart).
fn media_turn_cert(ctx: &Ctx, stdin: &[u8]) -> HResult<J> {
    let req: TurnCertRequest = parse_json(stdin, "turn-cert refused: input")?;
    req.validate().map_err(Fail::argv)?;
    require_known(ctx)?;
    let (c, k) = put_turn_tls(ctx, &req.cert_pem, &req.key_pem)?;
    let res = run_sew(ctx, &["media", "turn-cert", "--cert", &c, "--key", &k, "--when-idle"], None);
    drop_turn_tls(ctx);
    res
}
