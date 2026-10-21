//! Requests: ids, states, the stored row (migration 0218), the `permission`
//! object a refusal carries, error codes and limits (PRD rev 3.1 §3, §6.2,
//! §6.6, §17; vs-live C12, C35).
//!
//! P0: types, constants and pure helpers only. The store, limits and
//! routes are lane A1's (`k2-daemon/src/permission/`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Request id prefix (`pr_7f3a2c…`). Held-request actions use the held
/// request's own id instead (`ar_22`, §3.2).
pub const ID_PREFIX: &str = "pr_";

/// A permission request id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    /// A fresh id: `pr_` + 12 lowercase hex.
    pub fn new() -> Self {
        let hex = uuid::Uuid::new_v4().simple().to_string();
        RequestId(format!("{ID_PREFIX}{}", &hex[..12]))
    }

    /// `pr_` + 6 to 32 lowercase hex characters.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix(ID_PREFIX)?;
        let ok = (6..=32).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        ok.then(|| RequestId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a request stands; also `view.state` (§6.2, §17.2.4). The
/// `permission_requests.state` column. `pending → applying` is a
/// compare-and-set in one SQLite transaction before any apply (C12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    /// Pending, but the current Settings state already matches (Rosson
    /// Q1): drawn "Already on" with one option, `Close` (a no-op).
    AlreadyDone,
    /// Won the compare-and-set; the apply is running.
    Applying,
    Approved,
    Denied,
    Expired,
    /// Values that moved (widget version, held request decided elsewhere,
    /// workspace gone); nothing applied.
    Stale,
    /// The apply failed; nothing applied.
    Failed,
    /// Withdrawn by the agent (`k2 permission cancel`).
    Cancelled,
    /// Closed because a permission-cards switch turned off (§2.5).
    Closed,
}

impl RequestState {
    pub const ALL: [RequestState; 10] = [
        RequestState::Pending,
        RequestState::AlreadyDone,
        RequestState::Applying,
        RequestState::Approved,
        RequestState::Denied,
        RequestState::Expired,
        RequestState::Stale,
        RequestState::Failed,
        RequestState::Cancelled,
        RequestState::Closed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::AlreadyDone => "already_done",
            RequestState::Applying => "applying",
            RequestState::Approved => "approved",
            RequestState::Denied => "denied",
            RequestState::Expired => "expired",
            RequestState::Stale => "stale",
            RequestState::Failed => "failed",
            RequestState::Cancelled => "cancelled",
            RequestState::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        RequestState::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// Still answerable (`Close` on an already-done card is an answer).
    pub fn is_open(self) -> bool {
        matches!(self, RequestState::Pending | RequestState::AlreadyDone)
    }

    /// The Thread card's own `choice.status` for this state (§17.2.4).
    pub fn choice_status(self) -> &'static str {
        match self {
            RequestState::Pending | RequestState::AlreadyDone | RequestState::Applying => "pending",
            RequestState::Approved | RequestState::Denied | RequestState::Failed | RequestState::Stale => "answered",
            RequestState::Expired => "expired",
            RequestState::Cancelled | RequestState::Closed => "voided",
        }
    }
}

/// One `permission_requests` row (migration 0218).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: RequestId,
    /// The asking workspace (always from the passport).
    pub workspace_id: String,
    /// The asking session, when known.
    pub session_id: Option<String>,
    /// The Thread conversation the card lives on.
    pub conversation_id: Option<String>,
    /// The card's doc id on that Thread.
    pub card_id: Option<String>,
    pub action: String,
    /// Values fixed at request time.
    pub params: Value,
    pub params_hash: String,
    /// Hash of the K2-written card text, so a catalog wording change between
    /// request and click is `stale`, never a silently re-worded card (C35).
    pub text_hash: String,
    /// The agent's `--why`: plain text, ≤ 200 chars, one line.
    pub why: String,
    pub state: RequestState,
    /// The catalog floor at request time (`Floor::as_str`, or `garden-owner`).
    pub floor: String,
    /// An existing held request this card attaches to (`ar_22`).
    pub held_ref: Option<String>,
    /// The batch card this request sits on (§4.5).
    pub batch_id: Option<String>,
    /// "ask 2 of 2" (§3.2).
    pub ask_number: i64,
    /// The login that last posted to the conversation (label only).
    pub asked_for: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub answered_at: Option<i64>,
    pub answered_by: Option<String>,
    /// `view.answer.via`: `desktop`, `phone`, `web` or `terminal` (§17.2.1).
    pub answered_via: Option<String>,
    /// The deny reason (≤ 200 chars), shown to the agent and the asker.
    pub reason: Option<String>,
    pub result_message: Option<String>,
    /// The one `update` (§3.2).
    pub updated_why: Option<String>,
    pub updated_at: Option<i64>,
}

/// The `permission` object a refusal carries while the gate is on for the
/// calling agent (§3.1). Absent entirely while off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionRefusal {
    /// A mapped wall: the exact command to ask with.
    Requestable {
        action: String,
        params: serde_json::Map<String, Value>,
        approvers: String,
        command: String,
    },
    /// Anything else, including a machine-only refusal (`where` = "at the
    /// machine", §5.5): "It can't be requested."
    NotRequestable {
        requestable: bool,
        #[serde(rename = "where")]
        where_: String,
    },
}

impl PermissionRefusal {
    pub fn not_requestable(where_: impl Into<String>) -> Self {
        PermissionRefusal::NotRequestable { requestable: false, where_: where_.into() }
    }
}

/// The two closing lines (§3.1): exactly one ends every refusal while the
/// gate is on.
pub const CLOSING_ASK_WITH: &str = "Ask with: ";
pub const CLOSING_CANNOT: &str = "It can't be requested.";
/// `where` for a machine-only refusal (§5.5).
pub const WHERE_AT_MACHINE: &str = "at the machine";

/// Error codes of the family (§3.2, §17.3, §17.5).
pub mod code {
    pub const BAD_ACTION: &str = "bad_action";
    pub const BAD_PARAMS: &str = "bad_params";
    pub const BAD_WHY: &str = "bad_why";
    pub const BAD_NOTE: &str = "bad_note";
    pub const NOT_CARDABLE: &str = "not_cardable";
    pub const ALREADY_DONE: &str = "already_done";
    pub const ALREADY_PENDING: &str = "already_pending";
    pub const RECENTLY_DENIED: &str = "recently_denied";
    pub const ASK_LIMIT: &str = "ask_limit";
    pub const TOO_MANY_REQUESTS: &str = "too_many_requests";
    pub const REQUEST_NOT_PENDING: &str = "request_not_pending";
    pub const ALREADY_UPDATED: &str = "already_updated";
    pub const NOTE_NOT_ALLOWED: &str = "note_not_allowed";
    pub const UNDO_EXPIRED: &str = "undo_expired";
    /// Exists on the keep-as-mine code; reused, never renamed.
    pub const ACTION_CARD_PERSON_ONLY: &str = "action_card_person_only";
    /// Exists in the vault confirm broker; reused.
    pub const VAULT_CONFIRM_REQUIRED: &str = "vault_confirm_required";
}

/// Limits (§3.2, §4.2–§4.5, §6.6, §6.7, §17.5, §17.7).
pub mod limits {
    pub const WHY_MAX_CHARS: usize = 200;
    pub const DENY_REASON_MAX_CHARS: usize = 200;
    pub const NOTE_MAX_CHARS: usize = 500;
    pub const PENDING_PER_WORKSPACE: usize = 5;
    pub const ASKS_PER_WORKSPACE_PER_HOUR: usize = 20;
    pub const PENDING_PER_SERVER: usize = 50;
    pub const DENIAL_BLOCK_SECS: i64 = 24 * 3600;
    pub const UPDATE_WINDOW_SECS: i64 = 7 * 24 * 3600;
    /// A third ask for the same action and values within this window → `ask_limit`.
    pub const ASK_LIMIT_WINDOW_SECS: i64 = 7 * 24 * 3600;
    pub const ASKS_PER_WINDOW: i64 = 2;
    pub const BATCH_WINDOW_SECS: i64 = 5 * 60;
    pub const BATCH_MAX_ITEMS: usize = 8;
    pub const TURN_OFF_AGAIN_SECS: i64 = 7 * 24 * 3600;
    pub const ASKED_FOR_WINDOW_SECS: i64 = 30 * 60;
    /// The render-time `check` cache (§4.2).
    pub const CHECK_CACHE_SECS: u64 = 2;
    /// The push budget, per person per hour (§6.7, §17.7).
    pub const PUSHES_PER_PERSON_PER_HOUR: u32 = 30;
    /// The Tickets item is filed when no eligible person was visible on the
    /// Thread within this window (§6.7).
    pub const VISIBLE_WINDOW_SECS: i64 = 10 * 60;
}

/// Canonical hash of an action's values: keys sorted, JSON, SHA-256 hex.
/// The block and dedupe key is (workspace, action, params hash) (§3.2).
pub fn params_hash(params: &serde_json::Map<String, Value>) -> String {
    let sorted: std::collections::BTreeMap<&String, &Value> = params.iter().collect();
    let canon = serde_json::to_string(&sorted).expect("a map of JSON values serializes");
    hex_sha256(canon.as_bytes())
}

/// SHA-256 hex of the K2-written card text (C35).
pub fn text_hash(text: &str) -> String {
    hex_sha256(text.as_bytes())
}

fn hex_sha256(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    d.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_have_the_prefix_and_parse() {
        let id = RequestId::new();
        assert!(id.as_str().starts_with("pr_") && id.as_str().len() == 15, "{id}");
        assert_eq!(RequestId::parse(id.as_str()), Some(id.clone()));
        assert!(RequestId::parse("pr_7f3a2c").is_some());
        for bad in ["pr_", "pr_7f3", "ar_7f3a2c", "pr_7F3A2C", "pr_7f3a2z", "7f3a2c"] {
            assert_eq!(RequestId::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn states_round_trip_and_map_to_the_card_status() {
        for s in RequestState::ALL {
            assert_eq!(RequestState::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        assert!(RequestState::Pending.is_open() && RequestState::AlreadyDone.is_open());
        assert!(!RequestState::Applying.is_open());
        let table: Vec<(&str, &str)> = RequestState::ALL.iter().map(|s| (s.as_str(), s.choice_status())).collect();
        assert_eq!(
            table,
            [
                ("pending", "pending"),
                ("already_done", "pending"),
                ("applying", "pending"),
                ("approved", "answered"),
                ("denied", "answered"),
                ("expired", "expired"),
                ("stale", "answered"),
                ("failed", "answered"),
                ("cancelled", "voided"),
                ("closed", "voided"),
            ],
            "PRD §17.2.4"
        );
    }

    #[test]
    fn params_hash_ignores_key_order() {
        let a = json!({"to": "sales", "core": "db"});
        let b = json!({"core": "db", "to": "sales"});
        let c = json!({"core": "files", "to": "sales"});
        let h = |v: &Value| params_hash(v.as_object().unwrap());
        assert_eq!(h(&a), h(&b));
        assert_ne!(h(&a), h(&c));
        assert_eq!(h(&json!({})).len(), 64);
    }

    #[test]
    fn refusal_object_wire_shapes() {
        let r = PermissionRefusal::Requestable {
            action: "compute.use".into(),
            params: serde_json::Map::new(),
            approvers: "owner or admin".into(),
            command: "k2 permission request compute.use --why \"<what you need it for>\"".into(),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"action": "compute.use", "params": {}, "approvers": "owner or admin",
                   "command": "k2 permission request compute.use --why \"<what you need it for>\""})
        );
        let n = PermissionRefusal::not_requestable(WHERE_AT_MACHINE);
        assert_eq!(serde_json::to_value(&n).unwrap(), json!({"requestable": false, "where": "at the machine"}));
    }
}
