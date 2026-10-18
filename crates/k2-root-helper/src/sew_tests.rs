//! `engine sew` namespace tests (protocol 7, Sew S29 K11) over the temp
//! root and fakes of `tests.rs`. `/usr/local/bin/sew` is never executed:
//! the fake exec records argv + stdin and answers with canned Sew JSON.
//! The pin is K2's fixture release (`k2-root-proto/testdata/sew`, a shell
//! stub), active because helper tests build with `test-signer`.

use std::os::unix::fs::{symlink, PermissionsExt};

use k2_root_proto::paths::SYSTEMCTL;
use k2_root_proto::sew::*;
use k2_root_proto::verbs::{exit, Class, SewVerb, Verb};

use super::{box_root, ok, T};
use crate::ctx::ExecOut;

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn pin_tarball() -> Vec<u8> {
    fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz")
}
fn old_tarball() -> Vec<u8> {
    fixture("sew-0.0.1-x86_64-unknown-linux-gnu.tar.gz")
}
fn media_tarball() -> Vec<u8> {
    fixture("sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz")
}

/// The `sew` member of a fixture tarball.
fn member(tarball: &[u8], name: &str) -> Vec<u8> {
    crate::sew::extract_member(tarball, name).expect("fixture member")
}

fn json(o: &crate::Outcome) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("stdout not JSON ({e}): {:?}", String::from_utf8_lossy(&o.stdout)))
}

/// Put a binary at /usr/local/bin/sew (root-owned in spirit; tests never
/// chown).
fn install_binary(t: &T, bytes: &[u8]) {
    let p = t.p(SEW_BIN);
    std::fs::write(&p, bytes).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn with_pin_installed(t: &T) {
    install_binary(t, &member(&pin_tarball(), "sew"));
}

fn sew_answers(t: &T, status: i32, stdout: &str) {
    t.exec.0.borrow_mut().answers.insert(
        SEW_BIN.into(),
        ExecOut { status, stdout: stdout.as_bytes().to_vec(), stderr: b"sew: canned stderr".to_vec() },
    );
}

/// Every call to /usr/local/bin/sew: (argv, stdin).
fn sew_calls(t: &T) -> Vec<(Vec<String>, Option<Vec<u8>>)> {
    t.calls().into_iter().filter(|(p, _, _)| p == SEW_BIN).map(|(_, a, s)| (a, s)).collect()
}

fn systemctl_calls(t: &T) -> Vec<String> {
    t.calls()
        .into_iter()
        .filter(|(p, a, _)| p == SYSTEMCTL && !a.iter().any(|x| x == "MainPID") && !a.iter().any(|x| x == "is-active"))
        .map(|(_, a, _)| a.join(" "))
        .collect()
}

fn write(t: &T, abs: &str, body: &str) {
    let p = t.p(abs);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const KEY: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";
const JWKS: &str = "http://127.0.0.1:38471/v1/chat/jwks";

fn install_body() -> String {
    format!(r#"{{"jwksUrl":"{JWKS}","masterKey":"{KEY}"}}"#)
}

// ── version / status ─────────────────────────────────────────────────────

#[test]
fn version_answers_protocol_8_with_the_sew_namespace_and_pin() {
    let t = box_root();
    let o = t.call(&["helper", "version"]);
    ok(&o);
    let v = json(&o);
    // 7 brought `engine sew`; 8 is 0.46.1's helper changes (protocols are
    // additive, so the Sew namespace is still here).
    assert_eq!(v["protocol"], 8);
    assert!(v["protocol"].as_u64().expect("number") >= u64::from(k2_root_proto::version::SEW_MIN_PROTOCOL));
    assert_eq!(v["namespaces"]["sew"], true);
    assert_eq!(v["namespaces"]["apps"], true, "protocols are additive");
    assert_eq!(v["sewPin"], "0.0.2");
    assert_eq!(v["sewPinSha256"], pinned_artifact("x86_64").unwrap().tarball_sha256);
}

#[test]
fn status_reports_pin_binary_staging_and_units_without_running_sew() {
    let t = box_root();
    let v = json(&t.call(&["engine", "sew", "status"]));
    assert_eq!(v["pin"]["version"], "0.0.2");
    assert_eq!(v["pin"]["placeholder"], false);
    assert_eq!(v["pin"]["media"]["version"], "0.0.2");
    assert_eq!(v["binary"]["present"], false);
    assert!(v["staged"]["core"].is_null());
    with_pin_installed(&t);
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let v = json(&t.call(&["engine", "sew", "status"]));
    assert_eq!(v["binary"]["version"], "0.0.2");
    assert_eq!(v["binary"]["known"], true);
    assert_eq!(v["staged"]["core"], "0.0.2");
    install_binary(&t, b"#!/bin/sh\necho evil\n");
    let v = json(&t.call(&["engine", "sew", "status"]));
    assert_eq!(v["binary"]["known"], false);
    assert!(v["binary"]["version"].is_null());
    assert!(sew_calls(&t).is_empty(), "status never runs sew as root");
}

// ── release build without a Sew pin ──────────────────────────────────────

/// A release build while K2's Sew pins are placeholders: every install verb
/// refuses `sew_not_released` before any effect, even with the pinned
/// binary already on the box and a valid stage in hand.
#[test]
fn unreleased_pin_refuses_every_install_verb_before_any_effect() {
    let mut t = box_root();
    t.ctx.sew_pinned = false;
    with_pin_installed(&t);
    let cases: Vec<(Vec<&str>, Vec<u8>)> = vec![
        (vec!["engine", "sew", "stage"], pin_tarball()),
        (vec!["engine", "sew", "stage-media"], media_tarball()),
        (vec!["engine", "sew", "install-bin"], vec![]),
        (vec!["engine", "sew", "install"], install_body().into_bytes()),
        (vec!["engine", "sew", "upgrade"], vec![]),
        (vec!["engine", "sew", "media", "install"], br#"{"public_url":"wss://voice.box.k2.dev"}"#.to_vec()),
        (vec!["engine", "sew", "media", "upgrade"], vec![]),
    ];
    for (argv, stdin) in cases {
        let o = t.call_in(&argv, &stdin);
        assert_eq!(o.code, exit::STATE, "{argv:?}: {}", o.stderr);
        assert!(o.stderr.starts_with("sew_not_released: "), "{argv:?}: {}", o.stderr);
        assert!(o.stderr.contains(SEW_NOT_RELEASED_MESSAGE), "{argv:?}: {}", o.stderr);
    }
    assert!(!t.p(SEW_STAGE_TARBALL).exists());
    assert!(!t.p(SEW_STAGE_MEDIA_TARBALL).exists());
    assert!(!t.p(SEW_STAGE_INSTALL_JSON).exists());
    assert!(sew_calls(&t).is_empty(), "nothing ran as root");
    assert!(systemctl_calls(&t).is_empty(), "no unit touched: {:?}", systemctl_calls(&t));
    // Reads and clean-up verbs still answer.
    ok(&t.call(&["engine", "sew", "status"]));
    ok(&t.call(&["engine", "sew", "unstage"]));
}

/// The same box with a pin (test-signer here; `sew-test-pins` likewise):
/// stage works as before.
#[test]
fn pinned_build_stages_as_before() {
    let t = box_root();
    assert!(t.ctx.sew_pinned, "helper tests build with test-signer: the fixture pin");
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    assert!(t.p(SEW_STAGE_TARBALL).exists());
}

// ── stage / install-bin ──────────────────────────────────────────────────

#[test]
fn stage_keeps_only_the_pin_tarball_0600() {
    let t = box_root();
    let o = t.call_in(&["engine", "sew", "stage"], &pin_tarball());
    ok(&o);
    assert_eq!(json(&o)["staged"], "0.0.2");
    let p = t.p(SEW_STAGE_TARBALL);
    assert_eq!(std::fs::read(&p).unwrap(), pin_tarball());
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(t.p(SEW_STAGE_DIR)).unwrap().permissions().mode() & 0o777, 0o700);
    // An older known release is not the pin: never staged for install.
    let t2 = box_root();
    let o = t2.call_in(&["engine", "sew", "stage"], &old_tarball());
    assert_eq!(o.code, exit::ARGV);
    assert!(o.stderr.contains("sha256 mismatch"), "{}", o.stderr);
    assert!(!t2.p(SEW_STAGE_TARBALL).exists());
    for garbage in [&b""[..], b"not a tarball", &media_tarball()] {
        let o = t2.call_in(&["engine", "sew", "stage"], garbage);
        assert_eq!(o.code, exit::ARGV, "{}", o.stderr);
        assert!(!t2.p(SEW_STAGE_TARBALL).exists());
    }
    // The media tarball stages only through stage-media.
    ok(&t2.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    assert!(t2.p(SEW_STAGE_MEDIA_TARBALL).exists());
    assert_eq!(t2.call_in(&["engine", "sew", "stage-media"], &pin_tarball()).code, exit::ARGV);
    ok(&t2.call(&["engine", "sew", "unstage"]));
    assert!(!t2.p(SEW_STAGE_MEDIA_TARBALL).exists());
}

fn tgz(members: &[(&str, &[u8], tar::EntryType, Option<&str>)]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (name, data, kind, link) in members {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(*kind);
        h.set_mode(0o755);
        h.set_size(data.len() as u64);
        if let Some(l) = link {
            h.set_link_name(l).unwrap();
        }
        // set_path refuses `..`; write the raw name like an attacker would.
        let raw = h.as_old_mut();
        raw.name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    let tar = b.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &tar).unwrap();
    gz.finish().unwrap()
}

#[test]
fn member_extraction_refuses_nested_dotted_linked_missing_and_empty_members() {
    use tar::EntryType as E;
    let x = crate::sew::extract_member;
    assert_eq!(x(&tgz(&[("LICENSE", b"l", E::Regular, None), ("sew", b"bin", E::Regular, None)]), "sew").unwrap(), b"bin");
    for (bad, why) in [
        (tgz(&[("bin/sew", b"x", E::Regular, None)]), "nested"),
        (tgz(&[("../sew", b"x", E::Regular, None)]), "dotdot"),
        (tgz(&[("sew", b"", E::Symlink, Some("/etc/shadow"))]), "symlink"),
        (tgz(&[("sew", b"", E::Link, Some("/usr/bin/sudo"))]), "hardlink"),
        (tgz(&[("sew", b"", E::Regular, None)]), "empty"),
        (tgz(&[("sewx", b"x", E::Regular, None)]), "missing"),
    ] {
        assert!(x(&bad, "sew").is_err(), "{why} must be refused");
    }
}

#[test]
fn install_bin_installs_the_pin_once_and_never_replaces_another_version() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    let o = t.call(&["engine", "sew", "install-bin"]);
    assert_eq!(o.code, exit::STATE, "nothing staged: {}", o.stderr);
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let o = t.call(&["engine", "sew", "install-bin"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], true);
    assert_eq!(std::fs::read(t.p(SEW_BIN)).unwrap(), member(&pin_tarball(), "sew"));
    assert_eq!(std::fs::metadata(t.p(SEW_BIN)).unwrap().permissions().mode() & 0o777, 0o755);
    assert!(!t.p(SEW_BIN_STAGING).exists());
    let o = t.call(&["engine", "sew", "install-bin"]);
    ok(&o);
    assert_eq!(json(&o)["changed"], false, "same version: no-op");

    // An older known release on disk: install-bin refuses (upgrade does it).
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let o = t.call(&["engine", "sew", "install-bin"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("use engine sew upgrade"), "{}", o.stderr);
    assert_eq!(std::fs::read(t.p(SEW_BIN)).unwrap(), member(&old_tarball(), "sew"));

    // A link at the path is a refusal, never followed.
    let t = box_root();
    std::fs::write(t.p("/usr/local/bin/real"), b"x").unwrap();
    symlink(t.p("/usr/local/bin/real"), t.p(SEW_BIN)).unwrap();
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    assert_eq!(t.call(&["engine", "sew", "install-bin"]).code, exit::STATE);
    assert_eq!(std::fs::read(t.p("/usr/local/bin/real")).unwrap(), b"x");
}

// ── licence notices (Sew SW1 B1, protocol 8) ─────────────────────────────

/// A gzip tarball with `(name, bytes, is_symlink)` members, like Sew's
/// release script (root-owned, mtime 0).
fn tarball_of(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut b = tar::Builder::new(enc);
    for (name, body, link) in members {
        let mut h = tar::Header::new_ustar();
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        if *link {
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, name, "/etc/shadow").unwrap();
        } else {
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            b.append_data(&mut h, name, *body).unwrap();
        }
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn sew_0_2_0_shape() -> Vec<u8> {
    tarball_of(&[
        ("LICENSE", b"FSL-1.1-ALv2 text", false),
        ("NOTICE", b"Sew notice", false),
        ("THIRD_PARTY_NOTICES.md", b"# third parties", false),
        ("sew", b"\x7fELF fake sew", false),
    ])
}

#[test]
fn notices_extract_only_top_level_regular_licence_files() {
    let got = crate::sew::extract_notices(&sew_0_2_0_shape()).expect("0.2.0 shape");
    let names: Vec<&str> = got.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, NOTICE_MEMBERS);
    assert_eq!(got[1].1, b"Sew notice");
    // The binary member is still found the same way beside them.
    assert_eq!(member(&sew_0_2_0_shape(), "sew"), b"\x7fELF fake sew");
    // Pre-0.2.0 tarballs (K2's 0.0.1 fixture) carry none; the 0.0.2 pin
    // fixture is 0.2.0-shaped.
    assert!(crate::sew::extract_notices(&old_tarball()).expect("fixture").is_empty());
    assert_eq!(crate::sew::extract_notices(&pin_tarball()).expect("fixture").len(), 3);
    // Nested copies are not top-level notices.
    let nested = tarball_of(&[("doc/LICENSE", b"x", false), ("sew", b"bin", false)]);
    assert!(crate::sew::extract_notices(&nested).expect("nested").is_empty());
    // A link, a duplicate: refused.
    let link = tarball_of(&[("LICENSE", b"", true), ("sew", b"bin", false)]);
    assert!(crate::sew::extract_notices(&link).unwrap_err().msg.contains("not a regular file"));
    let twice = tarball_of(&[("NOTICE", b"a", false), ("NOTICE", b"b", false)]);
    assert!(crate::sew::extract_notices(&twice).unwrap_err().msg.contains("twice"));
}

#[test]
fn notices_install_beside_the_binary_and_go_with_it() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    // No notices in the tarball: nothing written, no directory made.
    let v = crate::sew::install_notices(&t.ctx, &old_tarball(), SEW_DOC_DIR).expect("fixture");
    assert_eq!(v, serde_json::json!({"dir": null, "files": []}));
    assert!(!t.p(DOC_ROOT).exists());

    let v = crate::sew::install_notices(&t.ctx, &sew_0_2_0_shape(), SEW_DOC_DIR).expect("0.2.0 shape");
    assert_eq!(v, serde_json::json!({"dir": SEW_DOC_DIR, "files": NOTICE_MEMBERS}));
    for name in NOTICE_MEMBERS {
        let p = t.p(&format!("{SEW_DOC_DIR}/{name}"));
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644, "{name}");
    }
    assert_eq!(std::fs::read(t.p(&format!("{SEW_DOC_DIR}/LICENSE"))).unwrap(), b"FSL-1.1-ALv2 text");
    assert_eq!(std::fs::metadata(t.p(SEW_DOC_DIR)).unwrap().permissions().mode() & 0o777, 0o755);
    // Re-install is idempotent (atomic rewrite).
    crate::sew::install_notices(&t.ctx, &sew_0_2_0_shape(), SEW_DOC_DIR).expect("again");
    let v = json(&t.call(&["engine", "sew", "status"]));
    assert_eq!(v["notices"], serde_json::json!({"sew": true, "sewMedia": false}));

    // Removal takes the files and the emptied directory; a foreign file
    // keeps the directory.
    assert_eq!(crate::sew::remove_notices(&t.ctx, SEW_DOC_DIR).unwrap(), NOTICE_MEMBERS);
    assert!(!t.p(SEW_DOC_DIR).exists());
    assert!(t.p(DOC_ROOT).is_dir(), "/usr/share/doc stays");
    crate::sew::install_notices(&t.ctx, &sew_0_2_0_shape(), SEW_MEDIA_DOC_DIR).unwrap();
    std::fs::write(t.p(&format!("{SEW_MEDIA_DOC_DIR}/README.Debian")), b"x").unwrap();
    assert_eq!(crate::sew::remove_notices(&t.ctx, SEW_MEDIA_DOC_DIR).unwrap().len(), 3);
    assert!(t.p(&format!("{SEW_MEDIA_DOC_DIR}/README.Debian")).exists());
    assert!(crate::sew::remove_notices(&t.ctx, SEW_DOC_DIR).unwrap().is_empty(), "missing is fine");

    // A link where the doc directory goes is refused, never followed.
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    std::fs::create_dir_all(t.p("/tmp/elsewhere")).unwrap();
    symlink(t.p("/tmp/elsewhere"), t.p(DOC_ROOT)).unwrap();
    assert!(crate::sew::install_notices(&t.ctx, &sew_0_2_0_shape(), SEW_DOC_DIR).is_err());
    assert_eq!(std::fs::read_dir(t.p("/tmp/elsewhere")).unwrap().count(), 0);
}

fn release_with_notices(tag: &str) -> Vec<u8> {
    let lic = format!("LICENSE {tag}");
    let notice = format!("NOTICE {tag}");
    tarball_of(&[
        ("LICENSE", lic.as_bytes(), false),
        ("NOTICE", notice.as_bytes(), false),
        ("THIRD_PARTY_NOTICES.md", b"# third parties", false),
        ("sew", b"\x7fELF fake sew", false),
    ])
}

fn licence_in(t: &T, dir: &str) -> Option<String> {
    std::fs::read_to_string(t.p(&format!("{dir}/LICENSE"))).ok()
}

/// Sew keeps the old binary as `sew.prev` on an upgrade and swaps the two
/// back on a rollback: the notices go with them (Sew, 2026-10-17).
#[test]
fn notices_follow_the_binary_through_upgrade_and_rollback() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    crate::sew::install_notices(&t.ctx, &release_with_notices("v1"), SEW_DOC_DIR).unwrap();
    // Upgrade: v1's notices are kept beside the rollback binary.
    let v = crate::sew::upgrade_notices(&t.ctx, &release_with_notices("v2")).unwrap();
    assert_eq!(v["files"], serde_json::json!(NOTICE_MEMBERS));
    assert_eq!(v["previous"], serde_json::json!(NOTICE_MEMBERS));
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("LICENSE v2"));
    assert_eq!(licence_in(&t, SEW_DOC_PREV_DIR).as_deref(), Some("LICENSE v1"));
    // Rollback: swapped, like sew and sew.prev.
    crate::sew::rollback_notices(&t.ctx).unwrap();
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("LICENSE v1"));
    assert_eq!(licence_in(&t, SEW_DOC_PREV_DIR).as_deref(), Some("LICENSE v2"));
    // Through the verb: Sew's rollback exits 0 → the sets swap again.
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"state":"rolled_back"}"#);
    let o = t.call(&["engine", "sew", "rollback"]);
    ok(&o);
    assert_eq!(json(&o)["notices"]["files"], serde_json::json!(NOTICE_MEMBERS));
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("LICENSE v2"));
    assert_eq!(licence_in(&t, SEW_DOC_PREV_DIR).as_deref(), Some("LICENSE v1"));
    // A failed rollback leaves them as they are.
    sew_answers(&t, 1, r#"{"error":{"message":"no sew.prev"}}"#);
    let o = t.call(&["engine", "sew", "rollback"]);
    ok(&o);
    assert!(json(&o).get("notices").is_none());
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("LICENSE v2"));
    // Purge takes both sets.
    let r = crate::sew::remove_all_notices(&t.ctx).unwrap();
    assert_eq!(r["sew"], serde_json::json!(NOTICE_MEMBERS));
    assert!(!t.p(SEW_DOC_DIR).exists());

    // Upgrading from a release without notices (0.1.x), then rolling back:
    // the old binary had none, so after the rollback there are none.
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    let v = crate::sew::upgrade_notices(&t.ctx, &release_with_notices("v2")).unwrap();
    assert_eq!(v["previous"], serde_json::json!([]));
    crate::sew::rollback_notices(&t.ctx).unwrap();
    assert_eq!(licence_in(&t, SEW_DOC_DIR), None);
    assert_eq!(licence_in(&t, SEW_DOC_PREV_DIR).as_deref(), Some("LICENSE v2"));
}

/// The fixture pin is shaped like Sew 0.2.0 (notices beside `sew`):
/// install-bin puts them in place; a tarball from before 0.2.0 (the 0.0.1
/// fixture, one member) writes none.
#[test]
fn install_bin_writes_the_pins_notices_and_an_older_tarball_writes_none() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let o = t.call(&["engine", "sew", "install-bin"]);
    ok(&o);
    assert_eq!(json(&o)["notices"]["files"], serde_json::json!(NOTICE_MEMBERS));
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("lic\n"));
    // The same-version re-run answers notices too.
    let o = t.call(&["engine", "sew", "install-bin"]);
    assert_eq!(json(&o)["notices"]["files"], serde_json::json!(NOTICE_MEMBERS));

    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    let v = crate::sew::install_notices(&t.ctx, &old_tarball(), SEW_DOC_DIR).unwrap();
    assert_eq!(v, serde_json::json!({"dir": null, "files": []}));
    assert!(!t.p(SEW_DOC_DIR).exists());
}

// ── known-binary rule ────────────────────────────────────────────────────

#[test]
fn every_root_exec_verb_refuses_an_unknown_or_missing_binary_and_runs_nothing() {
    for binary in [None, Some(&b"#!/bin/sh\nrm -rf /\n"[..])] {
        let t = box_root();
        if let Some(b) = binary {
            install_binary(&t, b);
        }
        ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
        ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
        write(&t, SEW_CONFIG, "[server]\n");
        write(&t, SEW_MGMT_TOKEN, &format!("sewsvc_{KEY}\n"));
        std::fs::create_dir_all(t.p("/mnt/off")).unwrap();
        let cases: Vec<(Vec<&str>, Vec<u8>)> = vec![
            (vec!["engine", "sew", "install"], install_body().into_bytes()),
            (vec!["engine", "sew", "config"], br#"{"keys.mirror.kind":"none"}"#.to_vec()),
            (vec!["engine", "sew", "upgrade"], vec![]),
            (vec!["engine", "sew", "rollback"], vec![]),
            (vec!["engine", "sew", "uninstall"], vec![]),
            (vec!["engine", "sew", "purge", "none"], vec![]),
            (vec!["engine", "sew", "doctor"], vec![]),
            (vec!["engine", "sew", "rotate-master"], KEY.as_bytes().to_vec()),
            (vec!["engine", "sew", "drop-prev"], vec![]),
            (vec!["engine", "sew", "restore", "20260927T031512Z"], vec![]),
            (vec!["engine", "sew", "media", "install"], br#"{"public_url":"wss://voice.box.k2.dev"}"#.to_vec()),
            (vec!["engine", "sew", "media", "uninstall"], vec![]),
            (vec!["engine", "sew", "media", "upgrade"], vec![]),
            (vec!["engine", "sew", "media", "reconfigure"], br#"{"public_url":"wss://hearth.box.k2.dev"}"#.to_vec()),
            (vec!["engine", "sew", "media", "turn-cert"], turn_cert_body().into_bytes()),
            (vec!["engine", "sew", "media", "status"], vec![]),
            (vec!["engine", "sew", "media", "enable"], vec![]),
            (vec!["engine", "sew", "media", "disable"], vec![]),
            (vec!["engine", "sew", "media", "probe-record", "ok", "ok", "fail"], vec![]),
            (vec!["engine", "sew", "systemctl", "start"], vec![]),
            (vec!["engine", "sew", "systemctl", "migrate"], vec![]),
        ];
        for (argv, stdin) in cases {
            let o = t.call_in(&argv, &stdin);
            assert_eq!(o.code, exit::STATE, "{argv:?}: {}", o.stderr);
            let want = if binary.is_some() { "unknown sew binary" } else { "not installed" };
            assert!(o.stderr.contains(want), "{argv:?}: {}", o.stderr);
        }
        assert!(sew_calls(&t).is_empty(), "nothing ran as root");
        assert!(systemctl_calls(&t).is_empty(), "no unit touched: {:?}", systemctl_calls(&t));
    }
}

// ── install ──────────────────────────────────────────────────────────────

/// Capture the install JSON Sew is handed (the file is removed after the
/// call), at the moment `sew install` runs.
fn capture_install_json(t: &T) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (s, path) = (seen.clone(), t.p(SEW_STAGE_INSTALL_JSON));
    t.exec.0.borrow_mut().effects.push((
        SEW_BIN.into(),
        std::rc::Rc::new(move |args: &[String]| {
            if args.first().map(String::as_str) == Some("install") {
                s.borrow_mut().push(std::fs::read_to_string(&path).expect("install JSON exists while sew runs"));
            }
        }),
    ));
    seen
}

#[test]
fn install_pipes_the_key_on_stdin_only_and_hands_sew_the_daemon_uid() {
    let t = box_root();
    with_pin_installed(&t);
    let sew_toml = "# generated by sew\n[server]\nlisten = \"127.0.0.1:8470\"\n\n[mgmt]\nsocket = \"/run/sew/mgmt.sock\"\n";
    write(&t, SEW_CONFIG, sew_toml);
    std::fs::set_permissions(t.p(SEW_CONFIG), std::fs::Permissions::from_mode(0o640)).unwrap();
    sew_answers(&t, 0, r#"{"changed":["sew.toml"],"versions":{"sew":"0.0.2","pg":"16"}}"#);
    let seen = capture_install_json(&t);
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 0);
    assert_eq!(v["sew"]["versions"]["sew"], "0.0.2");
    assert_eq!(v["allowUid"]["uid"], 1001);
    assert_eq!(v["allowUid"]["restartNeeded"], false, "sew.service was not running");
    // SW5: the uid (from the caller check) rides Sew's install JSON.
    let inputs: serde_json::Value = serde_json::from_str(&seen.borrow()[0]).unwrap();
    assert_eq!(inputs["mgmt_allow_uids"], serde_json::json!([1001]));
    assert_eq!(inputs["provider"], "k2");
    assert_eq!(inputs["k2_jwks_url"], JWKS);
    assert!(!seen.borrow()[0].contains(KEY), "the key never reaches the JSON file");

    let calls = sew_calls(&t);
    assert_eq!(calls.len(), 1);
    let (argv, stdin) = &calls[0];
    let json_path = t.p(SEW_STAGE_INSTALL_JSON).display().to_string();
    assert_eq!(
        argv,
        &["install", "--config-json", json_path.as_str(), "--master-key-from", "-", "--no-start", "--json"]
    );
    assert_eq!(stdin.as_deref(), Some(format!("{KEY}\n").as_bytes()), "the key reaches Sew on stdin");
    assert!(!argv.iter().any(|a| a.contains(KEY) || a.contains("plain") || a.contains("mgmt-user")));
    assert!(!t.p(SEW_STAGE_INSTALL_JSON).exists(), "the transient config is removed");
    let audit = t.audit.0.borrow().join("\n");
    assert!(!audit.contains(KEY), "never audited");
    assert!(!String::from_utf8_lossy(&o.stdout).contains(KEY));

    // K2 never edits Sew's sew.toml: bytes and mode are Sew's.
    assert_eq!(std::fs::read_to_string(t.p(SEW_CONFIG)).unwrap(), sew_toml);
    assert_eq!(std::fs::metadata(t.p(SEW_CONFIG)).unwrap().permissions().mode() & 0o777, 0o640);
    // Again over a running sew.service: Sew reads allow_uids at start, so
    // the daemon is told to restart it.
    t.exec.0.borrow_mut().rules.push((SYSTEMCTL.into(), "is-active".into(), ExecOut { status: 0, stdout: b"active\n".to_vec(), stderr: vec![] }));
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    assert_eq!(json(&o)["allowUid"]["restartNeeded"], true);
    assert_eq!(std::fs::read_to_string(t.p(SEW_CONFIG)).unwrap(), sew_toml);
}

#[test]
fn install_input_is_validated_before_anything_runs() {
    let t = box_root();
    with_pin_installed(&t);
    write(&t, SEW_CONFIG, "");
    sew_answers(&t, 0, "{}");
    for bad in [
        format!(r#"{{"jwksUrl":"http://evil.example/v1/chat/jwks","masterKey":"{KEY}"}}"#),
        format!(r#"{{"jwksUrl":"{JWKS}","masterKey":"short"}}"#),
        format!(r#"{{"jwksUrl":"{JWKS}","masterKey":"{KEY}","provider":"standalone"}}"#),
        format!(r#"{{"jwksUrl":"{JWKS}","masterKey":"{KEY}","mgmtUid":0}}"#),
        "not json".to_string(),
        String::new(),
    ] {
        let o = t.call_in(&["engine", "sew", "install"], bad.as_bytes());
        assert_eq!(o.code, exit::ARGV, "{bad}: {}", o.stderr);
        assert!(!o.stderr.contains(KEY), "stdin never echoed");
    }
    assert!(sew_calls(&t).is_empty());
    assert!(!t.p(SEW_STAGE_INSTALL_JSON).exists());
    // An older installed release is not installed over.
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(sew_calls(&t).is_empty());
}

#[test]
fn install_failure_is_data_and_leaves_sew_toml_alone() {
    let t = box_root();
    with_pin_installed(&t);
    write(&t, SEW_CONFIG, "[server]\n");
    sew_answers(&t, 3, r#"{"ok":false,"error":{"exit":3,"message":"master key differs: use sew keys rotate-master"}}"#);
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 3);
    assert_eq!(v["sew"]["error"]["exit"], 3);
    assert!(v.get("allowUid").is_none());
    assert_eq!(std::fs::read_to_string(t.p(SEW_CONFIG)).unwrap(), "[server]\n");
    assert!(!t.p(SEW_STAGE_INSTALL_JSON).exists());
}


// ── config / keys / token ────────────────────────────────────────────────

#[test]
fn config_sets_each_validated_key_in_order_and_stops_at_a_failure() {
    let t = box_root();
    with_pin_installed(&t);
    std::fs::create_dir_all(t.p("/mnt/offbox")).unwrap();
    sew_answers(&t, 0, r#"{"changed":true,"restarted":true}"#);
    let body = br#"{"keys.mirror.kind":"dir","keys.mirror.path":"/mnt/offbox","server.public_url":"https://chat.acme.dev"}"#;
    let o = t.call_in(&["engine", "sew", "config"], body);
    ok(&o);
    let argvs: Vec<Vec<String>> = sew_calls(&t).into_iter().map(|(a, _)| a).collect();
    assert_eq!(
        argvs,
        vec![
            vec!["config", "set", "server.public_url", "https://chat.acme.dev", "--json"],
            vec!["config", "set", "keys.mirror.path", "/mnt/offbox", "--json"],
            vec!["config", "set", "keys.mirror.kind", "dir", "--json"],
        ]
    );
    // Refusals before any exec.
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, "{}");
    std::fs::write(t.p("/var/log/afile"), b"x").unwrap();
    for bad in [
        &br#"{"keys.mirror.path":"/mnt/missing"}"#[..],
        br#"{"keys.mirror.path":"/var/log/afile"}"#,
        br#"{"keys.mirror.path":"/etc/sew"}"#,
        br#"{"keys.mirror.kind":"k2"}"#,
        br#"{"server.public_url":"http://x.y"}"#,
        br#"{"mgmt.allow_uids":"0"}"#,
        br#"{}"#,
    ] {
        let o = t.call_in(&["engine", "sew", "config"], bad);
        assert_ne!(o.code, exit::OK, "{}", String::from_utf8_lossy(bad));
    }
    assert!(sew_calls(&t).is_empty());
    // A failed key stops the rest.
    sew_answers(&t, 2, r#"{"ok":false}"#);
    let o = t.call_in(&["engine", "sew", "config"], br#"{"server.public_url":"","keys.mirror.kind":"none"}"#);
    ok(&o);
    assert_eq!(json(&o)["sewExit"], 2);
    assert_eq!(sew_calls(&t).len(), 1);
}

#[test]
fn rotate_master_takes_the_key_on_stdin_only() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"kek_id":"0000000000000001","prev_kek_id":"0000000000000002","restarted":true}"#);
    for bad in [&b"short"[..], b"", &[0xffu8; 43]] {
        assert_eq!(t.call_in(&["engine", "sew", "rotate-master"], bad).code, exit::ARGV);
    }
    assert!(sew_calls(&t).is_empty());
    let o = t.call_in(&["engine", "sew", "rotate-master"], format!("{KEY}\n").as_bytes());
    ok(&o);
    let calls = sew_calls(&t);
    assert_eq!(calls[0].0, vec!["keys", "rotate-master", "--new-from", "-", "--json"]);
    assert_eq!(calls[0].1.as_deref(), Some(format!("{KEY}\n").as_bytes()));
    ok(&t.call(&["engine", "sew", "drop-prev"]));
    assert_eq!(sew_calls(&t)[1].0, vec!["keys", "drop-prev", "--json"]);
}

#[test]
fn token_hands_over_a_well_formed_token_only() {
    let t = box_root();
    assert_eq!(t.call(&["engine", "sew", "token"]).code, exit::STATE);
    write(&t, SEW_MGMT_TOKEN, &format!("sewsvc_{KEY}\n"));
    let o = t.call(&["engine", "sew", "token"]);
    ok(&o);
    assert_eq!(json(&o)["token"], format!("sewsvc_{KEY}"));
    assert!(!t.audit.0.borrow().join("\n").contains(KEY), "the token is never audited");
    write(&t, SEW_MGMT_TOKEN, "root:x:0:0\n");
    assert_eq!(t.call(&["engine", "sew", "token"]).code, exit::STATE);
    std::fs::remove_file(t.p(SEW_MGMT_TOKEN)).unwrap();
    write(&t, "/etc/shadow", "root:$6$secret\n");
    symlink(t.p("/etc/shadow"), t.p(SEW_MGMT_TOKEN)).unwrap();
    let o = t.call(&["engine", "sew", "token"]);
    assert_ne!(o.code, exit::OK);
    assert!(!String::from_utf8_lossy(&o.stdout).contains("secret"));
    // Only the daemon gets it.
    write(&t, SEW_MGMT_TOKEN, &format!("sewsvc_{KEY}\n"));
    let _ = std::fs::remove_file(t.p(SEW_MGMT_TOKEN));
    write(&t, SEW_MGMT_TOKEN, &format!("sewsvc_{KEY}\n"));
    t.as_agent();
    let o = t.call(&["engine", "sew", "token"]);
    assert_eq!(o.code, exit::NOT_DAEMON);
    assert!(o.stdout.is_empty());
}

// ── units / upgrade / rollback / purge / restore ─────────────────────────

#[test]
fn systemctl_ops_are_exact_unit_sets() {
    let t = box_root();
    with_pin_installed(&t);
    for op in ["enable", "start-db", "migrate", "start", "restart", "stop", "disable-now"] {
        ok(&t.call(&["engine", "sew", "systemctl", op]));
    }
    assert_eq!(
        systemctl_calls(&t),
        vec![
            "enable sew-db.service sew.service sew-backup.timer sew-drill.timer",
            "start sew-db.service",
            "start sew-migrate.service",
            "start sew.service sew-backup.timer sew-drill.timer",
            "restart sew.service",
            "stop sew.service",
            "disable --now sew.service sew-db.service sew-backup.timer sew-drill.timer",
        ]
    );
}

#[test]
fn upgrade_runs_the_installed_binary_on_the_staged_pin_and_returns_sews_exit() {
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    assert_eq!(t.call(&["engine", "sew", "upgrade"]).code, exit::STATE, "nothing staged");
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    sew_answers(&t, 1, r#"{"ok":false,"error":{"exit":1,"message":"health failed; rolled back"}}"#);
    let o = t.call(&["engine", "sew", "upgrade"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 1, "Sew's rollback is data, not a helper failure");
    assert_eq!(v["stderrTail"], "sew: canned stderr");
    assert_eq!(v["reason"], "health failed; rolled back");
    // Sew's contract (2026-10-17): the bare binary, and the sha256 of THAT
    // file (the pin's binary_sha256), never the tarball and its sha.
    let staged = t.p(SEW_STAGE_UPGRADE_BIN).display().to_string();
    let pin = pinned_artifact("x86_64").unwrap().binary_sha256;
    assert_eq!(sew_calls(&t)[0].0, vec!["upgrade", "--binary", staged.as_str(), "--sha256", pin, "--json"]);
    assert!(!t.p(SEW_STAGE_UPGRADE_BIN).exists(), "the staged binary never outlives the call");
    assert!(t.p(SEW_STAGE_TARBALL).exists(), "the staged tarball stays for the daemon's unstage");

    // Already the pin: no exec.
    let t = box_root();
    with_pin_installed(&t);
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let o = t.call(&["engine", "sew", "upgrade"]);
    ok(&o);
    assert_eq!(json(&o)["sew"]["state"], "unchanged");
    assert!(sew_calls(&t).is_empty());
}

#[test]
fn rollback_purge_and_uninstall_argv_never_widen() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"state":"rolled_back"}"#);
    ok(&t.call(&["engine", "sew", "rollback"]));
    ok(&t.call(&["engine", "sew", "uninstall"]));
    ok(&t.call(&["engine", "sew", "purge", "48831765147549701"]));
    ok(&t.call(&["engine", "sew", "purge", "none", "keep-backups"]));
    let argvs: Vec<Vec<String>> = sew_calls(&t).into_iter().map(|(a, _)| a).collect();
    assert_eq!(
        argvs,
        vec![
            vec!["upgrade", "--rollback", "--json"],
            vec!["uninstall", "--json"],
            vec!["uninstall", "--purge", "--yes-delete-all-data=48831765147549701", "--json"],
            vec!["uninstall", "--purge", "--yes-delete-all-data=none", "--keep-backups", "--json"],
        ]
    );
    assert!(!argvs.concat().iter().any(|a| a.contains("accept-data-loss")));
}

#[test]
fn restore_stops_sew_restores_and_always_starts_it_again() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 3, r#"{"ok":false,"error":{"exit":3,"message":"no such bundle"}}"#);
    let o = t.call(&["engine", "sew", "restore", "20260927T031512Z"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 3);
    assert_eq!(v["restarted"], true);
    assert_eq!(systemctl_calls(&t), vec!["stop sew.service", "start sew.service"]);
    let argv = &sew_calls(&t)[0].0;
    assert_eq!(argv, &vec!["restore", "20260927T031512Z", "--replace", "--json"]);
    assert!(!argv.iter().any(|a| a == "--no-safety-backup"), "Sew's safety bundle is always taken");
}

#[test]
fn backups_are_not_root_verbs_since_sw15() {
    // The daemon backs up and lists over Sew's mgmt socket as itself.
    let t = box_root();
    with_pin_installed(&t);
    for argv in [&["engine", "sew", "backup"][..], &["engine", "sew", "backups"]] {
        assert_eq!(t.call(argv).code, exit::ARGV, "{argv:?}");
    }
    assert!(sew_calls(&t).is_empty());
}

// ── upgrade: Sew 0.1.2's contract, refusal reasons ───────────────────────

/// `--flag <value>` in a fake program's argv.
fn arg_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn out(status: i32, stdout: &str, stderr: &str) -> ExecOut {
    ExecOut { status, stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() }
}

fn sew_err(exit: i32, message: &str) -> ExecOut {
    out(exit, &serde_json::json!({"ok": false, "error": {"exit": exit, "message": message}}).to_string(), "")
}

/// A fake `/usr/local/bin/sew` that behaves like Sew 0.1.2's `sew upgrade`
/// (sew-ops upgrade.rs before Sew e96c76d0): `--tarball` holds exactly one
/// regular member `sew` or exit 3 with the reason in its `--json` error;
/// `--binary` must hash to `--sha256` (exit 3) and be executable (Sew runs
/// its `version --json`; exit 1), then it is swapped in. Records whether
/// the staged binary was there, root-executable, when Sew ran.
fn sew_0_1_2(t: &T) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (log, bin) = (seen.clone(), t.p(SEW_BIN));
    let program: super::Program = std::rc::Rc::new(move |args: &[String]| {
        if args.first().map(String::as_str) != Some("upgrade") {
            return out(0, "{}", "");
        }
        let want = arg_of(args, "--sha256").unwrap_or("").to_string();
        if let Some(tb) = arg_of(args, "--tarball") {
            let bytes = std::fs::read(tb).unwrap();
            let gz = flate2::read::GzDecoder::new(&bytes[..]);
            let mut a = tar::Archive::new(gz);
            for e in a.entries().unwrap() {
                let e = e.unwrap();
                let name = e.path().unwrap().to_string_lossy().into_owned();
                if name != "sew" {
                    log.borrow_mut().push(format!("refused tarball member {name}"));
                    return sew_err(3, &format!("the tarball must hold exactly one regular file named sew; found {name:?}"));
                }
            }
            return sew_err(3, "test fake: a one-member tarball is not exercised here");
        }
        let Some(p) = arg_of(args, "--binary") else {
            return out(2, "", "error: give --tarball <file> or --binary <file>, with --sha256");
        };
        let Ok(bytes) = std::fs::read(p) else {
            return sew_err(1, &format!("read {p}: No such file or directory"));
        };
        let got = crate::sew::sha256_hex(&bytes);
        if got != want {
            return sew_err(3, &format!("sha256 of {p} is {got}, not {want}; refusing"));
        }
        let mode = std::fs::metadata(p).unwrap().permissions().mode();
        log.borrow_mut().push(format!("binary mode {:o}", mode & 0o777));
        if mode & 0o100 == 0 {
            return sew_err(1, &format!("{p} version --json: Permission denied"));
        }
        std::fs::write(&bin, &bytes).unwrap();
        out(0, r#"{"state":"upgraded","from":"0.0.1","to":"0.0.2","pre_bundle":"20261017T000000Z-pre-upgrade"}"#, "")
    });
    t.exec.0.borrow_mut().programs.insert(SEW_BIN.into(), program);
    seen
}

/// The box test of 2026-10-17: Sew 0.1.2 installed, a 0.2.0-shaped pin
/// (LICENSE, NOTICE, THIRD_PARTY_NOTICES.md, sew). Handing 0.1.2 the
/// tarball was refused (exit 3) and the reason was lost; the helper now
/// hands it the extracted binary, which it takes.
#[test]
fn upgrade_hands_an_older_sew_the_binary_it_accepts_not_the_multi_member_tarball() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    install_binary(&t, &member(&old_tarball(), "sew"));
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    let seen = sew_0_1_2(&t);

    // What the 0.46.1 helper did: the tarball → refused, with Sew's words.
    let staged = t.p(SEW_STAGE_TARBALL).display().to_string();
    let pin = pinned_artifact("x86_64").unwrap();
    let old_way = super::Program::clone(&t.exec.0.borrow().programs[SEW_BIN]);
    let refused = old_way(&["upgrade".into(), "--tarball".into(), staged, "--sha256".into(), pin.tarball_sha256.into(), "--json".into()]);
    assert_eq!(refused.status, 3, "Sew 0.1.2 refuses the 0.2.0-shaped tarball");
    assert!(seen.borrow()[0].starts_with("refused tarball member LICENSE"), "{:?}", seen.borrow());

    // The fixed verb.
    let o = t.call(&["engine", "sew", "upgrade"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 0, "{v}");
    assert!(v.get("reason").is_none(), "{v}");
    assert_eq!((v["from"].as_str(), v["to"].as_str()), (Some("0.0.1"), Some("0.0.2")));
    assert_eq!(seen.borrow().last().map(String::as_str), Some("binary mode 700"), "root 0700: Sew can run its version");
    assert_eq!(std::fs::read(t.p(SEW_BIN)).unwrap(), member(&pin_tarball(), "sew"), "the pin is in place");
    assert!(!t.p(SEW_STAGE_UPGRADE_BIN).exists());
    // The notices followed the binary; the old (none) are the previous set.
    assert_eq!(v["notices"]["files"], serde_json::json!(NOTICE_MEMBERS), "{v}");
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("lic\n"));
    // ... and a rollback swaps them back, as Sew swaps sew and sew.prev.
    t.exec.0.borrow_mut().programs.clear();
    sew_answers(&t, 0, r#"{"state":"rolled_back","from":"0.0.2","to":"0.0.1"}"#);
    let o = t.call(&["engine", "sew", "rollback"]);
    ok(&o);
    assert_eq!(licence_in(&t, SEW_DOC_DIR), None, "0.0.1 had no notices");
    assert_eq!(licence_in(&t, SEW_DOC_PREV_DIR).as_deref(), Some("lic\n"));
}

/// A refusal reaches the daemon with Sew's words, whichever stream Sew
/// used, bounded and without control characters; the helper journal says
/// what Sew exited with (helper code 0 is not "Sew said yes").
#[test]
fn a_sew_refusal_answers_its_reason_from_json_stderr_or_stdout() {
    let refusal = "the tarball must hold exactly one regular file named sew; found \"LICENSE\"";
    let cases: Vec<(ExecOut, String)> = vec![
        // 0.1.2 / 0.2.0 with --json: the error body on stdout.
        (sew_err(3, refusal), refusal.to_string()),
        // The reason on stderr only (Sew's own words, 2026-10-17).
        (out(3, "", &format!("sew upgrade: {refusal}\n")), format!("sew upgrade: {refusal}")),
        // Not JSON on stdout, nothing on stderr (a panic message).
        (out(101, "thread 'main' panicked at upgrade.rs:42", ""), "thread 'main' panicked at upgrade.rs:42".to_string()),
        // Control characters never pass.
        (out(3, "", "\u{1b}[31mrefused\u{1b}[0m\r\u{0}"), "[31mrefused [0m".to_string()),
        // Nothing at all: still a reason.
        (out(3, "", ""), "sew exited 3 and printed nothing".to_string()),
    ];
    for (answer, want) in cases {
        let t = box_root();
        install_binary(&t, &member(&old_tarball(), "sew"));
        ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
        let status = answer.status;
        t.exec.0.borrow_mut().answers.insert(SEW_BIN.into(), answer);
        let o = t.call(&["engine", "sew", "upgrade"]);
        ok(&o);
        let v = json(&o);
        assert_eq!(v["sewExit"], status);
        assert_eq!(v["reason"].as_str(), Some(want.as_str()), "{v}");
        assert!(v.get("notices").is_none(), "a refused upgrade moves no notices");
        let line = t.audit.0.borrow().last().cloned().expect("audited");
        let j: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!((j["code"].as_i64(), j["sewExit"].as_i64()), (Some(0), Some(i64::from(status))), "{line}");
        assert_eq!(j["sewReason"].as_str(), Some(want.as_str()), "{line}");
    }
    // Bounded.
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    t.exec.0.borrow_mut().answers.insert(SEW_BIN.into(), out(3, "", &"x".repeat(10_000)));
    let v = json(&t.call(&["engine", "sew", "upgrade"]));
    assert_eq!(v["reason"].as_str().map(str::len), Some(600));
    // Exit 0 carries no reason.
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    ok(&t.call_in(&["engine", "sew", "stage"], &pin_tarball()));
    sew_answers(&t, 0, r#"{"state":"upgraded"}"#);
    let v = json(&t.call(&["engine", "sew", "upgrade"]));
    assert!(v.get("reason").is_none(), "{v}");
}

/// The voice engine upgrades through the NEW core only (its baked sidecar
/// pins check the media tarball): refused before anything runs while the
/// core is older; then `sew media upgrade --tarball <staged pin> --now`,
/// and the media notices follow a success only.
#[test]
fn media_upgrade_runs_only_on_the_pin_core_with_the_staged_media_pin() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    install_binary(&t, &member(&old_tarball(), "sew"));
    ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    let o = t.call(&["engine", "sew", "media", "upgrade"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("the installed Sew is 0.0.1, the pin is 0.0.2"), "{}", o.stderr);
    assert!(sew_calls(&t).is_empty());

    with_pin_installed(&t);
    ok(&t.call(&["engine", "sew", "unstage"]));
    let o = t.call(&["engine", "sew", "media", "upgrade"]);
    assert_eq!(o.code, exit::STATE, "nothing staged");
    assert!(sew_calls(&t).is_empty());

    ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    sew_answers(&t, 3, r#"{"ok":false,"error":{"exit":3,"message":"voice is not installed"}}"#);
    let v = json(&t.call(&["engine", "sew", "media", "upgrade"]));
    assert_eq!((v["sewExit"].as_i64(), v["reason"].as_str()), (Some(3), Some("voice is not installed")));
    assert!(v.get("notices").is_none());
    assert!(!t.p(SEW_MEDIA_DOC_DIR).exists());

    sew_answers(&t, 0, r#"{"changed":["/usr/local/lib/sew-media/sew-media"],"from":"0.0.1","to":"0.0.2","mode":"now"}"#);
    let v = json(&t.call(&["engine", "sew", "media", "upgrade"]));
    assert_eq!(v["sewExit"], 0);
    assert_eq!(v["notices"]["files"], serde_json::json!(NOTICE_MEMBERS), "{v}");
    let staged = t.p(SEW_STAGE_MEDIA_TARBALL).display().to_string();
    for (argv, _) in sew_calls(&t) {
        assert_eq!(argv, vec!["media", "upgrade", "--tarball", staged.as_str(), "--now", "--json"]);
    }
}

// ── rollback mirrors the upgrade: the OLD core reinstalls its engine ─────

fn old_media_tarball() -> Vec<u8> {
    fixture("sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz")
}

/// A fake `/usr/local/bin/sew` for the rollback path. Records, per call,
/// the argv and WHICH core ran it (the version whose binary is at
/// /usr/local/bin/sew at that moment). `upgrade --rollback` swaps the old
/// core back, as Sew swaps sew and sew.prev.
fn sew_cores(t: &T) -> std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (log, bin) = (seen.clone(), t.p(SEW_BIN));
    let old = member(&old_tarball(), "sew");
    let program: super::Program = std::rc::Rc::new(move |args: &[String]| {
        let core = known_binary(&crate::sew::sha256_hex(&std::fs::read(&bin).unwrap()), "x86_64").unwrap_or("?").to_string();
        log.borrow_mut().push((core.clone(), args.join(" ")));
        match args.first().map(String::as_str) {
            Some("upgrade") if args.iter().any(|a| a == "--rollback") => {
                std::fs::write(&bin, &old).unwrap();
                out(0, r#"{"state":"rolled_back","from":"0.0.2","to":"0.0.1"}"#, "")
            }
            Some("media") => out(0, &format!(r#"{{"changed":["/usr/local/lib/sew-media/sew-media"],"core":"{core}"}}"#), ""),
            _ => out(0, "{}", ""),
        }
    });
    t.exec.0.borrow_mut().programs.insert(SEW_BIN.into(), program);
    seen
}

/// Sew 2026-10-17: upgrade = core → check → engine with the NEW core;
/// rollback = core → check → engine with the OLD core, from the previous
/// pin's engine tarball the helper kept (never sew-media.prev). The media
/// notices become the reinstalled engine's set.
#[test]
fn rollback_has_the_old_core_reinstall_the_kept_previous_engine_in_order() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    with_pin_installed(&t);
    let seen = sew_cores(&t);
    // The upgrade's engine step (new core): the pin's engine is kept.
    ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    let v = json(&t.call(&["engine", "sew", "media", "upgrade"]));
    assert_eq!(v["sewExit"], 0, "{v}");
    assert_eq!(v["kept"]["media"], "0.0.2", "{v}");
    assert_eq!(std::fs::read(t.p(&kept_media_path("0.0.2").unwrap())).unwrap(), media_tarball());
    assert_eq!(licence_in(&t, SEW_MEDIA_DOC_DIR).as_deref(), Some("test\n"));

    // Before the core is rolled back, the engine rollback refuses, runs nothing.
    let o = t.call(&["engine", "sew", "media", "rollback"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("the core is rolled back first"), "{}", o.stderr);
    assert_eq!(seen.borrow().len(), 1);

    // The daemon stages the previous pin's engine: kept, not staged.
    let o = t.call_in(&["engine", "sew", "stage-media"], &old_media_tarball());
    ok(&o);
    assert_eq!(json(&o)["kept"], "0.0.1");
    let st = json(&t.call(&["engine", "sew", "status"]));
    assert_eq!(st["staged"]["keptMedia"], serde_json::json!(["0.0.1", "0.0.2"]));
    assert_eq!(st["staged"]["media"], "0.0.2", "the staged slot still holds only the pin");
    let o = t.call_in(&["engine", "sew", "stage-media"], b"not a release");
    assert_eq!(o.code, exit::ARGV, "an unknown tarball is neither staged nor kept");

    // Rollback: the core first, then the OLD core reinstalls its engine.
    ok(&t.call(&["engine", "sew", "rollback"]));
    let v = json(&t.call(&["engine", "sew", "media", "rollback"]));
    assert_eq!((v["sewExit"].as_i64(), v["to"].as_str()), (Some(0), Some("0.0.1")), "{v}");
    let kept = t.p(&kept_media_path("0.0.1").unwrap()).display().to_string();
    let staged = t.p(SEW_STAGE_MEDIA_TARBALL).display().to_string();
    assert_eq!(
        *seen.borrow(),
        vec![
            ("0.0.2".to_string(), format!("media upgrade --tarball {staged} --now --json")),
            ("0.0.2".to_string(), "upgrade --rollback --json".to_string()),
            ("0.0.1".to_string(), format!("media upgrade --tarball {kept} --now --json")),
        ],
        "new core upgrades the engine; the core rolls back; the OLD core reinstalls its engine from the kept tarball"
    );
    assert!(!seen.borrow().iter().any(|(_, a)| a.contains("sew-media.prev") || a.contains("--force")));
    // The engine's notices are now 0.0.1's set.
    assert_eq!(licence_in(&t, SEW_MEDIA_DOC_DIR).as_deref(), Some("old1\n"));
}

/// The previous pin's engine tarball is not kept: refused with the fix in
/// the words, nothing run.
#[test]
fn media_rollback_without_the_kept_previous_engine_refuses_and_runs_nothing() {
    let t = box_root();
    install_binary(&t, &member(&old_tarball(), "sew"));
    let o = t.call(&["engine", "sew", "media", "rollback"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("the sew-media 0.0.1 tarball (Sew 0.0.1's engine) is not kept on this box"), "{}", o.stderr);
    assert!(o.stderr.contains("--media-tarball sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz"), "{}", o.stderr);
    assert!(sew_calls(&t).is_empty());
    // A kept file that is not the release's tarball is refused too.
    std::fs::create_dir_all(t.p(SEW_KEEP_DIR)).unwrap();
    std::fs::write(t.p(&kept_media_path("0.0.1").unwrap()), b"tampered").unwrap();
    let o = t.call(&["engine", "sew", "media", "rollback"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.contains("is not Sew 0.0.1's pin"), "{}", o.stderr);
    assert!(sew_calls(&t).is_empty());
}

/// Rolling back to a release that shipped no notices leaves NONE, never
/// the newer release's files (Sew 2026-10-17: after a rollback to 0.1.2,
/// /usr/share/doc/sew and sew-media hold what 0.1.2 had). The core's sets
/// swap (an empty previous set removes the current files); the engine's
/// are replaced by the reinstalled tarball's set.
#[test]
fn a_rollback_to_a_release_without_notices_leaves_none() {
    let t = box_root();
    std::fs::create_dir_all(t.p("/usr/share")).unwrap();
    // Core: upgrade from a release without notices, then roll back.
    crate::sew::upgrade_notices(&t.ctx, &pin_tarball()).unwrap();
    assert_eq!(licence_in(&t, SEW_DOC_DIR).as_deref(), Some("lic\n"));
    crate::sew::rollback_notices(&t.ctx).unwrap();
    for name in NOTICE_MEMBERS {
        assert!(!t.p(&format!("{SEW_DOC_DIR}/{name}")).exists(), "{name} left behind");
    }
    // Engine: the reinstalled tarball has no notices → the current set goes.
    crate::sew::install_notices(&t.ctx, &media_tarball(), SEW_MEDIA_DOC_DIR).unwrap();
    let no_notices = tarball_of(&[("sew-media", b"bin", false)]);
    let v = crate::sew::replace_notices(&t.ctx, &no_notices, SEW_MEDIA_DOC_DIR).unwrap();
    assert_eq!(v["removed"], serde_json::json!(NOTICE_MEMBERS));
    assert!(!t.p(SEW_MEDIA_DOC_DIR).exists(), "the emptied directory goes too");
}

/// The kept set is bounded, and a purge takes it.
#[test]
fn kept_engine_tarballs_are_bounded_and_purged() {
    let t = box_root();
    with_pin_installed(&t);
    ok(&t.call_in(&["engine", "sew", "stage-media"], &old_media_tarball()));
    assert!(t.p(&kept_media_path("0.0.1").unwrap()).exists());
    let mode = std::fs::metadata(t.p(&kept_media_path("0.0.1").unwrap())).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    sew_answers(&t, 0, r#"{"purged":true}"#);
    let v = json(&t.call(&["engine", "sew", "purge", "none"]));
    assert_eq!(v["keptRemoved"]["media"], serde_json::json!(["0.0.1"]), "{v}");
    assert!(!t.p(&kept_media_path("0.0.1").unwrap()).exists());
}

// ── media ────────────────────────────────────────────────────────────────

#[test]
fn media_install_uses_the_staged_pin_and_a_transient_json() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"changed":[],"sysctl":"applied"}"#);
    let body = br#"{"public_url":"wss://voice.box.k2.dev","sysctl":"apply","udp_port":7882,"tcp_port":7881}"#;
    assert_eq!(t.call_in(&["engine", "sew", "media", "install"], body).code, exit::STATE, "nothing staged");
    ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    for bad in [
        &br#"{"public_url":"wss://voice.box.k2.dev","turn_cert":"/etc/shadow"}"#[..],
        br#"{"public_url":"wss://voice.box.k2.dev","tarball":"/tmp/evil.tar.gz"}"#,
        br#"{"public_url":"https://voice.box.k2.dev"}"#,
    ] {
        assert_eq!(t.call_in(&["engine", "sew", "media", "install"], bad).code, exit::ARGV);
    }
    assert!(sew_calls(&t).is_empty());
    let o = t.call_in(&["engine", "sew", "media", "install"], body);
    ok(&o);
    let path = t.p(SEW_STAGE_MEDIA_JSON).display().to_string();
    assert_eq!(sew_calls(&t)[0].0, vec!["media", "install", "--config-json", path.as_str(), "--json"]);
    assert!(!t.p(SEW_STAGE_MEDIA_JSON).exists(), "the transient config is removed");
    assert!(systemctl_calls(&t).is_empty(), "no firewall, no unit edits by K2");
}

// ── the class table holds end to end ─────────────────────────────────────

#[test]
fn sew_daemon_verbs_refuse_an_agent_caller_before_reading_stdin() {
    let t = box_root();
    with_pin_installed(&t);
    t.as_agent();
    for v in SewVerb::catalog().into_iter().map(Verb::Sew).filter(|v| v.class() == Class::Daemon) {
        let argv = v.argv();
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let o = t.call_in(&refs, install_body().as_bytes());
        assert_eq!(o.code, exit::NOT_DAEMON, "{argv:?}");
    }
    assert!(sew_calls(&t).is_empty());
}

// ── media: TURN TLS, reconfigure, status, probe (K13) ───────────────────

const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIU\n-----END CERTIFICATE-----\n";
const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49\n-----END PRIVATE KEY-----\n";

fn turn_cert_body() -> String {
    serde_json::json!({"cert_pem": TEST_CERT, "key_pem": TEST_KEY}).to_string()
}

/// What the staged TURN files hold while `sew` runs (they are removed after).
fn capture_turn_files(t: &T) -> std::rc::Rc<std::cell::RefCell<Vec<(String, String, String, u32)>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (s, c, k, j) = (seen.clone(), t.p(SEW_STAGE_TURN_CERT), t.p(SEW_STAGE_TURN_KEY), t.p(SEW_STAGE_MEDIA_JSON));
    t.exec.0.borrow_mut().effects.push((
        SEW_BIN.into(),
        std::rc::Rc::new(move |_args: &[String]| {
            let cert = std::fs::read_to_string(&c).unwrap_or_default();
            let key = std::fs::read_to_string(&k).unwrap_or_default();
            let json = std::fs::read_to_string(&j).unwrap_or_default();
            let mode = std::fs::metadata(&k).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
            s.borrow_mut().push((cert, key, json, mode));
        }),
    ));
    seen
}

#[test]
fn media_install_hands_sew_the_turn_tls_as_staged_files_never_paths_from_input() {
    let t = box_root();
    with_pin_installed(&t);
    ok(&t.call_in(&["engine", "sew", "stage-media"], &media_tarball()));
    sew_answers(&t, 0, r#"{"changed":["turn-tls"]}"#);
    let seen = capture_turn_files(&t);
    let body = serde_json::json!({
        "public_url": "wss://hearth.box.k2.dev", "node_ip": "203.0.113.7", "udp_port": 7882, "tcp_port": 7881,
        "turn_tls": "direct", "turn_domain": "turn-box.k2.dev", "turn_advertise_port": 5349, "sysctl": "apply",
        "turn_cert_pem": TEST_CERT, "turn_key_pem": TEST_KEY,
    });
    ok(&t.call_in(&["engine", "sew", "media", "install"], body.to_string().as_bytes()));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    let (cert, key, json_text, mode) = &seen[0];
    assert_eq!(cert, TEST_CERT);
    assert_eq!(key, TEST_KEY);
    assert_eq!(*mode, 0o600, "the key file is root-only");
    let j: serde_json::Value = serde_json::from_str(json_text).unwrap();
    assert_eq!(j["turn_cert"], t.p(SEW_STAGE_TURN_CERT).display().to_string());
    assert_eq!(j["turn_key"], t.p(SEW_STAGE_TURN_KEY).display().to_string());
    assert_eq!(j["public_url"], "wss://hearth.box.k2.dev");
    assert_eq!(j["turn_domain"], "turn-box.k2.dev");
    assert!(j.get("turn_key_pem").is_none() && j.get("turn_cert_pem").is_none(), "no key text in Sew's JSON");
    assert!(!json_text.contains("PRIVATE KEY"));
    assert!(!t.p(SEW_STAGE_TURN_KEY).exists() && !t.p(SEW_STAGE_TURN_CERT).exists(), "removed after the call");
    assert!(!t.p(SEW_STAGE_MEDIA_JSON).exists());
}

#[test]
fn media_reconfigure_sets_the_voice_url_without_a_tarball() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"changed":["/etc/sew/sew.toml [voice]"],"restart_required":[]}"#);
    let seen = capture_turn_files(&t);
    for bad in [
        &b"{}"[..],
        br#"{"public_url":"https://hearth.box.k2.dev"}"#,
        br#"{"public_url":"wss://hearth.box.k2.dev","tarball":"/tmp/x"}"#,
        br#"{"turn_cert":"/etc/shadow","turn_key":"/etc/shadow"}"#,
    ] {
        assert_eq!(t.call_in(&["engine", "sew", "media", "reconfigure"], bad).code, exit::ARGV, "{}", String::from_utf8_lossy(bad));
    }
    assert!(sew_calls(&t).is_empty());
    ok(&t.call_in(&["engine", "sew", "media", "reconfigure"], br#"{"public_url":"wss://hearth.box.k2.dev"}"#));
    let path = t.p(SEW_STAGE_MEDIA_JSON).display().to_string();
    assert_eq!(sew_calls(&t)[0].0, vec!["media", "install", "--reconfigure", "--config-json", path.as_str(), "--json"]);
    let j: serde_json::Value = serde_json::from_str(&seen.borrow()[0].2).unwrap();
    assert_eq!(j, serde_json::json!({"public_url": "wss://hearth.box.k2.dev", "profile": "shared"}));
    // A new node IP: the engine drains and restarts in the same call.
    ok(&t.call_in(&["engine", "sew", "media", "reconfigure", "restart-now"], br#"{"node_ip":"203.0.113.9"}"#));
    assert_eq!(
        sew_calls(&t)[1].0,
        vec!["media", "install", "--reconfigure", "--restart", "--now", "--config-json", path.as_str(), "--json"]
    );
}

#[test]
fn media_turn_cert_renews_through_staged_files_when_idle() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"changed":["turn-tls/fullchain.pem"]}"#);
    let seen = capture_turn_files(&t);
    for bad in [
        serde_json::json!({"cert_pem": "/etc/shadow", "key_pem": TEST_KEY}).to_string(),
        serde_json::json!({"cert_pem": TEST_CERT, "key_pem": TEST_CERT}).to_string(),
        serde_json::json!({"cert_pem": TEST_CERT, "key_pem": TEST_KEY, "now": true}).to_string(),
    ] {
        assert_eq!(t.call_in(&["engine", "sew", "media", "turn-cert"], bad.as_bytes()).code, exit::ARGV, "{bad}");
    }
    ok(&t.call_in(&["engine", "sew", "media", "turn-cert"], turn_cert_body().as_bytes()));
    let c = t.p(SEW_STAGE_TURN_CERT).display().to_string();
    let k = t.p(SEW_STAGE_TURN_KEY).display().to_string();
    assert_eq!(
        sew_calls(&t)[0].0,
        vec!["media", "turn-cert", "--cert", c.as_str(), "--key", k.as_str(), "--when-idle", "--json"]
    );
    assert_eq!(seen.borrow()[0].1, TEST_KEY);
    assert!(!t.p(SEW_STAGE_TURN_KEY).exists(), "the key copy is gone after the call");
}

#[test]
fn media_status_enable_disable_and_probe_record_argv_are_fixed() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"{"installed":true,"enabled":true,"state":"running","alarms":[]}"#);
    let v = json(&t.call(&["engine", "sew", "media", "status"]));
    assert_eq!(v["sew"]["state"], "running");
    ok(&t.call(&["engine", "sew", "media", "enable"]));
    ok(&t.call(&["engine", "sew", "media", "disable"]));
    ok(&t.call(&["engine", "sew", "media", "probe-record", "ok", "fail", "ok"]));
    let argv: Vec<Vec<String>> = sew_calls(&t).into_iter().map(|(a, _)| a).collect();
    let want: Vec<Vec<&str>> = vec![
        vec!["media", "status", "--json"],
        vec!["media", "enable", "--json"],
        vec!["media", "disable", "--json"],
        vec!["media", "probe-record", "--udp", "ok", "--tcp", "fail", "--turn-tls", "ok", "--from", "k2", "--json"],
    ];
    assert_eq!(argv, want);
    assert_eq!(t.call(&["engine", "sew", "media", "probe-record", "yes", "ok", "ok"]).code, exit::ARGV);
}
