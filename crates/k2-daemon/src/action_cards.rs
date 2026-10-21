//! Agents present settings cards (0.46.3, Rosson 2026-10-20): "agents
//! should be able to present cards to the user so that they don't have to
//! go hunting around in the settings to make those changes."
//!
//! A card is posted with [`crate::overlay_routes::present_action_card`]
//! (a Thread choice card carrying an action id + params). When a PERSON
//! taps an option, the Thread answer route checks it is the person (never
//! an agent, a K2 terminal, a Garden widget or an App guest) and calls
//! [`run`] here. Settings stays the place to undo every action.
//!
//! The first action is [`KEEP_WIDGET`]. A later PRD covers the general
//! pattern; a new action is one arm in [`run`] plus the code that presents
//! it.

use serde_json::{json, Value as J};

use k2_core::overlay::CardAction;

/// "Keep as mine": stamp a widget version as the person's (the same as
/// Settings → Gardens → Local Data → It's mine).
pub const KEEP_WIDGET: &str = "zen.widget.keep";
/// [`KEEP_WIDGET`]'s options.
pub const KEEP_AS_MINE: &str = "Keep as mine";
pub const LET_SYNC_UPDATE: &str = "Let sync update it";

/// Run `action` for the person's `answer` (already checked to be one of the
/// card's options). `by` is the answering person (`owner` or a login). The
/// result is shown in the answer's JSON.
pub fn run(action: &CardAction, answer: &str, by: &str) -> Result<J, String> {
    match action.id.as_str() {
        KEEP_WIDGET => keep_widget(&action.params, answer, by),
        other => Err(format!("K2 doesn't know the card action '{other}'")),
    }
}

/// The card for a widget version an agent changed: Keep as mine claims
/// that version (`params.hash`); Let sync update it leaves it unsigned.
fn keep_widget(params: &J, answer: &str, by: &str) -> Result<J, String> {
    let widget = params["widget"].as_str().filter(|w| !w.is_empty()).ok_or("the card names no widget")?;
    let hash = params["hash"].as_str().filter(|h| !h.is_empty()).ok_or("the card names no widget version")?;
    match answer {
        KEEP_AS_MINE => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::garden_data::authors::claim(&conn, widget, hash, chrono::Utc::now().timestamp())
                .map_err(|e| format!("{widget} wasn't kept: {e:?}"))?;
            drop(conn);
            crate::gardens_manage::audit("zen-widget-keep", by, &format!("{widget} {}", &hash[..hash.len().min(12)]), None, None);
            if let Err(e) = crate::zen_routes::refresh_and_emit() {
                k2_core::log_debug!("[action-card] refresh after keep: {e}");
            }
            Ok(json!({ "kept": true, "widget": widget, "hash": hash }))
        }
        LET_SYNC_UPDATE => Ok(json!({ "kept": false, "widget": widget, "hash": hash })),
        other => Err(format!("'{other}' isn't an answer to this card")),
    }
}
