//! The three string names the key wire carries (prd-sew-50 §6, rev 75): the `r_…` room that is
//! a call's `call_id`, the 32-hex `join_id` Sew mints at join, and the `p_…` engine identity
//! derived from it. Each checks its grammar on decode, so a malformed value is a `400` before
//! any handler runs. They match S37's `RoomName`, `JoinId` and `Identity` (`sew-media-proto`)
//! character for character; `sew-proto` cannot depend on that crate (rev 26 G19), so the
//! server converts through [`CallId::as_str`] and friends.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use super::tls::{base32_lower, lk_identity_for};

/// Why a name did not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameError {
    what: &'static str,
    grammar: &'static str,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a valid {} ({})", self.what, self.grammar)
    }
}

impl std::error::Error for NameError {}

fn is_b32(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

fn prefixed_b32(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|rest| rest.len() == 26 && is_b32(rest))
}

/// `^r_[a-z2-7]{26}$`.
pub const CALL_ID_PATTERN: &str = "^r_[a-z2-7]{26}$";
/// `^[0-9a-f]{32}$`.
pub const JOIN_ID_PATTERN: &str = "^[0-9a-f]{32}$";
/// `^p_[a-z2-7]{26}$`.
pub const LK_IDENTITY_PATTERN: &str = "^p_[a-z2-7]{26}$";

macro_rules! string_name {
    ($(#[$m:meta])* $name:ident, $what:literal, $pattern:expr, $check:expr) => {
        $(#[$m])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Parses and checks the grammar.
            pub fn parse(s: &str) -> Result<$name, NameError> {
                let check: fn(&str) -> bool = $check;
                if check(s) {
                    Ok($name(s.to_owned()))
                } else {
                    Err(NameError { what: $what, grammar: $pattern })
                }
            }

            /// The string form.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = NameError;
            fn from_str(s: &str) -> Result<$name, NameError> {
                $name::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<$name, D::Error> {
                let s = String::deserialize(d)?;
                $name::parse(&s).map_err(de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": $pattern,
                    "description": concat!("A ", $what, ".")
                })
            }
        }
    };
}

string_name!(
    /// A call's `call_id`: its engine room, `r_` + 26 base32 characters (random per call,
    /// rev 75).
    CallId, "call id", CALL_ID_PATTERN, |s| prefixed_b32(s, "r_")
);
string_name!(
    /// A Sew-minted join id: 32 lowercase hex characters (16 random bytes, rev 75).
    JoinIdHex, "join id", JOIN_ID_PATTERN, |s| s.len() == 32
        && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
);
string_name!(
    /// An engine identity: `p_` + 26 base32 characters of the join id (rev 75).
    LkIdentity, "engine identity", LK_IDENTITY_PATTERN, |s| prefixed_b32(s, "p_")
);

impl CallId {
    /// `r_` + base32 of 16 random bytes.
    pub fn from_random(bytes: [u8; 16]) -> CallId {
        CallId(format!("r_{}", base32_lower(&bytes)))
    }
}

impl JoinIdHex {
    /// The hex form of 16 bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> JoinIdHex {
        let mut s = String::with_capacity(32);
        for b in bytes {
            s.push(char::from(HEX[usize::from(b >> 4)]));
            s.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
        JoinIdHex(s)
    }

    /// The 16 bytes.
    pub fn bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        let raw = self.0.as_bytes();
        for (i, o) in out.iter_mut().enumerate() {
            *o = (nibble(raw[2 * i]) << 4) | nibble(raw[2 * i + 1]);
        }
        out
    }

    /// The engine identity of this join (`p_` + base32 of the bytes).
    pub fn lk_identity(&self) -> LkIdentity {
        LkIdentity(lk_identity_for(&self.bytes()))
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

impl LkIdentity {
    /// The identity of `join_id`.
    pub fn of(join_id: &JoinIdHex) -> LkIdentity {
        join_id.lk_identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_id_bytes_round_trip_and_identity() {
        let bytes = [
            0x5b, 0x0e, 0x00, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0,
            0xb0, 0xc0,
        ];
        let j = JoinIdHex::from_bytes(bytes);
        assert_eq!(j.as_str(), "5b0e00ff102030405060708090a0b0c0");
        assert_eq!(j.bytes(), bytes);
        assert_eq!(JoinIdHex::parse(j.as_str()), Ok(j.clone()));
        let id = j.lk_identity();
        assert_eq!(id.as_str(), lk_identity_for(&bytes));
        assert!(LkIdentity::parse(id.as_str()).is_ok());
    }

    #[test]
    fn grammars_refuse() {
        assert!(JoinIdHex::parse("5B0E00FF102030405060708090A0B0C0").is_err());
        assert!(JoinIdHex::parse("5b0e").is_err());
        assert!(CallId::parse("r_ABCDEFGHIJKLMNOPQRSTUVWXYZ").is_err());
        assert!(CallId::parse(&format!("r_{}", "a".repeat(26))).is_ok());
        assert!(CallId::parse(&format!("p_{}", "a".repeat(26))).is_err());
        assert!(LkIdentity::parse(&format!("p_{}", "a".repeat(25))).is_err());
        assert_eq!(CallId::from_random([0; 16]).as_str().len(), 28);
    }
}
