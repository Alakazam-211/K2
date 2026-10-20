//! One root sequence at a time (0.46.3 F9).
//!
//! The root helper serializes each *call* (its own lock), not a *sequence*
//! of calls: hosted mail's enable and upgrade (stop → snapshot → install →
//! health → rollback), the helper's self-heal, App box prep (apt, K2's
//! Podman, the k2app user, nft…), the App engine install, Podman's removal
//! and box undo each make several root calls that must not interleave
//! (an `apt-get` from box prep in the middle of a Stalwart upgrade's
//! snapshot, two `dpkg` runs fighting over the lock).
//!
//! They all share hosted mail's enable latch
//! (`mail::supervisor::try_begin_enable`), which the helper heal already
//! used: one `AtomicBool`, claimed without waiting. This module adds who
//! holds it (for the refusal text and the doctor) and an RAII guard.
//! Mail's own code still claims the latch directly; then the holder is
//! unnamed and the refusal says "a hosted-mail enable or upgrade".

use std::sync::Mutex;

static HOLDER: Mutex<Option<&'static str>> = Mutex::new(None);

/// Claim the latch for `what` (a short noun phrase: "automatic App box
/// prep"). `false` = someone else holds it; nothing changed.
pub fn begin(what: &'static str) -> bool {
    if !crate::mail::supervisor::try_begin_enable() {
        return false;
    }
    *HOLDER.lock().unwrap_or_else(|p| p.into_inner()) = Some(what);
    true
}

/// Release a latch taken with [`begin`].
pub fn end() {
    *HOLDER.lock().unwrap_or_else(|p| p.into_inner()) = None;
    crate::mail::supervisor::end_enable();
}

/// Who holds the latch now (`None` when free, or held by mail's own code).
pub fn holder() -> Option<&'static str> {
    if !crate::mail::supervisor::enable_running().load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    *HOLDER.lock().unwrap_or_else(|p| p.into_inner())
}

/// The latch while this lives.
#[must_use = "the latch is released when the guard drops"]
pub struct Held(());

impl Drop for Held {
    fn drop(&mut self) {
        end();
    }
}

/// The refusal text while the latch is held.
pub fn busy_text() -> String {
    let who = holder().map(str::to_string).unwrap_or_else(|| "a hosted-mail enable or upgrade".into());
    format!("{who} is running and K2 runs one root sequence at a time: try again when it finishes")
}

/// Claim the latch for `what`, or say who has it.
pub fn try_hold(what: &'static str) -> Result<Held, String> {
    if begin(what) {
        Ok(Held(()))
    } else {
        Err(busy_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// F9: box prep, mail and every other root sequence exclude each other
    /// both ways, the refusal names the holder, and the guard frees it.
    #[test]
    fn the_latch_is_shared_with_hosted_mail_and_names_its_holder() {
        let _g = crate::mail::mail_server_test_lock();
        let held = try_hold("automatic App box prep").expect("free at start");
        assert_eq!(holder(), Some("automatic App box prep"));
        assert!(!crate::mail::supervisor::try_begin_enable(), "a mail enable/upgrade can't start inside box prep");
        let e = try_hold("App engine install").err().expect("second sequence refused");
        assert!(e.starts_with("automatic App box prep is running"), "{e}");
        drop(held);
        assert_eq!(holder(), None);
        // Mail holds it (its own direct claim): box prep is refused, unnamed holder.
        assert!(crate::mail::supervisor::try_begin_enable());
        let e = try_hold("App box prep").err().expect("refused while mail holds it");
        assert!(e.starts_with("a hosted-mail enable or upgrade is running"), "{e}");
        crate::mail::supervisor::end_enable();
        drop(try_hold("App box prep").expect("free again"));
    }

    /// F9: under contention, never two root sequences at once.
    #[test]
    fn the_latch_serializes_concurrent_sequences() {
        let _g = crate::mail::mail_server_test_lock();
        let inside = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let names: [&'static str; 4] = ["auto prep", "engine install", "box undo", "helper heal"];
        let threads: Vec<_> = names
            .into_iter()
            .map(|n| {
                let (inside, max, done) = (inside.clone(), max.clone(), done.clone());
                std::thread::spawn(move || {
                    let mut runs = 0;
                    while runs < 25 {
                        let Ok(h) = try_hold(n) else {
                            std::thread::yield_now();
                            continue;
                        };
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        max.fetch_max(now, Ordering::SeqCst);
                        assert_eq!(holder(), Some(n), "the holder is whoever is inside");
                        std::thread::sleep(std::time::Duration::from_micros(200));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        drop(h);
                        runs += 1;
                    }
                    done.fetch_add(runs, Ordering::SeqCst);
                })
            })
            .collect();
        for t in threads {
            t.join().expect("no thread panicked");
        }
        assert_eq!(done.load(Ordering::SeqCst), 100, "every sequence ran");
        assert_eq!(max.load(Ordering::SeqCst), 1, "never two root sequences at once");
        assert_eq!(holder(), None);
    }
}
