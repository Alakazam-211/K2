//! Ensure git has `user.name` / `user.email` before operations that need
//! an author (empty initial commits, etc.).
//!
//! Policy (Rosson 2026-08-11):
//! - If both values are already set (global or system config git resolves),
//!   **do nothing** — leave the user's existing setup alone.
//! - If missing, set **only the missing fields** via `git config --global`.
//! - Defaults come from the OS account (username / display name) and a
//!   synthetic local email — fine for local commits; users can change
//!   globals later for GitHub etc.

use std::process::Command;

/// Ensure `user.name` and `user.email` resolve for subsequent git commits.
/// Idempotent and skip-safe when already configured.
pub fn ensure_git_identity() -> Result<(), String> {
    let name = git_config_get("user.name");
    let email = git_config_get("user.email");

    if name.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        && email.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
    {
        return Ok(());
    }

    if name.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        let n = detect_user_display_name();
        git_config_set_global("user.name", &n)?;
    }
    if email.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        let e = detect_user_email();
        git_config_set_global("user.email", &e)?;
    }
    Ok(())
}

fn git_config_get(key: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn git_config_set_global(key: &str, value: &str) -> Result<(), String> {
    let out = Command::new("git")
        .args(["config", "--global", key, value])
        .output()
        .map_err(|e| format!("git config --global {key}: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git config --global {key} failed: {}",
            stderr.trim()
        ));
    }
    Ok(())
}

fn detect_user_display_name() -> String {
    // Prefer full display name where cheap; fall back to login.
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = Command::new("id").arg("-F").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        // USERNAME is always present on interactive Windows sessions.
        if let Ok(u) = std::env::var("USERNAME") {
            let t = u.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    if let Ok(u) = std::env::var("USER") {
        let t = u.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(u) = std::env::var("USERNAME") {
        let t = u.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    "K2 User".to_string()
}

fn detect_user_email() -> String {
    // Synthetic local-only address. Not sent to remotes unless the user
    // pushes without changing config; clearly machine-local.
    let user = detect_user_display_name()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '.'
            }
        })
        .collect::<String>();
    let user = user.trim_matches('.').to_string();
    let user = if user.is_empty() {
        "user".to_string()
    } else {
        user
    };
    let host = hostname_label();
    format!("{user}@{host}.local")
}

fn hostname_label() -> String {
    if let Ok(h) = std::env::var("COMPUTERNAME") {
        let t = h.trim().to_lowercase();
        if !t.is_empty() {
            return sanitize_label(&t);
        }
    }
    if let Ok(h) = std::env::var("HOSTNAME") {
        let t = h.trim().to_lowercase();
        if !t.is_empty() {
            return sanitize_label(&t);
        }
    }
    if let Ok(out) = Command::new("hostname").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
            if !s.is_empty() {
                return sanitize_label(&s);
            }
        }
    }
    "localhost".to_string()
}

fn sanitize_label(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c| c == '-' || c == '.').to_string();
    if cleaned.is_empty() {
        "localhost".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_label_strips_junk() {
        assert_eq!(sanitize_label("my-pc"), "my-pc");
        assert_eq!(sanitize_label("z3flow"), "z3flow");
        assert_eq!(sanitize_label("!!!"), "localhost");
        assert_eq!(sanitize_label("foo bar"), "foo-bar");
    }

    #[test]
    fn detect_name_nonempty() {
        assert!(!detect_user_display_name().is_empty());
    }

    #[test]
    fn detect_email_has_at() {
        let e = detect_user_email();
        assert!(e.contains('@'), "email={e}");
        assert!(e.ends_with(".local"), "email={e}");
    }
}
