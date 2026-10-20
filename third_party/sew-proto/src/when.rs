//! Time input for scheduled sends, reminders and snoozes (contracts rev 26; S17 §6).
//!
//! `When` is plain data here. Turning a local time plus an IANA zone into UTC needs a time-zone
//! database, so that conversion lives in the server (rev 26 keeps `sew-proto` at its four
//! dependencies). The owning slice (S17) adds min/max bounds relative to the request time.

use chrono::NaiveDateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};
use crate::time::Timestamp;

/// The local wall-clock format: `YYYY-MM-DDTHH:MM` (no seconds, no offset).
pub const LOCAL_DATE_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M";
/// Longest IANA time-zone name accepted.
pub const TIMEZONE_MAX_LEN: usize = 64;

/// "At what time": an absolute instant, a local wall-clock time in a named zone, or a delay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum When {
    /// An absolute instant.
    At {
        /// RFC 3339 with `Z` or an offset.
        at: Timestamp,
    },
    /// A local wall-clock time.
    Local {
        /// `YYYY-MM-DDTHH:MM`, no seconds, no offset.
        date_time: String,
        /// An IANA zone name; absent means the member's profile zone, then UTC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    /// A delay from the request's clock time.
    In {
        /// Milliseconds from now.
        ms: u64,
    },
}

/// Whether `s` looks like an IANA zone name (`Area/City`, `UTC`, `Etc/GMT+5`). Whether the
/// zone exists is checked by the server.
pub fn is_timezone_name(s: &str) -> bool {
    (1..=TIMEZONE_MAX_LEN).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'+'))
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.contains("//")
}

impl When {
    /// The parsed local time for `Local`.
    pub fn local_date_time(&self) -> Option<NaiveDateTime> {
        match self {
            When::Local { date_time, .. } => {
                NaiveDateTime::parse_from_str(date_time, LOCAL_DATE_TIME_FORMAT).ok()
            }
            _ => None,
        }
    }
}

impl Validate for When {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            When::At { .. } | When::In { .. } => Ok(()),
            When::Local {
                date_time,
                timezone,
            } => {
                if date_time.len() != 16 || self.local_date_time().is_none() {
                    return Err(ValidationError::invalid(
                        "date_time",
                        "bad_local_time: expected YYYY-MM-DDTHH:MM",
                    ));
                }
                if let Some(tz) = timezone
                    && !is_timezone_name(tz)
                {
                    return Err(ValidationError::invalid(
                        "timezone",
                        "bad_timezone: expected an IANA zone name",
                    ));
                }
                Ok(())
            }
        }
    }
}
