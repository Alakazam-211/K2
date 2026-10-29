//! An App's update feed (prd-tauri-core-0467-v1 §4, TC-6 … TC-12; vs-live
//! TA-8 … TA-16; 0.46.7): the pure rules and the k2.db records.
//!
//! The shape (K2 Web, 2026-10-29): the App's owner signs each build with the
//! App's own minisign key on their own Mac. The box checks every signature
//! against the App's public key, uploads each archive **once** to the
//! download host (`dl.k2.dev`, [`dl_client`]), and answers Tauri's updater
//! on the App's own hosts with a few KB of JSON that point there. The box
//! never serves archive bytes and never holds a private key.
//!
//! - [`store`]: the records (migration 0255): keys, releases, artifacts,
//!   channels and their log, check-in counts.
//! - [`feed`]: Tauri's static `latest.json` and dynamic answer from records.
//! - [`dl_client`]: the download host's upload, manifest and stats API.
//!
//! This module holds the rules every caller shares: channel names, platform
//! keys, file types, strict SemVer, the download id and the key checks.

pub mod dl_client;
#[cfg(any(test, feature = "test-util"))]
pub mod fake_host;
pub mod feed;
pub mod store;

use std::cmp::Ordering;

use base64::Engine as _;

/// Refusal codes (snake_case; the CLI prints them, `k2 explain` lists them).
pub mod codes {
    pub const SECRET_KEY_REFUSED: &str = "secret_key_refused";
    pub const PUBKEY_INVALID: &str = "pubkey_invalid";
    pub const NO_UPDATE_KEY: &str = "no_update_key";
    pub const SIGNATURE_INVALID: &str = "signature_invalid";
    pub const SIGNATURE_MISSING: &str = "signature_missing";
    pub const ARTIFACT_TOO_LARGE: &str = "artifact_too_large";
    pub const ARTIFACT_TYPE_REFUSED: &str = "artifact_type_refused";
    pub const PLATFORM_INVALID: &str = "platform_invalid";
    pub const PLATFORM_DROPPED: &str = "platform_dropped";
    pub const VERSION_INVALID: &str = "version_invalid";
    pub const VERSION_NOT_NEWER: &str = "version_not_newer";
    pub const VERSION_EXISTS: &str = "version_exists";
    pub const CHANNEL_INVALID: &str = "channel_invalid";
    pub const CHANNEL_EMPTY: &str = "channel_empty";
    pub const RELEASE_NOT_FOUND: &str = "release_not_found";
    pub const RELEASE_NOT_LIVE: &str = "release_not_live";
    pub const UPLOAD_PENDING: &str = "upload_pending";
    pub const NEEDS_K2_FRONT: &str = "needs_k2_front";
    pub const DL_ID_INVALID: &str = "dl_id_invalid";
    pub const DL_TOKEN_MISSING: &str = "dl_token_missing";
    pub const DL_TOKEN_INVALID: &str = "dl_token_invalid";
    pub const UPDATES_HOST_UNAVAILABLE: &str = "updates_host_unavailable";
    pub const UPDATES_HOST_REFUSED: &str = "updates_host_refused";
    pub const FILE_OUTSIDE_WORKSPACE: &str = "file_outside_workspace";
    pub const KEY_CHANGE_HELD: &str = "key_change_held";
}

/// Largest archive the box accepts (TC-8). The download host has its own
/// per-App cap (200 MiB by default for an App id; K2 Web raises it per App):
/// its `413 too_large` comes back as [`codes::UPDATES_HOST_REFUSED`].
pub const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
/// Largest `--notes-file`.
pub const MAX_NOTES_BYTES: usize = 16 * 1024;
/// Most platforms in one release.
pub const MAX_PLATFORMS: usize = 16;
/// The download host (TC-30). `K2_DL_BASE` points tests (and a staging
/// host) elsewhere.
pub const DL_BASE: &str = "https://dl.k2.dev";

/// The download host's base URL now (no trailing slash).
pub fn dl_base() -> String {
    std::env::var("K2_DL_BASE").ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| DL_BASE.to_string())
}

// ── names ────────────────────────────────────────────────────────────

/// Channel names: `stable`, `beta`, or `[a-z][a-z0-9-]{0,19}` (the download
/// host wants a leading letter).
pub fn valid_channel(c: &str) -> bool {
    let b = c.as_bytes();
    !b.is_empty() && b.len() <= 20 && b[0].is_ascii_lowercase() && b.iter().all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || *x == b'-')
}

pub const OSES: &[&str] = &["darwin", "linux", "windows"];
pub const ARCHES: &[&str] = &["x86_64", "aarch64", "i686", "armv7"];
/// Tauri v2's installer suffixes for `<os>-<arch>-<installer>` keys.
pub const INSTALLERS: &[&str] = &["app", "appimage", "deb", "rpm", "msi", "nsis"];

/// A Tauri v2 platform key: `<os>-<arch>` or `<os>-<arch>-<installer>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub installer: Option<String>,
}

impl Platform {
    pub fn key(&self) -> String {
        match &self.installer {
            Some(i) => format!("{}-{}-{i}", self.os, self.arch),
            None => format!("{}-{}", self.os, self.arch),
        }
    }
}

pub fn parse_platform(raw: &str) -> Result<Platform, String> {
    let k = raw.trim();
    let mut parts = k.splitn(3, '-');
    let (os, arch) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let installer = parts.next();
    if !OSES.contains(&os) || !ARCHES.contains(&arch) || installer.is_some_and(|i| !INSTALLERS.contains(&i)) {
        return Err(format!(
            "'{k}' isn't a Tauri v2 platform key: <os>-<arch> or <os>-<arch>-<installer>, os {}, arch {}, installer {}",
            OSES.join("|"),
            ARCHES.join("|"),
            INSTALLERS.join("|")
        ));
    }
    Ok(Platform { os: os.into(), arch: arch.into(), installer: installer.map(str::to_string) })
}

/// The archive types the feed takes (TC-8), longest suffix first, with the
/// content type the download host stores.
pub const ARTIFACT_TYPES: &[(&str, &str)] = &[
    (".AppImage.tar.gz", "application/gzip"),
    (".app.tar.gz", "application/gzip"),
    (".nsis.zip", "application/zip"),
    (".msi.zip", "application/zip"),
    (".AppImage", "application/x-appimage"),
    (".deb", "application/vnd.debian.binary-package"),
    (".rpm", "application/x-rpm"),
    (".msi", "application/x-msi"),
    (".exe", "application/vnd.microsoft.portable-executable"),
];

/// `(suffix, content type)` for a file name, or `None` (not on the list).
pub fn artifact_type(file_name: &str) -> Option<(&'static str, &'static str)> {
    ARTIFACT_TYPES.iter().find(|(sfx, _)| file_name.ends_with(sfx) && file_name.len() > sfx.len()).copied()
}

/// The name the download host stores a file under
/// (`^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`): other characters become `-`, the
/// type suffix is kept. URLs are content-addressed, so the name is cosmetic.
pub fn host_file_name(file_name: &str) -> Option<String> {
    let (sfx, _) = artifact_type(file_name)?;
    let stem = &file_name[..file_name.len() - sfx.len()];
    let mut s: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' }).collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    let s = s.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).to_string();
    let s = if s.is_empty() { "app".to_string() } else { s };
    let max = 128 - sfx.len();
    let s: String = s.chars().take(max).collect();
    Some(format!("{s}{sfx}"))
}

// ── versions (strict SemVer 2.0.0) ───────────────────────────────────

/// A strict SemVer 2.0.0 version (`1.4.1`, `1.4.0-build.42`; `1.0`,
/// `v1.0.0` and leading zeros are refused, as the download host does).
pub fn parse_version(raw: &str) -> Result<semver::Version, String> {
    let t = raw.trim();
    if t.len() > 64 {
        return Err(format!("'{t}' is longer than 64 characters"));
    }
    semver::Version::parse(t).map_err(|e| {
        format!(
            "'{t}' isn't strict SemVer 2.0.0 ({e}): use MAJOR.MINOR.PATCH like 1.4.1, or a prerelease like 1.4.0-build.42 \
             (no leading v; a build number after + is ignored by the updater, so put it in the version)"
        )
    })
}

/// Order by SemVer precedence, ignoring build metadata (`+…`), as Tauri's
/// updater compares.
pub fn cmp_versions(a: &semver::Version, b: &semver::Version) -> Ordering {
    a.cmp_precedence(b)
}

/// `a` is newer than `b` (by precedence). Unparsable strings are never newer.
pub fn newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => cmp_versions(&a, &b) == Ordering::Greater,
        _ => false,
    }
}

// ── the download id (reverse DNS, Rosson 10-29) ──────────────────────

fn dns_part(p: &str) -> bool {
    let b = p.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !p.contains("--")
}

/// The App's fleet-unique download id: `dev.k2.<box>.<app>` (the box's K2
/// Connect name keeps App ids unique across boxes, TA-14).
pub fn dl_id(box_sub: &str, app_id: &str) -> Result<String, String> {
    let (b, a) = (box_sub.trim().to_ascii_lowercase(), app_id.trim().to_ascii_lowercase());
    if b.is_empty() {
        return Err("this box has no K2 Connect name yet, so its Apps have no download id (k2 connect login first)".into());
    }
    if !dns_part(&b) {
        return Err(format!("the box name '{b}' can't be part of a download id ([a-z0-9-], no leading, trailing or double -)"));
    }
    if !dns_part(&a) || a == "ci" {
        return Err(format!(
            "the App id '{a}' can't be part of a download id (dev.k2.<box>.<app>: [a-z0-9-], no leading, trailing or double -, not 'ci')"
        ));
    }
    let id = format!("dev.k2.{b}.{a}");
    if id.len() > 200 {
        return Err("the download id is longer than 200 characters".into());
    }
    Ok(id)
}

/// A `k2dl_` App token: `k2dl_` + 32 lowercase hex.
pub fn valid_dl_token(t: &str) -> bool {
    t.strip_prefix("k2dl_").is_some_and(|h| h.len() == 32 && h.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
}

/// A short, non-reversible fingerprint of a token (for status and audit).
pub fn token_fingerprint(t: &str) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha256::digest(t.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

// ── keys and signatures ──────────────────────────────────────────────

/// The App's updater public key, as `tauri.conf.json` `plugins.updater
/// .pubkey` holds it (base64 of the two-line `.pub` file), and its key id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubKey {
    pub wrapped: String,
    /// minisign's key id, upper-case hex (as `minisign` prints it).
    pub key_id: String,
}

/// Parse a public key given as Tauri's wrapped form, the two-line `.pub`
/// text, or the bare key line. A secret key (any form) is refused with
/// [`codes::SECRET_KEY_REFUSED`]; anything else unreadable is
/// [`codes::PUBKEY_INVALID`].
pub fn parse_pubkey(raw: &str) -> Result<PubKey, (&'static str, String)> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let t = raw.trim();
    if t.is_empty() {
        return Err((codes::PUBKEY_INVALID, "the key is empty".into()));
    }
    let unwrapped = b64.decode(t).ok().and_then(|d| String::from_utf8(d).ok()).filter(|s| s.contains("untrusted comment"));
    let (text, given_wrapped) = match unwrapped {
        Some(s) => (s, true),
        None => (t.to_string(), false),
    };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let comment = lines.iter().find(|l| l.starts_with("untrusted comment")).copied().unwrap_or("");
    let key_line = lines.iter().rev().find(|l| !l.starts_with("untrusted comment")).copied().unwrap_or("");
    let secret = || {
        (
            codes::SECRET_KEY_REFUSED,
            "that is a SECRET key: K2 keeps only the App's public key. Keep the private key on your signing Mac (or in your vault \
             for an unattended job on your own node) and give K2 the .pub file's content (tauri.conf.json plugins.updater.pubkey)"
                .to_string(),
        )
    };
    if comment.to_ascii_lowercase().contains("secret key") {
        return Err(secret());
    }
    let bytes = b64.decode(key_line).map_err(|_| (codes::PUBKEY_INVALID, "the key line isn't base64".to_string()))?;
    if bytes.len() > 42 && bytes.starts_with(b"Ed") {
        return Err(secret());
    }
    if bytes.len() != 42 || !bytes.starts_with(b"Ed") {
        return Err((codes::PUBKEY_INVALID, "not a minisign public key (Ed25519, 42 bytes)".into()));
    }
    minisign_verify::PublicKey::from_base64(key_line).map_err(|e| (codes::PUBKEY_INVALID, format!("not a minisign public key: {e}")))?;
    let key_id: String = bytes[2..10].iter().rev().map(|b| format!("{b:02X}")).collect();
    let wrapped = if given_wrapped {
        t.to_string()
    } else {
        let c = if comment.is_empty() { format!("untrusted comment: minisign public key: {key_id}") } else { comment.to_string() };
        b64.encode(format!("{c}\n{key_line}\n"))
    };
    Ok(PubKey { wrapped, key_id })
}

/// The `.sig` text Tauri's signer writes (base64 of the two-line minisig) or
/// the raw minisig: both decode to the raw text. Returns `(raw, base64)`:
/// the feed carries the base64 form (what Tauri's updater expects).
pub fn normalize_signature(sig: &str) -> Result<(String, String), String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let t = sig.trim();
    let raw = if t.starts_with("untrusted comment") {
        t.to_string()
    } else {
        let d = b64.decode(t).map_err(|_| "the signature is neither minisig text nor its base64".to_string())?;
        String::from_utf8(d).map_err(|_| "the signature isn't text".to_string())?
    };
    if !raw.trim_start().starts_with("untrusted comment") {
        return Err("the signature isn't a minisign signature".into());
    }
    minisign_verify::Signature::decode(raw.trim()).map_err(|e| format!("the signature doesn't decode: {e}"))?;
    let raw = format!("{}\n", raw.trim_end());
    let enc = b64.encode(raw.as_bytes());
    Ok((raw, enc))
}

/// A streaming signature check plus the sha256 and size of the same bytes.
pub struct Checked {
    pub sha256: String,
    pub size: u64,
}

/// Read `r` once: verify the minisign `sig` (raw text) against `pubkey`
/// and hash the bytes. `Err` is a one-line reason (wrong key, bad
/// signature, read error).
pub fn verify_and_hash(pubkey: &PubKey, sig_raw: &str, mut r: impl std::io::Read, max: u64) -> Result<Checked, (&'static str, String)> {
    use sha2::Digest as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let pub_text = b64.decode(pubkey.wrapped.trim()).ok().and_then(|d| String::from_utf8(d).ok()).unwrap_or_default();
    let pk = minisign_verify::PublicKey::decode(pub_text.trim()).map_err(|e| (codes::PUBKEY_INVALID, format!("the App's key doesn't decode: {e}")))?;
    let sig = minisign_verify::Signature::decode(sig_raw.trim()).map_err(|e| (codes::SIGNATURE_INVALID, format!("the signature doesn't decode: {e}")))?;
    let mut v = pk.verify_stream(&sig).map_err(|e| match e {
        minisign_verify::Error::UnexpectedKeyId => {
            (codes::SIGNATURE_INVALID, format!("signed with another key, not the App's updater key ({})", pubkey.key_id))
        }
        e => (codes::SIGNATURE_INVALID, format!("unsupported signature: {e}")),
    })?;
    let mut h = sha2::Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = r.read(&mut buf).map_err(|e| ("read_failed", format!("reading the file: {e}")))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > max {
            return Err((codes::ARTIFACT_TOO_LARGE, format!("larger than {} MiB", max / (1024 * 1024))));
        }
        v.update(&buf[..n]);
        h.update(&buf[..n]);
    }
    v.finalize().map_err(|_| (codes::SIGNATURE_INVALID, "the signature doesn't match the file (wrong file, or the file changed after signing)".to_string()))?;
    Ok(Checked { sha256: h.finalize().iter().map(|b| format!("{b:02x}")).collect(), size })
}

/// `YYYY-MM-DD` (UTC) for a unix time.
pub fn day_of(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).unwrap_or_default().format("%Y-%m-%d").to_string()
}

/// RFC 3339, whole seconds, UTC (`2026-10-29T12:00:00Z`): the download host
/// normalises `pub_date` to this, so K2 writes it so too.
pub fn rfc3339(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
