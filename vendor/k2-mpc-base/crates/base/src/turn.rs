//! A host-given **turn**: the id of one agent step, which the host passes on every write call
//! so a view can group a run's changes.
//!
//! - **The host's, never the kit's.** The kit never invents one, never reads one from a
//!   client, and `None` is the default on every call. A host computes it server-side (K2: a
//!   hash of the agent session and its turn number) and gives none for people and shells.
//! - **Opaque.** The kit stores and reports it as given, compares it only for equality, and
//!   never parses it.
//! - **Per call, not per connection.** Agents share one connection per participant, so the
//!   turn rides on each write call's options ([`crate::tools::ToolOptions::turn`],
//!   [`crate::session::EditOptions::turn`], [`crate::session::WriteOptions::turn`],
//!   [`crate::session::UndoRequest::turn`], [`crate::session::HistoryOptions::turn`],
//!   [`crate::session::BlobWriteOptions::turn`]).
//! - **Where it goes:** the change record (live: the `change_turns` table; tier 0: the
//!   `blob_turns` table), the change events ([`crate::session::ChangeEvent::turn`]) and the
//!   history ([`crate::store::ChangeInfo::turn`], [`crate::store::BlobVersion::turn`]).
//! - **Erasure clears it.** It links changes into one agent run, so a participant's erasure
//!   removes the turn of every change they made or that was made on their behalf.

use std::fmt;

/// At most this many bytes in a [`Turn`].
pub const TURN_MAX: usize = 64;

/// One agent step's id, as the host gave it: 1 to [`TURN_MAX`] bytes of printable ASCII
/// with no spaces (`0x21..=0x7e`), e.g. `t:1a2b3c4d5e6f7a8b.12`. `Copy`, so the options that
/// carry it stay `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Turn {
    len: u8,
    bytes: [u8; TURN_MAX],
}

/// Why [`Turn::new`] refused a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BadTurn {
    Empty,
    /// More than [`TURN_MAX`] bytes.
    TooLong {
        len: usize,
    },
    /// A byte outside `0x21..=0x7e` (a space, a control character, anything not ASCII).
    Unprintable {
        at: usize,
    },
}

impl fmt::Display for BadTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BadTurn::Empty => f.write_str("a turn id is empty"),
            BadTurn::TooLong { len } => {
                write!(f, "a turn id of {len} bytes; at most {TURN_MAX}")
            }
            BadTurn::Unprintable { at } => write!(
                f,
                "a turn id has a byte at {at} that isn't printable ASCII without spaces"
            ),
        }
    }
}

impl std::error::Error for BadTurn {}

impl Turn {
    /// The host's turn id, checked: [`BadTurn`] for an empty one, one over [`TURN_MAX`]
    /// bytes, or one with a byte that isn't printable ASCII without spaces.
    pub fn new(s: &str) -> Result<Self, BadTurn> {
        let b = s.as_bytes();
        if b.is_empty() {
            return Err(BadTurn::Empty);
        }
        if b.len() > TURN_MAX {
            return Err(BadTurn::TooLong { len: b.len() });
        }
        if let Some(at) = b.iter().position(|c| !(0x21..=0x7e).contains(c)) {
            return Err(BadTurn::Unprintable { at });
        }
        let mut bytes = [0u8; TURN_MAX];
        bytes[..b.len()].copy_from_slice(b);
        Ok(Self {
            len: b.len() as u8,
            bytes,
        })
    }

    pub fn as_str(&self) -> &str {
        // only printable ASCII is ever stored (`new`)
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl fmt::Display for Turn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Turn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Turn({:?})", self.as_str())
    }
}

impl std::str::FromStr for Turn {
    type Err = BadTurn;

    fn from_str(s: &str) -> Result<Self, BadTurn> {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_checked_and_kept_as_given() {
        let t = Turn::new("t:1a2b3c4d5e6f7a8b.12").unwrap();
        assert_eq!(t.as_str(), "t:1a2b3c4d5e6f7a8b.12");
        assert_eq!(t.to_string(), "t:1a2b3c4d5e6f7a8b.12");
        assert_eq!(Turn::new(""), Err(BadTurn::Empty));
        assert_eq!(
            Turn::new(&"x".repeat(TURN_MAX)).unwrap().as_str().len(),
            TURN_MAX
        );
        assert_eq!(
            Turn::new(&"x".repeat(TURN_MAX + 1)),
            Err(BadTurn::TooLong { len: TURN_MAX + 1 })
        );
        assert_eq!(Turn::new("t 1"), Err(BadTurn::Unprintable { at: 1 }));
        assert_eq!(Turn::new("t\n"), Err(BadTurn::Unprintable { at: 1 }));
        assert_eq!(Turn::new("tü"), Err(BadTurn::Unprintable { at: 1 }));
        // equality is the string's, never the padding's
        assert_eq!(Turn::new("ab").unwrap(), "ab".parse::<Turn>().unwrap());
        assert_ne!(Turn::new("ab").unwrap(), Turn::new("abc").unwrap());
    }
}
