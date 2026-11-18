//! The tool table ([`k5_cli::tools`]) against the shell's own functions:
//! `_cli_tool_id_for_verb` and `_cli_tool_default_mode` from the built
//! `cli/k2` run under bash, for every word either function names (each
//! case label), a few that neither does, and every pair of them as the
//! verb and its sub-verb. Then the mode of every tool id either side
//! gives, and of each label of the mode function.

#![cfg(unix)]

mod common;

use std::collections::BTreeSet;

/// The shell's policy block.
fn policy() -> String {
    common::block(
        &common::cli_script(),
        "# BEGIN_CLI_TOOL_POLICY",
        "# END_CLI_TOOL_POLICY",
    )
}

/// Every word a `case` label in `text` names (`a|b|"")` gives `a`, `b`
/// and the empty word).
fn labels(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') || t.starts_with("case ") {
            continue;
        }
        let Some((head, _)) = t.split_once(')') else {
            continue;
        };
        if head.is_empty() || head.contains(' ') || head.contains('(') || head.contains('$') {
            continue;
        }
        for w in head.split('|') {
            let w = w.trim_matches('"');
            if w != "*" {
                out.insert(w.to_string());
            }
        }
    }
    out
}

#[test]
fn the_table_is_the_shell_s() {
    let block = policy();
    let mut words = labels(&block);
    assert!(
        words.len() > 100,
        "only {} words in the policy's labels: {words:?}",
        words.len()
    );
    for extra in ["", "x", "nope", "Mail", "mail ", "--json", "team", "env"] {
        words.insert(extra.to_string());
    }
    let words: Vec<String> = words.into_iter().collect();
    let list = words.join("\n");
    // Builtins only (case, echo, printf), so the cross product is quick.
    let script = format!(
        "{block}\nset -f\nIFS=$'\\n'\nW=($K5T_WORDS)\nunset IFS\n\
         for v in \"${{W[@]}}\" ''; do for s in \"${{W[@]}}\" ''; do \
         printf '%s\\t%s\\t' \"$v\" \"$s\"; _cli_tool_id_for_verb \"$v\" \"$s\"; done; done\n\
         printf -- '--modes--\\n'\n\
         for t in \"${{W[@]}}\" '' tools-policy awareness; do printf '%s\\t' \"$t\"; _cli_tool_default_mode \"$t\"; done\n"
    );
    let out = common::bash(&script, "bash", &[("K5T_WORDS", list.as_str())]);
    let (ids, modes) = out.split_once("--modes--\n").expect("the modes marker");
    let mut rows = 0;
    let mut tools = BTreeSet::new();
    for line in ids.lines() {
        let mut f = line.split('\t');
        let (v, s, want) = (f.next().unwrap(), f.next().unwrap(), f.next().unwrap());
        assert_eq!(k5_cli::tools::tool_id(v, s), want, "tool id of {v:?} {s:?}");
        tools.insert(want.to_string());
        rows += 1;
    }
    assert!(
        rows > 10_000,
        "only {rows} verb and sub-verb pairs compared"
    );
    assert!(tools.len() > 40, "only {} tool ids: {tools:?}", tools.len());
    let mut checked = 0;
    for line in modes.lines() {
        let (t, mode) = line.split_once('\t').expect("a tool and its mode");
        assert!(mode == "id" || mode == "open", "mode {mode:?} of {t:?}");
        assert_eq!(k5_cli::tools::is_id(t), mode == "id", "mode of {t:?}");
        checked += 1;
    }
    assert!(checked > 100, "only {checked} modes compared");
    // Every tool id a verb gives has its mode compared.
    for t in &tools {
        assert!(
            words.contains(t) || t.is_empty(),
            "the tool id {t:?} is not one of the words, so its mode was not compared"
        );
    }
    for t in k5_cli::tools::ID_TOOLS {
        assert!(
            words.iter().any(|w| w == t),
            "{t} is not a label in the shell's block"
        );
    }
}
