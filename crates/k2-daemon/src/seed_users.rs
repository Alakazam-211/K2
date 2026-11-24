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
//! `tests/it_misc/provision_users_integration.rs`):
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
//! - An `owner` row is made only on the server's first start (no K2
//!   database yet): owner logins made later take this computer's own user
//!   check (`connect_users_routes::owner_login_gate`), and a file in
//!   `~/.k2` is no such check. Other rows apply as before.
//!
//! Called from `main.rs` once the config dir exists, before/independent
//! of the listeners. Idempotent + instant no-op when the file is absent
//! (every normal boot).

use k2_core::connect_users::{self, Role};
use k2_core::{cli_name, log_debug};

/// Path to the consume-once seed file: `~/.k2/seed-users.json`.
pub fn seed_file_path() -> std::path::PathBuf {
    k2_core::paths::k2_home()
        .join("seed-users.json")
}

/// One row of the seed file. `role` stays a string here so ONE bad row
/// fails at apply time (logged, others proceed) instead of failing the
/// whole-file parse.
#[derive(Debug, serde::Deserialize)]
struct SeedUser {
    username: String,
    password: String,
    /// "owner" | "admin" | "member". The retired "viewer" fails the row
    /// with [`connect_users::VIEWER_ROLE_REMOVED`] (nobody is created).
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

    let first_start = is_first_start(&k2_core::paths::k2_home());
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let outcome = apply_row(&row, first_start);
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

/// The text of an owner row on a server that has started before.
pub const OWNER_ROW_AFTER_FIRST_START: &str =
    concat!("an owner login from the seed file is made only on the server's first start; the owner adds one in person (", cli_name!(), " users set-role asks for this computer's own user check)");

/// This server's first start: the K2 database isn't there yet (it is
/// opened right after the seed file is read).
pub fn is_first_start(k2_home: &std::path::Path) -> bool {
    !k2_home.join("k2.db").exists() && !k2_home.join("k2so.db").exists()
}

fn apply_row(row: &SeedUser, first_start: bool) -> SeedOutcome {
    let role = match Role::parse_wire(&row.role) {
        Ok(r) => r,
        Err(e) => return SeedOutcome::Failed(e),
    };
    if role == Role::Owner && !first_start {
        return SeedOutcome::Failed(OWNER_ROW_AFTER_FIRST_START.to_string());
    }
    // Existing user → skip, never modify. role_for_user resolves any
    // stored account (every row has a role, defaulted on load).
    if connect_users::role_for_user(&row.username).is_some() {
        return SeedOutcome::SkippedExists;
    }
    // prd-voice-chat-only-members-v1 V5: made at its role and password
    // flag in one write (a voice_chat row is never a Member, even briefly).
    let new = connect_users::NewLogin {
        role: Some(role),
        must_change_password: row.must_change_password,
        shares: Default::default(),
        fixed_password: false,
    };
    if let Err(e) = connect_users::add_user_with_role(&row.username, &row.password, None, None, &new) {
        return SeedOutcome::Failed(format!("add_user: {e}"));
    }
    SeedOutcome::Created
}
