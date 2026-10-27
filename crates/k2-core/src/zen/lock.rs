//! Garden Lock (prd-garden-lock-and-sync-v1 §5, with Rosson's rulings of
//! 2026-10-13).
//!
//! A locked Garden can't be changed by agents. The person still can, in
//! the app (rename, theme, reset, Start with the default, delete), and
//! K2's template updates still reach it while "Keep in sync" is on (the
//! file only names its template; the defaults resolve at read time).
//!
//! - **State.** `locked` (+ `lockedAt`) on the Garden's `sync.json` entry
//!   and its history mirror: daemon-written, never watched. Missing means
//!   unlocked, so every Garden from before Lock stays as it was.
//! - **Frozen copy.** `.locks/<id>/garden.toml` is the Garden's live text
//!   when it was locked; `.locks/<id>/widgets/<name>/` is the live bundle
//!   of each widget folder its page places (the bundle the person saw).
//!   Themes and `zen.toml` are NOT frozen (ruling 4): a change to a theme
//!   the Garden uses reaches it, as intended. History keeps only
//!   [`super::store::HISTORY_KEEP`] snapshots, so the frozen copy lives
//!   here, never in `.history/`.
//! - **File edits.** The daemon can't tell who saved a file, so it decides
//!   from the lock alone: [`ZenFiles::enforce_locks`] (first thing in every
//!   `refresh`) moves an edited Garden file to
//!   `.history/gardens/<id>.toml/refused/<utc>.toml` (nothing is lost),
//!   writes the locked text back and reports it for the audit line. A
//!   locked Garden's live text is always the frozen copy
//!   (`ZenFiles::read_current`), so an edit never shows, even before the
//!   watcher fires. The person unlocks first to hand-edit.
//! - **Who.** Only the person flips it: `POST /cli/zen/garden/lock`, owner
//!   token and no passport. An agent's change to a locked Garden is 423
//!   `garden_locked` (the daemon's routes check [`ZenFiles::refuse_if_locked`]).
//!
//! Known limit (§10): an agent running as the same OS user can still read
//! the owner token or edit `sync.json`. Lock stops normal agent work.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::json;

use super::builtin_widgets::{parse_widget_ref, WidgetRef};
use super::store::{garden_locked_message, valid_garden_id, GardenEntry, ZenError, ZenFile, ZenFiles, GARDENS_DIR, HISTORY_DIR};
use super::sync::{now, sync_lock};
use super::widget_store::{read_saved_bundle, write_saved_bundle, WidgetState, WidgetStatus};

/// The daemon-owned folder of frozen copies (never watched).
pub const LOCKS_DIR: &str = ".locks";
/// A locked Garden's frozen page text, in `.locks/<id>/`.
pub const LOCKED_GARDEN_FILE: &str = "garden.toml";
/// A locked Garden's frozen widget bundles, in `.locks/<id>/`.
pub const LOCKED_WIDGETS_DIR: &str = "widgets";
/// `{garden, at, widgets}`, in `.locks/<id>/`.
pub const LOCK_RECORD: &str = "lock.json";
/// Refused edits, in `.history/gardens/<id>.toml/` (a folder, so they never
/// count as snapshots and never become a Garden's live version).
pub const REFUSED_DIR: &str = "refused";
/// Where a deleted Garden's lock folder goes, next to its snapshots.
pub const DELETED_LOCK_DIR: &str = "lock";

/// One edit `enforce_locks` put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LockRefusal {
    pub garden: String,
    pub name: String,
    /// Where the edited file was kept, relative to the Zen folder (`None`
    /// when the file had been deleted).
    pub kept: Option<String>,
}

/// What `garden/lock` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockChange {
    pub garden: String,
    pub name: String,
    pub locked: bool,
    pub changed: bool,
    /// Widget folders frozen with the Garden (locking only).
    pub widgets: Vec<String>,
}

fn io(e: std::io::Error, what: &std::path::Path) -> ZenError {
    ZenError::Io(format!("{}: {e}", what.display()))
}

impl ZenFiles {
    /// `~/.k2/zen/.locks/`.
    pub fn locks_dir(&self) -> PathBuf {
        self.root().join(LOCKS_DIR)
    }

    /// `~/.k2/zen/.locks/<id>/`.
    pub fn lock_dir(&self, id: &str) -> PathBuf {
        self.locks_dir().join(id)
    }

    /// Whether Garden `id` is locked: its `sync.json` entry, else (a Garden
    /// in the list with no entry) its history mirror. Reads one small file;
    /// never runs the sync loader.
    pub fn is_locked(&self, id: &str) -> bool {
        if !valid_garden_id(id) {
            return false;
        }
        if let Some(e) = self.read_sync_file().and_then(|sf| sf.gardens.get(id).cloned()) {
            return e.locked;
        }
        self.read_mirror(id).is_some_and(|m| m.locked) && self.gardens().iter().any(|g| g.id == id)
    }

    /// A locked Garden's frozen text; `None` when it isn't locked (or its
    /// copy is missing, which the next `refresh` repairs).
    pub(crate) fn frozen_text_if_locked(&self, id: &str) -> Option<String> {
        if !self.is_locked(id) {
            return None;
        }
        fs::read_to_string(self.lock_dir(id).join(LOCKED_GARDEN_FILE)).ok()
    }

    /// 423 `garden_locked` when Garden `sel` (id or name) is locked. A
    /// Garden that isn't in the list passes (the route answers its own 404).
    pub fn refuse_if_locked(&self, sel: &str) -> Result<(), ZenError> {
        match self.find_garden(sel) {
            Some((_, g)) if self.is_locked(&g.id) => Err(ZenError::GardenLocked { garden: g.id, name: g.name }),
            _ => Ok(()),
        }
    }

    /// The message an agent gets for Garden `id` (§6).
    pub fn locked_message(&self, id: &str) -> String {
        let name = self.find_garden(id).map(|(_, g)| g.name).unwrap_or_else(|| id.to_string());
        garden_locked_message(&name)
    }

    /// A Garden's live text on disk, lock aside: the file when it checks
    /// clean, else its newest clean snapshot, else its stub ("Locking takes
    /// a copy of the current live version", §8).
    fn live_text_on_disk(&self, id: &str) -> String {
        let f = ZenFile::Garden(id.to_string());
        let base = self.base_for(&f);
        if let Some(Ok(t)) = self.read_disk(&f) {
            if self.check_src(&f, &t, &base).is_clean() {
                return t;
            }
        }
        for s in self.snapshots(&f) {
            if let Some(t) = self.snapshot_text(&f, &s.name) {
                if self.check_src(&f, &t, &base).is_clean() {
                    return t;
                }
            }
        }
        self.stub(&f)
    }

    /// Write `.locks/<id>/`: the Garden's live text from disk plus the live
    /// bundle of every widget folder its page places. Returns the widgets.
    fn freeze(&self, id: &str) -> Result<Vec<String>, ZenError> {
        let text = self.live_text_on_disk(id);
        let dir = self.lock_dir(id);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let gfile = dir.join(LOCKED_GARDEN_FILE);
        crate::fs_atomic::atomic_write_str(&gfile, &text).map_err(|e| io(e, &gfile))?;
        let wdir = dir.join(LOCKED_WIDGETS_DIR);
        match fs::remove_dir_all(&wdir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e, &wdir)),
        }
        let f = ZenFile::Garden(id.to_string());
        let checked = self.check_src(&f, &text, &self.base_for(&f));
        let page = super::garden_page(&checked.layer, &self.default_template(id));
        let mut frozen: Vec<String> = Vec::new();
        for w in page["widgets"].as_array().into_iter().flatten() {
            if w["kind"] != super::schema::CUSTOM_KIND {
                continue;
            }
            let Ok(WidgetRef::User(name)) = parse_widget_ref(w["widget"].as_str().unwrap_or_default()) else { continue };
            if frozen.contains(&name) {
                continue;
            }
            // A folder with no good bundle at all has nothing to freeze: it
            // stays the live folder's, so a fix still reaches it.
            let st = self.widget_status(&name);
            if let (Some(m), Some(b)) = (&st.manifest, &st.bundle) {
                write_saved_bundle(&wdir.join(&name), m, b)?;
                frozen.push(name);
            }
        }
        let rec = dir.join(LOCK_RECORD);
        let body = serde_json::to_string_pretty(&json!({ "garden": id, "at": now(), "widgets": frozen }))
            .map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&rec, &(body + "\n")).map_err(|e| io(e, &rec))?;
        Ok(frozen)
    }

    /// POST `/cli/zen/garden/lock` (§5.2): lock or unlock Garden `garden`.
    /// Locking freezes its live version first, so the flag never points at
    /// nothing; unlocking removes the frozen copy (the file on disk already
    /// holds the same text). The person only: the route checks the caller.
    pub fn set_garden_lock(&self, garden: &str, locked: bool) -> Result<LockChange, ZenError> {
        let (_, g) = self.garden(garden)?;
        self.sync_boot();
        let mut widgets = Vec::new();
        if locked {
            let has_copy = self.lock_dir(&g.id).join(LOCKED_GARDEN_FILE).is_file();
            if !(self.is_locked(&g.id) && has_copy) {
                widgets = self.freeze(&g.id)?;
            }
        }
        let changed = {
            let _l = sync_lock();
            let mut sf = self.sync_state_unlocked();
            let mut entry = sf.garden(&g.id);
            if entry.locked == locked {
                false
            } else {
                entry.locked = locked;
                entry.locked_at = locked.then(now);
                self.write_mirror(&g.id, &entry)?;
                sf.gardens.insert(g.id.clone(), entry);
                self.write_sync(&sf)?;
                true
            }
        };
        if !locked {
            let dir = self.lock_dir(&g.id);
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e, &dir)),
            }
        }
        Ok(LockChange { garden: g.id, name: g.name, locked, changed, widgets })
    }

    /// After the person wrote a locked Garden's file (reset, Start with the
    /// default): the lock now holds the new text and widgets.
    pub(crate) fn lock_follow_owner_write(&self, id: &str) -> Result<(), ZenError> {
        if self.is_locked(id) {
            self.freeze(id)?;
        }
        Ok(())
    }

    /// `garden/delete`: the lock folder travels into the Garden's history
    /// folder with its page and mirror (the mirror keeps `locked`).
    pub(crate) fn lock_on_delete(&self, id: &str) -> Result<(), ZenError> {
        let src = self.lock_dir(id);
        if !src.is_dir() {
            return Ok(());
        }
        let parent = self.history_dir(&ZenFile::Garden(id.to_string()));
        fs::create_dir_all(&parent).map_err(|e| io(e, &parent))?;
        let dst = parent.join(DELETED_LOCK_DIR);
        if dst.exists() {
            fs::remove_dir_all(&dst).map_err(|e| io(e, &dst))?;
        }
        fs::rename(&src, &dst).map_err(|e| io(e, &dst))
    }

    /// §5.4: every locked Garden's file on disk is its frozen text. An edit
    /// is moved to `.history/gardens/<id>.toml/refused/<utc>.toml` and the
    /// locked text written back. Returns what it put back (one per Garden).
    /// Never fails the refresh: a Garden it can't fix is logged and skipped.
    pub fn enforce_locks(&self) -> Vec<LockRefusal> {
        let mut out = Vec::new();
        for g in self.gardens() {
            if !self.is_locked(&g.id) {
                continue;
            }
            match self.enforce_one(&g) {
                Ok(Some(r)) => {
                    crate::log_debug!(
                        "[zen/lock] {} ({}) is locked; put its text back (edit kept at {})",
                        g.name,
                        g.id,
                        r.kept.as_deref().unwrap_or("nowhere: the file was deleted")
                    );
                    out.push(r);
                }
                Ok(None) => {}
                Err(e) => crate::log_debug!("[zen/lock] {} ({}): {e}", g.name, g.id),
            }
        }
        out
    }

    fn enforce_one(&self, g: &GardenEntry) -> Result<Option<LockRefusal>, ZenError> {
        let f = ZenFile::Garden(g.id.clone());
        let frozen_path = self.lock_dir(&g.id).join(LOCKED_GARDEN_FILE);
        let frozen = match fs::read_to_string(&frozen_path) {
            Ok(t) => t,
            Err(_) => {
                // Locked with no frozen copy (the folder was removed by
                // hand): the lock holds from the live text now.
                self.freeze(&g.id)?;
                fs::read_to_string(&frozen_path).map_err(|e| io(e, &frozen_path))?
            }
        };
        let path = self.path_of(&f);
        match fs::read(&path) {
            Ok(bytes) if bytes == frozen.as_bytes() => return Ok(None),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e, &path)),
        }
        let kept = if fs::symlink_metadata(&path).is_ok() {
            let dir = self.history_dir(&f).join(REFUSED_DIR);
            fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            let mut seq = 0u32;
            let dst = loop {
                let p = dir.join(format!("{stamp}-{seq:03}.toml"));
                if !p.exists() {
                    break p;
                }
                seq += 1;
            };
            fs::rename(&path, &dst).map_err(|e| io(e, &dst))?;
            let name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            Some(format!("{HISTORY_DIR}/{}/{REFUSED_DIR}/{name}", f.label()))
        } else {
            None
        };
        let gdir = self.root().join(GARDENS_DIR);
        fs::create_dir_all(&gdir).map_err(|e| io(e, &gdir))?;
        crate::fs_atomic::atomic_write_str(&path, &frozen).map_err(|e| io(e, &path))?;
        Ok(Some(LockRefusal { garden: g.id.clone(), name: g.name.clone(), kept }))
    }

    /// The edits `enforce_locks` kept for Garden `id`, newest first (file
    /// names in `.history/gardens/<id>.toml/refused/`).
    pub fn refused_edits(&self, id: &str) -> Vec<String> {
        let dir = self.history_dir(&ZenFile::Garden(id.to_string())).join(REFUSED_DIR);
        let mut out: Vec<String> = fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        out.sort();
        out.reverse();
        out
    }

    /// A locked Garden's frozen copy of widget folder `widget` (`None` for
    /// an unlocked Garden, a built-in, or a widget it didn't freeze).
    ///
    /// The copy is checked every time it is read (0.46.5 S0): its version
    /// is recomputed from the saved document AND the saved manifest. A copy
    /// that doesn't check out (an edit on disk to its caps, reads or
    /// libraries, say) is `broken` here: the Garden doesn't run it, and
    /// doesn't fall back to the live folder either.
    pub fn frozen_widget(&self, garden: &str, widget: &str) -> Option<WidgetStatus> {
        let Ok(WidgetRef::User(name)) = parse_widget_ref(widget) else { return None };
        if !self.is_locked(garden) {
            return None;
        }
        let dir = self.lock_dir(garden).join(LOCKED_WIDGETS_DIR).join(&name);
        let Some((m, b, at)) = read_saved_bundle(&dir) else {
            if !dir.join(super::widget_store::LAST_GOOD_JSON).exists() {
                return None;
            }
            return Some(WidgetStatus {
                widget: widget.to_string(),
                exists: true,
                builtin: false,
                manifest: None,
                bundle: None,
                state: WidgetState::Broken,
                errors: vec![super::schema::Diagnostic {
                    suggestion: Some("the person unlocks the Garden and locks it again to freeze the folder's current version".into()),
                    file: super::widget_store::widget_label(&name),
                    line: 1,
                    col: 1,
                    message: format!(
                        "this Garden's locked copy of {name} was changed on disk, so it doesn't run; unlock the Garden and lock it again"
                    ),
                }],
                warnings: Vec::new(),
                last_good_at: None,
                assets: None,
            });
        };
        Some(WidgetStatus {
            widget: widget.to_string(),
            exists: true,
            builtin: false,
            manifest: Some(m),
            bundle: Some(b),
            state: WidgetState::Ok,
            errors: Vec::new(),
            warnings: Vec::new(),
            last_good_at: Some(at),
            assets: None,
        })
    }

    /// The widget as Garden `garden` (an id) runs it: its frozen copy when
    /// the Garden is locked, else the folder's live state.
    pub fn garden_widget_status(&self, garden: &str, widget: &str) -> WidgetStatus {
        self.frozen_widget(garden, widget).unwrap_or_else(|| self.widget_status(widget))
    }

    /// GET `/cli/zen/widget/bundle?widget=&garden=`: a locked Garden's
    /// frozen bundle, else the live one ([`ZenFiles::widget_bundle`]).
    pub fn widget_bundle_in(&self, garden: Option<&str>, widget: &str) -> Result<WidgetStatus, ZenError> {
        if let Some((_, g)) = garden.and_then(|g| self.find_garden(g)) {
            if let Some(st) = self.frozen_widget(&g.id, widget) {
                return Ok(st);
            }
        }
        self.widget_bundle(widget)
    }
}

impl ZenFiles {
    /// The source names this computer's locked Gardens read (contract-
    /// sources-v1 §4.1 step 8, GV16): every custom placement on a locked
    /// Garden whose widget (its frozen copy) names sources, by its
    /// manifest's fixed names or its placement config. An address counts
    /// when it's bare (`fleet`, this computer) or names `this_host`
    /// (`fleet::<this host>`); other servers' sources are theirs to guard
    /// (pinning, GV65). Bare names come back.
    pub fn locked_placement_sources_for(&self, this_host: Option<&str>) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let this_host = this_host.map(str::to_ascii_lowercase);
        for g in self.gardens() {
            if !self.is_locked(&g.id) {
                continue;
            }
            let f = ZenFile::Garden(g.id.clone());
            let eff = self.effective(&f, &self.base_for(&f));
            let page = super::garden_page(&eff.layer, &g.template);
            for w in page["widgets"].as_array().into_iter().flatten() {
                if w["kind"] != super::schema::CUSTOM_KIND {
                    continue;
                }
                let widget = w["widget"].as_str().unwrap_or_default();
                let st = self.garden_widget_status(&g.id, widget);
                let sources = st.manifest.as_ref().and_then(|m| m.sources.as_ref());
                let config = w.get("props").and_then(|p| p.get("config")).cloned().unwrap_or_else(|| json!({}));
                for a in super::widgets::placement_source_addresses(sources, &config) {
                    let Ok(addr) = crate::sources::Address::parse(&a) else { continue };
                    match (&addr.host, &this_host) {
                        (None, _) => {
                            out.insert(addr.name);
                        }
                        (Some(h), Some(me)) if h == me => {
                            out.insert(addr.name);
                        }
                        _ => {}
                    }
                }
            }
        }
        out
    }
}

/// This computer's Connect name (`<sub>.k2.dev`), when it has one.
fn this_host() -> Option<String> {
    let cfg = crate::tunnel::config::load().ok()?;
    let sub = cfg.subdomain.trim().to_ascii_lowercase();
    (!sub.is_empty()).then(|| format!("{sub}.{}", crate::tunnel::config::SUBDOMAIN_HOST))
}

/// Garden v3 data sources (contract-sources-v1 §4.1 step 8, GV16): the
/// source names that locked Gardens on this computer read (bare `name`, or
/// `name::<this host>`). An agent's edit or remove of one of them is 423
/// `garden_locked`. Empty when Zen was never set up here.
pub fn locked_placement_sources() -> std::collections::BTreeSet<String> {
    let f = ZenFiles::local();
    if !f.exists() {
        return std::collections::BTreeSet::new();
    }
    f.locked_placement_sources_for(this_host().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (PathBuf, ZenFiles) {
        let dir = std::env::temp_dir().join(format!("k2-zen-lock-unit-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        let f = ZenFiles::new(dir.join("zen"));
        f.setup().expect("setup");
        (dir, f)
    }

    #[test]
    fn a_new_folder_has_every_garden_unlocked() {
        let (dir, f) = temp();
        for g in f.gardens() {
            assert!(!f.is_locked(&g.id), "{} starts unlocked", g.name);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn lock_then_unlock_round_trips_and_cleans_up() {
        let (dir, f) = temp();
        let id = f.gardens()[0].id.clone();
        let out = f.set_garden_lock(&id, true).expect("lock");
        assert!(out.changed && out.locked, "{out:?}");
        assert!(f.is_locked(&id));
        assert!(f.lock_dir(&id).join(LOCKED_GARDEN_FILE).is_file(), "frozen copy written");
        let again = f.set_garden_lock(&id, true).expect("lock again");
        assert!(!again.changed, "locking a locked Garden changes nothing: {again:?}");
        let off = f.set_garden_lock(&id, false).expect("unlock");
        assert!(off.changed && !off.locked);
        assert!(!f.is_locked(&id));
        assert!(!f.lock_dir(&id).exists(), "unlock removes the frozen copy");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refused_edits_are_not_snapshots() {
        let (dir, f) = temp();
        let id = f.gardens()[0].id.clone();
        f.set_garden_lock(&id, true).expect("lock");
        let gf = ZenFile::Garden(id.clone());
        let before = f.snapshots(&gf).len();
        fs::write(f.path_of(&gf), "schema = 1\n[[widget]]\nid = \"x\"\n").expect("edit");
        let r = f.enforce_locks();
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(f.snapshots(&gf).len(), before, "a refused edit never becomes a snapshot");
        assert_eq!(f.refused_edits(&id).len(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}
