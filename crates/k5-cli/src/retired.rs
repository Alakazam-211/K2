//! The retired verbs (`_K2_RETIRED_VERBS`, `cli/src/deprecation.sh`):
//! words the CLI no longer runs, each with a pointer to what replaced it.
//! The shell uses the table twice:
//!
//! - `_retired_verb_exit "<verb> [<sub>]"`: before the router says
//!   `Unknown command` (and the `agent` arm says `Unknown agent
//!   subcommand`), a retired verb's pointer on one line on stderr, then
//!   exit 1 ([`line`]).
//! - `_retired_verbs_map`: the deprecated tier of the help
//!   (`k2 help deprecated`, `k2 help-deprecated`) lists every row between
//!   its two files ([`map`]).
//!
//! [`TABLE`] is a copy of the shell's array, row for row; the tests below
//! read `cli/src/deprecation.sh` and fail when the two drift, or when the
//! two functions no longer print what [`line`] and [`map`] print.

/// `_K2_RETIRED_VERBS`: each row is the old verb (one word, or two) and
/// its pointer, in the shell's order.
pub const TABLE: &[(&str, &str)] = &[
    ("agents create", "k2 skills create <name> --role \"...\""),
    ("agents running", "k2 connections list  (a person: k2 workspace list --running)"),
    ("agents list", "k2 connections list  (or k2 skills list; a person: k2 workspace list)"),
    ("agents work", "k2 inbox"),
    ("agents status", "k2 connections list  (a person: k2 workspace list)"),
    ("agents delete", "k2 skills remove <name>"),
    ("agents remove", "k2 skills remove <name>"),
    (
        "agents launch",
        concat!(
            "your harness's own session start (",
            k2_client::product_name!(),
            " no longer launches agents)"
        ),
    ),
    ("agents generate-md", "k2 skills regenerate [<name>]"),
    ("agents profile", "k2 skills profile <name>"),
    ("agents lock", "(nothing replaces it)"),
    ("agents unlock", "(nothing replaces it)"),
    ("agents triage", "k2 workspace triage"),
    ("agents reap", "(nothing replaces it)"),
    ("agent create", "k2 skills create <name> --role \"...\""),
    ("agent delete", "k2 agent retire <name>"),
    ("agent remove", "k2 agent retire <name>"),
    (
        "work create",
        "k2 inbox compose --title \"...\" --body \"...\"",
    ),
    (
        "work send",
        "k2 msg <workspace> --inbox-silent <file>  (or --inbox-wake)",
    ),
    (
        "work move",
        "k2 agent complete --agent <name> --file <file>",
    ),
    ("work inbox", "k2 inbox"),
    ("work done", "k2 inbox archive <id>"),
    (
        "delegate",
        "your harness's sub-agent feature, with k2 skills profile <agent>",
    ),
    ("mode", "k2 settings --mode custom  (or off|manager|k2)"),
    ("who", "k2 connections list"),
    ("roster", "k2 connections list"),
    ("whatsnew", "k2 whats-new"),
    ("commit-merge", "k2 commit --merge"),
    ("feed", "k2 activity"),
    (
        "signal",
        "k2 msg <workspace> --signal <kind> [--payload <json>]",
    ),
    ("companion", "k2 users pair-phone"),
    ("agentic", "(agentic mode is always on)"),
];

/// The pointer for `key` (`"<verb> <sub>"` or `"<verb>"`), when it is a
/// retired verb.
pub fn find(key: &str) -> Option<&'static str> {
    TABLE.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The router's two looks, in its order: `"$1 ${2:-}"`, then `"$1"`. The
/// key and its pointer, when either names a retired verb.
pub fn lookup(verb: &str, sub: &str) -> Option<(String, &'static str)> {
    let two = format!("{verb} {sub}");
    if let Some(new) = find(&two) {
        return Some((two, new));
    }
    find(verb).map(|new| (verb.to_string(), new))
}

/// What `_retired_verb_exit` prints on stderr for `key` (newline
/// included) before it exits 1: a pointer in parentheses is a sentence of
/// its own, any other is what to use instead.
pub fn line(key: &str, new: &str) -> String {
    if new.starts_with('(') {
        format!("Error: '{key}' is retired {new}. Run 'k2 help-deprecated' for the full map.\n")
    } else {
        format!("Error: '{key}' is retired. Use instead: {new}\n")
    }
}

/// `_retired_verbs_map`: `printf '  %-31s → %s\n'` for each row. The keys
/// are ASCII, so bash's width in bytes is Rust's in characters.
pub fn map() -> String {
    let mut out = String::new();
    for (key, new) in TABLE {
        out.push_str(&format!("  {key:<31} \u{2192} {new}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn deprecation_sh() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/deprecation.sh");
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    /// The shell's array, read as bash reads it: each row one
    /// double-quoted word on a line of its own, whose only escape is `\"`.
    fn shell_rows() -> Vec<(String, String)> {
        let text = deprecation_sh();
        let start = "_K2_RETIRED_VERBS=(\n";
        let at = text
            .find(start)
            .expect("_K2_RETIRED_VERBS in deprecation.sh")
            + start.len();
        let body = &text[at..at + text[at..].find("\n)\n").expect("the array's end")];
        body.lines()
            .map(|l| {
                let word = l
                    .strip_prefix("    \"")
                    .and_then(|r| r.strip_suffix('"'))
                    .unwrap_or_else(|| panic!("not one quoted row: {l:?}"));
                for bad in ['$', '`'] {
                    assert!(!word.contains(bad), "a row bash would expand: {l:?}");
                }
                let unescaped = word.replace("\\\"", "\"");
                assert!(!unescaped.contains('\\'), "another escape in {l:?}");
                let (k, v) = unescaped
                    .split_once('|')
                    .unwrap_or_else(|| panic!("no | in {l:?}"));
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    #[test]
    fn the_table_is_the_shell_s_row_for_row() {
        let shell = shell_rows();
        let ours: Vec<(String, String)> = TABLE
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            ours, shell,
            "k5_cli::retired::TABLE drifted from deprecation.sh"
        );
        assert!(TABLE.len() > 20, "only {} rows", TABLE.len());
        for (key, _) in TABLE {
            // `"$1 ${2:-}"` for a verb alone ends in a space: no key may.
            let words: Vec<&str> = key.split(' ').collect();
            assert!(
                (1..=2).contains(&words.len()) && words.iter().all(|w| !w.is_empty()),
                "{key:?}"
            );
            assert!(
                key.is_ascii() && key.len() <= 31,
                "{key:?}: the map's width"
            );
            assert_eq!(
                TABLE.iter().filter(|(k, _)| k == key).count(),
                1,
                "{key} twice"
            );
        }
    }

    /// The two functions print what [`line`] and [`map`] print.
    #[test]
    fn the_functions_are_the_shell_s() {
        let text = deprecation_sh();
        let exit = "_retired_verb_exit() {\n    local key=\"$1\" row new\n    for row in \"${_K2_RETIRED_VERBS[@]}\"; do\n        [ \"${row%%|*}\" = \"$key\" ] || continue\n        new=\"${row#*|}\"\n        case \"$new\" in\n            \"(\"*) echo \"Error: '$key' is retired $new. Run 'k2 help-deprecated' for the full map.\" >&2 ;;\n            *)    echo \"Error: '$key' is retired. Use instead: $new\" >&2 ;;\n        esac\n        exit 1\n    done\n}\n";
        assert!(text.contains(exit), "_retired_verb_exit changed");
        let map_fn = "_retired_verbs_map() {\n    local row\n    for row in \"${_K2_RETIRED_VERBS[@]}\"; do\n        printf '  %-31s \u{2192} %s\\n' \"${row%%|*}\" \"${row#*|}\"\n    done\n}\n";
        assert!(text.contains(map_fn), "_retired_verbs_map changed");
        assert_eq!(
            line("who", "k2 connections list"),
            "Error: 'who' is retired. Use instead: k2 connections list\n"
        );
        assert_eq!(
            line("agentic", "(agentic mode is always on)"),
            "Error: 'agentic' is retired (agentic mode is always on). Run 'k2 help-deprecated' for the full map.\n"
        );
        let first = map().lines().next().unwrap().to_string();
        assert_eq!(
            first,
            "  agents create                   \u{2192} k2 skills create <name> --role \"...\""
        );
        assert_eq!(map().lines().count(), TABLE.len());
    }

    #[test]
    fn two_words_first_then_one() {
        assert_eq!(
            lookup("agents", "create"),
            Some((
                "agents create".to_string(),
                "k2 skills create <name> --role \"...\""
            ))
        );
        assert_eq!(lookup("agents", ""), None);
        assert_eq!(lookup("agents", "nope"), None);
        assert_eq!(
            lookup("who", "x"),
            Some(("who".to_string(), "k2 connections list"))
        );
        assert_eq!(
            lookup("who", ""),
            Some(("who".to_string(), "k2 connections list"))
        );
        assert_eq!(lookup("Who", ""), None);
        assert_eq!(lookup("nope", "who"), None);
    }
}
