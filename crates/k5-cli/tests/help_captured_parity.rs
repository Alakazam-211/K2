//! The captured help forms (`src/help_captured.txt`, what bash printed for
//! each, written and proved by `tests/cli/help_native.sh`): the binary
//! prints exactly that for every form, with no `python3` on `PATH`, so a
//! hand-over to the shell can't pass (it ends with exit 69 and its line).
//! `tests/cli/help_native.sh` proves the list is what this checkout's bash
//! prints; `tests/cli/help_text.sh` runs the binary beside bash for every
//! form too.
//!
//! Two environments, as the script's A and C: the owner's terminal (a
//! port and a token in the K2 home), and an agent in a K2 terminal (a
//! passport, a port and `K2_HOOK_SOCK` a live socket, no token files).
//!
//! The text names the command and the product by the brand: in a build
//! with `K2_BRAND_DIR=brand/xy` every captured page says `xy` and `XY`
//! where bash says `k2` and `K2` ([`the_pages_say_the_brand_s_name`]).

#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const LIST: &str = "src/help_captured.txt";
const WORKERS: usize = 8;

/// One form of the list, with its page's code and escaped text.
#[derive(Debug, Clone)]
struct Form {
    argv: Vec<String>,
    code: i32,
    out: String,
    err: String,
}

/// The list on disk, form by form.
fn forms() -> Vec<Form> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut pages: Vec<(i32, String, String)> = Vec::new();
    let mut out = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let cols: Vec<&str> = line.split('\t').collect();
        match cols[..] {
            ["P", no, code, o, e] => {
                assert_eq!(no, pages.len().to_string(), "{line:?}");
                pages.push((code.parse().expect("a code"), o.to_string(), e.to_string()));
            }
            ["F", argv, page] => {
                let (code, o, e) = pages[page.parse::<usize>().expect("a page number")].clone();
                out.push(Form {
                    argv: argv.split(' ').map(str::to_string).collect(),
                    code,
                    out: o,
                    err: e,
                });
            }
            _ => panic!("{}: not a P or an F row: {line:?}", path.display()),
        }
    }
    assert!(out.len() > 1000, "only {} captured forms", out.len());
    out
}

/// The escaped text as bash printed it, the names by the brand. The
/// test's own reading of the escapes, not the binary's.
fn render(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next().expect("an escape has a letter") {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            'c' => out.push_str(k2_client::brand::CLI_NAME),
            'p' => out.push_str(k2_client::brand::PRODUCT_NAME),
            other => panic!("an escape the list doesn't have: \\{other}"),
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Where the binary's answer differs from what the list says, if it does.
fn differs(form: &Form, got: &Out) -> Option<String> {
    let want = Out {
        code: Some(form.code),
        stdout: render(&form.out),
        stderr: render(&form.err),
    };
    (*got != want).then(|| format!("{:?}:\n  want {want:?}\n  got  {got:?}", form.argv))
}

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    agent_home: PathBuf,
    proj: PathBuf,
    sock: PathBuf,
    path: String,
    _listener: std::os::unix::net::UnixListener,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("k5-captured-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        let agent_home = dir.join("agent-home");
        let k2home = k2_client::paths::k2_home_in(&home);
        let proj = dir.join("proj");
        for d in [
            &k2home,
            &k2_client::paths::k2_home_in(&agent_home),
            &proj.join(".k2"),
            &dir.join("tmp"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(k2home.join("heartbeat.port"), "1\n").unwrap();
        std::fs::write(k2home.join("heartbeat.token"), "owner-tok-5151\n").unwrap();
        // A short path: a Unix socket's name has a small limit.
        let sock = PathBuf::from(format!("/tmp/k5cap-{tag}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let path = common::path_without_python3(&dir);
        Rig {
            dir,
            home,
            agent_home,
            proj,
            sock,
            path,
            _listener: listener,
        }
    }

    /// The binary on `argv`: the owner's terminal, or an agent's.
    fn run(&self, argv: &[String], agent: bool) -> Out {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .env("PATH", &self.path)
            .env("TMPDIR", self.dir.join("tmp"))
            .current_dir(&self.proj)
            .args(argv)
            .stdin(Stdio::null());
        if agent {
            cmd.env("HOME", &self.agent_home)
                .env("K5_PORT", "4100")
                .env("K2_HOOK_TOKEN", "sessQ.notreal1")
                .env("K2_HOOK_SOCK", &self.sock);
        } else {
            cmd.env("HOME", &self.home);
        }
        let o = cmd.output().unwrap_or_else(|e| panic!("run {argv:?}: {e}"));
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// Every form in both environments, `WORKERS` at a time: the forms
    /// whose answer differs, with how.
    fn failures(&self, forms: &[Form]) -> Vec<String> {
        let next = AtomicUsize::new(0);
        let bad = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..WORKERS {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(f) = forms.get(i) else { break };
                    for agent in [false, true] {
                        if let Some(d) = differs(f, &self.run(&f.argv, agent)) {
                            let who = if agent { "agent" } else { "owner" };
                            bad.lock().unwrap().push(format!("{who}: {d}"));
                        }
                    }
                });
            }
        });
        let mut bad = bad.into_inner().unwrap();
        bad.sort();
        bad
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.sock);
    }
}

#[test]
fn every_captured_form_prints_what_bash_printed() {
    let forms = forms();
    let rig = Rig::new("all");
    let bad = rig.failures(&forms);
    assert!(
        bad.is_empty(),
        "{} of {} forms differ:\n{}",
        bad.len(),
        forms.len(),
        bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// The bite: a page changed by one byte, or a stream or code moved, fails
/// the check.
#[test]
fn a_changed_page_fails_the_check() {
    let forms = forms();
    let rig = Rig::new("bite");
    let f = forms
        .iter()
        .find(|f| !f.out.is_empty())
        .expect("a form that prints on stdout");
    let got = rig.run(&f.argv, false);
    assert_eq!(differs(f, &got), None);
    let mut one_byte = f.clone();
    one_byte.out.push('x');
    let mut swapped = f.clone();
    std::mem::swap(&mut swapped.out, &mut swapped.err);
    let mut code = f.clone();
    code.code += 1;
    for bad in [one_byte, swapped, code] {
        assert!(differs(&bad, &got).is_some(), "{bad:?}");
    }
}

/// How often `word` is a word of its own in `text`, as the list's script
/// marks the names: nothing before it that a name could go on with, and
/// `after` true of the text after it.
fn standalone(text: &str, word: &str, after: fn(&str) -> bool) -> usize {
    text.match_indices(word)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            !before.is_some_and(glue) && after(&text[at + word.len()..])
        })
        .count()
}

/// How many `\\<letter>` escapes an escaped text has.
fn marks(text: &str, letter: char) -> usize {
    let mut n = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.next() == Some(letter) {
            n += 1;
        }
    }
    n
}

/// A character a name goes on with (`k2.dev`, `K2_HOME`, `~/.k2`).
fn glue(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./~|<:-".contains(c)
}

/// A character that goes on after a `.` or `:` in a name (`K2.app`).
fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_/".contains(c)
}

/// After the command's name: a space.
fn cli_after(rest: &str) -> bool {
    rest.starts_with(' ')
}

/// After the product's name: nothing it goes on with, or a `.` or `:`
/// that ends a sentence or a label.
fn product_after(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        Some('.' | ':') => !chars.next().is_some_and(word_char),
        Some(c) => !glue(c),
        None => true,
    }
}

/// The pages name the command and the product by the brand (in a build
/// with `K2_BRAND_DIR=brand/xy` they say `xy` and `XY`), never a typed
/// `k2` or `K2` where bash named them.
#[test]
fn the_pages_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    let forms = forms();
    let rig = Rig::new("brand");
    let named: Vec<&Form> = forms
        .iter()
        .filter(|f| marks(&f.out, 'c') > 0 && marks(&f.out, 'p') > 0)
        .collect();
    assert!(named.len() > 10, "only {} pages name both", named.len());
    for f in named.iter().step_by((named.len() / 50).max(1)) {
        let got = rig.run(&f.argv, false);
        let cli_marks = marks(&f.out, 'c');
        assert!(
            standalone(&got.stdout, cli, cli_after) >= cli_marks,
            "{:?}: {cli} is not where bash named the command:\n{}",
            f.argv,
            got.stdout
        );
        assert!(
            standalone(&got.stdout, product, product_after) >= marks(&f.out, 'p'),
            "{:?}: {product} is not where bash named the product:\n{}",
            f.argv,
            got.stdout
        );
        if cli != "k2" {
            assert_eq!(
                standalone(&got.stdout, "k2", cli_after),
                0,
                "{:?}: a typed k2:\n{}",
                f.argv,
                got.stdout
            );
            assert_eq!(
                standalone(&got.stdout, "K2", product_after),
                0,
                "{:?}: a typed K2:\n{}",
                f.argv,
                got.stdout
            );
        }
    }
}
