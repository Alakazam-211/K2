//! Auto-heal (§8.2): the daemon keeps the root helper at its own version.
//!
//! After boot (= after every daemon hop) and on `k2 root-helper update`:
//! 1. `helper version`; at or above the daemon's version → done;
//! 2. package-owned → skip; air-gapped → skip (owner copies signed assets
//!    in and runs `k2 root-helper update --from <dir>`);
//! 3. a legacy (protocol 1–2) helper can't self-update → "needs one root
//!    step";
//! 4. fetch `k2-root-helper-linux-<arch>` + `.sig` for THIS daemon's
//!    version; pre-verify here (signature, trusted comment, marker) so a
//!    bad asset never costs a root call;
//! 5. `helper self-update`, then `helper version` again; wrong answer →
//!    `helper self-rollback` + status `rolled back`;
//! 6. 404 / offline → back off (1 h doubling, max 24 h). Never blocks boot.
//!
//! It shares the hosted-mail busy latch (enable/upgrade) so it never runs
//! inside a mail window.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};

use k2_root_proto::keys::TrustedKey;
use k2_root_proto::sig::{asset_name, encode_payload, verify_signed_build};
use k2_root_proto::version::{Semver, SELF_UPDATE_MIN_PROTOCOL};

use super::{Kind, RootHelperState};

/// A failed fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 404 (a dev build has no release assets).
    NotFound,
    Other(String),
}

/// Everything heal touches, so tests never run sudo or the network.
pub trait HealOps {
    fn probe(&self) -> RootHelperState;
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
    fn self_update(&self, payload: &[u8]) -> Result<Value, String>;
    fn self_rollback(&self) -> Result<Value, String>;
    /// Claim the hosted-mail busy latch for the install window only (the
    /// probe, the up-to-date check and the download run without it, so a
    /// user's `hostmail enable` is never refused by a routine heal).
    fn begin_busy(&self) -> bool;
    fn end_busy(&self);
}

/// What one heal did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    UpToDate {
        version: String,
    },
    Updated {
        from: String,
        to: String,
    },
    RolledBack {
        from: String,
        to: String,
        reason: String,
    },
    Skipped {
        reason: String,
    },
    NeedsRoot {
        reason: String,
    },
    /// Try again later (backoff).
    Retry {
        reason: String,
    },
    /// The hosted-mail busy latch was held.
    Deferred,
}

/// Inputs that aren't side effects.
#[derive(Debug, Clone)]
pub struct Plan {
    pub daemon_version: String,
    pub arch: String,
    /// Release download base (`…/releases/download`), or a `file://`-less
    /// local dir marker for `--from` (see [`DirOps`]).
    pub base_url: String,
    pub airgap: bool,
    pub keys: Vec<TrustedKey>,
}

pub fn heal_with(ops: &dyn HealOps, plan: &Plan) -> Outcome {
    let st = ops.probe();
    match st.kind {
        Kind::Missing => {
            return Outcome::NeedsRoot {
                reason: "k2-root-helper is not installed".into(),
            }
        }
        Kind::NotAllowed => {
            return Outcome::NeedsRoot {
                reason: "sudoers does not let this daemon user run k2-root-helper".into(),
            }
        }
        Kind::Root => {}
    }
    if st.protocol < SELF_UPDATE_MIN_PROTOCOL {
        return Outcome::NeedsRoot {
            reason: format!(
                "the helper speaks protocol {} and cannot update itself",
                st.protocol
            ),
        };
    }
    if st.package_owned {
        return Outcome::Skipped {
            reason: "managed by your package manager".into(),
        };
    }
    let Some(want) = Semver::parse(&plan.daemon_version) else {
        return Outcome::Skipped {
            reason: "daemon version is not x.y.z (dev build)".into(),
        };
    };
    let have = st.version.as_deref().and_then(Semver::parse);
    if have.is_some_and(|h| h >= want) {
        return Outcome::UpToDate {
            version: st.version.clone().unwrap_or_default(),
        };
    }
    if plan.airgap {
        return Outcome::Skipped {
            reason: "older than K2 (air-gapped; run k2 root-helper update --from <dir>)".into(),
        };
    }
    let asset = asset_name(&plan.arch);
    let url = format!("{}/v{want}/{asset}", plan.base_url.trim_end_matches('/'));
    let fetch = |u: &str| match ops.fetch(u) {
        Ok(b) => Ok(b),
        Err(FetchError::NotFound) => Err(format!("no signed helper asset at {u}")),
        Err(FetchError::Other(e)) => Err(format!("fetch {u}: {e}")),
    };
    let elf = match fetch(&url) {
        Ok(b) => b,
        Err(reason) => return Outcome::Retry { reason },
    };
    let sig = match fetch(&format!("{url}.sig")).map(|b| String::from_utf8_lossy(&b).to_string()) {
        Ok(s) => s,
        Err(reason) => return Outcome::Retry { reason },
    };
    // Advisory pre-verify: the helper checks all of this again as root.
    let built = match verify_signed_build(&elf, &sig, &plan.keys, &plan.arch) {
        Ok(b) => b,
        Err(e) => {
            return Outcome::Retry {
                reason: format!("downloaded helper refused before install: {e}"),
            }
        }
    };
    if built.version != want {
        return Outcome::Retry {
            reason: format!(
                "signed helper is v{}, this daemon is v{want}",
                built.version
            ),
        };
    }
    if !ops.begin_busy() {
        return Outcome::Deferred;
    }
    let out = install_window(ops, &st, want, &sig, &elf);
    ops.end_busy();
    out
}

/// self-update → re-check → rollback, inside the busy latch.
fn install_window(
    ops: &dyn HealOps,
    st: &RootHelperState,
    want: Semver,
    sig: &str,
    elf: &[u8],
) -> Outcome {
    let from = st.version.clone().unwrap_or_default();
    if let Err(e) = ops.self_update(&encode_payload(sig, elf)) {
        return Outcome::Retry {
            reason: format!("self-update refused: {e}"),
        };
    }
    let after = ops.probe();
    if after.version.as_deref() != Some(&want.to_string())
        || after.protocol < SELF_UPDATE_MIN_PROTOCOL
    {
        let reason = after
            .error
            .clone()
            .unwrap_or_else(|| format!("after the update the helper answers {:?}", after.version));
        return match ops.self_rollback() {
            Ok(_) => Outcome::RolledBack {
                from: want.to_string(),
                to: from,
                reason,
            },
            Err(e) => Outcome::Retry {
                reason: format!("{reason}; rollback failed: {e}"),
            },
        };
    }
    Outcome::Updated {
        from,
        to: want.to_string(),
    }
}

/// The daemon's latest heal, for status.
#[derive(Debug, Default, Clone)]
pub(super) struct HealStatus {
    pub(super) last_attempt: Option<i64>,
    pub(super) last_result: Option<Outcome>,
    pub(super) next_attempt: Option<i64>,
}

static STATUS: Mutex<Option<HealStatus>> = Mutex::new(None);

/// The `autoHeal` block of `GET /cli/root-helper/status`, against the live
/// probe `live` that the same status read made.
pub fn status_value(live: &RootHelperState) -> Value {
    let s = STATUS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .unwrap_or_default();
    status_from(&s, live)
}

/// A `needs_root` result is about the helper the heal saw (missing, not
/// allowed, or too old to self-update). Once the live probe shows a helper
/// that is installed, allowed and able to self-update (an admin ran the
/// installer after that heal), that result contradicts the live state: it
/// moves out of `lastResult` into `superseded` with its time, so status
/// never reports "not installed" for an installed helper.
pub(super) fn status_from(s: &HealStatus, live: &RootHelperState) -> Value {
    let healthy = live.kind == Kind::Root && live.protocol >= SELF_UPDATE_MIN_PROTOCOL;
    match &s.last_result {
        Some(Outcome::NeedsRoot { reason }) if healthy => json!({
            "lastAttempt": s.last_attempt,
            "lastResult": null,
            "superseded": {
                "result": "needs_root",
                "reason": reason,
                "at": s.last_attempt,
                "why": format!(
                    "the helper is installed now (v{}, protocol {})",
                    live.version.as_deref().unwrap_or("?"),
                    live.protocol
                ),
            },
            "nextAttempt": s.next_attempt,
        }),
        _ => json!({
            "lastAttempt": s.last_attempt,
            "lastResult": s.last_result,
            "nextAttempt": s.next_attempt,
        }),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn audit(event: &str, user: &str, outcome: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        outcome.to_string(),
        "daemon",
        "-",
        "cli",
    ));
}

/// One heal under the hosted-mail busy latch; records status + audit rows.
pub fn run_once(ops: &dyn HealOps, plan: &Plan, who: &str) -> Outcome {
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| heal_with(ops, plan)))
        .unwrap_or(Outcome::Retry {
            reason: "heal panicked".into(),
        });
    super::invalidate();
    match &out {
        Outcome::Updated { from, to } => audit(
            "root_helper_self_update",
            who,
            &format!("ok {from} -> {to}"),
        ),
        Outcome::RolledBack { from, to, reason } => audit(
            "root_helper_rollback",
            who,
            &format!("{from} -> {to}: {reason}"),
        ),
        Outcome::Retry { reason } => {
            audit("root_helper_self_update", who, &format!("failed: {reason}"))
        }
        _ => {}
    }
    let mut s = STATUS.lock().unwrap_or_else(|p| p.into_inner());
    let st = s.get_or_insert_with(HealStatus::default);
    st.last_attempt = Some(now());
    st.last_result = Some(out.clone());
    out
}

/// Production ops: sudo door + HTTP.
pub struct LiveOps;

impl HealOps for LiveOps {
    fn probe(&self) -> RootHelperState {
        super::invalidate();
        super::probe_with(&super::RealDoor)
    }
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let resp = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| FetchError::Other(e.to_string()))?
            .get(url)
            .send()
            .map_err(|e| FetchError::Other(e.to_string()))?;
        if resp.status().as_u16() == 404 {
            return Err(FetchError::NotFound);
        }
        if !resp.status().is_success() {
            return Err(FetchError::Other(format!("HTTP {}", resp.status())));
        }
        let b = resp.bytes().map_err(|e| FetchError::Other(e.to_string()))?;
        if b.len() > k2_root_proto::verbs::CAP_SELF_UPDATE {
            return Err(FetchError::Other(
                "asset larger than the self-update cap".into(),
            ));
        }
        Ok(b.to_vec())
    }
    fn self_update(&self, payload: &[u8]) -> Result<Value, String> {
        let out = super::call_text(&["helper".into(), "self-update".into()], Some(payload))?;
        serde_json::from_str(&out).map_err(|_| "self-update answer is not JSON".into())
    }
    fn self_rollback(&self) -> Result<Value, String> {
        let out = super::call_text(&["helper".into(), "self-rollback".into()], None)?;
        serde_json::from_str(&out).map_err(|_| "self-rollback answer is not JSON".into())
    }
    fn begin_busy(&self) -> bool {
        super::busy::begin("the root helper's self-update")
    }
    fn end_busy(&self) {
        super::busy::end();
    }
}

/// `k2 root-helper update --from <dir>`: the owner copied
/// `k2-root-helper-linux-<arch>` + `.sig` into a local directory (air-gap).
pub struct DirOps {
    pub dir: std::path::PathBuf,
}

impl HealOps for DirOps {
    fn probe(&self) -> RootHelperState {
        LiveOps.probe()
    }
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let name = url.rsplit('/').next().unwrap_or_default();
        if name.is_empty() || name.contains("..") {
            return Err(FetchError::Other("bad asset name".into()));
        }
        match std::fs::read(self.dir.join(name)) {
            Ok(b) => Ok(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(FetchError::NotFound),
            Err(e) => Err(FetchError::Other(e.to_string())),
        }
    }
    fn self_update(&self, payload: &[u8]) -> Result<Value, String> {
        LiveOps.self_update(payload)
    }
    fn self_rollback(&self) -> Result<Value, String> {
        LiveOps.self_rollback()
    }
    fn begin_busy(&self) -> bool {
        LiveOps.begin_busy()
    }
    fn end_busy(&self) {
        LiveOps.end_busy()
    }
}

pub fn live_plan(airgap_override: Option<bool>) -> Plan {
    Plan {
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        arch: std::env::consts::ARCH.to_string(),
        base_url: super::RELEASE_DOWNLOAD_BASE.to_string(),
        airgap: airgap_override.unwrap_or_else(k2_core::airgap::enabled),
        keys: k2_root_proto::keys::RELEASE_KEYS.to_vec(),
    }
}

/// Boot: one detached thread (Linux only). First try ~2 min after boot;
/// then daily when current, or backoff 1 h doubling to 24 h on a retry.
pub fn spawn_auto_heal() {
    if cfg!(test) || !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("k2-root-helper-heal".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(120));
            let mut backoff = 3600u64;
            loop {
                let out = run_once(&LiveOps, &live_plan(None), "daemon");
                // The helper is at the daemon's version now (or says why
                // not): keep a root-owned /usr/local/bin/k2 at it too.
                let cli = super::cli_refresh::run_once(
                    &super::cli_refresh::LiveCliOps,
                    &super::cli_refresh::daemon_cli(),
                    "daemon",
                );
                k2_core::log_debug!("[root-helper] cli refresh: {:?}", cli);
                // 0.46.5: a helper that just updated (or is current while
                // the unit's drop-in is still an older helper's) converges
                // the hand-off drop-in now; no root call when it's current.
                if matches!(out, Outcome::Updated { .. } | Outcome::UpToDate { .. }) {
                    let d = crate::handoff_setup::after_helper_update(true);
                    k2_core::log_debug!("[root-helper] unit drop-in after heal: {d}");
                }
                let wait = match out {
                    Outcome::Deferred => 300,
                    Outcome::Retry { .. } => {
                        let w = backoff;
                        backoff = (backoff * 2).min(86_400);
                        w
                    }
                    _ => {
                        backoff = 3600;
                        86_400
                    }
                };
                if let Some(s) = STATUS.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
                    s.next_attempt = Some(now() + wait as i64);
                }
                k2_core::log_debug!("[root-helper] heal: {:?}; next in {wait}s", out);
                std::thread::sleep(std::time::Duration::from_secs(wait));
            }
        });
}
