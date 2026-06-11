use serde::Serialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/Alakazam-211/K2SO/releases/latest";

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

#[tauri::command]
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    // Run the blocking HTTP call on a background thread to avoid freezing the UI
    tokio::task::spawn_blocking(|| {
        let client = reqwest::blocking::Client::builder()
            .user_agent("K2SO-UpdateChecker")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e: reqwest::Error| e.to_string())?;

        let body = client
            .get(GITHUB_RELEASES_URL)
            .send()
            .map_err(|e: reqwest::Error| e.to_string())?
            .text()
            .map_err(|e: reqwest::Error| e.to_string())?;

        let resp: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;

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
