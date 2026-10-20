//! External identities (C6, C9, gap G30).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::addr::Handle;
use crate::limits::EXTERNAL_ID_MAX_BYTES;

/// Who a member is outside Sew.
///
/// Wire forms: `k2:principal:<uuid>`, `k2:workspace:<uuid>`, `k2:connect:<uuid>`, `k2:owner`,
/// `local:<handle>`, `link:<channel_id>:<base32>` (Meet MC2a, CC2: a meeting link guest; never
/// accepted by mgmt provision or the K2 pass). `<uuid>` is lowercase hyphenated 8-4-4-4-12 hex (K2 principal ids are
/// `Uuid::new_v4()`; a Connect user's is the never-reused uuid K2 gives each Connect user).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExternalId {
    /// A K2 person: `k2:principal:<uuid>`. Holds the uuid.
    K2Principal(String),
    /// A K2 agent (workspace): `k2:workspace:<uuid>`. Holds the uuid.
    K2Workspace(String),
    /// The K2 box owner: `k2:owner` (Q9, gap G30).
    K2Owner,
    /// A K2 Connect user of the server: `k2:connect:<uuid>` (SW2, contracts rev 134: Connect
    /// users are members of their server's Space). A person, never an agent. Holds the uuid.
    K2Connect(String),
    /// A standalone local user: `local:<handle>`.
    Local(Handle),
    /// A meeting link guest (Meet MC2a, CC2): `link:<meeting_channel_id>:<16 base32 chars>`.
    /// Made only by `POST /api/v1/meetings/redeem`; mgmt provision and the K2 pass refuse it.
    Link {
        /// The meeting channel the guest is bound to.
        channel: crate::id::Id,
        /// 16 characters of RFC 4648 base32 (`A–Z2–7`, upper case): 10 random bytes.
        token: String,
    },
}

/// Length of a link guest's random external id part, in base32 characters (10 bytes).
pub const LINK_GUEST_TOKEN_CHARS: usize = 16;

/// Whether `s` is 16 characters of upper-case RFC 4648 base32 (no padding).
pub fn is_link_token(s: &str) -> bool {
    s.len() == LINK_GUEST_TOKEN_CHARS
        && s.bytes()
            .all(|c| c.is_ascii_uppercase() || (b'2'..=b'7').contains(&c))
}

/// Error parsing an [`ExternalId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdError(String);

impl fmt::Display for ExternalIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid external id {:?}: expected k2:principal:<uuid>, k2:workspace:<uuid>, k2:connect:<uuid>, k2:owner, local:<handle> or link:<channel_id>:<base32>",
            self.0
        )
    }
}

impl std::error::Error for ExternalIdError {}

/// Whether `s` is a lowercase hyphenated 8-4-4-4-12 hex uuid.
pub fn is_lower_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

impl ExternalId {
    /// Whether this is an agent identity (`k2:workspace:*`).
    pub fn is_agent(&self) -> bool {
        matches!(self, ExternalId::K2Workspace(_))
    }

    /// Whether this is a meeting link guest (`link:*`, CC2).
    pub fn is_link(&self) -> bool {
        matches!(self, ExternalId::Link { .. })
    }
}

impl FromStr for ExternalId {
    type Err = ExternalIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ExternalIdError(s.to_owned());
        if s.len() > EXTERNAL_ID_MAX_BYTES {
            return Err(err());
        }
        if s == "k2:owner" {
            return Ok(ExternalId::K2Owner);
        }
        if let Some(u) = s.strip_prefix("k2:principal:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Principal(u.to_owned()))
            } else {
                Err(err())
            };
        }
        if let Some(u) = s.strip_prefix("k2:workspace:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Workspace(u.to_owned()))
            } else {
                Err(err())
            };
        }
        if let Some(u) = s.strip_prefix("k2:connect:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Connect(u.to_owned()))
            } else {
                Err(err())
            };
        }
        if let Some(h) = s.strip_prefix("local:") {
            return Handle::new(h).map(ExternalId::Local).map_err(|_| err());
        }
        if let Some(rest) = s.strip_prefix("link:") {
            let (c, t) = rest.split_once(':').ok_or_else(err)?;
            let channel: crate::id::Id = c.parse().map_err(|_| err())?;
            // Canonical form only: the id must print back as given.
            if channel.to_string() != c || !is_link_token(t) {
                return Err(err());
            }
            return Ok(ExternalId::Link {
                channel,
                token: t.to_owned(),
            });
        }
        Err(err())
    }
}

impl fmt::Display for ExternalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalId::K2Principal(u) => write!(f, "k2:principal:{u}"),
            ExternalId::K2Workspace(u) => write!(f, "k2:workspace:{u}"),
            ExternalId::K2Owner => f.write_str("k2:owner"),
            ExternalId::K2Connect(u) => write!(f, "k2:connect:{u}"),
            ExternalId::Local(h) => write!(f, "local:{h}"),
            ExternalId::Link { channel, token } => write!(f, "link:{channel}:{token}"),
        }
    }
}

impl Serialize for ExternalId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ExternalId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ExternalId {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "ExternalId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "maxLength": 128,
            "pattern": "^(k2:(principal|workspace|connect):[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|k2:owner|local:[a-z0-9_-]{2,64}|link:[1-9][0-9]{0,18}:[A-Z2-7]{16})$",
            "description": "An external identity: k2:principal:<uuid>, k2:workspace:<uuid>, k2:connect:<uuid>, k2:owner, local:<handle> or link:<channel_id>:<base32> (a meeting link guest)."
        })
    }
}
