//! Custom widgets in the Zen folder (prd-zen-user-widgets-v2 UW11, UW12,
//! UW35, UW38, UWB21): `ZenFiles` methods for `~/.k2/zen/widgets/<name>/`.
//!
//! - **Live version (UW11).** A folder that bundles clean is live. A folder
//!   with errors serves its last good bundle (`state: "errors"`); with no
//!   last good it is `state: "broken"`. A built-in `k2:<name>@<n>` is
//!   always its compiled files.
//! - **What the daemon keeps.** `.history/widgets/<name>/last-good.html`
//!   (the nonce-free bundle) + `last-good.json` (hash, manifest, time,
//!   nonce offsets), and the last [`HISTORY_KEEP`] versions of the code
//!   files (manifest, HTML, JS, CSS; never assets) in
//!   `.history/widgets/<name>/<utc>/`. Only [`ZenFiles::refresh_widget`]
//!   writes them (the watcher and `get` call it through `refresh`), and
//!   `reset --widget` keeps the code it replaces.
//! - There are no grants (2026-10-08): a widget in your own Garden runs
//!   with every Garden-safe cap it asks for ([`super::widget_access`]).
//!   The daemon's runaway pauses come in as a [`WidgetPauses`].

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value as J};

use super::builtin_widgets::{self, parse_widget_ref, WidgetRef};
use super::bundle::Bundle;
use super::widget_access::{effective_caps, WidgetOrigin, WidgetPauses};
use super::schema::{self, Diagnostic};
use super::store::{ZenError, ZenFile, ZenFiles, HISTORY_KEEP};
use super::widgets::{self, Manifest, WIDGETS_DIR};

pub const LAST_GOOD_HTML: &str = "last-good.html";
pub const LAST_GOOD_JSON: &str = "last-good.json";
/// Each code snapshot folder's record: `{at, hash, clean}`.
pub const SNAPSHOT_META: &str = "meta.json";

/// A widget's live state (UW35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetState {
    /// The folder bundles clean.
    Ok,
    /// The folder has errors; its last good bundle is live.
    Errors,
    /// The folder has errors and no last good bundle, or doesn't exist.
    Broken,
}

impl WidgetState {
    pub fn as_str(self) -> &'static str {
        match self {
            WidgetState::Ok => "ok",
            WidgetState::Errors => "errors",
            WidgetState::Broken => "broken",
        }
    }
}

/// One widget as the routes and `resolve` read it.
#[derive(Debug, Clone)]
pub struct WidgetStatus {
    /// The folder name or `k2:<name>@<n>`.
    pub widget: String,
    pub exists: bool,
    pub builtin: bool,
    /// The live manifest (the folder's when clean, else the last good one).
    pub manifest: Option<Manifest>,
    /// The live bundle.
    pub bundle: Option<Bundle>,
    pub state: WidgetState,
    /// What is wrong with the folder on disk now.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// When the live bundle was made good (RFC 3339), for a last good.
    pub last_good_at: Option<String>,
}

impl WidgetStatus {
    pub fn hash(&self) -> &str {
        self.bundle.as_ref().map_or("", |b| b.hash.as_str())
    }

    /// The manifest's `caps` (empty with no live manifest).
    pub fn requested(&self) -> Vec<String> {
        self.manifest.as_ref().map(|m| m.caps.clone()).unwrap_or_default()
    }

    /// The name K2's dialog shows: the manifest's, else the widget name.
    pub fn title(&self) -> String {
        self.manifest.as_ref().map_or_else(|| self.widget.clone(), |m| m.name.clone())
    }
}

/// One custom placement on a Garden's live page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub garden: String,
    pub garden_name: String,
    pub placement: String,
    pub widget: String,
}

/// Bundling is the slow part; every `get` refreshes. Keyed by label and
/// the files' content hash, so an edit is a new entry.
static CHECKED: Mutex<BTreeMap<String, Arc<widgets::Checked>>> = Mutex::new(BTreeMap::new());

fn checked_cached(prefix: &str, folder: &str, files: &BTreeMap<String, Vec<u8>>) -> Arc<widgets::Checked> {
    let key = format!("{prefix}\0{}", widgets::content_hash(files));
    let mut cache = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.get(&key) {
        return c.clone();
    }
    let c = Arc::new(widgets::check_files(prefix, folder, files));
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(key, c.clone());
    c
}

fn io(e: std::io::Error, what: &std::path::Path) -> ZenError {
    ZenError::Io(format!("{}: {e}", what.display()))
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `widgets/<name>` (the label findings use).
pub fn widget_label(name: &str) -> String {
    format!("{WIDGETS_DIR}/{name}")
}

fn missing_folder_message(name: &str) -> String {
    format!("no widget folder '{name}'; make one with k2 zen widget new {name}")
}

fn snapshot_at(name: &str) -> String {
    let stem = name.split('-').next().unwrap_or(name);
    chrono::NaiveDateTime::parse_from_str(stem.trim_end_matches('Z'), "%Y%m%dT%H%M%S%3f")
        .map(|dt| dt.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

fn valid_snapshot_dir(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// One code snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct CodeSnapshot {
    pub name: String,
    pub at: String,
    pub bytes: u64,
    pub clean: bool,
    pub hash: Option<String>,
}

impl ZenFiles {
    /// `~/.k2/zen/widgets/`.
    pub fn widgets_dir(&self) -> PathBuf {
        self.root().join(WIDGETS_DIR)
    }

    /// `~/.k2/zen/widgets/<name>/`.
    pub fn widget_dir(&self, name: &str) -> PathBuf {
        self.widgets_dir().join(name)
    }

    fn widget_history_dir(&self, name: &str) -> PathBuf {
        self.history_root().join(WIDGETS_DIR).join(name)
    }

    /// Folders under `widgets/` with a valid name, sorted.
    pub fn widget_names(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.widgets_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let dir = fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir());
                        (dir && super::valid_theme_name(&name)).then_some(name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    fn require_set_up(&self) -> Result<(), ZenError> {
        if self.is_set_up() {
            Ok(())
        } else {
            Err(ZenError::NotSetUp)
        }
    }

    /// A user widget name (a folder), or 400.
    fn user_widget_name(&self, name: &str) -> Result<String, ZenError> {
        match parse_widget_ref(name.trim()) {
            Ok(WidgetRef::User(n)) => Ok(n),
            Ok(WidgetRef::Builtin { .. }) => Err(ZenError::BadRequest(format!(
                "'{name}' is built into K2 and can't be changed; copy it with k2 zen widget new <name> --from {name}"
            ))),
            Err(m) => Err(ZenError::BadRequest(m)),
        }
    }

    /// The folder's (or built-in's) files and the read findings, checked.
    /// `None` when the folder doesn't exist or the built-in is unknown.
    fn check_now(&self, widget: &str) -> Option<(Arc<widgets::Checked>, Vec<Diagnostic>, Vec<Diagnostic>, bool)> {
        match parse_widget_ref(widget).ok()? {
            WidgetRef::User(name) => {
                let dir = self.widget_dir(&name);
                if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                    return None;
                }
                let read = widgets::read_folder(&dir, &widget_label(&name));
                let c = checked_cached(&widget_label(&name), &name, &read.files);
                Some((c, read.errors, read.warnings, false))
            }
            r @ WidgetRef::Builtin { version: Some(_), .. } => {
                let w = builtin_widgets::builtin_widget(&r)?;
                let files = widgets::builtin_files(w);
                let prefix = format!("builtin:{}", w.id());
                let c = checked_cached(&prefix, w.name, &files);
                Some((c, Vec::new(), Vec::new(), true))
            }
            WidgetRef::Builtin { version: None, .. } => None,
        }
    }

    /// The last good bundle kept for a folder: `(manifest, bundle, at)`.
    fn last_good(&self, name: &str) -> Option<(Manifest, Bundle, String)> {
        let dir = self.widget_history_dir(name);
        let meta: J = serde_json::from_str(&fs::read_to_string(dir.join(LAST_GOOD_JSON)).ok()?).ok()?;
        let html = fs::read_to_string(dir.join(LAST_GOOD_HTML)).ok()?;
        let offsets: Vec<usize> = meta["nonceAt"].as_array()?.iter().map(|v| v.as_u64().map(|n| n as usize)).collect::<Option<_>>()?;
        let code = meta["codeBytes"].as_u64()? as usize;
        let bundle = Bundle::from_template(&html, &offsets, code)?;
        if Some(bundle.hash.as_str()) != meta["hash"].as_str() {
            return None;
        }
        let manifest = Manifest::from_saved(&meta["manifest"])?;
        Some((manifest, bundle, meta["at"].as_str().unwrap_or_default().to_string()))
    }

    fn write_last_good(&self, name: &str, m: &Manifest, b: &Bundle) -> Result<(), ZenError> {
        let dir = self.widget_history_dir(name);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let html = dir.join(LAST_GOOD_HTML);
        crate::fs_atomic::atomic_write_str(&html, &b.template()).map_err(|e| io(e, &html))?;
        let meta = json!({
            "hash": b.hash,
            "at": now_rfc3339(),
            "nonceAt": b.nonce_offsets(),
            "codeBytes": b.code_bytes,
            "manifest": widgets::manifest_json(m),
        });
        let path = dir.join(LAST_GOOD_JSON);
        let body = serde_json::to_string_pretty(&meta).map_err(|e| ZenError::Io(e.to_string()))?;
        crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| io(e, &path))
    }

    /// A widget's live state (read-only: never writes history).
    pub fn widget_status(&self, widget: &str) -> WidgetStatus {
        let builtin = widget.starts_with(builtin_widgets::BUILTIN_PREFIX);
        let mut st = WidgetStatus {
            widget: widget.to_string(),
            exists: false,
            builtin,
            manifest: None,
            bundle: None,
            state: WidgetState::Broken,
            errors: Vec::new(),
            warnings: Vec::new(),
            last_good_at: None,
        };
        let Some((c, read_errors, read_warnings, _)) = self.check_now(widget) else {
            let message = if builtin {
                format!("this K2 has no built-in widget '{widget}'; update K2, or name one it has")
            } else {
                missing_folder_message(widget)
            };
            st.errors.push(Diagnostic { file: widget_label(widget), line: 1, col: 1, message });
            return st;
        };
        st.exists = true;
        st.errors = read_errors;
        st.errors.extend(c.errors.iter().cloned());
        st.warnings = read_warnings;
        st.warnings.extend(c.warnings.iter().cloned());
        if st.errors.is_empty() && c.is_clean() {
            st.state = WidgetState::Ok;
            st.manifest = c.manifest.clone();
            st.bundle = c.bundle.clone();
            if !builtin {
                st.last_good_at = self.last_good(widget).filter(|(_, b, _)| Some(&b.hash) == st.bundle.as_ref().map(|x| &x.hash)).map(|(_, _, at)| at);
            }
            return st;
        }
        if !builtin {
            if let Some((m, b, at)) = self.last_good(widget) {
                st.state = WidgetState::Errors;
                st.manifest = Some(m);
                st.bundle = Some(b);
                st.last_good_at = Some(at);
            }
        }
        st
    }

    /// The live bundle, or 404 `unknown_widget` / 409 `widget_broken`.
    pub fn widget_bundle(&self, widget: &str) -> Result<WidgetStatus, ZenError> {
        self.require_set_up()?;
        if let Err(m) = parse_widget_ref(widget) {
            return Err(ZenError::BadRequest(m));
        }
        let st = self.widget_status(widget);
        if !st.exists {
            return Err(ZenError::UnknownWidget(
                st.errors.first().map_or_else(|| format!("no widget '{widget}'"), |d| d.message.clone()),
            ));
        }
        if st.bundle.is_none() {
            let first = st.errors.first().map_or_else(String::new, |d| d.render());
            return Err(ZenError::WidgetBroken(format!(
                "This widget has errors and no earlier good version: {first}. Ask your agent to fix it."
            )));
        }
        Ok(st)
    }

    // ── code history ─────────────────────────────────────────────────

    /// A folder's code snapshots, newest first.
    pub fn widget_snapshots(&self, name: &str) -> Vec<CodeSnapshot> {
        let dir = self.widget_history_dir(name);
        let mut out: Vec<CodeSnapshot> = fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        if !valid_snapshot_dir(&n) {
                            return None;
                        }
                        let meta: J = fs::read_to_string(e.path().join(SNAPSHOT_META))
                            .ok()
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or(J::Null);
                        let bytes = fs::read_dir(e.path())
                            .map(|rd| rd.flatten().filter(|f| f.file_name() != SNAPSHOT_META).filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum())
                            .unwrap_or(0);
                        Some(CodeSnapshot {
                            at: snapshot_at(&n),
                            name: n,
                            bytes,
                            clean: meta["clean"].as_bool().unwrap_or(false),
                            hash: meta["hash"].as_str().map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.name.cmp(&a.name));
        out
    }

    fn snapshot_files(&self, name: &str, snap: &str) -> Option<BTreeMap<String, Vec<u8>>> {
        if !valid_snapshot_dir(snap) {
            return None;
        }
        let dir = self.widget_history_dir(name).join(snap);
        let rd = fs::read_dir(&dir).ok()?;
        let mut out = BTreeMap::new();
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == SNAPSHOT_META || !e.path().is_file() {
                continue;
            }
            out.insert(n, fs::read(e.path()).ok()?);
        }
        Some(out)
    }

    fn write_code_snapshot(
        &self,
        name: &str,
        files: &BTreeMap<String, Vec<u8>>,
        clean: bool,
        hash: Option<&str>,
    ) -> Result<String, ZenError> {
        let root = self.widget_history_dir(name);
        fs::create_dir_all(&root).map_err(|e| io(e, &root))?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let newest = self.widget_snapshots(name).into_iter().next().map(|s| s.name);
        let mut seq = 0u32;
        let snap = loop {
            let n = format!("{stamp}-{seq:03}");
            if newest.as_deref().is_none_or(|x| n.as_str() > x) && !root.join(&n).exists() {
                break n;
            }
            seq += 1;
            if seq > 999 {
                break format!("{}x", newest.as_deref().unwrap_or("0"));
            }
        };
        let dir = root.join(&snap);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        for (f, b) in files {
            let p = dir.join(f);
            crate::fs_atomic::atomic_write(&p, b).map_err(|e| io(e, &p))?;
        }
        let meta = json!({ "at": now_rfc3339(), "clean": clean, "hash": hash });
        let p = dir.join(SNAPSHOT_META);
        crate::fs_atomic::atomic_write_str(&p, &(meta.to_string() + "\n")).map_err(|e| io(e, &p))?;
        for old in self.widget_snapshots(name).into_iter().skip(HISTORY_KEEP) {
            let _ = fs::remove_dir_all(root.join(&old.name));
        }
        Ok(snap)
    }

    /// Re-read one folder (UW11): when it bundles clean with new content,
    /// keep it as the last good bundle and its code as a snapshot. Returns
    /// the fingerprint part for `refresh` (UW12).
    pub fn refresh_widget(&self, name: &str) -> Result<J, ZenError> {
        let st = self.widget_status(name);
        if st.state == WidgetState::Ok {
            if let (Some(m), Some(b)) = (&st.manifest, &st.bundle) {
                let kept = self.last_good(name).map(|(_, kb, _)| kb.hash);
                if kept.as_deref() != Some(b.hash.as_str()) {
                    self.write_last_good(name, m, b)?;
                }
                let read = widgets::read_folder(&self.widget_dir(name), &widget_label(name));
                let code = widgets::code_files(&read.files);
                let newest_clean = self.widget_snapshots(name).into_iter().find(|s| s.clean);
                let same = newest_clean.and_then(|s| self.snapshot_files(name, &s.name)).is_some_and(|f| f == code);
                if !same {
                    self.write_code_snapshot(name, &code, true, Some(&b.hash))?;
                }
            }
        }
        Ok(json!({
            "hash": st.hash(),
            // The manifest is outside the bundle (name, caps, reasons, libs).
            "manifest": st.manifest.as_ref().map(widgets::manifest_json),
            "state": st.state.as_str(),
            "errors": st.errors,
            "warnings": st.warnings,
        }))
    }

    /// Every folder's fingerprint part, refreshing each (UW12).
    pub fn refresh_widgets(&self) -> Result<J, ZenError> {
        let mut out = serde_json::Map::new();
        for name in self.widget_names() {
            out.insert(widget_label(&name), self.refresh_widget(&name)?);
        }
        Ok(J::Object(out))
    }

    /// GET `/cli/zen/validate?widget=`: what is on disk now (no writes).
    pub fn validate_widget(&self, widget: &str) -> Result<J, ZenError> {
        self.require_set_up()?;
        if let Err(m) = parse_widget_ref(widget) {
            return Err(ZenError::BadRequest(m));
        }
        let Some((c, mut errors, mut warnings, _)) = self.check_now(widget) else {
            return Err(ZenError::NotFound(if widget.starts_with(builtin_widgets::BUILTIN_PREFIX) {
                format!("this K2 has no built-in widget '{widget}'")
            } else {
                missing_folder_message(widget)
            }));
        };
        errors.extend(c.errors.iter().cloned());
        warnings.extend(c.warnings.iter().cloned());
        Ok(json!({
            "ok": errors.is_empty(),
            "errors": errors,
            "warnings": warnings,
            "files": [widget_label(widget)],
        }))
    }

    /// Every folder's findings now (for `validate` with no target).
    pub fn validate_all_widgets(&self) -> (Vec<Diagnostic>, Vec<Diagnostic>, Vec<String>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut labels = Vec::new();
        for name in self.widget_names() {
            if let Some((c, e, w, _)) = self.check_now(&name) {
                errors.extend(e);
                errors.extend(c.errors.iter().cloned());
                warnings.extend(w);
                warnings.extend(c.warnings.iter().cloned());
                labels.push(widget_label(&name));
            }
        }
        (errors, warnings, labels)
    }

    /// GET `/cli/zen/history?widget=`.
    pub fn widget_history(&self, widget: &str) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = self.user_widget_name(widget)?;
        let last = self.last_good(&name).map(|(_, b, at)| json!({ "hash": b.hash, "at": at }));
        Ok(json!({
            "ok": true,
            "keep": HISTORY_KEEP,
            "files": [{
                "file": widget_label(&name),
                "widget": name,
                "exists": self.widget_dir(&name).is_dir(),
                "snapshots": self.widget_snapshots(&name),
                "lastGood": last,
            }],
        }))
    }

    /// POST `/cli/zen/reset {widget, to?}` (UW11): restore the code files
    /// (manifest, HTML, JS, CSS) of snapshot `to`, else of the newest clean
    /// snapshot. Assets are untouched. The code it replaces is kept as a
    /// snapshot first (marked clean or not).
    pub fn reset_widget(&self, widget: &str, to: Option<&str>) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = self.user_widget_name(widget)?;
        let snaps = self.widget_snapshots(&name);
        let pick = match to {
            Some(t) => snaps.iter().find(|s| s.name == t.trim()).cloned().ok_or_else(|| {
                ZenError::NotFound(format!(
                    "no snapshot '{t}' for {}; list them with k2 zen history --widget {name}",
                    widget_label(&name)
                ))
            })?,
            None => snaps.iter().find(|s| s.clean).cloned().ok_or_else(|| {
                ZenError::NotFound(format!("{} has no good version kept yet", widget_label(&name)))
            })?,
        };
        let restore = self
            .snapshot_files(&name, &pick.name)
            .ok_or_else(|| ZenError::Io(format!("can't read snapshot {}", pick.name)))?;
        let dir = self.widget_dir(&name);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let read = widgets::read_folder(&dir, &widget_label(&name));
        let current = widgets::code_files(&read.files);
        let mut kept = None;
        if !current.is_empty() && current != restore {
            let st = self.widget_status(&name);
            let clean = st.state == WidgetState::Ok;
            kept = Some(self.write_code_snapshot(&name, &current, clean, clean.then(|| st.hash().to_string()).as_deref())?);
        }
        for f in current.keys().filter(|f| !restore.contains_key(*f)) {
            let p = dir.join(f);
            fs::remove_file(&p).map_err(|e| io(e, &p))?;
        }
        for (f, b) in &restore {
            let p = dir.join(f);
            crate::fs_atomic::atomic_write(&p, b).map_err(|e| io(e, &p))?;
        }
        Ok(json!({ "ok": true, "file": widget_label(&name), "restored": pick.name, "snapshot": kept }))
    }

    /// POST `/cli/zen/widget/new {name, from?}` (UW35, UWB21): copy a
    /// starter (`hello`, the default; `arcade`) or a built-in
    /// (`k2:diary`, `k2:diary@1`) into `widgets/<name>/`.
    pub fn new_widget(&self, name: &str, from: Option<&str>) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = name.trim();
        if !super::valid_theme_name(name) {
            return Err(ZenError::BadRequest(format!(
                "'{name}' is not a widget name: lower-case letters, digits, - and _ (up to 40), starting with a letter or digit"
            )));
        }
        let from = from.map(str::trim).filter(|f| !f.is_empty()).unwrap_or("hello");
        let files: Vec<(String, String)> = if let Some(ex) = widgets::example(from) {
            ex.files.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
        } else {
            match parse_widget_ref(from) {
                Ok(r @ WidgetRef::Builtin { .. }) => {
                    let w = builtin_widgets::builtin_widget(&r)
                        .ok_or_else(|| ZenError::BadRequest(format!("this K2 has no built-in widget '{from}'")))?;
                    w.files.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
                }
                _ => {
                    return Err(ZenError::BadRequest(format!(
                        "unknown starter '{from}'; use hello, arcade or a built-in like k2:diary"
                    )))
                }
            }
        };
        let dir = self.widget_dir(name);
        if fs::symlink_metadata(&dir).is_ok() {
            return Err(ZenError::WidgetExists(format!(
                "{} already exists; edit it, or pick another name",
                widget_label(name)
            )));
        }
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let mut written = Vec::new();
        for (f, text) in files {
            let text = if f == widgets::MANIFEST_FILE { widgets::rename_manifest(&text, name) } else { text };
            let p = dir.join(&f);
            crate::fs_atomic::atomic_write_str(&p, &text).map_err(|e| io(e, &p))?;
            written.push(f);
        }
        written.sort();
        Ok(json!({
            "ok": true,
            "name": name,
            "from": from,
            "path": dir.display().to_string(),
            "files": written,
        }))
    }

    // ── placements and the resolved page ─────────────────────────────

    /// Every custom placement on every Garden's live page.
    pub fn custom_placements(&self) -> Vec<Placement> {
        let mut out = Vec::new();
        for g in self.gardens() {
            let f = ZenFile::Garden(g.id.clone());
            let eff = self.effective(&f, &self.base_for(&f));
            let page = super::garden_page(&eff.layer, &g.template);
            for w in page["widgets"].as_array().into_iter().flatten() {
                if w["kind"] != schema::CUSTOM_KIND {
                    continue;
                }
                out.push(Placement {
                    garden: g.id.clone(),
                    garden_name: g.name.clone(),
                    placement: w["id"].as_str().unwrap_or_default().to_string(),
                    widget: w["widget"].as_str().unwrap_or_default().to_string(),
                });
            }
        }
        out
    }

    /// Fill each custom widget on a resolved page with its folder's state
    /// (UW38): `name`, `description`, `reasons`, `libs`, `hash`, `state`,
    /// `errors`, `warnings`, `requested`, `origin`, the `caps` it runs with
    /// (every Garden-safe cap it asks for, no grant: [`effective_caps`]) and
    /// `paused` (the runaway guard's pause, or `null`).
    pub(crate) fn fill_custom_widgets(&self, garden: &str, page: &mut J, pauses: &WidgetPauses) {
        let Some(widgets) = page["widgets"].as_array_mut() else { return };
        for w in widgets.iter_mut().filter(|w| w["kind"] == schema::CUSTOM_KIND) {
            let widget = w["widget"].as_str().unwrap_or_default().to_string();
            let placement = w["id"].as_str().unwrap_or_default().to_string();
            let st = self.widget_status(&widget);
            let requested = st.requested();
            let origin = WidgetOrigin::of(&widget);
            let m = st.manifest.as_ref();
            w["name"] = json!(st.title());
            w["description"] = json!(m.and_then(|m| m.description.clone()));
            w["reasons"] = json!(m.map(|m| m.reasons.clone()).unwrap_or_default());
            w["libs"] = json!(m.map(|m| m.libs.clone()).unwrap_or_default());
            w["requested"] = json!(requested);
            w["hash"] = json!(st.hash());
            w["state"] = json!(st.state.as_str());
            w["errors"] = json!(st.errors);
            w["warnings"] = json!(st.warnings);
            w["origin"] = json!(origin);
            w["caps"] = json!(effective_caps(origin, &requested));
            w["paused"] = json!(pauses.get(garden, &placement));
        }
    }

    /// Garden-file findings that need the folders (store-level, so a
    /// deleted folder never rolls a Garden back to an old snapshot): a
    /// placement naming a missing folder or an unknown built-in, and a
    /// Conversation following a custom widget that doesn't ask for
    /// `agents:read` (UW40). `file` labels them; `positions` are the
    /// Garden file's.
    pub fn placement_diagnostics(
        &self,
        file: &str,
        page: &J,
        file_widgets: Option<&J>,
        positions: &BTreeMap<String, (usize, usize)>,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        // Positions only for widgets the file itself declares.
        let index_of = |id: &str| -> Option<usize> {
            file_widgets?.as_array()?.iter().position(|w| w["id"] == id)
        };
        let at = |key: String| positions.get(&key).copied().unwrap_or((1, 1));
        let widgets = page["widgets"].as_array().cloned().unwrap_or_default();
        for w in &widgets {
            if w["kind"] != schema::CUSTOM_KIND {
                continue;
            }
            let id = w["id"].as_str().unwrap_or_default();
            let widget = w["widget"].as_str().unwrap_or_default();
            // Only what the file itself declares is the file's finding; a
            // template's widget shows its own state on the page.
            let Some(i) = index_of(id) else { continue };
            if self.check_now(widget).is_none() {
                let (line, col) = at(format!("page.widget.{i}.widget"));
                let message = if widget.starts_with(builtin_widgets::BUILTIN_PREFIX) {
                    format!("this K2 has no built-in widget '{widget}'; update K2, or name one it has")
                } else {
                    missing_folder_message(widget)
                };
                out.push(Diagnostic { file: file.to_string(), line, col, message });
            }
        }
        for w in &widgets {
            if w["kind"] != "conversation" {
                continue;
            }
            let Some(target) = w["props"]["agents"].as_str() else { continue };
            let Some(custom) = widgets.iter().find(|x| x["id"] == target && x["kind"] == schema::CUSTOM_KIND) else {
                continue;
            };
            let st = self.widget_status(custom["widget"].as_str().unwrap_or_default());
            let id = w["id"].as_str().unwrap_or_default();
            let Some(i) = index_of(id) else { continue };
            if st.manifest.is_some() && !st.requested().iter().any(|c| c == "agents:read") {
                let (line, col) = at(format!("page.widget.{i}.props.agents"));
                out.push(Diagnostic {
                    file: file.to_string(),
                    line,
                    col,
                    message: format!(
                        "conversation '{id}' follows custom widget '{target}', which doesn't ask for agents:read; add it to the widget's manifest.json caps"
                    ),
                });
            }
        }
        out
    }

    /// GET `/cli/zen/widgets` (UW35): every folder with its state and each
    /// placement (and whether the runaway guard paused it).
    pub fn widgets_json(&self, pauses: &WidgetPauses) -> Result<J, ZenError> {
        self.require_set_up()?;
        let placements = self.custom_placements();
        let mut names = self.widget_names();
        for p in &placements {
            if p.widget.starts_with(builtin_widgets::BUILTIN_PREFIX) && !names.contains(&p.widget) {
                names.push(p.widget.clone());
            }
        }
        let out: Vec<J> = names
            .iter()
            .map(|name| {
                let st = self.widget_status(name);
                let requested = st.requested();
                let mine: Vec<J> = placements
                    .iter()
                    .filter(|p| &p.widget == name)
                    .map(|p| {
                        json!({
                            "garden": p.garden,
                            "gardenName": p.garden_name,
                            "placement": p.placement,
                            "paused": pauses.get(&p.garden, &p.placement),
                        })
                    })
                    .collect();
                let m = st.manifest.as_ref();
                json!({
                    "name": name,
                    "builtin": st.builtin,
                    "title": st.title(),
                    "description": m.and_then(|m| m.description.clone()),
                    "caps": requested,
                    "hash": st.hash(),
                    "state": st.state.as_str(),
                    "errors": st.errors,
                    "warnings": st.warnings,
                    "placements": mine,
                })
            })
            .collect();
        Ok(json!({ "ok": true, "widgets": out }))
    }
}

#[cfg(test)]
#[path = "widget_store_tests.rs"]
mod tests;
