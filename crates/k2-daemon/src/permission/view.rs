//! A3 · projection — `project()`, one function at the three doors that serve
//! Thread docs (PRD rev 3.1 §4.2, vs-live C9): `thread_items_view` (every GET
//! page, the delta's `items`/`changed`, catchup), `wire_json_for` (every WS
//! frame, the skin stream, room-stream's `doc_for`) and `thread_preview`.
//!
//! Contract (frozen at P0):
//! - the identity for any doc without `choice.action`;
//! - `cards` is 2 only when the request asked for it **and** the viewer is a
//!   person (`Viewer::is_person`); then the person gets the raw choice doc
//!   with `action.view` written for them (`k2_core::permission::card`);
//! - everyone else (old clients, agent passports) gets the §17.6 fallback;
//! - App guests never reach it: every guest path drops action cards first;
//! - the doc's `id` and `seq` never change;
//! - the current Settings state is shown: `check` runs at render (cached
//!   `limits::CHECK_CACHE_SECS`), so a satisfied pending card is drawn
//!   `already_done` with one option, `Close`.
//!
//! P0 stub: the identity for every doc. Lane A3 fills the person arms; until
//! then nothing calls this.

use k2_core::overlay::doc::OverlayDoc;
use k2_core::permission::card::Viewer;

pub(crate) fn project(doc: &OverlayDoc, viewer: &Viewer, cards: u8) -> OverlayDoc {
    let _ = (viewer, cards);
    doc.clone()
}
