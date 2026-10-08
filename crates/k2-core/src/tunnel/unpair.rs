//! Release subdomain from this device (unpair) — the DESTRUCTIVE half of
//! the Disable-vs-Release split (PRD `prd-tunnel-disable-unpair-v1.md` §2B).
//!
//! Born from a real incident: a decommissioned machine kept a valid
//! `~/.k2/tunnel.json` forever, and its orphaned daemon re-claimed the
//! subdomain from the replacement server for days. **Release** is the
//! product answer: permanently delete this device's tunnel identity so it
//! can NEVER contest the name again — even from a stale backup copy of the
//! old files.
//!
//! What a release does, in order:
//!   1. Stops the connector (frpc down, supervisor exits).
//!   2. Reports the release UPSTREAM so the relay refuses any future
//!      connection presenting the released identity (the load-bearing
//!      half — local deletion alone is cosmetic). Offline-tolerant: a
//!      failed report is QUEUED in the tombstone and replayed at daemon
//!      boot ([`replay_pending`]); the account portal can also
//!      force-release server-side.
//!   3. Writes the `~/.k2/unpaired.json` tombstone — when/what was
//!      released, for support, plus the identity FINGERPRINT the local
//!      spawn gate uses to refuse a planted copy of the old tunnel.json.
//!   4. Deletes the identity files: `tunnel.json` (device id + bearer),
//!      the rendered `frpc.toml` (embeds the bearer), and the E2E cert
//!      (`tunnel-cert.pem`). The key (`tunnel-key.pem`) stays since 0.45.2:
//!      it is also the box's federation identity, and no credential for
//!      the name.
//!
//! 0.45.2 holder rule: step 2 runs only when this device's tunnel is
//! CONNECTED under the released identity (the only local evidence that it
//! holds the name). Otherwise — and for "Forget this tunnel on this
//! device" ([`ReleaseMode::ForgetLocal`]) — nothing is sent or queued. A
//! queued revocation replays only when it was made by the holder, so a
//! stale copy of another box's `tunnel.json` can never free a live name.
//!
//! Re-pairing later mints a FRESH identity through the normal pairing
//! flow — a new token/device id doesn't match the tombstone fingerprint,
//! so the gate lets it through. The subdomain stays OWNED by the account
//! (release is device-scoped, not account-scoped); this is the migration
//! story: release on the old box, pair on the new box.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::config::{self, TunnelConfig};

/// Path of the tombstone note a release leaves behind:
/// `~/.k2/unpaired.json`. Honors `$HOME` so tests can redirect it.
pub fn tombstone_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("unpaired.json")
}

/// The on-disk tombstone a release writes. Two jobs:
///   * a SUPPORT record (when / which subdomain / which device), and
///   * the identity FINGERPRINT the spawn gate checks so a planted copy
///     of the released `tunnel.json` can never re-arm the connector
///     locally (the relay-side refusal is the authoritative backstop).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpairTombstone {
    /// RFC3339 UTC timestamp of the release.
    pub released_at: String,
    /// The subdomain label this device held when released.
    pub subdomain: String,
    /// The released device id (when the identity had one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// SHA-256 hex fingerprint of the released bearer token — enough for
    /// the gate to recognize the dead identity, never the secret itself.
    pub token_sha256: String,
    /// Whether the upstream (relay/control-plane) revocation landed. When
    /// false, [`replay_pending`] retries at daemon boot using
    /// `pending_token`.
    pub upstream_reported: bool,
    /// The released bearer, kept (0600) ONLY while `upstream_reported` is
    /// false so the queued revocation can authenticate its replay; scrubbed
    /// the moment the report lands. Never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_token: Option<String>,
    /// 0.45.2: this device's tunnel was CONNECTED under the released
    /// identity when the release ran — the only local evidence that it was
    /// the holder. A queued revocation replays only when this is true; a
    /// tombstone written before 0.45.2 has no such record, so its queued
    /// revocation is dropped instead of replayed (field case: an old Mac
    /// with a stale copy of a migrated customer's `tunnel.json` queued a
    /// release of a name a live box holds).
    #[serde(default)]
    pub holder_at_release: bool,
    /// 0.45.2: the device forgot its tunnel locally and sent NOTHING
    /// upstream (it was not the holder, or the owner chose "Forget").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local_only: bool,
    /// 0.45.2: why a queued revocation was dropped instead of replayed
    /// (the bearer is scrubbed at the same time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_dropped: Option<String>,
}

/// SHA-256 hex fingerprint of a bearer token. The tombstone stores this
/// instead of the secret so recognizing the dead identity never requires
/// keeping it.
pub fn token_fingerprint(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.trim().as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Load the tombstone. Missing file → `Ok(None)` (never released — the
/// overwhelmingly common case). A malformed file is an ERROR (fail loud):
/// a corrupt tombstone must never silently read as "not released".
pub fn load() -> Result<Option<UnpairTombstone>, String> {
    let file = tombstone_path();
    if !file.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| format!("parse {}: {e}", file.display()))
}

/// Persist the tombstone via tmp+rename, chmod 0600 (it may carry the
/// pending bearer until the upstream report lands).
pub fn save(t: &UnpairTombstone) -> Result<(), String> {
    let file = tombstone_path();
    let dir = file
        .parent()
        .ok_or_else(|| "tombstone path has no parent dir".to_string())?
        .to_path_buf();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("unpaired.json.tmp.{}", std::process::id()));
    let body = serde_json::to_string_pretty(t)
        .map_err(|e| format!("serialize unpair tombstone: {e}"))?;
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", file.display())
    })?;
    restrict_mode(&file);
    Ok(())
}

#[cfg(unix)]
fn restrict_mode(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_p: &std::path::Path) {}

/// Does `cfg` present the RELEASED identity? `Some(tombstone)` when a
/// tombstone exists AND the config's identity matches it — same bearer
/// (fingerprint) or same device id. A FRESH identity (re-pair after
/// release) matches neither and passes.
///
/// A malformed tombstone reads as released (fail closed): the gate must
/// never wave a possibly-dead identity through over a corrupt note.
pub fn identity_released(cfg: &TunnelConfig) -> Option<UnpairTombstone> {
    let t = match load() {
        Ok(Some(t)) => t,
        Ok(None) => return None,
        Err(e) => {
            crate::log_debug!("[tunnel/unpair] tombstone unreadable — treating identity as released (fail closed): {e}");
            return Some(UnpairTombstone {
                released_at: String::new(),
                subdomain: String::new(),
                device_id: None,
                token_sha256: String::new(),
                upstream_reported: false,
                pending_token: None,
                ..Default::default()
            });
        }
    };
    let token = cfg.token.trim();
    if !token.is_empty() && token_fingerprint(token) == t.token_sha256 {
        return Some(t);
    }
    if let (Some(dev), Some(released_dev)) = (cfg.device_id.as_deref(), t.device_id.as_deref()) {
        if !dev.trim().is_empty() && dev.trim() == released_dev.trim() {
            return Some(t);
        }
    }
    None
}

/// What an explicit owner re-pair did to the tombstone ([`note_repair`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// No tombstone — nothing to do (the common case).
    NotReleased,
    /// A FRESH token on a released device: the tombstone stays (it still
    /// refuses the released bearer, and a queued upstream revocation still
    /// replays) but no longer matches this device's id.
    FreedDeviceId,
    /// The owner re-bound the RELEASED identity itself (same bearer, e.g.
    /// "switch back to the old name"): the tombstone is removed, including
    /// any queued revocation — replaying it would revoke the token the
    /// device now uses.
    Removed,
}

/// An explicit owner bind (`POST /cli/tunnel/config` with a token — the
/// Settings subdomain picker, `k2 tunnel config`, K2 Cloud re-pair) is a
/// RE-PAIR, never a zombie: zombies are stale COPIES of `tunnel.json`
/// that never pass through this call. Before 0.45.2 the tombstone matched
/// by device id, and the app re-sends the same per-install device id, so
/// "release, then connect another subdomain" on the same device was
/// refused as released forever (or, with no device id stored, connected
/// while the gate kept a dead match around).
pub fn note_repair(new_token: &str) -> Result<RepairOutcome, String> {
    let token = new_token.trim();
    if token.is_empty() {
        return Ok(RepairOutcome::NotReleased);
    }
    let mut t = match load()? {
        Some(t) => t,
        None => return Ok(RepairOutcome::NotReleased),
    };
    if token_fingerprint(token) == t.token_sha256 {
        let file = tombstone_path();
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove {}: {e}", file.display())),
        }
        crate::log_debug!(
            "[tunnel/unpair] owner re-bound the released identity for {:?} — tombstone removed",
            t.subdomain
        );
        return Ok(RepairOutcome::Removed);
    }
    if t.device_id.is_some() {
        t.device_id = None;
        save(&t)?;
    }
    Ok(RepairOutcome::FreedDeviceId)
}

/// The status-surface view: is this device in the RELEASED state? True
/// when a tombstone exists and the device holds no fresh identity — i.e.
/// no config token at all (the normal post-release state) or a token that
/// matches the released fingerprint (a planted stale backup). A re-pair
/// with a fresh token flips this back to false.
pub fn released_state(cfg: &TunnelConfig) -> bool {
    if cfg.token.trim().is_empty() {
        return matches!(load(), Ok(Some(_)) | Err(_));
    }
    identity_released(cfg).is_some()
}

/// What a completed release looked like — returned to the route/CLI so
/// the confirmation can name the subdomain and be honest about whether
/// the upstream revocation landed, was queued, or was never sent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseReport {
    /// The released subdomain label.
    pub subdomain: String,
    /// True when the upstream revocation landed during the release.
    pub upstream_reported: bool,
    /// True when this call found the device ALREADY released (idempotent
    /// re-release) — nothing new was deleted.
    pub already_released: bool,
    /// 0.45.2: the release was local only — nothing was sent upstream and
    /// nothing was queued. Set for "Forget this tunnel on this device" and
    /// for a release from a device whose tunnel was not connected (not the
    /// holder: it must never free a name another device holds).
    pub local_only: bool,
    /// 0.45.2: a revocation is queued for boot-time replay (the relay was
    /// unreachable, and this device WAS connected as the holder).
    pub queued: bool,
    /// Plain-language note for the confirmation (why nothing was sent, why
    /// the report failed). `None` on a clean upstream release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How a release treats the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseMode {
    /// Report the release upstream — only when this device is the holder
    /// (its tunnel is connected under this identity); otherwise it acts as
    /// [`ReleaseMode::ForgetLocal`].
    Release,
    /// Forget the tunnel on this device only: delete the local identity,
    /// tombstone it, and send NOTHING upstream (stale config from a
    /// migrated or cloned machine).
    ForgetLocal,
}

/// Why an upstream report failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    /// Worth retrying later (network, timeout, 5xx).
    Transient(String),
    /// Never worth retrying: the control plane has no release endpoint
    /// (404), refused the caller as not the holder (409), or rejected the
    /// token (401/403). The queued revocation is dropped.
    Terminal(String),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportError::Transient(m) | ReportError::Terminal(m) => f.write_str(m),
        }
    }
}

/// Report the release upstream so the relay refuses any future connection
/// presenting the released identity (PRD §2B — the client half of the
/// revocation contract; the relay-side refusal belongs to the proprietary
/// k2-connect control plane).
///
/// Contract: `POST {control-plane}/tunnel/release`, `Authorization:
/// Bearer <released token>`, JSON body
/// `{"subdomain": ..., "deviceId": ..., "releasedAt": <RFC 3339>}`.
/// `releasedAt` is the moment of the release on this device (the
/// tombstone's `released_at`), also when a queued report is replayed later.
/// The control plane refuses a missing value (409 `legacy_release`) and one
/// older than 7 days (409 `stale_release`); both are terminal here.
/// The control plane must refuse (409 `not_holder`) unless this token AND
/// device id hold the name right now. As of 0.45.2 the endpoint does not
/// exist (connect.k2.dev answers 404 for everything but `/subdomains*`),
/// so no release has ever reached the server.
pub fn report_upstream(
    subdomain: &str,
    device_id: Option<&str>,
    token: &str,
    released_at: &str,
) -> Result<(), ReportError> {
    if token.trim().is_empty() {
        return Err(ReportError::Terminal(
            "no bearer token to authenticate the release report".to_string(),
        ));
    }
    if chrono::DateTime::parse_from_rfc3339(released_at.trim()).is_err() {
        return Err(ReportError::Terminal(format!(
            "the release time {released_at:?} is not RFC 3339"
        )));
    }
    let base = super::subdomains::control_plane_base();
    let url = format!("{base}/tunnel/release");
    let body = serde_json::json!({
        "subdomain": subdomain,
        "deviceId": device_id,
        "releasedAt": released_at.trim(),
    });
    // Connect timeout (0.45.1): a control plane that never answers the SYN
    // (a firewall that drops instead of refusing) fails in 5 s, not 20.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ReportError::Transient(format!("build http client: {e}")))?;
    let resp = client
        .post(&url)
        .bearer_auth(token.trim())
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| ReportError::Transient(format!("POST {url}: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let msg = format!("POST {url}: HTTP {status}");
    match status.as_u16() {
        401 | 403 | 404 | 409 | 410 => Err(ReportError::Terminal(msg)),
        _ => Err(ReportError::Transient(msg)),
    }
}

/// Perform the release: stop the connector, report upstream when this
/// device is the holder (queue on a transient failure), tombstone, and
/// delete the identity files. See the module doc for the ordering.
///
/// Idempotent-honest: releasing an already-released device (tombstone
/// present, no fresh identity) returns `already_released: true` and
/// touches nothing; releasing a device with NO identity and NO tombstone
/// is an error (there is nothing to release — surfacing that beats a
/// no-op that reads as success).
pub fn perform_release() -> Result<ReleaseReport, String> {
    perform_release_mode(ReleaseMode::Release)
}

/// [`perform_release`] with an explicit [`ReleaseMode`].
pub fn perform_release_mode(mode: ReleaseMode) -> Result<ReleaseReport, String> {
    let cfg = config::load()?;
    let token = cfg.token.trim().to_string();

    if token.is_empty() {
        // No live identity. Already tombstoned → idempotent success;
        // otherwise there is genuinely nothing to release.
        return match load()? {
            Some(t) => Ok(ReleaseReport {
                subdomain: t.subdomain,
                upstream_reported: t.upstream_reported,
                already_released: true,
                local_only: t.local_only,
                queued: !t.upstream_reported && t.pending_token.is_some(),
                note: None,
            }),
            None => Err(
                "no tunnel identity on this device — nothing to release \
                 (tunnel.json has no token)"
                    .to_string(),
            ),
        };
    }

    // Holder evidence, read BEFORE the stop: this device's connector is up
    // under exactly this identity. A device whose tunnel is not connected
    // (an old machine with a stale copy of a migrated box's tunnel.json)
    // cannot know it holds the name, so it never reports a release.
    let holder = super::connector::running_config().is_some_and(|r| {
        super::identity::TunnelIdentity::of(&r) == super::identity::TunnelIdentity::of(&cfg)
    });

    // 1. Tunnel down first — a releasing device must stop contesting the
    //    name immediately, before any network round-trip.
    super::connector::stop()?;

    // 2. Upstream revocation, only from the holder. Transient failure →
    //    queued for boot-time replay; terminal failure → recorded, never
    //    retried.
    let subdomain = cfg.subdomain.trim().to_string();
    // The moment of release: recorded in the tombstone AND sent upstream, so
    // a later replay reports the same time, not the time of the replay.
    let released_at = chrono::Utc::now().to_rfc3339();
    let mut upstream_reported = false;
    let mut queued = false;
    let mut local_only = false;
    let note: Option<String> = match mode {
        ReleaseMode::ForgetLocal => {
            local_only = true;
            Some("forgotten on this device only; nothing was sent to K2 Connect".to_string())
        }
        ReleaseMode::Release if !holder => {
            local_only = true;
            Some(
                "this device's tunnel was not connected, so it is not the holder of the \
                 name; it was forgotten on this device and nothing was sent to K2 Connect"
                    .to_string(),
            )
        }
        ReleaseMode::Release => {
            match report_upstream(&subdomain, cfg.device_id.as_deref(), &token, &released_at) {
                Ok(()) => {
                    upstream_reported = true;
                    None
                }
                Err(ReportError::Transient(e)) => {
                    crate::log_debug!(
                        "[tunnel/unpair] upstream release report failed — queued for boot-time replay: {e}"
                    );
                    queued = true;
                    Some(format!("K2 Connect was unreachable ({e}); the release is queued"))
                }
                Err(ReportError::Terminal(e)) => {
                    crate::log_debug!(
                        "[tunnel/unpair] upstream release report refused — not queued: {e}"
                    );
                    Some(format!(
                        "K2 Connect did not record the release ({e}); released on this device"
                    ))
                }
            }
        }
    };

    // 3. Tombstone BEFORE deletion: if the deletes below fail halfway, the
    //    gate already refuses the identity (fail closed, never fail open).
    save(&UnpairTombstone {
        released_at: released_at.clone(),
        subdomain: subdomain.clone(),
        device_id: cfg.device_id.clone(),
        token_sha256: token_fingerprint(&token),
        upstream_reported,
        pending_token: queued.then_some(token),
        holder_at_release: holder,
        local_only,
        replay_dropped: None,
    })?;

    // 4. Delete the identity: tunnel.json (device id + bearer), the
    //    rendered frpc.toml (embeds the bearer), and the old name's
    //    certificate. A failed delete is an ERROR — a release that leaves
    //    the bearer on disk must not report success. (The tombstone is
    //    already down, so even the failure state is gate-blocked.)
    for path in [
        config::config_path(),
        super::connector::frpc_config_path(),
        super::tls::cert_path(),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("delete {}: {e}", path.display())),
        }
    }
    // `tunnel-key.pem` STAYS (0.45.2). It is the key the tunnel certificate
    // is issued for, and on a box that has not loaded its federation
    // identity yet it is still the source `federation::identity` copies
    // (`~/.k2/federation/identity.pem`): deleting it then would change the
    // fingerprint every peer pinned. It is no credential for the released
    // name: the cert broker and the relay authenticate the bearer token,
    // which IS deleted.

    Ok(ReleaseReport {
        subdomain,
        upstream_reported,
        already_released: false,
        local_only,
        queued,
        note,
    })
}

/// What a boot-time replay did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// Nothing queued (the common case).
    Nothing,
    /// The queued revocation landed; the bearer is scrubbed.
    Reported,
    /// The queued revocation was dropped (bearer scrubbed) and will never
    /// be sent: the reason.
    Dropped(String),
}

/// Boot-time replay of a queued upstream revocation (offline release,
/// PRD §2B). Only a release made while this device was CONNECTED as the
/// holder replays; anything else (a pre-0.45.2 tombstone, a local-only
/// release) is dropped and its bearer scrubbed, so a stale token can never
/// free a name another device holds. A terminal refusal (no endpoint,
/// `not_holder`, bad token) also drops it. A transient failure is an error
/// (logged by the caller; retried next boot).
pub fn replay_pending() -> Result<ReplayOutcome, String> {
    if crate::airgap::enabled() {
        return Ok(ReplayOutcome::Nothing);
    }
    let mut t = match load()? {
        Some(t) if !t.upstream_reported && t.pending_token.is_some() => t,
        _ => return Ok(ReplayOutcome::Nothing),
    };
    let token = t.pending_token.clone().unwrap_or_default().trim().to_string();
    let drop_reason = if token.is_empty() {
        Some("the queued release has no token".to_string())
    } else if !t.holder_at_release || t.local_only {
        Some(
            "this device was not verified as the holder when it released (tombstone from \
             before 0.45.2 or a local-only release); a replay could free a name another \
             device holds"
                .to_string(),
        )
    } else {
        match report_upstream(&t.subdomain, t.device_id.as_deref(), &token, &t.released_at) {
            Ok(()) => {
                t.upstream_reported = true;
                t.pending_token = None; // scrub the secret the moment the report lands
                save(&t)?;
                return Ok(ReplayOutcome::Reported);
            }
            Err(ReportError::Terminal(e)) => Some(format!("K2 Connect refused it: {e}")),
            Err(ReportError::Transient(e)) => return Err(e),
        }
    };
    let reason = drop_reason.unwrap_or_default();
    t.pending_token = None;
    t.replay_dropped = Some(reason.clone());
    save(&t)?;
    Ok(ReplayOutcome::Dropped(reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    /// Point the control plane at a dead local port so upstream reports
    /// fail FAST (connection refused) instead of dialing the real
    /// connect.k2.dev from a unit test. Restores the prior value (the
    /// `EnvVar` guard holds the ONE env lock and restores on drop, also on
    /// panic).
    /// Point the control plane at a test-owned server that answers 503 (not
    /// a dead port such as `127.0.0.1:9`, which HANGS where loopback RSTs
    /// are dropped): the upstream report fails fast and the release queues.
    fn with_dead_control_plane<F: FnOnce()>(f: F) {
        let var = super::super::subdomains::CONTROL_PLANE_BASE_ENV;
        let down = crate::test_env::ErrorHttpServer::start(503);
        let _cp = crate::test_env::EnvVar::set(var, down.url());
        f();
    }

    #[test]
    fn token_fingerprint_is_stable_sha256_hex() {
        // Pinned vector (`printf 'tok' | shasum -a 256`) so the tombstone
        // format can't drift silently between releases.
        assert_eq!(
            token_fingerprint("tok"),
            "1a7674eb4ee78df7e1ac439a93c3fa8e3c945784d4dec9fd8e3011738b2f1d62"
        );
        // Trimmed: config values are trimmed everywhere else too.
        assert_eq!(token_fingerprint(" tok \n"), token_fingerprint("tok"));
        assert_ne!(token_fingerprint("tok"), token_fingerprint("tok2"));
    }

    #[test]
    fn tombstone_round_trips_and_is_0600() {
        with_temp_home(|| {
            assert_eq!(load().expect("no tombstone yet"), None);
            let t = UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                device_id: Some("dev-abc".to_string()),
                token_sha256: token_fingerprint("tok"),
                upstream_reported: false,
                pending_token: Some("tok".to_string()),
                holder_at_release: true,
                local_only: false,
                replay_dropped: None,
            };
            save(&t).expect("save tombstone");
            assert_eq!(load().expect("load tombstone"), Some(t));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(tombstone_path())
                    .expect("stat unpaired.json")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "tombstone may carry the pending bearer");
            }
        });
    }

    #[test]
    fn identity_released_matches_token_or_device_and_frees_fresh_identity() {
        with_temp_home(|| {
            save(&UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                device_id: Some("dev-old".to_string()),
                token_sha256: token_fingerprint("tok-old"),
                upstream_reported: true,
                pending_token: None,
                ..Default::default()
            })
            .expect("save tombstone");

            // The released bearer (a planted stale backup) is recognized.
            let planted = TunnelConfig {
                token: "tok-old".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            };
            assert!(identity_released(&planted).is_some(), "released token must match");
            assert!(released_state(&planted));

            // Same device id with a different token is ALSO the dead
            // identity (a re-rendered backup with the device id intact).
            let same_device = TunnelConfig {
                token: "tok-other".to_string(),
                device_id: Some("dev-old".to_string()),
                ..Default::default()
            };
            assert!(identity_released(&same_device).is_some(), "released device id must match");

            // A FRESH identity (normal re-pair) passes.
            let fresh = TunnelConfig {
                token: "tok-new".to_string(),
                device_id: Some("dev-new".to_string()),
                subdomain: "rosson".to_string(),
                ..Default::default()
            };
            assert!(identity_released(&fresh).is_none(), "fresh identity must pass");
            assert!(!released_state(&fresh), "re-pair clears the released state");

            // No identity at all + tombstone = the released state.
            assert!(released_state(&TunnelConfig::default()));
        });
    }

    /// A fake control plane that answers every request with `status` and
    /// COUNTS the requests it saw (the stale-token tests assert zero).
    struct CountingControlPlane {
        addr: std::net::SocketAddr,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        _env: crate::test_env::EnvVar,
    }

    impl CountingControlPlane {
        fn start(status: u16) -> Self {
            use std::io::{Read, Write};
            use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
            use std::sync::Arc;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake control plane");
            let addr = listener.local_addr().expect("addr");
            let hits = Arc::new(AtomicUsize::new(0));
            let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (hits_t, bodies_t, stop_t) = (hits.clone(), bodies.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop_t.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut s) = stream else { continue };
                    hits_t.fetch_add(1, Ordering::SeqCst);
                    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 2048];
                    let header_end = loop {
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(i + 4);
                        }
                        match s.read(&mut chunk) {
                            Ok(0) | Err(_) => break None,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    if let Some(end) = header_end {
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        while buf.len() < end + len {
                            match s.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let body = String::from_utf8_lossy(&buf[end..]).to_string();
                        bodies_t.lock().expect("bodies lock").push(body);
                    }
                    let _ = s.write_all(
                        format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .as_bytes(),
                    );
                }
            });
            let env = crate::test_env::EnvVar::set(
                super::super::subdomains::CONTROL_PLANE_BASE_ENV,
                format!("http://{addr}"),
            );
            Self { addr, hits, bodies, stop, thread: Some(thread), _env: env }
        }

        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// The JSON bodies of every request seen, in order.
        fn bodies(&self) -> Vec<serde_json::Value> {
            self.bodies
                .lock()
                .expect("bodies lock")
                .iter()
                .map(|b| serde_json::from_str(b).unwrap_or_else(|e| panic!("body is not JSON ({e}): {b}")))
                .collect()
        }
    }

    impl Drop for CountingControlPlane {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(self.addr);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn true_bin() -> std::path::PathBuf {
        for cand in ["/usr/bin/true", "/bin/true"] {
            let p = std::path::PathBuf::from(cand);
            if p.exists() {
                return p;
            }
        }
        panic!("no `true` binary at /usr/bin/true or /bin/true");
    }

    /// Seed `cfg` and bring the connector up under it (fake frpc, E2E off,
    /// lease off) — the "this device's tunnel is connected" state.
    fn start_connected(cfg: TunnelConfig) {
        config::save(&TunnelConfig { e2e: false, ..cfg }).expect("seed identity");
        super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46123)));
        let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
        let st = super::super::connector::start(
            None,
            57839,
            &super::super::connector::FrpcBinary::Explicit(true_bin()),
        )
        .expect("start fake tunnel");
        assert!(st.running, "fake tunnel must be running");
    }

    fn live_identity() -> TunnelConfig {
        TunnelConfig {
            token: "tok-live".to_string(),
            subdomain: "rosson".to_string(),
            device_id: Some("dev-abc".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn release_while_connected_reports_upstream_and_keeps_the_federation_key() {
        with_temp_home(|| {
            let cp = CountingControlPlane::start(200);
            let key_before = super::super::tls::load_or_generate_keypair()
                .expect("key")
                .serialize_pem();
            std::fs::write(super::super::tls::cert_path(), "old cert").expect("seed cert");
            start_connected(live_identity());

            let report = perform_release().expect("release");
            assert_eq!(report.subdomain, "rosson");
            assert!(report.upstream_reported, "the holder's release must be reported");
            assert!(!report.local_only && !report.queued && !report.already_released);
            assert_eq!(cp.hits(), 1, "exactly one POST /tunnel/release");

            assert!(!config::config_path().exists(), "tunnel.json must be deleted");
            assert!(!super::super::tls::cert_path().exists(), "the old name's cert must be deleted");
            assert_eq!(
                std::fs::read_to_string(super::super::tls::key_path()).expect("key kept"),
                key_before,
                "tunnel-key.pem is the federation identity and must survive a release"
            );
            let t = load().expect("load").expect("tombstone");
            assert!(t.holder_at_release && t.upstream_reported && !t.local_only);
            assert_eq!(t.pending_token, None);
            assert!(!super::super::connector::status().running, "tunnel down after release");

            // Wire contract: releasedAt is the tombstone's release time.
            let bodies = cp.bodies();
            assert_eq!(bodies.len(), 1, "{bodies:?}");
            assert_eq!(bodies[0]["subdomain"], "rosson");
            assert_eq!(bodies[0]["deviceId"], "dev-abc");
            let sent = bodies[0]["releasedAt"]
                .as_str()
                .unwrap_or_else(|| panic!("releasedAt missing: {}", bodies[0]));
            assert_eq!(sent, t.released_at, "releasedAt must equal the tombstone's released_at");
            chrono::DateTime::parse_from_rfc3339(sent).expect("releasedAt is RFC 3339");

            let again = perform_release().expect("re-release");
            assert!(again.already_released);
        });
    }

    #[test]
    fn release_while_not_connected_sends_nothing_and_queues_nothing() {
        // Field case 2026-10-08: an old Mac kept a stale copy of a migrated
        // customer's tunnel.json; its "release" must never reach the relay.
        with_temp_home(|| {
            let cp = CountingControlPlane::start(200);
            config::save(&live_identity()).expect("seed stale identity");

            let report = perform_release().expect("release");
            assert!(report.local_only, "not the holder → local only");
            assert!(!report.upstream_reported && !report.queued);
            assert!(
                report.note.as_deref().is_some_and(|n| n.contains("not the holder")),
                "the note must say why: {:?}",
                report.note
            );
            assert_eq!(cp.hits(), 0, "a non-holder must send NOTHING upstream");

            let t = load().expect("load").expect("tombstone");
            assert!(!t.holder_at_release && t.local_only);
            assert_eq!(t.pending_token, None, "no bearer kept for a replay");
            assert!(!config::config_path().exists());
            assert_eq!(replay_pending().expect("replay"), ReplayOutcome::Nothing);
            assert_eq!(cp.hits(), 0);
        });
    }

    #[test]
    fn forget_local_sends_nothing_even_when_connected() {
        with_temp_home(|| {
            let cp = CountingControlPlane::start(200);
            start_connected(live_identity());
            let report = perform_release_mode(ReleaseMode::ForgetLocal).expect("forget");
            assert!(report.local_only && !report.upstream_reported && !report.queued);
            assert_eq!(cp.hits(), 0, "Forget must never call the relay");
            assert!(!config::config_path().exists());
            let t = load().expect("load").expect("tombstone");
            assert!(t.local_only);
            assert_eq!(t.pending_token, None);
            assert!(!super::super::connector::status().running);
        });
    }

    #[test]
    fn holder_release_with_unreachable_relay_is_queued() {
        with_temp_home(|| {
            with_dead_control_plane(|| {
                start_connected(live_identity());
                let report = perform_release().expect("release");
                assert!(report.queued && !report.upstream_reported && !report.local_only);
                let t = load().expect("load").expect("tombstone");
                assert!(t.holder_at_release);
                assert_eq!(t.device_id.as_deref(), Some("dev-abc"));
                assert_eq!(t.token_sha256, token_fingerprint("tok-live"));
                assert_eq!(t.pending_token.as_deref(), Some("tok-live"));
            });
        });
    }

    #[test]
    fn holder_release_against_missing_endpoint_is_not_queued() {
        // connect.k2.dev answers 404 for /tunnel/release today: terminal.
        with_temp_home(|| {
            let cp = CountingControlPlane::start(404);
            start_connected(live_identity());
            let report = perform_release().expect("release");
            assert_eq!(cp.hits(), 1);
            assert!(!report.queued && !report.upstream_reported);
            assert!(report.note.as_deref().is_some_and(|n| n.contains("404")), "{:?}", report.note);
            let t = load().expect("load").expect("tombstone");
            assert_eq!(t.pending_token, None, "a 404 must not queue a replay");
        });
    }

    #[test]
    fn perform_release_with_no_identity_and_no_tombstone_errors() {
        with_temp_home(|| {
            let err = perform_release().expect_err("nothing to release must fail loud");
            assert!(
                err.contains("nothing to release"),
                "expected nothing-to-release error, got: {err}"
            );
        });
    }

    #[test]
    fn replay_drops_a_pre_0452_queued_release_without_sending_it() {
        // The exact unpaired.json shape 0.45.0 wrote on Alakazam's Mac mini
        // (no holder record): replaying it could free `nsi`, which a live
        // box holds. It must be dropped, its bearer scrubbed, nothing sent.
        with_temp_home(|| {
            let cp = CountingControlPlane::start(200);
            std::fs::create_dir_all(tombstone_path().parent().expect("dir")).expect("mkdir");
            std::fs::write(
                tombstone_path(),
                format!(
                    r#"{{"released_at":"2026-10-08T15:00:00+00:00","subdomain":"nsi",
                        "device_id":"dev-old-mac","token_sha256":"{}",
                        "upstream_reported":false,"pending_token":"tok-nsi"}}"#,
                    token_fingerprint("tok-nsi")
                ),
            )
            .expect("plant 0.45.0 tombstone");

            match replay_pending().expect("replay") {
                ReplayOutcome::Dropped(reason) => {
                    assert!(reason.contains("not verified as the holder"), "{reason}")
                }
                other => panic!("a legacy queued release must be dropped, got {other:?}"),
            }
            assert_eq!(cp.hits(), 0, "the stale token must never reach the relay");
            let t = load().expect("load").expect("tombstone");
            assert_eq!(t.pending_token, None, "bearer scrubbed");
            assert!(!t.upstream_reported);
            assert!(t.replay_dropped.is_some());
            // The tombstone still refuses the released bearer locally.
            assert!(identity_released(&TunnelConfig {
                token: "tok-nsi".to_string(),
                ..Default::default()
            })
            .is_some());
            assert_eq!(replay_pending().expect("second boot"), ReplayOutcome::Nothing);
            assert_eq!(cp.hits(), 0);
        });
    }

    fn queued_holder_tombstone(token: &str) -> UnpairTombstone {
        UnpairTombstone {
            released_at: "2026-07-12T00:00:00+00:00".to_string(),
            subdomain: "rosson".to_string(),
            device_id: Some("dev-abc".to_string()),
            token_sha256: token_fingerprint(token),
            upstream_reported: false,
            pending_token: Some(token.to_string()),
            holder_at_release: true,
            ..Default::default()
        }
    }

    #[test]
    fn replay_refused_as_not_holder_is_dropped() {
        with_temp_home(|| {
            let cp = CountingControlPlane::start(409);
            save(&queued_holder_tombstone("tok-q")).expect("save");
            match replay_pending().expect("replay") {
                ReplayOutcome::Dropped(reason) => assert!(reason.contains("409"), "{reason}"),
                other => panic!("409 not_holder must drop the replay, got {other:?}"),
            }
            assert_eq!(cp.hits(), 1);
            assert_eq!(load().expect("load").expect("t").pending_token, None);
            assert_eq!(replay_pending().expect("again"), ReplayOutcome::Nothing);
            assert_eq!(cp.hits(), 1, "a dropped release is never retried");
        });
    }

    #[test]
    fn replay_with_no_release_time_is_dropped_without_sending() {
        // The control plane refuses a report without releasedAt (409
        // legacy_release); never send one.
        with_temp_home(|| {
            let cp = CountingControlPlane::start(200);
            save(&UnpairTombstone { released_at: String::new(), ..queued_holder_tombstone("tok-q") })
                .expect("save");
            match replay_pending().expect("replay") {
                ReplayOutcome::Dropped(reason) => assert!(reason.contains("RFC 3339"), "{reason}"),
                other => panic!("a release with no time must be dropped, got {other:?}"),
            }
            assert_eq!(cp.hits(), 0, "nothing sent without a release time");
            assert_eq!(load().expect("load").expect("t").pending_token, None);
        });
    }

    #[test]
    fn replay_transient_failure_keeps_the_queue() {
        with_temp_home(|| {
            with_dead_control_plane(|| {
                save(&queued_holder_tombstone("tok-q")).expect("save");
                let err = replay_pending().expect_err("503 is transient");
                assert!(err.contains("503"), "{err}");
                assert_eq!(
                    load().expect("load").expect("t").pending_token.as_deref(),
                    Some("tok-q"),
                    "a transient failure keeps the queued release for the next boot"
                );
            });
        });
    }

    #[test]
    fn replay_pending_noop_without_tombstone_or_when_reported() {
        with_temp_home(|| {
            assert_eq!(replay_pending().expect("no tombstone"), ReplayOutcome::Nothing);
            save(&UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                token_sha256: token_fingerprint("tok"),
                upstream_reported: true,
                holder_at_release: true,
                ..Default::default()
            })
            .expect("save reported tombstone");
            assert_eq!(replay_pending().expect("already reported"), ReplayOutcome::Nothing);
        });
    }

    #[test]
    fn replay_pending_airgap_does_not_post() {
        with_temp_home(|| {
            let _air = crate::test_env::EnvVar::set("K2_AIRGAP", "1");
            let cp = CountingControlPlane::start(200);
            save(&queued_holder_tombstone("tok-q")).expect("save leftover unpaired.json");
            assert_eq!(replay_pending().expect("airgap skip"), ReplayOutcome::Nothing);
            assert_eq!(cp.hits(), 0, "air-gap must not POST /tunnel/release");
        });
    }

    #[test]
    fn replay_pending_succeeds_against_a_fake_control_plane_and_scrubs_token() {
        with_temp_home(|| {
            // Fake control plane: accept one POST /tunnel/release, 200 it.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake relay");
            let port = listener.local_addr().expect("addr").port();
            let srv = std::thread::spawn(move || {
                use std::io::{Read, Write};
                let (mut sock, _) = listener.accept().expect("accept");
                sock.set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("read timeout");
                let mut req = String::new();
                let mut buf = [0u8; 4096];
                for _ in 0..32 {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            req.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if req.contains("\r\n\r\n") && req.trim_end().ends_with('}') {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = sock.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                );
                req
            });

            let var = super::super::subdomains::CONTROL_PLANE_BASE_ENV;
            let _cp = crate::test_env::EnvVar::set(var, format!("http://127.0.0.1:{port}"));
            save(&queued_holder_tombstone("tok-q")).expect("save queued tombstone");

            assert_eq!(replay_pending().expect("replay"), ReplayOutcome::Reported);
            let req = srv.join().expect("fake relay thread");
            assert!(req.starts_with("POST /tunnel/release "), "wire contract\n{req}");
            assert!(req.contains("Bearer tok-q"), "{req}");
            assert!(req.contains("\"subdomain\":\"rosson\""), "{req}");
            assert!(req.contains("\"deviceId\":\"dev-abc\""), "the holder check needs the device id\n{req}");
            // A replay reports the time of the RELEASE, not of the replay.
            assert!(
                req.contains("\"releasedAt\":\"2026-07-12T00:00:00+00:00\""),
                "releasedAt must be the tombstone's stored released_at\n{req}"
            );

            let t = load().expect("load").expect("tombstone");
            assert!(t.upstream_reported);
            assert_eq!(t.pending_token, None, "pending bearer must be scrubbed after replay");
        });
    }

    #[test]
    fn owner_repair_with_a_fresh_token_frees_the_device_id_only() {
        with_temp_home(|| {
            save(&UnpairTombstone {
                device_id: Some("dev-app".to_string()),
                ..queued_holder_tombstone("tok-old")
            })
            .expect("save");
            assert_eq!(note_repair("tok-new").expect("repair"), RepairOutcome::FreedDeviceId);
            let t = load().expect("load").expect("tombstone kept");
            assert_eq!(t.device_id, None, "the app's device id no longer matches");
            assert_eq!(t.pending_token.as_deref(), Some("tok-old"), "the queued release stays");
            // The same app device id with the new token passes the gate...
            let fresh = TunnelConfig {
                token: "tok-new".to_string(),
                device_id: Some("dev-app".to_string()),
                ..Default::default()
            };
            assert!(identity_released(&fresh).is_none());
            assert!(!released_state(&fresh));
            // ...the released bearer is still refused.
            assert!(identity_released(&TunnelConfig {
                token: "tok-old".to_string(),
                ..Default::default()
            })
            .is_some());
        });
    }

    #[test]
    fn owner_rebinding_the_released_token_removes_the_tombstone() {
        // "Switch back to the old name": the queued revocation would revoke
        // the token the device now uses, so it goes with the tombstone.
        with_temp_home(|| {
            save(&queued_holder_tombstone("tok-old")).expect("save");
            assert_eq!(note_repair(" tok-old ").expect("repair"), RepairOutcome::Removed);
            assert_eq!(load().expect("load"), None);
            assert_eq!(note_repair("tok-x").expect("no tombstone"), RepairOutcome::NotReleased);
        });
    }
}
