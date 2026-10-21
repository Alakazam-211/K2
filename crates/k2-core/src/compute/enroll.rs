//! One-time enroll codes (§7.2, CN1). In memory only: a code lives 10
//! minutes and a daemon restart voids it, which is fine.
//!
//! The node never sends the code, only `pairing::code_binding(code)`; the
//! controller compares bindings in constant time. Five failed attempts
//! while any code is live void every live code (the relay path can't see
//! client IPs, so the lockout is per controller, not per IP); the owner
//! mints a new one. A code is single use.

use std::sync::Mutex;

use super::proto::{crypto, pairing};

/// Code lifetime.
pub const CODE_TTL_SECS: i64 = 600;
/// Failed attempts before every live code is voided.
pub const MAX_FAILURES: u32 = 5;
/// Live codes at once.
pub const MAX_LIVE_CODES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeEntry {
    /// `pairing::code_binding` of the normalized code.
    pub binding: String,
    /// The name the owner chose for the node.
    pub name: String,
    pub minted_by: String,
    pub expires_at: i64,
}

#[derive(Debug, Default)]
pub struct CodeBook {
    live: Vec<CodeEntry>,
    failures: u32,
}

/// Why a code was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeRefusal {
    /// No live code matches (wrong, used, or expired).
    Invalid,
    /// This attempt hit the lockout: every live code was voided.
    LockedOut,
}

impl CodeRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            CodeRefusal::Invalid => "code_invalid",
            CodeRefusal::LockedOut => "code_locked",
        }
    }
}

impl CodeBook {
    fn sweep(&mut self, now: i64) {
        self.live.retain(|c| c.expires_at > now);
        if self.live.is_empty() {
            self.failures = 0;
        }
    }

    /// Mint a code for `name`. Returns `(display code, entry)`.
    pub fn mint(&mut self, name: &str, by: &str, now: i64) -> Result<(String, CodeEntry), String> {
        self.sweep(now);
        if self.live.len() >= MAX_LIVE_CODES {
            return Err(format!("{MAX_LIVE_CODES} enroll codes are already waiting; let one expire first"));
        }
        self.live.retain(|c| c.name != name);
        let display = pairing::new_code()?;
        let normalized = pairing::normalize_code(&display).ok_or("minted code didn't normalize")?;
        let entry = CodeEntry {
            binding: pairing::code_binding(&normalized),
            name: name.to_string(),
            minted_by: by.to_string(),
            expires_at: now + CODE_TTL_SECS,
        };
        self.live.push(entry.clone());
        Ok((display, entry))
    }

    /// Consume the code whose binding matches. Single use.
    pub fn take(&mut self, binding: &str, now: i64) -> Result<CodeEntry, CodeRefusal> {
        self.sweep(now);
        let mut found = None;
        for (i, c) in self.live.iter().enumerate() {
            // Compare every entry; don't stop at the first match.
            if crypto::ct_eq(c.binding.as_bytes(), binding.as_bytes()) {
                found = Some(i);
            }
        }
        match found {
            Some(i) => {
                self.failures = 0;
                Ok(self.live.remove(i))
            }
            None => {
                if self.live.is_empty() {
                    return Err(CodeRefusal::Invalid);
                }
                self.failures += 1;
                if self.failures >= MAX_FAILURES {
                    self.live.clear();
                    self.failures = 0;
                    return Err(CodeRefusal::LockedOut);
                }
                Err(CodeRefusal::Invalid)
            }
        }
    }

    /// Put a taken code back, until its own expiry: the node proved the
    /// code and its key, but the enrol couldn't finish for a reason the
    /// owner can fix (this server still has the node's old row). The node
    /// retries with the same code once the row is removed.
    pub fn restore(&mut self, entry: CodeEntry, now: i64) {
        self.sweep(now);
        if entry.expires_at <= now || self.live.len() >= MAX_LIVE_CODES || self.live.iter().any(|c| c.name == entry.name || c.binding == entry.binding) {
            return;
        }
        self.live.push(entry);
    }

    /// Live codes (no secrets: names and expiry only).
    pub fn pending(&mut self, now: i64) -> Vec<(String, i64)> {
        self.sweep(now);
        self.live.iter().map(|c| (c.name.clone(), c.expires_at)).collect()
    }
}

static BOOK: Mutex<Option<CodeBook>> = Mutex::new(None);

fn with_book<T>(f: impl FnOnce(&mut CodeBook) -> T) -> T {
    let mut g = BOOK.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(CodeBook::default))
}

pub fn mint(name: &str, by: &str) -> Result<(String, CodeEntry), String> {
    with_book(|b| b.mint(name, by, super::now()))
}

pub fn take(binding: &str) -> Result<CodeEntry, CodeRefusal> {
    with_book(|b| b.take(binding, super::now()))
}

pub fn pending() -> Vec<(String, i64)> {
    with_book(|b| b.pending(super::now()))
}

pub fn restore(entry: CodeEntry) {
    with_book(|b| b.restore(entry, super::now()))
}

/// A node name: 1–32 of `a-z 0-9 -`, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding_of(display: &str) -> String {
        pairing::code_binding(&pairing::normalize_code(display).unwrap())
    }

    /// A code taken by an enrol that couldn't finish (the old row) goes
    /// back until its own expiry; never past it, never twice.
    #[test]
    fn a_restored_code_is_good_again_until_it_expires() {
        let mut b = CodeBook::default();
        let (code, _) = b.mint("mini-1", "owner", 1000).unwrap();
        let e = b.take(&binding_of(&code), 1001).unwrap();
        assert_eq!(b.take(&binding_of(&code), 1002), Err(CodeRefusal::Invalid), "single use");
        b.restore(e.clone(), 1003);
        b.restore(e.clone(), 1003);
        assert_eq!(b.pending(1003).len(), 1, "restored once");
        assert_eq!(b.take(&binding_of(&code), 1004).unwrap().name, "mini-1", "good again");
        b.restore(e.clone(), e.expires_at);
        assert_eq!(b.take(&binding_of(&code), e.expires_at), Err(CodeRefusal::Invalid), "never past its expiry");
    }

    #[test]
    fn code_is_single_use() {
        let mut b = CodeBook::default();
        let (code, e) = b.mint("mini-1", "owner", 100).unwrap();
        assert_eq!(e.expires_at, 100 + CODE_TTL_SECS);
        let got = b.take(&binding_of(&code), 101).unwrap();
        assert_eq!(got.name, "mini-1");
        assert_eq!(b.take(&binding_of(&code), 102), Err(CodeRefusal::Invalid));
    }

    #[test]
    fn code_expires() {
        let mut b = CodeBook::default();
        let (code, _) = b.mint("a", "o", 100).unwrap();
        assert_eq!(b.take(&binding_of(&code), 100 + CODE_TTL_SECS), Err(CodeRefusal::Invalid));
    }

    #[test]
    fn five_failures_void_every_live_code() {
        let mut b = CodeBook::default();
        let (good, _) = b.mint("a", "o", 100).unwrap();
        for i in 0..4 {
            assert_eq!(b.take(&format!("wrong{i}"), 101), Err(CodeRefusal::Invalid));
        }
        assert_eq!(b.take("wrong-last", 101), Err(CodeRefusal::LockedOut));
        assert_eq!(b.take(&binding_of(&good), 102), Err(CodeRefusal::Invalid), "the good code died in the lockout");
        // A new code works again.
        let (fresh, _) = b.mint("a", "o", 103).unwrap();
        assert!(b.take(&binding_of(&fresh), 104).is_ok());
    }

    #[test]
    fn remint_for_a_name_replaces_the_old_code() {
        let mut b = CodeBook::default();
        let (old, _) = b.mint("a", "o", 1).unwrap();
        let (new, _) = b.mint("a", "o", 2).unwrap();
        assert_eq!(b.take(&binding_of(&old), 3), Err(CodeRefusal::Invalid));
        assert!(b.take(&binding_of(&new), 3).is_ok());
    }

    #[test]
    fn names() {
        assert!(valid_name("mini-1"));
        assert!(valid_name("z13flow"));
        assert!(!valid_name("-x"));
        assert!(!valid_name("Mini"));
        assert!(!valid_name(""));
        assert!(!valid_name(&"a".repeat(33)));
    }
}
