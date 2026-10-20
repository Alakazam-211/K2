//! HTTP conventions shared by the client and mgmt APIs (C5, C8, gaps G6, G28).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Validate, ValidationError};
use crate::id::Id;
use crate::limits::{
    CHANGES_LIMIT_DEFAULT, CHANGES_LIMIT_MAX, IDEMPOTENCY_KEY_MAX_LEN, IDEMPOTENCY_KEY_MIN_LEN,
    NONCE_MAX_CHARS, PAGE_LIMIT_DEFAULT, PAGE_LIMIT_MAX,
};

/// Request header: idempotency key for creating `POST`s (required on every mgmt `POST`).
pub const IDEMPOTENCY_KEY: &str = "Idempotency-Key";
/// Response header: `true` when the response is a stored replay.
pub const IDEMPOTENT_REPLAYED: &str = "Idempotent-Replayed";
/// Response header: the bucket size.
pub const RATE_LIMIT_LIMIT: &str = "X-RateLimit-Limit";
/// Response header: requests left in the bucket.
pub const RATE_LIMIT_REMAINING: &str = "X-RateLimit-Remaining";
/// Response header: milliseconds until the bucket refills.
pub const RATE_LIMIT_RESET_MS: &str = "X-RateLimit-Reset-Ms";
/// Response header on 429: seconds to wait.
pub const RETRY_AFTER: &str = "Retry-After";
/// Request header for conditional reads.
pub const IF_NONE_MATCH: &str = "If-None-Match";
/// Response header carrying a version tag.
pub const ETAG: &str = "ETag";
/// Mgmt act-as header: the member id or external id acted as (C9).
pub const SEW_ACT_AS: &str = "Sew-Act-As";
/// Mgmt act-as header: `k2; agent=<uuid>; k2_session=<id>` (C9, gap G31).
pub const SEW_VIA: &str = "Sew-Via";
/// Optional mgmt header naming the K2 person behind a call, for audit only (contracts rev 13,
/// S27 G8): 1–128 chars of `[A-Za-z0-9_.:@-]`, stored as `details.k2_actor`.
pub const SEW_ACTOR: &str = "Sew-Actor";

/// An `Idempotency-Key` value: 1–64 chars of `[A-Za-z0-9_-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

/// Error parsing an [`IdempotencyKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKeyError(String);

impl fmt::Display for IdempotencyKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid Idempotency-Key {:?}: expected 1–64 chars of [A-Za-z0-9_-]",
            self.0
        )
    }
}

impl std::error::Error for IdempotencyKeyError {}

impl IdempotencyKey {
    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for IdempotencyKey {
    type Err = IdempotencyKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ok = (IDEMPOTENCY_KEY_MIN_LEN..=IDEMPOTENCY_KEY_MAX_LEN).contains(&s.len())
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
        if ok {
            Ok(IdempotencyKey(s.to_owned()))
        } else {
            Err(IdempotencyKeyError(s.to_owned()))
        }
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for IdempotencyKey {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "IdempotencyKey".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[A-Za-z0-9_-]{1,64}$",
            "description": "An idempotency key."
        })
    }
}

/// A client-chosen message nonce, echoed back in the response and the `message_created` event.
///
/// Valid nonces are 1–64 Unicode scalar values. Deserialization accepts any string so the
/// request validator can report `validation_failed` on the `nonce` field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Nonce(String);

impl Nonce {
    /// Wraps a string without checking it; see [`Nonce::check`].
    pub fn new(s: impl Into<String>) -> Self {
        Nonce(s.into())
    }

    /// The nonce text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks the 1–64 character rule, reporting `field` on failure.
    pub fn check(&self, field: &str) -> Result<(), ValidationError> {
        let n = self.0.chars().count();
        if n == 0 || n > NONCE_MAX_CHARS {
            return Err(ValidationError::invalid(
                field,
                format!("{field} must be 1–{NONCE_MAX_CHARS} characters, got {n}"),
            ));
        }
        Ok(())
    }
}

impl Serialize for Nonce {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Nonce {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Nonce)
    }
}

impl JsonSchema for Nonce {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "Nonce".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": 64,
            "description": "A client nonce, 1–64 characters."
        })
    }
}

/// Query for id-keyed lists: at most one cursor (gap G6) and an optional `limit` (1–100,
/// default 50). No cursor means the newest page, newest-first. `before` pages newest-first,
/// `after` oldest-first, `around` centered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    /// Items older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Items newer than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Items around this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Which page a [`PageQuery`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cursor {
    /// No cursor: the newest page, newest-first.
    Newest,
    /// Older than the id, newest-first.
    Before(Id),
    /// Newer than the id, oldest-first.
    After(Id),
    /// Centered on the id.
    Around(Id),
}

impl PageQuery {
    /// The cursor. With more than one set (invalid; see `validate`), `before` wins, then `after`.
    pub fn cursor(&self) -> Cursor {
        match (self.before, self.after, self.around) {
            (Some(id), _, _) => Cursor::Before(id),
            (None, Some(id), _) => Cursor::After(id),
            (None, None, Some(id)) => Cursor::Around(id),
            (None, None, None) => Cursor::Newest,
        }
    }

    /// The page size, default 50.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(PAGE_LIMIT_DEFAULT)
    }
}

impl Validate for PageQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        let set = [self.before, self.after, self.around]
            .iter()
            .filter(|c| c.is_some())
            .count();
        if set > 1 {
            return Err(ValidationError::invalid(
                "before",
                "at most one of before, after, around",
            ));
        }
        if let Some(l) = self.limit
            && !(1..=PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

/// A page of an id-keyed list: `{"items": [...], "has_more": bool}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Page<T> {
    /// The items.
    pub items: Vec<T>,
    /// Whether more items exist past this page.
    pub has_more: bool,
}

/// Query for change-log catch-up (C8): `since` a seq, `limit` 1–500 (default 500, gap G28).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesQuery {
    /// Return entries with `seq > since`.
    pub since: u64,
    /// Page size, 1–500 (default 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ChangesQuery {
    /// The page size, default 500.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(CHANGES_LIMIT_DEFAULT)
    }
}

impl Validate for ChangesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.limit
            && !(1..=CHANGES_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{CHANGES_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

/// Body of a bodiless `POST`. The server accepts an empty body or `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}

impl Validate for EmptyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response for mutations with no canonical object: `{"ok": true}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OkResponse {
    /// Always `true`.
    pub ok: bool,
}

impl Default for OkResponse {
    fn default() -> Self {
        OkResponse { ok: true }
    }
}

/// A tri-state update field: absent = [`Patch::Keep`], `null` = [`Patch::Clear`], a value =
/// [`Patch::Set`]. Fields use `#[serde(default, skip_serializing_if = "Patch::is_keep")]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Patch<T> {
    /// Leave the value as it is.
    #[default]
    Keep,
    /// Remove the value.
    Clear,
    /// Replace the value.
    Set(T),
}

impl<T> Patch<T> {
    /// Whether this is `Keep` (used by `skip_serializing_if`).
    pub fn is_keep(&self) -> bool {
        matches!(self, Patch::Keep)
    }

    /// The new value, if one is set.
    pub fn as_set(&self) -> Option<&T> {
        match self {
            Patch::Set(v) => Some(v),
            _ => None,
        }
    }

    /// Applies the patch to a current optional value.
    pub fn apply(self, current: Option<T>) -> Option<T> {
        match self {
            Patch::Keep => current,
            Patch::Clear => None,
            Patch::Set(v) => Some(v),
        }
    }
}

impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Patch::Keep | Patch::Clear => serializer.serialize_none(),
            Patch::Set(v) => serializer.serialize_some(v),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(v) => Patch::Set(v),
            None => Patch::Clear,
        })
    }
}

impl<T: JsonSchema> JsonSchema for Patch<T> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("Patch_{}", T::schema_name()).into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = generator.subschema_for::<Option<T>>();
        schema.insert(
            "description".to_owned(),
            "Absent keeps the value, null clears it, a value replaces it.".into(),
        );
        schema
    }
}
