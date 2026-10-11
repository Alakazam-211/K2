//! Signed `k2-node` builds (`prd-k2-node-admin-v1` §8.1, Q4).
//!
//! `k2-node` gains a root mode (`k2-node admin serve`), so its updates are
//! signed with the **root-helper key** ([`crate::keys::RELEASE_KEYS`]),
//! never the app-updater key. One key now guards two binaries; the
//! trusted comment's file name keeps them apart: a node verifier refuses
//! `file:k2-root-helper-…` and the helper verifier refuses `file:k2-node-…`.
//!
//! - Asset: `k2-node-<os>-<arch>` (`linux`/`macos` × `x86_64`/`aarch64`).
//! - Trusted comment: `file:k2-node-<os>-<arch> version:<semver> protocol:<n>`
//!   (signed with plain `minisign -S -t`, like the helper).
//! - Build marker, exactly once in the binary:
//!   `K2_NODE_BUILD v=<semver> proto=<n> os=<os> arch=<arch>;`
//! - Binary format: 64-bit little-endian ELF (Linux) or a thin 64-bit
//!   Mach-O (macOS) for that arch.

use crate::keys::{key_id_of, TrustedKey};
use crate::sig::parse_trusted_comment;
use crate::version::{arch_ok, elf_matches, Semver};

pub fn os_ok(os: &str) -> bool {
    matches!(os, "linux" | "macos")
}

pub fn asset_name(os: &str, arch: &str) -> String {
    format!("k2-node-{os}-{arch}")
}

pub fn trusted_comment(os: &str, arch: &str, version: &str, protocol: u32) -> String {
    format!("file:{} version:{version} protocol:{protocol}", asset_name(os, arch))
}

/// The marker needle, stored shifted by one byte so the plain prefix
/// appears in a binary only inside its one embedded marker.
pub fn marker_prefix() -> String {
    let shifted: &[u8] = std::hint::black_box(b"L3`OPEF`CVJME!w>");
    shifted.iter().map(|b| (b - 1) as char).collect()
}

pub fn format_marker(version: &str, protocol: u32, os: &str, arch: &str) -> String {
    format!("{}{version} proto={protocol} os={os} arch={arch};", marker_prefix())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMarker {
    pub version: Semver,
    pub protocol: u32,
    pub os: String,
    pub arch: String,
}

/// Every node marker in `bytes`; `Err` on any doctored occurrence.
pub fn find_markers(bytes: &[u8]) -> Result<Vec<NodeMarker>, &'static str> {
    let needle = marker_prefix();
    let needle = needle.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let rest = &bytes[i + needle.len()..];
        let end = rest.iter().take(80).position(|&b| b == b';').ok_or("marker not terminated")?;
        let body = std::str::from_utf8(&rest[..end]).map_err(|_| "marker not utf-8")?;
        out.push(parse_marker_body(body).ok_or("marker malformed")?);
        i += needle.len() + end + 1;
    }
    Ok(out)
}

fn parse_marker_body(body: &str) -> Option<NodeMarker> {
    let mut it = body.split(' ');
    let version = Semver::parse(it.next()?)?;
    let protocol = it.next()?.strip_prefix("proto=")?;
    let os = it.next()?.strip_prefix("os=")?;
    let arch = it.next()?.strip_prefix("arch=")?;
    if it.next().is_some() || !arch_ok(arch) || !os_ok(os) {
        return None;
    }
    if protocol.is_empty() || protocol.len() > 4 || !protocol.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(NodeMarker { version, protocol: protocol.parse().ok()?, os: os.into(), arch: arch.into() })
}

/// Mach-O `cputype` for an arch.
pub fn macho_cputype(arch: &str) -> Option<u32> {
    match arch {
        "x86_64" => Some(0x0100_0007),
        "aarch64" => Some(0x0100_000c),
        _ => None,
    }
}

/// A thin 64-bit little-endian Mach-O executable for `arch`.
pub fn macho_matches(bytes: &[u8], arch: &str) -> bool {
    let Some(want) = macho_cputype(arch) else { return false };
    bytes.len() >= 32
        && bytes[0..4] == [0xcf, 0xfa, 0xed, 0xfe]
        && u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) == want
        && u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) == 2 // MH_EXECUTE
}

pub fn binary_matches(bytes: &[u8], os: &str, arch: &str) -> bool {
    match os {
        "linux" => elf_matches(bytes, arch),
        "macos" => macho_matches(bytes, arch),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedNodeBuild {
    pub version: Semver,
    pub protocol: u32,
    pub os: String,
    pub arch: String,
    pub key_id: String,
    pub key_role: &'static str,
}

/// The one check (§8.2 step 3): a minisign signature (incl. the global
/// signature over the trusted comment) by one of `keys`; the comment
/// names this os/arch's node asset; the right binary format; exactly one
/// marker equal to the comment. Static refusal strings only.
pub fn verify_node_build(bytes: &[u8], sig_text: &str, keys: &[TrustedKey], os: &str, arch: &str) -> Result<VerifiedNodeBuild, &'static str> {
    if !arch_ok(arch) || !os_ok(os) {
        return Err("unsupported os/arch");
    }
    let sig = minisign_verify::Signature::decode(sig_text).map_err(|_| "signature unreadable")?;
    let mut verified: Option<&TrustedKey> = None;
    for k in keys {
        let Ok(pk) = minisign_verify::PublicKey::from_base64(k.public_key_b64) else { continue };
        if pk.verify(bytes, &sig, false).is_ok() {
            verified = Some(k);
            break;
        }
    }
    let key = verified.ok_or("signature not valid for any trusted key")?;
    let comment = parse_trusted_comment(sig.trusted_comment()).ok_or("trusted comment malformed")?;
    if comment.asset != asset_name(os, arch) {
        return Err("trusted comment names another file");
    }
    if !binary_matches(bytes, os, arch) {
        return Err("not a binary for this os/arch");
    }
    let markers = find_markers(bytes)?;
    if markers.len() != 1 {
        return Err("build marker missing or repeated");
    }
    let m = &markers[0];
    if m.version != comment.version || m.protocol != comment.protocol || m.os != os || m.arch != arch {
        return Err("build marker does not match the trusted comment");
    }
    Ok(VerifiedNodeBuild {
        version: comment.version,
        protocol: comment.protocol,
        os: os.into(),
        arch: arch.into(),
        key_id: key_id_of(key.public_key_b64).unwrap_or_else(|| key.key_id.to_string()),
        key_role: key.role,
    })
}

/// A fake Mach-O header for tests.
pub fn fake_macho(arch: &str, marker: &str) -> Vec<u8> {
    let mut h = vec![0u8; 32];
    h[0..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    h[4..8].copy_from_slice(&macho_cputype(arch).unwrap_or(0).to_le_bytes());
    h[12..16].copy_from_slice(&2u32.to_le_bytes());
    h.extend_from_slice(b"\0pad\0");
    h.extend_from_slice(marker.as_bytes());
    h.extend_from_slice(b"\0tail\0");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_round_trip_and_doctored_fail() {
        let m = format_marker("0.45.5", 2, "linux", "x86_64");
        assert!(m.starts_with("K2_NODE_BUILD v=0.45.5 proto=2 os=linux arch=x86_64;"));
        let found = find_markers(m.as_bytes()).unwrap();
        assert_eq!(found, vec![NodeMarker { version: Semver(0, 45, 5), protocol: 2, os: "linux".into(), arch: "x86_64".into() }]);
        for bad in [
            format!("{}0.45.5 proto=2 os=windows arch=x86_64;", marker_prefix()),
            format!("{}0.45.5 proto=2 arch=x86_64;", marker_prefix()),
            format!("{}0.45.5 proto=2 os=linux arch=x86_64", marker_prefix()),
        ] {
            assert!(find_markers(bad.as_bytes()).is_err(), "{bad}");
        }
        // The root-helper marker is not a node marker (and vice versa).
        let helper = crate::version::format_marker("0.45.5", 4, "x86_64");
        assert!(find_markers(helper.as_bytes()).unwrap().is_empty());
        assert!(crate::version::find_markers(m.as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn macho_header_check() {
        let b = fake_macho("aarch64", "x");
        assert!(macho_matches(&b, "aarch64"));
        assert!(!macho_matches(&b, "x86_64"));
        assert!(!binary_matches(&b, "linux", "aarch64"));
        let mut fat = b.clone();
        fat[0..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        assert!(!macho_matches(&fat, "aarch64"), "universal binaries are refused");
    }

    #[cfg(feature = "test-signer")]
    #[test]
    fn signed_node_builds_verify_and_each_tamper_fails() {
        use crate::keys::TEST_KEY;
        use crate::testsign::{fake_elf, sign, sign_with_test_key};
        let keys = [TEST_KEY];
        let elf = fake_elf("x86_64", &format_marker("0.45.5", 2, "linux", "x86_64"));
        let tc = trusted_comment("linux", "x86_64", "0.45.5", 2);
        let sig = sign_with_test_key(&elf, &tc);
        let v = verify_node_build(&elf, &sig, &keys, "linux", "x86_64").unwrap();
        assert_eq!((v.version, v.protocol), (Semver(0, 45, 5), 2));
        // Release keys only: refused.
        assert!(verify_node_build(&elf, &sig, crate::keys::RELEASE_KEYS, "linux", "x86_64").is_err());
        // Flipped byte.
        let mut bad = elf.clone();
        bad[70] ^= 1;
        assert!(verify_node_build(&bad, &sig, &keys, "linux", "x86_64").is_err());
        // A root-helper comment (domain separation by file name).
        let s = sign_with_test_key(&elf, &crate::sig::trusted_comment("x86_64", "0.45.5", 2));
        assert_eq!(verify_node_build(&elf, &s, &keys, "linux", "x86_64"), Err("trusted comment names another file"));
        // And the helper verifier refuses a node comment.
        let helper_elf = fake_elf("x86_64", &crate::version::format_marker("0.45.5", 4, "x86_64"));
        let s = sign_with_test_key(&helper_elf, &tc);
        assert_eq!(crate::sig::verify_signed_build(&helper_elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
        // Wrong os, marker mismatch, untrusted key.
        assert!(verify_node_build(&elf, &sig, &keys, "macos", "x86_64").is_err());
        let s = sign_with_test_key(&elf, &trusted_comment("linux", "x86_64", "0.45.6", 2));
        assert_eq!(verify_node_build(&elf, &s, &keys, "linux", "x86_64"), Err("build marker does not match the trusted comment"));
        let other = sign(&[9u8; 32], [1, 2, 3, 4, 5, 6, 7, 8], &elf, &tc);
        assert!(verify_node_build(&elf, &other, &keys, "linux", "x86_64").is_err());
        // macOS asset.
        let mach = fake_macho("aarch64", &format_marker("0.45.5", 2, "macos", "aarch64"));
        let s = sign_with_test_key(&mach, &trusted_comment("macos", "aarch64", "0.45.5", 2));
        assert!(verify_node_build(&mach, &s, &keys, "macos", "aarch64").is_ok());
    }
}
