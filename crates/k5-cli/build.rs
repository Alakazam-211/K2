//! Packs the shell CLI (`cli/k2`) into the binary: gzipped, with its
//! BLAKE3 (hex) and the version on its `K2_CLI_VERSION="x.y.z"` line. A
//! verb the front doesn't handle runs this copy, so the binary and the
//! script it falls through to are always the same release. The front
//! checks a copy by its BLAKE3 before every run.
//!
//! Writes `$OUT_DIR/k2.gz` and `$OUT_DIR/embedded.rs` (read by
//! `src/embedded.rs`). Fails the build when the script or its version line
//! is missing.
//!
//! A build of a brand with no shell CLI (`k2_brand::CLI_NO_BASH`, the
//! brand file's `cli.no_bash`) packs nothing: it never reads `cli/k2`, so
//! the file need not be there, and writes the script's items in
//! `embedded.rs` as `None`. The version then comes from the same line in
//! `cli/src/prelude.sh`, the line `scripts/build-cli.sh` copies into
//! `cli/k2`, read the same way (`src/version_line.rs`). The version marker
//! is written in both builds. Such a build fails when prelude.sh or its
//! version line is missing. The pages and the manifest below are text the
//! binary prints, not the script, so every build embeds them.
//!
//! Also embeds the CLI's static text, the same files `scripts/build-cli.sh`
//! inlines into `cli/k2`, keyed as bash looks them up: every
//! `cli/text/study/<id>.txt` under `<id>` and every
//! `cli/text/help/<words>.txt` under `<words>` (`mail/send`), and the help
//! forms the binary answers itself (`src/help_native.txt`, written by
//! `tests/cli/help_native.sh`; a page there is a help file's key, or
//! `study:<key>` for a study file). Writes `$OUT_DIR/text.rs` (read by
//! `src/text.rs`). Fails the build when a row of the list names a page
//! that is not there. With them the other help forms, what bash printed
//! for each (`src/help_captured.txt`, written by the same script): the
//! pages as they are escaped there, and the forms in argv order (a form
//! the permission cards answer or chat's manage switch decides has two
//! pages). Fails the build on a row it can't read, an escape it doesn't
//! know, a form twice or a form the first list has.
//!
//! And the manifest `k2 --schema` prints, joined from `cli/src` as the
//! shell's build joins it (`src/families/schema/join.rs`): writes
//! `$OUT_DIR/schema.rs` (read by `src/families/schema.rs`).
//!
//! The pages and the manifest spell the command as the default brand does
//! (`src/brand_text.rs`, `SOURCE_NAME`). In a build of a brand with
//! another command (`K2_BRAND_DIR`, read through the `k2-brand` build
//! dependency) each page and the manifest are rewritten to that name by
//! the CLI name rule (`brand_text::rewrite`). Then, in every build, their
//! `{company}`, `{login}`, `{contact_email}` and `{repo}` slots are filled
//! with the brand's `names.company`, `names.login`, `links.contact_email`
//! and `links.repo` (`brand_text::fill`; in the manifest, escaped for its
//! JSON strings), so no slot reaches a person. A page that changes is
//! embedded from its copy under `$OUT_DIR/text`, any other from its file.
//! In the default brand the embedded bytes are the bytes `cli/k2` prints.
//! The captured help forms hold the company, the sign-in service and the
//! contact address as escapes (`\o`, `\l`, `\m`), which `text::render`
//! spells at run time.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/families/schema/join.rs"]
mod schema_join;

#[path = "src/brand_text.rs"]
mod brand_text;

#[path = "src/version_line.rs"]
mod version_line;

/// This build's brand, as its text needs it: the values the slots take
/// (the brand file's `names.company`, `names.login`,
/// `links.contact_email` and `links.repo`, the default brand's where it
/// leaves one out), and the command's rewrite when its name differs from
/// the one the sources spell.
struct Brand {
    values: brand_text::Values<'static>,
    rename: Option<Rename>,
}

/// The command's name in this build (the brand file's `[cli] name`) and
/// the verbs the rewrite reads text by.
struct Rename {
    name: &'static str,
    verbs: std::collections::BTreeSet<String>,
}

impl Rename {
    /// `None` in a build whose command is the name the sources spell
    /// (nothing is rewritten).
    fn of_this_build(manifest: &Path) -> Option<Rename> {
        let name = k2_brand::CLI_NAME;
        if name == brand_text::SOURCE_NAME {
            return None;
        }
        let root = manifest.join("../..");
        println!("cargo:rerun-if-changed={}", root.join("cli/src").display());
        let verbs = brand_text::verbs(&root).unwrap_or_else(|e| panic!("the CLI's verbs: {e}"));
        Some(Rename { name, verbs })
    }
}

impl Brand {
    fn of_this_build(manifest: &Path) -> Brand {
        Brand {
            values: brand_text::Values {
                company: k2_brand::names::COMPANY,
                login: k2_brand::LOGIN_NAME,
                contact_email: k2_brand::links::CONTACT_EMAIL,
                repo: k2_brand::links::REPO,
            },
            rename: Rename::of_this_build(manifest),
        }
    }

    fn rewrite(&self, text: &str) -> String {
        match &self.rename {
            Some(r) => brand_text::rewrite(text, brand_text::SOURCE_NAME, r.name, &r.verbs),
            None => text.to_string(),
        }
    }

    /// A page as this build prints it: rewritten, then its slots filled.
    fn page(&self, text: &str, at: &Path) -> String {
        filled(brand_text::fill(&self.rewrite(text), &self.values), at)
    }

    /// The manifest as this build prints it: the same, with the values
    /// escaped for the JSON strings the slots sit in.
    fn manifest(&self, text: &str, at: &Path) -> String {
        let v = &self.values;
        let company = brand_text::json_escaped(v.company);
        let login = brand_text::json_escaped(v.login);
        let contact_email = brand_text::json_escaped(v.contact_email);
        let repo = brand_text::json_escaped(v.repo);
        let escaped = brand_text::Values {
            company: &company,
            login: &login,
            contact_email: &contact_email,
            repo: &repo,
        };
        filled(brand_text::fill(&self.rewrite(text), &escaped), at)
    }
}

/// `text`, which must hold no slot now: a brand value that is itself a
/// slot would leave one.
fn filled(text: String, at: &Path) -> String {
    for slot in brand_text::SLOTS {
        assert!(
            !text.contains(slot),
            "{}: the slot {slot} is still there after the fill",
            at.display()
        );
    }
    text
}

fn main() {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    println!("cargo:rerun-if-changed=build.rs");

    let rs = if k2_brand::CLI_NO_BASH {
        no_script(&manifest)
    } else {
        script(&manifest, &out)
    };
    let rs_path = out.join("embedded.rs");
    std::fs::write(&rs_path, rs).unwrap_or_else(|e| panic!("write {}: {e}", rs_path.display()));

    let brand = Brand::of_this_build(&manifest);
    text(&manifest, &out, &brand);
    schema(&manifest, &out, &brand);
}

/// The version on the `K2_CLI_VERSION="x.y.z"` line of `bytes`, read from
/// `path`; the build fails without one.
fn version_of(bytes: &[u8], path: &Path) -> String {
    version_line::cli_version(bytes).unwrap_or_else(|| {
        panic!(
            "{}: no K2_CLI_VERSION=\"x.y.z\" line in its first {} lines",
            path.display(),
            version_line::VERSION_LINES
        )
    })
}

/// The version items of `embedded.rs`, the same in every build. `from`
/// is the file the version was read from.
fn version_items(version: &str, from: &str) -> String {
    format!(
        "/// The version on {from}'s `K2_CLI_VERSION=` line.\n\
         pub const SCRIPT_VERSION: &str = {version:?};\n\
         /// The version as a marker other code finds in the binary's bytes\n\
         /// without running it (k2-client's `cli_install::check_binary`).\n\
         /// The NUL ends it, so `0.46.1` is never read inside `0.46.15`.\n\
         pub static VERSION_MARKER: &str = \"K5_CLI_VERSION={version}\\0\";\n"
    )
}

/// `embedded.rs` for a build that carries the shell CLI: `cli/k2`,
/// gzipped to `$OUT_DIR/k2.gz`, its BLAKE3 and its version.
fn script(manifest: &Path, out: &Path) -> String {
    let script = manifest.join("../../cli/k2");
    println!("cargo:rerun-if-changed={}", script.display());
    let bytes = std::fs::read(&script).unwrap_or_else(|e| {
        panic!(
            "the shell CLI is not at {}: {e} (build it with scripts/build-cli.sh)",
            script.display()
        )
    });
    let version = version_of(&bytes, &script);

    let blake = blake3::hash(&bytes).to_hex().to_string();

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&bytes).expect("gzip the shell CLI");
    let gz = gz.finish().expect("gzip the shell CLI");
    let gz_path = out.join("k2.gz");
    std::fs::write(&gz_path, &gz).unwrap_or_else(|e| panic!("write {}: {e}", gz_path.display()));

    let gz_lit = gz_path
        .to_str()
        .unwrap_or_else(|| panic!("OUT_DIR is not UTF-8: {}", gz_path.display()));
    format!(
        "/// `cli/k2`, gzipped.\n\
         pub static SCRIPT_GZ: Option<&[u8]> = Some(include_bytes!({gz_lit:?}));\n\
         /// The BLAKE3 of `cli/k2` (not of the gzip), in lowercase hex: what\n\
         /// the front checks a copy by before it runs it.\n\
         pub const SCRIPT_BLAKE3: Option<&str> = Some({blake:?});\n\
         {}",
        version_items(&version, "`cli/k2`")
    )
}

/// `embedded.rs` for a build with no shell CLI: no copy and no BLAKE3,
/// and the version from `cli/src/prelude.sh`. `cli/k2` is never read.
fn no_script(manifest: &Path) -> String {
    let prelude = manifest.join("../../cli/src/prelude.sh");
    println!("cargo:rerun-if-changed={}", prelude.display());
    let bytes =
        std::fs::read(&prelude).unwrap_or_else(|e| panic!("read {}: {e}", prelude.display()));
    let version = version_of(&bytes, &prelude);
    format!(
        "/// No copy of the shell CLI: this brand's build carries none\n\
         /// (its brand file's `cli.no_bash`).\n\
         pub static SCRIPT_GZ: Option<&[u8]> = None;\n\
         /// No copy, so no BLAKE3.\n\
         pub const SCRIPT_BLAKE3: Option<&str> = None;\n\
         {}",
        version_items(&version, "`cli/src/prelude.sh`")
    )
}

/// `$OUT_DIR/schema.rs`: the body `k2 --schema` prints after its version
/// line, joined from `cli/src` as `scripts/build-cli.sh` joins it
/// (`src/families/schema/join.rs`), as this build prints it.
fn schema(manifest: &Path, out: &Path, brand: &Brand) {
    let src = manifest.join("../../cli/src");
    for name in ["ORDER", "schema-head.sh", "schema-tail.sh", "schema"] {
        println!("cargo:rerun-if-changed={}", src.join(name).display());
    }
    let read = |name: &str| {
        let p = src.join(name);
        std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))
    };
    let order = read("ORDER").unwrap_or_else(|e| panic!("{e}"));
    let body = schema_join::body(&order, read).unwrap_or_else(|e| panic!("--schema: {e}"));
    let body = brand.manifest(&body, &src.join("schema"));
    let txt = out.join("schema.txt");
    std::fs::write(&txt, body).unwrap_or_else(|e| panic!("write {}: {e}", txt.display()));
    let lit = txt
        .to_str()
        .unwrap_or_else(|| panic!("OUT_DIR is not UTF-8: {}", txt.display()));
    let rs = format!(
        "/// The manifest `cmd_schema` prints after its version line.\n\
         pub static BODY: &str = include_str!({lit:?});\n"
    );
    let rs_path = out.join("schema.rs");
    std::fs::write(&rs_path, rs).unwrap_or_else(|e| panic!("write {}: {e}", rs_path.display()));
}

/// `$OUT_DIR/text.rs`: the study and help pages, and the help forms.
fn text(manifest: &Path, out: &Path, brand: &Brand) {
    let text_dir = manifest.join("../../cli/text");
    let native = manifest.join("src/help_native.txt");
    println!("cargo:rerun-if-changed={}", text_dir.display());
    println!("cargo:rerun-if-changed={}", native.display());

    let study = built(
        pages(&text_dir.join("study")),
        brand,
        &out.join("text/study"),
    );
    let help = built(pages(&text_dir.join("help")), brand, &out.join("text/help"));
    let mut rs = String::new();
    for (name, list) in [("STUDY", &study), ("HELP", &help)] {
        writeln!(rs, "pub static {name}: &[(&str, &str)] = &[").expect("write to a String");
        for (key, path) in list.iter() {
            let path = path
                .to_str()
                .unwrap_or_else(|| panic!("not UTF-8: {}", path.display()));
            writeln!(rs, "    ({key:?}, include_str!({path:?})),").expect("write to a String");
        }
        rs.push_str("];\n");
    }

    let list = std::fs::read_to_string(&native)
        .unwrap_or_else(|e| panic!("read {}: {e}", native.display()));
    let mut file_forms: Vec<Vec<String>> = Vec::new();
    rs.push_str("pub static HELP_FORMS: &[HelpForm] = &[\n");
    for (n, line) in list.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let at = || format!("{}:{}", native.display(), n + 1);
        let parts: Vec<&str> = line.split('|').collect();
        let [argv, page, stream, code] = parts[..] else {
            panic!("{}: not <argv>|<page>|<out or err>|<code>: {line:?}", at())
        };
        // `study:<key>` names a study page (`help study` prints one).
        match page.strip_prefix("study:") {
            Some(key) => assert!(
                study.iter().any(|(k, _)| k == key),
                "{}: no cli/text/study/{key}.txt",
                at()
            ),
            None => assert!(
                help.iter().any(|(k, _)| k == page),
                "{}: no cli/text/help/{page}.txt",
                at()
            ),
        }
        let stderr = match stream {
            "out" => false,
            "err" => true,
            _ => panic!("{}: the stream is out or err, not {stream:?}", at()),
        };
        let code: u8 = code
            .parse()
            .unwrap_or_else(|_| panic!("{}: the exit code is not a number: {code:?}", at()));
        let words: Vec<&str> = argv.split(' ').collect();
        assert!(
            words.iter().all(|w| !w.is_empty()),
            "{}: an empty word in {argv:?}",
            at()
        );
        writeln!(
            rs,
            "    HelpForm {{ argv: &{words:?}, page: {page:?}, stderr: {stderr}, code: {code}, captured: None, variant: None }},"
        )
        .expect("write to a String");
        file_forms.push(words.iter().map(|w| w.to_string()).collect());
    }
    rs.push_str("];\n");
    captured(manifest, &file_forms, &mut rs);
    let rs_path = out.join("text.rs");
    std::fs::write(&rs_path, rs).unwrap_or_else(|e| panic!("write {}: {e}", rs_path.display()));
}

/// `CAPTURED` and `CAPTURED_FORMS` from `src/help_captured.txt`: a page
/// row is `P`, its number, the exit code, stdout and stderr (escaped as
/// [`escapes_ok`] reads them), a form row `F`, its words between single
/// spaces and its page's number. A `C` row is a form whose page goes
/// through the permission cards filter: its words, the page while cards
/// are off, then the page while they are on. An `M` row prints chat's
/// usage: its words, the page while `_CHAT_HELP_MANAGE` is unset, empty or
/// `1`, then the page while it is anything else. A `Z` row is a `zen`
/// form: the router arm's Garden changes hint runs first, then its page.
/// Pages come first, numbered from 0 in order.
fn captured(manifest: &Path, file_forms: &[Vec<String>], rs: &mut String) {
    let path = manifest.join("src/help_captured.txt");
    println!("cargo:rerun-if-changed={}", path.display());
    let list =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut pages: Vec<(u8, &str, &str)> = Vec::new();
    let mut forms: Vec<(Vec<&str>, usize, Option<String>)> = Vec::new();
    for (n, line) in list.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let at = || format!("{}:{}", path.display(), n + 1);
        let cols: Vec<&str> = line.split('\t').collect();
        match cols[..] {
            ["P", no, code, out, err] => {
                assert!(forms.is_empty(), "{}: a page after the forms", at());
                assert_eq!(
                    no,
                    pages.len().to_string(),
                    "{}: pages are numbered in order",
                    at()
                );
                let Ok(code) = code.parse::<u8>() else {
                    panic!("{}: the exit code is not a number: {code:?}", at())
                };
                for text in [out, err] {
                    if let Err(e) = escapes_ok(text) {
                        panic!("{}: {e}", at());
                    }
                }
                pages.push((code, out, err));
            }
            [kind @ ("F" | "C" | "M" | "Z"), argv, page, ref other @ ..] => {
                let words: Vec<&str> = argv.split(' ').collect();
                assert!(
                    words.iter().all(|w| !w.is_empty()),
                    "{}: an empty word",
                    at()
                );
                let number = |page: &str| -> usize {
                    page.parse()
                        .ok()
                        .filter(|p| *p < pages.len())
                        .unwrap_or_else(|| panic!("{}: no page {page:?}", at()))
                };
                let page = number(page);
                let variant = match (kind, other) {
                    ("F", []) => None,
                    ("C", [on]) => Some(format!("Variant::CardsOn({})", number(on))),
                    ("M", [off]) => Some(format!("Variant::NoManage({})", number(off))),
                    ("Z", []) => Some("Variant::GardenHint".to_string()),
                    _ => panic!("{}: a {kind} row of {} columns: {line:?}", at(), cols.len()),
                };
                assert!(
                    !file_forms.iter().any(|f| *f == words),
                    "{}: {argv:?} is in src/help_native.txt too",
                    at()
                );
                forms.push((words, page, variant));
            }
            _ => panic!("{}: not a P, F, C, M or Z row: {line:?}", at()),
        }
    }
    assert!(!forms.is_empty(), "{}: no forms", path.display());
    forms.sort();
    for w in forms.windows(2) {
        assert!(
            w[0].0 != w[1].0,
            "{}: {:?} is there twice",
            path.display(),
            w[0].0
        );
    }
    rs.push_str("pub static CAPTURED: &[Captured] = &[\n");
    for (code, out, err) in &pages {
        writeln!(
            rs,
            "    Captured {{ code: {code}, out: {out:?}, err: {err:?} }},"
        )
        .expect("write to a String");
    }
    rs.push_str("];\n");
    rs.push_str("pub static CAPTURED_FORMS: &[HelpForm] = &[\n");
    for (words, page, variant) in &forms {
        let (code, out, err) = pages[*page];
        // The stream a lone page is on; with both, `stderr` is false and
        // stderr is written first ([`crate::pages::help_page`]). A form
        // with two pages: its first page's (the other's code and streams
        // are its own, `CAPTURED`).
        let stderr = out.is_empty() && !err.is_empty();
        let variant = variant
            .as_deref()
            .map_or("None".to_string(), |v| format!("Some({v})"));
        writeln!(
            rs,
            "    HelpForm {{ argv: &{words:?}, page: \"\", stderr: {stderr}, code: {code}, captured: Some({page}), variant: {variant} }},"
        )
        .expect("write to a String");
    }
    rs.push_str("];\n");
}

/// The pages to embed, as this build prints them ([`Brand::page`]): a
/// page that changes is written under `dir` (`<key>.txt`) and embedded
/// from there; any other is embedded from its file.
fn built(list: Vec<(String, PathBuf)>, brand: &Brand, dir: &Path) -> Vec<(String, PathBuf)> {
    list.into_iter()
        .map(|(key, path)| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let page = brand.page(&text, &path);
            if page == text {
                return (key, path);
            }
            let to = dir.join(format!("{key}.txt"));
            let parent = to.parent().expect("a page's file has a folder");
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
            std::fs::write(&to, page).unwrap_or_else(|e| panic!("write {}: {e}", to.display()));
            (key, to)
        })
        .collect()
}

/// Whether `text` uses only the escapes `src/help_captured.txt` has:
/// `\\`, `\n`, `\t`, `\c` (the command's name), `\p` (the
/// product's), `\o` (the company's), `\l` (the sign-in service's) and
/// `\m` (the contact address), and no tab or newline of its own.
fn escapes_ok(text: &str) -> Result<(), String> {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\\' | 'n' | 't' | 'c' | 'p' | 'o' | 'l' | 'm') => {}
                other => return Err(format!("an escape it doesn't know: \\{other:?}")),
            },
            '\t' | '\n' | '\r' => return Err(format!("a raw {c:?} in a text")),
            _ => {}
        }
    }
    Ok(())
}

/// Every `.txt` file under `dir`, keyed by its path below `dir` with `/`
/// between the words and no `.txt`, in key order.
fn pages(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let entries = std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("read {}: {e}", d.display()))
                .path();
            if path.is_dir() {
                todo.push(path);
            } else if path.extension().is_some_and(|x| x == "txt") {
                let rel = path
                    .strip_prefix(dir)
                    .expect("under the folder")
                    .with_extension("");
                let words: Vec<&str> = rel
                    .components()
                    .map(|c| {
                        c.as_os_str()
                            .to_str()
                            .unwrap_or_else(|| panic!("not UTF-8: {}", path.display()))
                    })
                    .collect();
                found.push((
                    words.join("/"),
                    path.canonicalize()
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
                ));
            }
        }
    }
    assert!(!found.is_empty(), "no pages under {}", dir.display());
    found.sort();
    found
}
