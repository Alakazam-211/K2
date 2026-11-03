//! A Garden's mobile layout: `page.phone` (contract-gardens-phone rev 2 §A).
//!
//! **One Garden, two versions (Rosson 2026-11-03):** every device shows the
//! same list of Gardens. A Garden is never left off a phone's list; one with
//! no mobile version uses the default layout. A widget may be left out of
//! `page.phone`, which is a layout choice inside the Garden. There is no
//! per-device visibility field, and none may be added.
//!
//! The file's `[phone]` is walked by `schema_phone`; this module is what the
//! page and the door read: [`settle`] (the `page.phone` a page answers), and
//! for one placement its merged `config` ([`merged_config`]), its height
//! ([`hello_height`]), its `frameVersion` ([`frame_version`]) and the sources
//! it binds on a phone ([`source_addresses`]).

use std::sync::OnceLock;

use serde_json::{json, Map, Value as J};

/// A phone layout has 1 to this many screens.
pub const MAX_SCREENS: usize = 12;
/// A screen label, in characters.
pub const LABEL_MAX: usize = 24;
/// A widget's px height, on `[[phone.screen.widget]]`.
pub const HEIGHT_MIN: u16 = 160;
pub const HEIGHT_MAX: u16 = 4000;
/// `height = "fill"`: fills the screen; the widget must be alone on it.
pub const HEIGHT_FILL: &str = "fill";
/// `height = "auto"`: starts at 60% of the visible height, then follows the
/// frame's reports.
pub const HEIGHT_AUTO: &str = "auto";

/// How the phone runs a placement (contract rev 1 §3 `remote.widgets[].run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Frame,
    Native,
    Placeholder,
}

impl Run {
    pub fn as_str(self) -> &'static str {
        match self {
            Run::Frame => "frame",
            Run::Native => "native",
            Run::Placeholder => "placeholder",
        }
    }
}

pub fn run_of_kind(kind: &str) -> Run {
    match kind {
        super::schema::CUSTOM_KIND => Run::Frame,
        "agents" | "conversation" | "garden-empty" => Run::Native,
        _ => Run::Placeholder,
    }
}

/// `config` merged, key by key, over the placement's desktop `config`: the
/// phone's key wins, and what it doesn't name stays. A replace would unbind
/// a canvas's sources (contract rev 2 §A.3).
pub fn merge_config(widget: &J, phone: Option<&J>) -> J {
    let mut out: Map<String, J> = widget["props"].get("config").and_then(J::as_object).cloned().unwrap_or_default();
    if let Some(p) = phone.and_then(J::as_object) {
        for (k, v) in p {
            out.insert(k.clone(), v.clone());
        }
    }
    J::Object(out)
}

/// A template's `[phone]`, in the wire shape. A template is K2's own data,
/// so this only reshapes; `settle` drops what doesn't fit its widgets.
pub fn from_template(t: &J) -> Option<J> {
    let screens = t.get("phone")?.get("screen")?.as_array()?;
    let screens: Vec<J> = screens
        .iter()
        .map(|s| {
            let mut out = Map::new();
            if let Some(l) = s.get("label") {
                out.insert("label".into(), l.clone());
            }
            if s.get("canvas").and_then(J::as_str) == Some("full") {
                out.insert("canvas".into(), json!("full"));
            }
            out.insert("widgets".into(), s.get("widget").cloned().unwrap_or_else(|| json!([])));
            J::Object(out)
        })
        .collect();
    Some(json!({ "screens": screens }))
}

/// Set `page.phone` to what this page answers: the file's `[phone]`, else
/// the template's unless the file replaced the template's widgets
/// (`replaced_content`), with anything that no longer fits left out: an id
/// that isn't a content widget, `config` on a built-in kind or merged too
/// long, then a screen with nothing left, then a layout with no screens
/// (never an error: a K2 update may move a template under a good file). The
/// key is absent unless there is a layout, so every other page answers byte
/// for byte as before.
pub fn settle(page: &mut J, file_phone: Option<&J>, replaced_content: bool) {
    let phone = match file_phone {
        Some(p) => Some(p.clone()),
        None if replaced_content => None,
        None => page.get("phone").cloned(),
    };
    if let Some(o) = page.as_object_mut() {
        o.remove("phone");
    }
    let Some(phone) = phone else { return };
    let content: Vec<J> = page["widgets"].as_array().cloned().unwrap_or_default();
    let mut screens = Vec::new();
    for s in phone["screens"].as_array().into_iter().flatten() {
        let mut rows = Vec::new();
        for row in s["widgets"].as_array().into_iter().flatten() {
            let Some(w) = content.iter().find(|w| w["id"] == row["id"]) else { continue };
            let mut row = row.clone();
            let fits = w["kind"] == super::schema::CUSTOM_KIND
                && merge_config(w, row.get("config")).to_string().len() <= super::schema::MAX_CONFIG_BYTES;
            if !fits {
                if let Some(o) = row.as_object_mut() {
                    o.remove("config");
                }
            }
            rows.push(row);
        }
        if rows.is_empty() {
            continue;
        }
        let mut s = s.clone();
        s["widgets"] = J::Array(rows);
        screens.push(s);
    }
    if !screens.is_empty() {
        page["phone"] = json!({ "screens": screens });
    }
}

/// Where a placement's phone `config` key sits in the file, by placement id
/// (a page's list may have left rows out, so indexes don't carry over).
pub(crate) fn config_pos_key(id: &str, key: &str) -> String {
    format!("page.phone.config.{id}.{key}")
}

/// The `page.phone` row for a placement, with its screen. `None`: no layout,
/// or the layout leaves it out.
pub fn row_of<'a>(page: &'a J, placement: &str) -> Option<(&'a J, &'a J)> {
    for s in page.get("phone")?["screens"].as_array()? {
        for row in s["widgets"].as_array()? {
            if row["id"] == placement {
                return Some((s, row));
            }
        }
    }
    None
}

/// `hello.config`: the desktop's `config` with this placement's phone `config`
/// merged over it. With no layout, or a placement it leaves out, the desktop's.
pub fn merged_config(page: &J, widget: &J) -> J {
    let row = widget["id"].as_str().and_then(|id| row_of(page, id));
    merge_config(widget, row.and_then(|(_, r)| r.get("config")))
}

/// `hello.height` (contract rev 2 §A.4, §B.1): the `page.phone` row's value,
/// else its default (`fill` alone on its screen, `auto` in a stack). With no
/// `page.phone`, from the placement's column, because the fallback is one
/// column per screen: `fill` when it's the only widget in the column's body,
/// else `auto`. Native and placeholder widgets count; controls and band items
/// don't. `None` for a placement `page.phone` leaves out.
pub fn hello_height(page: &J, widget: &J) -> Option<J> {
    let alone = |n: usize| json!(if n <= 1 { HEIGHT_FILL } else { HEIGHT_AUTO });
    if page.get("phone").is_some() {
        let (screen, row) = row_of(page, widget["id"].as_str()?)?;
        return Some(match row.get("height") {
            Some(h) => h.clone(),
            None => alone(screen["widgets"].as_array().map_or(0, Vec::len)),
        });
    }
    let column = widget["column"].as_u64().unwrap_or(0);
    let body = page["widgets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| super::spot_of(w) == super::Spot::Body && w["column"].as_u64().unwrap_or(0) == column)
        .count();
    Some(alone(body))
}

/// A fingerprint of K2's own part of a frame document: the sealed guard, the
/// frame runtime and the wrapper's shape. A K2 update that changes any of
/// them changes the document without changing a widget's `hash`.
fn runtime_fingerprint() -> &'static str {
    static FP: OnceLock<String> = OnceLock::new();
    FP.get_or_init(|| {
        use super::widget_shot::{FRAME_SHAPE_GOLDEN, K2_FRAME_JS, SEALED_FRAME_GUARD};
        super::schema::fnv_hex(format!("{SEALED_FRAME_GUARD}\n{K2_FRAME_JS}\n{FRAME_SHAPE_GOLDEN}").as_bytes())
    })
}

/// `frameVersion` (contract rev 2 §C): what a phone reloads a frame on and
/// caches its document by. It covers the bundle `hash`, what the hello carries
/// from the page (`config`, `height`, `caps`) and K2's frame runtime, so it
/// moves on a `config` edit, which `hash` doesn't, and on nothing else on the
/// page. `caps` is here because signing a widget changes them with no change to
/// its code: a frame keeping the old hello would give a wrong `k2.can`.
pub fn frame_version(hash: &str, config: &J, height: Option<&J>, caps: &J) -> String {
    let covered = json!({ "hash": hash, "config": config, "height": height, "caps": caps, "runtime": runtime_fingerprint() });
    super::schema::fnv_hex(covered.to_string().as_bytes())
}

/// The sources this placement binds on a phone: the manifest's, from the
/// merged `config`. Empty when `page.phone` leaves it out (nothing runs).
pub fn source_addresses(page: &J, widget: &J, sources: Option<&J>) -> Vec<String> {
    let Some((_, row)) = widget["id"].as_str().and_then(|id| row_of(page, id)) else { return Vec::new() };
    super::widgets::placement_source_addresses(sources, &merge_config(widget, row.get("config")))
}

/// Every source this placement binds on any device: the desktop's, then what
/// a phone's `config` adds. What a lock protects and Settings shows; the
/// desktop frame itself still reads only its own `sourceAddresses`.
pub fn all_source_addresses(page: &J, widget: &J, sources: Option<&J>) -> Vec<String> {
    let config = widget["props"].get("config").cloned().unwrap_or_else(|| json!({}));
    let mut out = super::widgets::placement_source_addresses(sources, &config);
    for a in source_addresses(page, widget, sources) {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
#[path = "phone_tests.rs"]
mod tests;
