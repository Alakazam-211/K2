//! The card's `cards=2` view (`doc.choice.action.view`, PRD rev 3.1
//! §17.2) and the [`Viewer`] the projection is computed for (§4.2, C9).
//!
//! The daemon writes `view` **per viewer**: two people reading the same card
//! get different `viewer`, `mode`, `noteBox` and options. Only people get it
//! (`cards=2`); passports get the text fallback (§17.6). App guests never
//! see an action or permission card at all: it is dropped on every guest
//! path, neither a card nor any fallback text (K2's rule, 2026-10-21).
//! Field names are the phone contract's, exactly.
//!
//! P0: types only. The projection (`project()`) is daemon-side, in lane
//! A3's `k2-daemon/src/permission/view.rs`.

use serde::{Deserialize, Serialize};

use super::catalog::{ConfirmAction, Direction, Grant};
use super::request::RequestState;

/// `view.v`.
pub const VIEW_VERSION: u8 = 1;

/// Who is looking at a Thread doc (§4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Viewer {
    /// An App guest pass (`k2skn_`). Never reaches `project()`: guest
    /// paths drop action cards before any projection.
    Guest { app: Option<String> },
    /// A Connect login with its role (`owner`, `admin`, `member`).
    Login { user: String, role: String },
    /// The owner token (the desktop app, a terminal outside K2).
    OwnerToken,
    /// An agent passport (any transport): only the waiting text, ever.
    Passport { workspace_id: String },
}

impl Viewer {
    /// `cards=2` is honoured only for people (§17.1).
    pub fn is_person(&self) -> bool {
        matches!(self, Viewer::Login { .. } | Viewer::OwnerToken)
    }
}

/// `view.viewer.mode` (§17.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Draw the action buttons.
    Approve,
    /// No buttons: the waiting text, plus the note box when allowed.
    Wait,
}

/// `view.viewer` (§17.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerView {
    /// The viewer's login; `"owner"` for the owner token.
    pub login: String,
    /// `owner`, `admin` or `member`.
    pub role: String,
    /// `login` or `owner-token`.
    pub via: String,
    pub mode: Mode,
    /// The viewer meets the action's floor (re-checked at the tap).
    pub can_answer: bool,
    /// `viewer.login == askedFor`.
    pub is_asker: bool,
    /// Batch only: meets every item's floor and no pending item needs a
    /// confirm (§17.2.2).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub can_answer_all: Option<bool>,
}

/// One verbatim value (`[{key, value}]`, drawn monospace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// The agent's words, labelled `"<by> says:"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Why {
    pub by: String,
    pub text: String,
}

/// A note from a person below the floor (§4.3, §17.5). Never
/// reaches the agent's turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub by: String,
    /// `owner`, `admin` or `member`.
    pub role: String,
    pub text: String,
    pub at: i64,
}

/// Phone contract field (§17.2.1); always null in v1: App guests never see
/// or ask through a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Askers {
    pub count: i64,
    pub today: i64,
}

/// `view.approvers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approvers {
    /// `owner` or `admin`.
    pub floor: String,
    /// e.g. `owner or admin`.
    pub label: String,
    /// Logins.
    pub names: Vec<String>,
}

/// `view.proof` (§17.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ViewProof {
    Click,
    Confirm {
        action: ConfirmAction,
        #[serde(rename = "secretId")]
        secret_id: String,
    },
}

/// `view.noteBox`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteBox {
    pub allowed: bool,
    /// 500 (people only).
    pub max: usize,
    /// The viewer's current note, or null.
    pub mine: Option<String>,
}

/// `view.answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerView {
    /// `allow` or `deny`.
    pub decision: String,
    pub by: String,
    /// `desktop`, `phone`, `web` or `terminal`.
    pub via: String,
    pub at: i64,
    pub reason: Option<String>,
}

/// `view.undo`: present on an approved standing grant for 7 days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoView {
    /// Exactly `"Turn off again"`.
    pub label: String,
    pub until: i64,
}

/// One permission item: `view` for `action.id = "permission"` (§17.2.1),
/// and each batch item (the same object minus `viewer`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    /// `pr_…`, or the held request's own id (`ar_22`).
    pub request_id: String,
    pub state: RequestState,
    /// The catalog id (details view only).
    pub action: String,
    /// The Settings label.
    pub setting: String,
    /// K2's sentence: the headline.
    pub what_changes: String,
    pub direction: Direction,
    pub grant: Grant,
    pub scope: String,
    pub duration: String,
    pub consequences: String,
    pub values: Vec<KeyValue>,
    pub why: Option<Why>,
    pub notes: Vec<Note>,
    pub asked_for: Option<String>,
    pub askers: Option<Askers>,
    pub approvers: Approvers,
    pub proof: ViewProof,
    pub expires_at: i64,
    pub settings_page: String,
    pub note_box: NoteBox,
    pub answer: Option<AnswerView>,
    pub undo: Option<UndoView>,
}

/// `view` for `action.id = "permission"`: `{v, kind:"permission", viewer,
/// …item fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionView {
    pub v: u8,
    /// Always `"permission"`.
    pub kind: String,
    pub viewer: ViewerView,
    #[serde(flatten)]
    pub item: ItemView,
}

/// `view.state` of a batch: `pending` while any item is, then `done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchState {
    Pending,
    Done,
}

/// `view` for `action.id = "permission.batch"` (§17.2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub v: u8,
    /// Always `"batch"`.
    pub kind: String,
    pub state: BatchState,
    pub viewer: ViewerView,
    pub items: Vec<ItemView>,
    pub approvers: Approvers,
    pub expires_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item() -> ItemView {
        ItemView {
            request_id: "pr_7f3a2c".into(),
            state: RequestState::Pending,
            action: "compute.use".into(),
            setting: "Allow compute nodes".into(),
            what_changes: "Let billing run jobs on this server's compute nodes".into(),
            direction: Direction::Loosen,
            grant: Grant::Standing,
            scope: "billing".into(),
            duration: "until someone turns it off".into(),
            consequences: "Every active machine; default limits.".into(),
            values: vec![KeyValue { key: "workspace".into(), value: "billing".into() }],
            why: Some(Why { by: "billing".into(), text: "Run the test suite on k2n2-m1".into() }),
            notes: vec![Note { by: "dana".into(), role: "member".into(), text: "payroll run".into(), at: 1_761_060_000 }],
            asked_for: Some("dana".into()),
            askers: None,
            approvers: Approvers { floor: "admin".into(), label: "owner or admin".into(), names: vec!["rosson".into()] },
            proof: ViewProof::Click,
            expires_at: 1_761_300_000,
            settings_page: "workspaces/billing/agent".into(),
            note_box: NoteBox { allowed: false, max: 500, mine: None },
            answer: None,
            undo: None,
        }
    }

    fn viewer() -> ViewerView {
        ViewerView {
            login: "rosson".into(),
            role: "owner".into(),
            via: "login".into(),
            mode: Mode::Approve,
            can_answer: true,
            is_asker: false,
            can_answer_all: None,
        }
    }

    #[test]
    fn permission_view_has_the_phone_contract_field_names() {
        let v = PermissionView { v: VIEW_VERSION, kind: "permission".into(), viewer: viewer(), item: item() };
        let w = serde_json::to_value(&v).unwrap();
        let mut keys: Vec<&str> = w.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        let mut want = vec![
            "v", "kind", "requestId", "state", "viewer", "action", "setting", "whatChanges", "direction", "grant",
            "scope", "duration", "consequences", "values", "why", "notes", "askedFor", "askers", "approvers",
            "proof", "expiresAt", "settingsPage", "noteBox", "answer", "undo",
        ];
        want.sort_unstable();
        assert_eq!(keys, want, "PRD §17.2.1");
        assert_eq!(w["viewer"], json!({"login": "rosson", "role": "owner", "via": "login", "mode": "approve",
                                        "canAnswer": true, "isAsker": false}));
        assert_eq!(w["direction"], json!("loosen"));
        assert_eq!(w["grant"], json!("standing"));
        assert_eq!(w["proof"], json!({"kind": "click"}));
        assert_eq!(w["values"], json!([{"key": "workspace", "value": "billing"}]));
        assert_eq!(w["noteBox"], json!({"allowed": false, "max": 500, "mine": null}));
        let back: PermissionView = serde_json::from_value(w).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn confirm_proof_and_states_on_the_wire() {
        let p = ViewProof::Confirm { action: ConfirmAction::AppApprove, secret_id: "ar_22".into() };
        assert_eq!(serde_json::to_value(&p).unwrap(), json!({"kind": "confirm", "action": "app-approve", "secretId": "ar_22"}));
        let mut i = item();
        i.state = RequestState::AlreadyDone;
        i.grant = Grant::OneTime;
        let w = serde_json::to_value(&i).unwrap();
        assert_eq!((w["state"].clone(), w["grant"].clone()), (json!("already_done"), json!("one-time")));
    }

    #[test]
    fn batch_view_shape() {
        let mut vw = viewer();
        vw.can_answer_all = Some(true);
        let b = BatchView {
            v: VIEW_VERSION,
            kind: "batch".into(),
            state: BatchState::Pending,
            viewer: vw,
            items: vec![item()],
            approvers: Approvers { floor: "admin".into(), label: "owner or admin".into(), names: vec![] },
            expires_at: 1,
        };
        let w = serde_json::to_value(&b).unwrap();
        assert_eq!(w["viewer"]["canAnswerAll"], json!(true));
        assert_eq!(w["state"], json!("pending"));
        assert!(w["items"][0].get("viewer").is_none(), "items carry no viewer");
    }

    #[test]
    fn only_people_get_cards_2() {
        assert!(Viewer::OwnerToken.is_person());
        assert!(Viewer::Login { user: "dana".into(), role: "member".into() }.is_person());
        assert!(!Viewer::Guest { app: None }.is_person());
        assert!(!Viewer::Passport { workspace_id: "w".into() }.is_person());
    }
}
