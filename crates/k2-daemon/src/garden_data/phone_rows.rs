//! Rows only the phone door answers (contract-gardens-phone rev 2 §D; plan
//! B0): verbs a desktop frame's renderer answers itself, so they have no
//! `garden-local` runner, and on a phone have to be answered here. They stay
//! `widget` exposure: `/cli/zen/call` still refuses them, and nothing on the
//! desktop widens.
//!
//! **What's new here is widget code acting as the person from a phone**, so
//! every limit the renderer applies on the desktop is applied here: the phone
//! decides nothing. Before any row runs: the door's own gate (the owner
//! person's paired phone), then [`place`] (placement, live hash, cap, the
//! signer gate, the placement's read budget).

use serde_json::json;

use k2_core::contract::{catalog, phone::DOOR_ROW_VERBS};
use k2_core::zen::{phone as garden_phone, widgets, ZenFiles};

use super::budget::{self, BudgetKey, Spend};
use super::call::{check_cap, needs_editor, resolve, CallBody, Placed};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub fn is_verb(verb: &str) -> bool {
    DOOR_ROW_VERBS.contains(&verb)
}

/// One door row, as the phone's person (`who`, from the door's own gate).
pub(crate) fn handle(b: &CallBody, who: &Who) -> CliResponse {
    match run(b, who) {
        Ok(v) => crate::garden_data::ok(json!({ "ok": true, "result": v })),
        Err(r) => r,
    }
}

fn run(b: &CallBody, _who: &Who) -> Result<serde_json::Value, CliResponse> {
    let (_w, _addresses) = place(b, &b.verb, Spend::Read)?;
    Err(crate::garden_data::response(&crate::garden_data::refused("unknown_verb", 400, format!("no verb {}", b.verb))))
}

/// Everything a door row needs before it runs: the placement and live hash
/// (`stale_widget`), the row's cap (`cap_not_granted`), the signer gate
/// (`needs_editor`), the read budget (`rate_limited`), and the addresses this
/// placement binds on a phone: from the merged `config`, so a phone's own
/// `config` may name a source and a desktop-only one stays out.
pub(crate) fn place(b: &CallBody, verb: &str, spend: Spend) -> Result<(Placed, Vec<String>), CliResponse> {
    let fail = |e: k2_core::garden_data::DataError| crate::garden_data::response(&e);
    let w = resolve(b).map_err(fail)?;
    let cap = catalog().verb(verb).and_then(|r| r.cap.clone());
    if let Some(cap) = cap.as_deref() {
        check_cap(&w, verb, cap).map_err(fail)?;
    }
    if !w.signed {
        return Err(fail(needs_editor(&w)));
    }
    let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
    budget::spend(&key, spend).map_err(fail)?;
    let addresses = bound_addresses(&w).map_err(crate::zen_routes::error_response)?;
    Ok((w, addresses))
}

/// The addresses this placement binds on a phone (contract rev 2 §A.3).
fn bound_addresses(w: &Placed) -> Result<Vec<String>, k2_core::zen::ZenError> {
    let f = ZenFiles::local();
    let page = f.page_of(&w.garden)?;
    let placed = page["widgets"].as_array().into_iter().flatten().find(|x| x["id"] == w.placement.as_str());
    let config = placed.map_or_else(|| json!({}), |x| garden_phone::merged_config(&page, x));
    let st = f.garden_widget_status(&w.garden, &w.widget);
    let sources = st.manifest.as_ref().and_then(|m| m.sources.as_ref());
    Ok(widgets::placement_source_addresses(sources, &config))
}

#[cfg(test)]
#[path = "phone_rows_tests.rs"]
mod tests;
