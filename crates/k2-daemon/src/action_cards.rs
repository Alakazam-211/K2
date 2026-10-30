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
/// [`KEEP_WIDGET`]'s first option; the second is [`leave_label`].
pub const KEEP_AS_MINE: &str = "Keep as mine";

/// [`KEEP_WIDGET`]'s second option: "Leave it as <agent>'s".
pub fn leave_label(agent: &str) -> String {
    format!("Leave it as {agent}'s")
}

/// [`KEEP_WIDGET`]'s words (the card asks for a rights decision, so it says
/// what happens): "<agent> changed <widget> in <garden>. Keep this version
/// as yours? It will then read and write its data with your access instead
/// of <agent>'s."
pub fn keep_prompt(agent: &str, widget: &str, garden: &str) -> String {
    format!(
        "{agent} changed {widget} in {garden}. Keep this version as yours? It will then read and write its data with your access instead of {agent}'s."
    )
}

/// Run `action` for the person's `answer` (already checked to be one of the
/// card's options). `by` is the answering person (`owner` or a login). The
/// result is shown in the answer's JSON.
pub fn run(action: &CardAction, answer: &str, by: &str) -> Result<J, String> {
    match action.id.as_str() {
        KEEP_WIDGET => keep_widget(&action.params, answer, by),
        // R8 upgrade (prd-env-to-vault-v1): keep time-boxed `.env.k2` grants.
        crate::vault::server_refs::KEEP_ACTION => crate::vault::server_refs::keep_action(&action.params, answer, by),
        // prd-security-sweep-0464-v1 SS10: Trust it / Always trust this
        // folder / Don't trust.
        crate::folder_trust::CARD_ACTION => crate::folder_trust::run_card(&action.params, answer, by),
        // agent-app-users §5.5: a tied person's access waits for a reset.
        crate::app_user_invites::CARD_ACTION => crate::app_user_invites::run_card(&action.params, answer, by),
        // Mobile dev (M-A23): the box owner accepts a device lane's licence.
        crate::sim_routes::CARD_ACTION => crate::sim_routes::run_card(&action.params, answer, by),
        // prd-k2-db-rename-v1 DB-A6: which database to keep (owner only).
        crate::db_home_routes::CARD_ACTION => crate::db_home_routes::run_card(&action.params, answer, by),
        // PV-11: sites public from before 0.46.7, one click each.
        crate::publish_cards::UPGRADE_ACTION => crate::publish_cards::run_upgrade_card(answer, by),
        other => Err(format!("K2 doesn't know the card action '{other}'")),
    }
}

/// The card for a widget version an agent changed: Keep as mine claims
/// that version (`params.hash`); Leave it as <agent>'s changes nothing.
/// `message` is what the person is told after the tap.
fn keep_widget(params: &J, answer: &str, by: &str) -> Result<J, String> {
    let widget = params["widget"].as_str().filter(|w| !w.is_empty()).ok_or("the card names no widget")?;
    let hash = params["hash"].as_str().filter(|h| !h.is_empty()).ok_or("the card names no widget version")?;
    let agent = params["agent"].as_str().filter(|a| !a.is_empty()).ok_or("the card names no agent")?;
    if answer == leave_label(agent) {
        return Ok(json!({ "kept": false, "widget": widget, "hash": hash, "message": format!("Left as {agent}'s.") }));
    }
    match answer {
        KEEP_AS_MINE => {
            // Through the claim route's own entry (PRD §4.1, C15): the same
            // GardenOwner check as Settings → It's mine, as the answering
            // person, and only while the version the card names is still
            // the live one (409 widget_changed otherwise: never keep
            // something else than the card showed).
            let caller = if by == "owner" {
                crate::sidecar_routes::Caller::Owner
            } else {
                crate::sidecar_routes::Caller::Login { username: by.to_string() }
            };
            let r = crate::zen_routes::claim_entry(&caller, widget, Some(hash), "keep-card");
            if !r.status.starts_with('2') {
                return Err(format!("{widget} wasn't kept: {}", r.body));
            }
            crate::gardens_manage::audit("zen-widget-keep", by, &format!("{widget} {}", &hash[..hash.len().min(12)]), None, None);
            Ok(json!({
                "kept": true,
                "widget": widget,
                "hash": hash,
                "message": "Kept as yours. Undo in Settings → Gardens → Local Data.",
            }))
        }
        other => Err(format!("'{other}' isn't an answer to this card")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keep card applies through the claim route's GardenOwner check:
    /// a login that isn't the server's Owner (an Admin, a Member, a name
    /// K2 doesn't know) is refused and nothing is claimed, whatever the
    /// card's floor let through.
    #[test]
    fn a_keep_card_answered_by_a_login_without_garden_owner_is_refused() {
        let action = CardAction {
            id: KEEP_WIDGET.into(),
            params: json!({ "widget": "notes", "hash": "0123456789abcdef0123", "agent": "builder" }),
            dedupe: String::new(),
            result: None,
            view: None,
        };
        let e = run(&action, KEEP_AS_MINE, "not-an-owner-login").expect_err("only the Garden's owner keeps a widget");
        assert!(e.starts_with("notes wasn't kept:"), "{e}");
        assert!(e.contains("Only you can claim a widget"), "the claim route's own owner-only refusal: {e}");
        // Leave it as <agent>'s changes nothing and needs no rights.
        let left = run(&action, &leave_label("builder"), "not-an-owner-login").expect("leave");
        assert_eq!(left["kept"], false);
    }
}
