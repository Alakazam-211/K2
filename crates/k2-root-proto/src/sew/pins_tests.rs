//! The newest pins held to their vendored SHA256SUMS files.

use super::tests::{all_artifacts, pin_equals_sums, sha256_hex};
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

/// 0.10.3 (the pin before 0.10.4; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// c5d18339…7c67): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note is checked here too. Its DeepFilterNet3 asset is the same
/// bytes as 0.10.2's.
#[test]
fn the_0_10_3_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.3.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "c5d18339dcccc4af27390ea447ba7d3a64f9766ebc7afe4e58ad5cf32c8c7c67",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.10.3", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.3").expect("0.10.3 is known");
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

/// 0.10.4 (the pin before 0.10.5; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 8fdcdf24…b1f0): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note (k2-pins-0104) is checked here too. Its DeepFilterNet3
/// asset is the same bytes as 0.10.3's (and 0.10.2's).
#[test]
fn the_0_10_4_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.4.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "8fdcdf24d5db5c1a46dec63f3219e7f6f90e036b5fcc291744bab5ded754b1f0",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.10.4", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.4").expect("0.10.4 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("a7ed703ff6045bb9bf06ae2016cc1249b00d8dab4aa7f40d8c9ace3394763c81", "dea46132ea5f930ec64d85126e51982c32ca89513d6ed3175dd3684ebe08cd02")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("621c6531d75c62906158fdfd7183b25455e6ffa14aba038a78e689b40773629b", "7e78c2d747e317492400c9dcced352b3c2e4223af242a117ec3f238cba942c1e")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "61aae7d3428ef727a4cdbeb60bc31594342266f84b3d4d72cc332c47224b17ea", "bf30774074c9df722f30be38378a61caf868687e4a5f3601f0b4747ce0069d7e"),
        (pin.transcriber.as_ref(), "ab6235d05508523436426f6bf058c1e9c1c220e50665de059e1c80343e37edd5", "5f319fe781cb7b913af13933b305d8ce2571e70b2da5dd4b3084ba851d9fee81"),
        (pin.recorder.as_ref(), "a3978790f59001d8577fa116acda54c2675f8d48c850ba3f584d70b5a501d536", "2ae6b1fb351179868dff5ab03bded2fdfa88feb671d766a2b61c85b840f1fdb7"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.4").map(|k| k.sha256),
        Some("4827cc8abf860223abdf9b7bd12b0cd4454dc66b64a9f5634307846db8e29a5f")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.10.4").expect("0.10.4 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.10.4.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.10.3").expect("0.10.3's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.10.3's");
    // 0.10.3, 0.10.2 and 0.10.0 are older (upgrade sources; no migration, so
    // the rollback to each is the plain one), by number, not as a string.
    for v in ["0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// 0.10.5 (the pin before 0.10.6; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 2cb14372…4b84f): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note (its corrected pins file) is checked here too. Its
/// DeepFilterNet3 asset is the same bytes as 0.10.4's (and 0.10.2's).
#[test]
fn the_0_10_5_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.5.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "2cb14372555b9a866e73ea0753393ed497e6fc13996c853686506f8032d4b84f",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.10.5", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.5").expect("0.10.5 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("a24a07003724f567f698051f6389e7328f57908b8ac5faed6039879ace843c6a", "dafecbfc7a5d77be303a024dc983aef4ccf2600164ab3de8a0add36a85ac78d8")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("e2dcd2fbfb533e5886228cd08627618a41f148a03e03345f5cc282973f02afca", "d06f2771c6759456ae9ef701cfa3115922da64438b5c01cd72ef75278733e6b6")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "2ed0105d65c605e2e246781a7a91d5b07108c8e1e94bf6efb53f27b7fe418b81", "44cdaa8d9cba94abfff0203d67727b43b56b1467d9662beab7cecb5e5730e10b"),
        (pin.transcriber.as_ref(), "5096f83f80986cb2784690ddddcf72d08e755cef1ca2aed0e8fd44d0ecfcb8cf", "0f0424a33968df14aa3958c561d4f6ae5d740b076c2e56d7024c5806c14b253b"),
        (pin.recorder.as_ref(), "d598292e5ee10dcf93de02b789e4cc340e98612d328f91d202e44e8f54f744a7", "c3b8eac7407645c4b7703aa58078cb7b4aa550c166da9e1b60cf261a41d6d5b3"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.5").map(|k| k.sha256),
        Some("efe66a88c4372a3c77f163231292b1654bbdde4c519c6e2116abeeb2f36cf1a9")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.10.5").expect("0.10.5 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.10.5.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.10.4").expect("0.10.4's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.10.4's");
    // 0.10.4, 0.10.3, 0.10.2 and 0.10.0 are older (upgrade sources), by
    // number, not as a string. 0.10.5's migration 1000501 means the way back
    // to any of them is the rollback with --accept-data-loss.
    for v in ["0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// 0.10.6 (the pin before 0.11.1; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 6a0a29dc…c6f4): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note (sew-0-10-6-is-live-pins) is checked here too, and each
/// was hashed again from the public v0.10.6 release. Its DeepFilterNet3
/// asset is the same bytes as 0.10.5's (and 0.10.2's).
#[test]
fn the_0_10_6_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.10.6.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "6a0a29dc80392d345792ca7e9ed4774ff5b6d5dd1f5c81d856dbdcc5baa7c6f4",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.10.6", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.10.6").expect("0.10.6 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("82132cca7b22933e890983375507d16449c19a3f720b16d4011b251fbf8adbc4", "5512be01df664a54ce536ed9ecb87674e0a5de666a763305bc4781eaabb33dc4")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("a43d5399b93cfa04ade4cd33925629b7b19d084001e7fda250c6b903d47f243d", "c9ebd0f278a49ed88736b44fadd60c7844d804399d1fa012d6552841b2a79f14")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "7e3adc3384d5bbb3b35b0489d8a1edbb0d75381e2982e79d3826f2f12821a6fc", "087115c36c37e28b5e1f70c70ee4e7953fb3ab5adf247b67b903a1f8efe8b890"),
        (pin.transcriber.as_ref(), "235b288948066d8c98409de47206ab336f6b00fa1f137aa3af57df7a03a5d162", "c4cd7a05bd3db7504ec6f14d78ff421618b09106a038de893237f4a50280f724"),
        (pin.recorder.as_ref(), "4784e4f28120b841394171f9bf296125b196fccd0b8ca79dd771bf123625c784", "2318f37430d4c0e85509c20defaf75b1a995a548b3631dcbfc0a1db48b92de24"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.10.6").map(|k| k.sha256),
        Some("4aab213ddddda542d2f7d47c01aeda608169632c4f2ac563a1cca65fd0238177")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.10.6").expect("0.10.6 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.10.6.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.10.5").expect("0.10.5's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.10.5's");
    // 0.10.5, 0.10.4, 0.10.3, 0.10.2 and 0.10.0 are older (upgrade sources),
    // by number, not as a string. 0.10.6's migration 1000601 means the way
    // back to any of them is the rollback with --accept-data-loss.
    for v in ["0.10.5", "0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// 0.11.1 (the pin before 0.11.3; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 6f42ab68…5438): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's pin note (sew-0-11-1-pins-published: the core and sidecar
/// tarballs, the core binaries, the kit, the model and DeepFilterNet3) is
/// checked here too, and each was hashed again from the public v0.11.1
/// release. Its DeepFilterNet3 asset is the same bytes as 0.10.6's.
#[test]
fn the_0_11_1_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.11.1.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "6f42ab68714a57d3552b1b4a4d21400f39167bc76cc49868e7f69ec6fb715438",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.11.1", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.11.1").expect("0.11.1 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("6da0a7bb2cad40b40a0eeb2d0aa039eabca28640f625da99328e4c7111f2ed43", "e55ae62d5396820f9365a8c2f07880e85472315e53d5ebabcb0c0ab3023f6738")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("6437e90715fcd72898b5b40ceb371edd774b4017d9565fbe7aad99cc276174ed", "4c26976f00006ab7b3b0a117bdcee399e325dab1ff15b7a91abd861b84bc8803")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "54cc60fb0b81dd7eb05d5ab5fe742c0c3598492c7ba921b6bf48c0c6a94972cb", "5079641a0a8a78939fb3920429e62ae227ebe1316df40857e2fbd6e1b6158563"),
        (pin.transcriber.as_ref(), "399e9651bd8f01113755485f214597c0492cf47d09d5ac8c0ba4159dc040a00d", "f8a0eefdeb114158c7a4f6d7bf6bc8a27efa9458154f4dbb122e20e6b636386f"),
        (pin.recorder.as_ref(), "8707882796b24a30318bd8c7943b686f903a1b727cd64447021869200e8df218", "c01d5e722523103aa6c15221691592c787c04dc4958b6f902416ff8e4ebfd3ef"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.11.1").map(|k| k.sha256),
        Some("fc432560a739c274724de6a16e83ea6642fe9906e120aa7582fe039f0ceb2c98")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.11.1").expect("0.11.1 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.11.1.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.10.6").expect("0.10.6's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.10.6's");
    // 0.10.6 back to 0.10.0 are older (upgrade sources), by number, not as
    // a string. 0.11.0's two migrations (0.11.1 adds none) mean the way back
    // to any of them is the rollback with --accept-data-loss.
    for v in ["0.10.6", "0.10.5", "0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// 0.11.3 (the pin before 0.11.4; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// c10cd9c7…68b9): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's 0.11.3 pin note (the core and sidecar tarballs, the core binaries,
/// the kit, the model and DeepFilterNet3) is checked here too, and each was
/// hashed again from the public v0.11.3 release. Its DeepFilterNet3 asset is the same bytes as 0.11.1's.
#[test]
fn the_0_11_3_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.11.3.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "c10cd9c7487dc05c0f74d3184624826670013949db50ef4e8a35cc3fcd1168b9",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.11.3", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.11.3").expect("0.11.3 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("0941e56f984c54830d53e73c1fb95ef2c954cddd727be6231d11cdd6fa22cb1a", "5e3b768d61625da775563ca9543b7ee87ef142d12e78fb4013122a0105cc6485")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("d570d911324e15f4c19dd5b0df271b16dad58473d1d0c3bbb19a1f36aefc393e", "492012b06766e74838aa3fb5316529c1a0a4d6dd3e535b25bbd8bc3223f3717a")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "7d00e6fd3250efe06180bbd03c32a78a923a9c6867f20c7683878dd36a3e3dcf", "8daabeddb34416da9a6d66a1a9d3740d0d7ed46d026bb35b2bbf6bbcc4a7fb74"),
        (pin.transcriber.as_ref(), "c43ca434102110be1e74f53add32cf97e921393aa44a444efd250a4cbd5eb46c", "ec97e9ac1d529b75bdcc253a9eb7da3ac9652000fee108a7ed6373dd87982cef"),
        (pin.recorder.as_ref(), "487823441276c28c3fb9fe2cd98eb981e2edbb741b41524a3a78650c5a14a133", "2339e97f615dd8dd22526830914d46bd1da639bc2dfc572aab28c9bf81535b79"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.11.3").map(|k| k.sha256),
        Some("faf8edeefa657b21d121713a43a1cf09eb0b9246ef47dfe8b0325601996857a0")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.11.3").expect("0.11.3 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.11.3.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.11.1").expect("0.11.1's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.11.1's");
    // 0.11.1 back to 0.10.0 are older (upgrade sources), by number, not as
    // a string. 0.11.3 adds no migration, so going back to 0.11.1 is a plain
    // rollback; 0.11.0's two mean the way back to 0.10.6 or older is the
    // rollback with --accept-data-loss.
    for v in ["0.11.1", "0.10.6", "0.10.5", "0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// 0.11.4 (the pin before 0.11.5; an upgrade source and rollback target) is
/// exactly its SHA256SUMS (vendored, sha256
/// 082839cd…6d26): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's 0.11.4 pin note (sew-0-11-4-pins-published: the core and sidecar
/// tarballs, the core binaries, the kit, the model and DeepFilterNet3) is
/// checked here too, and each was hashed again from the public v0.11.4
/// release. Its DeepFilterNet3 asset is the same bytes as 0.11.3's.
#[test]
fn the_0_11_4_entry_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.11.4.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "082839cd0aafb8f6cae4cff92f9b210bae8fbf52f377ea821362e590a01b6d26",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(
        pin_equals_sums("0.11.4", sums),
        19,
        "4 binaries × 2 arches × (tarball, member) + the App kit + the DeepFilterNet3 asset and weights"
    );
    let pin = SEW_RELEASES.iter().find(|r| r.version == "0.11.4").expect("0.11.4 is known");
    let bin = |side: Option<&SewSidecar>, arch: &str| {
        let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
        arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
    };
    assert_eq!(
        bin(None, "x86_64"),
        ("d2bdd098cd828cd946cd0ed15be3b5508386e66a5197b72c71734896f93ada8e", "fb635e693a7b49bf3822afe9aa87fa494e386b8caa4418eb5613fc125c2c1bc8")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("5936272ccdc4b4d2ccaa2f1488470285aac615fefb49f8aa4612f59e88cab4e2", "86ea61d837b0672b567101760857fdc9a84133dbefefbdf81fbcc19a6fd3ef72")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "58aa919e09c9a6a8f19395bc131861de051e95824f0a71997aba7b739af33eb6", "8c74906955cdc07ac9eb2b473aec07790011579419796c86b36f4f340c343e13"),
        (pin.transcriber.as_ref(), "51f50156b9e3b0ec1ccf4a14c4b64e8feabc5dbc1e04746b912a8a10d0f12d27", "22575f7dfee852f6d5446fb082a2700cb0a0416b7d8709def89ff9fa96a163ca"),
        (pin.recorder.as_ref(), "0677c1decace7d24d4225887afe85cf41a5e42ff214a9b9ee69d1c558e47c98f", "126963b6944d2f1a68d10e0199504c5bb7bf5d12902716ff04518276c84c63dd"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.11.4").map(|k| k.sha256),
        Some("7d2148583e13ad9dc7c996b5094c56b8e9dd23fb89f6b08d53c3c0a11eea20cb")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.11.4").expect("0.11.4 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.11.4.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.11.3").expect("0.11.3's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.11.3's");
    // 0.11.3 back to 0.10.0 are older (upgrade sources), by number, not as
    // a string. 0.11.4 adds no migration, so going back to 0.11.3 or 0.11.1
    // is a plain rollback; 0.11.0's two mean the way back to 0.10.6 or older
    // is the rollback with --accept-data-loss.
    for v in ["0.11.3", "0.11.1", "0.10.6", "0.10.5", "0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
}

/// The 0.11.5 pin is exactly its SHA256SUMS (vendored, sha256
/// c55d50be…f62c): every core and sidecar tarball and binary member line,
/// the App kit's, and the DeepFilterNet3 bundle's two lines. Each hash in
/// Sew's 0.11.5 pin note (core pins from sew-0.11.5-release.json, PINS-CHECK
/// PASS: the core and sidecar tarballs, the core binaries, the kit, the model
/// and DeepFilterNet3) is checked here too, and each was hashed again from the
/// public v0.11.5 release. Its DeepFilterNet3 asset is the same bytes as 0.11.4's.
#[test]
fn the_0_11_5_pin_equals_its_sha256sums() {
    let sums = include_str!("../../testdata/sew/sew-0.11.5.SHA256SUMS");
    assert_eq!(
        sha256_hex(sums.as_bytes()),
        "c55d50be8f8ae7b7369ef5e83a9c380b4f1b6db879a5785a25b5671acb38f62c",
        "the vendored SUMS file is the published one"
    );
    assert_eq!(SEW_RELEASES.last().unwrap().version, "0.11.5", "0.11.5 is the pin");
    assert_eq!(
        pin_equals_sums("0.11.5", sums),
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
        ("59cc57ce10527721e581252cc7354aaec58b17f3f7ce1b9d0a78cd5ba276d1dc", "d147b7083939e0b4c710168f180840d0fba01b30792823ee10decd7fbd4ea61d")
    );
    assert_eq!(
        bin(None, "aarch64"),
        ("772b820c41d830ab1f6c247609ed787c6ae686aaa2f1ab5eaf57b192629cf856", "b50232069dcb551a0d9d738ae5b0c93df8614d515c682d6ac271e1f93265fbe3")
    );
    for (side, x86, arm) in [
        (pin.media.as_ref(), "33b392840738ff9bd9fd63a3afcb9da741b45f4c891dc096a26b9a0a64a0a3a5", "55639efc747c76b8c80a601126193349895f26b946bd87da6bb344972abaa255"),
        (pin.transcriber.as_ref(), "0b264789ec206268d308b8480618a8e158792fffb51640b66cb5d4d63777fbbd", "4e1c91ae81fcc3cde2b75e2a88c9677773fbb944f911c407650e387f1bec20bb"),
        (pin.recorder.as_ref(), "48c1d92f83ce05d0f6d81ca4fe4d33d4b0578dd4c3fdd8d6364171d37d090ed3", "b5da93a4d994d17b38316cdc81ac0c272a27d77bd9ccf3774461f604b59a70c3"),
    ] {
        assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
    }
    assert_eq!(
        SEW_APP_KITS.iter().find(|k| k.version == "0.11.5").map(|k| k.sha256),
        Some("d2182ca2420fe9cd5a4e488044c1d92ee39b94afd67ecd79e38298cdbdbec894")
    );
    let d = SEW_DFN3.iter().find(|d| d.version == "0.11.5").expect("0.11.5 ships DeepFilterNet3");
    assert_eq!(
        (d.file, d.sha256, d.bytes, d.weights_sha256),
        (
            "sew-dfn3-0.11.5.tar.gz",
            "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
            36_353_582,
            "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
        )
    );
    let prev = SEW_DFN3.iter().find(|d| d.version == "0.11.4").expect("0.11.4's bundle stays");
    assert_eq!((prev.sha256, prev.weights_sha256), (d.sha256, d.weights_sha256), "the same bytes as 0.11.4's");
    assert_eq!(SEW_DFN3.last().map(|d| d.version), Some("0.11.5"), "the app ships the pin's bundle");
    // 0.11.4 back to 0.10.0 are older (upgrade sources), by number, not as
    // a string. 0.11.5 adds no migration, so going back to 0.11.4, 0.11.3 or
    // 0.11.1 is a plain rollback; 0.11.0's two mean the way back to 0.10.6 or
    // older is the rollback with --accept-data-loss.
    for v in ["0.11.4", "0.11.3", "0.11.1", "0.10.6", "0.10.5", "0.10.4", "0.10.3", "0.10.2", "0.10.0"] {
        assert_eq!(version_cmp(v, pin.version), Some(std::cmp::Ordering::Less), "{v}");
    }
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
    assert_eq!(pinned().version, "0.11.5");
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

/// The real table: the old 0.1.0 placeholder stays and refuses, 0.2.0
/// and 0.2.1 through 0.11.4 stay known; the pin is Sew 0.11.5 for both
/// linux-gnu triples, with all three sidecars, equal to its SHA256SUMS.
#[test]
fn real_table_pins_sew_0_11_5_and_keeps_the_old_entries() {
    assert_eq!(SEW_RELEASES.len(), 28);
    let known = [(26, "0.11.4"), (25, "0.11.3"), (24, "0.11.1"), (23, "0.10.6"), (22, "0.10.5"), (21, "0.10.4"), (20, "0.10.3"), (19, "0.10.2"), (18, "0.10.0"), (17, "0.9.3"), (16, "0.9.2"), (15, "0.9.0"), (14, "0.8.3")];
    for (i, v) in known.into_iter().chain([(13, "0.8.2"), (12, "0.8.1"), (11, "0.8.0"), (10, "0.7.3"), (9, "0.7.2"), (8, "0.7.1")]) {
        assert_eq!(SEW_RELEASES[i].version, v);
        assert!(release_is_pinned(&SEW_RELEASES[i]), "{v} stays a real release (an upgrade source and rollback target)");
    }
    assert_eq!(SEW_RELEASES[7].version, "0.6.1");
    assert!(release_is_pinned(&SEW_RELEASES[7]), "0.6.1 stays a real release (the upgrade source)");
    assert_eq!(SEW_RELEASES[6].version, "0.6.0");
    assert!(release_is_pinned(&SEW_RELEASES[6]), "0.6.0 stays a real release (the upgrade source)");
    assert_eq!(SEW_RELEASES[5].version, "0.5.0");
    assert!(release_is_pinned(&SEW_RELEASES[5]), "0.5.0 stays a real release (the upgrade source)");
    assert_eq!(SEW_RELEASES[4].version, "0.3.1");
    assert!(release_is_pinned(&SEW_RELEASES[4]), "0.3.1 stays a real release (the upgrade source)");
    assert_eq!(SEW_RELEASES[3].version, "0.3.0");
    assert!(release_is_pinned(&SEW_RELEASES[3]), "0.3.0 stays a real release (the reconfigure source)");
    assert_eq!(SEW_RELEASES[1].version, "0.2.0");
    assert!(release_is_pinned(&SEW_RELEASES[1]), "0.2.0 stays a real release");
    assert_eq!(SEW_RELEASES[2].version, "0.2.1");
    assert!(release_is_pinned(&SEW_RELEASES[2]), "0.2.1 stays a real release");
    let old = &SEW_RELEASES[0];
    assert_eq!(old.version, "0.1.0");
    assert!(old.artifacts.iter().all(checksum_is_placeholder));
    assert!(!release_is_pinned(old));
    assert!(artifact_in(old, "x86_64").unwrap_err().contains("no Sew pin"));

    let pin = SEW_RELEASES.last().unwrap();
    assert_eq!(pin.version, "0.11.5");
    assert!(release_is_pinned(pin));
    assert!(artifact_in(pin, "riscv64").unwrap_err().contains("unsupported"));
    let want = [
        ("x86_64", "59cc57ce10527721e581252cc7354aaec58b17f3f7ce1b9d0a78cd5ba276d1dc", "d147b7083939e0b4c710168f180840d0fba01b30792823ee10decd7fbd4ea61d"),
        ("aarch64", "772b820c41d830ab1f6c247609ed787c6ae686aaa2f1ab5eaf57b192629cf856", "b50232069dcb551a0d9d738ae5b0c93df8614d515c682d6ac271e1f93265fbe3"),
    ];
    for (arch, tar, bin) in want {
        let a = artifact_in(pin, arch).expect("pinned");
        assert_eq!((a.tarball_sha256, a.binary_sha256), (tar, bin), "{arch}");
        assert_eq!(a.triple, format!("{arch}-unknown-linux-gnu"));
    }
    for (name, s) in [("media", &pin.media), ("transcriber", &pin.transcriber), ("recorder", &pin.recorder)] {
        let s = s.as_ref().unwrap_or_else(|| panic!("{name} pinned"));
        assert_eq!(s.version, "0.11.5", "{name}");
        let mut arches: Vec<&str> = s.artifacts.iter().map(|a| a.arch).collect();
        arches.sort();
        assert_eq!(arches, ["aarch64", "x86_64"], "{name}");
    }
    for a in all_artifacts(pin) {
        assert!(!checksum_is_placeholder(a), "{a:?}");
    }
    // Every hash is distinct (no copy-paste slip between rows), across
    // all twenty-seven real releases.
    let mut seen = std::collections::HashSet::new();
    for r in &SEW_RELEASES[1..] {
        for a in all_artifacts(r) {
            assert!(seen.insert(a.tarball_sha256) && seen.insert(a.binary_sha256), "{a:?}");
        }
    }
    assert_eq!(seen.len(), 432);
}
