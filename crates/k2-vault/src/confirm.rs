//! Owner confirm (§11.3, KV7, VS16).
//!
//! - Confirm tokens: 60 s, single use, bound to (action, secret, actor).
//! - Vault confirm password: argon2id (m = 64 MiB, t = 3, p = 4; never
//!   below m = 19 MiB, t = 2, p = 1), 16-byte salt, ≥ 10 characters,
//!   common-password check. Lockout on every check: 5 failures → 15
//!   minutes, doubling.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use parking_lot::Mutex;

use crate::crypto::{base62, hex, sha256};
use crate::error::{Result, VaultError};
use crate::policy::HumanProof;

pub const TOKEN_TTL: Duration = Duration::from_secs(60);
pub const MIN_PASSWORD_LEN: usize = 10;
pub const LOCKOUT_AFTER: u32 = 5;
pub const LOCKOUT_BASE: Duration = Duration::from_secs(15 * 60);

/// KDF cost. `Floor` is OWASP's minimum and is used only by tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfCost {
    Strong,
    Floor,
}

fn argon(cost: KdfCost) -> Argon2<'static> {
    let params = match cost {
        KdfCost::Strong => Params::new(64 * 1024, 3, 4, None),
        KdfCost::Floor => Params::new(19 * 1024, 2, 1, None),
    }
    .expect("argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

const COMMON: &[&str] = &[
    "password", "password1", "password12", "password123", "1234567890", "12345678910", "qwertyuiop",
    "1q2w3e4r5t", "iloveyou12", "letmein123", "welcome123", "administrator", "passw0rd123",
    "qwerty1234", "abc1234567", "aaaaaaaaaa", "0123456789", "changeme123", "trustno1234",
    "superman123", "football123", "baseball123", "dragon1234", "monkey1234", "sunshine123",
    "princess123", "k2password", "k2vault1234", "vaultpassword", "mypassword1",
];

pub fn check_password_rules(pw: &str) -> Result<()> {
    if pw.chars().count() < MIN_PASSWORD_LEN {
        return Err(VaultError::bad_request(
            "confirm_password_too_short",
            format!("use at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    let lower = pw.to_lowercase();
    if COMMON.contains(&lower.as_str()) || lower.chars().all(|c| c == lower.chars().next().unwrap_or(' ')) {
        return Err(VaultError::bad_request("confirm_password_common", "that password is too easy to guess"));
    }
    Ok(())
}

pub fn hash_password(pw: &str, cost: KdfCost) -> Result<String> {
    check_password_rules(pw)?;
    let salt = SaltString::encode_b64(&crate::crypto::random_bytes::<16>())
        .map_err(|_| VaultError::internal("salt"))?;
    argon(cost)
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| VaultError::internal("password hash failed"))
}

pub fn verify_password(pw: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        // The PHC string carries its own params; Argon2::default() reads them.
        Ok(h) => Argon2::default().verify_password(pw.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

#[derive(Debug, Default)]
struct Lockout {
    failures: u32,
    locked_until: Option<Instant>,
    lockouts: u32,
}

/// In-memory lockout for confirm checks (one counter for every caller).
#[derive(Default)]
pub struct Throttle {
    inner: Mutex<Lockout>,
}

impl Throttle {
    pub fn check(&self) -> Result<()> {
        let g = self.inner.lock();
        if let Some(t) = g.locked_until {
            if Instant::now() < t {
                let secs = t.saturating_duration_since(Instant::now()).as_secs();
                return Err(VaultError::new(
                    "confirm_locked",
                    429,
                    format!("too many wrong passwords; try again in {} minutes", secs.div_ceil(60)),
                ));
            }
        }
        Ok(())
    }

    /// Record a result. Returns true when this failure started a lockout.
    pub fn record(&self, ok: bool) -> bool {
        let mut g = self.inner.lock();
        if ok {
            g.failures = 0;
            g.locked_until = None;
            return false;
        }
        g.failures += 1;
        if g.failures >= LOCKOUT_AFTER {
            let mult = 1u32 << g.lockouts.min(8);
            g.locked_until = Some(Instant::now() + LOCKOUT_BASE * mult);
            g.lockouts += 1;
            g.failures = 0;
            return true;
        }
        false
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn clear(&self) {
        *self.inner.lock() = Lockout::default();
    }
}

struct Pending {
    action: String,
    secret: Option<String>,
    actor: String,
    method: String,
    expires: Instant,
}

/// Single-use confirm tokens.
#[derive(Default)]
pub struct Broker {
    tokens: Mutex<HashMap<String, Pending>>,
}

fn token_key(t: &str) -> String {
    hex(&sha256(t.as_bytes()))
}

impl Broker {
    /// Mint a token after a successful check.
    pub fn mint(&self, action: &str, secret: Option<&str>, actor: &str, method: &str) -> String {
        let t = format!("k2vc_{}", base62(32));
        let mut g = self.tokens.lock();
        let now = Instant::now();
        g.retain(|_, p| p.expires > now);
        g.insert(
            token_key(&t),
            Pending {
                action: action.into(),
                secret: secret.map(str::to_string),
                actor: actor.into(),
                method: method.into(),
                expires: now + TOKEN_TTL,
            },
        );
        t
    }

    /// Consume a token. Fails unless it matches (action, secret, actor)
    /// and hasn't expired. A failed take still burns the token.
    pub fn take(&self, token: &str, action: &str, secret: Option<&str>, actor: &str) -> Result<HumanProof> {
        let p = self.tokens.lock().remove(&token_key(token));
        let bad = || VaultError::forbidden("vault_confirm_required", "confirm again (it expires after 60 seconds and works once)");
        let p = p.ok_or_else(bad)?;
        if Instant::now() > p.expires || p.action != action || p.secret.as_deref() != secret || p.actor != actor {
            return Err(bad());
        }
        Ok(HumanProof::Confirmed { method: p.method })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn expire_all(&self) {
        for p in self.tokens.lock().values_mut() {
            p.expires = Instant::now() - Duration::from_secs(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_rules_and_hash() {
        assert_eq!(check_password_rules("short").expect_err("short").code, "confirm_password_too_short");
        assert_eq!(check_password_rules("password123").expect_err("common").code, "confirm_password_common");
        assert_eq!(check_password_rules("aaaaaaaaaaaa").expect_err("same").code, "confirm_password_common");
        let h = hash_password("correct horse battery", KdfCost::Floor).expect("hash");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong horse battery", &h));
        assert!(!verify_password("x", "not-a-phc"));
    }

    #[test]
    fn strong_params_are_rfc9106_second_profile() {
        let a = argon(KdfCost::Strong);
        let p = a.params();
        assert_eq!((p.m_cost(), p.t_cost(), p.p_cost()), (64 * 1024, 3, 4));
        let f = argon(KdfCost::Floor);
        let p = f.params();
        assert!(p.m_cost() >= 19 * 1024 && p.t_cost() >= 2 && p.p_cost() >= 1);
    }

    #[test]
    fn tokens_are_single_use_and_bound() {
        let b = Broker::default();
        let t = b.mint("reveal", Some("s1"), "owner", "password");
        assert!(b.take(&t, "reveal", Some("s2"), "owner").is_err(), "other secret");
        // A failed take burns it.
        assert!(b.take(&t, "reveal", Some("s1"), "owner").is_err());
        let t = b.mint("reveal", Some("s1"), "owner", "password");
        assert_eq!(b.take(&t, "reveal", Some("s1"), "owner").expect("ok"), HumanProof::Confirmed { method: "password".into() });
        assert!(b.take(&t, "reveal", Some("s1"), "owner").is_err(), "single use");
        let t = b.mint("reveal", None, "owner", "password");
        b.expire_all();
        assert!(b.take(&t, "reveal", None, "owner").is_err(), "expired");
        let t = b.mint("kek-rotate", None, "owner", "password");
        assert!(b.take(&t, "reveal", None, "owner").is_err(), "other action");
        let t = b.mint("reveal", None, "owner", "password");
        assert!(b.take(&t, "reveal", None, "connect:x").is_err(), "other actor");
    }

    #[test]
    fn lockout_after_five() {
        let t = Throttle::default();
        for i in 0..4 {
            t.check().expect("not yet");
            assert!(!t.record(false), "failure {i}");
        }
        assert!(t.record(false), "fifth failure locks");
        assert_eq!(t.check().expect_err("locked").code, "confirm_locked");
        t.clear();
        t.check().expect("cleared");
    }
}
