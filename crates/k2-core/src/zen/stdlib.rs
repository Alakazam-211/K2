//! The widget standard library manifest (prd-zen-user-widgets-v2 §13.9,
//! UWB12–UWB17; Rosson R2, R4, R8 2026-10-08).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B3** (the manifest, the
//! vendor script, the daemon download routes, the renderer loader). B2's
//! validator reads [`LibRef`] from a widget manifest's `requires.libs`; B1's
//! skill reads the manifest for "what's available". The TypeScript mirror
//! and the renderer's `zenLib.texts` are `src/renderer/lib/zen/zen-lib-loader.ts`.
//!
//! One pinned manifest, `src/shared/zen-lib.json`, read by TS (import) and
//! Rust ([`ZEN_LIB_JSON`]). Each library is one of:
//! - **`bundled`**: files in `src/renderer/zen-lib/<id>@<version>/`, shipped
//!   in the desktop renderer (not `public/`, so the web build never carries
//!   them), written by `scripts/vendor-zen-lib.sh` (never run in CI or at
//!   build time; output checked in).
//! - **`download`** (Babylon.js, Plotly; R8: in this release): fetched once
//!   **by the daemon** from the pinned `url` of each file, sha256-checked,
//!   cached in `~/.k2/cache/zen-lib/<id>@<version>/`, refused under air-gap.
//!
//! A widget may also name a CDN file directly (R2): `{"url", "integrity"}`,
//! hosts in `cdnHosts`, exact versions only. The daemon fetches and checks
//! it the same way. **The sealed frame never touches the network**: the
//! renderer inlines every library as a nonced classic script after `k2.js`
//! and before the widget's own scripts (UWB13).
//!
//! p5.js is not in the list (R4: LGPL). GSAP is not in the list (licence).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value as J;

/// The checked-in manifest.
pub const ZEN_LIB_JSON: &str = include_str!("../../../../src/shared/zen-lib.json");

/// The whole `zen-lib.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZenLibManifest {
    pub schema: u32,
    /// At most this many bytes of library text inlined into one widget
    /// (UWB13: 8 MB), outside UW8's 256 KB code limit.
    pub max_bytes_per_widget: u64,
    /// Hosts a widget's `{url, integrity}` entry may name (R2).
    pub cdn_hosts: Vec<String>,
    pub libs: Vec<ZenLibEntry>,
}

/// Where a library's bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibSource {
    Bundled,
    Download,
}

/// One library version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZenLibEntry {
    /// npm-style name: `three`, `perfect-freehand`, `pixi.js`.
    pub id: String,
    /// The exact version shipped: `0.170.0`.
    pub version: String,
    /// Human name for the guide: `three.js`.
    pub title: String,
    /// The classic global it defines (`THREE`, `PIXI`, `d3`). Libraries are
    /// classic globals only; ESM-only packages are pre-bundled to an IIFE.
    pub global: String,
    /// SPDX id: `MIT`, `ISC`, `BSD-3-Clause`, `Apache-2.0`, `OFL-1.1`.
    pub license: String,
    /// The licence file inside the library's folder.
    pub license_file: String,
    /// The copyright line for NOTICE.md (UWB16).
    pub copyright: String,
    pub source: LibSource,
    /// In inline order.
    pub files: Vec<ZenLibFile>,
    /// How an ESM-only package was pre-bundled, recorded for the next
    /// vendor run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prebundled: Option<String>,
    /// "Limited under the sealed frame: …" for the guide (UWB17).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One file of a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZenLibFile {
    /// Flat file name in the library folder: `three.min.js`.
    pub name: String,
    pub bytes: u64,
    /// Lower-case hex sha256 of the file.
    pub sha256: String,
    /// The pinned download URL; required when the library is `download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ZenLibEntry {
    /// `<id>@<version>`.
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// Whether a requested version names this entry: the exact version, or
    /// a prefix of it that ends at a dot (`0.170` and `0` both name
    /// `0.170.0`; `0.17` doesn't).
    pub fn matches_version(&self, want: &str) -> bool {
        self.version == want || self.version.strip_prefix(want).is_some_and(|rest| rest.starts_with('.'))
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// The checked-in manifest, parsed once. A bad file panics with the serde
/// error (a test runs this load).
pub fn zen_lib_manifest() -> &'static ZenLibManifest {
    static M: OnceLock<ZenLibManifest> = OnceLock::new();
    M.get_or_init(|| {
        serde_json::from_str(ZEN_LIB_JSON).unwrap_or_else(|e| panic!("src/shared/zen-lib.json does not parse: {e}"))
    })
}

/// One `requires.libs` item in a widget manifest (UWB13, UWB15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibRef {
    /// `"three@0.170"`: a library from the manifest.
    Named { id: String, version: String },
    /// `{"url": "https://cdn.jsdelivr.net/npm/x@1.2.3/dist/x.min.js",
    /// "integrity": "sha384-…"}`: one file the daemon fetches and checks.
    Cdn { url: String, integrity: String },
}

impl LibRef {
    /// Parse one item: a `"<id>@<version>"` string or a `{url, integrity}`
    /// object. Shape only; [`find`] and the daemon check the rest.
    pub fn from_json(v: &J) -> Result<Self, String> {
        match v {
            J::String(s) => {
                let (id, version) = s
                    .rsplit_once('@')
                    .filter(|(i, v)| !i.is_empty() && !v.is_empty())
                    .ok_or_else(|| format!("'{s}': name a library as <id>@<version>, like three@0.170"))?;
                Ok(LibRef::Named { id: id.to_string(), version: version.to_string() })
            }
            J::Object(o) => {
                if let Some(k) = o.keys().find(|k| *k != "url" && *k != "integrity") {
                    return Err(format!("unknown key '{k}' in a requires.libs entry; use url and integrity"));
                }
                let get = |k: &str| o.get(k).and_then(J::as_str).map(str::to_string);
                match (get("url"), get("integrity")) {
                    (Some(url), Some(integrity)) => Ok(LibRef::Cdn { url, integrity }),
                    _ => Err("a CDN entry needs both url and integrity".into()),
                }
            }
            _ => Err("a requires.libs entry is \"<id>@<version>\" or {url, integrity}".into()),
        }
    }
}

/// The manifest entry a named reference resolves to. `None` for a CDN
/// reference or an unknown name.
pub fn find<'a>(m: &'a ZenLibManifest, r: &LibRef) -> Option<&'a ZenLibEntry> {
    let LibRef::Named { id, version } = r else { return None };
    m.libs.iter().find(|e| &e.id == id && e.matches_version(version))
}

/// Check a CDN reference (R2): https, a host in `cdnHosts`, an exact
/// version in the path (no `@latest`, no ranges), and an SRI integrity of
/// sha256, sha384 or sha512.
pub fn check_cdn(m: &ZenLibManifest, url: &str, integrity: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or("CDN libraries load over https only")?;
    let host = rest.split('/').next().unwrap_or("");
    if !m.cdn_hosts.iter().any(|h| h == host) {
        return Err(format!("'{host}' isn't an allowed CDN; use {}", m.cdn_hosts.join(", ")));
    }
    if rest.contains("@latest") || rest.contains('^') || rest.contains('~') || rest.contains('*') {
        return Err("pin an exact version in the URL (no @latest or ranges)".into());
    }
    let ok = ["sha256-", "sha384-", "sha512-"].iter().any(|p| integrity.strip_prefix(p).is_some_and(|b| b.len() >= 43));
    if !ok {
        return Err("integrity must be an SRI hash: sha256-…, sha384-… or sha512-…".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> ZenLibEntry {
        ZenLibEntry {
            id: "three".into(),
            version: "0.170.0".into(),
            title: "three.js".into(),
            global: "THREE".into(),
            license: "MIT".into(),
            license_file: "LICENSE".into(),
            copyright: "Copyright © 2010-2024 three.js authors".into(),
            source: LibSource::Bundled,
            files: vec![ZenLibFile { name: "three.min.js".into(), bytes: 10, sha256: "00".into(), url: None }],
            prebundled: Some("esbuild three/src/Three.js --bundle --format=iife --global-name=THREE".into()),
            notes: None,
        }
    }

    #[test]
    fn checked_in_manifest_loads_and_has_no_p5() {
        let m = zen_lib_manifest();
        assert_eq!(m.schema, 1);
        assert_eq!(m.max_bytes_per_widget, 8 * 1024 * 1024);
        assert!(!m.libs.iter().any(|l| l.id == "p5"), "R4: p5.js is dropped (LGPL)");
        assert!(!m.libs.iter().any(|l| l.id == "gsap"), "§13.9: no GSAP");
        for l in &m.libs {
            assert!(!l.files.is_empty(), "{} has files", l.key());
            if l.source == LibSource::Download {
                assert!(l.files.iter().all(|f| f.url.is_some()), "{} download files carry a url", l.key());
            }
        }
    }

    #[test]
    fn named_refs_match_by_dot_prefix() {
        let mut m = zen_lib_manifest().clone();
        m.libs.push(entry());
        for (want, hit) in [("0.170.0", true), ("0.170", true), ("0", true), ("0.17", false), ("0.171", false)] {
            let r = LibRef::from_json(&json!(format!("three@{want}"))).expect("parses");
            assert_eq!(find(&m, &r).is_some(), hit, "three@{want}");
        }
        assert!(LibRef::from_json(&json!("three")).is_err());
        assert!(LibRef::from_json(&json!("@1")).is_err());
        assert!(LibRef::from_json(&json!(3)).is_err());
    }

    #[test]
    fn cdn_refs_are_checked() {
        let m = zen_lib_manifest();
        let sri = format!("sha384-{}", "A".repeat(64));
        let good = "https://cdn.jsdelivr.net/npm/x@1.2.3/dist/x.min.js";
        let r = LibRef::from_json(&json!({"url": good, "integrity": sri})).expect("parses");
        assert_eq!(r, LibRef::Cdn { url: good.into(), integrity: sri.clone() });
        assert_eq!(check_cdn(m, good, &sri), Ok(()));
        assert!(check_cdn(m, "http://cdn.jsdelivr.net/npm/x@1.2.3/x.js", &sri).is_err());
        assert!(check_cdn(m, "https://evil.example.test/x@1.2.3/x.js", &sri).is_err());
        assert!(check_cdn(m, "https://unpkg.com/x@latest/x.js", &sri).is_err());
        assert!(check_cdn(m, "https://unpkg.com/x@^1/x.js", &sri).is_err());
        assert!(check_cdn(m, good, "md5-abc").is_err());
        assert!(LibRef::from_json(&json!({"url": good})).is_err());
        assert!(LibRef::from_json(&json!({"url": good, "integrity": sri, "x": 1})).is_err());
    }
}
