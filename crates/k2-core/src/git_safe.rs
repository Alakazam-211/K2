//! One builder for every `git` process K2 itself runs (prd-security-sweep-0464-v1
//! SS15).
//!
//! K2 runs plain `git` inside workspace folders: worktree list/add/prune,
//! rev-parse, init + the first commit, status and diff for `k2 compute run
//! --dirty`, the retire guard. A workspace folder is outside content (a
//! clone, a copied or unpacked folder), and its `.git/config` can name
//! programs git runs on its own: `core.fsmonitor` on every status/diff,
//! hooks (`post-checkout` on `worktree add`, `pre-commit`/`commit-msg` on
//! commit), `diff.external` and textconv drivers on diff. Every K2-run git
//! call goes through [`git`] (or [`command`] when there is no repo), which
//! turns those off for that one process:
//!
//! - `-c core.fsmonitor=false`, `-c core.hooksPath=/dev/null` (`NUL` on
//!   Windows): no fsmonitor daemon, no hooks;
//! - `-c protocol.ext.allow=never`, `-c protocol.file.allow=user`: no
//!   `ext::` transport, and `file://`/local-path remotes only when the
//!   person runs git themselves (submodules a repo names can't pull a
//!   local path);
//! - `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, stdin closed, and an
//!   inherited `GIT_EXTERNAL_DIFF` dropped;
//! - [`NO_EXT_DIFF`] (`--no-ext-diff --no-textconv`) goes on every
//!   `diff`/`show`/`log -p` K2 runs. These are subcommand options, so the
//!   caller adds them after the subcommand.
//!
//! What it keeps: the person's global and system config (identity,
//! credentials, LFS filters, signing). It never sets `safe.directory`, so
//! git's own refusal of a repo owned by another user still applies.
//!
//! What it doesn't stop: **smudge/clean filters** still run when
//! `worktree add` checks files out. A filter from the person's global
//! config (e.g. LFS) is theirs and fine; a repo-local `filter.*` in
//! `.git/config` is outside code, which the risky-folder scanner
//! (`cli_folder_trust::scan`, SS8) reports. K2's worktree create needs the
//! checkout, so it can't use `--no-checkout` today; once the folder-trust
//! gate (SS9) exists, an undecided folder gets `--no-checkout`.
//!
//! Hooks are off only for K2's own calls. Agents' own `git` in their
//! terminal is unchanged.
//!
//! The grep test below fails on a bare `Command::new("git")` anywhere in
//! `crates/k2-core/src` or `crates/k2-daemon/src` outside test code and
//! this file. A line that must stay raw carries the marker
//! `git_safe: allow (<reason>)` on the same line or the line above.

use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(windows)]
const NULL_HOOKS: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const NULL_HOOKS: &str = "core.hooksPath=/dev/null";

/// The `-c` pairs every K2-run git gets, in order.
pub fn harden_args() -> [&'static str; 8] {
    [
        "-c",
        "core.fsmonitor=false",
        "-c",
        NULL_HOOKS,
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "protocol.file.allow=user",
    ]
}

/// Put after `diff`, `show` or `log -p`: no external diff program and no
/// textconv driver from the repo's config or `.gitattributes`.
pub const NO_EXT_DIFF: [&str; 2] = ["--no-ext-diff", "--no-textconv"];

/// A hardened `git` with no working directory set (global config, or a
/// caller that passes `-C`/`--git-dir` itself).
pub fn command() -> Command {
    let mut c = Command::new("git"); // git_safe: allow (the one builder)
    c.args(harden_args())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EXTERNAL_DIFF")
        .stdin(Stdio::null());
    c
}

/// A hardened `git` that runs in `dir`.
pub fn git(dir: impl AsRef<Path>) -> Command {
    let mut c = command();
    c.current_dir(dir.as_ref());
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-git-safe-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mkdir temp");
        d
    }

    /// Plain git for building fixtures (not K2's own path): isolated from
    /// the machine's config so the fixture is the only config in play.
    fn fixture_git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git") // git_safe: allow (test fixture)
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[cfg(unix)]
    fn write_exec(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).expect("write script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod script");
    }

    /// T13: a repo whose own config names an fsmonitor program, a
    /// post-checkout hook, a textconv driver and an external diff runs none
    /// of them through K2's `rev-parse`, `status`, `diff` and `worktree add`.
    #[cfg(unix)]
    #[test]
    fn repo_config_programs_do_not_run_through_git_safe() {
        let d = temp("t13");
        let repo = d.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        fixture_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.bin"), "one\n").expect("write a.bin");
        std::fs::write(repo.join(".gitattributes"), "*.bin diff=evil\n").expect("write attrs");
        fixture_git(&repo, &["add", "."]);
        fixture_git(&repo, &["commit", "-q", "-m", "base"]);
        std::fs::write(repo.join("a.bin"), "two\n").expect("change a.bin");

        let marks = d.join("marks");
        std::fs::create_dir_all(&marks).expect("mkdir marks");
        let prog = |name: &str| {
            let p = d.join(format!("{name}.sh"));
            write_exec(&p, &format!("#!/bin/sh\ntouch '{}'\n", marks.join(name).display()));
            p
        };
        let fsmon = prog("fsmonitor");
        let textconv = prog("textconv");
        let extdiff = prog("extdiff");
        let hooks = repo.join(".git/hooks");
        write_exec(&hooks.join("post-checkout"), &format!("#!/bin/sh\ntouch '{}'\n", marks.join("post-checkout").display()));
        fixture_git(&repo, &["config", "core.fsmonitor", fsmon.to_str().expect("utf8")]);
        fixture_git(&repo, &["config", "diff.evil.textconv", textconv.to_str().expect("utf8")]);
        fixture_git(&repo, &["config", "diff.external", extdiff.to_str().expect("utf8")]);

        // The fixture is armed: plain git runs the hook on worktree add and
        // the external diff on diff. (If this half fails, the test proves
        // nothing, so it fails loudly instead.)
        fixture_git(&repo, &["worktree", "add", "-q", "-b", "plain", d.join("wt-plain").to_str().expect("utf8")]);
        assert!(marks.join("post-checkout").exists(), "fixture: plain git must run post-checkout");
        let plain_diff = Command::new("git") // git_safe: allow (test fixture)
            .arg("-C")
            .arg(&repo)
            .args(["diff"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("plain diff");
        assert!(plain_diff.status.success(), "fixture: plain diff: {}", String::from_utf8_lossy(&plain_diff.stderr));
        assert!(marks.join("extdiff").exists(), "fixture: plain git must run diff.external");
        for m in ["post-checkout", "extdiff", "fsmonitor", "textconv"] {
            let _ = std::fs::remove_file(marks.join(m));
        }

        // K2's path.
        let run = |args: &[&str]| {
            let out = git(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .expect("run git_safe");
            assert!(out.status.success(), "git_safe {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        run(&["rev-parse", "--show-toplevel"]);
        let st = run(&["status", "--porcelain"]);
        assert!(st.contains("a.bin"), "status sees the change: {st:?}");
        let mut diff_args = vec!["diff"];
        diff_args.extend(NO_EXT_DIFF);
        let diff = run(&diff_args);
        assert!(diff.contains("-one") && diff.contains("+two"), "a real diff, not a textconv one: {diff:?}");
        run(&["worktree", "add", "-q", "-b", "k2", d.join("wt-k2").to_str().expect("utf8")]);
        assert!(d.join("wt-k2/a.bin").exists(), "worktree add still checks out");

        let ran: Vec<String> = std::fs::read_dir(&marks)
            .expect("read marks")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert!(ran.is_empty(), "repo-named programs ran through git_safe: {ran:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn command_carries_the_hardening_flags_first() {
        let c = git("/tmp");
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(&args[..8], &harden_args().map(String::from)[..]);
        assert_eq!(c.get_current_dir(), Some(Path::new("/tmp")));
        let envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string())))
            .collect();
        assert!(envs.contains(&("GIT_TERMINAL_PROMPT".into(), Some("0".into()))), "{envs:?}");
        assert!(envs.contains(&("GIT_OPTIONAL_LOCKS".into(), Some("0".into()))), "{envs:?}");
        assert!(envs.contains(&("GIT_EXTERNAL_DIFF".into(), None)), "{envs:?}");
        assert!(!args.iter().any(|a| a.starts_with("safe.directory")), "never safe.directory: {args:?}");
    }

    // ── grep test ────────────────────────────────────────────────────────

    const RAW: &str = concat!("Command::new(", "\"git\")");
    const MARK: &str = "git_safe: allow (";

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "tests") {
                    continue;
                }
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    fn is_test_file(p: &Path) -> bool {
        let name = p.file_name().expect("file name").to_string_lossy();
        name == "tests.rs" || name.ends_with("_tests.rs")
    }

    fn ident_byte(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_'
    }

    /// `r"…"`, `r#"…"#`, `br"…"` start at `i` (and not `r#ident`).
    fn raw_string_at(b: &[u8], i: usize) -> bool {
        if i > 0 && ident_byte(b[i - 1]) {
            let byte_prefix = b[i - 1] == b'b' && (i < 2 || !ident_byte(b[i - 2]));
            if !byte_prefix {
                return false;
            }
        }
        let mut j = i + 1;
        while b.get(j) == Some(&b'#') {
            j += 1;
        }
        b.get(j) == Some(&b'"')
    }

    fn utf8_len(first: Option<&u8>) -> usize {
        match first {
            Some(&c) if c >= 0xF0 => 4,
            Some(&c) if c >= 0xE0 => 3,
            Some(&c) if c >= 0x80 => 2,
            _ => 1,
        }
    }

    /// Byte offset just past the `}` closing the first block that opens at
    /// or after `start`. Comments, strings, raw strings and char literals
    /// are skipped, so a `"{"` in a test doesn't count. `None` = it never
    /// closes.
    fn block_end(src: &str, start: usize) -> Option<usize> {
        let b = src.as_bytes();
        let (mut i, mut depth, mut opened) = (start, 0i64, false);
        while i < b.len() {
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    let mut nest = 1;
                    i += 2;
                    while i < b.len() && nest > 0 {
                        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                            nest += 1;
                            i += 2;
                        } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                            nest -= 1;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    continue;
                }
                b'r' if raw_string_at(b, i) => {
                    let mut j = i + 1;
                    let mut hashes = 0;
                    while b[j] == b'#' {
                        hashes += 1;
                        j += 1;
                    }
                    j += 1;
                    loop {
                        if j >= b.len() {
                            return None;
                        }
                        if b[j] == b'"' && b.len() >= j + 1 + hashes && b[j + 1..j + 1 + hashes].iter().all(|&c| c == b'#') {
                            j += 1 + hashes;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                b'\'' => {
                    if b.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        while i < b.len() && b[i] != b'\'' {
                            i += 1;
                        }
                        i += 1;
                        continue;
                    }
                    let n = utf8_len(b.get(i + 1));
                    if b.get(i + 1 + n) == Some(&b'\'') {
                        i += 2 + n;
                        continue;
                    }
                    // A lifetime.
                }
                b'{' => {
                    depth += 1;
                    opened = true;
                }
                b'}' => {
                    depth -= 1;
                    if opened && depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Lines of `src` outside `#[cfg(test)] mod … { … }` blocks, with their
    /// 1-based numbers. A block that never closes panics, so a lexing miss
    /// can't silently hide production code.
    fn non_test_lines(file: &Path, src: &str) -> Vec<(usize, String)> {
        let mut starts = Vec::new();
        let mut off = 0;
        for l in src.split('\n') {
            starts.push(off);
            off += l.len() + 1;
        }
        let lines: Vec<&str> = src.split('\n').collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim().starts_with("#[cfg(test)]") {
                let mut j = i + 1;
                while j < lines.len() && (lines[j].trim().is_empty() || lines[j].trim().starts_with("#[")) {
                    j += 1;
                }
                let item = lines.get(j).map(|l| l.trim()).unwrap_or("");
                let inline_mod = (item.starts_with("mod ") || item.starts_with("pub mod ") || item.starts_with("pub(crate) mod ")) && item.contains('{');
                if inline_mod {
                    let end = block_end(src, starts[j]).unwrap_or_else(|| panic!("{}:{}: test module never closes", file.display(), j + 1));
                    let mut k = j;
                    while k < lines.len() && starts[k] < end {
                        k += 1;
                    }
                    i = k;
                    continue;
                }
            }
            out.push((i + 1, lines[i].to_string()));
            i += 1;
        }
        out
    }

    #[test]
    fn no_raw_git_command_outside_git_safe() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for root in ["k2-core/src", "k2-daemon/src"] {
            rs_files(&crates.join(root), &mut files);
        }
        assert!(files.len() > 100, "the scan found the sources ({} files)", files.len());
        let mut bad = Vec::new();
        let mut allowed = Vec::new();
        for f in &files {
            if is_test_file(f) || f.ends_with("k2-core/src/git_safe.rs") {
                continue;
            }
            let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            let kept = non_test_lines(f, &src);
            for (idx, (n, line)) in kept.iter().enumerate() {
                if !line.contains(RAW) {
                    continue;
                }
                let prev = if idx > 0 { kept[idx - 1].1.as_str() } else { "" };
                if line.contains(MARK) || prev.contains(MARK) {
                    allowed.push(format!("{}:{n}", f.display()));
                } else {
                    bad.push(format!("{}:{n}: {}", f.display(), line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "raw git commands outside crate::git_safe (use git_safe::git(dir) / git_safe::command(), or mark the line `git_safe: allow (<reason>)`):\n{}",
            bad.join("\n")
        );
        // The allow-list stays small and known: the two temp-bare-repo calls
        // in compute/sync.rs (SS15).
        assert_eq!(allowed.len(), 2, "allow-listed raw git lines changed: {allowed:?}");
        assert!(allowed.iter().all(|a| a.contains("compute/sync.rs")), "{allowed:?}");
    }

    #[test]
    fn non_test_lines_skips_inline_test_modules_only() {
        let src = concat!(
            "fn a() {}\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn b<'a>(x: &'a str) { let _ = (\"{\", '{', r#\"}}\"#, x); } // }\n",
            "    /* { */\n",
            "}\n",
            "fn c() {}\n",
            "#[cfg(test)]\n",
            "mod ext_tests;\n",
        );
        let kept: Vec<String> = non_test_lines(Path::new("x.rs"), src).into_iter().map(|(_, l)| l).collect();
        assert_eq!(kept, vec!["fn a() {}", "fn c() {}", "#[cfg(test)]", "mod ext_tests;", ""]);
        let never = "#[cfg(test)]\nmod tests {\n fn x() {\n";
        assert!(std::panic::catch_unwind(|| non_test_lines(Path::new("y.rs"), never)).is_err(), "an unclosed module panics");
    }
}
