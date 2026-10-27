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

/// A fingerprint counts within this many bytes of a file's start (WL17 (b)).
pub const FINGERPRINT_WINDOW: usize = 1024;

/// The resolver's shared request → key cases (WL7): this file's tests and
/// `zen-lib-loader.test.ts` read the same fixture, so the Rust resolver and
/// the renderer's can't drift.
pub const RESOLVE_CASES: &str = include_str!("../../../../src/shared/zen-lib-resolve-cases.json");

/// The whole `zen-lib.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZenLibManifest {
    pub schema: u32,
    /// At most this many bytes of library text inlined into one widget
    /// (UWB13: 8 MB), outside UW8's code limit (1 MB from 0.46.5) and the
    /// page limit (3.5 MB).
    pub max_bytes_per_widget: u64,
    /// Hosts a widget's `{url, integrity}` entry may name (R2).
    pub cdn_hosts: Vec<String>,
    pub libs: Vec<ZenLibEntry>,
    /// Library lines K2 retired (0.46.5 WL6, vs-live L3, Rosson's call): a
    /// retired line is never deleted; its row becomes `download` (fetched on
    /// first use, refused under air-gap) and a widget that asks for it gets
    /// a warning naming the line to move to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<RetiredLine>,
}

/// One retired library line (`zen-lib.json` `retired`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetiredLine {
    pub id: String,
    /// The line (`18`, `0.186`).
    pub line: String,
    /// The K2 release that retired it (`0.47.0`).
    pub release: String,
    /// What to ask for instead (`marked@19`).
    #[serde(rename = "use")]
    pub use_instead: String,
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
    /// The exact version shipped: `0.186.0`.
    pub version: String,
    /// Its line ([`line_of`] of `version`; schema 2, a test checks it): what
    /// a widget asks for. One row per line; K2 updates patches inside it.
    pub line: String,
    /// Other libraries it needs, as line requests (`marked@18`): K2 loads
    /// them first, once (schema 2, WL3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Text a copy of this library starts with (its upstream banner, within
    /// the first [`FINGERPRINT_WINDOW`] bytes): how K2 spots a copy pasted
    /// into a widget (WL17 (b), vs-live L11). None for a library whose
    /// banner isn't its own (ECharts' ASF header).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fingerprints: Vec<String>,
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
    /// A file copied from an npm package: the sha256 of the package's own
    /// file before K2's inline rewrite (vs-live L11), so an agent's copy from
    /// npm or a CDN is recognised even where K2's bytes differ.
    #[serde(default, rename = "upstreamSha256", skip_serializing_if = "Option::is_none")]
    pub upstream_sha256: Option<String>,
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

/// A version's **line**: the part that may change its API (prd-widget-size-
/// and-shared-libs-v1 §6.2). The major from 1.0.0 up (`1.6.32` → `1`,
/// `3001.0.19` → `3001`), `0.minor` below 1 (`0.186.0` → `0.186`). A widget
/// asks for a line; K2 updates patches inside it.
pub fn line_of(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or(version);
    if major == "0" {
        match parts.next() {
            Some(minor) => format!("0.{minor}"),
            None => "0".to_string(),
        }
    } else {
        major.to_string()
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
    /// `"uplot@1"` (a line) or `"three@0.186"`: a library from the manifest.
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
                    .ok_or_else(|| format!("'{s}': name a library as <id>@<line>, like uplot@1 (k2 zen guide libs)"))?;
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

/// The manifest entry a named reference resolves to (the one resolver,
/// [`resolve_one`], warnings dropped). `None` for a CDN reference or a
/// request K2 can't serve.
pub fn find<'a>(m: &'a ZenLibManifest, r: &LibRef) -> Option<&'a ZenLibEntry> {
    let LibRef::Named { id, version } = r else { return None };
    resolve_one(m, id, version).ok().map(|(e, _)| e)
}

/// A version's numeric parts (`1.6.32` → `[1, 6, 32]`; a part's leading
/// digits only), for comparing versions numerically, never as text.
fn vparts(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u64>().unwrap_or(0))
        .collect()
}

/// `a` against `b`, numerically, a missing part counting as 0.
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (vparts(a), vparts(b));
    for i in 0..x.len().max(y.len()) {
        let o = x.get(i).copied().unwrap_or(0).cmp(&y.get(i).copied().unwrap_or(0));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

/// The retired entry for `(id, line)`, if K2 retired that line.
pub fn retired<'a>(m: &'a ZenLibManifest, id: &str, line: &str) -> Option<&'a RetiredLine> {
    m.retired.iter().find(|r| r.id == id && r.line == line)
}

/// **The one resolver** (prd-widget-size-and-shared-libs-v1 WL4–WL6, vs-live
/// L4): what `<id>@<want>` loads, and a warning when the widget should
/// write it differently. The daemon (validate, the payload's `libKeys`,
/// the shot tool) and the renderer (`zen-lib-loader.ts`, kept in step by
/// the shared fixture `src/shared/zen-lib-resolve-cases.json`) use it.
///
/// (a) Today's rule first, so every manifest that validated still does:
/// the exact version, or a dot-ended prefix of it; with several matches the
/// newest, numerically, and a warning when they span lines (`three@0`).
/// (b) Else the newest shipped row in the same line whose version is at
/// least the request: K2 updated the patch (`uplot@1.6.31` → `1.6.32`),
/// with a warning to write the line. (c) Else an error naming the lines K2
/// ships; a request newer than K2's row is an error, never a downgrade.
/// A retired line still loads (it downloads on first use) and warns.
pub fn resolve_one<'a>(m: &'a ZenLibManifest, id: &str, want: &str) -> Result<(&'a ZenLibEntry, Option<String>), String> {
    let rows: Vec<&ZenLibEntry> = m.libs.iter().filter(|e| e.id == id).collect();
    if rows.is_empty() {
        return Err(format!("K2's widget library has no {id}@{want}; see k2 zen guide libs for what's available"));
    }
    let newest = |v: Vec<&'a ZenLibEntry>| v.into_iter().max_by(|a, b| version_cmp(&a.version, &b.version));
    let mut warning: Option<String> = None;
    let matched: Vec<&ZenLibEntry> = rows.iter().copied().filter(|e| e.matches_version(want)).collect();
    let pick = if let Some(e) = newest(matched.clone()) {
        let mut lines: Vec<&str> = matched.iter().map(|e| e.line.as_str()).collect();
        lines.sort();
        lines.dedup();
        if lines.len() > 1 {
            warning = Some(format!("{id}@{want} spans versions that break each other; write {id}@{}", e.line));
        }
        e
    } else {
        let line = line_of(want);
        let forward: Vec<&ZenLibEntry> = rows
            .iter()
            .copied()
            .filter(|e| e.line == line && version_cmp(&e.version, want) != std::cmp::Ordering::Less)
            .collect();
        match newest(forward) {
            Some(e) => {
                warning = Some(format!("K2 ships {id} {} and updates it inside line {}; write \"{id}@{}\"", e.version, e.line, e.line));
                e
            }
            None => {
                if let Some(e) = newest(rows.iter().copied().filter(|e| e.line == line).collect()) {
                    return Err(format!(
                        "K2 ships {id} {} in line {}; {id}@{want} is newer than K2's. Write \"{id}@{}\", or update K2",
                        e.version, e.line, e.line
                    ));
                }
                let mut shipped: Vec<&ZenLibEntry> = rows.clone();
                shipped.sort_by(|a, b| version_cmp(&a.version, &b.version));
                let list: Vec<String> = shipped.iter().map(|e| format!("{} ({})", e.line, e.version)).collect();
                return Err(format!(
                    "K2 ships {id} line{} {}; this widget asks for {id}@{want} (k2 zen guide libs)",
                    if list.len() == 1 { "" } else { "s" },
                    list.join(", ")
                ));
            }
        }
    };
    if let Some(r) = retired(m, id, &pick.line) {
        let w = format!(
            "K2 retired {id}@{} in {}: it downloads on first use now (not under air-gap); move to {}",
            pick.line, r.release, r.use_instead
        );
        warning = Some(match warning {
            Some(prev) => format!("{prev}; {w}"),
            None => w,
        });
    }
    Ok((pick, warning))
}

/// One finding from [`resolve`]: about `requires.libs` item `index` (`None`:
/// the whole list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibFinding {
    pub index: Option<usize>,
    pub error: bool,
    pub message: String,
}

/// What a widget's `requires.libs` loads, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    /// The load order (`libKeys`): exact `<id>@<version>` keys for K2's rows,
    /// each one's `needs` first, every key once; CDN entries as written.
    pub keys: Vec<J>,
    /// Bytes of K2's rows (needs included, vs-live L4).
    pub bytes: u64,
    /// A CDN file's size isn't known until it is fetched (counted as 0).
    pub unknown_bytes: bool,
    pub findings: Vec<LibFinding>,
}

impl Resolved {
    pub fn errors(&self) -> impl Iterator<Item = &LibFinding> {
        self.findings.iter().filter(|f| f.error)
    }
}

/// Resolve a whole `requires.libs` list (WL3–WL7, vs-live L4): every item
/// through [`resolve_one`]; a library's `needs` load before it; every key
/// loads once (a need listed again is silent, an item listed twice warns);
/// two lines of one library is an error (both would define its global);
/// over the per-widget budget is an error.
pub fn resolve(m: &ZenLibManifest, libs: &[J]) -> Resolved {
    let mut out = Resolved::default();
    // key → (id, line, explicit)
    let mut loaded: Vec<(String, String, String, bool)> = Vec::new();
    fn add<'a>(
        m: &'a ZenLibManifest,
        out: &mut Resolved,
        loaded: &mut Vec<(String, String, String, bool)>,
        index: usize,
        e: &'a ZenLibEntry,
        explicit: bool,
        depth: usize,
    ) {
        if depth > 8 {
            out.findings.push(LibFinding { index: Some(index), error: true, message: format!("{}: its needs go round in a circle", e.key()) });
            return;
        }
        for need in &e.needs {
            let Some((nid, nwant)) = need.rsplit_once('@') else {
                out.findings.push(LibFinding { index: Some(index), error: true, message: format!("{} needs '{need}', which isn't <id>@<line>", e.key()) });
                continue;
            };
            match resolve_one(m, nid, nwant) {
                Ok((ne, _)) => add(m, out, loaded, index, ne, false, depth + 1),
                Err(msg) => out.findings.push(LibFinding { index: Some(index), error: true, message: format!("{} needs {need}: {msg}", e.key()) }),
            }
        }
        let key = e.key();
        if let Some(prev) = loaded.iter_mut().find(|(k, ..)| *k == key) {
            if explicit && prev.3 {
                out.findings.push(LibFinding { index: Some(index), error: false, message: format!("'{key}' is listed twice; K2 loads it once") });
            }
            prev.3 |= explicit;
            return;
        }
        if let Some((other, ..)) = loaded.iter().find(|(_, id, line, _)| *id == e.id && *line != e.line) {
            out.findings.push(LibFinding {
                index: Some(index),
                error: true,
                message: format!("two versions of {} in one widget ({other} and {key}) define {} twice; ask for one line", e.id, e.global),
            });
            return;
        }
        out.bytes += e.total_bytes();
        out.keys.push(J::String(key.clone()));
        loaded.push((key, e.id.clone(), e.line.clone(), explicit));
    }
    for (i, item) in libs.iter().enumerate() {
        match LibRef::from_json(item) {
            Err(e) => out.findings.push(LibFinding { index: Some(i), error: true, message: e }),
            Ok(LibRef::Named { id, version }) => match resolve_one(m, &id, &version) {
                Err(msg) => out.findings.push(LibFinding { index: Some(i), error: true, message: msg }),
                Ok((e, warn)) => {
                    if let Some(w) = warn {
                        out.findings.push(LibFinding { index: Some(i), error: false, message: w });
                    }
                    add(m, &mut out, &mut loaded, i, e, true, 0);
                }
            },
            Ok(LibRef::Cdn { url, integrity }) => match check_cdn(m, &url, &integrity) {
                Err(e) => out.findings.push(LibFinding { index: Some(i), error: true, message: e }),
                Ok(()) => {
                    if out.keys.contains(item) {
                        out.findings.push(LibFinding { index: Some(i), error: false, message: format!("{url} is listed twice; K2 loads it once") });
                    } else {
                        out.unknown_bytes = true;
                        out.keys.push(item.clone());
                    }
                }
            },
        }
    }
    if out.bytes > m.max_bytes_per_widget {
        out.findings.push(LibFinding {
            index: None,
            error: true,
            message: format!(
                "these libraries are {} bytes (with what they need); a widget may load at most {} (8 MB)",
                out.bytes, m.max_bytes_per_widget
            ),
        });
    }
    out
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
            line: "0.170".into(),
            needs: Vec::new(),
            fingerprints: Vec::new(),
            title: "three.js".into(),
            global: "THREE".into(),
            license: "MIT".into(),
            license_file: "LICENSE".into(),
            copyright: "Copyright © 2010-2024 three.js authors".into(),
            source: LibSource::Bundled,
            files: vec![ZenLibFile { name: "three.min.js".into(), bytes: 10, sha256: "00".into(), upstream_sha256: None, url: None }],
            prebundled: Some("esbuild three/src/Three.js --bundle --format=iife --global-name=THREE".into()),
            notes: None,
        }
    }

    #[test]
    fn checked_in_manifest_loads_and_has_no_p5() {
        let m = zen_lib_manifest();
        assert_eq!(m.schema, 2, "0.46.5: lines, needs and retired lines");
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

    /// The vendor rules (WL6, L3): each row's `line` is its version's line;
    /// one row per line; every `needs` names a line K2 ships; a retired line
    /// keeps a row, demoted to `download` (never deleted).
    #[test]
    fn the_manifest_keeps_one_row_per_line_and_never_drops_a_retired_line() {
        let m = zen_lib_manifest();
        let mut seen: Vec<(String, String)> = Vec::new();
        for e in &m.libs {
            assert_eq!(e.line, line_of(&e.version), "{}: line must be {}", e.key(), line_of(&e.version));
            let k = (e.id.clone(), e.line.clone());
            assert!(!seen.contains(&k), "{}@{}: two rows in one line (K2 updates patches in place)", e.id, e.line);
            seen.push(k);
            for need in &e.needs {
                let (id, want) = need.rsplit_once('@').unwrap_or_else(|| panic!("{}: need {need} isn't <id>@<line>", e.key()));
                let (n, warn) = resolve_one(m, id, want).unwrap_or_else(|err| panic!("{}: need {need}: {err}", e.key()));
                assert_eq!(n.line, want, "{}: need {need} names a line exactly", e.key());
                assert!(warn.is_none(), "{}: need {need}: {warn:?}", e.key());
            }
        }
        for r in &m.retired {
            let row = m.libs.iter().find(|e| e.id == r.id && e.line == r.line).unwrap_or_else(|| panic!("retired {}@{} has no row: never delete a line", r.id, r.line));
            assert_eq!(row.source, LibSource::Download, "retired {}@{} downloads on first use", r.id, r.line);
            let (to, _) = r.use_instead.rsplit_once('@').expect("use is <id>@<line>");
            assert!(m.libs.iter().any(|e| e.id == to && format!("{}@{}", e.id, e.line) == r.use_instead), "retired {}@{}: {} ships", r.id, r.line, r.use_instead);
        }
    }

    /// The shared fixture (WL7): the same request → key cases the vitest
    /// twin reads, against its own small manifest and against K2's.
    #[test]
    fn the_shared_resolver_fixture_passes() {
        let f: J = serde_json::from_str(RESOLVE_CASES).expect("fixture json");
        let m: ZenLibManifest = serde_json::from_value(f["manifest"].clone()).expect("fixture manifest");
        let mut n = 0;
        for c in f["cases"].as_array().expect("cases") {
            let libs = c["libs"].as_array().expect("libs").clone();
            let r = resolve(&m, &libs);
            let errors: Vec<&str> = r.findings.iter().filter(|x| x.error).map(|x| x.message.as_str()).collect();
            let warnings: Vec<&str> = r.findings.iter().filter(|x| !x.error).map(|x| x.message.as_str()).collect();
            let want_errors: Vec<&str> = c["errors"].as_array().map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
            let want_warnings: Vec<&str> = c["warnings"].as_array().map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
            assert_eq!(errors.len(), want_errors.len(), "{}: errors {errors:?}", c["name"]);
            for (got, want) in errors.iter().zip(&want_errors) {
                assert!(got.contains(want), "{}: error {got:?} should say {want:?}", c["name"]);
            }
            assert_eq!(warnings.len(), want_warnings.len(), "{}: warnings {warnings:?}", c["name"]);
            for (got, want) in warnings.iter().zip(&want_warnings) {
                assert!(got.contains(want), "{}: warning {got:?} should say {want:?}", c["name"]);
            }
            if errors.is_empty() {
                assert_eq!(J::Array(r.keys.clone()), c["keys"], "{}", c["name"]);
            }
            n += 1;
        }
        assert!(n >= 10, "the fixture has its cases");
        // Today's forms against K2's own manifest resolve to the same line, clean.
        for c in f["real"].as_array().expect("real") {
            let req = c["request"].as_str().expect("request");
            let (id, want) = req.rsplit_once('@').expect("id@want");
            let (e, warn) = resolve_one(zen_lib_manifest(), id, want).unwrap_or_else(|err| panic!("{req}: {err}"));
            assert_eq!(e.line, c["line"].as_str().expect("line"), "{req}");
            assert_eq!(warn.is_some(), c["warns"].as_bool().unwrap_or(false), "{req}: {warn:?}");
        }
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
