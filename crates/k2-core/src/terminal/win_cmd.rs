//! Windows bare-name spawn for ConPTY / CreateProcess.
//!
//! CreateProcess does **not** apply `PATHEXT`. PowerShell/`cmd` do.
//! npm global CLIs (`claude`, `codex`, `gemini`) are `claude.cmd` in
//! `%APPDATA%\npm`. Launch-bar spawn of `"claude"` therefore ENOENTs
//! while `claude --dangerously-skip-permissions` in PowerShell works.
//! Native `grok.exe` already resolves — that is why Grok works.

use std::path::{Path, PathBuf};

/// How the caller starts what [`resolve_for`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    /// ConPTY: K2 writes the whole command line ([`conpty_command`]), so a
    /// `.bat`/`.cmd` or a cmd.exe built-in goes through cmd.exe here, with
    /// every argument escaped for cmd.exe ([`bat_arg`]).
    Pty,
    /// `std::process::Command`: std runs a `.bat`/`.cmd` through cmd.exe
    /// itself (escaping its arguments, refusing a line break), so the
    /// batch file is returned as the program; built-ins aren't rewritten.
    Std,
}

/// Rewrite `(program, args)` so a ConPTY spawn can start npm `.cmd`
/// shims and cmd.exe built-ins. No-op on Unix, and when `program` is
/// already an `.exe`.
pub fn resolve_spawn(program: &str, args: &[String], path: &str) -> (String, Vec<String>) {
    resolve_native(Launcher::Pty, program, args, path)
}

/// [`resolve_spawn`] for a `std::process::Command` spawn.
pub fn resolve_command(program: &str, args: &[String], path: &str) -> (String, Vec<String>) {
    resolve_native(Launcher::Std, program, args, path)
}

fn resolve_native(launcher: Launcher, program: &str, args: &[String], path: &str) -> (String, Vec<String>) {
    #[cfg(not(windows))]
    {
        let _ = (launcher, path);
        (program.to_string(), args.to_vec())
    }
    #[cfg(windows)]
    {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        resolve_for(launcher, program, args, path, &pathext, &comspec, |p| p.is_file(), |p| {
            // A shim is a few hundred bytes; anything big is no npm shim.
            let meta = std::fs::metadata(p).ok()?;
            if meta.len() > SHIM_MAX_BYTES {
                return None;
            }
            std::fs::read_to_string(p).ok()
        })
    }
}

/// Windows: where `program` (a bare name) is on `path`, by cmd.exe's
/// rule (`PATHEXT`; an extensionless file is never a program).
#[cfg(windows)]
pub fn which(program: &str, path: &str) -> Option<PathBuf> {
    if program.is_empty() || program.contains(['/', '\\']) {
        return None;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    find_on_path(program, &split_search_dirs(path), &pathext, &|p: &Path| p.is_file())
}

/// The switches of the cmd.exe wrapper [`resolve_for`] builds; its last
/// argument, the payload, follows them, in a pair of quotes of its own.
/// `/e:ON` for the `%` escape ([`bat_arg`]), `/v:OFF` so `!` is plain
/// text, `/d` skips AutoRun, and `/s` strips just that outer pair.
pub const CMD_WRAPPER: [&str; 5] = ["/e:ON", "/v:OFF", "/d", "/s", "/c"];

/// How a ConPTY spawn of `(program, args)` must be handed to alacritty,
/// which joins `program` and `args` into one command line:
/// `(program, args, escape_args)`. A program path with spaces is quoted
/// (else `C:\Program Files\...` splits at the space), and args are
/// escaped the C-runtime way (else an arg with spaces, a whole prompt,
/// arrives as several). The one exception is the cmd.exe wrapper
/// ([`CMD_WRAPPER`]) [`resolve_for`] builds: its payload is already in
/// cmd.exe's own syntax, every argument escaped, and must reach it as written.
pub fn conpty_command(program: &str, args: &[String]) -> (String, Vec<String>, bool) {
    let quoted = if program.contains(char::is_whitespace) && !program.starts_with('"') {
        format!("\"{program}\"")
    } else {
        program.to_string()
    };
    let cmd_wrapper = args.len() == CMD_WRAPPER.len() + 1 && args[..CMD_WRAPPER.len()].iter().map(String::as_str).eq(CMD_WRAPPER);
    (quoted, args.to_vec(), !cmd_wrapper)
}

/// Testable core of [`resolve_spawn`] for ConPTY (no npm shim is read).
pub fn resolve_for_pty(
    program: &str,
    args: &[String],
    path: &str,
    pathext: &str,
    comspec: &str,
    exists: impl Fn(&Path) -> bool,
) -> (String, Vec<String>) {
    resolve_for(Launcher::Pty, program, args, path, pathext, comspec, exists, |_| None)
}

/// Testable core. `exists` and `read` (a small file's text) are injected
/// so macOS unit tests can fake a Windows npm layout.
///
/// - An npm shim (`claude.cmd`, `codex.cmd`…) runs what it would run,
///   without cmd.exe: `node <script>` or the `.exe` it names ([`npm_shim`]).
/// - Another `.bat`/`.cmd`, or a cmd.exe built-in, runs through cmd.exe
///   ([`Launcher::Pty`]: every argument escaped by [`bat_arg`]; one that
///   can't pass through cmd.exe (a line break) leaves `(program, args)` as
///   given, so the spawn fails instead) or is left to std ([`Launcher::Std`]).
#[allow(clippy::too_many_arguments)]
pub fn resolve_for(
    launcher: Launcher,
    program: &str,
    args: &[String],
    path: &str,
    pathext: &str,
    comspec: &str,
    exists: impl Fn(&Path) -> bool,
    read: impl Fn(&Path) -> Option<String>,
) -> (String, Vec<String>) {
    let unchanged = || (program.to_string(), args.to_vec());
    let dirs = split_search_dirs(path);
    let Some(found) = find_on_path(program, &dirs, pathext, &exists) else {
        // A cmd.exe built-in (`echo`, `dir`…) is no file on PATH: cmd.exe runs it.
        if launcher == Launcher::Pty && is_cmd_builtin(program) {
            if let Some(payload) = cmd_payload(program.to_string(), args) {
                return (comspec.to_string(), cmd_wrapper(payload));
            }
        }
        return unchanged();
    };
    if !is_batch(&found) {
        return (found.to_string_lossy().into_owned(), args.to_vec());
    }
    if let Some((prog, mut lead)) = npm_shim(&found, &dirs, pathext, &exists, &read) {
        lead.extend(args.iter().cloned());
        return (prog, lead);
    }
    match launcher {
        Launcher::Std => (found.to_string_lossy().into_owned(), args.to_vec()),
        Launcher::Pty => match cmd_payload(format!("\"{}\"", found.display()), args) {
            Some(payload) => (comspec.to_string(), cmd_wrapper(payload)),
            None => unchanged(),
        },
    }
}

fn is_batch(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("cmd") | Some("bat") => true,
        _ => false,
    }
}

/// The switches, then the payload in one more pair of quotes: the pair
/// `/s` strips (so the payload's own quotes are kept as written).
fn cmd_wrapper(payload: String) -> Vec<String> {
    CMD_WRAPPER.iter().map(|s| s.to_string()).chain(std::iter::once(format!("\"{payload}\""))).collect()
}

/// `head` (a quoted batch path, or a built-in's name) and each argument
/// escaped for cmd.exe; `None` when one can't be ([`bat_arg`]).
fn cmd_payload(head: String, args: &[String]) -> Option<String> {
    let mut payload = head;
    for a in args {
        payload.push(' ');
        payload.push_str(&bat_arg(a)?);
    }
    Some(payload)
}

/// One argument for cmd.exe (inside the [`CMD_WRAPPER`] payload), so it
/// arrives as the same text: std's rule for batch files. Anything but
/// letters, digits and `#$*+-./:?@\_` puts it in quotes, where cmd.exe's
/// operators (`& | < > ^ ( )`) are plain text; a `"` is doubled; a `%`
/// becomes `%%cd:~,%` (an empty substring of `%cd%`, so no `%VAR%` is
/// expanded); backslashes before a quote are doubled. `None` for a line
/// break or a NUL: cmd.exe ends the command there, and no escape keeps it.
pub fn bat_arg(arg: &str) -> Option<String> {
    if arg.contains(['\r', '\n', '\0']) {
        return None;
    }
    const UNQUOTED: &str = r"#$*+-./:?@\_";
    let quote = arg.is_empty()
        || arg.ends_with('\\')
        || arg.chars().any(|c| (c.is_ascii() && !(c.is_ascii_alphanumeric() || UNQUOTED.contains(c))) || c.is_control());
    let mut s = String::with_capacity(arg.len() + 2);
    if quote {
        s.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                s.extend(std::iter::repeat_n('\\', backslashes));
                s.push('"');
            } else if c == '%' {
                s.push_str("%%cd:~,");
            }
            backslashes = 0;
        }
        s.push(c);
    }
    if quote {
        s.extend(std::iter::repeat_n('\\', backslashes));
        s.push('"');
    }
    Some(s)
}

/// The most of a `.cmd` [`npm_shim`] reads.
#[cfg(windows)]
const SHIM_MAX_BYTES: u64 = 16 << 10;

/// An npm shim (cmd-shim: `claude.cmd`, `codex.cmd`, `gemini.cmd`…) runs
/// one target with `%*`: `"%_prog%" "%dp0%\…\cli.js" %*` (node, or the
/// shim's own `node.exe` beside it) or `"%dp0%\…\x.exe" %*`. Returns that
/// program and the arguments before the caller's: `(node, [script])` or
/// `(exe, [])`. `None` for anything else (it then runs as a batch file).
fn npm_shim(
    cmd: &Path,
    dirs: &[PathBuf],
    pathext: &str,
    exists: &impl Fn(&Path) -> bool,
    read: &impl Fn(&Path) -> Option<String>,
) -> Option<(String, Vec<String>)> {
    let text = read(cmd)?;
    let line = text.lines().map(str::trim).filter(|l| l.ends_with("%*")).last()?;
    let body = line[..line.len() - 2].trim_end();
    let start = body.rfind("\"%dp0%\\").or_else(|| body.rfind("\"%~dp0\\"))?;
    let quoted = &body[start + 1..];
    let end = quoted.find('"')?;
    // The path must be the last thing before `%*` (no fixed arguments).
    if end + 1 != quoted.len() {
        return None;
    }
    let rel = quoted[..end].strip_prefix("%dp0%\\").or_else(|| quoted[..end].strip_prefix("%~dp0\\"))?;
    if rel.is_empty() || rel.contains(['%', '"', '!', '^']) {
        return None;
    }
    let dir = cmd.parent()?;
    let target = dir.join(rel);
    if !exists(&target) {
        return None;
    }
    let target = target.to_string_lossy().into_owned();
    let lower = target.to_ascii_lowercase();
    let prefix = body[..start].trim_end();
    let by_node = prefix.ends_with("\"%_prog%\"") || prefix.ends_with("node") || prefix.to_ascii_lowercase().ends_with("node.exe\"");
    if by_node {
        if ![".js", ".cjs", ".mjs"].iter().any(|e| lower.ends_with(e)) {
            return None;
        }
        let beside = dir.join("node.exe");
        let node = if exists(&beside) {
            beside.to_string_lossy().into_owned()
        } else {
            let n = find_on_path("node", dirs, pathext, exists)?;
            if !n.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                return None;
            }
            n.to_string_lossy().into_owned()
        };
        return Some((node, vec![target]));
    }
    // No interpreter: the line is the path alone (or follows cmd-shim's `&`).
    if (prefix.is_empty() || prefix.ends_with('&')) && lower.ends_with(".exe") {
        return Some((target, Vec::new()));
    }
    None
}

/// cmd.exe's own commands: they are in cmd.exe, not files a spawn can find.
const CMD_BUILTINS: &[&str] = &[
    "assoc", "break", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir", "echo", "erase", "ftype",
    "md", "mkdir", "mklink", "move", "path", "pause", "popd", "prompt", "pushd", "rd", "ren", "rename", "rmdir", "set",
    "start", "time", "title", "type", "ver", "verify", "vol",
];

fn is_cmd_builtin(program: &str) -> bool {
    !program.contains(['/', '\\', '.']) && CMD_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(program))
}

/// Split a Windows PATH (`;` — and `:` only between entries, not
/// `C:\…`). Used so unit tests on macOS can feed Windows-shaped
/// strings. Live Windows spawn uses `std::env::split_paths` in
/// [`resolve_spawn`].
fn split_search_dirs(path: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        return std::env::split_paths(path).collect();
    }
    #[cfg(not(windows))]
    {
        path.split(';')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}

fn find_on_path(
    name: &str,
    dirs: &[PathBuf],
    pathext: &str,
    exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let raw = Path::new(name);
    let has_ext = raw.extension().is_some();
    let exts = parse_pathext(pathext);
    // cmd.exe's rule: a name with an extension runs as given; one without
    // runs only as <name><PATHEXT>. An extensionless file is never a
    // Windows program: npm puts a `codex` sh script (for Git Bash) beside
    // `codex.cmd`, and handing that script to CreateProcess fails with
    // "%1 is not a valid Win32 application".
    let in_dir = |base: &Path| -> Option<PathBuf> {
        if has_ext {
            return exists(base).then(|| base.to_path_buf());
        }
        for ext in &exts {
            // PATHEXT is usually uppercase (.EXE); npm files are
            // lowercase (.cmd). Real Win32 is_file is case-insensitive;
            // try both so tests and case-sensitive mounts work.
            let lower = ext.to_ascii_lowercase();
            for e in [ext.as_str(), lower.as_str()] {
                let mut cand = base.as_os_str().to_os_string();
                cand.push(e);
                let cand = PathBuf::from(cand);
                if exists(&cand) {
                    return Some(cand);
                }
            }
        }
        None
    };
    if raw.components().count() > 1 || raw.is_absolute() {
        return in_dir(raw);
    }
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = in_dir(&dir.join(name)) {
            return Some(found);
        }
    }
    None
}

/// PATHEXT entries we will try. Skip `.PS1` — CreateProcess cannot
/// run PowerShell scripts; the npm `.cmd` shim is the one we want.
fn parse_pathext(pathext: &str) -> Vec<String> {
    pathext
        .split(';')
        .filter_map(|raw| {
            let e = raw.trim();
            if e.is_empty() {
                return None;
            }
            let e = if e.starts_with('.') {
                e.to_string()
            } else {
                format!(".{e}")
            };
            if e.eq_ignore_ascii_case(".ps1") {
                return None;
            }
            Some(e)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn exists_in(files: &HashSet<PathBuf>) -> impl Fn(&Path) -> bool + '_ {
        move |p| files.contains(p)
    }

    #[test]
    fn grok_exe_stays_direct() {
        let dir = PathBuf::from(r"C:\tools");
        let grok = dir.join("grok.exe");
        let files: HashSet<_> = [grok.clone()].into_iter().collect();
        let path = dir.to_string_lossy().into_owned();
        let (prog, args) = resolve_for_pty(
            "grok",
            &["--always-approve".into()],
            &path,
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, grok.to_string_lossy());
        assert_eq!(args, vec!["--always-approve"]);
    }

    #[test]
    fn claude_cmd_wraps_comspec() {
        let dir = PathBuf::from(r"C:\Users\x\AppData\Roaming\npm");
        let cmd = dir.join("claude.cmd");
        let files: HashSet<_> = [cmd.clone()].into_iter().collect();
        let path = dir.to_string_lossy().into_owned();
        let (prog, args) = resolve_for_pty(
            "claude",
            &["--dangerously-skip-permissions".into()],
            &path,
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, r"C:\Windows\System32\cmd.exe");
        assert_eq!(&args[..5], &CMD_WRAPPER);
        assert_eq!(args[5], format!("\"\"{}\" --dangerously-skip-permissions\"", cmd.display()));
    }

    #[test]
    fn exe_wins_over_cmd_when_both_exist() {
        let dir = PathBuf::from(r"C:\bin");
        let files: HashSet<_> = [dir.join("foo.exe"), dir.join("foo.cmd")]
            .into_iter()
            .collect();
        let (prog, args) = resolve_for_pty(
            "foo",
            &[],
            &dir.to_string_lossy(),
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, dir.join("foo.exe").to_string_lossy());
        assert!(args.is_empty());
    }

    /// npm's global dir holds `codex` (an sh script for Git Bash), `codex.cmd`
    /// and `codex.ps1`: the .cmd is the Windows program, never the script.
    #[test]
    fn conpty_command_quotes_a_spaced_program_escapes_args_and_leaves_the_cmd_wrapper() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // A prompt with spaces stays one arg: escaping on.
        let (p, a, esc) = conpty_command(r"C:\Program Files\nodejs\codex.exe", &s(&["--", "hello from the API"]));
        assert_eq!(p, r#""C:\Program Files\nodejs\codex.exe""#);
        assert_eq!(a, s(&["--", "hello from the API"]));
        assert!(esc);
        // No spaces: the program is untouched.
        let (p, _, esc) = conpty_command(r"C:\tools\claude.exe", &[]);
        assert_eq!((p.as_str(), esc), (r"C:\tools\claude.exe", true));
        // K2's own npm .cmd wrapper goes through as cmd.exe syntax.
        let (p, a, esc) = conpty_command(r"C:\Windows\System32\cmd.exe", &s(&["/e:ON", "/v:OFF", "/d", "/s", "/c", r#""C:\npm\codex.cmd" "a b""#]));
        assert_eq!(p, r"C:\Windows\System32\cmd.exe");
        assert_eq!(a.len(), 6);
        assert!(!esc, "the wrapper's payload is already quoted for cmd.exe");
        // Any other cmd use is escaped like everything else.
        assert!(conpty_command("cmd.exe", &s(&["/c", "dir", r"C:\Program Files"])).2);
    }

    #[test]
    fn an_extensionless_npm_script_is_never_the_program() {
        let dir = PathBuf::from(r"C:\Users\x\AppData\Roaming\npm");
        let files: HashSet<_> = [dir.join("codex"), dir.join("codex.cmd"), dir.join("codex.ps1")].into_iter().collect();
        let (prog, args) = resolve_for_pty(
            "codex",
            &[],
            &dir.to_string_lossy(),
            ".COM;.EXE;.BAT;.CMD;.PS1",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, r"C:\Windows\System32\cmd.exe");
        assert!(args[5].contains("codex.cmd"), "{args:?}");
        // The same through an absolute path without an extension.
        let full = dir.join("codex").to_string_lossy().into_owned();
        let (prog, args) = resolve_for_pty(&full, &[], "", ".EXE;.CMD", "cmd.exe", exists_in(&files));
        assert_eq!(prog, "cmd.exe");
        assert!(args[5].contains("codex.cmd"), "{args:?}");
        // An extensionless file alone resolves to nothing (left as given).
        let only: HashSet<_> = [dir.join("lonely")].into_iter().collect();
        let (prog, _) = resolve_for_pty("lonely", &[], &dir.to_string_lossy(), ".EXE;.CMD", "cmd.exe", exists_in(&only));
        assert_eq!(prog, "lonely");
    }

    #[test]
    fn a_cmd_builtin_runs_through_cmd_exe() {
        let files: HashSet<PathBuf> = HashSet::new();
        let (prog, args) = resolve_for_pty("echo", &["hello".into(), "two words".into()], r"C:\empty", ".EXE;.CMD", "cmd.exe", exists_in(&files));
        assert_eq!(prog, "cmd.exe");
        assert_eq!(args, vec!["/e:ON", "/v:OFF", "/d", "/s", "/c", "\"echo hello \"two words\"\""]);
        // A real program of that name on PATH wins, as in cmd.exe itself.
        let echo = PathBuf::from(r"C:\tools").join("echo.exe");
        let files: HashSet<PathBuf> = [echo.clone()].into_iter().collect();
        let (prog, _) = resolve_for_pty("echo", &[], r"C:\tools", ".EXE;.CMD", "cmd.exe", exists_in(&files));
        assert_eq!(prog, echo.to_string_lossy());
        // A path is never a built-in.
        let none: HashSet<PathBuf> = HashSet::new();
        let (prog, _) = resolve_for_pty(r".\echo", &[], r"C:\empty", ".EXE", "cmd.exe", exists_in(&none));
        assert_eq!(prog, r".\echo");
    }

    /// Every argument reaches cmd.exe as text: its operators quoted, `"`
    /// doubled, `%` kept from expanding (std's rule for batch files).
    #[test]
    fn cmd_exe_arguments_are_escaped_as_text() {
        let cases: &[(&str, &str)] = &[
            ("plain-arg", "plain-arg"),
            (r"C:\dir\file.txt", r"C:\dir\file.txt"),
            ("two words", "\"two words\""),
            ("a&b", "\"a&b\""),
            ("a|b<c>d^e(f)", "\"a|b<c>d^e(f)\""),
            ("x\" & more & \"", "\"x\"\" & more & \"\"\""),
            ("%PATH%", "\"%%cd:~,%PATH%%cd:~,%\""),
            ("!x!", "\"!x!\""),
            (r"tail\", r#""tail\\""#),
            (r#"a\"b"#, r#""a\\""b""#),
            ("", "\"\""),
        ];
        for (arg, want) in cases {
            assert_eq!(bat_arg(arg).as_deref(), Some(*want), "{arg:?}");
        }
        for arg in ["a\nb", "a\r\nb", "a\0b"] {
            assert_eq!(bat_arg(arg), None, "{arg:?} can't pass through cmd.exe");
        }
    }

    #[test]
    fn a_batch_file_or_built_in_never_gets_an_unescaped_argument() {
        let dir = PathBuf::from(r"C:\tools");
        let files: HashSet<_> = [dir.join("run.bat")].into_iter().collect();
        let args: Vec<String> = vec!["ok".into(), "a&more".into(), "x\"&more&\"".into()];
        let (prog, a) = resolve_for_pty("run", &args, &dir.to_string_lossy(), ".EXE;.BAT", "cmd.exe", exists_in(&files));
        assert_eq!(prog, "cmd.exe");
        assert_eq!(a[5], format!(r#"""{}" ok "a&more" "x""&more&"""""#, dir.join("run.bat").display()));
        let (prog, a) = resolve_for_pty("echo", &args, r"C:\empty", ".EXE", "cmd.exe", exists_in(&files));
        assert_eq!(prog, "cmd.exe");
        assert_eq!(a[5], r#""echo ok "a&more" "x""&more&"""""#);
        // A line break can't pass through cmd.exe: left as given, the spawn fails.
        let broken: Vec<String> = vec!["line one\nline two".into()];
        assert_eq!(resolve_for_pty("run", &broken, &dir.to_string_lossy(), ".EXE;.BAT", "cmd.exe", exists_in(&files)), ("run".to_string(), broken.clone()));
        assert_eq!(resolve_for_pty("echo", &broken, r"C:\empty", ".EXE", "cmd.exe", exists_in(&files)), ("echo".to_string(), broken));
    }

    #[test]
    fn std_spawns_get_the_batch_file_itself_and_no_built_ins() {
        let dir = PathBuf::from(r"C:\tools");
        let files: HashSet<_> = [dir.join("run.bat")].into_iter().collect();
        let args: Vec<String> = vec!["a&b".into()];
        let r = resolve_for(Launcher::Std, "run", &args, &dir.to_string_lossy(), ".EXE;.BAT", "cmd.exe", exists_in(&files), |_| None);
        assert_eq!(r, (dir.join("run.bat").to_string_lossy().into_owned(), args.clone()), "std escapes a batch file's arguments itself");
        let r = resolve_for(Launcher::Std, "echo", &args, r"C:\empty", ".EXE", "cmd.exe", exists_in(&files), |_| None);
        assert_eq!(r, ("echo".to_string(), args));
    }

    const MODERN_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";
    const OLD_SHIM: &str = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\node_modules\\npm\\bin\\npm-cli.js\" %*\r\n) ELSE (\r\n  @SETLOCAL\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\node_modules\\npm\\bin\\npm-cli.js\" %*\r\n)\r\n";
    const EXE_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.exe\"   %*\r\n";

    #[test]
    fn an_npm_shim_runs_its_target_without_cmd_exe() {
        let npm = PathBuf::from(r"C:\Users\x\AppData\Roaming\npm");
        let node = PathBuf::from(r"C:\Program Files\nodejs").join("node.exe");
        // `\` inside: the shim's own spelling (on Windows one path; joined as Windows does).
        let cli = npm.join(r"node_modules\@anthropic-ai\claude-code\cli.js").to_string_lossy().into_owned();
        let npm_cli = npm.join(r"node_modules\npm\bin\npm-cli.js").to_string_lossy().into_owned();
        let codex = npm.join(r"node_modules\@openai\codex\bin\codex.exe").to_string_lossy().into_owned();
        let files: HashSet<PathBuf> =
            [npm.join("claude.cmd"), npm.join("npm.cmd"), npm.join("codex.cmd"), node.clone(), PathBuf::from(&cli), PathBuf::from(&npm_cli), PathBuf::from(&codex)]
                .into_iter()
                .collect();
        let path = format!(r"{};C:\Program Files\nodejs", npm.display());
        let read = |p: &Path| -> Option<String> {
            let name = p.file_name()?.to_str()?;
            Some(match name { "claude.cmd" => MODERN_SHIM, "npm.cmd" => OLD_SHIM, "codex.cmd" => EXE_SHIM, _ => return None }.to_string())
        };
        // A whole multi-line prompt with cmd.exe's operators: one argument, untouched.
        let prompt = "line one & \"two\"\n%PATH% | three".to_string();
        let args = vec!["--append-system-prompt".to_string(), prompt.clone()];
        for launcher in [Launcher::Pty, Launcher::Std] {
            let r = resolve_for(launcher, "claude", &args, &path, ".EXE;.CMD", "cmd.exe", exists_in(&files), read);
            assert_eq!(r, (node.to_string_lossy().into_owned(), vec![cli.clone(), "--append-system-prompt".into(), prompt.clone()]), "{launcher:?}");
            let r = resolve_for(launcher, "npm", &["ls".into()], &path, ".EXE;.CMD", "cmd.exe", exists_in(&files), read);
            assert_eq!(r, (node.to_string_lossy().into_owned(), vec![npm_cli.clone(), "ls".into()]), "{launcher:?}");
            let r = resolve_for(launcher, "codex", &args, &path, ".EXE;.CMD", "cmd.exe", exists_in(&files), read);
            assert_eq!(r, (codex.clone(), args.clone()), "{launcher:?}");
        }
        // A node.exe beside the shim wins over the one on PATH.
        let mut with_local = files.clone();
        with_local.insert(npm.join("node.exe"));
        let r = resolve_for(Launcher::Pty, "claude", &[], &path, ".EXE;.CMD", "cmd.exe", exists_in(&with_local), read);
        assert_eq!(r.0, npm.join("node.exe").to_string_lossy());
        // The target missing, or a shim with fixed arguments: a batch file as before.
        let no_cli: HashSet<PathBuf> = files.iter().filter(|p| p.to_string_lossy() != cli.as_str()).cloned().collect();
        let r = resolve_for(Launcher::Pty, "claude", &[], &path, ".EXE;.CMD", "cmd.exe", exists_in(&no_cli), read);
        assert_eq!(r.0, "cmd.exe");
        let fixed = |_: &Path| Some("\"%dp0%\\node_modules\\x\\x.exe\" --fixed %*\r\n".to_string());
        let r = resolve_for(Launcher::Pty, "claude", &[], &path, ".EXE;.CMD", "cmd.exe", exists_in(&files), fixed);
        assert_eq!(r.0, "cmd.exe");
    }

    /// Windows only (it runs cmd.exe): what a batch file and a built-in get
    /// is the text that was sent, operators and `%VAR%` included, and
    /// nothing else runs.
    #[cfg(windows)]
    #[test]
    fn cmd_exe_runs_nothing_an_argument_says() {
        use std::os::windows::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("k2-win-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("show.cmd"), "@echo off\r\necho(ARGS[%*]\r\n").expect("batch");
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let args: Vec<String> = vec!["a&echo SECOND".into(), "q\"&echo SECOND&\"".into(), "%USERNAME%".into(), "x|echo SECOND".into()];
        let path = dir.to_string_lossy().into_owned();
        for program in ["show", "echo"] {
            let (prog, a) = resolve_for(Launcher::Pty, program, &args, &path, ".EXE;.CMD", &comspec, |p| p.is_file(), |_| None);
            assert_eq!(&a[..5], &CMD_WRAPPER, "{program}");
            // As ConPTY gets it: the switches, then the payload as written.
            let out = std::process::Command::new(&prog).args(&a[..5]).raw_arg(&a[5]).output().expect("cmd.exe");
            let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
            let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
            assert_eq!(lines.len(), 1, "{program}: one line, nothing else ran: {text}");
            assert!(lines[0].contains("%USERNAME%"), "{program}: no variable expanded: {text}");
            assert!(lines[0].contains("a&echo SECOND"), "{program}: {text}");
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn missing_name_left_unchanged() {
        let files: HashSet<PathBuf> = HashSet::new();
        let (prog, args) = resolve_for_pty(
            "nope",
            &["-x".into()],
            r"C:\empty",
            ".EXE;.CMD",
            "cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, "nope");
        assert_eq!(args, vec!["-x"]);
    }

    #[test]
    fn parse_pathext_skips_ps1() {
        let exts = parse_pathext(".COM;.EXE;.PS1;.CMD");
        assert!(exts.iter().any(|e| e.eq_ignore_ascii_case(".cmd")));
        assert!(!exts.iter().any(|e| e.eq_ignore_ascii_case(".ps1")));
    }
}
