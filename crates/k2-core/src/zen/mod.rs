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
//! The stack for one Garden: built-in `default` → built-in `<active>` →
//! `themes/<active>/theme.toml` → `zen.toml` → `gardens/<id>.toml`. `grants.json` is reserved for v2 widget grants: v1 never
//! reads or writes it, and only the daemon may ever write it, in answer to
//! a click in the K2 app (Z19). No route, CLI verb or agent can grant.
//!
//! - [`schema`]: tokens, ranges, validation with `file:line:col`, resolve.
//! - [`store`]: the folder: setup, Gardens, last-good, history, reset.
//! - [`skill`]: the `k2-zen` skill body, generated from [`schema`].
//!
//! Page templates are data (Z10, G40): TOML shipped in this crate
//! ([`TEMPLATES`]) and returned as the resolved page's layout, widgets and
//! controls.

pub mod schema;
pub mod skill;
pub mod store;

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value as J};

pub use schema::{Diagnostic, FileKind, Layer, BLANK_TEMPLATE_ID, SCHEMA_VERSION, TEMPLATE_ID};
pub use store::{GardenEntry, ZenError, ZenFile, ZenFiles};

/// The `zen.toml` written for a new computer, and the stub `reset` restores:
/// `schema = 1` and comments, no tables. The defaults live in the built-in
/// themes, so `~/.k2/zen` holds only the user's changes.
pub const DEFAULT_ZEN_TOML: &str = include_str!("default-zen.toml");

/// The theme every other theme starts from, and the one a new computer uses.
pub const DEFAULT_THEME: &str = "default";

/// A theme built into K2: read-only, embedded in the binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTheme {
    pub name: &'static str,
    pub summary: &'static str,
    pub toml: &'static str,
}

/// Built-in themes, in cycle order (`k2 zen theme next`). `default` is
/// first and complete; every other theme sits on top of it.
pub const BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        name: DEFAULT_THEME,
        summary: "clean, smooth, simple: warm light, soft dark, follows the computer",
        toml: include_str!("themes/default.toml"),
    },
    BuiltinTheme {
        name: "paper",
        summary: "always light, serif type, ink-blue accent",
        toml: include_str!("themes/paper.toml"),
    },
    BuiltinTheme {
        name: "midnight",
        summary: "always dark, cool blue, JetBrains Mono everywhere",
        toml: include_str!("themes/midnight.toml"),
    },
];

pub fn builtin_theme(name: &str) -> Option<&'static BuiltinTheme> {
    BUILTIN_THEMES.iter().find(|t| t.name == name)
}

/// The label a built-in's diagnostics carry.
fn builtin_label(name: &str) -> String {
    format!("builtin:{name}")
}

/// A built-in theme checked over the default (the default over nothing).
pub fn check_builtin_theme(name: &str) -> Option<schema::Checked> {
    let t = builtin_theme(name)?;
    let base = if name == DEFAULT_THEME { Layer::new() } else { builtin_layer().clone() };
    Some(schema::check(&builtin_label(name), t.toml, FileKind::Theme, &base))
}

/// The full built-in layer of a theme: `default` merged with the named
/// built-in (just `default` for `default`). `None` when not built in.
pub fn builtin_theme_layer(name: &str) -> Option<&'static Layer> {
    static LAYERS: OnceLock<std::collections::BTreeMap<&'static str, Layer>> = OnceLock::new();
    LAYERS
        .get_or_init(|| {
            BUILTIN_THEMES
                .iter()
                .map(|t| {
                    let own = schema::check(
                        &builtin_label(t.name),
                        t.toml,
                        FileKind::Theme,
                        &Layer::new(),
                    )
                    .layer;
                    let layer = if t.name == DEFAULT_THEME {
                        own
                    } else {
                        let base = schema::check(
                            &builtin_label(DEFAULT_THEME),
                            BUILTIN_THEMES[0].toml,
                            FileKind::Theme,
                            &Layer::new(),
                        )
                        .layer;
                        schema::merge(&[&base, &own])
                    };
                    (t.name, layer)
                })
                .collect()
        })
        .get(name)
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
/// Zen toggle and the Garden switcher). Every template's `controls` names
/// both; a Garden file can't drop one (its `[[control]]` is ignored). The
/// templates also declare `drag-region`, which K2 binds for window drag but
/// never checks.
pub const REQUIRED_CONTROLS: &[&str] = &["zen-toggle", "garden-switcher"];

/// Bridge caps (Z34, G29). `thread:*` reuse the app-gateway names;
/// `agents:read`, `agents:add` and `presence:read` are Zen-bridge only.
/// `agents:add` lets a widget OPEN K2's Add agent picker for its Home
/// (`agents.add`); the human picks and K2 writes the row. `gardens:manage`
/// (create, rename, delete Gardens) is granted by K2 to the template's
/// controls only, never to a widget. `app:navigate` (the `nav-rail`
/// widget) switches the Garden's rail view (inside Zen) and reads the top
/// bar's badges.
pub const BRIDGE_CAPS: &[&str] = &[
    "agents:read",
    "agents:add",
    "presence:read",
    "thread:read",
    "thread:post",
    "gardens:manage",
    "app:navigate",
];

/// The caps K2 grants each built-in widget kind (`source: "builtin"`). A
/// Garden file never names caps (G38); these are the only ones.
pub const BUILTIN_WIDGET_CAPS: &[(&str, &[&str])] = &[
    ("agents", &["agents:read", "agents:add", "presence:read"]),
    ("conversation", &["agents:read", "presence:read", "thread:read", "thread:post"]),
    ("garden-empty", &["agents:read", "thread:read", "thread:post"]),
    ("nav-rail", &["app:navigate"]),
];

/// K2's caps for a built-in widget kind.
pub fn widget_caps(kind: &str) -> Option<&'static [&'static str]> {
    BUILTIN_WIDGET_CAPS.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c)
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
/// "builtin"`, every prop filled (defaults, `agents.mode`).
pub fn builtin_widget(mut w: J) -> J {
    let kind = w["kind"].as_str().unwrap_or_default().to_string();
    let mut props = w["props"].as_object().cloned().unwrap_or_default();
    schema::normalize_props(&kind, &mut props);
    w["props"] = J::Object(props);
    w["caps"] = json!(widget_caps(&kind).unwrap_or(&[]));
    w["source"] = json!("builtin");
    w
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

/// The resolved page of a template: `{template, layout, widgets, controls}`
/// (Z10, Z15). `None` for an unknown id.
pub fn template_page(id: &str) -> Option<J> {
    static PAGES: OnceLock<std::collections::BTreeMap<&'static str, J>> = OnceLock::new();
    PAGES
        .get_or_init(|| {
            TEMPLATES
                .iter()
                .map(|(tid, toml_src)| {
                    let v: toml::Value = toml::from_str(toml_src)
                        .unwrap_or_else(|e| panic!("built-in Zen template {tid} does not parse: {e}"));
                    let t = serde_json::to_value(v)
                        .unwrap_or_else(|e| panic!("built-in Zen template {tid} to JSON: {e}"));
                    let columns = t["layout"]["column"].as_array().cloned().unwrap_or_default();
                    let mut widgets: Vec<J> =
                        t["widget"].as_array().cloned().unwrap_or_default().into_iter().map(builtin_widget).collect();
                    link_conversations(&mut widgets);
                    (
                        *tid,
                        json!({
                            "template": tid,
                            "layout": layout_json(&t["layout"]["kind"], &columns),
                            "widgets": widgets,
                            "controls": t["control"].clone(),
                        }),
                    )
                })
                .collect()
        })
        .get(id)
        .cloned()
}

/// The `k2.texting@1` page (Garden 1's).
pub fn texting_page() -> J {
    template_page(TEMPLATE_ID).unwrap_or_else(|| panic!("TEMPLATES must carry {TEMPLATE_ID}"))
}

/// The page a Garden shows: its template (the file's `template`, else
/// `default_template`), with the file's `[layout]` and `[[widget]]` in
/// place of the template's (G38). Controls always come from the template.
pub fn garden_page(layer: &Layer, default_template: &str) -> J {
    let tid = layer.get("page.template").and_then(J::as_str).unwrap_or(default_template);
    let mut page = template_page(tid).unwrap_or_else(texting_page);
    let layout = layer.get("page.layout");
    let widgets = layer.get("page.widgets");
    if layout.is_none() && widgets.is_none() {
        return page;
    }
    if let Some(w) = widgets {
        let mut ws: Vec<J> = w.as_array().cloned().unwrap_or_default().into_iter().map(builtin_widget).collect();
        link_conversations(&mut ws);
        page["widgets"] = J::Array(ws);
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

/// The built-in `default` theme's layer. Every built-in must be clean (a
/// daemon test pins that).
pub fn builtin_layer() -> &'static Layer {
    builtin_theme_layer(DEFAULT_THEME)
        .unwrap_or_else(|| panic!("BUILTIN_THEMES must carry '{DEFAULT_THEME}'"))
}

/// The built-in `default` theme fully checked, for tests and the doctor.
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
