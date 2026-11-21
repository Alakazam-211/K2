//! A Connect login hears when its own access changes, on `/cli/sessions/events`,
//! `/cli/voice/events` and `/cli/home-chat/events`.
//!
//! An App pass hears it on `me.events` (`access_live`), which refuses every
//! credential but an App pass. A person's login (the desktop, or a phone paired
//! as that login) holds `/cli/sessions/events` instead, and a Voice & Chat only
//! login `/cli/voice/events` or only `/cli/home-chat/events`, so it gets the
//! same frame there: `{"kind":"access_changed"}`, nothing else (on the Home
//! chat socket `{"type":"access_changed"}`, that socket's own envelope). The client reads its access
//! again (`GET /cli/auth/whoami`, and in a call its recording controls).
//!
//! **Whose.** Only the sockets of the login whose access changed
//! ([`crate::session_events::SessionEvent::login`]): never the owner token's,
//! never another login's, not even a login of the same person. The owner
//! token's access never changes, so it gets none. The door admits only the
//! owner token and live logins, so an agent passport or an API key never holds
//! this socket.
//!
//! **What counts.** What the login store and skin.db say this login may do: its
//! role, "Can clone", its Voice & Chat only shares, a forced password change,
//! whether the owner linked it, and its chat grant (none, on, off). A change
//! that ends the login's sessions (a removal, a disable, most role changes)
//! closes its sockets instead (4003); no frame is owed then.
//!
//! **What it can't see.** Permissions Sew keeps itself, such as a moderator
//! granting RECORD_CALLS in a room: K2 doesn't hold them. The person's own Sew
//! feed (`voice::kit_feed`) forwards Sew's `perms_changed` for those.
//!
//! **How.** Each login with an open socket is one entry, read when its first
//! socket opens (before the `hello`, so a change after the client's first read
//! moves the print). One thread watches two counters, the login store's
//! (`connect_users::auth_generation`) and skin.db's access writes
//! (`skin::write_generation`): an atomic load each, every [`POLL`]. When either
//! moved, every entry is read again and its print compared; a different print
//! sends `access_changed` to that login. A sweep every [`SWEEP`] compares them
//! whatever moved: the owner link lives in the app settings, and the other
//! daemon's writes during a handoff move neither counter.
//!
//! Off with the access hub (`K2_ACCESS_HUB=off`), like `me.events`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::session_events::{self, SessionEvent};

/// How often the thread looks at the two counters.
const POLL: Duration = Duration::from_millis(200);
/// How often every entry is compared whatever moved.
const SWEEP: Duration = Duration::from_secs(30);

/// Not read yet (the store couldn't be read): the next compare only records.
const UNREAD: u64 = 0;
/// Gone or disabled: its sockets close on their own check.
const GONE: u64 = 1;

struct Entry {
    print: u64,
    holders: usize,
}

static ENTRIES: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);
static THREAD: OnceLock<()> = OnceLock::new();

fn with_entries<T>(f: impl FnOnce(&mut HashMap<String, Entry>) -> T) -> T {
    let mut guard = ENTRIES.lock();
    f(guard.get_or_insert_with(HashMap::new))
}

/// One socket's hold on its login's entry. Dropping the last one forgets the
/// login.
#[derive(Debug)]
pub struct Held(String);

impl Drop for Held {
    fn drop(&mut self) {
        with_entries(|m| {
            if let Some(e) = m.get_mut(&self.0) {
                e.holders = e.holders.saturating_sub(1);
                if e.holders == 0 {
                    m.remove(&self.0);
                }
            }
        });
    }
}

/// Hold `login` while its socket is open. Reads the login store and skin.db
/// the first time: call it off the async workers. `None` with the hub off.
pub fn hold(login: &str) -> Option<Held> {
    if !crate::access_live::enabled() {
        return None;
    }
    start_once();
    let known = with_entries(|m| match m.get_mut(login) {
        Some(e) => {
            e.holders += 1;
            true
        }
        None => false,
    });
    if !known {
        let print = print_or_unread(login);
        with_entries(|m| {
            // Another socket of this login may have read it meanwhile: keep its print.
            m.entry(login.to_string()).or_insert(Entry { print, holders: 0 }).holders += 1;
        });
    }
    Some(Held(login.to_string()))
}

fn start_once() {
    THREAD.get_or_init(|| {
        std::thread::Builder::new().name("k2-login-access".into()).spawn(run_guarded).expect("spawn the login access watch");
    });
}

/// A panic is logged and the loop starts again: a dead thread would leave every
/// socket silently stale.
fn run_guarded() {
    loop {
        if std::panic::catch_unwind(run).is_err() {
            k2_core::log_debug!("[login_access_live] the watch panicked; restarting it");
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn counters() -> (u64, u64) {
    (k2_core::connect_users::auth_generation(), k2_core::skin::write_generation())
}

fn run() {
    let mut seen = counters();
    let mut last_sweep = Instant::now();
    loop {
        std::thread::sleep(POLL);
        // Read before the compare: a write during it moves them again.
        let now = counters();
        let sweep = last_sweep.elapsed() >= SWEEP;
        if now == seen && !sweep {
            continue;
        }
        seen = now;
        if sweep {
            last_sweep = Instant::now();
        }
        for login in compare_all() {
            let _ = session_events::emit(SessionEvent::AccessChanged { login });
        }
    }
}

/// Read every held login again. Returns the logins whose access changed.
fn compare_all() -> Vec<String> {
    let logins: Vec<String> = with_entries(|m| m.keys().cloned().collect());
    let mut changed = Vec::new();
    for login in logins {
        let print = print_or_unread(&login);
        if print == UNREAD {
            continue;
        }
        let old = with_entries(|m| m.get_mut(&login).map(|e| std::mem::replace(&mut e.print, print)));
        if let Some(old) = old {
            if is_news(old, print) {
                changed.push(login);
            }
        }
    }
    changed
}

/// A change is news only between two live answers: the first read only records,
/// and a login that went is closed by its sockets' own check.
fn is_news(old: u64, new: u64) -> bool {
    old != new && old != UNREAD && old != GONE && new != GONE
}

fn print_or_unread(login: &str) -> u64 {
    match print(login) {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[login_access_live] a login's access couldn't be read: {e}");
            UNREAD
        }
    }
}

/// What `login` may do, hashed. [`GONE`] for a removed or disabled login.
fn print(login: &str) -> Result<u64, String> {
    let Some(rights) = k2_core::connect_users::login_rights(login)? else {
        return Ok(GONE);
    };
    let linked = k2_core::notify_prefs::People { users: Vec::new(), linked: k2_core::app_settings::load().owner_accounts }.is_linked(login);
    let chat = k2_core::tool_hook::connect_chat_grant_now(&rights.id)?.map(|g| (g.enabled, g.scope, g.role_id));
    // Not the id: a backfill that gives an old row its id changes no access.
    let k2_core::connect_users::LoginRights { id: _, role, can_clone, shares, must_change_password } = rights;
    Ok(hash_of(&format!("{role:?}|{can_clone}|{shares:?}|{must_change_password}|{linked}|{chat:?}")))
}

/// Never [`UNREAD`] or [`GONE`] but by a one-in-2^63 accident, which costs one
/// frame.
fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish().max(GONE + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_change_between_two_live_answers_is_news() {
        let (a, b) = (hash_of("a"), hash_of("b"));
        assert!(is_news(a, b));
        assert!(!is_news(a, a), "the same answer");
        assert!(!is_news(UNREAD, b), "the first read only records");
        assert!(!is_news(GONE, b), "a login that went and came back has no socket left");
        assert!(!is_news(a, GONE), "its sockets close on their own check");
        assert!(hash_of("") > GONE && hash_of("x") > GONE);
    }
}
