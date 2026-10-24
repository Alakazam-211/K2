//! The matching App engine, staged ahead of the boot that needs it
//! (prd-app-runtime-update-continuity-v1 R1, R2, R3, A1, A17, A26).
//!
//! - **Manifest (A1/R2).** `daemon-latest.json` lists each engine build in
//!   `artifacts` under its own key, [`manifest_key`] =
//!   `k2-app-engine-linux-<arch>`, `Artifact`-shaped `{url, sig, sha256}`;
//!   never inside or beside a `"linux-<arch>"` key (older `install-daemon.sh`
//!   copies sed-parse the last url/sha256 after that key). Written by the CI
//!   merge job (`scripts/merge-daemon-manifest.py`).
//! - **Stage (R1).** The updater (`POST /cli/daemon/update/start`) fetches
//!   the target version's engine when one is installed here, checks its
//!   sha256, and stores it in [`staged_dir`]`(<version>)` under the asset
//!   names `engine_install` reads. The signature is published up to ~60 min
//!   after the release (A17): a missing one is retried a few times, then the
//!   binary is staged without it ("signature not published yet") and the
//!   boot heal fetches it later. With a signature the daemon's advisory
//!   check runs against the **target** version first; a refused build is
//!   never kept. The installer (`install-daemon.sh`) stages the same way.
//! - **Safety (A26).** The staged folder is writable by the daemon's user
//!   (and agents share it). That is safe only because the heal hands it to
//!   `engine_install::install_core`, which reads the bytes once, checks
//!   version and signature, and sends **those bytes** to the helper; no path
//!   ever reaches the helper.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_root_proto::engine::asset_name;
use sha2::{Digest, Sha256};

/// The engine's key in `daemon-latest.json` `artifacts` (A1).
pub fn manifest_key(arch: &str) -> String {
    asset_name(arch)
}

/// `~/.k2/update/engine/<version>/`.
pub fn staged_dir(version: &str) -> PathBuf {
    k2_core::paths::k2_home().join("update").join("engine").join(version)
}

/// What a staged folder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Present {
    pub bin: bool,
    pub sig: bool,
}

pub fn present(dir: &Path, arch: &str) -> Present {
    let name = asset_name(arch);
    Present { bin: dir.join(&name).is_file(), sig: dir.join(format!("{name}.sig")).is_file() }
}

/// A failed fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 404: e.g. the signature isn't published yet (A17).
    NotFound,
    Other(String),
}

/// How one stage ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staged {
    /// The binary (and the signature when `sig`) are in place.
    Yes { sig: bool },
    /// Not staged, and why (the daemon update goes on regardless).
    No(String),
}

impl Staged {
    /// The update job's `engine` field.
    pub fn json(&self) -> serde_json::Value {
        match self {
            Staged::Yes { sig: true } => serde_json::json!({ "staged": true, "sig": true }),
            Staged::Yes { sig: false } => serde_json::json!({
                "staged": true, "sig": false,
                "note": "engine staged without its signature (signature not published yet); the boot heal fetches it"
            }),
            Staged::No(why) => serde_json::json!({ "staged": false, "note": why }),
        }
    }
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Fetch the signature, retrying a 404 `tries` times with `pause` between.
pub fn fetch_sig(url: &str, tries: u32, pause: Duration, fetch: &dyn Fn(&str) -> Result<Vec<u8>, FetchError>, sleep: &dyn Fn(Duration)) -> Result<String, FetchError> {
    let mut last = FetchError::NotFound;
    for i in 0..tries.max(1) {
        match fetch(url) {
            Ok(b) => return String::from_utf8(b).map_err(|_| FetchError::Other("the signature isn't text".into())),
            Err(e) => last = e,
        }
        if i + 1 < tries {
            sleep(pause);
        }
    }
    Err(last)
}

/// R1: stage `target_version`'s engine from its manifest entry into `dir`.
#[allow(clippy::too_many_arguments)]
pub fn stage(
    dir: &Path,
    arch: &str,
    target_version: &str,
    url: &str,
    sig_url: &str,
    sha256: &str,
    keys: &[k2_root_proto::keys::TrustedKey],
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, FetchError>,
    sleep: &dyn Fn(Duration),
) -> Staged {
    let bin = match fetch(url) {
        Ok(b) => b,
        Err(FetchError::NotFound) => return Staged::No(format!("engine not staged: {url} is not published")),
        Err(FetchError::Other(e)) => return Staged::No(format!("engine not staged: {e}")),
    };
    if !sha256.eq_ignore_ascii_case(&sha256_hex(&bin)) {
        return Staged::No("engine not staged: sha256 mismatch".into());
    }
    let sig = match fetch_sig(sig_url, 3, Duration::from_secs(20), fetch, sleep) {
        Ok(s) => Some(s),
        Err(FetchError::NotFound) => None,
        Err(FetchError::Other(e)) => return Staged::No(format!("engine not staged: signature: {e}")),
    };
    if let Some(sig) = &sig {
        // Advisory: the TARGET version (this daemon is still the old one).
        if let Err(e) = super::engine_install::check(&bin, sig, arch, target_version, keys) {
            return Staged::No(format!("engine not staged: {e}"));
        }
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Staged::No(format!("engine not staged: {}: {e}", dir.display()));
    }
    let name = asset_name(arch);
    let sig_path = dir.join(format!("{name}.sig"));
    // A stale signature beside new bytes would only be refused later; drop it.
    let _ = std::fs::remove_file(&sig_path);
    if let Err(e) = write_atomic(&dir.join(&name), &bin) {
        return Staged::No(format!("engine not staged: {e}"));
    }
    match sig {
        Some(s) => match write_atomic(&sig_path, s.as_bytes()) {
            Ok(()) => Staged::Yes { sig: true },
            Err(e) => Staged::No(format!("engine not staged: {e}")),
        },
        None => Staged::Yes { sig: false },
    }
}

/// The updater's stage (R1): `None` when no engine is installed here (or
/// not Linux, or the manifest has no engine for this arch: then the heal
/// downloads from the release at boot).
pub fn stage_for_update(manifest: &crate::update_routes::DaemonManifest, target_version: &str) -> Option<Staged> {
    if !super::guarded::engine_installed() {
        return None;
    }
    let arch = match std::env::consts::ARCH {
        a @ ("x86_64" | "aarch64") => a,
        _ => return None,
    };
    let Some(a) = manifest.artifacts.get(&manifest_key(arch)) else {
        return Some(Staged::No(format!("the release manifest has no {} entry; the boot heal downloads it", manifest_key(arch))));
    };
    let fetch = |u: &str| live_fetch(u);
    Some(stage(
        &staged_dir(target_version),
        arch,
        target_version,
        &a.url,
        &a.sig,
        &a.sha256,
        &k2_root_proto::keys::trusted_keys(cfg!(debug_assertions)),
        &fetch,
        &|d| std::thread::sleep(d),
    ))
}

/// A blocking GET (`NotFound` on 404).
pub fn live_fetch(url: &str) -> Result<Vec<u8>, FetchError> {
    let r = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| FetchError::Other(format!("http client: {e}")))?
        .get(url)
        .send()
        .map_err(|e| FetchError::Other(format!("{url}: {e}")))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError::NotFound);
    }
    if !r.status().is_success() {
        return Err(FetchError::Other(format!("{url}: http {}", r.status())));
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| FetchError::Other(format!("{url}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_root_proto::engine::{format_marker, trusted_comment};
    use k2_root_proto::testsign::{fake_elf, sign_with_test_key};
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-engine-stage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    struct Net {
        files: HashMap<String, Vec<u8>>,
        asked: RefCell<Vec<String>>,
    }

    impl Net {
        fn fetch(&self, u: &str) -> Result<Vec<u8>, FetchError> {
            self.asked.borrow_mut().push(u.to_string());
            self.files.get(u).cloned().ok_or(FetchError::NotFound)
        }
    }

    fn build(version: &str) -> (Vec<u8>, String) {
        let elf = fake_elf("x86_64", &format_marker(version, 3, "x86_64"));
        let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", version, 3));
        (elf, sig)
    }

    #[test]
    fn stages_bin_and_sig_against_the_target_version() {
        let (bin, sig) = build("0.46.5");
        let net = Net {
            files: HashMap::from([("u/bin".to_string(), bin.clone()), ("u/bin.sig".to_string(), sig.clone().into_bytes())]),
            asked: RefCell::new(vec![]),
        };
        let d = tmp("ok");
        let keys = [k2_root_proto::keys::TEST_KEY];
        let out = stage(&d, "x86_64", "0.46.5", "u/bin", "u/bin.sig", &sha256_hex(&bin), &keys, &|u| net.fetch(u), &|_| {});
        assert_eq!(out, Staged::Yes { sig: true });
        assert_eq!(std::fs::read(d.join("k2-app-engine-linux-x86_64")).unwrap(), bin);
        assert_eq!(std::fs::read_to_string(d.join("k2-app-engine-linux-x86_64.sig")).unwrap(), sig);
        // The target version is checked, not this daemon's.
        let refused = stage(&tmp("old"), "x86_64", "0.46.6", "u/bin", "u/bin.sig", &sha256_hex(&bin), &keys, &|u| net.fetch(u), &|_| {});
        assert!(matches!(&refused, Staged::No(w) if w.contains("must match")), "{refused:?}");
        assert!(!present(&tmp("old"), "x86_64").bin, "a refused build is never kept");
        let bad = stage(&tmp("sha"), "x86_64", "0.46.5", "u/bin", "u/bin.sig", "00", &keys, &|u| net.fetch(u), &|_| {});
        assert_eq!(bad, Staged::No("engine not staged: sha256 mismatch".into()));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// T33 (A17), the stage half: the signature isn't published yet → the
    /// binary is staged alone after the retries, and the job says why.
    #[test]
    fn a_signature_not_published_yet_stages_the_binary_alone() {
        let (bin, _) = build("0.46.5");
        let net = Net { files: HashMap::from([("u/bin".to_string(), bin.clone())]), asked: RefCell::new(vec![]) };
        let d = tmp("nosig");
        let slept = RefCell::new(0u32);
        let out = stage(&d, "x86_64", "0.46.5", "u/bin", "u/bin.sig", &sha256_hex(&bin), &[k2_root_proto::keys::TEST_KEY], &|u| net.fetch(u), &|_| {
            *slept.borrow_mut() += 1
        });
        assert_eq!(out, Staged::Yes { sig: false });
        assert_eq!(net.asked.borrow().iter().filter(|u| u.ends_with(".sig")).count(), 3, "retried");
        assert_eq!(*slept.borrow(), 2);
        assert_eq!(present(&d, "x86_64"), Present { bin: true, sig: false });
        assert!(out.json()["note"].as_str().unwrap().contains("signature not published yet"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_manifest_key_is_its_own_never_the_platform_key() {
        assert_eq!(manifest_key("x86_64"), "k2-app-engine-linux-x86_64");
        assert!(!manifest_key("aarch64").starts_with("linux-"));
    }
}
