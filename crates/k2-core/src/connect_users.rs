//! Connect-user accounts + sessions for the PUBLIC K2 Connect tunnel
//! surface (K2SO #617).
//!
//! **Two auth levels.** The daemon owner holds the local daemon token
//! (`~/.k2so/daemon.token`); that token is the OWNER credential — full
//! access including user management + tunnel control. A *connect-user*
//! is a username+password account the owner provisions; the remote
//! person logs in over `https://<sub>.k2.dev` and receives a session
//! token they then pass on every request. A connect-user gets general
//! daemon access but is NOT the owner (no user management, no tunnel
//! control). Granular per-user scopes are a FUTURE pass.
//!
//! **Storage.** Accounts live in `~/.k2so/connect-users.json`, written
//! 0600 (owner-only) via tmp+rename — mirroring `tunnel/config.rs`. The
//! file holds argon2id password *hashes*, never plaintext. Sessions are
//! in-memory only (an `OnceLock<Mutex<HashMap>>` singleton like the
//! tunnel connector): a daemon restart simply forces a re-login, which
//! is fine for a session token.
//!
//! **Security posture.** This is the auth boundary for the public
//! tunnel. `verify` always runs an argon2 hash even for an unknown user
//! (constant-ish work) to blunt timing-based user enumeration. The
//! daemon route layer adds a fixed failure delay on top to slow
//! brute-force; richer rate-limiting is deferred.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Default session lifetime. A connect-user's session token is valid
/// for 30 days from issue; after that they re-login. In-memory store
/// means a daemon restart also ends the session early.
const SESSION_TTL_DAYS: i64 = 30;

/// Brute-force lockout policy: this many consecutive failed password
/// attempts for one username triggers a lockout.
const LOCKOUT_THRESHOLD: u32 = 3;

/// How long a username stays locked once the threshold is hit.
const LOCKOUT_DURATION_MINUTES: i64 = 15;

/// Permission tier for a connect-user (K2SO #629). Strict hierarchy
/// `Owner > Admin > Member`. The local daemon-token holder (the host
/// machine's owner) is ALWAYS treated as `Owner` regardless of any stored
/// row — that authority lives in the token, not the file.
///
/// - **Owner**: add/remove/enable/disable ANY user + CHANGE ROLES + use
///   K2SO. Assignable to a connect-user.
/// - **Admin**: add/remove/enable/disable users + use K2SO. CANNOT change
///   roles; CANNOT act on an Owner-role user.
/// - **Member**: connect + use K2SO only. No user management. The DEFAULT
///   for existing rows (via `#[serde(default)]`) and newly added users.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Lowest tier — listed first so the derived `Ord` ranks
    /// `Member < Admin < Owner`.
    Member,
    Admin,
    Owner,
}

impl Default for Role {
    fn default() -> Self {
        // Existing stored rows (pre-#629) and new users default to Member.
        Role::Member
    }
}

impl Role {
    /// Parse a wire role string (`"owner"` | `"admin"` | `"member"`).
    /// Case-insensitive; returns `None` for anything else.
    pub fn from_wire(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            _ => None,
        }
    }

    /// The canonical wire string for this role (snake_case, matches serde).
    pub fn as_wire(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

/// Whether `role` may manage users at all (add/remove/enable/disable).
/// True for `Admin` and `Owner`; false for `Member`.
pub fn can_manage_users(role: Role) -> bool {
    matches!(role, Role::Admin | Role::Owner)
}

/// Whether `role` may CHANGE other users' roles. Owner only.
pub fn can_change_roles(role: Role) -> bool {
    matches!(role, Role::Owner)
}

/// Whether an actor of `actor` role may perform a management action
/// (remove/disable/etc.) on a target of `target` role.
///
/// - `Owner` can act on anyone (Owner/Admin/Member).
/// - `Admin` can act on `Admin`/`Member` but NOT on an `Owner`.
/// - `Member` can act on no one.
pub fn can_act_on(actor: Role, target: Role) -> bool {
    match actor {
        Role::Owner => true,
        Role::Admin => target != Role::Owner,
        Role::Member => false,
    }
}

/// A provisioned connect-user account. Persisted to
/// `~/.k2so/connect-users.json`. The `password_hash` is an argon2id
/// PHC string and is NEVER exposed off-disk (see [`ConnectUserView`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectUser {
    /// Unique, lowercased, `^[a-z0-9_-]{2,}$`.
    pub username: String,
    /// argon2id PHC hash string. Secret — only ever lives on the 0600
    /// file and is the input to [`verify`].
    pub password_hash: String,
    /// RFC3339 creation timestamp.
    pub created_at: DateTime<Utc>,
    /// When true the account cannot authenticate (login fails) but the
    /// row is retained so the owner can re-enable it.
    #[serde(default)]
    pub disabled: bool,
    /// Permission tier (K2SO #629). `#[serde(default)]` → `Member` so
    /// pre-#629 stored rows (no `role` field) deserialize as Member.
    #[serde(default)]
    pub role: Role,
}

/// Redacted projection of a [`ConnectUser`] safe to return over the
/// wire — username + created_at + disabled, NEVER the hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectUserView {
    pub username: String,
    pub created_at: DateTime<Utc>,
    pub disabled: bool,
    /// Permission tier (K2SO #629). `#[serde(default)]` mirrors the stored
    /// row so a view round-trips even from a pre-#629 source.
    #[serde(default)]
    pub role: Role,
}

impl From<&ConnectUser> for ConnectUserView {
    fn from(u: &ConnectUser) -> Self {
        Self {
            username: u.username.clone(),
            created_at: u.created_at,
            disabled: u.disabled,
            role: u.role,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Password policy (K2SO #620)
// ─────────────────────────────────────────────────────────────────────

/// Owner-configurable, server-enforced complexity policy for connect-user
/// passwords. Persisted alongside the user list in
/// `~/.k2so/connect-users.json` under a top-level `policy` field. The
/// default preserves the pre-#620 behavior (length-8, no character-class
/// requirements) so existing stores load unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PasswordPolicy {
    /// Minimum password length. Clamped to a sane band on `set_policy`.
    pub min_length: usize,
    /// Require at least one non-alphanumeric character.
    pub require_special: bool,
    /// Require at least one ASCII digit.
    pub require_number: bool,
    /// Require at least one uppercase letter.
    pub require_uppercase: bool,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_length: 8,
            require_special: false,
            require_number: false,
            require_uppercase: false,
        }
    }
}

/// Lower/upper bounds the policy's `min_length` is clamped to on save so a
/// misconfigured owner can't lock everyone out (too high) or disable the
/// length floor entirely (too low).
pub const POLICY_MIN_LENGTH_FLOOR: usize = 4;
pub const POLICY_MIN_LENGTH_CEIL: usize = 128;

/// Validate `pw` against `policy`, returning the FIRST unmet requirement as
/// a human-readable message. `Ok(())` means the password satisfies every
/// active rule. A "special" character is any non-alphanumeric character.
pub fn validate_password(pw: &str, policy: &PasswordPolicy) -> Result<(), String> {
    if pw.chars().count() < policy.min_length {
        return Err(format!(
            "Password must be at least {} characters.",
            policy.min_length
        ));
    }
    if policy.require_special && !pw.chars().any(|c| !c.is_alphanumeric()) {
        return Err("Password must include a special character.".to_string());
    }
    if policy.require_number && !pw.chars().any(|c| c.is_ascii_digit()) {
        return Err("Password must include a number.".to_string());
    }
    if policy.require_uppercase && !pw.chars().any(|c| c.is_uppercase()) {
        return Err("Password must include an uppercase letter.".to_string());
    }
    Ok(())
}

/// Read the stored password policy (default when the store/file is absent
/// or the field is missing).
pub fn get_policy() -> PasswordPolicy {
    load_store().map(|s| s.policy).unwrap_or_default()
}

/// Persist a new password policy, clamping `min_length` into
/// `[POLICY_MIN_LENGTH_FLOOR, POLICY_MIN_LENGTH_CEIL]`.
pub fn set_policy(mut policy: PasswordPolicy) -> Result<(), String> {
    policy.min_length = policy
        .min_length
        .clamp(POLICY_MIN_LENGTH_FLOOR, POLICY_MIN_LENGTH_CEIL);
    update_store(|store| {
        store.policy = policy;
        Ok(())
    })
}

// ─────────────────────────────────────────────────────────────────────
// Validation
// ─────────────────────────────────────────────────────────────────────

/// Normalize + validate a requested username. Lowercases, then enforces
/// `^[a-z0-9_-]{2,}$`. Returns the canonical (lowercased) form on
/// success, or an error string describing the rejection.
pub fn normalize_username(raw: &str) -> Result<String, String> {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.len() < 2 {
        return Err("username must be at least 2 characters".to_string());
    }
    if !lowered
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(
            "username may contain only lowercase letters, digits, '_' and '-'".to_string(),
        );
    }
    Ok(lowered)
}

// ─────────────────────────────────────────────────────────────────────
// Hashing
// ─────────────────────────────────────────────────────────────────────

/// Hash a password with argon2id + a fresh per-user random salt.
fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

/// Verify a plaintext password against a stored argon2 PHC hash.
fn verify_hash(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// ─────────────────────────────────────────────────────────────────────
// Persistence (mirrors tunnel/config.rs: $HOME-relative, 0600, tmp+rename)
// ─────────────────────────────────────────────────────────────────────

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to `~/.k2so/connect-users.json`.
pub fn store_path() -> PathBuf {
    config_dir().join("connect-users.json")
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
        crate::log_debug!("[connect_users] WARN chmod 0600 {}: {e}", file.display());
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// On-disk shape: a flat list of accounts + the password policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    users: Vec<ConnectUser>,
    /// K2SO #620: owner-configurable password policy. `default` so existing
    /// files without the field still load (yielding the permissive default).
    #[serde(default)]
    policy: PasswordPolicy,
}

/// Load the store. A missing/empty file yields an empty store; a
/// malformed file is an error (fail loud — never silently drop accounts
/// over a corrupt credential store).
fn load_store() -> Result<Store, String> {
    let file = store_path();
    if !file.exists() {
        return Ok(Store::default());
    }
    let raw = fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    if raw.trim().is_empty() {
        return Ok(Store::default());
    }
    serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", file.display()))
}

/// Persist the store via tmp+rename then chmod 0600.
fn save_store(store: &Store) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let file = store_path();
    let tmp = dir.join(format!("connect-users.json.tmp.{}", std::process::id()));
    let body =
        serde_json::to_string_pretty(store).map_err(|e| format!("serialize store: {e}"))?;
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", file.display())
    })?;
    restrict_mode(&file);
    Ok(())
}

/// Load-modify-save under a process lock so concurrent mutations don't
/// clobber each other.
fn update_store<R>(f: impl FnOnce(&mut Store) -> Result<R, String>) -> Result<R, String> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut store = load_store()?;
    let r = f(&mut store)?;
    save_store(&store)?;
    Ok(r)
}

// ─────────────────────────────────────────────────────────────────────
// Account management (owner-only at the route layer)
// ─────────────────────────────────────────────────────────────────────

/// List all accounts as redacted views (no hashes). Sorted by username.
pub fn list_users() -> Result<Vec<ConnectUserView>, String> {
    let store = load_store()?;
    let mut views: Vec<ConnectUserView> = store.users.iter().map(ConnectUserView::from).collect();
    views.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(views)
}

/// Provision a new account. Validates + lowercases the username, rejects
/// duplicates, hashes the password with a fresh salt.
pub fn add_user(username: &str, password: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    let hash = hash_password(password)?;
    update_store(|store| {
        // Enforce the stored password policy (K2SO #620).
        validate_password(password, &store.policy)?;
        if store.users.iter().any(|u| u.username == username) {
            return Err(format!("user '{username}' already exists"));
        }
        store.users.push(ConnectUser {
            username,
            password_hash: hash,
            created_at: Utc::now(),
            disabled: false,
            // New users default to Member (K2SO #629); an Owner promotes
            // via set_role.
            role: Role::Member,
        });
        Ok(())
    })
}

/// Remove an account and revoke any live sessions for it.
pub fn remove_user(username: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    update_store(|store| {
        let before = store.users.len();
        store.users.retain(|u| u.username != username);
        if store.users.len() == before {
            return Err(format!("user '{username}' not found"));
        }
        Ok(())
    })?;
    revoke_user_sessions(&username);
    Ok(())
}

/// Reset an account's password (re-hashes) and revoke its live sessions
/// so the old session token can't be replayed after a rotation.
pub fn set_password(username: &str, password: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    let hash = hash_password(password)?;
    update_store(|store| {
        // Enforce the stored password policy (K2SO #620).
        validate_password(password, &store.policy)?;
        let user = store
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or_else(|| format!("user '{username}' not found"))?;
        user.password_hash = hash;
        Ok(())
    })?;
    revoke_user_sessions(&username);
    // K2SO #620: an owner reset must immediately UNLOCK a stuck user —
    // clear any lockout (failed_count/locked_until) so a correct login
    // succeeds right away. (change_password already clears on success via
    // check_and_record; this is specifically the owner-reset path.)
    clear_lockout(&username);
    Ok(())
}

/// Enable/disable an account. Disabling revokes live sessions so a
/// disabled user is immediately locked out, not just blocked at next
/// login.
pub fn set_disabled(username: &str, disabled: bool) -> Result<(), String> {
    let username = normalize_username(username)?;
    update_store(|store| {
        let user = store
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or_else(|| format!("user '{username}' not found"))?;
        user.disabled = disabled;
        Ok(())
    })?;
    if disabled {
        revoke_user_sessions(&username);
    }
    Ok(())
}

/// Set a user's permission role (K2SO #629). Authorization (only an
/// Owner may change roles) is enforced at the route layer; this is the
/// raw store mutation. Revokes the user's live sessions so the new role
/// takes effect immediately (forced re-login) rather than lingering for
/// up to the token TTL on already-issued sessions. Errors when the user
/// doesn't exist.
pub fn set_role(username: &str, role: Role) -> Result<(), String> {
    let username = normalize_username(username)?;
    update_store(|store| {
        let user = store
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or_else(|| format!("user '{username}' not found"))?;
        user.role = role;
        Ok(())
    })?;
    revoke_user_sessions(&username);
    Ok(())
}

/// Look up the stored role for `username`. `None` when the user doesn't
/// exist (or the store can't be read). Used by the route layer to resolve
/// a target's role before an `can_act_on` check.
pub fn role_for_user(username: &str) -> Option<Role> {
    let normalized = normalize_username(username).ok()?;
    let store = load_store().ok()?;
    store
        .users
        .iter()
        .find(|u| u.username == normalized)
        .map(|u| u.role)
}

/// Resolve the effective role for a session `token` (K2SO #629). Returns
/// the connect-user's stored role for a live session, else `None`.
///
/// NOTE: the OWNER (host daemon-token holder) is ALWAYS `Role::Owner`, but
/// that authority lives in the daemon token — NOT in any session — so the
/// route layer maps the owner token to `Role::Owner` BEFORE calling this.
/// This function only handles connect-user *session* tokens.
pub fn role_for_session(token: &str) -> Option<Role> {
    let username = validate_session(token)?;
    role_for_user(&username)
}

/// Verify a username+password against the store. Returns `true` only for
/// a known, NON-disabled account whose password matches.
///
/// **Anti-enumeration:** always runs an argon2 verify even when the user
/// is unknown (against a throwaway hash) so the response time doesn't
/// leak whether the username exists. No early return on miss.
pub fn verify(username: &str, password: &str) -> bool {
    // Normalize without leaking via early-return: an invalid username can
    // never match a stored (already-normalized) one, but we still do the
    // dummy hash work below before returning.
    let normalized = normalize_username(username).ok();

    let store = match load_store() {
        Ok(s) => s,
        Err(_) => None.unwrap_or_default(),
    };

    let found = normalized
        .as_ref()
        .and_then(|n| store.users.iter().find(|u| &u.username == n));

    // A process-wide dummy hash so the unknown/disabled branches incur the
    // same argon2 cost as a real verify — blunting user-enumeration timing.
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(build_dummy_hash);

    match found {
        Some(user) if !user.disabled => verify_hash(password, &user.password_hash),
        Some(_) => {
            // Disabled: still burn a hash cycle, then fail.
            let _ = verify_hash(password, dummy);
            false
        }
        None => {
            // Unknown user: run a verify against a fixed dummy hash so the
            // timing matches the found-user path. Result is discarded.
            let _ = verify_hash(password, dummy);
            false
        }
    }
}

fn build_dummy_hash() -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"k2so-dummy-verify-target", &salt)
        .map(|h| h.to_string())
        // Fall back to a baked PHC-shaped string only if hashing fails
        // (effectively never); verify against it will just return false.
        .unwrap_or_else(|_| String::from("$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
}

// ─────────────────────────────────────────────────────────────────────
// Brute-force lockout (in-memory, per-username)
// ─────────────────────────────────────────────────────────────────────

/// Per-username failed-attempt + lockout state. In-memory only (resets
/// on daemon restart, which is acceptable — a restart is rare and the
/// worst case is an attacker getting their counter reset, still bounded
/// by the slow argon2 verify + the route-layer fixed delay).
#[derive(Debug, Clone, Default)]
struct LockEntry {
    /// Consecutive failed password attempts since the last success/clear.
    failed_count: u32,
    /// If `Some`, the username is locked until this instant. While locked,
    /// attempts are rejected WITHOUT checking the password and do NOT
    /// extend the lock.
    locked_until: Option<DateTime<Utc>>,
}

fn locks() -> &'static Mutex<HashMap<String, LockEntry>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, LockEntry>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_locks() -> std::sync::MutexGuard<'static, HashMap<String, LockEntry>> {
    locks().lock().unwrap_or_else(|p| p.into_inner())
}

/// The outcome of a credential check routed through the lockout gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginOutcome {
    /// Credentials verified; the lockout counter has been cleared.
    Ok,
    /// Username is currently locked; the password was NOT checked.
    LockedOut,
    /// Wrong credentials (unknown user, disabled, or bad password). The
    /// failure has been recorded and may have just triggered a lockout.
    BadCreds,
}

/// Verify `password` for `username` THROUGH the brute-force lockout gate.
/// This is the single entry point the login + change-password paths use
/// so the policy can never be bypassed.
///
/// Policy:
/// - If the username is currently locked (`now < locked_until`): return
///   `LockedOut` WITHOUT verifying the password and WITHOUT extending the
///   lock (a locked attempt doesn't push the deadline out).
/// - Otherwise verify the password via [`verify`]:
///   - On success: clear the entry, return `Ok`.
///   - On failure: increment `failed_count`; at [`LOCKOUT_THRESHOLD`] set
///     `locked_until = now + LOCKOUT_DURATION` and reset the count to 0,
///     then return `BadCreds`.
///
/// Lockout is keyed by the *normalized* username so casing can't be used
/// to dodge the counter. An un-normalizable username can never match a
/// stored account; it's keyed under its lossy lowercase form so repeated
/// junk still gets rate-limited rather than silently uncounted.
pub fn check_and_record(username: &str, password: &str) -> LoginOutcome {
    let key = normalize_username(username).unwrap_or_else(|_| username.trim().to_ascii_lowercase());

    // First: are we currently locked? Hold the lock only long enough to
    // read; release before the (slow) argon2 verify.
    {
        let mut map = lock_locks();
        if let Some(entry) = map.get_mut(&key) {
            match entry.locked_until {
                Some(until) if Utc::now() < until => return LoginOutcome::LockedOut,
                Some(_) => {
                    // Lock expired — clear it and let this attempt proceed
                    // with a fresh counter.
                    entry.locked_until = None;
                    entry.failed_count = 0;
                }
                None => {}
            }
        }
    }

    // Not locked: do the real (slow) verify outside the lock.
    let ok = verify(username, password);

    let mut map = lock_locks();
    if ok {
        map.remove(&key);
        LoginOutcome::Ok
    } else {
        let entry = map.entry(key).or_default();
        entry.failed_count += 1;
        if entry.failed_count >= LOCKOUT_THRESHOLD {
            entry.locked_until = Some(Utc::now() + Duration::minutes(LOCKOUT_DURATION_MINUTES));
            entry.failed_count = 0;
        }
        LoginOutcome::BadCreds
    }
}

/// Result of a self-service password change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePasswordOutcome {
    /// Password changed; all of the user's sessions were revoked.
    Ok,
    /// Current password was wrong (routed through the lockout gate) OR the
    /// username is currently locked. Generic — the route must not reveal
    /// which.
    BadCurrent,
    /// New password failed the policy. Carries the human-readable first
    /// unmet requirement (K2SO #620) so the portal can surface it.
    WeakNew(String),
}

/// Self-service password change for an authenticated connect-user.
/// Verifies `current` THROUGH the lockout gate (so a self-service form
/// can't be used to brute-force the current password), validates the new
/// password against the stored policy (K2SO #620), then [`set_password`]
/// (which re-hashes AND revokes every live session for the user, forcing a
/// re-login everywhere).
pub fn change_password(username: &str, current: &str, new: &str) -> ChangePasswordOutcome {
    match check_and_record(username, current) {
        LoginOutcome::Ok => {}
        LoginOutcome::LockedOut | LoginOutcome::BadCreds => {
            return ChangePasswordOutcome::BadCurrent
        }
    }
    // Enforce the stored policy (replaces the old hardcoded >= 8 check).
    if let Err(msg) = validate_password(new, &get_policy()) {
        return ChangePasswordOutcome::WeakNew(msg);
    }
    // set_password re-hashes and revokes ALL of this user's sessions
    // (including the one that authorized this call) → forced re-login.
    match set_password(username, new) {
        Ok(()) => ChangePasswordOutcome::Ok,
        // The user was resolved from a live session, so the account
        // exists; a failure here is an unexpected store error. Treat as a
        // generic bad-current rather than leaking internals.
        Err(_) => ChangePasswordOutcome::BadCurrent,
    }
}

/// Clear any lockout state (failed_count + locked_until) for `username`.
/// Keyed by the normalized username — same key `check_and_record`/`is_locked`
/// use. Idempotent; a no-op when there's no entry. Used by the owner-reset
/// path so a password reset immediately unlocks a stuck user.
pub fn clear_lockout(username: &str) {
    let key = normalize_username(username).unwrap_or_else(|_| username.trim().to_ascii_lowercase());
    lock_locks().remove(&key);
}

/// Whether `username` is currently locked out. Read-only; used by callers
/// that want to surface a hint without attempting a verify.
pub fn is_locked(username: &str) -> bool {
    let key = normalize_username(username).unwrap_or_else(|_| username.trim().to_ascii_lowercase());
    match lock_locks().get(&key).and_then(|e| e.locked_until) {
        Some(until) => Utc::now() < until,
        None => false,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Sessions (in-memory singleton)
// ─────────────────────────────────────────────────────────────────────

/// A live login session for a connect-user.
#[derive(Debug, Clone)]
pub struct Session {
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_sessions() -> std::sync::MutexGuard<'static, HashMap<String, Session>> {
    sessions().lock().unwrap_or_else(|p| p.into_inner())
}

/// Generate a 32-byte (256-bit) random token, hex-encoded.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Issue a new session token for an authenticated `username`. Default
/// 30-day expiry. The caller is responsible for having already verified
/// credentials.
pub fn create_session(username: &str) -> String {
    let token = new_token();
    let session = Session {
        username: username.to_string(),
        expires_at: Utc::now() + Duration::days(SESSION_TTL_DAYS),
    };
    lock_sessions().insert(token.clone(), session);
    token
}

/// Validate a session token. Returns the owning username if the session
/// exists and hasn't expired; lazily drops an expired session.
pub fn validate_session(token: &str) -> Option<String> {
    let mut map = lock_sessions();
    let expired = match map.get(token) {
        Some(s) if s.expires_at > Utc::now() => return Some(s.username.clone()),
        Some(_) => true,
        None => false,
    };
    if expired {
        map.remove(token);
    }
    None
}

/// Revoke every live session belonging to `username`. Called on remove,
/// disable, and password rotation so a stale token can't outlive the
/// account change.
pub fn revoke_user_sessions(username: &str) {
    lock_sessions().retain(|_, s| s.username != username);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn hash_then_verify_round_trips() {
        let h = hash_password("correct horse battery staple").expect("hash");
        assert!(verify_hash("correct horse battery staple", &h));
    }

    #[test]
    fn wrong_password_fails_verify() {
        let h = hash_password("hunter2").expect("hash");
        assert!(!verify_hash("hunter3", &h));
    }

    #[test]
    fn normalize_username_lowercases_and_validates() {
        assert_eq!(normalize_username("Rosson").unwrap(), "rosson");
        assert_eq!(normalize_username("  a_b-1 ").unwrap(), "a_b-1");
        assert!(normalize_username("a").is_err(), "too short");
        assert!(normalize_username("has space").is_err());
        assert!(normalize_username("Bang!").is_err());
    }

    #[test]
    fn add_user_then_verify_succeeds() {
        with_temp_home(|| {
            add_user("Alice", "s3cretpass").expect("add");
            // Stored lowercased; verify is case-insensitive on the name.
            assert!(verify("alice", "s3cretpass"));
            assert!(verify("ALICE", "s3cretpass"));
            assert!(!verify("alice", "wrongpass"));
        });
    }

    #[test]
    fn verify_unknown_user_is_false_and_still_does_work() {
        with_temp_home(|| {
            // No users provisioned at all.
            assert!(!verify("ghost", "whatever"));
            // Add one, then check a different unknown user still false.
            add_user("real", "password").expect("add");
            assert!(!verify("nobody", "password"));
        });
    }

    #[test]
    fn add_rejects_duplicate() {
        with_temp_home(|| {
            add_user("bob", "password1").expect("add");
            let err = add_user("BOB", "password2").expect_err("dup must reject");
            assert!(err.contains("already exists"), "got: {err}");
        });
    }

    #[test]
    fn add_rejects_invalid_username() {
        with_temp_home(|| {
            assert!(add_user("x", "password").is_err(), "too short");
            assert!(add_user("bad name", "password").is_err(), "space");
            assert!(add_user("ok", "").is_err(), "empty password");
        });
    }

    #[test]
    fn list_users_redacts_hash() {
        with_temp_home(|| {
            add_user("carol", "password").expect("add");
            let views = list_users().expect("list");
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].username, "carol");
            assert!(!views[0].disabled);
            // ConnectUserView simply has no hash field — compile-time
            // guarantee. Serialize and confirm no hash leaks.
            let json = serde_json::to_string(&views[0]).unwrap();
            assert!(!json.contains("password"), "view leaked a hash: {json}");
        });
    }

    #[test]
    fn set_password_changes_credential_and_revokes_sessions() {
        with_temp_home(|| {
            add_user("dave", "oldpassword").expect("add");
            let tok = create_session("dave");
            assert_eq!(validate_session(&tok), Some("dave".to_string()));
            set_password("dave", "newpassword").expect("set-password");
            assert!(!verify("dave", "oldpassword"));
            assert!(verify("dave", "newpassword"));
            assert_eq!(validate_session(&tok), None, "rotation must revoke session");
        });
    }

    #[test]
    fn set_disabled_blocks_login_and_revokes_sessions() {
        with_temp_home(|| {
            add_user("eve", "password").expect("add");
            let tok = create_session("eve");
            set_disabled("eve", true).expect("disable");
            assert!(!verify("eve", "password"), "disabled user must not verify");
            assert_eq!(validate_session(&tok), None, "disable must revoke session");
            set_disabled("eve", false).expect("re-enable");
            assert!(verify("eve", "password"), "re-enabled user verifies again");
        });
    }

    #[test]
    fn set_role_revokes_sessions() {
        with_temp_home(|| {
            add_user("mallory", "password").expect("add");
            let tok = create_session("mallory");
            assert_eq!(
                validate_session(&tok),
                Some("mallory".to_string()),
                "fresh session is valid"
            );
            set_role("mallory", Role::Admin).expect("promote");
            assert_eq!(role_for_user("mallory"), Some(Role::Admin));
            assert_eq!(
                validate_session(&tok),
                None,
                "role change must revoke the old session so the new role takes effect on re-login"
            );
        });
    }

    #[test]
    fn session_create_validate_expire() {
        with_temp_home(|| {
            let tok = create_session("frank");
            assert_eq!(validate_session(&tok), Some("frank".to_string()));

            // Construct an already-expired session directly and confirm
            // validate drops it.
            let expired_tok = new_token();
            lock_sessions().insert(
                expired_tok.clone(),
                Session {
                    username: "frank".to_string(),
                    expires_at: Utc::now() - Duration::seconds(1),
                },
            );
            assert_eq!(validate_session(&expired_tok), None, "expired -> None");
            // Lazily dropped.
            assert!(
                !lock_sessions().contains_key(&expired_tok),
                "expired session must be evicted on validate"
            );
        });
    }

    #[test]
    fn remove_user_revokes_sessions() {
        with_temp_home(|| {
            add_user("grace", "password").expect("add");
            let tok = create_session("grace");
            assert_eq!(validate_session(&tok), Some("grace".to_string()));
            remove_user("grace").expect("remove");
            assert_eq!(validate_session(&tok), None, "remove must revoke session");
            assert!(!verify("grace", "password"), "removed user must not verify");
            let err = remove_user("grace").expect_err("removing twice errors");
            assert!(err.contains("not found"), "got: {err}");
        });
    }

    #[test]
    fn unknown_token_validates_to_none() {
        with_temp_home(|| {
            assert_eq!(validate_session("deadbeef"), None);
        });
    }

    #[test]
    fn lockout_after_three_fails_rejects_even_correct_password() {
        with_temp_home(|| {
            add_user("lock1", "rightpass").expect("add");
            // Three wrong attempts → locked.
            assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
            assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
            assert_eq!(check_and_record("lock1", "x"), LoginOutcome::BadCreds);
            assert!(is_locked("lock1"), "3 fails must lock");
            // Now even the CORRECT password is rejected as LockedOut
            // (and the password is not even checked).
            assert_eq!(
                check_and_record("lock1", "rightpass"),
                LoginOutcome::LockedOut,
                "locked account rejects correct password without checking"
            );
        });
    }

    #[test]
    fn success_before_threshold_clears_failed_count() {
        with_temp_home(|| {
            add_user("lock2", "rightpass").expect("add");
            assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
            assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
            // Success on the 3rd attempt clears the counter.
            assert_eq!(check_and_record("lock2", "rightpass"), LoginOutcome::Ok);
            assert!(!is_locked("lock2"));
            // Two more fails should NOT lock (counter was reset).
            assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
            assert_eq!(check_and_record("lock2", "x"), LoginOutcome::BadCreds);
            assert!(!is_locked("lock2"), "counter must have reset on success");
        });
    }

    #[test]
    fn locked_attempt_does_not_extend_lock_and_expiry_clears() {
        with_temp_home(|| {
            add_user("lock3", "rightpass").expect("add");
            for _ in 0..LOCKOUT_THRESHOLD {
                assert_eq!(check_and_record("lock3", "x"), LoginOutcome::BadCreds);
            }
            assert!(is_locked("lock3"));
            // Manually force the lock to have already expired, simulating
            // the 15-minute window elapsing.
            {
                let mut map = lock_locks();
                let e = map.get_mut("lock3").expect("entry");
                e.locked_until = Some(Utc::now() - Duration::seconds(1));
            }
            assert!(!is_locked("lock3"), "expired lock reads as unlocked");
            // After expiry the correct password works again and clears
            // the entry.
            assert_eq!(check_and_record("lock3", "rightpass"), LoginOutcome::Ok);
        });
    }

    #[test]
    fn change_password_happy_path_revokes_sessions() {
        with_temp_home(|| {
            add_user("chp1", "oldpassword").expect("add");
            let tok = create_session("chp1");
            assert_eq!(validate_session(&tok), Some("chp1".to_string()));
            assert_eq!(
                change_password("chp1", "oldpassword", "newpassword"),
                ChangePasswordOutcome::Ok
            );
            assert!(verify("chp1", "newpassword"));
            assert!(!verify("chp1", "oldpassword"));
            assert_eq!(
                validate_session(&tok),
                None,
                "change-password must revoke existing sessions"
            );
        });
    }

    #[test]
    fn change_password_wrong_current_is_bad_current() {
        with_temp_home(|| {
            add_user("chp2", "oldpassword").expect("add");
            assert_eq!(
                change_password("chp2", "WRONG", "newpassword"),
                ChangePasswordOutcome::BadCurrent
            );
            // Password unchanged.
            assert!(verify("chp2", "oldpassword"));
        });
    }

    #[test]
    fn change_password_short_new_is_weak() {
        with_temp_home(|| {
            add_user("chp3", "oldpassword").expect("add");
            assert_eq!(
                change_password("chp3", "oldpassword", "short"),
                ChangePasswordOutcome::WeakNew(
                    "Password must be at least 8 characters.".to_string()
                )
            );
            assert!(verify("chp3", "oldpassword"), "weak-new must not change pw");
        });
    }

    #[test]
    fn change_password_through_lockout_locks_after_three_wrong_current() {
        with_temp_home(|| {
            add_user("chp4", "oldpassword").expect("add");
            for _ in 0..LOCKOUT_THRESHOLD {
                assert_eq!(
                    change_password("chp4", "WRONG", "newpassword"),
                    ChangePasswordOutcome::BadCurrent
                );
            }
            assert!(is_locked("chp4"), "self-service form is subject to lockout");
            // Even the correct current password is now rejected.
            assert_eq!(
                change_password("chp4", "oldpassword", "newpassword"),
                ChangePasswordOutcome::BadCurrent
            );
        });
    }

    #[test]
    fn token_is_64_hex_chars() {
        let t = new_token();
        assert_eq!(t.len(), 64, "32 bytes -> 64 hex chars");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── K2SO #620 — password policy ─────────────────────────────────────

    #[test]
    fn default_policy_preserves_legacy_behavior() {
        let p = PasswordPolicy::default();
        assert_eq!(p.min_length, 8);
        assert!(!p.require_special);
        assert!(!p.require_number);
        assert!(!p.require_uppercase);
    }

    #[test]
    fn policy_round_trips_in_store() {
        with_temp_home(|| {
            // Absent file → default.
            assert_eq!(get_policy(), PasswordPolicy::default());
            let want = PasswordPolicy {
                min_length: 12,
                require_special: true,
                require_number: true,
                require_uppercase: true,
            };
            set_policy(want.clone()).expect("set policy");
            assert_eq!(get_policy(), want, "policy must round-trip on disk");
            // Adding a user must NOT disturb the stored policy.
            add_user("pol1", "Abcdef1!ghij").expect("add under policy");
            assert_eq!(get_policy(), want, "policy survives a user mutation");
        });
    }

    #[test]
    fn set_policy_clamps_min_length() {
        with_temp_home(|| {
            set_policy(PasswordPolicy {
                min_length: 1,
                ..Default::default()
            })
            .expect("set");
            assert_eq!(get_policy().min_length, POLICY_MIN_LENGTH_FLOOR);
            set_policy(PasswordPolicy {
                min_length: 9999,
                ..Default::default()
            })
            .expect("set");
            assert_eq!(get_policy().min_length, POLICY_MIN_LENGTH_CEIL);
        });
    }

    #[test]
    fn validate_password_enforces_each_rule_with_message() {
        // Length.
        let p = PasswordPolicy {
            min_length: 10,
            ..Default::default()
        };
        assert_eq!(
            validate_password("short", &p),
            Err("Password must be at least 10 characters.".to_string())
        );
        assert!(validate_password("longenough!", &p).is_ok());

        // Special.
        let p = PasswordPolicy {
            min_length: 4,
            require_special: true,
            ..Default::default()
        };
        assert_eq!(
            validate_password("abcd1234", &p),
            Err("Password must include a special character.".to_string())
        );
        assert!(validate_password("abcd!", &p).is_ok());

        // Number.
        let p = PasswordPolicy {
            min_length: 4,
            require_number: true,
            ..Default::default()
        };
        assert_eq!(
            validate_password("abcdef", &p),
            Err("Password must include a number.".to_string())
        );
        assert!(validate_password("abc1", &p).is_ok());

        // Uppercase.
        let p = PasswordPolicy {
            min_length: 4,
            require_uppercase: true,
            ..Default::default()
        };
        assert_eq!(
            validate_password("abcdef", &p),
            Err("Password must include an uppercase letter.".to_string())
        );
        assert!(validate_password("abcD", &p).is_ok());
    }

    #[test]
    fn validate_password_reports_first_unmet_rule() {
        // All rules on; a too-short pw must complain about LENGTH first.
        let p = PasswordPolicy {
            min_length: 12,
            require_special: true,
            require_number: true,
            require_uppercase: true,
        };
        assert_eq!(
            validate_password("aB1!", &p),
            Err("Password must be at least 12 characters.".to_string())
        );
    }

    #[test]
    fn add_user_rejects_password_failing_policy() {
        with_temp_home(|| {
            set_policy(PasswordPolicy {
                min_length: 10,
                require_special: true,
                ..Default::default()
            })
            .expect("set");
            // Too short → length message.
            let err = add_user("polA", "abc").expect_err("too short rejected");
            assert_eq!(err, "Password must be at least 10 characters.");
            // Long enough but no special char → special message.
            let err = add_user("polA", "abcdefghij").expect_err("missing special rejected");
            assert_eq!(err, "Password must include a special character.");
            // Satisfies the policy → accepted.
            add_user("polA", "abcdefghi!").expect("compliant add");
        });
    }

    #[test]
    fn change_password_rejects_password_failing_policy() {
        with_temp_home(|| {
            // Add under the default (permissive) policy, then tighten it.
            add_user("polB", "oldpassword").expect("add");
            set_policy(PasswordPolicy {
                min_length: 8,
                require_special: true,
                ..Default::default()
            })
            .expect("set");
            assert_eq!(
                change_password("polB", "oldpassword", "newpassword"),
                ChangePasswordOutcome::WeakNew(
                    "Password must include a special character.".to_string()
                )
            );
            assert!(verify("polB", "oldpassword"), "weak-new must not change pw");
            // Compliant new password succeeds.
            assert_eq!(
                change_password("polB", "oldpassword", "newpass!"),
                ChangePasswordOutcome::Ok
            );
            assert!(verify("polB", "newpass!"));
        });
    }

    // ── K2SO #629 — role model ──────────────────────────────────────────

    #[test]
    fn role_ordering_is_member_lt_admin_lt_owner() {
        assert!(Role::Member < Role::Admin);
        assert!(Role::Admin < Role::Owner);
        assert!(Role::Member < Role::Owner);
        assert_eq!(Role::default(), Role::Member);
    }

    #[test]
    fn role_wire_round_trips() {
        for r in [Role::Owner, Role::Admin, Role::Member] {
            assert_eq!(Role::from_wire(r.as_wire()), Some(r));
        }
        // Case-insensitive + trimmed.
        assert_eq!(Role::from_wire("  OWNER "), Some(Role::Owner));
        assert_eq!(Role::from_wire("Admin"), Some(Role::Admin));
        assert_eq!(Role::from_wire("nonsense"), None);
        // serde uses the same snake_case strings.
        assert_eq!(serde_json::to_string(&Role::Owner).unwrap(), "\"owner\"");
        assert_eq!(serde_json::to_string(&Role::Member).unwrap(), "\"member\"");
    }

    #[test]
    fn can_manage_users_admin_and_owner_only() {
        assert!(can_manage_users(Role::Owner));
        assert!(can_manage_users(Role::Admin));
        assert!(!can_manage_users(Role::Member));
    }

    #[test]
    fn can_change_roles_owner_only() {
        assert!(can_change_roles(Role::Owner));
        assert!(!can_change_roles(Role::Admin));
        assert!(!can_change_roles(Role::Member));
    }

    #[test]
    fn can_act_on_enforces_admin_cannot_touch_owner() {
        // Owner can act on anyone.
        assert!(can_act_on(Role::Owner, Role::Owner));
        assert!(can_act_on(Role::Owner, Role::Admin));
        assert!(can_act_on(Role::Owner, Role::Member));
        // Admin can act on Admin/Member but NOT Owner.
        assert!(!can_act_on(Role::Admin, Role::Owner));
        assert!(can_act_on(Role::Admin, Role::Admin));
        assert!(can_act_on(Role::Admin, Role::Member));
        // Member can act on no one.
        assert!(!can_act_on(Role::Member, Role::Owner));
        assert!(!can_act_on(Role::Member, Role::Admin));
        assert!(!can_act_on(Role::Member, Role::Member));
    }

    #[test]
    fn new_user_defaults_to_member_and_set_role_promotes() {
        with_temp_home(|| {
            add_user("rolea", "password").expect("add");
            assert_eq!(role_for_user("rolea"), Some(Role::Member));
            set_role("rolea", Role::Admin).expect("promote");
            assert_eq!(role_for_user("rolea"), Some(Role::Admin));
            set_role("rolea", Role::Owner).expect("promote owner");
            assert_eq!(role_for_user("rolea"), Some(Role::Owner));
            // Unknown user errors.
            assert!(set_role("ghost", Role::Admin).is_err());
            assert_eq!(role_for_user("ghost"), None);
        });
    }

    #[test]
    fn role_for_session_resolves_stored_role() {
        with_temp_home(|| {
            add_user("rsess", "password").expect("add");
            set_role("rsess", Role::Admin).expect("promote");
            let tok = create_session("rsess");
            assert_eq!(role_for_session(&tok), Some(Role::Admin));
            // Unknown token → None.
            assert_eq!(role_for_session("deadbeef"), None);
        });
    }

    #[test]
    fn list_users_includes_role() {
        with_temp_home(|| {
            add_user("rl1", "password").expect("add");
            set_role("rl1", Role::Admin).expect("promote");
            let views = list_users().expect("list");
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].role, Role::Admin);
            // Wire shape carries the role string.
            let json = serde_json::to_string(&views[0]).unwrap();
            assert!(json.contains("\"role\":\"admin\""), "got: {json}");
        });
    }

    #[test]
    fn legacy_json_without_role_migrates_to_member() {
        with_temp_home(|| {
            // Hand-write a pre-#629 store: a user row with NO `role` field.
            let dir = config_dir();
            fs::create_dir_all(&dir).unwrap();
            let legacy = r#"{
                "users": [
                    {
                        "username": "legacy",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "created_at": "2024-01-01T00:00:00Z",
                        "disabled": false
                    }
                ]
            }"#;
            fs::write(store_path(), legacy).unwrap();
            // Deserializes cleanly; missing role → Member.
            let views = list_users().expect("legacy store loads");
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].username, "legacy");
            assert_eq!(views[0].role, Role::Member, "missing role defaults to Member");
            assert_eq!(role_for_user("legacy"), Some(Role::Member));
        });
    }

    #[test]
    fn owner_set_password_clears_lockout() {
        with_temp_home(|| {
            add_user("locked", "rightpass").expect("add");
            // Drive the account into a lockout via the lockout path.
            for _ in 0..LOCKOUT_THRESHOLD {
                assert_eq!(check_and_record("locked", "x"), LoginOutcome::BadCreds);
            }
            assert!(is_locked("locked"), "must be locked after threshold");
            // Even the correct password is rejected while locked.
            assert_eq!(
                check_and_record("locked", "rightpass"),
                LoginOutcome::LockedOut
            );
            // Owner resets the password → lockout cleared immediately.
            set_password("locked", "brandnewpw").expect("owner reset");
            assert!(!is_locked("locked"), "owner reset must clear lockout");
            // A correct login with the NEW password now succeeds right away.
            assert_eq!(
                check_and_record("locked", "brandnewpw"),
                LoginOutcome::Ok,
                "unlocked user logs in immediately after owner reset"
            );
        });
    }

    // ── K2SO #631 (P1) — connect-users store security ───────────────────

    /// The credential store holds argon2 password hashes; it MUST land on
    /// disk owner-only (0600). Locks the `restrict_mode` chmod on both the
    /// initial write (add_user) AND a subsequent rewrite (set_policy), since
    /// the tmp+rename path re-chmods the final file each time.
    #[cfg(unix)]
    #[test]
    fn store_file_is_written_0600() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home(|| {
            add_user("permu", "password").expect("add");
            let mode = fs::metadata(store_path())
                .expect("stat store")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, 0o600,
                "credential store must be owner-only 0600, got {mode:o}"
            );
            // A second write (different mutation, same tmp+rename path) must
            // also leave it 0600 — rename replaces the inode, so perms are
            // re-applied, not inherited.
            set_policy(PasswordPolicy {
                min_length: 10,
                ..Default::default()
            })
            .expect("set policy");
            let mode2 = fs::metadata(store_path())
                .expect("stat store after rewrite")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode2, 0o600,
                "store must stay 0600 after a rewrite, got {mode2:o}"
            );
        });
    }

    /// The lockout counter is keyed by username and shared by BOTH credential
    /// surfaces — interactive login (`check_and_record`) and self-service
    /// change-password (`change_password`, which routes its current-password
    /// check through the SAME gate). An attacker must not be able to dodge the
    /// 3-strike lockout by alternating between the two endpoints.
    #[test]
    fn lockout_counter_shared_across_login_and_change_password() {
        with_temp_home(|| {
            add_user("mixlock", "rightpass").expect("add");
            // Two failed LOGINs — counter at 2, not yet locked.
            assert_eq!(check_and_record("mixlock", "x"), LoginOutcome::BadCreds);
            assert_eq!(check_and_record("mixlock", "x"), LoginOutcome::BadCreds);
            assert!(!is_locked("mixlock"), "two fails must not lock yet");
            // A failed CHANGE-PASSWORD (wrong current) trips the SAME counter
            // to the threshold → locked. Proves one shared lockout key.
            assert_eq!(
                change_password("mixlock", "WRONG", "newpassword"),
                ChangePasswordOutcome::BadCurrent
            );
            assert!(
                is_locked("mixlock"),
                "3rd fail (via change-password) must lock the shared counter"
            );
            // And the lock applies back on the LOGIN surface: the correct
            // password is rejected without being checked.
            assert_eq!(
                check_and_record("mixlock", "rightpass"),
                LoginOutcome::LockedOut,
                "a lockout earned via change-password also blocks login"
            );
        });
    }

    /// Pre-#620 stores have no top-level `policy` field. They must load
    /// (the `#[serde(default)]` on `Store::policy` yields the permissive
    /// default) and remain fully usable — the serde-migration sibling of
    /// `legacy_json_without_role_migrates_to_member`.
    #[test]
    fn legacy_json_without_policy_loads_default_policy() {
        with_temp_home(|| {
            let dir = config_dir();
            fs::create_dir_all(&dir).unwrap();
            // A store with a user row but NO `policy` field and NO `role`.
            let legacy = r#"{
                "users": [
                    {
                        "username": "oldtimer",
                        "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "created_at": "2024-01-01T00:00:00Z",
                        "disabled": false
                    }
                ]
            }"#;
            fs::write(store_path(), legacy).unwrap();
            // Missing policy → permissive default.
            assert_eq!(
                get_policy(),
                PasswordPolicy::default(),
                "absent policy field must yield the default policy"
            );
            // And the store is still usable: the legacy user lists, and a new
            // add succeeds under the default (length-8) policy.
            let views = list_users().expect("legacy store loads");
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].username, "oldtimer");
            add_user("newcomer", "password").expect("add under default policy");
            assert_eq!(list_users().unwrap().len(), 2);
        });
    }
}
