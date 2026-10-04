//! Zen Mode v1 core (prd-zen-mode-v1 S1, S3).
//!
//! `~/.k2/zen/` belongs to the LOCAL daemon (Z8): one theme for every Home
//! (`zen.toml`), one page per Home (`pages/<home-id>.toml`), the daemon's
//! own `homes.json` (id → name) and `.history/` (the last 20 good versions
//! of each file). `grants.json` is reserved for v2 widget grants: v1 never
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

/// The `zen.toml` written for a new computer, and the stub `reset` restores.
/// It spells out the default theme, so it is also the builtin layer.
pub const DEFAULT_ZEN_TOML: &str = include_str!("default-zen.toml");

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

/// The builtin theme layer: [`DEFAULT_ZEN_TOML`] checked against nothing.
/// It must be clean (a daemon test pins that).
pub fn builtin_layer() -> &'static Layer {
    static BUILTIN: OnceLock<Layer> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        schema::check("builtin", DEFAULT_ZEN_TOML, FileKind::Zen, &Layer::new()).layer
    })
}

/// The builtin default theme fully checked, for tests and the doctor.
pub fn check_builtin() -> schema::Checked {
    schema::check("builtin", DEFAULT_ZEN_TOML, FileKind::Zen, &Layer::new())
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
