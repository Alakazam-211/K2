//! 0.40.0 rebrand — one-time `~/.k2so` → `~/.k2` home-directory
//! migration.
//!
//! Everything daemon-owned (k2so.db, daemon.port/token, heartbeat.port/
//! token, logs, frpc config, themes, settings.json, connect-users.json,
//! clone-tmp) lived under `~/.k2so`. As of 0.40.0 the canonical home is
//! `~/.k2`; this module moves the directory ONCE and leaves
//! `~/.k2so` as a SYMLINK to `~/.k2` so:
//!   - older CLIs / user scripts that hardcode `~/.k2so/...` keep
//!     working through the transition (symlink-transparent), and
//!   - a half-upgraded fleet (old app + new daemon, or vice versa)
//!     reads/writes the same underlying files either way.
//!
//! Idempotent and crash-safe by construction:
//!   - `~/.k2` exists already → nothing to move (subsequent boots, or a
//!     fresh 0.40 install). If a REAL `~/.k2so` dir also exists (not a
//!     symlink), it is left untouched and we log — that machine ran
//!     mixed versions; the operator decides. We never merge or delete.
//!   - `~/.k2so` is already a symlink → done (migration ran before).
//!   - neither exists → fresh machine; create `~/.k2` AND the `~/.k2so`
//!     compat symlink (the app/CLI still resolve hardcoded `~/.k2so/...`
//!     paths — incl. the daemon pairing handshake — through it).
//!
//! MUST run before anything touches the DB or port files — both the
//! daemon (`main.rs`, before `db::shared()`/port claim) and the app
//! (`src-tauri` setup, before spawning/handshaking the daemon) call
//! [`migrate_home_dir`] at boot. First caller wins; the other finds the
//! migrated layout.

use std::path::{Path, PathBuf};

/// Outcome of a migration attempt — returned for logging/telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeMigration {
    /// `~/.k2so` was moved to `~/.k2` and the compat symlink created.
    Migrated,
    /// Already in the post-migration layout (or fresh `~/.k2`).
    AlreadyDone,
    /// Fresh machine: created `~/.k2` + compat symlink.
    FreshInit,
    /// BOTH a real `~/.k2` and a real `~/.k2so` directory exist —
    /// ambiguous; nothing was touched. Carries both paths for the log.
    Conflict(PathBuf, PathBuf),
}

/// Run the one-time migration against the real `$HOME`. See module docs.
pub fn migrate_home_dir() -> Result<HomeMigration, String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    migrate_home_dir_at(&home)
}

/// Testable core — operates on an explicit home root.
pub fn migrate_home_dir_at(home: &Path) -> Result<HomeMigration, String> {
    let old = home.join(".k2so");
    let new = home.join(".k2");

    let old_is_symlink = old.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    let old_is_real_dir = !old_is_symlink && old.is_dir();
    let new_exists = new.exists();

    if old_is_symlink {
        // Migration already ran (or operator pre-linked). Ensure ~/.k2
        // exists for the symlink to resolve to.
        if !new_exists {
            std::fs::create_dir_all(&new).map_err(|e| format!("create ~/.k2: {e}"))?;
        }
        return Ok(HomeMigration::AlreadyDone);
    }

    match (old_is_real_dir, new_exists) {
        (true, false) => {
            // The migration proper: move, then symlink.
            std::fs::rename(&old, &new)
                .map_err(|e| format!("move ~/.k2so → ~/.k2: {e}"))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&new, &old)
                .map_err(|e| format!("symlink ~/.k2so → ~/.k2: {e}"))?;
            crate::log_debug!(
                "[migration/0.40] moved {} → {} (+compat symlink)",
                old.display(),
                new.display()
            );
            Ok(HomeMigration::Migrated)
        }
        (true, true) => {
            // Ambiguous — refuse to guess. Log loudly; the newer layout
            // (~/.k2) is what the code will use from here on.
            crate::log_debug!(
                "[migration/0.40] CONFLICT: both {} and {} exist — left untouched; \
                 the app uses ~/.k2. Merge or remove ~/.k2so manually.",
                old.display(),
                new.display()
            );
            Ok(HomeMigration::Conflict(old, new))
        }
        (false, true) => {
            // `~/.k2` exists but `~/.k2so` is neither a symlink nor a real
            // dir — i.e. the compat symlink is MISSING. A normal
            // post-migration machine has the symlink and returns at the top,
            // so reaching here means a 0.40.33/0.40.34 fresh install created
            // `~/.k2` WITHOUT the symlink (the pairing-breaking regression).
            // SELF-HEAL: create it now so the next launch pairs. Only when
            // `~/.k2so` is genuinely absent (never clobber a user file that
            // happens to sit there).
            #[cfg(unix)]
            {
                if old.symlink_metadata().is_err() {
                    std::os::unix::fs::symlink(&new, &old).map_err(|e| {
                        format!("heal missing ~/.k2so → ~/.k2 symlink: {e}")
                    })?;
                    crate::log_debug!(
                        "[migration/0.40] healed missing ~/.k2so compat symlink \
                         (0.40.33/0.40.34 fresh-install pairing regression)"
                    );
                }
            }
            Ok(HomeMigration::AlreadyDone)
        }
        (false, false) => {
            // Fresh install — create `~/.k2` AND the `~/.k2so` compat
            // symlink. The symlink is NOT optional cosmetics: the Tauri app
            // (and CLI, and user hook scripts) still resolve dozens of
            // hardcoded `~/.k2so/...` paths — daemon.port/daemon.token (the
            // client↔daemon pairing handshake, src-tauri/src/lib.rs +
            // daemon_events.rs), bin/frpc, templates/, connect-hosts.json,
            // heartbeat.*, hooks/notify.sh — and the symlink is what maps
            // them onto the canonical `~/.k2`. 0.40.33 briefly dropped it
            // here to avoid a "mystery folder"; that BROKE first-launch
            // pairing on every fresh Mac (the daemon writes `~/.k2/...`, the
            // app read `~/.k2so/...` → not found). Restored 0.40.35. Do NOT
            // remove without first de-hardcoding every `~/.k2so` reader.
            std::fs::create_dir_all(&new).map_err(|e| format!("create ~/.k2: {e}"))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&new, &old)
                .map_err(|e| format!("symlink ~/.k2so → ~/.k2: {e}"))?;
            Ok(HomeMigration::FreshInit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-mig-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// THE convergence-critical case: a real pre-0.40 ~/.k2so moves to
    /// ~/.k2, contents intact, with a working compat symlink behind.
    #[test]
    fn migrates_real_k2so_dir_and_leaves_symlink() {
        let home = tmp_home("move");
        let old = home.join(".k2so");
        std::fs::create_dir_all(old.join("hooks")).unwrap();
        std::fs::write(old.join("k2so.db"), b"dbdb").unwrap();
        std::fs::write(old.join("daemon.port"), b"12345").unwrap();

        let out = migrate_home_dir_at(&home).expect("migration ok");
        assert_eq!(out, HomeMigration::Migrated);

        // Contents moved.
        assert_eq!(std::fs::read(home.join(".k2/k2so.db")).unwrap(), b"dbdb");
        assert_eq!(std::fs::read(home.join(".k2/daemon.port")).unwrap(), b"12345");
        assert!(home.join(".k2/hooks").is_dir());
        // Compat symlink resolves to the same files.
        assert!(home.join(".k2so").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(home.join(".k2so/daemon.port")).unwrap(), b"12345");
        // Writing through the OLD path lands in the new dir (old CLIs).
        std::fs::write(home.join(".k2so/probe"), b"x").unwrap();
        assert!(home.join(".k2/probe").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn idempotent_on_second_run() {
        let home = tmp_home("idem");
        std::fs::create_dir_all(home.join(".k2so")).unwrap();
        std::fs::write(home.join(".k2so/t"), b"1").unwrap();
        assert_eq!(migrate_home_dir_at(&home).unwrap(), HomeMigration::Migrated);
        assert_eq!(migrate_home_dir_at(&home).unwrap(), HomeMigration::AlreadyDone);
        assert_eq!(std::fs::read(home.join(".k2/t")).unwrap(), b"1");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn heals_missing_symlink_from_0_40_33_34_fresh_install() {
        // The regression state: `~/.k2` exists, no `~/.k2so` at all. Next
        // 0.40.35 launch must CREATE the compat symlink so pairing works.
        let home = tmp_home("heal");
        std::fs::create_dir_all(home.join(".k2")).unwrap();
        std::fs::write(home.join(".k2/daemon.port"), b"5000").unwrap();
        assert!(home.join(".k2so").symlink_metadata().is_err(), "precondition");
        assert_eq!(migrate_home_dir_at(&home).unwrap(), HomeMigration::AlreadyDone);
        let link = home.join(".k2so");
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "must heal the missing ~/.k2so symlink"
        );
        // The app's hardcoded ~/.k2so/daemon.port read now resolves.
        assert_eq!(std::fs::read(link.join("daemon.port")).unwrap(), b"5000");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn fresh_machine_creates_k2_and_compat_symlink() {
        // 0.40.35: fresh installs MUST get the `~/.k2so` symlink — the app's
        // hardcoded `~/.k2so/daemon.{port,token}` reads (the client↔daemon
        // pairing handshake) resolve through it. 0.40.33 dropped it here and
        // broke first-launch pairing on every fresh Mac.
        let home = tmp_home("fresh");
        assert_eq!(migrate_home_dir_at(&home).unwrap(), HomeMigration::FreshInit);
        assert!(home.join(".k2").is_dir());
        let link = home.join(".k2so");
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "fresh install must create the ~/.k2so compat symlink"
        );
        // And it must resolve to ~/.k2 (writing through it lands in ~/.k2).
        std::fs::write(link.join("probe"), b"x").unwrap();
        assert!(home.join(".k2/probe").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn conflict_touches_nothing() {
        let home = tmp_home("conflict");
        std::fs::create_dir_all(home.join(".k2so")).unwrap();
        std::fs::write(home.join(".k2so/old"), b"o").unwrap();
        std::fs::create_dir_all(home.join(".k2")).unwrap();
        std::fs::write(home.join(".k2/new"), b"n").unwrap();
        match migrate_home_dir_at(&home).unwrap() {
            HomeMigration::Conflict(..) => {}
            other => panic!("expected Conflict, got {other:?}"),
        }
        // Both intact, no symlink games.
        assert!(home.join(".k2so/old").exists());
        assert!(home.join(".k2/new").exists());
        assert!(!home.join(".k2so").symlink_metadata().unwrap().file_type().is_symlink());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
