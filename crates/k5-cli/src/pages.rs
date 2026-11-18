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
//! `k2 study` (`cli/src/study-command.sh`): its arguments are read in
//! order, as the shell reads them. The binary answers the list (`study`,
//! `study list`), the usage (`--help` or `-h` anywhere after the errors
//! the shell reports first), and every topic with a page under
//! `cli/text/study`, as text or `--json`. The shell keeps: the list as
//! JSON (it needs the titles, which live in the shell), the topics still
//! written in bash (`app-api`, `app-errors`, `live`, no file), the
//! permission cards page (a topic only while cards are on), an unknown
//! topic, and every error.

use crate::exit::Exit;
use crate::offline::{self, Env, Prelude};
use crate::out::json::{self, Value};
use crate::text::{self, HelpForm};
use crate::{cards, prelude, tools};
use std::borrow::Cow;
use std::path::Path;

/// What a page prints, where, and the exit after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub text: Cow<'static, str>,
    pub stderr: bool,
    pub exit: Exit,
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
        t if t == CARDS_ONLY || t.starts_with('_') => return None,
        t => text::STUDY.iter().find(|(k, _)| *k == t)?.0,
    };
    Some(Study::Page { topic, json })
}

/// `_perm_lines` with cards off: a line ending ` #if-cards` is dropped, a
/// line ending ` #if-no-cards` loses the tag, every line ends in a newline.
pub fn cards_off(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        if line.ends_with(" #if-cards") {
            continue;
        }
        out.push_str(line.strip_suffix(" #if-no-cards").unwrap_or(line));
        out.push('\n');
    }
    out
}

/// The page text of a study topic: the file, the apps page through the
/// card filter, and the two aliases with their line first.
fn study_text(topic: &str) -> Option<Cow<'static, str>> {
    Some(match topic {
        "skins" => format!("{SKINS_LINE}\n\n{}", cards_off(text::study(CARDS_PAGE)?)).into(),
        "llm-accounts" => format!("{LLM_ACCOUNTS_LINE}\n\n{}", text::study("llm-tokens")?).into(),
        t if t == CARDS_PAGE => cards_off(text::study(t)?).into(),
        t => text::study(t)?.into(),
    })
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

/// Whether the shell's `python3` reads `page` as the binary does: a
/// `python3` on `PATH`, no `PYTHON*` setting (an encoding, a path, a
/// startup file), and a page that is ASCII or a locale Python reads as
/// UTF-8 (one that names UTF-8, or C/POSIX/none, which Python 3.7 and
/// later read as UTF-8).
pub fn python_reads_as_we_do(env: &Env, page: &str) -> bool {
    if env.any_named("PYTHON") || !offline::on_path(env, "python3") {
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

/// The study page, or `None` when the shell answers.
pub fn study_page(s: Study, p: &Prelude, env: &Env) -> Option<Page> {
    // With a passport the list, the usage and the apps page ask the daemon
    // whether permission cards are on.
    let asks = match s {
        Study::Usage | Study::Catalog => true,
        Study::Page { topic, .. } => topic == CARDS_PAGE || topic == "skins",
    };
    if asks && p.passport {
        return None;
    }
    let text: Cow<'static, str> = match s {
        Study::Usage => text::study("_usage")?.into(),
        Study::Catalog => text::study("_catalog")?.into(),
        Study::Page { topic, json: false } => study_text(topic)?,
        Study::Page { topic, json: true } => {
            let page = study_text(topic)?;
            if !python_reads_as_we_do(env, &page) {
                return None;
            }
            study_json(topic, &page).into()
        }
    };
    Some(Page {
        text,
        stderr: false,
        exit: Exit::Ok,
    })
}

/// Whether a help form's first word is an open tool (`help`, `--help`,
/// `-h`, `help-advanced`, `help-internal`): the shell's session checks
/// pass it with `HOOK_SOCK` set. A family's own help may not be: its
/// tool may be an `id` tool ([`session_checks`]).
pub fn open_tool(form: &HelpForm) -> bool {
    form.argv
        .first()
        .is_some_and(|w| ["help", "--help", "-h"].contains(w) || w.starts_with("help-"))
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
        text: text::help(form.page)?.into(),
        stderr: form.stderr,
        exit,
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
        // topic, before any --help), unknown topics, the bash pages, and
        // the cards page.
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
            &["permissions"],
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
            python_reads_as_we_do(&Env::of(&all), page)
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
        assert!(!python_reads_as_we_do(
            &Env::of(&[("PATH", "/nonexistent")]),
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
    fn a_passport_leaves_the_card_pages_to_the_shell() {
        let env = Env::of(&[]);
        let with = prelude(false, true, true, true);
        let without = prelude(false, false, false, false);
        for s in [
            Study::Usage,
            Study::Catalog,
            Study::Page {
                topic: "apps",
                json: false,
            },
            Study::Page {
                topic: "skins",
                json: false,
            },
        ] {
            assert_eq!(study_page(s, &with, &env), None, "{s:?}");
            assert!(study_page(s, &without, &env).is_some(), "{s:?}");
        }
        let errors = Study::Page {
            topic: "errors",
            json: false,
        };
        assert_eq!(
            study_page(errors, &with, &env).unwrap().text,
            text::study("errors").unwrap()
        );
        let usage = study_page(Study::Usage, &without, &env).unwrap();
        assert_eq!(
            (usage.text.as_ref(), usage.stderr, usage.exit),
            (text::study("_usage").unwrap(), false, Exit::Ok)
        );
        let skins = study_page(
            Study::Page {
                topic: "skins",
                json: false,
            },
            &without,
            &env,
        )
        .unwrap();
        assert!(skins.text.starts_with(&format!("{SKINS_LINE}\n\n")));
        assert_eq!(
            &skins.text[SKINS_LINE.len() + 2..],
            cards_off(text::study("apps").unwrap())
        );
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
}
