//! Rows only the phone door answers (contract-gardens-phone rev 2 §D, §G;
//! plan B0, B1): verbs a desktop frame's renderer answers itself, so they have
//! no `garden-local` runner, and on a phone have to be answered here. They stay
//! `widget` exposure: `/cli/zen/call` still refuses them, and nothing on the
//! desktop widens.
//!
//! **What's new here is widget code acting as the person from a phone**, so
//! every limit the renderer applies on the desktop is applied here: the phone
//! decides nothing. Before any row runs: the door's own gate (the owner
//! person's paired phone), then [`place`] (placement, live hash, cap, the
//! signer gate, the placement's read budget).
//!
//! **Live sources (B1).** A placement reads only the sources it binds, and on a
//! phone that comes from its merged `config` ([`bound_addresses`]). What it
//! gets is the stored card view (`sources::view::build`, through the read and
//! changes routes), never `/cli/sources/show`, `list` or `export`: those carry
//! the secret's name, the fetch definition and who made it, and the view has no
//! field for any of them. No token, secret name, fetch URL or header reaches a
//! phone. Other servers answer `server_not_saved`: this daemon holds no logins
//! for them (plan Q2). The subscribe is the phone's: event, then re-read.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use k2_core::contract::{catalog, phone::DOOR_ROW_VERBS};
use k2_core::sources::Address;
use k2_core::zen::{lock, phone as garden_phone, widgets, ZenFiles};

use super::budget::{self, BudgetKey, Spend};
use super::call::{check_cap, needs_editor, resolve, CallBody, Placed};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::sources::SourceErr;
use crate::sources_routes as routes;

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

fn run(b: &CallBody, who: &Who) -> Result<J, CliResponse> {
    let (w, bound) = place(b, &b.verb, Spend::Read)?;
    let refusal = |e: SourceErr| e.response();
    match b.verb.as_str() {
        "sources.list" => list(&w, &bound, who),
        "sources.read" => {
            let s = local_source(b, &bound).map_err(refusal)?;
            read_view(who, &s).map_err(refusal)
        }
        "sources.history" => {
            let s = local_source(b, &bound).map_err(refusal)?;
            let params = history_params(b, &s).map_err(refusal)?;
            routes::history_route(who, &params).map_err(refusal)
        }
        "sources.refresh" => {
            let s = local_source(b, &bound).map_err(refusal)?;
            let body = json!({ "name": s.name }).to_string();
            let v = routes::refresh(who, body.as_bytes()).map_err(refusal)?;
            Ok(json!({ "queued": v["queued"], "nextAt": v["nextAt"] }))
        }
        other => Err(crate::garden_data::response(&crate::garden_data::refused("unknown_verb", 400, format!("no verb {other}")))),
    }
}

/// Everything a door row needs before it runs: the placement and live hash
/// (`stale_widget`), the row's cap (`cap_not_granted`), the signer gate
/// (`needs_editor`), the read budget (`rate_limited`), and the addresses this
/// placement binds on a phone: from the merged `config`, so a phone's own
/// `config` may name a source and a desktop-only one stays out.
pub(crate) fn place(b: &CallBody, verb: &str, spend: Spend) -> Result<(Placed, Vec<Bound>), CliResponse> {
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
    Ok((w, of_addresses(&addresses, lock::this_host().as_deref())))
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

/// One bound address, canonical (the host lower-cased).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bound {
    pub(crate) address: String,
    pub(crate) name: String,
    /// On this server: a bare name, or `name::<this server>`.
    pub(crate) local: bool,
}

/// What can be read of each address. One that doesn't parse binds nothing
/// (`k2 zen validate` reports it).
fn of_addresses(addresses: &[String], here: Option<&str>) -> Vec<Bound> {
    let mut out: Vec<Bound> = Vec::new();
    for a in addresses {
        let Ok(parsed) = Address::parse(a) else { continue };
        let (address, local) = match &parsed.host {
            None => (parsed.name.clone(), true),
            Some(h) => (format!("{}::{h}", parsed.name), here.is_some_and(|me| me.eq_ignore_ascii_case(h))),
        };
        if !out.iter().any(|b| b.address == address) {
            out.push(Bound { address, name: parsed.name, local });
        }
    }
    out
}

/// The verb's first argument, a source this placement binds and this server
/// holds: `failed` for no address, `not_bound` for one it doesn't bind,
/// `server_not_saved` for another server's.
fn local_source(b: &CallBody, bound: &[Bound]) -> Result<Bound, SourceErr> {
    let Some(given) = b.args.first().and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(SourceErr::new("failed", 400, format!("{} needs a source address", b.verb)));
    };
    let parsed = Address::parse(given).map_err(|_| SourceErr::new("failed", 400, format!("'{given}' isn't a source address (name or name::host)")))?;
    let address = match &parsed.host {
        None => parsed.name.clone(),
        Some(h) => format!("{}::{h}", parsed.name),
    };
    let Some(s) = bound.iter().find(|x| x.address == address) else {
        return Err(SourceErr::new("not_bound", 403, format!("{address} isn't one of this placement's sources (config.source / config.sources)")).with("room", address));
    };
    if !s.local {
        return Err(SourceErr::new("server_not_saved", 409, "On a phone a widget reads this server's sources only for now").with("room", address));
    }
    Ok(s.clone())
}

fn history_params(b: &CallBody, s: &Bound) -> Result<HashMap<String, String>, SourceErr> {
    let Some(field) = b.args.get(1).and_then(J::as_str).filter(|f| !f.is_empty()) else {
        return Err(SourceErr::new("failed", 400, "sources.history needs a field name"));
    };
    let mut params: HashMap<String, String> = [("name".to_string(), s.name.clone()), ("field".to_string(), field.to_string())].into();
    let opts = b.args.get(2);
    for (key, param) in [("fromAt", "from"), ("toAt", "to")] {
        if let Some(n) = opts.and_then(|o| o[key].as_f64()).filter(|n| n.is_finite()) {
            params.insert(param.to_string(), (n.floor() as i64).to_string());
        }
    }
    Ok(params)
}

fn read_view(who: &Who, s: &Bound) -> Result<J, SourceErr> {
    let params: HashMap<String, String> = [("name".to_string(), s.name.clone())].into();
    Ok(frame_view(&s.address, routes::read(who, &params)?))
}

/// The daemon's card view as a frame gets it (the renderer's `zenSourceView`):
/// the address and reach added, `fields` as scalars (anything else as JSON
/// text). The view's own shape is closed: `sources::view::SourceResult`.
pub(crate) fn frame_view(address: &str, mut view: J) -> J {
    fn scalars(v: &mut J) {
        if let Some(fields) = v.as_object_mut() {
            for x in fields.values_mut() {
                if x.is_object() || x.is_array() {
                    *x = J::String(x.to_string());
                }
            }
        }
    }
    scalars(&mut view["fields"]);
    if let Some(items) = view["items"].as_array_mut() {
        for it in items {
            scalars(&mut it["fields"]);
        }
    }
    view["address"] = json!(address);
    view["reach"] = json!("ok");
    view
}

/// `sources.list`: one row per bound address, what can't be read named in its
/// `reach`. Each read counts against the placement's budget.
fn list(w: &Placed, bound: &[Bound], who: &Who) -> Result<J, CliResponse> {
    if bound.len() > 1 {
        let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
        budget::spend(&key, Spend::Reads(bound.len() as u32 - 1)).map_err(|e| crate::garden_data::response(&e))?;
    }
    let rows: Vec<J> = bound
        .iter()
        .map(|s| {
            let unread = |reach: &str| json!({ "address": s.address, "label": null, "health": null, "phase": null, "display": null, "reach": reach });
            if !s.local {
                return unread("server_not_saved");
            }
            match read_view(who, s) {
                Ok(v) => json!({ "address": s.address, "label": v["label"], "health": v["health"], "phase": v["phase"], "display": v["display"], "reach": "ok" }),
                Err(e) if e.code == "removed" || e.code == "cap_not_granted" => unread(e.code),
                Err(_) => unread("unreachable"),
            }
        })
        .collect();
    Ok(J::Array(rows))
}

/// `GET /cli/zen/remote/sources-changes` (contract rev 2 §G): the card views of
/// this placement's bound sources that changed since `since`, and which went.
/// Same gate as a door row, but not a verb, so a widget can't call it. One call
/// per `sources_changed` instead of one read per source.
pub(crate) fn sources_changes(b: &CallBody, since: i64, who: &Who) -> CliResponse {
    let (_, bound) = match place(b, "sources.read", Spend::Read) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let locals: Vec<&Bound> = bound.iter().filter(|s| s.local).collect();
    let keep = |name: &str| locals.iter().any(|s| s.name == name);
    let c = match routes::changes_for(who, since, &keep, true) {
        Ok(c) => c,
        Err(e) => return e.response(),
    };
    let address_of = |name: &str| locals.iter().find(|s| s.name == name).map_or_else(|| name.to_string(), |s| s.address.clone());
    let changed: Vec<J> = c
        .changed
        .into_iter()
        .map(|view| {
            let address = address_of(&view.name);
            frame_view(&address, serde_json::to_value(view).unwrap_or(J::Null))
        })
        .collect();
    let removed: Vec<String> = c.removed.iter().map(|n| address_of(n)).collect();
    crate::garden_data::ok(json!({ "ok": true, "rev": c.rev, "full": c.full, "changed": changed, "removed": removed }))
}

#[cfg(test)]
#[path = "phone_rows_tests.rs"]
mod tests;
