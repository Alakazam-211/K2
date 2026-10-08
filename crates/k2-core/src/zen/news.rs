//! Garden news for the "What's new" side card (prd-zen-garden-sync-defaults-v1
//! §8.3, GS43a). The daemon decides what is news; the renderer only shows it.
//!
//! - **Catalog item** `catalog:<short>`: a current catalog Garden whose
//!   `short` isn't in `catalogSeen`. A newer version of a known short isn't
//!   news by itself (synced Gardens on it produce an update item instead).
//! - **Update item** `update:<liveFp>:<garden>`: worked out once, when K2's
//!   live defaults change (GS12), for each synced Garden part whose resolved
//!   look differs between the previous set and the live one.
//! - **Copies line**: how many own-copy Gardens have a newer default (no
//!   item, no seen state).
//!
//! `news.json` (daemon-written): `{version, catalogSeen, items, seen}`. The
//! catalog's metadata comes from the live defaults set, so a test set can
//! carry its own catalog.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::defaults::Defaults;
use super::store::{ZenError, ZenFiles};
use super::sync::{now, part_view, Mode, Part, PartState, View};

/// The news file.
pub const NEWS_FILE: &str = "news.json";
/// `news.json`'s `version`.
pub const NEWS_VERSION: u32 = 1;

static NEWS_LOCK: Mutex<()> = Mutex::new(());

fn news_lock() -> MutexGuard<'static, ()> {
    NEWS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// An improved default a synced Garden picked up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateItem {
    pub id: String,
    pub garden: String,
    pub garden_name: String,
    /// `page`, `theme` or `both`.
    pub part: String,
    pub previous: String,
    pub live: String,
    pub at: String,
}

/// `news.json`. Unknown top-level keys (a newer K2's) are kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFile {
    #[serde(default)]
    pub version: u32,
    /// Catalog `short` → the version seen.
    #[serde(default)]
    pub catalog_seen: BTreeMap<String, u32>,
    #[serde(default)]
    pub items: Vec<UpdateItem>,
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(flatten)]
    pub other: BTreeMap<String, J>,
}

impl NewsFile {
    pub fn new() -> NewsFile {
        NewsFile { version: NEWS_VERSION, ..NewsFile::default() }
    }
}

/// One current catalog Garden as news sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogNews {
    pub short: String,
    pub version: u32,
    pub template: String,
    pub label: String,
    pub description: String,
    pub order: i64,
}

/// The current catalog of the set `d`: every template with a `[catalog]`
/// table, the highest version of each `short`, by `order` then `short`.
pub fn catalog_entries(d: &Defaults) -> Vec<CatalogNews> {
    let mut best: BTreeMap<String, CatalogNews> = BTreeMap::new();
    for id in d.template_ids() {
        let Some(t) = d.template_toml(id) else { continue };
        let c = &t["catalog"];
        if !c.is_object() {
            continue;
        }
        let Some((fam, version)) = super::sync::template_family(id) else { continue };
        let short = c["short"].as_str().unwrap_or(fam).to_string();
        let e = CatalogNews {
            short: short.clone(),
            version,
            template: id.to_string(),
            label: c["label"].as_str().unwrap_or(&short).to_string(),
            description: c["description"].as_str().unwrap_or_default().to_string(),
            order: c["order"].as_i64().unwrap_or(0),
        };
        if best.get(&short).is_none_or(|b| b.version < version) {
            best.insert(short, e);
        }
    }
    let mut out: Vec<CatalogNews> = best.into_values().collect();
    out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.short.cmp(&b.short)));
    out
}

/// `(short, version)` of every current catalog Garden.
pub fn catalog_current(d: &Defaults) -> Vec<(String, u32)> {
    catalog_entries(d).into_iter().map(|c| (c.short, c.version)).collect()
}

impl ZenFiles {
    pub fn news_path(&self) -> PathBuf {
        self.root().join(NEWS_FILE)
    }

    /// `news.json`, or an empty one (nothing seen) when missing or
    /// unreadable.
    pub fn read_news(&self) -> NewsFile {
        std::fs::read_to_string(self.news_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(NewsFile::new)
    }

    pub(crate) fn write_news_file(&self, n: &NewsFile) -> Result<(), ZenError> {
        let path = self.news_path();
        let mut n = n.clone();
        n.version = NEWS_VERSION;
        let body = serde_json::to_string_pretty(&n).map_err(|e| ZenError::Io(e.to_string()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
        }
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n"))
            .map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))
    }

    /// GS43a: when the live defaults change from `prev` to `live`, one
    /// update item per Garden whose synced part looks different now.
    pub fn record_update_news(&self, prev: &str, live: &str) -> Result<usize, ZenError> {
        if self.archived_defaults(prev).is_err() {
            return Ok(0);
        }
        let sf = self.sync_state();
        let prev_state = PartState { mode: Mode::Copy, defaults: Some(prev.to_string()), since: None, reason: None };
        let mut items = Vec::new();
        for g in self.gardens() {
            let entry = sf.garden(&g.id);
            let (pg, th) = (entry.part(Part::Page), entry.part(Part::Theme));
            let mut moved = Vec::new();
            for p in [Part::Page, Part::Theme] {
                if entry.part(p).is_copy() {
                    continue;
                }
                let now_v = self.resolve_view(Some(&g.id), &View { page: Some(pg.clone()), theme: Some(th.clone()), ..View::default() })?;
                let then = match p {
                    Part::Page => View { page: Some(prev_state.clone()), theme: Some(th.clone()), ..View::default() },
                    Part::Theme => View { page: Some(pg.clone()), theme: Some(prev_state.clone()), ..View::default() },
                };
                let prev_v = self.resolve_view(Some(&g.id), &then)?;
                if part_view(&now_v, p) != part_view(&prev_v, p) {
                    moved.push(p);
                }
            }
            if moved.is_empty() {
                continue;
            }
            let part = if moved.len() == 2 { "both" } else { moved[0].name() };
            items.push(UpdateItem {
                id: format!("update:{live}:{}", g.id),
                garden: g.id.clone(),
                garden_name: g.name.clone(),
                part: part.to_string(),
                previous: prev.to_string(),
                live: live.to_string(),
                at: now(),
            });
        }
        let _g = news_lock();
        let mut n = self.read_news();
        let mut added = 0;
        for it in items {
            if !n.items.iter().any(|x| x.id == it.id) {
                n.items.push(it);
                added += 1;
            }
        }
        if added > 0 || !self.news_path().exists() {
            self.write_news_file(&n)?;
        }
        Ok(added)
    }

    /// GS43a: creating a Garden from a catalog entry marks it seen. A
    /// template that isn't a catalog entry changes nothing.
    pub fn news_mark_template_seen(&self, template: &str) -> Result<(), ZenError> {
        let live = self.live_defaults();
        let Some(c) = catalog_entries(&live).into_iter().find(|c| {
            c.template == template || c.short == template || super::sync::template_family(template).is_some_and(|(s, _)| s == c.short)
        }) else {
            return Ok(());
        };
        let _g = news_lock();
        let mut n = self.read_news();
        let id = format!("catalog:{}", c.short);
        if n.catalog_seen.get(&c.short).is_some_and(|v| *v >= c.version) && n.seen.contains(&id) {
            return Ok(());
        }
        n.catalog_seen.insert(c.short.clone(), c.version);
        if !n.seen.contains(&id) {
            n.seen.push(id);
        }
        self.write_news_file(&n)
    }

    /// Unseen news, newest first: update items (latest release first), then
    /// new catalog Gardens.
    fn unseen_items(&self) -> Result<Vec<J>, ZenError> {
        let n = self.read_news();
        let live = self.live_defaults();
        let gardens = self.gardens();
        let mut out = Vec::new();
        for it in n.items.iter().rev() {
            if n.seen.contains(&it.id) {
                continue;
            }
            let Some(g) = gardens.iter().find(|g| g.id == it.garden) else { continue };
            let offered = self.keep_previous_offered(&g.id)?;
            let wants: &[Part] = match it.part.as_str() {
                "page" => &[Part::Page],
                "theme" => &[Part::Theme],
                _ => &[Part::Page, Part::Theme],
            };
            let current_prev = self.sync_state().previous_defaults;
            out.push(json!({
                "id": it.id,
                "kind": "update",
                "garden": g.id,
                "gardenName": g.name,
                "part": it.part,
                "previous": it.previous,
                "at": it.at,
                "keepPrevious": current_prev.as_deref() == Some(it.previous.as_str())
                    && wants.iter().any(|p| offered.contains(p)),
            }));
        }
        for c in catalog_entries(&live) {
            let id = format!("catalog:{}", c.short);
            if n.catalog_seen.contains_key(&c.short) || n.seen.contains(&id) {
                continue;
            }
            out.push(json!({
                "id": id,
                "kind": "catalog",
                "short": c.short,
                "label": c.label,
                "description": c.description,
                "template": c.template,
                "version": c.version,
            }));
        }
        Ok(out)
    }

    /// Own-copy Gardens with a newer default (the card's copies line).
    pub fn copies_with_newer_default(&self) -> Result<usize, ZenError> {
        let mut n = 0;
        for g in self.gardens() {
            let gd = self.garden_defaults(&g.id, &View::default());
            if self.newer_default(&g.id, &gd, Part::Page)? || self.newer_default(&g.id, &gd, Part::Theme)? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// `GET /cli/zen/news` (GS31, §8.3).
    pub fn news(&self) -> Result<J, ZenError> {
        if !self.is_set_up() {
            return Err(ZenError::NotSetUp);
        }
        self.sync_boot();
        let items = self.unseen_items()?;
        let sf = self.sync_state();
        Ok(json!({
            "ok": true,
            "items": items,
            "copiesWithNewerDefault": self.copies_with_newer_default()?,
            "liveDefaults": self.live_defaults().fingerprint(),
            "previousDefaults": sf.previous_defaults,
        }))
    }

    /// `POST /cli/zen/news/seen` (GS43): mark `ids` seen, or every unseen
    /// item with `all`. Catalog ids also go into `catalogSeen`.
    pub fn news_seen(&self, ids: &[String], all: bool) -> Result<J, ZenError> {
        if !self.is_set_up() {
            return Err(ZenError::NotSetUp);
        }
        if all == !ids.is_empty() {
            return Err(ZenError::BadRequest("news/seen takes {ids: [...]} or {all: true}".into()));
        }
        for id in ids {
            if !(id.starts_with("catalog:") || id.starts_with("update:")) || id.len() > 200 {
                return Err(ZenError::BadRequest(format!("'{id}' is not a news id (catalog:<short> or update:<fp>:<garden>)")));
            }
        }
        let wanted: Vec<String> = if all {
            self.unseen_items()?.iter().filter_map(|v| v["id"].as_str().map(str::to_string)).collect()
        } else {
            ids.to_vec()
        };
        let live = self.live_defaults();
        let catalog = catalog_entries(&live);
        let _g = news_lock();
        let mut n = self.read_news();
        let mut marked = Vec::new();
        for id in wanted {
            if let Some(short) = id.strip_prefix("catalog:") {
                let v = catalog.iter().find(|c| c.short == short).map(|c| c.version).unwrap_or(1);
                n.catalog_seen.insert(short.to_string(), v);
            }
            if !n.seen.contains(&id) {
                n.seen.push(id.clone());
            }
            marked.push(id);
        }
        self.write_news_file(&n)?;
        Ok(json!({ "ok": true, "seen": marked }))
    }
}
