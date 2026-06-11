//! macOS launchd plist generation + install/uninstall helpers.
//!
//! The persistent-agents feature puts k2so-daemon under launchd so the
//! process runs in the background regardless of whether the Tauri app is
//! open, survives crashes via `KeepAlive: true`, and can (optionally)
//! wake the system from sleep on a scheduled interval to fire heartbeats
//! while the laptop lid is closed.
//!
//! This module owns two launchd agents:
//!
//! 1. **Daemon agent** (`dev.k2.daemon`): always-on. `RunAtLoad:
//!    true` + `KeepAlive: true`. launchd starts it on user login and
//!    restarts it on crash.
//! 2. **Heartbeat agent** (`dev.k2.heartbeat`, already in the
//!    codebase): fires every N minutes (user-configurable). When the
//!    wake-to-run setting is on, `Wake: true` makes launchd actually
//!    wake a sleeping machine to run it. This is how Time Machine
//!    handles battery-powered hourly backups.
//!
//! Scope for this module: plist XML generation + file writes + launchctl
//! invocation. The orchestration ("on first launch of 0.33.0, install
//! the daemon plist") is a `code_migrations` row wired up separately.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Where user-scope launchd agents live on macOS.
pub fn launch_agents_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/LaunchAgents"))
}

/// Describes the k2so-daemon plist. Exposed as a builder so the Tauri
/// app (or a future `k2so daemon install` CLI subcommand) can compose
/// the config from user settings without this module knowing about
/// Settings.
#[derive(Debug, Clone)]
pub struct DaemonPlist {
    /// Reverse-DNS identifier. Matches the plist filename: `<label>.plist`.
    pub label: String,
    /// Absolute path to the k2so-daemon binary.
    pub program: PathBuf,
    /// Additional arguments passed to the binary after `program`.
    pub args: Vec<String>,
    /// `RunAtLoad: true` starts the daemon on user login.
    pub run_at_load: bool,
    /// `KeepAlive: true` tells launchd to restart the daemon on crash.
    pub keep_alive: bool,
    /// Where to redirect stderr (`~/.k2/daemon.stderr.log`).
    pub stderr_path: PathBuf,
    /// Where to redirect stdout (`~/.k2/daemon.stdout.log`).
    pub stdout_path: PathBuf,
}

impl DaemonPlist {
    /// Canonical config: k2so-daemon, always-on, logs under `~/.k2so/`.
    pub fn canonical(program: PathBuf) -> Self {
        let k2so_dir = dirs::home_dir()
            .map(|h| h.join(".k2"))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            label: "dev.k2.daemon".to_string(),
            program,
            args: vec![],
            run_at_load: true,
            keep_alive: true,
            stderr_path: k2so_dir.join("daemon.stderr.log"),
            stdout_path: k2so_dir.join("daemon.stdout.log"),
        }
    }

    /// Emit the plist as an XML string. Validates via `plutil -lint` in
    /// the unit test below.
    pub fn to_xml(&self) -> String {
        let mut program_args = format!(
            "        <string>{}</string>\n",
            xml_escape(self.program.to_string_lossy().as_ref())
        );
        for arg in &self.args {
            program_args.push_str(&format!(
                "        <string>{}</string>\n",
                xml_escape(arg)
            ));
        }
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{program_args}    </array>
    <key>RunAtLoad</key>
    <{run_at_load}/>
    <key>KeepAlive</key>
    <{keep_alive}/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
</dict>
</plist>
"#,
            label = xml_escape(&self.label),
            program_args = program_args,
            run_at_load = if self.run_at_load { "true" } else { "false" },
            keep_alive = if self.keep_alive { "true" } else { "false" },
            stderr = xml_escape(self.stderr_path.to_string_lossy().as_ref()),
            stdout = xml_escape(self.stdout_path.to_string_lossy().as_ref()),
        )
    }

    /// Target plist path under `~/Library/LaunchAgents/`.
    pub fn plist_path(&self) -> Option<PathBuf> {
        launch_agents_dir().map(|d| d.join(format!("{}.plist", self.label)))
    }

    /// Write the plist XML to disk with 0644 permissions. Parent
    /// directory is created if missing. Idempotent — existing file is
    /// overwritten.
    pub fn write(&self) -> Result<PathBuf, String> {
        let target = self
            .plist_path()
            .ok_or_else(|| "cannot locate ~/Library/LaunchAgents".to_string())?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut f = fs::File::create(&target)
            .map_err(|e| format!("create {}: {e}", target.display()))?;
        f.write_all(self.to_xml().as_bytes())
            .map_err(|e| format!("write {}: {e}", target.display()))?;
        Ok(target)
    }
}

/// `launchctl load -w <plist>`. `-w` persists the enable state so the
/// agent survives login cycles. Returns stderr on non-zero exit for
/// caller to surface.
pub fn launchctl_load(plist_path: &PathBuf) -> Result<(), String> {
    let out = Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(plist_path)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "launchctl load failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    Ok(())
}

/// `launchctl unload -w <plist>`. Safe to call on a non-loaded plist
/// (launchctl returns 0 or a harmless "service not found" message).
pub fn launchctl_unload(plist_path: &PathBuf) -> Result<(), String> {
    let out = Command::new("launchctl")
        .arg("unload")
        .arg("-w")
        .arg(plist_path)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if !out.status.success() {
        // unload on non-existent label prints to stderr with non-zero
        // exit; treat that as OK for idempotent uninstalls.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("Could not find") || stderr.contains("No such") {
            return Ok(());
        }
        return Err(format!(
            "launchctl unload failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }
    Ok(())
}

/// Install: write the plist + load it. Idempotent — if a plist with
/// the same label is already loaded (e.g. user upgrading over an
/// existing 0.33.x install, or rolling back + re-installing), we
/// best-effort unload first so `launchctl load -w` doesn't fail with
/// "already loaded." The unload's own errors are swallowed because a
/// fresh-install `unload` call on a non-existent label is a harmless
/// no-op (launchctl_unload treats that case as Ok).
pub fn install(plist: &DaemonPlist) -> Result<PathBuf, String> {
    let target = plist.write()?;
    // Best-effort unload of any stale plist already claiming this
    // label. Ignored on fresh installs.
    let _ = launchctl_unload(&target);
    launchctl_load(&target)?;
    Ok(target)
}

/// Outcome of [`ensure_loaded`] — lets the caller log/report whether
/// the daemon was already alive, freshly loaded, or not installed yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// Plist file does not exist on disk. Caller needs to run
    /// [`install`] (or the install migration) first.
    NotInstalled,
    /// Plist was already loaded in launchctl when we checked. No-op.
    AlreadyLoaded,
    /// Plist existed on disk but wasn't loaded; we just loaded it.
    Loaded,
}

/// Idempotent "make sure the daemon is loaded" check. Safe to call on
/// every Tauri launch: if the daemon is already alive we do nothing,
/// if the plist exists but isn't loaded we load it, and if the plist
/// file is missing we report that so the install migration can run.
///
/// Used to auto-start the daemon on every Tauri launch regardless of
/// the "Keep server running when window is closed" toggle — so a user
/// who red-buttoned with toggle-OFF (which unloads the daemon) gets
/// the daemon back automatically on next app launch.
pub fn ensure_loaded(plist: &DaemonPlist) -> Result<LoadOutcome, String> {
    let target = plist
        .plist_path()
        .ok_or_else(|| "cannot locate ~/Library/LaunchAgents".to_string())?;
    if !target.exists() {
        return Ok(LoadOutcome::NotInstalled);
    }
    // `launchctl list <label>` exits 0 if the service is loaded (even
    // if not currently running), non-zero otherwise. Cheapest way to
    // check without racing the plist.
    let out = Command::new("launchctl")
        .arg("list")
        .arg(&plist.label)
        .output()
        .map_err(|e| format!("launchctl list: {e}"))?;
    if out.status.success() {
        return Ok(LoadOutcome::AlreadyLoaded);
    }
    launchctl_load(&target)?;
    Ok(LoadOutcome::Loaded)
}

/// Uninstall: unload + remove the plist file. Safe to call on a fresh
/// system (no plist yet).
pub fn uninstall(plist: &DaemonPlist) -> Result<(), String> {
    let target = plist
        .plist_path()
        .ok_or_else(|| "cannot locate ~/Library/LaunchAgents".to_string())?;
    if target.exists() {
        // Ignore unload errors — we want to get to the file-delete
        // step even if launchctl was unhappy.
        let _ = launchctl_unload(&target);
        fs::remove_file(&target).map_err(|e| format!("remove {}: {e}", target.display()))?;
    }
    Ok(())
}

/// Escape XML special characters in plist string values. Minimal — we
/// don't touch quote marks since they're only a problem in attribute
/// values.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DaemonPlist {
        DaemonPlist {
            label: "dev.k2.test-daemon".to_string(),
            program: PathBuf::from("/Applications/K2SO.app/Contents/MacOS/k2so-daemon"),
            args: vec![],
            run_at_load: true,
            keep_alive: true,
            stderr_path: PathBuf::from("/tmp/daemon.err"),
            stdout_path: PathBuf::from("/tmp/daemon.out"),
        }
    }

    #[test]
    fn plist_contains_required_keys() {
        let xml = sample().to_xml();
        for key in &[
            "<key>Label</key>",
            "<key>ProgramArguments</key>",
            "<key>RunAtLoad</key>",
            "<key>KeepAlive</key>",
            "<key>ProcessType</key>",
            "<key>StandardErrorPath</key>",
            "<key>StandardOutPath</key>",
        ] {
            assert!(xml.contains(key), "plist missing required {key}\n{xml}");
        }
        assert!(xml.contains("<string>dev.k2.test-daemon</string>"));
        assert!(xml.contains("<true/>"));
        // ProcessType Interactive gives the daemon's child processes
        // (spawned PTY shells + Claude) the same priority Tauri's
        // foreground app enjoys. Before this, the daemon was
        // Background — Claude startup in Kessel took 3s vs 1s in
        // Alacritty purely from priority starvation on fork/exec.
        assert!(xml.contains("<string>Interactive</string>"));
    }

    #[test]
    fn plist_program_arg_includes_full_path() {
        let xml = sample().to_xml();
        assert!(
            xml.contains("<string>/Applications/K2SO.app/Contents/MacOS/k2so-daemon</string>"),
            "program path missing: {xml}"
        );
    }

    #[test]
    fn plist_escapes_xml_special_chars_in_paths() {
        let mut p = sample();
        p.program = PathBuf::from("/tmp/has<weird>&stuff/bin");
        let xml = p.to_xml();
        assert!(xml.contains("&lt;weird&gt;&amp;stuff"), "missing escapes: {xml}");
        assert!(!xml.contains("<weird>"), "raw angle brackets leaked: {xml}");
    }

    #[test]
    fn canonical_uses_expected_label_and_log_paths() {
        let p = DaemonPlist::canonical(PathBuf::from("/opt/k2so-daemon"));
        assert_eq!(p.label, "dev.k2.daemon");
        assert!(p.run_at_load);
        assert!(p.keep_alive);
        assert!(p.stderr_path.ends_with(".k2/daemon.stderr.log"));
        assert!(p.stdout_path.ends_with(".k2/daemon.stdout.log"));
    }

    #[test]
    fn plutil_lints_canonical_plist() {
        // Optional: skip this test if plutil isn't on PATH (unlikely on
        // macOS CI but keeps the suite portable).
        let plutil_exists = Command::new("which")
            .arg("plutil")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !plutil_exists {
            eprintln!("plutil not available; skipping lint assertion");
            return;
        }

        let tmp_plist = std::env::temp_dir().join(format!(
            "k2so-wake-test-{}.plist",
            uuid::Uuid::new_v4()
        ));
        let p = DaemonPlist::canonical(PathBuf::from("/opt/k2so-daemon"));
        fs::write(&tmp_plist, p.to_xml()).expect("write plist");

        let out = Command::new("plutil")
            .arg("-lint")
            .arg(&tmp_plist)
            .output()
            .expect("run plutil");
        let _ = fs::remove_file(&tmp_plist);
        assert!(
            out.status.success(),
            "plutil -lint rejected plist: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
