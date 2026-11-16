//! Shared by the pin tests: reading the tables, running the tool in-process, temp folders and
//! the one failure that lists every wrong row.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub use mpc_tools::Output;

/// The data folder of one table.
pub fn data(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(sub)
}

/// A table's rows: its lines but `#` comments and blank ones, split at tabs.
pub fn rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Undoes the generators' escaping: `\\`, `\n`, `\t`, `\r` and `\xNN`.
pub fn unescape(field: &str) -> String {
    let mut out = String::new();
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                let byte = u8::from_str_radix(&hex, 16)
                    .unwrap_or_else(|_| panic!("bad \\x escape in table field {field:?}"));
                out.push(char::from(byte));
            }
            other => panic!("bad escape {other:?} in table field {field:?}"),
        }
    }
    out
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex in the table"))
        .collect()
}

pub fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Runs the tool in-process: `args` after the program name, with this stdin, environment
/// and folder.
pub fn run(args: &[&str], stdin: &[u8], env: &[(&str, &str)], cwd: &Path) -> Output {
    let ctx = mpc_tools::Ctx {
        stdin: Some(stdin.to_vec()),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        cwd: cwd.to_path_buf(),
    };
    let mut all = vec!["mpc-tools"];
    all.extend_from_slice(args);
    mpc_tools::run(all, &ctx)
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

static COUNT: AtomicUsize = AtomicUsize::new(0);

/// A new empty folder of this test's own, removed when dropped.
pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Temp {
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("mpc-tools-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("a temp folder");
        Temp(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Fails once, listing every wrong row (the first forty in full), after checking the table
/// has at least `floor` rows.
pub fn report(table: &str, rows: usize, floor: usize, wrong: Vec<String>) {
    assert!(
        rows >= floor,
        "{table}: only {rows} rows, fewer than {floor}: the table was cut"
    );
    assert!(
        wrong.is_empty(),
        "{table}: {} of {rows} rows differ from the old tool\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("{table}: {rows} rows checked, all as the old tool");
}
