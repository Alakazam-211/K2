//! Owner handoff codes (S32 §11.1, S28 G6; the pass side of K26).
//!
//! The install owner has no App login, and a BFF must never hold the owner
//! token. So "Open Sew as owner" mints a single-use 60 s code, the owner's
//! browser carries it (URL fragment) to the BFF, and the BFF redeems it with
//! `POST /cli/sew/pass {"handoff":"k2sho_…"}` for a `k2:owner` pass.
//!
//! **Person-proof (Rosson 2026-10-15):** minting a code needs a vault
//! confirm (`chat-owner-handoff`), never the owner token alone; agents
//! never get install-owner. Install-owner is Sew server admin, not App
//! Space membership.
//!
//! Codes: 32 random bytes, base64url without padding → `k2sho_` + 43
//! chars. Kept in daemon memory only, as SHA-256 of the full code; at most
//! [`MAX_OUTSTANDING`] (the oldest is evicted); cleared on restart. Marked
//! used **before** the pass is signed.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use sha2::{Digest, Sha256};

pub const PREFIX: &str = "k2sho_";
pub const TTL_SECS: i64 = 60;
pub const MAX_OUTSTANDING: usize = 5;
/// The vault confirm action a person completes before a code is minted.
pub const CONFIRM_ACTION: &str = "chat-owner-handoff";

fn digest_hex(code: &str) -> String {
    crate::es256::bytes_to_hex(&Sha256::digest(code.as_bytes()))
}

/// The `own_` sid a redeemed code's pass carries (S32 §11.1).
pub fn sid_for(code: &str) -> String {
    format!("own_{}", &digest_hex(code)[..32])
}

/// First 8 hex of the digest: the audit handle (never the code).
pub fn audit_handle(code: &str) -> String {
    digest_hex(code)[..8].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemError {
    Unknown,
    Expired,
    Used,
}

impl RedeemError {
    pub fn reason(self) -> &'static str {
        match self {
            RedeemError::Unknown => "unknown",
            RedeemError::Expired => "expired",
            RedeemError::Used => "used",
        }
    }
}

struct Entry {
    expires_at: i64,
    used: bool,
    minted_at: i64,
    /// Who confirmed (actor stamp), for the redeem audit line.
    minted_by: String,
}

#[derive(Default)]
pub struct Store {
    codes: Mutex<HashMap<String, Entry>>,
}

impl Store {
    /// Mint one code at `now`. Evicts expired codes, then the oldest when
    /// [`MAX_OUTSTANDING`] are live.
    pub fn mint(&self, now: i64, minted_by: &str) -> Result<String, String> {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).map_err(|e| format!("os rng unavailable for a handoff code: {e}"))?;
        let code = format!("{PREFIX}{}", B64URL.encode(b));
        let mut g = self.codes.lock().unwrap_or_else(|p| p.into_inner());
        g.retain(|_, e| e.expires_at > now && !e.used);
        while g.len() >= MAX_OUTSTANDING {
            let oldest = g.iter().min_by_key(|(_, e)| e.minted_at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    g.remove(&k);
                }
                None => break,
            }
        }
        g.insert(
            digest_hex(&code),
            Entry { expires_at: now + TTL_SECS, used: false, minted_at: now, minted_by: minted_by.to_string() },
        );
        Ok(code)
    }

    /// Redeem `code` at `now`: marks it used before returning. Returns who
    /// minted it.
    pub fn redeem(&self, code: &str, now: i64) -> Result<String, RedeemError> {
        if !is_code_shape(code) {
            return Err(RedeemError::Unknown);
        }
        let mut g = self.codes.lock().unwrap_or_else(|p| p.into_inner());
        let e = g.get_mut(&digest_hex(code)).ok_or(RedeemError::Unknown)?;
        if e.used {
            return Err(RedeemError::Used);
        }
        if now >= e.expires_at {
            return Err(RedeemError::Expired);
        }
        e.used = true;
        Ok(e.minted_by.clone())
    }

    pub fn outstanding(&self, now: i64) -> usize {
        self.codes.lock().unwrap_or_else(|p| p.into_inner()).values().filter(|e| !e.used && e.expires_at > now).count()
    }

    pub fn clear(&self) {
        self.codes.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

/// `k2sho_` + 43 base64url characters.
pub fn is_code_shape(s: &str) -> bool {
    s.strip_prefix(PREFIX)
        .is_some_and(|b| b.len() == 43 && b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
}

/// The daemon's code store (memory only).
pub static CODES: LazyLock<Store> = LazyLock::new(Store::default);
