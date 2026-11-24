//! Every exit code the shell CLI can end with has a name on
//! [`k5_cli::exit::Exit`], so the parity gate can say both sides speak the
//! same codes.
//!
//! The shell's own codes are read from `cli/src`: every `exit <number>`
//! where a command can stand (start of line, after `;`, `&&`, `||`, `{`,
//! `(`, `then`, `else` or `do`), outside comments, including the end of a
//! quoted `trap` command (`...; exit 130"`). Codes the shell passes
//! on from the daemon or a child (`exit "$rc"`) can't be read that way;
//! they are listed in [`PASSED_ON`], each with where it is documented.

mod common;

use k5_cli::exit::Exit;
use std::collections::BTreeMap;

/// Codes the shell CLI ends with without a literal `exit <number>`: the
/// daemon sends them and the shell passes them on.
const PASSED_ON: &[(i32, &str)] = &[
    (6, "doc.sh: `6 the docs store failed`, sent by the daemon"),
    (7, "cli-tools-policy.sh: curl exit 7, nothing accepted the connection; _k2_curl returns it"),
    (8, "doc.sh and study-docs-live.sh: too old, sent by the daemon (revert_too_old, base_expired)"),
    (26, "cli-tools-policy.sh: curl exit 26, it couldn't read its input (Git for Windows' /dev/fd/3)"),
    (124, "compute-jobs.sh and compute-lib.sh: wait --timeout passed first, exit \"${_CP_DEADLINE_RC:-3}\" and K2_CP_LIMIT_RC"),
];

/// The front's own codes, which the shell script never uses: 69 no
/// Python 3 for a hand-off and 70 a hand-off in a build with no shell CLI
/// (`k2 study errors` names both), 127 a call by an alias in mode
/// tombstone, 9009 a command not built into k5 for Windows.
const FRONT_ONLY: &[i32] = &[69, 70, 127, 9009];

/// The literal `exit <number>` codes in one line of shell, where a command
/// can stand. Comments are skipped.
fn literal_exits(line: &str) -> Vec<i32> {
    let code_part = line.trim_start();
    if code_part.starts_with('#') {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find("exit ") {
        let start = from + at;
        from = start + 5;
        let before = line[..start].trim_end();
        let at_command = before.is_empty()
            || [";", "&&", "||", "{", "(", "then", "else", "do"]
                .iter()
                .any(|t| {
                    before.ends_with(t)
                        && (t.len() == 1
                            || !before[..before.len() - t.len()]
                                .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
                });
        if !at_command {
            continue;
        }
        let digits: String = line[from..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            continue;
        }
        let rest = &line[from + digits.len()..];
        if rest.is_empty()
            || rest.starts_with(|c: char| {
                c.is_whitespace() || matches!(c, ';' | '}' | ')' | '"' | '\'')
            })
        {
            found.push(digits.parse().expect("a short number"));
        }
    }
    found
}

#[test]
fn the_reader_finds_exits_where_a_command_stands_only() {
    assert_eq!(literal_exits("    exit 2"), vec![2]);
    assert_eq!(
        literal_exits("  [ -n \"$x\" ] || { echo no >&2; exit 3; }"),
        vec![3]
    );
    assert_eq!(
        literal_exits("if a; then exit 4; else exit 5; fi"),
        vec![4, 5]
    );
    assert_eq!(literal_exits("  cmd && exit 0"), vec![0]);
    assert_eq!(literal_exits("  # exit 9 in a comment"), Vec::<i32>::new());
    assert_eq!(
        literal_exits("  [compute] job <id>: exit 100 after 6m"),
        Vec::<i32>::new()
    );
    assert_eq!(literal_exits("  exit \"$rc\""), Vec::<i32>::new());
    assert_eq!(literal_exits("  exit 12x"), Vec::<i32>::new());
    assert_eq!(literal_exits("  redo exit 7"), Vec::<i32>::new());
    assert_eq!(
        literal_exits("    trap \"rm -f x; echo detached >&2; exit 130\" INT"),
        vec![130]
    );
    assert_eq!(
        literal_exits("    trap 'echo cancelled >&2; exit 130' INT TERM"),
        vec![130]
    );
}

#[test]
fn every_code_the_shell_cli_ends_with_is_on_the_enum() {
    let mut seen: BTreeMap<i32, String> = BTreeMap::new();
    for (file, text) in common::shell_sources() {
        for (n, line) in text.lines().enumerate() {
            for code in literal_exits(line) {
                seen.entry(code)
                    .or_insert_with(|| format!("{file}:{}", n + 1));
            }
        }
    }
    assert!(
        seen.contains_key(&2) && seen.contains_key(&3),
        "the reader found too little: {seen:?}"
    );
    for (code, why) in PASSED_ON {
        assert!(
            !seen.contains_key(code),
            "{code} is now a literal exit at {}; drop it from PASSED_ON ({why})",
            seen[code]
        );
        seen.insert(*code, (*why).to_string());
    }
    let named: Vec<i32> = Exit::ALL.iter().map(|e| e.code()).collect();
    for (code, at) in &seen {
        assert!(
            named.contains(code),
            "the shell CLI ends with {code} ({at}) and Exit has no name for it"
        );
    }
    let shell: Vec<i32> = seen.keys().copied().collect();
    let unused: Vec<i32> = named
        .iter()
        .copied()
        .filter(|c| !shell.contains(c) && !FRONT_ONLY.contains(c))
        .collect();
    assert!(
        unused.is_empty(),
        "Exit names codes the shell CLI never ends with and the front doesn't own: {unused:?}"
    );
}
