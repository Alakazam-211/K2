//! Permission cards (`.k2/prds/prd-agent-settings-cards-v1.md` rev 3.1): an
//! agent that hits an owner/admin wall asks for **one named change**; K2
//! puts a card on the agent's Thread; a person at the Settings floor clicks
//! once (plus the confirm Settings itself takes, if any) and K2 applies it
//! through the Settings route's own entry, as that person.
//!
//! **P0 contract (lane A1).** Shared types and the switch reads only.
//! Nothing calls into this module yet, so with both switches off (the
//! default) K2 behaves exactly as before; the off-goldens
//! (`k2-daemon/tests/permission_off_golden.rs`,
//! `tests/cli/permission_off_golden.sh`) pin that.
//!
//! | File | Owner after P0 | What |
//! |---|---|---|
//! | [`catalog`] | A1 | `ActionSpec` and its field types (floor, proof, walls, machine hints…) |
//! | [`request`] | A1 | request ids, states, the stored row, the refusal's `permission` object, error codes, limits |
//! | [`card`] | A1 | the `cards=2` view (`choice.action.view`, PRD §17.2) and `Viewer` |
//! | [`gate`] | B | the two switches: `server_on()`, `agent_switch_on()`, `enabled_for()` |
//!
//! The projection itself (`project()`, PRD §4.2) is daemon-side, in
//! `k2-daemon/src/permission/view.rs` (A3), because it runs the catalog's
//! `check`. App guests never see action or permission cards: every guest
//! path drops them (`OverlayDoc::is_action_card`). Contract changes after P0 go through A1 only.

pub mod card;
pub mod catalog;
pub mod gate;
pub mod request;

/// `/boot-status` feature key, listed only while the global switch is on
/// (PRD §2.1, §10; lane B).
pub const FEATURE_KEY: &str = "permission-cards-v1";

/// The `cards=2` contract version (PRD §17): honoured only for people.
pub const CARDS_CONTRACT: u8 = 2;

/// The `k2-permissions` skill's own version constant (vs-live C8, PRD
/// §11.4): bumping it never touches `SKILL_VERSION_WORKSPACE` or `_ZEN`, so
/// the k2-cli skill stays byte-identical in every switch state. Lane C
/// moves it to `skills/version.rs` (one new line) when it writes the skill.
pub const SKILL_VERSION_PERMISSIONS: u32 = 1;

/// Folder name of the separate teaching skill (`.k2/skills/k2-permissions/`),
/// written only while both switches are on (PRD §11.4; lane C).
pub const SKILL_DIR: &str = "k2-permissions";

/// The env force-on for the global switch (the `K2_COMPUTE` pattern).
pub const ENV_FORCE_ON: &str = "K2_PERMISSION_CARDS";

/// The family's route prefix. While the global switch is off the route
/// tables skip it, so it answers as an unknown route (PRD §2.4; lane B).
pub const ROUTE_PREFIX: &str = "/cli/permission/";

/// The note route (people below the floor, PRD §4.3, §17.5).
pub const NOTE_ROUTE: &str = "/cli/thread/permission-note";

/// "Turn off again" on an approved standing grant (PRD §4.4, §17.3).
pub const UNDO_ROUTE: &str = "/cli/thread/permission-undo";

/// Action-card ids on the Thread doc (`choice.action.id`, PRD §4.1).
pub const CARD_ACTION_PERMISSION: &str = "permission";
pub const CARD_ACTION_BATCH: &str = "permission.batch";

/// `doc.via` on a permission card (PRD §17.2).
pub const DOC_VIA: &str = "permission";

/// Exact option labels, matched character for character (PRD §17.3).
pub mod label {
    pub const ALLOW: &str = "Allow";
    pub const DENY: &str = "Deny";
    pub const ALLOW_ALL: &str = "Allow all";
    pub const DENY_ALL: &str = "Deny all";
    pub const CLOSE: &str = "Close";
    pub const TURN_OFF_AGAIN: &str = "Turn off again";
}

/// Access Audit events (PRD §6.5).
pub mod audit_event {
    pub const REQUEST: &str = "permission-request";
    pub const ANSWER: &str = "permission-answer";
    pub const NOTE: &str = "permission-note";
    pub const UNDO: &str = "permission-undo";
    pub const SWITCH: &str = "permission-switch";
}
