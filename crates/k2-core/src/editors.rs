use serde::{Deserialize, Serialize};
use std::process::Command;
use parking_lot::Mutex;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorInfo {
    pub id: String,
    pub label: String,
    pub mac_app: String,
    pub cli_command: String,
    pub installed: bool,
    #[serde(rename = "type")]
    pub type_: String, // "editor" or "terminal"
}

struct EditorDefinition {
    id: &'static str,
    label: &'static str,
    mac_app: &'static str,
    cli_command: &'static str,
    type_: &'static str,
}

const EDITOR_DEFINITIONS: &[EditorDefinition] = &[
    EditorDefinition { id: "cursor", label: "Cursor", mac_app: "Cursor", cli_command: "cursor", type_: "editor" },
    EditorDefinition { id: "vscode", label: "VS Code", mac_app: "Visual Studio Code", cli_command: "code", type_: "editor" },
    EditorDefinition { id: "vscode-insiders", label: "VS Code Insiders", mac_app: "Visual Studio Code - Insiders", cli_command: "code-insiders", type_: "editor" },
    EditorDefinition { id: "windsurf", label: "Windsurf", mac_app: "Windsurf", cli_command: "windsurf", type_: "editor" },
    EditorDefinition { id: "zed", label: "Zed", mac_app: "Zed", cli_command: "zed", type_: "editor" },
    EditorDefinition { id: "sublime", label: "Sublime Text", mac_app: "Sublime Text", cli_command: "subl", type_: "editor" },
    EditorDefinition { id: "xcode", label: "Xcode", mac_app: "Xcode", cli_command: "xcode", type_: "editor" },
    EditorDefinition { id: "fleet", label: "Fleet", mac_app: "Fleet", cli_command: "fleet", type_: "editor" },
    EditorDefinition { id: "webstorm", label: "WebStorm", mac_app: "WebStorm", cli_command: "webstorm", type_: "editor" },
    EditorDefinition { id: "intellij", label: "IntelliJ IDEA", mac_app: "IntelliJ IDEA", cli_command: "idea", type_: "editor" },
    EditorDefinition { id: "pycharm", label: "PyCharm", mac_app: "PyCharm", cli_command: "pycharm", type_: "editor" },
    EditorDefinition { id: "goland", label: "GoLand", mac_app: "GoLand", cli_command: "goland", type_: "editor" },
    EditorDefinition { id: "rustrover", label: "RustRover", mac_app: "RustRover", cli_command: "rustrover", type_: "editor" },
    EditorDefinition { id: "android-studio", label: "Android Studio", mac_app: "Android Studio", cli_command: "studio", type_: "editor" },
    EditorDefinition { id: "iterm", label: "iTerm", mac_app: "iTerm", cli_command: "", type_: "terminal" },
    EditorDefinition { id: "warp", label: "Warp", mac_app: "Warp", cli_command: "", type_: "terminal" },
    EditorDefinition { id: "ghostty", label: "Ghostty", mac_app: "Ghostty", cli_command: "", type_: "terminal" },
];

// ── Detection cache ──────────────────────────────────────────────────────

static EDITOR_CACHE: Mutex<Option<Vec<EditorInfo>>> = Mutex::new(None);

fn mac_app_exists(app_name: &str) -> bool {
    Command::new("open")
        .args(["-Ra", app_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cli_exists(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn detect_all() -> Vec<EditorInfo> {
    EDITOR_DEFINITIONS
        .iter()
        .map(|def| {
            let installed = mac_app_exists(def.mac_app) || cli_exists(def.cli_command);
            EditorInfo {
                id: def.id.to_string(),
                label: def.label.to_string(),
                mac_app: def.mac_app.to_string(),
                cli_command: def.cli_command.to_string(),
                type_: def.type_.to_string(),
                installed,
            }
        })
        .collect()
}

// ── Public API ───────────────────────────────────────────────────────────

pub fn get_all_editors() -> Vec<EditorInfo> {
    // parking_lot::Mutex never poisons, so `.lock()` always returns a
    // guard directly — no error branch needed.
    let mut cache = EDITOR_CACHE.lock();
    if cache.is_none() {
        *cache = Some(detect_all());
    }
    cache.as_ref().cloned().unwrap_or_default()
}

pub fn get_installed_editors() -> Vec<EditorInfo> {
    get_all_editors()
        .into_iter()
        .filter(|e| e.installed && e.type_ == "editor")
        .collect()
}

pub fn clear_editor_cache() -> Vec<EditorInfo> {
    *EDITOR_CACHE.lock() = None;
    get_all_editors()
}

pub fn open_in_editor(editor_id: &str, path: &str) -> Result<(), String> {
    let all = get_all_editors();
    let editor = all
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| format!("Unknown editor: {}", editor_id))?;

    if !editor.installed {
        return Err(format!("Editor not installed: {}", editor.label));
    }

    // Prefer macOS `open -a` for GUI apps
    if mac_app_exists(&editor.mac_app) {
        Command::new("open")
            .args(["-a", &editor.mac_app, path])
            .spawn()
            .map_err(|e| format!("Failed to open editor: {}", e))?;
        return Ok(());
    }

    // Fallback to CLI command
    if !editor.cli_command.is_empty() && cli_exists(&editor.cli_command) {
        Command::new(&editor.cli_command)
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open editor: {}", e))?;
        return Ok(());
    }

    Err(format!("No available launch method for: {}", editor.label))
}

pub fn open_in_terminal(terminal_app: &str, path: &str) -> Result<(), String> {
    // "Terminal" is the macOS default — use `open` with the path
    if terminal_app == "Terminal" {
        Command::new("open")
            .args(["-a", "Terminal", path])
            .spawn()
            .map_err(|e| format!("Failed to open Terminal: {}", e))?;
        return Ok(());
    }

    // Look up in our known terminal apps
    let all = get_all_editors();
    let terminal = all
        .iter()
        .find(|e| e.type_ == "terminal" && e.label == terminal_app);

    if let Some(term) = terminal {
        if mac_app_exists(&term.mac_app) {
            Command::new("open")
                .args(["-a", &term.mac_app, path])
                .spawn()
                .map_err(|e| format!("Failed to open {}: {}", term.label, e))?;
            return Ok(());
        }
    }

    // Fallback: try `open -a <name> <path>` directly
    Command::new("open")
        .args(["-a", terminal_app, path])
        .spawn()
        .map_err(|e| format!("Failed to open {}: {}", terminal_app, e))?;
    Ok(())
}
