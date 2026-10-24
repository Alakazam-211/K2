use serde::Serialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
// 0.40.x: releases publish to the new Alakazam-211/K2 repo (release.sh
// RELEASE_REPO default). The old Alakazam-211/K2SO repo is archived — point
// the desktop "Check for updates" path here so every update channel (this,
// the Tauri auto-updater's latest.json, and the daemon's daemon-latest.json)
// reads from the same K2 repo.
const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/Alakazam-211/K2/releases/latest";
// Same endpoint as `plugins.updater.endpoints` in tauri.conf.json.
// GitHub 302s this to release-assets.githubusercontent.com, which the
// webview CSP does not allow, so the app process fetches it.
const LATEST_JSON_URL: &str =
    "https://github.com/Alakazam-211/K2/releases/latest/download/latest.json";

#[derive(Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub download_url: String,
    pub has_update: bool,
}

/// Compare two semver strings. Returns true if `latest` is newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let c = parse(current);
    let l = parse(latest);
    for i in 0..c.len().max(l.len()) {
        let cv = c.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if lv > cv {
            return true;
        }
        if lv < cv {
            return false;
        }
    }
    false
}

fn update_http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("K2-UpdateChecker")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e: reqwest::Error| e.to_string())
}

/// `appimage` when `APPIMAGE` is set, else `arch` for an Arch/Omarchy
/// install, else `linux-package` (a deb or rpm install). `APPIMAGE` wins so
/// an AppImage launched on Arch is not offered the pacman package.
#[cfg(any(target_os = "linux", test))]
fn classify_install_kind(
    appimage_set: bool,
    arch_release_exists: bool,
    os_release: &str,
) -> &'static str {
    if appimage_set {
        return "appimage";
    }
    if arch_release_exists || os_release_is_arch(os_release) {
        return "arch";
    }
    "linux-package"
}

#[cfg(any(target_os = "linux", test))]
fn unquote(value: &str) -> &str {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let open = bytes[0];
        let close = bytes[bytes.len() - 1];
        if (open == b'"' && close == b'"') || (open == b'\'' && close == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(any(target_os = "linux", test))]
fn os_release_is_arch(text: &str) -> bool {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key != "ID" && key != "ID_LIKE" {
            continue;
        }
        for token in unquote(raw_value).split_whitespace() {
            let token = unquote(token);
            if token == "arch" || token == "omarchy" {
                return true;
            }
        }
    }
    false
}

pub(crate) fn detect_install_kind() -> &'static str {
    // Before any Arch file: an AppImage on Arch must stay on check().
    if std::env::var_os("APPIMAGE").is_some() {
        return "appimage";
    }
    #[cfg(target_os = "linux")]
    {
        return linux_install_kind();
    }
    #[cfg(not(target_os = "linux"))]
    {
        "other"
    }
}

#[cfg(target_os = "linux")]
fn linux_install_kind() -> &'static str {
    if std::path::Path::new("/etc/arch-release").exists() {
        return "arch";
    }
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    classify_install_kind(false, false, &os_release)
}

fn manifest_version(body: &str) -> Result<String, String> {
    let resp: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    match resp.get("version").and_then(|v| v.as_str()) {
        Some(version) if !version.is_empty() => Ok(version.to_string()),
        _ => Err("latest.json has no version".to_string()),
    }
}

/// Linux installs updated by their package, not the Tauri updater:
/// `latest.json` only has darwin entries, so plugin-updater `check()`
/// fails there with "None of the fallback platforms ... linux-x86_64".
fn is_package_install(kind: &str) -> bool {
    kind == "arch" || kind == "linux-package"
}

/// deb/rpm: the release page lists every package for that version.
fn linux_release_page_url(version: &str) -> String {
    let version = version.trim().trim_start_matches('v');
    format!("https://github.com/Alakazam-211/K2/releases/tag/v{version}")
}

fn arch_pacman_download_url(version: &str) -> String {
    let version = version.trim().trim_start_matches('v');
    format!(
        "https://github.com/Alakazam-211/K2/releases/download/v{version}/k2-{version}-x86_64.pkg.tar.zst"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallUpdateProbe {
    pub kind: String,
    pub version: Option<String>,
    pub has_update: bool,
    pub download_url: Option<String>,
    /// True when this install can take the in-app download, verify and
    /// pkexec install (commands::linux_pkg_update). False keeps the old
    /// "open the download" offer.
    pub can_install: bool,
}

fn non_arch_probe(kind: &str) -> InstallUpdateProbe {
    InstallUpdateProbe {
        kind: kind.to_string(),
        version: None,
        has_update: false,
        download_url: None,
        can_install: false,
    }
}

/// Kind plus a manifest body. Non-package kinds ignore the body (platforms
/// included). Arch and deb/rpm read `version` only and compare with
/// [`is_newer`]; Arch gets the pacman package, deb/rpm the release page.
fn decide_install_update(
    kind: &str,
    current: &str,
    manifest_body: &str,
) -> Result<InstallUpdateProbe, String> {
    if !is_package_install(kind) {
        return Ok(non_arch_probe(kind));
    }
    let version = manifest_version(manifest_body)?;
    if is_newer(current, &version) {
        let download_url = if kind == "arch" {
            arch_pacman_download_url(&version)
        } else {
            linux_release_page_url(&version)
        };
        Ok(InstallUpdateProbe {
            kind: kind.to_string(),
            version: Some(version),
            has_update: true,
            download_url: Some(download_url),
            can_install: false,
        })
    } else {
        Ok(InstallUpdateProbe {
            kind: kind.to_string(),
            version: None,
            has_update: false,
            download_url: None,
            can_install: false,
        })
    }
}

fn fetch_latest_manifest() -> Result<String, String> {
    let client = update_http_client()?;
    let response = client
        .get(LATEST_JSON_URL)
        .send()
        .map_err(|e: reqwest::Error| e.to_string())?;
    let response = response
        .error_for_status()
        .map_err(|e: reqwest::Error| e.to_string())?;
    response.text().map_err(|e: reqwest::Error| e.to_string())
}

#[tauri::command]
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    if k2_client::airgap::enabled_for_client() {
        return Err(k2_client::airgap::TEACHING.to_string());
    }
    // Run the blocking HTTP call on a background thread to avoid freezing the UI
    tokio::task::spawn_blocking(|| {
        if k2_client::airgap::enabled_for_client() {
            return Err(k2_client::airgap::TEACHING.to_string());
        }
        let client = update_http_client()?;

        let body = client
            .get(GITHUB_RELEASES_URL)
            .send()
            .map_err(|e: reqwest::Error| e.to_string())?
            .text()
            .map_err(|e: reqwest::Error| e.to_string())?;

        let resp: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;

        let tag = resp["tag_name"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('v');

        let mut download_url = String::new();
        if let Some(assets) = resp["assets"].as_array() {
            for asset in assets {
                let name: &str = asset["name"].as_str().unwrap_or("");
                if name.ends_with(".dmg") {
                    download_url = asset["browser_download_url"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    break;
                }
            }
        }

        Ok(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: tag.to_string(),
            download_url,
            has_update: is_newer(CURRENT_VERSION, tag),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_current_version() -> String {
    CURRENT_VERSION.to_string()
}

/// Install kind, and on Arch the updater manifest version.
///
/// The webview cannot see `APPIMAGE` or `/etc/os-release`, and it cannot
/// follow the `latest.json` redirect. Arch and deb/rpm skip plugin-updater
/// `check()` (that manifest has no `linux-x86_64`). Only `version` is read.
#[tauri::command]
pub async fn probe_install_update() -> Result<InstallUpdateProbe, String> {
    let kind = detect_install_kind();
    if !is_package_install(kind) {
        return Ok(non_arch_probe(kind));
    }
    let body = tokio::task::spawn_blocking(fetch_latest_manifest)
        .await
        .map_err(|e| e.to_string())??;
    let mut probe = decide_install_update(kind, CURRENT_VERSION, &body)?;
    probe.can_install = probe.has_update && super::linux_pkg_update::this_install().is_ok();
    Ok(probe)
}

// ── Desktop app update, verified with the K2 update keys (SS4) ───────
//
// prd-security-sweep-0464-v1 SS4 (option C): the Tauri plugin verifies a
// download with ONE key, the `signature` field, which carries the retired
// key U1 for installs older than 0.46.4. This app instead trusts only
// `k2_release_keys::UPDATER_KEYS` (U2 + the offline backup U3) and reads
// `k2_signatures` from latest.json. The plugin still finds the update
// (`check()`, which verifies nothing) and installs the bytes (`install()`,
// which verifies nothing either); the download and the verify are ours.
// The webview no longer calls the plugin (no `updater:default`).

/// What the renderer shows for an available update.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppUpdateAvailable {
    pub version: String,
    pub notes: Option<String>,
}

/// Progress events while downloading (`app-update-progress`).
#[derive(Debug, Clone, Serialize)]
struct AppUpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

struct StagedAppUpdate {
    update: tauri_plugin_updater::Update,
    /// Set once downloaded AND verified.
    verified_bytes: Option<Vec<u8>>,
}

fn staged_app_update() -> &'static std::sync::Mutex<Option<StagedAppUpdate>> {
    static S: std::sync::OnceLock<std::sync::Mutex<Option<StagedAppUpdate>>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(None))
}

/// Refuse before a 100 MB download when the entry has no K2 signature
/// from a key this build trusts (the full check runs on the bytes).
fn app_update_has_trusted_signature(raw_json: &serde_json::Value, download_url: &str) -> Result<(), String> {
    let entry = raw_json
        .get("platforms")
        .and_then(|p| p.as_object())
        .and_then(|p| p.values().find(|e| e.get("url").and_then(|u| u.as_str()) == Some(download_url)))
        .ok_or_else(|| format!("latest.json lists no platform with url {download_url}"))?;
    let sigs = entry.get("k2_signatures").and_then(|s| s.as_object()).ok_or_else(|| {
        format!(
            "this update has no K2 signature (k2_signatures): releases before {} are signed with the retired key only; download K2 from the website instead",
            k2_release_keys::FIRST_U2_SIGNED_VERSION
        )
    })?;
    if k2_release_keys::UPDATER_KEYS.iter().any(|k| sigs.contains_key(k.key_id)) {
        Ok(())
    } else {
        Err("this update isn't signed by a key this version of K2 trusts".to_string())
    }
}

fn download_app_update(app: &tauri::AppHandle, url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    use tauri::Emitter;
    let client = reqwest::blocking::Client::builder()
        .user_agent("K2-Updater")
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download {url}: {e}"))?;
    let total = resp.content_length();
    let mut out = Vec::with_capacity(total.unwrap_or(0).min(512 * 1024 * 1024) as usize);
    let mut chunk = vec![0u8; 256 * 1024];
    let mut last_emit = 0u64;
    loop {
        let n = resp.read(&mut chunk).map_err(|e| format!("download {url}: {e}"))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
        let done = out.len() as u64;
        if done - last_emit >= 1024 * 1024 {
            last_emit = done;
            let _ = app.emit("app-update-progress", AppUpdateProgress { downloaded: done, total });
        }
    }
    let _ = app.emit("app-update-progress", AppUpdateProgress { downloaded: out.len() as u64, total });
    Ok(out)
}

/// Is there a newer K2? (The plugin's `check()`; nothing is verified yet.)
#[tauri::command]
pub async fn app_update_check(app: tauri::AppHandle) -> Result<Option<AppUpdateAvailable>, String> {
    use tauri_plugin_updater::UpdaterExt as _;
    if k2_client::airgap::enabled_for_client() {
        return Err(k2_client::airgap::TEACHING.to_string());
    }
    let found = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let mut slot = staged_app_update().lock().map_err(|_| "update state poisoned".to_string())?;
    Ok(match found {
        Some(update) => {
            let info = AppUpdateAvailable { version: update.version.clone(), notes: update.body.clone() };
            *slot = Some(StagedAppUpdate { update, verified_bytes: None });
            Some(info)
        }
        None => {
            *slot = None;
            None
        }
    })
}

/// Download the update found by [`app_update_check`] and verify it with
/// the K2 update keys (`k2_signatures`: the URL's file name and the
/// announced version must both be in the signed comment). Returns the key
/// id that signed it. Nothing unverified is ever kept.
#[tauri::command]
pub async fn app_update_download(app: tauri::AppHandle) -> Result<String, String> {
    let update = staged_app_update()
        .lock()
        .map_err(|_| "update state poisoned".to_string())?
        .as_ref()
        .map(|s| s.update.clone())
        .ok_or("no update to download (check for updates first)")?;
    let url = update.download_url.to_string();
    let raw = update.raw_json.clone();
    let version = update.version.clone();
    app_update_has_trusted_signature(&raw, &url)?;
    let app2 = app.clone();
    let url2 = url.clone();
    let bytes = tokio::task::spawn_blocking(move || download_app_update(&app2, &url2))
        .await
        .map_err(|e| e.to_string())??;
    let v = k2_release_keys::verify_app_update(&raw, &url, &version, &bytes)
        .map_err(|e| format!("K2 refused this update: {e}"))?;
    let mut slot = staged_app_update().lock().map_err(|_| "update state poisoned".to_string())?;
    match slot.as_mut() {
        Some(s) if s.update.version == version => s.verified_bytes = Some(bytes),
        _ => return Err("the update changed while downloading; check again".into()),
    }
    Ok(v.key_id)
}

/// Install the downloaded, verified update (the plugin's `install()`).
#[tauri::command]
pub async fn app_update_install() -> Result<(), String> {
    let staged = staged_app_update()
        .lock()
        .map_err(|_| "update state poisoned".to_string())?
        .take()
        .ok_or("no update to install (download it first)")?;
    let Some(bytes) = staged.verified_bytes else {
        return Err("the update isn't downloaded and verified yet".into());
    };
    staged.update.install(&bytes).map_err(|e| e.to_string())
}

/// Broadcast an event from one window to all windows (used for tab sync etc.)
#[tauri::command]
pub fn broadcast_sync(
    app: tauri::AppHandle,
    channel: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    use tauri::Emitter;
    app.emit(&channel, payload).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARWIN_ONLY: &str = r#"{
        "version": "0.41.2",
        "platforms": {
            "darwin-aarch64": {
                "url": "https://example.invalid/K2.app.tar.gz",
                "signature": "sig"
            }
        }
    }"#;

    const ZST_0412: &str =
        "https://github.com/Alakazam-211/K2/releases/download/v0.41.2/k2-0.41.2-x86_64.pkg.tar.zst";

    #[test]
    fn arch_newer_manifest_ignores_darwin_platform_and_uses_zst() {
        let probe = decide_install_update("arch", "0.41.1", DARWIN_ONLY).unwrap();
        assert_eq!(probe.kind, "arch");
        assert!(probe.has_update);
        assert_eq!(probe.version.as_deref(), Some("0.41.2"));
        assert_eq!(probe.download_url.as_deref(), Some(ZST_0412));
        assert!(!format!("{probe:?}").contains("darwin-aarch64"));
    }

    #[test]
    fn arch_equal_manifest_is_not_an_update() {
        let body = r#"{"version":"0.41.1","platforms":{"darwin-aarch64":{}}}"#;
        let probe = decide_install_update("arch", "0.41.1", body).unwrap();
        assert_eq!(
            probe,
            InstallUpdateProbe {
                kind: "arch".into(),
                version: None,
                has_update: false,
                download_url: None,
                can_install: false,
            }
        );
    }

    #[test]
    fn arch_older_manifest_is_not_an_update() {
        let body = r#"{"version":"0.41.0","platforms":{"darwin-aarch64":{}}}"#;
        let probe = decide_install_update("arch", "0.41.1", body).unwrap();
        assert!(!probe.has_update);
        assert!(probe.download_url.is_none());
    }

    #[test]
    fn version_compare_is_numeric_not_string_order() {
        assert!(is_newer("0.41.9", "0.41.10"));
        assert!(!is_newer("0.41.10", "0.41.9"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("v0.41.2", "0.41.2"));
        assert!(is_newer("v0.41.1", "0.41.2"));
        assert_eq!(arch_pacman_download_url("v0.41.2"), ZST_0412);
    }

    #[test]
    fn macos_and_appimage_ignore_manifest_platforms() {
        let broken = r#"{"platforms":{"darwin-aarch64":{}}}"#;
        for kind in ["macos", "appimage", "other"] {
            let probe = decide_install_update(kind, "0.41.1", broken).unwrap();
            assert_eq!(probe, non_arch_probe(kind), "{kind}");
        }
    }

    #[test]
    fn deb_rpm_newer_manifest_offers_the_release_page_not_a_platform_error() {
        let probe = decide_install_update("linux-package", "0.41.1", DARWIN_ONLY).unwrap();
        assert_eq!(
            probe,
            InstallUpdateProbe {
                kind: "linux-package".into(),
                version: Some("0.41.2".into()),
                has_update: true,
                download_url: Some("https://github.com/Alakazam-211/K2/releases/tag/v0.41.2".into()),
                can_install: false,
            }
        );
        let same = r#"{"version":"0.41.1","platforms":{"darwin-aarch64":{}}}"#;
        let probe = decide_install_update("linux-package", "0.41.1", same).unwrap();
        assert!(!probe.has_update);
        assert!(probe.download_url.is_none());
        assert_eq!(probe.kind, "linux-package");
    }

    #[test]
    fn rpm_os_release_is_a_linux_package() {
        let text = "NAME=\"Fedora Linux\"\nID=fedora\n";
        assert_eq!(classify_install_kind(false, false, text), "linux-package");
        assert_eq!(classify_install_kind(true, false, text), "appimage");
    }

    #[test]
    fn arch_manifest_without_version_is_not_a_missing_platform_error() {
        let body = r#"{"platforms":{"darwin-aarch64":{}}}"#;
        let err = decide_install_update("arch", "0.41.1", body).unwrap_err();
        assert_eq!(err, "latest.json has no version");
        assert!(!err.contains("fallback platforms"));
        assert!(!err.contains("linux-x86_64"));
    }

    #[test]
    fn os_release_id_arch() {
        assert!(os_release_is_arch("ID=arch\n"));
        assert_eq!(classify_install_kind(false, false, "ID=arch\n"), "arch");
    }

    #[test]
    fn os_release_id_quoted_arch() {
        assert!(os_release_is_arch("ID=\"arch\"\n"));
        assert_eq!(
            classify_install_kind(false, false, "NAME=\"Arch Linux\"\nID=\"arch\"\n"),
            "arch"
        );
    }

    #[test]
    fn os_release_omarchy_id_like_arch() {
        let text = "NAME=Omarchy\nID=omarchy\nID_LIKE=arch\n";
        assert!(os_release_is_arch(text));
        assert_eq!(classify_install_kind(false, false, text), "arch");
        let quoted = "ID=omarchy\nID_LIKE=\"arch\"\n";
        assert!(os_release_is_arch(quoted));
    }

    #[test]
    fn os_release_ubuntu_is_not_arch() {
        let text = "ID=ubuntu\nID_LIKE=debian\n";
        assert!(!os_release_is_arch(text));
        assert_eq!(classify_install_kind(false, false, text), "linux-package");
        assert!(!os_release_is_arch("ID=archlinux\n"));
    }

    #[test]
    fn appimage_wins_over_arch_os_release() {
        let text = "ID=arch\nID_LIKE=\"arch\"\n";
        assert_eq!(classify_install_kind(true, true, text), "appimage");
        assert_eq!(classify_install_kind(false, true, ""), "arch");
        assert_eq!(classify_install_kind(false, false, text), "arch");
    }
}
