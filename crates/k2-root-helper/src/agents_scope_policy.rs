//! `agents setup` / `agents teardown` / `agents layout`, protocol 19: the
//! bus policy and the daemon unit's `Delegate=no` drop-in
//! (`k2_root_proto::agent_scope_policy`, kernel rows 1c, 2 and P4).
//!
//! - **setup** writes `/etc/dbus-1/system.d/k2-agents.conf` for the calling
//!   daemon's user (`daemon.toml`, validated) and
//!   `/etc/systemd/system/<unit>.d/zz-k2-agents-delegate.conf` on its
//!   system unit; the caller runs one `daemon-reload` for every unit file
//!   and this module reloads the bus (`ReloadConfig` on the bus driver).
//!   No path component is followed through a link.
//! - **teardown** removes both (a link at either path is removed, never
//!   followed) and reloads what changed. It removes the policy file even
//!   when the caller's `daemon.toml` entry can't be read, so a box is never
//!   left with the denial after K2's scopes are gone.
//! - **layout** reports the two files (`present`, `missing`, `differs`) and
//!   the unit's delegation (`off`, `pending-full-restart`, `on`,
//!   `unknown`).

use std::ffi::OsStr;

use k2_root_proto::agent_scope_policy::{
    bus_policy, bus_reload_argv, classify_delegation, delegate_dropin, full_restart_command, Delegation, FileState, BUS_CONFIG_DIR,
    BUS_POLICY_DIR, BUS_POLICY_FILE, BUS_POLICY_PATH, DELEGATE_DROPIN_FILE,
};
use k2_root_proto::creds::BUSCTL;
use serde_json::json;

use crate::caller::DaemonEntry;
use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::Kind;

const POLICY_MAX: usize = 64 * 1024;

/// The bus policy text for `entry`'s user, or the refusal.
fn policy_for(entry: &DaemonEntry, what: &str) -> HResult<String> {
    bus_policy(&entry.user).ok_or_else(|| Fail::state(format!("{what} refused: the daemon's user can't be named in a bus policy")))
}

/// Open `parent/name`, made (0755) when missing; a link or a file there
/// refuses.
fn sub_dir(parent: &crate::fsx::Dir, name: &str, what: &str) -> HResult<crate::fsx::Dir> {
    let n = OsStr::new(name);
    match parent.kind(n).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => parent.mkdir(n, 0o755).map_err(|e| Fail::io_err("mkdir", e))?,
        Kind::Dir => {}
        _ => return Err(Fail::state(format!("{what} refused: {name} is not a directory"))),
    }
    parent.sub(n).map_err(|e| Fail::io_err("open", e))
}

/// What setup changed (each needs its own reload).
pub(crate) struct Installed {
    pub bus_policy: bool,
    pub delegate_dropin: bool,
}

/// Write the bus policy and the drop-in. The caller reloads systemd when
/// `delegate_dropin` changed; the bus is reloaded by [`reload_bus`].
pub(crate) fn install(ctx: &Ctx, entry: &DaemonEntry) -> HResult<Installed> {
    if entry.scope != "system" {
        return Err(Fail::state("agents setup refused: a user-scope daemon has no system unit to take delegation from"));
    }
    let text = policy_for(entry, "agents setup")?;
    // `/etc/dbus-1/system.d` (= BUS_POLICY_DIR), each level made when
    // missing (a distro that keeps its bus config under /usr/share).
    debug_assert_eq!(format!("{BUS_CONFIG_DIR}/system.d"), BUS_POLICY_DIR);
    let etc = ctx.dir("/etc").map_err(|e| Fail::io_err("open /etc", e))?;
    let dbus = sub_dir(&etc, "dbus-1", "agents setup")?;
    let sysd = sub_dir(&dbus, "system.d", "agents setup")?;
    let bus_policy = crate::apps::put(ctx, &sysd, BUS_POLICY_FILE, text.as_bytes())?;
    let delegate_dropin =
        crate::daemon_unit::set_dropin(ctx, &entry.unit, DELEGATE_DROPIN_FILE, Some(delegate_dropin().as_bytes()), "agents setup")?;
    Ok(Installed { bus_policy, delegate_dropin })
}

/// `ReloadConfig` on the system bus driver (dbus-daemon and dbus-broker
/// alike), as root.
pub(crate) fn reload_bus(ctx: &Ctx) -> HResult<()> {
    ctx.run_ok(BUSCTL, &bus_reload_argv(), None)
        .map(|_| ())
        .map_err(|f| Fail::state(format!("the system bus did not reload its policy ({})", f.msg)))
}

/// What teardown removed.
pub(crate) struct Removed {
    pub bus_policy: bool,
    pub delegate_dropin: bool,
}

/// Remove the policy file (always) and the drop-in on `unit` (a
/// system-scope entry's, when there is one).
pub(crate) fn remove(ctx: &Ctx, entry: Option<&DaemonEntry>) -> HResult<Removed> {
    let bus_policy = crate::apps::unput(ctx, BUS_POLICY_DIR, None, BUS_POLICY_FILE)?;
    let delegate_dropin = match entry.filter(|e| e.scope == "system") {
        Some(e) => crate::daemon_unit::set_dropin(ctx, &e.unit, DELEGATE_DROPIN_FILE, None, "agents teardown")?,
        None => false,
    };
    Ok(Removed { bus_policy, delegate_dropin })
}

/// The bus policy as found, against the text for `entry`'s user.
fn policy_state(ctx: &Ctx, entry: &DaemonEntry) -> FileState {
    let Some(want) = bus_policy(&entry.user) else { return FileState::Differs };
    let found = ctx.dir(BUS_POLICY_DIR).ok().and_then(|d| d.read_file(OsStr::new(BUS_POLICY_FILE), POLICY_MAX).ok().flatten());
    FileState::of(found.as_deref(), want.as_bytes())
}

fn dropin_state(ctx: &Ctx, unit: &str) -> FileState {
    match crate::daemon_unit::dropin_matches(ctx, unit, DELEGATE_DROPIN_FILE, delegate_dropin().as_bytes()) {
        Some(true) => FileState::Present,
        Some(false) => FileState::Differs,
        None => FileState::Missing,
    }
}

/// `agents layout`'s part: whether both files are in place and the unit
/// is not delegated (`off`, or `pending-full-restart`: systemd already
/// refuses moves into it, row P4), and the fields for the answer.
pub(crate) fn report(ctx: &Ctx, entry: &DaemonEntry, delegate: Option<bool>, cgroup_owner: Option<u32>) -> (bool, serde_json::Map<String, serde_json::Value>) {
    let bus = policy_state(ctx, entry);
    let dropin = dropin_state(ctx, &entry.unit);
    let delegation = classify_delegation(delegate, cgroup_owner, ctx.root_uid);
    let ready = bus == FileState::Present && dropin == FileState::Present && delegation.not_delegated();
    let mut m = serde_json::Map::new();
    m.insert("busPolicy".into(), json!(bus.as_str()));
    m.insert("busPolicyPath".into(), json!(BUS_POLICY_PATH));
    m.insert("delegateDropin".into(), json!(dropin.as_str()));
    m.insert("delegateDropinPath".into(), json!(format!("{}/{}.d/{DELEGATE_DROPIN_FILE}", k2_root_proto::paths::SYSTEMD_SYSTEM_DIR, entry.unit)));
    m.insert("delegation".into(), json!(delegation.as_str()));
    if delegation == Delegation::PendingFullRestart {
        m.insert("fullRestart".into(), json!(full_restart_command(&entry.unit)));
    }
    (ready, m)
}
