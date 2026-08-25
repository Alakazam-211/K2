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
//!      keypair (`tunnel-key.pem` / `tunnel-cert.pem`).
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
/// the upstream revocation landed or was queued.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseReport {
    /// The released subdomain label.
    pub subdomain: String,
    /// True when the upstream revocation landed during the release; false
    /// when it was queued for boot-time replay (offline release).
    pub upstream_reported: bool,
    /// True when this call found the device ALREADY released (idempotent
    /// re-release) — nothing new was deleted.
    pub already_released: bool,
}

/// Report the release upstream so the relay refuses any future connection
/// presenting the released identity (PRD §2B — the client half of the
/// revocation contract; the relay-side refusal is implemented in the
/// proprietary k2-connect control plane).
///
/// Contract: `POST {control-plane}/tunnel/release`, `Authorization:
/// Bearer <released token>`, JSON body `{"subdomain": ..., "deviceId": ...}`.
/// Non-2xx (including a control plane that predates the endpoint) is an
/// error — the caller queues the revocation rather than pretending.
pub fn report_upstream(
    subdomain: &str,
    device_id: Option<&str>,
    token: &str,
) -> Result<(), String> {
    if token.trim().is_empty() {
        return Err("no bearer token to authenticate the release report".to_string());
    }
    let base = super::subdomains::control_plane_base();
    let url = format!("{base}/tunnel/release");
    let body = serde_json::json!({
        "subdomain": subdomain,
        "deviceId": device_id,
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .post(&url)
        .bearer_auth(token.trim())
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("POST {url}: HTTP {status}"));
    }
    Ok(())
}

/// Perform the release: stop the connector, report upstream (queue on
/// failure), tombstone, and delete the identity files. See the module doc
/// for the full ordering rationale.
///
/// Idempotent-honest: releasing an already-released device (tombstone
/// present, no fresh identity) returns `already_released: true` and
/// touches nothing; releasing a device with NO identity and NO tombstone
/// is an error (there is nothing to release — surfacing that beats a
/// no-op that reads as success).
pub fn perform_release() -> Result<ReleaseReport, String> {
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
            }),
            None => Err(
                "no tunnel identity on this device — nothing to release \
                 (tunnel.json has no token)"
                    .to_string(),
            ),
        };
    }

    // 1. Tunnel down first — a releasing device must stop contesting the
    //    name immediately, before any network round-trip.
    super::connector::stop()?;

    // 2. Upstream revocation (the load-bearing half). Offline release is
    //    tolerated: the failure is queued in the tombstone and replayed at
    //    boot; the portal force-release covers a device that never comes
    //    back.
    let subdomain = cfg.subdomain.trim().to_string();
    let upstream_reported =
        match report_upstream(&subdomain, cfg.device_id.as_deref(), &token) {
            Ok(()) => true,
            Err(e) => {
                crate::log_debug!(
                    "[tunnel/unpair] upstream release report failed — queued for boot-time replay: {e}"
                );
                false
            }
        };

    // 3. Tombstone BEFORE deletion: if the deletes below fail halfway, the
    //    gate already refuses the identity (fail closed, never fail open).
    save(&UnpairTombstone {
        released_at: chrono::Utc::now().to_rfc3339(),
        subdomain: subdomain.clone(),
        device_id: cfg.device_id.clone(),
        token_sha256: token_fingerprint(&token),
        upstream_reported,
        pending_token: (!upstream_reported).then_some(token),
    })?;

    // 4. Delete the identity: tunnel.json (device id + bearer), the
    //    rendered frpc.toml (embeds the bearer), and the E2E cert keypair.
    //    A failed delete is an ERROR — a release that leaves the bearer on
    //    disk must not report success. (The tombstone is already down, so
    //    even the failure state is gate-blocked.)
    for path in [
        config::config_path(),
        super::connector::frpc_config_path(),
        super::tls::key_path(),
        super::tls::cert_path(),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("delete {}: {e}", path.display())),
        }
    }

    Ok(ReleaseReport {
        subdomain,
        upstream_reported,
        already_released: false,
    })
}

/// Boot-time replay of a queued upstream revocation (offline release,
/// PRD §2B). `Ok(true)` = a pending report was replayed and the token
/// scrubbed; `Ok(false)` = nothing pending. A replay FAILURE is an error
/// (logged by the caller; retried next boot).
pub fn replay_pending() -> Result<bool, String> {
    if crate::airgap::enabled() {
        return Ok(false);
    }
    let mut t = match load()? {
        Some(t) if !t.upstream_reported => t,
        _ => return Ok(false),
    };
    let token = match t.pending_token.as_deref() {
        Some(tok) if !tok.trim().is_empty() => tok.trim().to_string(),
        // Reported=false but no token to replay with — the portal
        // force-release is the only remaining path; nothing to do here.
        _ => return Ok(false),
    };
    report_upstream(&t.subdomain, t.device_id.as_deref(), &token)?;
    t.upstream_reported = true;
    t.pending_token = None; // scrub the secret the moment the report lands
    save(&t)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    /// Point the control plane at a dead local port so upstream reports
    /// fail FAST (connection refused) instead of dialing the real
    /// connect.k2.dev from a unit test. Restores the prior value.
    fn with_dead_control_plane<F: FnOnce()>(f: F) {
        let var = super::super::subdomains::CONTROL_PLANE_BASE_ENV;
        let prev = std::env::var_os(var);
        std::env::set_var(var, "http://127.0.0.1:9");
        f();
        match prev {
            Some(p) => std::env::set_var(var, p),
            None => std::env::remove_var(var),
        }
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

    #[test]
    fn perform_release_tombstones_deletes_identity_and_queues_upstream() {
        with_temp_home(|| {
            with_dead_control_plane(|| {
                config::save(&TunnelConfig {
                    token: "tok-live".to_string(),
                    subdomain: "rosson".to_string(),
                    device_id: Some("dev-abc".to_string()),
                    ..Default::default()
                })
                .expect("seed identity");

                let report = perform_release().expect("release");
                assert_eq!(report.subdomain, "rosson");
                assert!(!report.already_released);
                // Control plane unreachable → queued, honestly reported.
                assert!(!report.upstream_reported, "dead relay must queue, not lie");

                // Identity files are GONE.
                assert!(!config::config_path().exists(), "tunnel.json must be deleted");
                // Tombstone records the release + holds the pending token
                // for the boot-time replay.
                let t = load().expect("load").expect("tombstone written");
                assert_eq!(t.subdomain, "rosson");
                assert_eq!(t.device_id.as_deref(), Some("dev-abc"));
                assert_eq!(t.token_sha256, token_fingerprint("tok-live"));
                assert!(!t.upstream_reported);
                assert_eq!(t.pending_token.as_deref(), Some("tok-live"));

                // Releasing again is idempotent-honest, not an error.
                let again = perform_release().expect("re-release");
                assert!(again.already_released);
                assert_eq!(again.subdomain, "rosson");
            });
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
    fn replay_pending_noop_without_tombstone_or_when_reported() {
        with_temp_home(|| {
            assert_eq!(replay_pending().expect("no tombstone"), false);
            save(&UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                device_id: None,
                token_sha256: token_fingerprint("tok"),
                upstream_reported: true,
                pending_token: None,
            })
            .expect("save reported tombstone");
            assert_eq!(replay_pending().expect("already reported"), false);
        });
    }

    #[test]
    fn replay_pending_airgap_does_not_post() {
        with_temp_home(|| {
            let prev_air = std::env::var_os("K2_AIRGAP");
            std::env::set_var("K2_AIRGAP", "1");
            let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy bind");
            spy.set_nonblocking(true).expect("nonblocking");
            let port = spy.local_addr().expect("addr").port();
            let var = super::super::subdomains::CONTROL_PLANE_BASE_ENV;
            let prev_cp = std::env::var_os(var);
            std::env::set_var(var, format!("http://127.0.0.1:{port}"));

            save(&UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "leftover".to_string(),
                device_id: Some("dev".to_string()),
                token_sha256: token_fingerprint("tok-q"),
                upstream_reported: false,
                pending_token: Some("tok-q".to_string()),
            })
            .expect("save leftover unpaired.json");

            let posted = replay_pending().expect("airgap skip must not error");
            assert!(!posted, "airgap must skip replay_pending POST");
            assert!(
                spy.accept().is_err(),
                "leftover unpaired.json must not POST /tunnel/release when air-gap is on"
            );

            match prev_cp {
                Some(p) => std::env::set_var(var, p),
                None => std::env::remove_var(var),
            }
            match prev_air {
                Some(p) => std::env::set_var("K2_AIRGAP", p),
                None => std::env::remove_var("K2_AIRGAP"),
            }
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
                // Read until the JSON body's closing brace arrives — the
                // request may land in more than one TCP segment.
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
            let prev = std::env::var_os(var);
            std::env::set_var(var, format!("http://127.0.0.1:{port}"));

            save(&UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                device_id: Some("dev-abc".to_string()),
                token_sha256: token_fingerprint("tok-q"),
                upstream_reported: false,
                pending_token: Some("tok-q".to_string()),
            })
            .expect("save queued tombstone");

            let replayed = replay_pending().expect("replay against fake relay");
            assert!(replayed, "queued report must replay");

            let req = srv.join().expect("fake relay thread");
            assert!(req.starts_with("POST /tunnel/release "), "wire contract\n{req}");
            assert!(req.contains("Bearer tok-q"), "must authenticate with the released bearer\n{req}");
            assert!(req.contains("\"subdomain\":\"rosson\""), "{req}");

            // Reported + secret scrubbed.
            let t = load().expect("load").expect("tombstone");
            assert!(t.upstream_reported);
            assert_eq!(t.pending_token, None, "pending bearer must be scrubbed after replay");

            match prev {
                Some(p) => std::env::set_var(var, p),
                None => std::env::remove_var(var),
            }
        });
    }
}
