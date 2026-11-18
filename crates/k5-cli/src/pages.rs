//! The study and help pages the binary prints itself, from the embedded
//! files ([`crate::text`]), byte for byte what the shell CLI prints.
//!
//! Two steps. [`study`] and [`crate::text::help_form`] read only the
//! arguments: they say which page a command line is, when it is one the
//! binary may answer. [`study_page`] and [`help_page`] then check the
//! shell's environment ([`crate::offline`]): when the shell would print
//! anything else first, stop, or ask the daemon (the permission card
//! probe a passport makes), they give `None` and the command falls
//! through to the shell.
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
use std::borrow::Cow;

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
    let topic = if HEARTBEAT_ALIASES.contains(&topic) { "heartbeats" } else { topic };
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
/// pass it with `HOOK_SOCK` set. A family's own help is not: its tool may
/// be an `id` tool, whose checks look at the session.
pub fn open_tool(form: &HelpForm) -> bool {
    form.argv
        .first()
        .is_some_and(|w| ["help", "--help", "-h"].contains(w) || w.starts_with("help-"))
}

/// The help page, or `None` when the shell answers: the connection gate
/// needs a port and a token for help, and a family's help with
/// `HOOK_SOCK` set runs the session checks.
pub fn help_page(form: &'static HelpForm, p: &Prelude) -> Option<Page> {
    if !(p.port && p.token) || (p.hook_sock && !open_tool(form)) {
        return None;
    }
    let exit = Exit::ALL.into_iter().find(|e| e.code() == i32::from(form.code))?;
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
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src").join(file);
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
        assert_eq!(study(&["", "errors"]), page("errors", false), "an empty topic is no topic");
        assert_eq!(study(&["errors"]), page("errors", false));
        assert_eq!(study(&["errors", "--json"]), page("errors", true));
        assert_eq!(study(&["--json", "errors"]), page("errors", true));
        assert_eq!(study(&["cron"]), page("heartbeats", false));
        assert_eq!(study(&["skins", "--json"]), page("skins", true));
        assert_eq!(study(&["llm-accounts"]), page("llm-accounts", false));
        for usage in [&["--help"][..], &["-h"], &["errors", "--help"], &["--json", "-h"], &["a", "-h", "b"]] {
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
        let aliases = format!("case \"$topic\" in {}) topic=\"heartbeats\" ;; esac", HEARTBEAT_ALIASES.join("|"));
        assert!(command.contains(&aliases), "no `{aliases}` in study-command.sh");
        let skins = src("study-skins.sh");
        assert!(skins.contains("    cat <<'EOF' | _perm_lines\n%include cli/text/study/apps.txt\nEOF\n"), "the apps page's filter moved");
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
            assert!(!HEARTBEAT_ALIASES.contains(key), "{key} is a heartbeats alias");
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
        assert!(apps.lines().any(|l| l.ends_with(" #if-cards")), "the apps page has no card line");
        assert!(!cards_off(apps).contains("#if-"));
    }

    #[test]
    fn the_json_is_python_s() {
        assert_eq!(study_json("x", "a\n"), "{\"id\":\"x\",\"body\":\"a\"}\n");
        assert_eq!(study_json("x", "a\n\n"), "{\"id\":\"x\",\"body\":\"a\\n\"}\n");
        assert_eq!(study_json("x", "\"\\\t\u{2014}\r\n"), "{\"id\":\"x\",\"body\":\"\\\"\\\\\\t\\u2014\"}\n");
        assert_eq!(study_json("x", "a\rb"), "{\"id\":\"x\",\"body\":\"a\\nb\"}\n");
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
        assert!(reads(&[("LC_ALL", ""), ("LC_CTYPE", "de_DE.utf8")], "\u{2014}"));
        assert!(!reads(&[("LANG", "en_US.ISO8859-1")], "\u{2014}"));
        assert!(reads(&[("LANG", "en_US.ISO8859-1")], "ascii"));
        assert!(!reads(&[("PYTHONIOENCODING", "latin-1")], "ascii"));
        assert!(!reads(&[("PYTHONPATH", "/x")], "ascii"));
        assert!(!python_reads_as_we_do(&Env::of(&[("PATH", "/nonexistent")]), "ascii"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn prelude(port: bool, token: bool, passport: bool, hook_sock: bool) -> Prelude {
        Prelude { port, token, passport, hook_sock }
    }

    #[test]
    fn a_passport_leaves_the_card_pages_to_the_shell() {
        let env = Env::of(&[]);
        let with = prelude(false, true, true, true);
        let without = prelude(false, false, false, false);
        for s in [Study::Usage, Study::Catalog, Study::Page { topic: "apps", json: false }, Study::Page { topic: "skins", json: false }] {
            assert_eq!(study_page(s, &with, &env), None, "{s:?}");
            assert!(study_page(s, &without, &env).is_some(), "{s:?}");
        }
        let errors = Study::Page { topic: "errors", json: false };
        assert_eq!(study_page(errors, &with, &env).unwrap().text, text::study("errors").unwrap());
        let usage = study_page(Study::Usage, &without, &env).unwrap();
        assert_eq!((usage.text.as_ref(), usage.stderr, usage.exit), (text::study("_usage").unwrap(), false, Exit::Ok));
        let skins = study_page(Study::Page { topic: "skins", json: false }, &without, &env).unwrap();
        assert!(skins.text.starts_with(&format!("{SKINS_LINE}\n\n")));
        assert_eq!(&skins.text[SKINS_LINE.len() + 2..], cards_off(text::study("apps").unwrap()));
    }

    #[test]
    fn help_needs_the_gate_s_port_and_token() {
        let family = text::help_form(&["mail", "send", "--help"]).expect("mail send --help is a plain print");
        let open = text::HELP_FORMS.iter().find(|f| f.argv.first() == Some(&"help")).expect("a help <w> form");
        assert!(!open_tool(family) && open_tool(open));
        let ok = prelude(true, true, true, false);
        let page = help_page(family, &ok).unwrap();
        assert_eq!((page.text.as_ref(), page.stderr, page.exit), (text::help("mail/send").unwrap(), false, Exit::Ok));
        for no in [prelude(false, true, false, false), prelude(true, false, false, false)] {
            assert_eq!(help_page(family, &no), None);
            assert_eq!(help_page(open, &no), None);
        }
        let sock = prelude(true, true, true, true);
        assert_eq!(help_page(family, &sock), None, "a family's help runs the session checks");
        assert!(help_page(open, &sock).is_some(), "help is an open tool");
        for f in text::HELP_FORMS {
            assert!(Exit::ALL.iter().any(|e| e.code() == i32::from(f.code)), "{f:?}");
        }
    }
}
