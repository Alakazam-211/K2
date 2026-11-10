//! Single-use stream tickets (vs-live R20).
//!
//! A session token in a URL ends up in relay and proxy logs, and the media
//! lane is a different host the web client's cookie doesn't reach. So a
//! viewer asks `POST /cli/desktop/ticket` (a login, in its own request)
//! and opens `/cli/desktop/stream?ticket=…`. A ticket lasts [`TTL`], works
//! once, and is bound to the login and its session; the stream re-checks
//! the bound session, never the ticket. Only the ticket's SHA-256 is kept,
//! in memory (a daemon restart ends every unspent ticket); the bound
//! session token stays in this process's memory and is never logged
//! (its `Debug` is redacted).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

pub const PREFIX: &str = "k2dt_";
pub const TTL: Duration = Duration::from_secs(60);
const BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Most unspent tickets kept (the oldest go first).
const MAX_LIVE: usize = 1024;

/// What a ticket was minted for.
#[derive(Clone, PartialEq, Eq)]
pub struct Claim {
    pub desktop: String,
    pub username: String,
    /// The login's session token: the stream re-checks that session every
    /// 5 s (`login_still_admitted`), so a sign-out or revoke ends it.
    pub session_token: String,
    pub display: Option<String>,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("desktop", &self.desktop)
            .field("username", &self.username)
            .field("session_token", &"<redacted>")
            .field("display", &self.display)
            .finish()
    }
}

struct Live {
    claim: Claim,
    expires: Instant,
}

fn store() -> &'static Mutex<HashMap<String, Live>> {
    static S: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn digest(t: &str) -> String {
    let d = Sha256::digest(t.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn fresh() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut out = String::with_capacity(PREFIX.len() + BODY_LEN);
    out.push_str(PREFIX);
    let mut n = 0;
    let mut scratch = [0u8; 64];
    while n < BODY_LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in &scratch {
            if b >= 248 {
                continue;
            }
            out.push(BASE62[(b % 62) as usize] as char);
            n += 1;
            if n == BODY_LEN {
                break;
            }
        }
    }
    out
}

pub fn is_ticket(s: &str) -> bool {
    s.len() == PREFIX.len() + BODY_LEN && s.starts_with(PREFIX)
}

/// Mint a ticket at `now`.
pub fn mint_at(claim: Claim, now: Instant) -> String {
    let t = fresh();
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    s.retain(|_, l| l.expires > now);
    if s.len() >= MAX_LIVE {
        if let Some(oldest) = s.iter().min_by_key(|(_, l)| l.expires).map(|(k, _)| k.clone()) {
            s.remove(&oldest);
        }
    }
    s.insert(digest(&t), Live { claim, expires: now + TTL });
    t
}

pub fn mint(claim: Claim) -> String {
    mint_at(claim, Instant::now())
}

/// Spend a ticket at `now`: its claim once, then never again. An expired
/// or unknown ticket is `None`.
pub fn redeem_at(ticket: &str, now: Instant) -> Option<Claim> {
    if !is_ticket(ticket) {
        return None;
    }
    let live = store().lock().unwrap_or_else(|p| p.into_inner()).remove(&digest(ticket))?;
    (now < live.expires).then_some(live.claim)
}

pub fn redeem(ticket: &str) -> Option<Claim> {
    redeem_at(ticket, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> Claim {
        Claim { desktop: "console".into(), username: "rosson".into(), session_token: "sess-token".into(), display: Some("1".into()) }
    }

    #[test]
    fn a_ticket_works_once() {
        let t = mint(claim());
        assert!(is_ticket(&t), "{t}");
        assert_eq!(redeem(&t), Some(claim()));
        assert_eq!(redeem(&t), None, "reused");
    }

    #[test]
    fn an_expired_ticket_is_refused_and_spent() {
        let t0 = Instant::now();
        let t = mint_at(claim(), t0);
        assert_eq!(redeem_at(&t, t0 + TTL), None, "expired at 60 s");
        assert_eq!(redeem_at(&t, t0), None, "and gone");
        let t = mint_at(claim(), t0);
        assert_eq!(redeem_at(&t, t0 + TTL - Duration::from_millis(1)), Some(claim()));
    }

    #[test]
    fn the_session_token_never_prints() {
        assert!(!format!("{:?}", claim()).contains("sess-token"));
    }

    #[test]
    fn junk_and_lookalikes_are_refused() {
        assert_eq!(redeem("k2dt_short"), None);
        assert_eq!(redeem(""), None);
        let t = mint(claim());
        let mut forged = t.clone();
        forged.pop();
        forged.push(if t.ends_with('A') { 'B' } else { 'A' });
        assert_eq!(redeem(&forged), None);
        assert_eq!(redeem(&t), Some(claim()), "the real one still works");
        assert_ne!(mint(claim()), mint(claim()), "random");
    }
}
