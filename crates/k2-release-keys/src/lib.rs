//! The update-signing keys every K2 updater verifier trusts, and the one
//! verify they all share (prd-security-sweep-0464-v1 SS4, option C).
//!
//! **Keys.** U2 is the primary (secret on z3mbpZ only), U3 the offline
//! backup (Rosson's password manager, never on a box). Both are in
//! [`UPDATER_KEYS`]. U1 (`E9111446F65EBB05`) is the retired key: z3mbpZ
//! still signs every release with it during the bridge, in the old fields
//! (`signature` in `latest.json`, `sig` in `daemon-latest.json`), so
//! installs older than 0.46.6 keep updating. **No verifier in this tree
//! trusts U1**: it appears only as [`LEGACY_UPDATER_KEY_B64`], which the
//! signing scripts and the copies test read.
//!
//! **Where the copies live** (the `copies` test fails when they drift):
//! - here, [`UPDATER_KEYS`] (the daemon's self-update and the app's
//!   `app_update_install` link this crate);
//! - `cli/k2` `_DAEMON_UPDATE_PUBKEYS` (`k2 daemon install`);
//! - `scripts/install-daemon.sh` `K2_DAEMON_PUBKEYS`;
//! - `src-tauri/tauri.conf.json` `plugins.updater.pubkey` (the primary, in
//!   Tauri's wrapped form; only the plugin's own download path reads it,
//!   and K2 doesn't use that path).
//!
//! `scripts/updater-key-ceremony.sh pubkeys <U2> <U3>` writes all four. Until
//! the ceremony lands, the list is empty: every verifier refuses every
//! update (fail closed), `tauri.conf.json` still holds U1, and
//! `release.sh` refuses to start.
//!
//! **Signature format (U2/U3).** Plain `minisign -S`, prehashed (`ED`),
//! trusted comment `timestamp:<unix>\tfile:<asset>\tversion:<x.y.z>`.
//! [`verify_updater_sig`] requires exactly one `file:` equal to the asset
//! and exactly one `version:` equal to the announced version, so an old
//! signed build can't be served as a new one, and one asset can't be
//! swapped for another.

use base64::Engine as _;

/// One trusted minisign public key (the same shape as
/// `k2_root_proto::keys::TrustedKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    /// `primary` | `backup` | `test`.
    pub role: &'static str,
    /// minisign key id as minisign prints it (16 upper hex).
    pub key_id: &'static str,
    /// The base64 line of a `minisign.pub` file ("Ed" + key id + key).
    pub public_key_b64: &'static str,
}

// BEGIN UPDATER_KEYS (written by scripts/updater-key-ceremony.sh pubkeys)
/// The update keys: U2 (primary) then U3 (backup). Empty until the SS5 key
/// ceremony lands; an empty list refuses every update.
pub const UPDATER_KEYS: &[TrustedKey] = &[];
// END UPDATER_KEYS

/// U1, the retired update key `E9111446F65EBB05` (the minisign key line).
/// Signing scripts use it to make bridge signatures for installs older than
/// 0.46.6; no verifier may use it. Removed when the bridge ends (SS6).
pub const LEGACY_UPDATER_KEY_B64: &str = "RWQFu172RhQR6QBqzmij2E9btDGhDWmpdH+ah1/M4PmuHPINUWvKLf6o";
/// U1's key id.
pub const LEGACY_UPDATER_KEY_ID: &str = "E9111446F65EBB05";

/// The first release signed with U2. Older releases carry no `sig2` /
/// `k2_signatures`, so a 0.46.6+ verifier can't install them.
pub const FIRST_U2_SIGNED_VERSION: &str = "0.46.6";

/// What a successful verify proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub key_id: String,
    pub role: &'static str,
    pub trusted_comment: String,
}

/// minisign prints the key id as the little-endian u64 of the 8 raw bytes.
pub fn key_id_of(public_key_b64: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(public_key_b64.trim()).ok()?;
    if raw.len() != 42 || &raw[0..2] != b"Ed" {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[2..10]);
    Some(format!("{:016X}", u64::from_le_bytes(id)))
}

/// Tauri's wrapped public-key form (`plugins.updater.pubkey`): base64 of
/// the two-line `minisign.pub` file.
pub fn tauri_pubkey(public_key_b64: &str) -> Option<String> {
    let id = key_id_of(public_key_b64)?;
    let file = format!("untrusted comment: minisign public key: {id}\n{}\n", public_key_b64.trim());
    Some(base64::engine::general_purpose::STANDARD.encode(file))
}

/// The minisig text, whether given raw or base64-wrapped (Tauri's form).
pub fn sig_text(sig: &str) -> String {
    let t = sig.trim();
    if t.starts_with("untrusted comment:") {
        return t.to_string();
    }
    let compact: String = t.split_whitespace().collect();
    match base64::engine::general_purpose::STANDARD.decode(compact) {
        Ok(b) => String::from_utf8(b).map(|s| s.trim().to_string()).unwrap_or_else(|_| t.to_string()),
        Err(_) => t.to_string(),
    }
}

/// The values of every `name:` field in a tab-separated trusted comment.
fn comment_fields<'a>(tc: &'a str, name: &str) -> Vec<&'a str> {
    let prefix = format!("{name}:");
    tc.split('\t').filter_map(|f| f.strip_prefix(prefix.as_str())).collect()
}

/// Verify an update artifact against [`UPDATER_KEYS`].
pub fn verify_updater_sig(data: &[u8], sig: &str, expect_file: &str, expect_version: &str) -> Result<Verified, String> {
    verify_updater_sig_with(UPDATER_KEYS, data, sig, expect_file, expect_version)
}

/// [`verify_updater_sig`] with an explicit key list (tests use throwaway
/// keys).
pub fn verify_updater_sig_with(
    keys: &[TrustedKey],
    data: &[u8],
    sig: &str,
    expect_file: &str,
    expect_version: &str,
) -> Result<Verified, String> {
    if keys.is_empty() {
        return Err("this build trusts no update key yet (the update-key ceremony has not landed); refusing".into());
    }
    if expect_file.is_empty() || expect_version.is_empty() {
        return Err("internal: no expected file or version to check the signature against".into());
    }
    let text = sig_text(sig);
    let signature = minisign_verify::Signature::decode(&text).map_err(|e| format!("signature decode failed: {e}"))?;
    let mut last = String::from("signed by a key this build doesn't trust");
    for k in keys {
        let pk = minisign_verify::PublicKey::from_base64(k.public_key_b64)
            .map_err(|e| format!("trusted key {} is malformed: {e}", k.key_id))?;
        match pk.verify(data, &signature, false) {
            Ok(()) => {}
            Err(minisign_verify::Error::UnexpectedKeyId) => continue,
            Err(e) => {
                last = format!("signature check failed with key {}: {e}", k.key_id);
                continue;
            }
        }
        // Only now is the trusted comment trustworthy (the global
        // signature over it was just checked).
        let tc = signature.trusted_comment();
        let files = comment_fields(tc, "file");
        if files != [expect_file] {
            return Err(format!("the signature names file {files:?}, not {expect_file}"));
        }
        let versions = comment_fields(tc, "version");
        if versions != [expect_version] {
            return Err(format!("the signature is for version {versions:?}, not {expect_version}"));
        }
        return Ok(Verified { key_id: k.key_id.to_string(), role: k.role, trusted_comment: tc.to_string() });
    }
    Err(last)
}

/// The release asset each `latest.json` platform installs (what release.sh
/// and windows-patch-latest-json.sh sign); `None` = not a K2 app update.
pub fn app_update_asset(platform: &str, version: &str) -> Option<String> {
    match platform {
        "darwin-aarch64" => Some("K2.app.tar.gz".to_string()),
        "windows-x86_64" => Some(format!("K2_{version}_x64-setup.exe")),
        _ => None,
    }
}

/// The last path segment of a URL (the asset name a signature must name).
pub fn url_asset_name(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The desktop app update (`latest.json`): find the platform entry whose
/// `url` is `download_url`, take its `k2_signatures` (`{"<key id>": "<base64
/// of the minisig text>"}`), and verify `data` with the first trusted key
/// that has one. The signature must name the URL's asset and `version`.
pub fn verify_app_update(
    raw_json: &serde_json::Value,
    download_url: &str,
    version: &str,
    data: &[u8],
) -> Result<Verified, String> {
    verify_app_update_with(UPDATER_KEYS, raw_json, download_url, version, data)
}

/// [`verify_app_update`] with an explicit key list.
pub fn verify_app_update_with(
    keys: &[TrustedKey],
    raw_json: &serde_json::Value,
    download_url: &str,
    version: &str,
    data: &[u8],
) -> Result<Verified, String> {
    let platforms = raw_json
        .get("platforms")
        .and_then(|p| p.as_object())
        .ok_or("latest.json has no platforms")?;
    let entries: Vec<(&String, &serde_json::Value)> = platforms
        .iter()
        .filter(|(_, e)| e.get("url").and_then(|u| u.as_str()) == Some(download_url))
        .collect();
    let (platform, entry) = match entries.as_slice() {
        [one] => *one,
        [] => return Err(format!("latest.json lists no platform with url {download_url}")),
        _ => return Err(format!("latest.json lists {download_url} under more than one platform")),
    };
    // The file a platform's update must be, pinned here, not taken from the
    // (unsigned) manifest URL: the signature has to name exactly this.
    let asset = app_update_asset(platform, version)
        .ok_or_else(|| format!("K2 publishes no app update for platform {platform}"))?;
    if url_asset_name(download_url) != Some(asset.as_str()) {
        return Err(format!("the {platform} update URL is not {asset}: {download_url}"));
    }
    let sigs = entry.get("k2_signatures").and_then(|s| s.as_object()).ok_or_else(|| {
        format!(
            "this update has no K2 signature (k2_signatures): releases before {FIRST_U2_SIGNED_VERSION} are signed with the retired key only, and this version can't install them"
        )
    })?;
    for k in keys {
        let Some(s) = sigs.get(k.key_id).and_then(|v| v.as_str()) else { continue };
        let text = base64::engine::general_purpose::STANDARD
            .decode(s.split_whitespace().collect::<String>())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| format!("k2_signatures[{}] is not base64 text", k.key_id))?;
        return verify_updater_sig_with(&[*k], data, &text, &asset, version);
    }
    if keys.is_empty() {
        return Err("this build trusts no update key yet (the update-key ceremony has not landed); refusing".into());
    }
    Err(format!(
        "k2_signatures has no signature from a key this build trusts (has {:?}, trusts {:?})",
        sigs.keys().collect::<Vec<_>>(),
        keys.iter().map(|k| k.key_id).collect::<Vec<_>>()
    ))
}

/// `a` is an older semver than `b` (`x.y.z`, numeric parts; anything
/// unparsable counts as older, so it is refused).
pub fn version_older_than(a: &str, b: &str) -> bool {
    let p = |s: &str| -> Option<Vec<u64>> {
        let v: Vec<u64> = s.trim().trim_start_matches('v').split('.').map(|x| x.parse().ok()).collect::<Option<_>>()?;
        (v.len() == 3).then_some(v)
    };
    match (p(a), p(b)) {
        (Some(x), Some(y)) => x < y,
        _ => true,
    }
}

#[cfg(test)]
mod tests;
