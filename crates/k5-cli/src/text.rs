//! The CLI's static text, embedded from the same files the shell CLI
//! prints: `cli/text/study/<id>.txt` ([`STUDY`], by `<id>`) and
//! `cli/text/help/<words>.txt` ([`HELP`], by `<words>` with `/` between
//! them: `mail/send`). `scripts/build-cli.sh` inlines each file into
//! `cli/k2` at its `%include` line, and `build.rs` embeds the very same
//! files here, so the two print the same bytes.
//!
//! [`HELP_FORMS`] is `src/help_native.txt`: the help command lines the
//! binary answers itself. Each is one in which the shell CLI only prints
//! one of these files; `tests/cli/help_native.sh` proves that from the
//! shell's own trace and writes the list.
//!
//! [`CAPTURED_FORMS`] is `src/help_captured.txt`: every other help
//! command line of the router that the shell only prints for, the same
//! in every environment (the same script proves it and writes the list),
//! with what it printed ([`CAPTURED`]). That text names the command and
//! the product by the brand ([`render`]). A captured form whose page bash
//! puts through the permission cards filter, or that prints chat's usage,
//! has a page for each way its switch goes ([`Variant`]); a `zen` form
//! runs the router arm's Garden changes hint first ([`Variant::GardenHint`]).

/// One help command line the binary answers: `argv` exactly (the
/// arguments after the program's name) prints the page `page` ([`page`]:
/// a help file, or `study:<key>` for a study file) on stdout, or on
/// stderr when `stderr`, and exits with `code`. A captured form
/// (`captured`, its page in [`CAPTURED`]) has no `page`: it prints what
/// the shell printed. One with a `variant` has a second page, for the
/// other way its switch goes; `stderr` and `code` are its first page's.
#[derive(Debug, PartialEq, Eq)]
pub struct HelpForm {
    pub argv: &'static [&'static str],
    pub page: &'static str,
    pub stderr: bool,
    pub code: u8,
    pub captured: Option<usize>,
    pub variant: Option<Variant>,
}

/// What picks a captured form's page when one switch does, and the other
/// page (a [`CAPTURED`] index). `tests/cli/help_native.sh` captures both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The page goes through the permission cards filter (`_perm_lines`):
    /// `captured` while cards are off for this caller (with no passport
    /// nothing is asked and they are off), this page while they are on.
    /// The binary asks as the shell asks ([`crate::cards`]).
    CardsOn(usize),
    /// The page is chat's usage (`_chat_usage`): `captured` while
    /// `_CHAT_HELP_MANAGE` is unset, empty or `1`, this page while it is
    /// anything else ([`crate::pages::chat_manage`]).
    NoManage(usize),
    /// A `zen` form: the router arm runs `_zen_garden_changes_hint` before
    /// the page ([`crate::families::zen::changes_hint_in`]): once per
    /// workspace, a line on stderr and a marker in its `.k2`. The page is
    /// `captured` either way.
    GardenHint,
}

/// What the shell printed for a captured help form: the exit code, and
/// stdout and stderr as `src/help_captured.txt` holds them ([`render`]
/// gives the text).
#[derive(Debug, PartialEq, Eq)]
pub struct Captured {
    pub code: u8,
    pub out: &'static str,
    pub err: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/text.rs"));

fn find(list: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    list.binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| list[i].1)
}

/// `cli/text/study/<key>.txt`.
pub fn study(key: &str) -> Option<&'static str> {
    find(STUDY, key)
}

/// `cli/text/help/<key>.txt`.
pub fn help(key: &str) -> Option<&'static str> {
    find(HELP, key)
}

/// A help form's page: `study:<key>` is [`study`]`(key)` (`help study`
/// prints the study usage), any other key [`help`]`(key)`.
pub fn page(key: &str) -> Option<&'static str> {
    match key.strip_prefix("study:") {
        Some(k) => study(k),
        None => help(key),
    }
}

/// The help form that is exactly `argv`, if the binary answers it: a
/// file's print ([`HELP_FORMS`]), or a captured form ([`captured_form`]).
pub fn help_form(argv: &[&str]) -> Option<&'static HelpForm> {
    HELP_FORMS
        .iter()
        .find(|f| f.argv == argv)
        .or_else(|| captured_form(argv))
}

/// The captured help form that is exactly `argv` ([`CAPTURED_FORMS`],
/// in argv order).
pub fn captured_form(argv: &[&str]) -> Option<&'static HelpForm> {
    CAPTURED_FORMS
        .binary_search_by(|f| f.argv.cmp(argv))
        .ok()
        .map(|i| &CAPTURED_FORMS[i])
}

/// A captured text as it prints: `\\`, `\n` and `\t` are a backslash,
/// a newline and a tab, `\c` the command's name
/// ([`k2_client::brand::CLI_NAME`]) and `\p` the product's
/// ([`k2_client::brand::PRODUCT_NAME`]); in the default build those are
/// `k2` and `K2`, as the shell printed them. `build.rs` lets no other
/// escape in.
pub fn render(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('c') => out.push_str(k2_client::brand::CLI_NAME),
            Some('p') => out.push_str(k2_client::brand::PRODUCT_NAME),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn text_dir(which: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/text")
            .join(which)
    }

    /// Every `.txt` under `dir` on disk now, by key, with its bytes.
    fn on_disk(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for entry in
                std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display()))
            {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    todo.push(path);
                } else if path.extension().is_some_and(|x| x == "txt") {
                    let rel = path.strip_prefix(dir).unwrap().with_extension("");
                    let key: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_str().unwrap().to_string())
                        .collect();
                    out.insert(key.join("/"), std::fs::read(&path).unwrap());
                }
            }
        }
        out
    }

    fn same_as_disk(list: &[(&str, &str)], which: &str) {
        let disk = on_disk(&text_dir(which));
        let embedded: BTreeMap<String, Vec<u8>> = list
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect();
        let only_disk: Vec<&String> = disk.keys().filter(|k| !embedded.contains_key(*k)).collect();
        let only_here: Vec<&String> = embedded.keys().filter(|k| !disk.contains_key(*k)).collect();
        assert!(
            only_disk.is_empty() && only_here.is_empty(),
            "cli/text/{which}: on disk only {only_disk:?}, embedded only {only_here:?} (a stale build)"
        );
        for (key, bytes) in &disk {
            assert!(
                embedded[key] == *bytes,
                "cli/text/{which}/{key}.txt differs from the embedded copy (a stale build)"
            );
        }
        assert!(
            disk.len() > 50,
            "only {} files under cli/text/{which}",
            disk.len()
        );
    }

    /// A build that missed a change to cli/text can't pass: the key set is
    /// the file set, and each page is the file's bytes now.
    #[test]
    fn the_embedded_pages_are_the_files_on_disk() {
        same_as_disk(STUDY, "study");
        same_as_disk(HELP, "help");
    }

    #[test]
    fn the_lists_are_in_key_order_so_the_lookup_finds_every_page() {
        for list in [STUDY, HELP] {
            assert!(list.windows(2).all(|w| w[0].0 < w[1].0), "not in key order");
            for (key, page) in list {
                assert_eq!(find(list, key), Some(*page), "{key}");
            }
        }
        assert_eq!(
            study("errors"),
            STUDY.iter().find(|(k, _)| *k == "errors").map(|(_, v)| *v)
        );
        assert_eq!(
            help("mail/send"),
            HELP.iter()
                .find(|(k, _)| *k == "mail/send")
                .map(|(_, v)| *v)
        );
        assert_eq!(study("nope"), None);
        assert_eq!(
            help("mail"),
            HELP.iter().find(|(k, _)| *k == "mail").map(|(_, v)| *v)
        );
        assert_eq!(help("mail/"), None);
    }

    /// The forms are `src/help_native.txt` as it is on disk now, each
    /// form once, each naming a page there is.
    #[test]
    fn the_forms_are_the_list_on_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/help_native.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<&str> = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(
            rows.len(),
            HELP_FORMS.len(),
            "a stale build of {}",
            path.display()
        );
        for (row, form) in rows.iter().zip(HELP_FORMS) {
            let stream = if form.stderr { "err" } else { "out" };
            assert_eq!(
                *row,
                format!(
                    "{}|{}|{stream}|{}",
                    form.argv.join(" "),
                    form.page,
                    form.code
                )
            );
            assert!(page(form.page).is_some(), "{row}");
            let first = help_form(form.argv).unwrap();
            assert!(std::ptr::eq(first, form), "{row} is there twice");
        }
        assert!(HELP_FORMS.len() > 300, "only {} forms", HELP_FORMS.len());
        assert_eq!(help_form(&["mail", "send", "--help", "x"]), None);
        assert_eq!(page("study:_usage"), study("_usage"));
        assert_eq!(page("mail"), help("mail"));
        assert_eq!(page("study:nope"), None);
        assert_eq!(help_form(&[]), None);
    }

    /// The captured forms are `src/help_captured.txt` as it is on disk
    /// now: each form once, in argv order, with its page's code and text.
    #[test]
    fn the_captured_forms_are_the_list_on_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/help_captured.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut pages = Vec::new();
        let mut forms: Vec<(Vec<&str>, usize, Option<Variant>)> = Vec::new();
        for line in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let cols: Vec<&str> = line.split('\t').collect();
            let n = |s: &str| s.parse::<usize>().unwrap();
            match cols[..] {
                ["P", _, code, out, err] => pages.push((code.parse::<u8>().unwrap(), out, err)),
                ["F", argv, page] => forms.push((argv.split(' ').collect(), n(page), None)),
                ["C", argv, off, on] => forms.push((
                    argv.split(' ').collect(),
                    n(off),
                    Some(Variant::CardsOn(n(on))),
                )),
                ["M", argv, page, other] => forms.push((
                    argv.split(' ').collect(),
                    n(page),
                    Some(Variant::NoManage(n(other))),
                )),
                ["Z", argv, page] => forms.push((
                    argv.split(' ').collect(),
                    n(page),
                    Some(Variant::GardenHint),
                )),
                _ => panic!("{}: {line:?}", path.display()),
            }
        }
        let embedded: Vec<(u8, &str, &str)> =
            CAPTURED.iter().map(|c| (c.code, c.out, c.err)).collect();
        assert_eq!(pages, embedded, "a stale build of {}", path.display());
        forms.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(forms.len(), CAPTURED_FORMS.len(), "a stale build");
        for ((argv, page, variant), form) in forms.iter().zip(CAPTURED_FORMS) {
            assert_eq!(form.argv, &argv[..]);
            assert_eq!(form.captured, Some(*page), "{argv:?}");
            assert_eq!(form.variant, *variant, "{argv:?}");
            if let Some(Variant::CardsOn(other) | Variant::NoManage(other)) = variant {
                assert!(*other < pages.len(), "{argv:?}");
            }
            assert_eq!(form.code, pages[*page].0, "{argv:?}");
            assert_eq!(form.page, "");
            assert!(std::ptr::eq(help_form(argv).unwrap(), form), "{argv:?}");
            assert_eq!(help_form(argv), captured_form(argv));
        }
        assert!(
            CAPTURED_FORMS.len() > 1000,
            "only {} forms",
            CAPTURED_FORMS.len()
        );
        for f in HELP_FORMS {
            assert_eq!(captured_form(f.argv), None, "{:?} is in both lists", f.argv);
            assert_eq!(f.captured, None);
            assert_eq!(f.variant, None);
        }
        assert_eq!(captured_form(&["agent", "nope", "--help"]), None);
    }

    #[test]
    fn render_reads_the_escapes() {
        let cli = k2_client::brand::CLI_NAME;
        let product = k2_client::brand::PRODUCT_NAME;
        assert_eq!(
            render("Usage: \\c agent\\n\\tx \\\\n \\p's"),
            format!("Usage: {cli} agent\n\tx \\n {product}'s")
        );
        assert_eq!(render(""), "");
        assert_eq!(render("plain \u{2014} text"), "plain \u{2014} text");
    }
}
