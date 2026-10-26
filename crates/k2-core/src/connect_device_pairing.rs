//! Companion phone pairing codes (phone-web-off, Rosson 2026-10-24).
//!
//! A pairing code is the only way to mint a Companion **device session**
//! ([`crate::connect_users::create_device_session`]), the credential that
//! lets a paired phone through the "web access off" wall.
//!
//! - **Minted** by `POST /cli/auth/device/pair-code`: the owner token for
//!   any Connect login, or a login for itself after typing its password
//!   again (security review F1/F2), and never on a request that looks like
//!   a browser (the daemon route checks all of that, and audits it).
//! - At most [`MAX_PENDING_PER_LOGIN`] outstanding per login (the oldest
//!   goes) and [`MAX_OUTSTANDING`] in all (F4). The minting actor rides on
//!   the code into the device session and the `device-pair` audit row.
//! - Refused for a login that must change its password (at mint and at
//!   redeem), so a paired session never starts restricted.
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
/// Outstanding codes per login; minting past this drops that login's
/// oldest (security review F4).
pub const MAX_PENDING_PER_LOGIN: usize = 3;
/// A device name is shown with at most this many characters.
pub const MAX_NAME_CHARS: usize = 80;
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
    /// The stored (cleaned) device name.
    pub device_name: String,
    /// Who made the code (`owner-token` / `user:<name>`).
    pub minted_by: String,
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
    /// The login must change its password first (answered 409, not 401).
    PasswordChangeRequired,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::Unknown => "unknown",
            Refused::Expired => "expired",
            Refused::LoginChanged => "login_changed",
            Refused::PasswordChangeRequired => "password_change_required",
        }
    }
}

struct Pending {
    username: String,
    minted_by: String,
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

/// A device name safe to store and show: control, format and bidi
/// characters dropped, whitespace runs folded to one space, trimmed, at
/// most [`MAX_NAME_CHARS`]; empty becomes `Phone` (security review F7).
/// Display surfaces still escape it for their medium.
pub fn clean_device_name(raw: &str) -> String {
    clean_name(raw, MAX_NAME_CHARS, "Phone")
}

/// [`clean_device_name`]'s rules with the caller's cap and empty default
/// (an App's device sign-in names default to `Device`, app pairing V12).
pub fn clean_name(raw: &str, max_chars: usize, empty: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in raw.chars() {
        let invisible = c.is_control()
            || matches!(
                c as u32,
                0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB
            );
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if invisible {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    let cut: String = out.chars().take(max_chars).collect();
    let cut = cut.trim().to_string();
    if cut.is_empty() {
        empty.to_string()
    } else {
        cut
    }
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

/// Why a mint did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// No such login, or it is disabled.
    NoSuchLogin(String),
    /// The login must change its password first.
    PasswordChangeRequired,
}

/// Mint a code for an existing, enabled Connect user that need not change
/// its password. `minted_by` is the acting caller (`owner-token` /
/// `user:<name>`). Authorisation (who may mint for whom, the password
/// re-check, not from a browser, throttles) is the route's job.
pub fn mint(username: &str, minted_by: &str) -> Result<PairCode, MintError> {
    let username = connect_users::normalize_username(username).map_err(MintError::NoSuchLogin)?;
    let Some(epoch) = connect_users::live_user_epoch(&username) else {
        return Err(MintError::NoSuchLogin(format!("no enabled login named {username:?}")));
    };
    if connect_users::must_change_password(&username) {
        return Err(MintError::PasswordChangeRequired);
    }
    let body = random_body();
    let now = Utc::now();
    let expires_at = now + Duration::seconds(CODE_TTL_SECS);
    let mut map = codes();
    map.retain(|_, p| p.expires_at > now);
    loop {
        let mine: Vec<(String, DateTime<Utc>)> =
            map.iter().filter(|(_, p)| p.username == username).map(|(k, p)| (k.clone(), p.created_at)).collect();
        if mine.len() < MAX_PENDING_PER_LOGIN {
            break;
        }
        let Some((oldest, _)) = mine.into_iter().min_by_key(|(_, t)| *t) else { break };
        map.remove(&oldest);
    }
    while map.len() >= MAX_OUTSTANDING {
        let Some(oldest) = map.iter().min_by_key(|(_, p)| p.created_at).map(|(k, _)| k.clone()) else { break };
        map.remove(&oldest);
    }
    map.insert(
        digest(&body),
        Pending { username: username.clone(), minted_by: minted_by.to_string(), epoch, created_at: now, expires_at },
    );
    Ok(PairCode { code: display(&body), username, expires_at })
}

/// What a still-valid code would pair as (`POST /cli/auth/device/pair/check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub username: String,
    /// Seconds left before the code expires (at least 1).
    pub expires_in_secs: i64,
}

/// Look at `raw` WITHOUT consuming it (Companion's pair screen confirms the
/// login before pairing). Refuses exactly what a redeem would refuse at
/// this moment: unknown/used, expired, the login changed, or the login must
/// change its password. Never mints anything. The route throttles it like
/// a redeem, so it is no cheaper a guessing oracle.
pub fn check(raw: &str) -> Result<Checked, Refused> {
    let Some(body) = normalize(raw) else { return Err(Refused::Unknown) };
    let (username, epoch, expires_at) = {
        let map = codes();
        let Some(p) = map.get(&digest(&body)) else { return Err(Refused::Unknown) };
        (p.username.clone(), p.epoch, p.expires_at)
    };
    let left = (expires_at - Utc::now()).num_seconds();
    if left <= 0 {
        return Err(Refused::Expired);
    }
    if connect_users::live_user_epoch(&username) != Some(epoch) {
        return Err(Refused::LoginChanged);
    }
    if connect_users::must_change_password(&username) {
        return Err(Refused::PasswordChangeRequired);
    }
    Ok(Checked { username, expires_in_secs: left })
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
    if connect_users::must_change_password(&p.username) {
        return Ok(Err(Refused::PasswordChangeRequired));
    }
    let name = clean_device_name(device_name);
    let token = connect_users::create_device_session(&p.username, p.epoch, device_id, &name, &p.minted_by)?;
    let Some((username, session_id)) = connect_users::validate_device_session(&token) else {
        return Err("the new device session did not validate".to_string());
    };
    Ok(Ok(Paired { token, username, session_id, device_name: name, minted_by: p.minted_by }))
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
    fn device_names_lose_invisible_characters_and_are_capped() {
        assert_eq!(clean_device_name("  Rosson's\u{202E}  iPhone\n "), "Rosson's iPhone");
        assert_eq!(clean_device_name("\u{200B}\u{0007}"), "Phone");
        assert_eq!(clean_device_name(&"x".repeat(200)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(clean_device_name("<b>"), "<b>", "kept as text; display escapes it");
    }

    #[test]
    fn pending_codes_are_capped_per_login_and_carry_the_minter() {
        with_temp_home(|| {
            connect_users::add_user("capped", "password123").expect("add");
            let minted: Vec<PairCode> =
                (0..MAX_PENDING_PER_LOGIN + 1).map(|_| mint("capped", "user:capped").expect("mint")).collect();
            assert_eq!(
                redeem(&minted[0].code, "d0", "D").expect("store").err(),
                Some(Refused::Unknown),
                "the oldest code went when a 4th was made"
            );
            let last = minted.last().expect("last");
            let paired = redeem(&last.code, "d1", "D").expect("store").expect("pairs");
            assert_eq!(paired.minted_by, "user:capped");
            let list = connect_users::device_sessions(Some("capped")).expect("list");
            assert_eq!(list[0].paired_by.as_deref(), Some("user:capped"));
        });
    }

    #[test]
    fn a_login_that_must_change_its_password_gets_no_code_and_no_redeem() {
        with_temp_home(|| {
            connect_users::add_user("rotate_first", "password123").expect("add");
            let code = mint("rotate_first", "owner-token").expect("mint");
            connect_users::set_must_change_password("rotate_first", true).expect("flag");
            assert_eq!(mint("rotate_first", "owner-token").err(), Some(MintError::PasswordChangeRequired));
            assert_eq!(redeem(&code.code, "d", "D").expect("store").err(), Some(Refused::PasswordChangeRequired));
        });
    }

    #[test]
    fn a_check_never_consumes_the_code() {
        with_temp_home(|| {
            connect_users::add_user("checker", "password123").expect("add");
            let code = mint("checker", "owner-token").expect("mint").code;
            for _ in 0..3 {
                let c = check(&code).expect("valid");
                assert_eq!(c.username, "checker");
                assert!(c.expires_in_secs > 0 && c.expires_in_secs <= CODE_TTL_SECS, "{c:?}");
            }
            assert!(redeem(&code, "chk-1", "C").expect("store").is_ok(), "still redeemable after checks");
            assert_eq!(check(&code), Err(Refused::Unknown), "used");
            assert_eq!(check("K2P-garbage"), Err(Refused::Unknown));
        });
    }

    #[test]
    fn two_redeems_of_one_code_at_once_pair_exactly_once() {
        with_temp_home(|| {
            connect_users::add_user("racer", "password123").expect("add");
            let code = mint("racer", "owner-token").expect("mint").code;
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|i| {
                    let c = code.clone();
                    let b = barrier.clone();
                    std::thread::spawn(move || {
                        b.wait();
                        redeem(&c, &format!("race{i}"), "R").expect("store").is_ok()
                    })
                })
                .collect();
            let wins = handles.into_iter().map(|h| h.join().expect("join")).filter(|ok| *ok).count();
            assert_eq!(wins, 1);
        });
    }

    #[test]
    fn ended_device_sessions_are_told_apart_and_the_lifetime_is_capped() {
        with_temp_home(|| {
            connect_users::add_user("lapsing", "password123").expect("add");
            let paired =
                redeem(&mint("lapsing", "owner-token").expect("mint").code, "p1", "P").expect("store").expect("pairs");
            assert_eq!(connect_users::token_state(&paired.token), connect_users::TokenState::Live);
            let plain = connect_users::create_session("lapsing");
            let list = connect_users::device_sessions(Some("lapsing")).expect("list");
            let created = chrono::DateTime::parse_from_rfc3339(&list[0].created_at).expect("ts");
            let max = chrono::DateTime::parse_from_rfc3339(&list[0].max_expires_at).expect("ts");
            assert_eq!(max - created, Duration::days(connect_users::DEVICE_SESSION_MAX_DAYS));
            connect_users::revoke_device_session(&list[0].id, None).expect("revoke");
            assert_eq!(connect_users::token_state(&paired.token), connect_users::TokenState::DeviceEnded);
            assert_eq!(connect_users::token_state(&plain), connect_users::TokenState::Live);
            connect_users::revoke_user_sessions("lapsing");
            assert_eq!(
                connect_users::token_state(&plain),
                connect_users::TokenState::Unknown,
                "a password login's ended session is not a device"
            );
            assert_eq!(connect_users::token_state("garbage"), connect_users::TokenState::Unknown);
            assert_eq!(connect_users::token_state(&"0".repeat(64)), connect_users::TokenState::Unknown);
        });
    }

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
            let code = mint("pairer", "owner-token").expect("mint");
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
            let paired_code = mint("rotator", "owner-token").expect("mint");
            let paired = redeem(&paired_code.code, "a1", "A").expect("store").expect("pairs");
            let pending = mint("rotator", "owner-token").expect("mint");
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
            assert!(mint("nobody", "owner-token").is_err(), "unknown user");
            connect_users::add_user("sleepy", "password123").expect("add");
            connect_users::set_disabled("sleepy", true).expect("disable");
            assert!(mint("sleepy", "owner-token").is_err(), "disabled user");
            connect_users::set_disabled("sleepy", false).expect("enable");
            let code = mint("sleepy", "owner-token").expect("mint");
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
            let first = redeem(&mint("many", "owner-token").expect("mint").code, "same", "S").expect("store").expect("pairs");
            let second = redeem(&mint("many", "owner-token").expect("mint").code, "same", "S").expect("store").expect("pairs");
            assert_eq!(connect_users::validate_device_session(&first.token), None, "replaced");
            assert!(connect_users::validate_device_session(&second.token).is_some());
            for i in 0..connect_users::MAX_DEVICE_SESSIONS_PER_USER {
                redeem(&mint("many", "owner-token").expect("mint").code, &format!("dev{i}"), "D")
                    .expect("store")
                    .expect("pairs");
            }
            let live = connect_users::device_sessions(Some("many")).expect("list");
            assert_eq!(live.len(), connect_users::MAX_DEVICE_SESSIONS_PER_USER);
        });
    }
}
