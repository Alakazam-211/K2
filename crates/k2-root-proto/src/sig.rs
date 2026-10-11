//! Signed helper builds: the trusted comment, the self-update payload,
//! and the one check both sides run (§9.2 rules 1–4).
//!
//! The helper is signed with plain `minisign -S -t` (never the Tauri
//! signer, which writes only `timestamp` + `file` and uses the app-updater
//! key; RH7). The trusted comment is exactly
//! `file:k2-root-helper-linux-<arch> version:<semver> protocol:<n>` and the
//! build's embedded marker must say the same.

use crate::keys::{key_id_of, TrustedKey};
use crate::version::{arch_ok, elf_matches, find_markers, Semver};

/// Release asset name for `arch`.
pub fn asset_name(arch: &str) -> String {
    format!("k2-root-helper-linux-{arch}")
}

/// The exact trusted comment a release helper is signed with.
pub fn trusted_comment(arch: &str, version: &str, protocol: u32) -> String {
    format!(
        "file:{} version:{version} protocol:{protocol}",
        asset_name(arch)
    )
}

/// A parsed trusted comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub asset: String,
    pub version: Semver,
    pub protocol: u32,
}

/// Strict: three space-separated fields in this order, nothing else.
pub fn parse_trusted_comment(s: &str) -> Option<Comment> {
    let mut it = s.split(' ');
    let asset = it.next()?.strip_prefix("file:")?;
    let version = Semver::parse(it.next()?.strip_prefix("version:")?)?;
    let protocol = it.next()?.strip_prefix("protocol:")?;
    if it.next().is_some()
        || protocol.is_empty()
        || protocol.len() > 4
        || !protocol.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if !asset.starts_with("k2-") || asset.len() > 64 {
        return None;
    }
    Some(Comment {
        asset: asset.to_string(),
        version,
        protocol: protocol.parse().ok()?,
    })
}

/// What a verified build is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBuild {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
    /// The key that verified it (minisign key id).
    pub key_id: String,
    pub key_role: &'static str,
}

/// §9.2 rules 1–4: signature (incl. the global signature over the trusted
/// comment) by one of `keys`; the comment names this arch's helper asset;
/// an ELF for this arch; exactly one marker, equal to the comment.
/// Static refusal strings only — no caller bytes are echoed.
pub fn verify_signed_build(
    elf: &[u8],
    sig_text: &str,
    keys: &[TrustedKey],
    arch: &str,
) -> Result<VerifiedBuild, &'static str> {
    if !arch_ok(arch) {
        return Err("unsupported arch");
    }
    let sig = minisign_verify::Signature::decode(sig_text).map_err(|_| "signature unreadable")?;
    let mut verified: Option<&TrustedKey> = None;
    for k in keys {
        let Ok(pk) = minisign_verify::PublicKey::from_base64(k.public_key_b64) else {
            continue;
        };
        // allow_legacy = false: prehashed (ED) signatures only.
        if pk.verify(elf, &sig, false).is_ok() {
            verified = Some(k);
            break;
        }
    }
    let key = verified.ok_or("signature not valid for any trusted key")?;
    let comment =
        parse_trusted_comment(sig.trusted_comment()).ok_or("trusted comment malformed")?;
    if comment.asset != asset_name(arch) {
        return Err("trusted comment names another file");
    }
    if !elf_matches(elf, arch) {
        return Err("not an ELF for this arch");
    }
    let markers = find_markers(elf)?;
    if markers.len() != 1 {
        return Err("build marker missing or repeated");
    }
    let m = &markers[0];
    if m.version != comment.version || m.protocol != comment.protocol || m.arch != arch {
        return Err("build marker does not match the trusted comment");
    }
    Ok(VerifiedBuild {
        version: comment.version,
        protocol: comment.protocol,
        arch: arch.to_string(),
        key_id: key_id_of(key.public_key_b64).unwrap_or_else(|| key.key_id.to_string()),
        key_role: key.role,
    })
}

/// Largest signature text a payload may carry.
pub const MAX_SIG_TEXT: usize = 4096;

/// `helper self-update` stdin: `<u32 BE sig len><minisign sig text><ELF>`.
pub fn encode_payload(sig_text: &str, elf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + sig_text.len() + elf.len());
    out.extend_from_slice(&(sig_text.len() as u32).to_be_bytes());
    out.extend_from_slice(sig_text.as_bytes());
    out.extend_from_slice(elf);
    out
}

/// Split a self-update payload. Never echoes its bytes.
pub fn decode_payload(bytes: &[u8]) -> Result<(&str, &[u8]), &'static str> {
    if bytes.len() < 4 {
        return Err("payload too short");
    }
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if n == 0 || n > MAX_SIG_TEXT || bytes.len() < 4 + n {
        return Err("payload signature length invalid");
    }
    let sig = std::str::from_utf8(&bytes[4..4 + n]).map_err(|_| "payload signature not utf-8")?;
    let elf = &bytes[4 + n..];
    if elf.is_empty() {
        return Err("payload has no binary");
    }
    Ok((sig, elf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_format_round_trips_and_is_strict() {
        let c = trusted_comment("x86_64", "0.45.5", 4);
        assert_eq!(
            c,
            "file:k2-root-helper-linux-x86_64 version:0.45.5 protocol:4"
        );
        assert_eq!(
            parse_trusted_comment(&c),
            Some(Comment {
                asset: "k2-root-helper-linux-x86_64".into(),
                version: Semver(0, 45, 5),
                protocol: 4
            })
        );
        for bad in [
            "timestamp:1791420765\tfile:k2-mail-helper-linux-x86_64",
            "file:k2-root-helper-linux-x86_64 version:0.45.5",
            "file:k2-root-helper-linux-x86_64 protocol:4 version:0.45.5",
            "file:k2-root-helper-linux-x86_64 version:0.45.5 protocol:4 extra:1",
            "file:k2-root-helper-linux-x86_64  version:0.45.5 protocol:4",
            "file:k2-root-helper-linux-x86_64 version:v0.45.5 protocol:4",
            "file:k2-root-helper-linux-x86_64 version:0.45.5 protocol:x",
            "file:/etc/passwd version:0.45.5 protocol:4",
        ] {
            assert!(parse_trusted_comment(bad).is_none(), "{bad}");
        }
    }

    /// The real v0.45.0 mail-helper `.sig` (Tauri signer, app-updater key):
    /// it parses as minisign, and its trusted comment has no version or
    /// protocol — which is why the root helper is signed separately (RH7).
    #[test]
    fn tauri_signer_comment_lacks_version_and_protocol() {
        let real = include_str!("../tests/fixtures/k2-mail-helper-linux-x86_64-v0.45.0.sig");
        let sig = minisign_verify::Signature::decode(real).expect("real v0.45.0 sig decodes");
        assert!(sig
            .trusted_comment()
            .contains("file:k2-mail-helper-linux-x86_64"));
        assert!(parse_trusted_comment(sig.trusted_comment()).is_none());
    }

    #[test]
    fn payload_codec() {
        let p = encode_payload("sig", b"\x7fELFxx");
        assert_eq!(decode_payload(&p).unwrap(), ("sig", &b"\x7fELFxx"[..]));
        assert!(decode_payload(b"").is_err());
        assert!(decode_payload(&encode_payload("", b"x")).is_err());
        assert!(decode_payload(&encode_payload("sig", b"")).is_err());
        let mut long = 5000u32.to_be_bytes().to_vec();
        long.extend(vec![b'a'; 6000]);
        assert!(decode_payload(&long).is_err());
        let mut lie = 10u32.to_be_bytes().to_vec();
        lie.extend(b"abc");
        assert!(decode_payload(&lie).is_err());
    }
}
