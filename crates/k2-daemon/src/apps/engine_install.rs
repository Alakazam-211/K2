//! `POST /cli/app/engine/install {from?}` (S11): the owner installs or
//! updates `k2-app-engine` through the root helper (`apps install-engine`,
//! protocol 6).
//!
//! - The build is this daemon's version only (engine and daemon move in
//!   lockstep), from the release (`k2-app-engine-linux-<arch>` + `.sig`)
//!   or, air-gapped, from a folder the owner names (`from`).
//! - The daemon checks the signature first (advisory; the helper checks it
//!   again as root with its own keys) so a bad download never costs a root
//!   call.
//! - Then it waits for the engine to answer on its socket.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_core::connect_users::Role;
use k2_root_proto::engine::{asset_name, verify_engine_build};
use k2_root_proto::sig::encode_payload;
use k2_root_proto::verbs::CAP_ENGINE;
use k2_root_proto::version::{Semver, APPS_MIN_PROTOCOL};
use serde_json::{json, Value as J};

use super::routes::{need_credential, no, R};
use crate::compute_routes::Who;
use crate::root_helper;

fn arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

/// Keys the daemon's advisory check trusts: the release keys; debug
/// builds (the Linux integration run) also the TEST key, which a
/// test-keys helper then checks for real.
fn keys() -> Vec<k2_root_proto::keys::TrustedKey> {
    k2_root_proto::keys::trusted_keys(cfg!(debug_assertions))
}

/// The signed build must be this daemon's version, for this arch.
pub fn check(bin: &[u8], sig: &str, arch: &str, daemon_version: &str, keys: &[k2_root_proto::keys::TrustedKey]) -> Result<J, String> {
    if bin.len() > CAP_ENGINE {
        return Err("the engine binary is larger than the install cap".into());
    }
    let v = verify_engine_build(bin, sig, keys, arch).map_err(|e| format!("k2-app-engine refused before install: {e}"))?;
    let want = Semver::parse(daemon_version).ok_or("this daemon's version is not x.y.z (dev build)")?;
    if v.version != want {
        return Err(format!("the signed engine is v{} and this daemon is v{want}; they must match", v.version));
    }
    Ok(json!({ "version": v.version.to_string(), "protocol": v.protocol, "keyId": v.key_id }))
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let r = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("http client: {e}"))?
        .get(url)
        .send()
        .map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: http {}", r.status()));
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| format!("{url}: {e}"))
}

/// The binary and its signature: from `from` (a folder holding
/// `k2-app-engine-linux-<arch>` and `.sig`) or the release.
fn obtain(from: Option<&str>, arch: &str) -> Result<(Vec<u8>, String), String> {
    let name = asset_name(arch);
    if let Some(dir) = from {
        let d = Path::new(dir);
        if !d.is_absolute() {
            return Err("give --from as an absolute folder".into());
        }
        let bin = std::fs::read(d.join(&name)).map_err(|e| format!("{}: {e}", d.join(&name).display()))?;
        let sig = std::fs::read_to_string(d.join(format!("{name}.sig"))).map_err(|e| format!("{name}.sig: {e}"))?;
        return Ok((bin, sig));
    }
    if k2_core::airgap::enabled() {
        return Err(format!("air-gapped: copy {name} and {name}.sig from the release into a folder and run k2 app engine install --from <folder>"));
    }
    let url = format!("{}/v{}/{name}", root_helper::RELEASE_DOWNLOAD_BASE, env!("CARGO_PKG_VERSION"));
    let bin = fetch(&url)?;
    let sig = String::from_utf8(fetch(&format!("{url}.sig"))?).map_err(|_| "the signature isn't text".to_string())?;
    Ok((bin, sig))
}

/// The route.
pub(crate) fn install_route(who: &Who, body: &J) -> R<J> {
    need_credential(who)?;
    if !matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. }) {
        return Err(no("owner_only", 403, "installing the App engine is the owner's: ask your owner to run it (Settings → Apps → This server → Install App engine, or k2 app engine install from a terminal outside K2)"));
    }
    let from = body.get("from").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty());
    if !cfg!(target_os = "linux") {
        return Err(no("linux_only", 409, "the App engine runs on Linux boxes only"));
    }
    // 0.46.3 F9: never inside another root sequence.
    let _held = root_helper::busy::try_hold("an App engine install").map_err(|e| no("root_busy", 409, e))?;
    install_core(from)
}

/// The install itself (the owner's route, and the automatic box prep the
/// owner's `appsAutoPrepare` allows): this daemon's signed build, checked,
/// installed through the root helper, then waited on.
pub(crate) fn install_core(from: Option<&str>) -> R<J> {
    if !cfg!(target_os = "linux") {
        return Err(no("linux_only", 409, "the App engine runs on Linux boxes only"));
    }
    let st = root_helper::state();
    if !st.usable(APPS_MIN_PROTOCOL) {
        return Err(no("helper_needed", 409, root_helper::helper_needed("the App engine", APPS_MIN_PROTOCOL, &st)));
    }
    let arch = arch().ok_or_else(|| no("unsupported", 409, "no k2-app-engine build for this CPU"))?;
    let (bin, sig) = obtain(from, arch).map_err(|e| no("engine_download_failed", 409, e))?;
    let checked = check(&bin, &sig, arch, env!("CARGO_PKG_VERSION"), &keys()).map_err(|e| no("engine_refused", 409, e))?;
    let out = root_helper::call_text(&["apps".into(), "install-engine".into()], Some(&encode_payload(&sig, &bin)))
        .map_err(|e| no("engine_install_failed", 409, e))?;
    root_helper::invalidate();
    let installed: J = serde_json::from_str(&out).unwrap_or(J::String(out));
    // The engine restarts under k2app's manager; wait for its answer.
    let deadline = Instant::now() + Duration::from_secs(30);
    let answer = loop {
        match super::guarded::engine_version() {
            Ok(v) => break Ok(v),
            Err(e) if Instant::now() >= deadline => break Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    match answer {
        Ok(v) => Ok(json!({ "ok": true, "checked": checked, "installed": installed, "engine": v })),
        Err(e) => {
            let mut r = no("engine_not_answering", 409, format!("installed, but the engine isn't answering: {e}"));
            r.extra = Some(json!({ "installed": installed }));
            Err(r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_root_proto::engine::{format_marker, trusted_comment};
    use k2_root_proto::testsign::{fake_elf, sign_with_test_key};

    #[test]
    fn only_this_versions_signed_build_passes() {
        let keys = [k2_root_proto::keys::TEST_KEY];
        let elf = fake_elf("x86_64", &format_marker("0.46.1", 1, "x86_64"));
        let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.1", 1));
        let v = check(&elf, &sig, "x86_64", "0.46.1", &keys).unwrap();
        assert_eq!(v["protocol"], 1);
        let e = check(&elf, &sig, "x86_64", "0.46.2", &keys).unwrap_err();
        assert!(e.contains("must match"), "{e}");
        assert!(check(&elf, &sig, "aarch64", "0.46.1", &keys).is_err());
        assert!(check(&elf, &sig, "x86_64", "0.46.1", k2_root_proto::keys::RELEASE_KEYS).is_err(), "test-signed builds never pass release keys");
        let node = sign_with_test_key(&elf, &k2_root_proto::node_build::trusted_comment("linux", "x86_64", "0.46.1", 1));
        assert!(check(&elf, &node, "x86_64", "0.46.1", &keys).unwrap_err().contains("another file"));
    }

    #[test]
    fn from_must_be_an_absolute_folder_with_both_files() {
        assert!(obtain(Some("relative/dir"), "x86_64").unwrap_err().contains("absolute"));
        let d = std::env::temp_dir().join(format!("k2-engine-from-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(obtain(Some(d.to_str().unwrap()), "x86_64").is_err());
        std::fs::write(d.join("k2-app-engine-linux-x86_64"), b"x").unwrap();
        std::fs::write(d.join("k2-app-engine-linux-x86_64.sig"), "sig").unwrap();
        let (b, s) = obtain(Some(d.to_str().unwrap()), "x86_64").unwrap();
        assert_eq!((b.as_slice(), s.as_str()), (&b"x"[..], "sig"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
