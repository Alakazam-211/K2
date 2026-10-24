//! `daemon` namespace (protocol 5, runtime stage H): the K2 daemon's own
//! systemd unit.
//!
//! `daemon handoff-dropin install|remove` writes (or removes)
//! `/etc/systemd/system/<unit>.d/k2-handoff.conf` — the canned
//! [`k2_root_proto::templates::handoff_dropin`] bytes — then runs
//! `systemctl daemon-reload`.
//!
//! The unit is never taken from argv: it is the `daemon.toml` entry whose
//! main process is the caller (the verb is `daemon` class, so the caller
//! walk has already proven that). A user-scope entry is refused: the
//! daemon writes its own user-unit drop-in under `~/.config` without root.
//! Every path component is opened without following links; a symlinked or
//! non-directory `.d` refuses with nothing written.
//!
//! [`set_dropin`] is shared with the `front` namespace (front443's
//! `k2-front.conf`, protocol 8).

use std::ffi::OsStr;

use k2_root_proto::paths::{HANDOFF_DROPIN_FILE, SYSTEMCTL, SYSTEMD_SYSTEM_DIR};
use k2_root_proto::templates::handoff_dropin;

use crate::caller::{self, Caller, DaemonEntry};
use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::Kind;

/// The caller's own `daemon.toml` entry (the one whose unit's main
/// process it is).
pub(crate) fn caller_entry(ctx: &Ctx, who: &Caller) -> HResult<DaemonEntry> {
    let user = caller::require_daemon(who)?;
    let daemons = caller::load_daemons(ctx).map_err(Fail::state)?;
    daemons
        .into_iter()
        .find(|d| d.user == user && caller::main_pid(ctx, d) == Some(who.pid))
        .ok_or_else(Fail::not_daemon)
}

/// Write (`Some(bytes)`) or remove (`None`) one canned drop-in file
/// `/etc/systemd/system/<unit>.d/<file>`. `unit` comes from `daemon.toml`
/// (already validated by `unit_ok`: no `/`, no `..`, ends in `.service`)
/// or, for `creds bind`, from argv validated by
/// `k2_root_proto::creds::unit_name_ok` (same alphabet, ends in
/// `.service`); `file` is a fixed name from `k2_root_proto::paths` or
/// `k2-cred-<validated cred>.conf`. No link is ever
/// followed: a symlinked or non-directory `.d`, or a non-regular file,
/// refuses with nothing written. `Ok(changed)`; never reloads systemd
/// (the caller does, once).
pub(crate) fn set_dropin(
    ctx: &Ctx,
    unit: &str,
    file: &str,
    bytes: Option<&[u8]>,
    what: &str,
) -> HResult<bool> {
    let base = ctx
        .dir(SYSTEMD_SYSTEM_DIR)
        .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    let dname = format!("{unit}.d");
    let dname = OsStr::new(&dname);
    let fname = OsStr::new(file);
    let not_dir = || Fail::state(format!("{what} refused: the unit's drop-in dir is not a directory"));
    let not_file = || Fail::state(format!("{what} refused: {file} is not a regular file"));
    match bytes {
        Some(want) => {
            match base.kind(dname).map_err(|e| Fail::io_err("stat", e))? {
                Kind::Missing => base
                    .mkdir(dname, 0o755)
                    .map_err(|e| Fail::io_err("mkdir drop-in dir", e))?,
                Kind::Dir => {}
                _ => return Err(not_dir()),
            }
            let dir = base
                .sub(dname)
                .map_err(|e| Fail::io_err("open drop-in dir", e))?;
            match dir.kind(fname).map_err(|e| Fail::io_err("stat", e))? {
                Kind::Missing | Kind::File => {}
                _ => return Err(not_file()),
            }
            let same = dir
                .read_file(fname, 64 * 1024)
                .map_err(|e| Fail::io_err("read drop-in", e))?
                .is_some_and(|b| b == want);
            if !same {
                dir.write_atomic(fname, want, 0o644, ctx.root_owner())
                    .map_err(|e| Fail::io_err("write drop-in", e))?;
            }
            Ok(!same)
        }
        None => match base.kind(dname).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing => Ok(false),
            Kind::Dir => {
                let dir = base
                    .sub(dname)
                    .map_err(|e| Fail::io_err("open drop-in dir", e))?;
                match dir.kind(fname).map_err(|e| Fail::io_err("stat", e))? {
                    Kind::Missing => Ok(false),
                    Kind::File | Kind::Symlink => {
                        dir.unlink(fname).map_err(|e| Fail::io_err("remove drop-in", e))?;
                        Ok(true)
                    }
                    _ => Err(not_file()),
                }
            }
            _ => Err(not_dir()),
        },
    }
}

/// Read one drop-in for status: `Some(true)` = the canned bytes,
/// `Some(false)` = something else, `None` = absent (or unreadable).
pub(crate) fn dropin_matches(ctx: &Ctx, unit: &str, file: &str, want: &[u8]) -> Option<bool> {
    let base = ctx.dir(SYSTEMD_SYSTEM_DIR).ok()?;
    let dname = format!("{unit}.d");
    let dir = base.sub(OsStr::new(&dname)).ok()?;
    let b = dir.read_file(OsStr::new(file), 64 * 1024).ok()??;
    Some(b == want)
}

pub fn handoff_dropin_set(ctx: &Ctx, who: &Caller, install: bool) -> HResult<serde_json::Value> {
    let d = caller_entry(ctx, who)?;
    if d.scope != "system" {
        return Err(Fail::state(
            "user-scope unit: the daemon writes its own drop-in (no root needed)",
        ));
    }
    let bytes = install.then(|| handoff_dropin().as_bytes());
    let changed = set_dropin(ctx, &d.unit, HANDOFF_DROPIN_FILE, bytes, "handoff-dropin")?;
    if changed {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(serde_json::json!({
        "unit": d.unit,
        "installed": install,
        "changed": changed,
    }))
}
