//! 0.40.0 rebrand — one-time launchd label migration
//! (`com.k2so.*` → `dev.k2.*`).
//!
//! Pre-0.40 LaunchAgents:
//!   - `com.k2so.k2so-daemon.plist`        (the daemon — app-installed)
//!   - `com.k2so.agent-heartbeat.plist`    (wake scheduler)
//!   - `com.k2so.claude-auth-refresh.plist`(credential refresher)
//!
//! 0.40.0 installs the same agents under `dev.k2.daemon`,
//! `dev.k2.heartbeat`, `dev.k2.claude-auth`. This sweep boots out the
//! OLD labels and deletes their plist files; the NORMAL install/ensure
//! paths (which run at every boot: app's daemon plist install/heal, the
//! daemon's heartbeat apply + claude-auth ensure) then create the new
//! ones. Without this sweep, both old- and new-labeled agents would run
//! simultaneously — two daemons fighting over the port claim is exactly
//! the disconnected-after-update state the R1 rig flagged.
//!
//! Idempotent: a label whose plist no longer exists is skipped; bootout
//! of a non-running label is ignored. macOS-only (no-op elsewhere).

use std::path::PathBuf;

/// (old label, new label) pairs — new labels listed for the log only;
/// installation of the new agents belongs to their owners.
const LABEL_MIGRATIONS: &[(&str, &str)] = &[
    ("com.k2so.k2so-daemon", "dev.k2.daemon"),
    ("com.k2so.agent-heartbeat", "dev.k2.heartbeat"),
    ("com.k2so.claude-auth-refresh", "dev.k2.claude-auth"),
];

/// Sweep old-labeled LaunchAgents. Returns the OLD labels that were
/// actually present (so callers can eagerly re-ensure their successors
/// instead of waiting for the next natural re-install).
pub fn migrate_launchd_labels() -> Vec<&'static str> {
    #[cfg(not(target_os = "macos"))]
    {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let agents_dir = home.join("Library").join("LaunchAgents");
        let uid = unsafe { libc::getuid() };
        let mut migrated = Vec::new();

        for (old_label, new_label) in LABEL_MIGRATIONS {
            let plist: PathBuf = agents_dir.join(format!("{old_label}.plist"));
            if !plist.exists() {
                continue;
            }
            // Boot out the old agent (kills the old-labeled process).
            // Failure is fine — it may not be loaded.
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{old_label}")])
                .output();
            match std::fs::remove_file(&plist) {
                Ok(()) => {
                    crate::log_debug!(
                        "[migration/0.40] launchd: retired {old_label} → {new_label} \
                         (plist removed; successor installs via its normal path)"
                    );
                    migrated.push(*old_label);
                }
                Err(e) => {
                    crate::log_debug!(
                        "[migration/0.40] launchd: bootout {old_label} ok but plist \
                         remove failed: {e} — will retry next boot"
                    );
                }
            }
        }
        migrated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mapping itself is load-bearing — lock the label pairs so a
    /// future rename can't silently desync sweep and installers.
    #[test]
    fn label_pairs_are_the_canonical_set() {
        assert_eq!(
            LABEL_MIGRATIONS,
            &[
                ("com.k2so.k2so-daemon", "dev.k2.daemon"),
                ("com.k2so.agent-heartbeat", "dev.k2.heartbeat"),
                ("com.k2so.claude-auth-refresh", "dev.k2.claude-auth"),
            ]
        );
    }

    /// New labels must match what the installers actually use.
    #[test]
    fn new_daemon_label_matches_lifecycle_constant() {
        assert_eq!(
            crate::daemon_lifecycle::DAEMON_LAUNCH_AGENT_LABEL,
            "dev.k2.daemon"
        );
    }
}
