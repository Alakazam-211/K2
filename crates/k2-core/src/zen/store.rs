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

/// One of the two kinds of user-editable Zen file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ZenFile {
    Zen,
    Page(String),
}

impl ZenFile {
    /// `zen.toml` or `pages/<id>.toml`: the name errors and history use.
    pub fn label(&self) -> String {
        match self {
            ZenFile::Zen => ZEN_FILE.to_string(),
            ZenFile::Page(id) => format!("{PAGES_DIR}/{id}.toml"),
        }
    }

    pub fn kind(&self) -> FileKind {
        match self {
            ZenFile::Zen => FileKind::Zen,
            ZenFile::Page(_) => FileKind::Page,
        }
    }

    /// Accepts `zen.toml` (or `zen`) and `pages/<id>.toml`. Anything else
    /// (`grants.json`, `homes.json`, `../x`) is refused: no route may name
    /// a file the daemon owns.
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
        Err(ZenError::BadRequest(format!(
            "'{t}' is not a Zen file you can name; use zen.toml or pages/<home-id>.toml"
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
    Io(String),
}

impl std::fmt::Display for ZenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZenError::NotSetUp => f.write_str(NOT_SET_UP),
            ZenError::BadRequest(m) | ZenError::NotFound(m) | ZenError::Io(m) => f.write_str(m),
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

    /// The stub for a file: the default theme for `zen.toml`, the template
    /// line plus an empty `[theme]` for a page.
    pub fn stub(&self, f: &ZenFile) -> String {
        match f {
            ZenFile::Zen => super::DEFAULT_ZEN_TOML.to_string(),
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
                };
            }
        }
        Effective {
            layer: Layer::new(),
            origin: Origin::Default,
            errors: checked.errors,
            warnings: checked.warnings,
            at: None,
        }
    }

    /// `zen.toml`'s live version and the base pages stack on (builtin + it).
    pub fn zen_effective(&self) -> (Effective, Layer) {
        let builtin = super::builtin_layer();
        let eff = self.effective(&ZenFile::Zen, builtin);
        let base = schema::merge(&[builtin, &eff.layer]);
        (eff, base)
    }

    /// GET `/cli/zen/get` body (Z15). `home` None resolves `zen.toml` over
    /// the template with no page override.
    pub fn resolve(&self, home: Option<&str>) -> Result<J, ZenError> {
        self.require()?;
        if let Some(id) = home {
            if !valid_home_id(id) {
                return Err(ZenError::BadRequest(format!("'{id}' is not a Home id")));
            }
        }
        let builtin = super::builtin_layer();
        let (zen, base) = self.zen_effective();
        let page = home.map(|id| self.effective(&ZenFile::Page(id.to_string()), &base));
        let empty = Layer::new();
        let user = schema::merge(&[&zen.layer, page.as_ref().map_or(&empty, |p| &p.layer)]);
        let rt = schema::resolve(builtin, &user);
        let page_json = super::texting_page();
        let versioned = json!({
            "page": page_json,
            "theme": rt.theme,
            "chrome": rt.chrome,
            "motion": rt.motion,
        });
        let version = schema::fnv_hex(versioned.to_string().as_bytes());
        let mut errors = zen.errors.clone();
        let mut warnings = zen.warnings.clone();
        if let Some(p) = &page {
            errors.extend(p.errors.iter().cloned());
            warnings.extend(p.warnings.iter().cloned());
        }
        let last_good_at = [zen.at.clone(), page.as_ref().and_then(|p| p.at.clone())]
            .into_iter()
            .flatten()
            .max();
        let mut sources = serde_json::Map::new();
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
            "chrome": versioned["chrome"],
            "motion": versioned["motion"],
            "errors": errors,
            "warnings": warnings,
            "lastGoodAt": last_good_at,
            "sources": sources,
        }))
    }

    /// Every file `validate`/`refresh` looks at: `zen.toml` and each page.
    fn all_files(&self) -> Vec<ZenFile> {
        let mut out = vec![ZenFile::Zen];
        out.extend(self.page_ids().into_iter().map(ZenFile::Page));
        out
    }

    /// GET `/cli/zen/validate`: what is on disk now. Never changes the live
    /// version and never writes a snapshot.
    pub fn validate(&self, target: Option<&ZenFile>) -> Result<J, ZenError> {
        self.require()?;
        let files = match target {
            Some(f) => vec![f.clone()],
            None => self.all_files(),
        };
        let (_, base) = self.zen_effective();
        let builtin = super::builtin_layer();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut checked = Vec::new();
        for f in &files {
            let Some(text) = self.read_current(f) else {
                if target.is_some() {
                    return Err(ZenError::NotFound(format!("{} doesn't exist", f.label())));
                }
                continue;
            };
            let b = if *f == ZenFile::Zen { builtin } else { &base };
            let c = self.check_text(f, &text, b);
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

    /// Re-read the folder: snapshot every clean file with new content (Z14)
    /// and return a fingerprint of the live state plus its diagnostics. Two
    /// calls with nothing effectively changed return the same fingerprint,
    /// so a caller emits `zen_changed` only when it moves (Z12).
    pub fn refresh(&self) -> Result<String, ZenError> {
        self.require()?;
        let builtin = super::builtin_layer();
        let mut state = serde_json::Map::new();
        let zen_eff = self.refresh_one(&ZenFile::Zen, builtin)?;
        let base = schema::merge(&[builtin, &zen_eff.layer]);
        state.insert(ZenFile::Zen.label(), effective_state(&zen_eff));
        for id in self.page_ids() {
            let f = ZenFile::Page(id);
            let eff = self.refresh_one(&f, &base)?;
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
            // Pages of deleted Homes live only in history.
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
        }
        let (_, base) = self.zen_effective();
        let builtin = super::builtin_layer();
        let out: Vec<J> = files
            .iter()
            .map(|f| {
                let b = if *f == ZenFile::Zen { builtin } else { &base };
                let snaps: Vec<J> = self
                    .snapshots(f)
                    .into_iter()
                    .map(|s| {
                        let clean = self
                            .snapshot_text(f, &s.name)
                            .map(|t| schema::check(&f.label(), &t, f.kind(), b).is_clean())
                            .unwrap_or(false);
                        json!({ "name": s.name, "at": s.at, "bytes": s.bytes, "clean": clean })
                    })
                    .collect();
                json!({ "file": f.label(), "exists": self.path_of(f).is_file(), "snapshots": snaps })
            })
            .collect();
        Ok(json!({ "ok": true, "keep": HISTORY_KEEP, "files": out }))
    }

    /// POST `/cli/zen/reset` (Z14): keep the current file as a snapshot
    /// first, then restore snapshot `to`, or the stub when `to` is None.
    /// Never touches `homes.json` or `grants.json`.
    pub fn reset(&self, f: &ZenFile, to: Option<&str>) -> Result<ResetOutcome, ZenError> {
        self.require()?;
        let restored_text = match to {
            Some(name) => {
                let name = if name.ends_with(".toml") { name.to_string() } else { format!("{name}.toml") };
                let text = self.snapshot_text(f, &name).ok_or_else(|| {
                    ZenError::NotFound(format!(
                        "no snapshot '{name}' for {}; list them with k2 zen history",
                        f.label()
                    ))
                })?;
                (name, text)
            }
            None => ("default".to_string(), self.stub(f)),
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
        crate::fs_atomic::atomic_write_str(&path, &restored_text.1).map_err(|e| io(e, &path))?;
        self.prune(f);
        Ok(ResetOutcome { file: f.label(), restored: restored_text.0, snapshot: kept })
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
        let builtin = super::check_builtin();
        checks.push(check(
            "builtin theme",
            builtin.is_clean(),
            if builtin.is_clean() { "clean".into() } else { format!("{:?}", builtin.errors) },
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
