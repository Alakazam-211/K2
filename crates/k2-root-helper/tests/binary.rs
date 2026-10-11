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
