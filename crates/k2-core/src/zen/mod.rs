//! Zen Mode v1 core (prd-zen-mode-v1 S1, S3).
//!
//! `~/.k2/zen/` belongs to the LOCAL daemon (Z8). Since the Omarchy
//! additions (2026-10-04) it holds only the user's changes, layered over
//! read-only defaults built into K2:
//!
//! - built-in themes ([`BUILTIN_THEMES`], embedded TOML) and the built-in
//!   page template `k2.texting@1` are the defaults; app updates replace
//!   them and never touch user files;
//! - `themes/<name>/theme.toml` (+ an optional background image) is a
//!   user theme bundle, or the user's override of the built-in of that
//!   name;
//! - `zen.toml` is the user's changes for every Home on top of the active
//!   theme; `pages/<home-id>.toml` the changes for one Home;
//! - `active.json` (daemon-written) names the active theme, globally and
//!   per Home (decision 8: one theme plus an optional per-Home override);
//! - `homes.json` (id → name) and `.history/` (the last 20 good versions
//!   of each file) are the daemon's own.
//!
//! The stack for one Home: built-in `default` → built-in `<active>` →
//! `themes/<active>/theme.toml` → `zen.toml` → `pages/<home>.toml`. `grants.json` is reserved for v2 widget grants: v1 never
//! reads or writes it, and only the daemon may ever write it, in answer to
//! a click in the K2 app (Z19). No route, CLI verb or agent can grant.
//!
//! - [`schema`]: tokens, ranges, validation with `file:line:col`, resolve.
//! - [`store`]: the folder: ensure, homes, last-good, history, reset.
//! - [`skill`]: the `k2-zen` skill body, generated from [`schema`].
//!
//! The page template `k2.texting@1` is data (Z10): TOML shipped in this
//! crate and returned as the resolved page's layout, widgets and controls.

pub mod schema;
pub mod skill;
pub mod store;

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value as J};

pub use schema::{Diagnostic, FileKind, Layer, SCHEMA_VERSION, TEMPLATE_ID};
pub use store::{HomeEntry, ZenError, ZenFile, ZenFiles};

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

/// The `k2.texting@1` template (Z10).
pub const TEXTING_TEMPLATE_TOML: &str = include_str!("template-k2-texting-1.toml");

/// Required page controls (Z27). Every page's `controls` names all three.
pub const REQUIRED_CONTROLS: &[&str] = &["zen-toggle", "home-switcher", "drag-region"];

/// Bridge caps a widget may declare (Z34). `thread:*` reuse the app-gateway
/// names; `agents:read` and `presence:read` are Zen-bridge only in v1.
pub const BRIDGE_CAPS: &[&str] = &["agents:read", "presence:read", "thread:read", "thread:post"];

/// `~/.k2/zen` on this computer.
pub fn zen_root() -> PathBuf {
    crate::paths::k2_home().join("zen")
}

/// Whether this computer has set Zen up (the folder exists). Gates the
/// `k2-zen` skill (Z18) and the watcher's boot start (Z61).
pub fn is_set_up() -> bool {
    zen_root().is_dir()
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

fn template_value() -> J {
    let v: toml::Value = toml::from_str(TEXTING_TEMPLATE_TOML)
        .unwrap_or_else(|e| panic!("built-in Zen template does not parse: {e}"));
    serde_json::to_value(v).unwrap_or_else(|e| panic!("built-in Zen template to JSON: {e}"))
}

/// The resolved page of the `k2.texting@1` template:
/// `{template, layout, widgets, controls}` (Z10, Z15). Built-in widgets are
/// `source: "builtin"`: their caps are granted by K2, never by grants.json.
pub fn texting_page() -> J {
    static PAGE: OnceLock<J> = OnceLock::new();
    PAGE.get_or_init(|| {
        let t = template_value();
        // Shape the S4 renderer reads (docs/zen-contract.md): `split` and
        // `minWidths` per column; `columns` keeps the per-column detail.
        let columns = t["layout"]["column"].as_array().cloned().unwrap_or_default();
        let layout = json!({
            "kind": t["layout"]["kind"].clone(),
            "split": columns.iter().map(|c| c["size"].clone()).collect::<Vec<_>>(),
            "minWidths": columns.iter().map(|c| c["min-width"].clone()).collect::<Vec<_>>(),
            "columns": columns,
        });
        let widgets: Vec<J> = t["widget"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut w| {
                w["source"] = json!("builtin");
                w
            })
            .collect();
        json!({
            "template": TEMPLATE_ID,
            "layout": layout,
            "widgets": widgets,
            "controls": t["control"].clone(),
        })
    })
    .clone()
}
