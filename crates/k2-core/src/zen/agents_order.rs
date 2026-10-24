//! The person's order and pins in each Garden's Agents widget, on this
//! computer (0.46.4; Baden via Appa 2026-10-22;
//! prd-garden-column-collapse-and-agent-list-v1 §3 and Q3).
//!
//! `agents-order.json` (daemon-written): per Garden id,
//! - `order`: the person's drag order, full row addresses as
//!   `agents.list()` gives them (`handle::local`, `host:port`), first first.
//!   It sits on top of the widget's own order (the Home's, the Agents
//!   view's, or a `list-from` list's): the person's order wins (Q3).
//! - `pins`: this Garden's own pins. Absent until the person pins or unpins
//!   an agent in this Garden; until then the pin grid shows each agent's
//!   workspace pin (`projects.pinned`), as before. Once present, this list
//!   alone decides the Garden's pin grid.
//!
//! It is the person's view state on this computer:
//! - never in a Garden file, so never in a Garden's TOML or its history;
//! - never in Garden sync (`sync.rs` never reads it, and the refresh
//!   fingerprint doesn't cover it, so a change is not a `zen_changed`);
//! - agents read it (`GET /cli/zen/agents/order`, `k2 zen agents-order`),
//!   and only the person sets it (`POST /cli/zen/agents/order/set`, the
//!   owner token: the app). Agents never write the file
//!   ([`super::widget_access::NEVER_WRITE`]).
//!
//! The lists only reorder (or pin) rows already in a widget's reach: an
//! address that isn't drawn is ignored when drawing, so they never widen
//! what a widget shows. Bounded: at most [`MAX_ADDRESSES`] per list, each a
//! one-line string of at most [`MAX_ADDRESS_BYTES`] bytes. A deleted
//! Garden's entry goes with it ([`ZenFiles::delete_garden`]).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::store::{ZenError, ZenFiles};

/// The file, under `~/.k2/zen/`.
pub const AGENTS_ORDER_FILE: &str = "agents-order.json";
/// `agents-order.json`'s `version`.
pub const AGENTS_ORDER_VERSION: u32 = 1;
/// Addresses one list may hold.
pub const MAX_ADDRESSES: usize = 200;
/// Bytes one address may take.
pub const MAX_ADDRESS_BYTES: usize = 256;

static LOCK: Mutex<()> = Mutex::new(());

fn file_lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// One Garden's view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GardenAgentsView {
    /// The person's drag order; `None` = never dragged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    /// This Garden's own pins; `None` = the workspace pins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

impl GardenAgentsView {
    fn is_empty(&self) -> bool {
        self.order.is_none() && self.pins.is_none()
    }
}

/// `agents-order.json`. Unknown top-level keys (a newer K2's) are kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsOrderFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub gardens: BTreeMap<String, GardenAgentsView>,
    #[serde(flatten)]
    pub other: BTreeMap<String, J>,
}

/// One change to a Garden's view. For each list: `None` leaves it as it
/// is, `Some(None)` clears it (back to the widget's order, or the
/// workspace pins), `Some(Some(list))` sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentsViewChange {
    pub order: Option<Option<Vec<String>>>,
    pub pins: Option<Option<Vec<String>>>,
}

/// A list as stored: each address trimmed, one line, at most
/// [`MAX_ADDRESS_BYTES`]; duplicates (case aside) dropped, the first kept;
/// at most [`MAX_ADDRESSES`]. `what` names the list in the refusal.
pub fn clean_addresses(what: &str, list: &[String]) -> Result<Vec<String>, ZenError> {
    if list.len() > MAX_ADDRESSES {
        return Err(ZenError::BadRequest(format!(
            "{what} holds at most {MAX_ADDRESSES} addresses (got {})",
            list.len()
        )));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (i, raw) in list.iter().enumerate() {
        let a = raw.trim();
        if a.is_empty() {
            return Err(ZenError::BadRequest(format!("{what}[{i}] is empty: give a row address (handle::host)")));
        }
        if a.len() > MAX_ADDRESS_BYTES {
            return Err(ZenError::BadRequest(format!(
                "{what}[{i}] is longer than {MAX_ADDRESS_BYTES} bytes: give a row address (handle::host)"
            )));
        }
        if a.chars().any(char::is_control) {
            return Err(ZenError::BadRequest(format!("{what}[{i}] has a control character")));
        }
        if seen.insert(a.to_lowercase()) {
            out.push(a.to_string());
        }
    }
    Ok(out)
}

impl ZenFiles {
    pub fn agents_order_path(&self) -> PathBuf {
        self.root().join(AGENTS_ORDER_FILE)
    }

    /// `agents-order.json`, or an empty one (nothing set) when missing or
    /// unreadable.
    pub fn read_agents_order(&self) -> AgentsOrderFile {
        std::fs::read_to_string(self.agents_order_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| AgentsOrderFile { version: AGENTS_ORDER_VERSION, ..AgentsOrderFile::default() })
    }

    fn write_agents_order(&self, file: &AgentsOrderFile) -> Result<(), ZenError> {
        let path = self.agents_order_path();
        let mut file = file.clone();
        file.version = AGENTS_ORDER_VERSION;
        let body = serde_json::to_string_pretty(&file).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n"))
            .map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))
    }

    /// One Garden's view, as the routes answer it.
    fn agents_view_row(id: &str, name: &str, v: Option<&GardenAgentsView>) -> J {
        let v = v.cloned().unwrap_or_default();
        json!({
            "id": id,
            "name": name,
            "order": v.order,
            "pins": v.pins,
            "updatedAt": if v.updated_at.is_empty() { J::Null } else { J::String(v.updated_at) },
        })
    }

    /// `GET /cli/zen/agents/order`: every Garden in list order (or just
    /// `garden`, an id or a name), each with its `order` and `pins` (`null`
    /// when the person never set them). Entries of Gardens no longer in the
    /// list are never shown.
    pub fn agents_order_json(&self, garden: Option<&str>) -> Result<J, ZenError> {
        self.require()?;
        let file = self.read_agents_order();
        let gardens = match garden.map(str::trim).filter(|g| !g.is_empty()) {
            Some(sel) => vec![self.garden(sel)?.1],
            None => self.gardens(),
        };
        let rows: Vec<J> =
            gardens.iter().map(|g| Self::agents_view_row(&g.id, &g.name, file.gardens.get(&g.id))).collect();
        Ok(json!({ "ok": true, "gardens": rows }))
    }

    /// `POST /cli/zen/agents/order/set`: apply `change` to `garden` (an id
    /// or a name). Returns the Garden's row and whether anything changed.
    /// Entries of Gardens no longer in the list are dropped on the way.
    pub fn set_agents_view(&self, garden: &str, change: AgentsViewChange) -> Result<(J, bool), ZenError> {
        self.require()?;
        if change.order.is_none() && change.pins.is_none() {
            return Err(ZenError::BadRequest("agents/order/set needs order or pins (a list, or null to clear)".into()));
        }
        let order = match &change.order {
            Some(Some(l)) => Some(Some(clean_addresses("order", l)?)),
            Some(None) => Some(None),
            None => None,
        };
        let pins = match &change.pins {
            Some(Some(l)) => Some(Some(clean_addresses("pins", l)?)),
            Some(None) => Some(None),
            None => None,
        };
        let (_, g) = self.garden(garden)?;
        let _l = file_lock();
        let mut file = self.read_agents_order();
        let known: HashSet<String> = self.gardens().into_iter().map(|g| g.id).collect();
        let before_len = file.gardens.len();
        file.gardens.retain(|id, _| known.contains(id));
        let pruned = file.gardens.len() != before_len;
        let before = file.gardens.get(&g.id).cloned().unwrap_or_default();
        let mut next = before.clone();
        if let Some(o) = order {
            next.order = o;
        }
        if let Some(p) = pins {
            next.pins = p;
        }
        let changed = next.order != before.order || next.pins != before.pins;
        if changed {
            if next.is_empty() {
                file.gardens.remove(&g.id);
            } else {
                next.updated_at = Self::now();
                file.gardens.insert(g.id.clone(), next);
            }
        }
        if changed || pruned {
            self.write_agents_order(&file)?;
        }
        Ok((Self::agents_view_row(&g.id, &g.name, file.gardens.get(&g.id)), changed))
    }

    /// A deleted Garden's view goes with it. Writes only when there was one.
    pub(crate) fn agents_order_on_delete(&self, id: &str) -> Result<(), ZenError> {
        let _l = file_lock();
        let mut file = self.read_agents_order();
        if file.gardens.remove(id).is_some() {
            self.write_agents_order(&file)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A Zen folder under the temp dir, set up. Never the real `~/.k2`.
    fn set_up(tag: &str) -> (Tmp, ZenFiles) {
        let d = std::env::temp_dir().join(format!("k2-zen-agents-order-{tag}-{}", uuid::Uuid::new_v4().simple()));
        let f = ZenFiles::new(d.join(".k2").join("zen"));
        assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
        f.setup().expect("setup");
        (Tmp(d), f)
    }

    fn gid(f: &ZenFiles, name: &str) -> String {
        f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}: {:?}", f.gardens())).1.id
    }

    fn list(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn row<'a>(v: &'a J, id: &str) -> &'a J {
        v["gardens"]
            .as_array()
            .unwrap_or_else(|| panic!("gardens must be an array: {v}"))
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}: {v}"))
    }

    #[test]
    fn a_new_computer_has_no_order_and_no_garden_pins() {
        let (_t, f) = set_up("fresh");
        let v = f.agents_order_json(None).expect("read");
        let rows = v["gardens"].as_array().expect("array");
        assert_eq!(rows.len(), f.gardens().len(), "one row per Garden: {v}");
        for r in rows {
            assert_eq!(r["order"], J::Null, "{v}");
            assert_eq!(r["pins"], J::Null, "{v}");
            assert_eq!(r["updatedAt"], J::Null, "{v}");
        }
        assert!(!f.agents_order_path().exists(), "reading writes nothing");
    }

    #[test]
    fn order_and_pins_are_set_per_garden_and_cleared_with_null() {
        let (_t, f) = set_up("set");
        let g1 = gid(&f, "Garden 1");
        let g2 = gid(&f, "Garden 2");
        let change = AgentsViewChange {
            order: Some(Some(list(&["b::local", " a::local ", "B::LOCAL", "host:7000"]))),
            pins: None,
        };
        let (r, changed) = f.set_agents_view("Garden 1", change.clone()).expect("set order");
        assert!(changed, "{r}");
        assert_eq!(r["order"], json!(["b::local", "a::local", "host:7000"]), "trimmed, de-duplicated case aside: {r}");
        assert_eq!(r["pins"], J::Null, "pins untouched: {r}");
        assert!(r["updatedAt"].is_string(), "{r}");
        // The same change again changes nothing.
        let (_, again) = f.set_agents_view(&g1, change).expect("again");
        assert!(!again, "an identical order is a no-op");
        // Pins on Garden 2 only.
        let (r2, _) = f
            .set_agents_view(&g2, AgentsViewChange { order: None, pins: Some(Some(list(&["a::local"]))) })
            .expect("pins");
        assert_eq!(r2["pins"], json!(["a::local"]), "{r2}");
        let all = f.agents_order_json(None).expect("read");
        assert_eq!(row(&all, &g1)["pins"], J::Null, "Garden 1 keeps the workspace pins: {all}");
        assert_eq!(row(&all, &g2)["order"], J::Null, "Garden 2 keeps its widget's order: {all}");
        // An empty pin list is a choice (no pins here), not "the workspace pins".
        let (r2, _) = f.set_agents_view(&g2, AgentsViewChange { order: None, pins: Some(Some(vec![])) }).expect("empty pins");
        assert_eq!(r2["pins"], json!([]), "{r2}");
        // null clears: back to the default, and the entry goes when both are clear.
        let (r1, changed) = f.set_agents_view(&g1, AgentsViewChange { order: Some(None), pins: None }).expect("clear");
        assert!(changed);
        assert_eq!(r1["order"], J::Null, "{r1}");
        let stored = f.read_agents_order();
        assert!(!stored.gardens.contains_key(&g1), "a cleared Garden leaves the file: {stored:?}");
        assert_eq!(stored.version, AGENTS_ORDER_VERSION);
        // Just one Garden.
        let one = f.agents_order_json(Some("Garden 2")).expect("one");
        assert_eq!(one["gardens"].as_array().map(Vec::len), Some(1), "{one}");
        assert_eq!(one["gardens"][0]["id"], g2.as_str(), "{one}");
    }

    #[test]
    fn bad_lists_and_unknown_gardens_are_refused_and_nothing_is_written() {
        let (_t, f) = set_up("bad");
        let too_many: Vec<String> = (0..=MAX_ADDRESSES).map(|i| format!("a{i}::local")).collect();
        let cases: Vec<(&str, AgentsViewChange, &str)> = vec![
            ("Garden 1", AgentsViewChange::default(), "needs order or pins"),
            ("Garden 1", AgentsViewChange { order: Some(Some(too_many)), pins: None }, "at most 200"),
            ("Garden 1", AgentsViewChange { order: Some(Some(list(&["ok::local", "  "]))), pins: None }, "order[1] is empty"),
            ("Garden 1", AgentsViewChange { order: None, pins: Some(Some(vec!["x".repeat(MAX_ADDRESS_BYTES + 1)])) }, "pins[0] is longer"),
            ("Garden 1", AgentsViewChange { order: Some(Some(list(&["a::local\nb"]))), pins: None }, "control character"),
        ];
        for (garden, change, want) in cases {
            match f.set_agents_view(garden, change.clone()) {
                Err(ZenError::BadRequest(m)) => assert!(m.contains(want), "{change:?}: {m:?} must contain {want:?}"),
                other => panic!("{change:?}: want BadRequest containing {want:?}, got {other:?}"),
            }
        }
        match f.set_agents_view("No Such Garden", AgentsViewChange { order: Some(Some(list(&["a::local"]))), pins: None }) {
            Err(ZenError::UnknownGarden { garden, .. }) => assert_eq!(garden, "No Such Garden"),
            other => panic!("want UnknownGarden, got {other:?}"),
        }
        assert!(!f.agents_order_path().exists(), "a refused change writes nothing");
        // Exactly the cap is fine.
        let cap: Vec<String> = (0..MAX_ADDRESSES).map(|i| format!("a{i}::local")).collect();
        let (r, _) = f.set_agents_view("Garden 1", AgentsViewChange { order: Some(Some(cap)), pins: None }).expect("cap");
        assert_eq!(r["order"].as_array().map(Vec::len), Some(MAX_ADDRESSES), "{r}");
    }

    #[test]
    fn deleting_a_garden_drops_its_entry_and_sync_never_carries_it() {
        let (_t, f) = set_up("delete");
        let g2 = gid(&f, "Garden 2");
        let g1 = gid(&f, "Garden 1");
        f.set_agents_view(&g2, AgentsViewChange { order: Some(Some(list(&["zz-marker::local"]))), pins: None })
            .expect("set g2");
        f.set_agents_view(&g1, AgentsViewChange { order: None, pins: Some(Some(list(&["zz-pin::local"]))) })
            .expect("set g1");
        // Not Garden sync's: its state and the refresh fingerprint never
        // mention it.
        let sync = serde_json::to_string(&f.sync_fingerprint_state()).expect("sync state");
        assert!(!sync.contains("zz-marker") && !sync.contains(AGENTS_ORDER_FILE), "sync state: {sync}");
        let list_json = serde_json::to_string(&f.sync_list().expect("sync list")).expect("json");
        assert!(!list_json.contains("zz-marker") && !list_json.contains("zz-pin"), "sync list: {list_json}");
        let (fp_before, _) = f.refresh_with_report(&Default::default()).expect("refresh");
        f.set_agents_view(&g1, AgentsViewChange { order: Some(Some(list(&["zz-other::local"]))), pins: None })
            .expect("change g1");
        let (fp_after, _) = f.refresh_with_report(&Default::default()).expect("refresh");
        assert_eq!(fp_before, fp_after, "a view change is never a zen_changed");
        // Nor any Garden page.
        for g in f.gardens() {
            if let Some(Ok(text)) = f.read_current(&super::super::ZenFile::Garden(g.id.clone())) {
                assert!(!text.contains("zz-"), "{}: the Garden's TOML never holds the view: {text}", g.id);
            }
        }
        f.delete_garden(&g2).expect("delete Garden 2");
        let stored = f.read_agents_order();
        assert!(!stored.gardens.contains_key(&g2), "the deleted Garden's view is gone: {stored:?}");
        assert!(stored.gardens.contains_key(&g1), "the others stay: {stored:?}");
    }

    #[test]
    fn a_stale_entry_is_never_shown_and_is_pruned_on_the_next_write() {
        let (_t, f) = set_up("stale");
        let g1 = gid(&f, "Garden 1");
        let mut file = f.read_agents_order();
        file.gardens.insert(
            "g-gone0000".into(),
            GardenAgentsView { order: Some(list(&["a::local"])), pins: None, updated_at: String::new() },
        );
        file.other.insert("newerKey".into(), json!(1));
        f.write_agents_order(&file).expect("write");
        let v = f.agents_order_json(None).expect("read");
        assert!(!v.to_string().contains("g-gone0000"), "{v}");
        f.set_agents_view(&g1, AgentsViewChange { order: Some(Some(list(&["a::local"]))), pins: None }).expect("set");
        let stored = f.read_agents_order();
        assert!(!stored.gardens.contains_key("g-gone0000"), "{stored:?}");
        assert_eq!(stored.other.get("newerKey"), Some(&json!(1)), "a newer K2's keys are kept: {stored:?}");
    }
}
