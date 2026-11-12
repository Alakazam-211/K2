//! `desktop packages install` (protocol 16, `namespaces.desktop`): the
//! Linux virtual desktop's distro packages (`prd-linux-virtual-desktop-v1`
//! §2.6, L5; LD-13). Daemon-only: the daemon asks on an Owner's yes.
//!
//! - The names come from [`k2_root_proto::desktop::DESKTOP_PACKAGES`],
//!   picked by the box's package manager; argv carries none.
//! - apt: `--no-install-recommends --no-upgrade`, simulated first; any
//!   `Remv` line is a refusal and nothing is installed.
//! - pacman: `-S --needed` (`--print` first for the list); with
//!   `--noconfirm` a conflict's "remove?" takes its default, no, and the
//!   install stops.
//! - dnf: no list until Fedora's names are checked; refused with the
//!   manual text, nothing installed.
//! - Refused below [`DESKTOP_MIN_FREE_BYTES`] on `/` or `/var`.

use k2_root_proto::apps::{gib, APT_GET, ENV, FREE_CHECK_PATHS, PACMAN};
use k2_root_proto::desktop::{
    apt_sim_installs, apt_sim_removals, manual_command, packages_for, APT_LOCK_WAIT_SECS,
    DESKTOP_MIN_FREE_BYTES, ICEWM_SESSION, OPENBOX, XVFB,
};
use serde_json::{json, Value as J};

use crate::apps::{exists, package_manager, pm_name, Pm};
use crate::ctx::{Ctx, Fail, HResult};

fn disk_floor(ctx: &Ctx) -> HResult<()> {
    let low: Vec<String> = FREE_CHECK_PATHS
        .iter()
        .filter_map(|p| (ctx.free_bytes)(&ctx.p(p)).ok().map(|b| (*p, b)))
        .filter(|(_, b)| *b < DESKTOP_MIN_FREE_BYTES)
        .map(|(p, b)| format!("{} free on {p}", gib(b)))
        .collect();
    if low.is_empty() {
        return Ok(());
    }
    Err(Fail::state(format!(
        "low_disk: {}; the desktop packages need {} free on {} first (nothing changed: free some space, then try again)",
        low.join(", "),
        gib(DESKTOP_MIN_FREE_BYTES),
        FREE_CHECK_PATHS.join(" and ")
    )))
}

/// Runs the install; answers what the package manager said it would add.
fn install(ctx: &Ctx, pm: Pm, pkgs: &[&str]) -> HResult<Vec<String>> {
    Ok(match pm {
        Pm::Apt => {
            let fe = "DEBIAN_FRONTEND=noninteractive";
            let lock = format!("DPkg::Lock::Timeout={APT_LOCK_WAIT_SECS}");
            ctx.run_ok(ENV, &[fe, APT_GET, "update", "-q", "-o", &lock], None)?;
            let opts = [
                "-q",
                "--no-install-recommends",
                "--no-upgrade",
                "-o",
                lock.as_str(),
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confold",
            ];
            let mut sim = vec![fe, APT_GET, "install", "-s"];
            sim.extend_from_slice(&opts);
            sim.extend_from_slice(pkgs);
            let out = ctx.run_ok(ENV, &sim, None)?;
            let text = String::from_utf8_lossy(&out.stdout);
            let removes = apt_sim_removals(&text);
            if !removes.is_empty() {
                return Err(Fail::state(format!(
                    "apt_would_remove: installing {} would remove {}; K2 never removes packages (nothing changed)",
                    pkgs.join(" "),
                    removes.join(" ")
                )));
            }
            let added = apt_sim_installs(&text);
            let mut a = vec![fe, APT_GET, "install", "-y"];
            a.extend_from_slice(&opts);
            a.extend_from_slice(pkgs);
            ctx.run_ok(ENV, &a, None)?;
            added
        }
        Pm::Pacman => {
            let mut p = vec!["-S", "--needed", "--print", "--print-format", "%n"];
            p.extend_from_slice(pkgs);
            let out = ctx.run_ok(PACMAN, &p, None)?;
            let added = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            let mut a = vec!["-S", "--needed", "--noconfirm"];
            a.extend_from_slice(pkgs);
            ctx.run_ok(PACMAN, &a, None)?;
            added
        }
        // packages_install refuses dnf before this (no checked list).
        Pm::Dnf => return Err(Fail::state("no_package_list: dnf (nothing was installed)")),
    })
}

pub fn packages_install(ctx: &Ctx) -> HResult<J> {
    let pm = package_manager(ctx).ok_or_else(|| {
        Fail::state(format!(
            "no_package_manager: K2 knows the desktop packages for apt, pacman and dnf, and this server has none of them, so nothing was installed. {}",
            manual_command(None)
        ))
    })?;
    let pkgs = packages_for(pm_name(pm)).ok_or_else(|| {
        Fail::state(format!(
            "no_package_list: K2 has no checked desktop package list for {} yet, so nothing was installed. {}",
            pm_name(pm),
            manual_command(Some(pm_name(pm)))
        ))
    })?;
    disk_floor(ctx)?;
    let added = install(ctx, pm, pkgs)?;
    if !exists(ctx, XVFB) {
        return Err(Fail::state(format!(
            "xvfb_missing: {} finished, but {XVFB} isn't there. Install it by hand: {}",
            pm_name(pm),
            manual_command(Some(pm_name(pm)))
        )));
    }
    let wm = if exists(ctx, ICEWM_SESSION) {
        "icewm"
    } else if exists(ctx, OPENBOX) {
        "openbox"
    } else {
        return Err(Fail::state(format!(
            "window_manager_missing: {} finished, but neither {ICEWM_SESSION} nor {OPENBOX} is there. Install IceWM by hand: {}",
            pm_name(pm),
            manual_command(Some(pm_name(pm)))
        )));
    };
    Ok(json!({
        "packageManager": pm_name(pm),
        "requested": pkgs,
        "added": added,
        "xvfb": true,
        "windowManager": wm,
    }))
}
