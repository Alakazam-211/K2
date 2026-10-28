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

/// Run the embedded installer as root. Returns its exit code.
pub fn run(kind: Kind, user_args: &[String]) -> Result<i32, String> {
    if unsafe { libc::geteuid() } != 0 {
        let verb = if kind == Kind::Install { "setup" } else { "uninstall" };
        return Err(format!(
            "run as root, once: sudo {} {verb} … (over SSH: one sudo call, password on stdin: sudo -S -p '' {} {verb} …)",
            self_exe_display(),
            self_exe_display()
        ));
    }
    let os = crate::sysinfo::os_label();
    let body = script(kind, os)?;
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

    #[test]
    fn scripts_are_embedded_and_start_from_root_dir() {
        for (k, os) in [(Kind::Install, "linux"), (Kind::Install, "macos"), (Kind::Uninstall, "linux"), (Kind::Uninstall, "macos")] {
            let s = script(k, os).unwrap();
            assert!(s.starts_with("#!/usr/bin/env bash"), "{os}");
            assert!(s.contains("\ncd /\n"), "{k:?} {os}: every installer must cd / first");
        }
        assert!(script(Kind::Install, "windows").is_err());
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
