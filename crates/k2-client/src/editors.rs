use serde::{Deserialize, Serialize};
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

#[cfg(not(windows))]
fn mac_app_exists(app_name: &str) -> bool {
    crate::bg_proc::command("open")
        .args(["-Ra", app_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn cli_exists(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    crate::bg_proc::command("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── Windows: find an editor's CLI without starting anything ────────────
//
// macOS asks LaunchServices (`open -Ra`) and Linux `which`; neither exists
// on Windows (`which` only with Git Bash on PATH), so nothing was found
// there. Windows looks the CLI up the way cmd.exe would: each PATH folder,
// then the folders the usual installers put the CLI in, trying .exe, .cmd
// and .bat (`code` is `code.cmd`).

/// Where per-user (%LOCALAPPDATA%\Programs) and machine-wide (Program
/// Files) installs keep the editor CLIs, when the installer didn't add
/// them to PATH (or K2 started before it did).
#[cfg(windows)]
const WINDOWS_CLI_DIRS: &[&str] = &[
    r"Microsoft VS Code\bin",
    r"Microsoft VS Code Insiders\bin",
    r"cursor\resources\app\bin",
    r"Windsurf\bin",
    r"Zed\bin",
    r"Sublime Text",
];

/// `cmd` (no extension) in `dirs`, as `<dir>\<cmd><ext>` for the first
/// extension that exists, in order.
#[cfg_attr(not(windows), allow(dead_code))]
fn find_cli_in(cmd: &str, dirs: &[std::path::PathBuf], exts: &[&str]) -> Option<std::path::PathBuf> {
    if cmd.is_empty() || cmd.contains(['/', '\\']) {
        return None;
    }
    dirs.iter().find_map(|d| exts.iter().map(|x| d.join(format!("{cmd}{x}"))).find(|p| p.is_file()))
}

#[cfg(windows)]
fn windows_cli_path(cmd: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for root in ["LOCALAPPDATA", "ProgramFiles"] {
        if let Some(r) = std::env::var_os(root) {
            let base = if root == "LOCALAPPDATA" { PathBuf::from(r).join("Programs") } else { PathBuf::from(r) };
            dirs.extend(WINDOWS_CLI_DIRS.iter().map(|d| base.join(d)));
        }
    }
    find_cli_in(cmd, &dirs, &[".exe", ".cmd", ".bat"])
}

fn is_installed(def: &EditorDefinition) -> bool {
    #[cfg(windows)]
    {
        // Mac-only apps (Xcode) and the Mac terminal apps never match:
        // their CLI names don't exist on Windows, or they have none.
        def.type_ == "editor" && windows_cli_path(def.cli_command).is_some()
    }
    #[cfg(not(windows))]
    {
        mac_app_exists(def.mac_app) || cli_exists(def.cli_command)
    }
}

fn detect_all() -> Vec<EditorInfo> {
    EDITOR_DEFINITIONS
        .iter()
        .map(|def| {
            let installed = is_installed(def);
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

/// Windows Terminal's `wt.exe` (an app execution alias under
/// %LOCALAPPDATA%\Microsoft\WindowsApps, usually also on PATH).
#[cfg(windows)]
fn windows_wt_path() -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(std::path::PathBuf::from(l).join(r"Microsoft\WindowsApps"));
    }
    // The alias is a zero-byte reparse point: `is_file` follows it, so
    // look for the entry itself.
    dirs.into_iter().map(|d| d.join("wt.exe")).find(|p| std::fs::symlink_metadata(p).is_ok())
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

    // Windows: the CLI it was found by (a `.cmd` shim runs under a hidden
    // cmd.exe; the editor's own window opens as usual).
    #[cfg(windows)]
    {
        let cli = windows_cli_path(&editor.cli_command)
            .ok_or_else(|| format!("No available launch method for: {}", editor.label))?;
        crate::bg_proc::command(cli)
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open editor: {}", e))?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        // Prefer macOS `open -a` for GUI apps
        if mac_app_exists(&editor.mac_app) {
            crate::bg_proc::command("open")
                .args(["-a", &editor.mac_app, path])
                .spawn()
                .map_err(|e| format!("Failed to open editor: {}", e))?;
            return Ok(());
        }

        // Fallback to CLI command
        if !editor.cli_command.is_empty() && cli_exists(&editor.cli_command) {
            crate::bg_proc::command(&editor.cli_command)
                .arg(path)
                .spawn()
                .map_err(|e| format!("Failed to open editor: {}", e))?;
            return Ok(());
        }

        Err(format!("No available launch method for: {}", editor.label))
    }
}

pub fn open_in_terminal(terminal_app: &str, path: &str) -> Result<(), String> {
    // Windows: Windows Terminal in that folder when it's installed, else a
    // Command Prompt window there. Both are terminals the person asked
    // for, so they get a window (CREATE_NEW_CONSOLE for cmd.exe).
    #[cfg(windows)]
    {
        let _ = terminal_app;
        if let Some(wt) = windows_wt_path() {
            crate::bg_proc::command(wt)
                .args(["-d", path])
                .spawn()
                .map_err(|e| format!("Failed to open Windows Terminal: {e}"))?;
            return Ok(());
        }
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        crate::bg_proc::command("cmd.exe")
            .current_dir(path)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| format!("Failed to open Command Prompt: {e}"))?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        // "Terminal" is the macOS default — use `open` with the path
        if terminal_app == "Terminal" {
            crate::bg_proc::command("open")
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
                crate::bg_proc::command("open")
                    .args(["-a", &term.mac_app, path])
                    .spawn()
                    .map_err(|e| format!("Failed to open {}: {}", term.label, e))?;
                return Ok(());
            }
        }

        // Fallback: try `open -a <name> <path>` directly
        crate::bg_proc::command("open")
            .args(["-a", terminal_app, path])
            .spawn()
            .map_err(|e| format!("Failed to open {}: {}", terminal_app, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' lookup rule, on any OS: PATH order, then extension order;
    /// a name with a separator is never a bare CLI.
    #[test]
    fn find_cli_in_takes_the_first_folder_and_extension_that_exist() {
        let root = std::env::temp_dir().join(format!("k2-editors-find-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("code.cmd"), "@echo off\n").unwrap();
        std::fs::write(b.join("code.exe"), "").unwrap();
        std::fs::write(a.join("cursor.cmd"), "").unwrap();
        let dirs = vec![a.clone(), b.clone()];
        let exts = [".exe", ".cmd", ".bat"];
        assert_eq!(find_cli_in("code", &dirs, &exts), Some(b.join("code.exe")));
        assert_eq!(find_cli_in("cursor", &dirs, &exts), Some(a.join("cursor.cmd")));
        assert_eq!(find_cli_in("zed", &dirs, &exts), None);
        assert_eq!(find_cli_in("", &dirs, &exts), None);
        assert_eq!(find_cli_in("b/code", &[root.clone()], &exts), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
