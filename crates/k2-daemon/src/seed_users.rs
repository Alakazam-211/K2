//! K2 Cloud S2 (prd-k2-cloud-hosted-servers §6) — boot-time seed-users
//! file: `~/.k2/seed-users.json`, consumed ONCE and DELETED.
//!
//! Lets a golden image / cloud-init provision connect-user accounts
//! without hitting HTTP and without a password ever being echoed to a
//! shell or polled port. Shape:
//!
//! ```json
//! [
//!   { "username": "acme",       "password": "temp-…", "role": "owner",
//!     "mustChangePassword": true },
//!   { "username": "k2cloud-ops","password": "…",      "role": "admin" }
//! ]
//! ```
//!
//! Contract (all enforced here, tested in
//! `tests/provision_users_integration.rs`):
//!
//! - The file is DELETED in every outcome — parse failure, per-user
//!   failures, success — immediately after it is read, so plaintext
//!   passwords never linger on disk. Consume-once.
//! - Users are created via the SAME `connect_users` primitives the HTTP
//!   routes use (`add_user` → `set_role` → `set_must_change_password`).
//! - A username that already exists is SKIPPED (logged) — never
//!   modified, so a re-flashed seed file can't rotate a live customer's
//!   password or role out from under them.
//! - A malformed file / malformed row logs LOUDLY and is still deleted.
//!
//! Called from `main.rs` once the config dir exists, before/independent
//! of the listeners. Idempotent + instant no-op when the file is absent
//! (every normal boot).

use k2_core::connect_users::{self, Role};
use k2_core::log_debug;

/// Path to the consume-once seed file: `~/.k2/seed-users.json`.
pub fn seed_file_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".k2")
        .join("seed-users.json")
}

/// One row of the seed file. `role` stays a string here so ONE bad row
/// fails at apply time (logged, others proceed) instead of failing the
/// whole-file parse.
#[derive(Debug, serde::Deserialize)]
struct SeedUser {
    username: String,
    password: String,
    /// "owner" | "admin" | "member" | "viewer".
    role: String,
    #[serde(default, rename = "mustChangePassword")]
    must_change_password: bool,
}

/// Per-user outcome, returned so the integration suite can assert the
/// exact results (and boot logging stays honest).
#[derive(Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    /// Created with the requested role (+ flag when requested).
    Created,
    /// Username already exists — untouched.
    SkippedExists,
    /// Creation/role/flag failed; carries the reason.
    Failed(String),
}

/// Consume `~/.k2/seed-users.json` if present: create each user, then
/// DELETE the file. Returns `(username, outcome)` per row (empty when
/// the file is absent or unparseable). Never panics — boot must proceed
/// regardless.
pub fn consume_seed_file() -> Vec<(String, SeedOutcome)> {
    let path = seed_file_path();
    if !path.exists() {
        return Vec::new();
    }
    log_debug!("[daemon/seed-users] found {} — consuming", path.display());

    // Read, then delete IMMEDIATELY — before parsing, before applying —
    // so no failure mode below can leave plaintext passwords on disk.
    let raw = std::fs::read_to_string(&path);
    match std::fs::remove_file(&path) {
        Ok(()) => log_debug!("[daemon/seed-users] deleted {} (consume-once)", path.display()),
        Err(e) => log_debug!(
            "[daemon/seed-users] ERROR: could not delete {}: {e} — plaintext seed passwords may remain on disk; remove it manually",
            path.display()
        ),
    }

    let raw = match raw {
        Ok(r) => r,
        Err(e) => {
            log_debug!("[daemon/seed-users] ERROR: read {}: {e} — no users seeded", path.display());
            return Vec::new();
        }
    };
    let rows: Vec<SeedUser> = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            log_debug!(
                "[daemon/seed-users] ERROR: malformed seed file (expected a JSON array of {{username,password,role[,mustChangePassword]}}): {e} — no users seeded; file already deleted"
            );
            return Vec::new();
        }
    };

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let outcome = apply_row(&row);
        match &outcome {
            SeedOutcome::Created => log_debug!(
                "[daemon/seed-users] created '{}' role={} mustChangePassword={}",
                row.username,
                row.role,
                row.must_change_password
            ),
            SeedOutcome::SkippedExists => log_debug!(
                "[daemon/seed-users] '{}' already exists — skipped (not modified)",
                row.username
            ),
            SeedOutcome::Failed(e) => log_debug!(
                "[daemon/seed-users] ERROR: '{}' not seeded: {e}",
                row.username
            ),
        }
        results.push((row.username, outcome));
    }
    results
}

fn apply_row(row: &SeedUser) -> SeedOutcome {
    let Some(role) = Role::from_wire(&row.role) else {
        return SeedOutcome::Failed(format!(
            "invalid role '{}' (expected owner|admin|member|viewer)",
            row.role
        ));
    };
    // Existing user → skip, never modify. role_for_user resolves any
    // stored account (every row has a role, defaulted on load).
    if connect_users::role_for_user(&row.username).is_some() {
        return SeedOutcome::SkippedExists;
    }
    if let Err(e) = connect_users::add_user(&row.username, &row.password) {
        return SeedOutcome::Failed(format!("add_user: {e}"));
    }
    // add_user defaults to Member; only non-default roles need a write.
    if role != Role::Member {
        if let Err(e) = connect_users::set_role(&row.username, role) {
            return SeedOutcome::Failed(format!("created, but set_role: {e}"));
        }
    }
    if row.must_change_password {
        if let Err(e) = connect_users::set_must_change_password(&row.username, true) {
            return SeedOutcome::Failed(format!("created, but mustChangePassword: {e}"));
        }
    }
    SeedOutcome::Created
}
