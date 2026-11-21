//! `sessions archives` and `sessions compact`, the owner's on-disk view
//! of the workspace's session archives, as the shell CLI's
//! `cmd_sessions_list` and `cmd_sessions_compact` answer them
//! (`cli/src/awareness.sh`). No daemon call: the folders are the source.
//!
//! - `archives [--json]`: each folder in `<dot dir>/sessions` (by name,
//!   as Python's `sorted(os.listdir())` orders them) with its archive
//!   segments counted and summed (`archive.ndjson`, `archive.*.ndjson`,
//!   `archive.*.ndjson.gz`), the active segment's size and the newest
//!   segment's whole seconds; as JSON (`json.dumps(rows, indent=2)`) or
//!   a table whose times are `time.strftime("%Y-%m-%d %H:%M:%S",
//!   time.localtime(t))`, by the C library with the same `TZ`
//!   ([`crate::out::pylib::time::strftime_local`]). Other words are
//!   skipped; `--` ends them.
//! - `compact <session-id>`: each `archive.NNN.ndjson` in the session's
//!   folder (a plain file, by name), compressed in place by the system's
//!   `gzip -f`, as the shell runs it: the same program, so the same bytes
//!   (a Rust deflate would not write gzip's). Each size is the file's
//!   length, as the shell's `wc -c <FILE` reads it. Then one JSON line of
//!   the counts, the id as `_setup_jstr` writes it, or the nothing-to-do
//!   line.
//!
//! Before either: the router's gates and the `sessions-spawn` tool's
//! session checks ([`super::more`]'s order).
//!
//! The shell answers ([`plan`] or the run gives `None`, before anything
//! is printed or changed): a session folder or the sessions folder that
//! can't be listed (Python raises), a name that is not UTF-8, a time the C
//! library can't show; for `compact`, an id that ends in `/` (find prints
//! the paths its own way there). Before 0.46.17 the shell read each size
//! with `stat -f '%z'`, which GNU's `stat` reads as "the file system", so
//! compact stopped on Linux and the binary handed it on there; both now
//! read the length, and compact is native on every system.

use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, pylib::time::strftime_local};
use k2_client::cli_name;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `compact`'s usage lines, exit 1.
pub const COMPACT_USAGE: [&str; 2] = [
    concat!("Usage: ", cli_name!(), " sessions compact <session-id>"),
    concat!(
        "(use '",
        cli_name!(),
        " sessions list' to find session ids)"
    ),
];

/// No `gzip` on `PATH`.
pub const NO_GZIP: &str = "gzip not found in PATH \u{2014} cannot compact";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disk {
    Archives {
        json: bool,
    },
    /// The id (empty: the usage).
    Compact(String),
}

/// The words after `sessions`, for these verbs.
pub fn parse(words: &[&str]) -> Option<Disk> {
    let (verb, rest) = words.split_first()?;
    match *verb {
        "archives" => {
            let mut json = false;
            for w in rest {
                match *w {
                    "--json" => json = true,
                    "--" => break,
                    _ => {}
                }
            }
            Some(Disk::Archives { json })
        }
        "compact" => Some(Disk::Compact(rest.first().unwrap_or(&"").to_string())),
        _ => None,
    }
}

/// `_ws_dot_dir ROOT`, as the shell's text: `ROOT/.k2` or `ROOT/.k2so`.
pub fn dot_dir(root: &str) -> String {
    let (k2, old) = (format!("{root}/.k2"), format!("{root}/.k2so"));
    if Path::new(&k2).is_dir() {
        k2
    } else if Path::new(&old).is_dir() || Path::new(&k2).exists() {
        old
    } else {
        k2
    }
}

/// One session folder's counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub total: u64,
    pub segments: u64,
    pub active: u64,
    pub latest: i64,
}

/// Whether a file in a session folder is a segment.
fn segment(name: &[u8]) -> bool {
    name == b"archive.ndjson"
        || (name.starts_with(b"archive.")
            && (name.ends_with(b".ndjson") || name.ends_with(b".ndjson.gz")))
}

/// `int(st.st_mtime)`: Python's float (`sec + nsec * 1e-9`, as its
/// `os.stat` makes it), toward zero.
fn whole_mtime(m: &std::fs::Metadata) -> i64 {
    (m.mtime() as f64 + m.mtime_nsec() as f64 * 1e-9).trunc() as i64
}

/// The rows of `sessions_dir`, or `None` where Python would raise or
/// read a name the binary doesn't.
pub fn rows(sessions_dir: &Path) -> Option<Vec<Row>> {
    let mut names = Vec::new();
    for e in std::fs::read_dir(sessions_dir).ok()? {
        let name = e.ok()?.file_name();
        names.push(name.to_str()?.to_string());
    }
    names.sort();
    let mut rows = Vec::new();
    for name in names {
        let dir = sessions_dir.join(&name);
        if !dir.is_dir() {
            continue;
        }
        let mut row = Row {
            id: name,
            total: 0,
            segments: 0,
            active: 0,
            latest: 0,
        };
        for e in std::fs::read_dir(&dir).ok()? {
            let f = e.ok()?.file_name();
            let Ok(st) = std::fs::metadata(dir.join(&f)) else {
                continue;
            };
            let f = f.as_bytes();
            if !segment(f) {
                continue;
            }
            row.total += st.len();
            row.segments += 1;
            row.latest = row.latest.max(whole_mtime(&st));
            if f == b"archive.ndjson" {
                row.active = st.len();
            }
        }
        rows.push(row);
    }
    Some(rows)
}

/// `human_bytes`.
pub fn human(n: u64) -> String {
    const K: u64 = 1024;
    if n < K {
        return format!("{n} B");
    }
    let f = n as f64;
    if n < K * K {
        return format!("{:.2} KB", f / 1024.0);
    }
    if n < K * K * K {
        return format!("{:.2} MB", f / (1024.0 * 1024.0));
    }
    format!("{:.2} GB", f / (1024.0 * 1024.0 * 1024.0))
}

/// Python's `f"{s:N}"` (left, by code points) and `f"{s:>N}"`.
fn left(s: &str, n: usize) -> String {
    let len = s.chars().count();
    format!("{s}{}", " ".repeat(n.saturating_sub(len)))
}

fn right(s: &str, n: usize) -> String {
    let len = s.chars().count();
    format!("{}{s}", " ".repeat(n.saturating_sub(len)))
}

/// The JSON `archives --json` prints.
pub fn json_of(rows: &[Row]) -> String {
    let items = rows
        .iter()
        .map(|r| {
            Value::Object(vec![
                (
                    Text::from("session_id"),
                    Value::Str(Text::from(r.id.as_str())),
                ),
                (Text::from("total_bytes"), Value::Int(r.total.to_string())),
                (Text::from("segments"), Value::Int(r.segments.to_string())),
                (Text::from("active_bytes"), Value::Int(r.active.to_string())),
                (
                    Text::from("last_modified_epoch"),
                    Value::Int(r.latest.to_string()),
                ),
            ])
        })
        .collect();
    let options = Options {
        indent: Some(2),
        ..Options::default()
    };
    let mut out =
        json::to_string(&Value::Array(items), &options).expect("a str holds no lone surrogate");
    out.push('\n');
    out
}

/// The table `archives` prints, or `None` where a time can't be shown.
pub fn table(rows: &[Row]) -> Option<String> {
    let mut out = format!(
        "{}  {}  {}  {}  LAST MODIFIED\n",
        left("SESSION", 38),
        right("SEGMENTS", 8),
        right("TOTAL", 12),
        right("ACTIVE", 12)
    );
    for r in rows {
        let ts = if r.latest != 0 {
            strftime_local(r.latest, "%Y-%m-%d %H:%M:%S").ok()?
        } else {
            "-".to_string()
        };
        out.push_str(&format!(
            "{}  {}  {}  {}  {ts}\n",
            left(&r.id, 38),
            right(&r.segments.to_string(), 8),
            right(&human(r.total), 12),
            right(&human(r.active), 12)
        ));
    }
    Some(out)
}

/// `archives`, after the gates. `None`: the shell's (nothing printed).
pub fn archives(project: &str, as_json: bool) -> Option<Exit> {
    let dir = PathBuf::from(format!("{}/sessions", dot_dir(project)));
    if !dir.is_dir() {
        if as_json {
            return Some(print("[]\n"));
        }
        out::stderr_line(&format!("No sessions yet for {project}"));
        return Some(Exit::Ok);
    }
    let rows = rows(&dir)?;
    if as_json {
        return Some(print(&json_of(&rows)));
    }
    if rows.is_empty() {
        out::stderr_line("No sessions yet");
        return Some(Exit::Ok);
    }
    Some(print(&table(&rows)?))
}

fn print(text: &str) -> Exit {
    match out::stdout(text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `find DIR -maxdepth 1 -type f -name 'archive.[0-9][0-9][0-9].ndjson'`,
/// by name: the plain files (not links) so named.
pub fn rotated(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut names = Vec::new();
    for e in std::fs::read_dir(dir).ok()? {
        let e = e.ok()?;
        let name = e.file_name();
        let b = name.as_bytes();
        let digits = b.len() == "archive.000.ndjson".len()
            && b.starts_with(b"archive.")
            && b.ends_with(b".ndjson")
            && b[8..11].iter().all(u8::is_ascii_digit);
        if digits && e.file_type().ok()?.is_file() {
            names.push(name.to_str()?.to_string());
        }
    }
    names.sort();
    Some(names.into_iter().map(|n| dir.join(n)).collect())
}

/// `compact <id>`, after the gates. `None`: the shell's (nothing printed
/// or changed).
pub fn compact(project: &str, id: &str) -> Option<Exit> {
    if id.is_empty() {
        for l in COMPACT_USAGE {
            out::stderr_line(l);
        }
        return Some(Exit::Error);
    }
    if id.ends_with('/') {
        return None;
    }
    let dir_text = format!("{}/sessions/{id}", dot_dir(project));
    let dir = PathBuf::from(&dir_text);
    if !dir.is_dir() {
        out::stderr_line(&format!(
            "No archive directory for session {id} at {dir_text}"
        ));
        return Some(Exit::Error);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let Some(gzip) = crate::daemon::find_on_path("gzip", path) else {
        out::stderr_line(NO_GZIP);
        return Some(Exit::Error);
    };
    let segs = rotated(&dir)?;
    // `wc -c <FILE`, or 0 when it can't be read.
    let size = |p: &Path| {
        std::fs::File::open(p)
            .and_then(|f| f.metadata())
            .map_or(0, |m| m.len())
    };
    let (mut count, mut before, mut after) = (0u64, 0u64, 0u64);
    for seg in &segs {
        before += size(seg);
        let ok = Command::new(&gzip)
            .arg("-f")
            .arg(seg)
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            let mut gz = seg.as_os_str().to_os_string();
            gz.push(".gz");
            after += size(Path::new(&gz));
            count += 1;
        } else {
            out::stderr_line(&format!("gzip failed for {}", seg.display()));
        }
    }
    if count == 0 {
        out::stderr_line(&format!(
            "Nothing to compact for session {id} (no uncompressed rotated segments)."
        ));
        return Some(Exit::Ok);
    }
    // Bash's 64-bit arithmetic; the id as `_setup_jstr` writes it.
    let saved = before as i64 - after as i64;
    let jid = super::super::setup::jstr(id);
    Some(print(&format!(
        "{{\"sessionId\":{jid},\"compacted\":{count},\"bytesBefore\":{before},\"bytesAfter\":{after},\"bytesSaved\":{saved}}}\n"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert_eq!(parse(&["archives"]), Some(Disk::Archives { json: false }));
        assert_eq!(
            parse(&["archives", "x", "--json"]),
            Some(Disk::Archives { json: true })
        );
        assert_eq!(
            parse(&["archives", "--", "--json"]),
            Some(Disk::Archives { json: false })
        );
        assert_eq!(parse(&["compact"]), Some(Disk::Compact(String::new())));
        assert_eq!(
            parse(&["compact", "s1", "x"]),
            Some(Disk::Compact("s1".into()))
        );
        assert_eq!(parse(&["list"]), None);
    }

    #[test]
    fn sizes_as_python_prints_them() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1024), "1.00 KB");
        // 1.125 KB: a tie, rounded to even as Python's format rounds it.
        assert_eq!(human(1152), "1.12 KB");
        assert_eq!(human(1024 * 1024 * 3 / 2), "1.50 MB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.00 GB");
        assert!(segment(b"archive.ndjson") && segment(b"archive.ndjson.gz"));
        assert!(segment(b"archive.001.ndjson") && segment(b"archive.x.ndjson.gz"));
        assert!(!segment(b"archive.json") && !segment(b"notes.ndjson"));
    }

    #[test]
    fn the_disk_lines_say_the_brand_s_name() {
        let c = k2_client::brand::CLI_NAME;
        assert_eq!(
            COMPACT_USAGE[0],
            format!("Usage: {c} sessions compact <session-id>")
        );
        assert_eq!(
            COMPACT_USAGE[1],
            format!("(use '{c} sessions list' to find session ids)")
        );
    }

    #[test]
    fn the_shell_s_lines() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/awareness.sh");
        let sh = std::fs::read_to_string(&p).expect("awareness.sh");
        for line in [
            r#"        echo "Usage: k2 sessions compact <session-id>" >&2"#,
            r#"        echo "(use 'k2 sessions list' to find session ids)" >&2"#,
            r#"        echo "gzip not found in PATH — cannot compact" >&2"#,
            r#"        before_size=$(wc -c <"$seg" 2>/dev/null) || before_size=0"#,
            r#"            after_size=$(wc -c <"$seg.gz" 2>/dev/null) || after_size=0"#,
            r#"    _setup_jstr "$session_id""#,
            r#"    echo "{\"sessionId\":${_SETUP_J},\"compacted\":$count,\"bytesBefore\":$compacted_bytes_before,\"bytesAfter\":$compacted_bytes_after,\"bytesSaved\":$saved}""#,
            r#"        if gzip -f "$seg"; then"#,
            r#"        -name 'archive.[0-9][0-9][0-9].ndjson' -print0 | sort -z)"#,
            r#"    print(json.dumps(rows, indent=2))"#,
            r#"        ts = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(r["last_modified_epoch"])) if r["last_modified_epoch"] else "-""#,
        ] {
            assert!(sh.contains(line), "awareness.sh no longer has {line:?}");
        }
    }
}
