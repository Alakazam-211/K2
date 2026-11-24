//! Change-password tickets (K2 Companion, 2026-11-17).
//!
//! A Connect login that must change its password (someone else set it: the
//! owner, an admin, provisioning) signs in with that temporary password and
//! gets a session restricted to whoami / change-password / logout. With
//! web access off, a phone has no way to get that session: a password
//! sign-in through `<label>.app.k2.dev` meets the web wall, and pairing is
//! refused for such a login. A ticket is the phone's narrow way through.
//!
//! - **Issued** by `POST /cli/auth/device/password-ticket {username,
//!   password}` only for a login that must change its password, and only
//!   after the temporary password is verified THROUGH the normal lockout
//!   gate ([`connect_users::check_and_record_must_change`]): every attempt
//!   counts toward the login's lockout, and any other name (unknown,
//!   disabled, no pending change) is answered like a wrong password.
//! - **Good for one thing**: `POST /cli/auth/change-password` for that
//!   login, presented as `Authorization: Bearer` (or `?token=`). It is not a
//!   session: every other route treats it as an unknown token. The route
//!   still checks the current password, through the same lockout gate.
//! - **Short-lived**: [`TICKET_TTL_SECS`].
//! - **Single use**: spent by a successful change; also dead once the
//!   login's `token_epoch` moves (any password write, role change, disable,
//!   removal) or its must-change flag clears. One live ticket per login (a
//!   new one replaces the old).
//! - Held in memory only, by digest: a daemon restart drops every ticket,
//!   and the raw ticket is never stored.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::connect_users::{self, LoginOutcome};

/// How long a ticket works.
pub const TICKET_TTL_SECS: i64 = 600;
/// Every ticket starts with this.
pub const TICKET_PREFIX: &str = "k2pct_";
/// Random hex characters after the prefix (256 bits).
const TICKET_HEX: usize = 64;
/// Tickets kept at once; issuing past this drops the one that ends soonest.
pub const MAX_OUTSTANDING: usize = 64;

struct Pending {
    username: String,
    epoch: u64,
    expires_at: DateTime<Utc>,
}

fn tickets() -> MutexGuard<'static, HashMap<String, Pending>> {
    static TICKETS: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    TICKETS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn digest(ticket: &str) -> String {
    Sha256::digest(ticket.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The stored key matching `key` (a digest), compared in constant time
/// against EVERY stored key: no early exit, no string `==`, no hash-map
/// probe on the presented value.
fn find_key(map: &HashMap<String, Pending>, key: &str) -> Option<String> {
    let mut hit: Option<String> = None;
    for k in map.keys() {
        if bool::from(k.as_bytes().ct_eq(key.as_bytes())) {
            hit = Some(k.clone());
        }
    }
    hit
}

/// A freshly issued ticket (shown once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    /// `k2pct_` + 64 hex.
    pub ticket: String,
    /// The login it changes the password of (normalized).
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

/// What an issue attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    Ticket(Ticket),
    /// The presented name is locked (before this attempt, or by it).
    Locked { retry_after_secs: i64 },
    /// Wrong password, or a name that can't have a ticket. One answer.
    Refused,
}

/// Verify `password` for `username` through the lockout gate and, only for
/// a login that must change its password, issue a ticket. `ip` keys a
/// Voice & Chat only login's lockout, as at sign-in.
pub fn issue(username: &str, password: &str, ip: Option<&str>) -> Issue {
    let locked = |fallback: bool| {
        connect_users::lock_retry_after_secs_from(username, ip)
            .or_else(|| fallback.then_some(connect_users::LOCKOUT_DURATION_MINUTES * 60))
            .map(|retry_after_secs| Issue::Locked { retry_after_secs })
    };
    match connect_users::check_and_record_must_change(username, password, ip) {
        LoginOutcome::Ok => {}
        LoginOutcome::LockedOut => return locked(true).unwrap_or(Issue::Refused),
        LoginOutcome::BadCreds => return locked(false).unwrap_or(Issue::Refused),
    }
    let Ok(username) = connect_users::normalize_username(username) else { return Issue::Refused };
    let Some(epoch) = connect_users::live_user_epoch(&username) else { return Issue::Refused };
    let mut bytes = [0u8; TICKET_HEX / 2];
    OsRng.fill_bytes(&mut bytes);
    let ticket = format!("{TICKET_PREFIX}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let now = Utc::now();
    let expires_at = now + Duration::seconds(TICKET_TTL_SECS);
    {
        let mut map = tickets();
        map.retain(|_, p| p.expires_at > now && p.username != username);
        while map.len() >= MAX_OUTSTANDING {
            let Some(soonest) = map.iter().min_by_key(|(_, p)| p.expires_at).map(|(k, _)| k.clone()) else { break };
            map.remove(&soonest);
        }
        map.insert(digest(&ticket), Pending { username: username.clone(), epoch, expires_at });
    }
    Issue::Ticket(Ticket { ticket, username, expires_at })
}

/// True when `token` has a ticket's shape (checked before any lookup).
pub fn looks_like_ticket(token: &str) -> bool {
    token.strip_prefix(TICKET_PREFIX).is_some_and(|hex| {
        hex.len() == TICKET_HEX && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The login a live ticket may change the password of: not expired, the
/// login's epoch unchanged since the ticket was issued, and it still must
/// change its password. `None` otherwise (and a dead ticket is dropped).
pub fn holder(token: &str) -> Option<String> {
    if !looks_like_ticket(token) {
        return None;
    }
    let presented = digest(token);
    let (key, username, epoch, expires_at) = {
        let map = tickets();
        let key = find_key(&map, &presented)?;
        let p = map.get(&key)?;
        (key, p.username.clone(), p.epoch, p.expires_at)
    };
    let live = expires_at > Utc::now()
        && connect_users::live_user_epoch(&username) == Some(epoch)
        && connect_users::must_change_password(&username);
    if !live {
        tickets().remove(&key);
        return None;
    }
    Some(username)
}

/// Spend a ticket (after the change it was for).
pub fn spend(token: &str) {
    let mut map = tickets();
    if let Some(key) = find_key(&map, &digest(token)) {
        map.remove(&key);
    }
}

/// Test seam: make a ticket expire now. `false` when no such ticket.
#[cfg(any(test, feature = "test-util"))]
pub fn expire_now_for_test(token: &str) -> bool {
    let mut map = tickets();
    let Some(key) = find_key(&map, &digest(token)) else { return false };
    match map.get_mut(&key) {
        Some(p) => {
            p.expires_at = Utc::now() - Duration::seconds(1);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    const TEMP: &str = "temporary-pw-1";

    fn issued(i: Issue) -> Ticket {
        match i {
            Issue::Ticket(t) => t,
            other => panic!("expected a ticket, got {other:?}"),
        }
    }

    #[test]
    fn only_a_login_that_must_change_gets_a_ticket_and_only_with_its_password() {
        with_temp_home(|| {
            connect_users::add_user("tmp_pt1", TEMP).expect("add");
            connect_users::add_user("plain_pt1", TEMP).expect("add");
            connect_users::set_must_change_password("tmp_pt1", true).expect("flag");

            assert_eq!(issue("tmp_pt1", "wrong-guess", None), Issue::Refused);
            assert_eq!(connect_users::lockout_failed_count("tmp_pt1"), 1, "a wrong password counts");
            let t = issued(issue("TMP_PT1", TEMP, None));
            assert_eq!(t.username, "tmp_pt1");
            assert!(looks_like_ticket(&t.ticket), "{}", t.ticket);
            assert_eq!(holder(&t.ticket).as_deref(), Some("tmp_pt1"));
            assert_eq!(connect_users::lockout_failed_count("tmp_pt1"), 0, "the right password clears the count");

            // The right password of a login with no pending change: the same
            // answer as a wrong one, and it counts like one.
            assert_eq!(issue("plain_pt1", TEMP, None), Issue::Refused);
            assert_eq!(connect_users::lockout_failed_count("plain_pt1"), 1);
            assert_eq!(issue("nobody_pt1", TEMP, None), Issue::Refused);
            assert!(holder("k2pct_not-a-ticket").is_none());
            assert!(holder(&format!("{TICKET_PREFIX}{}", "0".repeat(64))).is_none());
        });
    }

    /// Steward (0.46.18): an unknown name, a disabled login, a login with
    /// nothing to change and a wrong password each pay for exactly one
    /// argon2 verify, like the right password; the unknown-name dummy hash
    /// uses the same argon2 parameters as a stored password.
    #[test]
    fn every_ticket_attempt_pays_for_one_argon2_verify_with_the_same_parameters() {
        with_temp_home(|| {
            connect_users::add_user("tmp_pt5", TEMP).expect("add");
            connect_users::set_must_change_password("tmp_pt5", true).expect("flag");
            connect_users::add_user("plain_pt5", TEMP).expect("add");
            connect_users::add_user("off_pt5", TEMP).expect("add");
            connect_users::set_must_change_password("off_pt5", true).expect("flag");
            connect_users::set_disabled("off_pt5", true).expect("disable");
            for (what, name, pw) in [
                ("unknown name", "nobody_pt5", TEMP),
                ("not a valid name", "no such name!", TEMP),
                ("disabled login", "off_pt5", TEMP),
                ("nothing to change", "plain_pt5", TEMP),
                ("wrong password", "tmp_pt5", "wrong-guess"),
                ("right password", "tmp_pt5", TEMP),
            ] {
                let before = connect_users::verify_hash_calls_on_this_thread();
                let _ = issue(name, pw, None);
                assert_eq!(connect_users::verify_hash_calls_on_this_thread() - before, 1, "{what}: one argon2 verify");
            }
            // Same algorithm, version and cost parameters: only salt and hash differ.
            let params = |h: &str| h.split('$').take(4).collect::<Vec<_>>().join("$");
            let stored = connect_users::hash_password(TEMP).expect("hash");
            assert_eq!(params(connect_users::dummy_hash()), params(&stored));
            assert!(params(&stored).starts_with("$argon2id$v=19$m="), "{stored}");
        });
    }

    #[test]
    fn tickets_count_toward_the_lockout_and_a_locked_name_says_how_long() {
        with_temp_home(|| {
            connect_users::add_user("tmp_pt2", TEMP).expect("add");
            connect_users::set_must_change_password("tmp_pt2", true).expect("flag");
            assert_eq!(issue("tmp_pt2", "x", None), Issue::Refused);
            assert_eq!(issue("tmp_pt2", "x", None), Issue::Refused);
            let third = issue("tmp_pt2", "x", None);
            assert!(
                matches!(third, Issue::Locked { retry_after_secs } if retry_after_secs > 0 && retry_after_secs <= 900),
                "the third wrong password locks: {third:?}"
            );
            assert!(connect_users::is_locked("tmp_pt2"), "the login's normal lockout");
            assert!(matches!(issue("tmp_pt2", TEMP, None), Issue::Locked { .. }), "locked: the right password too");
            // A name with no login locks the same way (no oracle).
            for _ in 0..3 {
                issue("ghost_pt2", "x", None);
            }
            assert!(matches!(issue("ghost_pt2", "x", None), Issue::Locked { .. }));
        });
    }

    #[test]
    fn a_ticket_dies_on_expiry_on_a_new_ticket_and_once_the_password_changes() {
        with_temp_home(|| {
            connect_users::add_user("tmp_pt3", TEMP).expect("add");
            connect_users::set_must_change_password("tmp_pt3", true).expect("flag");
            let first = issued(issue("tmp_pt3", TEMP, None));
            let second = issued(issue("tmp_pt3", TEMP, None));
            assert!(holder(&first.ticket).is_none(), "a new ticket replaces the old");
            assert!(expire_now_for_test(&second.ticket));
            assert!(holder(&second.ticket).is_none(), "expired");

            let live = issued(issue("tmp_pt3", TEMP, None));
            assert_eq!(
                connect_users::change_password("tmp_pt3", TEMP, "brand-new-pw-2"),
                connect_users::ChangePasswordOutcome::Ok
            );
            assert!(holder(&live.ticket).is_none(), "the password changed");
            assert_eq!(issue("tmp_pt3", "brand-new-pw-2", None), Issue::Refused, "no pending change: no ticket");

            connect_users::add_user("tmp_pt4", TEMP).expect("add");
            connect_users::set_must_change_password("tmp_pt4", true).expect("flag");
            let t = issued(issue("tmp_pt4", TEMP, None));
            spend(&t.ticket);
            assert!(holder(&t.ticket).is_none(), "spent");
        });
    }
}
