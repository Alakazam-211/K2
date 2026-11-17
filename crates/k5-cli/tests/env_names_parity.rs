//! The shell CLI and `k2_client::env_names` agree on every setting the
//! shell reads under more than one name, so the Rust CLI, which reads
//! through the registry, tries the same names in the same order.
//!
//! The shell reads such a setting in one of these ways (`cli/src/*.sh`):
//!
//! - `_envn_into VAR NAME`, `_envn NAME`, `_envn_any NAME...`: the reader
//!   in `prelude.sh` tries `K5_NAME`, `K2_NAME`, `K2SO_NAME`.
//! - inline, `${K5_X:-${K2_X:-${K2SO_X:-}}}` or
//!   `${K5_X:-}${K2_X:-}${K2SO_X:-}`.
//! - in embedded Python, `os.environ.get("K5_X") or os.environ.get("K2_X")
//!   or ...`.
//!
//! Every such read must match a registry entry name for name, and every
//! entry of the registry's command line section must be read somewhere. Any
//! other `K5_` name in the shell's code fails too, so a new way of reading
//! one can't slip past.

mod common;

use k2_client::env_names;
use k5_cli::called_as::FRONT_VARS;
use std::collections::{BTreeMap, BTreeSet};

/// One read the shell makes: where, and the names it tries, in order.
#[derive(Debug)]
struct Read {
    at: String,
    names: Vec<String>,
}

/// The names the Rust front hands the shell: one name each, not settings.
const FRONT: &[&str] = &FRONT_VARS;

/// Reads in cli/src that do not match the registry today and wait for a
/// ruling: the file, the names it tries in its order, and why it is left.
/// Each must still be there (the list only shrinks): fix the read, then
/// drop its row.
const KNOWN_DRIFT: &[(&str, &[&str], &str)] = &[(
    "cli/src/vault.sh",
    &["K2_HOOK_SOCK", "K2_HOOK_TOKEN", "K2SO_HOOK_SOCK", "K2SO_HOOK_TOKEN"],
    "the --scope/--workspace refusal for agents misses K5_HOOK_SOCK and K5_HOOK_TOKEN; \
     vault code, changed only with K2's sign-off (`_envn_any HOOK_SOCK HOOK_TOKEN` is the fix)",
)];

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The upper-case name at the start of `s` (`[A-Z][A-Z0-9_]*`), if any.
fn name_at(s: &str) -> Option<&str> {
    let end = s.bytes().position(|b| !(b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')).unwrap_or(s.len());
    let name = &s[..end];
    name.bytes().next().is_some_and(|b| b.is_ascii_uppercase()).then_some(name)
}

/// The prefix of a registry name: `K5_`, `K2_` or `K2SO_`.
fn prefixed(name: &str) -> bool {
    env_names::PREFIXES.iter().any(|p| name.strip_prefix(p).is_some_and(|rest| !rest.is_empty()))
}

/// The names the prelude's reader tries for `NAME`, read from the reader
/// itself: the quoted words of its `for _envn_n in ...` line, with the
/// name's variable dropped.
fn reader_prefixes(function: &str, var: &str) -> Vec<String> {
    let prelude = common::prelude();
    let body = common::block(&prelude, &format!("{function}() {{"), "}");
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("for _envn_n in "))
        .unwrap_or_else(|| panic!("{function}: no `for _envn_n in` line"));
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(|word| word.strip_suffix(var).unwrap_or_else(|| panic!("{function}: {word:?} does not end in {var}")).to_string())
        .collect()
}

#[test]
fn the_shell_reader_tries_the_registry_prefixes_in_order() {
    assert_eq!(reader_prefixes("_envn_into", "$2"), env_names::PREFIXES);
    assert_eq!(reader_prefixes("_envn_any", "$_envn_a"), env_names::PREFIXES);
}

/// The `_envn*` calls on one line.
fn envn_calls(line: &str, at: &str, reads: &mut Vec<Read>, errors: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(off) = line[from..].find("_envn") {
        let start = from + off;
        from = start + 1;
        if start > 0 && is_ident(bytes[start - 1]) {
            continue;
        }
        let end = start + line[start..].bytes().position(|b| !is_ident(b)).unwrap_or(line.len() - start);
        let word = &line[start..end];
        if !matches!(word, "_envn" | "_envn_into" | "_envn_any") || bytes.get(end) != Some(&b' ') {
            continue;
        }
        let mut args = line[end..].split_whitespace();
        if word == "_envn_into" {
            args.next();
        }
        let mut names = Vec::new();
        for arg in args {
            match name_at(arg) {
                Some(name) => {
                    names.push(name.to_string());
                    if word != "_envn_any" || name.len() < arg.len() {
                        break;
                    }
                }
                None => break,
            }
        }
        if names.is_empty() {
            if line.trim() != r#"_envn_into _envn_out "$@""# {
                errors.push(format!("{at}: {word} with no literal setting name: {}", line.trim()));
            }
            continue;
        }
        for name in names {
            reads.push(Read { at: at.to_string(), names: env_names::PREFIXES.iter().map(|p| format!("{p}{name}")).collect() });
        }
    }
}

/// `${A:-${B:-...` and `${A:-}${B:-}...` from `at`: the names, and where
/// the chain ends.
fn shell_chain(line: &str, mut at: usize) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    while let Some(rest) = line[at..].strip_prefix("${") {
        let Some(name) = name_at(rest) else { break };
        let after = &rest[name.len()..];
        let Some(after) = after.strip_prefix(":-") else { break };
        names.push(name.to_string());
        at = line.len() - after.len();
        if after.starts_with("}${") {
            at += 1;
        }
    }
    (names, at)
}

/// `("A") or x.get("B") or ...` from `at` (the opening parenthesis): the
/// names, and where the chain ends.
fn python_chain(line: &str, at: usize) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut rest = &line[at..];
    let opening_quote = |r: &str| r.strip_prefix('(').and_then(|r| r.chars().next()).filter(|q| *q == '"' || *q == '\'');
    while let Some(quote) = opening_quote(rest) {
        let inner = &rest[2..];
        let Some(name) = name_at(inner) else { break };
        let Some(after) = inner[name.len()..].strip_prefix(quote).and_then(|r| r.strip_prefix(')')) else { break };
        names.push(name.to_string());
        rest = after;
        // ` or <callee>(` continues the chain.
        let Some(next) = after.trim_start().strip_prefix("or ") else { break };
        let next = next.trim_start();
        let callee = next.bytes().position(|b| !(is_ident(b) || b == b'.')).unwrap_or(next.len());
        if callee == 0 || !next[callee..].starts_with('(') {
            break;
        }
        rest = &next[callee..];
    }
    (names, line.len() - rest.len())
}

/// A chain's leading names that carry a registry prefix.
fn prefixed_run(names: Vec<String>) -> Vec<String> {
    names.into_iter().take_while(|n| prefixed(n)).collect()
}

/// The inline reads on one line, shell and Python, and the `K5_` names
/// they account for.
fn inline_reads(line: &str, at: &str, reads: &mut Vec<Read>, errors: &mut Vec<String>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut from = 0;
    while from < line.len() {
        let shell = line[from..].find("${K").map(|i| (from + i, true));
        let python = ["(\"K", "('K"].iter().filter_map(|pat| line[from..].find(pat).map(|i| (from + i, false))).min();
        let Some((start, is_shell)) = [shell, python].into_iter().flatten().min() else { break };
        let (names, end) = if is_shell { shell_chain(line, start) } else { python_chain(line, start) };
        from = end.max(start + 1);
        let names = prefixed_run(names);
        match names.as_slice() {
            [] => {}
            [one] if FRONT.contains(&one.as_str()) => {
                seen.insert(one.clone());
            }
            [one] if !one.starts_with("K5_") => {}
            _ => {
                seen.extend(names.iter().filter(|n| n.starts_with("K5_")).cloned());
                if names.len() == 1 {
                    errors.push(format!("{at}: {} is read under one name; the registry's entries have two or more", names[0]));
                }
                reads.push(Read { at: at.to_string(), names });
            }
        }
    }
    seen
}

/// Every read of a setting in `cli/src`, with every unexplained `K5_`
/// name as an error.
fn shell_reads() -> (Vec<Read>, Vec<String>) {
    let mut reads = Vec::new();
    let mut errors = Vec::new();
    for (file, text) in common::shell_sources() {
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let at = format!("cli/src/{file}:{}", n + 1);
            envn_calls(line, &at, &mut reads, &mut errors);
            let seen = inline_reads(line, &at, &mut reads, &mut errors);
            // Any other K5_ name on a code line: an assignment, or a read
            // this test does not know.
            let bytes = line.as_bytes();
            for (i, _) in line.match_indices("K5_") {
                if i > 0 && is_ident(bytes[i - 1]) {
                    continue;
                }
                let Some(name) = name_at(&line[i..]) else { continue };
                if name == "K5_" || seen.contains(name) || FRONT.contains(&name) || bytes.get(i + name.len()) == Some(&b'=') {
                    continue;
                }
                errors.push(format!("{at}: {name} is read in a way this test does not know: {}", line.trim()));
            }
        }
    }
    (reads, errors)
}

#[test]
fn every_setting_the_shell_reads_is_in_the_registry_with_the_same_names() {
    let (reads, mut errors) = shell_reads();
    assert!(reads.len() > 50, "only {} reads found in cli/src: the scan is broken", reads.len());
    let mut by_names: BTreeMap<&[String], Vec<&str>> = BTreeMap::new();
    let mut drift_seen = vec![false; KNOWN_DRIFT.len()];
    for read in &reads {
        let file = read.at.split(':').next().expect("a file");
        let known = KNOWN_DRIFT.iter().position(|(f, names, _)| *f == file && names.iter().eq(read.names.iter()));
        match known {
            Some(i) => drift_seen[i] = true,
            None => by_names.entry(read.names.as_slice()).or_default().push(&read.at),
        }
    }
    for ((file, names, _), seen) in KNOWN_DRIFT.iter().zip(drift_seen) {
        if !seen {
            errors.push(format!("{file}: the known drift {names:?} is gone; drop its row from KNOWN_DRIFT"));
        }
    }
    for (names, ats) in &by_names {
        if env_names::ALL.iter().any(|entry| entry.iter().eq(names.iter())) {
            continue;
        }
        let near: Vec<&[&str]> =
            env_names::ALL.iter().copied().filter(|entry| entry.iter().any(|n| names.iter().any(|m| m == n))).collect();
        let registry = if near.is_empty() { "no entry".to_string() } else { format!("{near:?}") };
        errors.push(format!("{}: the shell reads {names:?} (at {}); the registry has {registry}", names[0], ats.join(", ")));
    }
    for entry in env_names::CLI {
        if !by_names.keys().any(|names| entry.iter().eq(names.iter())) {
            errors.push(format!("{}: {entry:?} is in the registry's command line section but cli/src never reads it", entry[0]));
        }
    }
    assert!(errors.is_empty(), "{} problems:\n{}", errors.len(), errors.join("\n"));
}

/// `K2_AIRGAP` is the one setting read under one name that counts as set
/// when empty (`${K2_AIRGAP+x}`), as `k2_client::airgap` reads it. It is
/// not in the registry, and no reader there must take it.
#[test]
fn the_air_gap_switch_stays_one_name_and_set_when_empty() {
    assert_eq!(k2_client::airgap::ENV_VAR, "K2_AIRGAP");
    assert!(common::prelude().contains(r#"if [ "${K2_AIRGAP+x}" = "x" ]; then"#), "prelude.sh reads K2_AIRGAP another way");
    assert!(!env_names::ALL.iter().any(|e| e.iter().any(|n| n.ends_with("_AIRGAP"))), "the air-gap switch is in the registry");
}

/// The prelude's reader and `first_nonempty_in` give the same answer for
/// every mix of the three names unset, empty and set.
#[cfg(unix)]
#[test]
fn the_shell_reader_and_first_nonempty_agree() {
    const NAMES: &[&str] = &["K5_PARITY_PROBE", "K2_PARITY_PROBE", "K2SO_PARITY_PROBE"];
    let prelude = common::prelude();
    let script = format!(
        "{}\n{}\n_envn_into v PARITY_PROBE fallback\nprintf '%s|' \"$v\"\nif _envn_any PARITY_PROBE; then echo any; else echo none; fi",
        common::block(&prelude, "_envn_into() {", "}"),
        common::block(&prelude, "_envn_any() {", "}"),
    );
    // Each name unset, empty, or set to a value that says which it is.
    let states = |name: &'static str| [None, Some(""), Some(name)];
    let mut wrong = Vec::new();
    for a in states("from-k5") {
        for b in states("from-k2") {
            for c in states("from-k2so") {
                let env: Vec<(&str, &str)> =
                    NAMES.iter().zip([a, b, c]).filter_map(|(n, v)| v.map(|v| (*n, v))).collect();
                let shell = common::bash(&script, "k2", &env);
                let lookup = |name: &str| env.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
                let rust = env_names::first_nonempty_in(NAMES, lookup);
                let want = match &rust {
                    Some((_, v)) => format!("{v}|any\n"),
                    None => "fallback|none\n".to_string(),
                };
                if shell != want {
                    wrong.push(format!("{env:?}: shell {shell:?}, rust {rust:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} mixes differ:\n{}", wrong.len(), wrong.join("\n"));
}
