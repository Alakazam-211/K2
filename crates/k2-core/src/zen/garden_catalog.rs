//! The Garden catalog: ready-made Gardens a person loads from New Garden
//! (Rosson 2026-10-08, R5 as changed; prd-zen-user-widgets-v2 UWB21–UWB23,
//! UWB25).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B2** (loader, templates
//! route, setup). B4 reads [`TemplateInfo`] from
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
//! theme = "haunted"               # optional: a built-in theme it shows by default
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
//! - **New users.** Setup seeds Gardens 1 and 2, then every
//!   entry with `new_users = true`, in `order`, once, on an empty list. The
//!   Diary ships with `true` (Rosson 2026-10-08).
//! - **Immutable per version.** A released `<short>-<n>.toml` page never
//!   changes; a better page is `<short>-<n+1>.toml`, and the catalog offers
//!   the highest version of each `short` (a synced Garden shows it; an own
//!   copy keeps the id it names: prd-zen-garden-sync-defaults-v1 GS21,
//!   GS45a, which keep exact ids meaningful for files and v4 bundles). The `[catalog]`
//!   table is metadata: it is not part of the page and not in the page hash,
//!   so `label`, `description`, `order` and `new_users` may change.
//! - A Garden file may name any catalog template id (`template =
//!   "k2.diary@1"`); `schema::is_template_id` accepts every catalog id.
//! - **No grants (Rosson 2026-10-08).** A catalog Garden's widgets work the
//!   moment it's created, like any widget in your own Garden
//!   (`zen::widget_access`); the old `[catalog.grant]` table is gone and
//!   refused as an unknown key.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The Diary template (UWB21) New Garden and setup use today: the highest
/// `diary-<n>.toml` (k2.diary@1 was the first; @2 moves to `k2:diary@2`).
pub const DIARY_TEMPLATE_ID: &str = "k2.diary@2";

/// The Kanban New Garden makes: `kanban-<n>.toml` (k2.kanban@1 was the
/// first, 0.45.7; @2 moved to `k2:kanban@2`, 0.46.1; @3 moved to
/// `k2:kanban@3` and @4 moved to `k2:kanban@4` (Thread parity), both 0.46.3;
/// @5 is `k2:kanban@5` beside K2's own Conversation in a collapsible
/// "Thread" column, 0.46.4).
pub const KANBAN_TEMPLATE_ID: &str = "k2.kanban@5";

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
    /// The built-in theme a Garden made from this template shows until the
    /// person picks one for it (Rosson 2026-10-08: the Diary's haunted
    /// study). Metadata: not part of the page or its hash.
    #[serde(default)]
    pub theme: Option<String>,
}

/// The default theme of a template (a catalog entry's `theme`), if any.
pub fn template_theme(template_id: &str) -> Option<&'static str> {
    garden_catalog().iter().find(|e| e.template_id == template_id).and_then(|e| e.meta.theme.as_deref())
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
/// only known keys.
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
    if let Some(t) = &meta.theme {
        if super::builtin_theme(t).is_none() {
            return Err(format!("{file}: [catalog] theme '{t}' isn't a built-in theme"));
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

/// The texting start's card in New Garden (Rosson 2026-10-15).
pub const GARDEN_ONE_LABEL: &str = "Garden 1";
pub const GARDEN_ONE_DESCRIPTION: &str =
    "K2’s default: your agents beside a conversation, pinned agents on top, live activity above the box.";

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
            // Rosson 2026-10-15: Garden 1 is offered by name next to the
            // Diary. It stays a start (k2.texting@1, not a catalog copy), so
            // a Garden made from it keeps getting Garden 1's improvements.
            label: GARDEN_ONE_LABEL.into(),
            description: GARDEN_ONE_DESCRIPTION.into(),
            section: TemplateSection::Start,
            new_users: true,
        },
        TemplateInfo {
            id: super::schema::BLANK_TEMPLATE_ID.into(),
            short: "blank".into(),
            label: "Start empty and ask my agent".into(),
            description: "An empty page. Your agent builds it with you.".into(),
            section: TemplateSection::Start,
            new_users: true,
        },
    ];
    out.extend(current_entries().into_iter().map(|e| TemplateInfo {
        id: e.template_id.clone(),
        short: e.meta.short.clone(),
        label: e.meta.label.clone(),
        description: e.meta.description.clone(),
        section: TemplateSection::Catalog,
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
        assert_eq!(list[0].label, "Garden 1", "Garden 1 is offered by name (Rosson 2026-10-15)");
        assert_eq!(list[1].short, "blank");
        assert_eq!(list.len(), 2 + current_entries().len());
    }

    #[test]
    fn an_entry_parses_and_defaults_new_users_off() {
        let e = parse_entry("diary-1.toml", DIARY_FIXTURE).expect("fixture parses");
        assert_eq!(e.template_id, "k2.diary@1");
        assert_eq!(e.version, 1);
        assert!(!e.meta.new_users, "Diary is not preinstalled unless the file says so");
    }

    #[test]
    fn mismatched_or_bad_entries_are_refused() {
        assert!(parse_entry("diary-2.toml", DIARY_FIXTURE).is_err(), "version differs from id");
        assert!(parse_entry("notes-1.toml", DIARY_FIXTURE).is_err(), "short differs from id");
        assert!(parse_entry("diary.toml", DIARY_FIXTURE).is_err(), "no version in the name");
        assert!(parse_entry("diary-01.toml", DIARY_FIXTURE).is_err());
        let extra: &'static str = Box::leak(DIARY_FIXTURE.replace("order = 10", "order = 10\nicon = \"x\"").into_boxed_str());
        assert!(parse_entry("diary-1.toml", extra).is_err(), "unknown [catalog] key");
        // Rosson 2026-10-08: no grants. The old `[catalog.grant]` table
        // is an unknown key now, so a stale entry can't come back.
        let granted: &'static str = Box::leak(
            DIARY_FIXTURE
                .replace("order = 10", "order = 10\n\n[catalog.grant]\nwidget = \"k2:diary@1\"\ncaps = [\"agents:read\"]")
                .into_boxed_str(),
        );
        assert!(parse_entry("diary-1.toml", granted).is_err(), "[catalog.grant] is gone");
    }

    /// Rosson 2026-10-08: a catalog Garden may name a built-in default theme
    /// (the Diary's haunted study); anything else is refused.
    #[test]
    fn a_catalog_theme_is_a_built_in() {
        let themed: &'static str =
            Box::leak(DIARY_FIXTURE.replace("order = 10", "order = 10\ntheme = \"haunted\"").into_boxed_str());
        assert_eq!(parse_entry("diary-1.toml", themed).expect("parses").meta.theme.as_deref(), Some("haunted"));
        let unknown: &'static str =
            Box::leak(DIARY_FIXTURE.replace("order = 10", "order = 10\ntheme = \"neon\"").into_boxed_str());
        assert!(parse_entry("diary-1.toml", unknown).expect_err("refused").contains("isn't a built-in theme"));
        assert_eq!(template_theme(DIARY_TEMPLATE_ID), Some("haunted"), "the shipped Diary shows the study");
        // Garden 1 is a start, not a catalog Garden: its default theme
        // (Glass, 0.46.0) is template data read through the Garden's theme
        // set, so only a synced Garden 1 shows it; the catalog names none.
        assert_eq!(template_theme("k2.texting@1"), None);
        assert_eq!(
            crate::zen::Defaults::live().template_default_theme("k2.texting@1").as_deref(),
            Some("glass"),
            "Garden 1 names Glass in its template"
        );
    }

    /// Rosson 2026-10-08: the Diary works the moment it's created; New
    /// Garden asks nothing and the templates route carries no grant.
    #[test]
    fn the_shipped_diary_needs_no_grant() {
        let row = template_list().into_iter().find(|t| t.short == "diary").expect("listed");
        let wire = serde_json::to_value(&row).expect("serializes");
        assert!(wire.get("needsGrant").is_none(), "{wire}");
        assert_eq!(wire["section"], "catalog");
    }

    /// Rosson 2026-10-15 (0.45.7): the Kanban Garden is new catalog data,
    /// offered in New Garden after the Diary, never seeded on a new
    /// computer. 0.46.1 (Rosson 2026-10-18): `kanban-2.toml` on
    /// `k2:kanban@2` (the Thread look in K2 Companion's colours). 0.46.3
    /// (Rosson 2026-10-20): `kanban-3.toml` on `k2:kanban@3` (the panel keeps
    /// a scrolled-up reader in place), then `kanban-4.toml` on `k2:kanban@4`
    /// (Thread parity: the Thread's files and links). @1–@4 are one page on
    /// four boards, and all still ship for own copies.
    /// 0.46.4 (Rosson 2026-10-24, "the Kanban should get the same update"):
    /// `kanban-5.toml` is the newest: the board (`k2:kanban@5`) and, in a
    /// second collapsible "Thread" column that starts folded, K2's own
    /// Conversation following the board with `pick = "all"`. The Garden
    /// shows K2 Companion's look (`theme = "glass"`, metadata) until the
    /// person picks one; a Garden made from @1–@4 names no theme.
    #[test]
    fn the_kanban_is_offered_after_the_diary_and_not_preinstalled() {
        let older: Vec<&GardenCatalogEntry> = (1..=4)
            .map(|n| {
                let id = format!("k2.kanban@{n}");
                let e = garden_catalog().iter().find(|e| e.template_id == id).unwrap_or_else(|| panic!("kanban-{n}.toml still ships"));
                assert_eq!((e.file.to_string(), e.version), (format!("kanban-{n}.toml"), n));
                assert!(super::super::schema::is_template_id(&id));
                assert_eq!(e.meta.theme, None, "{id} names no theme: a Garden made from it keeps the person's");
                assert_eq!(template_theme(&id), None);
                e
            })
            .collect();
        let one = older[0];
        let e = garden_catalog().iter().find(|e| e.template_id == KANBAN_TEMPLATE_ID).expect("kanban-5.toml ships");
        assert_eq!((e.file, e.version), ("kanban-5.toml", 5));
        assert_eq!(e.meta.label, one.meta.label, "the same Garden, a newer page");
        assert_eq!(e.meta.order, one.meta.order);
        assert_eq!(e.meta.label, "Kanban");
        assert!(e.meta.description.contains("To do, Doing and Done"), "{}", e.meta.description);
        assert!(e.meta.description.contains("the Thread beside it talks to any of them"), "{}", e.meta.description);
        assert!(!e.meta.new_users, "offered, not preinstalled");
        assert_eq!(e.meta.theme.as_deref(), Some("glass"), "K2 Companion's look, so the board and K2's Thread match");
        assert_eq!(template_theme(KANBAN_TEMPLATE_ID), Some("glass"));
        assert!(super::super::schema::is_template_id(KANBAN_TEMPLATE_ID));
        let list = template_list();
        let rows: Vec<(&str, TemplateSection)> = list.iter().map(|t| (t.short.as_str(), t.section)).collect();
        assert_eq!(
            rows,
            [
                ("texting", TemplateSection::Start),
                ("blank", TemplateSection::Start),
                ("diary", TemplateSection::Catalog),
                ("kanban", TemplateSection::Catalog)
            ]
        );
        let row = list.iter().find(|t| t.short == "kanban").expect("listed");
        assert_eq!((row.id.as_str(), row.new_users), (KANBAN_TEMPLATE_ID, false));
        let wire = serde_json::to_value(row).expect("serializes");
        assert_eq!(wire["section"], "catalog");
        assert!(wire.get("needsGrant").is_none(), "{wire}");

        // @5's page: the board, and K2's Thread in a folded "Thread" column.
        let v: toml::Value = toml::from_str(e.toml).expect("parses");
        assert_eq!(v["layout"]["canvas"].as_str(), Some("full"));
        let cols = v["layout"]["column"].as_array().expect("columns");
        assert_eq!(cols.len(), 2, "board + Thread");
        assert_eq!(cols[0].get("collapsible"), None, "the board never folds");
        assert_eq!(cols[1]["collapsible"].as_bool(), Some(true));
        assert_eq!(cols[1]["collapsed"].as_bool(), Some(true), "closed until you press Thread, as @1–@4's panel");
        assert_eq!(cols[1]["label"].as_str(), Some("Thread"), "the label the board's Thread button looks for");
        let widgets = v["widget"].as_array().expect("widgets");
        let content: Vec<(&str, &str, i64)> = widgets
            .iter()
            .filter(|w| w.get("column").is_some())
            .map(|w| (w["id"].as_str().expect("id"), w["kind"].as_str().expect("kind"), w["column"].as_integer().expect("column")))
            .collect();
        assert_eq!(content, [("board", "custom", 0), ("thread", "conversation", 1)]);
        let board = widgets.iter().find(|w| w["id"].as_str() == Some("board")).expect("board");
        assert_eq!(board["widget"].as_str(), Some(crate::zen::builtin_widgets::KANBAN_WIDGET_LATEST));
        let thread = widgets.iter().find(|w| w["id"].as_str() == Some("thread")).expect("thread");
        let props = thread["props"].as_table().expect("props");
        assert_eq!(props.get("agents").and_then(toml::Value::as_str), Some("board"), "it follows the board: one pick for both");
        assert_eq!(props.get("pick").and_then(toml::Value::as_str), Some("all"), "any agent from its header");
        for on in ["activity-drawer", "mute"] {
            assert_eq!(props.get(on).and_then(toml::Value::as_bool), Some(true), "{on}: K2's full Thread");
        }

        // @1–@4 are one page on four boards (only the widget and the id
        // differ); @5 keeps their chrome and controls.
        let page_of = |src: &str, widget: &str| -> toml::Value {
            let mut c: toml::Value = toml::from_str(src).expect("parses");
            let t = c.as_table_mut().expect("table");
            t.remove("id");
            for w in t.get_mut("widget").and_then(toml::Value::as_array_mut).expect("widgets") {
                if w.get("kind").and_then(toml::Value::as_str) == Some("custom") {
                    assert_eq!(w.get("widget").and_then(toml::Value::as_str), Some(widget));
                    w.as_table_mut().expect("widget").remove("widget");
                }
            }
            c
        };
        let base = page_of(one.toml, "k2:kanban@1");
        for (n, older) in older.iter().enumerate().skip(1) {
            assert_eq!(page_of(older.toml, &format!("k2:kanban@{}", n + 1)), base, "{} is kanban-1.toml on its board", older.file);
        }
        let chrome = |v: &toml::Value| -> Vec<toml::Value> {
            v["widget"].as_array().expect("widgets").iter().filter(|w| w.get("column").is_none()).cloned().collect()
        };
        assert_eq!(chrome(&v), chrome(&base), "the same ⋯ menu and its controls");
        assert_eq!(v["control"], base["control"], "the same template controls");
    }

    /// Integration (Zen v2, B1 × B2): every catalog template, every shipped
    /// version, names only built-in widgets that exist in this build. A
    /// catalog Garden has no widget folder of its own, so a `custom` widget
    /// must be a versioned `k2:<name>@<n>` from `BUILTIN_WIDGETS`.
    #[test]
    fn every_catalog_template_names_an_existing_builtin_widget() {
        use crate::zen::builtin_widgets::{builtin_widget, parse_widget_ref, WidgetRef};
        let all = garden_catalog();
        assert!(!all.is_empty(), "the catalog ships at least the Diary");
        for e in all {
            let v: toml::Value = toml::from_str(e.toml).unwrap_or_else(|err| panic!("{}: {err}", e.file));
            let widgets = v.get("widget").and_then(toml::Value::as_array).cloned().unwrap_or_default();
            let mut custom = 0;
            for w in &widgets {
                if w.get("kind").and_then(toml::Value::as_str) != Some("custom") {
                    continue;
                }
                let name = w
                    .get("widget")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_else(|| panic!("{}: a custom widget without `widget =`", e.file));
                let r = parse_widget_ref(name).unwrap_or_else(|err| panic!("{}: {name}: {err}", e.file));
                assert!(
                    matches!(r, WidgetRef::Builtin { version: Some(_), .. }),
                    "{}: '{name}' must be a versioned built-in (k2:<name>@<n>); a catalog Garden has no widget folder",
                    e.file
                );
                assert!(builtin_widget(&r).is_some(), "{}: '{name}' isn't a built-in widget in this build", e.file);
                custom += 1;
            }
            assert!(custom <= crate::zen::schema::MAX_CUSTOM_WIDGETS, "{}: too many custom widgets", e.file);
        }
    }
}
