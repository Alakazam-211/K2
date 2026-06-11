use std::path::Path;
use std::process::Command;

/// Detect the formatter command for a file extension.
fn formatter_for_ext(ext: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match ext {
        // JavaScript / TypeScript / CSS / HTML / JSON / Markdown
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
        | "css" | "scss" | "less" | "html" | "htm" | "json" | "jsonc"
        | "md" | "mdx" | "yaml" | "yml" | "vue" | "svelte" | "astro"
        | "graphql" | "gql" => Some(("npx", vec!["prettier", "--write"])),

        // Rust
        "rs" => Some(("rustfmt", vec![])),

        // Python
        "py" | "pyw" | "pyi" => Some(("black", vec![])),

        // Go
        "go" => Some(("gofmt", vec!["-w"])),

        // Swift
        "swift" => Some(("swift-format", vec!["format", "--in-place"])),

        // C / C++
        "c" | "cpp" | "h" | "hpp" | "cc" | "cxx" => Some(("clang-format", vec!["-i"])),

        _ => None,
    }
}

/// Check if a formatter is available on PATH.
fn is_available(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tauri::command]
pub fn format_file(file_path: String) -> Result<String, String> {
    let path = Path::new(&file_path);

    // Validate file exists and is a regular file
    if !path.is_file() {
        return Err(format!("Not a file: {}", file_path));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let (cmd, base_args) = formatter_for_ext(ext)
        .ok_or_else(|| format!("No formatter configured for .{} files", ext))?;

    // For npx we don't need to check PATH (it bootstraps itself)
    if cmd != "npx" && !is_available(cmd) {
        return Err(format!("Formatter '{}' not found on PATH", cmd));
    }

    let parent = path.parent().ok_or("Cannot determine parent directory")?;

    let mut args = base_args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    args.push(file_path.clone());

    let mut child = Command::new(cmd)
        .args(&args)
        .current_dir(parent)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run {}: {}", cmd, e))?;

    // Wait with 30-second timeout (npx may need to download)
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(format!("Formatted with {}", cmd));
                } else {
                    let mut stderr = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        use std::io::Read;
                        let _ = err.read_to_string(&mut stderr);
                    }
                    return Err(format!("{} failed: {}", cmd, stderr.trim()));
                }
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err(format!("{} timed out after 30 seconds", cmd));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Failed to wait for {}: {}", cmd, e)),
        }
    }
}

#[tauri::command]
pub fn format_file_check(file_path: String) -> Result<serde_json::Value, String> {
    let path = Path::new(&file_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match formatter_for_ext(ext) {
        Some((cmd, _)) => {
            let available = cmd == "npx" || is_available(cmd);
            Ok(serde_json::json!({
                "formatter": cmd,
                "available": available,
                "extension": ext,
            }))
        }
        None => Ok(serde_json::json!({
            "formatter": null,
            "available": false,
            "extension": ext,
        })),
    }
}
