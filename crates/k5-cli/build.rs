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

use flate2::write::GzEncoder;
use flate2::Compression;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/families/schema/join.rs"]
mod schema_join;

/// How far into the script the version line may be (prelude.sh puts it
/// on line 12; k2-client's script check reads 40 lines too).
const VERSION_LINES: usize = 40;

fn main() {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let script = manifest.join("../../cli/k2");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-changed=build.rs");

    let bytes = std::fs::read(&script).unwrap_or_else(|e| {
        panic!(
            "the shell CLI is not at {}: {e} (build it with scripts/build-cli.sh)",
            script.display()
        )
    });
    let version = cli_version(&bytes).unwrap_or_else(|| {
        panic!(
            "{}: no K2_CLI_VERSION=\"x.y.z\" line in its first {VERSION_LINES} lines",
            script.display()
        )
    });

    let blake = blake3::hash(&bytes).to_hex().to_string();

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&bytes).expect("gzip the shell CLI");
    let gz = gz.finish().expect("gzip the shell CLI");
    let gz_path = out.join("k2.gz");
    std::fs::write(&gz_path, &gz).unwrap_or_else(|e| panic!("write {}: {e}", gz_path.display()));

    let gz_lit = gz_path
        .to_str()
        .unwrap_or_else(|| panic!("OUT_DIR is not UTF-8: {}", gz_path.display()));
    let rs = format!(
        "/// `cli/k2`, gzipped.\n\
         pub static SCRIPT_GZ: &[u8] = include_bytes!({gz_lit:?});\n\
         /// The BLAKE3 of `cli/k2` (not of the gzip), in lowercase hex: what\n\
         /// the front checks a copy by before it runs it.\n\
         pub const SCRIPT_BLAKE3: &str = {blake:?};\n\
         /// The version on `cli/k2`'s `K2_CLI_VERSION=` line.\n\
         pub const SCRIPT_VERSION: &str = {version:?};\n\
         /// The version as a marker other code finds in the binary's bytes\n\
         /// without running it (k2-client's `cli_install::check_binary`).\n\
         /// The NUL ends it, so `0.46.1` is never read inside `0.46.15`.\n\
         pub static VERSION_MARKER: &str = \"K5_CLI_VERSION={version}\\0\";\n"
    );
    let rs_path = out.join("embedded.rs");
    std::fs::write(&rs_path, rs).unwrap_or_else(|e| panic!("write {}: {e}", rs_path.display()));

    text(&manifest, &out);
    schema(&manifest, &out);
}

/// `$OUT_DIR/schema.rs`: the body `k2 --schema` prints after its version
/// line, joined from `cli/src` as `scripts/build-cli.sh` joins it
/// (`src/families/schema/join.rs`).
fn schema(manifest: &Path, out: &Path) {
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
fn text(manifest: &Path, out: &Path) {
    let text_dir = manifest.join("../../cli/text");
    let native = manifest.join("src/help_native.txt");
    println!("cargo:rerun-if-changed={}", text_dir.display());
    println!("cargo:rerun-if-changed={}", native.display());

    let study = pages(&text_dir.join("study"));
    let help = pages(&text_dir.join("help"));
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

/// Whether `text` uses only the escapes `src/help_captured.txt` has:
/// `\\`, `\n`, `\t`, `\c` (the command's name) and `\p` (the
/// product's), and no tab or newline of its own.
fn escapes_ok(text: &str) -> Result<(), String> {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\\' | 'n' | 't' | 'c' | 'p') => {}
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

/// The value of the script's `K2_CLI_VERSION="x.y.z"` line: digits, letters,
/// dots, `-` and `+` only, never empty.
fn cli_version(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]);
    let line = text
        .lines()
        .take(VERSION_LINES)
        .find_map(|l| l.strip_prefix("K2_CLI_VERSION=\""))?;
    let value = line.strip_suffix('"')?;
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
    ok.then(|| value.to_string())
}
