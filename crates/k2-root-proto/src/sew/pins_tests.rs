//! The newest pins held to their vendored SHA256SUMS files.

use super::tests::{pin_equals_sums, sha256_hex};
use super::*;

/// 0.9.2 (the pin before 0.9.3; an upgrade source and rollback target)
/// is exactly the published SHA256SUMS (vendored from the v0.9.2
/// release, sha256 8e9a63bb…3f25d): every core and sidecar tarball and
/// binary member line, plus the App kit's. Each hash in Sew's pin note
/// (2026-11-11) is checked here too, so a slip in either the table or
/// the vendored file fails.
#[test]
fn the_0_9_2_entry_equals_the_published_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.9.2.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "8e9a63bb00acb76d14eb91d5fe927bc2b247bd1d4ef40422a3ea7a2654a3f25d",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(pin_equals_sums("0.9.2", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.9.2").expect("0.9.2 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("91985f336d747e4490bd26d56e733381492b11a4f8f7f1da81fbb440cb53dd8b", "f13d7c7cd3be3a114dceaf1b36ab0092735fd971471c53f68201a32b7cf0d1c6")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("fce76f3ef55ad53ea6ad3f32614551f806fd112f5aede8caee4be8aced2c2038", "9af08256897c1a279ab1919c78eb2bb12af1561246f197f58eea3602b59117fd")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "ee9da2230c133e563909ee248d7fff30400c1c71ccbcbea25e7ebabe042c50e1", "8fb823bbfe24836a218741d57516b1920bd151b76fb10117b4c7e973b16d9649"),
        (pin.transcriber.as_ref(), "94dbee0b56afdf4b0f199f1facd2fe356426398f6792d6829f9dc991dbf457d5", "53b0708c9ca1d4ec9363cadf1ffa33a04affff324b095957bcc1b5d3fbecea63"),
        (pin.recorder.as_ref(), "9e9354e76ecac2e9c3de54d219908ca8da3e8d63f10039eac5d6a8cb70278cf7", "ecf082ea188df04a6bbef4808eaf24e62afa64abd4d82d2747a7a3aec9d71ee0"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.9.2").map(|k| k.sha256),
        Some("59f9ae96dcc9a54597daeba5f3b73fe3fc6f720a7acc7e0cb1e64caa45a34a20")
    );
}

/// 0.9.3 (the pin before 0.10.0; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256 2811b8e1…7264): every core and
/// sidecar tarball and binary member line, plus the App kit's. The core
/// tarballs and binaries, the sidecar tarballs and the kit are also checked
/// against the hashes in Sew's pin note, so a slip in either the table or
/// the vendored file fails.
#[test]
fn the_0_9_3_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.9.3.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "2811b8e144cf90da7ed763c4d41807a7058c510aa62ce79db36c73ab00837264",
        "the vendored SUMS file is Sew's"
    );
    assert_eq!(pin_equals_sums("0.9.3", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.9.3").expect("0.9.3 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("2e7e4b11baf7cc03f88f56d25a51fcf0980b9894a661961608859b04230b7ecb", "f64d3f511e4441487f30c713aac571a7ee614d461f7c49ffbd951e7210a0c3cc")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("57b5adcb7f21f91f12bdb9137dc6720895bff17162a259e32b21d995bfa29acc", "b93b2ae448afd2132bf1d5865936310216fbe49b9ef759f6e0b29a4ee6fccd47")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "c96720f9e88eb404af65fd15b3e8f176028bbd887ba24818cc081bfdedece526", "05df6dfd3f18d6b46c0ed8f62e8a22fabf821e48bc3b5b2d1ffd5e1850ed7ea2"),
        (pin.transcriber.as_ref(), "24fcb26d14e35aa8d39f99c6aa247cdeae314721099cab06182286d94855b805", "f0f389505dd2f8166c22e2294992efdfaabca3b83ba59d29a9f0bb383469b17f"),
        (pin.recorder.as_ref(), "5d67bcfe51856ae7a4111373343b308feec457ffbba62ef526985c8c9941d9eb", "8d712d686df9d984fcb42638d98a38b485910c5a6a536c4a8523e40e0388741c"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.9.3").map(|k| k.sha256),
        Some("7e956da38e07dc2a20c63b87c6b64c965f48f69eebbd5dd95cd0bd27fc8378e0")
    );
}

/// 0.10.0 (the pin before 0.10.2; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 4b3bce65…905a, as Sew's pin note names it): every core and sidecar
/// tarball and binary member line, plus the App kit's. The core tarballs
/// and binaries, the sidecar tarballs and the kit are also checked against
/// the hashes in Sew's pin note, so a slip in either the table or the
/// vendored file fails.
#[test]
fn the_0_10_0_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.0.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "4b3bce65cec92e144811a77365390de2946543cc4a83cff048055317f1f6905a",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(pin_equals_sums("0.10.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.0").expect("0.10.0 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("befdaa2b0fea78044e9acc060851328a942cf68e45c070b2e2ad41e71cd3fd06", "c9665962a04e2c77508ffd558cad747a904c4d8aa19d8e96ce40e4449a8803a9")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("7f2bf6949329e682fa3225de0bf12e16408c4749f69d7e892044a91419046901", "61a163a4b247c6f91ee6145e2ef4339b46193b01a02dda15aaf811a334d4e5d7")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "b4aadcde5cec8494c8b7360c2c9fa1e940948a809653b0b0a88cd52bd7d36b7a", "5e2c74db8eaac51f93c8d7daafa8491284f48fe1d792dd2e21b7ddeb49d303ad"),
        (pin.transcriber.as_ref(), "dd2eebf1285d26168a39d1694be5713169f0c5b43938827f61acb4a1ed7145bc", "3abb05e03bfdb9c1082907752c62e049b2b533381f3f11efc85e176a17e96bab"),
        (pin.recorder.as_ref(), "b6b36b506bed96feb525a32e9c66a61ea0dab27283fb986e1f9c702ebbf9a7ae", "9f3938b9efe328013fedbfd087b12f688df7d4ad05ee080ad433e07af044f30e"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.0").map(|k| k.sha256),
        Some("61ff6ca52f74c2f3ef77060730b808602038629560bc684b52d306d4f55d502e")
    );
    // 0.9.3 is older (an upgrade source, the rollback target across 1000101),
    // by number, not as a string.
    assert_eq!(version_cmp("0.9.3", pin.version), Some(std::cmp::Ordering::Less));
}

/// 0.10.2 (the pin before 0.10.3; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 1b1ae068…ae33): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines (the asset and
/// its weights member). Each hash in Sew's pin note is checked here too, so a
/// slip in either the table or the vendored file fails.
#[test]
fn the_0_10_2_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.2.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "1b1ae068e250b9bd0ec18a504507379cdfb755d6bb168c09b76f9f705a73ae33",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.10.2", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.2").expect("0.10.2 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("846ae420121a8bfb061519d8e2a69e1e1b20b9659f9965f758cbdb7fc8f38aa2", "43213a2387d60fe7bcb7dc609222004ce5bcff816d4c83a7d938606329333fe5")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("170f9564ca1cea0c7bd92bd8cf1be0bab4d7822bf7a0c2410fe78006c9bfdfd3", "8ee7eb268cadbe11f5f91a214db6ae7be6534764e4d2d1af9b6327d65bae0901")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "aecc84fe3ad805055b594953c5d057328a548e906c1163eb1e641d72d766a4e0", "0a01057e7c6a834d6d54e5cac8ea520a84bdd6b833c0a83e9577a1d343d27018"),
        (pin.transcriber.as_ref(), "5b443957de94a9a853773eebb294b4444a441e9d3c613b363ff354cf277fafc1", "7ef1508b12539af202a2a0075d3ddc3d0c2a4394bb16b70454cfadb9f8d91b96"),
        (pin.recorder.as_ref(), "7f4ec4b70cd6a4e8cb265bb433b3d9657b34c4cdf2a715a00c785800cce6c090", "d2abc955ef395db085ff076b51ddcde305475a7f356ef0c65ffdeae15eeccaf6"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.2").map(|k| k.sha256),
        Some("1615bc10998f16483dc6883c0e7c61a90b388ee0eb2f6ffad2ca4b584df023dc")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.10.2").expect("0.10.2 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.10.2.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    // 0.10.0 is older (an upgrade source; no migration between them, so the
    // rollback to it is the plain one), by number, not as a string.
    assert_eq!(version_cmp("0.10.0", pin.version), Some(std::cmp::Ordering::Less));
}

/// The 0.10.3 pin is exactly its SHA256SUMS (vendored, sha256
/// c5d18339…7c67): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note is checked here too. Its DeepFilterNet3 asset is the same
/// bytes as 0.10.2's.
#[test]
fn the_0_10_3_pin_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.3.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "c5d18339dcccc4af27390ea447ba7d3a64f9766ebc7afe4e58ad5cf32c8c7c67",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(SEW_RELEASES.last().unwrap().version, "0.10.3", "0.10.3 is the pin");
    assert_eq!(
        pin_equals_sums("0.10.3", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.last().unwrap();
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("b79cebeb2410c1bcba3edfd9d972b6cc847572911303f8bb329badf185dd7564", "6997f5ffc26154b0f85a494027983b4cb5608496384e378e07bfbec7f59399d3")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("be1ee72bd949918fab9b70df847f2c45f18d5af4281454fd0d94a4336814b68c", "47ab234bb14b3d04e629f3a9722da9974e8d6cbd6f88ffff6adff12d1c90d2e3")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "134a2c98a9daf39132a4ead2cdb7cc4ff45cc5d7ae0e78e14b23b240d2bea2c3", "7dcd0033f2bab3bda78404fe96d012bd377c0ed7424dca153ecc9115c9c07029"),
        (pin.transcriber.as_ref(), "2916e2eb469a322eecc9bf05871ade681d1f1f1f91c7fd3b5895f8b5e4b154d9", "2bb8c5cc113a74edd07fa69ebd482b953762b7bf00b3a19e266120aa3b768a1f"),
        (pin.recorder.as_ref(), "7be1fbd82ae2d605b8b567c14cab47d29cfc9d6cd36de36907507541ef68f40c", "77aee904785ec93b2436e12a237dab130caa22b2f0fd135c5c19d8dab7b68124"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.3").map(|k| k.sha256),
        Some("120f8ed70653a0ec22adec99c7f699ecbaede374dc7f5636177ef1a73fe00d4a")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.10.3").expect("0.10.3 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.10.3.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.10.2").expect("0.10.2's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.10.2's");
    // 0.10.2 and 0.10.0 are older (upgrade sources; no migration, so the
    // rollback to either is the plain one), by number, not as a string.
    assert_eq!(version_cmp("0.10.2", pin.version), Some(std::cmp::Ordering::Less));
    assert_eq!(version_cmp("0.10.0", pin.version), Some(std::cmp::Ordering::Less));
}

/// A box moved to Sew 0.8.3 by hand (never K2's pin) upgrades to the
/// 0.9.0 pin as a plain release: its published binary (the sha256 its
/// own `sew version` printed on a compute node, 37c7d41d… on x86_64) is
/// a known release, not an accept-binary case; it is older than the pin;
/// and a rollback onto it has its own voice engine to put back. A binary
/// that is NOT the published 0.8.3 stays unknown (the owner's
/// `--accept-binary` path, helper item 12).
#[cfg(not(any(feature = "test-signer", feature = "sew-test-pins")))]
#[test]
fn a_box_on_sew_0_8_3_upgrades_to_the_0_9_0_pin_as_a_known_release() {
    let x86 = "37c7d41d371eff99eb943a76ffe10140c398310fd087d1ac34bf6ac532dfc277";
    assert_eq!(known_binary(x86, "x86_64"), Some("0.8.3"));
    assert_eq!(known_binary(x86, "aarch64"), None, "another arch's binary is not this arch's release");
    assert!(upgrade_only_binary(x86, "x86_64").is_none(), "a release, never an upgrade-only build");
    let r = release("0.8.3").expect("0.8.3 is known");
    for a in r.artifacts {
        assert_eq!(known_binary(a.binary_sha256, a.arch), Some("0.8.3"), "{}", a.arch);
    }
    assert_eq!(pinned().version, "0.10.3");
    assert_eq!(version_cmp("0.8.3", pinned().version), Some(std::cmp::Ordering::Less), "an upgrade, not a downgrade");
    assert_eq!(version_cmp("0.8.2", "0.8.3"), Some(std::cmp::Ordering::Less), "0.8.3 sits between 0.8.2 and the pin");
    for arch in ["x86_64", "aarch64"] {
        let (mv, m) = media_artifact_of("0.8.3", arch).expect("0.8.3's engine");
        assert_eq!(mv, "0.8.3");
        assert_eq!(known_media_tarball(m.tarball_sha256, arch), Some("0.8.3"), "kept for a rollback to 0.8.3");
    }
    // A hand-built 0.8.3 (any other sha) is no release K2 knows.
    assert_eq!(known_binary(&"ab".repeat(32), "x86_64"), None);
}
