//! `desktop packages install` (protocol 16, `namespaces.desktop`): the
//! distro packages a Linux server needs for K2's virtual desktop
//! (`prd-linux-virtual-desktop-v1` §2.6, slice L5; LD-13, LD-14).
//!
//! The verb takes no package names. The helper picks one closed list by
//! package manager from [`DESKTOP_PACKAGES`], simulates the install first
//! and refuses one that would remove anything. The names were checked
//! with a simulated install on Ubuntu 24.04 (apt) and Arch (pacman) in L0
//! (11-11), and the Kessel Steward approved them (R32). Fedora (dnf) has
//! no list until its names are checked: a Fedora server gets the manual
//! text and the helper installs nothing there.
//!
//! Licences (Kessel Steward ruling R32): Xvfb (MIT/X11), IceWM (LGPL-2)
//! and the DejaVu fonts come from the distro's own repositories and run
//! as separate programs; K2 never links, bundles, copies or patches them.
//! Any other package goes back to the Steward first.

/// The list for each package manager K2 knows, in the order K2 asks for
/// them. The one place to change a name.
pub const DESKTOP_PACKAGES: &[(&str, &[&str])] = &[
    // Ubuntu 24.04 / Debian. On Ubuntu `xvfb` pulls `x11-xkb-utils`
    // (xkbcomp) and `xserver-common`, and IceWM's imlib2 pulls the
    // Ghostscript libraries; `dbus-run-session` comes with `dbus-daemon`,
    // which every systemd box has. xauth is only recommended: K2 writes
    // the cookie file itself.
    ("apt", &["xvfb", "icewm", "fonts-dejavu-core"]),
    // Arch and Omarchy.
    ("pacman", &["xorg-server-xvfb", "icewm", "ttf-dejavu"]),
];

/// The fixed list for a package manager (`apt`, `pacman`), or `None` for
/// one K2 has no list for (`dnf` too, until Fedora's names are checked).
pub fn packages_for(pm: &str) -> Option<&'static [&'static str]> {
    DESKTOP_PACKAGES.iter().find(|(p, _)| *p == pm).map(|(_, l)| *l)
}

/// Free space the install needs on `/` and `/var` first. L0: about
/// 70 MB on a box that already has Mesa, about 300 MB on a bare Ubuntu
/// server, plus apt's lists; the rest keeps a full box out of ENOSPC.
pub const DESKTOP_MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How long apt waits for the dpkg lock (unattended-upgrades on a fresh
/// Ubuntu box, or another helper call) before it gives up, in seconds.
pub const APT_LOCK_WAIT_SECS: u32 = 120;

/// What the desktop runs, checked after the install.
pub const XVFB: &str = "/usr/bin/Xvfb";
pub const ICEWM_SESSION: &str = "/usr/bin/icewm-session";
/// The fallback window manager when IceWM is missing and openbox is there
/// (§2.2). K2 never installs it.
pub const OPENBOX: &str = "/usr/bin/openbox";

/// The packages an `apt-get install -s` answer would remove (`Remv <name>
/// …` lines), arch suffix dropped. Pure.
pub fn apt_sim_removals(sim: &str) -> Vec<String> {
    sim.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            match (w.next(), w.next()) {
                (Some("Remv"), Some(p)) => Some(p.split(':').next().unwrap_or(p).to_string()),
                _ => None,
            }
        })
        .collect()
}

/// The packages an `apt-get install -s` answer would add or upgrade
/// (`Inst <name> …` lines). Pure.
pub fn apt_sim_installs(sim: &str) -> Vec<String> {
    sim.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            match (w.next(), w.next()) {
                (Some("Inst"), Some(p)) => Some(p.split(':').next().unwrap_or(p).to_string()),
                _ => None,
            }
        })
        .collect()
}

/// The command a person runs when K2 can't install the packages itself
/// (no root helper, or a package manager K2 has no list for). `pm` is the
/// box's package manager (`apt`, `pacman`, `dnf`), if K2 found one. Facts
/// only; nothing is run.
pub fn manual_command(pm: Option<&str>) -> String {
    let Some((pm, list)) = pm.and_then(|p| packages_for(p).map(|l| (p, l))) else {
        return "K2 has no package list for this computer's package manager. Install Xvfb, IceWM and a DejaVu font with it, then try again.".to_string();
    };
    let names = list.join(" ");
    match pm {
        "apt" => format!("sudo apt-get update && sudo apt-get install --no-install-recommends {names}"),
        "pacman" => format!("sudo pacman -S --needed {names}"),
        _ => unreachable!("every DESKTOP_PACKAGES entry has a command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_package_manager_has_one_closed_list() {
        assert_eq!(packages_for("apt"), Some(&["xvfb", "icewm", "fonts-dejavu-core"][..]));
        assert_eq!(packages_for("pacman"), Some(&["xorg-server-xvfb", "icewm", "ttf-dejavu"][..]));
        assert_eq!(packages_for("dnf"), None, "Fedora's names are unchecked: no list, no install");
        assert_eq!(packages_for("zypper"), None);
        assert_eq!(packages_for(""), None);
        // No list asks for a terminal, a browser or anything outside R32.
        for (_, list) in DESKTOP_PACKAGES {
            for p in *list {
                assert!(!["xterm", "chromium", "firefox", "openbox"].contains(p), "{p}");
            }
        }
    }

    #[test]
    fn the_manual_command_names_the_listed_packages_and_nothing_else() {
        assert_eq!(
            manual_command(Some("apt")),
            "sudo apt-get update && sudo apt-get install --no-install-recommends xvfb icewm fonts-dejavu-core"
        );
        assert_eq!(manual_command(Some("pacman")), "sudo pacman -S --needed xorg-server-xvfb icewm ttf-dejavu");
        for unknown in [None, Some("dnf"), Some("zypper"), Some("apk")] {
            let m = manual_command(unknown);
            assert!(m.contains("Install Xvfb, IceWM and a DejaVu font"), "{m}");
            assert!(!m.contains("sudo"), "no guessed command: {m}");
        }
    }

    #[test]
    fn apt_simulation_lines_are_read_by_name() {
        let sim = "NOTE: This is only a simulation!\n\
                   Inst x11-xkb-utils (7.7+8build2 Ubuntu:24.04/noble [amd64])\n\
                   Inst xvfb (2:21.1.12-1ubuntu1.4 Ubuntu:24.04/noble-updates [amd64])\n\
                   Remv xserver-xephyr:amd64 [2:21.1.12-1ubuntu1.4]\n\
                   Conf xvfb (2:21.1.12-1ubuntu1.4 Ubuntu:24.04/noble-updates [amd64])\n";
        assert_eq!(apt_sim_installs(sim), vec!["x11-xkb-utils", "xvfb"]);
        assert_eq!(apt_sim_removals(sim), vec!["xserver-xephyr"]);
        assert!(apt_sim_removals("Inst xvfb (1)\nConf xvfb (1)\n").is_empty());
    }
}
