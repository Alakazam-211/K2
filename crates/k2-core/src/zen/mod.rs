//! Zen Mode core (prd-zen-mode-v1 S1, S3; prd-zen-gardens-v1 S1).
//!
//! `~/.k2/zen/` belongs to the LOCAL daemon (Z8). Zen is a mode of the
//! window, and in Zen the window shows a **Garden**: a personal page that
//! lives on this computer. The folder holds only the user's changes,
//! layered over read-only defaults built into K2:
//!
//! - built-in themes ([`BUILTIN_THEMES`], embedded TOML) and the built-in
//!   page templates `k2.texting@1` (Garden 1) and `k2.blank@1`
//!   (Garden 2 and every new Garden) are the defaults; app updates replace them and
//!   never touch user files;
//! - `themes/<name>/theme.toml` (+ an optional background image) is a
//!   user theme bundle, or the user's override of the built-in of that
//!   name;
//! - `zen.toml` is the user's changes for every Garden on top of the
//!   active theme; `gardens/<id>.toml` is one Garden's page: its template,
//!   theme override, and optional layout of built-in widgets (G38);
//! - `gardens.json` (daemon-written) is the Garden list: ids, names,
//!   order, templates;
//! - `active.json` (daemon-written) names the active theme, globally and
//!   per Garden (G16: one theme plus an optional per-Garden override);
//! - `.history/` (the last 20 good versions of each file, and deleted
//!   Gardens) is the daemon's own.
//!
//! The stack for one Garden: built-in `basic` → built-in `<active>` →
//! `themes/<active>/theme.toml` → `zen.toml` → `gardens/<id>.toml`.
//!
//! Zen v2 (prd-zen-user-widgets-v2): `widgets/<name>/` holds custom
//! widgets an agent writes ([`widgets`], [`bundle`], [`widget_store`]);
//! a Garden places one with `kind = "custom"`. What a widget may do is a
//! signed row in the daemon's database ([`grants`]), made only by the
//! owner's click in the K2 app; `grants.json` is never read or written.
//! Ready-made Gardens (the Diary) are data in [`garden_catalog`], and every
//! built-in default is read through [`Defaults`] (sync-defaults D0).
//!
//! - [`schema`]: tokens, ranges, validation with `file:line:col`, resolve.
//! - [`store`]: the folder: setup, Gardens, last-good, history, reset.
//! - [`skill`]: the `k2-zen` skill body, generated from [`schema`].
//!
//! Page templates are data (Z10, G40): TOML shipped in this crate
//! ([`TEMPLATES`]) and returned as the resolved page's layout, widgets and
//! controls.

pub mod builtin_widgets;
pub mod bundle;
pub mod defaults;
pub mod garden_catalog;
pub mod grants;
pub mod schema;
pub mod skill;
pub mod stdlib;
pub mod store;
pub mod widget_store;
pub mod widgets;

use std::path::PathBuf;

pub use defaults::Defaults;

use serde_json::{json, Value as J};

pub use schema::{Diagnostic, FileKind, Layer, BLANK_TEMPLATE_ID, SCHEMA_VERSION, TEMPLATE_ID};
pub use store::{GardenEntry, ZenError, ZenFile, ZenFiles};

/// The `zen.toml` written for a new computer, and the stub `reset` restores:
/// `schema = 1` and comments, no tables. The defaults live in the built-in
/// themes, so `~/.k2/zen` holds only the user's changes.
pub const DEFAULT_ZEN_TOML: &str = include_str!("default-zen.toml");

/// The theme every other theme starts from, and the one a new computer uses:
/// K2's built-in `basic` (Rosson 2026-10-04: renamed from `default`).
pub const DEFAULT_THEME: &str = "basic";

/// The built-in theme's name before it was `basic`. Zen never shipped, so
/// nothing is migrated; a saved pick of it in `active.json` reads as
/// [`DEFAULT_THEME`], quietly, unless the person has a theme of that name.
pub const DEFAULT_THEME_ALIAS: &str = "default";

/// A theme built into K2: read-only, embedded in the binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTheme {
    /// The id (lower case): `k2 zen theme set <name>`, `active.json`, the folder.
    pub name: &'static str,
    /// The name people see (Rosson 2026-10-04: capitalized, "Basic").
    pub label: &'static str,
    pub summary: &'static str,
    pub toml: &'static str,
}

/// Built-in themes, in cycle order (`k2 zen theme next`). `basic` is
/// first and complete; every other theme sits on top of it.
pub const BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        name: DEFAULT_THEME,
        label: "Basic",
        summary: "clean, smooth, simple: warm light, soft dark, follows the computer",
        toml: include_str!("themes/basic.toml"),
    },
    BuiltinTheme {
        name: "paper",
        label: "Paper",
        summary: "always light, serif type, ink-blue accent",
        toml: include_str!("themes/paper.toml"),
    },
    BuiltinTheme {
        name: "midnight",
        label: "Midnight",
        summary: "always dark, cool blue, JetBrains Mono everywhere",
        toml: include_str!("themes/midnight.toml"),
    },
];

pub fn builtin_theme(name: &str) -> Option<&'static BuiltinTheme> {
    BUILTIN_THEMES.iter().find(|t| t.name == name)
}

/// The name a theme shows in the picker and theme lists: a built-in's
/// label ("Basic"), else the id with its first letter capitalized
/// ("sunset" → "Sunset"). Ids stay lower case everywhere else.
pub fn theme_label(name: &str) -> String {
    if let Some(t) = builtin_theme(name) {
        return t.label.to_string();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The label a built-in's diagnostics carry.
fn builtin_label(name: &str) -> String {
    format!("builtin:{name}")
}

/// A built-in theme checked over `basic` (`basic` over nothing).
pub fn check_builtin_theme(name: &str) -> Option<schema::Checked> {
    let t = builtin_theme(name)?;
    let base = if name == DEFAULT_THEME { Layer::new() } else { builtin_layer().clone() };
    Some(schema::check(&builtin_label(name), t.toml, FileKind::Theme, &base))
}

/// The full built-in layer of a theme: `basic` merged with the named
/// built-in (just `basic` for `basic`). `None` when not built in. Read
/// through [`Defaults::live`] (D0).
pub fn builtin_theme_layer(name: &str) -> Option<&'static Layer> {
    Defaults::live().theme_layer(name)
}

/// The `k2.texting@1` template (Z10): Garden 1, the first Garden.
pub const TEXTING_TEMPLATE_TOML: &str = include_str!("template-k2-texting-1.toml");
/// The `k2.blank@1` template (G11): every new Garden.
pub const BLANK_TEMPLATE_TOML: &str = include_str!("template-k2-blank-1.toml");

/// Page templates are data (Z10, G40): `(id, TOML)`.
pub const TEMPLATES: &[(&str, &str)] = &[
    (schema::TEMPLATE_ID, TEXTING_TEMPLATE_TOML),
    (schema::BLANK_TEMPLATE_ID, BLANK_TEMPLATE_TOML),
];

/// Required page controls (Z27, G24; Rosson 2026-10-04: exactly two, the
/// Zen toggle and the Garden switcher). Every template places both as
/// chrome widgets and names both in `controls`. A Garden file that places
/// any chrome must place both (prd-zen-freeform-chrome FC2, FC24), so a
/// page can never drop one. `[[control]]` in a Garden file is ignored.
/// `drag-region` is bound for window drag but never checked or placed.
pub const REQUIRED_CONTROLS: &[&str] = &["zen-toggle", "garden-switcher"];

/// Bridge caps (Z34, G29). `thread:*` reuse the app-gateway names;
/// `agents:read`, `agents:add` and `presence:read` are Zen-bridge only.
/// `agents:add` lets a widget OPEN K2's Add agent picker for its Home
/// (`agents.add`); the human picks and K2 writes the row. `gardens:manage`
/// (create, rename, delete Gardens) is granted by K2 to the template's
/// controls only, never to a widget. `app:navigate` (the `nav-rail`
/// widget) switches the Garden's rail view (inside Zen) and reads the top
/// bar's badges. `gardens:template` (the `garden-empty` widget's "Start
/// with the default") turns the Garden the widget is on into a built-in
/// template (`gardens.useTemplate`), never another Garden.
pub const BRIDGE_CAPS: &[&str] = &[
    "agents:read",
    "agents:add",
    "presence:read",
    "thread:read",
    "thread:post",
    "gardens:manage",
    "gardens:template",
    "app:navigate",
];

/// The caps a custom widget may ask for and be granted
/// (prd-zen-user-widgets-v2 UW5, UW24). Equal to the catalog's caps exposed
/// to `widget` (`crate::contract`, asserted in `grants` tests), and a subset
/// of [`BRIDGE_CAPS`]. `agents:add`, `gardens:manage`, `gardens:template`
/// and `app:navigate` stay built-in only.
pub const USER_WIDGET_CAPS: &[&str] = &["agents:read", "presence:read", "thread:read", "thread:post"];

/// The caps K2 grants each built-in widget kind (`source: "builtin"`). A
/// Garden file never names caps (G38); these are the only ones.
pub const BUILTIN_WIDGET_CAPS: &[(&str, &[&str])] = &[
    ("agents", &["agents:read", "agents:add", "presence:read"]),
    ("conversation", &["agents:read", "presence:read", "thread:read", "thread:post"]),
    ("garden-empty", &["agents:read", "thread:read", "thread:post", "gardens:template"]),
    ("nav-rail", &["app:navigate"]),
];

/// K2's caps for a built-in widget kind.
pub fn widget_caps(kind: &str) -> Option<&'static [&'static str]> {
    BUILTIN_WIDGET_CAPS.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c)
}

/// The caps K2 grants each chrome kind (prd-zen-freeform-chrome FC31).
/// Chrome is drawn by K2 with the template bridge; a file never names caps.
/// Only the Garden switcher gets one (`gardens:manage`, for + New Garden).
pub const CHROME_CAPS: &[(&str, &[&str])] = &[
    ("garden-switcher", &["gardens:manage"]),
    ("zen-toggle", &[]),
    ("usage", &[]),
    ("theme-picker", &[]),
    ("menu", &[]),
];

/// K2's caps for a chrome kind (empty for an unknown one).
pub fn chrome_caps(kind: &str) -> &'static [&'static str] {
    CHROME_CAPS.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c).unwrap_or(&[])
}

/// The renderer's layout shape (docs/zen-contract.md): `split` and
/// `minWidths` per column; `columns` keeps the per-column detail.
pub fn layout_json(kind: &J, columns: &[J]) -> J {
    json!({
        "kind": kind,
        "split": columns.iter().map(|c| c["size"].clone()).collect::<Vec<_>>(),
        "minWidths": columns.iter().map(|c| c["min-width"].clone()).collect::<Vec<_>>(),
        "columns": columns,
    })
}

/// A built-in widget as the renderer reads it: K2's caps, `source:
/// "builtin"`, its `slot` (`column` unless the file put it in a band),
/// `edge` and `align` only when the file set them (FC28), every prop filled
/// (defaults, `agents.mode`, `nav-rail.orientation`).
pub fn builtin_widget(w: J) -> J {
    builtin_widget_in(Defaults::live(), w)
}

/// [`builtin_widget`] with the prop defaults of `d` (a Garden's own copy,
/// prd-zen-garden-sync-defaults-v1 GS20).
pub fn builtin_widget_in(d: &Defaults, mut w: J) -> J {
    let kind = w["kind"].as_str().unwrap_or_default().to_string();
    let slot = w["slot"].as_str().unwrap_or(schema::DEFAULT_SLOT).to_string();
    let edge = w["edge"].as_str().map(str::to_string);
    let mut props = w["props"].as_object().cloned().unwrap_or_default();
    schema::normalize_props_in(d, &kind, &mut props);
    schema::normalize_slot_props(&kind, &slot, edge.as_deref(), &mut props);
    w["slot"] = json!(slot);
    w["props"] = J::Object(props);
    w["caps"] = json!(widget_caps(&kind).unwrap_or(&[]));
    w["source"] = json!("builtin");
    w
}

/// A custom widget placement as the page carries it before the store fills
/// in what the folder and the grant say (prd-zen-user-widgets-v2 UW38):
/// `{id, kind: "custom", widget, column, props: {home?, agent?, config},
/// caps: [], requested: [], source: "user"}`. The store's resolve adds
/// `name`, `description`, `reasons`, `libs`, `hash`, `state`, `errors`,
/// `warnings`, `requested`, the effective `caps` and `grant`.
pub fn custom_widget(w: J) -> J {
    let mut props = serde_json::Map::new();
    for k in ["home", "agent"] {
        if let Some(v) = w["props"].get(k).filter(|v| v.is_string()) {
            props.insert(k.into(), v.clone());
        }
    }
    props.insert(
        "config".into(),
        w["props"].get("config").filter(|c| c.is_object()).cloned().unwrap_or_else(|| json!({})),
    );
    json!({
        "id": w["id"],
        "kind": schema::CUSTOM_KIND,
        "widget": w["widget"],
        "column": w["column"].as_u64().unwrap_or(0),
        "props": props,
        "caps": [],
        "requested": [],
        "source": "user",
    })
}

/// A content widget as the renderer reads it: a custom placement
/// ([`custom_widget`]) or a built-in ([`builtin_widget`]).
pub fn content_widget(w: J) -> J {
    content_widget_in(Defaults::live(), w)
}

/// [`content_widget`] with the prop defaults of `d` (GS20).
pub fn content_widget_in(d: &Defaults, w: J) -> J {
    if w["kind"] == schema::CUSTOM_KIND {
        custom_widget(w)
    } else {
        builtin_widget_in(d, w)
    }
}

/// A chrome item as the renderer reads it (prd-zen-freeform-chrome FC29):
/// `{id, kind, slot, column?, edge?, align?, menu?, props, caps}`. The id
/// defaults to the kind; a band item gets `align`; an item in a column
/// gets `edge` and `align`; a menu item gets `menu`. Every prop is filled
/// (`menu.icon`); caps are K2's (FC31).
pub fn chrome_item(w: &J) -> J {
    chrome_item_in(Defaults::live(), w)
}

/// [`chrome_item`] with the prop defaults of `d` (GS20).
pub fn chrome_item_in(d: &Defaults, w: &J) -> J {
    let kind = w["kind"].as_str().unwrap_or_default().to_string();
    let slot = w["slot"].as_str().unwrap_or(schema::DEFAULT_SLOT).to_string();
    let id = w["id"].as_str().unwrap_or(&kind).to_string();
    let mut props = w["props"].as_object().cloned().unwrap_or_default();
    schema::normalize_props_in(d, &kind, &mut props);
    let mut out = serde_json::Map::new();
    out.insert("id".into(), json!(id));
    out.insert("kind".into(), json!(kind));
    out.insert("slot".into(), json!(slot));
    let align = w["align"].as_str().unwrap_or(schema::DEFAULT_ALIGN);
    if schema::BAND_SLOTS.contains(&slot.as_str()) {
        out.insert("align".into(), json!(align));
    } else if slot == schema::MENU_SLOT {
        out.insert("menu".into(), w["menu"].clone());
    } else {
        out.insert("column".into(), json!(w["column"].as_u64().unwrap_or(0)));
        out.insert("edge".into(), json!(w["edge"].as_str().unwrap_or(schema::DEFAULT_EDGE)));
        out.insert("align".into(), json!(align));
    }
    out.insert("props".into(), J::Object(props));
    out.insert("caps".into(), json!(chrome_caps(&kind)));
    J::Object(out)
}

/// Where a page item is drawn: a band, a column edge, inside a menu, or the
/// body of a column (a content widget that fills it, or a column rail).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Spot {
    Band(String),
    /// `(column, edge order: 0 top, 1 bottom)`.
    Edge(u64, u8),
    Menu(String),
    Body,
}

fn spot_of(w: &J) -> Spot {
    let slot = w["slot"].as_str().unwrap_or(schema::DEFAULT_SLOT);
    if schema::BAND_SLOTS.contains(&slot) {
        return Spot::Band(slot.to_string());
    }
    if slot == schema::MENU_SLOT {
        return Spot::Menu(w["menu"].as_str().unwrap_or_default().to_string());
    }
    let kind = w["kind"].as_str().unwrap_or_default();
    let row = schema::is_chrome_kind(kind)
        || w["edge"].is_string()
        || (kind == "nav-rail" && w["props"]["orientation"] == "row");
    if !row {
        return Spot::Body;
    }
    let edge = w["edge"].as_str().unwrap_or(schema::DEFAULT_EDGE);
    Spot::Edge(w["column"].as_u64().unwrap_or(0), u8::from(edge == "bottom"))
}

fn edge_name(order: u8) -> &'static str {
    if order == 1 { "bottom" } else { "top" }
}

/// The `placement` string `page.controls` gives a Garden-placed control
/// (FC29): `top-start`, `column-1-bottom-end`, `menu:more`.
fn placement_of(w: &J) -> String {
    let align = w["align"].as_str().unwrap_or(schema::DEFAULT_ALIGN);
    match spot_of(w) {
        Spot::Band(b) => format!("{b}-{align}"),
        Spot::Edge(c, e) => format!("column-{c}-{}-{align}", edge_name(e)),
        Spot::Menu(m) => format!("menu:{m}"),
        Spot::Body => w["slot"].as_str().unwrap_or(schema::DEFAULT_SLOT).to_string(),
    }
}

/// `bands`, `edges` and `menus` (FC29): ids in draw order, computed by the
/// daemon so the renderer only draws. `ordered` is every content widget and
/// chrome item in page order, each flagged when it is the TEMPLATE's
/// chrome: template chrome keeps its corners (its `start` items lead their
/// group, its other items close theirs), so a band widget from the file
/// sits between the Garden switcher and the toggle, as live.
fn place_rows(ordered: &[(&J, bool)]) -> (J, J, J) {
    let rank = |(w, tpl): &(&J, bool)| -> u8 {
        match (tpl, w["align"].as_str().unwrap_or(schema::DEFAULT_ALIGN)) {
            (false, _) => 1,
            (true, "start") => 0,
            (true, _) => 2,
        }
    };
    let mut items: Vec<(&J, bool)> = ordered.to_vec();
    items.sort_by_key(rank);
    let align_idx = |w: &J| match w["align"].as_str().unwrap_or(schema::DEFAULT_ALIGN) {
        "center" => 1,
        "end" => 2,
        _ => 0,
    };
    let mut groups: std::collections::BTreeMap<Spot, [Vec<J>; 3]> = std::collections::BTreeMap::new();
    let mut menus = serde_json::Map::new();
    for (w, _) in ordered {
        if w["kind"] == "menu" {
            menus.insert(w["id"].as_str().unwrap_or("menu").to_string(), json!([]));
        }
    }
    for (w, _) in &items {
        let id = w["id"].clone();
        match spot_of(w) {
            Spot::Body => {}
            Spot::Menu(m) => {
                if let Some(list) = menus.entry(m).or_insert_with(|| json!([])).as_array_mut() {
                    list.push(id);
                }
            }
            spot => groups.entry(spot).or_default()[align_idx(w)].push(id),
        }
    }
    let group_json = |g: &[Vec<J>; 3]| json!({ "start": g[0], "center": g[1], "end": g[2] });
    let mut bands = serde_json::Map::new();
    for b in schema::BAND_SLOTS {
        let v = groups.get(&Spot::Band((*b).to_string())).map(group_json).unwrap_or(J::Null);
        bands.insert((*b).to_string(), v);
    }
    let edges: Vec<J> = groups
        .iter()
        .filter_map(|(spot, g)| match spot {
            Spot::Edge(c, e) => {
                let mut v = group_json(g);
                v["column"] = json!(c);
                v["edge"] = json!(edge_name(*e));
                Some(v)
            }
            _ => None,
        })
        .collect();
    (J::Object(bands), J::Array(edges), J::Object(menus))
}

/// `page.controls` for a page whose chrome is the Garden file's (FC29):
/// the switcher and the toggle with their placements, the drag region on
/// the bands, and Add agent when the page has an Agents widget. Older apps
/// read only each `kind`, so the required pair stays declared.
fn garden_controls(chrome: &[J], content: &[J]) -> J {
    let mut out = Vec::new();
    let find = |k: &str| chrome.iter().find(|c| c["kind"] == k);
    if let Some(s) = find("garden-switcher") {
        out.push(json!({ "kind": "garden-switcher", "placement": placement_of(s) }));
    }
    out.push(json!({ "kind": "drag-region", "placement": "bands" }));
    if let Some(t) = find("zen-toggle") {
        out.push(json!({ "kind": "zen-toggle", "placement": placement_of(t) }));
    }
    if let Some(a) = content.iter().find(|w| w["kind"] == "agents") {
        out.push(json!({ "kind": "add-agent", "placement": "widget-bottom-left", "widget": a["id"] }));
    }
    J::Array(out)
}

/// Write `chrome`, `bands`, `edges` and `menus` into a page (FC29).
fn set_chrome(page: &mut J, from: &str, chrome: &[J], ordered: &[(&J, bool)]) {
    let (bands, edges, menus) = place_rows(ordered);
    page["chrome"] = json!({ "from": from, "items": chrome });
    page["bands"] = bands;
    page["edges"] = edges;
    page["menus"] = menus;
}

fn kind_is_chrome(w: &J) -> bool {
    w["kind"].as_str().is_some_and(schema::is_chrome_kind)
}

/// Link each Conversation that follows nothing to the page's first Agents
/// widget (column order, then declaration order), so the renderer reads
/// an explicit `agents` prop (G27).
pub fn link_conversations(widgets: &mut [J]) {
    let mut agents: Vec<(u64, usize, String)> = widgets
        .iter()
        .enumerate()
        .filter(|(_, w)| w["kind"] == "agents")
        .filter_map(|(i, w)| Some((w["column"].as_u64().unwrap_or(0), i, w["id"].as_str()?.to_string())))
        .collect();
    agents.sort();
    let Some((_, _, first)) = agents.first().cloned() else { return };
    for w in widgets.iter_mut().filter(|w| w["kind"] == "conversation") {
        if w["props"].get("agents").is_none() && w["props"].get("agent").is_none() {
            w["props"]["agents"] = json!(first);
        }
    }
}

/// Each column's `widget`: the first widget placed in it (informational).
fn name_columns(columns: &mut [J], widgets: &[J]) {
    for (i, c) in columns.iter_mut().enumerate() {
        match widgets.iter().find(|w| w["column"].as_u64() == Some(i as u64)).and_then(|w| w["id"].as_str()) {
            Some(id) => c["widget"] = json!(id),
            None => {
                if let Some(o) = c.as_object_mut() {
                    o.remove("widget");
                }
            }
        }
    }
}

/// The resolved page of a template: `{template, layout, widgets, controls,
/// chrome, bands, edges, menus}` (Z10, Z15, FC29, FC30). Its `[[widget]]`
/// tables split into content (`widgets`) and chrome (`chrome.items`, from
/// `"template"`); `controls` is the template's `[[control]]` list as is.
/// `None` for an unknown id.
///
/// Every template, the Garden catalog's included, is read through
/// [`Defaults::live`] (D0).
pub fn template_page(id: &str) -> Option<J> {
    Defaults::live().template(id)
}

/// [`template_page`] from the set `d` (GS20).
pub fn template_page_in(d: &Defaults, id: &str) -> Option<J> {
    d.template(id)
}

/// Build one template's resolved page from its TOML with the prop defaults
/// of `d` (the work behind [`Defaults::template`]).
pub(crate) fn build_template_page_in(d: &Defaults, tid: &str, toml_src: &str) -> J {
    let v: toml::Value =
        toml::from_str(toml_src).unwrap_or_else(|e| panic!("built-in Zen template {tid} does not parse: {e}"));
    let t = serde_json::to_value(v).unwrap_or_else(|e| panic!("built-in Zen template {tid} to JSON: {e}"));
    let columns = t["layout"]["column"].as_array().cloned().unwrap_or_default();
    let all = t["widget"].as_array().cloned().unwrap_or_default();
    let mut widgets: Vec<J> =
        all.iter().filter(|w| !kind_is_chrome(w)).cloned().map(|w| content_widget_in(d, w)).collect();
    link_conversations(&mut widgets);
    let chrome: Vec<J> = all.iter().filter(|w| kind_is_chrome(w)).map(|w| chrome_item_in(d, w)).collect();
    let mut page = json!({
        "template": tid,
        "layout": layout_json(&t["layout"]["kind"], &columns),
        "widgets": widgets,
        "controls": t["control"].clone(),
    });
    let ordered: Vec<(&J, bool)> = widgets.iter().map(|w| (w, false)).chain(chrome.iter().map(|c| (c, true))).collect();
    set_chrome(&mut page, "template", &chrome, &ordered);
    page
}

/// A template's chrome `[[widget]]` tables written out as a Garden file
/// (`schema = 1` + those tables), so a test and the doctor can check them
/// with the same rules as a Garden file (FC30). `None` for an unknown id.
pub fn template_chrome_toml(id: &str) -> Option<String> {
    let src = Defaults::live().template_source(id)?;
    let v: toml::Value =
        toml::from_str(src).unwrap_or_else(|e| panic!("built-in Zen template {id} does not parse: {e}"));
    let chrome: Vec<toml::Value> = v
        .get("widget")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.get("kind").and_then(toml::Value::as_str).is_some_and(schema::is_chrome_kind))
        .collect();
    let mut file = toml::map::Map::new();
    file.insert("schema".into(), toml::Value::Integer(schema::SCHEMA_VERSION));
    file.insert("widget".into(), toml::Value::Array(chrome));
    Some(toml::to_string(&toml::Value::Table(file)).unwrap_or_else(|e| panic!("template {id} chrome to TOML: {e}")))
}

/// A template's chrome checked as a Garden file on that template (FC30).
pub fn check_template_chrome(id: &str) -> Option<schema::Checked> {
    let src = template_chrome_toml(id)?;
    Some(schema::check_garden(&format!("builtin:{id}#chrome"), &src, builtin_layer(), id))
}

/// The `k2.texting@1` page (Garden 1's).
pub fn texting_page() -> J {
    template_page(TEMPLATE_ID).unwrap_or_else(|| panic!("TEMPLATES must carry {TEMPLATE_ID}"))
}

/// The page a Garden shows: its template (the file's `template`, else
/// `default_template`), with the file's `[layout]` and `[[widget]]` in
/// place of the template's (G38).
///
/// The file's widgets are two groups with their own replace rules
/// (prd-zen-freeform-chrome FC3): any content widget replaces the
/// template's content widgets; any chrome widget replaces ALL of the
/// template's chrome (`chrome.from: "garden"`, `controls` rebuilt from the
/// file's placements). Chrome never goes into `widgets` (FC28), so an older
/// app never draws a toggle as a placeholder widget.
pub fn garden_page(layer: &Layer, default_template: &str) -> J {
    garden_page_in(Defaults::live(), layer, default_template)
}

/// [`garden_page`] on the set `d` (GS20). A template the set doesn't have
/// resolves live (GS22).
pub fn garden_page_in(d: &Defaults, layer: &Layer, default_template: &str) -> J {
    let tid = layer.get("page.template").and_then(J::as_str).unwrap_or(default_template);
    let mut page = template_page_in(d, tid).or_else(|| template_page(tid)).unwrap_or_else(texting_page);
    let layout = layer.get("page.layout");
    let widgets = layer.get("page.widgets");
    if layout.is_none() && widgets.is_none() {
        return page;
    }
    let file: Vec<J> = widgets.and_then(J::as_array).cloned().unwrap_or_default();
    // Every file widget in file order, as the renderer reads it.
    let file_items: Vec<(J, bool)> = file
        .iter()
        .map(|w| if kind_is_chrome(w) { (chrome_item_in(d, w), true) } else { (content_widget_in(d, w.clone()), false) })
        .collect();
    let has_content = file_items.iter().any(|(_, c)| !c);
    let has_chrome = file_items.iter().any(|(_, c)| *c);
    if has_content {
        let mut ws: Vec<J> = file_items.iter().filter(|(_, c)| !c).map(|(w, _)| w.clone()).collect();
        link_conversations(&mut ws);
        page["widgets"] = J::Array(ws);
    }
    let content: Vec<J> = page["widgets"].as_array().cloned().unwrap_or_default();
    if has_chrome {
        let chrome: Vec<J> = file_items.iter().filter(|(_, c)| *c).map(|(w, _)| w.clone()).collect();
        // File order across both groups when the file declares both;
        // otherwise the template's content, then the file's chrome.
        let ordered: Vec<(&J, bool)> = if has_content {
            file_items.iter().map(|(w, _)| (w, false)).collect()
        } else {
            content.iter().map(|w| (w, false)).chain(chrome.iter().map(|c| (c, false))).collect()
        };
        set_chrome(&mut page, "garden", &chrome, &ordered);
        page["controls"] = garden_controls(&chrome, &content);
    } else {
        let chrome: Vec<J> = page["chrome"]["items"].as_array().cloned().unwrap_or_default();
        let ordered: Vec<(&J, bool)> =
            content.iter().map(|w| (w, false)).chain(chrome.iter().map(|c| (c, true))).collect();
        set_chrome(&mut page, "template", &chrome, &ordered);
    }
    let mut columns: Vec<J> = match layout {
        Some(l) => l["columns"].as_array().cloned().unwrap_or_default(),
        None => page["layout"]["columns"].as_array().cloned().unwrap_or_default(),
    };
    let kind = layout.map(|l| l["kind"].clone()).unwrap_or_else(|| page["layout"]["kind"].clone());
    let ws = page["widgets"].as_array().cloned().unwrap_or_default();
    name_columns(&mut columns, &ws);
    page["layout"] = layout_json(&kind, &columns);
    page
}

/// `~/.k2/zen` on this computer.
pub fn zen_root() -> PathBuf {
    crate::paths::k2_home().join("zen")
}

/// Whether this computer has set Zen up: the folder holds a Garden list
/// (`POST /cli/zen/setup` made it). Gates the `k2-zen` skill (Z18) and the
/// watcher's boot start (Z61).
pub fn is_set_up() -> bool {
    ZenFiles::local().is_set_up()
}

/// The built-in `basic` theme's layer. Every built-in must be clean (a
/// daemon test pins that).
pub fn builtin_layer() -> &'static Layer {
    builtin_theme_layer(DEFAULT_THEME)
        .unwrap_or_else(|| panic!("BUILTIN_THEMES must carry '{DEFAULT_THEME}'"))
}

/// The built-in `basic` theme fully checked, for tests and the doctor.
pub fn check_builtin() -> schema::Checked {
    check_builtin_theme(DEFAULT_THEME)
        .unwrap_or_else(|| panic!("BUILTIN_THEMES must carry '{DEFAULT_THEME}'"))
}

/// A theme name: lower-case letters, digits, `-` and `_`, starting with a
/// letter or digit, up to 40. It is also the folder name under `themes/`.
pub fn valid_theme_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}
