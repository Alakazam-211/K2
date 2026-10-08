//! Custom widget folders (prd-zen-user-widgets-v2 UW4–UW12, UWA7, UWB13,
//! UWB17, UWB21).
//!
//! A user widget is one flat folder, `~/.k2/zen/widgets/<name>/`:
//! `manifest.json`, the entry HTML, `*.js`, `*.css` and image and font
//! assets. A built-in widget (`k2:<name>@<n>`, `builtin_widgets`) is the
//! same file set compiled into K2. Both go through [`check_files`]: the
//! manifest (strict, the shared schema 1), the folder rules (flat, no
//! links, sizes, real image types, no SVG) and the bundler
//! (`zen::bundle`).
//!
//! What the daemon keeps (UW11) lives in `~/.k2/zen/.history/widgets/<name>/`
//! and is written by `ZenFiles` (`store`): the last good bundle and the
//! last 20 good versions of the code files.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value as J};

use super::bundle::{self, asset_mime, sniff_asset, Bundle};
use super::schema::Diagnostic;
use super::stdlib::{self, LibRef};

/// `~/.k2/zen/widgets/`.
pub const WIDGETS_DIR: &str = "widgets";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const DEFAULT_ENTRY: &str = "index.html";
/// Manifest `schema` this K2 reads (the shared schema, UWA7).
pub const MANIFEST_SCHEMA: u64 = 1;
/// `name` is 1 to this many characters (UW5).
pub const MAX_NAME_CHARS: usize = 40;
pub const MAX_DESCRIPTION_CHARS: usize = 200;
pub const MAX_REASON_CHARS: usize = 140;
/// One asset (UW8).
pub const MAX_ASSET_BYTES: u64 = 1024 * 1024;
/// All assets of one widget (UW8).
pub const MAX_ASSETS_BYTES: u64 = 2 * 1024 * 1024;
/// Any one file K2 reads from a widget folder (a guard before reading).
pub const MAX_FILE_BYTES: u64 = 3 * 1024 * 1024;
/// Code files: copied into history and restored by `reset --widget`.
pub const CODE_EXTS: &[&str] = &["html", "js", "css", "json"];

/// The `/boot-status` feature keys a manifest's `requires.features` may
/// name without a warning: the catalog's verb features plus these (the
/// daemon test keeps this list inside its `features()`).
pub const KNOWN_FEATURES: &[&str] = &[
    "zen-v1",
    "zen-gardens-v1",
    "zen-chrome-v1",
    "zen-widgets-v1",
    "zen-lib-v1",
    "zen-sync-v1",
    "thread-latest",
    "thread-widget-origin",
    "thread-address-stable",
    "daemon-activity",
];

/// A widget's manifest after checking (UW5, UWA7, UWB13).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u64,
    pub name: String,
    pub description: Option<String>,
    pub entry: String,
    /// The caps it asks for, each in `USER_WIDGET_CAPS`, no repeats.
    pub caps: Vec<String>,
    pub reasons: BTreeMap<String, String>,
    /// `requires.libs`, as written (strings or `{url, integrity}`), checked.
    pub libs: Vec<J>,
    /// `requires.features`.
    pub features: Vec<String>,
}

impl Manifest {
    /// Read back what [`Manifest`]'s JSON wrote (`last-good.json`).
    pub fn from_saved(v: &J) -> Option<Self> {
        Some(Self {
            schema: v["schema"].as_u64()?,
            name: v["name"].as_str()?.to_string(),
            description: v["description"].as_str().map(str::to_string),
            entry: v["entry"].as_str()?.to_string(),
            caps: v["caps"].as_array()?.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<_>>()?,
            reasons: v["reasons"]
                .as_object()?
                .iter()
                .map(|(k, r)| r.as_str().map(|r| (k.clone(), r.to_string())))
                .collect::<Option<_>>()?,
            libs: v["libs"].as_array()?.clone(),
            features: v["features"]
                .as_array()?
                .iter()
                .map(|c| c.as_str().map(str::to_string))
                .collect::<Option<_>>()?,
        })
    }
}

// ── JSON key positions (for errors at a key's line and column) ──────────

/// `(line, col)` of each object key in `src`, by dotted path
/// (`requires.libs`, `bindings.main.type`). A tiny scanner: `src` already
/// parsed with serde_json, so it is valid JSON.
fn key_positions(src: &str) -> BTreeMap<String, (usize, usize)> {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
        line: usize,
        col_start: usize,
        out: BTreeMap<String, (usize, usize)>,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                if self.b[self.i] == b'\n' {
                    self.line += 1;
                    self.col_start = self.i + 1;
                }
                self.i += 1;
            }
        }
        fn pos(&self) -> (usize, usize) {
            let col = String::from_utf8_lossy(&self.b[self.col_start..self.i]).chars().count() + 1;
            (self.line, col)
        }
        fn string(&mut self) -> String {
            let start = self.i + 1;
            self.i += 1;
            while self.i < self.b.len() && self.b[self.i] != b'"' {
                if self.b[self.i] == b'\\' {
                    self.i += 1;
                }
                self.i += 1;
            }
            let s = String::from_utf8_lossy(&self.b[start..self.i.min(self.b.len())]).to_string();
            self.i += 1;
            s
        }
        fn value(&mut self, path: &str) {
            self.ws();
            match self.b.get(self.i) {
                Some(b'{') => {
                    self.i += 1;
                    loop {
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b'}') | None => {
                                self.i += 1;
                                return;
                            }
                            Some(b',') => self.i += 1,
                            Some(b'"') => {
                                let at = self.pos();
                                let k = self.string();
                                let p = if path.is_empty() { k } else { format!("{path}.{k}") };
                                self.out.entry(p.clone()).or_insert(at);
                                self.ws();
                                self.i += 1; // ':'
                                self.value(&p);
                            }
                            Some(_) => self.i += 1,
                        }
                    }
                }
                Some(b'[') => {
                    self.i += 1;
                    let mut n = 0;
                    loop {
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b']') | None => {
                                self.i += 1;
                                return;
                            }
                            Some(b',') => {
                                self.i += 1;
                                n += 1;
                            }
                            Some(_) => {
                                let p = format!("{path}.{n}");
                                let at = self.pos();
                                self.out.entry(p.clone()).or_insert(at);
                                self.value(&p);
                            }
                        }
                    }
                }
                Some(b'"') => {
                    self.string();
                }
                Some(_) => {
                    while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']') && !self.b[self.i].is_ascii_whitespace() {
                        self.i += 1;
                    }
                }
                None => {}
            }
        }
    }
    let mut p = P { b: src.as_bytes(), i: 0, line: 1, col_start: 0, out: BTreeMap::new() };
    p.value("");
    p.out
}

// ── The manifest (UW5, UWA7, UWB13) ─────────────────────────────────────

/// Keys UWA7 reserves for later: accepted with one warning, type-checked.
const RESERVED_KEYS: &[(&str, &str)] = &[("version", "string"), ("license", "string"), ("bindings", "object")];
/// Keys of a package root, refused in a widget folder (UWA7).
const ROOT_KEYS: &[(&str, &str)] = &[
    ("kind", "a Garden bundle's or app package's root manifest"),
    ("contents", "a Garden bundle's root manifest"),
    ("host", "an app package's root manifest"),
    ("roles", "an app package's root manifest"),
];

/// Parse and check `manifest.json`. `folder` is the folder name (or the
/// built-in's name) `id` must match.
pub fn parse_manifest(file: &str, src: &str, folder: &str) -> (Option<Manifest>, Vec<Diagnostic>, Vec<Diagnostic>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let d = |line: usize, col: usize, message: String| Diagnostic { file: file.to_string(), line, col, message };
    let src = src.trim_start_matches('\u{feff}');
    let v: J = match serde_json::from_str(src) {
        Ok(v) => v,
        Err(e) => {
            errors.push(d(e.line().max(1), e.column().max(1), format!("manifest.json isn't valid JSON: {e}")));
            return (None, errors, warnings);
        }
    };
    let Some(obj) = v.as_object() else {
        errors.push(d(1, 1, "manifest.json must be one JSON object".into()));
        return (None, errors, warnings);
    };
    let pos = key_positions(src);
    let at = |k: &str| pos.get(k).copied().unwrap_or((1, 1));
    let mut err = |k: &str, m: String| {
        let (l, c) = at(k);
        errors.push(d(l, c, m));
    };
    let user_caps = super::USER_WIDGET_CAPS;

    let mut name = None;
    let mut description = None;
    let mut entry = DEFAULT_ENTRY.to_string();
    let mut caps: Vec<String> = Vec::new();
    let mut reasons: BTreeMap<String, String> = BTreeMap::new();
    let mut libs: Vec<J> = Vec::new();
    let mut features: Vec<String> = Vec::new();
    let mut saw_schema = false;

    for (k, val) in obj {
        match k.as_str() {
            "schema" => {
                saw_schema = true;
                if val.as_u64() != Some(MANIFEST_SCHEMA) {
                    err(k, format!("schema must be {MANIFEST_SCHEMA}; this K2 reads widget manifest schema 1"));
                }
            }
            "name" => match val.as_str().map(str::trim) {
                Some(n) if !n.is_empty() && n.chars().count() <= MAX_NAME_CHARS && !n.chars().any(char::is_control) => {
                    name = Some(n.to_string())
                }
                _ => err(k, format!("name must be 1 to {MAX_NAME_CHARS} characters of text, no control characters")),
            },
            "description" => match val.as_str() {
                Some(s) if s.chars().count() <= MAX_DESCRIPTION_CHARS && !s.chars().any(|c| c.is_control() && c != '\n') => {
                    description = Some(s.to_string())
                }
                _ => err(k, format!("description must be text of at most {MAX_DESCRIPTION_CHARS} characters")),
            },
            "entry" => match val.as_str() {
                Some(e) if !e.is_empty() && !e.contains(['/', '\\']) && !e.starts_with('.') && e.to_ascii_lowercase().ends_with(".html") => {
                    entry = e.to_string()
                }
                _ => err(k, "entry must be an .html file name in the widget's folder (no folders)".into()),
            },
            "id" => match val.as_str() {
                Some(id) if id == folder => {}
                Some(id) => err(k, format!("id '{id}' doesn't match the folder '{folder}'")),
                None => err(k, "id must be the folder name (a string)".into()),
            },
            "caps" => match val.as_array() {
                Some(list) => {
                    for (i, c) in list.iter().enumerate() {
                        let p = format!("caps.{i}");
                        match c.as_str() {
                            Some(c) if !user_caps.contains(&c) => {
                                err(&p, format!("'{c}' isn't available to custom widgets; they may ask for {}", user_caps.join(", ")))
                            }
                            Some(c) if caps.iter().any(|x| x == c) => err(&p, format!("'{c}' is listed twice")),
                            Some(c) => caps.push(c.to_string()),
                            None => err(&p, "each cap is a string".into()),
                        }
                    }
                }
                None => err(k, format!("caps must be a list of: {}", user_caps.join(", "))),
            },
            "reasons" => match val.as_object() {
                Some(m) => {
                    for (cap, r) in m {
                        let p = format!("reasons.{cap}");
                        match r.as_str() {
                            Some(r) if r.chars().count() <= MAX_REASON_CHARS && !r.chars().any(char::is_control) => {
                                reasons.insert(cap.clone(), r.to_string());
                            }
                            _ => err(&p, format!("each reason is one line of at most {MAX_REASON_CHARS} characters")),
                        }
                    }
                }
                None => err(k, "reasons must be an object: {\"<cap>\": \"why it needs it\"}".into()),
            },
            "requires" => match val.as_object() {
                Some(m) => {
                    for (rk, rv) in m {
                        let p = format!("requires.{rk}");
                        match rk.as_str() {
                            "features" => match rv.as_array().and_then(|a| a.iter().map(J::as_str).collect::<Option<Vec<_>>>()) {
                                Some(list) => {
                                    let catalog_features: Vec<&str> =
                                        crate::contract::catalog().verbs.iter().map(|v| v.feature.as_str()).collect();
                                    for (i, f) in list.iter().enumerate() {
                                        if !KNOWN_FEATURES.contains(f) && !catalog_features.contains(f) {
                                            let (l, c) = at(&format!("{p}.{i}"));
                                            warnings.push(d(
                                                l,
                                                c,
                                                format!("no K2 feature is called '{f}'; copy the key from the verb's row in k2 zen guide api"),
                                            ));
                                        }
                                        features.push(f.to_string());
                                    }
                                }
                                None => err(&p, "requires.features is a list of feature keys (strings)".into()),
                            },
                            "libs" => match rv.as_array() {
                                Some(list) => {
                                    let m = stdlib::zen_lib_manifest();
                                    let mut bytes: u64 = 0;
                                    for (i, item) in list.iter().enumerate() {
                                        let ip = format!("{p}.{i}");
                                        match LibRef::from_json(item) {
                                            Err(e) => err(&ip, e),
                                            Ok(r @ LibRef::Named { .. }) => match stdlib::find(m, &r) {
                                                Some(e) => {
                                                    if libs.iter().any(|x| x == item) {
                                                        err(&ip, format!("'{}' is listed twice", e.key()));
                                                    } else {
                                                        bytes += e.total_bytes();
                                                        libs.push(item.clone());
                                                    }
                                                }
                                                None => err(
                                                    &ip,
                                                    format!(
                                                        "K2's widget library has no {}; see k2 zen guide libs for what's available",
                                                        item.as_str().unwrap_or("such library")
                                                    ),
                                                ),
                                            },
                                            Ok(LibRef::Cdn { url, integrity }) => match stdlib::check_cdn(m, &url, &integrity) {
                                                Ok(()) => libs.push(item.clone()),
                                                Err(e) => err(&ip, e),
                                            },
                                        }
                                    }
                                    if bytes > m.max_bytes_per_widget {
                                        err(
                                            &p,
                                            format!(
                                                "these libraries are {bytes} bytes; a widget may load at most {} (8 MB)",
                                                m.max_bytes_per_widget
                                            ),
                                        );
                                    }
                                }
                                None => err(&p, "requires.libs is a list: \"<id>@<version>\" or {url, integrity}".into()),
                            },
                            other => {
                                let (l, c) = at(&p);
                                warnings.push(d(l, c, format!("requires.{other} is read by a later K2; this version ignores it")));
                            }
                        }
                    }
                }
                None => err(k, "requires must be an object: {features?, libs?}".into()),
            },
            "net" | "secrets" => {
                err(k, format!("'{k}' isn't part of manifest schema 1; outside data comes in a later version"))
            }
            other => {
                if let Some((_, ty)) = RESERVED_KEYS.iter().find(|(r, _)| *r == other) {
                    let ok = match *ty {
                        "string" => val.is_string(),
                        _ => val.is_object(),
                    };
                    if !ok {
                        err(k, format!("'{other}' must be a{} {ty}", if *ty == "object" { "n" } else { "" }));
                        continue;
                    }
                    if other == "bindings" {
                        let mut bad = false;
                        for (slot, sv) in val.as_object().into_iter().flatten() {
                            let ty = sv.get("type").and_then(J::as_str);
                            if ty != Some("home") {
                                let p = if sv.get("type").is_some() { format!("bindings.{slot}.type") } else { format!("bindings.{slot}") };
                                err(
                                    &p,
                                    format!(
                                        "a widget's binding slot is type \"home\" (a Home, or one agent in it); '{}' isn't one",
                                        ty.unwrap_or("(none)")
                                    ),
                                );
                                bad = true;
                            }
                        }
                        if bad {
                            continue;
                        }
                    }
                    let (l, c) = at(k);
                    warnings.push(d(l, c, format!("'{other}' is read when Gardens are shared; this version ignores it")));
                } else if let Some((_, whose)) = ROOT_KEYS.iter().find(|(r, _)| *r == other) {
                    err(k, format!("'{other}' belongs in {whose}, not in a widget folder"));
                } else {
                    err(
                        k,
                        format!(
                            "unknown key '{other}' in manifest.json; use schema, name, description, entry, caps, reasons, id, requires"
                        ),
                    );
                }
            }
        }
    }
    if !saw_schema {
        errors.push(d(1, 1, "manifest.json needs \"schema\": 1".into()));
    }
    if name.is_none() && !obj.contains_key("name") {
        errors.push(d(1, 1, "manifest.json needs a \"name\" (shown in K2's review dialog as the widget's own words)".into()));
    }
    for cap in reasons.keys() {
        if !caps.contains(cap) {
            let (l, c) = at(&format!("reasons.{cap}"));
            errors.push(d(l, c, format!("reasons names '{cap}', which caps doesn't ask for")));
        }
    }
    if !errors.is_empty() {
        return (None, errors, warnings);
    }
    let m = Manifest {
        schema: MANIFEST_SCHEMA,
        name: name.unwrap_or_default(),
        description,
        entry,
        caps,
        reasons,
        libs,
        features,
    };
    (Some(m), errors, warnings)
}

// ── The folder (UW4, UW8) ───────────────────────────────────────────────

/// A widget folder's files, read flat.
#[derive(Debug, Clone, Default)]
pub struct WidgetFiles {
    pub files: BTreeMap<String, Vec<u8>>,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

/// What a file name is to K2: code, an asset, ignored quietly, or ignored
/// with a warning.
fn file_role(name: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with('.') || lower.starts_with("readme") || lower == "license" || lower.starts_with("license.") {
        return Some(false);
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if CODE_EXTS.contains(&ext) || asset_mime(&lower).is_some() || ext == "svg" {
        return Some(true);
    }
    None
}

/// Read a widget folder: top-level regular files only. Subfolders and
/// unknown files are ignored with a warning; links and oversized files are
/// errors; dotfiles and README/LICENSE are ignored quietly.
pub fn read_folder(dir: &Path, prefix: &str) -> WidgetFiles {
    let mut out = WidgetFiles::default();
    let at = |file: &str, message: String| Diagnostic { file: format!("{prefix}/{file}"), line: 1, col: 1, message };
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.errors.push(at("", format!("can't read the widget's folder: {e}")));
            return out;
        }
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(meta) = fs::symlink_metadata(e.path()) else { continue };
        if meta.file_type().is_symlink() {
            if file_role(&name) != Some(false) {
                out.errors.push(at(&name, format!("'{name}' is a link; put the file itself in the widget's folder")));
            }
            continue;
        }
        if meta.is_dir() {
            if !name.starts_with('.') {
                out.warnings.push(at(&name, format!("widgets are one flat folder; K2 ignores {name}/")));
            }
            continue;
        }
        match file_role(&name) {
            Some(false) => continue,
            None => {
                out.warnings.push(at(&name, format!("K2 ignores '{name}': a widget reads .html, .js, .css, images and .woff2 fonts")));
                continue;
            }
            Some(true) => {}
        }
        if meta.len() > MAX_FILE_BYTES {
            out.errors.push(at(&name, format!("'{name}' is {} bytes; no widget file may be over 3 MB", meta.len())));
            continue;
        }
        match fs::read(e.path()) {
            Ok(b) => {
                out.files.insert(name, b);
            }
            Err(err) => out.errors.push(at(&name, format!("can't read '{name}': {err}"))),
        }
    }
    out
}

/// A built-in widget's files (UWB21).
pub fn builtin_files(w: &super::builtin_widgets::BuiltinWidget) -> BTreeMap<String, Vec<u8>> {
    w.files.iter().map(|(n, t)| (n.to_string(), t.as_bytes().to_vec())).collect()
}

/// A widget's state after checking.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    pub manifest: Option<Manifest>,
    pub bundle: Option<Bundle>,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl Checked {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.bundle.is_some() && self.manifest.is_some()
    }
}

/// Check a widget's files: manifest, asset rules, bundle. `prefix` labels
/// findings (`widgets/<name>`), `folder` is the name `id` must match.
pub fn check_files(prefix: &str, folder: &str, files: &BTreeMap<String, Vec<u8>>) -> Checked {
    let mut c = Checked::default();
    let label = |f: &str| format!("{prefix}/{f}");
    // Assets: sizes, real types, no SVG (UW8).
    let mut total: u64 = 0;
    for (name, bytes) in files {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".svg") {
            c.errors.push(Diagnostic {
                file: label(name),
                line: 1,
                col: 1,
                message: format!("'{name}': SVG files aren't supported in this version; use .png, .webp, .jpg or .gif"),
            });
            continue;
        }
        let Some(want) = asset_mime(name) else { continue };
        total += bytes.len() as u64;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            c.errors.push(Diagnostic {
                file: label(name),
                line: 1,
                col: 1,
                message: format!("'{name}' is {} bytes; each image or font is at most 1 MB", bytes.len()),
            });
        }
        match sniff_asset(bytes) {
            Some(m) if m == want => {}
            Some(m) => c.errors.push(Diagnostic {
                file: label(name),
                line: 1,
                col: 1,
                message: format!("'{name}' is really {m}, not {want}; fix its extension"),
            }),
            None => c.errors.push(Diagnostic {
                file: label(name),
                line: 1,
                col: 1,
                message: format!("'{name}' is not a {want} file"),
            }),
        }
    }
    if total > MAX_ASSETS_BYTES {
        c.errors.push(Diagnostic {
            file: label(""),
            line: 1,
            col: 1,
            message: format!("this widget's images and fonts are {total} bytes together; the limit is 2 MB"),
        });
    }
    let Some(raw) = files.get(MANIFEST_FILE) else {
        c.errors.push(Diagnostic {
            file: label(MANIFEST_FILE),
            line: 1,
            col: 1,
            message: "no manifest.json: a widget needs one ({\"schema\": 1, \"name\": …, \"caps\": […]})".into(),
        });
        return c;
    };
    let Ok(src) = std::str::from_utf8(raw) else {
        c.errors.push(Diagnostic { file: label(MANIFEST_FILE), line: 1, col: 1, message: "manifest.json is not UTF-8 text".into() });
        return c;
    };
    let (manifest, errs, warns) = parse_manifest(&label(MANIFEST_FILE), src, folder);
    c.errors.extend(errs);
    c.warnings.extend(warns);
    let Some(manifest) = manifest else { return c };
    let out = bundle::bundle(prefix, &manifest.entry, files);
    c.errors.extend(out.errors);
    c.warnings.extend(out.warnings);
    if c.errors.is_empty() {
        c.bundle = out.bundle;
    }
    c.manifest = Some(manifest);
    c
}

/// The bytes of every code file (manifest, HTML, JS, CSS), for history.
pub fn code_files(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .filter(|(n, _)| {
            let lower = n.to_ascii_lowercase();
            lower.rsplit_once('.').is_some_and(|(_, e)| CODE_EXTS.contains(&e))
        })
        .map(|(n, b)| (n.clone(), b.clone()))
        .collect()
}

/// A hash of every file's name and bytes: the bundle cache key.
pub fn content_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (n, b) in files {
        h.update((n.len() as u64).to_le_bytes());
        h.update(n.as_bytes());
        h.update((b.len() as u64).to_le_bytes());
        h.update(b);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The manifest as `last-good.json` and the `get` payload carry it.
pub fn manifest_json(m: &Manifest) -> J {
    serde_json::to_value(m).unwrap_or_else(|e| panic!("manifest to JSON: {e}"))
}

// ── Examples (UW44) ─────────────────────────────────────────────────────

/// A copy-from starter for `k2 zen widget new --from`: `(file, text)`.
pub struct Example {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// `hello` (a clock and "hello <first agent>") and `arcade` (the game
/// Garden widget). Never placed by setup; starters and test fixtures.
pub const EXAMPLES: &[Example] = &[
    Example {
        name: "hello",
        files: &[
            ("manifest.json", include_str!("examples/hello/manifest.json")),
            ("index.html", include_str!("examples/hello/index.html")),
            ("hello.js", include_str!("examples/hello/hello.js")),
            ("hello.css", include_str!("examples/hello/hello.css")),
        ],
    },
    Example {
        name: "arcade",
        files: &[
            ("manifest.json", include_str!("examples/arcade/manifest.json")),
            ("index.html", include_str!("examples/arcade/index.html")),
            ("arcade.js", include_str!("examples/arcade/arcade.js")),
            ("arcade.css", include_str!("examples/arcade/arcade.css")),
        ],
    },
];

pub fn example(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|e| e.name == name)
}

/// A manifest's text with `id` set to `name` when it has an `id` key (a
/// copy of a built-in or example into a new folder).
pub fn rename_manifest(src: &str, name: &str) -> String {
    match serde_json::from_str::<J>(src) {
        Ok(mut v) if v.get("id").is_some() => {
            v["id"] = json!(name);
            serde_json::to_string_pretty(&v).map(|s| s + "\n").unwrap_or_else(|_| src.to_string())
        }
        _ => src.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(body: &str) -> (Option<Manifest>, Vec<Diagnostic>, Vec<Diagnostic>) {
        parse_manifest("widgets/agent-arcade/manifest.json", body, "agent-arcade")
    }

    const BASE: &str = "{\n  \"schema\": 1,\n  \"name\": \"Agent Arcade\",\n  \"caps\": [\"agents:read\"]";

    fn with(extra: &str) -> String {
        format!("{BASE},\n  {extra}\n}}\n")
    }

    #[test]
    fn a_good_manifest_parses() {
        let (m, e, w) = manifest(&with("\"reasons\": {\"agents:read\": \"to draw your agents\"}"));
        assert!(e.is_empty() && w.is_empty(), "{e:?} {w:?}");
        let m = m.expect("manifest");
        assert_eq!(m.entry, "index.html");
        assert_eq!(m.caps, vec!["agents:read".to_string()]);
        assert_eq!(Manifest::from_saved(&manifest_json(&m)), Some(m));
    }

    /// TUWA8 + TUW1.2's manifest lines: one finding each, at the key.
    #[test]
    fn reserved_unknown_and_refused_keys() {
        for (extra, errs, warns, line_col) in [
            ("\"version\": \"1.0.0\"", 0, 1, (5, 3)),
            ("\"license\": \"MIT\"", 0, 1, (5, 3)),
            ("\"bindings\": {\"main\": {\"type\": \"home\"}}", 0, 1, (5, 3)),
            ("\"requires\": {\"features\": [\"zen-widgets-v1\"]}", 0, 0, (0, 0)),
            ("\"requires\": {\"features\": [\"made-up\"]}", 0, 1, (5, 29)),
            ("\"bindings\": {\"main\": {\"type\": \"agents\"}}", 1, 0, (5, 25)),
            ("\"bindings\": {\"main\": {\"type\": \"room\"}}", 1, 0, (5, 25)),
            ("\"net\": [\"example.test\"]", 1, 0, (5, 3)),
            ("\"secrets\": {}", 1, 0, (5, 3)),
            ("\"kind\": \"garden\"", 1, 0, (5, 3)),
            ("\"contents\": []", 1, 0, (5, 3)),
            ("\"host\": {}", 1, 0, (5, 3)),
            ("\"roles\": []", 1, 0, (5, 3)),
            ("\"colour\": \"red\"", 1, 0, (5, 3)),
            ("\"id\": \"arcade\"", 1, 0, (5, 3)),
            ("\"id\": \"agent-arcade\"", 0, 0, (0, 0)),
            ("\"requires\": {\"libs\": [\"nope@1\"]}", 1, 0, (5, 25)),
            ("\"requires\": {\"libs\": [{\"url\": \"https://cdn.jsdelivr.net/npm/x@latest/x.js\", \"integrity\": \"sha384-x\"}]}", 1, 0, (5, 25)),
        ] {
            let (_, e, w) = manifest(&with(extra));
            assert_eq!((e.len(), w.len()), (errs, warns), "{extra}: errors {e:?} warnings {w:?}");
            if let Some(d) = e.first().or(w.first()) {
                assert_eq!((d.line, d.col), line_col, "{extra}: {d:?}");
            }
        }
    }

    #[test]
    fn bad_caps_and_json_are_errors_with_place() {
        let (_, e, _) = manifest("{\n  \"schema\": 1,\n  \"name\": \"A\",\n  \"caps\": [\"agents:read\", \"gardens:manage\"]\n}");
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!((e[0].line, e[0].col), (4, 27));
        assert!(e[0].message.contains("'gardens:manage' isn't available to custom widgets"));
        let (_, e, _) = manifest("{\n  \"schema\": 1,\n  \"name\": \"A\",\n}");
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].line, 4);
        assert!(e[0].message.contains("valid JSON"));
    }

    fn folder(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("k2-zen-widget-{tag}-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&d).expect("mkdir");
        d
    }

    #[test]
    fn the_folder_is_flat_and_links_are_refused() {
        let d = folder("flat");
        fs::write(d.join("manifest.json"), "{}").expect("w");
        fs::write(d.join("README.md"), "x").expect("w");
        fs::write(d.join(".DS_Store"), "x").expect("w");
        fs::write(d.join("notes.txt"), "x").expect("w");
        fs::create_dir_all(d.join("assets")).expect("mkdir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("manifest.json"), d.join("x.js")).expect("link");
        let f = read_folder(&d, "widgets/w");
        assert_eq!(f.files.keys().cloned().collect::<Vec<_>>(), vec!["manifest.json".to_string()]);
        assert_eq!(f.warnings.len(), 2, "{:?}", f.warnings);
        assert!(f.warnings.iter().any(|w| w.message.contains("one flat folder; K2 ignores assets/")));
        #[cfg(unix)]
        assert!(f.errors.iter().any(|e| e.message.contains("is a link")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn assets_must_be_what_they_say_and_svg_is_refused() {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert("manifest.json".into(), with("\"entry\": \"index.html\"").into_bytes());
        files.insert("index.html".into(), b"<p>hi</p>".to_vec());
        files.insert("cat.gif".into(), b"\x89PNG\r\n\x1a\nxxxx".to_vec());
        files.insert("logo.svg".into(), b"<svg/>".to_vec());
        let c = check_files("widgets/agent-arcade", "agent-arcade", &files);
        assert_eq!(c.errors.len(), 2, "{:?}", c.errors);
        assert!(c.errors.iter().any(|e| e.message.contains("is really image/png, not image/gif")));
        assert!(c.errors.iter().any(|e| e.message.contains("SVG")));
        assert!(!c.is_clean());
    }

    #[test]
    fn examples_bundle_clean() {
        for ex in EXAMPLES {
            let files: BTreeMap<String, Vec<u8>> =
                ex.files.iter().map(|(n, t)| (n.to_string(), t.as_bytes().to_vec())).collect();
            let c = check_files(&format!("widgets/{}", ex.name), ex.name, &files);
            assert!(c.is_clean(), "{}: {:?}", ex.name, c.errors);
            let m = c.manifest.expect("manifest");
            assert!(m.caps.iter().all(|c| super::super::USER_WIDGET_CAPS.contains(&c.as_str())));
        }
        assert_eq!(example("hello").map(|e| e.name), Some("hello"));
    }

    #[test]
    fn rename_manifest_sets_id_only_when_present() {
        assert_eq!(rename_manifest("{\"schema\":1}", "x"), "{\"schema\":1}");
        let r: J = serde_json::from_str(&rename_manifest("{\"id\":\"diary\",\"schema\":1}", "my-diary")).expect("json");
        assert_eq!(r["id"], "my-diary");
    }
}
