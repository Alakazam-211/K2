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
/// install, else `other`. `APPIMAGE` wins so an AppImage launched on Arch
/// is not offered the pacman package.
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
    "other"
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

fn detect_install_kind() -> &'static str {
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
}

fn non_arch_probe(kind: &str) -> InstallUpdateProbe {
    InstallUpdateProbe {
        kind: kind.to_string(),
        version: None,
        has_update: false,
        download_url: None,
    }
}

/// Kind plus a manifest body. Non-Arch kinds ignore the body (platforms
/// included). Arch reads `version` only and compares with [`is_newer`].
fn decide_install_update(
    kind: &str,
    current: &str,
    manifest_body: &str,
) -> Result<InstallUpdateProbe, String> {
    if kind != "arch" {
        return Ok(non_arch_probe(kind));
    }
    let version = manifest_version(manifest_body)?;
    if is_newer(current, &version) {
        let download_url = arch_pacman_download_url(&version);
        Ok(InstallUpdateProbe {
            kind: "arch".to_string(),
            version: Some(version),
            has_update: true,
            download_url: Some(download_url),
        })
    } else {
        Ok(InstallUpdateProbe {
            kind: "arch".to_string(),
            version: None,
            has_update: false,
            download_url: None,
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
    if k2_core::airgap::enabled() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    // Run the blocking HTTP call on a background thread to avoid freezing the UI
    tokio::task::spawn_blocking(|| {
        if k2_core::airgap::enabled() {
            return Err(k2_core::airgap::TEACHING.to_string());
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
/// follow the `latest.json` redirect. Arch skips plugin-updater `check()`
/// (that manifest has no `linux-x86_64`). Only `version` is read.
#[tauri::command]
pub async fn probe_install_update() -> Result<InstallUpdateProbe, String> {
    let kind = detect_install_kind();
    if kind != "arch" {
        return Ok(non_arch_probe(kind));
    }
    let body = tokio::task::spawn_blocking(fetch_latest_manifest)
        .await
        .map_err(|e| e.to_string())??;
    decide_install_update(kind, CURRENT_VERSION, &body)
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
        assert_eq!(classify_install_kind(false, false, text), "other");
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
