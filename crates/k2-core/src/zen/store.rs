//! The `~/.k2/zen/` folder (prd-zen-mode-v1 Z8, Z11, Z13, Z14).
//!
//! Everything here is a pure function of what is on disk, so a restart
//! changes nothing: the live ("last good") version of a file is the file
//! itself when it validates clean, else its newest clean `.history/`
//! snapshot, else the default. [`ZenFiles::refresh`] is the only call that
//! writes snapshots: every clean file whose content differs from its newest
//! snapshot is copied in, and each file keeps [`HISTORY_KEEP`].
//!
//! Every function takes the folder from [`ZenFiles::root`], so tests run on
//! a temp folder and never touch the real `~/.k2/zen`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::schema::{self, Checked, Diagnostic, FileKind, Layer};

/// Snapshots kept per file (Z14).
pub const HISTORY_KEEP: usize = 20;
pub const ZEN_FILE: &str = "zen.toml";
pub const PAGES_DIR: &str = "pages";
pub const HISTORY_DIR: &str = ".history";
pub const HOMES_FILE: &str = "homes.json";
/// Reserved for v2 widget grants. v1 never reads or writes it.
pub const GRANTS_FILE: &str = "grants.json";
/// User theme bundles: `themes/<name>/theme.toml` plus an optional image.
pub const THEMES_DIR: &str = "themes";
pub const THEME_FILE: &str = "theme.toml";
/// The active theme, globally and per Home. Daemon-written (`k2 zen theme
/// set|next|prev`), never hand-edited, never watched.
pub const ACTIVE_FILE: &str = "active.json";
/// Where the last good background image of each theme is kept.
pub const BACKGROUND_HISTORY: &str = "background";

/// One of the three kinds of user-editable Zen file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ZenFile {
    Zen,
    Page(String),
    /// `themes/<name>/theme.toml`.
    Theme(String),
}

impl ZenFile {
    /// `zen.toml` or `pages/<id>.toml`: the name errors and history use.
    pub fn label(&self) -> String {
        match self {
            ZenFile::Zen => ZEN_FILE.to_string(),
            ZenFile::Page(id) => format!("{PAGES_DIR}/{id}.toml"),
            ZenFile::Theme(name) => format!("{THEMES_DIR}/{name}/{THEME_FILE}"),
        }
    }

    pub fn kind(&self) -> FileKind {
        match self {
            ZenFile::Zen => FileKind::Zen,
            ZenFile::Page(_) => FileKind::Page,
            ZenFile::Theme(_) => FileKind::Theme,
        }
    }

    /// Accepts `zen.toml` (or `zen`), `pages/<id>.toml` and
    /// `themes/<name>/theme.toml` (or `themes/<name>`). Anything else
    /// (`grants.json`, `homes.json`, `active.json`, `../x`) is refused: no
    /// route may name a file the daemon owns.
    pub fn parse(s: &str) -> Result<ZenFile, ZenError> {
        let t = s.trim();
        if t == ZEN_FILE || t == "zen" {
            return Ok(ZenFile::Zen);
        }
        if let Some(id) = t
            .strip_prefix("pages/")
            .and_then(|r| r.strip_suffix(".toml"))
        {
            if valid_home_id(id) {
                return Ok(ZenFile::Page(id.to_string()));
            }
        }
        if let Some(rest) = t.strip_prefix("themes/") {
            let name = rest.strip_suffix(&format!("/{THEME_FILE}")).unwrap_or(rest);
            if super::valid_theme_name(name) {
                return Ok(ZenFile::Theme(name.to_string()));
            }
        }
        Err(ZenError::BadRequest(format!(
            "'{t}' is not a Zen file you can name; use zen.toml, pages/<home-id>.toml or themes/<name>/theme.toml"
        )))
    }
}

/// A Home id: what `createHome` makes (a UUID) or any short slug.
pub fn valid_home_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn clean_name(name: &str) -> Result<String, ZenError> {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if n.is_empty() {
        return Err(ZenError::BadRequest("a Home needs a name".into()));
    }
    if n.chars().count() > 200 {
        return Err(ZenError::BadRequest("a Home name must be 200 characters or fewer".into()));
    }
    Ok(n)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZenError {
    /// The folder doesn't exist: Zen was never turned on on this computer.
    NotSetUp,
    BadRequest(String),
    NotFound(String),
    /// A theme name that is neither built in nor in `themes/`.
    UnknownTheme { name: String, known: Vec<String> },
    /// `theme new` onto a theme that already has a user file.
    Conflict(String),
    Io(String),
}

impl std::fmt::Display for ZenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZenError::NotSetUp => f.write_str(NOT_SET_UP),
            ZenError::UnknownTheme { name, known } => write!(
                f,
                "no theme '{name}' on this computer; themes: {}. Make one with k2 zen theme new {name}",
                known.join(", ")
            ),
            ZenError::BadRequest(m) | ZenError::NotFound(m) | ZenError::Conflict(m) | ZenError::Io(m) => {
                f.write_str(m)
            }
        }
    }
}

/// Z17's sentence, shared by the routes and the CLI.
pub const NOT_SET_UP: &str =
    "Zen isn't set up on this computer. Turn it on from Home in the K2 app.";

fn io(e: std::io::Error, what: &Path) -> ZenError {
    ZenError::Io(format!("{}: {e}", what.display()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomesFile {
    version: u32,
    homes: Vec<HomeEntry>,
}

/// Where a file's live version came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The file on disk (clean).
    Current,
    /// A `.history/` snapshot (the file on disk has errors).
    Snapshot(String),
    /// Nothing usable: the default (builtin theme / bare template).
    Default,
}

impl Origin {
    fn as_json(&self) -> J {
        match self {
            Origin::Current => json!("file"),
            Origin::Snapshot(n) => json!(format!("snapshot:{n}")),
            Origin::Default => json!("default"),
        }
    }
}

/// A file's live version plus what is wrong with the file on disk now.
#[derive(Debug, Clone)]
pub struct Effective {
    pub layer: Layer,
    pub origin: Origin,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// When the live version was last good (RFC 3339), or None for default.
    pub at: Option<String>,
    /// `(line, col)` of each key of the live version.
    pub positions: BTreeMap<String, (usize, usize)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub at: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnsureOutcome {
    #[serde(rename = "createdFolder")]
    pub created_folder: bool,
    #[serde(rename = "createdZen")]
    pub created_zen: bool,
    #[serde(rename = "createdPage")]
    pub created_page: bool,
    pub file: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResetOutcome {
    pub file: String,
    /// The snapshot name restored, or `"default"`.
    pub restored: String,
    /// Where the pre-reset file was kept (`None` when there was no file).
    pub snapshot: Option<String>,
}

/// The folder. Cheap to build; holds only the root path.
#[derive(Debug, Clone)]
pub struct ZenFiles {
    root: PathBuf,
}

fn mtime_rfc3339(p: &Path) -> Option<String> {
    let t = fs::metadata(p).ok()?.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// `20261004T120000123Z-000.toml` → `2026-10-04T12:00:00.123Z`.
fn snapshot_at(name: &str) -> String {
    let stem = name.split('-').next().unwrap_or(name);
    chrono::NaiveDateTime::parse_from_str(stem.trim_end_matches('Z'), "%Y%m%dT%H%M%S%3f")
        .map(|dt| {
            dt.and_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_default()
}

fn valid_snapshot_name(name: &str) -> bool {
    name.ends_with(".toml")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

impl ZenFiles {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ZenFiles { root: root.into() }
    }

    /// This computer's `~/.k2/zen`.
    pub fn local() -> Self {
        ZenFiles::new(super::zen_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    fn require(&self) -> Result<(), ZenError> {
        if self.exists() {
            Ok(())
        } else {
            Err(ZenError::NotSetUp)
        }
    }

    pub fn pages_dir(&self) -> PathBuf {
        self.root.join(PAGES_DIR)
    }

    pub fn history_root(&self) -> PathBuf {
        self.root.join(HISTORY_DIR)
    }

    pub fn path_of(&self, f: &ZenFile) -> PathBuf {
        self.root.join(f.label())
    }

    fn history_dir(&self, f: &ZenFile) -> PathBuf {
        self.history_root().join(f.label())
    }

    /// Ids of `pages/*.toml` with a valid id, sorted.
    pub fn page_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.pages_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let id = name.strip_suffix(".toml")?.to_string();
                        (valid_home_id(&id) && e.path().is_file()).then_some(id)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    pub fn read_homes(&self) -> Vec<HomeEntry> {
        fs::read_to_string(self.root.join(HOMES_FILE))
            .ok()
            .and_then(|s| serde_json::from_str::<HomesFile>(&s).ok())
            .map(|h| h.homes)
            .unwrap_or_default()
    }

    fn write_homes(&self, homes: &[HomeEntry]) -> Result<(), ZenError> {
        let path = self.root.join(HOMES_FILE);
        let body = serde_json::to_string_pretty(&HomesFile { version: 1, homes: homes.to_vec() })
            .map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| io(e, &path))
    }

    /// A Home id from an id or a name (`k2 zen reset --home Work`).
    pub fn find_home(&self, id_or_name: &str) -> Option<String> {
        let want = id_or_name.trim();
        let homes = self.read_homes();
        if let Some(h) = homes.iter().find(|h| h.id == want) {
            return Some(h.id.clone());
        }
        if let Some(h) = homes.iter().find(|h| h.name.eq_ignore_ascii_case(want)) {
            return Some(h.id.clone());
        }
        (valid_home_id(want) && self.path_of(&ZenFile::Page(want.to_string())).is_file())
            .then(|| want.to_string())
    }

    /// The stub for a file: an empty set of changes for `zen.toml`, the
    /// template line plus an empty `[theme]` for a page, and a copy of the
    /// default theme for a user theme.
    pub fn stub(&self, f: &ZenFile) -> String {
        match f {
            ZenFile::Zen => super::DEFAULT_ZEN_TOML.to_string(),
            ZenFile::Theme(name) => theme_starter(name, &super::BUILTIN_THEMES[0]),
            ZenFile::Page(id) => {
                let name = self
                    .read_homes()
                    .into_iter()
                    .find(|h| &h.id == id)
                    .map(|h| h.name)
                    .unwrap_or_else(|| id.clone());
                page_stub(id, &name)
            }
        }
    }

    /// Z11: create the folder, `zen.toml` and `pages/<id>.toml` if missing.
    /// Never overwrites a file. Records the Home's name in `homes.json`.
    pub fn ensure_page(&self, id: &str, name: &str) -> Result<EnsureOutcome, ZenError> {
        if !valid_home_id(id) {
            return Err(ZenError::BadRequest(format!(
                "'{id}' is not a Home id (letters, digits, - and _, up to 64)"
            )));
        }
        let name = clean_name(name)?;
        let created_folder = !self.exists();
        fs::create_dir_all(self.pages_dir()).map_err(|e| io(e, &self.pages_dir()))?;
        let zen = self.path_of(&ZenFile::Zen);
        let created_zen = !zen.exists();
        if created_zen {
            crate::fs_atomic::atomic_write_str(&zen, super::DEFAULT_ZEN_TOML)
                .map_err(|e| io(e, &zen))?;
        }
        let mut homes = self.read_homes();
        match homes.iter_mut().find(|h| h.id == id) {
            Some(h) if h.name == name => {}
            Some(h) => {
                h.name = name.clone();
                self.write_homes(&homes)?;
            }
            None => {
                homes.push(HomeEntry { id: id.to_string(), name: name.clone() });
                self.write_homes(&homes)?;
            }
        }
        let f = ZenFile::Page(id.to_string());
        let page = self.path_of(&f);
        let created_page = !page.exists();
        if created_page {
            crate::fs_atomic::atomic_write_str(&page, &page_stub(id, &name))
                .map_err(|e| io(e, &page))?;
        }
        Ok(EnsureOutcome { created_folder, created_zen, created_page, file: f.label() })
    }

    /// Z11: replace `homes.json` with the renderer's list. A page whose Home
    /// is gone moves into `.history/` (restorable with reset). An empty list
    /// archives nothing: K2 never deletes the last Home, so it is a bug.
    /// Returns `(written, archived ids)`; nothing is written when Zen was
    /// never set up.
    pub fn sync_homes(&self, homes: Vec<HomeEntry>) -> Result<(bool, Vec<String>), ZenError> {
        let mut clean: Vec<HomeEntry> = Vec::new();
        for h in homes {
            if !valid_home_id(&h.id) {
                return Err(ZenError::BadRequest(format!("'{}' is not a Home id", h.id)));
            }
            let name = clean_name(&h.name)?;
            if !clean.iter().any(|c| c.id == h.id) {
                clean.push(HomeEntry { id: h.id, name });
            }
        }
        if !self.exists() {
            return Ok((false, Vec::new()));
        }
        self.write_homes(&clean)?;
        let mut archived = Vec::new();
        if clean.is_empty() {
            return Ok((true, archived));
        }
        // A deleted Home's own theme pick goes with it.
        let mut active = self.read_active();
        let before = active.homes.len();
        active.homes.retain(|id, _| clean.iter().any(|h| &h.id == id));
        if active.homes.len() != before {
            self.write_active(&active)?;
        }
        for id in self.page_ids() {
            if clean.iter().any(|h| h.id == id) {
                continue;
            }
            let f = ZenFile::Page(id.clone());
            let path = self.path_of(&f);
            let text = fs::read_to_string(&path).map_err(|e| io(e, &path))?;
            self.write_snapshot(&f, &text)?;
            fs::remove_file(&path).map_err(|e| io(e, &path))?;
            self.prune(&f);
            archived.push(id);
        }
        Ok((true, archived))
    }

    fn read_current(&self, f: &ZenFile) -> Option<Result<String, String>> {
        let p = self.path_of(f);
        match fs::read(&p) {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(Err(format!("can't read the file: {e}"))),
        }
    }

    fn check_text(&self, f: &ZenFile, text: &Result<String, String>, base: &Layer) -> Checked {
        match text {
            Ok(t) => schema::check(&f.label(), t, f.kind(), base),
            Err(msg) => Checked {
                errors: vec![Diagnostic { file: f.label(), line: 1, col: 1, message: msg.clone() }],
                ..Checked::default()
            },
        }
    }

    /// Snapshots of `f`, newest first.
    pub fn snapshots(&self, f: &ZenFile) -> Vec<SnapshotInfo> {
        let mut out: Vec<SnapshotInfo> = fs::read_dir(self.history_dir(f))
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if !valid_snapshot_name(&name) || !e.path().is_file() {
                            return None;
                        }
                        let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                        Some(SnapshotInfo { at: snapshot_at(&name), name, bytes })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.name.cmp(&a.name));
        out
    }

    fn snapshot_text(&self, f: &ZenFile, name: &str) -> Option<String> {
        if !valid_snapshot_name(name) {
            return None;
        }
        fs::read_to_string(self.history_dir(f).join(name)).ok()
    }

    fn write_snapshot(&self, f: &ZenFile, text: &str) -> Result<String, ZenError> {
        let dir = self.history_dir(f);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let newest = self.snapshots(f).into_iter().next().map(|s| s.name);
        let mut seq = 0u32;
        loop {
            let name = format!("{stamp}-{seq:03}.toml");
            // Keep names strictly increasing even if the clock steps back.
            let after_newest = newest.as_deref().map_or(true, |n| name.as_str() > n);
            let path = dir.join(&name);
            if after_newest && !path.exists() {
                crate::fs_atomic::atomic_write_str(&path, text).map_err(|e| io(e, &path))?;
                return Ok(name);
            }
            seq += 1;
            if seq > 999 {
                // Clock behind the newest snapshot: continue after it.
                let base = newest.as_deref().unwrap_or("0").trim_end_matches(".toml").to_string();
                let name = format!("{base}x.toml");
                let path = dir.join(&name);
                crate::fs_atomic::atomic_write_str(&path, text).map_err(|e| io(e, &path))?;
                return Ok(name);
            }
        }
    }

    fn prune(&self, f: &ZenFile) {
        let dir = self.history_dir(f);
        for s in self.snapshots(f).into_iter().skip(HISTORY_KEEP) {
            let _ = fs::remove_file(dir.join(&s.name));
        }
    }

    /// The live version of `f` over `base` (last-good semantics, Z13).
    pub fn effective(&self, f: &ZenFile, base: &Layer) -> Effective {
        let Some(text) = self.read_current(f) else {
            return Effective {
                layer: Layer::new(),
                origin: Origin::Default,
                errors: Vec::new(),
                warnings: Vec::new(),
                at: None,
                positions: BTreeMap::new(),
            };
        };
        let checked = self.check_text(f, &text, base);
        if checked.is_clean() {
            return Effective {
                layer: checked.layer,
                origin: Origin::Current,
                errors: Vec::new(),
                warnings: checked.warnings,
                at: mtime_rfc3339(&self.path_of(f)),
                positions: checked.positions,
            };
        }
        for snap in self.snapshots(f) {
            let Some(t) = self.snapshot_text(f, &snap.name) else { continue };
            let c = schema::check(&f.label(), &t, f.kind(), base);
            if c.is_clean() {
                return Effective {
                    layer: c.layer,
                    origin: Origin::Snapshot(snap.name.clone()),
                    errors: checked.errors,
                    warnings: checked.warnings,
                    at: Some(snap.at),
                    positions: c.positions,
                };
            }
        }
        Effective {
            layer: Layer::new(),
            origin: Origin::Default,
            errors: checked.errors,
            warnings: checked.warnings,
            at: None,
            positions: BTreeMap::new(),
        }
    }

    // ── themes (Omarchy additions 1–3) ─────────────────────────────────

    pub fn themes_dir(&self) -> PathBuf {
        self.root.join(THEMES_DIR)
    }

    pub fn theme_dir(&self, name: &str) -> PathBuf {
        self.themes_dir().join(name)
    }

    /// Folders under `themes/` with a valid name and a `theme.toml`, sorted.
    pub fn user_theme_names(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.themes_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        (super::valid_theme_name(&name) && e.path().join(THEME_FILE).is_file())
                            .then_some(name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Every theme in cycle order: K2's built-ins first (in K2's order),
    /// then the user's own themes by name. `user` is true when the user has
    /// a file for it (for a built-in: an override).
    pub fn themes(&self) -> Vec<ThemeInfo> {
        let user = self.user_theme_names();
        let mut out: Vec<ThemeInfo> = super::BUILTIN_THEMES
            .iter()
            .map(|t| ThemeInfo {
                name: t.name.to_string(),
                builtin: true,
                user: user.iter().any(|u| u == t.name),
                summary: Some(t.summary.to_string()),
            })
            .collect();
        for u in user {
            if super::builtin_theme(&u).is_none() {
                out.push(ThemeInfo { name: u, builtin: false, user: true, summary: None });
            }
        }
        out
    }

    fn theme_names(&self) -> Vec<String> {
        self.themes().into_iter().map(|t| t.name).collect()
    }

    pub fn theme_exists(&self, name: &str) -> bool {
        super::builtin_theme(name).is_some()
            || (super::valid_theme_name(name)
                && self.path_of(&ZenFile::Theme(name.to_string())).is_file())
    }

    fn unknown_theme(&self, name: &str) -> ZenError {
        ZenError::UnknownTheme { name: name.to_string(), known: self.theme_names() }
    }

    /// `active.json`, or the empty default (global `default`, no Home picks).
    pub fn read_active(&self) -> ActiveFile {
        fs::read_to_string(self.root.join(ACTIVE_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_active(&self, a: &ActiveFile) -> Result<(), ZenError> {
        let path = self.root.join(ACTIVE_FILE);
        let body = serde_json::to_string_pretty(a).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| io(e, &path))
    }

    /// The theme a Home shows (decision 8): its own pick, else the global
    /// one, else `default`. A pick whose theme is gone is reported in
    /// `missing` and skipped.
    pub fn active_theme(&self, home: Option<&str>) -> ActiveTheme {
        let a = self.read_active();
        let mut missing = None;
        let mut chain: Vec<(String, &'static str)> = Vec::new();
        if let Some(n) = home.and_then(|id| a.homes.get(id)) {
            chain.push((n.clone(), "home"));
        }
        if let Some(n) = &a.theme {
            chain.push((n.clone(), "global"));
        }
        for (name, scope) in chain {
            if self.theme_exists(&name) {
                return ActiveTheme { name, scope, missing };
            }
            missing.get_or_insert(name);
        }
        ActiveTheme { name: super::DEFAULT_THEME.to_string(), scope: "global", missing }
    }

    /// The built-in layer a theme sits on: its own built-in (over
    /// `default`), or `default` for a theme only the user has.
    fn theme_parent(name: &str) -> &'static Layer {
        super::builtin_theme_layer(name).unwrap_or_else(super::builtin_layer)
    }

    /// `(built-in parent, the user file's live version, parent + file)`.
    fn theme_stack(&self, name: &str) -> (&'static Layer, Effective, Layer) {
        let parent = Self::theme_parent(name);
        let eff = self.effective(&ZenFile::Theme(name.to_string()), parent);
        let base = schema::merge(&[parent, &eff.layer]);
        (parent, eff, base)
    }

    /// What a file is checked over: a theme over its built-in parent,
    /// `zen.toml` over the global theme, a page over its Home's theme plus
    /// `zen.toml`.
    fn base_for(&self, f: &ZenFile) -> Layer {
        match f {
            ZenFile::Theme(name) => Self::theme_parent(name).clone(),
            ZenFile::Zen => self.theme_stack(&self.active_theme(None).name).2,
            ZenFile::Page(id) => {
                let tbase = self.theme_stack(&self.active_theme(Some(id)).name).2;
                let zen = self.effective(&ZenFile::Zen, &tbase);
                schema::merge(&[&tbase, &zen.layer])
            }
        }
    }

    fn home_id(&self, home: &str) -> Result<String, ZenError> {
        self.find_home(home).ok_or_else(|| {
            ZenError::NotFound(format!("no Home '{home}' on this computer; list them with k2 zen pages"))
        })
    }

    /// `k2 zen theme set`: pick `name` globally, or for one Home. `name`
    /// None with a Home clears that Home's pick (it follows the global one
    /// again). An unknown name changes nothing.
    pub fn set_theme(&self, name: Option<&str>, home: Option<&str>) -> Result<ThemeSwitch, ZenError> {
        self.require()?;
        let home_id = home.map(|h| self.home_id(h)).transpose()?;
        if let Some(n) = name {
            if !self.theme_exists(n) {
                return Err(self.unknown_theme(n));
            }
        }
        let mut a = self.read_active();
        a.version = 1;
        match (&home_id, name) {
            (Some(id), Some(n)) => {
                a.homes.insert(id.clone(), n.to_string());
            }
            (Some(id), None) => {
                a.homes.remove(id);
            }
            (None, Some(n)) => a.theme = Some(n.to_string()),
            (None, None) => {
                return Err(ZenError::BadRequest(
                    "theme set needs a theme name, or a Home to clear (--home <name> --clear)".into(),
                ))
            }
        }
        self.write_active(&a)?;
        let now = self.active_theme(home_id.as_deref());
        Ok(ThemeSwitch { theme: now.name, scope: now.scope.to_string(), home: home_id })
    }

    /// `k2 zen theme next|prev`: step through [`ZenFiles::themes`] (wrapping)
    /// from what the Home (or the computer) shows now.
    pub fn cycle_theme(&self, step: i64, home: Option<&str>) -> Result<ThemeSwitch, ZenError> {
        self.require()?;
        let home_id = home.map(|h| self.home_id(h)).transpose()?;
        let names = self.theme_names();
        let current = self.active_theme(home_id.as_deref()).name;
        let n = names.len() as i64;
        let i = names.iter().position(|x| *x == current).unwrap_or(0) as i64;
        let next = names[(((i + step) % n + n) % n) as usize].clone();
        self.set_theme(Some(&next), home_id.as_deref())
    }

    /// `k2 zen theme new <name>`: start a user theme bundle from a copy of
    /// `from` (default: the built-in of the same name, else `default`).
    /// Never overwrites. Doesn't switch to it.
    pub fn new_theme(&self, name: &str, from: Option<&str>) -> Result<NewThemeOutcome, ZenError> {
        self.require()?;
        if !super::valid_theme_name(name) {
            return Err(ZenError::BadRequest(format!(
                "'{name}' is not a theme name: use lower-case letters, digits, - and _ (up to 40), starting with a letter or digit"
            )));
        }
        let f = ZenFile::Theme(name.to_string());
        let path = self.path_of(&f);
        if path.exists() {
            return Err(ZenError::Conflict(format!(
                "{} already exists; edit it, or start over with k2 zen reset --theme {name}",
                f.label()
            )));
        }
        let source = from.map(str::to_string).unwrap_or_else(|| {
            if super::builtin_theme(name).is_some() { name.to_string() } else { super::DEFAULT_THEME.to_string() }
        });
        let user_src = self.path_of(&ZenFile::Theme(source.clone()));
        let (text, image) = if super::valid_theme_name(&source) && user_src.is_file() {
            let text = fs::read_to_string(&user_src).map_err(|e| io(e, &user_src))?;
            let layer = schema::check(&f.label(), &text, FileKind::Theme, Self::theme_parent(&source)).layer;
            let image = layer
                .get("background.image")
                .and_then(J::as_str)
                .map(|n| (n.to_string(), self.theme_dir(&source).join(n)))
                .filter(|(_, p)| p.is_file());
            (text, image)
        } else if let Some(b) = super::builtin_theme(&source) {
            (theme_starter(name, b), None)
        } else {
            return Err(self.unknown_theme(&source));
        };
        let dir = self.theme_dir(name);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let mut copied = None;
        if let Some((file, src)) = image {
            let dst = dir.join(&file);
            fs::copy(&src, &dst).map_err(|e| io(e, &dst))?;
            copied = Some(file);
        }
        crate::fs_atomic::atomic_write_str(&path, &text).map_err(|e| io(e, &path))?;
        Ok(NewThemeOutcome {
            name: name.to_string(),
            file: f.label(),
            path: path.display().to_string(),
            from: source,
            copied_image: copied,
        })
    }

    /// The last good background kept for a theme: `(file, mime, bytes)`.
    fn last_good_background(&self, name: &str) -> Option<(String, &'static str, Vec<u8>)> {
        let dir = self.history_root().join(THEMES_DIR).join(name).join(BACKGROUND_HISTORY);
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).collect();
        entries.sort();
        entries.into_iter().find_map(|p| {
            let file = p.file_name()?.to_string_lossy().to_string();
            schema::image_ext(&file)?;
            let (mime, bytes) = load_background(&p).ok()?;
            Some((file, mime, bytes))
        })
    }

    /// Keep `bytes` as the theme's last good background (one copy).
    fn keep_background(&self, name: &str, file: &str, bytes: &[u8]) -> Result<(), ZenError> {
        if self.last_good_background(name).is_some_and(|(f, _, b)| f == file && b == bytes) {
            return Ok(());
        }
        let dir = self.history_root().join(THEMES_DIR).join(name).join(BACKGROUND_HISTORY);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let dst = dir.join(file);
        fs::write(&dst, bytes).map_err(|e| io(e, &dst))
    }

    /// The image a theme's live layer names, checked on disk: `None` when
    /// it names none, else the image or a diagnostic at the `image =` line.
    fn check_background(&self, name: &str, eff: &Effective) -> Option<Result<(String, &'static str, Vec<u8>), Diagnostic>> {
        let file = eff.layer.get("background.image").and_then(J::as_str)?.to_string();
        let path = self.theme_dir(name).join(&file);
        Some(load_background(&path).map(|(m, b)| (file.clone(), m, b)).map_err(|msg| {
            let (line, col) = eff.positions.get("background.image").copied().unwrap_or((1, 1));
            Diagnostic { file: ZenFile::Theme(name.to_string()).label(), line, col, message: msg }
        }))
    }

    /// The resolved `background` for the active theme, as a `data:` URL.
    /// A bad image reports its error and falls back to the theme's last good
    /// image (`lastGood: true`), else to no background.
    fn background_json(&self, name: &str, spec: &J, eff: &Effective, errors: &mut Vec<Diagnostic>) -> Option<J> {
        let (file, mime, bytes, last_good) = match self.check_background(name, eff)? {
            Ok((file, mime, bytes)) => (file, mime, bytes, false),
            Err(d) => {
                errors.push(d);
                let (file, mime, bytes) = self.last_good_background(name)?;
                (file, mime, bytes, true)
            }
        };
        use base64::Engine as _;
        Some(json!({
            "dataUrl": format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)),
            "mime": mime,
            "bytes": bytes.len(),
            "file": file,
            "fit": spec["fit"],
            "opacity": spec["opacity"],
            "lastGood": last_good,
        }))
    }

    fn themes_json(&self, active: &str) -> J {
        J::Array(
            self.themes()
                .into_iter()
                .map(|t| {
                    let mut v = serde_json::to_value(&t).unwrap_or_else(|e| panic!("ThemeInfo to JSON: {e}"));
                    v["active"] = json!(t.name == active);
                    v
                })
                .collect(),
        )
    }

    /// GET `/cli/zen/theme/list`.
    pub fn theme_list(&self, home: Option<&str>) -> Result<J, ZenError> {
        self.require()?;
        let home_id = home.map(|h| self.home_id(h)).transpose()?;
        let active = self.active_theme(home_id.as_deref());
        let a = self.read_active();
        Ok(json!({
            "ok": true,
            "active": active.name,
            "scope": active.scope,
            "missing": active.missing,
            "global": a.theme.clone().unwrap_or_else(|| super::DEFAULT_THEME.to_string()),
            "home": home_id,
            "homeTheme": home_id.as_deref().and_then(|id| a.homes.get(id).cloned()),
            "themes": self.themes_json(&active.name),
        }))
    }

    /// GET `/cli/zen/get` body (Z15). `home` None resolves the global theme
    /// and `zen.toml` over the template with no page override.
    pub fn resolve(&self, home: Option<&str>) -> Result<J, ZenError> {
        self.require()?;
        if let Some(id) = home {
            if !valid_home_id(id) {
                return Err(ZenError::BadRequest(format!("'{id}' is not a Home id")));
            }
        }
        let active = self.active_theme(home);
        let (parent, theme, tbase) = self.theme_stack(&active.name);
        let zen = self.effective(&ZenFile::Zen, &tbase);
        let base = schema::merge(&[&tbase, &zen.layer]);
        let page = home.map(|id| self.effective(&ZenFile::Page(id.to_string()), &base));
        let empty = Layer::new();
        let user = schema::merge(&[&theme.layer, &zen.layer, page.as_ref().map_or(&empty, |p| &p.layer)]);
        let rt = schema::resolve(parent, &user);

        let mut errors = theme.errors.clone();
        errors.extend(zen.errors.iter().cloned());
        let mut warnings = theme.warnings.clone();
        warnings.extend(zen.warnings.iter().cloned());
        if let Some(p) = &page {
            errors.extend(p.errors.iter().cloned());
            warnings.extend(p.warnings.iter().cloned());
        }
        if let Some(m) = &active.missing {
            warnings.push(Diagnostic {
                file: ACTIVE_FILE.to_string(),
                line: 1,
                col: 1,
                message: format!(
                    "theme '{m}' doesn't exist any more; showing '{}'. Pick one with k2 zen theme set <name>",
                    active.name
                ),
            });
        }
        let background = rt
            .background
            .as_ref()
            .and_then(|spec| self.background_json(&active.name, spec, &theme, &mut errors));
        let info = self.themes();
        let me = info.iter().find(|t| t.name == active.name);
        let mut theme_json = json!({
            "name": active.name,
            "builtin": me.is_some_and(|t| t.builtin),
            "user": me.is_some_and(|t| t.user),
            "scope": active.scope,
            "tokens": rt.tokens,
            "font": rt.font,
            "terminal": rt.terminal,
        });
        if let Some(bg) = background {
            theme_json["background"] = bg;
        }
        let versioned = json!({
            "page": super::texting_page(),
            "theme": theme_json,
            "chrome": rt.chrome,
            "motion": rt.motion,
        });
        let version = schema::fnv_hex(versioned.to_string().as_bytes());
        let last_good_at = [theme.at.clone(), zen.at.clone(), page.as_ref().and_then(|p| p.at.clone())]
            .into_iter()
            .flatten()
            .max();
        let mut sources = serde_json::Map::new();
        sources.insert(ZenFile::Theme(active.name.clone()).label(), theme.origin.as_json());
        sources.insert(ZenFile::Zen.label(), zen.origin.as_json());
        if let (Some(id), Some(p)) = (home, &page) {
            sources.insert(ZenFile::Page(id.to_string()).label(), p.origin.as_json());
        }
        Ok(json!({
            "ok": true,
            "schema": schema::SCHEMA_VERSION,
            "version": version,
            "home": home,
            "page": versioned["page"],
            "theme": versioned["theme"],
            "themes": self.themes_json(&active.name),
            "chrome": versioned["chrome"],
            "motion": versioned["motion"],
            "errors": errors,
            "warnings": warnings,
            "lastGoodAt": last_good_at,
            "sources": sources,
        }))
    }

    /// Every file `validate`/`refresh` looks at: `zen.toml`, each page and
    /// each user theme.
    fn all_files(&self) -> Vec<ZenFile> {
        let mut out = vec![ZenFile::Zen];
        out.extend(self.page_ids().into_iter().map(ZenFile::Page));
        out.extend(self.user_theme_names().into_iter().map(ZenFile::Theme));
        out
    }

    /// GET `/cli/zen/validate`: what is on disk now (a theme's background
    /// image included). Never changes the live version and never writes a
    /// snapshot.
    pub fn validate(&self, target: Option<&ZenFile>) -> Result<J, ZenError> {
        self.require()?;
        let files = match target {
            Some(f) => vec![f.clone()],
            None => self.all_files(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut checked = Vec::new();
        for f in &files {
            let Some(text) = self.read_current(f) else {
                if target.is_some() {
                    let hint = match f {
                        ZenFile::Theme(n) if super::builtin_theme(n).is_some() => format!(
                            " (the built-in theme '{n}' has no override; make one with k2 zen theme new {n})"
                        ),
                        _ => String::new(),
                    };
                    return Err(ZenError::NotFound(format!("{} doesn't exist{hint}", f.label())));
                }
                continue;
            };
            let c = self.check_text(f, &text, &self.base_for(f));
            if let (ZenFile::Theme(name), true) = (f, c.is_clean()) {
                let eff = Effective {
                    layer: c.layer.clone(),
                    origin: Origin::Current,
                    errors: Vec::new(),
                    warnings: Vec::new(),
                    at: None,
                    positions: c.positions.clone(),
                };
                if let Some(Err(d)) = self.check_background(name, &eff) {
                    errors.push(d);
                }
            }
            errors.extend(c.errors);
            warnings.extend(c.warnings);
            checked.push(f.label());
        }
        Ok(json!({
            "ok": errors.is_empty(),
            "errors": errors,
            "warnings": warnings,
            "files": checked,
        }))
    }

    /// Re-read the folder: snapshot every clean file with new content (Z14),
    /// keep each theme's last good background, and return a fingerprint of
    /// the live state (active theme included) plus its diagnostics. Two
    /// calls with nothing effectively changed return the same fingerprint,
    /// so a caller emits `zen_changed` only when it moves (Z12).
    pub fn refresh(&self) -> Result<String, ZenError> {
        self.require()?;
        let mut state = serde_json::Map::new();
        state.insert(
            ACTIVE_FILE.to_string(),
            serde_json::to_value(self.read_active()).map_err(|e| ZenError::Io(e.to_string()))?,
        );
        for name in self.user_theme_names() {
            let f = ZenFile::Theme(name.clone());
            let eff = self.refresh_one(&f, Self::theme_parent(&name))?;
            let mut st = effective_state(&eff);
            st["background"] = match self.check_background(&name, &eff) {
                None => J::Null,
                Some(Ok((file, _, bytes))) => {
                    self.keep_background(&name, &file, &bytes)?;
                    json!({ "file": file, "hash": schema::fnv_hex(&bytes) })
                }
                Some(Err(d)) => json!({ "error": d }),
            };
            state.insert(f.label(), st);
        }
        let zen_eff = self.refresh_one(&ZenFile::Zen, &self.base_for(&ZenFile::Zen))?;
        state.insert(ZenFile::Zen.label(), effective_state(&zen_eff));
        for id in self.page_ids() {
            let f = ZenFile::Page(id);
            let eff = self.refresh_one(&f, &self.base_for(&f))?;
            state.insert(f.label(), effective_state(&eff));
        }
        Ok(schema::fnv_hex(J::Object(state).to_string().as_bytes()))
    }

    fn refresh_one(&self, f: &ZenFile, base: &Layer) -> Result<Effective, ZenError> {
        if let Some(Ok(text)) = self.read_current(f) {
            if schema::check(&f.label(), &text, f.kind(), base).is_clean() {
                let newest = self.snapshots(f).into_iter().next();
                let same = newest
                    .as_ref()
                    .and_then(|s| self.snapshot_text(f, &s.name))
                    .is_some_and(|t| t == text);
                if !same {
                    self.write_snapshot(f, &text)?;
                    self.prune(f);
                }
            }
        }
        Ok(self.effective(f, base))
    }

    /// GET `/cli/zen/history`.
    pub fn history(&self, target: Option<&ZenFile>) -> Result<J, ZenError> {
        self.require()?;
        let mut files: Vec<ZenFile> = match target {
            Some(f) => vec![f.clone()],
            None => self.all_files(),
        };
        if target.is_none() {
            // Pages of deleted Homes and deleted themes live only in history.
            if let Ok(rd) = fs::read_dir(self.history_root().join(PAGES_DIR)) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if let Ok(f) = ZenFile::parse(&format!("{PAGES_DIR}/{name}")) {
                        if !files.contains(&f) {
                            files.push(f);
                        }
                    }
                }
            }
            if let Ok(rd) = fs::read_dir(self.history_root().join(THEMES_DIR)) {
                let mut names: Vec<String> =
                    rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
                names.sort();
                for name in names {
                    let f = ZenFile::Theme(name.clone());
                    if super::valid_theme_name(&name) && !files.contains(&f) && !self.snapshots(&f).is_empty() {
                        files.push(f);
                    }
                }
            }
        }
        let out: Vec<J> = files
            .iter()
            .map(|f| {
                let b = self.base_for(f);
                let snaps: Vec<J> = self
                    .snapshots(f)
                    .into_iter()
                    .map(|s| {
                        let clean = self
                            .snapshot_text(f, &s.name)
                            .map(|t| schema::check(&f.label(), &t, f.kind(), &b).is_clean())
                            .unwrap_or(false);
                        json!({ "name": s.name, "at": s.at, "bytes": s.bytes, "clean": clean })
                    })
                    .collect();
                json!({ "file": f.label(), "exists": self.path_of(f).is_file(), "snapshots": snaps })
            })
            .collect();
        Ok(json!({ "ok": true, "keep": HISTORY_KEEP, "files": out }))
    }

    /// POST `/cli/zen/reset` (Z14, Omarchy addition 3): keep the current
    /// file as a snapshot first, then restore snapshot `to`, or clear the
    /// user's changes when `to` is None: `zen.toml` and pages go back to
    /// their empty stubs, an override of a built-in theme is removed (the
    /// built-in shows again), and a theme only the user has goes back to a
    /// copy of `default`. Never touches `homes.json`, `active.json` or
    /// `grants.json`.
    pub fn reset(&self, f: &ZenFile, to: Option<&str>) -> Result<ResetOutcome, ZenError> {
        self.require()?;
        if let ZenFile::Theme(name) = f {
            if !self.theme_exists(name) && to.is_none() {
                return Err(self.unknown_theme(name));
            }
        }
        let (restored, text): (String, Option<String>) = match to {
            Some(name) => {
                let name = if name.ends_with(".toml") { name.to_string() } else { format!("{name}.toml") };
                let text = self.snapshot_text(f, &name).ok_or_else(|| {
                    ZenError::NotFound(format!(
                        "no snapshot '{name}' for {}; list them with k2 zen history",
                        f.label()
                    ))
                })?;
                (name, Some(text))
            }
            None => match f {
                ZenFile::Theme(n) if super::builtin_theme(n).is_some() => ("builtin".to_string(), None),
                _ => ("default".to_string(), Some(self.stub(f))),
            },
        };
        let path = self.path_of(f);
        let mut kept = None;
        if let Some(Ok(current)) = self.read_current(f) {
            let newest = self.snapshots(f).into_iter().next();
            match newest {
                Some(s) if self.snapshot_text(f, &s.name).as_deref() == Some(current.as_str()) => {
                    kept = Some(s.name)
                }
                _ => kept = Some(self.write_snapshot(f, &current)?),
            }
        }
        match text {
            Some(t) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| io(e, parent))?;
                }
                crate::fs_atomic::atomic_write_str(&path, &t).map_err(|e| io(e, &path))?;
            }
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e, &path)),
            },
        }
        self.prune(f);
        Ok(ResetOutcome { file: f.label(), restored, snapshot: kept })
    }

    /// `k2 zen doctor` checks that need only the folder.
    pub fn doctor_checks(&self) -> Vec<J> {
        let mut checks = Vec::new();
        let check = |name: &str, ok: bool, detail: String| json!({ "name": name, "ok": ok, "detail": detail });
        if !self.exists() {
            checks.push(check("folder", false, format!("{} doesn't exist. {NOT_SET_UP}", self.root.display())));
            return checks;
        }
        checks.push(check("folder", true, self.root.display().to_string()));
        let bad: Vec<String> = super::BUILTIN_THEMES
            .iter()
            .filter_map(|t| {
                let c = super::check_builtin_theme(t.name)?;
                (!c.is_clean()).then(|| format!("{}: {:?}", t.name, c.errors))
            })
            .collect();
        checks.push(check(
            "builtin themes",
            bad.is_empty(),
            if bad.is_empty() { format!("{} clean", super::BUILTIN_THEMES.len()) } else { bad.join("; ") },
        ));
        let active = self.active_theme(None);
        let themes = self.themes();
        checks.push(check(
            "themes",
            active.missing.is_none(),
            match &active.missing {
                None => format!(
                    "{} built in, {} yours; active: {}",
                    themes.iter().filter(|t| t.builtin).count(),
                    themes.iter().filter(|t| t.user).count(),
                    active.name
                ),
                Some(m) => format!("active theme '{m}' doesn't exist; showing '{}'", active.name),
            },
        ));
        if let Ok(v) = self.validate(None) {
            let errs = v["errors"].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = errs
                .iter()
                .filter_map(|e| serde_json::from_value::<Diagnostic>(e.clone()).ok())
                .map(|d| d.render())
                .collect();
            checks.push(check(
                "files validate",
                lines.is_empty(),
                if lines.is_empty() { format!("{} file(s) clean", v["files"].as_array().map_or(0, Vec::len)) } else { lines.join("; ") },
            ));
        }
        let homes = self.read_homes();
        let orphans: Vec<String> = self
            .page_ids()
            .into_iter()
            .filter(|id| !homes.iter().any(|h| &h.id == id))
            .collect();
        checks.push(check(
            "homes.json",
            orphans.is_empty(),
            if orphans.is_empty() {
                format!("{} Home(s)", homes.len())
            } else {
                format!("pages with no Home in homes.json: {}", orphans.join(", "))
            },
        ));
        let grants = self.root.join(GRANTS_FILE).exists();
        checks.push(check(
            "grants.json",
            true,
            if grants {
                "present and ignored: Zen v1 has no grantable widgets, and only the K2 app may write it".into()
            } else {
                "absent (Zen v1 grants nothing)".into()
            },
        ));
        let total: usize = self.all_files().iter().map(|f| self.snapshots(f).len()).sum();
        checks.push(check("history", true, format!("{total} snapshot(s), {HISTORY_KEEP} kept per file")));
        checks
    }
}

fn effective_state(e: &Effective) -> J {
    json!({ "layer": e.layer, "errors": e.errors, "warnings": e.warnings })
}

/// `active.json`: the global theme and each Home's own pick. Daemon-written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub homes: BTreeMap<String, String>,
}

/// The theme a Home shows and where the pick came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTheme {
    pub name: String,
    /// `"home"` (the Home's own pick) or `"global"`.
    pub scope: &'static str,
    /// A pick that names a theme that is gone (skipped).
    pub missing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeInfo {
    pub name: String,
    /// Built into K2 (read-only).
    pub builtin: bool,
    /// The user has `themes/<name>/theme.toml` (for a built-in: an override).
    pub user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeSwitch {
    /// The theme now shown (for `home`, or globally).
    pub theme: String,
    pub scope: String,
    pub home: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewThemeOutcome {
    pub name: String,
    pub file: String,
    pub path: String,
    pub from: String,
    #[serde(rename = "copiedImage")]
    pub copied_image: Option<String>,
}

/// Sniff a raster image's type from its first bytes.
fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Read and check a background image: a regular file (not a link) of at
/// most [`schema::MAX_BACKGROUND_BYTES`], whose bytes are the type its
/// extension says. Returns `(mime, bytes)` or the message for the user.
pub fn load_background(path: &Path) -> Result<(&'static str, Vec<u8>), String> {
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ext = schema::image_ext(&file)
        .ok_or_else(|| format!("background image '{file}' must be .png, .jpg, .jpeg, .webp or .gif"))?;
    let want = schema::BACKGROUND_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, m)| *m)
        .unwrap_or("image/png");
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("background image '{file}' isn't in this theme's folder"))
        }
        Err(e) => return Err(format!("can't read background image '{file}': {e}")),
    };
    if meta.file_type().is_symlink() {
        return Err(format!("background image '{file}' is a link; put the image itself in this theme's folder"));
    }
    if !meta.is_file() {
        return Err(format!("background image '{file}' is not a file"));
    }
    if meta.len() == 0 {
        return Err(format!("background image '{file}' is empty"));
    }
    if meta.len() > schema::MAX_BACKGROUND_BYTES {
        return Err(format!(
            "background image '{file}' is {} bytes; the limit is {} bytes (2 MB)",
            meta.len(),
            schema::MAX_BACKGROUND_BYTES
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("can't read background image '{file}': {e}"))?;
    if bytes.len() as u64 > schema::MAX_BACKGROUND_BYTES {
        return Err(format!("background image '{file}' grew past the {} byte limit", schema::MAX_BACKGROUND_BYTES));
    }
    match sniff_image(&bytes) {
        Some(m) if m == want => Ok((m, bytes)),
        Some(m) => Err(format!("background image '{file}' is really {m}, not {want}; fix its extension")),
        None => Err(format!("background image '{file}' is not a {want} image")),
    }
}

/// The text `theme new` writes (and `reset` restores for a theme only the
/// user has): the built-in's TOML under a header saying the folder is the
/// user's, plus a commented `[background]` example.
pub fn theme_starter(name: &str, from: &super::BuiltinTheme) -> String {
    let body: String = from
        .toml
        .lines()
        .skip_while(|l| l.starts_with('#') || l.trim().is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    let parent = if super::builtin_theme(name).is_some() { name } else { super::DEFAULT_THEME };
    format!(
        "# Your Zen theme \"{name}\", started from K2's \"{src}\" theme.\n\
         #\n\
         # This folder is yours: K2 never changes it, app updates included.\n\
         # It sits on top of K2's \"{parent}\" theme: delete a line and K2's value\n\
         # shows again. Put a background image next to this file and name it in\n\
         # [background] below. Check with `k2 zen validate`.\n\
         #\n\
         #   use it:   k2 zen theme set {name}\n\
         #   undo:     k2 zen reset --theme {name}\n\
         {body}\n\
         # [background]\n\
         # image = \"background.jpg\"  # .png .jpg .jpeg .webp .gif, up to 2 MB\n\
         # fit = \"cover\"             # cover | contain | tile | center\n\
         # opacity = 1               # 0 to 1\n",
        src = from.name,
    )
}

/// The stub `page/ensure` writes and `reset` restores for a page.
pub fn page_stub(id: &str, name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    format!(
        "# Zen page for the Home \"{name}\" (id {id}).\n\
         #\n\
         # Zen v1 uses the built-in texting template. Add a [theme], [colors.light],\n\
         # [colors.dark], [type], [shape], [chrome], [bezier] or [animation] table\n\
         # below to override ~/.k2/zen/zen.toml for this Home only.\n\
         # Check your edit with `k2 zen validate`.\n\
         schema = 1\n\
         template = \"{template}\"\n\
         \n\
         [theme]\n",
        template = schema::TEMPLATE_ID,
    )
}
