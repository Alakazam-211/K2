//! `k2 app sdk add` in k2-core: the folder edits, the Sew kit checks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// One npm-pack tarball (`package/package.json` + `package/index.js`).
fn npm_tgz(pj: serde_json::Value) -> Vec<u8> {
    let js = format!("export const name = {};\n", pj["name"]);
    k2_server::pack(&[("index.js".into(), js.into_bytes()), ("package.json".into(), serde_json::to_vec_pretty(&pj).unwrap())])
}

/// A `.tar.gz` of `(path, bytes)` as they are (no `package/` prefix).
fn raw_tgz(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let gz = flate2::GzBuilder::new().mtime(0).write(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (p, b) in entries {
        let mut h = tar::Header::new_ustar();
        h.set_size(b.len() as u64);
        h.set_mode(0o644);
        tar.append_data(&mut h, p, b.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

#[derive(Clone, Copy, Default)]
struct KitOpts {
    /// Corrupt @sew/voice's tarball after kit.json was written.
    tamper_voice: bool,
    /// Give @sew/bff an install script.
    install_script: bool,
    /// Leave @sew/e2ee out.
    drop_e2ee: bool,
}

/// A kit with the real kit's shape (REL-050) at `v`.
fn kit_bytes(v: &str, o: KitOpts) -> Vec<u8> {
    use serde_json::json;
    let mut pkgs: Vec<(&str, serde_json::Value)> = vec![
        ("@sew/client", json!({})),
        ("@sew/e2ee", json!({"peerDependencies": {"livekit-client": "2.22.3"}})),
        (
            "@sew/voice",
            json!({"dependencies": {"@sew/client": v, "@sew/e2ee": v, "lucide-react": "1.48.0"},
                   "peerDependencies": {"react": "^19", "react-dom": "^19", "livekit-client": "2.22.3"}}),
        ),
        (
            "@sew/channel",
            json!({"dependencies": {"@sew/client": v, "@tanstack/react-virtual": "3.14.13", "emojibase-data": "17.0.0", "lucide-react": "1.48.0"},
                   "peerDependencies": {"react": "^19", "react-dom": "^19"}}),
        ),
        ("@sew/bff", json!({"dependencies": {"@sew/client": v, "undici": "8.11.2"}})),
    ];
    if o.drop_e2ee {
        pkgs.retain(|(n, _)| *n != "@sew/e2ee");
    }
    let mut entries = Vec::new();
    let mut rows = Vec::new();
    let mut sums = String::new();
    for (name, extra) in pkgs {
        let mut pj = json!({"name": name, "version": v, "license": "Apache-2.0", "type": "module"});
        for (k, x) in extra.as_object().unwrap() {
            pj[k] = x.clone();
        }
        if o.install_script && name == "@sew/bff" {
            pj["scripts"] = json!({"postinstall": "node evil.js"});
        }
        let b = npm_tgz(pj);
        let file = vendored_file_name(name, v);
        rows.push(json!({"name": name, "version": v, "file": file, "sha256": sha256_hex(&b)}));
        sums.push_str(&format!("{}  {file}\n", sha256_hex(&b)));
        let b = if o.tamper_voice && name == "@sew/voice" {
            npm_tgz(json!({"name": name, "version": v, "scripts": {"postinstall": "curl evil"}}))
        } else {
            b
        };
        entries.push((format!("sew-app-kit-{v}/{file}"), b));
    }
    let kit = json!({"kit": "sew-app-kit", "version": v, "packages": rows,
        "external": {"undici": "8.11.2"}, "peers": {"react": "^19"}});
    entries.push((format!("sew-app-kit-{v}/kit.json"), serde_json::to_vec_pretty(&kit).unwrap()));
    entries.push((format!("sew-app-kit-{v}/SHA256SUMS"), sums.into_bytes()));
    entries.push((format!("sew-app-kit-{v}/NOTICES"), b"Apache-2.0 notices\n".to_vec()));
    raw_tgz(&entries)
}

fn kit(v: &str) -> SewKit {
    let b = kit_bytes(v, KitOpts::default());
    sew_kit::parse(&b, &sha256_hex(&b)).expect("the fixture kit parses")
}

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn tmp(tag: &str) -> Dir {
    let p = std::env::temp_dir().join(format!("k2-app-sdk-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&p).unwrap();
    Dir(dunce::canonicalize(&p).unwrap())
}

fn read_json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn vendor_files(folder: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(folder.join("vendor")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

fn k2_file() -> String {
    format!("k2-server-{}.tgz", env!("CARGO_PKG_VERSION"))
}

#[test]
fn add_on_an_empty_folder_creates_package_json_and_vendor() {
    let d = tmp("empty");
    let app = d.0.join("My App");
    fs::create_dir(&app).unwrap();
    let r = add(&app, None).unwrap();
    assert_eq!(r.package_json, "created");
    assert_eq!(vendor_files(&app), vec![k2_file()]);
    assert_eq!(fs::read(app.join("vendor").join(k2_file())).unwrap(), k2_server::tarball(), "the box's tarball, byte for byte");
    let pj = read_json(&app.join("package.json"));
    assert_eq!(pj["name"], "my-app");
    assert_eq!(pj["private"], true);
    assert_eq!(pj["type"], "module");
    assert_eq!(pj["dependencies"]["@k2/server"], format!("file:vendor/{}", k2_file()));
    assert!(pj.get("overrides").is_none());
    assert_eq!(r.vendored.len(), 1);
    assert!(r.vendored[0].written);
    assert_eq!(r.vendored[0].sha256, k2_server::sha256());
    assert!(r.install.is_empty() && r.sew.is_none());
}

#[test]
fn add_keeps_an_existing_package_json_in_order_and_indent() {
    let d = tmp("existing");
    let text = "{\n    \"name\": \"jobs\",\n    \"version\": \"1.2.0\",\n    \"scripts\": {\n        \"start\": \"node server.mjs\"\n    },\n    \"dependencies\": {\n        \"zod\": \"^3.0.0\",\n        \"@k2/server\": \"file:sdk/server\"\n    },\n    \"devDependencies\": {\n        \"@k2/server\": \"^0.1.0\",\n        \"vitest\": \"^2\"\n    }\n}\n";
    fs::write(d.0.join("package.json"), text).unwrap();
    let r = add(&d.0, None).unwrap();
    assert_eq!(r.package_json, "updated");
    let out = fs::read_to_string(d.0.join("package.json")).unwrap();
    // Keys stay where they were; @k2/server is updated in place; the
    // 4-space indent is kept; the devDependencies duplicate is gone.
    let want = format!(
        "{{\n    \"name\": \"jobs\",\n    \"version\": \"1.2.0\",\n    \"scripts\": {{\n        \"start\": \"node server.mjs\"\n    }},\n    \"dependencies\": {{\n        \"zod\": \"^3.0.0\",\n        \"@k2/server\": \"file:vendor/{}\"\n    }},\n    \"devDependencies\": {{\n        \"vitest\": \"^2\"\n    }}\n}}\n",
        k2_file()
    );
    assert_eq!(out, want);
}

#[test]
fn a_rerun_is_idempotent_and_replaces_older_versions() {
    let d = tmp("rerun");
    fs::create_dir(d.0.join("vendor")).unwrap();
    // An older K2 SDK and an unrelated vendored file.
    fs::write(d.0.join("vendor/k2-server-0.40.1.tgz"), b"old").unwrap();
    fs::write(d.0.join("vendor/k2-server-extra-1.0.0.tgz"), b"not ours").unwrap();
    fs::write(d.0.join("vendor/left-pad-1.3.0.tgz"), b"theirs").unwrap();
    let first = add(&d.0, None).unwrap();
    assert_eq!(first.removed, vec!["vendor/k2-server-0.40.1.tgz".to_string()]);
    let pj1 = fs::read_to_string(d.0.join("package.json")).unwrap();
    let second = add(&d.0, None).unwrap();
    assert_eq!(second.package_json, "unchanged");
    assert!(second.removed.is_empty());
    assert!(!second.vendored[0].written, "the same bytes are not rewritten");
    assert_eq!(fs::read_to_string(d.0.join("package.json")).unwrap(), pj1);
    assert_eq!(vendor_files(&d.0), vec![k2_file(), "k2-server-extra-1.0.0.tgz".into(), "left-pad-1.3.0.tgz".into()]);
    // One entry, never two.
    assert_eq!(pj1.matches("@k2/server").count(), 1);
}

#[test]
fn a_bad_package_json_changes_nothing() {
    let d = tmp("badjson");
    fs::write(d.0.join("package.json"), "{ not json").unwrap();
    let e = add(&d.0, None).unwrap_err();
    assert_eq!(e.code, codes::PACKAGE_JSON);
    assert!(!d.0.join("vendor").exists(), "nothing written");
    fs::write(d.0.join("package.json"), "[1,2]").unwrap();
    assert_eq!(add(&d.0, None).unwrap_err().code, codes::PACKAGE_JSON);
    fs::write(d.0.join("package.json"), "{\"dependencies\": []}").unwrap();
    assert_eq!(add(&d.0, None).unwrap_err().code, codes::PACKAGE_JSON);
}

#[cfg(unix)]
#[test]
fn a_symlinked_vendor_or_package_json_is_refused() {
    let d = tmp("links");
    let outside = tmp("outside");
    let app = d.0.join("app");
    fs::create_dir(&app).unwrap();
    std::os::unix::fs::symlink(&outside.0, app.join("vendor")).unwrap();
    let e = add(&app, None).unwrap_err();
    assert_eq!(e.code, codes::SDK_FOLDER);
    assert!(e.hint.contains("symlink"), "{e}");
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0, "nothing written outside the folder");
    fs::remove_file(app.join("vendor")).unwrap();
    fs::write(outside.0.join("package.json"), "{}").unwrap();
    std::os::unix::fs::symlink(outside.0.join("package.json"), app.join("package.json")).unwrap();
    assert_eq!(add(&app, None).unwrap_err().code, codes::SDK_FOLDER);
    assert_eq!(fs::read_to_string(outside.0.join("package.json")).unwrap(), "{}");
}

#[test]
fn sew_adds_five_packages_overrides_and_the_install_list() {
    let d = tmp("sew");
    fs::write(d.0.join("package.json"), "{\n  \"name\": \"chat\",\n  \"dependencies\": {\n    \"react\": \"^19.0.0\"\n  }\n}\n").unwrap();
    let k = kit("0.5.0");
    let r = add(&d.0, Some(&k)).unwrap();
    assert_eq!(r.sew.as_deref(), Some("0.5.0"));
    assert_eq!(
        vendor_files(&d.0),
        vec![
            k2_file(),
            "sew-NOTICES".into(),
            "sew-bff-0.5.0.tgz".into(),
            "sew-channel-0.5.0.tgz".into(),
            "sew-client-0.5.0.tgz".into(),
            "sew-e2ee-0.5.0.tgz".into(),
            "sew-voice-0.5.0.tgz".into(),
        ]
    );
    // The vendored bytes are the kit's (their shas are kit.json's).
    for p in &k.packages {
        assert_eq!(fs::read(d.0.join("vendor").join(p.file_name())).unwrap(), p.bytes);
    }
    let pj = read_json(&d.0.join("package.json"));
    for n in ["client", "voice", "channel", "e2ee", "bff"] {
        assert_eq!(pj["dependencies"][format!("@sew/{n}")], format!("file:vendor/sew-{n}-0.5.0.tgz"));
    }
    assert_eq!(pj["dependencies"]["react"], "^19.0.0", "the App's own deps stay");
    // Kit-internal deps (voice → client + e2ee, channel → client, bff →
    // client) are pinned to the vendored files.
    let ov: BTreeMap<String, String> = serde_json::from_value(pj["overrides"].clone()).unwrap();
    assert_eq!(
        ov,
        BTreeMap::from([
            ("@sew/client".into(), "file:vendor/sew-client-0.5.0.tgz".into()),
            ("@sew/e2ee".into(), "file:vendor/sew-e2ee-0.5.0.tgz".into()),
        ])
    );
    assert_eq!(r.overrides, vec!["@sew/client".to_string(), "@sew/e2ee".to_string()]);
    // External deps and peers stay plain semver (in the packages), listed.
    assert_eq!(
        r.peers,
        BTreeMap::from([("livekit-client".into(), "2.22.3".into()), ("react".into(), "^19".into()), ("react-dom".into(), "^19".into())])
    );
    assert_eq!(
        r.external,
        BTreeMap::from([
            ("@tanstack/react-virtual".into(), "3.14.13".into()),
            ("emojibase-data".into(), "17.0.0".into()),
            ("lucide-react".into(), "1.48.0".into()),
            ("undici".into(), "8.11.2".into()),
        ])
    );
    assert_eq!(
        r.install,
        "npm i livekit-client@2.22.3 react@^19 react-dom@^19 @tanstack/react-virtual@3.14.13 emojibase-data@17.0.0 lucide-react@1.48.0 undici@8.11.2"
    );
    // Rerun with a newer kit: older @sew files go, nothing duplicates.
    let k2 = kit("0.5.1");
    let r2 = add(&d.0, Some(&k2)).unwrap();
    assert_eq!(r2.removed.len(), 5, "{:?}", r2.removed);
    assert_eq!(vendor_files(&d.0).iter().filter(|f| f.starts_with("sew-client-")).count(), 1);
    let pj = read_json(&d.0.join("package.json"));
    assert_eq!(pj["overrides"]["@sew/client"], "file:vendor/sew-client-0.5.1.tgz");
    // A plain rerun (no --sew) leaves the Sew packages alone.
    let r3 = add(&d.0, None).unwrap();
    assert!(r3.removed.is_empty());
    assert_eq!(r3.package_json, "unchanged");
    assert!(d.0.join("vendor/sew-voice-0.5.1.tgz").exists());
}

#[test]
fn a_tampered_kit_is_refused() {
    let good = kit_bytes("0.5.0", KitOpts::default());
    // The kit itself against its pin.
    let e = sew_kit::parse(&good, &"0".repeat(64)).unwrap_err();
    assert_eq!(e.code, codes::SEW_KIT_TAMPERED);
    // A package tarball that doesn't match kit.json.
    let bad = kit_bytes("0.5.0", KitOpts { tamper_voice: true, ..Default::default() });
    let e = sew_kit::parse(&bad, &sha256_hex(&bad)).unwrap_err();
    assert_eq!(e.code, codes::SEW_KIT_TAMPERED, "{e}");
    assert!(e.hint.contains("sew-voice-0.5.0.tgz"), "{e}");
    // An install script, a missing package.
    let bad = kit_bytes("0.5.0", KitOpts { install_script: true, ..Default::default() });
    let e = sew_kit::parse(&bad, &sha256_hex(&bad)).unwrap_err();
    assert_eq!(e.code, codes::SEW_KIT_INVALID);
    assert!(e.hint.contains("postinstall"), "{e}");
    let bad = kit_bytes("0.5.0", KitOpts { drop_e2ee: true, ..Default::default() });
    let e = sew_kit::parse(&bad, &sha256_hex(&bad)).unwrap_err();
    assert!(e.hint.contains("@sew/e2ee"), "{e}");
}

#[test]
fn the_fixture_kit_parses() {
    // K2's committed fixture (the daemon's test pin) has the real shape.
    let b = include_bytes!("../../../k2-root-proto/testdata/sew/sew-app-kit-0.0.2.tar.gz");
    let k = sew_kit::parse(b, &sha256_hex(b)).unwrap();
    assert_eq!(k.version, "0.0.2");
    assert_eq!(k.packages.len(), 5);
    assert_eq!(k.internal_targets().into_iter().collect::<Vec<_>>(), vec!["@sew/client", "@sew/e2ee"]);
    assert!(k.notices.is_some());
}

#[test]
fn the_kit_store_keeps_one_read_only_copy_and_reverifies() {
    let _home = crate::test_env::TempHome::new();
    let v1 = kit_bytes("0.5.0", KitOpts::default());
    assert!(load_kit("0.5.0", &sha256_hex(&v1)).unwrap().is_none(), "none kept yet");
    store_kit(&v1, "0.5.0", &sha256_hex(&v1)).unwrap();
    let p = kit_path("0.5.0");
    assert!(fs::metadata(&p).unwrap().permissions().readonly());
    assert_eq!(load_kit("0.5.0", &sha256_hex(&v1)).unwrap().unwrap().version, "0.5.0");
    // A pin that no longer matches the kept bytes refuses them.
    assert_eq!(load_kit("0.5.0", &"a".repeat(64)).unwrap_err().code, codes::SEW_KIT_TAMPERED);
    // A newer kit replaces the old one.
    let v2 = kit_bytes("0.5.1", KitOpts::default());
    store_kit(&v2, "0.5.1", &sha256_hex(&v2)).unwrap();
    assert!(!p.exists());
    // The wrong version for the pin is refused.
    assert_eq!(store_kit(&v1, "0.5.1", &sha256_hex(&v1)).unwrap_err().code, codes::SEW_KIT_INVALID);
}

#[test]
fn status_compares_vendored_with_the_box() {
    let d = tmp("status");
    let rows = status(&d.0, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].name.as_str(), rows[0].state), ("@k2/server", "missing"));
    fs::create_dir(d.0.join("vendor")).unwrap();
    fs::write(d.0.join("vendor/k2-server-0.40.1.tgz"), b"x").unwrap();
    assert_eq!(status(&d.0, None).unwrap()[0].state, "older");
    add(&d.0, Some(&kit("0.5.0"))).unwrap();
    let rows = status(&d.0, Some("0.5.0")).unwrap();
    assert_eq!(rows.len(), 6);
    assert!(rows.iter().all(|r| r.state == "current"), "{rows:?}");
    let rows = status(&d.0, None).unwrap();
    assert!(rows.iter().filter(|r| r.name.starts_with("@sew/")).all(|r| r.state == "not_on_box"));
    let rows = status(&d.0, Some("0.6.0")).unwrap();
    assert!(rows.iter().filter(|r| r.name.starts_with("@sew/")).all(|r| r.state == "older"));
}
