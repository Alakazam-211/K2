//! Install one missing LLM CLI (claude, codex, grok, or gemini).
//!
//! [`ensure_cli`] is the production entry: it looks up the fixed command
//! table and resolves the basename on [`super::login_path::augmented_path`].
//! [`ensure_cli_with_shell`] is the inner function tests call with a local
//! script. The script is an argument, never an environment variable, so a
//! test cannot point production at an arbitrary shell.
//!
//! The child is `/bin/sh -c <script>` with stdin null and PATH set to the
//! same augmented PATH spawn uses. The wait is 180s. A timeout kills the
//! child's process group. That is not the 5s login-shell PATH capture.

use std::path::PathBuf;
use std::time::Duration;

use super::login_path;
use super::path_env;

/// How long the installer may run before its process group is killed.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// Result of a successful ensure. `installed` is false when the basename
/// was already on the spawn PATH and no process was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnsureCliResult {
    pub installed: bool,
}

/// Fixed installer for one basename. `None` for anything else — callers
/// must not invent a command.
pub fn fixed_install_command(program: &str) -> Option<&'static str> {
    match program {
        "claude" => Some("curl -fsSL https://claude.ai/install.sh | bash"),
        "codex" => {
            Some("curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh")
        }
        "grok" => Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
        "gemini" => Some(r#"npm install -g --prefix "$HOME/.local" @google/gemini-cli"#),
        _ => None,
    }
}

/// Production ensure. Unknown programs return an error and start nothing.
/// The shell command always comes from [`fixed_install_command`].
pub fn ensure_cli(program: &str) -> Result<EnsureCliResult, String> {
    let Some(shell) = fixed_install_command(program) else {
        return Err(format!("unknown program: {program}"));
    };
    ensure_cli_with_shell(program, shell, spawn_resolve_path)
}

fn spawn_resolve_path() -> String {
    login_path::augmented_path(&login_path::process_path())
}

/// Inner ensure. `shell_command` is what `/bin/sh -c` runs when the
/// basename is missing. Production passes the fixed table entry; tests
/// pass a local script. A program outside the four returns before any
/// process is started, even if a script was passed.
///
/// `resolve_path` is called before the installer and again after it
/// exits 0, so a directory the installer just created (`~/.grok`,
/// `~/.local`) is visible. Production passes [`spawn_resolve_path`],
/// which re-reads [`login_path::known_fallback_dirs`] each call.
pub fn ensure_cli_with_shell<F>(
    program: &str,
    shell_command: &str,
    mut resolve_path: F,
) -> Result<EnsureCliResult, String>
where
    F: FnMut() -> String,
{
    if fixed_install_command(program).is_none() {
        return Err(format!("unknown program: {program}"));
    }
    #[cfg(unix)]
    {
        unix_ensure(program, shell_command, &mut resolve_path)
    }
    #[cfg(not(unix))]
    {
        let _ = (shell_command, &mut resolve_path);
        Err("CLI install is unsupported on this platform".to_string())
    }
}

#[cfg(unix)]
fn unix_ensure<F>(
    program: &str,
    shell_command: &str,
    resolve_path: &mut F,
) -> Result<EnsureCliResult, String>
where
    F: FnMut() -> String,
{
    if resolve_basename(program, &resolve_path()).is_some() {
        return Ok(EnsureCliResult { installed: false });
    }
    // Child PATH is the spawn PATH, not `resolve_path`. Tests isolate
    // resolution with `resolve_path` so a host that already has `grok`
    // does not hide a not-on-PATH failure. The installer itself still
    // sees the same PATH a PTY child would.
    let child_path = spawn_resolve_path();
    run_installer(shell_command, &child_path)?;
    if resolve_basename(program, &resolve_path()).is_some() {
        Ok(EnsureCliResult { installed: true })
    } else {
        Err(format!("{program} installed but not on PATH"))
    }
}

#[cfg(unix)]
fn resolve_basename(program: &str, path: &str) -> Option<PathBuf> {
    if program.is_empty() || program.contains('/') || program.contains('\\') {
        return None;
    }
    for dir in path_env::split(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(program);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(unix)]
fn run_installer(shell_command: &str, child_path: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;

    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", shell_command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", child_path)
        .process_group(0);
    let child = cmd
        .spawn()
        .map_err(|e| format!("installer failed to start: {e}"))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    // `wait_with_output` reads both pipes, so a chatty installer cannot
    // fill one and deadlock the wait. The thread is what lets the 180s
    // deadline kill the process group instead of blocking the caller.
    thread::spawn(move || {
        let outcome = child.wait_with_output().map(|output| {
            let code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            (output.status.success(), code, stdout, stderr)
        });
        let _ = tx.send(outcome);
    });
    match rx.recv_timeout(INSTALL_TIMEOUT) {
        Ok(Ok((true, _, _, _))) => Ok(()),
        Ok(Ok((false, code, stdout, stderr))) => Err(format!(
            "installer exited {code}: {}",
            output_tail(&stderr, &stdout)
        )),
        Ok(Err(e)) => Err(format!("installer failed: {e}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process_group(pid);
            Err(format!(
                "installer timed out after {}s",
                INSTALL_TIMEOUT.as_secs()
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("installer failed: waiter dropped".to_string())
        }
    }
}

#[cfg(unix)]
fn output_tail(stderr: &str, stdout: &str) -> String {
    let mut detail = String::new();
    let err = stderr.trim();
    let out = stdout.trim();
    if !err.is_empty() {
        detail.push_str(err);
    }
    if !out.is_empty() {
        if !detail.is_empty() {
            detail.push_str(" | ");
        }
        detail.push_str(out);
    }
    if detail.is_empty() {
        return "no output".to_string();
    }
    const MAX: usize = 2000;
    if detail.len() > MAX {
        let mut end = MAX;
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
    }
    detail
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    let pgid = pid as i32;
    unsafe {
        if libc::killpg(pgid, libc::SIGKILL) != 0 {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// The ONE crate-wide test env lock (re-entrant, never poisons).
    fn lock_home() -> crate::test_env::EnvLock {
        crate::test_env::lock()
    }

    /// Points `$HOME` at `home` until dropped (holds the env lock;
    /// restores the previous HOME on drop, even on panic).
    struct HomeGuard {
        _home: crate::test_env::EnvVar,
    }

    impl HomeGuard {
        fn set(home: &std::path::Path) -> Self {
            Self { _home: crate::test_env::EnvVar::set("HOME", home) }
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = crate::test_env::unique_temp_path(&format!("cli-install-{tag}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    #[test]
    fn cli_install_fixed_commands_are_one_cli_each() {
        assert_eq!(
            fixed_install_command("claude"),
            Some("curl -fsSL https://claude.ai/install.sh | bash")
        );
        assert_eq!(
            fixed_install_command("codex"),
            Some("curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh")
        );
        let grok = fixed_install_command("grok").unwrap();
        assert_eq!(grok, "curl -fsSL https://x.ai/cli/install.sh | bash");
        assert!(!grok.contains("https://claude.ai/"));
        assert!(!grok.contains("https://chatgpt.com/codex/"));
        assert!(!grok.contains("gemini"));
        assert!(!grok.contains("@anthropic-ai/gemini-cli"));
        assert_eq!(
            fixed_install_command("gemini"),
            Some(r#"npm install -g --prefix "$HOME/.local" @google/gemini-cli"#)
        );
        assert!(!fixed_install_command("gemini")
            .unwrap()
            .contains("@anthropic-ai/gemini-cli"));
        assert!(fixed_install_command("vim").is_none());
        assert!(fixed_install_command("claude --dangerously-skip-permissions").is_none());
        assert!(fixed_install_command("/usr/bin/claude").is_none());
    }

    #[test]
    fn cli_install_unknown_program_does_not_run_script() {
        let _lock = lock_home();
        let dir = temp_dir("unknown");
        let marker = dir.join("ran");
        let script = format!("printf x > {}", shell_quote(&marker.to_string_lossy()));
        let err = ensure_cli_with_shell("vim", &script, || String::new()).unwrap_err();
        assert!(
            !marker.exists(),
            "a program outside the four must not start the script"
        );
        assert!(err.contains("unknown program"), "unexpected error: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cli_install_known_fallback_includes_grok_bin_only_when_it_exists() {
        let _lock = lock_home();
        let home = temp_dir("fallback");
        let _guard = HomeGuard::set(&home);
        let grok = home.join(".grok/bin");
        let local = home.join(".local/bin");
        let missing = login_path::known_fallback_dirs();
        assert!(
            missing.iter().all(|p| p != &grok),
            "absent ~/.grok/bin must be omitted: {missing:?}"
        );
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&grok).unwrap();
        let present = login_path::known_fallback_dirs();
        let local_at = present.iter().position(|p| p == &local).unwrap();
        let grok_at = present.iter().position(|p| p == &grok).unwrap();
        assert_eq!(grok_at, local_at + 1, "present: {present:?}");
        fs::remove_dir_all(&grok).unwrap();
        let gone = login_path::known_fallback_dirs();
        assert!(
            gone.iter().all(|p| p != &grok),
            "removed ~/.grok/bin must be omitted: {gone:?}"
        );
        drop(_guard);
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    fn warm_login_path() {
        // Cache the login PATH under the real HOME. Later tests point
        // HOME at a temp dir; the cache must not record that.
        let _ = login_path::login_shell_path();
    }

    #[cfg(unix)]
    fn path_of(dir: PathBuf) -> impl FnMut() -> String {
        move || {
            if dir.is_dir() {
                dir.to_string_lossy().into_owned()
            } else {
                String::new()
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_install_ensure_grok_fake_script_omits_other_urls() {
        let _lock = lock_home();
        warm_login_path();
        let home = temp_dir("urls");
        let _guard = HomeGuard::set(&home);
        let marker = home.join("ran");
        let script = format!(
            "printf '%s' 'only-grok' > {}",
            shell_quote(&marker.to_string_lossy())
        );
        assert!(!script.contains("https://claude.ai/"));
        assert!(!script.contains("https://chatgpt.com/codex/"));
        assert!(!script.contains("@google/gemini-cli"));
        assert!(!script.contains("@anthropic-ai/gemini-cli"));
        let grok = fixed_install_command("grok").unwrap();
        assert!(!grok.contains("https://claude.ai/"));
        assert!(!grok.contains("https://chatgpt.com/codex/"));
        assert!(!grok.contains("@google/gemini-cli"));
        let err = ensure_cli_with_shell("grok", &script, || String::new()).unwrap_err();
        assert!(
            err.contains("installed but not on PATH"),
            "fake script wrote no binary: {err}"
        );
        let ran = fs::read_to_string(&marker).unwrap_or_else(|e| panic!("script did not run: {e}"));
        assert_eq!(ran, "only-grok");
        drop(_guard);
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn cli_install_fake_script_writes_grok_then_resolves() {
        let _lock = lock_home();
        warm_login_path();
        let home = temp_dir("writes");
        let _guard = HomeGuard::set(&home);
        let bin = home.join(".grok/bin");
        let script = format!(
            "/bin/mkdir -p {} && printf '%s\\n' '#!/bin/sh' 'exit 0' > {} && /bin/chmod 755 {}",
            shell_quote(&bin.to_string_lossy()),
            shell_quote(&bin.join("grok").to_string_lossy()),
            shell_quote(&bin.join("grok").to_string_lossy()),
        );
        let result = ensure_cli_with_shell("grok", &script, path_of(bin.clone())).unwrap();
        assert!(result.installed);
        let grok = bin.join("grok");
        assert!(grok.is_file(), "missing {grok:?}");
        assert!(is_executable(&grok));
        drop(_guard);
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn cli_install_exit_zero_without_binary_is_not_on_path() {
        let _lock = lock_home();
        warm_login_path();
        let home = temp_dir("empty");
        let _guard = HomeGuard::set(&home);
        let err = ensure_cli_with_shell("grok", "exit 0", || String::new()).unwrap_err();
        assert!(
            err.contains("installed but not on PATH"),
            "unexpected error: {err}"
        );
        assert!(!err.contains("https://claude.ai/"));
        drop(_guard);
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn cli_install_already_resolved_does_not_start_script() {
        let _lock = lock_home();
        warm_login_path();
        let home = temp_dir("present");
        let _guard = HomeGuard::set(&home);
        let bin = home.join(".grok/bin");
        fs::create_dir_all(&bin).unwrap();
        let grok = bin.join("grok");
        crate::test_env::write_executable(&grok, "#!/bin/sh\nexit 0\n");
        let marker = home.join("ran");
        let script = format!("printf x > {}", shell_quote(&marker.to_string_lossy()));
        let result =
            ensure_cli_with_shell("grok", &script, || bin.to_string_lossy().into_owned()).unwrap();
        assert!(!result.installed);
        assert!(
            !marker.exists(),
            "already-resolved basename must not start the installer"
        );
        drop(_guard);
        let _ = fs::remove_dir_all(&home);
    }
}
