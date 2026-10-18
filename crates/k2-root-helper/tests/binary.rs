//! The built binary itself (never run as root here): exactly one build
//! marker, `helper version` works unprivileged, everything else refuses
//! without root, and refused argv never echoes caller bytes.

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
        // Sew 0.2.1 (demo 0461e test pins), both triples.
        "8116e64ae13d43da196b77fbe1ff02ef81d9860212de3f7585094ed83b606ee2",
        "f909915d05886752f5a496c895b7ad248f79e1f5d32a95a3ef1c65f578be3cd7",
        "6cad696d906b250301fe34e44c080bc127025012257a562622ee85d1488cafcc",
        "cf456d16f2031b7b2b952ab10c333bdaea8a421aed435dfce6ae033f500a8a6e",
        "cfac4231e141ab0678778c40787c2186c13a43c2cefe7c6a1399b2bd0fabe401",
        "e806246429f954899630158d0da11e5ed6caaa7157d18cbd24599493f202456c",
        "abcecd1e5e6523e2a8cc6549beb299ffab28d3fa8960eb0dbda91287bbcfbed4",
        "7fbbbb9a6b46d9e76b4c2911eaa90715d61bcb8c8a8078d6a78201c46d1f7884",
        "d3671fd457d2cfcbedab9f8b9376840f1a3a530addfcb5bbf0b4b6a8300b108f",
        "1ca098c50c82975bdb6acf9e635caeb5b17245625ed0aaaf8ef9f3d81ce8acf5",
        "f840a861c9e4536e5777698f2c24475dc5bfa688a16f5223fec9c5e5e036675b",
        "b20b1684abdf8add1bf63b84bfa747b91dba4650c0dce9c1ef04ce12714434ce",
        "79db2e6348bcbb64c4dd0b6bfb947882110f4e334020bcf61e52783a599dbcd1",
        "6c62db7d4fad906c6b22cf6b7175257f9f50fd0af5f5d127841f0b3bd5470dda",
        "c45032512d75347603712369980197f9b92fb346a3d59c985b6739fb0615fd0b",
        "ad50834969fc941951b5b2fb0b122118d6572b6391bf0b72fb6c51cb92f1ef04",
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
