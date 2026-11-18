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

/// One help command line the binary answers: `argv` exactly (the
/// arguments after the program's name) prints the help page `page` on
/// stdout, or on stderr when `stderr`, and exits with `code`.
#[derive(Debug, PartialEq, Eq)]
pub struct HelpForm {
    pub argv: &'static [&'static str],
    pub page: &'static str,
    pub stderr: bool,
    pub code: u8,
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

/// The help form that is exactly `argv`, if the binary answers it.
pub fn help_form(argv: &[&str]) -> Option<&'static HelpForm> {
    HELP_FORMS.iter().find(|f| f.argv == argv)
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
            for entry in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display())) {
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
            assert!(embedded[key] == *bytes, "cli/text/{which}/{key}.txt differs from the embedded copy (a stale build)");
        }
        assert!(disk.len() > 50, "only {} files under cli/text/{which}", disk.len());
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
        assert_eq!(study("errors"), STUDY.iter().find(|(k, _)| *k == "errors").map(|(_, v)| *v));
        assert_eq!(help("mail/send"), HELP.iter().find(|(k, _)| *k == "mail/send").map(|(_, v)| *v));
        assert_eq!(study("nope"), None);
        assert_eq!(help("mail"), HELP.iter().find(|(k, _)| *k == "mail").map(|(_, v)| *v));
        assert_eq!(help("mail/"), None);
    }

    /// The forms are `src/help_native.txt` as it is on disk now, each
    /// form once, each naming a page there is.
    #[test]
    fn the_forms_are_the_list_on_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/help_native.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<&str> = text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        assert_eq!(rows.len(), HELP_FORMS.len(), "a stale build of {}", path.display());
        for (row, form) in rows.iter().zip(HELP_FORMS) {
            let stream = if form.stderr { "err" } else { "out" };
            assert_eq!(*row, format!("{}|{}|{stream}|{}", form.argv.join(" "), form.page, form.code));
            assert!(help(form.page).is_some(), "{row}");
            let first = help_form(form.argv).unwrap();
            assert!(std::ptr::eq(first, form), "{row} is there twice");
        }
        assert!(HELP_FORMS.len() > 300, "only {} forms", HELP_FORMS.len());
        assert_eq!(help_form(&["mail", "send", "--help", "x"]), None);
        assert_eq!(help_form(&[]), None);
    }
}
