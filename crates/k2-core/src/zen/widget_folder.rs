//! The widget dev loop (0.46.1 S3): a whole staged widget folder, checked
//! and written in one call through the daemon, so an agent never touches
//! `~/.k2`.
//!
//! The CLI reads a folder anywhere on disk (`k2 zen widget put <name> --dir
//! <path>`, `k2 zen validate --dir <path>`, `k2 zen widget preview --dir
//! <path>`) and sends its files: code files (`.html` `.js` `.css` `.json`)
//! as text, images and fonts as base64. The daemon never reads the staged
//! path itself.
//!
//! - [`check_staged`] checks a staged folder exactly as a widget folder is
//!   checked (`widgets::check_files`: manifest, folder rules, bundle). It
//!   writes nothing and needs no Zen folder.
//! - [`ZenFiles::put_widget_folder`] writes it into `widgets/<name>/` only
//!   when it checks clean (a folder with errors is refused and nothing is
//!   written, unlike a one-file put). Files the staged folder doesn't have
//!   are left alone, or removed with `prune` (code files, images and fonts
//!   only; dotfiles, README and LICENSE are never touched).
//! - [`ZenFiles::preview_widget_folder`] writes the folder as
//!   `widgets/preview-<name>/` and places it alone on K2's own preview
//!   Garden, "Widget preview" (made when missing, never one of the person's
//!   Gardens), with the placement config the caller gives (for example the
//!   source addresses it reads). [`ZenFiles::clear_widget_previews`] removes
//!   that Garden and every `preview-*` folder.

use std::collections::BTreeMap;
use std::fs;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value as J};

use super::agent_edit::{text_hash, valid_widget_file};
use super::builtin_widgets::BUILTIN_PREFIX;
use super::bundle::asset_mime;
use super::schema::Diagnostic;
use super::store::{ZenError, ZenFiles};
use super::widgets::{self, MANIFEST_FILE};

/// At most this many files in one staged folder.
pub const MAX_STAGED_FILES: usize = 64;
/// At most this many bytes (decoded) in one staged folder: the widget's
/// code plus its 2 MB of images and fonts, with room to spare.
pub const MAX_STAGED_BYTES: usize = 4 * 1024 * 1024;
/// A folder request body is at most this (base64 grows assets by a third).
pub const MAX_FOLDER_BODY: usize = 6 * 1024 * 1024;
/// The preview Garden's name. K2 makes it; it is never one of the
/// person's own Gardens.
pub const PREVIEW_GARDEN: &str = "Widget preview";
/// Preview folders are `widgets/preview-<name>/`.
pub const PREVIEW_PREFIX: &str = "preview-";
/// The placement id on the preview Garden.
const PREVIEW_PLACEMENT: &str = "preview";
/// At most this many config keys on a preview placement.
const MAX_PREVIEW_CONFIG: usize = 16;

/// One file of a staged folder as the CLI sends it: `text` for a code file,
/// `base64` for an image or font. Exactly one of them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedFile {
    pub name: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub base64: Option<String>,
}

/// An image or font name a widget folder may hold: one segment, letters,
/// digits, `-` `_` `.`, a known asset extension. SVG is refused by the
/// check itself (with its own message), so it passes here.
fn valid_asset_file(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && !name.contains("..")
        && (asset_mime(&name.to_ascii_lowercase()).is_some() || name.to_ascii_lowercase().ends_with(".svg"))
}

/// The staged files as the bytes `check_files` reads, after the name and
/// size rules. A code file must be text; an asset must be base64.
pub fn staged_files(list: &[StagedFile]) -> Result<BTreeMap<String, Vec<u8>>, ZenError> {
    if list.is_empty() {
        return Err(ZenError::BadRequest("the staged folder has no files (a widget needs at least manifest.json and its entry .html)".into()));
    }
    if list.len() > MAX_STAGED_FILES {
        return Err(ZenError::BadRequest(format!("a staged widget folder has at most {MAX_STAGED_FILES} files")));
    }
    let mut out = BTreeMap::new();
    let mut total = 0usize;
    for f in list {
        let name = f.name.as_str();
        let code = valid_widget_file(name);
        if !code && !valid_asset_file(name) {
            return Err(ZenError::BadRequest(format!(
                "'{name}' can't be in a widget folder: code files are one lower-case name ending .html, .js, .css or .json; images and fonts end .png .jpg .jpeg .gif .webp .woff2 (no folders, no dotfiles)"
            )));
        }
        let bytes = match (&f.text, &f.base64, code) {
            (Some(t), None, true) => t.as_bytes().to_vec(),
            (None, Some(b), false) => base64::engine::general_purpose::STANDARD
                .decode(b.as_bytes())
                .map_err(|e| ZenError::BadRequest(format!("'{name}' isn't valid base64: {e}")))?,
            (_, _, true) => return Err(ZenError::BadRequest(format!("'{name}' is a code file: send it as text"))),
            (_, _, false) => return Err(ZenError::BadRequest(format!("'{name}' is an image or font: send it as base64"))),
        };
        total += bytes.len();
        if total > MAX_STAGED_BYTES {
            return Err(ZenError::BadRequest(format!("a staged widget folder is at most {} MB", MAX_STAGED_BYTES / (1024 * 1024))));
        }
        if out.insert(name.to_string(), bytes).is_some() {
            return Err(ZenError::BadRequest(format!("'{name}' is in the staged folder twice")));
        }
    }
    Ok(out)
}

/// The widget name a staged folder checks as: the one given, else its
/// manifest's `id`, else `fallback` (the CLI sends the staged folder's own
/// name). Built-ins (`k2:`) are never a staged name.
pub fn staged_name(given: Option<&str>, files: &BTreeMap<String, Vec<u8>>, fallback: Option<&str>) -> Result<String, ZenError> {
    let given = given.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let from_manifest = || {
        files
            .get(MANIFEST_FILE)
            .and_then(|b| serde_json::from_slice::<J>(b).ok())
            .and_then(|v| v["id"].as_str().map(str::to_string))
    };
    let name = given
        .or_else(from_manifest)
        .or_else(|| fallback.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string))
        .ok_or_else(|| ZenError::BadRequest("name the widget (its folder name), or give manifest.json an \"id\"".into()))?;
    if name.starts_with(BUILTIN_PREFIX) {
        return Err(ZenError::BadRequest(format!(
            "'{name}' is built into K2 and can't be changed; copy it with k2 zen widget new <name> --from {name}"
        )));
    }
    if !super::valid_theme_name(&name) {
        return Err(ZenError::BadRequest(format!(
            "'{name}' is not a widget name: lower-case letters, digits, - and _ (up to 40), starting with a letter or digit"
        )));
    }
    Ok(name)
}

/// Check a staged folder as widget `name` (the label is `staged/<name>`).
/// Writes nothing. The answer is shaped like `k2 zen validate --widget`'s:
/// `{ok, status: ok|failed, errors, warnings, files, widget, manifest}`.
/// A staged folder hasn't run anywhere, so there's no runtime and never a
/// "pending".
pub fn check_staged(name: &str, files: &BTreeMap<String, Vec<u8>>) -> J {
    let prefix = format!("staged/{name}");
    let c = widgets::check_files(&prefix, name, files);
    let clean = c.is_clean();
    json!({
        "ok": clean,
        "status": if clean { "ok" } else { "failed" },
        "widget": name,
        "errors": c.errors,
        "warnings": c.warnings,
        "files": [prefix],
        "staged": files.keys().collect::<Vec<_>>(),
        "manifest": c.manifest.as_ref().map(widgets::manifest_json),
        "bundleHash": c.bundle.as_ref().map(|b| b.hash.clone()),
    })
}

fn render_errors(errors: &[Diagnostic]) -> String {
    errors.iter().take(10).map(Diagnostic::render).collect::<Vec<_>>().join(" | ")
}

/// Is `name` a file a widget folder's prune may remove (code, image, font)?
fn prunable(name: &str) -> bool {
    valid_widget_file(name) || valid_asset_file(name)
}

impl ZenFiles {
    /// Check a staged folder for a write into `widgets/<name>/`: the folder
    /// name (a user widget, never a built-in) once it checks clean, else
    /// 400 listing the errors (at most 10).
    pub fn check_widget_folder_put(&self, widget: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<String, ZenError> {
        self.require()?;
        let name = self.user_widget_name(widget)?;
        let name = staged_name(Some(&name), files, None)?;
        let c = widgets::check_files(&format!("staged/{name}"), &name, files);
        if !c.is_clean() {
            let errors = if c.errors.is_empty() {
                "the folder doesn't bundle".to_string()
            } else {
                render_errors(&c.errors)
            };
            return Err(ZenError::BadRequest(format!("the folder has errors, nothing was written: {errors}")));
        }
        Ok(name)
    }

    /// The text of each staged code file that differs from what is on disk
    /// (`None` on disk: a new file), for a proposal per file.
    pub fn widget_folder_changes(&self, name: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<(String, String, Option<String>)>, ZenError> {
        let mut out = Vec::new();
        for (file, bytes) in files {
            let current = match fs::read(self.widget_dir(name).join(file)) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(ZenError::Io(format!("{}: {e}", self.widget_dir(name).join(file).display()))),
            };
            if current.as_deref() == Some(bytes.as_slice()) {
                continue;
            }
            if !valid_widget_file(file) {
                return Err(ZenError::BadRequest(format!(
                    "'{file}' is an image or font: only a person, or an agent with \"Manage Gardens and data sources\", can add or change one (proposals hold code files)"
                )));
            }
            let text = String::from_utf8(bytes.clone()).map_err(|_| ZenError::BadRequest(format!("{file} is not UTF-8 text")))?;
            let current = match current {
                None => None,
                Some(b) => Some(String::from_utf8(b).map_err(|_| ZenError::BadRequest(format!("{file} on disk is not UTF-8 text")))?),
            };
            out.push((file.clone(), text, current));
        }
        Ok(out)
    }

    /// Write a staged folder into `widgets/<name>/` (made if missing) after
    /// [`Self::check_widget_folder_put`]. Each file is written atomically;
    /// unchanged files aren't rewritten. `prune` removes code files, images
    /// and fonts the staged folder doesn't have. The answer carries
    /// `validate` (the folder on disk now), `written`, `unchanged` and
    /// `removed` (with prune) or `kept` (files left alone without it).
    pub fn put_widget_folder(&self, widget: &str, files: &BTreeMap<String, Vec<u8>>, prune: bool) -> Result<J, ZenError> {
        let name = self.check_widget_folder_put(widget, files)?;
        let dir = self.widget_dir(&name);
        if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ZenError::BadRequest(format!("{} is a link; K2 writes only real widget folders", widgets::WIDGETS_DIR)));
        }
        fs::create_dir_all(&dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
        let mut written = Vec::new();
        let mut unchanged = Vec::new();
        for (file, bytes) in files {
            let p = dir.join(file);
            if fs::read(&p).ok().as_deref() == Some(bytes.as_slice()) {
                unchanged.push(file.clone());
                continue;
            }
            crate::fs_atomic::atomic_write(&p, bytes).map_err(|e| ZenError::Io(format!("{}: {e}", p.display())))?;
            written.push(file.clone());
        }
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        let mut extra: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?
            .flatten()
            .filter(|e| fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_file()))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !files.contains_key(n) && prunable(n))
            .collect();
        extra.sort();
        for n in extra {
            if prune {
                let p = dir.join(&n);
                fs::remove_file(&p).map_err(|e| ZenError::Io(format!("{}: {e}", p.display())))?;
                removed.push(n);
            } else {
                kept.push(n);
            }
        }
        let validate = self.validate_widget(&name).unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }));
        let manifest_hash = files.get(MANIFEST_FILE).map(|b| text_hash(std::str::from_utf8(b).ok()));
        Ok(json!({
            "ok": true,
            "widget": name,
            "file": format!("{}/{name}", widgets::WIDGETS_DIR),
            "changed": !written.is_empty() || !removed.is_empty(),
            "written": written,
            "unchanged": unchanged,
            "removed": removed,
            "kept": kept,
            "manifestHash": manifest_hash,
            "validate": validate,
        }))
    }

    /// The preview folder name for a widget: `preview-<name>`.
    pub fn preview_widget_name(name: &str) -> Result<String, ZenError> {
        let base = name.strip_prefix(PREVIEW_PREFIX).unwrap_or(name);
        let p = format!("{PREVIEW_PREFIX}{base}");
        if !super::valid_theme_name(&p) {
            return Err(ZenError::BadRequest(format!(
                "'{base}' is too long to preview: a preview folder is preview-<name>, at most 40 characters"
            )));
        }
        Ok(p)
    }

    /// The preview Garden's page: the preview widget alone, full width,
    /// with `config` on its placement.
    pub fn preview_page(folder: &str, config: &BTreeMap<String, String>) -> String {
        let mut cfg = String::new();
        for (i, (k, v)) in config.iter().enumerate() {
            if i > 0 {
                cfg.push_str(", ");
            }
            cfg.push_str(&format!("{k} = {}", toml_string(v)));
        }
        format!(
            "# K2's widget preview (k2 zen widget preview). K2 rewrites this page on\n\
             # every preview and removes it with k2 zen widget preview --clear.\n\
             schema = 1\n\
             template = \"{blank}\"\n\
             \n\
             [layout]\n\
             kind = \"columns\"\n\
             [[layout.column]]\n\
             size = 100\n\
             min-width = 240\n\
             \n\
             [[widget]]\n\
             id = \"{PREVIEW_PLACEMENT}\"\n\
             kind = \"custom\"\n\
             widget = \"{folder}\"\n\
             column = 0\n\
             [widget.props]\n\
             config = {{ {cfg} }}\n",
            blank = super::schema::BLANK_TEMPLATE_ID,
        )
    }

    /// Stage `files` as `widgets/preview-<name>/` (its manifest `id`
    /// renamed to match, pruned to the staged files) and place it alone on
    /// K2's "Widget preview" Garden (made when missing) with `config`.
    /// `by_person`: the person may preview onto a locked preview Garden.
    pub fn preview_widget_folder(
        &self,
        widget: Option<&str>,
        fallback: Option<&str>,
        files: &BTreeMap<String, Vec<u8>>,
        config: &BTreeMap<String, String>,
        by_person: bool,
    ) -> Result<J, ZenError> {
        self.require()?;
        if config.len() > MAX_PREVIEW_CONFIG {
            return Err(ZenError::BadRequest(format!("a preview takes at most {MAX_PREVIEW_CONFIG} config keys")));
        }
        for (k, v) in config {
            let ok_key = !k.is_empty()
                && k.len() <= 40
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
            if !ok_key {
                return Err(ZenError::BadRequest(format!("'{k}' is not a config key (letters, digits, - and _)")));
            }
            if v.chars().count() > 500 || v.chars().any(char::is_control) {
                return Err(ZenError::BadRequest(format!("config {k} is one line of at most 500 characters")));
            }
        }
        let name = staged_name(widget, files, fallback)?;
        let folder = Self::preview_widget_name(&name)?;
        let mut staged = files.clone();
        if let Some(m) = staged.get(MANIFEST_FILE).and_then(|b| std::str::from_utf8(b).ok()) {
            let renamed = widgets::rename_manifest(m, &folder);
            staged.insert(MANIFEST_FILE.to_string(), renamed.into_bytes());
        }
        let put = self.put_widget_folder(&folder, &staged, true)?;
        let garden = match self.find_garden(PREVIEW_GARDEN) {
            Some((_, g)) => g,
            None => self.new_garden(PREVIEW_GARDEN, None, None, None)?,
        };
        let page = Self::preview_page(&folder, config);
        let placed = self.put_garden(&garden.id, &page, by_person)?;
        Ok(json!({
            "ok": true,
            "widget": name,
            "preview": folder,
            "garden": { "id": garden.id, "name": garden.name },
            "placement": PREVIEW_PLACEMENT,
            "config": config,
            "written": put["written"],
            "validate": put["validate"],
            "page": placed["file"],
            "open": format!(
                "Open Zen (Ctrl+Cmd+Z) and switch to the \"{PREVIEW_GARDEN}\" Garden. Runtime errors: k2 zen widget errors {folder}. Remove it: k2 zen widget preview --clear."
            ),
        }))
    }

    /// Remove K2's preview Garden (kept in `.history/` like any deleted
    /// Garden) and every `widgets/preview-*` folder.
    pub fn clear_widget_previews(&self) -> Result<J, ZenError> {
        self.require()?;
        let garden = match self.find_garden(PREVIEW_GARDEN) {
            Some((_, g)) => match self.delete_garden(&g.id) {
                Ok(_) => Some(g.id),
                // The preview is the only Garden left: keep it, clear the rest.
                Err(ZenError::LastGarden) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        let mut removed = Vec::new();
        for n in self.widget_names().into_iter().filter(|n| n.starts_with(PREVIEW_PREFIX)) {
            let dir = self.widget_dir(&n);
            if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                fs::remove_dir_all(&dir).map_err(|e| ZenError::Io(format!("{}: {e}", dir.display())))?;
                removed.push(n);
            }
        }
        Ok(json!({ "ok": true, "garden": garden, "removed": removed }))
    }
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "widget_folder_tests.rs"]
mod tests;
