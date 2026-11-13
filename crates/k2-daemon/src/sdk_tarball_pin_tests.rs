//! The bytes of the `@k2/server` tarball are pinned here, in the daemon's
//! own build.
//!
//! `k2 app sdk add` vendors that tarball into an App and reports its
//! sha256, which is over the gzip bytes. Those bytes depend on the deflate
//! code flate2 is built with, and cargo picks that for the whole build: a
//! crate elsewhere in the daemon's dependencies that turns on another
//! flate2 backend changes the tarball for the same contents (k2-deckrender's
//! zip did, through zlib-rs, on 2026-11-11). k2-core's own tests can't
//! see that, because they build without the daemon's dependencies. This
//! test does.

/// A fixed input packs to the bytes 0.46.12 produced. If this fails, the
/// tarball an App vendors has changed for unchanged SDK contents: find
/// what changed the gzip (a flate2 backend feature in Cargo.lock is the
/// first place to look) before touching the pin.
#[test]
fn a_fixed_input_packs_to_the_same_tarball_as_0_46_12() {
    let entries = vec![
        ("LICENSE.md".to_string(), b"MIT\n".to_vec()),
        ("dist/index.js".to_string(), b"export const k2 = 1;\n".repeat(200)),
        ("package.json".to_string(), br#"{"name":"@k2/server","version":"0.0.0"}"#.to_vec()),
    ];
    let tgz = k2_core::app_sdk::k2_server::pack(&entries);
    assert_eq!(
        k2_core::app_sdk::sha256_hex(&tgz),
        "c041c9a0f14312e637217bd24f788216ebc4e7a0a710b2ebba3233ab7291a185",
        "the @k2/server tarball's bytes changed for the same input ({} bytes)",
        tgz.len()
    );
}
