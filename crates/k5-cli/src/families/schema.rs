//! `k5 --schema` and `k5 schema` (any words after them): the command
//! surface as JSON, as the shell CLI's `cmd_schema` prints it
//! (`cli/src/schema-head.sh`): the version line,
//!
//! ```text
//! printf '{\n  "tool": "<the command name>",\n  "version": "%s",\n' "$_v"
//! ```
//!
//! with `\` and `"` in the version escaped, then the manifest's quoted
//! heredoc ([`BODY`], joined from the same parts at build time:
//! [`join`]). Nothing is asked of the daemon, and the words after the
//! first are never read.
//!
//! Before it, the shell's prelude ([`crate::offline`]): a setting bash
//! reads at start, a missing program and the like hand the command to the
//! shell. The connection gate is skipped for both words. The virtual
//! desktop's guard is not skipped for `schema` (it is not one of the
//! guard's offline words, `--schema` is), unless a word is `-h` or
//! `--help`: with `NO_OWNER_FALLBACK=1` and no token, `schema` is the
//! guard's two lines and exit 3, as the shell's is, with no plain-http
//! line before them (the shell's guard exits before that step). The
//! session checks pass: the tool is `schema`, an open one, whatever
//! follows.
//!
//! Either word with one help flag after it (`--schema --help`) reaches
//! this family through its help form ([`crate::text::FAMILY_FORMS`]):
//! the shell's router reads no word after it and prints the manifest. Any
//! other line with `-h` or `--help` in it never gets here: the front sends
//! it to the shell ([`crate::front`]).

pub mod join;

use super::help::{shell_prelude, show};
use crate::exit::Exit;
use crate::offline::{Env, Prelude};
use crate::pages::Page;
use crate::prelude::VIRTUAL_DESKTOP;
use k2_client::brand::CLI_NAME;
use k2_client::env_names;
use std::ffi::OsString;

include!(concat!(env!("OUT_DIR"), "/schema.rs"));

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["--schema", "schema"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 --schema ...` or `k5 schema ...`: the page, or `None` when the
/// shell's prelude hands the command to the shell.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let (env, p) = shell_prelude()?;
    if guarded(args, &p, &env) {
        // The guard exits before the shell's plain-http step.
        crate::prelude::plain_http::skip();
    }
    Some(show(&page(args, &p, &env, crate::embedded::version())))
}

/// What `cmd_schema` prints for `version`.
pub fn text(version: &str) -> String {
    let v = version.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{{\n  \"tool\": \"{CLI_NAME}\",\n  \"version\": \"{v}\",\n{BODY}")
}

/// Whether the virtual desktop's guard stops `args` (`_k2_offline_verb`
/// in `cli/src/cli-tools-policy.sh`): `NO_OWNER_FALLBACK=1`, no token, a
/// first word that is not `--schema`, and no `-h` or `--help` word.
fn guarded(args: &[OsString], p: &Prelude, env: &Env) -> bool {
    let no_owner = env
        .first(env_names::NO_OWNER_FALLBACK)
        .is_some_and(|v| v == "1");
    let offline = args.first().is_some_and(|w| w == "--schema")
        || args.iter().any(|w| w == "-h" || w == "--help");
    no_owner && !p.token && !offline
}

/// The page for `args` (the first word `--schema` or `schema`), with the
/// shell's prelude `p` in `env`.
pub fn page(args: &[OsString], p: &Prelude, env: &Env, version: &str) -> Page {
    if guarded(args, p, env) {
        let mut text = String::new();
        for line in VIRTUAL_DESKTOP {
            text.push_str(line);
            text.push('\n');
        }
        return Page {
            text: text.into(),
            stderr: true,
            exit: Exit::Refused,
            lead: None,
        };
    }
    Page {
        text: text(version).into(),
        stderr: false,
        exit: Exit::Ok,
        lead: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn prelude(token: bool) -> Prelude {
        Prelude {
            port: true,
            token,
            passport: false,
            hook_sock: false,
        }
    }

    #[test]
    fn the_version_line_escapes_as_the_shell_does() {
        let t = text("1.2\"3\\4");
        let head = format!("{{\n  \"tool\": \"{CLI_NAME}\",\n  \"version\": \"1.2\\\"3\\\\4\",\n");
        assert!(t.starts_with(&head), "{}", &t[..80]);
        assert!(t.ends_with("  ]\n}\n"), "{}", &t[t.len() - 40..]);
    }

    #[test]
    fn the_guard_stops_schema_but_not_its_offline_forms() {
        let on = Env::of(&[("K2_NO_OWNER_FALLBACK", "1")]);
        let off = Env::of(&[("K2_NO_OWNER_FALLBACK", "yes")]);
        assert!(guarded(&args(&["schema"]), &prelude(false), &on));
        assert!(guarded(&args(&["schema", "json"]), &prelude(false), &on));
        assert!(!guarded(&args(&["schema"]), &prelude(true), &on));
        assert!(!guarded(&args(&["schema"]), &prelude(false), &off));
        assert!(!guarded(&args(&["--schema"]), &prelude(false), &on));
        assert!(!guarded(
            &args(&["schema", "x", "--help"]),
            &prelude(false),
            &on
        ));
        assert!(!guarded(&args(&["schema", "-h"]), &prelude(false), &on));
        let page = page(&args(&["schema"]), &prelude(false), &on, "1.0.0");
        assert_eq!((page.stderr, page.exit), (true, Exit::Refused));
        // `_k2_refuse_owner_fallback`'s two lines.
        let want: String = VIRTUAL_DESKTOP.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(&*page.text, want.as_str());
    }

    /// The session checks never stop it: its tool is an open one with any
    /// second word.
    #[test]
    fn the_tool_is_open() {
        for verb in ["--schema", "schema"] {
            for sub in ["", "json", "mail", "--help", "agent"] {
                assert!(!tools::is_id(tools::tool_id(verb, sub)), "{verb} {sub}");
            }
        }
    }

    #[test]
    fn spec_keys_are_left_out_with_the_comma_before_them() {
        let entry = concat!(
            "    {\n",
            "      \"name\": \"x\",\n",
            "      \"flags\": [],\n",
            "      \"impl\": \"spec\",\n",
            "      \"call\": {\"via\": \"compute\"}\n",
            "    },\n",
            "    {\n",
            "      \"name\": \"y\",\n",
            "      \"flags\": []\n",
            "    },\n",
        );
        let want = concat!(
            "    {\n",
            "      \"name\": \"x\",\n",
            "      \"flags\": []\n",
            "    },\n",
            "    {\n",
            "      \"name\": \"y\",\n",
            "      \"flags\": []\n",
            "    },\n",
        );
        assert_eq!(join::strip_spec_keys(entry).as_deref(), Some(want));
        assert_eq!(join::strip_spec_keys(want).as_deref(), Some(want));
        // No closing line, or no comma to take off: the shell refuses it.
        assert_eq!(
            join::strip_spec_keys("      \"x\": 1,\n      \"impl\": \"bash\"\n"),
            None
        );
        assert_eq!(
            join::strip_spec_keys("      \"x\": 1\n      \"impl\": \"bash\"\n    },\n"),
            None
        );
        // A key that only starts like one stays.
        let near = "      \"x\": 1,\n      \"implied\": 2\n    },\n";
        assert_eq!(join::strip_spec_keys(near).as_deref(), Some(near));
    }

    #[test]
    fn the_join_takes_the_heredoc_and_the_entries_in_order() {
        let order = "# c\nprelude.sh\nschema-head.sh\n\nschema/a.entries\nschema/b.entries\nschema-tail.sh\nrouter.sh\n";
        let read = |n: &str| -> Result<String, String> {
            Ok(match n {
                "schema-head.sh" => "x() {\n    cat <<'EOF'\n  \"commands\": [\n".into(),
                "schema/a.entries" => "A\n".into(),
                "schema/b.entries" => "B\n".into(),
                "schema-tail.sh" => "T\n  ]\n}\nEOF\n}\n".into(),
                _ => return Err(format!("read {n}")),
            })
        };
        assert_eq!(
            join::body(order, read).as_deref(),
            Ok("  \"commands\": [\nA\nB\nT\n  ]\n}\n")
        );
        let bad = "schema-head.sh\nother.sh\nschema-tail.sh\n";
        assert!(join::body(bad, read).is_err());
    }
}
