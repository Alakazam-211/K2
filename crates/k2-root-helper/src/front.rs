//! `front` namespace (protocol 8, 0.46.1 front443): the box's :443,
//! held by systemd for the K2 daemon's SNI router.
//!
//! - `front install` (daemon): writes the canned socket unit
//!   `/etc/systemd/system/k2-front-443.socket`
//!   ([`k2_root_proto::templates::front_socket_unit`]: `ListenStream=[::]:443`,
//!   `Service=` the caller's own unit) and the canned daemon drop-in
//!   `<unit>.d/k2-front.conf` (`Sockets=`), reloads systemd and starts the
//!   socket. A socket that cannot start (something else holds :443) is
//!   rolled back: nothing stays installed.
//! - `front remove` (daemon): stops the socket, removes both files,
//!   reloads systemd.
//! - `front status` (open): read-only.
//!
//! What the helper never takes from the caller: the unit (it is the
//! `daemon.toml` entry whose main process called, exactly as `daemon
//! handoff-dropin`), the port, the address, a path, or any unit text. The
//! two files are fixed names under `/etc/systemd/system`, opened without
//! following links, written atomically root:root 0644. A socket unit that
//! already serves ANOTHER configured daemon's unit is refused, never taken
//! over. User-scope daemons are refused (a user manager can't bind :443).
//!
//! The daemon still has to restart once to receive the fd
//! (`LISTEN_FDS`); it does that itself after `install` returns (a plain
//! supervisor restart, `Restart=always`).

use std::ffi::OsStr;

use k2_root_proto::paths::{FRONT_DROPIN_FILE, FRONT_SOCKET_FILE, FRONT_SOCKET_UNIT, SYSTEMCTL, SYSTEMD_SYSTEM_DIR};
use k2_root_proto::templates::{front_dropin, front_socket_unit};

use crate::caller::{self, Caller, DaemonEntry};
use crate::ctx::{Ctx, Fail, HResult};
use crate::daemon_unit::{caller_entry, dropin_matches, set_dropin};
use crate::fsx::Kind;

/// Largest socket unit file the helper reads back.
const UNIT_READ_CAP: usize = 16 * 1024;

/// What `/etc/systemd/system/k2-front-443.socket` holds now.
#[derive(Debug, PartialEq, Eq)]
enum SocketFile {
    Missing,
    /// A regular file; `service` is its `Service=` value, if any.
    File { bytes: Vec<u8>, service: Option<String> },
    /// A symlink, directory, fifo…: never read, never written through.
    Odd,
}

fn read_socket_file(ctx: &Ctx) -> HResult<SocketFile> {
    let base = ctx
        .dir(SYSTEMD_SYSTEM_DIR)
        .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    let name = OsStr::new(FRONT_SOCKET_FILE);
    match base.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(SocketFile::Missing),
        Kind::File => {
            let bytes = base
                .read_file(name, UNIT_READ_CAP)
                .map_err(|e| Fail::io_err("read socket unit", e))?
                .unwrap_or_default();
            let service = String::from_utf8_lossy(&bytes)
                .lines()
                .find_map(|l| l.trim().strip_prefix("Service=").map(|s| s.trim().to_string()));
            Ok(SocketFile::File { bytes, service })
        }
        _ => Ok(SocketFile::Odd),
    }
}

fn system_entry(ctx: &Ctx, who: &Caller) -> HResult<DaemonEntry> {
    let d = caller_entry(ctx, who)?;
    if d.scope != "system" {
        return Err(Fail::state(
            "front refused: a user-scope daemon can't hold :443 (system units only)",
        ));
    }
    Ok(d)
}

/// The socket unit may be (re)written for `unit` only when it is absent,
/// already ours, or names a unit no configured daemon owns (a leftover).
fn refuse_foreign(ctx: &Ctx, cur: &SocketFile, unit: &str) -> HResult<()> {
    match cur {
        SocketFile::Odd => Err(Fail::state("front refused: k2-front-443.socket is not a regular file")),
        SocketFile::File { service: Some(s), .. } if s != unit => {
            let owned = caller::load_daemons(ctx)
                .map(|ds| ds.iter().any(|d| d.unit == *s))
                .unwrap_or(true);
            if owned {
                Err(Fail::state("front refused: the :443 front is held for another K2 daemon on this box"))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn reload(ctx: &Ctx) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None).map(|_| ())
}

fn remove_socket_file(ctx: &Ctx) -> HResult<bool> {
    let base = ctx
        .dir(SYSTEMD_SYSTEM_DIR)
        .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    let name = OsStr::new(FRONT_SOCKET_FILE);
    match base.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => Ok(false),
        Kind::File | Kind::Symlink => {
            base.unlink(name).map_err(|e| Fail::io_err("remove socket unit", e))?;
            Ok(true)
        }
        _ => Err(Fail::state("front refused: k2-front-443.socket is not a regular file")),
    }
}

pub fn install(ctx: &Ctx, who: &Caller) -> HResult<serde_json::Value> {
    let d = system_entry(ctx, who)?;
    let want = front_socket_unit(&d.unit).ok_or_else(|| Fail::state("daemon.toml unit invalid"))?;
    let cur = read_socket_file(ctx)?;
    refuse_foreign(ctx, &cur, &d.unit)?;
    // Another configured daemon's `Sockets=` drop-in still pulls the socket
    // in (its socket file gone, say): two daemons would accept on :443.
    let others = caller::load_daemons(ctx).map_err(Fail::state)?;
    if others
        .iter()
        .filter(|o| o.scope == "system" && o.unit != d.unit)
        .any(|o| dropin_matches(ctx, &o.unit, FRONT_DROPIN_FILE, front_dropin().as_bytes()).is_some())
    {
        return Err(Fail::state("front refused: another K2 daemon's unit still carries the front drop-in"));
    }
    let socket_changed = match &cur {
        SocketFile::File { bytes, .. } if bytes == want.as_bytes() => false,
        _ => {
            let base = ctx
                .dir(SYSTEMD_SYSTEM_DIR)
                .map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
            base.write_atomic(OsStr::new(FRONT_SOCKET_FILE), want.as_bytes(), 0o644, ctx.root_owner())
                .map_err(|e| Fail::io_err("write socket unit", e))?;
            true
        }
    };
    let dropin_changed = match set_dropin(ctx, &d.unit, FRONT_DROPIN_FILE, Some(front_dropin().as_bytes()), "front") {
        Ok(c) => c,
        Err(f) => {
            if socket_changed {
                let _ = remove_socket_file(ctx);
            }
            return Err(f);
        }
    };
    let changed = socket_changed || dropin_changed;
    if changed {
        if let Err(f) = reload(ctx) {
            if socket_changed {
                let _ = remove_socket_file(ctx);
            }
            if dropin_changed {
                let _ = set_dropin(ctx, &d.unit, FRONT_DROPIN_FILE, None, "front");
            }
            let _ = reload(ctx);
            return Err(f);
        }
    }
    // Listen now. Something else on :443 (Stalwart's tls-alpn listener, a
    // Caddy, nginx) makes this fail: undo the files so a half-installed
    // front never pulls a failing socket into the daemon's next start.
    if let Err(f) = ctx.run_ok(SYSTEMCTL, &["start", FRONT_SOCKET_UNIT], None) {
        let _ = ctx.run_ok(SYSTEMCTL, &["stop", FRONT_SOCKET_UNIT], None);
        let removed_socket = remove_socket_file(ctx).unwrap_or(false);
        let removed_dropin = set_dropin(ctx, &d.unit, FRONT_DROPIN_FILE, None, "front").unwrap_or(false);
        if removed_socket || removed_dropin {
            let _ = reload(ctx);
        }
        let _ = ctx.run_ok(SYSTEMCTL, &["reset-failed", FRONT_SOCKET_UNIT], None);
        return Err(Fail::state(format!(
            "front refused: :443 could not be taken (is something else listening on it?) — {}",
            f.msg
        )));
    }
    Ok(serde_json::json!({
        "unit": d.unit,
        "socket": FRONT_SOCKET_UNIT,
        "installed": true,
        "changed": changed,
        "active": true,
    }))
}

pub fn remove(ctx: &Ctx, who: &Caller) -> HResult<serde_json::Value> {
    let d = system_entry(ctx, who)?;
    let cur = read_socket_file(ctx)?;
    refuse_foreign(ctx, &cur, &d.unit)?;
    if cur != SocketFile::Missing {
        // 5 = not loaded (a file systemd never reloaded): nothing to stop.
        let args: Vec<String> = ["stop", FRONT_SOCKET_UNIT].iter().map(|s| s.to_string()).collect();
        let out = ctx
            .exec
            .run(SYSTEMCTL, &args, None)
            .map_err(|e| Fail::io(format!("{SYSTEMCTL}: {e}")))?;
        if out.status != 0 && out.status != 5 {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(Fail::io(format!("{SYSTEMCTL} stop exit {}: {}", out.status, err.trim())));
        }
    }
    let socket_removed = remove_socket_file(ctx)?;
    let dropin_removed = set_dropin(ctx, &d.unit, FRONT_DROPIN_FILE, None, "front")?;
    let changed = socket_removed || dropin_removed;
    if changed {
        reload(ctx)?;
    }
    Ok(serde_json::json!({
        "unit": d.unit,
        "socket": FRONT_SOCKET_UNIT,
        "installed": false,
        "changed": changed,
    }))
}

pub fn status(ctx: &Ctx) -> HResult<serde_json::Value> {
    let daemons = caller::load_daemons(ctx).unwrap_or_default();
    let cur = read_socket_file(ctx)?;
    let (socket, held_for) = match &cur {
        SocketFile::Missing => ("missing", None),
        SocketFile::Odd => ("not-a-file", None),
        SocketFile::File { bytes, service } => {
            let canned = service
                .as_deref()
                .and_then(front_socket_unit)
                .is_some_and(|w| w.as_bytes() == bytes.as_slice());
            (if canned { "canned" } else { "other" }, service.as_ref().map(|s| s.chars().take(80).collect::<String>()))
        }
    };
    let active = if cur == SocketFile::Missing {
        "inactive".to_string()
    } else {
        let args = vec!["is-active".to_string(), FRONT_SOCKET_UNIT.to_string()];
        ctx.exec
            .run(SYSTEMCTL, &args, None)
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().chars().take(32).collect())
            .unwrap_or_else(|_| "unknown".into())
    };
    let dropins: Vec<serde_json::Value> = daemons
        .iter()
        .filter(|d| d.scope == "system")
        .map(|d| {
            let m = dropin_matches(ctx, &d.unit, FRONT_DROPIN_FILE, front_dropin().as_bytes());
            serde_json::json!({
                "unit": d.unit,
                "dropin": match m { None => "missing", Some(true) => "canned", Some(false) => "other" },
            })
        })
        .collect();
    Ok(serde_json::json!({
        "socket": FRONT_SOCKET_UNIT,
        "socketFile": socket,
        "heldFor": held_for,
        "active": active,
        "dropins": dropins,
    }))
}
