//! Per-service warnings, the static half (0.46.0 grace).
//!
//! K2 can't watch which environment variables a process reads, so it reads
//! the service's code instead: every literal env read (`process.env.X`,
//! `os.getenv("X")`, `$X` in shell and npm scripts, …), every reference to
//! K2's own files (the owner token file, `heartbeat.port`, K2's databases
//! and sockets), and every program the start command finds through `PATH`
//! (the command itself, npm scripts, `#!/usr/bin/env` shebangs). The daemon
//! joins this with what the service actually inherited and what clean mode
//! gives it, and names what breaks in 0.46.1.
//!
//! Bounded: at most [`MAX_FILES`] files of at most [`MAX_FILE_BYTES`],
//! [`MAX_DEPTH`] deep; dependency and build-cache folders are skipped.
//! Read-only; follows no symlink out of the folder.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const MAX_FILES: usize = 4000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_DEPTH: usize = 12;
/// Findings kept per kind (the rest are counted, not listed).
pub const MAX_FINDINGS: usize = 400;

/// The `PATH` a clean-mode service gets (and roughly what a 0.46.1 wall
/// gives): system folders only, never the daemon user's own bin folders.
#[cfg(target_os = "macos")]
pub const CLEAN_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/homebrew/bin:/opt/homebrew/sbin";
#[cfg(not(target_os = "macos"))]
pub const CLEAN_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

const SKIP_DIRS: &[&str] = &[
    "node_modules", "bower_components", "target", "vendor", "__pycache__", "venv", "coverage", "jspm_packages",
];

const CODE_EXT: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "py", "go", "rs", "rb", "sh", "bash", "zsh",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvRead {
    pub name: String,
    /// Folder-relative path (`<cmd>` for the start command itself).
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct K2PathRef {
    /// `owner_token_file` | `daemon_port_file` | `k2_database` | `k2_socket`.
    pub what: String,
    pub file: String,
    pub line: u32,
}

/// A file that sends form-encoded bodies (`application/x-www-form-urlencoded`)
/// and names K2's App-facing writes or K2's address (0.46.4 field report:
/// the App gateway reads a form only where the route's handler does,
/// `crate::app_body`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormPost {
    pub file: String,
    /// The line naming the form content type.
    pub line: u32,
    /// The App-facing `POST /cli/…` routes the same file names (may be
    /// empty when it builds the route at run time).
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub env_reads: Vec<EnvRead>,
    pub k2_paths: Vec<K2PathRef>,
    /// Files that post form-encoded bodies to K2 (or seem to).
    pub form_posts: Vec<FormPost>,
    pub files_scanned: usize,
    /// More files or findings than the caps; the lists are partial.
    pub truncated: bool,
}

impl ScanReport {
    /// Distinct env names read anywhere.
    pub fn env_names(&self) -> BTreeSet<String> {
        self.env_reads.iter().map(|r| r.name.clone()).collect()
    }
}

/// Scan `root` (a service's working folder) and its start command.
pub fn scan_folder(root: &Path, cmd: &str) -> ScanReport {
    let mut rep = ScanReport::default();
    for name in shell_env_names(cmd) {
        push_env(&mut rep, name, "<cmd>", 1);
    }
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(true)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .max_depth(Some(MAX_DEPTH))
        .filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir()) && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())));
    for entry in walk.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let p = entry.path();
        let fname = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let is_pkg = fname == "package.json";
        if !is_pkg && !CODE_EXT.contains(&ext.as_str()) {
            continue;
        }
        if rep.files_scanned >= MAX_FILES {
            rep.truncated = true;
            break;
        }
        let Some(text) = read_bounded_no_follow(p) else { continue };
        rep.files_scanned += 1;
        // A finding's file reads with '/' on every OS (Windows: '\' → '/';
        // on Unix a '\' is part of a file name and stays).
        let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
        let rel = if cfg!(windows) { rel.replace('\\', "/") } else { rel };
        scan_text(&mut rep, &rel, &ext, is_pkg, &text);
    }
    rep
}

/// One file's text for the scan, or `None` (skip it). The service's folder
/// can change while it is scanned (0.46.5, papercuts review: the scan
/// race). Before, the size came from a `metadata()` that follows links and
/// the text from a second, unbounded `read_to_string()` by path: a file
/// grown or swapped for a symlink in between was read whole, or read from
/// outside the folder. Now the one handle that is read is opened without
/// following a final symlink, must be a regular file by its own `fstat`,
/// and is read through a [`MAX_FILE_BYTES`] + 1 cap (over it: skipped).
fn read_bounded_no_follow(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: a FIFO swapped in never blocks the open.
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = opts.open(p).ok()?;
    let meta = f.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    f.take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(buf).ok()
}

fn push_env(rep: &mut ScanReport, name: String, file: &str, line: u32) {
    if rep.env_reads.iter().any(|r| r.name == name && r.file == file) {
        return;
    }
    if rep.env_reads.len() >= MAX_FINDINGS {
        rep.truncated = true;
        return;
    }
    rep.env_reads.push(EnvRead { name, file: file.to_string(), line });
}

fn push_path(rep: &mut ScanReport, what: &str, file: &str, line: u32) {
    if rep.k2_paths.iter().any(|r| r.what == what && r.file == file) {
        return;
    }
    if rep.k2_paths.len() >= MAX_FINDINGS {
        rep.truncated = true;
        return;
    }
    rep.k2_paths.push(K2PathRef { what: what.to_string(), file: file.to_string(), line });
}

fn line_of(text: &str, at: usize) -> u32 {
    text[..at.min(text.len())].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// An identifier at `i` (after optional spaces).
fn ident_at(b: &[u8], mut i: usize) -> Option<(String, usize)> {
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    if i >= b.len() || !is_ident_start(b[i]) {
        return None;
    }
    let s = i;
    while i < b.len() && is_ident(b[i]) {
        i += 1;
    }
    Some((String::from_utf8_lossy(&b[s..i]).to_string(), i))
}

/// A string literal (`'X'`, `"X"` or `` `X` ``) at `i` (after optional
/// spaces) that is a plain identifier.
fn literal_at(b: &[u8], mut i: usize) -> Option<String> {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\n' || b[i] == b'\t') {
        i += 1;
    }
    let q = *b.get(i)?;
    if q != b'\'' && q != b'"' && q != b'`' {
        return None;
    }
    let (name, j) = ident_at(b, i + 1)?;
    (b.get(j) == Some(&q)).then_some(name)
}

fn find_all<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(needle).map(|(i, _)| i)
}

fn scan_text(rep: &mut ScanReport, rel: &str, ext: &str, is_pkg: bool, text: &str) {
    let b = text.as_bytes();
    // K2's own files, in any scanned file.
    for (needle, what) in [
        ("daemon.token", "owner_token_file"),
        ("heartbeat.port", "daemon_port_file"),
        ("k2so.db", "k2_database"),
        ("skin.db", "k2_database"),
        ("vault.sock", "k2_socket"),
        (".k2/run/", "k2_socket"),
    ] {
        if let Some(at) = text.find(needle) {
            push_path(rep, what, rel, line_of(text, at));
        }
    }
    // `k2.db` (the home DB's name from 0.46.6) as a file name, not the SDK
    // object `k2.db.query(…)` / `k2.db(…)`: no identifier character or `.`
    // before it, and none of identifier / `.` / `(` after it.
    if let Some(at) = find_all(text, "k2.db").find(|&i| {
        let before = i.checked_sub(1).map(|j| b[j]);
        let after = b.get(i + "k2.db".len()).copied();
        !before.is_some_and(|c| is_ident(c) || c == b'.')
            && !after.is_some_and(|c| is_ident(c) || c == b'.' || c == b'(')
    }) {
        push_path(rep, "k2_database", rel, line_of(text, at));
    }
    if !is_pkg {
        form_posts(rep, rel, text);
    }
    if is_pkg {
        // npm scripts: `$X` / `${X}` in the "scripts" values.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) {
                for (k, sv) in scripts {
                    if let Some(s) = sv.as_str() {
                        let line = text.find(&format!("\"{k}\"")).map(|a| line_of(text, a)).unwrap_or(1);
                        for name in shell_env_names(s) {
                            push_env(rep, name, rel, line);
                        }
                    }
                }
            }
        }
        return;
    }
    match ext {
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => {
            for prefix in ["process.env", "Bun.env", "import.meta.env"] {
                for at in find_all(text, prefix) {
                    let after = at + prefix.len();
                    match b.get(after) {
                        Some(b'.') => {
                            if let Some((n, _)) = ident_at(b, after + 1) {
                                push_env(rep, n, rel, line_of(text, at));
                            }
                        }
                        Some(b'[') => {
                            if let Some(n) = literal_at(b, after + 1) {
                                push_env(rep, n, rel, line_of(text, at));
                            }
                        }
                        _ => {
                            for n in destructured_before(text, at) {
                                push_env(rep, n, rel, line_of(text, at));
                            }
                        }
                    }
                }
            }
            for at in find_all(text, "Deno.env.get(") {
                if let Some(n) = literal_at(b, at + "Deno.env.get(".len()) {
                    push_env(rep, n, rel, line_of(text, at));
                }
            }
        }
        "py" => call_literals(rep, rel, text, &["os.environ[", "os.environ.get(", "os.getenv(", "environ.get(", "getenv("]),
        "go" => call_literals(rep, rel, text, &["os.Getenv(", "os.LookupEnv("]),
        "rs" => call_literals(rep, rel, text, &["env::var(", "env::var_os(", "env!(", "option_env!("]),
        "rb" => call_literals(rep, rel, text, &["ENV[", "ENV.fetch("]),
        "sh" | "bash" | "zsh" => {
            for name in shell_env_names(text) {
                let line = text.find(&format!("${name}")).or_else(|| text.find(&format!("${{{name}"))).map(|a| line_of(text, a)).unwrap_or(1);
                push_env(rep, name, rel, line);
            }
        }
        _ => {}
    }
}

/// The form content type, as code spells it.
const FORM_CT: &str = "x-www-form-urlencoded";
/// Words that say a file talks to K2 when it names no `/cli/` route.
const K2_ADDRESS: &[&str] = &["K2_GATEWAY_URL", "K2_PORT", "K2_PASS_FILE"];

/// A file that names the form content type and K2 (a `/cli/…` write the
/// App gateway serves, or K2's address): one [`FormPost`] for it. Static
/// and coarse: it can't tell which request the form goes to, so it lists
/// the K2 writes the same file names.
fn form_posts(rep: &mut ScanReport, rel: &str, text: &str) {
    let lower = text.to_ascii_lowercase();
    let Some(at) = lower.find(FORM_CT) else { return };
    let mut routes: Vec<String> = Vec::new();
    for (i, _) in text.match_indices("/cli/") {
        let route: String = text[i..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_')).collect();
        let route = route.trim_end_matches('/').to_string();
        if crate::app_surface::link_allows_http("POST", &route) && !routes.contains(&route) {
            routes.push(route);
        }
    }
    if routes.is_empty() && !K2_ADDRESS.iter().any(|w| text.contains(w)) {
        return;
    }
    if rep.form_posts.len() >= MAX_FINDINGS {
        rep.truncated = true;
        return;
    }
    rep.form_posts.push(FormPost { file: rel.to_string(), line: line_of(text, at), routes });
}

fn call_literals(rep: &mut ScanReport, rel: &str, text: &str, calls: &[&str]) {
    let b = text.as_bytes();
    for call in calls {
        for at in find_all(text, call) {
            // `getenv(` must not be the tail of another identifier we
            // already matched (`os.getenv(`) or of an unrelated name.
            if *call == "getenv(" && at > 0 && (is_ident(b[at - 1]) || b[at - 1] == b'.') {
                continue;
            }
            if *call == "environ.get(" && at > 0 && b[at - 1] == b'.' {
                continue;
            }
            if let Some(n) = literal_at(b, at + call.len()) {
                push_env(rep, n, rel, line_of(text, at));
            }
        }
    }
}

/// `const { A, B: b, C = 1, ...rest } = process.env` → A, B, C.
fn destructured_before(text: &str, at: usize) -> Vec<String> {
    let head = text[..at].trim_end();
    let Some(head) = head.strip_suffix('=') else { return vec![] };
    let head = head.trim_end();
    let Some(head) = head.strip_suffix('}') else { return vec![] };
    let Some(open) = head.rfind('{') else { return vec![] };
    let inner = &head[open + 1..];
    if inner.len() > 2000 || inner.contains('{') {
        return vec![];
    }
    inner
        .split(',')
        .filter_map(|part| {
            let p = part.trim();
            if p.starts_with("...") {
                return None;
            }
            let key = p.split([':', '=']).next().unwrap_or("").trim();
            let kb = key.as_bytes();
            (!kb.is_empty() && is_ident_start(kb[0]) && kb.iter().all(|c| is_ident(*c))).then(|| key.to_string())
        })
        .collect()
}

/// Upper-case `$X` / `${X…}` names in shell text.
pub fn shell_env_names(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let mut j = i + 1;
            if b.get(j) == Some(&b'{') {
                j += 1;
            }
            let s = j;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit() || b[j] == b'_') {
                j += 1;
            }
            if j > s && (b[s].is_ascii_uppercase() || b[s] == b'_') && !(j < b.len() && b[j].is_ascii_lowercase()) {
                let n = String::from_utf8_lossy(&b[s..j]).to_string();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

// ── PATH lookups ─────────────────────────────────────────────────────

/// One program the start command finds through `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathLookup {
    pub program: String,
    /// Where the lookup happens: `cmd`, `npm script start`, `shebang server.js`.
    pub via: String,
    /// What the inherited `PATH` finds today.
    pub now: Option<String>,
    /// What the clean `PATH` finds (`None`: it doesn't).
    pub clean: Option<String>,
}

impl PathLookup {
    /// Found today only because of the inherited `PATH`.
    pub fn breaks(&self) -> bool {
        self.now.is_some() && self.clean.is_none()
    }
}

const BUILTINS: &[&str] = &[
    "cd", "export", "exec", "test", "[", "true", "false", "echo", "set", "unset", "source", ".", "trap", "wait", "eval",
    "printf", "read", "ulimit", "umask", "if", "then", "else", "fi", "for", "do", "done", "while", "case", "esac", "exit",
    "return", "shift", "local", ":", "(", ")", "{", "}",
];
const WRAPPERS: &[&str] = &["exec", "env", "nohup", "nice", "time", "command", "stdbuf", "setsid"];
const RUNNERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];

/// Resolve `program` on a `PATH` string (executable files only).
pub fn which(program: &str, path: &str) -> Option<PathBuf> {
    // Windows: PATH is ';'-separated (':' is part of a drive) and a program
    // runs as name.exe / .cmd / .bat / .com, as CreateProcess and cmd.exe
    // look it up.
    #[cfg(windows)]
    {
        for dir in std::env::split_paths(path).filter(|d| !d.as_os_str().is_empty()) {
            let exact = dir.join(program);
            if is_executable(&exact) {
                return Some(exact);
            }
            for ext in [".exe", ".cmd", ".bat", ".com"] {
                let p = dir.join(format!("{program}{ext}"));
                if is_executable(&p) {
                    return Some(p);
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            let p = Path::new(dir).join(program);
            if is_executable(&p) {
                return Some(p);
            }
        }
        None
    }
}

fn is_executable(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
        _ => false,
    }
}

/// Split a shell command into simple commands' words (rough: `&&`, `||`,
/// `;`, `|`, newlines; quotes keep words together).
fn simple_commands(cmd: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let chars = cmd.chars();
    let flush_word = |cur: &mut String, words: &mut Vec<String>| {
        if !cur.is_empty() {
            words.push(std::mem::take(cur));
        }
    };
    for c in chars {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            ' ' | '\t' => flush_word(&mut cur, &mut words),
            // `&&`, `||`, `;`, `|`, a background `&`, subshell parens:
            // each ends a simple command.
            ';' | '\n' | '|' | '&' | '(' | ')' => {
                flush_word(&mut cur, &mut words);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
            }
            _ => cur.push(c),
        }
    }
    flush_word(&mut cur, &mut words);
    if !words.is_empty() {
        out.push(words);
    }
    out
}

/// The program word of a simple command (after `A=b` and wrappers).
fn program_of(words: &[String]) -> Option<(String, Vec<String>)> {
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        if w.contains('=') && !w.starts_with('=') && w.split('=').next().is_some_and(|k| k.bytes().all(is_ident)) {
            i += 1;
            continue;
        }
        if WRAPPERS.contains(&w.as_str()) {
            i += 1;
            while i < words.len() && words[i].starts_with('-') {
                i += 1;
            }
            continue;
        }
        return Some((w.clone(), words[i + 1..].to_vec()));
    }
    None
}

/// Every `PATH` lookup the start command makes, with what the inherited
/// `PATH` (`now_path`) and the clean one (`clean_path`) find.
pub fn path_lookups(cmd: &str, cwd: &Path, now_path: &str, clean_path: &str) -> Vec<PathLookup> {
    let mut out: Vec<PathLookup> = Vec::new();
    lookups_in(cmd, "cmd", cwd, now_path, clean_path, 0, &mut out);
    out
}

fn add_lookup(out: &mut Vec<PathLookup>, program: &str, via: &str, now_path: &str, clean_path: &str) {
    if out.iter().any(|l| l.program == program && l.via == via) || out.len() >= 64 {
        return;
    }
    out.push(PathLookup {
        program: program.to_string(),
        via: via.to_string(),
        now: which(program, now_path).map(|p| p.display().to_string()),
        clean: which(program, clean_path).map(|p| p.display().to_string()),
    });
}

fn lookups_in(cmd: &str, via: &str, cwd: &Path, now_path: &str, clean_path: &str, depth: u8, out: &mut Vec<PathLookup>) {
    if depth > 2 {
        return;
    }
    for words in simple_commands(cmd) {
        let Some((prog, args)) = program_of(&words) else { continue };
        if BUILTINS.contains(&prog.as_str()) || prog.starts_with('$') {
            continue;
        }
        if prog.contains('/') {
            // A script run by path: its shebang may look a program up.
            let p = if Path::new(&prog).is_absolute() { PathBuf::from(&prog) } else { cwd.join(&prog) };
            if let Some(interp) = env_shebang(&p) {
                add_lookup(out, &interp, &format!("shebang {prog}"), now_path, clean_path);
            }
            continue;
        }
        // Programs npm puts on PATH for scripts (node_modules/.bin) are local.
        if depth > 0 && is_executable(&cwd.join("node_modules/.bin").join(&prog)) {
            continue;
        }
        add_lookup(out, &prog, via, now_path, clean_path);
        if RUNNERS.contains(&prog.as_str()) {
            if let Some(script) = runner_script(&prog, &args) {
                if let Some(body) = package_script(cwd, &script) {
                    lookups_in(&body, &format!("npm script {script}"), cwd, now_path, clean_path, depth + 1, out);
                }
            }
        }
    }
}

/// `npm start` → start; `npm run dev` → dev; `yarn dev` → dev.
fn runner_script(prog: &str, args: &[String]) -> Option<String> {
    let args: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    match (prog, args.first().copied()) {
        (_, Some("start")) => Some("start".into()),
        (_, Some("run" | "run-script")) => args.get(1).map(|s| s.to_string()),
        ("yarn" | "pnpm" | "bun", Some(s)) if !matches!(s, "install" | "add" | "exec" | "dlx" | "x") => Some(s.to_string()),
        _ => None,
    }
}

fn package_script(cwd: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(cwd.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("scripts")?.get(name)?.as_str().map(str::to_string)
}

/// `#!/usr/bin/env node` → `node`. Absolute interpreters need no lookup.
fn env_shebang(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = [0u8; 256];
    let n = f.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let first = head.lines().next()?.strip_prefix("#!")?.trim().to_string();
    let mut parts = first.split_whitespace();
    let interp = parts.next()?;
    if !interp.ends_with("/env") {
        return None;
    }
    parts.find(|a| !a.starts_with('-')).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-grace-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 0.46.5 (the scan race): the one read is bounded and never follows a
    /// symlink or opens anything but a regular file, whatever the folder
    /// turned into after the walk listed it.
    #[cfg(unix)]
    #[test]
    fn bounded_read_refuses_links_fifos_and_oversize() {
        let d = tmp();
        let outside = tmp();
        std::fs::write(outside.join("secret.js"), "process.env.OUTSIDE_SECRET_NAME").unwrap();
        std::fs::write(d.join("ok.js"), "process.env.INSIDE").unwrap();
        assert_eq!(read_bounded_no_follow(&d.join("ok.js")).as_deref(), Some("process.env.INSIDE"));
        std::os::unix::fs::symlink(outside.join("secret.js"), d.join("swapped.js")).unwrap();
        assert_eq!(read_bounded_no_follow(&d.join("swapped.js")), None, "a symlink is never followed");
        let fifo = d.join("pipe.js");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo has no other effect.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        assert_eq!(read_bounded_no_follow(&fifo), None, "a FIFO is skipped without blocking");
        let big = d.join("big.js");
        std::fs::write(&big, vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(read_bounded_no_follow(&big), None, "over the cap: skipped");
        std::fs::write(&big, vec![b'a'; MAX_FILE_BYTES as usize]).unwrap();
        assert_eq!(read_bounded_no_follow(&big).map(|t| t.len()), Some(MAX_FILE_BYTES as usize), "at the cap: read");
        let r = scan_folder(&d, "node ok.js");
        assert!(r.env_names().contains("INSIDE"), "{r:?}");
        assert!(!r.env_names().contains("OUTSIDE_SECRET_NAME"), "nothing from outside the folder: {r:?}");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn finds_env_reads_across_languages() {
        let d = tmp();
        std::fs::write(
            d.join("server.mjs"),
            "const port = process.env.PORT;\nconst t = process.env['K2_OWNER_TOKEN'];\nconst { DATABASE_URL, K2_HOST: h, MODE = 'x', ...rest } = process.env;\nconst dyn = process.env[name];\n",
        )
        .unwrap();
        std::fs::write(d.join("job.py"), "import os\nkey = os.environ['STRIPE_KEY']\nx = os.getenv(\"HOME\")\n").unwrap();
        std::fs::write(d.join("run.sh"), "#!/bin/sh\necho $NODE_ENV ${K2_PORT:-0} $lower $1\n").unwrap();
        std::fs::write(d.join("package.json"), r#"{"scripts":{"start":"PORT=$APP_PORT node server.mjs"}}"#).unwrap();
        std::fs::create_dir_all(d.join("node_modules/dep")).unwrap();
        std::fs::write(d.join("node_modules/dep/index.js"), "process.env.SHOULD_NOT_SEE").unwrap();
        let r = scan_folder(&d, "node server.mjs --flag $EXTRA_FLAG");
        let names = r.env_names();
        for want in ["PORT", "K2_OWNER_TOKEN", "DATABASE_URL", "K2_HOST", "MODE", "STRIPE_KEY", "HOME", "NODE_ENV", "K2_PORT", "APP_PORT", "EXTRA_FLAG"] {
            assert!(names.contains(want), "{want} missing from {names:?}");
        }
        assert!(!names.contains("SHOULD_NOT_SEE"), "dependency folders are skipped");
        assert!(!names.contains("rest") && !names.contains("name"), "spreads and dynamic reads are not names");
        let t = r.env_reads.iter().find(|e| e.name == "K2_OWNER_TOKEN").unwrap();
        assert_eq!((t.file.as_str(), t.line), ("server.mjs", 2));
        assert_eq!(r.env_reads.iter().find(|e| e.name == "EXTRA_FLAG").unwrap().file, "<cmd>");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_form_encoded_posts_to_k2() {
        let d = tmp();
        std::fs::write(
            d.join("bff.ts"),
            "const K = process.env.K2_GATEWAY_URL;\n\
             // send to the room\n\
             await fetch(`${K}/cli/thread/post?token=${p}`, { method: 'POST',\n\
               headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body });\n\
             await fetch(`${K}/cli/feedback/create`, { method: 'POST', body });\n\
             await fetch(`${K}/cli/thread?addr=x`);\n",
        )
        .unwrap();
        // A form to someone else (an OAuth token endpoint) is not K2's business.
        std::fs::write(d.join("oauth.py"), "requests.post(url, headers={'Content-Type': 'application/x-www-form-urlencoded'})\n").unwrap();
        // K2's address and a form, no route literal: still named.
        std::fs::write(d.join("k2.py"), "BASE = os.environ['K2_PORT']\nH = {'content-type': 'Application/X-WWW-Form-Urlencoded'}\n").unwrap();
        let r = scan_folder(&d, "node bff.js");
        let mut got: Vec<(String, u32, Vec<String>)> = r.form_posts.iter().map(|f| (f.file.clone(), f.line, f.routes.clone())).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("bff.ts".to_string(), 4, vec!["/cli/thread/post".to_string(), "/cli/feedback/create".to_string()]),
                ("k2.py".to_string(), 2, vec![]),
            ],
            "GET routes are not writes; a form to a non-K2 URL isn't listed"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_k2_file_references() {
        let d = tmp();
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(
            d.join("lib/k2.ts"),
            "import os from 'os';\nconst port = readFileSync(`${os.homedir()}/.k2/heartbeat.port`);\nconst tok = readFileSync(home + '/.k2/daemon.token');\n",
        )
        .unwrap();
        let r = scan_folder(&d, "node lib/k2.js");
        let whats: Vec<(&str, u32)> = r.k2_paths.iter().map(|p| (p.what.as_str(), p.line)).collect();
        assert!(whats.contains(&("daemon_port_file", 2)), "{whats:?}");
        assert!(whats.contains(&("owner_token_file", 3)), "{whats:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The home DB under its 0.46.6 name is found; the SDK's `k2.db.*`
    /// calls are not a file reference.
    #[test]
    fn finds_k2_db_file_but_not_sdk_calls() {
        let d = tmp();
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(
            d.join("lib/sdk.ts"),
            "const {rows} = await k2.db.query({workspace, sql});\nawait k2.db({x});\n",
        )
        .unwrap();
        std::fs::write(
            d.join("lib/peek.ts"),
            "// nothing here\nconst db = new Database(home + '/.k2/k2.db');\n",
        )
        .unwrap();
        let r = scan_folder(&d, "node lib/peek.js");
        let found: Vec<(&str, &str, u32)> =
            r.k2_paths.iter().map(|p| (p.what.as_str(), p.file.as_str(), p.line)).collect();
        assert_eq!(found, vec![("k2_database", "lib/peek.ts", 2)], "{found:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn path_lookups_flag_programs_only_the_inherited_path_finds() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp();
        let private_bin = d.join("home/.local/bin");
        let sys_bin = d.join("usr/bin");
        std::fs::create_dir_all(&private_bin).unwrap();
        std::fs::create_dir_all(&sys_bin).unwrap();
        for (dir, name) in [(&private_bin, "node"), (&sys_bin, "sh"), (&private_bin, "npm")] {
            let p = dir.join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let app = d.join("app");
        std::fs::create_dir_all(app.join("node_modules/.bin")).unwrap();
        let next = app.join("node_modules/.bin/next");
        std::fs::write(&next, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(app.join("package.json"), r#"{"scripts":{"start":"next start && node worker.js"}}"#).unwrap();
        std::fs::write(app.join("bin.js"), "#!/usr/bin/env node\nconsole.log(1)\n").unwrap();
        let now = format!("{}:{}", private_bin.display(), sys_bin.display());
        let clean = sys_bin.display().to_string();
        let v = path_lookups("PORT=3000 exec npm start; ./bin.js && cd x && sh -c true", &app, &now, &clean);
        let progs: Vec<(&str, &str, bool)> = v.iter().map(|l| (l.program.as_str(), l.via.as_str(), l.breaks())).collect();
        assert!(progs.contains(&("npm", "cmd", true)), "{progs:?}");
        assert!(progs.contains(&("node", "npm script start", true)), "{progs:?}");
        assert!(progs.contains(&("node", "shebang ./bin.js", true)), "{progs:?}");
        assert!(progs.contains(&("sh", "cmd", false)), "{progs:?}");
        assert!(!progs.iter().any(|p| p.0 == "next"), "node_modules/.bin programs are local: {progs:?}");
        assert!(!progs.iter().any(|p| p.0 == "cd" || p.0 == "PORT=3000"), "{progs:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn shell_names_are_upper_case_only() {
        assert_eq!(shell_env_names("a $FOO ${BAR:-x} $baz $1 $Mixed ${_U}"), vec!["FOO", "BAR", "_U"]);
    }
}
