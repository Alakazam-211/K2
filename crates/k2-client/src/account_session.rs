//! The k2.dev account session in the OS keychain (`dev.k2.connect.account`
//! / `session`): read, write with the trusted-app ACL, clear.
//!
//! Moved out of `k2_core::tunnel::lease` (0.46.4 build-speed lane) so the
//! thin client's `k2_account_session_*` commands share the one writer
//! without linking k2-core. `k2_core::tunnel::lease` re-exports every item
//! here, so daemon call sites are unchanged; the lease renewal loop and the
//! boot migration (`migrate_account_keychain`) stay in k2-core.

use serde::Deserialize;

/// Keychain coordinates for the k2.dev ACCOUNT session. MUST match the
/// renderer's `ACCOUNT_KEYCHAIN_SERVICE` / `SESSION_BLOB_KEY` in
/// `K2ConnectSection.tsx` so the daemon reads the very session the client
/// stored at sign-in.
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const ACCOUNT_KEYCHAIN_SERVICE: &str = "dev.k2.connect.account";
/// Pre-0.40 service name — read-only fallback, migrated on first read.
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const LEGACY_ACCOUNT_KEYCHAIN_SERVICE: &str = "com.k2so.connect.account";
/// Human-facing item LABEL (kSecAttrLabel). The service above is the
/// stable lookup KEY; the label is the only string macOS shows in the
/// "K2 wants to use your confidential information stored in '<label>'"
/// keychain dialog. macOS owns that dialog's body text entirely (no
/// per-prompt reason string like osascript's `with prompt`), so a clear
/// label is the only lever we have to explain what's being unlocked.
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const ACCOUNT_KEYCHAIN_LABEL: &str = "K2 Connect sign-in";
/// CURRENT session layout (renderer double-prompt fix, `SESSION_BLOB_KEY`
/// in `K2ConnectSection.tsx`): ONE keychain item under this account key
/// holding a JSON blob `{"refreshToken": "...", "email": "..."}`.
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const ACCOUNT_SESSION_BLOB_KEY: &str = "session";
/// LEGACY two-item layout: the bare refresh-token string. Read fallback
/// only — the renderer DELETES this key on every fresh sign-in
/// (`saveAccountSession`), so reading it alone silently breaks after the
/// first blob-era sign-in (the K2 Cloud P1-C bug this module fixes).
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const ACCOUNT_REFRESH_KEY: &str = "session-refresh-token";
/// LEGACY two-item layout: the companion email item. Read only during
/// migration so the forwarded blob keeps the email the renderer displays.
#[cfg(target_os = "macos")] // consumed only by the mac keychain paths below
pub const ACCOUNT_EMAIL_KEY: &str = "session-email";

// ── Account session material (pure, keychain-free seam) ─────────────────

/// The account session the renderer persists at sign-in, as far as the
/// daemon needs it: the refresh token (drives renewal) plus the account
/// email (carried through rotation write-backs so the renderer's blob
/// keeps the email it displays).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSession {
    pub refresh_token: String,
    /// From the blob's `email` field; `None` when absent/empty (e.g. a
    /// session recovered from the legacy string-only entry).
    pub email: Option<String>,
}

/// Wire shape of the renderer's persisted session blob — see
/// `saveAccountSession` in `K2ConnectSection.tsx`:
/// `JSON.stringify({ refreshToken, email })`.
#[derive(Debug, Deserialize)]
struct SessionBlob {
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// Pure decision seam (unit-testable without a keychain): resolve the
/// session from raw keychain material. `blob` = the JSON under the CURRENT
/// single-item key (`session`); `legacy` = the bare refresh-token string
/// under the OLD key (`session-refresh-token`). The blob wins; a corrupt
/// or token-less blob falls back to the legacy entry — mirroring the
/// renderer's `readAccountSession` order.
pub fn resolve_account_session(
    blob: Option<&str>,
    legacy: Option<&str>,
) -> Option<AccountSession> {
    if let Some(raw) = blob {
        if let Ok(parsed) = serde_json::from_str::<SessionBlob>(raw) {
            if let Some(token) = parsed
                .refresh_token
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                return Some(AccountSession {
                    refresh_token: token.to_string(),
                    email: parsed
                        .email
                        .as_deref()
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .map(str::to_string),
                });
            }
        }
        // Corrupt / token-less blob → fall through to the legacy entry.
    }
    let token = legacy?.trim();
    if token.is_empty() {
        return None;
    }
    Some(AccountSession {
        refresh_token: token.to_string(),
        email: None,
    })
}

/// Serialize a session into the EXACT blob shape the renderer persists and
/// parses back: `{"refreshToken": "...", "email": "..."}`. `email` is
/// always emitted as a string (the renderer reads it with
/// `parsed.email ?? ''` and its `AccountSession` type requires a string).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn session_blob_json(sess: &AccountSession) -> String {
    serde_json::json!({
        "refreshToken": sess.refresh_token,
        "email": sess.email.as_deref().unwrap_or(""),
    })
    .to_string()
}

// ── Keychain (account refresh token) ────────────────────────────────────
//
// The renderer persists the refresh token via the `keyring` crate
// (apple-native on macOS), which writes a standard generic-password
// item. The daemon reads it back with the `security` CLI — the same
// mechanism `companion::keychain` already relies on for cross-process
// access to a keyring-written secret.
//
// ## Why this item prompts, and the ACL fix (0.40.7)
//
// On macOS the `keyring` crate (and the `security` CLI) operate on the
// LEGACY file-based **login keychain** via `SecKeychainAddGenericPassword`
// / `find_generic_password`. Access to an item in that keychain is gated by
// the item's **ACL** — a per-item list of *trusted applications* — NOT by
// the modern `keychain-access-groups` entitlement (that entitlement only
// governs the iOS-style data-protection keychain, which neither keyring nor
// the `security` CLI use on macOS). So the entitlement added in 0.40.6
// makes "Always Allow" survive relaunches for the *creating app's* own
// signature, but it does nothing for a DIFFERENT binary touching the item.
//
// When the renderer (`keyring`) creates the item, its default ACL trusts
// only the creating app (`K2.app/Contents/MacOS/k2`). Every OTHER process
// that touches it then provokes the login-keychain password prompt:
//
//   * the daemon READ shells `/usr/bin/security find-generic-password`, so
//     the keychain sees `/usr/bin/security` as the requester — not on the
//     ACL → prompt, and "Always Allow" can't persist a grant for a tool
//     that re-launches as a fresh process each time;
//   * even reading via the Security framework from the daemon would make
//     `k2-daemon` the requester — still a different app than the writer
//     (`k2`), still not on the ACL → still a prompt — and the `keyring`
//     crate exposes NO API to add trusted apps to an item's ACL.
//
// The only lever on the login keychain is the `security` CLI's `-T <path>`
// flag (set the item's trusted-application list at write time). So we keep
// the `security` CLI and, whenever the daemon (re)writes the item, we stamp
// it with an explicit ACL that trusts every binary that legitimately reads
// it. See [`acl_trusted_apps`] for the exact `-T` set and the per-entry
// rationale tied to which process performs each access.
//
// `-U` updates the VALUE of an existing item but does NOT reset its ACL, so
// to (re)apply the ACL we DELETE + re-ADD (a plain add — no `-U` — installs
// a fresh ACL). The token value is preserved across the delete/re-add. This
// also self-heals a legacy item the renderer created without our ACL: the
// first daemon write after updating to 0.40.7 upgrades it in place.

/// The `-T` trusted-application set stamped onto the account keychain item
/// so every binary that reads it does so WITHOUT a login-keychain prompt.
///
/// Returns absolute paths to pass as `-T <path>`. Each entry is justified by
/// the process that actually performs a keychain access against this item:
///
/// 1. `/usr/bin/security` — the daemon's READ path
///    ([`read_keychain_item`]) shells `security find-generic-password`, so
///    the keychain sees `/usr/bin/security` as the requesting application on
///    every daemon read. This is the load-bearing entry.
/// 2. the running daemon executable (`std::env::current_exe()`, i.e.
///    `K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`) — covers any future direct
///    Security-framework read from the daemon and the daemon's own
///    rotation/migration re-writes, so they never re-prompt.
/// 3. BOTH bundle peers ([`bundle_peer_binaries`]) — the app/renderer `k2`
///    (created the item via `keyring`, reads it back through `k2_secret_get`)
///    and the `k2-daemon` (reads via the `security` CLI and any future direct
///    Security-framework read, plus its own rotation/migration re-writes).
///    A plain ADD REPLACES the item's ACL wholesale, so whichever process
///    performs the write must stamp BOTH siblings or it would lock the other
///    one out and re-introduce a prompt. Since 0.40.31 the app writes the
///    account session with this ACL at sign-in ([`write_account_session`]
///    via the renderer command), so `current_exe` may be EITHER binary — we
///    therefore add both siblings unconditionally rather than deriving "the
///    other one" from whichever process happens to be running. Paths are the
///    resolved from the host app's `Contents/MacOS`, so they stay correct across versions and install
///    locations without hard-coding `/Applications`.
///
/// The CLI (`@alakazamlabs/k2`, an npm-installed bash script) does NOT read
/// this item — it has no account-session code path — so it is intentionally
/// omitted; adding a `-T` for a binary that never touches the item would be
/// dead ACL surface.
#[cfg(target_os = "macos")]
fn acl_trusted_apps() -> Vec<String> {
    let mut apps = vec!["/usr/bin/security".to_string()];
    if let Ok(exe) = std::env::current_exe() {
        // Resolve symlinks so the ACL records the real bundle path (launchd
        // may exec the daemon through a symlink).
        let exe = dunce::canonicalize(&exe).unwrap_or(exe);
        // (2) the current executable — whichever process is writing (the
        // daemon on rotation/boot, or the app `k2` on sign-in).
        apps.push(exe.to_string_lossy().to_string());
        // (3) BOTH bundle siblings, so the ACL is identical no matter which
        // process wrote it. Only add a sibling that exists on disk (a bare
        // dev/CI build has no bundle siblings; a phantom path is inert) and
        // never duplicate `current_exe`.
        for p in bundle_peer_binaries(&exe) {
            if p != exe && p.exists() {
                apps.push(p.to_string_lossy().to_string());
            }
        }
    }
    apps
}

/// The app binary `k2` and the bundled `k2-daemon` that share a keychain
/// ACL with the process at `exe`, as pure paths (no FS check).
///
/// macOS 0.43.2+: `k2` is in `K2.app/Contents/MacOS/` and the daemon is in
/// `K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/`, so they are no
/// longer siblings. Resolve both from the host app's `Contents/MacOS`
/// whichever of the two is running. A dev/CI build keeps both side by side.
pub fn bundle_peer_binaries(exe: &std::path::Path) -> Vec<std::path::PathBuf> {
    use crate::daemon_lifecycle as dl;
    let Some(host_dir) = dl::bundle_sidecar_dir(exe) else {
        return Vec::new();
    };
    let app = host_dir.join("k2");
    let mut out = Vec::with_capacity(2);
    if let Some(daemon) = dl::bundled_daemon_path(&app) {
        out.push(app);
        out.push(daemon);
    }
    out
}

/// Read the account session the renderer stored at sign-in. Order mirrors
/// the renderer's `readAccountSession`: the single-item JSON blob first
/// (current service, then the pre-rename service), then the LEGACY bare
/// refresh-token entry (same service order). Returns `None` when absent
/// (never signed in / signed out) so the caller can skip renewal cleanly
/// rather than erroring.
///
/// The legacy entry is only probed when no blob resolves, so the common
/// post-migration case costs a single keychain read.
#[cfg(target_os = "macos")]
pub fn read_account_session() -> Option<AccountSession> {
    // 1. Current single-item blob (the renderer's SESSION_BLOB_KEY).
    let blob = read_keychain_item(ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_SESSION_BLOB_KEY).or_else(
        || read_keychain_item(LEGACY_ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_SESSION_BLOB_KEY),
    );
    if let Some(sess) = resolve_account_session(blob.as_deref(), None) {
        return Some(sess);
    }
    // 2. Legacy bare-string entry (pre-blob sign-ins that never re-authed;
    // the boot migration normally converts these before we get here).
    let legacy = read_keychain_item(ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_REFRESH_KEY)
        .or_else(|| read_keychain_item(LEGACY_ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_REFRESH_KEY));
    resolve_account_session(None, legacy.as_deref())
}

#[cfg(target_os = "macos")]
pub fn read_keychain_item(service: &str, account: &str) -> Option<String> {
    // Through `macos_keychain`: never a dialog. 0.45.1/0.45.2 wrote this
    // item from the daemon process, which left it with K2's partition only,
    // so every `security` read (every 60 s renewal) prompted "security
    // wants to access key 'K2 Connect sign-in'". Such an item is read in
    // process (the daemon is on its ACL and partition) and rewritten by
    // `security`: repaired on the first read after the update.
    // Test builds never touch the developer's real login keychain.
    if cfg!(any(test, feature = "test-util")) {
        return None;
    }
    crate::macos_keychain::read_k2_owned_text(service, account, &item_write_options(), "tunnel/lease")
}

/// How the daemon writes (and repairs) the Connect sign-in items: the
/// friendly label, our trusted-app ACL, delete-then-add so the ACL is
/// fresh, and K2-owned (never argv).
#[cfg(target_os = "macos")]
pub fn item_write_options() -> crate::macos_keychain::WriteOptions {
    crate::macos_keychain::WriteOptions {
        keychain: None,
        // Friendly label shown in the macOS keychain access dialog instead
        // of the bare service id.
        label: Some(ACCOUNT_KEYCHAIN_LABEL.to_string()),
        // One `-T <path>` per trusted binary (see acl_trusted_apps for the
        // rationale tied to which process performs each access).
        trusted_apps: acl_trusted_apps(),
        // Delete first so a fresh item carries our ACL (`-U` keeps a
        // legacy creator-only ACL).
        replace_acl: true,
        cli_owned: false,
    }
}

/// Persist the session back to the keychain in the CURRENT single-blob
/// layout the renderer reads first. Supabase rotates the refresh token on
/// every refresh; if we don't write the new one back the NEXT daemon
/// refresh (and the renderer's next mount) would present a spent token.
/// Writing the BLOB (not the legacy string key) is load-bearing: the
/// renderer's `readAccountSession` prefers the blob, so a rotation written
/// anywhere else would strand the renderer on the spent token. Best-effort
/// — a keychain write failure is logged and swallowed (the in-memory token
/// is still good for this run).
///
/// Every write goes through [`write_token_with_acl`], which stamps the
/// trusted-application ACL ([`acl_trusted_apps`]) so `security` CLI reads
/// never provoke a login-keychain prompt. A write also UPGRADES the ACL on
/// an item the renderer created without one (delete + re-add installs the
/// fresh ACL).
///
/// ⚠ The `-T` ACL satisfies CLI reads ONLY. An item created by the
/// `security` tool carries an `apple-tool:` partition list, and macOS
/// checks the partition list BEFORE the ACL for in-process
/// Security-framework reads — so a keyring-crate read from the signed app
/// still raises a password prompt, and because this writer deletes +
/// re-adds on every rotation, a user-granted "Always Allow" is wiped each
/// time. The renderer must therefore read this item through
/// [`read_account_session`] (via its tauri bridge), never through the
/// keyring crate. That asymmetry caused the 0.40.31 settings-page
/// prompt-on-every-visit regression.
#[cfg(target_os = "macos")]
pub fn write_account_session(sess: &AccountSession) {
    let _ = try_write_account_session(sess);
}

/// [`write_account_session`] that says whether the write landed (the
/// keychain write reads the item back). The daemon's lease loop uses it to
/// know when a rotated refresh token is only in memory, so it never
/// presents the spent one still in the keychain (0.46.6 refresh-loop fix).
#[cfg(target_os = "macos")]
pub fn try_write_account_session(sess: &AccountSession) -> bool {
    write_token_with_acl(
        ACCOUNT_KEYCHAIN_SERVICE,
        ACCOUNT_SESSION_BLOB_KEY,
        &session_blob_json(sess),
    )
}

/// Remove the persisted account session entirely (sign-out): the current
/// single-item blob AND every legacy layout, under BOTH service names —
/// leaving any copy behind would silently re-sign-in via a fallback read.
/// Deletes go through the `security` CLI for the same partition-list
/// reason reads do (see [`write_account_session`]); a keyring-crate delete
/// from the app could prompt on a CLI-created item. Best-effort and
/// idempotent: "not found" is the expected common case.
#[cfg(target_os = "macos")]
pub fn clear_account_session() {
    for service in [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE] {
        for account in [
            ACCOUNT_SESSION_BLOB_KEY,
            ACCOUNT_REFRESH_KEY,
            ACCOUNT_EMAIL_KEY,
        ] {
            let _ = crate::bg_proc::command("security")
                .args(["delete-generic-password", "-s", service, "-a", account])
                .output();
        }
    }
}

/// Best-effort removal of the superseded LEGACY two-item layout under BOTH
/// service names, mirroring the renderer's `saveAccountSession` cleanup —
/// leaving a copy behind would let a later fallback read resurrect a spent
/// refresh token. Called on migration only (not on every rotation write:
/// the read path prefers the blob, so already-purged is the steady state).
#[cfg(target_os = "macos")]
pub fn purge_legacy_session_items() {
    for service in [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE] {
        for account in [ACCOUNT_REFRESH_KEY, ACCOUNT_EMAIL_KEY] {
            // Ignore the status: "not found" is the expected common case.
            let _ = crate::bg_proc::command("security")
                .args(["delete-generic-password", "-s", service, "-a", account])
                .output();
        }
    }
}

/// Write `token` under `(service, account)` with our trusted-application
/// ACL applied, idempotently and migration-safely.
///
/// `security`'s `-U` updates an existing item's VALUE but does NOT reset its
/// ACL, so we cannot rely on `-U` to upgrade a legacy item that the renderer
/// created with a default (creator-only) ACL. Instead we DELETE any existing
/// item first, then a plain ADD (no `-U`) installs a fresh item carrying the
/// `-T` ACL. Deleting then adding the SAME value preserves the token across
/// the operation; the only observable change is the (now correct) ACL.
///
/// The token never goes on argv (`security add-generic-password -w <token>`
/// showed it in `ps`) and is never set in process: `security` creates the item
/// with the `-l` label and the `-T` list and the token on its stdin, and
/// it is read back ([`crate::macos_keychain::write`]).
///
/// Best-effort throughout: a delete of a missing item is fine, and a write
/// failure is logged + swallowed (the in-memory token is still good for
/// this run).
#[cfg(target_os = "macos")]
fn write_token_with_acl(service: &str, account: &str, token: &str) -> bool {
    match crate::macos_keychain::write(service, account, token.as_bytes(), &item_write_options()) {
        Ok(()) => true,
        Err(e) => {
            crate::log_debug!("[tunnel/lease] WARN: failed to persist refresh token with ACL (service={service}): {e}");
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_account_session() -> Option<AccountSession> {
    // Non-macOS keychain bridge for the account session is not yet wired
    // (the renderer uses the keyring crate's Secret Service / Credential
    // Manager backends, which have no stable CLI we read here). Renewal is
    // a no-op on those platforms — [`renewal_mode`] resolves to
    // `NoSession` and the connector skips the loop with ONE log line
    // (normal for provisioned/hosted daemons; the tunnel works without
    // the lease). Tracked as a follow-up.
    None
}

#[cfg(not(target_os = "macos"))]
pub fn write_account_session(_sess: &AccountSession) {}

/// Non-macOS: nothing is written (see [`write_account_session`]).
#[cfg(not(target_os = "macos"))]
pub fn try_write_account_session(_sess: &AccountSession) -> bool {
    false
}

/// Non-macOS: the renderer owns the account session via the keyring crate
/// (see [`read_account_session`]'s note); nothing for the daemon to clear.
#[cfg(not(target_os = "macos"))]
pub fn clear_account_session() {}
