//! Sew link state (S28 §11, K7): the install id a pass's `aud` names, and
//! the client URL the `--skin` gateway's Sew mode proxies to.
//!
//! The install id is read LIVE, on every mint, from the supervisor's
//! `sew_server` row ([`SewRow`]): the same row `k2 hostchat status`
//! reports as `installId`, that the enable machine's link step writes from
//! Sew's mgmt `status`, and that purge deletes. There is no second copy.
//! (0.46.1 box test: the mint used to read a cache in `sew-link.json` that
//! nothing in production ever filled, so every mint after a real
//! `hostchat enable` answered 503, restart or not.)
//!
//! [`SewRow`] refuses (→ 503 `chat_unavailable`) when there is no row (not
//! installed, or purged), the install was uninstalled or disabled, it runs
//! Sew's own logins (standalone), or enable has not reached its link step.
//!
//! `sew-link.json` (0600; nothing secret) keeps only the client URL. Not in
//! the vault: it survives a vault restore, and the chat-pass keys stay the
//! only thing under `engine:chat-pass`. Not `~/.k2/chat-pass/` either: K6
//! moved the keys into the vault and that directory must not come back.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Where a mint gets the install id (tests inject fixed answers).
pub trait SewStatus: Send + Sync {
    fn install_id(&self) -> Result<String, String>;
    /// Sew 0.3.1 readiness before a mint ([`crate::sew::health::mint_gate`]):
    /// `Err(details)` = refuse with 503 `sew_unavailable`. Test doubles keep
    /// the default (ready).
    fn ready_gate(&self) -> Result<(), serde_json::Value> {
        Ok(())
    }
}

/// Production: the supervisor's `sew_server` row, read on every call.
pub struct SewRow;

impl SewStatus for SewRow {
    fn install_id(&self) -> Result<String, String> {
        install_id_from_row(crate::sew::store::load().as_ref())
    }
    fn ready_gate(&self) -> Result<(), serde_json::Value> {
        if !crate::sew::health::GATE_PASS {
            return Ok(());
        }
        // Unit tests run the production row source with real ops but no Sew
        // listening: the gate itself is tested through `sew::health` and the
        // `NotReady` double in `mint_tests`.
        if cfg!(test) {
            return Ok(());
        }
        crate::sew::health::mint_gate(&crate::sew::supervisor::live())
    }
}

/// The install id a row admits a pass for (module doc).
pub fn install_id_from_row(row: Option<&crate::sew::store::Row>) -> Result<String, String> {
    let row = row.ok_or("Chat / Voice Hosting is not installed on this box (k2 hostchat enable)")?;
    match row.status.as_str() {
        "removed" => return Err("Chat / Voice Hosting was uninstalled (k2 hostchat enable brings it back)".into()),
        "disabled" => return Err("Chat / Voice Hosting is disabled (k2 hostchat enable)".into()),
        _ => {}
    }
    let provider = crate::sew::supervisor::row_provider(row);
    if provider != k2_root_proto::sew::PROVIDER_K2 {
        return Err(format!(
            "Sew runs its own logins on this box (identity {provider}); K2 chat passes are for --identity k2"
        ));
    }
    match row.install_id.as_deref() {
        Some(id) if is_install_id(id) => Ok(id.to_string()),
        Some(_) => Err("the recorded Sew install id is malformed".into()),
        None => Err("Sew has no install id yet (enable has not finished: k2 hostchat status)".into()),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkState {
    pub format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_url: Option<String>,
}

pub fn path() -> PathBuf {
    k2_core::paths::k2_home().join("sew-link.json")
}

static LOCK: Mutex<()> = Mutex::new(());

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

/// The install id for a mint, asked of `status` every time (module doc).
pub fn install_id_for_mint(status: &dyn SewStatus) -> Result<String, String> {
    match status.install_id()? {
        id if is_install_id(&id) => Ok(id),
        bad => Err(format!("Sew reported a malformed install id ({} chars)", bad.len())),
    }
}
