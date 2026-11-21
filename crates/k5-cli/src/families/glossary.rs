//! `glossary [<term>]`: the glossary, as the shell CLI's
//! `cmd_glossary` prints it (`cli/src/glossary.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]). A `-h` or `--help` word
//!    anywhere lets the virtual desktop's guard by (`_k2_offline_verb`):
//!    the connection gate then stops it. `glossary` is the open tool
//!    `help`, so no passport check comes after them.
//! 2. The first word after `glossary` is the term (`${1:-}`); every other
//!    word is never read. `--help` and `-h` are terms like any other.
//! 3. No term (none, or an empty one): two lines on stdout (the title and
//!    an empty one), then `cli_request /cli/glossary/list` over TCP on the
//!    left of a pipe into the shell's Python ([`list_reader`]).
//! 4. A term: `cli_request /cli/glossary/get term=<escaped>` the same way
//!    ([`get_reader`]), the term's bytes escaped as `urlencode` does.
//! 5. The Python's stderr goes to `/dev/null`: an answer it can't read is
//!    its exit 1 and what it printed before, nothing more. The EXIT trap's
//!    lines ([`crate::prelude::Closing`]), with the pipeline's code.
//!
//! A request with no answer is the transport's line, and the command then
//! dies of SIGPIPE, as for [`super::activity`] (after the title, for the
//! list).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! port that is set but is no port curl takes, and a `python3` the binary
//! can't tell ([`crate::families`]). `glossary --help` and `glossary -h`
//! reach [`run`] through their help forms ([`crate::text::FAMILY_FORMS`]):
//! the word is the term (a bash bug: help asks). Any other line with `-h`
//! or `--help` in it never gets here: the front sends it to the shell
//! ([`crate::front`]), which reads the word as a term.

use super::{curl_port, pipefail, py_get, py_iter, py_left, request_piped_tcp, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote_bytes;
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["glossary"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The list's route.
pub const LIST: &str = "/cli/glossary/list";
/// One term's route.
pub const GET: &str = "/cli/glossary/get";
/// The list's first line: `<product> glossary \u{2014} definitions of
/// <product>-specific terms` (prose: the hyphen is the sentence's, not a
/// token's).
pub fn title() -> String {
    let mut t = String::from(PRODUCT_NAME);
    t.push_str(" glossary \u{2014} definitions of ");
    t.push_str(PRODUCT_NAME);
    t.push_str("-specific terms");
    t
}
/// The list's line for no terms.
pub const NO_TERMS: &str = "  (no terms returned)";
/// The list's last line.
pub const RUN_LINE: &str = concat!(
    "Run `",
    cli_name!(),
    " glossary <term>` for a full definition."
);

/// The command line, when the binary answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if gate(args, &settings).is_none() {
        // A port curl won't take is the shell's.
        settings.port_text.as_deref().and_then(curl_port)?;
    }
    Some(run_with(args, settings))
}

/// The gate that stops `args`: the virtual desktop's guard but where a
/// help word lets it by, then the connection gate.
fn gate(args: &[OsString], s: &Settings) -> Option<Gate> {
    let help = args.iter().any(|w| w == "-h" || w == "--help");
    match prelude::gate(s) {
        Some(Gate::VirtualDesktop) if help => Some(Gate::CannotConnect),
        g => g,
    }
}

/// The term's bytes as the shell holds them.
#[cfg(unix)]
fn bytes(w: &OsString) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    w.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn bytes(w: &OsString) -> Vec<u8> {
    w.to_string_lossy().into_owned().into_bytes()
}

fn run_with(args: &[OsString], mut settings: Settings) -> Exit {
    if let Some(gate) = gate(args, &settings) {
        return gate.stop();
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let term = args.get(1).map(bytes).unwrap_or_default();
    let mut closing = Closing::default();
    let exit = if term.is_empty() {
        if let Err(e) = out::stdout(&format!("{}\n\n", title())) {
            return out::after_stdout_error(&e);
        }
        let piped = request_piped_tcp(&mut client, &mut closing, LIST, &[]);
        pipefail(piped.exit, list_reader(&piped.body).print())
    } else {
        let q = format!("term={}", quote_bytes(&term));
        let piped = request_piped_tcp(&mut client, &mut closing, GET, &[q.as_str()]);
        pipefail(piped.exit, get_reader(&piped.body).print())
    };
    closing.flush(&mut client, exit);
    exit
}

/// The Python's exit 1 after `stdout`: its stderr goes nowhere.
fn failed(stdout: String) -> Shown {
    Shown {
        stdout,
        stderr: String::new(),
        exit: Exit::Error,
    }
}

/// The list's Python on the answer:
///
/// ```text
/// try:
///     entries = json.loads(sys.stdin.read() or '[]')
/// except Exception:
///     entries = []
/// if not entries:
///     print('  (no terms returned)')
///     sys.exit(0)
/// for e in entries:
///     print('  %-16s %s' % (e.get('term', '?'), e.get('summary', '')))
/// print()
/// print('Run `k2 glossary <term>` for a full definition.')
/// ```
pub fn list_reader(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let text = if raw.is_empty() { "[]" } else { raw.as_ref() };
    let entries = json::parse(text).unwrap_or(Value::Array(Vec::new()));
    if !py::truthy(&entries) {
        return Shown::out(format!("{NO_TERMS}\n"));
    }
    let mut stdout = String::new();
    let walk = |stdout: &mut String| -> Result<(), Raise> {
        for e in py_iter(&entries, 0)? {
            let term = py_get(&e, "term", 0)?.cloned();
            let summary = py_get(&e, "summary", 0)?.cloned();
            let term = term.unwrap_or_else(|| super::s("?"));
            let summary = summary.unwrap_or_else(|| super::s(""));
            stdout.push_str(&format!(
                "  {} {}\n",
                py_left(&term, 16),
                py::str_of(&summary)
            ));
        }
        Ok(())
    };
    if walk(&mut stdout).is_err() {
        return failed(stdout);
    }
    stdout.push('\n');
    stdout.push_str(RUN_LINE);
    stdout.push('\n');
    Shown::out(stdout)
}

/// One term's Python on the answer:
///
/// ```text
/// try:
///     d = json.loads(sys.stdin.read())
/// except Exception as e:
///     sys.stderr.write('parse: %s\n' % e)
///     sys.exit(1)
/// if 'error' in d:
///     sys.stderr.write('Error: %s\n' % d['error'])
///     sys.exit(1)
/// print('%-16s %s' % (d.get('term', '?'), d.get('definition', '')))
/// ```
///
/// Every way it stops early is exit 1 with nothing printed (its stderr
/// goes nowhere): not JSON, an `error` (a key, a list item, a substring),
/// and anything but an object (`in` or `.get` raises).
pub fn get_reader(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = json::parse(&raw) else {
        return failed(String::new());
    };
    let Value::Object(_) = &d else {
        return failed(String::new());
    };
    let (term, definition) = match (py_get(&d, "term", 0), py_get(&d, "definition", 0)) {
        (Ok(t), Ok(v)) => (t.cloned(), v.cloned()),
        _ => return failed(String::new()),
    };
    if py_get(&d, "error", 0).ok().flatten().is_some() {
        return failed(String::new());
    }
    let term = term.unwrap_or_else(|| super::s("?"));
    let definition = definition.unwrap_or_else(|| super::s(""));
    Shown::out(format!(
        "{} {}\n",
        py_left(&term, 16),
        py::str_of(&definition)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(s: &Shown) -> (&str, &str, Exit) {
        (s.stdout.as_str(), s.stderr.as_str(), s.exit)
    }

    #[test]
    fn the_list() {
        let s = list_reader(br#"[{"term":"cell","summary":"a pane"},{"summary":null},{"term":5}]"#);
        let want = format!(
            "  cell             a pane\n  ?                None\n  5                \n\n{RUN_LINE}\n"
        );
        assert_eq!(out(&s), (want.as_str(), "", Exit::Ok));
        for none in ["", "nope", "[]", "null", "{}", "0", "false", "\"\""] {
            assert_eq!(
                out(&list_reader(none.as_bytes())),
                ("  (no terms returned)\n", "", Exit::Ok),
                "{none}"
            );
        }
        // Python raises part way: what it printed, exit 1.
        let s = list_reader(br#"[{"term":"a"},7,{"term":"b"}]"#);
        assert_eq!(out(&s), ("  a                \n", "", Exit::Error));
        for raise in [r#"{"error":"x"}"#, "5", "true", "\"ab\""] {
            assert_eq!(
                out(&list_reader(raise.as_bytes())),
                ("", "", Exit::Error),
                "{raise}"
            );
        }
    }

    #[test]
    fn one_term() {
        assert_eq!(
            out(&get_reader(
                r#"{"term":"réle","definition":"what you do"}"#.as_bytes()
            )),
            ("r\u{e9}le             what you do\n", "", Exit::Ok)
        );
        assert_eq!(
            out(&get_reader(b"{}")),
            ("?                \n", "", Exit::Ok)
        );
        assert_eq!(
            out(&get_reader(br#"{"term":null,"definition":[1,"a"]}"#)),
            ("None             [1, 'a']\n", "", Exit::Ok)
        );
        for bad in [
            "",
            "nope",
            r#"{"error":null,"term":"a"}"#,
            "[1]",
            r#"["error"]"#,
            "\"an error\"",
            "\"fine\"",
            "5",
            "null",
        ] {
            assert_eq!(
                out(&get_reader(bad.as_bytes())),
                ("", "", Exit::Error),
                "{bad}"
            );
        }
    }

    /// Every name a line prints is the brand's (a `K2_BRAND_DIR=brand/xy`
    /// build says `xy`).
    #[test]
    fn the_lines_name_the_brand() {
        let (cli, product) = (k2_client::brand::CLI_NAME, k2_client::brand::PRODUCT_NAME);
        assert_eq!(
            title(),
            format!("{product} glossary \u{2014} definitions of {product}-specific terms")
        );
        assert_eq!(
            RUN_LINE,
            format!("Run `{cli} glossary <term>` for a full definition.")
        );
    }
}
