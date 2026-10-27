//! The one-time move of the data-source cards to `@3` (0.46.4).
//!
//! 0.46.4 shipped the five built-in data-source cards as `@3`, without the
//! "Ask an agent" button (Rosson 2026-10-23: "not useful"), and
//! `k2.sources.ask` now answers `retired`. Built-ins are hash-pinned, so
//! `@1` and `@2` can't change in place: a Garden that names them still
//! draws a button that does nothing. This pass moves them, once.
//!
//! **What it changes.** In each Garden file (`gardens/<id>.toml`), a
//! `[[widget]]` with `kind = "custom"` and `widget = "k2:<card>@1"` or
//! `"k2:<card>@2"`, for exactly the five cards in [`MOVED_CARDS`], gets
//! `@3`. Only those characters of the value move: every other byte of the
//! file (comments, spacing, other widgets, theme tables, a card name in a
//! comment) stays as the person or their agent wrote it.
//!
//! **The Garden sync and lock rules** (wiki "Convention - Zen defaults and
//! Garden sync", [`super::sync`], [`super::lock`]):
//! - K2's defaults (templates, the catalog, prop defaults) never place a
//!   data-source card, so a card in a Garden is always in the Garden's own
//!   file. Synced or own copy, the file is where the card lives, so both
//!   move; nothing in the defaults needs moving. The pass never touches
//!   `sync.json`: whether a Garden syncs stays the person's choice.
//! - A **locked** Garden is skipped and listed: its live text is the frozen
//!   copy, and agents (and K2's passes) don't change it. The person unlocks
//!   it and moves the cards, or asks an agent to.
//! - A file that doesn't check clean (errors, not TOML), or names a card in
//!   an unusual spelling (escapes, a multi-line string), is left as it is
//!   and listed: K2 can't move it without guessing.
//! - The file is re-read just before the write; one that changed in
//!   between is left and listed (never an edit lost).
//!
//! **Backups.** Before a file changes, its text is kept as a history
//! snapshot (`k2 zen history`, `k2 zen reset --garden <id> --to <name>`)
//! and, because snapshots are pruned after [`super::store::HISTORY_KEEP`]
//! newer versions, also in `.history/gardens/<id>.toml/moved/` (a folder,
//! like `refused/`, so it never counts as a snapshot and is never pruned).
//!
//! **Once.** The marker `.moves/source-cards-at-3.json` (the report) says
//! it ran; with it the pass never runs again. It is written last, so a
//! pass that failed half way runs again on the next boot (moved files no
//! longer name `@1` or `@2`, so nothing moves twice).
//!
//! **Where it runs.** The daemon's Zen boot hook (`zen_watch::start_at_boot`),
//! after the Garden sync loader: headless too, with no window open.
//!
//! **News.** When it moved or left something, one `notice:source-cards-at-3`
//! item in `k2 zen news` and the What's new card says what.

use std::fs;
use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use toml_edit::{Item, TableLike, Value};

use super::builtin_widgets::{parse_widget_ref, WidgetRef, BUILTIN_PREFIX};
use super::news::NoticeItem;
use super::schema::CUSTOM_KIND;
use super::store::{list_lock, ZenError, ZenFile, ZenFiles};
use super::sync::now;

/// This move's name: the marker file and the backup file names.
pub const CARD_MOVE: &str = "source-cards-at-3";
/// The daemon-owned folder of one-time move markers (never watched).
pub const MOVES_DIR: &str = ".moves";
/// Backups of moved files, in `.history/gardens/<id>.toml/`.
pub const MOVED_DIR: &str = "moved";
/// The news item's id.
pub const CARD_MOVE_NOTICE: &str = "notice:source-cards-at-3";
/// The five data-source cards (`SOURCE_CARDS` in 0.46.4). No other
/// built-in moves.
pub const MOVED_CARDS: &[&str] = &["fleet-grid", "version-spread", "relays-dns", "deploys", "money-alerts"];
/// The versions that move.
pub const MOVED_FROM: &[u32] = &[1, 2];
/// Where they move.
pub const MOVED_TO: u32 = 3;
/// The marker's `version`.
pub const CARD_MOVE_VERSION: u32 = 1;

/// One card reference that moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardRef {
    pub from: String,
    pub to: String,
}

/// A Garden whose cards moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedGarden {
    pub garden: String,
    pub name: String,
    pub refs: Vec<CardRef>,
    /// The kept copy, relative to the Zen folder (never pruned).
    pub backup: String,
    /// The history snapshot of the old text (`k2 zen reset --to`).
    pub snapshot: String,
}

/// A Garden whose cards stayed, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftGarden {
    pub garden: String,
    pub name: String,
    /// The `@1`/`@2` references it still names (empty when its file isn't
    /// TOML and only the text was searched).
    pub refs: Vec<String>,
    /// `locked`, `has_errors`, `not_toml`, `unusual_spelling`,
    /// `changed_while_moving` or `would_not_check`.
    pub reason: String,
}

/// What the pass did; also the marker's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardMoveReport {
    pub version: u32,
    pub ran_at: String,
    pub moved: Vec<MovedGarden>,
    /// Locked Gardens that name an old card: skipped.
    pub locked: Vec<LeftGarden>,
    /// Gardens K2 couldn't move safely.
    pub left: Vec<LeftGarden>,
}

/// One `widget = "k2:<card>@1|2"` value in a Garden file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardHit {
    /// The value's bytes in the file, quotes included.
    pub range: Range<usize>,
    pub from: String,
    pub to: String,
    /// A plain one-line string (`"…"` or `'…'`) spelled exactly as the
    /// name: the only kind K2 rewrites.
    pub plain: bool,
}

/// The `@3` id for an old data-source card, else `None`.
pub fn moved_card(id: &str) -> Option<String> {
    match parse_widget_ref(id) {
        Ok(WidgetRef::Builtin { name, version: Some(v) }) if MOVED_CARDS.contains(&name.as_str()) && MOVED_FROM.contains(&v) => {
            Some(format!("{BUILTIN_PREFIX}{name}@{MOVED_TO}"))
        }
        _ => None,
    }
}

/// Every old data-source card a Garden file places (`kind = "custom"`
/// widgets, `[[widget]]` blocks or an inline array). `Err` when the text
/// isn't TOML.
pub fn find_card_refs(src: &str) -> Result<Vec<CardHit>, String> {
    let doc = toml_edit::Document::parse(src).map_err(|e| e.message().lines().next().unwrap_or("invalid TOML").to_string())?;
    let Some(item) = doc.as_table().get("widget") else {
        return Ok(Vec::new());
    };
    let tables: Vec<&dyn TableLike> = if let Some(a) = item.as_array_of_tables() {
        a.iter().map(|t| t as &dyn TableLike).collect()
    } else if let Some(arr) = item.as_value().and_then(Value::as_array) {
        arr.iter().filter_map(|v| v.as_inline_table().map(|t| t as &dyn TableLike)).collect()
    } else {
        Vec::new()
    };
    let mut hits = Vec::new();
    for t in tables {
        if t.get("kind").and_then(Item::as_str) != Some(CUSTOM_KIND) {
            continue;
        }
        let Some(w) = t.get("widget") else { continue };
        let Some(id) = w.as_str() else { continue };
        let Some(to) = moved_card(id) else { continue };
        let span = w.span().ok_or_else(|| format!("no position for widget = \"{id}\""))?;
        let raw = src.get(span.clone()).ok_or_else(|| format!("bad position for widget = \"{id}\""))?;
        let lead = raw.len() - raw.trim_start().len();
        let core = raw.trim();
        let start = span.start + lead;
        let plain = core == format!("\"{id}\"") || core == format!("'{id}'");
        hits.push(CardHit { range: start..start + core.len(), from: id.to_string(), to, plain });
    }
    Ok(hits)
}

/// The text with each hit's value set to its `@3` id, same quotes; every
/// other byte as it was. Only for plain hits.
pub fn apply_card_moves(src: &str, hits: &[CardHit]) -> String {
    let mut sorted: Vec<&CardHit> = hits.iter().collect();
    sorted.sort_by_key(|h| std::cmp::Reverse(h.range.start));
    let mut out = src.to_string();
    for h in sorted {
        let q = &src[h.range.start..h.range.start + 1];
        out.replace_range(h.range.clone(), &format!("{q}{}{q}", h.to));
    }
    out
}

/// Whether text that isn't TOML names an old card anywhere (only to list
/// the Garden: such a file is never rewritten).
fn mentions_moved_card(src: &str) -> bool {
    for card in MOVED_CARDS {
        for v in MOVED_FROM {
            let needle = format!("{BUILTIN_PREFIX}{card}@{v}");
            let mut rest = src;
            while let Some(i) = rest.find(&needle) {
                let after = &rest[i + needle.len()..];
                if !after.starts_with(|c: char| c.is_ascii_digit()) {
                    return true;
                }
                rest = after;
            }
        }
    }
    false
}

fn reason_text(reason: &str) -> &'static str {
    match reason {
        "has_errors" => "its file has errors",
        "not_toml" => "its file isn't valid TOML",
        "unusual_spelling" => "a card's name is written in an unusual way",
        "changed_while_moving" => "its file changed while K2 was moving it",
        "would_not_check" => "the moved file wouldn't check",
        "locked" => "locked",
        _ => "it couldn't be moved safely",
    }
}

fn names(list: &[String]) -> String {
    match list.len() {
        0 => String::new(),
        1 => list[0].clone(),
        n => format!("{} and {}", list[..n - 1].join(", "), list[n - 1]),
    }
}

fn unique(refs: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in refs {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// The news item for a report, or `None` when nothing moved or was left.
pub fn card_move_notice(r: &CardMoveReport) -> Option<NoticeItem> {
    if r.moved.is_empty() && r.locked.is_empty() && r.left.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    let mut detail = Vec::new();
    if !r.moved.is_empty() {
        let n: Vec<String> = r.moved.iter().map(|g| g.name.clone()).collect();
        let one = n.len() == 1;
        body.push(format!(
            "K2 moved the data cards in {} to their newest version, without the \"Ask an agent\" button that stopped working. Nothing else in {} changed; the old {} in {} history (k2 zen history).",
            names(&n),
            if one { "it" } else { "them" },
            if one { "file is" } else { "files are" },
            if one { "its" } else { "their" },
        ));
        for g in &r.moved {
            let moves = unique(g.refs.iter().map(|c| format!("{} → {}", c.from, c.to)));
            detail.push(format!("{} ({}): {}; the old file is {}", g.name, g.garden, moves.join(", "), g.backup));
        }
    }
    if !r.locked.is_empty() {
        let n: Vec<String> = r.locked.iter().map(|g| g.name.clone()).collect();
        let one = n.len() == 1;
        body.push(format!(
            "{} {} locked, so K2 left {} as {}: unlock {} and change each card's @1 or @2 to @3.",
            names(&n),
            if one { "is" } else { "are" },
            if one { "it" } else { "them" },
            if one { "it is" } else { "they are" },
            if one { "it" } else { "them" },
        ));
        for g in &r.locked {
            detail.push(format!("{} ({}): locked, not moved: {}", g.name, g.garden, unique(g.refs.clone()).join(", ")));
        }
    }
    if !r.left.is_empty() {
        let n: Vec<String> = r.left.iter().map(|g| format!("{} ({})", g.name, reason_text(&g.reason))).collect();
        body.push(format!("K2 couldn't move the cards in {}: change each card's @1 or @2 to @3 by hand.", names(&n)));
        for g in &r.left {
            let refs = unique(g.refs.clone());
            detail.push(format!(
                "{} ({}): not moved, {}{}",
                g.name,
                g.garden,
                reason_text(&g.reason),
                if refs.is_empty() { String::new() } else { format!(": {}", refs.join(", ")) }
            ));
        }
    }
    let title = if r.moved.is_empty() { "Data cards to update by hand" } else { "Data cards updated" };
    Some(NoticeItem { id: CARD_MOVE_NOTICE.to_string(), title: title.to_string(), body: body.join(" "), detail, at: r.ran_at.clone() })
}

impl ZenFiles {
    /// `.moves/source-cards-at-3.json`.
    pub fn card_move_marker(&self) -> PathBuf {
        self.root().join(MOVES_DIR).join(format!("{CARD_MOVE}.json"))
    }

    /// The marker's report, when the move ran here.
    pub fn card_move_report(&self) -> Option<CardMoveReport> {
        serde_json::from_str(&fs::read_to_string(self.card_move_marker()).ok()?).ok()
    }

    /// Move the data-source cards to `@3`, once (the marker says it ran).
    /// `Ok(None)` when Zen isn't set up or the move already ran. Never call
    /// it holding the Garden list lock.
    pub fn move_source_cards_once(&self) -> Result<Option<CardMoveReport>, ZenError> {
        if !self.is_set_up() {
            return Ok(None);
        }
        // The sync loader first, outside the list lock (it takes its own).
        self.sync_boot();
        let _l = list_lock();
        if self.card_move_marker().exists() {
            return Ok(None);
        }
        let report = self.move_source_cards()?;
        if let Some(n) = card_move_notice(&report) {
            self.record_notice(n)?;
        }
        let path = self.card_move_marker();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
        }
        let body = serde_json::to_string_pretty(&report).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))?;
        Ok(Some(report))
    }

    /// Garden ids and names: the list, then page files it doesn't name.
    fn card_move_gardens(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self.gardens().into_iter().map(|g| (g.id, g.name)).collect();
        for id in self.garden_file_ids() {
            if !out.iter().any(|(i, _)| *i == id) {
                out.push((id.clone(), id));
            }
        }
        out
    }

    /// The pass itself (no marker check, no marker write).
    fn move_source_cards(&self) -> Result<CardMoveReport, ZenError> {
        let mut r = CardMoveReport { version: CARD_MOVE_VERSION, ran_at: now(), moved: Vec::new(), locked: Vec::new(), left: Vec::new() };
        for (id, name) in self.card_move_gardens() {
            let f = ZenFile::Garden(id.clone());
            let left = |refs: Vec<String>, reason: &str| LeftGarden { garden: id.clone(), name: name.clone(), refs, reason: reason.to_string() };
            if self.is_locked(&id) {
                // The live text is the frozen copy; neither it nor the
                // file is changed.
                let Some(Ok(text)) = self.read_current(&f) else { continue };
                match find_card_refs(&text) {
                    Ok(h) if !h.is_empty() => r.locked.push(left(h.into_iter().map(|h| h.from).collect(), "locked")),
                    Ok(_) => {}
                    Err(_) if mentions_moved_card(&text) => r.locked.push(left(Vec::new(), "locked")),
                    Err(_) => {}
                }
                continue;
            }
            let Some(Ok(text)) = self.read_disk(&f) else { continue };
            let hits = match find_card_refs(&text) {
                Ok(h) => h,
                Err(_) => {
                    if mentions_moved_card(&text) {
                        r.left.push(left(Vec::new(), "not_toml"));
                    }
                    continue;
                }
            };
            if hits.is_empty() {
                continue;
            }
            let refs: Vec<String> = hits.iter().map(|h| h.from.clone()).collect();
            if hits.iter().any(|h| !h.plain) {
                r.left.push(left(refs, "unusual_spelling"));
                continue;
            }
            let base = self.base_for(&f);
            if !self.check_src(&f, &text, &base).is_clean() {
                r.left.push(left(refs, "has_errors"));
                continue;
            }
            let moved = apply_card_moves(&text, &hits);
            if !self.check_src(&f, &moved, &base).is_clean() {
                r.left.push(left(refs, "would_not_check"));
                continue;
            }
            // Backups: a history snapshot (reset --to), and a copy that is
            // never pruned.
            let snapshot = match self.snapshots(&f).into_iter().next() {
                Some(s) if self.snapshot_text(&f, &s.name).as_deref() == Some(text.as_str()) => s.name,
                _ => {
                    let n = self.write_snapshot(&f, &text)?;
                    self.prune(&f);
                    n
                }
            };
            let dir = self.history_dir(&f).join(MOVED_DIR);
            fs::create_dir_all(&dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            let backup = dir.join(format!("{stamp}-{CARD_MOVE}.toml"));
            crate::fs_atomic::atomic_write_str(&backup, &text).map_err(|e| ZenError::Io(format!("{}: {e}", backup.display())))?;
            // Never over an edit made meanwhile, nor into a Garden locked
            // meanwhile.
            if self.is_locked(&id) {
                r.locked.push(left(refs, "locked"));
                continue;
            }
            if !matches!(self.read_disk(&f), Some(Ok(now_text)) if now_text == text) {
                r.left.push(left(refs, "changed_while_moving"));
                continue;
            }
            let path = self.path_of(&f);
            crate::fs_atomic::atomic_write_str(&path, &moved).map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))?;
            let rel = crate::paths::slash_rel(backup.strip_prefix(self.root()).unwrap_or(&backup));
            r.moved.push(MovedGarden {
                garden: id.clone(),
                name: name.clone(),
                refs: hits.into_iter().map(|h| CardRef { from: h.from, to: h.to }).collect(),
                backup: rel,
                snapshot,
            });
        }
        Ok(r)
    }
}

#[cfg(test)]
#[path = "card_moves_tests.rs"]
mod tests;
