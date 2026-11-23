//! Agent Isolation M2 B4 (protocol 19, `namespaces.agentscope`): what
//! keeps systemd from moving agents' processes for the K2 user, written by
//! `agents setup` and removed by `agents teardown`
//! (`design-agent-isolation-m2-b4-nftables`, "Kernel rows run 2026-11-23").
//!
//! - **The bus policy** ([`bus_policy`], `/etc/dbus-1/system.d/k2-agents.conf`):
//!   the daemon's user may not send systemd (system bus) the calls that move
//!   existing processes into a unit. Row 1c: `AttachProcessesToUnit(<the
//!   delegated daemon unit>, "/agents/a-<B>", [own pid])` put a pid into B's
//!   root-owned cgroup. Row 2: the user's own manager, asked to start a
//!   transient scope with another K2-user pid, moved it out of a root-made
//!   system scope; the user manager does that by calling the system
//!   manager's `AttachProcessesToUnit` as that user, so the same rule
//!   closes it.
//! - **The `Delegate=no` drop-in** ([`delegate_dropin`]) on the daemon's
//!   system unit: row P4: `AttachProcessesToUnit` onto a non-delegated
//!   unit is refused at once ("Process migration not available on
//!   non-delegated units"), and after a full stop and start the unit's
//!   cgroup is root's.
//!
//! Everything here is pure: the two file texts, the bus reload argv, and
//! the delegation verdict `agents layout` reports.

use crate::caller::user_ok;

/// Where the system bus reads extra policy (both dbus-daemon and
/// dbus-broker include this folder).
pub const BUS_POLICY_DIR: &str = "/etc/dbus-1/system.d";
/// Its parent (made when missing, as is `system.d`).
pub const BUS_CONFIG_DIR: &str = "/etc/dbus-1";
pub const BUS_POLICY_FILE: &str = "k2-agents.conf";
/// `/etc/dbus-1/system.d/k2-agents.conf`.
pub const BUS_POLICY_PATH: &str = "/etc/dbus-1/system.d/k2-agents.conf";

/// The drop-in on the daemon's unit (`/etc/systemd/system/<unit>.d/…`).
/// systemd applies a unit's drop-ins in file-name order, the last one
/// winning: box prep's `k2-cores.conf` sets `Delegate=yes` and
/// `k2-delegate-subgroup.conf` sets `DelegateSubgroup=daemon`, and any name
/// starting `k2-a…` would sort before both and lose. `zz-` sorts after
/// every K2 drop-in and after `override.conf` (`systemctl edit`).
pub const DELEGATE_DROPIN_FILE: &str = "zz-k2-agents-delegate.conf";

/// The system manager's bus name and interfaces.
pub const SYSTEMD_DEST: &str = "org.freedesktop.systemd1";
pub const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";

/// Every systemd 255 method that moves existing processes into a unit
/// (`org.freedesktop.systemd1(5)`, systemd 255): the Manager's
/// `AttachProcessesToUnit(s unit, s subcgroup, au pids)`, and
/// `AttachProcesses(s subcgroup, au pids)` on each unit type that has a
/// cgroup: Service, Socket, Mount, Swap, Slice and Scope. The generic
/// `org.freedesktop.systemd1.Unit` interface has no such method.
/// `StartTransientUnit` with `PIDs=` also moves processes, but on the
/// system bus polkit refuses it to the K2 user (row P1, which `agents
/// layout` checks on every box); the bus policy leaves it alone.
pub const DENIED: &[(&str, &str)] = &[
    (MANAGER_IFACE, "AttachProcessesToUnit"),
    ("org.freedesktop.systemd1.Service", "AttachProcesses"),
    ("org.freedesktop.systemd1.Socket", "AttachProcesses"),
    ("org.freedesktop.systemd1.Mount", "AttachProcesses"),
    ("org.freedesktop.systemd1.Swap", "AttachProcesses"),
    ("org.freedesktop.systemd1.Slice", "AttachProcesses"),
    ("org.freedesktop.systemd1.Scope", "AttachProcesses"),
];

/// The same members denied by name alone: D-Bus lets a method call leave
/// its interface out, and a later systemd may add the method to another
/// unit type. A rule naming a member and no interface matches every
/// interface on both dbus-daemon and dbus-broker.
pub const DENIED_MEMBERS: &[&str] = &["AttachProcessesToUnit", "AttachProcesses"];

/// The bus policy for the daemon's user (`daemon.toml`'s, which must pass
/// [`user_ok`]: lowercase letters, digits, `_` and `-`, so nothing in it
/// needs XML escaping). `None` for any other name.
///
/// A `<policy user=…>` block is applied after every `context="default"`
/// block, so these denials win over systemd's own default allows for the
/// same members (`org.freedesktop.systemd1.conf`). D-Bus policy can't tell
/// two processes of one user apart: every process of this user is refused,
/// the daemon included (which never makes these calls: the helper, as
/// root, starts the scopes).
pub fn bus_policy(user: &str) -> Option<String> {
    if !user_ok(user) {
        return None;
    }
    let mut rules = String::new();
    for (iface, member) in DENIED {
        rules.push_str(&format!("    <deny send_destination=\"{SYSTEMD_DEST}\" send_interface=\"{iface}\" send_member=\"{member}\"/>\n"));
    }
    for member in DENIED_MEMBERS {
        rules.push_str(&format!("    <deny send_destination=\"{SYSTEMD_DEST}\" send_member=\"{member}\"/>\n"));
    }
    Some(format!(
        "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN\"\n \
         \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
         <busconfig>\n  \
         <!-- K2 agent scopes: the K2 daemon's user may not ask systemd to move processes\n       \
         between units. Written by k2-root-helper `agents setup`, removed by `agents teardown`. -->\n  \
         <policy user=\"{user}\">\n\
         {rules}  \
         </policy>\n\
         </busconfig>\n"
    ))
}

/// The daemon unit's drop-in: no delegation, and `DelegateSubgroup=`
/// cleared (an empty assignment resets it, systemd's way).
pub fn delegate_dropin() -> &'static str {
    "# K2 agent scopes (k2-root-helper `agents setup`): the daemon's unit is not delegated,\n\
     # so systemd refuses to move processes into it. Applies fully after a full stop and\n\
     # start of the unit (the daemon and k2-ptyd). Removed by `agents teardown`.\n\
     [Service]\n\
     Delegate=no\n\
     DelegateSubgroup=\n"
}

/// The `busctl` argv (after the program) that has the system bus re-read
/// its policy: `org.freedesktop.DBus.ReloadConfig` on the bus driver. Both
/// dbus-daemon and dbus-broker serve it (it is what each one's own
/// `ExecReload=` runs), whatever the bus's unit is called on this box, and
/// `busctl` waits for the answer. dbus-daemon would also notice the file by
/// itself; dbus-broker does not.
pub fn bus_reload_argv() -> [&'static str; 5] {
    ["call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig"]
}

/// One K2 file as `agents layout` finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Exactly the text this helper writes.
    Present,
    Missing,
    /// Something else at K2's path (an older text, another user's policy).
    Differs,
}

impl FileState {
    pub fn of(found: Option<&[u8]>, want: &[u8]) -> Self {
        match found {
            None => FileState::Missing,
            Some(b) if b == want => FileState::Present,
            Some(_) => FileState::Differs,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            FileState::Present => "present",
            FileState::Missing => "missing",
            FileState::Differs => "differs",
        }
    }
}

/// The daemon unit's delegation as `agents layout` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delegation {
    /// `Delegate=no` and the unit's cgroup is root's.
    Off,
    /// `Delegate=no` (the drop-in is loaded), but the unit's cgroup is
    /// still the daemon user's: k2-ptyd kept it alive through a plain
    /// restart. A full stop and start of the unit makes it root's.
    PendingFullRestart,
    /// `Delegate=yes`.
    On,
    /// systemd's `Delegate` could not be read.
    Unknown,
}

impl Delegation {
    pub fn as_str(self) -> &'static str {
        match self {
            Delegation::Off => "off",
            Delegation::PendingFullRestart => "pending-full-restart",
            Delegation::On => "on",
            Delegation::Unknown => "unknown",
        }
    }
    /// `off` or `pending-full-restart`: systemd already refuses moves into
    /// the unit (row P4), whoever owns its cgroup.
    pub fn not_delegated(self) -> bool {
        matches!(self, Delegation::Off | Delegation::PendingFullRestart)
    }
}

/// The verdict from the unit's live `Delegate` property and its cgroup's
/// owner (`root_uid`: 0 in production). An owner that can't be read is
/// not root's.
pub fn classify_delegation(delegate: Option<bool>, cgroup_owner: Option<u32>, root_uid: u32) -> Delegation {
    match delegate {
        None => Delegation::Unknown,
        Some(true) => Delegation::On,
        Some(false) if cgroup_owner == Some(root_uid) => Delegation::Off,
        Some(false) => Delegation::PendingFullRestart,
    }
}

/// The command that ends every process of the daemon's unit (the daemon
/// and k2-ptyd, so every agent session) and starts it again: a plain
/// `systemctl restart` leaves k2-ptyd, and with it the unit's cgroup, in
/// place (`KillMode=process`). `unit` must pass
/// [`crate::caller::unit_ok`]; `None` otherwise.
pub fn full_restart_command(unit: &str) -> Option<String> {
    crate::caller::unit_ok(unit)
        .then(|| format!("sudo systemctl stop {unit} && sudo systemctl kill --kill-whom=all --signal=KILL {unit}; sudo systemctl start {unit}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bus_policy_text_is_exactly_this() {
        let want = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <!-- K2 agent scopes: the K2 daemon's user may not ask systemd to move processes
       between units. Written by k2-root-helper `agents setup`, removed by `agents teardown`. -->
  <policy user="k2">
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Manager" send_member="AttachProcessesToUnit"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Service" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Socket" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Mount" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Swap" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Slice" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_interface="org.freedesktop.systemd1.Scope" send_member="AttachProcesses"/>
    <deny send_destination="org.freedesktop.systemd1" send_member="AttachProcessesToUnit"/>
    <deny send_destination="org.freedesktop.systemd1" send_member="AttachProcesses"/>
  </policy>
</busconfig>
"#;
        assert_eq!(bus_policy("k2").as_deref(), Some(want));
    }

    #[test]
    fn the_bus_policy_takes_only_a_valid_user_name() {
        for bad in ["", "K2", "k2\"/><allow send_destination=\"x\"", "k2 x", "k2&amp;", "1k2", "k2<"] {
            assert_eq!(bus_policy(bad), None, "{bad:?}");
        }
        assert!(bus_policy("k2iso").expect("k2iso").contains("<policy user=\"k2iso\">"));
    }

    #[test]
    fn the_delegate_dropin_is_exactly_this_and_sorts_after_box_preps() {
        assert_eq!(
            delegate_dropin(),
            "# K2 agent scopes (k2-root-helper `agents setup`): the daemon's unit is not delegated,\n\
             # so systemd refuses to move processes into it. Applies fully after a full stop and\n\
             # start of the unit (the daemon and k2-ptyd). Removed by `agents teardown`.\n\
             [Service]\nDelegate=no\nDelegateSubgroup=\n"
        );
        for other in [
            crate::apps::CORES_DROPIN_FILE,
            crate::apps::DAEMON_SUBGROUP_DROPIN_FILE,
            crate::paths::HANDOFF_DROPIN_FILE,
            crate::paths::FRONT_DROPIN_FILE,
            crate::paths::FRONT80_DROPIN_FILE,
            crate::apps::PROCESS_CAPS_DROPIN,
            "k2-cred-x.conf",
            "override.conf",
        ] {
            assert!(DELEGATE_DROPIN_FILE > other, "{DELEGATE_DROPIN_FILE} must sort after {other}");
        }
        assert_eq!(BUS_POLICY_PATH, format!("{BUS_POLICY_DIR}/{BUS_POLICY_FILE}"));
        assert!(BUS_POLICY_DIR.starts_with(BUS_CONFIG_DIR));
    }

    /// The B4 pre-rows script installs the same two texts on the test unit
    /// (its policy for the placeholder user `k2-row-user`, swapped for the
    /// box's user when it runs).
    #[test]
    fn the_prerows_script_installs_exactly_these_texts() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/agent-isolation-b4-prerows.sh");
        let script = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let policy = bus_policy("k2-row-user").expect("text");
        assert!(script.contains(&format!("<<'POLICY' | sed \"s/user=\\\"k2-row-user\\\"/user=\\\"$user\\\"/\"\n{policy}POLICY\n")), "the script's policy text drifted");
        assert!(script.contains(&format!("<<'DROPIN'\n{}DROPIN\n", delegate_dropin())), "the script's drop-in text drifted");
        assert!(script.contains(&format!("policy={BUS_POLICY_PATH}\n")));
        assert!(script.contains(&format!("hdropin=\"/etc/systemd/system/$unit.d/{DELEGATE_DROPIN_FILE}\"")));
        assert!(script.contains(&format!("sudo busctl {}", bus_reload_argv().join(" "))));
    }

    #[test]
    fn the_bus_reload_is_the_drivers_reload_config() {
        assert_eq!(bus_reload_argv(), ["call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig"]);
    }

    #[test]
    fn delegation_is_read_from_the_live_property_and_the_cgroups_owner() {
        assert_eq!(classify_delegation(Some(false), Some(0), 0), Delegation::Off);
        assert_eq!(classify_delegation(Some(false), Some(1001), 0), Delegation::PendingFullRestart);
        assert_eq!(classify_delegation(Some(false), None, 0), Delegation::PendingFullRestart);
        assert_eq!(classify_delegation(Some(true), Some(0), 0), Delegation::On);
        assert_eq!(classify_delegation(Some(true), Some(1001), 0), Delegation::On);
        assert_eq!(classify_delegation(None, Some(0), 0), Delegation::Unknown);
        assert!(Delegation::Off.not_delegated() && Delegation::PendingFullRestart.not_delegated());
        assert!(!Delegation::On.not_delegated() && !Delegation::Unknown.not_delegated());
        let names: Vec<&str> = [Delegation::Off, Delegation::PendingFullRestart, Delegation::On, Delegation::Unknown].iter().map(|d| d.as_str()).collect();
        assert_eq!(names, ["off", "pending-full-restart", "on", "unknown"]);
    }

    #[test]
    fn file_states_and_the_full_restart_command() {
        assert_eq!(FileState::of(None, b"x"), FileState::Missing);
        assert_eq!(FileState::of(Some(b"x"), b"x"), FileState::Present);
        assert_eq!(FileState::of(Some(b"y"), b"x"), FileState::Differs);
        assert_eq!(
            full_restart_command("k2-daemon.service").as_deref(),
            Some("sudo systemctl stop k2-daemon.service && sudo systemctl kill --kill-whom=all --signal=KILL k2-daemon.service; sudo systemctl start k2-daemon.service")
        );
        assert_eq!(full_restart_command("x; rm -rf /"), None);
    }
}
