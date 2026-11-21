//! A choice card's metadata map on `POST /cli/thread/ask` (`k2 thread ask
//! --meta`, 0.46.17). The limits live in k2-core [`overlay::card_meta`];
//! this file is the ask's side: the check before anything is stored, and
//! the map in the ask's answer.

use super::*;

/// The ask's checks before anything is stored: the §6.10 caps
/// (prd-agent-settings-cards-v1: at most 8 options of 80 characters and a
/// 1,000-character prompt), then the asker's metadata map. `Ok(None)` when
/// the ask has no map or an empty one. An `Err` is a usage refusal.
pub(super) fn check_ask(
    params: &HashMap<String, String>,
    prompt: &str,
    options: &[String],
) -> Result<Option<overlay::card_meta::CardMeta>, String> {
    overlay::check_ask_caps(prompt, options)?;
    let meta = opt_param(params, "meta").map(|raw| overlay::card_meta::parse_param(&raw)).transpose()?;
    Ok(meta.flatten())
}

/// The ask's answer with the stored map, only when the card has one (no
/// empty field, so a card asked without one answers as it always did).
pub(super) fn with_meta(mut body: serde_json::Value, choice: Option<&overlay::ChoiceBody>) -> serde_json::Value {
    if let Some(m) = choice.and_then(|c| c.meta.clone()) {
        body["meta"] = serde_json::Value::Object(m);
    }
    body
}
