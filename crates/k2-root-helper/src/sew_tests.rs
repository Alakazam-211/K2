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
fn version_answers_protocol_7_with_the_sew_namespace_and_pin() {
    let t = box_root();
    let o = t.call(&["helper", "version"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["protocol"], 7);
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
            (vec!["engine", "sew", "backup"], vec![]),
            (vec!["engine", "sew", "backups"], vec![]),
            (vec!["engine", "sew", "restore", "20260927T031512Z"], vec![]),
            (vec!["engine", "sew", "media", "install"], br#"{"public_url":"wss://voice.box.k2.dev"}"#.to_vec()),
            (vec!["engine", "sew", "media", "uninstall"], vec![]),
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

#[test]
fn install_pipes_the_key_on_stdin_only_and_allow_lists_the_daemon_uid() {
    let t = box_root();
    with_pin_installed(&t);
    write(&t, SEW_CONFIG, "# generated by sew\n[server]\nlisten = \"127.0.0.1:8470\"\n\n[mgmt]\nsocket = \"/run/sew/mgmt.sock\"\n");
    std::fs::set_permissions(t.p(SEW_CONFIG), std::fs::Permissions::from_mode(0o640)).unwrap();
    sew_answers(&t, 0, r#"{"changed":["sew.toml"],"versions":{"sew":"0.0.2","pg":"16"}}"#);
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    ok(&o);
    let v = json(&o);
    assert_eq!(v["sewExit"], 0);
    assert_eq!(v["sew"]["versions"]["sew"], "0.0.2");
    assert_eq!(v["allowUid"]["uid"], 1001);
    assert_eq!(v["allowUid"]["changed"], true);

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

    // sew.toml: the uid joined [mgmt] allow_uids; other keys and the mode stay.
    let toml_text = std::fs::read_to_string(t.p(SEW_CONFIG)).unwrap();
    let doc: toml::Table = toml_text.parse().unwrap();
    assert_eq!(doc["mgmt"]["allow_uids"].as_array().unwrap(), &vec![toml::Value::Integer(1001)]);
    assert_eq!(doc["mgmt"]["socket"].as_str(), Some("/run/sew/mgmt.sock"));
    assert_eq!(doc["server"]["listen"].as_str(), Some("127.0.0.1:8470"));
    assert_eq!(std::fs::metadata(t.p(SEW_CONFIG)).unwrap().permissions().mode() & 0o777, 0o640);
    // Again: already listed, no change.
    let o = t.call_in(&["engine", "sew", "install"], install_body().as_bytes());
    assert_eq!(json(&o)["allowUid"]["changed"], false);
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

#[test]
fn allow_uids_keeps_others_and_refuses_a_linked_or_broken_toml() {
    let t = box_root();
    write(&t, SEW_CONFIG, "[mgmt]\nallow_uids = [2002]\n");
    assert!(crate::sew::allow_uid(&t.ctx, 1001).unwrap());
    let doc: toml::Table = std::fs::read_to_string(t.p(SEW_CONFIG)).unwrap().parse().unwrap();
    let uids: Vec<i64> = doc["mgmt"]["allow_uids"].as_array().unwrap().iter().map(|v| v.as_integer().unwrap()).collect();
    assert_eq!(uids, vec![2002, 1001]);
    write(&t, SEW_CONFIG, "[mgmt]\nallow_uids = \"all\"\n");
    assert!(crate::sew::allow_uid(&t.ctx, 1001).is_err());
    write(&t, SEW_CONFIG, "not = = toml");
    assert!(crate::sew::allow_uid(&t.ctx, 1001).is_err());
    std::fs::remove_file(t.p(SEW_CONFIG)).unwrap();
    write(&t, "/etc/other.toml", "[mgmt]\n");
    symlink(t.p("/etc/other.toml"), t.p(SEW_CONFIG)).unwrap();
    assert!(crate::sew::allow_uid(&t.ctx, 1001).is_err());
    assert_eq!(std::fs::read_to_string(t.p("/etc/other.toml")).unwrap(), "[mgmt]\n");
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
    let staged = t.p(SEW_STAGE_TARBALL).display().to_string();
    let pin = pinned_artifact("x86_64").unwrap().tarball_sha256;
    assert_eq!(sew_calls(&t)[0].0, vec!["upgrade", "--tarball", staged.as_str(), "--sha256", pin, "--json"]);

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
fn backups_read_without_the_lock_backup_takes_it() {
    let t = box_root();
    with_pin_installed(&t);
    sew_answers(&t, 0, r#"[{"id":"20260927T031512Z","kind":"daily","created_at":"x","bytes":10}]"#);
    let v = json(&t.call(&["engine", "sew", "backups"]));
    assert_eq!(v["sew"][0]["id"], "20260927T031512Z");
    ok(&t.call(&["engine", "sew", "backup"]));
    let argvs: Vec<Vec<String>> = sew_calls(&t).into_iter().map(|(a, _)| a).collect();
    assert_eq!(argvs, vec![vec!["backup", "list", "--json"], vec!["backup", "now", "--json"]]);
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
