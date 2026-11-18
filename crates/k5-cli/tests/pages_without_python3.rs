//! The study and help pages the binary prints, with no `python3` on
//! `PATH` (K2's ruling: a native command runs where python3 is missing):
//! every study topic as text and as `--json`, the study list and usage,
//! every help form in `src/help_native.txt`, the daily tier (`k2`,
//! `k2 help`, `k2 --help`) and the other pages bash puts together
//! (`-h`, `help <word>`, the deprecated tier, chat's usage) print the
//! same bytes, on
//! the same stream, with the same exit as with `python3`. With none, the
//! `--json` pages are the binary's whatever the `PYTHON*` settings and the
//! locale say, as no Python reads them.
//!
//! The pages with permission cards and a passport, and the cards-only
//! topic, are `tests/study_*_cards_parity.rs`'s.
//!
//! Every run gets an empty environment but `PATH`, a temporary `HOME`
//! whose K2 home holds a port and a token (the shell's connection gate
//! wants both, even for help) and a temporary workspace as its folder.

#![cfg(unix)]

mod common;

use k5_cli::python::{self, Python3};
use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    no_python3: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("k5-pages-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("home");
        let k2home = k2_client::paths::k2_home_in(&home);
        let proj = dir.join("proj");
        for d in [&k2home, &proj.join(".k2"), &dir.join("tmp")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(k2home.join("heartbeat.port"), "1\n").unwrap();
        std::fs::write(k2home.join("heartbeat.token"), "owner-tok-6161\n").unwrap();
        let path = std::env::var("PATH").expect("a UTF-8 PATH");
        assert_eq!(
            python::in_env(&k5_cli::offline::Env::of(&[("PATH", &path)])),
            Python3::Present,
            "these tests compare against a run with python3: none on {path}"
        );
        let no_python3 = common::path_without_python3(&dir);
        Rig {
            dir,
            home,
            proj,
            no_python3,
        }
    }

    /// The binary on `args`, with `python3` on `PATH` or without, and
    /// `extra` set.
    fn run(&self, args: &[&str], with_python3: bool, extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(BIN);
        cmd.env_clear();
        if with_python3 {
            cmd.env("PATH", std::env::var_os("PATH").expect("a PATH"));
        } else {
            cmd.env("PATH", &self.no_python3);
        }
        cmd.env("HOME", &self.home)
            .env("TMPDIR", self.dir.join("tmp"))
            .envs(extra.iter().copied())
            .current_dir(&self.proj)
            .args(args);
        let o = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Every study topic (the aliases too), as `study <topic>` and
/// `study <topic> --json`.
fn study_lines() -> Vec<Vec<String>> {
    let mut topics: Vec<&str> = k5_cli::text::STUDY
        .iter()
        .map(|(k, _)| *k)
        // The cards-only topic is the shell's error without a passport
        // (handed over: tests/study_permissions_cards_parity.rs).
        .filter(|k| !k.starts_with('_') && *k != k5_cli::pages::CARDS_ONLY)
        .collect();
    topics.extend(["skins", "llm-accounts"]);
    topics.extend(k5_cli::pages::HEARTBEAT_ALIASES.iter().copied());
    let mut lines = vec![
        vec!["study".to_string()],
        vec!["study".to_string(), "list".to_string()],
        vec!["study".to_string(), "--help".to_string()],
    ];
    for t in topics {
        lines.push(vec!["study".to_string(), t.to_string()]);
        lines.push(vec![
            "study".to_string(),
            t.to_string(),
            "--json".to_string(),
        ]);
    }
    lines
}

#[test]
fn every_page_prints_the_same_without_python3() {
    let rig = Rig::new("same");
    let help: Vec<Vec<String>> = k5_cli::text::HELP_FORMS
        .iter()
        .map(|f| f.argv.iter().map(|w| w.to_string()).collect())
        .collect();
    assert!(help.len() > 100, "only {} help forms", help.len());
    let daily: Vec<Vec<String>> = k5_cli::pages::DAILY_FORMS
        .iter()
        .map(|f| f.iter().map(|w| w.to_string()).collect())
        .collect();
    let study = study_lines();
    for line in study.iter().chain(&help).chain(&daily) {
        let args: Vec<&str> = line.iter().map(String::as_str).collect();
        let with = rig.run(&args, true, &[]);
        let without = rig.run(&args, false, &[]);
        assert!(
            !without.stderr.contains("needs Python 3"),
            "{args:?} went to the shell with no python3: {without:?}"
        );
        assert_eq!(without, with, "{args:?}: with no python3, and with it");
    }
    // The help forms' own streams and codes, as the list says.
    for f in k5_cli::text::HELP_FORMS {
        let out = rig.run(f.argv, false, &[]);
        let page = k5_cli::text::page(f.page).expect("the form's page");
        let (stdout, stderr) = if f.stderr { ("", page) } else { (page, "") };
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(i32::from(f.code)), stdout, stderr),
            "{:?} with no python3",
            f.argv
        );
    }
    // The daily tier: no passport, so cards are off and nothing is asked.
    let tier = k5_cli::pages::daily_text(false).expect("the daily tier");
    for f in k5_cli::pages::DAILY_FORMS {
        let out = rig.run(f, false, &[]);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(0), tier.as_str(), ""),
            "{f:?} with no python3"
        );
    }
    println!(
        "{} study lines, {} help forms and the {} forms of the daily tier, the same with no python3",
        study.len(),
        help.len(),
        daily.len()
    );
}

/// The help pages bash puts together ([`k5_cli::pages::Composed`]): the
/// same bytes with no python3 as with it, and the pieces the binary puts
/// together for an owner's terminal (no passport: cards off).
#[test]
fn the_composed_pages_print_the_same_without_python3() {
    use k5_cli::pages::{self, Composed};
    let rig = Rig::new("composed");
    let tier = pages::daily_text(false).expect("the daily tier");
    let deprecated = pages::deprecated_text().expect("the deprecated tier");
    let chat = pages::chat_text(&k5_cli::offline::Env::of(&[])).expect("chat's usage");
    let lines: &[&[&str]] = &[
        &["-h"],
        &[""],
        &["help", "nope"],
        &["--help", ""],
        &["-h", "app"],
        &["help", "deprecated"],
        &["--help", "--deprecated"],
        &["help-deprecated"],
        &["chat"],
        &["chat", "--help"],
        &["chat", "-h"],
        &["chat", "help"],
        &["help", "chat"],
    ];
    for args in lines {
        let want = match pages::composed(args).expect("a composed form") {
            Composed::Daily => &tier,
            Composed::Deprecated => &deprecated,
            Composed::Chat => &chat,
        };
        let with = rig.run(args, true, &[]);
        let without = rig.run(args, false, &[]);
        assert_eq!(
            (
                without.code,
                without.stdout.as_str(),
                without.stderr.as_str()
            ),
            (Some(0), want.as_str(), ""),
            "{args:?} with no python3"
        );
        assert_eq!(without, with, "{args:?}: with no python3, and with it");
    }
    // `_CHAT_HELP_MANAGE` other than empty or 1: no manage piece.
    let off = rig.run(&["chat", "--help"], false, &[(pages::CHAT_MANAGE, "0")]);
    assert_eq!(
        off.stdout,
        pages::chat_text(&k5_cli::offline::Env::of(&[(pages::CHAT_MANAGE, "0")])).unwrap()
    );
    assert_ne!(off.stdout, chat);
}

#[test]
fn without_python3_the_json_pages_are_the_binary_s_whatever_python_s_settings_say() {
    let rig = Rig::new("json");
    let settings: &[&[(&str, &str)]] = &[
        &[("PYTHONIOENCODING", "latin-1")],
        &[("PYTHONPATH", "/nonexistent")],
        &[("LANG", "en_US.ISO8859-1")],
        &[("LC_ALL", "de_DE.ISO8859-1"), ("PYTHONUTF8", "0")],
    ];
    let mut non_ascii = 0;
    for line in study_lines()
        .iter()
        .filter(|l| l.last().is_some_and(|w| w == "--json"))
    {
        let args: Vec<&str> = line.iter().map(String::as_str).collect();
        let want = rig.run(&args, true, &[]);
        assert_eq!(want.code, Some(0), "{args:?}: {want:?}");
        assert!(want.stdout.starts_with("{\"id\":"), "{args:?}: {want:?}");
        if !want.stdout.is_ascii() || want.stdout.contains("\\u") {
            non_ascii += 1;
        }
        for extra in settings {
            assert_eq!(
                rig.run(&args, false, extra),
                want,
                "{args:?} with {extra:?} and no python3"
            );
        }
    }
    // Some pages have text a Latin-1 reader would read otherwise.
    assert!(non_ascii > 0, "no page has text past ASCII");
}
