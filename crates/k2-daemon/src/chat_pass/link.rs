//! Sew link state (S28 §11, K7): the install id a pass's `aud` names, and
//! the client URL the `--skin` gateway's Sew mode proxies to.
//!
//! Stored in `~/.k2/sew-link.json` (0600; nothing secret). Not in the
//! vault: it survives a vault restore, and the chat-pass keys stay the only
//! thing under `engine:chat-pass`. Not `~/.k2/chat-pass/` either: K6 moved
//! the keys into the vault and that directory must not come back.
//!
//! `install_id_for_mint`:
//! - cached and fetched < 600 s ago → use it;
//! - else ask Sew's mgmt `status` ([`SewStatus`]). A good answer is stored
//!   and used; the value must match `[1-9][0-9]{0,18}`;
//! - failure with a cached value → the stale value (the id only changes on
//!   a purge and reinstall), logged at most once per 10 min;
//! - failure with nothing cached → error → 503 `chat_unavailable`.
//!
//! The real `SewStatus` wraps S27's Sew mgmt client (`GET /mgmt/v1/status`
//! over the mgmt socket). S27 is not on this build, so no source is
//! installed by default and status fails at once (no 2 s wait); S29 calls
//! [`set_install_id`] after enable/upgrade/restore, and S27 installs the
//! live source with [`set_status_source`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

/// How long a fetched install id is fresh.
pub const INSTALL_ID_FRESH_SECS: i64 = 600;
/// Log a stale-id fallback at most this often.
const STALE_LOG_EVERY_SECS: i64 = 600;

/// `GET /mgmt/v1/status` → `install_id` (decimal string). Timeout 2 s.
pub trait SewStatus: Send + Sync {
    fn install_id(&self) -> Result<String, String>;
}

/// No Sew mgmt client configured (S27/S29 not on this box): fails at once.
pub struct NotConfigured;

impl SewStatus for NotConfigured {
    fn install_id(&self) -> Result<String, String> {
        Err("Sew is not configured on this box".into())
    }
}

fn status_slot() -> &'static RwLock<Option<Arc<dyn SewStatus>>> {
    static SLOT: RwLock<Option<Arc<dyn SewStatus>>> = RwLock::new(None);
    &SLOT
}

/// Install the live status source (S27's mgmt client).
pub fn set_status_source(src: Arc<dyn SewStatus>) {
    *status_slot().write().unwrap_or_else(|p| p.into_inner()) = Some(src);
}

/// The installed status source, or [`NotConfigured`].
pub fn status_source() -> Arc<dyn SewStatus> {
    status_slot().read().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| Arc::new(NotConfigured))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkState {
    pub format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id_fetched_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_url: Option<String>,
}

pub fn path() -> PathBuf {
    k2_core::paths::k2_home().join("sew-link.json")
}

static LOCK: Mutex<()> = Mutex::new(());
static LAST_STALE_LOG: Mutex<i64> = Mutex::new(i64::MIN);

/// `[1-9][0-9]{0,18}` (S1 §6.1 `Id`).
pub fn is_install_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 19 && (b'1'..=b'9').contains(&b[0]) && b.iter().all(u8::is_ascii_digit)
}

fn load() -> Result<LinkState, String> {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(raw) if raw.trim().is_empty() => Ok(LinkState::default()),
        Ok(raw) => {
            let s: LinkState = serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", p.display()))?;
            if s.format != 1 {
                return Err(format!("{} has format {}, expected 1", p.display(), s.format));
            }
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LinkState::default()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

fn save(s: &LinkState) -> Result<(), String> {
    let p = path();
    let dir = p.parent().ok_or("link path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let mut s = s.clone();
    s.format = 1;
    let body = serde_json::to_vec_pretty(&s).map_err(|e| format!("serialize link: {e}"))?;
    k2_core::fs_atomic::atomic_write_private(&p, &body).map_err(|e| format!("write {}: {e}", p.display()))
}

/// The stored link state (tests, status).
pub fn read() -> Result<LinkState, String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load()
}

/// S29 after enable/upgrade/restore: the install id Sew reports.
pub fn set_install_id(id: &str) -> Result<(), String> {
    set_install_id_at(id, super::keyring::now())
}

pub fn set_install_id_at(id: &str, now: i64) -> Result<(), String> {
    if !is_install_id(id) {
        return Err("install id must be decimal [1-9][0-9]{0,18}".into());
    }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut s = load()?;
    s.install_id = Some(id.to_string());
    s.install_id_fetched_at = Some(now);
    save(&s)
}

/// Forget the install id (Sew purged; tests).
pub fn clear_install_id() -> Result<(), String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut s = load()?;
    s.install_id = None;
    s.install_id_fetched_at = None;
    save(&s)
}

/// S29 after enable: Sew's client listen URL (loopback).
pub fn set_client_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://127.0.0.1:") || u.starts_with("http://[::1]:") || u.starts_with("http://localhost:")) {
        return Err("client url must be loopback http://127.0.0.1:<port>".into());
    }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut s = load()?;
    s.client_url = Some(u.to_string());
    save(&s)
}

pub fn client_url() -> Option<String> {
    read().ok().and_then(|s| s.client_url)
}

/// The install id for a mint at `now` (module doc).
pub fn install_id_for_mint(now: i64, status: &dyn SewStatus) -> Result<String, String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut s = load()?;
    if let (Some(id), Some(at)) = (s.install_id.as_deref(), s.install_id_fetched_at) {
        if now - at < INSTALL_ID_FRESH_SECS && is_install_id(id) {
            return Ok(id.to_string());
        }
    }
    match status.install_id() {
        Ok(id) if is_install_id(&id) => {
            s.install_id = Some(id.clone());
            s.install_id_fetched_at = Some(now);
            save(&s)?;
            Ok(id)
        }
        other => {
            let why = match other {
                Ok(bad) => format!("Sew reported a malformed install id ({} chars)", bad.len()),
                Err(e) => e,
            };
            match s.install_id.as_deref().filter(|id| is_install_id(id)) {
                Some(stale) => {
                    let mut last = LAST_STALE_LOG.lock().unwrap_or_else(|p| p.into_inner());
                    if now.saturating_sub(*last) >= STALE_LOG_EVERY_SECS {
                        *last = now;
                        super::keyring::klog(format!("Sew status unavailable ({why}); minting with the cached install id"));
                    }
                    Ok(stale.to_string())
                }
                None => Err(why),
            }
        }
    }
}
