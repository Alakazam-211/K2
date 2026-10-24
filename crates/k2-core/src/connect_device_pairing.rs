//! Companion phone pairing codes (phone-web-off, Rosson 2026-10-24).
//!
//! A pairing code is the only way to mint a Companion **device session**
//! ([`crate::connect_users::create_device_session`]), the credential that
//! lets a paired phone through the "web access off" wall.
//!
//! - **Minted** by `POST /cli/auth/device/pair-code`: the owner token for a
//!   named Connect user, or a Connect login for itself, and never on a
//!   request that looks like a browser (the daemon route checks that).
//! - **Redeemed** once by `POST /cli/auth/device/pair {code, device}` from
//!   the phone. The route is public (the code is the credential) and stays
//!   open while web access is off, because the phone's first contact may
//!   come through the edge.
//! - One-time: a redeem attempt consumes the code whatever the outcome.
//! - Short-lived: [`CODE_TTL_SECS`].
//! - Bound to the person's `token_epoch` at mint: a password change, role
//!   change, disable or removal before the redeem voids it.
//! - 100 random bits (20 Crockford base32 characters), so guessing one
//!   inside its ten minutes is not a practical attack and no throttle is
//!   needed.
//! - Held in memory only, by digest: a daemon restart drops every
//!   outstanding code (they are minutes long), and the raw code is never
//!   stored.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use crate::connect_users;

/// How long a pairing code works.
pub const CODE_TTL_SECS: i64 = 600;
/// Outstanding codes kept at once; minting past this drops the oldest.
pub const MAX_OUTSTANDING: usize = 64;
/// Random characters in a code (5 bits each).
pub const CODE_CHARS: usize = 20;
/// Human prefix on the shown code. Optional when typed.
pub const CODE_PREFIX: &str = "K2P";

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A freshly minted code (shown once).
#[derive(Debug, Clone)]
pub struct PairCode {
    /// `K2P-XXXXX-XXXXX-XXXXX-XXXXX`.
    pub code: String,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

/// A successful redeem: the device session token (shown once).
#[derive(Debug, Clone)]
pub struct Paired {
    pub token: String,
    pub username: String,
    pub session_id: String,
}

/// Why a redeem did not pair. One wire answer for all of them (no oracle);
/// the reason is for the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Unknown, already used, or malformed.
    Unknown,
    Expired,
    /// The person was disabled/removed or their epoch moved since the mint.
    LoginChanged,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::Unknown => "unknown",
            Refused::Expired => "expired",
            Refused::LoginChanged => "login_changed",
        }
    }
}

struct Pending {
    username: String,
    epoch: u64,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

fn codes() -> std::sync::MutexGuard<'static, HashMap<String, Pending>> {
    static CODES: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    CODES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn digest(normalized: &str) -> String {
    let out = Sha256::digest(normalized.as_bytes());
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// The canonical form of a typed code: upper case, separators and the
/// optional `K2P` prefix dropped, Crockford look-alikes folded
/// (`I`/`L` → `1`, `O` → `0`). `None` unless exactly [`CODE_CHARS`]
/// alphabet characters remain.
pub fn normalize(raw: &str) -> Option<String> {
    let up: String = raw
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '_'))
        .collect::<String>()
        .to_ascii_uppercase();
    // Only a prefix on a full-length code is the prefix (a body may itself
    // start with `K2P`).
    let body = match up.strip_prefix(CODE_PREFIX) {
        Some(rest) if rest.len() == CODE_CHARS => rest,
        _ => up.as_str(),
    };
    let folded: String = body
        .chars()
        .map(|c| match c {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect();
    (folded.len() == CODE_CHARS && folded.bytes().all(|b| ALPHABET.contains(&b))).then_some(folded)
}

fn random_body() -> String {
    let mut bytes = [0u8; CODE_CHARS];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect()
}

fn display(body: &str) -> String {
    let groups: Vec<&str> = (0..body.len()).step_by(5).map(|i| &body[i..(i + 5).min(body.len())]).collect();
    format!("{CODE_PREFIX}-{}", groups.join("-"))
}

/// Mint a code for an existing, enabled Connect user. Authorisation (who
/// may mint for whom, and not from a browser) is the route's job.
pub fn mint(username: &str) -> Result<PairCode, String> {
    let username = connect_users::normalize_username(username)?;
    let Some(epoch) = connect_users::live_user_epoch(&username) else {
        return Err(format!("no enabled login named {username:?}"));
    };
    let body = random_body();
    let now = Utc::now();
    let expires_at = now + Duration::seconds(CODE_TTL_SECS);
    let mut map = codes();
    map.retain(|_, p| p.expires_at > now);
    while map.len() >= MAX_OUTSTANDING {
        let Some(oldest) = map.iter().min_by_key(|(_, p)| p.created_at).map(|(k, _)| k.clone()) else { break };
        map.remove(&oldest);
    }
    map.insert(digest(&body), Pending { username: username.clone(), epoch, created_at: now, expires_at });
    Ok(PairCode { code: display(&body), username, expires_at })
}

/// Redeem `raw` for a device session bound to `device_id` / `device_name`
/// (already validated). The code is consumed by the attempt, paired or
/// not. `Err` only for a session-store failure.
pub fn redeem(raw: &str, device_id: &str, device_name: &str) -> Result<Result<Paired, Refused>, String> {
    let Some(body) = normalize(raw) else { return Ok(Err(Refused::Unknown)) };
    let pending = {
        let mut map = codes();
        map.remove(&digest(&body))
    };
    let Some(p) = pending else { return Ok(Err(Refused::Unknown)) };
    if p.expires_at <= Utc::now() {
        return Ok(Err(Refused::Expired));
    }
    if connect_users::live_user_epoch(&p.username) != Some(p.epoch) {
        return Ok(Err(Refused::LoginChanged));
    }
    let token = connect_users::create_device_session(&p.username, p.epoch, device_id, device_name)?;
    let Some((username, session_id)) = connect_users::validate_device_session(&token) else {
        return Err("the new device session did not validate".to_string());
    };
    Ok(Ok(Paired { token, username, session_id }))
}

/// Test seam: how many codes are outstanding (expired ones included until
/// the next mint sweeps them).
#[cfg(any(test, feature = "test-util"))]
pub fn outstanding() -> usize {
    codes().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn normalize_folds_prefix_case_separators_and_lookalikes() {
        let body = "0123456789ABCDEFGHJK";
        assert_eq!(normalize(body).as_deref(), Some(body));
        assert_eq!(normalize("k2p-01234-56789-abcde-fghjk").as_deref(), Some(body));
        assert_eq!(normalize("K2P 0i234 56789 ABCDE FGHJK").as_deref(), Some(body), "I folds to 1");
        assert_eq!(normalize("o1234-56789-ABCDE-FGHJK").as_deref(), Some(body), "O folds to 0");
        assert_eq!(normalize("0123456789ABCDEFGHJ"), None, "too short");
        assert_eq!(normalize("0123456789ABCDEFGHJKM"), None, "too long");
        assert_eq!(normalize("0123456789ABCDEFGHJU"), None, "U is not in the alphabet");
        let starts_like_prefix = "K2PAB56789ABCDEFGHJK";
        assert_eq!(normalize(starts_like_prefix).as_deref(), Some(starts_like_prefix), "typed without the prefix");
        assert_eq!(normalize(&display(starts_like_prefix)).as_deref(), Some(starts_like_prefix));
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn a_code_pairs_once_and_the_session_is_a_device_session() {
        with_temp_home(|| {
            connect_users::add_user("pairer", "password123").expect("add");
            let code = mint("pairer").expect("mint");
            assert!(code.code.starts_with("K2P-"), "{}", code.code);
            assert_eq!(code.username, "pairer");
            let paired = redeem(&code.code, "ios-install-1", "Rosson's iPhone")
                .expect("store")
                .expect("pairs");
            assert_eq!(paired.username, "pairer");
            let (user, sid) =
                connect_users::validate_device_session(&paired.token).expect("device session validates");
            assert_eq!(user, "pairer");
            assert_eq!(sid, paired.session_id);
            assert_eq!(connect_users::validate_session(&paired.token).as_deref(), Some("pairer"));
            // One-time.
            let again = redeem(&code.code, "ios-install-2", "Other").expect("store");
            assert_eq!(again.err(), Some(Refused::Unknown));
            // A password login's session is never a device session.
            let plain = connect_users::create_session("pairer");
            assert_eq!(connect_users::validate_device_session(&plain), None);
            // Listed, then revoked by its owner only.
            let list = connect_users::device_sessions(Some("pairer")).expect("list");
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].device_id, "ios-install-1");
            assert_eq!(list[0].name, "Rosson's iPhone");
            connect_users::add_user("other", "password123").expect("add other");
            assert_eq!(
                connect_users::revoke_device_session(&list[0].id, Some("other")).expect("revoke"),
                None,
                "someone else cannot end it"
            );
            assert_eq!(
                connect_users::revoke_device_session(&list[0].id, Some("pairer")).expect("revoke").as_deref(),
                Some("pairer")
            );
            assert_eq!(connect_users::validate_device_session(&paired.token), None);
            assert_eq!(connect_users::validate_session(&paired.token), None);
        });
    }

    #[test]
    fn a_password_change_between_mint_and_redeem_voids_the_code_and_ends_device_sessions() {
        with_temp_home(|| {
            connect_users::add_user("rotator", "password123").expect("add");
            let paired_code = mint("rotator").expect("mint");
            let paired = redeem(&paired_code.code, "a1", "A").expect("store").expect("pairs");
            let pending = mint("rotator").expect("mint");
            connect_users::set_password("rotator", "another-pass-456").expect("set_password");
            let r = redeem(&pending.code, "a2", "B").expect("store");
            assert_eq!(r.err(), Some(Refused::LoginChanged));
            assert_eq!(
                connect_users::validate_device_session(&paired.token),
                None,
                "a password change ends paired phones like every session"
            );
        });
    }

    #[test]
    fn a_disabled_or_unknown_user_cannot_mint_and_an_expired_code_is_refused() {
        with_temp_home(|| {
            assert!(mint("nobody").is_err(), "unknown user");
            connect_users::add_user("sleepy", "password123").expect("add");
            connect_users::set_disabled("sleepy", true).expect("disable");
            assert!(mint("sleepy").is_err(), "disabled user");
            connect_users::set_disabled("sleepy", false).expect("enable");
            let code = mint("sleepy").expect("mint");
            let body = normalize(&code.code).expect("normalizes");
            codes().get_mut(&digest(&body)).expect("pending").expires_at = Utc::now() - Duration::seconds(1);
            assert_eq!(redeem(&code.code, "d", "D").expect("store").err(), Some(Refused::Expired));
            assert_eq!(redeem("K2P-garbage", "d", "D").expect("store").err(), Some(Refused::Unknown));
        });
    }

    #[test]
    fn repairing_the_same_install_replaces_its_session_and_the_cap_holds() {
        with_temp_home(|| {
            connect_users::add_user("many", "password123").expect("add");
            let first = redeem(&mint("many").expect("mint").code, "same", "S").expect("store").expect("pairs");
            let second = redeem(&mint("many").expect("mint").code, "same", "S").expect("store").expect("pairs");
            assert_eq!(connect_users::validate_device_session(&first.token), None, "replaced");
            assert!(connect_users::validate_device_session(&second.token).is_some());
            for i in 0..connect_users::MAX_DEVICE_SESSIONS_PER_USER {
                redeem(&mint("many").expect("mint").code, &format!("dev{i}"), "D")
                    .expect("store")
                    .expect("pairs");
            }
            let live = connect_users::device_sessions(Some("many")).expect("list");
            assert_eq!(live.len(), connect_users::MAX_DEVICE_SESSIONS_PER_USER);
        });
    }
}
