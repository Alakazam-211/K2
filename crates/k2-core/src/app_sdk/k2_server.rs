//! `@k2/server` as an npm-pack tarball, at this K2's version (0.46.6).
//!
//! Every K2 carries it, the same way on macOS, Linux and Windows: the JS +
//! .d.ts in `sdk/server/dist` (committed, built from `sdk/server/src` by
//! `scripts/sdk-server-dist.mjs`) are compiled into k2-core with
//! `include_str!` (build.rs lists them), and [`tarball`] packs them on first
//! use. Why not ship the TypeScript sources: Node strips types only outside
//! `node_modules`, so an installed package must be plain JavaScript; and
//! why not pack in build.rs: the pack needs no build-time tool at all this
//! way (no node, no build-dependencies), and the bytes are still a pure
//! function of the embedded files and the version.
//!
//! The pack is reproducible: entries sorted under `package/`, mode 0644,
//! uid/gid 0, npm's fixed mtime (1985-10-26T08:15:00Z), gzip mtime 0. The
//! package.json is written here: name `@k2/server`, version = this K2's,
//! no scripts, no dependencies.

use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/sdk_server_dist.rs"));

/// The package name an App imports.
pub const NAME: &str = "@k2/server";
/// Where the entry point sits inside the package.
pub const ENTRY: &str = "dist/server/src/index.js";
/// Its types.
pub const TYPES: &str = "dist/server/src/index.d.ts";
/// npm pack's fixed mtime for every entry (1985-10-26T08:15:00Z).
pub const NPM_MTIME: u64 = 499_162_500;

/// The licence shipped in the package (K2's own).
const LICENSE: &str = include_str!("../../../../LICENSE.md");

/// This K2's version (the package's version).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `k2-server-<version>.tgz` (the name npm pack gives a scoped package).
pub fn file_name() -> String {
    format!("k2-server-{}.tgz", version())
}

/// The package.json inside the tarball.
pub fn package_json() -> String {
    let v = serde_json::json!({
        "name": NAME,
        "version": version(),
        "description": "K2's server SDK for App code: the App pass, guests, and every App-facing verb (typed from K2's verb catalog).",
        "license": "FSL-1.1-Apache-2.0",
        "type": "module",
        "engines": { "node": ">=22.18" },
        "main": format!("./{ENTRY}"),
        "types": format!("./{TYPES}"),
        "exports": { ".": { "types": format!("./{TYPES}"), "default": format!("./{ENTRY}") } },
    });
    let mut s = serde_json::to_string_pretty(&v).expect("a JSON value serializes");
    s.push('\n');
    s
}

/// Every `(path under package/, bytes)` the tarball holds, sorted.
pub fn entries() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = SDK_SERVER_DIST.iter().map(|(rel, text)| (format!("dist/{rel}"), text.as_bytes().to_vec())).collect();
    out.push(("package.json".into(), package_json().into_bytes()));
    out.push(("LICENSE.md".into(), LICENSE.as_bytes().to_vec()));
    out.sort();
    out
}

/// Pack `(path, bytes)` as an npm-pack style `.tgz` (entries under
/// `package/`), reproducibly.
pub fn pack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let gz = flate2::GzBuilder::new().mtime(0).write(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for (path, bytes) in entries {
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(NPM_MTIME);
        tar.append_data(&mut h, format!("package/{path}"), bytes.as_slice()).expect("append to an in-memory tar");
    }
    tar.into_inner().expect("finish the in-memory tar").finish().expect("finish the in-memory gzip")
}

/// The tarball (packed once per process).
pub fn tarball() -> &'static [u8] {
    static T: OnceLock<Vec<u8>> = OnceLock::new();
    T.get_or_init(|| pack(&entries()))
}

/// sha256 of [`tarball`], hex.
pub fn sha256() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| super::sha256_hex(tarball()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::path::PathBuf;

    fn unpack(tgz: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut a = tar::Archive::new(flate2::read::GzDecoder::new(tgz));
        let mut out = BTreeMap::new();
        for e in a.entries().expect("a tar") {
            let mut e = e.expect("an entry");
            assert_eq!(e.header().entry_type(), tar::EntryType::Regular, "only regular files");
            let p = e.path().expect("a path").to_string_lossy().to_string();
            let mut b = Vec::new();
            e.read_to_end(&mut b).expect("read an entry");
            assert!(out.insert(p.clone(), b).is_none(), "{p} twice");
        }
        out
    }

    #[test]
    fn the_tarball_is_a_valid_npm_pack_at_k2s_version() {
        let files = unpack(tarball());
        for p in files.keys() {
            assert!(p.starts_with("package/") && !p.contains(".."), "{p} is outside package/");
        }
        let pj: serde_json::Value = serde_json::from_slice(&files["package/package.json"]).expect("package.json is JSON");
        assert_eq!(pj["name"], "@k2/server");
        // K2's own version: the workspace version release.sh bumps.
        assert_eq!(pj["version"], env!("CARGO_PKG_VERSION"));
        let ws_toml = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../package.json")).unwrap();
        let root: serde_json::Value = serde_json::from_str(&ws_toml).unwrap();
        assert_eq!(pj["version"], root["version"], "the package's version is K2's (package.json)");
        assert!(pj.get("scripts").is_none(), "no scripts (so no install scripts)");
        assert!(pj.get("dependencies").is_none(), "no dependencies");
        assert!(pj.get("private").is_none(), "an installable package");
        assert_eq!(pj["type"], "module");
        // The entry and its types are in the tarball, and the entry is
        // JavaScript (Node won't strip types under node_modules).
        let entry = String::from_utf8(files[&format!("package/{ENTRY}")].clone()).unwrap();
        assert!(entry.contains("export function fromEnv("), "the entry exports fromEnv");
        assert!(!entry.contains(".ts'") && !entry.contains(".ts\""), "no .ts specifiers left in the JS");
        assert!(files.contains_key(&format!("package/{TYPES}")));
        // Every relative import in the JS resolves inside the package.
        for (p, b) in &files {
            if !p.ends_with(".js") {
                continue;
            }
            let text = String::from_utf8_lossy(b);
            for spec in text.split(['"', '\'']).filter(|s| s.starts_with("./") || s.starts_with("../")) {
                if !spec.ends_with(".js") {
                    continue;
                }
                let mut parts: Vec<&str> = p.split('/').collect();
                parts.pop();
                for seg in spec.split('/') {
                    match seg {
                        "." => {}
                        ".." => {
                            parts.pop();
                        }
                        s => parts.push(s),
                    }
                }
                let target = parts.join("/");
                assert!(files.contains_key(&target), "{p} imports {spec}: {target} is not in the tarball");
            }
        }
        assert!(files.contains_key("package/LICENSE.md"));
        assert_eq!(file_name(), format!("k2-server-{}.tgz", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn the_pack_is_reproducible() {
        let a = pack(&entries());
        let b = pack(&entries());
        assert_eq!(a, b);
        assert_eq!(super::super::sha256_hex(&a), sha256());
        // Every entry header is fixed: mode, owner, npm's mtime.
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(a.as_slice()));
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            let h = e.header();
            assert_eq!(h.mode().unwrap(), 0o644);
            assert_eq!(h.uid().unwrap(), 0);
            assert_eq!(h.mtime().unwrap(), NPM_MTIME);
        }
    }

    /// `sdk/server/dist` was built from the sources as they are now
    /// (`dist/SOURCES` holds each input's sha256), and lists every source.
    #[test]
    fn dist_is_fresh() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let regen = "sdk/server/dist is stale: run `node scripts/sdk-server-dist.mjs` (after bun install) and commit sdk/server/dist";
        let sources = std::fs::read_to_string(root.join("sdk/server/dist/SOURCES")).unwrap_or_else(|e| panic!("{e}: {regen}"));
        let mut listed = Vec::new();
        for line in sources.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let (sha, path) = line.split_once("  ").unwrap_or_else(|| panic!("SOURCES line {line:?}"));
            let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}; {regen}"));
            assert_eq!(super::super::sha256_hex(&bytes), sha, "{path} changed; {regen}");
            listed.push(path.to_string());
        }
        // Every source file of the package is an input (a new file too).
        fn ts_under(dir: &std::path::Path, base: &std::path::Path, acc: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    ts_under(&p, base, acc);
                } else if p.extension().is_some_and(|x| x == "ts") {
                    acc.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut src = Vec::new();
        ts_under(&root.join("sdk/server/src"), &root, &mut src);
        for s in &src {
            assert!(listed.contains(s), "{s} is not in dist/SOURCES; {regen}");
        }
        // The embedded list is the dist folder (build.rs), with JS for
        // every source under sdk/server/src.
        for s in &src {
            let js = s.trim_start_matches("sdk/").trim_end_matches(".ts").to_string() + ".js";
            assert!(SDK_SERVER_DIST.iter().any(|(p, _)| *p == js), "{js} is not in dist; {regen}");
        }
    }
}
