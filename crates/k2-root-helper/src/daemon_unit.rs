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

pub fn handoff_dropin_set(ctx: &Ctx, who: &Caller, install: bool) -> HResult<serde_json::Value> {
    let d = caller_entry(ctx, who)?;
    if d.scope != "system" {
        return Err(Fail::state(
            "user-scope unit: the daemon writes its own drop-in (no root needed)",
        ));
    }
    let base = ctx
        .dir(SYSTEMD_SYSTEM_DIR)
        .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    // `daemon.toml` already validated the unit name (`unit_ok`): no `/`,
    // no `..`, ends in `.service`.
    let dname = format!("{}.d", d.unit);
    let dname = OsStr::new(&dname);
    let file = OsStr::new(HANDOFF_DROPIN_FILE);
    let changed = if install {
        match base.kind(dname).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing => base
                .mkdir(dname, 0o755)
                .map_err(|e| Fail::io_err("mkdir drop-in dir", e))?,
            Kind::Dir => {}
            _ => {
                return Err(Fail::state(
                    "handoff-dropin refused: the unit's drop-in dir is not a directory",
                ))
            }
        }
        let dir = base
            .sub(dname)
            .map_err(|e| Fail::io_err("open drop-in dir", e))?;
        match dir.kind(file).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing | Kind::File => {}
            _ => {
                return Err(Fail::state(
                    "handoff-dropin refused: k2-handoff.conf is not a regular file",
                ))
            }
        }
        let same = dir
            .read_file(file, 64 * 1024)
            .map_err(|e| Fail::io_err("read drop-in", e))?
            .is_some_and(|b| b == handoff_dropin().as_bytes());
        if !same {
            dir.write_atomic(file, handoff_dropin().as_bytes(), 0o644, ctx.root_owner())
                .map_err(|e| Fail::io_err("write drop-in", e))?;
        }
        !same
    } else {
        match base.kind(dname).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Missing => false,
            Kind::Dir => {
                let dir = base
                    .sub(dname)
                    .map_err(|e| Fail::io_err("open drop-in dir", e))?;
                match dir.kind(file).map_err(|e| Fail::io_err("stat", e))? {
                    Kind::Missing => false,
                    Kind::File | Kind::Symlink => {
                        dir.unlink(file).map_err(|e| Fail::io_err("remove drop-in", e))?;
                        true
                    }
                    _ => {
                        return Err(Fail::state(
                            "handoff-dropin refused: k2-handoff.conf is not a regular file",
                        ))
                    }
                }
            }
            _ => {
                return Err(Fail::state(
                    "handoff-dropin refused: the unit's drop-in dir is not a directory",
                ))
            }
        }
    };
    if changed {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(serde_json::json!({
        "unit": d.unit,
        "installed": install,
        "changed": changed,
    }))
}
