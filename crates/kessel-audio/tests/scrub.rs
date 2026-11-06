//! Publish hygiene: the crate's files must not name internal hosts, paths,
//! environment variables or products of the team that wrote it. The
//! patterns are assembled at run time so this file doesn't match itself.
//!
//! This is the repository's own check. It stays out of the published package
//! (see `exclude` in Cargo.toml), because its list is itself those names.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read dir") {
        let e = e.expect("entry");
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn product() -> String {
    ["K", "2"].concat()
}

fn banned() -> Vec<String> {
    let k = product();
    let lower = k.to_lowercase();
    vec![
        format!("{k}_"),
        format!("~/.{lower}"),
        format!(".{lower}/"),
        format!("{lower}.dev"),
        format!("{lower}::"),
        format!("{lower}-daemon"),
        format!("{lower}-core"),
        format!("{lower}n1"),
        format!("{lower}n9"),
        format!("{lower}-arch"),
        format!("z3{}", "mbp"),
        format!("sew{}", "lab"),
        format!("{}-211", "Alakazam"),
    ]
}

/// `pattern` appears in `line` at the start of a word, so a pattern is not
/// found on the tail of a longer name. A pattern that starts with punctuation
/// matches anywhere.
fn has_pattern(line: &str, pattern: &str) -> bool {
    let word_start = pattern.starts_with(|c: char| c.is_ascii_alphanumeric());
    line.match_indices(pattern).any(|(at, _)| {
        !word_start
            || !line[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The README's one line saying where the crate is used may name the product.
fn may_name_product(file_name: &str, line: &str) -> bool {
    file_name == "README.md"
        && line
            .trim_start()
            .starts_with(&format!("Used in {}", product()))
}

/// The manifest's `repository` line, exactly. crates.io and docs.rs link the
/// source from it, and the source lives in the product's own repository, so
/// this one line names the organisation and the product. Nothing else may:
/// not another line of Cargo.toml, not a variant of this one, not another file.
fn repository_line() -> String {
    format!("repository = \"{}\"", repository_url())
}

/// The source repository's URL. Allowlisted for publishing: the README may
/// link it (and pages under it, such as its Security tab).
fn repository_url() -> String {
    format!("https://github.com/{}-211/{}", "Alakazam", product())
}

/// In the README, each whole occurrence of the repository URL, or a path
/// under it, is replaced by a placeholder before the line is checked. Only
/// the URL is exempt: the same names elsewhere on the line are still caught,
/// and a longer name that starts like the URL (another repository) is not
/// exempt.
fn without_repository_urls(file_name: &str, line: &str) -> String {
    if file_name != "README.md" {
        return line.to_string();
    }
    let url = repository_url();
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find(&url) {
        let after = &rest[at + url.len()..];
        let mut next = after.chars();
        let whole = match next.next() {
            None => true,
            Some('/' | ')' | '>' | '"' | '\'' | ' ' | ',' | ';' | ':') => true,
            // A full stop that ends the sentence, not a longer host name.
            Some('.') => next.next().is_none_or(char::is_whitespace),
            Some(_) => false,
        };
        out.push_str(&rest[..at]);
        out.push_str(if whole { "<repository>" } else { &url });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What's wrong with one line of one file.
fn line_hits(file_name: &str, line: &str, banned: &[String]) -> Vec<String> {
    if file_name == "Cargo.toml" && line == repository_line() {
        return Vec::new();
    }
    let line = &without_repository_urls(file_name, line);
    let k = product();
    let mut hits: Vec<String> = banned
        .iter()
        .filter(|b| has_pattern(line, b))
        .map(|b| format!("{b:?}"))
        .collect();
    // The product name as a word.
    let named = line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == k);
    if named && !may_name_product(file_name, line) {
        hits.push(format!("the word {k:?}"));
    }
    hits
}

#[test]
fn no_internal_names() {
    let banned = banned();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut all = Vec::new();
    files(root, &mut all);
    assert!(all.len() > 10, "found only {} files", all.len());
    let mut hits = Vec::new();
    for f in &all {
        // The test speech corpus is binary Opus data (see tests/data/README.md).
        if f.extension().is_some_and(|e| e == "kaop") {
            continue;
        }
        let text = std::fs::read_to_string(f)
            .unwrap_or_else(|e| panic!("{}: not UTF-8 text: {e}", f.display()));
        let name = f.file_name().expect("file name").to_string_lossy();
        for (i, line) in text.lines().enumerate() {
            for hit in line_hits(&name, line, &banned) {
                hits.push(format!("{}:{}: {hit}", f.display(), i + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "internal names found:\n{}",
        hits.join("\n")
    );
}

#[test]
fn a_pattern_matches_the_whole_word_not_the_tail_of_a_name() {
    let lower = product().to_lowercase();
    let path = format!("{lower}::");
    assert!(has_pattern(&format!("use {path}thing;"), &path));
    assert!(has_pattern(&format!("{path}thing"), &path));
    assert!(has_pattern(&format!("({path}thing)"), &path));
    assert!(
        !has_pattern(&format!("use bloc{path}Thing;"), &path),
        "the tail of a crate name"
    );
    assert!(!has_pattern(&format!("use a_{path}Thing;"), &path));
    // Either occurrence counts.
    assert!(has_pattern(&format!("bloc{path}a and {path}b"), &path));

    // A host name is still found after a dot, and a dotted path anywhere.
    let host = format!("{lower}.dev");
    assert!(has_pattern(&format!("https://a.{host}/"), &host));
    let dir = format!(".{lower}/");
    assert!(has_pattern(&format!("a{dir}b"), &dir));
}

#[test]
fn only_the_used_in_line_of_the_readme_may_name_the_product() {
    let k = product();
    let banned = banned();
    let used_in = format!("Used in {k}.");
    assert_eq!(
        line_hits("README.md", &used_in, &banned),
        Vec::<String>::new()
    );
    assert_eq!(
        line_hits("README.md", &format!("  {used_in}"), &banned),
        Vec::<String>::new()
    );

    let elsewhere = format!("Works well with {k}.");
    assert_eq!(line_hits("README.md", &elsewhere, &banned).len(), 1);
    assert_eq!(
        line_hits("lib.rs", &used_in, &banned).len(),
        1,
        "README only"
    );

    // The allowance covers the word, not the other names.
    let host = format!("{used_in} See {}.dev.", k.to_lowercase());
    assert_eq!(line_hits("README.md", &host, &banned).len(), 1);
}

#[test]
fn only_the_exact_repository_line_of_the_manifest_may_name_the_organisation() {
    let banned = banned();
    let line = repository_line();
    assert_eq!(
        line_hits("Cargo.toml", &line, &banned),
        Vec::<String>::new()
    );

    // The same line in another file, or changed at all, is still caught: once
    // for the organisation and once for the product. (The README is the
    // other place the URL itself may appear; see the next test.)
    assert_eq!(line_hits("lib.rs", &line, &banned).len(), 2);
    assert_eq!(
        line_hits("Cargo.toml", &format!("  {line}"), &banned).len(),
        2
    );
    assert_eq!(
        line_hits("Cargo.toml", &format!("{line} # note"), &banned).len(),
        2
    );
    let homepage = line.replacen("repository", "homepage", 1);
    assert_eq!(line_hits("Cargo.toml", &homepage, &banned).len(), 2);
    let org_only = format!("homepage = \"https://github.com/{}-211\"", "Alakazam");
    assert_eq!(line_hits("Cargo.toml", &org_only, &banned).len(), 1);
}

#[test]
fn the_readme_may_link_the_repository_url_and_nothing_else_by_those_names() {
    let banned = banned();
    let url = repository_url();
    let none = Vec::<String>::new();

    // The URL, a page under it, in a Markdown link or at the end of a sentence.
    assert_eq!(line_hits("README.md", &url, &banned), none);
    assert_eq!(
        line_hits("README.md", &format!("See {url}/security."), &banned),
        none
    );
    assert_eq!(
        line_hits("README.md", &format!("[the repository]({url})"), &banned),
        none
    );
    assert_eq!(
        line_hits("README.md", &format!("<{url}/security>"), &banned),
        none
    );
    assert_eq!(
        line_hits("README.md", &format!("Source: {url}."), &banned),
        none
    );

    // Only the URL is exempt: the names elsewhere on the same line are caught.
    let k = product();
    let org = format!("{}-211", "Alakazam");
    assert_eq!(
        line_hits("README.md", &format!("{url} is {k}'s."), &banned).len(),
        1
    );
    assert_eq!(
        line_hits("README.md", &format!("{url} by {org}"), &banned).len(),
        1
    );

    // A longer name that starts like the URL is another repository.
    assert_eq!(
        line_hits("README.md", &format!("{url}-private"), &banned).len(),
        2
    );
    assert_eq!(
        line_hits("README.md", &format!("{url}SO/security"), &banned).len(),
        1
    );
    assert_eq!(
        line_hits("README.md", &format!("{url}.example"), &banned).len(),
        2
    );

    // Outside the README the URL is still caught.
    assert_eq!(
        line_hits("lib.rs", &format!("// {url}/security"), &banned).len(),
        2
    );
}
