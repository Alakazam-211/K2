//! The captured help forms (`src/help_captured.txt`, what bash printed for
//! each, written and proved by `tests/cli/help_native.sh`): the binary
//! prints exactly that for every form, with no `python3` on `PATH`, so a
//! hand-over to the shell can't pass (it ends with exit 69 and its line).
//! `tests/cli/help_native.sh` proves the list is what this checkout's bash
//! prints; `tests/cli/help_captured.sh` runs the binary beside bash too.
//!
//! The environments, as the script's: the owner's terminal (a port and a
//! token in the K2 home), and an agent in a K2 terminal (a passport, a
//! port and `K2_HOOK_SOCK` a live socket, no token files). The socket is
//! a stub daemon's that answers the permission cards probe (`GET
//! /cli/permission/enabled`): off on one socket, on on the other.
//!
//! A form the cards answer decides (a `C` row: its page goes through the
//! cards filter) prints its cards-off page for the owner (nothing asked)
//! and for an agent whose daemon says off, and its cards-on page for an
//! agent whose daemon says on, asking once each time with the run's own
//! passport. Every other form asks nothing. A form that prints chat's
//! usage (an `M` row) prints its other page while `_CHAT_HELP_MANAGE` is
//! set to anything but empty or `1`.
//!
//! The text names the command and the product by the brand: in a build
//! with `K2_BRAND_DIR=brand/xy` every captured page says `xy` and `XY`
//! where bash says `k2` and `K2` ([`the_pages_say_the_brand_s_name`],
//! [`the_card_pages_say_the_brand_s_name`]).

#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const LIST: &str = "src/help_captured.txt";
const WORKERS: usize = 8;
const CHAT_MANAGE: &str = "_CHAT_HELP_MANAGE";
const PROBE: &str = "/cli/permission/enabled";

/// A page of the list: its exit code and escaped text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Page {
    code: i32,
    out: String,
    err: String,
}

/// What picks a form's second page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Switch {
    /// The permission cards answer: the second page is the one with
    /// cards on.
    Cards,
    /// `_CHAT_HELP_MANAGE`: the second page is the one with it set to
    /// something other than empty or `1`.
    Manage,
}

/// One form of the list, with its page and, for a `C` or `M` row, its
/// second page.
#[derive(Debug, Clone)]
struct Form {
    argv: Vec<String>,
    page: Page,
    second: Option<(Switch, Page)>,
}

impl Form {
    fn cards(&self) -> bool {
        matches!(self.second, Some((Switch::Cards, _)))
    }
}

/// The list on disk, form by form.
fn forms() -> Vec<Form> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut pages: Vec<Page> = Vec::new();
    let mut out = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let cols: Vec<&str> = line.split('\t').collect();
        let page = |n: &str| pages[n.parse::<usize>().expect("a page number")].clone();
        let words = |argv: &str| argv.split(' ').map(str::to_string).collect();
        let form = match cols[..] {
            ["P", no, code, o, e] => {
                assert_eq!(no, pages.len().to_string(), "{line:?}");
                pages.push(Page {
                    code: code.parse().expect("a code"),
                    out: o.to_string(),
                    err: e.to_string(),
                });
                continue;
            }
            ["F", argv, p] => Form {
                argv: words(argv),
                page: page(p),
                second: None,
            },
            ["C", argv, off, on] => Form {
                argv: words(argv),
                page: page(off),
                second: Some((Switch::Cards, page(on))),
            },
            ["M", argv, p, other] => Form {
                argv: words(argv),
                page: page(p),
                second: Some((Switch::Manage, page(other))),
            },
            _ => panic!("{}: not a P, F, C or M row: {line:?}", path.display()),
        };
        out.push(form);
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

/// Where the binary's answer differs from `page`, if it does.
fn differs(argv: &[String], page: &Page, got: &Out) -> Option<String> {
    let want = Out {
        code: Some(page.code),
        stdout: render(&page.out),
        stderr: render(&page.err),
    };
    (*got != want).then(|| format!("{argv:?}:\n  want {want:?}\n  got  {got:?}"))
}

/// A stub daemon on a Unix socket: it answers every request with the
/// cards probe's answer and logs each request's route and passport.
struct Daemon {
    sock: PathBuf,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Daemon {
    fn start(sock: PathBuf, on: bool) -> Daemon {
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let log = Arc::clone(&log);
                std::thread::spawn(move || answer(conn, on, &log));
            }
        });
        Daemon { sock, seen }
    }

    /// The routes of the requests that carried `passport`.
    fn asked_with(&self, passport: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p)| p == passport)
            .map(|(r, _)| r.clone())
            .collect()
    }

    fn total(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

/// Reads one request's head, logs it, and answers `{"enabled":<on>}`.
fn answer(mut conn: UnixStream, on: bool, log: &Mutex<Vec<(String, String)>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match conn.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let route = head
        .split(' ')
        .nth(1)
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();
    let passport = head
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().trim_start_matches("Bearer ").to_string())
        })
        .unwrap_or_default();
    log.lock().unwrap().push((route, passport));
    let body = format!("{{\"enabled\":{on}}}");
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.write_all(reply.as_bytes());
}

/// Who runs a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Owner,
    /// An agent whose daemon says cards are on, or off.
    Agent { on: bool },
}

/// One run of a form: who, the settings on top, the page it must print
/// and the routes it must ask.
struct Run {
    who: Who,
    extra: Vec<(&'static str, &'static str)>,
    page: Page,
    asked: Vec<String>,
}

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    agent_home: PathBuf,
    proj: PathBuf,
    path: String,
    off: Daemon,
    on: Daemon,
    runs: AtomicUsize,
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
        // Short paths: a Unix socket's name has a small limit.
        let sock = |which: &str| {
            PathBuf::from(format!(
                "/tmp/k5cap-{tag}-{which}-{}.sock",
                std::process::id()
            ))
        };
        let path = common::path_without_python3(&dir);
        Rig {
            dir,
            home,
            agent_home,
            proj,
            path,
            off: Daemon::start(sock("off"), false),
            on: Daemon::start(sock("on"), true),
            runs: AtomicUsize::new(0),
        }
    }

    /// The binary on `argv` as `who`, with `extra` settings: its answer,
    /// and the routes its run's passport asked (none for the owner).
    fn run(&self, argv: &[String], who: Who, extra: &[(&str, &str)]) -> (Out, Vec<String>) {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .env("PATH", &self.path)
            .env("TMPDIR", self.dir.join("tmp"))
            .current_dir(&self.proj)
            .args(argv)
            .stdin(Stdio::null());
        let mut passport = None;
        match who {
            Who::Owner => {
                cmd.env("HOME", &self.home);
            }
            Who::Agent { on } => {
                // The run's own passport, so a request names its run.
                let p = format!("sessQ.run{}", self.runs.fetch_add(1, Ordering::SeqCst));
                let daemon = if on { &self.on } else { &self.off };
                cmd.env("HOME", &self.agent_home)
                    .env("K5_PORT", "4100")
                    .env("K2_HOOK_TOKEN", &p)
                    .env("K2_HOOK_SOCK", &daemon.sock);
                passport = Some((p, daemon));
            }
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let o = cmd
            .output()
            .unwrap_or_else(|e| panic!("run {argv:?}: {e}"));
        let out = Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        let asked = passport.map_or_else(Vec::new, |(p, d)| d.asked_with(&p));
        (out, asked)
    }

    /// The runs one form gets.
    fn cases(form: &Form) -> Vec<Run> {
        let run = |who, extra: &[(&'static str, &'static str)], page: &Page, probe: bool| Run {
            who,
            extra: extra.to_vec(),
            page: page.clone(),
            asked: if probe { vec![PROBE.to_string()] } else { vec![] },
        };
        let first = &form.page;
        let (off, on) = (Who::Agent { on: false }, Who::Agent { on: true });
        match &form.second {
            None => vec![
                run(Who::Owner, &[], first, false),
                run(off, &[], first, false),
                run(on, &[], first, false),
            ],
            Some((Switch::Cards, cards_on)) => vec![
                run(Who::Owner, &[], first, false),
                run(off, &[], first, true),
                run(on, &[], cards_on, true),
            ],
            Some((Switch::Manage, other)) => vec![
                run(Who::Owner, &[], first, false),
                run(Who::Owner, &[(CHAT_MANAGE, "1")], first, false),
                run(Who::Owner, &[(CHAT_MANAGE, "")], first, false),
                run(Who::Owner, &[(CHAT_MANAGE, "0")], other, false),
                run(off, &[], first, false),
                run(on, &[(CHAT_MANAGE, "no")], other, false),
            ],
        }
    }

    /// Every form's runs, `WORKERS` at a time: each run whose answer or
    /// requests differ, with how.
    fn failures(&self, forms: &[Form]) -> Vec<String> {
        let next = AtomicUsize::new(0);
        let bad = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..WORKERS {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(f) = forms.get(i) else { break };
                    for c in Rig::cases(f) {
                        let (got, asked) = self.run(&f.argv, c.who, &c.extra);
                        let (who, extra) = (c.who, &c.extra);
                        if let Some(d) = differs(&f.argv, &c.page, &got) {
                            bad.lock().unwrap().push(format!("{who:?} {extra:?}: {d}"));
                        }
                        if asked != c.asked {
                            bad.lock().unwrap().push(format!(
                                "{who:?} {extra:?}: {:?} asked {asked:?}, want {:?}",
                                f.argv, c.asked
                            ));
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
        let _ = std::fs::remove_file(&self.off.sock);
        let _ = std::fs::remove_file(&self.on.sock);
    }
}

#[test]
fn every_captured_form_prints_what_bash_printed() {
    let forms = forms();
    let cards = forms.iter().filter(|f| f.cards()).count();
    let manage = forms
        .iter()
        .filter(|f| matches!(f.second, Some((Switch::Manage, _))))
        .count();
    assert!(cards > 100, "only {cards} forms the cards answer decides");
    assert!(manage > 20, "only {manage} forms that print chat's usage");
    let rig = Rig::new("all");
    let bad = rig.failures(&forms);
    assert!(
        bad.is_empty(),
        "{} runs of {} forms differ:\n{}",
        bad.len(),
        forms.len(),
        bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
    // Every request was a cards form's probe: one per agent run of one.
    assert_eq!(rig.off.total() + rig.on.total(), 2 * cards);
}

/// The bite: a page changed by one byte, or a stream or code moved, fails
/// the check; so does a card line wrong in one variant, and the other
/// variant's page.
#[test]
fn a_changed_page_fails_the_check() {
    let forms = forms();
    let rig = Rig::new("bite");
    let f = forms
        .iter()
        .find(|f| !f.page.out.is_empty() && f.second.is_none())
        .expect("a form that prints on stdout");
    let (got, _) = rig.run(&f.argv, Who::Owner, &[]);
    assert_eq!(differs(&f.argv, &f.page, &got), None);
    let mut one_byte = f.page.clone();
    one_byte.out.push('x');
    let mut swapped = f.page.clone();
    std::mem::swap(&mut swapped.out, &mut swapped.err);
    let mut code = f.page.clone();
    code.code += 1;
    for bad in [one_byte, swapped, code] {
        assert!(differs(&f.argv, &bad, &got).is_some(), "{bad:?}");
    }

    // A form whose two pages differ by their card lines: each run prints
    // its own variant, not the other, and one card line taken out of
    // either variant fails it.
    let c = forms
        .iter()
        .find(|f| matches!(&f.second, Some((Switch::Cards, on)) if *on != f.page))
        .expect("a form whose cards pages differ");
    let Some((_, on)) = &c.second else {
        unreachable!()
    };
    let (got_off, asked_off) = rig.run(&c.argv, Who::Agent { on: false }, &[]);
    let (got_on, asked_on) = rig.run(&c.argv, Who::Agent { on: true }, &[]);
    assert_eq!(asked_off, vec![PROBE.to_string()]);
    assert_eq!(asked_on, vec![PROBE.to_string()]);
    assert_eq!(differs(&c.argv, &c.page, &got_off), None);
    assert_eq!(differs(&c.argv, on, &got_on), None);
    assert!(differs(&c.argv, on, &got_off).is_some(), "cards off printed the on page");
    assert!(differs(&c.argv, &c.page, &got_on).is_some(), "cards on printed the off page");
    for (page, got) in [(&c.page, &got_off), (on, &got_on)] {
        let wrong = without_a_card_line(page, &c.page, on);
        assert!(differs(&c.argv, &wrong, got).is_some(), "{wrong:?}");
    }
}

/// `page` with its first line that is in only one of the two variants
/// taken out: a card line wrong.
fn without_a_card_line(page: &Page, off: &Page, on: &Page) -> Page {
    let lines = |p: &Page| -> Vec<String> {
        render(&format!("{}{}", p.out, p.err))
            .lines()
            .map(str::to_string)
            .collect()
    };
    let (a, b) = (lines(off), lines(on));
    let only_one: Vec<&String> = a
        .iter()
        .filter(|l| !b.contains(l))
        .chain(b.iter().filter(|l| !a.contains(l)))
        .collect();
    let mut wrong = page.clone();
    for text in [&mut wrong.out, &mut wrong.err] {
        let rendered = render(text);
        if let Some(line) = only_one.iter().find(|l| rendered.lines().any(|r| r == l.as_str())) {
            // The escaped text holds the line escaped: drop it from the
            // rendered text and keep that as the page (no escapes left
            // but the brand's, which render again as they are).
            let kept: String = rendered
                .split_inclusive('\n')
                .filter(|r| r.trim_end_matches('\n') != line.as_str())
                .collect();
            *text = kept.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t");
            return wrong;
        }
    }
    panic!("no line in only one variant in {page:?}");
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

/// `text` names the command and the product where the escaped `want`
/// marks them, by the brand, and never by a typed `k2` or `K2` in a
/// build with another name.
fn says_the_brand(argv: &[String], want: &str, text: &str) {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    assert!(
        standalone(text, cli, cli_after) >= marks(want, 'c'),
        "{argv:?}: {cli} is not where bash named the command:\n{text}"
    );
    assert!(
        standalone(text, product, product_after) >= marks(want, 'p'),
        "{argv:?}: {product} is not where bash named the product:\n{text}"
    );
    if cli != "k2" {
        assert_eq!(
            standalone(text, "k2", cli_after),
            0,
            "{argv:?}: a typed k2:\n{text}"
        );
        assert_eq!(
            standalone(text, "K2", product_after),
            0,
            "{argv:?}: a typed K2:\n{text}"
        );
    }
}

/// The pages name the command and the product by the brand (in a build
/// with `K2_BRAND_DIR=brand/xy` they say `xy` and `XY`), never a typed
/// `k2` or `K2` where bash named them.
#[test]
fn the_pages_say_the_brand_s_name() {
    let forms = forms();
    let rig = Rig::new("brand");
    let named: Vec<&Form> = forms
        .iter()
        .filter(|f| marks(&f.page.out, 'c') > 0 && marks(&f.page.out, 'p') > 0)
        .collect();
    assert!(named.len() > 10, "only {} pages name both", named.len());
    for f in named.iter().step_by((named.len() / 50).max(1)) {
        let (got, _) = rig.run(&f.argv, Who::Owner, &[]);
        says_the_brand(&f.argv, &f.page.out, &got.stdout);
    }
}

/// The pages the cards answer decides say the brand's name both ways:
/// the card lines (`k2 permission request …`) too.
#[test]
fn the_card_pages_say_the_brand_s_name() {
    let forms = forms();
    let rig = Rig::new("brandcards");
    let cards: Vec<&Form> = forms.iter().filter(|f| f.cards()).collect();
    assert!(cards.len() > 10, "only {} cards forms", cards.len());
    for f in cards.iter().step_by((cards.len() / 25).max(1)) {
        let Some((_, on)) = &f.second else {
            unreachable!()
        };
        for (page, who) in [(&f.page, Who::Agent { on: false }), (on, Who::Agent { on: true })] {
            let (got, _) = rig.run(&f.argv, who, &[]);
            assert!(marks(&page.err, 'c') + marks(&page.out, 'c') > 0, "{:?}", f.argv);
            says_the_brand(&f.argv, &page.err, &got.stderr);
            says_the_brand(&f.argv, &page.out, &got.stdout);
        }
    }
}
