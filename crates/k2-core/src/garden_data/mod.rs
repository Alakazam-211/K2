//! Garden data (prd-gardens-local-services-v1 rev 3, slice 1A;
//! contract-garden-data-v1).
//!
//! A custom Garden widget gets its own keyspace (`data:<widget>`) in a
//! daemon-owned SQLite file (`~/.k2/stores/gardens.sqlite`), plus a private
//! save space (`save:<widget>`). Widgets reach it through `/cli/zen/call`
//! with their maker's rights; agents reach the same data through
//! `/cli/zen/data/*` (`k2 store --widget`) under grants a person sets per
//! agent, per K2 project (convenience-level only, Rosson 2026-10-14) and
//! per key prefix, or that a grant admin the person named sets inside its
//! delegation.
//!
//! **C0 (day-1 contract commit).** The types, limits and signatures here
//! are the shared contract; owners per file are in contract §3.1. Stubs
//! answer [`DataError::NotBuilt`] (HTTP 501 `not_built`) until their
//! builder replaces them.
//!
//! Honest bar (PRD §4.1): an agent runs as the same OS user and can open
//! `gardens.sqlite` directly. Every rule here is a guardrail against
//! mistakes and injected prompts, not a wall.

use serde::{Deserialize, Serialize};

pub mod authors;
pub mod backend;
pub mod export;
pub mod grants;
pub mod keys;
pub mod rights;
pub mod snapshots;
pub mod sqlite;
pub mod tx;

pub use backend::{Change, ChangesPage, Cursor, Entry, Filter, Json, Op, Page, ReapReport, SpaceInfo, StoreBackend};
pub use grants::{Access, EffectiveGrant, GrantFrom, GrantRow, NewGrant, SubjectKind};

// ── limits (contract §3.6) ───────────────────────────────────────────

/// Key bytes; `[A-Za-z0-9._~:@+/-]`, no leading `/`, no `..` or empty segment.
pub const MAX_KEY_BYTES: usize = 256;
/// One value, as canonical JSON text (one bridge message with its envelope).
pub const MAX_VALUE_BYTES: usize = 60 * 1024;
/// A `data:` space.
pub const MAX_SPACE_BYTES: i64 = 20 * 1024 * 1024;
pub const MAX_SPACE_KEYS: i64 = 100_000;
/// A `save:` space.
pub const MAX_SAVE_BYTES: i64 = 5 * 1024 * 1024;
pub const MAX_SAVE_KEYS: i64 = 10_000;
/// `gardens.sqlite` in all (`store_full`).
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_LIST_LIMIT: usize = 500;
pub const DEFAULT_LIST_LIMIT: usize = 100;
/// `list` and `changes` pages.
pub const PAGE_BYTES: usize = 60 * 1024;
pub const CHANGES_PAGE_ITEMS: usize = 500;
/// A batch, and one transaction part.
pub const MAX_BATCH_OPS: usize = 64;
pub const MAX_BATCH_BYTES: usize = 60 * 1024;
/// One transaction commit.
pub const MAX_TX_OPS: usize = 1_000;
pub const MAX_TX_BYTES: usize = 1024 * 1024;
pub const TX_IDLE_SECS: i64 = 30;
pub const MAX_TX_PER_PLACEMENT: usize = 2;
pub const MAX_TX_PER_AGENT: usize = 4;
pub const MAX_TX_STAGED_BYTES: usize = 16 * 1024 * 1024;
/// Per placement, and per agent per space.
pub const WRITES_PER_SEC: u32 = 60;
pub const WRITE_BURST: u32 = 120;
/// Per placement (daemon side).
pub const READS_PER_SEC: u32 = 60;
/// A commit costs `ceil(ops / TX_OPS_PER_TOKEN)` write tokens.
pub const TX_OPS_PER_TOKEN: usize = 20;
/// Committed bytes per minute, per placement or agent.
pub const COMMIT_BYTES_PER_MIN: usize = 4 * 1024 * 1024;
/// `save.*` writes per placement per 10 minutes.
pub const SAVES_PER_10_MIN: u32 = 1_200;
/// The same key and value hash this many times in the window pauses the
/// placement (`PauseReason::Writes`; Resume is person-only).
pub const RUNAWAY_SAME_WRITES: u32 = 100;
pub const RUNAWAY_WINDOW_SECS: i64 = 600;
/// Grant changes per delegate per minute.
pub const GRANT_CHANGES_PER_MIN: u32 = 30;
/// `changes?wait=` ceiling.
pub const MAX_WAIT_SECS: u32 = 30;
pub const TOMBSTONE_KEEP_SECS: i64 = 7 * 86_400;
pub const CHANGE_LOG_KEEP_SECS: i64 = 7 * 86_400;
pub const SNAPSHOT_DAILY_KEEP: usize = 7;
pub const SNAPSHOT_WEEKLY_KEEP: usize = 4;
pub const IDLE_CLOSE_SECS: u64 = 300;
/// SDK `putChunked` (one transaction).
pub const MAX_CHUNKED_BYTES: usize = 1024 * 1024;
/// Request bodies (every route but import).
pub const MAX_BODY: usize = 64 * 1024;
/// `POST /cli/zen/data/import` body.
pub const MAX_IMPORT_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_IMPORT_ITEMS: usize = 100_000;
pub const MAX_EXPORT_PAGE_ITEMS: usize = 5_000;
pub const MAX_EXPORT_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// The `/boot-status` feature key and catalog `feature` for these verbs.
pub const FEATURE: &str = "garden-data-v1";

/// The response warning on every project grant (Rosson 2026-10-14).
pub const PROJECT_GRANT_WARNING: &str = "project_grant_convenience";

// ── spaces ───────────────────────────────────────────────────────────

/// A keyspace: `data:<widget>` (shared, grantable, pushed) or
/// `save:<widget>` (private to the widget; never granted, never pushed).
/// `<widget>` is a widget folder name (`zen::valid_theme_name`) or a
/// built-in ref without its version (`k2:diary`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Space(String);

impl Space {
    pub fn data(widget: &str) -> Result<Space, DataError> {
        Ok(Space(format!("data:{}", valid_widget(widget)?)))
    }

    pub fn save(widget: &str) -> Result<Space, DataError> {
        Ok(Space(format!("save:{}", valid_widget(widget)?)))
    }

    pub fn parse(s: &str) -> Result<Space, DataError> {
        if let Some(w) = s.strip_prefix("data:") {
            Space::data(w)
        } else if let Some(w) = s.strip_prefix("save:") {
            Space::save(w)
        } else {
            Err(DataError::BadRequest(format!("'{s}' isn't a Garden data space (data:<widget> or save:<widget>)")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The widget name (`board`, `k2:diary`).
    pub fn widget(&self) -> &str {
        &self.0[5..]
    }

    pub fn is_data(&self) -> bool {
        self.0.starts_with("data:")
    }
}

impl std::fmt::Display for Space {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A widget name a space may carry: a folder name, or `k2:<name>` for a
/// built-in (any `@version` suffix is dropped by the caller first).
fn valid_widget(w: &str) -> Result<&str, DataError> {
    let name = w.strip_prefix("k2:").unwrap_or(w);
    if crate::zen::valid_theme_name(name) {
        Ok(w)
    } else {
        Err(DataError::BadRequest(format!("'{w}' isn't a widget name")))
    }
}

// ── levels, actors, makers ───────────────────────────────────────────

/// A grant level. `Write` covers `Read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Read,
    Write,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Read => "read",
            Level::Write => "write",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "read" => Some(Level::Read),
            "write" => Some(Level::Write),
            _ => None,
        }
    }

    /// True when this level allows `need`.
    pub fn covers(self, need: Level) -> bool {
        self >= need
    }
}

/// Who wrote, as stored in `entries.updated_by` / `changes.by`: stable
/// ids, never display names (BG9). The wire form is contract §5.3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The person (owner token outside K2, Settings, import).
    Owner,
    /// An agent or a K2 shell tab, by its workspace.
    Agent { project_id: String },
    /// A widget call on the person's screen.
    Widget { widget: String },
}

impl Actor {
    pub fn stored(&self) -> String {
        match self {
            Actor::Owner => "owner".to_string(),
            Actor::Agent { project_id } => format!("agent:{project_id}"),
            Actor::Widget { widget } => format!("widget:{widget}"),
        }
    }

    pub fn parse_stored(s: &str) -> Option<Actor> {
        if s == "owner" {
            return Some(Actor::Owner);
        }
        if let Some(p) = s.strip_prefix("agent:").filter(|p| !p.is_empty()) {
            return Some(Actor::Agent { project_id: p.to_string() });
        }
        s.strip_prefix("widget:").filter(|w| !w.is_empty()).map(|w| Actor::Widget { widget: w.to_string() })
    }
}

/// Who made the live version of a widget (PRD §4.2): decides what its data
/// calls may touch (`rights::rights_for_widget`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MadeBy {
    /// Owner-signed, claimed ("It's mine") or imported.
    Owner,
    /// A built-in (`k2:<name>`).
    K2,
    /// An agent (or a K2 shell tab) signed this hash.
    Agent {
        #[serde(rename = "projectId")]
        project_id: String,
    },
    /// Nobody signed this hash (or its maker workspace is gone).
    Unclaimed,
}

// ── errors ───────────────────────────────────────────────────────────

/// Every refusal the engine gives (contract §4.6). The routes turn it into
/// the `{"error": {"code", "hint", ...extra}}` envelope.
#[derive(Debug, Clone, PartialEq)]
pub enum DataError {
    BadRequest(String),
    BadKey(String),
    ValueTooLarge { bytes: usize },
    TooLarge(String),
    Conflict { key: String, rev: i64 },
    NotGranted { key: Option<String> },
    AuthorUnknown,
    PersonOnly,
    SpaceFull { what: &'static str },
    StoreFull,
    Resync { reaped_rev: i64 },
    BadTx,
    TxExpired,
    Busy,
    RateLimited { retry_after_secs: u32 },
    /// `space_not_found` | `agent_not_found` | `project_not_found` |
    /// `grant_not_found` | `widget_not_found`.
    NotFound(&'static str),
    NoSnapshot { oldest: Option<String> },
    /// The other fixed codes in contract §4.6 (`self_grant`,
    /// `level_too_high`, `not_own`, `grant_exists`, `use_admin_revoke`,
    /// `space_exists`, `confirm_required`, `owner_only`, `origin_refused`,
    /// `stale_widget`, `cap_not_granted`, `paused`, `zen_local_only`,
    /// `apps_not_allowed`, …).
    Refused { code: &'static str, status: u16, hint: String },
    Unavailable(String),
    /// C0 stubs only; never in a release.
    NotBuilt,
}

impl DataError {
    pub fn code(&self) -> &'static str {
        match self {
            DataError::BadRequest(_) => "bad_request",
            DataError::BadKey(_) => "bad_key",
            DataError::ValueTooLarge { .. } => "value_too_large",
            DataError::TooLarge(_) => "too_large",
            DataError::Conflict { .. } => "conflict",
            DataError::NotGranted { .. } => "not_granted",
            DataError::AuthorUnknown => "author_unknown",
            DataError::PersonOnly => "person_only",
            DataError::SpaceFull { .. } => "space_full",
            DataError::StoreFull => "store_full",
            DataError::Resync { .. } => "resync",
            DataError::BadTx => "bad_tx",
            DataError::TxExpired => "tx_expired",
            DataError::Busy => "busy",
            DataError::RateLimited { .. } => "rate_limited",
            DataError::NotFound(code) => code,
            DataError::NoSnapshot { .. } => "no_snapshot",
            DataError::Refused { code, .. } => code,
            DataError::Unavailable(_) => "store_unavailable",
            DataError::NotBuilt => "not_built",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            DataError::BadRequest(_) | DataError::BadKey(_) => 400,
            DataError::NotGranted { .. } | DataError::AuthorUnknown | DataError::PersonOnly => 403,
            DataError::NotFound(_) | DataError::BadTx | DataError::NoSnapshot { .. } => 404,
            DataError::Conflict { .. } | DataError::SpaceFull { .. } | DataError::Resync { .. } => 409,
            DataError::TxExpired => 410,
            DataError::ValueTooLarge { .. } | DataError::TooLarge(_) => 413,
            DataError::Busy | DataError::RateLimited { .. } => 429,
            DataError::Refused { status, .. } => *status,
            DataError::NotBuilt => 501,
            DataError::Unavailable(_) => 503,
            DataError::StoreFull => 507,
        }
    }

    /// One plain sentence. Never a value, token or path.
    pub fn hint(&self) -> String {
        match self {
            DataError::BadRequest(m) | DataError::BadKey(m) | DataError::TooLarge(m) | DataError::Unavailable(m) => {
                m.clone()
            }
            DataError::ValueTooLarge { bytes } => {
                format!("a value is at most {} KiB of JSON; this one is {bytes} bytes", MAX_VALUE_BYTES / 1024)
            }
            DataError::Conflict { key, .. } => format!("'{key}' changed since you read it; read it again and retry"),
            DataError::NotGranted { .. } => {
                "you have no grant for this key. Ask the person (k2 feedback or the Thread), or a grant admin".into()
            }
            DataError::AuthorUnknown => {
                "nobody signed this widget version. Run `k2 zen validate --widget <name>`, or click It's mine in Settings → Gardens".into()
            }
            DataError::PersonOnly => {
                "this needs a person. Run it outside K2, or use Settings → Gardens".into()
            }
            DataError::SpaceFull { what } => format!("this widget's data is full ({what})"),
            DataError::StoreFull => "Garden data on this computer is full (512 MiB)".into(),
            DataError::Resync { .. } => "your cursor is older than kept history; list again".into(),
            DataError::BadTx => "no such transaction for you".into(),
            DataError::TxExpired => "the transaction expired (30 s idle, or the daemon restarted); start again".into(),
            DataError::Busy => "too many open transactions; finish or abort one".into(),
            DataError::RateLimited { retry_after_secs } => format!("too many writes; retry in {retry_after_secs} s"),
            DataError::NotFound(code) => code.replace('_', " "),
            DataError::NoSnapshot { .. } => "no snapshot on or before that date".into(),
            DataError::Refused { hint, .. } => hint.clone(),
            DataError::NotBuilt => "not built yet (Garden data C0 stub)".into(),
        }
    }

    /// Extra envelope fields (contract §4.6).
    pub fn extra(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            DataError::ValueTooLarge { bytes } => json!({ "bytes": bytes }),
            DataError::Conflict { key, rev } => json!({ "key": key, "rev": rev }),
            DataError::NotGranted { key: Some(k) } => json!({ "key": k }),
            DataError::SpaceFull { what } => json!({ "what": what }),
            DataError::Resync { reaped_rev } => json!({ "reapedRev": reaped_rev }),
            DataError::RateLimited { retry_after_secs } => json!({ "retryAfterSecs": retry_after_secs }),
            DataError::NoSnapshot { oldest } => json!({ "oldest": oldest }),
            _ => json!({}),
        }
    }

    /// The whole error envelope.
    pub fn envelope(&self) -> serde_json::Value {
        let mut e = serde_json::json!({ "code": self.code(), "hint": self.hint() });
        if let (Some(obj), serde_json::Value::Object(extra)) = (e.as_object_mut(), self.extra()) {
            obj.extend(extra);
        }
        serde_json::json!({ "error": e })
    }
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.hint())
    }
}

impl std::error::Error for DataError {}

impl From<rusqlite::Error> for DataError {
    fn from(e: rusqlite::Error) -> Self {
        DataError::Unavailable(format!("Garden data store error: {e}"))
    }
}

/// The C0 stub answer (HTTP 501 `not_built`).
pub fn not_built() -> DataError {
    DataError::NotBuilt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_parse_and_name_their_widget() {
        let s = Space::data("board").expect("data space");
        assert_eq!(s.as_str(), "data:board");
        assert_eq!(s.widget(), "board");
        assert!(s.is_data());
        let b = Space::parse("save:k2:diary").expect("built-in save space");
        assert_eq!(b.widget(), "k2:diary");
        assert!(!b.is_data());
        assert!(Space::data("Board").is_err());
        assert!(Space::data("../x").is_err());
        assert!(Space::parse("other:board").is_err());
    }

    #[test]
    fn actors_round_trip_their_stored_form() {
        for a in [
            Actor::Owner,
            Actor::Agent { project_id: "w-1".into() },
            Actor::Widget { widget: "board".into() },
        ] {
            assert_eq!(Actor::parse_stored(&a.stored()), Some(a));
        }
        assert_eq!(Actor::parse_stored("agent:"), None);
    }

    #[test]
    fn error_envelope_carries_code_hint_and_extra() {
        let e = DataError::Conflict { key: "obj/7".into(), rev: 41 }.envelope();
        assert_eq!(e["error"]["code"], "conflict");
        assert_eq!(e["error"]["key"], "obj/7");
        assert_eq!(e["error"]["rev"], 41);
        assert_eq!(DataError::NotBuilt.status(), 501);
        assert!(Level::Write.covers(Level::Read) && !Level::Read.covers(Level::Write));
    }
}
