//! One-click suite Apps, the core half: the compiled-in catalog, the
//! agreement checks, the safe unpack, Sew's seeds.json, addresses and the
//! job store. Every check here fails loudly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::testkit::{self, Raw};
use super::*;

fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("k2-suite-{tag}-{}", uuid::Uuid::new_v4().simple()))
}

fn write(p: &std::path::Path, b: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

// ── the catalog ──────────────────────────────────────────────────────

/// R25/R28: the file compiled into this K2 parses, passes the check, and
/// names Hearth (chat family) and Meet (meet family, one per box), each
/// under the licence Rosson ruled (2026-11-04).
#[test]
fn the_compiled_catalog_is_well_formed() {
    let c = catalog().expect("the compiled-in catalog loads");
    let h = c.entry("sew/hearth").expect("Hearth");
    assert_eq!((h.family.as_str(), h.default_label.as_str(), h.install_id.as_str(), h.one_per_box), ("chat", "chat", "hearth", false));
    let m = c.entry("SEW/Meet ").expect("Meet, case-insensitive");
    assert_eq!((m.family.as_str(), m.default_label.as_str(), m.install_id.as_str(), m.one_per_box), ("meet", "meet", "sew-meet", true));
    for e in &c.entries {
        assert_eq!(e.license, "MIT OR Apache-2.0", "{}", e.entry);
        // R7: the install id fits the guarded engine (≤ 32, no --) with room
        // for a -NN suffix, and is a free App id.
        assert!(crate::deploy::app_id_problem(&e.install_id).is_none(), "{}", e.install_id);
        // The family label is reserved: an App id can never be it.
        assert!(crate::skin::is_reserved_nested_label(&e.default_label), "{}", e.default_label);
    }
    assert!(c.entry("sew/mail").is_none());
}

/// R27: an unknown setup step fails the catalog, so an older K2 never
/// half-installs a newer entry.
#[test]
fn an_unknown_setup_step_fails_the_catalog() {
    let mut c = catalog().unwrap().clone();
    let e = c.entries.iter_mut().find(|e| e.entry == "sew/meet").unwrap();
    e.versions.push(testkit::version("meet", "0.2.4", "k2", b"t", &"a".repeat(64), testkit::K2_CAPS, &["sew-api"], &[SETUP_MEET_INIT]));
    check(&c).expect("a known step passes");
    c.entries[1].versions[0].setup.push("sew.rebind_everything".into());
    let err = check(&c).expect_err("unknown step");
    assert!(err.starts_with(codes::SETUP_UNKNOWN), "{err}");
}

/// A version must pin the source tarball and every file by sha256, and a
/// file name can't carry a path.
#[test]
fn a_version_pins_every_file_plainly() {
    let mut c = catalog().unwrap().clone();
    let v = testkit::version("hearth", "0.2.4", "k2", b"t", &"b".repeat(64), testkit::K2_CAPS, &["sew-api"], &[]);
    c.entries[0].versions.push(v);
    check(&c).expect("well formed");
    let mut bad = c.clone();
    bad.entries[0].versions[0].source.sha256 = "nothex".into();
    assert!(check(&bad).unwrap_err().contains("source"), "source sha256 checked");
    let mut bad = c.clone();
    bad.entries[0].versions[0].variants.get_mut("k2").unwrap().file = "../hearth.tar.gz".into();
    assert!(check(&bad).is_err(), "a path in a file name");
    let mut bad = c.clone();
    bad.entries[0].versions[0].variants.get_mut("k2").unwrap().seed_hash = "0".repeat(63);
    assert!(check(&bad).is_err(), "seedHash checked");
    let mut bad = c;
    bad.entries[0].stable = Some("9.9.9".into());
    assert!(check(&bad).is_err(), "stable must be listed");
}

#[test]
fn an_entry_with_no_pinned_version_says_so() {
    let c = catalog().unwrap();
    let (code, hint) = c.entry("sew/hearth").unwrap().version(None).expect_err("nothing pinned yet");
    assert_eq!(code, codes::NOT_PINNED);
    assert!(hint.contains("Hearth"), "{hint}");
}

#[test]
fn the_version_asked_else_stable_else_newest() {
    let mut e = catalog().unwrap().entry("sew/hearth").unwrap().clone();
    for v in ["0.2.4", "0.10.0", "0.3.1"] {
        e.versions.push(testkit::version("hearth", v, "k2", b"t", &"c".repeat(64), &[], &[], &[]));
    }
    assert_eq!(e.version(None).unwrap().version, "0.10.0", "newest by semver, not by string");
    e.stable = Some("0.3.1".into());
    assert_eq!(e.version(None).unwrap().version, "0.3.1");
    assert_eq!(e.version(Some("0.2.4")).unwrap().version, "0.2.4");
    assert_eq!(e.version(Some("0.9.0")).unwrap_err().0, codes::VERSION_NOT_FOUND);
}

/// R27 `sew.require_identity_match`: the variant follows the Sew identity;
/// forcing the other one is `variant_mismatch`.
#[test]
fn the_variant_follows_the_sew_identity() {
    let mut v = testkit::version("hearth", "0.2.4", "k2", b"a", &"d".repeat(64), testkit::K2_CAPS, &[], &[]);
    let st = testkit::version("hearth", "0.2.4", "standalone", b"b", &"e".repeat(64), &[], &[], &[]);
    v.variants.extend(st.variants);
    assert_eq!(pick_variant(&v, "k2", None).unwrap().0, "k2");
    assert_eq!(pick_variant(&v, "standalone", None).unwrap().0, "standalone");
    assert_eq!(pick_variant(&v, "k2", Some("k2")).unwrap().0, "k2");
    let (code, hint) = pick_variant(&v, "standalone", Some("k2")).unwrap_err();
    assert_eq!(code, codes::VARIANT_MISMATCH);
    assert!(hint.contains("breaks sign-in"), "{hint}");
    assert_eq!(pick_variant(&v, "weird", None).unwrap_err().0, codes::VARIANT_MISMATCH);
}

// ── agreement (R26, A12) ─────────────────────────────────────────────

fn pin(caps: &[&str], boxes: &[&str]) -> Pin {
    let v = testkit::version("hearth", "0.2.4", "k2", b"x", &"f".repeat(64), caps, boxes, &[]);
    let e = catalog().unwrap().entry("sew/hearth").unwrap().clone();
    Pin::of(&e, &v, "k2").unwrap()
}

fn manifest_of(v: serde_json::Value) -> Manifest {
    crate::deploy::manifest::parse(&v.to_string()).expect("a valid manifest")
}

#[test]
fn a_seed_that_agrees_with_its_pin_passes() {
    let p = pin(testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    let m = manifest_of(testkit::manifest("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-signal", "sew-api"]));
    agree(&p, &m, Some(&testkit::build_json("hearth", "0.2.4", "k2", testkit::COMMIT))).expect("agrees (order doesn't matter)");
}

/// AT6: caps differ → `catalog_mismatch (caps)`; the same for version, box
/// services and build.json.
#[test]
fn a_manifest_that_disagrees_is_a_named_mismatch() {
    let p = pin(testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    let b = testkit::build_json("hearth", "0.2.4", "k2", testkit::COMMIT);
    let m = manifest_of(testkit::manifest("hearth", "0.2.4", "k2", &["chat:read", "chat:post", "voice:join"], &["sew-api", "sew-signal"]));
    let e = agree(&p, &m, Some(&b)).unwrap_err();
    assert_eq!(e.field, "caps");
    assert!(e.hint().starts_with("catalog_mismatch (caps)"), "{}", e.hint());
    let m = manifest_of(testkit::manifest("hearth", "0.2.5", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"]));
    assert_eq!(agree(&p, &m, Some(&b)).unwrap_err().field, "version");
    let m = manifest_of(testkit::manifest("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api"]));
    assert_eq!(agree(&p, &m, Some(&b)).unwrap_err().field, "boxServices");
    let m = manifest_of(testkit::manifest("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"]));
    assert_eq!(agree(&p, &m, None).unwrap_err().field, "build.json");
    for bad in [
        testkit::build_json("meet", "0.2.4", "k2", testkit::COMMIT),
        testkit::build_json("hearth", "0.2.4", "standalone", testkit::COMMIT),
        testkit::build_json("hearth", "0.2.4", "k2", &"2".repeat(40)),
    ] {
        assert_eq!(agree(&p, &m, Some(&bad)).unwrap_err().field, "build.json", "{bad}");
    }
}

/// A12: the catalog's `requires` may be stricter than the Seed's, never
/// looser.
#[test]
fn requires_may_only_be_stricter() {
    let r = |v: serde_json::Value| -> Requires { serde_json::from_value(v).unwrap() };
    let seed = r(serde_json::json!({ "k2": ">=0.46.3", "sew": ">=0.2.0", "voice": true }));
    requires_ok(&r(serde_json::json!({ "k2": ">=0.46.9", "sew": ">=0.7.2", "voice": true })), Some(&seed)).expect("stricter");
    requires_ok(&seed, Some(&seed)).expect("the same");
    assert_eq!(requires_ok(&r(serde_json::json!({ "k2": ">=0.46.0", "sew": ">=0.2.0", "voice": true })), Some(&seed)).unwrap_err().field, "requires.k2");
    assert_eq!(requires_ok(&r(serde_json::json!({ "k2": ">=0.46.3", "voice": true })), Some(&seed)).unwrap_err().field, "requires.sew");
    assert_eq!(requires_ok(&r(serde_json::json!({ "k2": ">=0.46.3", "sew": ">=0.2.0" })), Some(&seed)).unwrap_err().field, "requires.voice");
}

#[test]
fn the_health_answer_must_name_what_was_installed() {
    let p = pin(&[], &[]);
    let ok = serde_json::json!({ "ok": true, "app": "hearth", "version": "0.2.4", "variant": "k2", "commit": testkit::COMMIT });
    assert_eq!(health_mismatch(&p, &ok), None);
    let old = serde_json::json!({ "ok": true, "app": "hearth", "version": "0.2.3", "variant": "k2", "commit": testkit::COMMIT });
    assert!(health_mismatch(&p, &old).unwrap().contains("version"), "an update that didn't land");
    assert!(health_mismatch(&p, &serde_json::json!({ "ok": true })).is_some(), "a Seed that reports nothing");
}

// ── unpack (A13) ─────────────────────────────────────────────────────

fn good_tarball() -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let m = testkit::manifest("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api"]);
    let files = testkit::seed_files(&m, &testkit::build_json("hearth", "0.2.4", "k2", testkit::COMMIT));
    (testkit::tar_gz(&files), files)
}

#[test]
fn a_publisher_seed_unpacks_and_its_sums_pass() {
    let dir = scratch("tgz");
    let tgz = dir.with_extension("tgz");
    let (bytes, files) = good_tarball();
    write(&tgz, &bytes);
    let out = dir.join("seed");
    let u = unpack::unpack_tar_gz(&tgz, &out, unpack::Limits::seed()).expect("unpacks");
    assert_eq!(u.files, files.len());
    for (p, b) in &files {
        assert_eq!(&std::fs::read(out.join(p)).unwrap(), b, "{p}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o700, "a private folder");
    }
    assert_eq!(unpack::verify_sums(&out).expect("sums pass"), files.len() - 1);
    // AT5-style: one changed byte in a shipped file → named.
    std::fs::write(out.join("server.js"), b"import('./evil.js')\n").unwrap();
    let (f, d) = unpack::verify_sums(&out).unwrap_err();
    assert_eq!(f, "server.js", "{d}");
    std::fs::write(out.join("server.js"), &files.iter().find(|(p, _)| p == "server.js").unwrap().1).unwrap();
    std::fs::write(out.join("extra.js"), b"x").unwrap();
    assert_eq!(unpack::verify_sums(&out).unwrap_err().0, "extra.js", "a file the sums don't list");
    std::fs::remove_file(out.join("extra.js")).unwrap();
    std::fs::remove_file(out.join("dist/server/main.js")).unwrap();
    assert_eq!(unpack::verify_sums(&out).unwrap_err().0, "dist/server/main.js", "a listed file that's missing");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&tgz);
    // The destination must be fresh.
    let again = scratch("again");
    std::fs::create_dir_all(&again).unwrap();
    write(&tgz, &bytes);
    assert!(unpack::unpack_tar_gz(&tgz, &again, unpack::Limits::seed()).unwrap_err().contains("already exists"));
    let _ = std::fs::remove_dir_all(&again);
    let _ = std::fs::remove_file(&tgz);
}

/// AT20: a `..` path, an absolute path, a symlink, a hardlink, a device, a
/// FIFO, a path twice and a too-big file are each refused before anything
/// is stored, and the folder is gone afterwards.
#[test]
fn a_hostile_tarball_is_refused_and_leaves_nothing() {
    let cases: Vec<(&str, Vec<u8>, unpack::Limits)> = vec![
        ("..", testkit::tar_gz_raw(&[Raw { name: b"../evil.js", kind: tar::EntryType::Regular, data: b"x", link: None }]), unpack::Limits::seed()),
        ("absolute", testkit::tar_gz_raw(&[Raw { name: b"/tmp/evil.js", kind: tar::EntryType::Regular, data: b"x", link: None }]), unpack::Limits::seed()),
        ("symlink", testkit::tar_gz_raw(&[Raw { name: b"server.js", kind: tar::EntryType::Symlink, data: b"", link: Some(b"/etc/passwd") }]), unpack::Limits::seed()),
        ("hardlink", testkit::tar_gz_raw(&[Raw { name: b"server.js", kind: tar::EntryType::Link, data: b"", link: Some(b"manifest.json") }]), unpack::Limits::seed()),
        ("device", testkit::tar_gz_raw(&[Raw { name: b"dev", kind: tar::EntryType::Char, data: b"", link: None }]), unpack::Limits::seed()),
        ("FIFO", testkit::tar_gz_raw(&[Raw { name: b"pipe", kind: tar::EntryType::Fifo, data: b"", link: None }]), unpack::Limits::seed()),
        (
            "twice",
            testkit::tar_gz_raw(&[
                Raw { name: b"a.js", kind: tar::EntryType::Regular, data: b"1", link: None },
                Raw { name: b"./a.js", kind: tar::EntryType::Regular, data: b"2", link: None },
            ]),
            unpack::Limits::seed(),
        ),
        (
            "bytes",
            testkit::tar_gz(&[("big.bin".to_string(), vec![0u8; 64 * 1024])]),
            unpack::Limits { max_entries: 10, max_bytes: 1024 },
        ),
        (
            "entries",
            testkit::tar_gz(&[("a".to_string(), b"1".to_vec()), ("b".to_string(), b"2".to_vec()), ("c".to_string(), b"3".to_vec())]),
            unpack::Limits { max_entries: 2, max_bytes: 1024 },
        ),
    ];
    for (want, bytes, limits) in cases {
        let dir = scratch("evil");
        let tgz = dir.with_extension("tgz");
        write(&tgz, &bytes);
        let err = unpack::unpack_tar_gz(&tgz, &dir, limits).expect_err(want);
        let expect = match want {
            "twice" => "listed twice",
            "bytes" => "bytes of files",
            "entries" => "entries",
            w => w,
        };
        assert!(err.contains(expect), "{want}: {err}");
        assert!(!dir.exists(), "{want}: the folder is removed");
        assert!(!std::env::temp_dir().join("evil.js").exists() || want != "..", "nothing written outside");
        let _ = std::fs::remove_file(&tgz);
    }
}

// ── seeds.json (pin day) ─────────────────────────────────────────────

fn hashes(vs: &[&str]) -> BTreeMap<String, String> {
    vs.iter().map(|v| (v.to_string(), "9".repeat(64))).collect()
}

/// Sew's seeds.json format (sew-web docs/hosting-on-k2.md "One-click
/// Seeds", the 0.2.4 release in its own `seeds-0.2.4` tag): each App
/// becomes one catalog version with both variants and its source tarball;
/// Meet's Sew steps map onto K2's vocabulary.
#[test]
fn sews_seeds_json_becomes_catalog_versions() {
    let doc = seeds_json::parse(include_str!("testdata/seeds-0.2.4.json")).expect("the fixture parses");
    let h = seeds_json::version_for(&doc, "hearth", "seeds-0.2.4", &hashes(&["k2", "standalone"])).expect("hearth");
    assert_eq!((h.version.as_str(), h.tag.as_str(), h.commit.as_str()), ("0.2.4", "seeds-0.2.4", testkit::COMMIT));
    assert_eq!(h.variants.keys().collect::<Vec<_>>(), vec!["k2", "standalone"]);
    assert_eq!(h.variants["k2"].caps, testkit::K2_CAPS);
    assert!(h.variants["standalone"].caps.is_empty());
    assert_eq!(h.variants["k2"].box_services, vec!["sew-api", "sew-signal"]);
    assert_eq!(h.source.file, "hearth-0.2.4-src.tar.gz");
    assert_eq!(h.source.size, 900000);
    assert_eq!(h.node.as_deref(), Some(">=22.13.0"), "the Node floor lives in the catalog only (A11)");
    assert!(h.setup.is_empty(), "Hearth needs no Sew setup");
    assert_eq!(h.data, vec!["hearth-sessions.sqlite"]);
    let m = seeds_json::version_for(&doc, "meet", "seeds-0.2.4", &hashes(&["k2"])).expect("meet");
    assert_eq!(m.setup, vec![SETUP_MEET_INIT, SETUP_APP_SPACE_RECORD, SETUP_MEETINGS_ENABLED]);
    assert_eq!(m.source.file, "meet-0.2.4-src.tar.gz");
    // Into a catalog: it passes the same check the compiled file does.
    let mut c = catalog().unwrap().clone();
    c.entries[0].versions.push(h);
    c.entries[1].versions.push(m);
    check(&c).expect("pinned versions pass the catalog check");
}

#[test]
fn seeds_json_refusals() {
    let text = include_str!("testdata/seeds-0.2.4.json");
    let doc = seeds_json::parse(text).unwrap();
    assert!(seeds_json::version_for(&doc, "hearth", "seeds-0.2.4", &hashes(&["k2"])).unwrap_err().contains("no seedHash"));
    assert!(seeds_json::version_for(&doc, "mail", "seeds-0.2.4", &hashes(&[])).is_err());
    let unknown = text.replacen("\"step\": \"meetings-enabled\"", "\"step\": \"drop-everything\"", 1);
    let doc = seeds_json::parse(&unknown).unwrap();
    assert!(seeds_json::version_for(&doc, "meet", "seeds-0.2.4", &hashes(&["k2"])).unwrap_err().starts_with(codes::SETUP_UNKNOWN));
    let other_source = text.replacen("\"size\": 900000", "\"size\": 900001", 1);
    let doc = seeds_json::parse(&other_source).unwrap();
    assert!(seeds_json::version_for(&doc, "hearth", "seeds-0.2.4", &hashes(&["k2", "standalone"])).unwrap_err().contains("source"));
    assert!(seeds_json::parse(&text.replacen("sew-app-seeds", "something-else", 1)).is_err());
}

/// Pin day end to end: every file's sha256 and size against seeds.json
/// (the source tarball too), each Seed unpacked and its sums checked, K2's
/// tree hash recorded. A tarball with one byte changed is refused.
#[test]
fn pin_release_checks_every_file_and_records_the_tree_hash() {
    let dir = scratch("pin");
    std::fs::create_dir_all(&dir).unwrap();
    let (seed, _) = good_tarball();
    let src = b"source tarball bytes".to_vec();
    write(&dir.join("hearth-k2-0.2.4.tar.gz"), &seed);
    write(&dir.join("hearth-0.2.4-src.tar.gz"), &src);
    let doc = serde_json::json!({
        "schema": 1, "kind": "sew-app-seeds", "commit": testkit::COMMIT,
        "seeds": [{
            "name": "hearth-k2-0.2.4.tar.gz", "app": "hearth", "variant": "k2", "sewIdentity": "k2", "version": "0.2.4",
            "sha256": testkit::sha256_hex(&seed), "size": seed.len(), "commit": testkit::COMMIT,
            "requires": { "k2": ">=0.46.3", "sew": ">=0.2.0", "voice": true },
            "caps": testkit::K2_CAPS, "boxServices": [{ "name": "sew-api" }],
            "health": { "live": "/healthz", "ready": "/readyz" }, "sewSetup": [],
            "source": { "name": "hearth-0.2.4-src.tar.gz", "sha256": testkit::sha256_hex(&src), "size": src.len() }
        }]
    });
    write(&dir.join("seeds.json"), doc.to_string().as_bytes());
    let seen = std::cell::RefCell::new(Vec::new());
    let hash_of = |p: &std::path::Path| -> Result<String, String> {
        seen.borrow_mut().push(std::fs::read(p.join("manifest.json")).unwrap());
        Ok("7".repeat(64))
    };
    let got = seeds_json::pin_release(&dir, "seeds-0.2.4", &hash_of).expect("pins");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.variants["k2"].seed_hash, "7".repeat(64));
    assert_eq!(got[0].1.variants["k2"].sha256, testkit::sha256_hex(&seed));
    assert_eq!(seen.borrow().len(), 1, "the tree hash comes from the unpacked Seed");
    // One byte changed in the source tarball: refused.
    write(&dir.join("hearth-0.2.4-src.tar.gz"), b"source tarball bytez");
    assert!(seeds_json::pin_release(&dir, "seeds-0.2.4", &hash_of).unwrap_err().contains("hearth-0.2.4-src.tar.gz"));
    write(&dir.join("hearth-0.2.4-src.tar.gz"), &src);
    let mut bad = seed.clone();
    let last = bad.len() - 9;
    bad[last] ^= 1;
    write(&dir.join("hearth-k2-0.2.4.tar.gz"), &bad);
    assert!(seeds_json::pin_release(&dir, "seeds-0.2.4", &hash_of).unwrap_err().contains("hearth-k2-0.2.4.tar.gz"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── addresses ────────────────────────────────────────────────────────

#[test]
fn an_address_is_one_label() {
    assert_eq!(normalize_address("chat").unwrap().as_deref(), Some("chat"));
    assert_eq!(normalize_address(" Team-Meet ").unwrap().as_deref(), Some("team-meet"));
    assert_eq!(normalize_address("chat.lztek.k2.dev").unwrap().as_deref(), Some("chat"));
    assert_eq!(normalize_address("auto").unwrap(), None);
    assert!(normalize_address("x.chat.lztek.k2.dev").unwrap_err().contains("one level"), "R12: one level only");
    assert!(normalize_address("chat.example.com").unwrap_err().contains("own domain"), "1b: custom domains later");
    assert!(normalize_address("-bad").is_err());
    assert!(normalize_address(&"a".repeat(41)).is_err());
}

/// R7/R14: second copies get `<installId>-2`; a taken family label
/// suggests the look's own altLabel, else `<installId>-<family>`.
#[test]
fn install_ids_and_suggestions() {
    let mut e = catalog().unwrap().entry("sew/hearth").unwrap().clone();
    let taken = |ids: &'static [&'static str]| move |x: &str| ids.iter().any(|i| *i == x);
    assert_eq!(install_id_for(&e, &taken(&[])), "hearth");
    assert_eq!(install_id_for(&e, &taken(&["hearth"])), "hearth-2");
    assert_eq!(install_id_for(&e, &taken(&["hearth", "hearth-2"])), "hearth-3");
    assert_eq!(suggestion(&e, "hearth-2", &taken(&["chat"])), "hearth-2-chat");
    e.alt_label = Some("community".into());
    assert_eq!(suggestion(&e, "hearth-2", &taken(&["chat"])), "community");
    assert_eq!(suggestion(&e, "hearth-2", &taken(&["chat", "community"])), "community-2");
}

// ── the job store ────────────────────────────────────────────────────

fn job(id: &str, app: &str) -> jobs::JobRow {
    jobs::JobRow {
        id: id.into(),
        entry: "sew/hearth".into(),
        version: "0.2.4".into(),
        variant: "k2".into(),
        app_id: app.into(),
        workspace_id: "ws-1".into(),
        address_kind: jobs::ADDRESS_SUITE_LABEL.into(),
        address: "chat".into(),
        actor: "owner".into(),
        who_json: "{\"kind\":\"owner\"}".into(),
        pin_json: "{}".into(),
        options: serde_json::json!({}),
        step: jobs::STEP_DOWNLOAD.into(),
        state: jobs::STATE_RUNNING.into(),
        wait_reason: None,
        wait_question: None,
        wait_requests: vec![],
        wait_hint: None,
        error_code: None,
        error_hint: None,
        seed_hash: None,
        deployment_id: None,
        space_id: None,
        steps: vec![],
        step_started_at: 1,
        created_at: 1,
        updated_at: 1,
        finished_at: None,
    }
}

#[test]
fn the_job_store_round_trips_and_lists_active_jobs() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    let mut a = job("j1", "hearth");
    jobs::insert(&conn, &a).unwrap();
    jobs::insert(&conn, &job("j2", "sew-meet")).unwrap();
    a.step = jobs::STEP_ADDRESS.into();
    a.state = jobs::STATE_WAITING.into();
    a.wait_reason = Some(jobs::WAIT_SEW_RIGHTS.into());
    a.wait_question = Some("Q13".into());
    a.wait_requests = vec!["req-1".into()];
    a.steps.push(jobs::StepDone { step: "download".into(), at: 2, note: None });
    jobs::save(&conn, &a).unwrap();
    let back = jobs::get(&conn, "j1").unwrap().unwrap();
    assert_eq!(back, a);
    assert_eq!(jobs::active(&conn).unwrap().len(), 2);
    let mut b = jobs::get(&conn, "j2").unwrap().unwrap();
    b.state = jobs::STATE_DONE.into();
    jobs::save(&conn, &b).unwrap();
    assert_eq!(jobs::active(&conn).unwrap().iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), vec!["j1"]);
    assert_eq!(jobs::list(&conn, Some("sew-meet")).unwrap().len(), 1);
    // The table refuses a state outside the four.
    assert!(conn.execute("UPDATE suite_install_jobs SET state = 'stuck' WHERE id = 'j1'", []).is_err());
    assert_eq!(jobs::next_step(jobs::STEP_ADDRESS), jobs::STEP_DEPLOY, "A5: the address before the first deploy");
    assert_eq!(jobs::next_step(jobs::STEP_READY), jobs::STEP_DONE);
}

/// A7: one App per suite label; the holder is named.
#[test]
fn a_suite_label_has_one_app() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    let l = |app: &str| jobs::SuiteLabel { label: "chat".into(), app_id: app.into(), front_port: 40001, bound_at: 5, bound_by: "owner".into() };
    jobs::label_bind(&conn, &l("hearth")).unwrap();
    jobs::label_bind(&conn, &l("hearth")).expect("the same App again is a no-op");
    assert_eq!(jobs::label_bind(&conn, &l("hearth-2")).unwrap_err(), "hearth");
    assert_eq!(jobs::labels_of(&conn, "hearth").unwrap().len(), 1);
    assert!(jobs::label_release(&conn, "chat").unwrap());
    jobs::label_bind(&conn, &l("hearth-2")).expect("free again");
}
