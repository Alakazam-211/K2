//! The Codex tab gate (`_cli_codex_tab_gate`; the binary's
//! [`k5_cli::codex_gate`]) on `cli/k2` and the binary, against one
//! logging stub daemon each ([`common::parity`]). A Codex call
//! (`CODEX_THREAD_ID`) in a K2 session with no tab marker, or another
//! tab's, sends nothing: native families, the front's own pages (`cli
//! status`, the help tiers, a retired verb's page) and the daily tier,
//! on the cell socket and with a pane id alone, with the owner token on
//! disk too. Help, schema, version and explain run, a help flag prints
//! `help <verb>`, and the daily tier asks nothing (the cards off, agents'
//! reports on). The marker that names the passport's session lets the
//! call through. `K2_CODEX_NO_DAEMON` is only words in the refusal.
//!
//! Each refused row is the binary's own answer (it did not hand the line
//! to the shell); a row where the front can't tell the shell's earlier
//! steps pass is the shell's, and the refusal there proves the line
//! reached the shell's gate.

#![cfg(unix)]

mod common;

use common::parity::{
    case, codex_walk_needs_python3, owner_and_port, with_passport, Case, Compare, Rig, Side, OWNER,
    PASSPORT,
};

const REFUSED_HEAD: &str = "Error: this Codex tab was started before this update or outside K2";

/// The passport's session: a matching tab marker.
fn session() -> &'static str {
    PASSPORT.split_once('.').expect("session.secret").0
}

/// The settings of a row.
#[derive(Clone, Copy, Debug)]
enum Env {
    /// Passport, cell socket, a Codex call, no marker.
    Sock,
    /// Passport, a pane id and no socket, a Codex call, no marker.
    Pane,
    /// The cell socket and a Codex call, no passport (the owner token is
    /// on disk).
    SockNoPassport,
    /// [`Env::Sock`] with another tab's marker.
    OtherMarker,
    /// [`Env::Sock`] with `K2_CODEX_NO_DAEMON=1` set here: only words.
    NoDaemonSet,
    /// [`Env::Sock`] and a daemon host setting: the shell's plain-http
    /// step comes first, so the binary hands the line to the shell.
    Host,
    /// [`Env::Sock`] with the matching marker: the call goes through.
    Matching,
    /// [`Env::Pane`] with the matching marker.
    PaneMatching,
}

impl Env {
    fn vars(self) -> Vec<(&'static str, String)> {
        let codex = ("CODEX_THREAD_ID", "t1");
        match self {
            Env::Sock => with_passport(&[("K2_HOOK_SOCK", "{sock}"), codex]),
            Env::Pane => with_passport(&[("K2_PANE_ID", "p1"), codex]),
            Env::SockNoPassport => vec![
                ("K2_HOOK_SOCK", "{sock}".to_string()),
                ("CODEX_THREAD_ID", "t1".to_string()),
            ],
            Env::OtherMarker => with_passport(&[
                ("K2_HOOK_SOCK", "{sock}"),
                codex,
                ("K2_CODEX_TAB_ISOLATED", "sessOther"),
            ]),
            Env::NoDaemonSet => with_passport(&[
                ("K2_HOOK_SOCK", "{sock}"),
                codex,
                ("K2_CODEX_NO_DAEMON", "1"),
            ]),
            Env::Host => {
                with_passport(&[("K2_HOOK_SOCK", "{sock}"), codex, ("K2_HOST", "127.0.0.1")])
            }
            Env::Matching => with_passport(&[
                ("K2_HOOK_SOCK", "{sock}"),
                codex,
                ("K2_CODEX_TAB_ISOLATED", session()),
            ]),
            Env::PaneMatching => with_passport(&[
                ("K2_PANE_ID", "p1"),
                codex,
                ("K2_CODEX_TAB_ISOLATED", session()),
            ]),
        }
    }
}

/// How the binary answers a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum By {
    /// Itself: it did not hand the line to the shell.
    Binary,
    /// The shell.
    Shell,
}

/// Runs one row on both sides and checks what the binary did. `owner`:
/// the owner token is on disk too (with the port), else only the port.
fn row(
    verb: &'static str,
    args: &'static [&'static str],
    env: Env,
    owner: bool,
    code: i32,
    requests: usize,
    by: By,
) {
    let tag = format!("{env:?}-{owner}").to_lowercase();
    let rig = Rig::new(verb, &tag);
    let setup: fn(&Rig) = if owner { owner_and_port } else { port_on_disk };
    let c = Case {
        env: env.vars(),
        compare: if by == By::Shell {
            Compare::Shell
        } else {
            Compare::Exact
        },
        ..case("codex gate", args, &[], vec![], setup, code, requests)
    };
    rig.check(&c);
    // The last run had no python3 on PATH: a line handed to the shell
    // ends there with the missing-Python line (the rig holds it to that),
    // a line the binary answers prints what it printed with one.
    let words: Vec<std::ffi::OsString> = rig
        .argv(&c)
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect();
    let no_python = k5_cli::fallthrough::FrontError::NoPython {
        noun: k5_cli::fallthrough::noun(&words),
    }
    .to_string();
    // (A matching marker's walk is python3's where ps can't give it,
    // except on macOS, where the binary walks itself.)
    let handed = String::from_utf8_lossy(&rig.raw(Side::NoPython3).1).contains(&no_python);
    assert_eq!(
        handed,
        by == By::Shell || codex_walk_needs_python3(&c.env),
        "{verb} {args:?} {env:?}: handed to the shell"
    );
    let (bash_err, rust_err) = (rig.raw(Side::Bash).1, rig.raw(Side::Rust).1);
    if code == 3 {
        let bash_err = String::from_utf8_lossy(&bash_err);
        assert!(
            bash_err.starts_with(REFUSED_HEAD),
            "{verb} {args:?} {env:?}: bash's refusal: {bash_err}"
        );
        assert_eq!(
            bash_err,
            String::from_utf8_lossy(&rust_err),
            "{verb} {args:?} {env:?}: the refusal"
        );
    }
    // The binary alone once more: what reached its stub, and never the
    // owner token.
    let (_, sent) = rig.binary(&c, false);
    assert_eq!(sent.len(), requests, "{verb} {args:?} {env:?}: {sent:?}");
    assert!(
        sent.iter().all(|r| !r.contains(OWNER)),
        "{verb} {args:?} {env:?}: the owner token was sent: {sent:?}"
    );
}

fn port_on_disk(r: &Rig) {
    r.port_on_disk();
}

/// A command line refused under every unconfirmed setting, with and
/// without the owner token on disk.
fn refused(verb: &'static str, args: &'static [&'static str]) {
    for env in [Env::Sock, Env::Pane, Env::OtherMarker, Env::NoDaemonSet] {
        for owner in [false, true] {
            row(verb, args, env, owner, 3, 0, By::Binary);
        }
    }
    // No passport: only the owner token is there to send.
    row(verb, args, Env::SockNoPassport, true, 3, 0, By::Binary);
    // The shell's earlier step first: the binary hands it over, and the
    // shell's gate refuses it.
    row(verb, args, Env::Host, true, 3, 0, By::Shell);
}

// The probe's commands: native families.

#[test]
fn whoami() {
    refused("whoami", &[]);
    refused("whoami", &["--json"]);
}

#[test]
fn inbox_and_msg() {
    refused("inbox", &[]);
    refused("msg", &["someone", "hi"]);
}

#[test]
fn activity_and_feedback() {
    refused("activity", &[]);
    refused("feedback", &["list"]);
}

#[test]
fn workspace_wiki_and_notifications() {
    refused("workspace", &["list"]);
    refused("wiki", &["index"]);
    refused("notifications", &[]);
}

// The front's own pages.

#[test]
fn the_front_s_pages() {
    refused("cli", &["status"]);
    refused("status", &[]);
    refused("help-advanced", &[]);
    refused("help-internal", &[]);
    refused("help-deprecated", &[]);
    refused("study", &[]);
}

/// The words the gate lets by: nothing asked or sent, with the owner
/// token on disk too.
#[test]
fn help_words_ask_nothing() {
    for env in [Env::Sock, Env::Pane] {
        for owner in [false, true] {
            // The daily tier, which asks the daemon twice outside the gate.
            row("", &[], env, owner, 0, 0, By::Binary);
            row("help", &[], env, owner, 0, 0, By::Binary);
            row("--help", &[], env, owner, 0, 0, By::Binary);
            row("help", &["msg"], env, owner, 0, 0, By::Binary);
            row("--version", &[], env, owner, 0, 0, By::Binary);
            // A help flag after the first word: `help <word>`.
            row("whoami", &["--help"], env, owner, 0, 0, By::Binary);
            row("help-advanced", &["x", "-h"], env, owner, 0, 0, By::Binary);
            // schema and explain are the shell's.
            row("schema", &[], env, owner, 0, 0, By::Shell);
            // (explain of an id it has no refusal for: exit 4.)
            row("explain", &["whoami"], env, owner, 4, 0, By::Shell);
        }
    }
}

/// `explain` with a Markdown kit id asks the daemon: refused.
#[test]
fn explain_a_kit_id() {
    for env in [Env::Sock, Env::Pane] {
        row("explain", &["MPC-12"], env, true, 3, 0, By::Binary);
    }
}

/// The matching marker: the call goes through as it does outside Codex.
#[test]
fn the_matching_marker_lets_it_by() {
    row("whoami", &[], Env::Matching, false, 0, 1, By::Binary);
    row("wiki", &["index"], Env::Matching, false, 0, 1, By::Binary);
    row(
        "wiki",
        &["index"],
        Env::PaneMatching,
        false,
        0,
        1,
        By::Binary,
    );
}
