//! K2's built-in Zen defaults as one value (prd-zen-garden-sync-defaults-v1
//! D0, GS10, GS11, GS19–GS20, GF1).
//!
//! **D0 interface (day-0 addendum, 2026-10-08).** Every read of a built-in
//! page template, built-in theme or widget prop default goes through a
//! [`Defaults`]. [`Defaults::live`] is what is compiled into this binary
//! (`TEMPLATES`, the Garden catalog, `BUILTIN_THEMES`, `WIDGET_PROPS`,
//! `frame.toml`). The sync work adds archived sets: [`DefaultsSet`] is one
//! set as plain data (what `~/.k2/zen/.defaults/<fp>.json` holds) and
//! [`Defaults::from_set`] parses one, so each Garden part can resolve on its
//! own `&Defaults` (`zen::sync`). Callers keep the same methods.
//!
//! Built lazily in parts, so building a template page (which normalizes
//! widget props through this same set) never re-enters an initializer.
//!
//! What is NOT here is grammar (GS15): `place_rows`, `link_conversations`,
//! `normalize_slot_props`, the `resolve` fallbacks and the validation rules
//! are code that defines what a file means, frozen by a hash test.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use super::schema::{self, FileKind, Layer};

/// The archive format (`format` in a `.defaults/<fp>.json`).
pub const DEFAULTS_FORMAT: u32 = 1;

/// The frame (GF1): the renderer's values as data.
pub const FRAME_TOML: &str = include_str!("frame.toml");

/// One set of K2's defaults as plain data: what `.defaults/<fp>.json` holds
/// (GS10). Templates and themes are their TOML text; `widgetProps` maps
/// `kind.name` to the default (JSON `null` when the prop has none); `frame`
/// is `frame.toml` as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsSet {
    pub format: u32,
    pub fingerprint: String,
    #[serde(default)]
    pub k2_version: String,
    #[serde(default)]
    pub archived_at: String,
    pub templates: BTreeMap<String, String>,
    pub themes: BTreeMap<String, String>,
    pub widget_props: BTreeMap<String, J>,
    pub frame: J,
}

fn toml_json(what: &str, src: &str) -> Result<J, String> {
    let v: toml::Value = toml::from_str(src).map_err(|e| format!("{what} does not parse: {e}"))?;
    serde_json::to_value(v).map_err(|e| format!("{what} to JSON: {e}"))
}

impl DefaultsSet {
    /// What this binary ships, with its fingerprint. `archivedAt` is empty
    /// (the archive stamps it when it writes the file).
    pub fn from_binary() -> DefaultsSet {
        let mut templates = BTreeMap::new();
        for (id, src) in super::TEMPLATES {
            templates.insert((*id).to_string(), (*src).to_string());
        }
        for e in super::garden_catalog::garden_catalog() {
            templates.insert(e.template_id.clone(), e.toml.to_string());
        }
        let themes = super::BUILTIN_THEMES.iter().map(|t| (t.name.to_string(), t.toml.to_string())).collect();
        let widget_props = schema::WIDGET_PROPS
            .iter()
            .map(|p| {
                let v = match p.default {
                    Some(d) => serde_json::from_str(d)
                        .unwrap_or_else(|e| panic!("WIDGET_PROPS default for {}.{} is not JSON: {e}", p.kind, p.name)),
                    None => J::Null,
                };
                (format!("{}.{}", p.kind, p.name), v)
            })
            .collect();
        let frame = toml_json("built-in frame.toml", FRAME_TOML).unwrap_or_else(|e| panic!("{e}"));
        let mut set = DefaultsSet {
            format: DEFAULTS_FORMAT,
            fingerprint: String::new(),
            k2_version: env!("CARGO_PKG_VERSION").to_string(),
            archived_at: String::new(),
            templates,
            themes,
            widget_props,
            frame,
        };
        set.fingerprint = set.compute_fingerprint().unwrap_or_else(|e| panic!("built-in Zen defaults: {e}"));
        set
    }

    /// The parsed content the fingerprint covers: templates and themes as
    /// parsed TOML (comments and spacing don't count; a catalog file's
    /// `[catalog]` metadata table doesn't either), the prop defaults and the
    /// frame.
    pub fn content(&self) -> Result<J, String> {
        let mut templates = serde_json::Map::new();
        for (id, src) in &self.templates {
            let mut t = toml_json(&format!("template {id}"), src)?;
            if let Some(o) = t.as_object_mut() {
                o.remove("catalog");
            }
            templates.insert(id.clone(), t);
        }
        let mut themes = serde_json::Map::new();
        for (name, src) in &self.themes {
            themes.insert(name.clone(), toml_json(&format!("theme {name}"), src)?);
        }
        Ok(json!({
            "format": self.format,
            "templates": templates,
            "themes": themes,
            "widgetProps": self.widget_props,
            "frame": self.frame,
        }))
    }

    /// `d-` + the first 16 hex of SHA-256 over the canonical JSON of
    /// [`DefaultsSet::content`] (GS11).
    pub fn compute_fingerprint(&self) -> Result<String, String> {
        let digest = Sha256::digest(canonical_json(&self.content()?).as_bytes());
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        Ok(format!("d-{hex}"))
    }
}

/// One set of built-in defaults.
#[derive(Debug)]
pub struct Defaults {
    set: DefaultsSet,
    /// `(template id, TOML)`: the two starts, then every catalog version.
    template_sources: Vec<(String, String)>,
    /// `kind` → `name` → default (JSON), props with a default only.
    prop_defaults: BTreeMap<String, BTreeMap<String, J>>,
    /// `kind.name` of every prop the set lists (with or without a default).
    prop_known: BTreeSet<String>,
    /// The resolved page of each template (built on first use).
    pages: OnceLock<BTreeMap<String, J>>,
    /// The full layer of each built-in theme (built on first use).
    theme_layers: OnceLock<BTreeMap<String, Layer>>,
}

impl Defaults {
    /// The defaults compiled into this binary.
    pub fn live() -> &'static Defaults {
        live_cell().as_ref()
    }

    /// [`Defaults::live`] as a shared handle (per-Garden sets are `Arc`s).
    pub fn live_arc() -> Arc<Defaults> {
        live_cell().clone()
    }

    /// Parse a set (an archived one, or this binary's). Refused when its
    /// format is unknown, its content's fingerprint isn't its `fingerprint`,
    /// a template or theme doesn't parse, `basic` is missing or the frame
    /// isn't a table (GS14).
    pub fn from_set(set: DefaultsSet) -> Result<Defaults, String> {
        if set.format != DEFAULTS_FORMAT {
            return Err(format!("format {} is not supported (this K2 reads {DEFAULTS_FORMAT})", set.format));
        }
        let fp = set.compute_fingerprint()?;
        if fp != set.fingerprint {
            return Err(format!("its content's fingerprint is {fp}, not {}", set.fingerprint));
        }
        if !set.frame.is_object() {
            return Err("frame is not a table".into());
        }
        if !set.themes.contains_key(super::DEFAULT_THEME) {
            return Err(format!("it has no '{}' theme", super::DEFAULT_THEME));
        }
        // The two starts first (in K2's order), then the rest by id.
        let mut template_sources: Vec<(String, String)> = Vec::new();
        for (id, _) in super::TEMPLATES {
            if let Some(src) = set.templates.get(*id) {
                template_sources.push(((*id).to_string(), src.clone()));
            }
        }
        for (id, src) in &set.templates {
            if !template_sources.iter().any(|(t, _)| t == id) {
                template_sources.push((id.clone(), src.clone()));
            }
        }
        let mut prop_defaults: BTreeMap<String, BTreeMap<String, J>> = BTreeMap::new();
        let mut prop_known = BTreeSet::new();
        for (key, v) in &set.widget_props {
            let Some((kind, name)) = key.split_once('.') else { return Err(format!("widget prop '{key}' isn't kind.name")) };
            prop_known.insert(key.clone());
            if !v.is_null() {
                prop_defaults.entry(kind.to_string()).or_default().insert(name.to_string(), v.clone());
            }
        }
        Ok(Defaults {
            set,
            template_sources,
            prop_defaults,
            prop_known,
            pages: OnceLock::new(),
            theme_layers: OnceLock::new(),
        })
    }

    /// The set as data (what the archive writes).
    pub fn set(&self) -> &DefaultsSet {
        &self.set
    }

    /// Every template id this set has: `k2.texting@1`, `k2.blank@1`, then
    /// the catalog's (every shipped version).
    pub fn template_ids(&self) -> Vec<&str> {
        self.template_sources.iter().map(|(id, _)| id.as_str()).collect()
    }

    /// Whether `id` is a template of this set.
    pub fn has_template(&self, id: &str) -> bool {
        self.template_sources.iter().any(|(t, _)| t == id)
    }

    /// A template's TOML as shipped.
    pub fn template_source(&self, id: &str) -> Option<&str> {
        self.template_sources.iter().find(|(t, _)| t == id).map(|(_, s)| s.as_str())
    }

    /// A template's TOML as JSON (`[catalog]` included). `None` for an
    /// unknown id.
    pub fn template_toml(&self, id: &str) -> Option<J> {
        toml_json(id, self.template_source(id)?).ok()
    }

    /// A template's resolved page (Z10, FC29): `{template, layout, widgets,
    /// controls, chrome, bands, edges, menus}`, built with this set's prop
    /// defaults. `None` for an unknown id.
    pub fn template(&self, id: &str) -> Option<J> {
        self.pages
            .get_or_init(|| {
                self.template_sources
                    .iter()
                    .map(|(tid, src)| (tid.clone(), super::build_template_page_in(self, tid, src)))
                    .collect()
            })
            .get(id)
            .cloned()
    }

    /// A built-in theme's full layer (`basic` merged with it; just `basic`
    /// for `basic`). `None` when the set has no theme of that name.
    pub fn theme_layer(&self, name: &str) -> Option<&Layer> {
        self.theme_layers
            .get_or_init(|| {
                let own = |name: &str, src: &str| {
                    schema::check(&format!("builtin:{name}"), src, FileKind::Theme, &Layer::new()).layer
                };
                let basic = self.set.themes.get(super::DEFAULT_THEME).map(|s| own(super::DEFAULT_THEME, s)).unwrap_or_default();
                self.set
                    .themes
                    .iter()
                    .map(|(n, src)| {
                        let layer = if n == super::DEFAULT_THEME { basic.clone() } else { schema::merge(&[&basic, &own(n, src)]) };
                        (n.clone(), layer)
                    })
                    .collect()
            })
            .get(name)
    }

    /// A widget prop's default (JSON), or `None` when it has none.
    pub fn widget_prop_default(&self, kind: &str, name: &str) -> Option<&J> {
        self.prop_defaults.get(kind).and_then(|m| m.get(name))
    }

    /// Whether the set lists this prop at all. A prop K2 added after the
    /// set was archived isn't listed; its default comes from the live set
    /// (GS22).
    pub fn knows_widget_prop(&self, kind: &str, name: &str) -> bool {
        self.prop_known.contains(&format!("{kind}.{name}"))
    }

    /// The frame (GF1) as archived (unclamped; `zen::sync` clamps the
    /// control thresholds to the floor before sending it).
    pub fn frame(&self) -> &J {
        &self.set.frame
    }

    /// `d-` + the first 16 hex of SHA-256 over the canonical JSON of the
    /// parsed content (GS11): comments never change it.
    pub fn fingerprint(&self) -> &str {
        &self.set.fingerprint
    }
}

fn live_cell() -> &'static Arc<Defaults> {
    static LIVE: OnceLock<Arc<Defaults>> = OnceLock::new();
    LIVE.get_or_init(|| {
        Arc::new(
            Defaults::from_set(DefaultsSet::from_binary())
                .unwrap_or_else(|e| panic!("built-in Zen defaults don't load: {e}")),
        )
    })
}

/// JSON with object keys sorted at every level (GS11).
pub fn canonical_json(v: &J) -> String {
    fn sort(v: &J) -> J {
        match v {
            J::Object(o) => {
                let sorted: BTreeMap<&String, J> = o.iter().map(|(k, v)| (k, sort(v))).collect();
                J::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
            }
            J::Array(a) => J::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_set_has_the_starts_the_catalog_and_every_theme() {
        let d = Defaults::live();
        let ids = d.template_ids();
        assert_eq!(&ids[..2], &[schema::TEMPLATE_ID, schema::BLANK_TEMPLATE_ID]);
        for e in super::super::garden_catalog::garden_catalog() {
            assert!(d.has_template(&e.template_id), "{} is in the live set", e.template_id);
            assert!(d.template(&e.template_id).is_some());
        }
        for t in super::super::BUILTIN_THEMES {
            assert!(d.theme_layer(t.name).is_some(), "{}", t.name);
        }
        assert!(d.template("k2.nope@1").is_none());
        assert_eq!(d.widget_prop_default("agents", "preview"), Some(&json!(true)));
        assert_eq!(d.widget_prop_default("agents", "home"), None);
        assert!(d.knows_widget_prop("agents", "home"));
        assert!(d.frame()["glass"].is_object(), "the frame is frame.toml");
    }

    #[test]
    fn fingerprint_is_stable_and_shaped() {
        let fp = Defaults::live().fingerprint();
        assert!(fp.starts_with("d-") && fp.len() == 18, "{fp}");
        assert_eq!(fp, Defaults::live().fingerprint());
        assert_eq!(fp, DefaultsSet::from_binary().compute_fingerprint().expect("fp"));
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let a: J = serde_json::from_str(r#"{"b":1,"a":{"d":[{"z":1,"y":2}],"c":0}}"#).expect("json");
        assert_eq!(canonical_json(&a), r#"{"a":{"c":0,"d":[{"y":2,"z":1}]},"b":1}"#);
    }
}
