//! The command's name in the CLI's printed text, rewritten for a brand.
//!
//! The pages under `cli/text` and the `--schema` manifest spell the
//! command as the default brand does ([`SOURCE_NAME`]). A build of another
//! brand (`K2_BRAND_DIR`) prints its own name there instead: `build.rs`
//! runs [`rewrite`] over every page and over the manifest before it embeds
//! them. A default build embeds the files as they are.
//!
//! The rule is `scripts/garden/cli-name-flip.py`'s for printed text (a
//! `cli/text` page or a schema entry): a use of the name is rewritten only
//! where it reads as the command (a verb, a flag or a placeholder after
//! it, backticks around it, words such as `run` or `Usage:` before it, or
//! one of a few exact phrases, `COMMAND_PHRASES`).
//! A `[k2 ...]` stamp, a string that is exactly the name (a value) and any
//! other use stay as they are. `text.rs`'s tests hold this port to the
//! Python, file by file.
//!
//! The company and the sign-in service are slots in the same text,
//! `{company}` and `{login}` ([`COMPANY_SLOT`], [`LOGIN_SLOT`]): every
//! build fills them with its brand's `names.company` and `names.login`
//! ([`fill`]; `scripts/build-cli.sh` fills bash's with ours, through
//! `scripts/brand/page_slots.py`), after the rewrite, so a brand's names
//! are never read as the command. In the default brand the filled text is
//! the text as it was before the slots, byte for byte.
//!
//! Used by `build.rs` (by path) and by the tests; nothing at run time.

use std::collections::BTreeSet;
use std::path::Path;

/// The name the sources spell: the default brand's command
/// (`brand/k2/brand.toml`). It changes with the rename's switch, when the
/// sources are rewritten once.
pub const SOURCE_NAME: &str = "k2";

/// Words after the name that make it the command, beside the CLI's verbs
/// (the flip's `EXTRA_VERBS`).
const EXTRA_VERBS: &[&str] = &[
    "help",
    "version",
    "study",
    "update",
    "setup",
    "explain",
    "capabilities",
    "whoami",
    "cli",
    "command",
    "commands",
    "verb",
    "verbs",
];

/// Exact phrases where the name is the command though no other rule says
/// so: the text just before it and just after it (the flip's
/// `COMMAND_PHRASES`, the same rows; its reasons are there).
const COMMAND_PHRASES: &[(&str, &str)] = &[
    ("ran ", " from"),
    ("run ", " from a terminal"),
    ("", " asks for the confirm"),
    ("where ", " uses the owner's token"),
    ("No ", " hostdb"),
];

/// Words that, with spaces after them, put the command after them (the
/// flip's `reads_as_command`, its fourth rule).
const LEADS: &[&str] = &[
    "Usage:", "usage:", "run", "Run", "try", "Try", "use", "Use", "via", "with", "the", "The",
    "see", "See", ":",
];

/// The first words of the CLI's commands, as the flip reads them from
/// `root`: every `"name": "<verb>` in a schema part, every label of the
/// router's arms, and [`EXTRA_VERBS`]. Reads `cli/src/*.sh`,
/// `cli/src/**/*.entries` and `cli/text/**/*.txt` under `root`.
pub fn verbs(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut found: BTreeSet<String> = EXTRA_VERBS.iter().map(|w| w.to_string()).collect();
    let mut files = Vec::new();
    walk(&root.join("cli/src"), &[".sh", ".entries"], &mut files)?;
    walk(&root.join("cli/text"), &[".txt"], &mut files)?;
    for path in files {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if base.ends_with(".entries") || base.starts_with("schema") {
            schema_names(&text, &mut found);
        }
        if base == "router.sh" {
            for line in text.split('\n') {
                router_labels(line, &mut found);
            }
        }
    }
    Ok(found)
}

/// Every file under `dir` whose name ends with one of `ends`. A folder
/// that is not there has none.
fn walk(dir: &Path, ends: &[&str], out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("read {}: {e}", dir.display()))?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, ends, out)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| ends.iter().any(|e| n.ends_with(e)))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// `"name":\s*"([a-z][a-z0-9-]*)`, every match.
fn schema_names(text: &str, found: &mut BTreeSet<String>) {
    const KEY: &str = "\"name\":";
    let mut from = 0;
    while let Some(at) = text[from..].find(KEY) {
        let rest = text[from + at + KEY.len()..].trim_start();
        from += at + KEY.len();
        if let Some(word) = rest.strip_prefix('"').and_then(lead_word) {
            found.insert(word.to_string());
        }
    }
}

/// `^\s+([a-z][a-z0-9-]*(?:\|[a-z][a-z0-9-]*)*)\)`: the words of one
/// router arm's label.
fn router_labels(line: &str, found: &mut BTreeSet<String>) {
    let rest = line.trim_start();
    if rest.len() == line.len() {
        return;
    }
    let mut words = Vec::new();
    let mut tail = rest;
    loop {
        let Some(word) = lead_word(tail) else { return };
        words.push(word);
        tail = &tail[word.len()..];
        match tail.chars().next() {
            Some('|') => tail = &tail[1..],
            Some(')') => break,
            _ => return,
        }
    }
    found.extend(words.into_iter().map(str::to_string));
}

/// `[a-z][a-z0-9-]*` at the start of `s`.
fn lead_word(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_lowercase) {
        return None;
    }
    let end = b
        .iter()
        .position(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-'))
        .unwrap_or(b.len());
    Some(&s[..end])
}

/// A word character as Python's `\w` reads one in a `str`.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `text` with every use of `from` that reads as the command spelled `to`.
/// Line by line; every other byte is kept.
pub fn rewrite(text: &str, from: &str, to: &str, known: &BTreeSet<String>) -> String {
    if from == to {
        return text.to_string();
    }
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| rewrite_line(line, from, to, known))
        .collect();
    lines.join("\n")
}

fn rewrite_line(line: &str, from: &str, to: &str, known: &BTreeSet<String>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    let mut i = 0;
    while let Some(at) = line[i..].find(from) {
        let start = i + at;
        let end = start + from.len();
        i = start + 1;
        while !line.is_char_boundary(i) {
            i += 1;
        }
        let before = &line[..start];
        let after = &line[end..];
        let b = before.chars().next_back();
        let a = after.chars().next();
        // The flip's WORD: (?<![\w./$~-])k2(?![\w.:/-])
        if b.is_some_and(|c| word_char(c) || "./$~-".contains(c))
            || a.is_some_and(|c| word_char(c) || ".:/-".contains(c))
        {
            continue;
        }
        if is_command(before, after, known) {
            out.push_str(&line[last..start]);
            out.push_str(to);
            last = end;
        }
        i = i.max(end);
    }
    out.push_str(&line[last..]);
    out
}

/// The flip's `classify` for printed text: `text` (true) or not.
fn is_command(before: &str, after: &str, known: &BTreeSet<String>) -> bool {
    // A `[k2 ...]` or `[k2]` stamp.
    if before.ends_with('[') && (after.starts_with(']') || after.starts_with(' ')) {
        return false;
    }
    // A string that is exactly the name (with nothing on either side, the
    // whole line is that string).
    let b = before.chars().next_back();
    let a = after.chars().next();
    match (b, a) {
        (None, None) => return false,
        (Some(q @ ('\'' | '"')), Some(c)) if c == q => return false,
        _ => {}
    }
    reads_as_command(before, after, known) || in_command_phrase(before, after)
}

/// The flip's `in_command_phrase`.
fn in_command_phrase(before: &str, after: &str) -> bool {
    COMMAND_PHRASES
        .iter()
        .any(|(b, a)| before.ends_with(b) && after.starts_with(a))
}

/// The flip's `reads_as_command`.
fn reads_as_command(before: &str, after: &str, known: &BTreeSet<String>) -> bool {
    if let Some(tail) = after.strip_prefix(' ') {
        let flag = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_lowercase());
        if tail.strip_prefix("--").is_some_and(flag)
            || tail.strip_prefix('-').is_some_and(flag)
            || tail.starts_with('<')
            || tail.starts_with('[')
            || tail.starts_with('\u{2026}')
            || tail.starts_with("...")
        {
            return true;
        }
        if lead_word(tail).is_some_and(|w| known.contains(w)) {
            return true;
        }
    }
    if before.ends_with('`') && after.trim_start_matches('\\').starts_with('`') {
        return true;
    }
    lead_before(before) && command_end(after)
}

/// `(Usage:|...|:)\s+[`'"]?$` on the text before the name.
fn lead_before(before: &str) -> bool {
    let b = before.strip_suffix(['`', '\'', '"']).unwrap_or(before);
    let trimmed = b.trim_end();
    trimmed.len() < b.len() && LEADS.iter().any(|w| trimmed.ends_with(w))
}

/// `[`'"]?( CLI\b|[`'".,;)]|$)` at the start of the text after the name.
fn command_end(after: &str) -> bool {
    let Some(c) = after.chars().next() else {
        return true;
    };
    if "`'\".,;)".contains(c) {
        return true;
    }
    after
        .strip_prefix(" CLI")
        .is_some_and(|rest| !rest.chars().next().is_some_and(word_char))
}

/// The slot a page or a schema entry holds for the company's name
/// (`names.company`).
pub const COMPANY_SLOT: &str = "{company}";

/// The slot for the sign-in service's name (`names.login`).
pub const LOGIN_SLOT: &str = "{login}";

/// `text` with every [`COMPANY_SLOT`] spelled `company` and every
/// [`LOGIN_SLOT`] spelled `login`. Every other byte is kept.
pub fn fill(text: &str, company: &str, login: &str) -> String {
    text.replace(COMPANY_SLOT, company)
        .replace(LOGIN_SLOT, login)
}

/// `name` as it goes inside a JSON string: the schema manifest's slots
/// sit in its strings, and a brand's name may hold a quote or a
/// backslash (a name is one line, so no other escape is needed).
pub fn json_escaped(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slots_are_filled_and_nothing_else_moves() {
        assert_eq!(
            fill("Tell {company}; {login} ({login}).", "Acme", "Acme ID"),
            "Tell Acme; Acme ID (Acme ID)."
        );
        let other = "{cli} {product} {Company} {company";
        assert_eq!(fill(other, "A", "B"), other);
        assert_eq!(json_escaped(r#"Say "hi" \ bye"#), r#"Say \"hi\" \\ bye"#);
        assert_eq!(json_escaped("Globex Corporation"), "Globex Corporation");
    }

    fn known() -> BTreeSet<String> {
        ["msg", "agent", "help"]
            .iter()
            .map(|w| w.to_string())
            .collect()
    }

    fn xy(line: &str) -> String {
        rewrite(line, "k2", "xy", &known())
    }

    #[test]
    fn a_use_that_reads_as_the_command_is_rewritten() {
        for (line, want) in [
            ("Run k2 msg k2 hi", "Run xy msg k2 hi"),
            ("  k2 agent list", "  xy agent list"),
            ("k2 --help", "xy --help"),
            ("k2 -h", "xy -h"),
            ("k2 <noun>", "xy <noun>"),
            ("k2 [--json]", "xy [--json]"),
            ("k2 \u{2026}", "xy \u{2026}"),
            ("k2 ...", "xy ..."),
            ("type `k2` here", "type `xy` here"),
            ("run k2,", "run xy,"),
            ("Usage: k2", "Usage: xy"),
            ("try 'k2 msg'", "try 'xy msg'"),
            ("the k2 CLI", "the xy CLI"),
            ("k2 msg and k2 agent", "xy msg and xy agent"),
            (
                "or where you ran k2 from inside it",
                "or where you ran xy from inside it",
            ),
            (
                "or run k2 from a terminal outside K2.",
                "or run xy from a terminal outside K2.",
            ),
            (
                "  terminal), k2 asks for the confirm.",
                "  terminal), xy asks for the confirm.",
            ),
            (
                "where k2 uses the owner's token;",
                "where xy uses the owner's token;",
            ),
            ("No k2 hostdb.", "No xy hostdb."),
        ] {
            assert_eq!(xy(line), want, "{line:?}");
        }
    }

    #[test]
    fn every_other_use_stays() {
        for line in [
            "[k2 note] x",
            "[k2]",
            "\"k2\"",
            "'k2'",
            "see 'k2'",
            "run k2.",
            "k2",
            "--mode <off|custom|manager|k2>",
            "default k2; standalone",
            "~/.k2/x",
            "cli/k2",
            "k2-daemon",
            "k2so",
            "$k2",
            "k2.dev",
            "k2: x",
            "k2 nope",
            "the k2 CLIs",
            "K2 msg",
            // Values a program reads, in every brand: the widget API's
            // object and firewall's --managed-by k2.
            "            k2 object's helpers.",
            "  k2     K2 applies it at your provider's firewall",
            "  auto   k2 once an apply was recorded, else owner",
            "ran k2.",
            "No k2",
        ] {
            assert_eq!(xy(line), line, "{line:?}");
        }
    }

    #[test]
    fn the_same_name_is_the_text_itself() {
        let text = "Run k2 msg\nk2 agent\n";
        assert_eq!(rewrite(text, "k2", "k2", &known()), text);
        assert_eq!(xy(text), "Run xy msg\nxy agent\n");
    }

    #[test]
    fn the_verbs_come_from_the_schema_parts_and_the_router() {
        let mut found = BTreeSet::new();
        schema_names(
            "{\"name\": \"mail send\"},\n{\"name\":\n \"agent\"}, \"name\": \"Nope\"",
            &mut found,
        );
        router_labels("    msg|send)", &mut found);
        router_labels("msg2)", &mut found);
        router_labels("    *)", &mut found);
        router_labels("    a|B)", &mut found);
        let want: BTreeSet<String> = ["mail", "agent", "msg", "send"]
            .iter()
            .map(|w| w.to_string())
            .collect();
        assert_eq!(found, want);
    }
}
