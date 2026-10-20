//! Chat addresses (C4, gaps G3–G5).
//!
//! ```text
//! addr     := space "/#" channel [ "/" thread ] | "dm/" dmtarget [ "/" thread ]
//! space    := slug | "id:" ID
//! channel  := slug | "id:" ID
//! thread   := ID
//! dmtarget := "@" handle | "id:" ID
//! slug     := [a-z0-9][a-z0-9-]{0,79}
//! handle   := [a-z0-9_-]{2,64}
//! ID       := [1-9][0-9]{0,18}, value <= i64::MAX
//! ```
//!
//! The parser is strict: no trimming, no lowercasing, no trailing `/`, no empty or extra
//! segments, at most 256 bytes. The space slug `dm` is reserved. In a URL query string the `#`
//! must be sent as `%23` ([`Addr::to_query_value`]).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::{Id, parse_id_str};
use crate::limits::{ADDR_MAX_BYTES, HANDLE_MAX_LEN, HANDLE_MIN_LEN, SLUG_MAX_LEN};

/// The reserved space slug (gap G4).
pub const RESERVED_SPACE_SLUG: &str = "dm";

/// Why a string is not a valid address, slug or handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddrError {
    /// The string is empty.
    Empty,
    /// Longer than 256 bytes.
    TooLong,
    /// Wrong number or arrangement of segments.
    BadShape,
    /// A slug breaks `[a-z0-9][a-z0-9-]{0,79}`.
    BadSlug,
    /// A handle breaks `[a-z0-9_-]{2,64}`.
    BadHandle,
    /// An id is not `[1-9][0-9]{0,18}` or exceeds `i64::MAX`.
    BadId,
    /// The space slug `dm` is reserved.
    ReservedSpaceSlug,
}

impl fmt::Display for AddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AddrError::Empty => "empty address",
            AddrError::TooLong => "address longer than 256 bytes",
            AddrError::BadShape => "address has the wrong shape",
            AddrError::BadSlug => "bad slug: expected [a-z0-9][a-z0-9-]{0,79}",
            AddrError::BadHandle => "bad handle: expected [a-z0-9_-]{2,64}",
            AddrError::BadId => "bad id: expected [1-9][0-9]{0,18} no larger than i64::MAX",
            AddrError::ReservedSpaceSlug => "the space slug \"dm\" is reserved",
        })
    }
}

impl std::error::Error for AddrError {}

fn is_valid_slug(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= SLUG_MAX_LEN
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn is_valid_handle(s: &str) -> bool {
    let b = s.as_bytes();
    (HANDLE_MIN_LEN..=HANDLE_MAX_LEN).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// A space or channel slug: `[a-z0-9][a-z0-9-]{0,79}`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

impl Slug {
    /// A channel slug (any valid slug).
    pub fn new(s: &str) -> Result<Slug, AddrError> {
        if is_valid_slug(s) {
            Ok(Slug(s.to_owned()))
        } else {
            Err(AddrError::BadSlug)
        }
    }

    /// A space slug: a valid slug other than the reserved `dm`.
    pub fn new_space(s: &str) -> Result<Slug, AddrError> {
        let slug = Slug::new(s)?;
        if slug.is_reserved_for_space() {
            return Err(AddrError::ReservedSpaceSlug);
        }
        Ok(slug)
    }

    /// Whether this slug may not name a space.
    pub fn is_reserved_for_space(&self) -> bool {
        self.0 == RESERVED_SPACE_SLUG
    }

    /// The slug text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Slug {
    type Err = AddrError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Slug::new(s)
    }
}

/// A member handle (username): `[a-z0-9_-]{2,64}` (gap G3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(String);

impl Handle {
    /// Validates a handle.
    pub fn new(s: &str) -> Result<Handle, AddrError> {
        if is_valid_handle(s) {
            Ok(Handle(s.to_owned()))
        } else {
            Err(AddrError::BadHandle)
        }
    }

    /// The handle text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Handle {
    type Err = AddrError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Handle::new(s)
    }
}

/// A space in an address: a slug or `id:<ID>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpaceRef {
    /// By slug.
    Slug(Slug),
    /// By id (`id:<ID>`).
    Id(Id),
}

/// A channel in an address: a slug or `id:<ID>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelRef {
    /// By slug.
    Slug(Slug),
    /// By id (`id:<ID>`).
    Id(Id),
}

/// A DM target: `@handle` (a 1:1 DM) or `id:<ID>` (any DM or group DM channel).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DmTarget {
    /// The 1:1 DM with this member.
    Handle(Handle),
    /// A DM or group DM channel by id.
    Id(Id),
}

/// A parsed chat address (C4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Addr {
    /// `space/#channel[/thread]`.
    Channel {
        /// The space.
        space: SpaceRef,
        /// The channel.
        channel: ChannelRef,
        /// The thread root message id.
        thread: Option<Id>,
    },
    /// `dm/target[/thread]`.
    Dm {
        /// The DM target.
        target: DmTarget,
        /// The thread root message id.
        thread: Option<Id>,
    },
}

fn parse_id_segment(s: &str) -> Result<Id, AddrError> {
    parse_id_str(s).ok_or(AddrError::BadId)
}

fn parse_thread(s: Option<&str>) -> Result<Option<Id>, AddrError> {
    match s {
        None => Ok(None),
        Some("") => Err(AddrError::BadShape),
        Some(t) => parse_id_segment(t).map(Some),
    }
}

impl Addr {
    /// The address as a URL query value: `#` becomes `%23`, `/` stays literal (gap G5).
    pub fn to_query_value(&self) -> String {
        self.to_string().replace('#', "%23")
    }

    /// The thread root id, if the address names a thread.
    pub fn thread(&self) -> Option<Id> {
        match self {
            Addr::Channel { thread, .. } | Addr::Dm { thread, .. } => *thread,
        }
    }
}

impl FromStr for Addr {
    type Err = AddrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(AddrError::Empty);
        }
        if s.len() > ADDR_MAX_BYTES {
            return Err(AddrError::TooLong);
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AddrError::BadShape);
        }
        if s.starts_with("dm/#") {
            return Err(AddrError::ReservedSpaceSlug);
        }
        if let Some(rest) = s.strip_prefix("dm/") {
            let mut parts = rest.split('/');
            let target = parts.next().unwrap_or_default();
            let thread = parts.next();
            if parts.next().is_some() || target.is_empty() {
                return Err(AddrError::BadShape);
            }
            let target = if let Some(h) = target.strip_prefix('@') {
                DmTarget::Handle(Handle::new(h)?)
            } else if let Some(id) = target.strip_prefix("id:") {
                DmTarget::Id(parse_id_segment(id)?)
            } else {
                return Err(AddrError::BadShape);
            };
            return Ok(Addr::Dm {
                target,
                thread: parse_thread(thread)?,
            });
        }
        let Some(split) = s.find("/#") else {
            return Err(AddrError::BadShape);
        };
        let (space, rest) = (&s[..split], &s[split + 2..]);
        if space.contains('/') {
            return Err(AddrError::BadShape);
        }
        let mut parts = rest.split('/');
        let channel = parts.next().unwrap_or_default();
        let thread = parts.next();
        if parts.next().is_some() {
            return Err(AddrError::BadShape);
        }
        let space = if let Some(id) = space.strip_prefix("id:") {
            SpaceRef::Id(parse_id_segment(id)?)
        } else {
            SpaceRef::Slug(Slug::new_space(space)?)
        };
        let channel = if let Some(id) = channel.strip_prefix("id:") {
            ChannelRef::Id(parse_id_segment(id)?)
        } else {
            ChannelRef::Slug(Slug::new(channel)?)
        };
        Ok(Addr::Channel {
            space,
            channel,
            thread: parse_thread(thread)?,
        })
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let thread = match self {
            Addr::Channel {
                space,
                channel,
                thread,
            } => {
                match space {
                    SpaceRef::Slug(s) => write!(f, "{s}")?,
                    SpaceRef::Id(id) => write!(f, "id:{id}")?,
                }
                f.write_str("/#")?;
                match channel {
                    ChannelRef::Slug(s) => write!(f, "{s}")?,
                    ChannelRef::Id(id) => write!(f, "id:{id}")?,
                }
                thread
            }
            Addr::Dm { target, thread } => {
                f.write_str("dm/")?;
                match target {
                    DmTarget::Handle(h) => write!(f, "@{h}")?,
                    DmTarget::Id(id) => write!(f, "id:{id}")?,
                }
                thread
            }
        };
        if let Some(t) = thread {
            write!(f, "/{t}")?;
        }
        Ok(())
    }
}

macro_rules! string_serde {
    ($ty:ty, $name:literal, $schema:tt) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(deserializer)?;
                s.parse().map_err(|e: AddrError| {
                    serde::de::Error::custom(format!("{}: {e} (got {:?})", $name, s))
                })
            }
        }

        impl JsonSchema for $ty {
            fn inline_schema() -> bool {
                true
            }
            fn schema_name() -> Cow<'static, str> {
                $name.into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!($schema)
            }
        }
    };
}

string_serde!(Slug, "Slug", {
    "type": "string",
    "pattern": "^[a-z0-9][a-z0-9-]{0,79}$",
    "description": "A space or channel slug. The space slug \"dm\" is reserved."
});

string_serde!(Handle, "Handle", {
    "type": "string",
    "pattern": "^[a-z0-9_-]{2,64}$",
    "description": "A member handle (username)."
});

string_serde!(Addr, "Addr", {
    "type": "string",
    "maxLength": 256,
    "description": "A chat address: space/#channel[/thread] or dm/@handle|id:ID[/thread]. Spaces and channels are a slug or id:ID. In a URL query string, send # as %23."
});
