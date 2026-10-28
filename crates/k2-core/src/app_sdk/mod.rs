//! `k2 app sdk add <app-folder> [--sew]` (0.46.6): put K2's server SDK
//! (`@k2/server`, packed at this K2's version, [`k2_server`]) and, with
//! `--sew`, Sew's five client packages ([`sew_kit`]) into an App folder's
//! `vendor/`, and point the App's package.json at them with `file:` specs.
//!
//! The App then installs with plain `npm install` (offline for these
//! packages), its Seed carries `vendor/`, so it builds offline and rolls
//! back with its SDK. K2 never runs npm, never touches the network and
//! never writes outside the folder: `vendor/<file>.tgz` and
//! `package.json` only (a symlinked `vendor/` or package.json is refused).
//! Rerunning replaces older vendored versions; it never duplicates.
//!
//! The daemon's `/cli/app/sdk/*` routes (permission + workspace jail) call
//! [`add`] and [`status`]; the Sew kit lives where [`kit_path`] says.

pub mod json;
pub mod k2_server;
pub mod sew_kit;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use json::J;
pub use sew_kit::SewKit;

/// The folder (inside the App folder) vendored tarballs go in.
pub const VENDOR_DIR: &str = "vendor";
/// Largest package.json K2 edits.
const MAX_PACKAGE_JSON: u64 = 1 << 20;

/// Refusal codes.
pub mod codes {
    /// The folder or its package.json can't be edited as asked.
    pub const SDK_FOLDER: &str = "sdk_folder_invalid";
    /// package.json isn't a JSON object (or a section in it isn't).
    pub const PACKAGE_JSON: &str = "sdk_package_json_invalid";
    /// No Sew App kit on this box.
    pub const SEW_KIT_MISSING: &str = "sew_kit_missing";
    /// The pinned Sew release has no App kit (before 0.5.0).
    pub const SEW_KIT_UNAVAILABLE: &str = "sew_kit_unavailable";
    /// The kit (or a tarball in it) doesn't match its sha256.
    pub const SEW_KIT_TAMPERED: &str = "sew_kit_tampered";
    /// The kit's contents aren't what K2 reads.
    pub const SEW_KIT_INVALID: &str = "sew_kit_invalid";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkError {
    pub code: &'static str,
    pub hint: String,
}

impl SdkError {
    pub fn new(code: &'static str, hint: impl Into<String>) -> Self {
        SdkError { code, hint: hint.into() }
    }
}

impl std::fmt::Display for SdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// npm pack's file name: `@scope/name` → `scope-name-<version>.tgz`.
pub fn vendored_file_name(name: &str, version: &str) -> String {
    format!("{}-{version}.tgz", name.trim_start_matches('@').replace('/', "-"))
}

/// The `file:` spec of a vendored tarball (relative to the App folder).
pub fn file_spec(file: &str) -> String {
    format!("file:{VENDOR_DIR}/{file}")
}

// ── the Sew kit on this box ──────────────────────────────────────────

/// `~/.k2/sdk/sew/sew-app-kit-<v>.tar.gz`: K2's own copy, read-only.
pub fn kit_path(version: &str) -> PathBuf {
    crate::paths::k2_home().join("sdk").join("sew").join(sew_kit::file_name(version))
}

/// Verify `bytes` as Sew's App kit `version` (pinned `sha256`) and keep it
/// at [`kit_path`] (0444). Other versions' kits are removed.
pub fn store_kit(bytes: &[u8], version: &str, sha256: &str) -> Result<SewKit, SdkError> {
    let kit = sew_kit::parse(bytes, sha256)?;
    if kit.version != version {
        return Err(SdkError::new(codes::SEW_KIT_INVALID, format!("the kit says Sew {}, the pin is {version}", kit.version)));
    }
    let path = kit_path(version);
    let dir = path.parent().expect("kit_path has a parent");
    let io = |e: std::io::Error| SdkError::new(codes::SEW_KIT_INVALID, format!("keep the kit in {}: {e}", dir.display()));
    fs::create_dir_all(dir).map_err(io)?;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("sew-app-kit-") && n != sew_kit::file_name(version) {
                let _ = make_writable(&e.path());
                let _ = fs::remove_file(e.path());
            }
        }
    }
    write_atomic(&path, bytes, true).map_err(io)?;
    Ok(kit)
}

/// The kit K2 keeps for `version`, verified again against `sha256`.
/// `Ok(None)`: none kept.
pub fn load_kit(version: &str, sha256: &str) -> Result<Option<SewKit>, SdkError> {
    let path = kit_path(version);
    match fs::read(&path) {
        Ok(b) => sew_kit::parse(&b, sha256).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SdkError::new(codes::SEW_KIT_INVALID, format!("{}: {e}", path.display()))),
    }
}

// ── add ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vendored {
    pub name: String,
    pub version: String,
    /// `vendor/<file>`.
    pub file: String,
    pub sha256: String,
    /// The `file:` spec in package.json.
    pub spec: String,
    /// The file was written (absent or different before).
    pub written: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddReport {
    pub folder: String,
    pub vendored: Vec<Vendored>,
    /// Older vendored tarballs removed (`vendor/<file>`).
    pub removed: Vec<String>,
    /// `created` | `updated` | `unchanged`.
    pub package_json: &'static str,
    /// Kit packages pinned to their vendored file in `overrides`.
    pub overrides: Vec<String>,
    /// Peers the App installs itself (name → spec).
    pub peers: BTreeMap<String, String>,
    /// Registry dependencies of the vendored packages (npm install fetches them).
    pub external: BTreeMap<String, String>,
    /// `npm i …` for the peers and registry dependencies, or empty.
    pub install: String,
    /// The Sew kit's version, with --sew.
    pub sew: Option<String>,
}

fn folder_err(hint: impl Into<String>) -> SdkError {
    SdkError::new(codes::SDK_FOLDER, hint)
}

fn pj_err(hint: impl Into<String>) -> SdkError {
    SdkError::new(codes::PACKAGE_JSON, hint)
}

#[cfg(unix)]
fn make_writable(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn make_writable(p: &Path) -> std::io::Result<()> {
    let mut perm = fs::metadata(p)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(p, perm)
}

/// Write `bytes` to `path` through a temp file in the same folder + rename.
fn write_atomic(path: &Path, bytes: &[u8], read_only: bool) -> std::io::Result<()> {
    let dir = path.parent().expect("a file in a folder");
    let tmp = dir.join(format!(".{}.k2-tmp-{}", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if read_only {
        let mut perm = fs::metadata(&tmp)?.permissions();
        perm.set_readonly(true);
        fs::set_permissions(&tmp, perm)?;
    }
    if fs::symlink_metadata(path).is_ok() {
        // Windows can't rename over a read-only file.
        let _ = make_writable(path);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// A path inside the folder that must not be a symlink (or other odd
/// file): `Ok(true)` it exists as `want_dir`, `Ok(false)` absent.
fn plain(p: &Path, want_dir: bool) -> Result<bool, SdkError> {
    match fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(folder_err(format!("{}: {e}", p.display()))),
        Ok(m) if m.file_type().is_symlink() => {
            Err(folder_err(format!("{} is a symlink: K2 writes only inside the App folder, so it won't follow it", p.display())))
        }
        Ok(m) if want_dir && !m.is_dir() => Err(folder_err(format!("{} is not a folder", p.display()))),
        Ok(m) if !want_dir && !m.is_file() => Err(folder_err(format!("{} is not a file", p.display()))),
        Ok(_) => Ok(true),
    }
}

/// A package name npm accepts, from the folder's name.
fn package_name(folder: &Path) -> String {
    let raw = folder.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let s: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    let s = s.trim_matches(|c| c == '-' || c == '.' || c == '_').to_string();
    if s.is_empty() {
        "app".into()
    } else {
        s.chars().take(64).collect()
    }
}

/// The dependency sections npm reads; K2's entries live in `dependencies`.
const OTHER_DEP_SECTIONS: [&str; 3] = ["devDependencies", "optionalDependencies", "peerDependencies"];

/// One package to vendor.
struct Item<'a> {
    name: &'a str,
    version: &'a str,
    bytes: &'a [u8],
    /// The family whose older files a rerun removes (`k2-server-`, …).
    stem: String,
}

/// Vendor `@k2/server` (and the Sew kit's packages) into `folder` and point
/// its package.json at them. `folder` is an existing, already-jailed folder.
pub fn add(folder: &Path, sew: Option<&SewKit>) -> Result<AddReport, SdkError> {
    if !plain(folder, true)? {
        return Err(folder_err(format!("{} is not a folder", folder.display())));
    }
    let vendor = folder.join(VENDOR_DIR);
    let pj_path = folder.join("package.json");
    let has_pj = plain(&pj_path, false)?;
    plain(&vendor, true)?;

    // Read package.json first: a bad one changes nothing.
    let (mut pj, indent, before) = if has_pj {
        let meta = fs::metadata(&pj_path).map_err(|e| folder_err(format!("{}: {e}", pj_path.display())))?;
        if meta.len() > MAX_PACKAGE_JSON {
            return Err(pj_err("package.json is larger than 1 MiB"));
        }
        let text = fs::read_to_string(&pj_path).map_err(|e| pj_err(format!("package.json: {e}")))?;
        let v: J = serde_json::from_str(&text).map_err(|e| pj_err(format!("package.json is not valid JSON ({e}); fix it and run again")))?;
        if v.as_obj().is_none() {
            return Err(pj_err("package.json is not a JSON object"));
        }
        let indent = json::indent_of(&text);
        (v, indent, Some(text))
    } else {
        let mut v = J::obj();
        v.set("name", J::Str(package_name(folder)));
        v.set("private", J::Bool(true));
        v.set("type", J::Str("module".into()));
        (v, b"  ".to_vec(), None)
    };
    for sec in ["dependencies", "overrides"].into_iter().chain(OTHER_DEP_SECTIONS) {
        if let Some(x) = pj.get(sec) {
            if x.as_obj().is_none() {
                return Err(pj_err(format!("package.json's {sec} is not an object")));
            }
        }
    }

    let mut items = vec![Item {
        name: k2_server::NAME,
        version: k2_server::version(),
        bytes: k2_server::tarball(),
        stem: vendored_file_name(k2_server::NAME, "").trim_end_matches(".tgz").to_string(),
    }];
    if let Some(kit) = sew {
        for p in &kit.packages {
            items.push(Item {
                name: &p.name,
                version: &p.version,
                bytes: &p.bytes,
                stem: vendored_file_name(&p.name, "").trim_end_matches(".tgz").to_string(),
            });
        }
    }

    // vendor/: write what's new, remove the older versions of the same
    // packages (and only those).
    fs::create_dir_all(&vendor).map_err(|e| folder_err(format!("{}: {e}", vendor.display())))?;
    let mut vendored = Vec::new();
    let mut removed = Vec::new();
    let keep: Vec<String> = items.iter().map(|i| vendored_file_name(i.name, i.version)).collect();
    let existing: Vec<String> = fs::read_dir(&vendor)
        .map_err(|e| folder_err(format!("{}: {e}", vendor.display())))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    for it in &items {
        let file = vendored_file_name(it.name, it.version);
        let path = vendor.join(&file);
        let present = plain(&path, false)?;
        let same = present && fs::read(&path).map(|b| b == it.bytes).unwrap_or(false);
        if !same {
            write_atomic(&path, it.bytes, false).map_err(|e| folder_err(format!("{}: {e}", path.display())))?;
        }
        for old in &existing {
            // `<stem><version>.tgz` where the version starts with a digit:
            // `sew-client-` never matches `sew-client-extra-1.0.0.tgz`.
            let is_family = old
                .strip_prefix(&it.stem)
                .and_then(|rest| rest.strip_suffix(".tgz"))
                .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()));
            if is_family && !keep.contains(old) && !removed.contains(&format!("{VENDOR_DIR}/{old}")) {
                let p = vendor.join(old);
                if plain(&p, false)? {
                    fs::remove_file(&p).map_err(|e| folder_err(format!("{}: {e}", p.display())))?;
                    removed.push(format!("{VENDOR_DIR}/{old}"));
                }
            }
        }
        vendored.push(Vendored {
            name: it.name.to_string(),
            version: it.version.to_string(),
            file: format!("{VENDOR_DIR}/{file}"),
            sha256: sha256_hex(it.bytes),
            spec: file_spec(&file),
            written: !same,
        });
    }
    if let Some(notes) = sew.and_then(|k| k.notices.as_ref()) {
        let p = vendor.join("sew-NOTICES");
        plain(&p, false)?;
        if fs::read(&p).ok().as_ref() != Some(notes) {
            write_atomic(&p, notes, false).map_err(|e| folder_err(format!("{}: {e}", p.display())))?;
        }
    }

    // package.json: dependencies (and overrides for kit-internal deps).
    if pj.get("dependencies").is_none() {
        pj.set("dependencies", J::obj());
    }
    for v in &vendored {
        for sec in OTHER_DEP_SECTIONS {
            if let Some(s) = pj.get_mut(sec) {
                s.remove(&v.name);
            }
        }
        pj.get_mut("dependencies").expect("set above").set(&v.name, J::Str(v.spec.clone()));
    }
    let mut overrides = Vec::new();
    if let Some(kit) = sew {
        let targets = kit.internal_targets();
        if !targets.is_empty() && pj.get("overrides").is_none() {
            pj.set("overrides", J::obj());
        }
        for t in targets {
            let v = vendored.iter().find(|v| v.name == t).expect("a kit package is vendored");
            pj.get_mut("overrides").expect("set above").set(t, J::Str(v.spec.clone()));
            overrides.push(t.to_string());
        }
    }
    let text = pj.to_pretty(&indent);
    let package_json = match &before {
        Some(b) if *b == text => "unchanged",
        Some(_) => "updated",
        None => "created",
    };
    if package_json != "unchanged" {
        write_atomic(&pj_path, text.as_bytes(), false).map_err(|e| folder_err(format!("{}: {e}", pj_path.display())))?;
    }

    let join = |m: BTreeMap<String, std::collections::BTreeSet<String>>| -> BTreeMap<String, String> {
        m.into_iter().map(|(k, v)| (k, v.into_iter().collect::<Vec<_>>().join(" || "))).collect()
    };
    let (peers, external) = match sew {
        Some(k) => (join(k.peers()), join(k.external())),
        None => (BTreeMap::new(), BTreeMap::new()),
    };
    let install = if peers.is_empty() && external.is_empty() {
        String::new()
    } else {
        let mut parts = vec!["npm i".to_string()];
        for (n, v) in peers.iter().chain(external.iter()) {
            // A `||` range can't go on one `npm i` word: the first range.
            let first = v.split(" || ").next().unwrap_or(v);
            parts.push(format!("{n}@{first}"));
        }
        parts.join(" ")
    };
    Ok(AddReport {
        folder: folder.display().to_string(),
        vendored,
        removed,
        package_json,
        overrides,
        peers,
        external,
        install,
        sew: sew.map(|k| k.version.clone()),
    })
}

// ── status ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRow {
    pub name: String,
    /// The version in `vendor/` (from its file name), if any.
    pub vendored: Option<String>,
    /// The `dependencies` spec in package.json, if any.
    pub spec: Option<String>,
    /// What this box carries (None: nothing for Sew here).
    pub on_box: Option<String>,
    /// `current` | `older` | `newer` | `missing` | `not_on_box`.
    pub state: &'static str,
}

/// What `folder` has vendored against what this box carries.
/// `sew_on_box`: the Sew kit version kept on this box, if any.
pub fn status(folder: &Path, sew_on_box: Option<&str>) -> Result<Vec<StatusRow>, SdkError> {
    if !plain(folder, true)? {
        return Err(folder_err(format!("{} is not a folder", folder.display())));
    }
    let vendor = folder.join(VENDOR_DIR);
    let files: Vec<String> = if plain(&vendor, true)? {
        fs::read_dir(&vendor).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let pj: Option<J> = if plain(&folder.join("package.json"), false)? {
        fs::read_to_string(folder.join("package.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
    } else {
        None
    };
    let mut rows = Vec::new();
    let names: Vec<(&str, Option<&str>)> = std::iter::once((k2_server::NAME, Some(k2_server::version())))
        .chain(sew_kit::PACKAGES.iter().map(|n| (*n, sew_on_box)))
        .collect();
    for (name, on_box) in names {
        let stem = vendored_file_name(name, "").trim_end_matches(".tgz").to_string();
        let mut versions: Vec<String> = files
            .iter()
            .filter_map(|f| f.strip_prefix(&stem).and_then(|r| r.strip_suffix(".tgz")).map(str::to_string))
            .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
            .collect();
        versions.sort_by(|a, b| crate::zen::stdlib::version_cmp(a, b));
        let vendored = versions.pop();
        let spec = pj.as_ref().and_then(|p| p.get("dependencies")).and_then(|d| d.get(name)).and_then(|s| s.as_str()).map(str::to_string);
        if vendored.is_none() && spec.is_none() && name != k2_server::NAME {
            continue;
        }
        let state = match (&vendored, on_box) {
            (None, _) => "missing",
            (Some(_), None) => "not_on_box",
            (Some(v), Some(b)) => match crate::zen::stdlib::version_cmp(v, b) {
                std::cmp::Ordering::Equal => "current",
                std::cmp::Ordering::Less => "older",
                std::cmp::Ordering::Greater => "newer",
            },
        };
        rows.push(StatusRow { name: name.to_string(), vendored, spec, on_box: on_box.map(str::to_string), state });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
