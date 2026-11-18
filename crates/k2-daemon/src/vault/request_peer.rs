//! Who is asking, for the one vault request this worker thread is
//! running: what a token from this computer's own user check is bound to,
//! and the words its prompt uses for the asker.
//!
//! Set around the request by the transport that knows it:
//!
//! - the vault socket: the peer's Unix session (`@sid:<n>`), so a check
//!   made from one terminal is spent from that terminal only, and the
//!   asker ("a terminal on this computer", "agent scout in a K2 terminal");
//! - TCP with a Connect login's session: that session (`@login:<hash>`).
//!
//! The owner token over TCP carries nothing more: every process of this
//! user can present it, and TCP says nothing about which one is calling.
//! A check made there is bound to the step, its folder or key, and the
//! owner, and lives 60 seconds; its token travels only in request bodies.

use std::cell::RefCell;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peer {
    /// Appended to the caller's stamp when a check's token is minted and
    /// when it is taken. Empty: nothing more to bind to.
    pub bind: String,
    /// The asker, in words, for this computer's prompt.
    pub asker: Option<String>,
}

thread_local! {
    static PEER: RefCell<Option<Peer>> = const { RefCell::new(None) };
}

/// Run `f` with `p` as this thread's request peer (the previous one is
/// put back after, panics included).
pub fn scoped<R>(p: Peer, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Peer>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            PEER.with(|c| *c.borrow_mut() = prev);
        }
    }
    let prev = PEER.with(|c| c.borrow_mut().replace(p));
    let _restore = Restore(prev);
    f()
}

/// This request's binding (empty when none).
pub fn bind() -> String {
    PEER.with(|c| c.borrow().as_ref().map(|p| p.bind.clone()).unwrap_or_default())
}

/// This request's asker, when the transport knows it.
pub fn asker() -> Option<String> {
    PEER.with(|c| c.borrow().as_ref().and_then(|p| p.asker.clone()))
}

/// A name from a peer's environment, safe to show in a prompt.
pub fn shown_name(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')).take(40).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_sets_and_restores() {
        assert_eq!(bind(), "");
        let r = scoped(Peer { bind: "@sid:7".into(), asker: Some("a terminal".into()) }, || {
            assert_eq!(asker().as_deref(), Some("a terminal"));
            scoped(Peer { bind: "@sid:8".into(), asker: None }, bind)
        });
        assert_eq!(r, "@sid:8");
        assert_eq!(bind(), "", "restored after");
        assert_eq!(shown_name("  sc\u{1b}out\n "), Some("scout".into()));
        assert_eq!(shown_name("\u{7}"), None);
    }
}
