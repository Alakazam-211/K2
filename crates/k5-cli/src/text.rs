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
//! with what it printed ([`CAPTURED`]). That text names the command, the
//! product, the company and the sign-in service by the brand ([`render`]).
//!
//! The pages name the company and the sign-in service by slot (`{company}`,
//! `{login}`), which `build.rs` fills with this build's brand: what is
//! embedded holds the names, never a slot. A captured form whose page bash
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
    /// A line the shell's verb reads as any other (no help page: `-h` or
    /// `--help` is a word it takes, or ignores), which the family's own
    /// code answers as the shell does ([`FAMILY_FORMS`],
    /// [`crate::pages::help_page`]). No page of its own.
    Family,
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
/// file's print ([`HELP_FORMS`]), a captured form ([`captured_form`]), or
/// a line its family answers ([`FAMILY_FORMS`]).
pub fn help_form(argv: &[&str]) -> Option<&'static HelpForm> {
    HELP_FORMS
        .iter()
        .find(|f| f.argv == argv)
        .or_else(|| captured_form(argv))
        .or_else(|| FAMILY_FORMS.iter().find(|f| f.argv == argv))
}

/// A [`Variant::Family`] form.
macro_rules! family {
    ($($word:literal)+) => {
        HelpForm {
            argv: &[$($word),+],
            page: "",
            stderr: false,
            code: 0,
            captured: None,
            variant: Some(Variant::Family),
        }
    };
}

/// The help lines whose family answers them ([`Variant::Family`]): the
/// front sends a line with `-h` or `--help` to a page, never to the
/// family, so these reach the family's code through their page.
///
/// `permission` ([`crate::families::permission`]): `cmd_permission` asks
/// whether the family exists first (the owner token over TCP for the
/// owner, the passport's probe for an agent; `beta` with no probe for the
/// owner), and off is `Unknown command: permission` and the daily tier.
/// On, help wins (`_permission_cmd`'s `_wants_help`): the usage, and
/// nothing sent but the probe.
///
/// `--schema` and `schema` ([`crate::families::schema`]): the router arm
/// reads no word after it, so `--schema --help` prints the manifest, the
/// version line and all (not a captured page: every release moves the
/// version, and the manifest names other versions too).
///
/// `approve verify` and its aliases `show` and `key-id` (the words go to
/// `k2-approve`, which answers its own help,
/// [`crate::families::approve`]); `hostchat media`, `people` (alias
/// `person`) and `approvals` (alias `approval`): the flag is the
/// sub-command: `media`'s help page, or the usage line
/// ([`crate::families::hostchat`]).
///
/// `permission`'s aliases `ask` (`request`), `deny` (`approve`) and
/// `beta off` (`beta on`) are the family's as their verbs are.
///
/// `mail`'s owner verbs that moved to `hostmail`
/// ([`crate::families::mail::RETIRED`], but `create`, whose help is a
/// captured page): the router arm reads no help flag after them, so `-h`
/// or `--help` there is the moved-verb usage line, exit 2, nothing sent.
///
/// `done --help` and `done -h` ([`crate::families::done`]): the help is
/// the loop's, after the gates (`_done_usage`).
pub static FAMILY_FORMS: &[HelpForm] = &[
    family!("permission" "--help"),
    family!("permission" "-h"),
    family!("permission" "request" "--help"),
    family!("permission" "request" "-h"),
    family!("permission" "ask" "--help"),
    family!("permission" "ask" "-h"),
    family!("permission" "update" "--help"),
    family!("permission" "update" "-h"),
    family!("permission" "status" "--help"),
    family!("permission" "status" "-h"),
    family!("permission" "cancel" "--help"),
    family!("permission" "cancel" "-h"),
    family!("permission" "list" "--help"),
    family!("permission" "list" "-h"),
    family!("permission" "pending" "--help"),
    family!("permission" "pending" "-h"),
    family!("permission" "approve" "--help"),
    family!("permission" "approve" "-h"),
    family!("permission" "deny" "--help"),
    family!("permission" "deny" "-h"),
    family!("permission" "beta" "--help"),
    family!("permission" "beta" "-h"),
    family!("permission" "beta" "on" "--help"),
    family!("permission" "beta" "on" "-h"),
    family!("permission" "beta" "off" "--help"),
    family!("permission" "beta" "off" "-h"),
    family!("permission" "beta" "status" "--help"),
    family!("permission" "beta" "status" "-h"),
    family!("permission" "approvers" "--help"),
    family!("permission" "approvers" "-h"),
    family!("permission" "agent" "--help"),
    family!("permission" "agent" "-h"),
    family!("--schema" "--help"),
    family!("--schema" "-h"),
    family!("schema" "--help"),
    family!("schema" "-h"),
    family!("done" "--help"),
    family!("done" "-h"),
    family!("approve" "verify" "--help"),
    family!("approve" "verify" "-h"),
    family!("approve" "show" "--help"),
    family!("approve" "show" "-h"),
    family!("approve" "key-id" "--help"),
    family!("approve" "key-id" "-h"),
    family!("hostchat" "media" "--help"),
    family!("hostchat" "media" "-h"),
    family!("hostchat" "people" "--help"),
    family!("hostchat" "people" "-h"),
    family!("hostchat" "person" "--help"),
    family!("hostchat" "person" "-h"),
    family!("hostchat" "approvals" "--help"),
    family!("hostchat" "approvals" "-h"),
    family!("hostchat" "approval" "--help"),
    family!("hostchat" "approval" "-h"),
    family!("mail" "status" "--help"),
    family!("mail" "status" "-h"),
    family!("mail" "domain" "--help"),
    family!("mail" "domain" "-h"),
    family!("mail" "doctor" "--help"),
    family!("mail" "doctor" "-h"),
    family!("mail" "config" "--help"),
    family!("mail" "config" "-h"),
    family!("mail" "approvals" "--help"),
    family!("mail" "approvals" "-h"),
    family!("mail" "catchall" "--help"),
    family!("mail" "catchall" "-h"),
    family!("mail" "alias" "--help"),
    family!("mail" "alias" "-h"),
    family!("mail" "ooo" "--help"),
    family!("mail" "ooo" "-h"),
    family!("mail" "footer" "--help"),
    family!("mail" "footer" "-h"),
    family!("mail" "spam" "--help"),
    family!("mail" "spam" "-h"),
    family!("mail" "queue" "--help"),
    family!("mail" "queue" "-h"),
    family!("mail" "acl" "--help"),
    family!("mail" "acl" "-h"),
    family!("mail" "autoconfig" "--help"),
    family!("mail" "autoconfig" "-h"),
    family!("mail" "app-password" "--help"),
    family!("mail" "app-password" "-h"),
    family!("mail" "person" "--help"),
    family!("mail" "person" "-h"),
    family!("mail" "dkim" "--help"),
    family!("mail" "dkim" "-h"),
    family!("mail" "dmarc" "--help"),
    family!("mail" "dmarc" "-h"),
    family!("mail" "ptr" "--help"),
    family!("mail" "ptr" "-h"),
    family!("mail" "bans" "--help"),
    family!("mail" "bans" "-h"),
    family!("mail" "allowlist" "--help"),
    family!("mail" "allowlist" "-h"),
    family!("mail" "audit-sieve" "--help"),
    family!("mail" "audit-sieve" "-h"),
];

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
/// ([`k2_client::brand::CLI_NAME`]), `\p` the product's
/// ([`k2_client::brand::PRODUCT_NAME`]), `\o` the company's
/// ([`k2_client::brand::COMPANY_NAME`]), `\l` the sign-in service's
/// ([`k2_client::brand::LOGIN_NAME`]) and `\m` the contact address
/// ([`k2_client::brand::CONTACT_EMAIL`]); in the default build those are
/// `k2`, `K2`, `Alakazam Labs`, `K2 Connect` and our contact address, as
/// the shell printed them. `build.rs` lets no other escape in.
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
            Some('o') => out.push_str(k2_client::brand::COMPANY_NAME),
            Some('l') => out.push_str(k2_client::brand::LOGIN_NAME),
            Some('m') => out.push_str(k2_client::brand::CONTACT_EMAIL),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The rewrite `build.rs` runs over the pages in a brand's build (by
/// path there too): the tests below hold the embedded pages to it, and it
/// to the flip's rule.
#[cfg(test)]
#[path = "brand_text.rs"]
mod brand_text;

#[cfg(test)]
mod tests {
    use super::*;
    use k2_client::brand::{COMPANY_NAME, CONTACT_EMAIL, LOGIN_NAME, REPO_URL};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The CLI's verbs, as `build.rs` reads them for the rewrite.
    fn verbs() -> &'static BTreeSet<String> {
        static VERBS: OnceLock<BTreeSet<String>> = OnceLock::new();
        VERBS.get_or_init(|| brand_text::verbs(&root()).unwrap())
    }

    /// A page's bytes as this build embeds them: the file's, with the
    /// command's name rewritten in a brand's build (`K2_BRAND_DIR`), then
    /// the slots filled with the brand's names.
    fn as_built(bytes: Vec<u8>) -> Vec<u8> {
        let name = k2_client::brand::CLI_NAME;
        let text = String::from_utf8(bytes).expect("a page is UTF-8");
        let text = if name == brand_text::SOURCE_NAME {
            text
        } else {
            brand_text::rewrite(&text, brand_text::SOURCE_NAME, name, verbs())
        };
        brand_text::fill(&text, &this_brand()).into_bytes()
    }

    /// The values this build fills the slots with.
    fn this_brand() -> brand_text::Values<'static> {
        brand_text::Values {
            company: COMPANY_NAME,
            login: LOGIN_NAME,
            contact_email: CONTACT_EMAIL,
            repo: REPO_URL,
        }
    }

    /// `[names] <key>` of the default brand's file (`brand/k2`): what the
    /// sources' slots are filled with in the default build.
    fn default_name(key: &str) -> String {
        default_value("names", key)
    }

    /// `[<table>] <key>` of the default brand's file, a one-line string.
    fn default_value(table: &str, key: &str) -> String {
        let toml = std::fs::read_to_string(root().join("brand/k2/brand.toml")).unwrap();
        let head = format!("[{table}]");
        let prefix = format!("{key} = \"");
        let value = toml
            .lines()
            .skip_while(|l| l.trim() != head)
            .take_while(|l| !l.starts_with('[') || l.trim() == head)
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .and_then(|l| l.strip_suffix('"'));
        value
            .unwrap_or_else(|| panic!("brand/k2/brand.toml has no [{table}] {key}"))
            .to_string()
    }

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
                embedded[key] == as_built(bytes.clone()),
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
    /// the file set, and each page is the file's bytes now (in a brand's
    /// build, with its command's name, [`as_built`]).
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

    /// Each family form is once, in neither other list, and its line is
    /// a `Native` family's.
    #[test]
    fn the_family_forms_are_native_families_lines() {
        for (i, f) in FAMILY_FORMS.iter().enumerate() {
            assert_eq!(f.variant, Some(Variant::Family), "{:?}", f.argv);
            assert!(!HELP_FORMS.iter().any(|h| h.argv == f.argv), "{:?}", f.argv);
            assert_eq!(captured_form(f.argv), None, "{:?}", f.argv);
            assert!(std::ptr::eq(help_form(f.argv).unwrap(), f), "{:?}", f.argv);
            assert!(
                !FAMILY_FORMS[..i].iter().any(|g| g.argv == f.argv),
                "{:?} twice",
                f.argv
            );
            let args: Vec<std::ffi::OsString> = f.argv.iter().map(Into::into).collect();
            let found = crate::route::find(crate::route::TABLE, &args);
            assert_eq!(found.owner, crate::route::Owner::Native, "{:?}", f.argv);
            assert!(f.argv.iter().any(|w| *w == "-h" || *w == "--help"));
        }
    }

    /// The default build embeds the files byte for byte: the default
    /// brand's command is the name the sources spell.
    #[test]
    fn the_sources_spell_the_default_brand_s_command() {
        let toml = std::fs::read_to_string(root().join("brand/k2/brand.toml")).unwrap();
        let name = toml
            .lines()
            .skip_while(|l| l.trim() != "[cli]")
            .find_map(|l| l.strip_prefix("name = \""))
            .and_then(|l| l.strip_suffix('"'))
            .expect("brand/k2/brand.toml has [cli] name");
        assert_eq!(name, brand_text::SOURCE_NAME);
        if k2_client::brand::BRAND_ID == "k2" {
            assert_eq!(k2_client::brand::CLI_NAME, brand_text::SOURCE_NAME);
            assert_eq!(as_built(b"Run k2 msg".to_vec()), b"Run k2 msg");
        }
    }

    /// The Rust rewrite is the flip's rule: `scripts/garden/cli-name-flip.py
    /// --apply --to xy` on a copy of `cli/` gives, for every `cli/text`
    /// page and every schema entries file, what [`brand_text::rewrite`]
    /// gives for the file. Needs `python3` (every build node has it).
    #[test]
    fn the_rewrite_is_the_flip_s_rule() {
        let tmp = std::env::temp_dir().join(format!(
            "k5-brand-text-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let path = entry.unwrap().path();
                let dest = to.join(path.file_name().unwrap());
                if path.is_dir() {
                    copy(&path, &dest);
                } else {
                    std::fs::copy(&path, &dest).unwrap();
                }
            }
        }
        for part in ["src", "text"] {
            copy(&root().join("cli").join(part), &tmp.join("cli").join(part));
        }
        let run = std::process::Command::new("python3")
            .arg(root().join("scripts/garden/cli-name-flip.py"))
            .args(["--apply", "--to", "xy", "--root"])
            .arg(&tmp)
            .output()
            .expect("run python3 scripts/garden/cli-name-flip.py");
        assert!(
            run.status.success(),
            "the flip failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let mut files = Vec::new();
        for (dir, end) in [("cli/text", ".txt"), ("cli/src/schema", ".entries")] {
            let mut todo = vec![root().join(dir)];
            while let Some(d) = todo.pop() {
                for entry in std::fs::read_dir(&d).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        todo.push(path);
                    } else if path.to_str().unwrap().ends_with(end) {
                        files.push(path.strip_prefix(root()).unwrap().to_path_buf());
                    }
                }
            }
        }
        assert!(files.len() > 400, "only {} files", files.len());
        let mut differ = Vec::new();
        for rel in &files {
            let ours = std::fs::read_to_string(root().join(rel)).unwrap();
            let flip = std::fs::read_to_string(tmp.join(rel)).unwrap();
            if brand_text::rewrite(&ours, "k2", "xy", verbs()) != flip {
                differ.push(rel.display().to_string());
            }
        }
        std::fs::remove_dir_all(&tmp).unwrap();
        assert!(
            differ.is_empty(),
            "the rewrite and the flip differ on {differ:?}"
        );
    }

    /// What this build embeds and prints holds no brand slot: every page,
    /// the `--schema` manifest and every captured help page as it renders.
    /// The sources do hold slots, so this reads the fill's work.
    #[test]
    fn no_raw_brand_slot_is_embedded() {
        let mut texts: Vec<(String, String)> = Vec::new();
        for (which, list) in [("study", STUDY), ("help", HELP)] {
            for (key, page) in list {
                texts.push((format!("cli/text/{which}/{key}.txt"), page.to_string()));
            }
        }
        for (n, c) in CAPTURED.iter().enumerate() {
            texts.push((format!("captured page {n}"), render(c.out) + &render(c.err)));
        }
        texts.push((
            "--schema".to_string(),
            crate::families::schema::BODY.to_string(),
        ));
        for (what, text) in &texts {
            for slot in brand_text::SLOTS {
                assert!(!text.contains(slot), "{what} holds the raw slot {slot}");
            }
        }
        let files = on_disk(&text_dir("study"));
        let slotted = files
            .values()
            .filter(|b| {
                let s = String::from_utf8_lossy(b);
                brand_text::SLOTS.iter().any(|slot| s.contains(slot))
            })
            .count();
        assert!(slotted > 5, "only {slotted} study files hold a slot");
    }

    /// The pages a partner's build prints name its company, its sign-in
    /// service and its contact address, never ours (rebrand B7):
    /// `study send-feedback`, `help setup`, `help send-feedback` (the file
    /// and its captured form) and `--schema` hold this build's
    /// `names.company`, `names.login` and `links.contact_email` where the
    /// sources hold the slots, and in another brand's build none of the
    /// default brand's, nor its company's first word alone, nor our repo
    /// (`links.repo`, its `owner/name`). Holds for any brand
    /// (`K2_BRAND_DIR`). `--schema` is not held to the repo: its one repo
    /// URL is the root helper's install line, ours in every brand (partner
    /// builds ship our root helper, and the daemon's real 409 names ours),
    /// and no other line of it prints a repo. It also names our Sew
    /// release repo (`<owner>/sew-releases`, which is not the word alone):
    /// partner builds ship our Sew.
    #[test]
    fn brand_free_pages_name_this_brands_company() {
        let ours = brand_text::Values {
            company: &default_name("company"),
            login: &default_name("login"),
            contact_email: &default_value("links", "contact_email"),
            repo: &default_value("links", "repo"),
        };
        let our_repo_name = ours
            .repo
            .strip_prefix("https://github.com/")
            .expect("our links.repo is a GitHub page");
        let our_word = ours.company.split(' ').next().unwrap();
        let source = |which: &str, key: &str| {
            std::fs::read_to_string(text_dir(which).join(format!("{key}.txt"))).unwrap()
        };
        for (which, key, slots) in [
            (
                "study",
                "send-feedback",
                &[
                    brand_text::COMPANY_SLOT,
                    brand_text::LOGIN_SLOT,
                    brand_text::CONTACT_EMAIL_SLOT,
                ][..],
            ),
            ("help", "setup", &[brand_text::COMPANY_SLOT][..]),
            (
                "help",
                "send-feedback",
                &[
                    brand_text::COMPANY_SLOT,
                    brand_text::LOGIN_SLOT,
                    brand_text::CONTACT_EMAIL_SLOT,
                ][..],
            ),
        ] {
            let text = source(which, key);
            for slot in slots {
                assert!(text.contains(slot), "{which} {key} holds {slot}");
            }
        }

        let mine = this_brand();
        let json = |v: &str| brand_text::json_escaped(v);
        let (company, login) = (json(mine.company), json(mine.login));
        let (contact, repo) = (json(mine.contact_email), json(mine.repo));
        let schema = brand_text::Values {
            company: &company,
            login: &login,
            contact_email: &contact,
            repo: &repo,
        };
        let captured = captured_form(&["send-feedback", "help"])
            .and_then(|f| f.captured)
            .map(|n| render(CAPTURED[n].out) + &render(CAPTURED[n].err))
            .expect("send-feedback help is a captured form");
        let printed = |what: &str, text: Option<&str>| {
            text.unwrap_or_else(|| panic!("{what} is embedded"))
                .to_string()
        };
        // Each page, what it must name of this brand's, and whether our
        // repo may be in it (the root helper's line, ours in every brand).
        let pages: [(&str, String, Vec<&str>, bool); 5] = [
            (
                "study send-feedback",
                printed("study send-feedback", study("send-feedback")),
                vec![mine.company, mine.login, mine.contact_email],
                false,
            ),
            (
                "help setup",
                printed("help setup", page("setup")),
                vec![mine.company],
                false,
            ),
            (
                "help send-feedback",
                printed("help send-feedback", page("send-feedback")),
                vec![mine.company, mine.login, mine.contact_email],
                false,
            ),
            (
                "help send-feedback (captured)",
                captured,
                vec![mine.company, mine.login, mine.contact_email],
                false,
            ),
            (
                "--schema",
                crate::families::schema::BODY.to_string(),
                vec![schema.company, schema.login],
                true,
            ),
        ];
        // `word` alone: no letter or digit before it, none after it, and
        // no `-` after it (an `owner-211/name` is not the word).
        let says = |text: &str, word: &str| {
            text.match_indices(word).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + word.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(|c| c.is_alphanumeric() || c == '-')
            })
        };
        for (what, text, want, our_repo_kept) in &pages {
            for v in want {
                assert!(text.contains(v), "{what} does not name {v}");
            }
            for (theirs, this) in [
                (ours.company, mine.company),
                (ours.login, mine.login),
                (ours.contact_email, mine.contact_email),
            ] {
                if this != theirs {
                    assert!(!text.contains(theirs), "{what} names {theirs}");
                }
            }
            if mine.repo != ours.repo && !*our_repo_kept {
                assert!(!text.contains(ours.repo), "{what} names {}", ours.repo);
                assert!(
                    !text.contains(our_repo_name),
                    "{what} names {our_repo_name}"
                );
            }
            if mine.company != ours.company {
                assert!(!says(text.as_str(), our_word), "{what} says {our_word}");
            }
        }
    }

    #[test]
    fn render_reads_the_escapes() {
        let cli = k2_client::brand::CLI_NAME;
        let product = k2_client::brand::PRODUCT_NAME;
        assert_eq!(
            render("Usage: \\c agent\\n\\tx \\\\n \\p's"),
            format!("Usage: {cli} agent\n\tx \\n {product}'s")
        );
        assert_eq!(
            render("Tell \\o; over \\l (\\\\l)"),
            format!("Tell {COMPANY_NAME}; over {LOGIN_NAME} (\\l)")
        );
        assert_eq!(
            render("email \\m (\\\\m)"),
            format!("email {CONTACT_EMAIL} (\\m)")
        );
        assert_eq!(render(""), "");
        assert_eq!(render("plain \u{2014} text"), "plain \u{2014} text");
    }
}
