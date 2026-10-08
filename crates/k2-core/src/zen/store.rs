//! The `~/.k2/zen/` folder (prd-zen-mode-v1 Z8, Z11, Z13, Z14;
//! prd-zen-gardens-v1 G8–G18).
//!
//! Everything here is a pure function of what is on disk, so a restart
//! changes nothing: the live ("last good") version of a file is the file
//! itself when it validates clean, else its newest clean `.history/`
//! snapshot, else the default. [`ZenFiles::refresh`] is the only call that
//! writes snapshots: every clean file whose content differs from its newest
//! snapshot is copied in, and each file keeps [`HISTORY_KEEP`].
//!
//! Gardens: `gardens.json` is the list (ids, names, order, templates), and
//! each Garden's page is `gardens/<id>.toml`. Only the daemon writes the
//! list, through the `garden/*` routes. A deleted Garden's page moves into
//! `.history/gardens/<id>.toml/`; nothing here ever deletes a user file.
//!
//! Every function takes the folder from [`ZenFiles::root`], so tests run on
//! a temp folder and never touch the real `~/.k2/zen`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::defaults::Defaults;
use super::schema::{self, Checked, Diagnostic, FileKind, Layer};

/// Snapshots kept per file (Z14).
pub const HISTORY_KEEP: usize = 20;
pub const ZEN_FILE: &str = "zen.toml";
/// One page per Garden: `gardens/<id>.toml` (G10).
pub const GARDENS_DIR: &str = "gardens";
/// The Garden list (G9). Daemon-written, atomic, never watched.
pub const GARDENS_FILE: &str = "gardens.json";
pub const HISTORY_DIR: &str = ".history";
/// Never read or written: Zen v2 keeps widget grants as signed rows in the
/// daemon's database (`zen::grants`, UWB4). No route may name it.
pub const GRANTS_FILE: &str = "grants.json";
/// The fingerprint keys for widget folders and grants (UW12).
const WIDGETS_DIR_KEY: &str = "widgets/";
const GRANTS_KEY: &str = "grants";
/// User theme bundles: `themes/<name>/theme.toml` plus an optional image.
pub const THEMES_DIR: &str = "themes";
pub const THEME_FILE: &str = "theme.toml";
/// The active theme, globally and per Garden. Daemon-written (`k2 zen
/// theme set|next|prev`), never hand-edited, never watched.
pub const ACTIVE_FILE: &str = "active.json";
/// Where the last good background image of each theme is kept.
pub const BACKGROUND_HISTORY: &str = "background";
/// The record a deleted Garden leaves next to its snapshots.
pub const DELETED_RECORD: &str = "deleted.json";
/// The name of the first Garden on a new computer (Q5; Rosson 2026-10-04):
/// the texting page (`k2.texting@1`).
pub const DEFAULT_GARDEN_NAME: &str = "Garden 1";
/// The second Garden setup makes (Rosson 2026-10-04): empty
/// (`k2.blank@1`), for the user to ask their agents to build. Made once,
/// with Garden 1; deleting it never brings it back.
pub const SECOND_GARDEN_NAME: &str = "Garden 2";
/// Garden names are 1 to this many characters (G9).
pub const MAX_GARDEN_NAME: usize = 60;

/// Serialises every change to the Garden list (routes run on worker
/// threads; two `garden/new` at once must not lose one).
static LIST_LOCK: Mutex<()> = Mutex::new(());

fn list_lock() -> std::sync::MutexGuard<'static, ()> {
    LIST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// One of the three kinds of user-editable Zen file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ZenFile {
    Zen,
    /// `gardens/<id>.toml`.
    Garden(String),
    /// `themes/<name>/theme.toml`.
    Theme(String),
}

impl ZenFile {
    /// `zen.toml`, `gardens/<id>.toml` or `themes/<name>/theme.toml`: the
    /// name errors and history use.
    pub fn label(&self) -> String {
        match self {
            ZenFile::Zen => ZEN_FILE.to_string(),
            ZenFile::Garden(id) => format!("{GARDENS_DIR}/{id}.toml"),
            ZenFile::Theme(name) => format!("{THEMES_DIR}/{name}/{THEME_FILE}"),
        }
    }

    pub fn kind(&self) -> FileKind {
        match self {
            ZenFile::Zen => FileKind::Zen,
            ZenFile::Garden(_) => FileKind::Garden,
            ZenFile::Theme(_) => FileKind::Theme,
        }
    }

    /// Accepts `zen.toml` (or `zen`), `gardens/<id>.toml` and
    /// `themes/<name>/theme.toml` (or `themes/<name>`). Anything else
    /// (`grants.json`, `gardens.json`, `active.json`, `../x`) is refused: no
    /// route may name a file the daemon owns.
    pub fn parse(s: &str) -> Result<ZenFile, ZenError> {
        let t = s.trim();
        if t == ZEN_FILE || t == "zen" {
            return Ok(ZenFile::Zen);
        }
        if let Some(id) = t
            .strip_prefix("gardens/")
            .and_then(|r| r.strip_suffix(".toml"))
        {
            if valid_garden_id(id) {
                return Ok(ZenFile::Garden(id.to_string()));
            }
        }
        if t.starts_with("pages/") {
            return Err(ZenError::BadRequest(
                "Zen pages are Gardens now: gardens/<id>.toml (list them with k2 zen garden list)".into(),
            ));
        }
        if let Some(rest) = t.strip_prefix("themes/") {
            let name = rest.strip_suffix(&format!("/{THEME_FILE}")).unwrap_or(rest);
            if super::valid_theme_name(name) {
                return Ok(ZenFile::Theme(name.to_string()));
            }
        }
        Err(ZenError::BadRequest(format!(
            "'{t}' is not a Zen file you can name; use zen.toml, gardens/<id>.toml or themes/<name>/theme.toml"
        )))
    }
}

/// A Garden id (G9): `g-` + 8 hex for a new Garden, or any short slug.
/// Letters, digits, `-` and `_`, starting with a letter or digit, up to 64.
pub fn valid_garden_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A Garden name (G9): trimmed, control characters dropped, 1 to 60.
pub fn clean_garden_name(name: &str) -> Result<String, ZenError> {
    let n: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
    if n.is_empty() {
        return Err(ZenError::BadRequest("a Garden needs a name".into()));
    }
    if n.chars().count() > MAX_GARDEN_NAME {
        return Err(ZenError::BadRequest(format!(
            "a Garden name must be {MAX_GARDEN_NAME} characters or fewer"
        )));
    }
    Ok(n)
}

/// The template a `garden/new` body names: `blank` (default) or `texting`,
/// a Garden catalog `short` (its newest version, `diary`), or a full
/// template id (any shipped version, `k2.diary@1`).
pub fn template_choice(t: Option<&str>) -> Result<&'static str, ZenError> {
    match t.map(str::trim) {
        None | Some("") | Some("blank") | Some(schema::BLANK_TEMPLATE_ID) => Ok(schema::BLANK_TEMPLATE_ID),
        Some("texting") | Some(schema::TEMPLATE_ID) => Ok(schema::TEMPLATE_ID),
        Some(other) => {
            if let Some(e) = super::garden_catalog::current_entries().into_iter().find(|e| e.meta.short == other) {
                return Ok(e.template_id.as_str());
            }
            if let Some(id) = schema::template_ids().into_iter().find(|id| *id == other) {
                return Ok(id);
            }
            let shorts: Vec<String> =
                super::garden_catalog::template_list().into_iter().map(|t| t.short).collect();
            Err(ZenError::BadRequest(format!("unknown template '{other}'; use {}", shorts.join(", "))))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZenError {
    /// The folder or its Garden list doesn't exist: Zen was never turned on
    /// on this computer.
    NotSetUp,
    BadRequest(String),
    NotFound(String),
    /// A theme name that is neither built in nor in `themes/`.
    UnknownTheme { name: String, known: Vec<String> },
    /// A Garden id or name that isn't in the list (G12).
    UnknownGarden { garden: String, known: Vec<String> },
    /// `theme new` onto a theme that already has a user file.
    Conflict(String),
    /// `garden/new` or `garden/rename` onto a name in use (case aside).
    GardenExists(String),
    /// `garden/delete` of the only Garden.
    LastGarden,
    /// `garden/template` onto a Garden whose file sets things the template
    /// switch would replace (its own layout, widgets or theme tables), with
    /// no `force`. `keys` are the file's own top-level keys.
    GardenHasChanges { garden: String, keys: Vec<String> },
    /// No widget folder (or built-in) of that name (404 `unknown_widget`).
    UnknownWidget(String),
    /// `widget/new` onto a folder that exists (409 `widget_exists`).
    WidgetExists(String),
    /// A widget with errors and no last good bundle (409 `widget_broken`).
    WidgetBroken(String),
    /// A grant for a bundle that changed while the dialog was open (409
    /// `widget_changed`).
    WidgetChanged(String),
    /// A grant route called by anything but the owner token (403
    /// `owner_only`, UWB3).
    OwnerOnly(String),
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
            ZenError::UnknownGarden { garden, .. } => write!(
                f,
                "no Garden '{garden}' on this computer; list them with k2 zen garden list"
            ),
            ZenError::LastGarden => f.write_str(LAST_GARDEN),
            ZenError::GardenHasChanges { keys, .. } => write!(
                f,
                "this Garden's file has its own changes ({}); starting over replaces them (the file is kept in history). Pass force to go ahead",
                keys.join(", ")
            ),
            ZenError::BadRequest(m)
            | ZenError::NotFound(m)
            | ZenError::Conflict(m)
            | ZenError::GardenExists(m)
            | ZenError::UnknownWidget(m)
            | ZenError::WidgetExists(m)
            | ZenError::WidgetBroken(m)
            | ZenError::WidgetChanged(m)
            | ZenError::OwnerOnly(m)
            | ZenError::Io(m) => f.write_str(m),
        }
    }
}

/// G46's sentence, shared by the routes and the CLI.
pub const NOT_SET_UP: &str =
    "Zen isn't set up on this computer. Turn it on with the Zen toggle in the K2 app's top bar.";
/// G35's sentence for `garden/delete` of the only Garden.
pub const LAST_GARDEN: &str = "That's your last Garden.";

fn io(e: std::io::Error, what: &Path) -> ZenError {
    ZenError::Io(format!("{}: {e}", what.display()))
}

fn default_entry_template() -> String {
    schema::BLANK_TEMPLATE_ID.to_string()
}

/// What `garden/template` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateOutcome {
    /// The Garden's list entry after the switch.
    pub garden: GardenEntry,
    /// The full template id it is on now.
    pub template: &'static str,
    /// False when the Garden already was that template with nothing of its
    /// own (nothing written, nothing announced).
    pub changed: bool,
    /// The snapshot that holds the previous file (`.history/gardens/<id>.toml/`).
    pub snapshot: Option<String>,
    /// The file's own top-level keys that the switch replaced (`force`).
    pub replaced: Vec<String>,
}

/// One Garden in `gardens.json` (G9). Order is the list's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GardenEntry {
    pub id: String,
    pub name: String,
    /// The template the Garden was made with; its page file may name
    /// another (G10).
    #[serde(default = "default_entry_template")]
    pub template: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: String,
    /// The Home the Agents widget starts on (G27). Only `garden/new` sets it.
    #[serde(rename = "seedHome", default, skip_serializing_if = "Option::is_none")]
    pub seed_home: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GardensFile {
    version: u32,
    gardens: Vec<GardenEntry>,
}

/// Where the Garden list came from (for `doctor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSource {
    /// `gardens.json`.
    File,
    /// No `gardens.json`; rebuilt from `gardens/*.toml` (G9).
    Rebuilt,
    /// `gardens.json` doesn't parse; rebuilt from the files.
    Unreadable(String),
    /// Neither exists: not set up.
    Missing,
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

#[derive(Debug, Clone)]
pub struct SetupOutcome {
    pub created_folder: bool,
    pub created_zen: bool,
    /// The first Gardens were made (there was no Garden): Garden 1 and
    /// Garden 2. Answered as `createdDefault`.
    pub created_default: bool,
    pub gardens: Vec<GardenEntry>,
}

#[derive(Debug, Clone)]
pub struct DeleteOutcome {
    pub deleted: GardenEntry,
    /// The snapshot the page moved to (`None` when it had no file).
    pub snapshot: Option<String>,
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

    /// This computer's `~/.k2/zen`. Under a test (and in a test daemon)
    /// it panics when that is the real home's (GS46).
    pub fn local() -> Self {
        let root = super::zen_root();
        super::sync::guard_real_home(&root);
        ZenFiles::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    /// Set up (G14): the folder holds a Garden list, from `gardens.json` or
    /// rebuilt from `gardens/*.toml`. A folder without one (for example a
    /// leftover from before Gardens) is not set up; `setup` adds Garden 1
    /// and Garden 2.
    pub fn is_set_up(&self) -> bool {
        self.exists() && !self.gardens().is_empty()
    }

    fn require(&self) -> Result<(), ZenError> {
        if self.is_set_up() {
            Ok(())
        } else {
            Err(ZenError::NotSetUp)
        }
    }

    pub fn gardens_dir(&self) -> PathBuf {
        self.root.join(GARDENS_DIR)
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

    /// Ids of `gardens/*.toml` with a valid id, sorted.
    pub fn garden_file_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.gardens_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let id = name.strip_suffix(".toml")?.to_string();
                        (valid_garden_id(&id) && e.path().is_file()).then_some(id)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// The template a Garden file names on its `template` line, if any.
    fn file_template(&self, id: &str) -> Option<&'static str> {
        let text = fs::read_to_string(self.path_of(&ZenFile::Garden(id.to_string()))).ok()?;
        let v: toml::Value = toml::from_str(&text).ok()?;
        let t = v.get("template")?.as_str()?;
        schema::template_ids().into_iter().find(|x| *x == t)
    }

    /// The Garden list and where it came from (G9). A missing or unreadable
    /// `gardens.json` is rebuilt from `gardens/*.toml` (id order, name = id);
    /// nothing is written or deleted here.
    pub fn read_list(&self) -> (Vec<GardenEntry>, ListSource) {
        let path = self.root.join(GARDENS_FILE);
        let unreadable = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<GardensFile>(&text) {
                Ok(f) => {
                    let mut seen: Vec<String> = Vec::new();
                    let list: Vec<GardenEntry> = f
                        .gardens
                        .into_iter()
                        .filter(|g| valid_garden_id(&g.id))
                        .filter(|g| {
                            let fresh = !seen.contains(&g.id);
                            seen.push(g.id.clone());
                            fresh
                        })
                        .map(|mut g| {
                            if !schema::is_template_id(&g.template) {
                                g.template = schema::BLANK_TEMPLATE_ID.to_string();
                            }
                            g
                        })
                        .collect();
                    return (list, ListSource::File);
                }
                Err(e) => Some(format!("{GARDENS_FILE} doesn't parse: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(format!("can't read {GARDENS_FILE}: {e}")),
        };
        let ids = self.garden_file_ids();
        let rebuilt: Vec<GardenEntry> = ids
            .into_iter()
            .map(|id| GardenEntry {
                name: id.clone(),
                template: self.file_template(&id).unwrap_or(schema::BLANK_TEMPLATE_ID).to_string(),
                created_at: mtime_rfc3339(&self.path_of(&ZenFile::Garden(id.clone()))).unwrap_or_default(),
                seed_home: None,
                id,
            })
            .collect();
        let source = match (unreadable, rebuilt.is_empty()) {
            (Some(msg), _) => ListSource::Unreadable(msg),
            (None, false) => ListSource::Rebuilt,
            (None, true) => ListSource::Missing,
        };
        (rebuilt, source)
    }

    /// The Garden list, in order.
    pub fn gardens(&self) -> Vec<GardenEntry> {
        self.read_list().0
    }

    /// Write `gardens.json` atomically. An existing file that doesn't parse
    /// is moved into `.history/gardens.json/` first, never overwritten.
    fn write_list(&self, list: &[GardenEntry]) -> Result<(), ZenError> {
        let path = self.root.join(GARDENS_FILE);
        if let Ok(text) = fs::read_to_string(&path) {
            if serde_json::from_str::<GardensFile>(&text).is_err() {
                let dir = self.history_root().join(GARDENS_FILE);
                fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
                let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
                let dst = dir.join(format!("{stamp}-unreadable.json"));
                fs::rename(&path, &dst).map_err(|e| io(e, &dst))?;
            }
        }
        let body = serde_json::to_string_pretty(&GardensFile { version: 1, gardens: list.to_vec() })
            .map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| io(e, &path))
    }

    /// A Garden by id, else by name (case-insensitive, exact), with its
    /// 0-based position.
    pub fn find_garden(&self, id_or_name: &str) -> Option<(usize, GardenEntry)> {
        let want = id_or_name.trim();
        let list = self.gardens();
        if let Some(i) = list.iter().position(|g| g.id == want) {
            return Some((i, list[i].clone()));
        }
        let lower = want.to_lowercase();
        list.iter().position(|g| g.name.to_lowercase() == lower).map(|i| (i, list[i].clone()))
    }

    /// [`ZenFiles::find_garden`] or 404 `unknown_garden` (G12).
    pub fn garden(&self, id_or_name: &str) -> Result<(usize, GardenEntry), ZenError> {
        self.find_garden(id_or_name).ok_or_else(|| ZenError::UnknownGarden {
            garden: id_or_name.trim().to_string(),
            known: self.gardens().into_iter().map(|g| g.id).collect(),
        })
    }

    /// The file a `garden` selector names. `deleted` also accepts the id of
    /// a Garden that only lives in `.history/` (for `history`, G17).
    pub fn garden_file(&self, id_or_name: &str, deleted: bool) -> Result<ZenFile, ZenError> {
        match self.find_garden(id_or_name) {
            Some((_, g)) => Ok(ZenFile::Garden(g.id)),
            None => {
                let id = id_or_name.trim();
                let f = ZenFile::Garden(id.to_string());
                if deleted && valid_garden_id(id) && self.history_dir(&f).is_dir() {
                    Ok(f)
                } else {
                    Err(self.garden(id_or_name).err().unwrap_or(ZenError::NotFound(id.to_string())))
                }
            }
        }
    }

    /// The template a Garden resolves to when its file doesn't name one.
    pub fn default_template(&self, id: &str) -> String {
        self.gardens()
            .into_iter()
            .find(|g| g.id == id)
            .map(|g| g.template)
            .unwrap_or_else(default_entry_template)
    }

    fn name_taken(list: &[GardenEntry], name: &str, except: Option<&str>) -> bool {
        let lower = name.to_lowercase();
        list.iter().any(|g| Some(g.id.as_str()) != except && g.name.to_lowercase() == lower)
    }

    fn exists_message(name: &str) -> ZenError {
        ZenError::GardenExists(format!("You already have a Garden called \u{201c}{name}\u{201d}."))
    }

    /// A fresh `g-` + 8 hex id, unused by the list, the files and history.
    fn new_id(&self, list: &[GardenEntry]) -> String {
        loop {
            let hex = uuid::Uuid::new_v4().simple().to_string();
            let id = format!("g-{}", &hex[..8]);
            let f = ZenFile::Garden(id.clone());
            if !list.iter().any(|g| g.id == id) && !self.path_of(&f).exists() && !self.history_dir(&f).exists() {
                return id;
            }
        }
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// Write a Garden's stub page when it has no file. Never overwrites.
    fn write_stub_if_missing(&self, g: &GardenEntry) -> Result<(), ZenError> {
        let path = self.path_of(&ZenFile::Garden(g.id.clone()));
        if path.exists() {
            return Ok(());
        }
        let dir = self.gardens_dir();
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        crate::fs_atomic::atomic_write_str(&path, &garden_stub(&g.id, &g.name, &g.template)).map_err(|e| io(e, &path))
    }

    /// The stub for a file: an empty set of changes for `zen.toml`, the
    /// template line and comments for a Garden, and a copy of the default
    /// theme for a user theme.
    pub fn stub(&self, f: &ZenFile) -> String {
        match f {
            ZenFile::Zen => super::DEFAULT_ZEN_TOML.to_string(),
            ZenFile::Theme(name) => theme_starter(name, &super::BUILTIN_THEMES[0]),
            ZenFile::Garden(id) => {
                let (name, template) = self
                    .gardens()
                    .into_iter()
                    .find(|g| &g.id == id)
                    .map(|g| (g.name, g.template))
                    .unwrap_or_else(|| (id.clone(), default_entry_template()));
                garden_stub(id, &name, &template)
            }
        }
    }

    /// G14: create the folder, `zen.toml` and, when there is no Garden,
    /// `gardens.json` with **Garden 1** (`k2.texting@1`) and **Garden 2**
    /// (`k2.blank@1`, empty), each with its stub. A list rebuilt from files
    /// is written down. Idempotent; never overwrites a file. Only an empty
    /// list seeds, so a deleted Garden 2 never comes back.
    pub fn setup(&self) -> Result<SetupOutcome, ZenError> {
        let _g = list_lock();
        let created_folder = !self.exists();
        let dir = self.gardens_dir();
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let zen = self.path_of(&ZenFile::Zen);
        let created_zen = !zen.exists();
        if created_zen {
            crate::fs_atomic::atomic_write_str(&zen, super::DEFAULT_ZEN_TOML).map_err(|e| io(e, &zen))?;
        }
        let (mut list, source) = self.read_list();
        let mut created_default = false;
        if list.is_empty() {
            let at = Self::now();
            for (name, template) in [
                (DEFAULT_GARDEN_NAME, schema::TEMPLATE_ID),
                (SECOND_GARDEN_NAME, schema::BLANK_TEMPLATE_ID),
            ] {
                let g = GardenEntry {
                    id: self.new_id(&list),
                    name: name.to_string(),
                    template: template.to_string(),
                    created_at: at.clone(),
                    seed_home: None,
                };
                self.write_stub_if_missing(&g)?;
                list.push(g);
            }
            // Catalog Gardens marked `new_users` (the Diary, Rosson
            // 2026-10-08), after Gardens 1 and 2, in catalog order. Only
            // here, on a new computer's first list: never appended to an
            // existing list, so a deleted one never comes back.
            for e in super::garden_catalog::current_entries().into_iter().filter(|e| e.meta.new_users) {
                if Self::name_taken(&list, &e.meta.label, None) {
                    continue;
                }
                let g = GardenEntry {
                    id: self.new_id(&list),
                    name: e.meta.label.clone(),
                    template: e.template_id.clone(),
                    created_at: at.clone(),
                    seed_home: None,
                };
                self.write_stub_if_missing(&g)?;
                list.push(g);
            }
            self.write_list(&list)?;
            created_default = true;
            if let Err(e) = self.sync_after_setup() {
                crate::log_debug!("[zen/sync] after setup: {e}");
            }
        } else if source != ListSource::File {
            self.write_list(&list)?;
        }
        Ok(SetupOutcome { created_folder, created_zen, created_default, gardens: list })
    }

    /// G13 `garden/new`: a Garden named `name` on `template` (default
    /// `k2.blank@1`) at 1-based position `at` (default the end), with its
    /// stub page. Agents can't set Zen up: no list is 404 `zen_not_set_up`.
    pub fn new_garden(
        &self,
        name: &str,
        template: Option<&str>,
        seed_home: Option<&str>,
        at: Option<usize>,
    ) -> Result<GardenEntry, ZenError> {
        let _g = list_lock();
        self.require()?;
        let name = clean_garden_name(name)?;
        let template = template_choice(template)?;
        if let Some(h) = seed_home {
            if !valid_garden_id(h) {
                return Err(ZenError::BadRequest(format!("'{h}' is not a Home id")));
            }
        }
        let mut list = self.gardens();
        if Self::name_taken(&list, &name, None) {
            return Err(Self::exists_message(&name));
        }
        let pos = match at {
            None => list.len(),
            Some(n) if n >= 1 && n <= list.len() + 1 => n - 1,
            Some(n) => {
                return Err(ZenError::BadRequest(format!(
                    "position {n} is outside 1 to {}",
                    list.len() + 1
                )))
            }
        };
        let g = GardenEntry {
            id: self.new_id(&list),
            name,
            template: template.to_string(),
            created_at: Self::now(),
            seed_home: seed_home.map(str::to_string),
        };
        self.write_stub_if_missing(&g)?;
        list.insert(pos, g.clone());
        self.write_list(&list)?;
        if let Err(e) = self.news_mark_template_seen(&g.template) {
            crate::log_debug!("[zen/sync] news for {}: {e}", g.id);
        }
        Ok(g)
    }

    /// G13 `garden/rename`. Returns the entry and whether the name changed
    /// (the same name is a no-op that writes nothing). Ids are stable, so
    /// history and the page file are untouched (G17).
    pub fn rename_garden(&self, garden: &str, name: &str) -> Result<(GardenEntry, bool), ZenError> {
        let _g = list_lock();
        self.require()?;
        let (i, mut g) = self.garden(garden)?;
        let name = clean_garden_name(name)?;
        if g.name == name {
            return Ok((g, false));
        }
        let mut list = self.gardens();
        if Self::name_taken(&list, &name, Some(&g.id)) {
            return Err(Self::exists_message(&name));
        }
        g.name = name;
        list[i] = g.clone();
        self.write_list(&list)?;
        Ok((g, true))
    }

    /// G13 `garden/reorder`: move a Garden to 1-based position `to`.
    /// Returns the list and whether it moved.
    pub fn reorder_garden(&self, garden: &str, to: i64) -> Result<(Vec<GardenEntry>, bool), ZenError> {
        let _g = list_lock();
        self.require()?;
        let (i, g) = self.garden(garden)?;
        let mut list = self.gardens();
        if to < 1 || to as usize > list.len() {
            return Err(ZenError::BadRequest(format!("position {to} is outside 1 to {}", list.len())));
        }
        let to = to as usize - 1;
        if to == i {
            return Ok((list, false));
        }
        list.remove(i);
        list.insert(to, g);
        self.write_list(&list)?;
        Ok((list, true))
    }

    /// G13/G17 `garden/delete`: the page moves into
    /// `.history/gardens/<id>.toml/` (with a `deleted.json` record of the
    /// list entry), the entry leaves the list, and its theme pick goes. The
    /// last Garden can't be deleted (409 `last_garden`). Nothing is removed
    /// from disk without a copy in history.
    pub fn delete_garden(&self, garden: &str) -> Result<DeleteOutcome, ZenError> {
        let _g = list_lock();
        self.require()?;
        let (i, g) = self.garden(garden)?;
        let mut list = self.gardens();
        if list.len() <= 1 {
            return Err(ZenError::LastGarden);
        }
        let f = ZenFile::Garden(g.id.clone());
        let path = self.path_of(&f);
        let snapshot = if path.is_file() {
            let dst = self.snapshot_slot(&f)?;
            fs::rename(&path, &dst).map_err(|e| io(e, &dst))?;
            dst.file_name().map(|n| n.to_string_lossy().to_string())
        } else {
            None
        };
        let dir = self.history_dir(&f);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let mut record = serde_json::to_value(&g).map_err(|e| ZenError::Io(e.to_string()))?;
        record["deletedAt"] = json!(Self::now());
        record["snapshot"] = json!(snapshot);
        self.sync_on_delete(&g.id, &mut record)?;
        let rec = dir.join(DELETED_RECORD);
        let body = serde_json::to_string_pretty(&record).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&rec, &(body + "\n")).map_err(|e| io(e, &rec))?;
        list.remove(i);
        self.write_list(&list)?;
        let mut active = self.read_active();
        if active.gardens.remove(&g.id).is_some() {
            self.write_active(&active)?;
        }
        self.prune(&f);
        Ok(DeleteOutcome { deleted: g, snapshot })
    }

    /// A Garden file's own top-level keys: everything but `schema` and
    /// `template` (its layout, widgets and theme tables). `Ok(vec![])` for no
    /// file or a bare stub; `Err` when the file isn't TOML text.
    pub fn garden_own_keys(&self, id: &str) -> Result<Vec<String>, String> {
        let f = ZenFile::Garden(id.to_string());
        let text = match self.read_current(&f) {
            None => return Ok(Vec::new()),
            Some(t) => t?,
        };
        let v: toml::Value = toml::from_str(&text).map_err(|e| format!("{} doesn't parse: {e}", f.label()))?;
        let table = v.as_table().ok_or_else(|| format!("{} is not a TOML table", f.label()))?;
        let mut keys: Vec<String> = table.keys().filter(|k| *k != "schema" && *k != "template").cloned().collect();
        keys.sort();
        Ok(keys)
    }

    /// `garden/template` (Rosson 2026-10-04, "Start with the default"): turn
    /// one Garden into a built-in template, keeping its id, name and place.
    /// The Garden's file becomes that template's stub and its list entry
    /// names the template. The previous file is kept as a snapshot first
    /// (`.history/gardens/<id>.toml/`). Idempotent: a Garden already on the
    /// template with nothing of its own changes nothing. A file that sets
    /// its own layout, widgets or theme tables is refused with
    /// [`ZenError::GardenHasChanges`] unless `force` (nothing is lost
    /// either way: the snapshot keeps it).
    pub fn set_garden_template(&self, garden: &str, template: &str, force: bool) -> Result<TemplateOutcome, ZenError> {
        let _g = list_lock();
        self.require()?;
        if template.trim().is_empty() {
            return Err(ZenError::BadRequest("garden/template needs a template: texting or blank".into()));
        }
        let tid = template_choice(Some(template))?;
        let (i, mut g) = self.garden(garden)?;
        let f = ZenFile::Garden(g.id.clone());
        let own = match self.garden_own_keys(&g.id) {
            Ok(k) => k,
            Err(_) => vec!["the whole file (it doesn't parse)".to_string()],
        };
        let path = self.path_of(&f);
        let on_template = self.file_template(&g.id).unwrap_or(g.template.as_str()) == tid;
        if own.is_empty() && on_template && g.template == tid && path.is_file() {
            return Ok(TemplateOutcome { garden: g, template: tid, changed: false, snapshot: None, replaced: Vec::new() });
        }
        if !own.is_empty() && !force {
            return Err(ZenError::GardenHasChanges { garden: g.id, keys: own });
        }
        let mut kept = None;
        if let Some(Ok(current)) = self.read_current(&f) {
            let newest = self.snapshots(&f).into_iter().next();
            kept = Some(match newest {
                Some(s) if self.snapshot_text(&f, &s.name).as_deref() == Some(current.as_str()) => s.name,
                _ => self.write_snapshot(&f, &current)?,
            });
        }
        let dir = self.gardens_dir();
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        crate::fs_atomic::atomic_write_str(&path, &garden_stub(&g.id, &g.name, tid)).map_err(|e| io(e, &path))?;
        if g.template != tid {
            let mut list = self.gardens();
            g.template = tid.to_string();
            list[i] = g.clone();
            self.write_list(&list)?;
        }
        self.prune(&f);
        self.sync_follow_k2(&g.id)?;
        if let Err(e) = self.news_mark_template_seen(tid) {
            crate::log_debug!("[zen/sync] news for {}: {e}", g.id);
        }
        Ok(TemplateOutcome { garden: g, template: tid, changed: true, snapshot: kept, replaced: own })
    }

    /// Check one file's text with the rules for its kind. A Garden page is
    /// checked against its own default template (G38).
    fn check_src(&self, f: &ZenFile, text: &str, base: &Layer) -> Checked {
        self.check_src_in(f, text, base, &self.defaults_for(f))
    }

    /// The set a file is checked on: a Garden page's own (its sync state),
    /// live for everything else.
    fn defaults_for(&self, f: &ZenFile) -> std::sync::Arc<Defaults> {
        match f {
            ZenFile::Garden(id) => self.page_defaults(id),
            _ => self.live_defaults(),
        }
    }

    /// [`ZenFiles::check_src`] with a Garden page checked on the set `d`
    /// (prd-zen-garden-sync-defaults-v1 GS20).
    fn check_src_in(&self, f: &ZenFile, text: &str, base: &Layer, d: &Defaults) -> Checked {
        match f {
            ZenFile::Garden(id) => schema::check_garden_in(d, &f.label(), text, base, &self.default_template(id)),
            _ => schema::check(&f.label(), text, f.kind(), base),
        }
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
        self.check_text_in(f, text, base, &self.defaults_for(f))
    }

    fn check_text_in(&self, f: &ZenFile, text: &Result<String, String>, base: &Layer, d: &Defaults) -> Checked {
        match text {
            Ok(t) => self.check_src_in(f, t, base, d),
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

    /// The path of a new snapshot of `f` (its folder made): names sort by
    /// time and stay strictly increasing even if the clock steps back.
    fn snapshot_slot(&self, f: &ZenFile) -> Result<PathBuf, ZenError> {
        let dir = self.history_dir(f);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let newest = self.snapshots(f).into_iter().next().map(|s| s.name);
        let mut seq = 0u32;
        loop {
            let name = format!("{stamp}-{seq:03}.toml");
            let after_newest = newest.as_deref().map_or(true, |n| name.as_str() > n);
            let path = dir.join(&name);
            if after_newest && !path.exists() {
                return Ok(path);
            }
            seq += 1;
            if seq > 999 {
                // Clock behind the newest snapshot: continue after it.
                let base = newest.as_deref().unwrap_or("0").trim_end_matches(".toml").to_string();
                return Ok(dir.join(format!("{base}x.toml")));
            }
        }
    }

    fn write_snapshot(&self, f: &ZenFile, text: &str) -> Result<String, ZenError> {
        let path = self.snapshot_slot(f)?;
        crate::fs_atomic::atomic_write_str(&path, text).map_err(|e| io(e, &path))?;
        Ok(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
    }

    fn prune(&self, f: &ZenFile) {
        let dir = self.history_dir(f);
        for s in self.snapshots(f).into_iter().skip(HISTORY_KEEP) {
            let _ = fs::remove_file(dir.join(&s.name));
        }
    }

    /// The live version of `f` over `base` (last-good semantics, Z13).
    pub fn effective(&self, f: &ZenFile, base: &Layer) -> Effective {
        self.effective_in(f, base, &self.defaults_for(f))
    }

    /// [`ZenFiles::effective`] with a Garden page checked on the set `d`.
    pub fn effective_in(&self, f: &ZenFile, base: &Layer, d: &Defaults) -> Effective {
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
        let checked = self.check_text_in(f, &text, base, d);
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
            let c = self.check_src_in(f, &t, base, d);
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
                label: t.label.to_string(),
                builtin: true,
                user: user.iter().any(|u| u == t.name),
                summary: Some(t.summary.to_string()),
            })
            .collect();
        for u in user {
            if super::builtin_theme(&u).is_none() {
                out.push(ThemeInfo { label: super::theme_label(&u), name: u, builtin: false, user: true, summary: None });
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

    /// `active.json`, or the empty default (global `basic`, no Garden
    /// picks). A pre-Gardens `homes` map is ignored (never released). A
    /// pick of `default` (the built-in's name before it was `basic`) reads
    /// as `basic`, quietly, unless the person has a theme called `default`.
    pub fn read_active(&self) -> ActiveFile {
        let mut a: ActiveFile = fs::read_to_string(self.root.join(ACTIVE_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let alias = super::DEFAULT_THEME_ALIAS;
        let picks_alias = a.theme.as_deref() == Some(alias) || a.gardens.values().any(|n| n == alias);
        if picks_alias && !self.path_of(&ZenFile::Theme(alias.to_string())).is_file() {
            let basic = || super::DEFAULT_THEME.to_string();
            if a.theme.as_deref() == Some(alias) {
                a.theme = Some(basic());
            }
            for n in a.gardens.values_mut().filter(|n| n.as_str() == alias) {
                *n = basic();
            }
        }
        a
    }

    fn write_active(&self, a: &ActiveFile) -> Result<(), ZenError> {
        let path = self.root.join(ACTIVE_FILE);
        let mut a = a.clone();
        a.version = ACTIVE_VERSION;
        let body = serde_json::to_string_pretty(&a).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| io(e, &path))
    }

    /// The theme a Garden shows (G16): its own pick, else the global one,
    /// else `basic`. A pick whose theme is gone is reported in `missing`
    /// and skipped. `garden` is a Garden id.
    pub fn active_theme(&self, garden: Option<&str>) -> ActiveTheme {
        let a = self.read_active();
        let mut missing = None;
        let mut chain: Vec<(String, &'static str)> = Vec::new();
        if let Some(n) = garden.and_then(|id| a.gardens.get(id)) {
            chain.push((n.clone(), "garden"));
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
    /// `basic`), or `basic` for a theme only the user has.
    fn theme_parent(name: &str) -> &'static Layer {
        super::builtin_theme_layer(name).unwrap_or_else(super::builtin_layer)
    }

    /// The built-in layer a theme sits on in the set `d` (GS20). A built-in
    /// the set doesn't have resolves live (GS22); a theme only the user has
    /// sits on the set's `basic`.
    fn theme_parent_in(d: &Defaults, name: &str) -> Layer {
        d.theme_layer(name)
            .or_else(|| super::builtin_theme(name).and_then(|_| Defaults::live().theme_layer(name)))
            .or_else(|| d.theme_layer(super::DEFAULT_THEME))
            .cloned()
            .unwrap_or_else(|| super::builtin_layer().clone())
    }

    /// `(built-in parent, the user file's live version, parent + file)`.
    fn theme_stack(&self, name: &str) -> (Layer, Effective, Layer) {
        self.theme_stack_in(Defaults::live(), name)
    }

    /// [`ZenFiles::theme_stack`] on the set `d`.
    fn theme_stack_in(&self, d: &Defaults, name: &str) -> (Layer, Effective, Layer) {
        let parent = Self::theme_parent_in(d, name);
        let eff = self.effective(&ZenFile::Theme(name.to_string()), &parent);
        let base = schema::merge(&[&parent, &eff.layer]);
        (parent, eff, base)
    }

    /// What a file is checked over: a theme over its built-in parent,
    /// `zen.toml` over the global theme, a Garden over its own theme plus
    /// `zen.toml`.
    pub(crate) fn base_for(&self, f: &ZenFile) -> Layer {
        match f {
            ZenFile::Theme(name) => Self::theme_parent(name).clone(),
            ZenFile::Zen => self.theme_stack(&self.active_theme(None).name).2,
            ZenFile::Garden(id) => {
                let tbase = self.theme_stack_in(&self.theme_defaults(id), &self.active_theme(Some(id)).name).2;
                let zen = self.effective(&ZenFile::Zen, &tbase);
                schema::merge(&[&tbase, &zen.layer])
            }
        }
    }

    fn garden_id(&self, garden: &str) -> Result<String, ZenError> {
        self.garden(garden).map(|(_, g)| g.id)
    }

    /// `k2 zen theme set`: pick `name` globally, or for one Garden (id or
    /// name). `name` None with a Garden clears that Garden's pick (it
    /// follows the global one again). An unknown name changes nothing.
    pub fn set_theme(&self, name: Option<&str>, garden: Option<&str>) -> Result<ThemeSwitch, ZenError> {
        self.require()?;
        let garden_id = garden.map(|g| self.garden_id(g)).transpose()?;
        if let Some(n) = name {
            if !self.theme_exists(n) {
                return Err(self.unknown_theme(n));
            }
        }
        let mut a = self.read_active();
        match (&garden_id, name) {
            (Some(id), Some(n)) => {
                a.gardens.insert(id.clone(), n.to_string());
            }
            (Some(id), None) => {
                a.gardens.remove(id);
            }
            (None, Some(n)) => a.theme = Some(n.to_string()),
            (None, None) => {
                return Err(ZenError::BadRequest(
                    "theme set needs a theme name, or a Garden to clear (--garden <name> --clear)".into(),
                ))
            }
        }
        self.write_active(&a)?;
        let now = self.active_theme(garden_id.as_deref());
        Ok(ThemeSwitch { theme: now.name, scope: now.scope.to_string(), garden: garden_id })
    }

    /// `k2 zen theme next|prev`: step through [`ZenFiles::themes`] (wrapping)
    /// from what the Garden (or the computer) shows now.
    pub fn cycle_theme(&self, step: i64, garden: Option<&str>) -> Result<ThemeSwitch, ZenError> {
        self.require()?;
        let garden_id = garden.map(|g| self.garden_id(g)).transpose()?;
        let names = self.theme_names();
        let current = self.active_theme(garden_id.as_deref()).name;
        let n = names.len() as i64;
        let i = names.iter().position(|x| *x == current).unwrap_or(0) as i64;
        let next = names[(((i + step) % n + n) % n) as usize].clone();
        self.set_theme(Some(&next), garden_id.as_deref())
    }

    /// `k2 zen theme new <name>`: start a user theme bundle from a copy of
    /// `from` (default: the built-in of the same name, else `basic`).
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
    pub fn theme_list(&self, garden: Option<&str>) -> Result<J, ZenError> {
        self.require()?;
        let garden_id = garden.map(|g| self.garden_id(g)).transpose()?;
        let active = self.active_theme(garden_id.as_deref());
        let a = self.read_active();
        Ok(json!({
            "ok": true,
            "active": active.name,
            "scope": active.scope,
            "missing": active.missing,
            "global": a.theme.clone().unwrap_or_else(|| super::DEFAULT_THEME.to_string()),
            "garden": garden_id,
            "gardenTheme": garden_id.as_deref().and_then(|id| a.gardens.get(id).cloned()),
            "themes": self.themes_json(&active.name),
        }))
    }

    /// One Garden as `GET /cli/zen/gardens` lists it (G13): 1-based
    /// `index`, whether its page file exists, and its own theme pick.
    pub fn garden_json(&self, index: usize, g: &GardenEntry, active: &ActiveFile) -> J {
        let mut v = json!({
            "id": g.id,
            "name": g.name,
            "index": index + 1,
            "template": g.template,
            "hasFile": self.path_of(&ZenFile::Garden(g.id.clone())).is_file(),
            "createdAt": g.created_at,
            "theme": active.gardens.get(&g.id),
        });
        if let Some(h) = &g.seed_home {
            v["seedHome"] = json!(h);
        }
        v
    }

    /// Every Garden, in order, as `GET /cli/zen/gardens` lists them.
    pub fn gardens_json(&self) -> Vec<J> {
        let active = self.read_active();
        self.gardens().iter().enumerate().map(|(i, g)| self.garden_json(i, g, &active)).collect()
    }

    /// GET `/cli/zen/get` body (Z15, G12) for a Garden (id or name; `None`
    /// is the first). The page is the Garden's template with its file's
    /// layout and widgets (G38); the theme stack ends with its file.
    pub fn resolve(&self, garden: Option<&str>) -> Result<J, ZenError> {
        self.resolve_with(garden, &super::grants::GrantSnapshot::empty())
    }

    /// [`ZenFiles::resolve`] with the daemon's grants: each custom widget
    /// carries its folder's state and its effective grant (UW38), and
    /// Garden-file findings that need the folders join `errors`.
    pub fn resolve_with(&self, garden: Option<&str>, grants: &super::grants::GrantSnapshot) -> Result<J, ZenError> {
        self.resolve_view_with(garden, grants, &super::sync::View::current())
    }

    /// [`ZenFiles::resolve`] through `view`, with no grants (the sync code's
    /// comparisons: both sides see the same grants).
    pub fn resolve_view(&self, garden: Option<&str>, view: &super::sync::View) -> Result<J, ZenError> {
        self.resolve_view_with(garden, &super::grants::GrantSnapshot::empty(), view)
    }

    /// [`ZenFiles::resolve_with`] through `view`: the Garden's page and theme
    /// on their sync state's defaults (or a preview / test override), plus
    /// `frame` and, with `view.meta`, `sync` (prd-zen-garden-sync-defaults-v1
    /// GS20, GS25, GS30). Writes nothing.
    pub fn resolve_view_with(
        &self,
        garden: Option<&str>,
        grants: &super::grants::GrantSnapshot,
        view: &super::sync::View,
    ) -> Result<J, ZenError> {
        self.require()?;
        let (index, entry) = match garden {
            Some(sel) => self.garden(sel)?,
            None => {
                let first = self.gardens().into_iter().next().ok_or(ZenError::NotSetUp)?;
                (0, first)
            }
        };
        let id = entry.id.clone();
        let gd = self.garden_defaults(&id, view);
        let active = self.active_theme(Some(&id));
        let (parent, theme, tbase) = self.theme_stack_in(&gd.theme, &active.name);
        let zen = self.effective(&ZenFile::Zen, &tbase);
        let base = schema::merge(&[&tbase, &zen.layer]);
        let gfile = ZenFile::Garden(id.clone());
        let page = self.effective_in(&gfile, &base, &gd.page);
        let user = schema::merge(&[&theme.layer, &zen.layer, &page.layer]);
        let rt = schema::resolve(&parent, &user);

        let mut errors = theme.errors.clone();
        errors.extend(zen.errors.iter().cloned());
        errors.extend(page.errors.iter().cloned());
        let mut warnings = theme.warnings.clone();
        warnings.extend(zen.warnings.iter().cloned());
        warnings.extend(page.warnings.iter().cloned());
        warnings.extend(gd.warnings());
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
            "label": super::theme_label(&active.name),
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
        let garden_json = json!({ "id": id, "name": entry.name, "index": index + 1 });
        let (page_layer, page_template) = gd.page_layer(&page.layer, &entry.template);
        let mut page_json = super::garden_page_in(&gd.page, &page_layer, &page_template);
        errors.extend(self.placement_diagnostics(
            &gfile.label(),
            &page_json,
            page.layer.get("page.widgets"),
            &page.positions,
        ));
        self.fill_custom_widgets(&id, &mut page_json, grants);
        let versioned = json!({
            "garden": garden_json,
            "page": page_json,
            "theme": theme_json,
            "chrome": rt.chrome,
            "motion": rt.motion,
            "frame": super::sync::frame_json(&gd.page),
            "sync": if view.meta { self.sync_json(&id, &gd, view)? } else { J::Null },
        });
        let version = schema::fnv_hex(versioned.to_string().as_bytes());
        let last_good_at = [theme.at.clone(), zen.at.clone(), page.at.clone()].into_iter().flatten().max();
        let mut sources = serde_json::Map::new();
        sources.insert(ZenFile::Theme(active.name.clone()).label(), theme.origin.as_json());
        sources.insert(ZenFile::Zen.label(), zen.origin.as_json());
        sources.insert(gfile.label(), page.origin.as_json());
        Ok(json!({
            "ok": true,
            "schema": schema::SCHEMA_VERSION,
            "version": version,
            "garden": versioned["garden"],
            "page": versioned["page"],
            "theme": versioned["theme"],
            "themes": self.themes_json(&active.name),
            "chrome": versioned["chrome"],
            "motion": versioned["motion"],
            "frame": versioned["frame"],
            "sync": versioned["sync"],
            "errors": errors,
            "warnings": warnings,
            "lastGoodAt": last_good_at,
            "sources": sources,
        }))
    }

    /// Garden ids with a list entry or a page file: the list's order, then
    /// files the list doesn't name.
    fn garden_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.gardens().into_iter().map(|g| g.id).collect();
        for id in self.garden_file_ids() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    /// Every file `validate`/`refresh` looks at: `zen.toml`, each Garden's
    /// page and each user theme.
    fn all_files(&self) -> Vec<ZenFile> {
        let mut out = vec![ZenFile::Zen];
        out.extend(self.garden_ids().into_iter().map(ZenFile::Garden));
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
            if let (ZenFile::Garden(id), true) = (f, c.is_clean()) {
                let page = super::garden_page(&c.layer, &self.default_template(id));
                errors.extend(self.placement_diagnostics(&f.label(), &page, c.layer.get("page.widgets"), &c.positions));
            }
            errors.extend(c.errors);
            warnings.extend(c.warnings);
            checked.push(f.label());
        }
        if target.is_none() {
            let (e, w, labels) = self.validate_all_widgets();
            errors.extend(e);
            warnings.extend(w);
            checked.extend(labels);
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
    /// the live state (active theme and the Garden list included, G41) plus
    /// its diagnostics. Two calls with nothing effectively changed return
    /// the same fingerprint, so a caller emits `zen_changed` only when it
    /// moves (Z12): a Garden create, rename, reorder or delete moves it.
    pub fn refresh(&self) -> Result<String, ZenError> {
        self.refresh_with(&super::grants::GrantSnapshot::empty())
    }

    /// [`ZenFiles::refresh`] that also covers every widget folder (keeping
    /// its last good bundle and code history, UW11) and the daemon's grants
    /// (UW12): a widget save, a grant and a revoke each move it once.
    pub fn refresh_with(&self, grants: &super::grants::GrantSnapshot) -> Result<String, ZenError> {
        self.require()?;
        let mut state = serde_json::Map::new();
        state.insert(WIDGETS_DIR_KEY.to_string(), self.refresh_widgets()?);
        state.insert(GRANTS_KEY.to_string(), grants.fingerprint());
        state.insert(super::sync::SYNC_FILE.to_string(), self.sync_fingerprint_state());
        state.insert(
            GARDENS_FILE.to_string(),
            serde_json::to_value(self.gardens()).map_err(|e| ZenError::Io(e.to_string()))?,
        );
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
        for id in self.garden_ids() {
            let f = ZenFile::Garden(id);
            let eff = self.refresh_one(&f, &self.base_for(&f))?;
            state.insert(f.label(), effective_state(&eff));
        }
        Ok(schema::fnv_hex(J::Object(state).to_string().as_bytes()))
    }

    fn refresh_one(&self, f: &ZenFile, base: &Layer) -> Result<Effective, ZenError> {
        if let Some(Ok(text)) = self.read_current(f) {
            if self.check_src(f, &text, base).is_clean() {
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
            // Deleted Gardens and deleted themes live only in history.
            if let Ok(rd) = fs::read_dir(self.history_root().join(GARDENS_DIR)) {
                let mut names: Vec<String> =
                    rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
                names.sort();
                for name in names {
                    if let Ok(f) = ZenFile::parse(&format!("{GARDENS_DIR}/{name}")) {
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
                            .map(|t| self.check_src(f, &t, &b).is_clean())
                            .unwrap_or(false);
                        json!({ "name": s.name, "at": s.at, "bytes": s.bytes, "clean": clean })
                    })
                    .collect();
                let mut v = json!({ "file": f.label(), "exists": self.path_of(f).is_file(), "snapshots": snaps });
                if let ZenFile::Garden(id) = f {
                    v["garden"] = json!(id);
                    v["deleted"] = json!(!self.gardens().iter().any(|g| &g.id == id));
                }
                v
            })
            .collect();
        Ok(json!({ "ok": true, "keep": HISTORY_KEEP, "files": out }))
    }

    /// POST `/cli/zen/reset` (Z14, Omarchy addition 3): keep the current
    /// file as a snapshot first, then restore snapshot `to`, or clear the
    /// user's changes when `to` is None: `zen.toml` and Garden pages go back
    /// to their stubs (a Garden's to its template's, G17), an override of a
    /// built-in theme is removed (the built-in shows again), and a theme
    /// only the user has goes back to a copy of `basic`. Never touches
    /// `gardens.json`, `active.json` or `grants.json`.
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
        if let (ZenFile::Garden(id), None) = (f, to) {
            self.sync_follow_k2(id)?;
        }
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
        let (list, source) = self.read_list();
        let orphans: Vec<String> =
            self.garden_file_ids().into_iter().filter(|id| !list.iter().any(|g| &g.id == id)).collect();
        let names = list.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ");
        let (ok, detail) = match (&source, orphans.is_empty()) {
            (ListSource::File, true) => (true, format!("{} Garden(s): {names}", list.len())),
            (ListSource::File, false) => (
                false,
                format!("pages with no Garden in gardens.json: {} (k2 zen garden list shows the Gardens)", orphans.join(", ")),
            ),
            (ListSource::Rebuilt, _) => (
                false,
                format!("gardens.json is missing; the list was rebuilt from gardens/*.toml ({names}). Any k2 zen garden change writes it again"),
            ),
            (ListSource::Unreadable(m), _) => (
                false,
                format!("{m}; the list was rebuilt from gardens/*.toml ({names}). The next k2 zen garden change keeps the bad file in .history/"),
            ),
            (ListSource::Missing, _) => (false, format!("no Gardens. {NOT_SET_UP}")),
        };
        checks.push(check("gardens.json", ok, detail));
        let bad_templates: Vec<String> = schema::template_ids()
            .iter()
            .filter(|t| {
                let page = super::template_page(t);
                let kinds: Vec<String> = page
                    .as_ref()
                    .and_then(|p| p["controls"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|c| c["kind"].as_str().map(str::to_string))
                    .collect();
                // FC30: the template's chrome passes the Garden checks too.
                let chrome_clean = super::check_template_chrome(t).is_some_and(|c| c.is_clean());
                !chrome_clean || !super::REQUIRED_CONTROLS.iter().all(|r| kinds.iter().any(|k| k == r))
            })
            .map(|t| t.to_string())
            .collect();
        checks.push(check(
            "templates",
            bad_templates.is_empty(),
            if bad_templates.is_empty() {
                format!("{} built in, each with the required controls", schema::template_ids().len())
            } else {
                format!("missing a required control: {}", bad_templates.join(", "))
            },
        ));
        let grants = self.root.join(GRANTS_FILE).exists();
        checks.push(check(
            "grants.json",
            true,
            if grants {
                "present and ignored: widget grants live in K2's database, signed, and only your click in the K2 app makes one (k2 zen widget list)".into()
            } else {
                "absent (widget grants live in K2's database; k2 zen widget list shows them)".into()
            },
        ));
        let names = self.widget_names();
        let broken: Vec<String> = names
            .iter()
            .filter_map(|n| {
                let st = self.widget_status(n);
                (st.state != super::widget_store::WidgetState::Ok)
                    .then(|| format!("{} ({})", n, st.errors.first().map_or_else(String::new, |d| d.render())))
            })
            .collect();
        checks.push(check(
            "widgets",
            broken.is_empty(),
            if broken.is_empty() {
                format!("{} widget folder(s), each bundles clean", names.len())
            } else {
                format!("with errors: {}", broken.join("; "))
            },
        ));
        let total: usize = self.all_files().iter().map(|f| self.snapshots(f).len()).sum();
        checks.push(check("history", true, format!("{total} snapshot(s), {HISTORY_KEEP} kept per file")));
        checks.extend(self.sync_doctor_checks());
        checks
    }
}

fn effective_state(e: &Effective) -> J {
    json!({ "layer": e.layer, "errors": e.errors, "warnings": e.warnings })
}

/// `active.json`'s version since Gardens (G16).
pub const ACTIVE_VERSION: u32 = 2;

/// `active.json`: the global theme and each Garden's own pick (G16).
/// Daemon-written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub gardens: BTreeMap<String, String>,
}

/// The theme a Garden shows and where the pick came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTheme {
    pub name: String,
    /// `"garden"` (the Garden's own pick) or `"global"`.
    pub scope: &'static str,
    /// A pick that names a theme that is gone (skipped).
    pub missing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeInfo {
    pub name: String,
    /// What people see: "Basic", "Paper", "Midnight", or the user theme's
    /// id with a capital first letter.
    pub label: String,
    /// Built into K2 (read-only).
    pub builtin: bool,
    /// The user has `themes/<name>/theme.toml` (for a built-in: an override).
    pub user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeSwitch {
    /// The theme now shown (for `garden`, or globally).
    pub theme: String,
    pub scope: String,
    /// The Garden id, when the switch was for one Garden.
    pub garden: Option<String>,
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

/// The stub a new Garden gets and `reset` restores (G10, G17): the
/// template line plus comments that name the Garden. It sets nothing.
pub fn garden_stub(id: &str, name: &str, template: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let body = if template == schema::BLANK_TEMPLATE_ID {
        format!(
            "# This Garden starts empty. Place K2's built-in widgets with [layout]\n\
             # and [[widget]] blocks, for example a Home's agents beside a\n\
             # conversation:\n\
             #\n\
             #   [layout]\n\
             #   kind = \"columns\"\n\
             #   [[layout.column]]\n\
             #   size = 40\n\
             #   min-width = 240\n\
             #   [[layout.column]]\n\
             #   size = 60\n\
             #   min-width = 360\n\
             #\n\
             #   [[widget]]\n\
             #   id = \"agents\"\n\
             #   kind = \"agents\"          # a whole Home, or one agent (mode = \"agent\")\n\
             #   column = 0\n\
             #   [widget.props]\n\
             #   home = \"Work\"            # a Home's name or id\n\
             #\n\
             #   [[widget]]\n\
             #   id = \"talk\"\n\
             #   kind = \"conversation\"    # follows the Agents widget\n\
             #   column = 1\n\
             #\n\
             # Theme tables ([colors.light], [font], [shape], ...) restyle this\n\
             # Garden only. Check with `k2 zen validate --garden {id}`.\n"
        )
    } else if let Some(e) = super::garden_catalog::garden_catalog().iter().find(|e| e.template_id == template) {
        let label: String = e.meta.label.chars().filter(|c| !c.is_control()).collect();
        format!(
            "# This Garden is K2's ready-made \"{label}\" page ({template}). Theme\n\
             # tables ([colors.light], [font], [shape], ...) restyle this Garden\n\
             # only; [layout] and [[widget]] replace its widgets (the k2-zen skill\n\
             # has the grammar). Check with `k2 zen validate --garden {id}`.\n"
        )
    } else {
        format!(
            "# This Garden is the texting page: a Home's agents beside the\n\
             # conversation with the one you pick. Theme tables ([colors.light],\n\
             # [font], [shape], ...) restyle this Garden only; [layout] and\n\
             # [[widget]] rearrange K2's built-in widgets (the k2-zen skill has\n\
             # the grammar). Check with `k2 zen validate --garden {id}`.\n"
        )
    };
    format!(
        "# Zen Garden \"{name}\" (id {id}).\n\
         #\n\
         {body}\
         schema = 1\n\
         template = \"{template}\"\n"
    )
}
