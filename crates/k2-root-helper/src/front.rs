//! `front` namespace (protocol 8, 0.46.1 front443): the box's :443,
//! held by systemd for the K2 daemon's SNI router.
//!
//! - `front install` (daemon): writes the canned socket unit
//!   `/etc/systemd/system/k2-front-443.socket`
//!   ([`k2_root_proto::templates::front_socket_unit`]: `ListenStream=[::]:443`,
//!   `Service=` the caller's own unit) and the canned daemon drop-in
//!   `<unit>.d/k2-front.conf` (`Sockets=`) and reloads systemd. It does NOT
//!   start the socket: systemd refuses to start a socket whose `Service=` is
//!   already active ("Socket service k2-daemon.service already active,
//!   refusing."), and the caller is always that live service. The socket
//!   starts with the daemon's next start (the drop-in's `Wants=`/`After=`;
//!   checked on systemd 255: a clean exit under `Restart=always`, or a
//!   `systemctl restart`, starts the socket first and passes its fd). If
//!   :443 is already taken, install refuses before writing anything and
//!   names the listener.
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
//! The daemon restarts once to receive the fd (`LISTEN_FDS`): `install`
//! schedules `systemctl restart <the caller's unit>` a few seconds out
//! (`systemd-run --on-active`, a transient `k2-front-restart` timer), so
//! the daemon's answer goes out first and the restart doesn't depend on the
//! unit's `Restart=` policy. systemd then stops the daemon, starts the
//! socket and starts the daemon with the fd. A hand-off can't carry it: the
//! successor is started by the daemon, not by systemd, and systemd only
//! passes a socket to a service it starts.

use std::ffi::OsStr;

use k2_root_proto::paths::{
    FRONT_DROPIN_FILE, FRONT_RESTART_DELAY_SECS, FRONT_RESTART_UNIT, FRONT_SOCKET_FILE, FRONT_SOCKET_UNIT, JOURNALCTL,
    SYSTEMCTL, SYSTEMD_RUN, SYSTEMD_SYSTEM_DIR,
};
use k2_root_proto::templates::{front_dropin, front_socket_unit};

use crate::caller::{self, Caller, DaemonEntry};
use crate::ctx::{Ctx, Fail, HResult};
use crate::daemon_unit::{caller_entry, dropin_matches, set_dropin};
use crate::fsx::Kind;

/// Largest socket unit file the helper reads back.
const UNIT_READ_CAP: usize = 16 * 1024;
/// The front's port.
const FRONT_PORT: u16 = 443;

/// Local addresses in LISTEN state on `port` in a `/proc/net/tcp` or
/// `/proc/net/tcp6` table, with their socket inodes. Pure.
pub(crate) fn listeners_on(table: &str, port: u16) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for line in table.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[3] != "0A" {
            continue;
        }
        let Some((addr, p)) = f[1].split_once(':') else { continue };
        if u16::from_str_radix(p, 16).ok() != Some(port) {
            continue;
        }
        let ip = match addr.len() {
            8 => u32::from_str_radix(addr, 16)
                .ok()
                .map(|v| std::net::Ipv4Addr::from(v.swap_bytes()).to_string()),
            32 => {
                let mut b = [0u8; 16];
                let mut ok = true;
                for (i, chunk) in addr.as_bytes().chunks(8).enumerate() {
                    let w = std::str::from_utf8(chunk).ok().and_then(|c| u32::from_str_radix(c, 16).ok());
                    match w {
                        Some(w) => b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes()),
                        None => ok = false,
                    }
                }
                ok.then(|| format!("[{}]", std::net::Ipv6Addr::from(b)))
            }
            _ => None,
        };
        if let Some(ip) = ip {
            out.push((format!("{ip}:{port}"), f[9].parse().unwrap_or(0)));
        }
    }
    out
}

/// The process holding socket `inode` (`comm[pid]`), when /proc says.
fn socket_owner(ctx: &Ctx, inode: u64) -> Option<String> {
    if inode == 0 {
        return None;
    }
    let want = format!("socket:[{inode}]");
    for e in std::fs::read_dir(ctx.p("/proc")).ok()?.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { continue };
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).ok().is_some_and(|l| l.as_os_str() == want.as_str()) {
                let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
                let comm: String = comm.trim().chars().filter(|c| !c.is_control()).take(32).collect();
                return Some(format!("{comm}[{pid}]"));
            }
        }
    }
    None
}

/// Who already listens on :443 (empty when it is free).
fn port_443_taken(ctx: &Ctx) -> Vec<String> {
    let mut who = Vec::new();
    for t in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(ctx.p(t)) else { continue };
        for (addr, inode) in listeners_on(&text, FRONT_PORT) {
            who.push(match socket_owner(ctx, inode) {
                Some(o) => format!("{addr} ({o})"),
                None => addr,
            });
        }
    }
    who
}

/// `systemctl show` state of the socket unit, plus systemd's own last log
/// lines for it (why it did or didn't start), trimmed.
fn socket_unit_report(ctx: &Ctx) -> serde_json::Value {
    let run = |prog: &str, args: &[&str]| -> String {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        ctx.exec
            .run(prog, &a, None)
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let show = run(SYSTEMCTL, &["show", "-p", "ActiveState", "-p", "SubState", "-p", "Result", FRONT_SOCKET_UNIT]);
    let mut state = serde_json::Map::new();
    for line in show.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if matches!(k, "ActiveState" | "SubState" | "Result") {
                state.insert(k.to_string(), serde_json::json!(v.chars().take(32).collect::<String>()));
            }
        }
    }
    let journal: Vec<String> = run(JOURNALCTL, &["-u", FRONT_SOCKET_UNIT, "-n", "6", "-o", "cat", "--no-pager"])
        .lines()
        .map(|l| l.chars().filter(|c| !c.is_control()).take(200).collect())
        .filter(|l: &String| !l.is_empty())
        .collect();
    serde_json::json!({"state": state, "journal": journal})
}

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
    // :443 must be free, unless our socket already holds it (a re-run).
    let socket_active = is_active(ctx);
    if !socket_active {
        let who = port_443_taken(ctx);
        if !who.is_empty() {
            return Err(Fail::state(format!(
                "front refused: port 443 is already in use on this box by {} — stop it or move it behind the \
                 front (hosted mail on :443: k2 front enable --mail)",
                who.join(", ")
            )));
        }
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
    // Never `systemctl start` the socket here: its service (the caller) is
    // active, and systemd refuses that. A left-over failed state would
    // block the start at the daemon's restart, so clear it, then schedule
    // the one restart that starts the socket and hands the daemon its fd.
    let mut restart_scheduled = false;
    if !socket_active {
        let _ = ctx.exec.run(SYSTEMCTL, &["reset-failed".to_string(), FRONT_SOCKET_UNIT.to_string()], None);
        restart_scheduled = schedule_restart(ctx, &d.unit)?;
    }
    Ok(serde_json::json!({
        "unit": d.unit,
        "socket": FRONT_SOCKET_UNIT,
        "installed": true,
        "changed": changed,
        "active": socket_active,
        "restartScheduled": restart_scheduled,
        "restartInSecs": restart_scheduled.then_some(FRONT_RESTART_DELAY_SECS),
    }))
}

/// `systemd-run --unit k2-front-restart --on-active=<delay>s --collect
/// systemctl restart <unit>`: a transient timer, so the daemon (the
/// caller) answers before systemd restarts it. `unit` is the daemon.toml
/// entry's (validated by `unit_ok`); nothing else comes from the caller.
fn schedule_restart(ctx: &Ctx, unit: &str) -> HResult<bool> {
    // A timer left from an earlier install would make the name clash.
    let _ = ctx.exec.run(SYSTEMCTL, &["stop".to_string(), format!("{FRONT_RESTART_UNIT}.timer")], None);
    let _ = ctx.exec.run(SYSTEMCTL, &["reset-failed".to_string(), format!("{FRONT_RESTART_UNIT}.service")], None);
    let delay = format!("--on-active={FRONT_RESTART_DELAY_SECS}");
    let name = format!("--unit={FRONT_RESTART_UNIT}");
    ctx.run_ok(
        SYSTEMD_RUN,
        &[&name, &delay, "--timer-property=AccuracySec=100ms", "--collect", "--quiet", SYSTEMCTL, "restart", unit],
        None,
    )
    .map_err(|f| {
        Fail::state(format!(
            "front installed, but the daemon's restart could not be scheduled ({}): run `systemctl restart {unit}` as root",
            f.msg
        ))
    })?;
    Ok(true)
}

fn is_active(ctx: &Ctx) -> bool {
    let args = vec!["is-active".to_string(), FRONT_SOCKET_UNIT.to_string()];
    ctx.exec
        .run(SYSTEMCTL, &args, None)
        .is_ok_and(|o| o.status == 0 && String::from_utf8_lossy(&o.stdout).trim() == "active")
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
        "unit": if cur == SocketFile::Missing { serde_json::Value::Null } else { socket_unit_report(ctx) },
    }))
}
