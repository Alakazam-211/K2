//! Wire timestamps (C5, gap G22).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, SecondsFormat, Utc};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::clock::{Clock, SystemClock};

/// A UTC instant with millisecond precision.
///
/// JSON: an RFC 3339 string, always UTC with three fraction digits and a `Z` suffix, for example
/// `"2026-09-27T12:00:00.123Z"`. Input accepts any RFC 3339 string with an offset; it is
/// converted to UTC and truncated to milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// The current time from the system clock. Server code uses its injected [`Clock`] instead.
    pub fn now() -> Self {
        SystemClock.now()
    }

    /// Builds a timestamp from Unix milliseconds. `None` when out of chrono's range.
    pub fn from_unix_ms(ms: i64) -> Option<Self> {
        DateTime::from_timestamp_millis(ms).map(Timestamp)
    }

    /// Builds a timestamp from unsigned Unix milliseconds, clamping values past chrono's range to
    /// its maximum. Used by clocks, which never read beyond year 262143.
    pub fn from_unix_ms_saturating(ms: u64) -> Self {
        let ms = i64::try_from(ms).unwrap_or(i64::MAX);
        match DateTime::from_timestamp_millis(ms) {
            Some(dt) => Timestamp(dt),
            None => Timestamp(DateTime::<Utc>::MAX_UTC),
        }
    }

    /// Wraps a chrono value, truncating it to milliseconds.
    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        match DateTime::from_timestamp_millis(dt.timestamp_millis()) {
            Some(t) => Timestamp(t),
            None => Timestamp(dt),
        }
    }

    /// Milliseconds since the Unix epoch.
    pub fn unix_ms(self) -> i64 {
        self.0.timestamp_millis()
    }

    /// The chrono value.
    pub fn as_datetime(self) -> DateTime<Utc> {
        self.0
    }

    /// The wire string: RFC 3339, UTC, milliseconds, `Z`.
    pub fn to_wire(self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

/// Error parsing a [`Timestamp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampError(String);

impl fmt::Display for TimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid RFC 3339 timestamp: {}", self.0)
    }
}

impl std::error::Error for TimestampError {}

impl FromStr for Timestamp {
    type Err = TimestampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| Timestamp::from_datetime(dt.with_timezone(&Utc)))
            .map_err(|e| TimestampError(format!("{s:?}: {e}")))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Timestamp {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Timestamp".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date-time",
            "description": "RFC 3339 UTC time with milliseconds, e.g. 2026-09-27T12:00:00.123Z."
        })
    }
}
