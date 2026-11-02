//! The custom names the tunnel's second frpc proxy carries (0.46.8,
//! `prd-custom-domains-path-b-0467-v1` PB-2; vs-live PB-A4).
//!
//! Path B: a customer's name (`shop.example.com`) points at K2's relays,
//! which pass the TLS stream through by name down this box's tunnel. frps
//! registers such a name only on a proxy named `k2-custom-<label>` from that
//! label's token, and only names the control plane verified for it. The
//! daemon decides the list (`live` + `issuing` names whose relay claim is
//! verified, `apps::custom_names::reload`) and hands it here with [`set`];
//! the frpc render ([`super::render`]) adds the second proxy from it, in
//! every per-relay render; the supervisor restarts frpc when the list
//! changes ([`restart_due`]): a few seconds' blip for all tunnel traffic
//! (Rosson 2026-11-01: OK). Changes that land within [`SETTLE`] of each
//! other share one restart. Memory only: the daemon sets the list at boot
//! before the tunnel starts, and on every rebuild after.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The custom proxy's name prefix: `k2-custom-<label>` (k2-connect's frps
/// plugin keys on it).
pub const PROXY_PREFIX: &str = "k2-custom-";

/// Changes closer together than this share one frpc restart.
pub const SETTLE: Duration = Duration::from_secs(3);

/// Most names one box may carry through the relay on Pro (Rosson
/// 2026-11-01). The control plane enforces it; the box checks first for a
/// good error.
pub const MAX_PER_BOX: usize = 20;

#[derive(Debug, Default)]
struct State {
    names: Vec<String>,
    generation: u64,
    changed_at: Option<Instant>,
    /// The generation the running frpc was rendered with.
    running: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|p| p.into_inner())
}

/// Normalize a list: lowercase, no trailing dot, sorted, no duplicates,
/// nothing empty.
pub fn normalized(names: Vec<String>) -> Vec<String> {
    let mut v: Vec<String> = names
        .into_iter()
        .map(|n| n.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Replace the list. `true` when it changed (the supervisor then restarts
/// frpc after [`SETTLE`]).
pub fn set(names: Vec<String>) -> bool {
    let names = normalized(names);
    let mut s = lock();
    if s.names == names {
        return false;
    }
    s.names = names;
    s.generation += 1;
    s.changed_at = Some(Instant::now());
    true
}

/// The list now (sorted).
pub fn current() -> Vec<String> {
    lock().names.clone()
}

/// Bumped on every change.
pub fn generation() -> u64 {
    lock().generation
}

/// The list and its generation, read together (a render records the
/// generation it used with [`note_rendered`]).
pub fn snapshot() -> (Vec<String>, u64) {
    let s = lock();
    (s.names.clone(), s.generation)
}

/// The supervisor rendered and started frpc with generation `g`.
pub fn note_rendered(g: u64) {
    let mut s = lock();
    if g > s.running {
        s.running = g;
    }
}

/// The generation the running frpc carries.
pub fn running_generation() -> u64 {
    lock().running
}

/// Should the supervisor restart frpc now, given it rendered generation
/// `spawned_with`? Yes once the list changed and has been still for
/// [`SETTLE`].
pub fn restart_due(spawned_with: u64, now: Instant) -> bool {
    let s = lock();
    s.generation != spawned_with && s.changed_at.is_none_or(|t| now.saturating_duration_since(t) >= SETTLE)
}

/// Wait (blocking) until the running frpc carries generation `g` or later,
/// at most `timeout`. `true` when it does.
pub fn wait_running(g: u64, timeout: Duration) -> bool {
    let until = Instant::now() + timeout;
    loop {
        if running_generation() >= g {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Test-only: back to empty.
#[cfg(any(test, feature = "test-util"))]
pub fn reset_for_test() {
    *lock() = State::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_normalizes_and_counts_changes_and_restart_waits_to_settle() {
        let _g = crate::test_env::lock();
        reset_for_test();
        assert!(set(vec!["Shop.Example.com.".into(), "a.example.com".into(), "shop.example.com".into(), " ".into()]));
        assert_eq!(current(), vec!["a.example.com".to_string(), "shop.example.com".to_string()]);
        let g = generation();
        assert!(!set(vec!["shop.example.com".into(), "A.example.com".into()]), "same list in another order: no change");
        assert_eq!(generation(), g);
        // Rendered with an older generation: due only after the settle.
        let now = Instant::now();
        assert!(!restart_due(g - 1, now), "not before the list has been still for SETTLE");
        assert!(restart_due(g - 1, now + SETTLE), "due once still");
        assert!(!restart_due(g, now + SETTLE), "the running frpc already carries it");
        note_rendered(g);
        assert_eq!(running_generation(), g);
        assert!(wait_running(g, Duration::from_millis(1)));
        assert!(!wait_running(g + 1, Duration::from_millis(1)));
        reset_for_test();
    }
}
