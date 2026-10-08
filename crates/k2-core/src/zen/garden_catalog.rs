//! The Garden catalog: ready-made Gardens a person loads from New Garden
//! (Rosson 2026-10-08, R5 as changed; prd-zen-user-widgets-v2 UWB21–UWB23,
//! UWB25).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B2** (loader, templates
//! route, `garden/new` with a grant, setup). B4 reads [`TemplateInfo`] from
//! `GET /cli/zen/templates`; its TypeScript mirror is `ZenTemplateInfo` in
//! `src/renderer/lib/zen/zen-custom-types.ts`.
//!
//! **Adding a catalog Garden is data, not code.** Each entry is one file,
//! `src/zen/garden-catalog/<short>-<n>.toml`, holding a page template in the
//! same shape as `template-k2-*.toml` (`id = "k2.<short>@<n>"`, `schema`,
//! `[layout]`, `[[widget]]`) plus one `[catalog]` table of metadata:
//!
//! ```toml
//! id = "k2.diary@1"
//! schema = 1
//!
//! [catalog]
//! short = "diary"                 # stable: New Garden, `--template diary`
//! label = "Diary"
//! description = "Write to one agent at a time; replies appear in handwriting."
//! order = 10                      # place in New Garden's catalog list
//! new_users = false               # true = setup also seeds it on a new computer
//!
//! [catalog.grant]                 # granted by the owner's create click (UWB22)
//! widget = "k2:diary@1"
//! caps = ["agents:read", "thread:read", "thread:post"]
//!
//! [layout]
//! kind = "columns"
//! # … the page, as in any template
//! ```
//!
//! `build.rs` globs the folder into `GARDEN_CATALOG_SOURCES`, so a new file
//! is in the catalog on the next build with no Rust edit. The rules:
//! - **Never auto-appended (R5).** Existing users' Garden lists are never
//!   changed on upgrade. A catalog Garden appears only when the person picks
//!   it in New Garden (or Start with the default).
//! - **New users (open question).** Setup seeds Gardens 1 and 2, then every
//!   entry with `new_users = true`, in `order`. Diary ships with `false`;
//!   preinstalling it is the one-line flip of that key.
//! - **Immutable per version.** A released `<short>-<n>.toml` page never
//!   changes; a better page is `<short>-<n+1>.toml`, and the catalog offers
//!   the highest version of each `short` (a synced Garden shows it; an own
//!   copy keeps the id it names: prd-zen-garden-sync-defaults-v1 GS21,
//!   GS45a, which keep exact ids meaningful for files and v4 bundles). The `[catalog]`
//!   table is metadata: it is not part of the page and not in the page hash,
//!   so `label`, `description`, `order` and `new_users` may change.
//! - A Garden file may name any catalog template id (`template =
//!   "k2.diary@1"`); B2 widens the `TEMPLATE_IDS` check to the catalog.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::builtin_widgets::{parse_widget_ref, WidgetRef};

/// The Diary template (UWB21): the first catalog entry.
pub const DIARY_TEMPLATE_ID: &str = "k2.diary@1";

include!(concat!(env!("OUT_DIR"), "/zen_garden_catalog.rs"));

/// The `[catalog]` table of one entry file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMeta {
    pub short: String,
    pub label: String,
    pub description: String,
    pub order: u32,
    #[serde(default)]
    pub new_users: bool,
    #[serde(default)]
    pub grant: Option<CatalogGrant>,
}

/// What the owner's create click grants (UWB22): the page's built-in
/// widget and its caps, Sending on, scope picked in the dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogGrant {
    /// A versioned built-in, `k2:<name>@<n>`.
    pub widget: String,
    pub caps: Vec<String>,
}

/// One catalog entry: a template version plus its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GardenCatalogEntry {
    /// `<short>-<n>.toml`.
    pub file: &'static str,
    /// `k2.<short>@<n>`.
    pub template_id: String,
    pub version: u32,
    pub meta: CatalogMeta,
    /// The whole file; `template_page` reads it like any template and
    /// ignores `[catalog]`.
    pub toml: &'static str,
}

/// Parse one entry file. Checks: the file name is `<short>-<n>.toml`, `id`
/// is `k2.<short>@<n>` with the same short and version, `[catalog]` has
/// only known keys, a grant names a versioned built-in widget and only
/// widget caps.
pub fn parse_entry(file: &'static str, src: &'static str) -> Result<GardenCatalogEntry, String> {
    let stem = file.strip_suffix(".toml").ok_or_else(|| format!("{file}: catalog files end in .toml"))?;
    let (short, n) = stem.rsplit_once('-').ok_or_else(|| format!("{file}: name it <short>-<version>.toml"))?;
    let version: u32 = match n.parse() {
        Ok(v) if v >= 1 && !n.starts_with('0') => v,
        _ => return Err(format!("{file}: the version after the last '-' is a whole number from 1")),
    };
    let v: toml::Value = toml::from_str(src).map_err(|e| format!("{file}: {e}"))?;
    let id = v.get("id").and_then(toml::Value::as_str).ok_or_else(|| format!("{file}: no id"))?;
    let want = format!("k2.{short}@{version}");
    if id != want {
        return Err(format!("{file}: id is '{id}', the file name says '{want}'"));
    }
    let meta = v.get("catalog").cloned().ok_or_else(|| format!("{file}: no [catalog] table"))?;
    let meta: CatalogMeta = meta.try_into().map_err(|e| format!("{file}: [catalog]: {e}"))?;
    if meta.short != short {
        return Err(format!("{file}: [catalog] short is '{}', the file name says '{short}'", meta.short));
    }
    if let Some(g) = &meta.grant {
        match parse_widget_ref(&g.widget) {
            Ok(WidgetRef::Builtin { version: Some(_), .. }) => {}
            _ => return Err(format!("{file}: [catalog.grant] widget must be a versioned built-in like k2:diary@1")),
        }
        if let Some(bad) = g.caps.iter().find(|c| !super::USER_WIDGET_CAPS.contains(&c.as_str())) {
            return Err(format!("{file}: [catalog.grant] cap '{bad}' isn't available to custom widgets"));
        }
    }
    Ok(GardenCatalogEntry { file, template_id: want, version, meta, toml: src })
}

/// Every catalog entry version K2 ships, in file-name order. A bad built-in
/// file panics with its name (a test runs this load).
pub fn garden_catalog() -> &'static [GardenCatalogEntry] {
    static ENTRIES: OnceLock<Vec<GardenCatalogEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut out: Vec<GardenCatalogEntry> = Vec::new();
        for (file, src) in GARDEN_CATALOG_SOURCES {
            let e = parse_entry(file, src).unwrap_or_else(|e| panic!("built-in Garden catalog: {e}"));
            if out.iter().any(|o| o.template_id == e.template_id) {
                panic!("built-in Garden catalog: {} is listed twice", e.template_id);
            }
            out.push(e);
        }
        out
    })
}

/// The entries New Garden offers: the highest version of each `short`,
/// sorted by `order`, then `short`.
pub fn current_entries() -> Vec<&'static GardenCatalogEntry> {
    let all = garden_catalog();
    let mut out: Vec<&GardenCatalogEntry> = all
        .iter()
        .filter(|e| !all.iter().any(|o| o.meta.short == e.meta.short && o.version > e.version))
        .collect();
    out.sort_by(|a, b| a.meta.order.cmp(&b.meta.order).then_with(|| a.meta.short.cmp(&b.meta.short)));
    out
}

/// A catalog template's TOML by id (any shipped version), for
/// `template_page`.
pub fn catalog_template(id: &str) -> Option<&'static str> {
    garden_catalog().iter().find(|e| e.template_id == id).map(|e| e.toml)
}

/// Where a template shows in New Garden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateSection {
    /// The two starts: Start with the default (`texting`), Start empty
    /// (`blank`).
    Start,
    /// A ready-made Garden from the catalog.
    Catalog,
}

/// One row of `GET /cli/zen/templates` (UWB23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    /// `k2.texting@1`, `k2.diary@1`, …
    pub id: String,
    /// What `garden/new {template}` and `gardens.useTemplate` take.
    pub short: String,
    pub label: String,
    pub description: String,
    pub section: TemplateSection,
    /// Set when creating it also makes a grant (UWB22): the dialog asks for
    /// a scope, and the same owner-only request creates the Garden and the
    /// signed grant.
    pub needs_grant: Option<CatalogGrant>,
    /// Seeded by setup on a new computer.
    pub new_users: bool,
}

/// The list `GET /cli/zen/templates` answers: the two starts, then the
/// catalog. An older daemon has no route; the renderer falls back to the
/// two starts (UWB23).
pub fn template_list() -> Vec<TemplateInfo> {
    let mut out = vec![
        TemplateInfo {
            id: super::schema::TEMPLATE_ID.into(),
            short: "texting".into(),
            label: "Start with the default".into(),
            description: "Garden 1’s layout: your agents beside a conversation.".into(),
            section: TemplateSection::Start,
            needs_grant: None,
            new_users: true,
        },
        TemplateInfo {
            id: super::schema::BLANK_TEMPLATE_ID.into(),
            short: "blank".into(),
            label: "Start empty and ask my agent".into(),
            description: "An empty page. Your agent builds it with you.".into(),
            section: TemplateSection::Start,
            needs_grant: None,
            new_users: true,
        },
    ];
    out.extend(current_entries().into_iter().map(|e| TemplateInfo {
        id: e.template_id.clone(),
        short: e.meta.short.clone(),
        label: e.meta.label.clone(),
        description: e.meta.description.clone(),
        section: TemplateSection::Catalog,
        needs_grant: e.meta.grant.clone(),
        new_users: e.meta.new_users,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIARY_FIXTURE: &str = r#"
id = "k2.diary@1"
schema = 1

[catalog]
short = "diary"
label = "Diary"
description = "Write to one agent at a time."
order = 10

[catalog.grant]
widget = "k2:diary@1"
caps = ["agents:read", "thread:read", "thread:post"]

[layout]
kind = "columns"
"#;

    #[test]
    fn shipped_catalog_loads() {
        let all = garden_catalog();
        for e in all {
            assert!(catalog_template(&e.template_id).is_some());
        }
        let list = template_list();
        assert_eq!(list[0].short, "texting");
        assert_eq!(list[1].short, "blank");
        assert_eq!(list.len(), 2 + current_entries().len());
    }

    #[test]
    fn an_entry_parses_and_defaults_new_users_off() {
        let e = parse_entry("diary-1.toml", DIARY_FIXTURE).expect("fixture parses");
        assert_eq!(e.template_id, DIARY_TEMPLATE_ID);
        assert_eq!(e.version, 1);
        assert!(!e.meta.new_users, "Diary is not preinstalled unless the file says so");
        let g = e.meta.grant.expect("grant");
        assert_eq!(g.widget, crate::zen::builtin_widgets::DIARY_WIDGET);
    }

    #[test]
    fn mismatched_or_bad_entries_are_refused() {
        assert!(parse_entry("diary-2.toml", DIARY_FIXTURE).is_err(), "version differs from id");
        assert!(parse_entry("notes-1.toml", DIARY_FIXTURE).is_err(), "short differs from id");
        assert!(parse_entry("diary.toml", DIARY_FIXTURE).is_err(), "no version in the name");
        assert!(parse_entry("diary-01.toml", DIARY_FIXTURE).is_err());
        let extra: &'static str = Box::leak(DIARY_FIXTURE.replace("order = 10", "order = 10\nicon = \"x\"").into_boxed_str());
        assert!(parse_entry("diary-1.toml", extra).is_err(), "unknown [catalog] key");
        let cap: &'static str =
            Box::leak(DIARY_FIXTURE.replace("\"thread:post\"]", "\"gardens:manage\"]").into_boxed_str());
        assert!(parse_entry("diary-1.toml", cap).is_err(), "non-widget cap in the grant");
        let unpinned: &'static str = Box::leak(DIARY_FIXTURE.replace("k2:diary@1", "k2:diary").into_boxed_str());
        assert!(parse_entry("diary-1.toml", unpinned).is_err(), "grant widget must be versioned");
    }
}
