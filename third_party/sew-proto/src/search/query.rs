//! The search query parser (S20 §9). Pure: no clock, no IO, no tokenizer.
//!
//! ```text
//! query   := ws? clause (ws clause)* ws?
//! clause  := neg? ( filter | phrase | prefix | word )
//! neg     := "-"                          ; only at clause start, followed by a non-space
//! filter  := NAME ":" value               ; NAME is a known filter name, case-insensitive
//! phrase  := QUOTE any* (QUOTE | end)     ; QUOTE is " or “ or ”; unclosed runs to the end
//! prefix  := wordchars "*"
//! word    := any run of non-space chars that is none of the above
//! ```
//!
//! - `NAME ∈ {from, in, space, has, mentions, on, before, after, ext, is}`. Any other `x:y` is a
//!   plain word, so `10:30`, `re:` and URLs stay text.
//! - Words and phrases with no word or CJK character (pure punctuation, a lone `-`, `*`) are
//!   dropped: they tokenize to nothing.
//! - The values of `from:`, `in:`, `space:` and `mentions:` are resolved by the server
//!   ([`parse_member_ref`], [`parse_in_ref`], [`parse_space_ref`] give their shapes); `has:`,
//!   `is:`, `ext:` and the date filters are checked here.
//!
//! Every error carries the byte offset in `q` of the clause it is about (`empty_query`: 0).

use std::fmt;
use std::ops::Range;

use chrono::{Duration, NaiveDate};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::{Addr, Handle, Slug};
use crate::api::search::SearchErrorReason;
use crate::id::{Id, parse_id_str};
use crate::limits::{
    GROUP_DM_MAX_OTHERS, SEARCH_FILTER_VALUES_MAX, SEARCH_FILTERS_MAX, SEARCH_PREFIX_MIN_CHARS,
    SEARCH_Q_MAX_BYTES, SEARCH_TEXT_CLAUSES_MAX,
};

// ---------------------------------------------------------------- character classes

/// A CJK character (S20 §6.4): Han `U+3400–4DBF`, `U+4E00–9FFF`, `U+F900–FAFF`,
/// `U+20000–2A6DF`; Hiragana `U+3040–309F`; Katakana `U+30A0–30FF`, `U+31F0–31FF`; Hangul
/// `U+1100–11FF`, `U+3130–318F`, `U+AC00–D7AF`. The tokenizer makes character bigrams of runs.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x2_0000..=0x2_A6DF
        | 0x3040..=0x309F
        | 0x30A0..=0x30FF
        | 0x31F0..=0x31FF
        | 0x1100..=0x11FF
        | 0x3130..=0x318F
        | 0xAC00..=0xD7AF)
}

/// A word character: alphanumeric and not CJK. Everything else that is not CJK separates
/// tokens (spaces, punctuation, `_`, `'`, `-`, `.`, emoji).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && !is_cjk(c)
}

/// Whether `s` holds at least one word or CJK character (it would make at least one token).
pub fn tokenizes_to_something(s: &str) -> bool {
    s.chars().any(|c| is_word_char(c) || is_cjk(c))
}

// ---------------------------------------------------------------- types

unit_enum! {
    /// A search filter name (case-insensitive in queries).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum FilterName {
        /// The author (for files, the uploader).
        From = "from",
        /// A channel, DM, group DM or thread.
        In = "in",
        /// A space.
        Space = "space",
        /// `file` or `link`.
        Has = "has",
        /// A direct member mention in the current version.
        Mentions = "mentions",
        /// One local day.
        On = "on",
        /// Strictly before a local day.
        Before = "before",
        /// Strictly after a local day.
        After = "after",
        /// A file extension (file search only).
        Ext = "ext",
        /// What a message is: `transcript` (S35 v2 §16).
        Is = "is",
    }
}

impl FilterName {
    /// The date filters (`on:`, `before:`, `after:`), which cannot be negated.
    pub const fn is_date(self) -> bool {
        matches!(
            self,
            FilterName::On | FilterName::Before | FilterName::After
        )
    }

    fn lookup(name: &str) -> Option<FilterName> {
        FilterName::ALL
            .iter()
            .copied()
            .find(|f| f.as_str().eq_ignore_ascii_case(name))
    }
}

unit_enum! {
    /// A `has:` value.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum HasValue {
        /// The message has at least one attachment.
        File = "file",
        /// The body or forwarded body holds `http://` or `https://`.
        Link = "link",
    }
}

unit_enum! {
    /// An `is:` value (S35 v2 §16).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum IsValue {
        /// A call transcript turn.
        Transcript = "transcript",
    }
}

/// What a clause is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClauseKind {
    /// A word (may tokenize to several tokens, which then match as a phrase).
    Word(String),
    /// `word*`: the text before the `*`.
    Prefix(String),
    /// A quoted phrase: the inner text, quotes stripped.
    Phrase(String),
    /// `name:value`: the raw text after the `:`.
    Filter {
        /// The filter.
        name: FilterName,
        /// The raw value.
        value: String,
    },
}

/// One clause of a query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Clause {
    /// Led by `-`.
    pub negated: bool,
    /// What it is.
    pub kind: ClauseKind,
    /// Byte span in `q`, including the `-` and the quotes.
    pub span: Range<usize>,
}

impl Clause {
    /// A word, prefix or phrase.
    pub fn is_text(&self) -> bool {
        !matches!(self.kind, ClauseKind::Filter { .. })
    }

    /// The filter name, for a filter.
    pub fn filter_name(&self) -> Option<FilterName> {
        match self.kind {
            ClauseKind::Filter { name, .. } => Some(name),
            _ => None,
        }
    }

    /// The filter value, for a filter.
    pub fn filter_value(&self) -> Option<&str> {
        match &self.kind {
            ClauseKind::Filter { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// A parsed, checked query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParsedQuery {
    /// The clauses in query order (clauses that tokenize to nothing are already dropped).
    pub clauses: Vec<Clause>,
}

impl ParsedQuery {
    /// Whether a non-negated word, prefix or phrase exists (else the sort is `recent`).
    pub fn has_positive_text(&self) -> bool {
        self.clauses.iter().any(|c| c.is_text() && !c.negated)
    }

    /// The word, prefix and phrase clauses.
    pub fn text(&self) -> impl Iterator<Item = &Clause> {
        self.clauses.iter().filter(|c| c.is_text())
    }

    /// The clauses of one filter name.
    pub fn filters(&self, name: FilterName) -> impl Iterator<Item = &Clause> {
        self.clauses
            .iter()
            .filter(move |c| c.filter_name() == Some(name))
    }

    /// The date filters: every `on:` day, the earliest `before:` and the latest `after:`.
    pub fn dates(&self) -> Result<DateFilter, QueryError> {
        let mut out = DateFilter::default();
        for c in &self.clauses {
            let (Some(name), Some(value)) = (c.filter_name(), c.filter_value()) else {
                continue;
            };
            if !name.is_date() {
                continue;
            }
            let d = parse_date(value)
                .ok_or_else(|| QueryError::new(SearchErrorReason::BadDate, c.span.start))?;
            match name {
                FilterName::On => out.on.push(d),
                FilterName::Before => out.before = Some(out.before.map_or(d, |b| b.min(d))),
                FilterName::After => out.after = Some(out.after.map_or(d, |a| a.max(d))),
                _ => {}
            }
        }
        out.on.sort_unstable();
        out.on.dedup();
        Ok(out)
    }
}

/// The date filters of a query, as local calendar days (the server maps them to instants in
/// the searcher's time zone).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct DateFilter {
    /// `on:` days, ascending (they OR together).
    pub on: Vec<NaiveDate>,
    /// The earliest `before:` day (excluded).
    pub before: Option<NaiveDate>,
    /// The latest `after:` day (excluded).
    pub after: Option<NaiveDate>,
}

impl DateFilter {
    /// No date filter at all.
    pub fn is_empty(&self) -> bool {
        self.on.is_empty() && self.before.is_none() && self.after.is_none()
    }

    /// Whether day `d` passes `before:` and `after:`.
    pub fn allows(&self, d: NaiveDate) -> bool {
        self.before.is_none_or(|b| d < b) && self.after.is_none_or(|a| d > a)
    }

    /// Whether no day at all passes (tz-independent: local midnights are monotonic).
    pub fn is_empty_range(&self) -> bool {
        if let (Some(a), Some(b)) = (self.after, self.before)
            && a.checked_add_signed(Duration::days(1))
                .is_none_or(|first| first >= b)
        {
            return true;
        }
        !self.on.is_empty() && !self.on.iter().any(|d| self.allows(*d))
    }
}

/// A parse error: the reason and the byte offset in `q` of the clause it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryError {
    /// Why.
    pub reason: SearchErrorReason,
    /// Byte offset in `q`.
    pub position: usize,
}

impl QueryError {
    /// An error.
    pub const fn new(reason: SearchErrorReason, position: usize) -> QueryError {
        QueryError { reason, position }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.position)
    }
}

impl std::error::Error for QueryError {}

// ---------------------------------------------------------------- the parser

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '“' | '”')
}

/// Splits `q` into raw clauses (no checks, nothing dropped).
fn scan(q: &str, start: usize, end: usize) -> Vec<Clause> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        let Some(c) = q[i..end].chars().next() else {
            break;
        };
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let clause_start = i;
        let mut negated = false;
        if c == '-' {
            let next = q[i + 1..end].chars().next();
            if next.is_some_and(|n| !n.is_whitespace()) {
                negated = true;
                i += 1;
            }
        }
        let first = q[i..end].chars().next();
        if let Some(open) = first.filter(|c| is_quote(*c)) {
            let inner_start = i + open.len_utf8();
            let (inner_end, clause_end) = match q[inner_start..end]
                .char_indices()
                .find(|(_, c)| is_quote(*c))
            {
                Some((off, close)) => (inner_start + off, inner_start + off + close.len_utf8()),
                None => (end, end),
            };
            out.push(Clause {
                negated,
                kind: ClauseKind::Phrase(q[inner_start..inner_end].to_owned()),
                span: clause_start..clause_end,
            });
            i = clause_end;
            continue;
        }
        let run_end = q[i..end]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map_or(end, |(off, _)| i + off);
        let token = &q[i..run_end];
        let filter = token
            .split_once(':')
            .and_then(|(n, v)| FilterName::lookup(n).map(|f| (f, v)));
        let kind = match filter {
            Some((name, value)) => ClauseKind::Filter {
                name,
                value: value.to_owned(),
            },
            None => match token.strip_suffix('*') {
                Some(p) if !p.is_empty() => ClauseKind::Prefix(p.to_owned()),
                _ => ClauseKind::Word(token.to_owned()),
            },
        };
        out.push(Clause {
            negated,
            kind,
            span: clause_start..run_end,
        });
        i = run_end;
    }
    out
}

/// Checks one clause; `Ok(false)` drops it (it tokenizes to nothing).
fn check(c: &Clause) -> Result<bool, QueryError> {
    let at = c.span.start;
    let err = |r| Err(QueryError::new(r, at));
    match &c.kind {
        ClauseKind::Word(w) | ClauseKind::Phrase(w) => Ok(tokenizes_to_something(w)),
        ClauseKind::Prefix(p) => {
            if !tokenizes_to_something(p) {
                return Ok(false);
            }
            let chars = p.chars().count();
            if !p.chars().all(is_word_char) || chars < SEARCH_PREFIX_MIN_CHARS {
                return err(SearchErrorReason::PrefixTooShort);
            }
            Ok(true)
        }
        ClauseKind::Filter { name, value } => {
            if value.is_empty() {
                return err(SearchErrorReason::EmptyFilterValue);
            }
            match name {
                FilterName::Has if parse_has_value(value).is_none() => {
                    err(SearchErrorReason::UnknownHasValue)
                }
                FilterName::Is if parse_is_value(value).is_none() => {
                    err(SearchErrorReason::UnknownIsValue)
                }
                FilterName::Ext if normalize_ext(value).is_none() => err(SearchErrorReason::BadExt),
                n if n.is_date() && c.negated => err(SearchErrorReason::FilterNotNegatable),
                n if n.is_date() && parse_date(value).is_none() => err(SearchErrorReason::BadDate),
                _ => Ok(true),
            }
        }
    }
}

/// Parses and checks a query (S20 §9, §14.4).
pub fn parse(q: &str) -> Result<ParsedQuery, QueryError> {
    let lead = q.len() - q.trim_start().len();
    let trimmed = q.trim();
    if trimmed.len() > SEARCH_Q_MAX_BYTES {
        return Err(QueryError::new(
            SearchErrorReason::QueryTooLong,
            lead + SEARCH_Q_MAX_BYTES,
        ));
    }
    let end = lead + trimmed.len();
    let mut clauses = Vec::new();
    for c in scan(q, lead, end) {
        if check(&c)? {
            clauses.push(c);
        }
    }
    let mut text = 0usize;
    let mut filters = 0usize;
    let mut per_name = [0usize; FilterName::ALL.len()];
    for c in &clauses {
        match c.filter_name() {
            None => {
                text += 1;
                if text > SEARCH_TEXT_CLAUSES_MAX {
                    return Err(QueryError::new(
                        SearchErrorReason::TooManyTerms,
                        c.span.start,
                    ));
                }
            }
            Some(name) => {
                filters += 1;
                let slot = FilterName::ALL.iter().position(|f| *f == name).unwrap_or(0);
                per_name[slot] += 1;
                if filters > SEARCH_FILTERS_MAX || per_name[slot] > SEARCH_FILTER_VALUES_MAX {
                    return Err(QueryError::new(
                        SearchErrorReason::TooManyFilters,
                        c.span.start,
                    ));
                }
            }
        }
    }
    if !clauses.iter().any(|c| !c.negated) {
        return Err(QueryError::new(SearchErrorReason::EmptyQuery, 0));
    }
    let parsed = ParsedQuery { clauses };
    let dates = parsed.dates()?;
    if dates.is_empty_range() {
        let last = parsed
            .clauses
            .iter()
            .filter(|c| c.filter_name().is_some_and(FilterName::is_date))
            .map(|c| c.span.start)
            .next_back()
            .unwrap_or(0);
        return Err(QueryError::new(SearchErrorReason::EmptyDateRange, last));
    }
    Ok(parsed)
}

// ---------------------------------------------------------------- value shapes

/// A `from:` or `mentions:` value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemberRef {
    /// `me`: the caller.
    Me,
    /// `@handle` or `handle`, lowercased: an active member by username.
    Handle(String),
    /// `<@ID>` or `id:ID`: any member, deactivated included.
    Id(Id),
}

/// Parses `me`, `@handle`, `handle`, `<@ID>` or `id:ID`. `None` for any other shape (the
/// server answers `unknown_filter_value`).
pub fn parse_member_ref(v: &str) -> Option<MemberRef> {
    if v.eq_ignore_ascii_case("me") {
        return Some(MemberRef::Me);
    }
    if let Some(id) = v.strip_prefix("<@").and_then(|r| r.strip_suffix('>')) {
        return parse_id_str(id).map(MemberRef::Id);
    }
    if let Some(id) = strip_id_prefix(v) {
        return parse_id_str(id).map(MemberRef::Id);
    }
    handle_of(v.strip_prefix('@').unwrap_or(v)).map(MemberRef::Handle)
}

/// An `in:` value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InRef {
    /// A C4 address (`acme/#general`, `acme/#general/<root>`, `dm/@carol`, `dm/id:…`).
    Addr(Addr),
    /// `#slug` or `slug`: a channel slug in the caller's spaces (or the one `space:`).
    Slug(String),
    /// `@handle`: the existing 1:1 DM with that member.
    Dm(String),
    /// `@a,@b[,…]`: the existing group DM with exactly the caller plus 2–8 listed members.
    GroupDm(Vec<String>),
    /// `id:ID`: a channel or DM by id.
    Id(Id),
}

/// Parses an `in:` value (lowercased first; ids are digits). `None` for any other shape.
pub fn parse_in_ref(v: &str) -> Option<InRef> {
    if let Some(id) = strip_id_prefix(v) {
        return parse_id_str(id).map(InRef::Id);
    }
    let v = v.to_ascii_lowercase();
    if v.contains('/') {
        return v.parse::<Addr>().ok().map(InRef::Addr);
    }
    if let Some(s) = v.strip_prefix('#') {
        return Slug::new(s)
            .ok()
            .map(|s| InRef::Slug(s.as_str().to_owned()));
    }
    if let Some(rest) = v.strip_prefix('@') {
        if rest.contains(',') {
            let mut handles = Vec::new();
            for part in rest.split(',') {
                handles.push(handle_of(part.strip_prefix('@').unwrap_or(part))?);
            }
            if !(2..=GROUP_DM_MAX_OTHERS).contains(&handles.len()) {
                return None;
            }
            return Some(InRef::GroupDm(handles));
        }
        return handle_of(rest).map(InRef::Dm);
    }
    Slug::new(&v)
        .ok()
        .map(|s| InRef::Slug(s.as_str().to_owned()))
}

/// A `space:` value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpaceRefQ {
    /// A space slug, lowercased.
    Slug(String),
    /// `id:ID`.
    Id(Id),
}

/// Parses a `space:` value (`slug` or `id:ID`). `None` for any other shape.
pub fn parse_space_ref(v: &str) -> Option<SpaceRefQ> {
    if let Some(id) = strip_id_prefix(v) {
        return parse_id_str(id).map(SpaceRefQ::Id);
    }
    Slug::new(&v.to_ascii_lowercase())
        .ok()
        .map(|s| SpaceRefQ::Slug(s.as_str().to_owned()))
}

/// Parses a `has:` value, case-insensitive.
pub fn parse_has_value(v: &str) -> Option<HasValue> {
    HasValue::from_wire(&v.to_ascii_lowercase())
}

/// Parses an `is:` value, case-insensitive.
pub fn parse_is_value(v: &str) -> Option<IsValue> {
    IsValue::from_wire(&v.to_ascii_lowercase())
}

/// A strict `YYYY-MM-DD` calendar day.
pub fn parse_date(v: &str) -> Option<NaiveDate> {
    let b = v.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: Range<usize>| -> Option<u32> {
        let s = &v[r];
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let y = i32::try_from(digits(0..4)?).ok()?;
    NaiveDate::from_ymd_opt(y, digits(5..7)?, digits(8..10)?)
}

/// An `ext:` value: lowercased, one leading `.` stripped, `[a-z0-9]{1,16}`.
pub fn normalize_ext(v: &str) -> Option<String> {
    let v = v.to_ascii_lowercase();
    let v = v.strip_prefix('.').unwrap_or(&v);
    let ok = (1..=16).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    ok.then(|| v.to_owned())
}

fn strip_id_prefix(v: &str) -> Option<&str> {
    let (p, rest) = v.split_at_checked(3)?;
    p.eq_ignore_ascii_case("id:").then_some(rest)
}

fn handle_of(s: &str) -> Option<String> {
    Handle::new(&s.to_ascii_lowercase())
        .ok()
        .map(|h| h.as_str().to_owned())
}
