//! The study and help pages the binary prints itself, from the embedded
//! files ([`crate::text`]), byte for byte what the shell CLI prints.
//!
//! Two steps. [`study`] and [`crate::text::help_form`] read only the
//! arguments: they say which page a command line is, when it is one the
//! binary may answer. [`study_page`] and [`help_page`] then check the
//! shell's environment ([`crate::offline`]): when the shell would print
//! anything else first, stop, or ask the daemon in a way the binary
//! doesn't (the permission card probe a passport makes), they give
//! `None` and the command falls through to the shell.
//!
//! A family's help in a session with `HOOK_SOCK` set runs the shell's
//! session checks first ([`session_checks`]): with a passport and a live
//! socket they pass, and the page prints.
//!
//! The daily tier of the help (`k2`, `k2 help`, `k2 --help`:
//! [`daily_page`]) is its files: send-feedback's block unless an agent's
//! server says agents may not report ([`crate::feedback_optin`]), and one
//! line while permission cards are on ([`crate::cards`]), each asked of
//! the daemon with a passport; its switch is [`DAILY_NATIVE`]. The other
//! help pages bash puts together ([`Composed`]: the same tier for `-h`
//! and `help <word with no arm>`, the deprecated tier, chat's usage) have
//! [`COMPOSED_NATIVE`]; the router's `*)` arm for a word it has no arm
//! for ([`unknown_page`]: a retired verb's line, or `Unknown command` and
//! the daily tier) has [`UNKNOWN_NATIVE`].
//!
//! `k2 study` (`cli/src/study-command.sh`): its arguments are read in
//! order, as the shell reads them. The binary answers the list (`study`,
//! `study list`), the usage (`--help` or `-h` anywhere after the errors
//! the shell reports first), and every topic with a page under
//! `cli/text/study`, as text or `--json`. The shell keeps: the list as
//! JSON (it needs the titles, which live in the shell), the topics still
//! written in bash (`app-api`, `app-errors`, `live`, no file), an unknown
//! topic, and every error.
//!
//! With a passport, the list, the usage, the apps page (and `skins`) and
//! the cards-only `permissions` topic depend on whether permission cards
//! are on, which the shell asks the daemon once ([`crate::cards`]). Each
//! of those has a switch ([`LIST_WITH_PASSPORT`], [`APPS_WITH_PASSPORT`],
//! [`PERMISSIONS_WITH_PASSPORT`]): on, the binary asks the same way and
//! prints the shell's page for the answer; off, the shell answers. Where
//! the shell's answer for `permissions` is its error (cards off), the
//! binary prints that error itself: the shell would ask again.
//!
//! With a passport, `send-feedback`'s page is one line while an agent's
//! server says agents may not report ([`crate::feedback_optin`], its
//! switch [`crate::feedback_optin::FEEDBACK_HIDE_NATIVE`]).

use crate::exit::Exit;
use crate::offline::{Env, Prelude};
use crate::out::json::{self, Value};
use crate::python::{self, Python3};
use crate::text::{self, HelpForm};
use crate::{cards, feedback_optin, prelude, tools};
use std::borrow::Cow;
use std::path::Path;

/// What a page prints, where, and the exit after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub text: Cow<'static, str>,
    pub stderr: bool,
    pub exit: Exit,
    /// Written to stderr before `text` (the unknown command's line).
    pub lead: Option<String>,
}

/// A `k2 study` command line the binary may answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Study {
    /// `--help` or `-h`: `_study_usage_all`.
    Usage,
    /// No topic, or `list`, without `--json`: `_study_catalog_all`.
    Catalog,
    /// A topic's page (after the heartbeats aliases), as text or JSON.
    Page { topic: &'static str, json: bool },
}

/// What people call heartbeats (`cmd_study`): each is the heartbeats page.
pub const HEARTBEAT_ALIASES: &[&str] = &["routines", "routine", "reminders", "reminder", "cron"];

/// `k2 study skins` is the apps page after this line and a blank one.
pub const SKINS_LINE: &str = "k2 study skins is now k2 study apps \u{2014} this page still works.";

/// `k2 study llm-accounts` is the llm-tokens page after this line and a
/// blank one.
pub const LLM_ACCOUNTS_LINE: &str =
    "k2 study llm-accounts is now k2 study llm-tokens \u{2014} this page still works.";

/// The page `cards` filters (`_study_page_skins ... | _perm_lines`).
pub const CARDS_PAGE: &str = "apps";

/// The topic only shown while permission cards are on.
pub const CARDS_ONLY: &str = "permissions";

/// The topic that is one line while agents may not report
/// ([`feedback_optin::OFF_LINE`]).
pub const FEEDBACK_PAGE: &str = "send-feedback";

/// The list's line for [`CARDS_ONLY`] while cards are on, after each line
/// that starts `  design   ` (`_study_catalog_all`'s awk).
pub const CARDS_CATALOG_LINE: &str = concat!(
    "  permissions         Permission cards: ask a person to allow what ",
    k2_client::product_name!(),
    " refused you"
);

/// The usage's last line while cards are on (`_study_usage_all`).
pub const CARDS_USAGE_LINE: &str = "  permissions";

/// With a passport, `k2 study`, `k2 study list` and `k2 study --help` ask
/// the daemon whether cards are on. True: the binary asks the same way
/// and prints the list or the usage for the answer. False: the shell
/// answers them.
pub const LIST_WITH_PASSPORT: bool = true;

/// With a passport, `k2 study apps` and `k2 study skins` (text or
/// `--json`) ask the daemon whether cards are on, for the apps page's
/// `#if-cards` lines. True: the binary asks the same way and prints the
/// page for the answer. False: the shell answers them.
pub const APPS_WITH_PASSPORT: bool = true;

/// With a passport, `k2 study permissions` asks the daemon whether cards
/// are on: on, its page (text or `--json`); off, an unknown topic. True:
/// the binary asks the same way and prints the page or the shell's error
/// for the answer. False: the shell answers it. Without a passport it is
/// always the shell's error.
pub const PERMISSIONS_WITH_PASSPORT: bool = true;

/// The study command line in `rest` (the arguments after `study`), when
/// the binary may answer it.
pub fn study(rest: &[&str]) -> Option<Study> {
    let mut json = false;
    let mut topic = "";
    for arg in rest {
        match *arg {
            "--help" | "-h" => return Some(Study::Usage),
            "--json" => json = true,
            // An unknown flag, then a second topic: the shell's errors.
            a if a.starts_with("--") => return None,
            _ if !topic.is_empty() => return None,
            a => topic = a,
        }
    }
    if topic.is_empty() || topic == "list" {
        return (!json).then_some(Study::Catalog);
    }
    let topic = if HEARTBEAT_ALIASES.contains(&topic) {
        "heartbeats"
    } else {
        topic
    };
    let topic = match topic {
        "skins" => "skins",
        "llm-accounts" => "llm-accounts",
        t if t.starts_with('_') => return None,
        t => text::STUDY.iter().find(|(k, _)| *k == t)?.0,
    };
    Some(Study::Page { topic, json })
}

/// `_perm_lines` with cards off: a line ending ` #if-cards` is dropped, a
/// line ending ` #if-no-cards` loses the tag, every line ends in a newline.
pub fn cards_off(text: &str) -> String {
    cards::perm_lines(text, false)
}

/// The page text of a study topic, cards `on` or off: the file, the apps
/// page through the card filter, and the two aliases with their line
/// first.
fn study_text(topic: &str, on: bool) -> Option<Cow<'static, str>> {
    Some(match topic {
        "skins" => format!(
            "{SKINS_LINE}\n\n{}",
            cards::perm_lines(text::study(CARDS_PAGE)?, on)
        )
        .into(),
        "llm-accounts" => format!("{LLM_ACCOUNTS_LINE}\n\n{}", text::study("llm-tokens")?).into(),
        t if t == CARDS_PAGE => cards::perm_lines(text::study(t)?, on).into(),
        t => text::study(t)?.into(),
    })
}

/// `_study_usage_all`: the usage, and the cards-only topic while cards
/// are on.
fn usage(on: bool) -> Option<Cow<'static, str>> {
    let text = text::study("_usage")?;
    Some(if on {
        format!("{text}{CARDS_USAGE_LINE}\n").into()
    } else {
        text.into()
    })
}

/// `_study_catalog_all`: the list as it is, or, while cards are on,
/// through `awk '{ print } /^  design   / { print "<line>" }'`: each line
/// with a newline after it, and [`CARDS_CATALOG_LINE`] after each line
/// that starts `  design   `.
fn catalog(on: bool) -> Option<Cow<'static, str>> {
    let text = text::study("_catalog")?;
    if !on {
        return Some(text.into());
    }
    let mut out = String::with_capacity(text.len() + CARDS_CATALOG_LINE.len() + 1);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
        if line.starts_with("  design   ") {
            out.push_str(CARDS_CATALOG_LINE);
            out.push('\n');
        }
    }
    Some(out.into())
}

/// Whether a study command line depends on the cards with a passport,
/// and if so its switch.
fn cards_switch(s: Study) -> Option<bool> {
    match s {
        Study::Usage | Study::Catalog => Some(LIST_WITH_PASSPORT),
        Study::Page { topic, .. } if topic == CARDS_PAGE || topic == "skins" => {
            Some(APPS_WITH_PASSPORT)
        }
        Study::Page { topic, .. } if topic == CARDS_ONLY => Some(PERMISSIONS_WITH_PASSPORT),
        Study::Page { .. } => None,
    }
}

/// `--json` for a page: the shell pipes the page through
/// `python3 -c '...json.dumps({"id": sys.argv[1], "body": body},
/// separators=(",", ":"))'`, where `body` is the page as Python's text
/// stdin reads it (`\r\n` and `\r` read as `\n`) with one trailing newline
/// taken off.
pub fn study_json(topic: &str, page: &str) -> String {
    let mut body = page.replace("\r\n", "\n").replace('\r', "\n");
    if body.ends_with('\n') {
        body.pop();
    }
    let value = Value::Object(vec![
        ("id".into(), Value::Str(topic.into())),
        ("body".into(), Value::Str(body.into())),
    ]);
    let options = json::Options {
        compact: true,
        ..json::Options::default()
    };
    let mut out = json::to_string(&value, &options).expect("a str holds no lone surrogate");
    out.push('\n');
    out
}

/// Whether the binary prints `page`'s `--json` itself. Where the shell
/// has a `python3`, only when it reads `page` as the binary does: no
/// `PYTHON*` setting (an encoding, a path, a startup file), and a page
/// that is ASCII or a locale Python reads as UTF-8 (one that names UTF-8,
/// or C/POSIX/none, which Python 3.7 and later read as UTF-8). Where it
/// has none, by [`python::NATIVE_WITHOUT_PYTHON3`] (no Python reads it,
/// so neither its settings nor the locale count).
pub fn native_json(env: &Env, page: &str) -> bool {
    match python::in_env(env) {
        Python3::Present => {}
        Python3::Missing => return python::NATIVE_WITHOUT_PYTHON3,
        Python3::Unknown => return false,
    }
    if env.any_named("PYTHON") {
        return false;
    }
    if page.is_ascii() {
        return true;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|n| env.get(n).filter(|v| !v.is_empty()))
        .map(|v| v.to_string_lossy().to_ascii_lowercase());
    match locale.as_deref() {
        None | Some("c") | Some("posix") => true,
        Some(l) => l.contains("utf-8") || l.contains("utf8"),
    }
}

/// `_study_page send-feedback` while hidden: [`feedback_optin::OFF_LINE`].
fn off_page() -> Cow<'static, str> {
    format!("{}\n", feedback_optin::OFF_LINE).into()
}

/// The study page, or `None` when the shell answers. `would_refuse`:
/// the passport rule's count, for the probe's request.
pub fn study_page(
    s: Study,
    p: &Prelude,
    env: &Env,
    would_refuse: impl Fn() -> bool,
) -> Option<Page> {
    // `send-feedback` with a passport: the shell's `_sf_agents_off_here`.
    let hidden = match s {
        Study::Page { topic, json } if topic == FEEDBACK_PAGE && p.passport => {
            if !feedback_optin::FEEDBACK_HIDE_NATIVE {
                return None;
            }
            if json {
                for page in [study_text(topic, false)?, off_page()] {
                    if !native_json(env, &page) {
                        return None;
                    }
                }
            }
            feedback_optin::hidden(feedback_optin::ask(env, p, &would_refuse)?)
        }
        _ => false,
    };
    let cards_only = matches!(s, Study::Page { topic, .. } if topic == CARDS_ONLY);
    let on = match cards_switch(s) {
        None => false,
        // No passport: cards are off and nothing is asked; the cards-only
        // topic is then unknown (the shell's error).
        Some(_) if !p.passport => {
            if cards_only {
                return None;
            }
            false
        }
        Some(false) => return None,
        Some(true) => {
            // What python3 will read must be known to read as here before
            // anything is sent: both ways the answer may go.
            if let Study::Page { topic, json: true } = s {
                for on in [false, true] {
                    if !native_json(env, &study_text(topic, on)?) {
                        return None;
                    }
                }
            }
            cards::on(cards::ask(env, p, &would_refuse)?)
        }
    };
    if cards_only && !on {
        // `cmd_study`'s refusal; its usage asks nothing more (the answer
        // is kept).
        return Some(Page {
            text: format!(
                "Error: unknown study topic '{CARDS_ONLY}'\n{}",
                usage(false)?
            )
            .into(),
            stderr: true,
            exit: Exit::Usage,
            lead: None,
        });
    }
    let text: Cow<'static, str> = match s {
        Study::Usage => usage(on)?,
        Study::Catalog => catalog(on)?,
        Study::Page { json: false, .. } if hidden => off_page(),
        Study::Page { topic, json: false } => study_text(topic, on)?,
        Study::Page { topic, json: true } => {
            let page = if hidden {
                off_page()
            } else {
                study_text(topic, on)?
            };
            if !native_json(env, &page) {
                return None;
            }
            study_json(topic, &page).into()
        }
    };
    Some(Page {
        text,
        stderr: false,
        exit: Exit::Ok,
        lead: None,
    })
}

/// Whether a help form's first word is an open tool (`help`, `--help`,
/// `-h`, `help-advanced`, `help-internal`): the shell's session checks
/// pass it with `HOOK_SOCK` set. A family's own help may not be: its
/// tool may be an `id` tool ([`session_checks`]).
pub fn open_tool(form: &HelpForm) -> bool {
    open_words(form.argv)
}

/// [`open_tool`] for a command line's words.
pub fn open_words(argv: &[&str]) -> bool {
    argv.first()
        .is_some_and(|w| HELP_FIRST.contains(w) || w.starts_with("help-"))
}

/// Lines on stderr, then exit 3: how a session check stops a command.
fn refused(lines: &[String]) -> Page {
    let mut text = String::new();
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    Page {
        text: text.into(),
        stderr: true,
        exit: Exit::Refused,
        lead: None,
    }
}

/// The shell router's session checks for `argv` in a session with
/// `HOOK_SOCK` set, before the verb runs (`cli/src/router.sh`, the start;
/// the tool table is [`crate::tools`]):
///
/// 1. `_cli_check_session_passport "$1"`: when the first word alone is an
///    `id` tool, a shared Codex server in the process's ancestry (the
///    shell looks only when a `CODEX_*` variable is set; with one set,
///    this gives `None`: the shell's check), then a passport whose socket
///    is gone: one probe asks the daemon whether it still knows the
///    passport, and a refusal is [`prelude::stale_passport_lines`].
/// 2. `_cli_apply_auth_for_verb "$1" "$2"`: when the first two words are
///    an `id` tool and there is no passport,
///    [`prelude::require_passport_lines`].
///
/// `Some(None)`: both pass, and nothing was printed. `Some(Some(page))`:
/// a check stops the command with that page. `None`: the shell's. The
/// probe goes out only when the port is a plain port number, the host an
/// IPv4 address and the passport plain ASCII ([`crate::cards::plain_port`],
/// [`crate::cards::plain`]); otherwise this gives `None`.
pub fn session_checks(
    argv: &[&str],
    p: &Prelude,
    env: &Env,
    would_refuse: impl FnOnce() -> bool,
) -> Option<Option<Page>> {
    let verb = argv.first().copied().unwrap_or("help");
    let sub = argv.get(1).copied().unwrap_or("");
    if tools::is_id(tools::tool_id(verb, "")) {
        if env.any_named("CODEX_") {
            return None;
        }
        let sock = env.first(k2_client::env_names::HOOK_SOCK)?;
        if p.passport && !crate::transport::client::is_socket(Path::new(sock)) {
            // The gate passed, so the shell has a port: the probe is sent
            // to it, here only when it is plainly the same request.
            let settings = cards::shell_settings(env, Path::new("/"))?;
            let port = cards::plain_port(settings.port_text.as_deref()?)?;
            settings.host.parse::<std::net::Ipv4Addr>().ok()?;
            let mut client = settings.client_on(port, would_refuse());
            if !client.passport().is_some_and(|t| cards::plain(t.as_str())) {
                return None;
            }
            if client.passport_rejected() {
                let lines = prelude::stale_passport_lines(sock.to_str()?);
                return Some(Some(refused(&lines)));
            }
        }
    }
    let tool = tools::tool_id(verb, sub);
    if tools::is_id(tool) && !p.passport {
        return Some(Some(refused(&prelude::require_passport_lines(tool))));
    }
    Some(None)
}

/// The help page, or `None` when the shell answers: the connection gate
/// needs a port and a token for help, and a family's help with
/// `HOOK_SOCK` set runs the session checks first ([`session_checks`]),
/// which may stop it with their own lines instead.
pub fn help_page(
    form: &'static HelpForm,
    p: &Prelude,
    env: &Env,
    would_refuse: impl FnOnce() -> bool,
) -> Option<Page> {
    if !(p.port && p.token) {
        return None;
    }
    if p.hook_sock && !open_tool(form) {
        if let Some(stop) = session_checks(form.argv, p, env, would_refuse)? {
            return Some(stop);
        }
    }
    let exit = Exit::ALL
        .into_iter()
        .find(|e| e.code() == i32::from(form.code))?;
    Some(Page {
        text: text::page(form.page)?.into(),
        stderr: form.stderr,
        exit,
        lead: None,
    })
}

/// The command lines that print the daily tier of `k2 help`
/// (`cmd_help_v2_daily`, `cli/src/help.sh`), the first thing anyone
/// types: `k2` alone, `k2 help` and `k2 --help`. `tests/cli/help_native.sh`
/// proves from bash's own trace that bash does nothing else for them
/// but the feedback probe and the cards probe. `-h`, an empty word and
/// `help <word>` for a word `cmd_help` has no arm for print the same
/// tier: [`Composed::Daily`].
pub const DAILY_FORMS: &[&[&str]] = &[&[], &["help"], &["--help"]];

/// The daily tier for [`DAILY_FORMS`]. True: the binary asks the daemon
/// whether agents may report ([`crate::feedback_optin`]) and whether
/// permission cards are on ([`crate::cards`]) as the shell does, and
/// prints the tier for the answers. False: the shell answers them.
pub const DAILY_NATIVE: bool = true;

/// The daily tier's line while permission cards are on, between
/// `_daily.txt` and `_daily-end.txt` (`cmd_help_v2_daily`'s `echo`).
pub const CARDS_HELP_LINE: &str = concat!(
    "  permission request <action> --why \"\u{2026}\"   Ask a person to allow what ",
    k2_client::product_name!(),
    " refused you (k2 study permissions)"
);

/// Whether `args` (the words after the program's name) is one of
/// [`DAILY_FORMS`].
pub fn daily(args: &[&str]) -> bool {
    DAILY_FORMS.contains(&args)
}

/// `cmd_help_v2_daily`: `_daily.txt`, `_daily-feedback.txt` unless
/// send-feedback is `hidden`, [`CARDS_HELP_LINE`] while cards are `on`,
/// then `_daily-end.txt`.
pub fn daily_text(on: bool, hidden: bool) -> Option<String> {
    let (head, block, end) = (
        text::help("_daily")?,
        text::help("_daily-feedback")?,
        text::help("_daily-end")?,
    );
    let mut out =
        String::with_capacity(head.len() + block.len() + CARDS_HELP_LINE.len() + 1 + end.len());
    out.push_str(head);
    if !hidden {
        out.push_str(block);
    }
    if on {
        out.push_str(CARDS_HELP_LINE);
        out.push('\n');
    }
    out.push_str(end);
    Some(out)
}

/// The daily tier for the shell's two questions, as `cmd_help_v2_daily` asks them:
/// whether send-feedback is hidden (`_sf_agents_off_here`), then whether
/// permission cards are on (`_permission_cli_enabled`). With no passport
/// neither asks anything: shown, and off. With one, the binary asks the
/// same way ([`feedback_optin::ask`], [`cards::ask`]), each on its own
/// connection as the shell's two curls are, or gives `None` (the shell
/// answers) where it can't be sure to, before anything is sent.
pub fn daily_tier(p: &Prelude, env: &Env, would_refuse: impl Fn() -> bool) -> Option<String> {
    if p.passport && !feedback_optin::FEEDBACK_HIDE_NATIVE {
        return None;
    }
    let feedback = feedback_optin::ask(env, p, &would_refuse)?;
    let cards = cards::ask(env, p, &would_refuse)?;
    let hidden = feedback_optin::hidden(feedback);
    daily_text(cards::on(cards), hidden)
}

/// The daily tier, or `None` when the shell answers. The connection gate
/// needs a port and a token for help. Its tool, `help`, is an open one,
/// so the session checks pass it ([`open_tool`]). Then the shell's two
/// questions ([`daily_tier`]).
pub fn daily_page(p: &Prelude, env: &Env, would_refuse: impl Fn() -> bool) -> Option<Page> {
    if !DAILY_NATIVE || !(p.port && p.token) {
        return None;
    }
    Some(Page {
        text: daily_tier(p, env, would_refuse)?.into(),
        stderr: false,
        exit: Exit::Ok,
        lead: None,
    })
}

/// The words the router sends to `cmd_help` (`help|--help|-h)`).
pub const HELP_FIRST: &[&str] = &["help", "--help", "-h"];

/// The words `cmd_help` (`cli/src/wiki.sh`) has an arm for, in its order,
/// but `''` and `*`, which print the daily tier. `help <word>` for any
/// other word prints the daily tier too.
pub const HELP_WORDS: &[&str] = &[
    "-a",
    "--advanced",
    "advanced",
    "--internal",
    "internal",
    "msg",
    "thread",
    "browser",
    "whoami",
    "setup",
    "talk",
    "read",
    "inbox",
    "agent",
    "preset",
    "sidecar",
    "compute",
    "ci",
    "changes",
    "sim",
    "hostchat",
    "chat",
    "sources",
    "source",
    "llm",
    "usage",
    "vault",
    "job",
    "jobs",
    "feedback",
    "tickets",
    "mail",
    "calendar",
    "cal",
    "db",
    "store",
    "board",
    "dns",
    "domain",
    "cert",
    "project",
    "workspace",
    "skills",
    "settings",
    "heartbeat",
    "wiki",
    "zen",
    "publish",
    "connect",
    "users",
    "remote-session",
    "study",
    "explain",
    "capabilities",
    "deprecated",
    "--deprecated",
];

/// A help page bash puts together from pieces, besides [`DAILY_FORMS`]
/// (`tests/cli/help_native.sh` proves each from bash's trace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composed {
    /// `-h`, an empty word, and `help`, `--help` or `-h` with one word
    /// `cmd_help` has no arm for (an empty one too): the daily tier, as
    /// [`daily_page`] prints it.
    Daily,
    /// `help`, `--help` or `-h` with `deprecated` or `--deprecated`, and
    /// `help-deprecated` (`cmd_help_deprecated`): `_deprecated.txt`, the
    /// retired verbs' map ([`crate::retired::map`]), `_deprecated-end.txt`.
    Deprecated,
    /// `chat`, `chat --help`, `chat -h`, `chat help`, and `help`, `--help`
    /// or `-h` with `chat` (`_chat_usage`): `chat/_head.txt`,
    /// `chat/_manage.txt` unless `_CHAT_HELP_MANAGE` is set to something
    /// other than `1`, `chat/_end.txt`. `chat` is an `id` tool: with
    /// `HOOK_SOCK` set its own forms run the session checks first.
    Chat,
}

/// The pages of [`Composed`]. True: the binary prints them as bash puts
/// them together. False: the shell answers them.
pub const COMPOSED_NATIVE: bool = true;

/// The setting `_chat_usage` reads: `${_CHAT_HELP_MANAGE:-1}` is `1` for
/// the manage piece.
pub const CHAT_MANAGE: &str = "_CHAT_HELP_MANAGE";

/// The composed page `args` (the words after the program's name) is.
pub fn composed(args: &[&str]) -> Option<Composed> {
    match args {
        ["-h"] | [""] => Some(Composed::Daily),
        ["help-deprecated"] => Some(Composed::Deprecated),
        ["chat"] | ["chat", "--help" | "-h" | "help"] => Some(Composed::Chat),
        [first, word] if HELP_FIRST.contains(first) => match *word {
            "deprecated" | "--deprecated" => Some(Composed::Deprecated),
            "chat" => Some(Composed::Chat),
            w if HELP_WORDS.contains(&w) => None,
            _ => Some(Composed::Daily),
        },
        _ => None,
    }
}

/// `cmd_help_deprecated`: the deprecated tier.
pub fn deprecated_text() -> Option<String> {
    let (head, end) = (text::help("_deprecated")?, text::help("_deprecated-end")?);
    Some(format!("{head}{}{end}", crate::retired::map()))
}

/// `_chat_usage` in `env`.
pub fn chat_text(env: &Env) -> Option<String> {
    let manage = env
        .get(CHAT_MANAGE)
        .is_none_or(|v| v.is_empty() || v == "1");
    let mut out = text::help("chat/_head")?.to_string();
    if manage {
        out.push_str(text::help("chat/_manage")?);
    }
    out.push_str(text::help("chat/_end")?);
    Some(out)
}

/// A [`Composed`] page for `argv`, or `None` when the shell answers. The
/// connection gate needs a port and a token (the shell skips it for some
/// of chat's forms; the binary hands those over). The daily tier asks
/// as [`daily_page`] does ([`daily_tier`]).
pub fn composed_page(
    c: Composed,
    argv: &[&str],
    p: &Prelude,
    env: &Env,
    would_refuse: impl Fn() -> bool,
) -> Option<Page> {
    if !COMPOSED_NATIVE || !(p.port && p.token) {
        return None;
    }
    let text = match c {
        Composed::Daily => daily_tier(p, env, would_refuse)?,
        Composed::Deprecated => deprecated_text()?,
        Composed::Chat => {
            if p.hook_sock && !open_words(argv) {
                if let Some(stop) = session_checks(argv, p, env, would_refuse)? {
                    return Some(stop);
                }
            }
            chat_text(env)?
        }
    };
    Some(Page {
        text: text.into(),
        stderr: false,
        exit: Exit::Ok,
        lead: None,
    })
}

/// The unknown command (the router's `*)` arm): when the binary prints
/// it itself. True: a word the router has no arm for is a retired verb's
/// line, or `Unknown command: <word>` and the daily tier, as
/// [`unknown_page`] says. False: the shell answers it.
pub const UNKNOWN_NATIVE: bool = true;

/// The router's `*)` arm for `argv`, whose first word has no arm
/// ([`crate::route::TABLE`]), or `None` when the shell answers:
///
/// ```text
/// _retired_verb_exit "$1 ${2:-}"
/// _retired_verb_exit "$1"
/// echo "Unknown command: $1" >&2
/// cmd_help
/// exit 1
/// ```
///
/// A retired verb ([`crate::retired`]) is its one line on stderr, exit 1.
/// Any other word is its line on stderr, then the daily tier (asked as
/// [`daily_page`] asks it: [`daily_tier`]), exit 1. The connection gate needs a
/// port and a token (and with a token the virtual desktop's guard
/// passes). With `HOOK_SOCK` set, a word whose tool is an `id` tool (the
/// tool table names a few words the router has no arm for) runs the
/// session checks first: those go to the shell.
pub fn unknown_page(
    argv: &[&str],
    p: &Prelude,
    env: &Env,
    would_refuse: impl Fn() -> bool,
) -> Option<Page> {
    let (verb, sub) = (*argv.first()?, argv.get(1).copied().unwrap_or(""));
    if !UNKNOWN_NATIVE || verb.is_empty() || !(p.port && p.token) {
        return None;
    }
    if p.hook_sock
        && (tools::is_id(tools::tool_id(verb, "")) || tools::is_id(tools::tool_id(verb, sub)))
    {
        return None;
    }
    if let Some((key, new)) = crate::retired::lookup(verb, sub) {
        return Some(Page {
            text: crate::retired::line(&key, new).into(),
            stderr: true,
            exit: Exit::Error,
            lead: None,
        });
    }
    Some(Page {
        text: daily_tier(p, env, would_refuse)?.into(),
        stderr: false,
        exit: Exit::Error,
        lead: Some(format!("Unknown command: {verb}\n")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn src(file: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    fn page(topic: &'static str, json: bool) -> Option<Study> {
        Some(Study::Page { topic, json })
    }

    #[test]
    fn the_arguments_are_read_as_the_shell_reads_them() {
        assert_eq!(study(&[]), Some(Study::Catalog));
        assert_eq!(study(&["list"]), Some(Study::Catalog));
        assert_eq!(study(&[""]), Some(Study::Catalog));
        assert_eq!(
            study(&["", "errors"]),
            page("errors", false),
            "an empty topic is no topic"
        );
        assert_eq!(study(&["errors"]), page("errors", false));
        assert_eq!(study(&["errors", "--json"]), page("errors", true));
        assert_eq!(study(&["--json", "errors"]), page("errors", true));
        assert_eq!(study(&["cron"]), page("heartbeats", false));
        assert_eq!(study(&["skins", "--json"]), page("skins", true));
        assert_eq!(study(&["llm-accounts"]), page("llm-accounts", false));
        assert_eq!(study(&["permissions"]), page("permissions", false));
        for usage in [
            &["--help"][..],
            &["-h"],
            &["errors", "--help"],
            &["--json", "-h"],
            &["a", "-h", "b"],
        ] {
            assert_eq!(study(usage), Some(Study::Usage), "{usage:?}");
        }
        // The shell's: the list as JSON, errors (an unknown flag, a second
        // topic, before any --help), unknown topics and the bash pages.
        for shell in [
            &["--json"][..],
            &["list", "--json"],
            &["--bogus", "--help"],
            &["--"],
            &["errors", "db"],
            &["errors", "db", "--help"],
            &["nope"],
            &["-x"],
            &["app-api"],
            &["app-errors"],
            &["live"],
            &["_catalog"],
            &["_usage"],
            &["Errors"],
        ] {
            assert_eq!(study(shell), None, "{shell:?}");
        }
    }

    #[test]
    fn the_aliases_and_lines_are_the_shell_s() {
        let index = src("study-index.sh");
        assert!(index.contains(&format!("            echo \"{SKINS_LINE}\"\n            echo\n            _study_page_skins\n")), "the skins arm moved");
        assert!(index.contains(&format!("llm-accounts) printf '%s\\n\\n' \"{LLM_ACCOUNTS_LINE}\"; _study_page_llm_tokens ;;")), "the llm-accounts arm moved");
        assert!(index.contains("        apps) _study_page_skins ;;\n"));
        let command = src("study-command.sh");
        let aliases = format!(
            "case \"$topic\" in {}) topic=\"heartbeats\" ;; esac",
            HEARTBEAT_ALIASES.join("|")
        );
        assert!(
            command.contains(&aliases),
            "no `{aliases}` in study-command.sh"
        );
        let skins = src("study-skins.sh");
        assert!(
            skins
                .contains("    cat <<'EOF' | _perm_lines\n%include cli/text/study/apps.txt\nEOF\n"),
            "the apps page's filter moved"
        );
        let perm = src("permission.sh");
        for arm in [
            "            *' #if-cards') if [ \"$on\" = 1 ]; then printf '%s\\n' \"${line% #if-cards}\"; fi ;;",
            "            *' #if-no-cards') if [ \"$on\" = 0 ]; then printf '%s\\n' \"${line% #if-no-cards}\"; fi ;;",
            "            *) printf '%s\\n' \"$line\" ;;",
        ] {
            assert!(perm.contains(arm), "_perm_lines changed: no {arm}");
        }
    }

    /// Every study file is a topic's page, or the catalog, the usage or
    /// the cards-only page; so a file the binary answers for is a topic
    /// the shell knows.
    #[test]
    fn every_study_file_is_a_topic() {
        let index = src("study-index.sh");
        let ids = index
            .lines()
            .find_map(|l| l.strip_prefix("_STUDY_TOPIC_IDS=\""))
            .and_then(|l| l.strip_suffix('"'))
            .expect("_STUDY_TOPIC_IDS");
        let ids: Vec<&str> = ids.split(' ').collect();
        for (key, _) in text::STUDY {
            if ["_catalog", "_usage", CARDS_ONLY].contains(key) {
                continue;
            }
            assert!(ids.contains(key), "cli/text/study/{key}.txt is not a topic");
            assert!(
                !HEARTBEAT_ALIASES.contains(key),
                "{key} is a heartbeats alias"
            );
            assert_eq!(study(&[*key]), page(key, false));
        }
        for alias in ["skins", "llm-accounts"] {
            assert!(ids.contains(&alias));
        }
    }

    #[test]
    fn cards_off_is_perm_lines_with_cards_off() {
        assert_eq!(cards_off("a\nb #if-cards\nc #if-no-cards\nd"), "a\nc\nd\n");
        assert_eq!(cards_off(""), "");
        assert_eq!(cards_off("\n"), "\n");
        assert_eq!(cards_off("x #if-no-cards #if-cards\n"), "");
        assert_eq!(cards_off("x#if-cards\n  #if-cards\n"), "x#if-cards\n");
        let apps = text::study(CARDS_PAGE).unwrap();
        assert!(
            apps.lines().any(|l| l.ends_with(" #if-cards")),
            "the apps page has no card line"
        );
        assert!(!cards_off(apps).contains("#if-"));
    }

    #[test]
    fn the_json_is_python_s() {
        assert_eq!(study_json("x", "a\n"), "{\"id\":\"x\",\"body\":\"a\"}\n");
        assert_eq!(
            study_json("x", "a\n\n"),
            "{\"id\":\"x\",\"body\":\"a\\n\"}\n"
        );
        assert_eq!(
            study_json("x", "\"\\\t\u{2014}\r\n"),
            "{\"id\":\"x\",\"body\":\"\\\"\\\\\\t\\u2014\"}\n"
        );
        assert_eq!(
            study_json("x", "a\rb"),
            "{\"id\":\"x\",\"body\":\"a\\nb\"}\n"
        );
    }

    #[test]
    fn the_locale_python_reads_as_utf8() {
        let d = std::env::temp_dir().join(format!("k5-pages-py-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let py = d.join("python3");
        std::fs::write(&py, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&py, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = d.to_str().unwrap();
        let reads = |pairs: &[(&str, &str)], page: &str| {
            let mut all = vec![("PATH", path)];
            all.extend_from_slice(pairs);
            native_json(&Env::of(&all), page)
        };
        assert!(reads(&[], "\u{2014}"));
        assert!(reads(&[("LC_ALL", "C")], "\u{2014}"));
        assert!(reads(&[("LANG", "en_US.UTF-8")], "\u{2014}"));
        assert!(reads(
            &[("LC_ALL", ""), ("LC_CTYPE", "de_DE.utf8")],
            "\u{2014}"
        ));
        assert!(!reads(&[("LANG", "en_US.ISO8859-1")], "\u{2014}"));
        assert!(reads(&[("LANG", "en_US.ISO8859-1")], "ascii"));
        assert!(!reads(&[("PYTHONIOENCODING", "latin-1")], "ascii"));
        assert!(!reads(&[("PYTHONPATH", "/x")], "ascii"));
        // No python3: the switch, whatever the settings and the locale.
        let none = |pairs: &[(&str, &str)], page: &str| {
            let mut all = vec![("PATH", "/nonexistent")];
            all.extend_from_slice(pairs);
            native_json(&Env::of(&all), page)
        };
        for (pairs, page) in [
            (&[][..], "ascii"),
            (&[("PYTHONIOENCODING", "latin-1")][..], "\u{2014}"),
            (&[("LANG", "en_US.ISO8859-1")][..], "\u{2014}"),
        ] {
            assert_eq!(
                none(pairs, page),
                python::NATIVE_WITHOUT_PYTHON3,
                "{pairs:?}"
            );
        }
        assert!(!native_json(
            &Env::of(&[("PATH", path), ("BASH_FUNC_python3%%", "() { :\n}")]),
            "ascii"
        ));
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn prelude(port: bool, token: bool, passport: bool, hook_sock: bool) -> Prelude {
        Prelude {
            port,
            token,
            passport,
            hook_sock,
        }
    }

    #[test]
    fn a_passport_and_the_card_pages() {
        let env = Env::of(&[]);
        let with = prelude(false, true, true, true);
        let without = prelude(false, false, false, false);
        let never = || -> bool { panic!("nothing is asked") };
        let apps = |topic| Study::Page { topic, json: false };
        for (s, switch) in [
            (Study::Usage, LIST_WITH_PASSPORT),
            (Study::Catalog, LIST_WITH_PASSPORT),
            (apps("apps"), APPS_WITH_PASSPORT),
            (apps("skins"), APPS_WITH_PASSPORT),
            (apps("permissions"), PERMISSIONS_WITH_PASSPORT),
        ] {
            // A switch that is off hands the page to the shell; one that
            // is on asks (here: no HOME, so the shell's again).
            assert_eq!(study_page(s, &with, &env, || false), None, "{s:?}");
            if !switch {
                assert_eq!(study_page(s, &with, &env, never), None, "{s:?}");
            }
            // No passport: cards off, nothing asked; the cards-only topic
            // is the shell's error.
            let page = study_page(s, &without, &env, never);
            assert_eq!(page.is_none(), s == apps("permissions"), "{s:?}");
        }
        let errors = Study::Page {
            topic: "errors",
            json: false,
        };
        assert_eq!(
            study_page(errors, &with, &env, never).unwrap().text,
            text::study("errors").unwrap()
        );
        let usage = study_page(Study::Usage, &without, &env, never).unwrap();
        assert_eq!(
            (usage.text.as_ref(), usage.stderr, usage.exit),
            (text::study("_usage").unwrap(), false, Exit::Ok)
        );
        let skins = study_page(apps("skins"), &without, &env, never).unwrap();
        assert!(skins.text.starts_with(&format!("{SKINS_LINE}\n\n")));
        assert_eq!(
            &skins.text[SKINS_LINE.len() + 2..],
            cards_off(text::study("apps").unwrap())
        );
    }

    /// `send-feedback`'s page: with a passport the feedback probe asks
    /// (here: no HOME, so the shell's); without one, the page, nothing
    /// asked. Hidden, it is the one line, and its `--json` that line.
    #[test]
    fn a_passport_and_send_feedback_s_page() {
        let env = Env::of(&[]);
        let with = prelude(true, true, true, false);
        let without = prelude(true, true, false, false);
        let never = || -> bool { panic!("nothing is asked") };
        assert_eq!(
            study(&[FEEDBACK_PAGE]),
            Some(Study::Page {
                topic: FEEDBACK_PAGE,
                json: false
            })
        );
        for json in [false, true] {
            let s = Study::Page {
                topic: FEEDBACK_PAGE,
                json,
            };
            assert_eq!(study_page(s, &with, &env, || false), None, "{json}");
            let page = study_page(s, &without, &env, never).expect("the page");
            let full = text::study(FEEDBACK_PAGE).unwrap();
            let want = if json {
                study_json(FEEDBACK_PAGE, full)
            } else {
                full.to_string()
            };
            assert_eq!(
                (page.text.as_ref(), page.stderr, page.exit),
                (want.as_str(), false, Exit::Ok)
            );
        }
        assert_eq!(off_page(), format!("{}\n", feedback_optin::OFF_LINE));
        assert_eq!(
            study_json(FEEDBACK_PAGE, &off_page()),
            format!(
                "{{\"id\":\"send-feedback\",\"body\":\"{}\"}}\n",
                feedback_optin::OFF_LINE
            )
        );
        assert!(off_page().is_ascii());
    }

    #[test]
    fn the_pages_with_cards_on_are_the_shell_s() {
        let index = src("study-index.sh");
        assert!(index.contains(&format!("        echo \"{CARDS_USAGE_LINE}\"\n")));
        let awk = format!(
            "            awk '{{ print }} /^  design   / {{ print \"{CARDS_CATALOG_LINE}\" }}'\n"
        );
        assert!(index.contains(&awk), "no {awk:?} in study-index.sh");
        assert!(src("garden-data.sh").contains(
            "_study_page_permissions() {\n    cat <<'EOF'\n%include cli/text/study/permissions.txt\nEOF\n}"
        ));
        let command = src("study-command.sh");
        assert!(command.contains(
            "        echo \"Error: unknown study topic '$topic'\" >&2\n        _study_usage_all >&2\n        exit 2\n"
        ));
        let usage_on = usage(true).unwrap();
        assert_eq!(
            usage_on.as_ref(),
            format!("{}  permissions\n", text::study("_usage").unwrap())
        );
        let list = catalog(true).unwrap();
        let off = catalog(false).unwrap();
        assert_eq!(off.as_ref(), text::study("_catalog").unwrap());
        let design = off
            .lines()
            .find(|l| l.starts_with("  design   "))
            .expect("the list has the design line");
        assert_eq!(
            list.as_ref(),
            off.replacen(
                &format!("{design}\n"),
                &format!("{design}\n{CARDS_CATALOG_LINE}\n"),
                1
            )
        );
        let on = study_text("apps", true).unwrap();
        assert!(!on.contains("#if-"));
        assert_ne!(on.as_ref(), cards_off(text::study("apps").unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn a_family_s_help_with_hook_sock_runs_the_session_checks() {
        let d = std::env::temp_dir().join(format!("k5-pages-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let live = d.join("live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live).unwrap();
        let live = live.to_str().unwrap().to_string();
        let gone = d.join("gone.sock").to_str().unwrap().to_string();
        let form = |w: &[&str]| text::help_form(w).unwrap_or_else(|| panic!("{w:?} is a form"));
        let mail = form(&["mail", "send", "--help"]);
        let agent = form(&["agent", "--help"]);
        let harness = form(&["agent", "harness", "--help"]);
        let with = prelude(true, true, true, true);
        let without = prelude(true, true, false, true);
        let sock = |s: &str| Env::of(&[("K2_HOOK_SOCK", s)]);
        let never = || -> bool { panic!("nothing is sent") };
        // A passport and a live socket: the page.
        let page = help_page(mail, &with, &sock(&live), never).unwrap();
        assert_eq!(page.text, text::help("mail/send").unwrap());
        // No passport: an id tool is refused, an open one is not.
        let stop = help_page(mail, &without, &sock(&gone), never).unwrap();
        let want: String = prelude::require_passport_lines("mail")
            .iter()
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(
            (stop.text.as_ref(), stop.stderr, stop.exit),
            (want.as_str(), true, Exit::Refused)
        );
        assert_eq!(
            help_page(agent, &without, &sock(&gone), never)
                .unwrap()
                .text,
            text::help("agent").unwrap()
        );
        // The tool of the first two words: agent harness is an id tool.
        let stop = help_page(harness, &without, &sock(&gone), never).unwrap();
        assert!(stop
            .text
            .starts_with("Error: tool 'agent-harness' requires"));
        // A CODEX_ setting: the shell looks for a shared Codex server.
        let codex = Env::of(&[("K2_HOOK_SOCK", &live), ("CODEX_THREAD_ID", "t")]);
        assert_eq!(help_page(mail, &with, &codex, never), None);
        assert!(
            help_page(agent, &with, &codex, never).is_some(),
            "an open tool has no Codex check"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn help_needs_the_gate_s_port_and_token() {
        let family = text::help_form(&["mail", "send", "--help"])
            .expect("mail send --help is a plain print");
        let open = text::HELP_FORMS
            .iter()
            .find(|f| f.argv.first() == Some(&"help"))
            .expect("a help <w> form");
        assert!(!open_tool(family) && open_tool(open));
        let none = Env::of(&[]);
        let ok = prelude(true, true, true, false);
        let page = help_page(family, &ok, &none, || false).unwrap();
        assert_eq!(
            (page.text.as_ref(), page.stderr, page.exit),
            (text::help("mail/send").unwrap(), false, Exit::Ok)
        );
        for no in [
            prelude(false, true, false, false),
            prelude(true, false, false, false),
        ] {
            assert_eq!(help_page(family, &no, &none, || false), None);
            assert_eq!(help_page(open, &no, &none, || false), None);
        }
        let sock = prelude(true, true, true, true);
        assert!(
            help_page(open, &sock, &none, || false).is_some(),
            "help is an open tool"
        );
        for f in text::HELP_FORMS {
            assert!(
                Exit::ALL.iter().any(|e| e.code() == i32::from(f.code)),
                "{f:?}"
            );
        }
    }

    /// The router sends the three forms to `cmd_help` with no word, which
    /// prints the daily tier as `cmd_help_v2_daily` puts it together:
    /// the feedback probe first, then the cards probe.
    #[test]
    fn the_daily_tier_is_the_shell_s() {
        let router = src("router.sh");
        assert!(router.contains("case \"${1:-help}\" in\n"));
        assert!(router.contains(
            "    help|--help|-h)\n        # Guard against `set -u` when no trailing args.\n        if [ $# -gt 0 ]; then shift; fi\n        cmd_help \"$@\"\n        ;;\n"
        ));
        assert!(src("wiki.sh").contains(
            "    case \"${1:-}\" in\n        -a|--advanced|advanced)   cmd_help_v2_advanced ;;\n"
        ));
        assert!(src("wiki.sh").contains("        '')                       cmd_help_v2_daily ;;\n"));
        let daily = format!(
            "cmd_help_v2_daily() {{\n    cat <<'EOF'\n%include cli/text/help/_daily.txt\nEOF\n    # 0.46.17: not in an agent's help while agents may not report\n    # (_sf_agents_off_here). Everyone else: today's text.\n    if ! _sf_agents_off_here; then\n        cat <<'EOF'\n%include cli/text/help/_daily-feedback.txt\nEOF\n    fi\n    # Permission cards (Beta): one line, only while both switches are on\n    # for this agent (PRD rev 3.1 \u{a7}11.4). Off = today's text.\n    if _permission_cli_enabled; then\n        echo '{CARDS_HELP_LINE}'\n    fi\n    cat <<'EOF'\n%include cli/text/help/_daily-end.txt\nEOF\n}}\n"
        );
        assert!(src("help.sh").contains(&daily), "cmd_help_v2_daily moved");
        let (head, block, end) = (
            text::help("_daily").unwrap(),
            text::help("_daily-feedback").unwrap(),
            text::help("_daily-end").unwrap(),
        );
        assert_eq!(
            daily_text(false, false).unwrap(),
            format!("{head}{block}{end}")
        );
        assert_eq!(
            daily_text(true, false).unwrap(),
            format!("{head}{block}{CARDS_HELP_LINE}\n{end}")
        );
        assert_eq!(daily_text(false, true).unwrap(), format!("{head}{end}"));
        assert_eq!(
            daily_text(true, true).unwrap(),
            format!("{head}{CARDS_HELP_LINE}\n{end}")
        );
        assert!(head.ends_with('\n') && block.ends_with('\n'));
        assert!(!CARDS_HELP_LINE.contains('\''));
        // The block is the blank line, the heading and send-feedback's
        // three lines.
        let heading = concat!(
            "\n",
            k2_client::product_name!(),
            " ITSELF\n  send-feedback "
        );
        assert!(block.starts_with(heading) && block.lines().count() == 5);
    }

    #[test]
    fn the_daily_forms() {
        for yes in [&[][..], &["help"], &["--help"]] {
            assert!(daily(yes), "{yes:?}");
        }
        for no in [
            &["-h"][..],
            &[""],
            &["help", "nope"],
            &["--help", "mail"],
            &["help", ""],
            &["HELP"],
            &["help-advanced"],
        ] {
            assert!(!daily(no), "{no:?}");
        }
        // help, --help and no word are the open tool `help`: the shell's
        // session checks pass them, so the binary has none to run.
        for verb in ["help", "--help"] {
            assert!(!tools::is_id(tools::tool_id(verb, "")), "{verb}");
        }
    }

    #[test]
    fn the_daily_page_needs_the_gate_and_its_switch() {
        // No passport: cards off, nothing asked.
        let env = Env::of(&[("HOME", "/nonexistent/k5-daily")]);
        let never = || -> bool { panic!("nothing is asked") };
        let owner = prelude(true, true, false, false);
        let page = daily_page(&owner, &env, never);
        if DAILY_NATIVE {
            let page = page.expect("the tier");
            assert_eq!(
                (page.text.as_ref(), page.stderr, page.exit),
                (daily_text(false, false).unwrap().as_str(), false, Exit::Ok)
            );
            let sock = prelude(true, true, false, true);
            assert!(daily_page(&sock, &env, never).is_some(), "HOOK_SOCK set");
        } else {
            assert_eq!(page, None);
        }
        for no in [
            prelude(false, true, false, false),
            prelude(true, false, false, false),
            prelude(false, false, true, true),
        ] {
            assert_eq!(daily_page(&no, &env, never), None, "{no:?}");
        }
    }

    /// `cli/k2`, the script the shell runs.
    fn script() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    /// Each function these pages stand for is defined once in the script
    /// (a second definition would win).
    #[test]
    fn each_function_is_defined_once() {
        let k2 = script();
        for f in [
            "cmd_help",
            "cmd_help_v2_daily",
            "cmd_help_deprecated",
            "cmd_chat",
            "_chat_usage",
            "_retired_verb_exit",
            "_retired_verbs_map",
        ] {
            let def = format!("\n{f}() {{\n");
            assert_eq!(
                k2.matches(&def).count(),
                1,
                "{f} is defined other than once"
            );
        }
    }

    /// [`HELP_WORDS`] is `cmd_help`'s arms, in order; `''` and `*` print
    /// the daily tier.
    #[test]
    fn the_help_words_are_cmd_help_s_arms() {
        let wiki = src("wiki.sh");
        let start = wiki.find("cmd_help() {\n").expect("cmd_help in wiki.sh");
        let body = &wiki[start..];
        let case = body
            .find("    case \"${1:-}\" in\n")
            .expect("cmd_help's case");
        let end = body[case..].find("\n    esac\n").expect("cmd_help's esac") + case;
        let mut words = Vec::new();
        let mut daily = Vec::new();
        for line in body[case..end].lines().skip(1) {
            let arm = line
                .strip_prefix("        ")
                .unwrap_or_else(|| panic!("not an arm: {line:?}"));
            let (pats, rest) = arm.split_once(')').unwrap_or_else(|| panic!("{line:?}"));
            let call = rest
                .trim()
                .strip_suffix(";;")
                .unwrap_or_else(|| panic!("{line:?}"))
                .trim();
            for w in pats.split('|') {
                if w == "''" || w == "*" {
                    daily.push((w.to_string(), call.to_string()));
                } else {
                    assert!(
                        !w.contains(['"', '\'', '*', '?', '[', '$']),
                        "a pattern: {w:?}"
                    );
                    words.push(w.to_string());
                }
            }
        }
        assert_eq!(words, HELP_WORDS, "cmd_help's arms moved");
        assert_eq!(
            daily,
            [
                ("''".to_string(), "cmd_help_v2_daily".to_string()),
                ("*".to_string(), "cmd_help_v2_daily".to_string())
            ]
        );
        for (word, call) in [
            ("deprecated|--deprecated)  ", "cmd_help_deprecated ;;"),
            ("chat)                     ", "_chat_usage ;;"),
        ] {
            assert!(
                body[case..end].contains(&format!("        {word}{call}\n")),
                "{word}{call}"
            );
        }
    }

    #[test]
    fn the_composed_forms() {
        for daily in [
            &["-h"][..],
            &[""],
            &["help", "nope"],
            &["--help", ""],
            &["-h", "--json"],
            &["help", "HELP"],
            &["help", "app"],
        ] {
            assert_eq!(composed(daily), Some(Composed::Daily), "{daily:?}");
        }
        for first in HELP_FIRST {
            for word in HELP_WORDS {
                let want = match *word {
                    "deprecated" | "--deprecated" => Some(Composed::Deprecated),
                    "chat" => Some(Composed::Chat),
                    _ => None,
                };
                assert_eq!(composed(&[first, word]), want, "{first} {word}");
            }
        }
        assert_eq!(composed(&["help-deprecated"]), Some(Composed::Deprecated));
        for chat in [
            &["chat"][..],
            &["chat", "-h"],
            &["chat", "--help"],
            &["chat", "help"],
        ] {
            assert_eq!(composed(chat), Some(Composed::Chat), "{chat:?}");
        }
        for none in [
            &[][..],
            &["help"],
            &["--help"],
            &["help", "mail"],
            &["help", "nope", "x"],
            &["chat", "x"],
            &["chat", "--help", "x"],
            &["", "x"],
            &["help-deprecated", "x"],
            &["deprecated"],
        ] {
            assert_eq!(composed(none), None, "{none:?}");
        }
    }

    /// The deprecated tier and chat's usage, as bash puts them together.
    #[test]
    fn the_pieces_are_the_shell_s() {
        let deprecated = "cmd_help_deprecated() {\n    cat <<'EOF'\n%include cli/text/help/_deprecated.txt\nEOF\n    _retired_verbs_map\n    cat <<'EOF'\n%include cli/text/help/_deprecated-end.txt\nEOF\n}\n";
        assert!(
            src("agent-context.sh").contains(deprecated),
            "cmd_help_deprecated moved"
        );
        assert_eq!(
            deprecated_text().unwrap(),
            format!(
                "{}{}{}",
                text::help("_deprecated").unwrap(),
                crate::retired::map(),
                text::help("_deprecated-end").unwrap()
            )
        );
        let router = src("router.sh");
        assert!(router.contains("    help-deprecated)\n        cmd_help_deprecated\n        ;;\n"));
        let chat = src("chat.sh");
        assert!(chat.contains(&format!(
            "_chat_usage() {{\n    cat <<'EOF'\n%include cli/text/help/chat/_head.txt\nEOF\n    if [ \"${{{CHAT_MANAGE}:-1}}\" = \"1\" ]; then\n        cat <<'EOF'\n%include cli/text/help/chat/_manage.txt\nEOF\n    fi\n    cat <<'EOF'\n%include cli/text/help/chat/_end.txt\nEOF\n}}\n"
        )));
        assert!(chat.contains(
            "cmd_chat() {\n    if [ $# -eq 0 ]; then _chat_usage; return 0; fi\n    local sub=\"$1\"; shift\n    case \"$sub\" in\n        -h|--help|help) _chat_usage; return 0 ;;\n    esac\n"
        ));
        assert!(router.contains("    chat)\n        # Sew chat inside Spaces, for agents (Sew S30 K21).\n        shift; cmd_chat \"$@\"\n        ;;\n"));
        let (head, manage, end) = (
            text::help("chat/_head").unwrap(),
            text::help("chat/_manage").unwrap(),
            text::help("chat/_end").unwrap(),
        );
        let with = format!("{head}{manage}{end}");
        let without = format!("{head}{end}");
        for (pairs, want) in [
            (&[][..], &with),
            (&[(CHAT_MANAGE, "1")], &with),
            (&[(CHAT_MANAGE, "")], &with),
            (&[(CHAT_MANAGE, "0")], &without),
            (&[(CHAT_MANAGE, " 1")], &without),
        ] {
            assert_eq!(&chat_text(&Env::of(pairs)).unwrap(), want, "{pairs:?}");
        }
        assert!(
            !tools::is_id(tools::tool_id("help", "chat"))
                && tools::is_id(tools::tool_id("chat", ""))
        );
    }

    /// The router's `*)` arm, as [`unknown_page`] answers it.
    #[test]
    fn the_unknown_command_is_the_router_s_arm() {
        let router = src("router.sh");
        assert!(router.contains(
            "    *)\n        _retired_verb_exit \"$1 ${2:-}\"\n        _retired_verb_exit \"$1\"\n        echo \"Unknown command: $1\" >&2\n        cmd_help\n        exit 1\n        ;;\nesac\n"
        ), "the router's *) arm moved");
        assert_eq!(Exit::Error.code(), 1);
    }

    /// Every word the prelude's gates read as the first word
    /// (`cli-tools-policy.sh`, before `BASE_URL`) is a router word, so a
    /// word the router has no arm for reaches none of them: the gates do
    /// the same for it as for `help`, but for the virtual desktop's
    /// guard, which a token passes.
    #[test]
    fn the_gates_read_no_word_the_router_lacks() {
        let policy = src("cli-tools-policy.sh");
        let start = policy
            .find("_skip_conn_gate=0\n")
            .expect("the gates' start");
        let end = policy
            .find("\nBASE_URL=\"http://${HOST}:${PORT}\"\n")
            .expect("BASE_URL");
        let gates = &policy[start..end];
        let mut words = std::collections::BTreeSet::new();
        for (at, _) in gates.match_indices("\"${1:-}\" = \"") {
            let rest = &gates[at + "\"${1:-}\" = \"".len()..];
            words.insert(rest[..rest.find('"').unwrap()].to_string());
        }
        // The arms of each `case "${1:-}" in` there.
        let mut rest = gates;
        while let Some(at) = rest.find("case \"${1:-}\" in") {
            let block = &rest[at..];
            let first = block.lines().next().unwrap();
            if first.ends_with(" esac") {
                // On one line: the guard's, checked below.
                rest = &block[first.len()..];
                continue;
            }
            let prefix = rest[..at].rsplit('\n').next().unwrap();
            assert!(prefix.bytes().all(|b| b == b' '), "a case after {prefix:?}");
            let indent = prefix.len();
            let esac = block
                .find(&format!("\n{}esac\n", " ".repeat(indent)))
                .expect("the case's esac");
            for line in block[..esac].lines().skip(1) {
                let Some(arm) = line.strip_prefix(&" ".repeat(indent + 4)) else {
                    continue;
                };
                if arm.starts_with(' ') || arm.starts_with('#') {
                    continue;
                }
                let Some((pats, _)) = arm.split_once(')') else {
                    continue;
                };
                for w in pats.split('|') {
                    words.insert(w.trim_matches('"').to_string());
                }
            }
            rest = &block[esac..];
        }
        assert!(words.len() > 30, "only read {words:?}");
        let known: Vec<&str> = crate::route::TABLE
            .iter()
            .flat_map(|n| n.words.iter().copied())
            .collect();
        for w in &words {
            assert!(
                w.is_empty() || known.contains(&w.as_str()),
                "the gates read {w:?}, which the router has no arm for"
            );
        }
        // The guard's offline words: `help` and the like, never a word the
        // router lacks.
        assert!(gates.contains(
            "case \"${1:-}\" in \"\"|study|help|--help|-h|--schema|version|--version) _k2_offline_verb=1 ;; esac\n"
        ));
    }

    #[test]
    fn the_switches_decide() {
        let env = Env::of(&[("HOME", "/nonexistent/k5-composed")]);
        let never = || -> bool { panic!("nothing is asked") };
        let owner = prelude(true, true, false, false);
        let tier = composed_page(Composed::Daily, &["-h"], &owner, &env, never);
        let deprecated = composed_page(
            Composed::Deprecated,
            &["help-deprecated"],
            &owner,
            &env,
            never,
        );
        let chat = composed_page(Composed::Chat, &["chat"], &owner, &env, never);
        let unknown = unknown_page(&["nope"], &owner, &env, never);
        let retired = unknown_page(&["who"], &owner, &env, never);
        if COMPOSED_NATIVE {
            let page = |text: String| {
                Some(Page {
                    text: text.into(),
                    stderr: false,
                    exit: Exit::Ok,
                    lead: None,
                })
            };
            assert_eq!(tier, page(daily_text(false, false).unwrap()));
            assert_eq!(deprecated, page(deprecated_text().unwrap()));
            assert_eq!(chat, page(chat_text(&env).unwrap()));
        } else {
            assert_eq!((tier, deprecated, chat), (None, None, None));
        }
        if UNKNOWN_NATIVE {
            assert_eq!(
                unknown,
                Some(Page {
                    text: daily_text(false, false).unwrap().into(),
                    stderr: false,
                    exit: Exit::Error,
                    lead: Some("Unknown command: nope\n".to_string()),
                })
            );
            assert_eq!(
                retired,
                Some(Page {
                    text: "Error: 'who' is retired. Use instead: k2 connections list\n".into(),
                    stderr: true,
                    exit: Exit::Error,
                    lead: None,
                })
            );
        } else {
            assert_eq!((unknown, retired), (None, None));
        }
        // No port or no token: the gate's, always.
        for no in [
            prelude(false, true, false, false),
            prelude(true, false, false, false),
        ] {
            assert_eq!(
                composed_page(Composed::Deprecated, &["help-deprecated"], &no, &env, never),
                None
            );
            assert_eq!(unknown_page(&["who"], &no, &env, never), None);
        }
        // HOOK_SOCK set and a word whose tool is an id tool: the shell's
        // session checks.
        let sock = prelude(true, true, true, true);
        assert!(tools::is_id(tools::tool_id("fs", "")));
        assert!(
            !crate::route::TABLE.iter().any(|n| n.names("fs")),
            "fs has an arm now"
        );
        assert_eq!(unknown_page(&["fs"], &sock, &env, never), None);
        assert_eq!(
            unknown_page(
                &["nope", "x"],
                &prelude(true, true, false, true),
                &env,
                never
            ),
            unknown_page(&["nope", "x"], &owner, &env, never)
        );
    }
}
