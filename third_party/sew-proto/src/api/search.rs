//! Search request and response bodies (S20 §14; contracts rev 27).
//!
//! Both search routes are `POST` reads (`/api/v1/search/messages`, `/api/v1/search/files`): the
//! query text is content and stays out of URLs and access logs. Pages use signed opaque cursors
//! (a C5 exception): a request with a `cursor` repeats page 1's `q`, `sort`, `tz`,
//! `include_unjoined_public` and `include_archived`. Errors are `400 validation_failed` or
//! `503 unavailable` with a [`SearchErrorReason`] in `details.reason`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::channels::ChannelType;
use crate::api::messages::Message;
use crate::error::{Validate, ValidationError};
use crate::id::Id;
use crate::limits::{SEARCH_PAGE_LIMIT_DEFAULT, SEARCH_PAGE_LIMIT_MAX};
use crate::time::Timestamp;

unit_enum! {
    /// Result order.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SearchSort {
        /// BM25 blended with a 30-day recency boost (the default).
        Relevance = "relevance",
        /// Newest first. Filter-only queries always sort this way.
        Recent = "recent",
    }
}

unit_enum! {
    /// `details.reason` of a search error (S20 §14.4). `400 validation_failed` carries
    /// `details.field` (`q` unless noted) and, for parser reasons, `details.position` (a byte
    /// offset in `q`); the last three are `503 unavailable` reasons.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SearchErrorReason {
        /// No positive clause (empty, punctuation only, or negative-only).
        EmptyQuery = "empty_query",
        /// `q` is longer than 1,000 bytes after trimming.
        QueryTooLong = "query_too_long",
        /// More than 32 text clauses.
        TooManyTerms = "too_many_terms",
        /// More than 20 filters, or more than 10 values of one filter name.
        TooManyFilters = "too_many_filters",
        /// A known filter name with nothing after the `:`.
        EmptyFilterValue = "empty_filter_value",
        /// A date that is not a real `YYYY-MM-DD` day.
        BadDate = "bad_date",
        /// The date filters leave no day at all.
        EmptyDateRange = "empty_date_range",
        /// `on:`, `before:` and `after:` cannot be negated.
        FilterNotNegatable = "filter_not_negatable",
        /// `has:` takes `file` or `link`.
        UnknownHasValue = "unknown_has_value",
        /// `is:` takes `transcript` (S35 v2 §16).
        UnknownIsValue = "unknown_is_value",
        /// `ext:` takes `[a-z0-9]{1,16}` with an optional leading `.`.
        BadExt = "bad_ext",
        /// The filter is not supported on this route (`ext:` on messages).
        FilterNotSupported = "filter_not_supported",
        /// A `word*` prefix shorter than 2 characters, or not one plain word.
        PrefixTooShort = "prefix_too_short",
        /// A `word*` prefix matches more than 1,000 distinct terms.
        PrefixTooBroad = "prefix_too_broad",
        /// A filter value that does not exist or that the caller cannot see
        /// (`details.filter`, `details.value`; hidden and missing are byte-identical).
        UnknownFilterValue = "unknown_filter_value",
        /// `in:#slug` names channels in more than one space (`details.candidates`).
        AmbiguousFilterValue = "ambiguous_filter_value",
        /// `tz` is not an IANA time zone (`field: tz`).
        UnknownTimezone = "unknown_timezone",
        /// The cursor is not one this server signed (`field: cursor`).
        CursorInvalid = "cursor_invalid",
        /// The cursor is older than 30 minutes (`field: cursor`).
        CursorExpired = "cursor_expired",
        /// The cursor belongs to another member, route or query (`field: cursor`).
        CursorMismatch = "cursor_mismatch",
        /// `503`: the index is being rebuilt (`details.progress_pct`, `Retry-After: 30`).
        SearchRebuilding = "search_rebuilding",
        /// `503`: too many concurrent searches (`Retry-After: 1`).
        SearchBusy = "search_busy",
        /// `503`: the query ran past its deadline.
        SearchTimeout = "search_timeout",
    }
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Body of `POST /api/v1/search/messages` and `POST /api/v1/search/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    /// The query (1–1,000 bytes after trimming; syntax in `docs/search.md`).
    pub q: String,
    /// `relevance` (default) or `recent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SearchSort>,
    /// Page size 1–100, default 20. Pages may be shorter; follow `has_more`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// From a previous response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// IANA time zone for date filters; default the caller's profile zone, else `UTC`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// Also search public channels the caller has not joined (ignored for guests).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_unjoined_public: bool,
    /// Include archived channels (default true; hits are marked).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub include_archived: bool,
}

impl SearchRequest {
    /// A request for `q` with every default.
    pub fn new(q: impl Into<String>) -> SearchRequest {
        SearchRequest {
            q: q.into(),
            sort: None,
            limit: None,
            cursor: None,
            tz: None,
            include_unjoined_public: false,
            include_archived: true,
        }
    }

    /// `limit`, or 20.
    pub fn limit_or_default(&self) -> u32 {
        self.limit.unwrap_or(SEARCH_PAGE_LIMIT_DEFAULT)
    }

    /// `sort`, or `relevance`.
    pub fn sort_or_default(&self) -> SearchSort {
        self.sort.unwrap_or(SearchSort::Relevance)
    }
}

impl Validate for SearchRequest {
    /// Checks `limit`. The query itself is checked by
    /// [`crate::search::query::parse`], which names the reason and position.
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(l) = self.limit
            && !(1..=SEARCH_PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{SEARCH_PAGE_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

/// Byte ranges `[start, end)` of matching tokens in the current text (at most 50 per field;
/// filter-only queries have none).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Highlights {
    /// In `message.body`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<[u32; 2]>,
    /// In `message.forwarded.body`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwarded_body: Vec<[u32; 2]>,
    /// In `file.name` (file hits).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name: Vec<[u32; 2]>,
}

/// One message hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchMessageHit {
    /// The message, current as of the read.
    pub message: Message,
    /// The ranking score (`relevance` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// Where the query matched.
    pub highlights: Highlights,
}

/// A channel that holds at least one hit on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchChannelRef {
    /// The channel id.
    pub id: Id,
    /// Its space; absent for DMs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// The channel type (`dm`, `group_dm` for DMs).
    #[serde(rename = "type")]
    pub kind: ChannelType,
    /// Slug (space channels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Name (space channels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Archived channels are included by default and marked.
    pub archived: bool,
    /// The caller (or the assistant's person) is a member.
    pub joined: bool,
    /// DM members, ascending (DMs only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dm_member_ids: Vec<Id>,
}

/// Response of `POST /api/v1/search/messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchMessagesResponse {
    /// Hits in order.
    pub items: Vec<SearchMessageHit>,
    /// One entry per distinct channel in `items`.
    pub channels: Vec<SearchChannelRef>,
    /// The sort actually used.
    pub sort: SearchSort,
    /// The fixed time relevance is computed against (page 1's).
    pub as_of: Timestamp,
    /// Index matches inside the caller's scope before the re-check.
    pub approx_total: u64,
    /// More pages may follow.
    pub has_more: bool,
    /// Pass back for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A file as search shows it (from the S15 attachment seam).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchFileRef {
    /// The attachment.
    pub attachment_id: Id,
    /// Its message.
    pub message_id: Id,
    /// Its channel.
    pub channel_id: Id,
    /// The file name.
    pub name: String,
    /// Lowercase extension without the dot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    /// MIME type.
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
}

/// One file hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchFileHit {
    /// The file.
    pub file: SearchFileRef,
    /// The message that carries it, current as of the read.
    pub message: Message,
    /// The ranking score (`relevance` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// Where the query matched (`name`).
    pub highlights: Highlights,
}

/// Response of `POST /api/v1/search/files`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchFilesResponse {
    /// Hits in order.
    pub items: Vec<SearchFileHit>,
    /// One entry per distinct channel in `items`.
    pub channels: Vec<SearchChannelRef>,
    /// The sort actually used.
    pub sort: SearchSort,
    /// The fixed time relevance is computed against (page 1's).
    pub as_of: Timestamp,
    /// Index matches inside the caller's scope before the re-check.
    pub approx_total: u64,
    /// More pages may follow.
    pub has_more: bool,
    /// Pass back for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
