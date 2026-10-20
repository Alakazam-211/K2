//! Call key delivery (S50, prd-sew-50 §9, §20): the wire contract between the MLS delivery
//! service in `sew` and the `sew-e2ee` client library (S51).
//!
//! - [`b64`]: the `B64` newtype (base64url, no padding) every MLS blob travels as.
//! - [`tls`]: the §9.5 binary structures (credentials, certificates, binding tables, release
//!   `info`), the labels and the slot formula.
//! - [`CallScope`]: what a KeyPackage may join (`call:<channel_id>`, mandatory, rev 88 D3).
//! - [`wire`]: the JSON types of the key routes, the `call_keys_delivered` dispatch and READY.
//! - [`device`]: device enrolment, renewal, revocation and the CRL (K11–K15).
//! - [`names`]: the `r_…` call id, the 32-hex join id and the `p_…` engine identity.
//! - [`reasons`]: every error reason string.

pub mod b64;
pub mod device;
pub mod names;
pub mod reasons;
pub mod tls;
pub mod wire;

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::id::Id;

pub use b64::{B64, B64Error};
pub use device::*;
pub use names::{CallId, JoinIdHex, LkIdentity, NameError};
pub use wire::*;

/// Whether a member who **declines** recording consent may still join a recorded call,
/// unrecorded (`true`), or is refused at join with `409 conflict` reason
/// `recording_consent_required` while a recording is live (`false`).
///
/// **Provisional** (rev 91: "Decline" is still to be confirmed with Rosson; the default until he
/// answers is that a decliner may join unrecorded, since the crypto supports it). With `true`,
/// S50 still refuses every release from that member (§14.3 rule 3). S33 v2's join gate reads
/// this one constant.
pub const RECORDING_DECLINE_JOINS_UNRECORDED: bool = true;

/// `details.reason` of the `409` a declined member gets at join while a recording is live, when
/// [`RECORDING_DECLINE_JOINS_UNRECORDED`] is `false`.
pub const RECORDING_CONSENT_REQUIRED: &str = "recording_consent_required";

/// What a KeyPackage may join: always `call:<channel_id>` (the voice channel, or the DM channel
/// of an S34 call), with the channel id in decimal (prd-sew-50 D17, rev 88 D3). One
/// constructor; [`CallScope::parse`] refuses every other form, including the empty scope and
/// the old `voice:<id>` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallScope {
    channel_id: Id,
}

/// The longest scope accepted (ASCII bytes).
pub const SCOPE_MAX_BYTES: usize = 64;

impl CallScope {
    /// The scope of a call in `channel_id`.
    pub fn call(channel_id: Id) -> CallScope {
        CallScope { channel_id }
    }

    /// The channel.
    pub fn channel_id(self) -> Id {
        self.channel_id
    }

    /// The wire bytes (`call:<decimal id>`).
    pub fn to_bytes(self) -> Vec<u8> {
        self.to_string().into_bytes()
    }

    /// Parses `call:<decimal id>` exactly (no sign, no leading zero, ≤ 64 bytes).
    pub fn parse(bytes: &[u8]) -> Option<CallScope> {
        if bytes.is_empty() || bytes.len() > SCOPE_MAX_BYTES {
            return None;
        }
        let s = std::str::from_utf8(bytes).ok()?;
        let digits = s.strip_prefix("call:")?;
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let id: Id = digits.parse().ok()?;
        (id.to_string() == digits).then_some(CallScope { channel_id: id })
    }
}

impl fmt::Display for CallScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "call:{}", self.channel_id)
    }
}

impl Serialize for CallScope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CallScope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<CallScope, D::Error> {
        let s = String::deserialize(d)?;
        CallScope::parse(s.as_bytes())
            .ok_or_else(|| de::Error::custom("not a call scope (call:<decimal channel id>)"))
    }
}

impl JsonSchema for CallScope {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "CallScope".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^call:[1-9][0-9]*$",
            "description": "What a KeyPackage may join: `call:<channel_id>` (decimal)."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_round_trip_and_refusals() {
        let id: Id = match "1839000000000000001".parse() {
            Ok(i) => i,
            Err(_) => return,
        };
        let s = CallScope::call(id);
        assert_eq!(s.to_string(), "call:1839000000000000001");
        assert_eq!(CallScope::parse(&s.to_bytes()), Some(s));
        for bad in [
            &b""[..],
            b"call:",
            b"voice:1839000000000000001",
            b"call:01",
            b"call:-1",
            b"call:1x",
            b"CALL:1",
        ] {
            assert_eq!(CallScope::parse(bad), None, "{bad:?}");
        }
    }
}
