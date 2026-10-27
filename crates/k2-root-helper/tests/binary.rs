//! The built binary itself (never run as root here): exactly one build
//! marker, `helper version` works unprivileged, everything else refuses
//! without root, and refused argv never echoes caller bytes.
// Server-only feature (Linux boxes); not shipped on personal Windows machines.
#![cfg(unix)]

use std::process::{Command, Stdio};

use k2_root_proto::version::{find_markers, format_marker, PROTOCOL};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_k2-root-helper")
}

#[test]
fn the_binary_carries_exactly_one_matching_marker() {
    let bytes = std::fs::read(bin()).expect("read built helper");
    let markers = find_markers(&bytes).expect("markers parse");
    assert_eq!(markers.len(), 1, "exactly one build marker in the binary");
    let m = &markers[0];
    assert_eq!(m.protocol, PROTOCOL);
    assert_eq!(m.arch, std::env::consts::ARCH);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(&format_marker(
        &m.version.to_string(),
        PROTOCOL,
        std::env::consts::ARCH
    )));
    #[cfg(not(feature = "test-keys"))]
    assert!(
        !text.contains("K2_ROOT_HELPER_TEST_KEYS_ENABLED"),
        "a default build never carries the test-keys flag"
    );
}

/// Sew's TEST-build pins (`sew-test-pins`) never reach a default build:
/// not the flag release signing greps for, not one of the test hashes, and
/// `helper version` reports no test pins.
#[cfg(not(feature = "sew-test-pins"))]
#[test]
fn a_default_build_carries_no_sew_test_pins() {
    // Sew 0.1.0 / 0.1.1 aarch64 test builds: tarball and binary sha256 of
    // the core, media, transcriber and recorder (release.json).
    const TEST_PIN_HASHES: &[&str] = &[
        "922f8f285a87bd22309eab916d000874004c02765a77d8abc7a74349a99b9006",
        "4dfe51382507e748830a0e008fb96a19cecc7c6d2adb9063c0b2dc533f669837",
        "2ebb00641837b17a7253d7984656c62a4c102705a0002cc18a8645f827d596a6",
        "5edf44e592c28ec3831b495434b98323133ab6438cd5ed74e43e84cea10883d6",
        "62630e62965cf3aa46d923a934a2a1bde2920b4ecff7b4cdb29dcf25922aff29",
        "e54b37f67dd701f4eed85a6a124c3f7be12ccc77a33a4fe2dd616804973fc919",
        "9370335de788e47e4be40aca4c9a8907bfc5112a708962146c594380b7260308",
        "110ebcaa87bf55283b0197dfdd8b209d73ac32d4eb164de3193409a594a05be1",
        "82fe9e196887b9cfa34528275e535df1b82f7f26309c97025767ee6821e189ba",
        "35d334be98c5610d45f72c3d19e82eaca02ea3b17ecc37910532f26f9f3f0ade",
        "a7d01cae2b5da55b0dfd5541a61833a03a175ccdc408dbe78f3ea6d8f187c328",
        "35958529b04ccf31a70bfbf147ee64f9803c44fb935c2523c76cc69030423355",
        "95a4260fe6297e4fa95bb8f68b40d52145530151b91e160cb8ffe56a70f15660",
        "13e1301969d726652c40f81a5aaac9b2cb45b9ef15aee003aebd3a7ca299460b",
        "231e744a1f824a918b898d9b5c2107a68a028fea6bd9ad2af63dbecfe7a4f3fa",
        "dd113b1d5844098b5cc92d04849291b6a5a9c678f064d6fbb270da214f5a284a",
    ];
    let bytes = std::fs::read(bin()).expect("read built helper");
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("K2_SEW_TEST_PINS_ENABLED"), "the sew-test-pins flag is in a default build");
    for h in TEST_PIN_HASHES {
        assert!(!text.contains(h), "Sew test pin {h} is in a default build");
    }
    assert!(k2_root_proto::sew::test_pins_flag().is_none());
    for r in k2_root_proto::sew::SEW_RELEASES {
        for a in r.artifacts {
            assert!(!TEST_PIN_HASHES.contains(&a.tarball_sha256) && !TEST_PIN_HASHES.contains(&a.binary_sha256), "{a:?}");
        }
    }
    let out = Command::new(bin()).args(["helper", "version"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["sewTestPins"].is_null(), "{v}");
}

#[test]
fn version_runs_unprivileged_and_other_verbs_refuse() {
    // SAFETY: no preconditions.
    assert_ne!(
        unsafe { libc_geteuid() },
        0,
        "run the helper tests as a non-root user"
    );
    let out = Command::new(bin())
        .args(["helper", "version"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["protocol"], PROTOCOL);
    assert_eq!(v["name"], "k2-root-helper");

    let out = Command::new(bin())
        .args(["mail", "usage"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));

    let out = Command::new(bin())
        .args(["mail", "remove", "/etc/CANARY-xyz"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("CANARY"));
}

extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}
