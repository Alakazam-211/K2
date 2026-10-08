//! Gardens that sync with K2's defaults, or keep their own copy
//! (prd-zen-garden-sync-defaults-v1).
//!
//! Each Garden part (the **page**: template, prop defaults, frame; the
//! **theme**: the built-in layers under its theme) is either synced to K2's
//! defaults (it follows improvements) or its own copy (it resolves on an
//! archived set of the defaults it sat on, so no K2 update changes it).
//!
//! Files (daemon-written, never hand-edited, never watched; GS7–GS10):
//! - `sync.json`: per-Garden state, the undo slot, the live and previous
//!   defaults fingerprints. A Garden with no entry is synced.
//! - `.history/gardens/<id>.toml/sync.json`: a mirror of one Garden's entry,
//!   rewritten on each change; it travels with delete (GS27) and rebuilds a
//!   lost `sync.json` (GS18).
//! - `.defaults/<fp>.json`: one archived set (written once, never pruned).
//! - `news.json` ([`super::news`]).
//!
//! The state, the archive and the one-time upgrade pass load lazily on the
//! first Zen read after boot ([`ZenFiles::sync_boot`]); the daemon's boot
//! hook calls the same loader (GS17). The toggle is the truth: editing a
//! Garden file never changes it (GS2).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::defaults::{Defaults, DefaultsSet};
use super::schema::{Diagnostic, Layer};
use super::store::{ZenError, ZenFile, ZenFiles, GARDENS_DIR};

/// Per-Garden state (GS8).
pub const SYNC_FILE: &str = "sync.json";
/// The archive folder (GS10).
pub const DEFAULTS_DIR: &str = ".defaults";
/// A Garden's mirror, in its history folder (GS8, SD17).
pub const MIRROR_FILE: &str = "sync.json";
/// `sync.json`'s `version`.
pub const SYNC_VERSION: u32 = 1;

/// The required-controls check's floor (GF3): today's values. An archived
/// frame can make the check stricter, never looser.
pub const CONTROL_FLOOR: &[(&str, f64)] =
    &[("target-px", 24.0), ("drag-min-width-px", 120.0), ("drag-min-height-px", 12.0), ("min-opacity", 0.3)];

// ── state shapes ───────────────────────────────────────────────────────

/// One part's mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Follows K2's defaults.
    #[default]
    Synced,
    /// Its own copy, on the archived set `defaults`.
    Copy,
}

/// A Garden part: the page or the theme (GS1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Page,
    Theme,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Page => "page",
            Part::Theme => "theme",
        }
    }
}

/// `page`, `theme` or `both` (routes, CLI, `get?preview=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parts {
    Page,
    Theme,
    Both,
}

impl Parts {
    pub fn parse(s: &str) -> Result<Parts, ZenError> {
        match s.trim() {
            "page" => Ok(Parts::Page),
            "theme" => Ok(Parts::Theme),
            "both" | "" => Ok(Parts::Both),
            other => Err(ZenError::BadRequest(format!("'{other}' is not a Garden part; use page, theme or both"))),
        }
    }

    pub fn list(self) -> &'static [Part] {
        match self {
            Parts::Page => &[Part::Page],
            Parts::Theme => &[Part::Theme],
            Parts::Both => &[Part::Page, Part::Theme],
        }
    }

    pub fn has(self, p: Part) -> bool {
        self.list().contains(&p)
    }

    pub fn name(self) -> &'static str {
        match self {
            Parts::Page => "page",
            Parts::Theme => "theme",
            Parts::Both => "both",
        }
    }
}

/// One part's state (GS8). `reason` is `upgrade`, `turned-off`,
/// `kept-previous`, `turned-on` or `created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartState {
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PartState {
    pub fn synced() -> PartState {
        PartState { mode: Mode::Synced, defaults: None, since: None, reason: None }
    }

    fn synced_because(reason: &str) -> PartState {
        PartState { mode: Mode::Synced, defaults: None, since: Some(now()), reason: Some(reason.into()) }
    }

    fn copy_of(fp: &str, reason: &str) -> PartState {
        PartState { mode: Mode::Copy, defaults: Some(fp.into()), since: Some(now()), reason: Some(reason.into()) }
    }

    pub fn is_copy(&self) -> bool {
        self.mode == Mode::Copy
    }
}

/// The one state before the last change (GS24).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoSlot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PartState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<PartState>,
    #[serde(default)]
    pub at: String,
}

/// One Garden's entry. A missing part is synced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GardenSync {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PartState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<PartState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoSlot>,
}

impl GardenSync {
    pub fn part(&self, p: Part) -> PartState {
        match p {
            Part::Page => self.page.clone(),
            Part::Theme => self.theme.clone(),
        }
        .unwrap_or_else(PartState::synced)
    }

    fn set_part(&mut self, p: Part, st: PartState) {
        match p {
            Part::Page => self.page = Some(st),
            Part::Theme => self.theme = Some(st),
        }
    }

    fn undo_part(u: &UndoSlot, p: Part) -> Option<&PartState> {
        match p {
            Part::Page => u.page.as_ref(),
            Part::Theme => u.theme.as_ref(),
        }
    }
}

/// `sync.json` (GS8). Unknown top-level keys (a newer K2's) are kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub migrated_at: String,
    #[serde(default)]
    pub live_defaults: Option<String>,
    #[serde(default)]
    pub previous_defaults: Option<String>,
    /// Set when the file was rebuilt (GS18): `mirrors` or `keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuilt: Option<String>,
    #[serde(default)]
    pub gardens: BTreeMap<String, GardenSync>,
    #[serde(flatten)]
    pub other: BTreeMap<String, J>,
}

impl SyncFile {
    pub fn garden(&self, id: &str) -> GardenSync {
        self.gardens.get(id).cloned().unwrap_or_default()
    }
}

// ── process state ──────────────────────────────────────────────────────

/// Serialises every write of `sync.json`, the mirrors, the archive and the
/// upgrade pass. Lock order: the list lock (store) before this one; nothing
/// that holds this one takes the list lock.
static SYNC_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn sync_lock() -> MutexGuard<'static, ()> {
    SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn booted() -> MutexGuard<'static, HashSet<PathBuf>> {
    static B: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn overrides() -> MutexGuard<'static, HashMap<PathBuf, Arc<Defaults>>> {
    static O: OnceLock<Mutex<HashMap<PathBuf, Arc<Defaults>>>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn archive_cache() -> MutexGuard<'static, HashMap<(PathBuf, String), Arc<Defaults>>> {
    static C: OnceLock<Mutex<HashMap<(PathBuf, String), Arc<Defaults>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn io(e: std::io::Error, what: &Path) -> ZenError {
    ZenError::Io(format!("{}: {e}", what.display()))
}

fn write_json(path: &Path, v: &impl Serialize) -> Result<(), ZenError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io(e, dir))?;
    }
    let body = serde_json::to_string_pretty(v).map_err(|e| ZenError::Io(e.to_string()))?;
    crate::fs_atomic::atomic_write_str(path, &(body + "\n")).map_err(|e| io(e, path))
}

/// A defaults fingerprint as a file name: `d-` + 16 lower-case hex.
pub fn valid_fingerprint(fp: &str) -> bool {
    fp.strip_prefix("d-").is_some_and(|h| h.len() == 16 && h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// `k2.<short>@<n>` → `(short, n)`.
pub fn template_family(id: &str) -> Option<(&str, u32)> {
    let rest = id.strip_prefix("k2.")?;
    let (short, n) = rest.rsplit_once('@')?;
    Some((short, n.parse().ok()?))
}

/// GS21: a synced page shows its family's highest version in `d`.
pub fn newest_in_family(d: &Defaults, id: &str) -> String {
    let Some((short, n)) = template_family(id) else { return id.to_string() };
    let mut best = (n, id.to_string());
    for tid in d.template_ids() {
        if let Some((s, m)) = template_family(tid) {
            if s == short && m > best.0 {
                best = (m, tid.to_string());
            }
        }
    }
    best.1
}

/// The frame sent in `/cli/zen/get` (GF1): the set's frame with the
/// control thresholds clamped to [`CONTROL_FLOOR`] (GF3).
pub fn frame_json(d: &Defaults) -> J {
    let mut f = d.frame().clone();
    if !f["controls"].is_object() {
        f["controls"] = json!({});
    }
    for (k, floor) in CONTROL_FLOOR {
        let v = f["controls"][*k].as_f64().unwrap_or(*floor);
        let clamped = if v < *floor { *floor } else { v };
        f["controls"][*k] = num(clamped);
    }
    f
}

fn num(v: f64) -> J {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// Control thresholds in a frame that sit below the floor (doctor).
pub fn frame_below_floor(frame: &J) -> Vec<String> {
    CONTROL_FLOOR
        .iter()
        .filter_map(|(k, floor)| {
            let v = frame["controls"][*k].as_f64()?;
            (v < *floor).then(|| format!("controls.{k} = {v} (floor {floor})"))
        })
        .collect()
}

/// How `resolve` should treat a Garden's parts.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// Resolve these parts as if synced (GS25 preview); writes nothing.
    pub preview: Option<Parts>,
    /// Resolve with these states instead of the stored ones.
    pub page: Option<PartState>,
    pub theme: Option<PartState>,
    /// Add the `sync` block (and work out `newerDefault`).
    pub meta: bool,
}

impl View {
    /// What `/cli/zen/get` answers.
    pub fn current() -> View {
        View { meta: true, ..View::default() }
    }

    pub fn preview(p: Parts) -> View {
        View { preview: Some(p), meta: true, ..View::default() }
    }

    fn bare(page: PartState, theme: PartState) -> View {
        View { preview: None, page: Some(page), theme: Some(theme), meta: false }
    }
}

/// The defaults one Garden resolves on.
#[derive(Debug, Clone)]
pub struct GardenDefaults {
    pub page: Arc<Defaults>,
    pub page_state: PartState,
    pub theme: Arc<Defaults>,
    pub theme_state: PartState,
    /// Damaged sets that fell back to live (GS14).
    pub damaged: Vec<(String, String)>,
}

impl GardenDefaults {
    /// The page layer and default template to build the page from: a
    /// synced page's template ids move to their family's newest (GS21).
    pub fn page_layer(&self, layer: &Layer, default_template: &str) -> (Layer, String) {
        if self.page_state.is_copy() {
            return (layer.clone(), default_template.to_string());
        }
        let mut l = layer.clone();
        if let Some(t) = l.get("page.template").and_then(J::as_str).map(str::to_string) {
            l.insert("page.template".into(), json!(newest_in_family(&self.page, &t)));
        }
        (l, newest_in_family(&self.page, default_template))
    }

    /// GS14 warnings for `/cli/zen/get`.
    pub fn warnings(&self) -> Vec<Diagnostic> {
        self.damaged
            .iter()
            .map(|(fp, msg)| Diagnostic {
                file: format!("{DEFAULTS_DIR}/{fp}.json"),
                line: 1,
                col: 1,
                message: format!(
                    "K2's archived defaults {fp} are damaged ({msg}); this Garden shows K2's current defaults until it is fixed. k2 zen doctor has the detail"
                ),
            })
            .collect()
    }
}

/// What a sync route did.
#[derive(Debug, Clone)]
pub struct SyncChange {
    pub garden: String,
    pub changed: bool,
}

// ── ZenFiles: defaults, archive, state ─────────────────────────────────

impl ZenFiles {
    /// A folder whose live defaults are `live` instead of this binary's
    /// (tests, GS48): resolving, archiving and news read `live` as K2's
    /// current defaults. Also forgets this root's boot, so the next read
    /// runs the loader again as a new release would.
    pub fn with_defaults(root: impl Into<PathBuf>, live: Defaults) -> ZenFiles {
        let f = ZenFiles::new(root);
        overrides().insert(f.root().to_path_buf(), Arc::new(live));
        f.forget_boot();
        f
    }

    /// Forget that this root's loader ran in this process (tests; a daemon
    /// restart does the same).
    pub fn forget_boot(&self) {
        booted().remove(self.root());
        archive_cache().retain(|(r, _), _| r != self.root());
    }

    /// K2's current defaults for this folder.
    pub fn live_defaults(&self) -> Arc<Defaults> {
        overrides().get(self.root()).cloned().unwrap_or_else(Defaults::live_arc)
    }

    pub fn sync_path(&self) -> PathBuf {
        self.root().join(SYNC_FILE)
    }

    pub fn defaults_dir(&self) -> PathBuf {
        self.root().join(DEFAULTS_DIR)
    }

    fn set_path(&self, fp: &str) -> PathBuf {
        self.defaults_dir().join(format!("{fp}.json"))
    }

    /// `.history/gardens/<id>.toml/sync.json`.
    pub fn mirror_path(&self, id: &str) -> PathBuf {
        self.history_root().join(ZenFile::Garden(id.to_string()).label()).join(MIRROR_FILE)
    }

    fn read_mirror(&self, id: &str) -> Option<GardenSync> {
        let text = fs::read_to_string(self.mirror_path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn any_mirror(&self) -> bool {
        fs::read_dir(self.history_root().join(GARDENS_DIR))
            .map(|rd| rd.flatten().any(|e| e.path().join(MIRROR_FILE).is_file()))
            .unwrap_or(false)
    }

    /// Every `.defaults/*.json`: `(fingerprint from the name, set or why
    /// it is damaged, bytes)`, by name.
    pub fn archived_sets(&self) -> Vec<(String, Result<DefaultsSet, String>, u64)> {
        let mut out: Vec<(String, Result<DefaultsSet, String>, u64)> = fs::read_dir(self.defaults_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let fp = name.strip_suffix(".json")?.to_string();
                        if !valid_fingerprint(&fp) || !e.path().is_file() {
                            return None;
                        }
                        let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                        Some((fp.clone(), read_set(&e.path(), &fp), bytes))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// An archived set, parsed and cached (GS19). The live fingerprint is
    /// the live set (same content). `Err` says why a set is damaged (GS14).
    pub fn archived_defaults(&self, fp: &str) -> Result<Arc<Defaults>, String> {
        let live = self.live_defaults();
        if fp == live.fingerprint() {
            return Ok(live);
        }
        if !valid_fingerprint(fp) {
            return Err(format!("'{fp}' is not a defaults fingerprint"));
        }
        let key = (self.root().to_path_buf(), fp.to_string());
        if let Some(d) = archive_cache().get(&key) {
            return Ok(d.clone());
        }
        let set = read_set(&self.set_path(fp), fp)?;
        let d = Arc::new(Defaults::from_set(set)?);
        archive_cache().insert(key, d.clone());
        Ok(d)
    }

    /// Write the live set to `.defaults/<fp>.json` when it isn't there (or
    /// is damaged: same content from the binary, GS14). Returns whether it
    /// wrote. Never rewrites a good set.
    fn archive_live(&self, live: &Defaults) -> Result<bool, ZenError> {
        let fp = live.fingerprint();
        let path = self.set_path(fp);
        if path.is_file() && read_set(&path, fp).is_ok() {
            return Ok(false);
        }
        let mut set = live.set().clone();
        set.archived_at = now();
        write_json(&path, &set)?;
        Ok(true)
    }

    /// `sync.json` as stored, with entries for list Gardens that have
    /// none taken from their mirror (a restored Garden comes back with its
    /// state, GS27). `None` when there is no readable file.
    fn read_sync_file(&self) -> Option<SyncFile> {
        let text = fs::read_to_string(self.sync_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The sync state (GS8), after the loader ran. Missing file = every
    /// Garden synced.
    pub fn sync_state(&self) -> SyncFile {
        self.sync_boot();
        let mut sf = self.read_sync_file().unwrap_or_else(|| SyncFile { version: SYNC_VERSION, ..SyncFile::default() });
        for g in self.gardens() {
            if !sf.gardens.contains_key(&g.id) {
                if let Some(m) = self.read_mirror(&g.id) {
                    sf.gardens.insert(g.id, m);
                }
            }
        }
        sf
    }

    fn write_sync(&self, sf: &SyncFile) -> Result<(), ZenError> {
        let mut sf = sf.clone();
        sf.version = SYNC_VERSION;
        write_json(&self.sync_path(), &sf)
    }

    fn write_mirror(&self, id: &str, g: &GardenSync) -> Result<(), ZenError> {
        let mut v = serde_json::to_value(g).map_err(|e| ZenError::Io(e.to_string()))?;
        v["garden"] = json!(id);
        write_json(&self.mirror_path(id), &v)
    }

    fn part_defaults(&self, st: &PartState, damaged: &mut Vec<(String, String)>) -> Arc<Defaults> {
        match (st.mode, st.defaults.as_deref()) {
            (Mode::Copy, Some(fp)) => match self.archived_defaults(fp) {
                Ok(d) => d,
                Err(e) => {
                    if !damaged.iter().any(|(f, _)| f == fp) {
                        damaged.push((fp.to_string(), e));
                    }
                    self.live_defaults()
                }
            },
            _ => self.live_defaults(),
        }
    }

    /// The defaults Garden `id` resolves on, seen through `view` (GS20).
    pub fn garden_defaults(&self, id: &str, view: &View) -> GardenDefaults {
        let stored = self.sync_state().garden(id);
        let pick = |p: Part, over: &Option<PartState>| -> PartState {
            if view.preview.is_some_and(|pv| pv.has(p)) {
                return PartState::synced();
            }
            over.clone().unwrap_or_else(|| stored.part(p))
        };
        let page_state = pick(Part::Page, &view.page);
        let theme_state = pick(Part::Theme, &view.theme);
        let mut damaged = Vec::new();
        let page = self.part_defaults(&page_state, &mut damaged);
        let theme = self.part_defaults(&theme_state, &mut damaged);
        GardenDefaults { page, page_state, theme, theme_state, damaged }
    }

    /// The set a Garden's page is checked and drawn on.
    pub fn page_defaults(&self, id: &str) -> Arc<Defaults> {
        self.garden_defaults(id, &View::default()).page
    }

    /// The set a Garden's theme stack sits on.
    pub fn theme_defaults(&self, id: &str) -> Arc<Defaults> {
        self.garden_defaults(id, &View::default()).theme
    }

    /// What the `refresh` fingerprint covers of the sync state (GS9).
    pub fn sync_fingerprint_state(&self) -> J {
        let sf = self.sync_state();
        json!({
            "liveDefaults": sf.live_defaults,
            "previousDefaults": sf.previous_defaults,
            "gardens": sf.gardens,
        })
    }

    // ── the loader and the upgrade pass ──────────────────────────────

    /// GS17: archive the live set and run the one-time upgrade pass (or
    /// rebuild a lost `sync.json`) on the first Zen read after boot. Runs
    /// once per process per folder; a no-op until Zen is set up. Returns
    /// one log line per Garden the pass or a rebuild decided.
    pub fn sync_boot(&self) -> Vec<String> {
        if booted().contains(self.root()) || !self.is_set_up() {
            return Vec::new();
        }
        let (lines, rotated) = {
            let _g = sync_lock();
            if booted().contains(self.root()) {
                return Vec::new();
            }
            let r = self.boot_locked();
            booted().insert(self.root().to_path_buf());
            match r {
                Ok(v) => v,
                Err(e) => {
                    crate::log_debug!("[zen/sync] loader: {e}");
                    (Vec::new(), None)
                }
            }
        };
        for l in &lines {
            crate::log_debug!("{l}");
        }
        if let Some((prev, live)) = rotated {
            if let Err(e) = self.record_update_news(&prev, &live) {
                crate::log_debug!("[zen/sync] news for {live}: {e}");
            }
        }
        lines
    }

    #[allow(clippy::type_complexity)]
    fn boot_locked(&self) -> Result<(Vec<String>, Option<(String, String)>), ZenError> {
        let live = self.live_defaults();
        let fp = live.fingerprint().to_string();
        let had_sets = !self.archived_sets().is_empty();
        let had_mirrors = self.any_mirror();
        self.archive_live(&live)?;
        let path = self.sync_path();
        let existing = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<SyncFile>(&text) {
                Ok(sf) => Some(sf),
                Err(e) => {
                    // Kept, never overwritten (like gardens.json).
                    let dir = self.history_root().join(SYNC_FILE);
                    fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
                    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
                    let dst = dir.join(format!("{stamp}-unreadable.json"));
                    fs::rename(&path, &dst).map_err(|e| io(e, &dst))?;
                    crate::log_debug!("[zen/sync] sync.json doesn't parse ({e}); kept as {}", dst.display());
                    None
                }
            },
            Err(_) => None,
        };
        if let Some(mut sf) = existing {
            if sf.live_defaults.as_deref() == Some(fp.as_str()) {
                return Ok((Vec::new(), None));
            }
            let prev = sf.live_defaults.take();
            sf.previous_defaults = prev.clone();
            sf.live_defaults = Some(fp.clone());
            self.write_sync(&sf)?;
            let line = format!("[zen/sync] K2's Garden defaults are now {fp} (previous {})", prev.as_deref().unwrap_or("none"));
            return Ok((vec![line], prev.map(|p| (p, fp))));
        }
        if !had_mirrors && !had_sets {
            return Ok((self.upgrade_pass(&fp)?, None));
        }
        Ok((self.rebuild(&fp, had_mirrors)?, None))
    }

    /// Ids of every Garden: the list, then page files it doesn't name.
    fn all_garden_ids(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self.gardens().into_iter().map(|g| (g.id, g.name)).collect();
        for id in self.garden_file_ids() {
            if !out.iter().any(|(i, _)| *i == id) {
                out.push((id.clone(), id));
            }
        }
        out
    }

    fn describe(id: &str, name: &str, g: &GardenSync) -> String {
        let p = g.part(Part::Page);
        let t = g.part(Part::Theme);
        let what = match (p.is_copy(), t.is_copy()) {
            (false, false) => "synced (page, theme)".to_string(),
            (true, true) if p.defaults == t.defaults => {
                format!("own copy (page, theme) of {}", p.defaults.as_deref().unwrap_or("?"))
            }
            _ => format!(
                "page {}, theme {}",
                if p.is_copy() { format!("own copy of {}", p.defaults.as_deref().unwrap_or("?")) } else { "synced".into() },
                if t.is_copy() { format!("own copy of {}", t.defaults.as_deref().unwrap_or("?")) } else { "synced".into() },
            ),
        };
        format!("[zen/sync] {id} {name}: {what}")
    }

    /// GS35: every Garden whose file sets nothing (or is missing) syncs;
    /// every Garden with its own keys (or a file that doesn't parse)
    /// becomes its own copy of the live set `fp`, page and theme. Writes
    /// the mirrors, then `sync.json`, then `news.json`. Never writes a
    /// Garden file, `gardens.json`, `active.json` or a theme.
    fn upgrade_pass(&self, fp: &str) -> Result<Vec<String>, ZenError> {
        let mut sf = SyncFile {
            version: SYNC_VERSION,
            migrated_at: now(),
            live_defaults: Some(fp.to_string()),
            ..SyncFile::default()
        };
        let mut lines = Vec::new();
        for (id, name) in self.all_garden_ids() {
            let untouched = matches!(self.garden_own_keys(&id), Ok(k) if k.is_empty());
            let entry = if untouched {
                GardenSync {
                    page: Some(PartState::synced_because("upgrade")),
                    theme: Some(PartState::synced_because("upgrade")),
                    undo: None,
                }
            } else {
                GardenSync {
                    page: Some(PartState::copy_of(fp, "upgrade")),
                    theme: Some(PartState::copy_of(fp, "upgrade")),
                    undo: None,
                }
            };
            lines.push(Self::describe(&id, &name, &entry));
            self.write_mirror(&id, &entry)?;
            sf.gardens.insert(id, entry);
        }
        self.write_sync(&sf)?;
        // GS35 step 5: on a computer already set up, nothing in the
        // catalog has been seen yet.
        if !self.news_path().exists() {
            self.write_news_file(&super::news::NewsFile::new())?;
        }
        Ok(lines)
    }

    /// GS18: `sync.json` is gone but mirrors or archived sets remain.
    /// From mirrors when there are any; else Gardens with their own keys
    /// become copies of the newest set that isn't live (or of live when it
    /// is the only one).
    fn rebuild(&self, fp: &str, from_mirrors: bool) -> Result<Vec<String>, ZenError> {
        let newest_other: Option<String> = {
            let mut sets: Vec<(String, String)> = self
                .archived_sets()
                .into_iter()
                .filter(|(f, s, _)| f != fp && s.is_ok())
                .map(|(f, s, _)| (s.map(|s| s.archived_at).unwrap_or_default(), f))
                .collect();
            sets.sort();
            sets.pop().map(|(_, f)| f)
        };
        let mut sf = SyncFile {
            version: SYNC_VERSION,
            migrated_at: now(),
            live_defaults: Some(fp.to_string()),
            previous_defaults: newest_other.clone(),
            rebuilt: Some(if from_mirrors { "mirrors" } else { "keys" }.to_string()),
            ..SyncFile::default()
        };
        let mut lines = Vec::new();
        for (id, name) in self.all_garden_ids() {
            let entry = if from_mirrors {
                match self.read_mirror(&id) {
                    Some(m) => m,
                    None => continue,
                }
            } else {
                let untouched = matches!(self.garden_own_keys(&id), Ok(k) if k.is_empty());
                if untouched {
                    continue;
                }
                let set = newest_other.clone().unwrap_or_else(|| fp.to_string());
                let e = GardenSync {
                    page: Some(PartState::copy_of(&set, "upgrade")),
                    theme: Some(PartState::copy_of(&set, "upgrade")),
                    undo: None,
                };
                self.write_mirror(&id, &e)?;
                e
            };
            lines.push(format!("{} (rebuilt from {})", Self::describe(&id, &name, &entry), sf.rebuilt.as_deref().unwrap_or("?")));
            sf.gardens.insert(id, entry);
        }
        self.write_sync(&sf)?;
        Ok(lines)
    }

    /// Fresh setup (GS38): the first Gardens start synced (no entries),
    /// the live set is archived, and every current catalog Garden counts as
    /// seen. Called by `setup` when it made the first Gardens. A folder
    /// that already has sync state is left alone.
    pub fn sync_after_setup(&self) -> Result<(), ZenError> {
        let _g = sync_lock();
        if self.sync_path().exists() || self.any_mirror() || !self.archived_sets().is_empty() {
            return Ok(());
        }
        let live = self.live_defaults();
        self.archive_live(&live)?;
        let sf = SyncFile {
            version: SYNC_VERSION,
            migrated_at: now(),
            live_defaults: Some(live.fingerprint().to_string()),
            ..SyncFile::default()
        };
        self.write_sync(&sf)?;
        let mut news = super::news::NewsFile::new();
        for (short, version) in super::news::catalog_current(&live) {
            news.catalog_seen.insert(short, version);
        }
        self.write_news_file(&news)?;
        booted().insert(self.root().to_path_buf());
        Ok(())
    }

    // ── changes ──────────────────────────────────────────────────────

    /// Apply `f` to Garden `id`'s entry under the sync lock, keeping the
    /// parts it changed in the undo slot (unless `keep_undo`). Writes the
    /// mirror and `sync.json` only when something changed.
    fn change_entry(
        &self,
        id: &str,
        keep_undo: bool,
        f: impl FnOnce(&GardenSync, &SyncFile) -> Result<Vec<(Part, PartState)>, ZenError>,
    ) -> Result<bool, ZenError> {
        self.sync_boot();
        let _g = sync_lock();
        let mut sf = self.sync_state_unlocked();
        let mut entry = sf.garden(id);
        let changes = f(&entry, &sf)?;
        let changes: Vec<(Part, PartState)> = changes.into_iter().filter(|(p, st)| entry.part(*p) != *st).collect();
        if changes.is_empty() {
            return Ok(false);
        }
        if !keep_undo {
            let mut undo = UndoSlot { page: None, theme: None, at: now() };
            for (p, _) in &changes {
                let before = entry.part(*p);
                match p {
                    Part::Page => undo.page = Some(before),
                    Part::Theme => undo.theme = Some(before),
                }
            }
            entry.undo = Some(undo);
        }
        for (p, st) in changes {
            entry.set_part(p, st);
        }
        self.write_mirror(id, &entry)?;
        sf.gardens.insert(id.to_string(), entry);
        self.write_sync(&sf)?;
        Ok(true)
    }

    /// [`ZenFiles::sync_state`] without the loader (the caller holds the
    /// sync lock and already ran it).
    fn sync_state_unlocked(&self) -> SyncFile {
        let mut sf = self.read_sync_file().unwrap_or_else(|| SyncFile {
            version: SYNC_VERSION,
            live_defaults: Some(self.live_defaults().fingerprint().to_string()),
            ..SyncFile::default()
        });
        for g in self.gardens() {
            if !sf.gardens.contains_key(&g.id) {
                if let Some(m) = self.read_mirror(&g.id) {
                    sf.gardens.insert(g.id, m);
                }
            }
        }
        sf
    }

    /// GS24/GS25: turn sync on or off for `parts` of a Garden. Off keeps
    /// the live set as the copy (nothing on screen changes); on follows
    /// K2's defaults. The state before goes to `undo`.
    pub fn set_garden_sync(&self, garden: &str, parts: Parts, on: bool) -> Result<SyncChange, ZenError> {
        let (_, g) = self.garden(garden)?;
        let live = self.live_defaults().fingerprint().to_string();
        let changed = self.change_entry(&g.id, false, |entry, _| {
            Ok(parts
                .list()
                .iter()
                .filter(|p| entry.part(**p).is_copy() == on)
                .map(|p| {
                    let st = if on { PartState::synced_because("turned-on") } else { PartState::copy_of(&live, "turned-off") };
                    (*p, st)
                })
                .collect())
        })?;
        Ok(SyncChange { garden: g.id, changed })
    }

    /// GS25: put the undo slot back. The slot is used up.
    pub fn undo_garden_sync(&self, garden: &str) -> Result<SyncChange, ZenError> {
        let (_, g) = self.garden(garden)?;
        self.sync_boot();
        let _l = sync_lock();
        let mut sf = self.sync_state_unlocked();
        let mut entry = sf.garden(&g.id);
        let Some(undo) = entry.undo.take() else {
            return Err(ZenError::BadRequest(format!("nothing to undo for {}", g.name)));
        };
        for p in [Part::Page, Part::Theme] {
            if let Some(st) = GardenSync::undo_part(&undo, p) {
                entry.set_part(p, st.clone());
            }
        }
        self.write_mirror(&g.id, &entry)?;
        sf.gardens.insert(g.id.clone(), entry);
        self.write_sync(&sf)?;
        Ok(SyncChange { garden: g.id, changed: true })
    }

    /// GS26: a synced part becomes a copy of `previousDefaults`. Only when
    /// it is offered: a previous set exists, loads, and this Garden's part
    /// resolves differently on it.
    pub fn keep_previous(&self, garden: &str, parts: Parts) -> Result<SyncChange, ZenError> {
        let (_, g) = self.garden(garden)?;
        let offered = self.keep_previous_offered(&g.id)?;
        let wanted: Vec<Part> = parts.list().iter().copied().filter(|p| offered.contains(p)).collect();
        if wanted.is_empty() {
            return Err(ZenError::BadRequest(format!(
                "Keep my previous look isn't offered for {} ({}): it needs a synced part whose look changed with this K2's defaults",
                g.name,
                parts.name()
            )));
        }
        let prev = self.sync_state().previous_defaults.unwrap_or_default();
        let changed = self.change_entry(&g.id, false, |_, _| {
            Ok(wanted.iter().map(|p| (*p, PartState::copy_of(&prev, "kept-previous"))).collect())
        })?;
        Ok(SyncChange { garden: g.id, changed })
    }

    /// The parts of Garden `id` that "Keep my previous look" may apply to
    /// (GS26, §8): synced, with a loadable previous set it resolves
    /// differently on.
    pub fn keep_previous_offered(&self, id: &str) -> Result<Vec<Part>, ZenError> {
        let sf = self.sync_state();
        let Some(prev) = sf.previous_defaults.clone() else { return Ok(Vec::new()) };
        let entry = sf.garden(id);
        let prev_state = PartState { mode: Mode::Copy, defaults: Some(prev.clone()), since: None, reason: None };
        if self.archived_defaults(&prev).is_err() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for p in [Part::Page, Part::Theme] {
            if entry.part(p).is_copy() {
                continue;
            }
            let (pg, th) = (entry.part(Part::Page), entry.part(Part::Theme));
            let (with_prev_page, with_prev_theme) = match p {
                Part::Page => (prev_state.clone(), th.clone()),
                Part::Theme => (pg.clone(), prev_state.clone()),
            };
            let now_v = self.resolve_view(Some(id), &View::bare(pg, th))?;
            let prev_v = self.resolve_view(Some(id), &View::bare(with_prev_page, with_prev_theme))?;
            if part_view(&now_v, p) != part_view(&prev_v, p) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// `newerDefault` (GS30): the part is a copy and resolving it synced
    /// would differ.
    pub fn newer_default(&self, id: &str, gd: &GardenDefaults, p: Part) -> Result<bool, ZenError> {
        let st = match p {
            Part::Page => &gd.page_state,
            Part::Theme => &gd.theme_state,
        };
        if !st.is_copy() {
            return Ok(false);
        }
        let live = self.live_defaults();
        let set = match p {
            Part::Page => &gd.page,
            Part::Theme => &gd.theme,
        };
        if set.fingerprint() == live.fingerprint() {
            // Same set: only a page's template family can move (GS21).
            if p == Part::Theme {
                return Ok(false);
            }
            let tid = self.default_template(id);
            let file_tid = self
                .resolve_view(Some(id), &View::bare(gd.page_state.clone(), gd.theme_state.clone()))?["page"]["template"]
                .as_str()
                .unwrap_or(&tid)
                .to_string();
            if newest_in_family(&live, &file_tid) == file_tid {
                return Ok(false);
            }
        }
        let (pg, th) = (gd.page_state.clone(), gd.theme_state.clone());
        let mine = self.resolve_view(Some(id), &View::bare(pg.clone(), th.clone()))?;
        let synced = match p {
            Part::Page => View::bare(PartState::synced(), th),
            Part::Theme => View::bare(pg, PartState::synced()),
        };
        let theirs = self.resolve_view(Some(id), &synced)?;
        Ok(part_view(&mine, p) != part_view(&theirs, p))
    }

    /// The `sync` block of `/cli/zen/get` (GS30).
    pub fn sync_json(&self, id: &str, gd: &GardenDefaults, view: &View) -> Result<J, ZenError> {
        let part = |p: Part, st: &PartState| -> Result<J, ZenError> {
            let mut v = json!({ "mode": st.mode, "newerDefault": self.newer_default(id, gd, p)? });
            if let Some(fp) = &st.defaults {
                v["defaults"] = json!(fp);
            }
            if let Some(s) = &st.since {
                v["since"] = json!(s);
            }
            Ok(v)
        };
        let mut out = json!({
            "page": part(Part::Page, &gd.page_state)?,
            "theme": part(Part::Theme, &gd.theme_state)?,
        });
        if let Some(p) = view.preview {
            out["preview"] = json!(p.name());
        }
        Ok(out)
    }

    /// `GET /cli/zen/sync` (GS31): every Garden's state.
    pub fn sync_list(&self) -> Result<J, ZenError> {
        if !self.is_set_up() {
            return Err(ZenError::NotSetUp);
        }
        let sf = self.sync_state();
        let active = self.read_active();
        let mut rows = Vec::new();
        for (i, g) in self.gardens().iter().enumerate() {
            let gd = self.garden_defaults(&g.id, &View::default());
            let entry = sf.garden(&g.id);
            let part = |p: Part, st: &PartState, d: &Defaults| -> Result<J, ZenError> {
                let mut v = json!({
                    "mode": st.mode,
                    "newerDefault": self.newer_default(&g.id, &gd, p)?,
                });
                if let Some(fp) = &st.defaults {
                    v["defaults"] = json!(fp);
                    v["k2Version"] = json!(d.set().k2_version);
                }
                if let Some(s) = &st.since {
                    v["since"] = json!(s);
                }
                if let Some(r) = &st.reason {
                    v["reason"] = json!(r);
                }
                Ok(v)
            };
            let theme = self.active_theme(Some(&g.id));
            let base = if super::builtin_theme(&theme.name).is_some() { theme.name.clone() } else { super::DEFAULT_THEME.to_string() };
            let offered = self.keep_previous_offered(&g.id)?;
            let (own, own_err) = match self.garden_own_keys(&g.id) {
                Ok(k) => (json!(k), J::Null),
                Err(e) => (J::Null, json!(e)),
            };
            rows.push(json!({
                "id": g.id,
                "name": g.name,
                "index": i + 1,
                "page": part(Part::Page, &gd.page_state, &gd.page)?,
                "theme": part(Part::Theme, &gd.theme_state, &gd.theme)?,
                "ownChanges": own,
                "ownChangesError": own_err,
                "undo": entry.undo.as_ref().map(|u| {
                    let parts: Vec<&str> = [Part::Page, Part::Theme]
                        .into_iter()
                        .filter(|p| GardenSync::undo_part(u, *p).is_some())
                        .map(Part::name)
                        .collect();
                    json!({ "parts": parts, "at": u.at })
                }),
                "keepPrevious": {
                    "page": offered.contains(&Part::Page),
                    "theme": offered.contains(&Part::Theme),
                },
                "themeName": theme.name,
                "themeLabel": super::theme_label(&theme.name),
                "themeBuiltin": super::builtin_theme(&theme.name).is_some(),
                "themeBase": { "name": base, "label": super::theme_label(&base) },
                "gardenTheme": active.gardens.get(&g.id),
                "damaged": gd.damaged.iter().map(|(f, m)| json!({ "defaults": f, "message": m })).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({
            "ok": true,
            "liveDefaults": self.live_defaults().fingerprint(),
            "k2Version": self.live_defaults().set().k2_version,
            "previousDefaults": sf.previous_defaults,
            "migratedAt": sf.migrated_at,
            "gardens": rows,
        }))
    }

    // ── hooks from the store ─────────────────────────────────────────

    /// GS28: "Start with the default" and `reset --garden` (to the stub)
    /// set page sync on, with undo. A page already synced is left alone.
    pub fn sync_follow_k2(&self, id: &str) -> Result<bool, ZenError> {
        self.change_entry(id, false, |entry, _| {
            Ok(if entry.part(Part::Page).is_copy() { vec![(Part::Page, PartState::synced_because("turned-on"))] } else { vec![] })
        })
    }

    /// GS27: a deleted Garden's state goes into its `deleted.json` record
    /// and its mirror, then leaves `sync.json`.
    pub fn sync_on_delete(&self, id: &str, record: &mut J) -> Result<(), ZenError> {
        self.sync_boot();
        let _g = sync_lock();
        let mut sf = self.sync_state_unlocked();
        let entry = sf.gardens.remove(id).unwrap_or_default();
        record["sync"] = serde_json::to_value(&entry).map_err(|e| ZenError::Io(e.to_string()))?;
        self.write_mirror(id, &entry)?;
        if self.sync_path().exists() {
            self.write_sync(&sf)?;
        }
        Ok(())
    }

    // ── doctor ───────────────────────────────────────────────────────

    /// `k2 zen doctor` lines for sync: `sync state`, `defaults archive`,
    /// `frame floor`.
    pub fn sync_doctor_checks(&self) -> Vec<J> {
        let check = |name: &str, ok: bool, detail: String| json!({ "name": name, "ok": ok, "detail": detail });
        let mut out = Vec::new();
        if !self.is_set_up() {
            return out;
        }
        let sf_disk = fs::read_to_string(self.sync_path()).ok().map(|t| serde_json::from_str::<SyncFile>(&t));
        let sf = self.sync_state();
        let mut problems = Vec::new();
        match &sf_disk {
            None => problems.push("sync.json is missing (every Garden syncs)".to_string()),
            Some(Err(e)) => problems.push(format!("sync.json doesn't parse: {e}")),
            Some(Ok(_)) => {}
        }
        if let Some(r) = &sf.rebuilt {
            problems.push(format!(
                "sync.json was rebuilt from {r}; check each Garden's switches in Settings → Gardens"
            ));
        }
        for g in self.gardens() {
            if let (Some(e), Some(m)) = (sf.gardens.get(&g.id), self.read_mirror(&g.id)) {
                if e.page != m.page || e.theme != m.theme {
                    problems.push(format!("{} ({}): its mirror disagrees with sync.json", g.name, g.id));
                }
            }
        }
        let copies = sf.gardens.values().filter(|e| e.part(Part::Page).is_copy() || e.part(Part::Theme).is_copy()).count();
        out.push(check(
            "sync state",
            problems.is_empty(),
            if problems.is_empty() {
                format!(
                    "{} Garden(s), {copies} with an own copy; K2's defaults {}",
                    self.gardens().len(),
                    sf.live_defaults.as_deref().unwrap_or("not archived yet")
                )
            } else {
                problems.join("; ")
            },
        ));
        let sets = self.archived_sets();
        let bytes: u64 = sets.iter().map(|(_, _, b)| *b).sum();
        let live = self.live_defaults();
        let mut damaged = Vec::new();
        let mut below = Vec::new();
        for (fp, s, _) in &sets {
            match s {
                Err(e) => damaged.push(format!("{fp}: {e}")),
                Ok(set) => {
                    if let Err(e) = Defaults::from_set(set.clone()) {
                        damaged.push(format!("{fp}: {e}"));
                    }
                    for b in frame_below_floor(&set.frame) {
                        below.push(format!("{fp}: {b}"));
                    }
                }
            }
        }
        let live_archived = sets.iter().any(|(fp, s, _)| fp == live.fingerprint() && s.is_ok());
        out.push(check(
            "defaults archive",
            damaged.is_empty() && live_archived,
            format!(
                "{} set(s), {} KB; live {}{}{}",
                sets.len(),
                bytes.div_ceil(1024),
                live.fingerprint(),
                if live_archived { "" } else { " (not archived yet)" },
                if damaged.is_empty() { String::new() } else { format!("; damaged: {}", damaged.join("; ")) }
            ),
        ));
        out.push(check(
            "frame floor",
            below.is_empty(),
            if below.is_empty() {
                "no set asks for a looser control check than K2's floor".into()
            } else {
                format!("clamped to the floor: {}", below.join("; "))
            },
        ));
        out
    }
}

/// The parts of a `/cli/zen/get` answer a part covers: the page answer
/// (and the frame) for the page; theme, chrome and motion for the theme.
pub fn part_view(v: &J, p: Part) -> J {
    match p {
        Part::Page => json!({ "page": v["page"], "frame": v["frame"] }),
        Part::Theme => json!({ "theme": v["theme"], "chrome": v["chrome"], "motion": v["motion"] }),
    }
}

fn read_set(path: &Path, fp: &str) -> Result<DefaultsSet, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("can't read it: {e}"))?;
    let set: DefaultsSet = serde_json::from_str(&text).map_err(|e| format!("it doesn't parse: {e}"))?;
    if set.fingerprint != fp {
        return Err(format!("it says it is {}, its name says {fp}", set.fingerprint));
    }
    let content_fp = set.compute_fingerprint()?;
    if content_fp != fp {
        return Err(format!("its content's fingerprint is {content_fp}"));
    }
    Ok(set)
}

/// GS46: under a test (`cfg(test)`) or in a test daemon
/// (`K2_TEST_AGENT_SHIM_DIR` set), the folder must never be the real
/// home's `~/.k2/zen`. Panics loudly when it is.
pub fn guard_real_home(root: &Path) {
    let test_mode = cfg!(test) || std::env::var_os(crate::terminal::agent_spawn_guard::SHIM_DIR_ENV).is_some();
    guard_real_home_with(root, test_mode, real_home().as_deref());
}

/// [`guard_real_home`] with its inputs explicit (a test checks the rule
/// without touching the process environment).
pub fn guard_real_home_with(root: &Path, test_mode: bool, real_home: Option<&Path>) {
    if !test_mode {
        return;
    }
    let Some(real) = real_home else { return };
    let real_zen = real.join(".k2").join("zen");
    let canon = |p: &Path| -> PathBuf {
        let parent = p.parent().and_then(|d| fs::canonicalize(d).ok());
        match (parent, p.file_name()) {
            (Some(d), Some(n)) => d.join(n),
            _ => p.to_path_buf(),
        }
    };
    if root.starts_with(&real_zen) || canon(root).starts_with(canon(&real_zen)) {
        panic!(
            "zen: a test reached the real {} — point HOME at a temp dir (prd-zen-garden-sync-defaults-v1 GS46)",
            real_zen.display()
        );
    }
}

/// The home the password database lists for this uid (never `$HOME`).
#[cfg(unix)]
fn real_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    let uid = unsafe { libc::getuid() };
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_string_lossy().into_owned();
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
}

#[cfg(not(unix))]
fn real_home() -> Option<PathBuf> {
    None
}
