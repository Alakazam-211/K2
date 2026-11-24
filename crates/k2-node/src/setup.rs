//! `k2-node setup` / `k2-node uninstall`: the installers travel inside
//! the binary (compute polish v1), so a box that has `k2-node` (bundled
//! with K2, or downloaded and signature-checked by the K2 daemon) needs
//! no repo checkout and no hand-copied scripts:
//!
//! ```text
//! sudo k2-node setup [--private-home|--no-private-home] [--allow-packages "a b"] [--controller … --enroll … --name …]
//! ```
//!
//! The embedded script is written to a fresh root-only file and run with
//! `/bin/bash` from `/` (never the caller's cwd: the old installers broke
//! when started inside a home they had just made private).

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

pub const LINUX_INSTALL: &str = include_str!("../../../scripts/node/install-node-linux.sh");
pub const MACOS_INSTALL: &str = include_str!("../../../scripts/node/install-node-macos.sh");
pub const LINUX_UNINSTALL: &str = include_str!("../../../scripts/node/uninstall-node-linux.sh");
pub const MACOS_UNINSTALL: &str = include_str!("../../../scripts/node/uninstall-node-macos.sh");
/// `setup --sim`: the iPhone service on a Mac node (prd-ios-on-mac-node-0467-v1 §3.3).
pub const MACOS_SIM: &str = include_str!("../../../scripts/node/install-sim-macos.sh");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Uninstall,
}

pub fn script(kind: Kind, os: &str) -> Result<&'static str, String> {
    match (kind, os) {
        (Kind::Install, "linux") => Ok(LINUX_INSTALL),
        (Kind::Install, "macos") => Ok(MACOS_INSTALL),
        (Kind::Uninstall, "linux") => Ok(LINUX_UNINSTALL),
        (Kind::Uninstall, "macos") => Ok(MACOS_UNINSTALL),
        (_, o) => Err(format!("no installer for {o}")),
    }
}

/// argv for the embedded installer: `--binary <this binary>` first unless
/// the caller named one.
pub fn installer_args(kind: Kind, self_path: &str, user_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if kind == Kind::Install && !user_args.iter().any(|a| a == "--binary" || a.starts_with("--binary=")) {
        out.push("--binary".to_string());
        out.push(self_path.to_string());
    }
    out.extend(user_args.iter().cloned());
    out
}

/// `setup --sim` (iPhone on a Mac node, prd-ios-on-mac-node-0467-v1 §3.3):
/// the checks before anything is installed. Simulator apps run outside the
/// job wall (CoreSimulator starts them, not a job), so they can read any
/// folder other users can read: a person's home that is open to others is
/// refused (review IR-4) unless the owner passes `--private-home`, which
/// makes it private.
pub fn sim_preflight(macos: bool, open_homes: &[(PathBuf, u32)], private_home: bool) -> Result<(), String> {
    if !macos {
        return Err("--sim is for a Mac node: the iOS Simulator needs macOS and Xcode.".into());
    }
    if !open_homes.is_empty() && !private_home {
        let list: Vec<String> = open_homes.iter().map(|(p, m)| format!("{} ({m:o})", p.display())).collect();
        let paths: Vec<String> = open_homes.iter().map(|(p, _)| p.display().to_string()).collect();
        return Err(format!(
            "These home folders are open to other users: {}. iPhone apps on this Mac run outside the job wall and could read them. Make them private first (sudo chmod 700 {}), or run setup again with --private-home.",
            list.join(", "),
            paths.join(" ")
        ));
    }
    Ok(())
}

/// Run the embedded installer as root. Returns its exit code.
pub fn run(kind: Kind, user_args: &[String]) -> Result<i32, String> {
    let sim = kind == Kind::Install && user_args.iter().any(|a| a == "--sim");
    if sim && !user_args.iter().any(|a| a == "--remove") {
        let private = user_args.iter().any(|a| a == "--private-home");
        sim_preflight(cfg!(target_os = "macos"), &crate::wall::open_homes_in(crate::wall::homes_root()), private)?;
    }
    if unsafe { libc::geteuid() } != 0 {
        let verb = if kind == Kind::Install { "setup" } else { "uninstall" };
        return Err(format!(
            "run as root, once: sudo {} {verb} … (over SSH: one sudo call, password on stdin: sudo -S -p '' {} {verb} …)",
            self_exe_display(),
            self_exe_display()
        ));
    }
    let os = crate::sysinfo::os_label();
    let body = if sim {
        if os != "macos" {
            return Err("--sim is for a Mac node: the iOS Simulator needs macOS and Xcode.".into());
        }
        MACOS_SIM
    } else {
        script(kind, os)?
    };
    let me = std::env::current_exe().map_err(|e| format!("own path: {e}"))?;
    let me = std::fs::canonicalize(&me).unwrap_or(me);
    let id = k2_node_proto::crypto::random_id()?;
    let path = PathBuf::from(format!("/var/tmp/k2-node-setup-{id}.sh"));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        f.write_all(body.as_bytes()).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    let args = installer_args(kind, &me.display().to_string(), user_args);
    let st = std::process::Command::new("/bin/bash")
        .arg(&path)
        .args(&args)
        .current_dir("/")
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .status();
    let _ = std::fs::remove_file(&path);
    let st = st.map_err(|e| format!("run installer: {e}"))?;
    Ok(st.code().unwrap_or(1))
}

fn self_exe_display() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "k2-node".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IR-4: Simulator apps run outside Seatbelt, so `setup --sim` refuses
    /// while a person's home is open to others (k2n1-m4's
    /// `/Users/scoutwilliams` was installed with `--no-private-home`).
    #[test]
    fn setup_sim_refuses_while_a_home_is_readable_by_others() {
        let open = vec![(PathBuf::from("/Users/scoutwilliams"), 0o755)];
        let e = sim_preflight(true, &open, false).unwrap_err();
        assert!(e.contains("/Users/scoutwilliams (755)") && e.contains("sudo chmod 700 /Users/scoutwilliams") && e.contains("--private-home"), "{e}");
        assert!(sim_preflight(true, &open, true).is_ok(), "--private-home makes them private");
        assert!(sim_preflight(true, &[], false).is_ok());
        assert!(sim_preflight(false, &[], false).unwrap_err().contains("Mac node"));
    }

    #[test]
    fn open_homes_are_found_by_their_other_bits() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("k2-homes-{}-{}", std::process::id(), k2_node_proto::crypto::random_id().unwrap()));
        for (n, mode) in [("scout", 0o755), ("private", 0o700), ("Shared", 0o777)] {
            std::fs::create_dir_all(root.join(n)).unwrap();
            std::fs::set_permissions(root.join(n), std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let open = crate::wall::open_homes_in(&root);
        assert_eq!(open, vec![(root.join("scout"), 0o755)]);
        assert!(sim_preflight(true, &open, false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scripts_are_embedded_and_start_from_root_dir() {
        for (k, os) in [(Kind::Install, "linux"), (Kind::Install, "macos"), (Kind::Uninstall, "linux"), (Kind::Uninstall, "macos")] {
            let s = script(k, os).unwrap();
            assert!(s.starts_with("#!/usr/bin/env bash"), "{os}");
            assert!(s.contains("\ncd /\n"), "{k:?} {os}: every installer must cd / first");
        }
        assert!(script(Kind::Install, "windows").is_err());
        assert!(MACOS_SIM.starts_with("#!/usr/bin/env bash") && MACOS_SIM.contains("\ncd /\n"));
    }

    /// `setup --sim` (IR-9): the socket's folder is reachable by k2simctl
    /// only, the node user is its one member, the Simulator user is in
    /// neither control group, and the home check (IR-4) refuses open homes.
    #[test]
    fn the_sim_installer_keeps_the_simulator_user_apart() {
        let s = MACOS_SIM;
        for want in [
            "SIM_USER=\"_k2sim\"",
            "CTL_GROUP=\"k2simctl\"",
            "install -d -m 0710 -o \"$SIM_USER\" -g \"$CTL_GROUP\" \"$ROOT\"",
            "install -d -m 0750 -o \"$SIM_USER\" -g \"$CTL_GROUP\" \"$ROOT/run\"",
            "dseditgroup -o edit -a \"$NODE_USER\" -t user \"$CTL_GROUP\"",
            "for g in k2nodectl \"$CTL_GROUP\"; do",
            "dscl . -create \"/Users/$SIM_USER\" UserShell /usr/bin/false",
            "dscl . -create \"/Users/$SIM_USER\" IsHidden 1",
            "\"$BIN\" admin homes --json",
            "\"$BIN\" sim enable --server \"$SERVER\"",
            "launchctl kickstart -k system/dev.k2.node",
        ] {
            assert!(s.contains(want), "install-sim-macos.sh must contain {want:?}");
        }
        let un = MACOS_UNINSTALL;
        assert!(un.contains("dev.k2.simhost") && un.contains("_k2sim") && un.contains("k2simctl") && un.contains("/var/k2sim"), "uninstall removes the iPhone service too");
    }

    /// 0.46.18 (Steward rev 3, 3): uninstalling takes K2's AppArmor profile
    /// for --userns units away: unloaded while loaded, the file deleted.
    #[test]
    fn linux_uninstall_unloads_and_deletes_the_userns_profile() {
        let un = script(Kind::Uninstall, "linux").unwrap();
        assert!(un.contains("apparmor_parser -R /etc/apparmor.d/k2-job-userns"), "unloads it");
        assert!(un.contains("rm -f /etc/apparmor.d/k2-job-userns"), "deletes the file");
        assert_eq!(crate::admin::apparmor::PROFILE_PATH, "/etc/apparmor.d/k2-job-userns", "the path the uninstaller names");
    }

    /// `sudo k2-node …` works on Linux (0.46.6): the installer links
    /// /usr/local/bin/k2-node to the libexec binary (never over a file that
    /// isn't a link), and the uninstaller removes only its own link.
    #[test]
    fn linux_installer_puts_k2_node_on_path() {
        let inst = script(Kind::Install, "linux").unwrap();
        assert!(inst.contains("\nPATH_LINK=\"/usr/local/bin/k2-node\"\n"), "installer must name the PATH link");
        assert!(inst.contains("ln -sfn \"$BIN\" \"$PATH_LINK\""), "installer must link the libexec binary");
        assert!(
            inst.contains("if [ -L \"$PATH_LINK\" ] || [ ! -e \"$PATH_LINK\" ]; then"),
            "installer must not replace a real file at the link path"
        );
        let link = inst.find("ln -sfn \"$BIN\"").unwrap();
        let bin = inst.find("\nBIN=\"$LIBEXEC/k2-node\"\n").expect("BIN set");
        assert!(bin < link, "the link is made after BIN is set");
        let un = script(Kind::Uninstall, "linux").unwrap();
        assert!(
            un.contains("[ \"$(readlink /usr/local/bin/k2-node 2>/dev/null)\" = \"$BIN\" ] && rm -f /usr/local/bin/k2-node"),
            "uninstaller removes only its own link"
        );
    }

    #[test]
    fn binary_is_added_unless_given() {
        let a = installer_args(Kind::Install, "/usr/bin/k2-node", &["--no-private-home".into()]);
        assert_eq!(a, vec!["--binary", "/usr/bin/k2-node", "--no-private-home"]);
        let b = installer_args(Kind::Install, "/x", &["--binary".into(), "/y".into()]);
        assert_eq!(b, vec!["--binary", "/y"]);
        assert!(installer_args(Kind::Uninstall, "/x", &[]).is_empty());
    }
}
