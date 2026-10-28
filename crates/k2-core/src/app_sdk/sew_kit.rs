//! Sew's App kit (`sew-app-kit-<v>.tar.gz`, Sew 0.5.0 on): the npm-pack
//! tarballs of `@sew/client`, `@sew/voice`, `@sew/channel`, `@sew/e2ee` and
//! `@sew/bff`, with `SHA256SUMS`, `NOTICES` and a `kit.json` manifest.
//!
//! **Trust.** The kit's own sha256 is pinned by K2 next to the Sew pin
//! (`k2_root_proto::sew::app_kit`); [`parse`] refuses any other bytes, then
//! checks every package tarball against `kit.json` (and `SHA256SUMS`, when
//! it lists it), and each tarball's `package/package.json` against its
//! `kit.json` row (name, version, no install scripts).
//!
//! **kit.json** (the shape K2 reads; other keys are ignored):
//! ```json
//! { "version": "0.5.0",
//!   "packages": [ { "name": "@sew/client", "version": "0.5.0",
//!                   "file": "sew-client-0.5.0.tgz", "sha256": "<64 hex>" }, … ] }
//! ```
//! Members sit at the kit's root or under one top folder. Dependencies,
//! peers and which of them are kit-internal come from each package's own
//! package.json (what npm reads), not from kit.json.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use serde::Deserialize;

use super::SdkError;

/// Largest kit K2 reads.
pub const MAX_KIT: usize = 64 << 20;
/// Largest member of a kit.
const MAX_MEMBER: u64 = 32 << 20;
/// The kit's file name for a Sew version.
pub fn file_name(version: &str) -> String {
    format!("sew-app-kit-{version}.tar.gz")
}
/// The packages every kit carries (Sew REL-050).
pub const PACKAGES: [&str; 5] = ["@sew/client", "@sew/voice", "@sew/channel", "@sew/e2ee", "@sew/bff"];
/// The first Sew release with an App kit.
pub const MIN_SEW: &str = "0.5.0";

/// One package of a verified kit.
#[derive(Debug, Clone)]
pub struct KitPackage {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
    /// `dependencies` from its package.json.
    pub dependencies: BTreeMap<String, String>,
    /// `peerDependencies` minus the optional ones.
    pub peers: BTreeMap<String, String>,
}

impl KitPackage {
    /// The vendored file name (`@sew/client` 0.5.0 → `sew-client-0.5.0.tgz`,
    /// what npm pack names it).
    pub fn file_name(&self) -> String {
        super::vendored_file_name(&self.name, &self.version)
    }
}

/// A verified kit.
#[derive(Debug, Clone)]
pub struct SewKit {
    pub version: String,
    pub sha256: String,
    pub packages: Vec<KitPackage>,
    pub notices: Option<Vec<u8>>,
}

impl SewKit {
    pub fn names(&self) -> BTreeSet<&str> {
        self.packages.iter().map(|p| p.name.as_str()).collect()
    }
    /// Kit packages another kit package depends on (npm `overrides` pin
    /// them to the vendored file).
    pub fn internal_targets(&self) -> BTreeSet<&str> {
        let names = self.names();
        self.packages.iter().flat_map(|p| p.dependencies.keys()).map(String::as_str).filter(|n| names.contains(n)).collect()
    }
    /// Dependencies from the registry (name → specs, merged).
    pub fn external(&self) -> BTreeMap<String, BTreeSet<String>> {
        let names = self.names();
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in &self.packages {
            for (n, v) in &p.dependencies {
                if !names.contains(n.as_str()) {
                    out.entry(n.clone()).or_default().insert(v.clone());
                }
            }
        }
        out
    }
    /// Peers the App installs itself (name → specs, merged).
    pub fn peers(&self) -> BTreeMap<String, BTreeSet<String>> {
        let names = self.names();
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in &self.packages {
            for (n, v) in &p.peers {
                if !names.contains(n.as_str()) {
                    out.entry(n.clone()).or_default().insert(v.clone());
                }
            }
        }
        out
    }
}

#[derive(Deserialize)]
struct KitJson {
    version: String,
    packages: Vec<KitRow>,
}

#[derive(Deserialize)]
struct KitRow {
    name: String,
    version: String,
    #[serde(alias = "tarball")]
    file: String,
    sha256: String,
}

fn bad(hint: impl Into<String>) -> SdkError {
    SdkError::new(super::codes::SEW_KIT_INVALID, hint)
}

/// Read every regular file of a `.tar.gz` (path → bytes). Refuses links,
/// absolute paths and `..`.
pub(crate) fn members(tgz: &[u8], what: &str) -> Result<BTreeMap<String, Vec<u8>>, SdkError> {
    let mut a = tar::Archive::new(flate2::read::GzDecoder::new(tgz));
    let mut out = BTreeMap::new();
    let mut total: u64 = 0;
    for e in a.entries().map_err(|e| bad(format!("{what} is not a tar.gz: {e}")))? {
        let mut e = e.map_err(|e| bad(format!("{what}: {e}")))?;
        let kind = e.header().entry_type();
        let path = e.path().map_err(|e| bad(format!("{what}: {e}")))?.to_string_lossy().replace('\\', "/");
        let path = path.trim_start_matches("./").to_string();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(bad(format!("{what}: {path} is not a regular file")));
        }
        if path.starts_with('/') || path.split('/').any(|s| s == "..") {
            return Err(bad(format!("{what}: {path} leaves the archive")));
        }
        let size = e.header().size().unwrap_or(u64::MAX);
        total = total.saturating_add(size);
        if size > MAX_MEMBER || total > MAX_KIT as u64 {
            return Err(bad(format!("{what}: {path} is too large")));
        }
        let mut b = Vec::new();
        e.read_to_end(&mut b).map_err(|e| bad(format!("{what}: {path}: {e}")))?;
        out.insert(path, b);
    }
    Ok(out)
}

/// The `package/package.json` of an npm-pack tarball.
pub(crate) fn package_json_of(tgz: &[u8], what: &str) -> Result<serde_json::Value, SdkError> {
    let files = members(tgz, what)?;
    let b = files.get("package/package.json").ok_or_else(|| bad(format!("{what} has no package/package.json")))?;
    let v: serde_json::Value = serde_json::from_slice(b).map_err(|e| bad(format!("{what}: package.json: {e}")))?;
    if !v.is_object() {
        return Err(bad(format!("{what}: package.json is not an object")));
    }
    Ok(v)
}

/// The scripts npm runs on install.
pub const INSTALL_SCRIPTS: [&str; 3] = ["preinstall", "install", "postinstall"];

fn str_map(v: &serde_json::Value, key: &str, what: &str) -> Result<BTreeMap<String, String>, SdkError> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(m)) => m
            .iter()
            .map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string())).ok_or_else(|| bad(format!("{what}: {key}.{k} is not a string"))))
            .collect(),
        Some(_) => Err(bad(format!("{what}: {key} is not an object"))),
    }
}

/// Verify a kit against its pinned sha256 and read it.
pub fn parse(kit: &[u8], expected_sha256: &str) -> Result<SewKit, SdkError> {
    let sha = super::sha256_hex(kit);
    if !sha.eq_ignore_ascii_case(expected_sha256) {
        return Err(SdkError::new(
            super::codes::SEW_KIT_TAMPERED,
            format!("the Sew App kit's sha256 is {sha}, not the {expected_sha256} K2 pins: refusing it"),
        ));
    }
    let files = members(kit, "the Sew App kit")?;
    // kit.json at the root or under one top folder.
    let kit_json_path = files
        .keys()
        .find(|p| *p == "kit.json" || (p.ends_with("/kit.json") && p.matches('/').count() == 1))
        .cloned()
        .ok_or_else(|| bad("the Sew App kit has no kit.json"))?;
    let prefix = kit_json_path.trim_end_matches("kit.json").to_string();
    let manifest: KitJson =
        serde_json::from_slice(&files[&kit_json_path]).map_err(|e| bad(format!("kit.json: {e}")))?;
    let sums: BTreeMap<String, String> = files
        .get(&format!("{prefix}SHA256SUMS"))
        .map(|b| {
            String::from_utf8_lossy(b)
                .lines()
                .filter_map(|l| l.split_once("  ").map(|(h, f)| (f.trim().trim_start_matches("./").to_string(), h.trim().to_ascii_lowercase())))
                .collect()
        })
        .unwrap_or_default();
    let mut packages = Vec::new();
    for row in &manifest.packages {
        if !row.name.starts_with("@sew/") {
            return Err(bad(format!("kit.json lists {}, not a @sew package", row.name)));
        }
        if row.file.contains('/') || row.file.contains("..") {
            return Err(bad(format!("kit.json: {} names the file {:?}", row.name, row.file)));
        }
        let bytes = files
            .get(&format!("{prefix}{}", row.file))
            .ok_or_else(|| bad(format!("the kit has no {} ({})", row.file, row.name)))?;
        let got = super::sha256_hex(bytes);
        if !got.eq_ignore_ascii_case(&row.sha256) {
            return Err(SdkError::new(
                super::codes::SEW_KIT_TAMPERED,
                format!("{} in the Sew App kit has sha256 {got}, not kit.json's {}: refusing the kit", row.file, row.sha256),
            ));
        }
        if let Some(listed) = sums.get(&row.file) {
            if !listed.eq_ignore_ascii_case(&got) {
                return Err(SdkError::new(
                    super::codes::SEW_KIT_TAMPERED,
                    format!("{} in the Sew App kit has sha256 {got}, not SHA256SUMS's {listed}: refusing the kit", row.file),
                ));
            }
        }
        let what = format!("{} ({})", row.file, row.name);
        let pj = package_json_of(bytes, &what)?;
        if pj.get("name").and_then(|v| v.as_str()) != Some(row.name.as_str())
            || pj.get("version").and_then(|v| v.as_str()) != Some(row.version.as_str())
        {
            return Err(bad(format!("{what}: its package.json is not {}@{}", row.name, row.version)));
        }
        let scripts = str_map(&pj, "scripts", &what)?;
        if let Some(s) = INSTALL_SCRIPTS.iter().find(|s| scripts.contains_key(**s)) {
            return Err(bad(format!("{what} has an install script ({s}): K2 vendors no package that runs code on install")));
        }
        let optional: BTreeSet<String> = pj
            .get("peerDependenciesMeta")
            .and_then(|m| m.as_object())
            .map(|m| m.iter().filter(|(_, v)| v.get("optional").and_then(|o| o.as_bool()) == Some(true)).map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        let mut peers = str_map(&pj, "peerDependencies", &what)?;
        peers.retain(|k, _| !optional.contains(k));
        packages.push(KitPackage {
            name: row.name.clone(),
            version: row.version.clone(),
            sha256: got,
            bytes: bytes.clone(),
            dependencies: str_map(&pj, "dependencies", &what)?,
            peers,
        });
    }
    for need in PACKAGES {
        if !packages.iter().any(|p| p.name == need) {
            return Err(bad(format!("the Sew App kit {} has no {need}", manifest.version)));
        }
    }
    let names: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    if names.len() != packages.len() {
        return Err(bad("kit.json lists a package twice"));
    }
    // A kit-internal dependency must be exactly the vendored version (the
    // `overrides` K2 writes pin it to that file).
    for p in &packages {
        for (n, spec) in &p.dependencies {
            if let Some(dep) = packages.iter().find(|q| &q.name == n) {
                if spec != &dep.version {
                    return Err(bad(format!("{} wants {n}@{spec}, the kit carries {}", p.name, dep.version)));
                }
            }
        }
    }
    Ok(SewKit { version: manifest.version, sha256: sha, packages, notices: files.get(&format!("{prefix}NOTICES")).cloned() })
}
