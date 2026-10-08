//! K2's built-in Zen defaults as one value (prd-zen-garden-sync-defaults-v1
//! D0, GS11, GS19–GS20).
//!
//! **D0 interface (day-0 addendum, 2026-10-08).** Every read of a built-in
//! page template, built-in theme or widget prop default goes through a
//! [`Defaults`]. Today there is one: [`Defaults::live`], a thin pass-through
//! over what is compiled into this binary (`TEMPLATES`, the Garden catalog,
//! `BUILTIN_THEMES`, `WIDGET_PROPS`). The sync builder later adds
//! `Defaults::archived(fp)` (a saved set from `~/.k2/zen/.defaults/`) and
//! hands each Garden part its own `&Defaults`; callers keep the same
//! methods. `frame()` is empty until that work adds `frame.toml` (GF1).
//!
//! Built lazily in parts, so building a template page (which normalizes
//! widget props, which reads [`Defaults::live`]) never re-enters an
//! initializer.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{json, Value as J};

use super::schema::{self, FileKind, Layer};

/// One set of built-in defaults.
#[derive(Debug)]
pub struct Defaults {
    /// `(template id, TOML)`: the two starts, then every catalog version.
    template_sources: Vec<(&'static str, &'static str)>,
    /// `kind` → `name` → default (JSON), from `WIDGET_PROPS`.
    prop_defaults: BTreeMap<&'static str, BTreeMap<&'static str, J>>,
    /// The resolved page of each template (built on first use).
    pages: OnceLock<BTreeMap<&'static str, J>>,
    /// The full layer of each built-in theme (built on first use).
    theme_layers: OnceLock<BTreeMap<&'static str, Layer>>,
    /// The parts of the app's frame (GF1). Empty until `frame.toml`.
    frame: J,
    fingerprint: OnceLock<String>,
}

impl Defaults {
    /// The defaults compiled into this binary.
    pub fn live() -> &'static Defaults {
        static LIVE: OnceLock<Defaults> = OnceLock::new();
        LIVE.get_or_init(|| {
            let mut template_sources: Vec<(&'static str, &'static str)> = super::TEMPLATES.to_vec();
            for e in super::garden_catalog::garden_catalog() {
                template_sources.push((e.template_id.as_str(), e.toml));
            }
            let mut prop_defaults: BTreeMap<&'static str, BTreeMap<&'static str, J>> = BTreeMap::new();
            for p in schema::WIDGET_PROPS {
                if let Some(d) = p.default {
                    let v: J = serde_json::from_str(d)
                        .unwrap_or_else(|e| panic!("WIDGET_PROPS default for {}.{} is not JSON: {e}", p.kind, p.name));
                    prop_defaults.entry(p.kind).or_default().insert(p.name, v);
                }
            }
            Defaults {
                template_sources,
                prop_defaults,
                pages: OnceLock::new(),
                theme_layers: OnceLock::new(),
                frame: json!({}),
                fingerprint: OnceLock::new(),
            }
        })
    }

    /// Every template id this set has: `k2.texting@1`, `k2.blank@1`, then
    /// the catalog's (every shipped version).
    pub fn template_ids(&self) -> Vec<&'static str> {
        self.template_sources.iter().map(|(id, _)| *id).collect()
    }

    /// Whether `id` is a template of this set.
    pub fn has_template(&self, id: &str) -> bool {
        self.template_sources.iter().any(|(t, _)| *t == id)
    }

    /// A template's TOML as shipped.
    pub fn template_source(&self, id: &str) -> Option<&'static str> {
        self.template_sources.iter().find(|(t, _)| *t == id).map(|(_, s)| *s)
    }

    /// A template's resolved page (Z10, FC29): `{template, layout, widgets,
    /// controls, chrome, bands, edges, menus}`. `None` for an unknown id.
    pub fn template(&self, id: &str) -> Option<J> {
        self.pages
            .get_or_init(|| {
                self.template_sources.iter().map(|(tid, src)| (*tid, super::build_template_page(tid, src))).collect()
            })
            .get(id)
            .cloned()
    }

    /// A built-in theme's full layer (`basic` merged with it; just `basic`
    /// for `basic`). `None` when not built in.
    pub fn theme_layer(&self, name: &str) -> Option<&Layer> {
        self.theme_layers
            .get_or_init(|| {
                let own = |t: &super::BuiltinTheme| {
                    schema::check(&format!("builtin:{}", t.name), t.toml, FileKind::Theme, &Layer::new()).layer
                };
                let basic = own(&super::BUILTIN_THEMES[0]);
                super::BUILTIN_THEMES
                    .iter()
                    .map(|t| {
                        let layer =
                            if t.name == super::DEFAULT_THEME { basic.clone() } else { schema::merge(&[&basic, &own(t)]) };
                        (t.name, layer)
                    })
                    .collect()
            })
            .get(name)
    }

    /// A widget prop's default (JSON), or `None` when it has none.
    pub fn widget_prop_default(&self, kind: &str, name: &str) -> Option<&J> {
        self.prop_defaults.get(kind).and_then(|m| m.get(name))
    }

    /// The frame (GF1): `{}` until `frame.toml` lands.
    pub fn frame(&self) -> &J {
        &self.frame
    }

    /// `d-` + the first 16 hex of SHA-256 over the canonical JSON of the
    /// parsed content (GS11): comments never change it.
    pub fn fingerprint(&self) -> &str {
        self.fingerprint.get_or_init(|| {
            let templates: serde_json::Map<String, J> = self
                .template_sources
                .iter()
                .map(|(id, src)| {
                    let v: toml::Value =
                        toml::from_str(src).unwrap_or_else(|e| panic!("built-in Zen template {id} does not parse: {e}"));
                    let mut v = serde_json::to_value(v).unwrap_or_else(|e| panic!("template {id} to JSON: {e}"));
                    // The catalog table is metadata, not part of the page.
                    if let Some(o) = v.as_object_mut() {
                        o.remove("catalog");
                    }
                    (id.to_string(), v)
                })
                .collect();
            let themes: serde_json::Map<String, J> = super::BUILTIN_THEMES
                .iter()
                .map(|t| {
                    let v: toml::Value =
                        toml::from_str(t.toml).unwrap_or_else(|e| panic!("built-in theme {} does not parse: {e}", t.name));
                    (t.name.to_string(), serde_json::to_value(v).unwrap_or_else(|e| panic!("theme to JSON: {e}")))
                })
                .collect();
            let props = serde_json::to_value(&self.prop_defaults).unwrap_or_else(|e| panic!("props to JSON: {e}"));
            let all = json!({ "templates": templates, "themes": themes, "props": props, "frame": self.frame });
            let digest = super::bundle::sha256_hex(canonical_json(&all).as_bytes());
            format!("d-{}", &digest[..16])
        })
    }
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
        assert_eq!(d.frame(), &json!({}));
    }

    #[test]
    fn fingerprint_is_stable_and_shaped() {
        let fp = Defaults::live().fingerprint();
        assert!(fp.starts_with("d-") && fp.len() == 18, "{fp}");
        assert_eq!(fp, Defaults::live().fingerprint());
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let a: J = serde_json::from_str(r#"{"b":1,"a":{"d":[{"z":1,"y":2}],"c":0}}"#).expect("json");
        assert_eq!(canonical_json(&a), r#"{"a":{"c":0,"d":[{"y":2,"z":1}]},"b":1}"#);
    }
}
