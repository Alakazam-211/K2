//! Tunnel identity changes (0.45.2): ONE path that moves every
//! subdomain-bound part of the daemon to a new K2 Connect name.
//!
//! The bug this exists for (Reggie's Mac mini, 0.45.0): the owner released
//! `afs` in Settings and connected `k2n1-m4`. `tunnel.json` and frpc moved
//! to the new name, but the E2E TLS listener (and its cert renewer) kept
//! serving the `afs.k2.dev` certificate, because the listener was bound
//! once per daemon process and the tunnel start reused any live listener
//! regardless of its name. Direct clients failed the TLS handshake
//! (`CertificateUnknown`) until the daemon was restarted.
//!
//! The identity is `(subdomain, bearer token, effective E2E flag)`. When it
//! changes — a config write, a switch, a release — [`reconcile`]:
//!
//!   1. **listener**: stops the E2E listener and its cert renewer when they
//!      serve another name (daemon hook [`super::tls::retire_https_listener_unless`]);
//!   2. **routing**: resets the nested-subdomain map to the new primary, so
//!      `<label>.<old>.k2.dev` routes never apply to the new name and a late
//!      refresh for the old name is discarded;
//!   3. **certificate**: when the tunnel will run on E2E, obtains the
//!      certificate for the new name and brings the listener up for it
//!      (a broker refusal is reported as THIS step, not a vague start error);
//!   4. **tunnel**: restarts frpc under the new name when it was running
//!      (or when the caller asked to start).
//!
//! [`switch_subdomain`] wraps a config write + reconcile and ROLLS BACK
//! (old `tunnel.json`, old tombstone, old tunnel) when a step fails, so a
//! failed switch leaves the old name working.
//!
//! The daemon emits `tunnel_status_changed` / `tunnel_subdomains_changed`
//! after these calls so every client refreshes without a restart.

use std::fs;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use super::config::{self, TunnelConfig, SUBDOMAIN_HOST};
use super::connector::{self, FrpcBinary, TunnelStatus};

/// What makes a running tunnel "the same tunnel". Device id, label,
/// auto-start and relays are not part of it (changing those never needs a
/// new certificate or a new listener).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelIdentity {
    /// Trimmed, lowercase subdomain label (empty = none).
    pub subdomain: String,
    /// SHA-256 of the bearer token (empty = no token). Never the secret.
    pub token_fp: String,
    /// The EFFECTIVE E2E flag (config + `K2_E2E` override).
    pub e2e: bool,
}

impl TunnelIdentity {
    pub fn of(cfg: &TunnelConfig) -> Self {
        let token = cfg.token.trim();
        Self {
            subdomain: cfg.subdomain.trim().to_ascii_lowercase(),
            token_fp: if token.is_empty() {
                String::new()
            } else {
                super::unpair::token_fingerprint(token)
            },
            e2e: config::e2e_enabled(cfg),
        }
    }

    /// The name the E2E listener must serve for this identity, or `None`
    /// when no listener is wanted (E2E off, air-gap, no subdomain).
    pub fn listener_subdomain(&self) -> Option<&str> {
        if !self.e2e || self.subdomain.is_empty() || crate::airgap::enabled() {
            None
        } else {
            Some(self.subdomain.as_str())
        }
    }
}

fn host_of(sub: &str) -> String {
    format!("{sub}.{SUBDOMAIN_HOST}")
}

/// One visible step of an identity change (the Settings switch flow shows
/// these to the owner).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IdentityStep {
    /// `config` | `listener` | `routing` | `certificate` | `tunnel` | `rollback`.
    pub step: String,
    pub ok: bool,
    pub detail: String,
}

impl IdentityStep {
    fn ok(step: &str, detail: impl Into<String>) -> Self {
        Self { step: step.to_string(), ok: true, detail: detail.into() }
    }
    fn failed(step: &str, detail: impl Into<String>) -> Self {
        Self { step: step.to_string(), ok: false, detail: detail.into() }
    }
}

/// The result of an identity change, returned to the route/CLI/GUI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityChangeReport {
    /// Every step succeeded (for a switch: the new name is live, or saved
    /// when the tunnel was not asked to run).
    pub ok: bool,
    /// The identity actually changed (false = nothing to do).
    pub changed: bool,
    pub previous_subdomain: Option<String>,
    pub subdomain: Option<String>,
    pub steps: Vec<IdentityStep>,
    /// The first failure, verbatim, when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A failed switch restored the previous name.
    pub rolled_back: bool,
    /// Tunnel status after the change.
    pub status: TunnelStatus,
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Serializes identity changes (two concurrent switches must not
/// interleave their stop/start/rollback).
fn identity_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Move the daemon to the identity now in `tunnel.json`. `previous` is the
/// identity before the change. `run` = the tunnel should be up afterwards
/// (it was running, or the caller asked to start). See the module doc for
/// the steps. Never panics on a failed step: the report says which one.
pub fn reconcile(
    previous: &TunnelIdentity,
    run: bool,
    daemon_port: u16,
    bin: &FrpcBinary,
) -> IdentityChangeReport {
    let _g = identity_lock().lock().unwrap_or_else(|p| p.into_inner());
    reconcile_locked(previous, run, daemon_port, bin)
}

fn reconcile_locked(
    previous: &TunnelIdentity,
    run: bool,
    daemon_port: u16,
    bin: &FrpcBinary,
) -> IdentityChangeReport {
    let mut steps = Vec::new();
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            return IdentityChangeReport {
                ok: false,
                changed: false,
                previous_subdomain: non_empty(&previous.subdomain),
                subdomain: None,
                steps: vec![IdentityStep::failed("config", e.clone())],
                error: Some(e),
                rolled_back: false,
                status: connector::status(),
            }
        }
    };
    let current = TunnelIdentity::of(&cfg);
    let changed = current != *previous;
    let mut error: Option<String> = None;

    // 1. listener — stop a listener (and renewer) serving another name.
    let wanted = current.listener_subdomain().map(str::to_string);
    match super::tls::retire_https_listener_unless(wanted.as_deref()) {
        Some(old) => steps.push(IdentityStep::ok(
            "listener",
            format!("stopped the encrypted listener and certificate renewer for {}", host_of(&old)),
        )),
        None => steps.push(IdentityStep::ok(
            "listener",
            match wanted.as_deref() {
                Some(_) => "no listener for another name was running".to_string(),
                None => "no encrypted listener needed".to_string(),
            },
        )),
    }

    // 2. routing — nested routes belong to one primary.
    if super::subdomains::current().primary != current.subdomain {
        let old = super::subdomains::current().primary.clone();
        super::subdomains::reset_for_primary(&current.subdomain);
        steps.push(IdentityStep::ok(
            "routing",
            if old.is_empty() {
                "nested routes start empty for the new name".to_string()
            } else {
                format!("cleared the nested routes of {}", host_of(&old))
            },
        ));
    }

    // 3 + 4. certificate, then the tunnel itself.
    let running_cfg = connector::running_config();
    let running_same = running_cfg
        .as_ref()
        .is_some_and(|r| TunnelIdentity::of(r) == current);
    let gate_open = cfg.enabled && super::unpair::identity_released(&cfg).is_none();
    if run && cfg.is_connectable() && gate_open {
        if let Some(sub) = wanted.as_deref() {
            match super::tls::ensure_https_port(daemon_port) {
                Ok(port) => steps.push(IdentityStep::ok(
                    "certificate",
                    format!("certificate and encrypted listener ready for {} (port {port})", host_of(sub)),
                )),
                Err(e) => {
                    steps.push(IdentityStep::failed("certificate", e.clone()));
                    error = Some(e);
                }
            }
        }
        if error.is_none() {
            if running_cfg.is_some() && !running_same {
                if let Err(e) = connector::stop() {
                    steps.push(IdentityStep::failed("tunnel", format!("stop old tunnel: {e}")));
                    error = Some(e);
                }
            }
        }
        if error.is_none() {
            match connector::start(None, daemon_port, bin) {
                Ok(st) => steps.push(IdentityStep::ok(
                    "tunnel",
                    match st.public_url.as_deref() {
                        Some(url) => format!("tunnel connected as {url}"),
                        None => "tunnel connected".to_string(),
                    },
                )),
                Err(e) => {
                    steps.push(IdentityStep::failed("tunnel", e.clone()));
                    error = Some(e);
                }
            }
        }
    } else {
        // Not running afterwards. A tunnel still up under another identity
        // (config moved under it, released, disabled) must go down: it
        // would keep serving the old name.
        if running_cfg.is_some() && !running_same {
            match connector::stop() {
                Ok(()) => steps.push(IdentityStep::ok("tunnel", "stopped the tunnel for the old name")),
                Err(e) => {
                    steps.push(IdentityStep::failed("tunnel", e.clone()));
                    error = Some(e);
                }
            }
        } else if run {
            let why = if !cfg.is_connectable() {
                "no subdomain is bound to this device"
            } else if !cfg.enabled {
                "the tunnel is disabled on this device"
            } else {
                "this device's tunnel identity was released"
            };
            steps.push(IdentityStep::ok("tunnel", format!("tunnel stays off: {why}")));
        } else {
            steps.push(IdentityStep::ok(
                "tunnel",
                "tunnel is off; the new name is used on the next Start",
            ));
        }
    }

    IdentityChangeReport {
        ok: error.is_none(),
        changed,
        previous_subdomain: non_empty(&previous.subdomain),
        subdomain: non_empty(&current.subdomain),
        steps,
        error,
        rolled_back: false,
        status: connector::status(),
    }
}

/// Apply a config update and move the daemon to the resulting identity.
/// A running tunnel whose identity changed is restarted under the new one
/// (the config is what runs; a config write no longer leaves the old name
/// half-alive). An explicit owner token write is a re-pair: see
/// [`super::unpair::note_repair`]. Returns the redacted view and, when
/// the identity changed, the report.
pub fn apply_config(
    upd: super::TunnelConfigUpdate,
    daemon_port: u16,
    bin: &FrpcBinary,
) -> Result<(super::TunnelConfigView, Option<IdentityChangeReport>), String> {
    let _g = identity_lock().lock().unwrap_or_else(|p| p.into_inner());
    let before = config::load()?;
    let previous = TunnelIdentity::of(&before);
    let was_running = connector::running_config().is_some();
    let new_token = upd.token.clone();
    super::set_config(upd)?;
    if let Some(t) = new_token.as_deref() {
        super::unpair::note_repair(t)?;
    }
    let after = config::load()?;
    if TunnelIdentity::of(&after) == previous {
        return Ok((super::get_config_view()?, None));
    }
    let report = reconcile_locked(&previous, was_running, daemon_port, bin);
    // The view re-reads disk so `released` reflects a re-pair.
    Ok((super::get_config_view()?, Some(report)))
}

/// A subdomain switch request (Settings picker / `k2 tunnel switch`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRequest {
    /// The new subdomain label.
    pub subdomain: String,
    /// Its tunnel token. May be omitted only when switching to the label
    /// already configured (re-run the switch to repair a stale listener).
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_label: Option<String>,
    /// Start (or keep) the tunnel on the new name. Defaults to whether the
    /// tunnel is running now.
    #[serde(default)]
    pub start: Option<bool>,
}

/// Valid subdomain label: 1–63 of `a-z 0-9 -`, not starting/ending in `-`.
pub fn valid_label(label: &str) -> bool {
    let l = label.trim();
    !l.is_empty()
        && l.len() <= 63
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Snapshot of the files a switch may change, for rollback.
struct DiskSnapshot {
    tunnel_json: Option<Vec<u8>>,
    tombstone: Option<Vec<u8>>,
    /// The installed certificate: a switch that failed after the new
    /// name's certificate was installed puts the old one back, so the
    /// rollback needs no broker round trip.
    cert: Option<Vec<u8>>,
}

fn read_opt(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

fn restore_opt(path: &std::path::Path, bytes: &Option<Vec<u8>>, mode: u32) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = mode;
    match bytes {
        Some(b) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
            }
            fs::write(path, b).map_err(|e| format!("restore {}: {e}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("chmod {}: {e}", path.display()))?;
            }
            Ok(())
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove {}: {e}", path.display())),
        },
    }
}

impl DiskSnapshot {
    fn take() -> Result<Self, String> {
        Ok(Self {
            tunnel_json: read_opt(&config::config_path())?,
            tombstone: read_opt(&super::unpair::tombstone_path())?,
            cert: read_opt(&super::tls::cert_path())?,
        })
    }
    fn restore(&self) -> Result<(), String> {
        // tunnel.json + the tombstone carry the bearer (0600); the cert is
        // public (0644, like `cert_broker::install_cert`).
        restore_opt(&config::config_path(), &self.tunnel_json, 0o600)?;
        restore_opt(&super::unpair::tombstone_path(), &self.tombstone, 0o600)?;
        restore_opt(&super::tls::cert_path(), &self.cert, 0o644)
    }
}

/// Switch this device to another subdomain in one step: write the new
/// binding, move every subdomain-bound component (see [`reconcile`]), and
/// bring the tunnel up on the new name when asked (or when it was up).
///
/// When a step fails, the previous `tunnel.json` and tombstone are put
/// back and the daemon is moved back to the old identity (the old tunnel
/// is restarted when it was running), so a failed switch leaves the old
/// name working. The report says which step failed and `rolledBack`.
///
/// Input errors (bad label, missing token) are `Err` and change nothing.
pub fn switch_subdomain(
    req: SwitchRequest,
    daemon_port: u16,
    bin: &FrpcBinary,
) -> Result<IdentityChangeReport, String> {
    crate::airgap::refuse()?;
    let label = req.subdomain.trim().to_ascii_lowercase();
    if !valid_label(&label) {
        return Err(format!(
            "invalid subdomain {:?}: use lowercase letters, digits and '-' (1–63 characters)",
            req.subdomain
        ));
    }
    let _g = identity_lock().lock().unwrap_or_else(|p| p.into_inner());

    let before = config::load()?;
    let token = match req.token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None if before.subdomain.trim().eq_ignore_ascii_case(&label)
            && !before.token.trim().is_empty() =>
        {
            before.token.trim().to_string()
        }
        None => {
            return Err(format!(
                "switching to {} needs its tunnel token (pick it from your subdomains in \
                 Settings → K2 Connect, or pass --token)",
                host_of(&label)
            ))
        }
    };
    let previous = TunnelIdentity::of(&before);
    let was_running = connector::running_config().is_some();
    let run = req.start.unwrap_or(was_running);
    let snapshot = DiskSnapshot::take()?;

    let mut steps = Vec::new();
    let write = (|| -> Result<(), String> {
        super::set_config(super::TunnelConfigUpdate {
            subdomain: Some(label.clone()),
            token: Some(token.clone()),
            device_id: req.device_id.clone(),
            device_label: req.device_label.clone(),
            ..Default::default()
        })?;
        // A switch is an explicit "use this name": raise a persisted pause
        // when the owner asked to run it, like Start does.
        if run {
            config::update(|c| c.enabled = true)?;
        }
        super::unpair::note_repair(&token)?;
        Ok(())
    })();
    if let Err(e) = write {
        let restore = snapshot.restore();
        return Err(match restore {
            Ok(()) => format!("save the new subdomain: {e} (nothing changed)"),
            Err(r) => format!("save the new subdomain: {e}; restoring the old config also failed: {r}"),
        });
    }
    steps.push(IdentityStep::ok(
        "config",
        match non_empty(&previous.subdomain) {
            Some(old) if old != label => format!("bound {} (was {})", host_of(&label), host_of(&old)),
            _ => format!("bound {}", host_of(&label)),
        },
    ));

    let mut report = reconcile_locked(&previous, run, daemon_port, bin);
    steps.append(&mut report.steps);
    report.steps = steps;
    if report.ok {
        return Ok(report);
    }

    // Roll back: old files, then the old identity (and the old tunnel when
    // it was running before the switch).
    let failed_identity = TunnelIdentity::of(&config::load().unwrap_or_default());
    match snapshot.restore() {
        Ok(()) => {
            let back = reconcile_locked(&failed_identity, was_running, daemon_port, bin);
            let detail = if back.ok {
                match non_empty(&previous.subdomain) {
                    Some(old) if was_running => format!("restored {} and its tunnel", host_of(&old)),
                    Some(old) => format!("restored {}", host_of(&old)),
                    None => "restored the previous (empty) tunnel config".to_string(),
                }
            } else {
                format!(
                    "restored the previous config, but bringing it back failed: {}",
                    back.error.clone().unwrap_or_default()
                )
            };
            report.steps.push(IdentityStep {
                step: "rollback".to_string(),
                ok: back.ok,
                detail,
            });
            report.rolled_back = true;
            report.subdomain = non_empty(&previous.subdomain);
            report.status = back.status;
        }
        Err(e) => {
            report.steps.push(IdentityStep::failed("rollback", e));
        }
    }
    Ok(report)
}

/// Release (unpair) this device and move the daemon off the released
/// name: stop the listener + renewer, clear nested routes. See
/// [`super::unpair::perform_release`] for the release itself.
pub fn release(
    mode: super::unpair::ReleaseMode,
    daemon_port: u16,
    bin: &FrpcBinary,
) -> Result<(super::unpair::ReleaseReport, IdentityChangeReport), String> {
    let _g = identity_lock().lock().unwrap_or_else(|p| p.into_inner());
    let previous = TunnelIdentity::of(&config::load()?);
    let released = super::unpair::perform_release_mode(mode)?;
    let report = reconcile_locked(&previous, false, daemon_port, bin);
    Ok((released, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::subdomains::{self, SubdomainMap};
    use crate::tunnel::test_support::with_temp_home;

    fn true_bin() -> FrpcBinary {
        for cand in ["/usr/bin/true", "/bin/true"] {
            let p = std::path::PathBuf::from(cand);
            if p.exists() {
                return FrpcBinary::Explicit(p);
            }
        }
        panic!("no `true` binary at /usr/bin/true or /bin/true");
    }

    /// Tunnel-ingress stub (E2E off in these tests: the E2E listener lives
    /// in k2-daemon and is covered by its switch integration test). Refuses
    /// the `broken` subdomain to drive the rollback path.
    fn install_ingress_stub() {
        super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| {
            match config::load() {
                Ok(c) if c.subdomain == "broken" => Err("ingress refused for broken".to_string()),
                Ok(_) => Ok(46123),
                Err(e) => Err(e),
            }
        }));
    }

    fn seed(sub: &str, token: &str) {
        config::save(&TunnelConfig {
            subdomain: sub.to_string(),
            token: token.to_string(),
            device_id: Some("dev-app".to_string()),
            e2e: false,
            ..Default::default()
        })
        .expect("seed tunnel.json");
    }

    /// Env + hooks every test here needs; hold the returned guards.
    fn env() -> (crate::test_env::EnvVar, crate::test_env::ErrorHttpServer, crate::test_env::EnvVar) {
        let lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
        let cp = crate::test_env::ErrorHttpServer::start(503);
        let cp_env = crate::test_env::EnvVar::set(
            super::super::subdomains::CONTROL_PLANE_BASE_ENV,
            cp.url(),
        );
        install_ingress_stub();
        (lease, cp, cp_env)
    }

    fn reset_routing() {
        subdomains::clear_expected_primary_for_test();
        subdomains::store(SubdomainMap::default());
    }

    fn frpc_toml() -> String {
        std::fs::read_to_string(connector::frpc_config_path()).expect("frpc.toml rendered")
    }

    fn running_sub() -> Option<String> {
        connector::running_config().map(|c| c.subdomain)
    }

    #[test]
    fn switch_while_running_restarts_the_tunnel_under_the_new_name() {
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            connector::start(None, 57839, &true_bin()).expect("start afs");
            subdomains::store(SubdomainMap {
                primary: "afs".to_string(),
                targets: [("app".to_string(), "localhost:3000".to_string())].into(),
            });

            let report = switch_subdomain(
                SwitchRequest {
                    subdomain: "k2n1-m4".to_string(),
                    token: Some("tok-new".to_string()),
                    device_id: Some("dev-app".to_string()),
                    ..Default::default()
                },
                57839,
                &true_bin(),
            )
            .expect("switch");
            assert!(report.ok, "{report:?}");
            assert!(report.changed && !report.rolled_back);
            assert_eq!(report.previous_subdomain.as_deref(), Some("afs"));
            assert_eq!(report.subdomain.as_deref(), Some("k2n1-m4"));
            assert_eq!(running_sub().as_deref(), Some("k2n1-m4"), "frpc restarted on the new name");
            assert!(report.status.running);
            let toml = frpc_toml();
            assert!(toml.contains("k2so-k2n1-m4") && !toml.contains("k2so-afs"), "{toml}");
            let cfg = config::load().expect("cfg");
            assert_eq!((cfg.subdomain.as_str(), cfg.token.as_str()), ("k2n1-m4", "tok-new"));
            // Nested routes of afs are gone.
            let map = subdomains::current();
            assert_eq!(map.primary, "k2n1-m4");
            assert!(map.targets.is_empty(), "afs's nested routes must not survive: {map:?}");
            let steps: Vec<&str> = report.steps.iter().map(|s| s.step.as_str()).collect();
            assert_eq!(steps, vec!["config", "listener", "routing", "tunnel"], "{report:?}");
            assert!(report.steps.iter().all(|s| s.ok), "{report:?}");
            let _ = connector::stop();
            reset_routing();
        });
    }

    #[test]
    fn failed_switch_rolls_back_and_keeps_the_old_name_running() {
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            std::fs::write(super::super::tls::cert_path(), "afs cert").expect("seed cert");
            let before = std::fs::read(config::config_path()).expect("cfg bytes");
            connector::start(None, 57839, &true_bin()).expect("start afs");

            let report = switch_subdomain(
                SwitchRequest {
                    subdomain: "broken".to_string(),
                    token: Some("tok-broken".to_string()),
                    ..Default::default()
                },
                57839,
                &true_bin(),
            )
            .expect("a failed step is a report, not an Err");
            assert!(!report.ok && report.rolled_back, "{report:?}");
            assert!(
                report.error.as_deref().is_some_and(|e| e.contains("ingress refused for broken")),
                "{report:?}"
            );
            let last = report.steps.last().expect("steps");
            assert_eq!(last.step, "rollback");
            assert!(last.ok, "{report:?}");
            assert_eq!(report.subdomain.as_deref(), Some("afs"));
            assert_eq!(
                std::fs::read(config::config_path()).expect("cfg bytes"),
                before,
                "tunnel.json is restored byte for byte"
            );
            assert_eq!(
                std::fs::read_to_string(super::super::tls::cert_path()).expect("cert"),
                "afs cert"
            );
            assert_eq!(running_sub().as_deref(), Some("afs"), "the old tunnel is back up");
            assert!(frpc_toml().contains("k2so-afs"));
            let _ = connector::stop();
            reset_routing();
        });
    }

    #[test]
    fn failed_switch_while_stopped_leaves_the_tunnel_stopped() {
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            let report = switch_subdomain(
                SwitchRequest {
                    subdomain: "broken".to_string(),
                    token: Some("tok-broken".to_string()),
                    start: Some(true),
                    ..Default::default()
                },
                57839,
                &true_bin(),
            )
            .expect("report");
            assert!(!report.ok && report.rolled_back, "{report:?}");
            assert_eq!(config::load().expect("cfg").subdomain, "afs");
            assert_eq!(running_sub(), None, "rollback must not start a tunnel that was off");
            reset_routing();
        });
    }

    #[test]
    fn config_write_with_a_new_subdomain_restarts_a_running_tunnel() {
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            connector::start(None, 57839, &true_bin()).expect("start afs");
            let (view, report) = apply_config(
                super::super::TunnelConfigUpdate {
                    subdomain: Some("k2n1-m4".to_string()),
                    token: Some("tok-new".to_string()),
                    ..Default::default()
                },
                57839,
                &true_bin(),
            )
            .expect("apply");
            assert_eq!(view.subdomain, "k2n1-m4");
            let report = report.expect("identity changed → report");
            assert!(report.ok, "{report:?}");
            assert_eq!(running_sub().as_deref(), Some("k2n1-m4"));

            // A device-id-only write is not an identity change.
            let (_, none) = apply_config(
                super::super::TunnelConfigUpdate {
                    device_id: Some("dev-2".to_string()),
                    ..Default::default()
                },
                57839,
                &true_bin(),
            )
            .expect("apply");
            assert!(none.is_none());
            assert_eq!(running_sub().as_deref(), Some("k2n1-m4"));
            let _ = connector::stop();
            reset_routing();
        });
    }

    #[test]
    fn start_restarts_a_tunnel_that_runs_another_identity() {
        // `start` used to return the running status for ANY live child —
        // after tunnel.json moved under it, frpc kept the old name.
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            connector::start(None, 57839, &true_bin()).expect("start afs");
            seed("k2n1-m4", "tok-new");
            let st = connector::start(None, 57839, &true_bin()).expect("start again");
            assert_eq!(st.subdomain.as_deref(), Some("k2n1-m4"), "{st:?}");
            assert_eq!(running_sub().as_deref(), Some("k2n1-m4"));
            // Same identity → idempotent (no restart, same status).
            let again = connector::start(None, 57839, &true_bin()).expect("idempotent");
            assert_eq!(again.subdomain.as_deref(), Some("k2n1-m4"));
            let _ = connector::stop();
        });
    }

    #[test]
    fn release_moves_the_daemon_off_the_name() {
        with_temp_home(|| {
            let _env = env();
            seed("afs", "tok-afs");
            connector::start(None, 57839, &true_bin()).expect("start afs");
            subdomains::store(SubdomainMap {
                primary: "afs".to_string(),
                targets: [("app".to_string(), "localhost:3000".to_string())].into(),
            });
            let (released, report) =
                release(super::super::unpair::ReleaseMode::ForgetLocal, 57839, &true_bin())
                    .expect("forget");
            assert!(released.local_only);
            assert!(report.ok, "{report:?}");
            assert_eq!(report.previous_subdomain.as_deref(), Some("afs"));
            assert_eq!(report.subdomain, None);
            assert_eq!(running_sub(), None);
            assert_eq!(subdomains::current().primary, "");
            assert!(subdomains::current().targets.is_empty());
            reset_routing();
        });
    }

    #[test]
    fn switch_rejects_bad_input_without_touching_anything() {
        with_temp_home(|| {
            seed("afs", "tok-afs");
            let before = std::fs::read(config::config_path()).expect("cfg");
            let bad = switch_subdomain(
                SwitchRequest { subdomain: "-bad-".into(), token: Some("t".into()), ..Default::default() },
                57839,
                &true_bin(),
            )
            .expect_err("bad label");
            assert!(bad.contains("invalid subdomain"), "{bad}");
            let no_token = switch_subdomain(
                SwitchRequest { subdomain: "other".into(), ..Default::default() },
                57839,
                &true_bin(),
            )
            .expect_err("token required for another name");
            assert!(no_token.contains("needs its tunnel token"), "{no_token}");
            assert_eq!(std::fs::read(config::config_path()).expect("cfg"), before);
            assert!(valid_label("k2n1-m4") && valid_label("a") && !valid_label("A") && !valid_label(""));
        });
    }
}
