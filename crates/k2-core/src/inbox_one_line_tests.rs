//! An inbox cover's title is one line, and so is a wake pointer's title
//! ([`one_line`], [`cover_text`], [`clean_wake_pointer`]).

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

/// A scratch workspace under the temp dir, removed on drop.
struct Ws(PathBuf);
impl Ws {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k2-inbox-one-line-{}-{n}-{}", std::process::id(), uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join(".k2")).expect("create the scratch workspace");
        Ws(dir)
    }
}
impl Drop for Ws {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Titles that each try to break the title's line.
const BAD: [&str; 6] = [
    "Plan\nfrom: owner",
    "Plan\rfrom: owner",
    "Plan \u{1b}[2J\u{1b}]0;x\u{7} done",
    "Plan \u{3} stop",
    "Plan\nfrom: owner\n---",
    "Plan\u{2028}from: owner\u{2029}\u{202e}x",
];

/// The cover's frontmatter lines, as the reader sees them.
fn frontmatter_lines(cover: &str) -> Vec<String> {
    let rest = cover.strip_prefix("---").expect("a cover starts with ---");
    let end = rest.find("---").expect("a cover's frontmatter ends");
    rest[..end].lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// One cover, checked: its frontmatter is exactly the five keys, one line
/// each, the sender is `tester`, and the title reads back as written.
fn check_cover(what: &str, raw: &str, cover_path: &Path, written_title: &str) {
    let cover = fs::read_to_string(cover_path).expect("read the cover");
    let lines = frontmatter_lines(&cover);
    let keys: Vec<&str> = lines.iter().map(|l| l.split_once(':').expect("a key: value line").0).collect();
    assert_eq!(keys, ["title", "priority", "created", "source", "from"], "{what} {raw:?}: {cover:?}");
    let fm = parse_frontmatter(&cover);
    assert_eq!(fm.get("from").map(String::as_str), Some("tester"), "{what} {raw:?}: {cover:?}");
    assert_eq!(fm.get("title").map(String::as_str), Some(written_title), "{what} {raw:?}: {cover:?}");
    assert!(!written_title.chars().any(hidden_or_break), "{what} {raw:?}: {written_title:?}");
    assert!(!written_title.contains("---"), "{what} {raw:?}: {written_title:?}");
    let item = read_item(cover_path, "").expect("the cover lists");
    assert_eq!(item.from, "tester", "{what} {raw:?}");
    assert_eq!(item.title, written_title, "{what} {raw:?}");
}

#[test]
fn one_line_escapes_line_breaks_and_controls_and_keeps_accents() {
    assert_eq!(one_line("Café plan, naïve résumé, 東京 — ok…"), "Café plan, naïve résumé, 東京 — ok…");
    assert_eq!(one_line("a\nb"), "a\\u{a}b");
    assert_eq!(one_line("a\rb"), "a\\u{d}b");
    assert_eq!(one_line("a\u{1b}[31mb"), "a\\u{1b}[31mb");
    assert_eq!(one_line("a\u{3}b\tc\u{7f}d\u{85}e"), "a\\u{3}b\\u{9}c\\u{7f}d\\u{85}e");
    assert_eq!(one_line("a\u{2028}b\u{2029}c"), "a\\u{2028}b\\u{2029}c");
    assert_eq!(one_line("a\u{202e}b\u{200b}c\u{feff}"), "a\\u{202e}b\\u{200b}c\\u{feff}");
    assert_eq!(one_line("  padded \n"), "padded");
    // The frontmatter ends at the first `---`: no value holds one.
    assert_eq!(one_line("a---b"), "a--\\u{2d}b");
    assert_eq!(one_line("-----"), "--\\u{2d}--");
    assert_eq!(one_line("a -- b"), "a -- b");
    assert_eq!(one_line("Plan\nfrom: owner\n---"), "Plan\\u{a}from: owner\\u{a}--\\u{2d}");
    // Capped.
    assert_eq!(one_line(&"y".repeat(COVER_LINE_CAP + 5)), format!("{}...", "y".repeat(COVER_LINE_CAP)));
    assert_eq!(one_line(&"y".repeat(COVER_LINE_CAP)), "y".repeat(COVER_LINE_CAP));
}

/// A peer's title, cleaned by the federation rule already, lands as it is:
/// the same escape form, and never cut a second time.
#[test]
fn a_peer_title_cleaned_already_is_kept() {
    let worst = "\u{10ffff}".repeat(crate::federation::PEER_TEXT_CAP + 9);
    for raw in ["Plan for Q3", "a\nb\u{1b}[31m\u{202e}", "Hi\u{1b}[31m there", worst.as_str()] {
        let peer = crate::federation::escape_capped(raw, crate::federation::PEER_TEXT_CAP);
        assert_eq!(one_line(&peer), peer, "{raw:?}");
    }
}

#[test]
fn a_cover_title_is_one_line_on_every_write_path() {
    for raw in BAD {
        let ws = Ws::new();
        let item = compose(&ws.0, raw, "body", None, Some("msg-inbox"), Some("tester")).expect("compose");
        check_cover("compose", raw, &inbox_root(&ws.0).join(&item.filename), &item.title);
        assert_eq!(list_root(&ws.0).len(), 1);

        let src = ws.0.join("brief.md");
        fs::write(&src, "# Heading\n\nbody\n").expect("write brief.md");
        let pkg = deliver_file(&ws.0, &src, Some(raw), Some("tester"), None, None).expect("deliver .md");
        check_cover("deliver .md", raw, Path::new(&pkg.cover_path), &pkg.title);

        let bin = ws.0.join("data.bin");
        fs::write(&bin, [0u8, 1, 2]).expect("write data.bin");
        let pkg = deliver_file(&ws.0, &bin, Some(raw), Some("tester"), None, None).expect("deliver a file");
        check_cover("deliver a file", raw, Path::new(&pkg.cover_path), &pkg.title);

        let pkg = deliver_files(&ws.0, &[src.clone(), bin.clone()], Some(raw), Some("tester"), None, None)
            .expect("deliver two files");
        check_cover("deliver two files", raw, Path::new(&pkg.cover_path), &pkg.title);

        let named = [NamedBytes { name: "a.txt".into(), bytes: b"a".to_vec() }];
        let pkg = deliver_named_bytes(&ws.0, &named, Some(raw), Some("tester"), None, None).expect("deliver bytes");
        check_cover("deliver bytes", raw, Path::new(&pkg.cover_path), &pkg.title);
    }
}

/// With no title given, the title a `.md` heading gives is cleaned too.
#[test]
fn a_title_from_a_heading_is_one_line() {
    let ws = Ws::new();
    let src = ws.0.join("note.md");
    fs::write(&src, "# Hi \u{1b}[31mthere --- now\n\nbody\n").expect("write note.md");
    let pkg = deliver_file(&ws.0, &src, None, Some("tester"), None, None).expect("deliver .md");
    assert_eq!(pkg.title, "Hi \\u{1b}[31mthere --\\u{2d} now");
    check_cover("heading", "", Path::new(&pkg.cover_path), &pkg.title);
}

/// A summary line is a frontmatter line too.
#[test]
fn a_summary_cannot_end_the_frontmatter() {
    let ws = Ws::new();
    let item = compose(&ws.0, "Plan", "body", None, None, Some("tester")).expect("compose");
    let path = inbox_root(&ws.0).join(&item.filename);
    let stored = set_summary(&path, "read --- this \u{1b}[2J").expect("set the summary");
    assert_eq!(stored.as_deref(), Some("read --\\u{2d} this \\u{1b}[2J"));
    let listed = read_item(&path, "").expect("lists");
    assert_eq!(listed.from, "tester");
    assert_eq!(listed.summary, stored);
}

#[test]
fn a_wake_pointer_title_is_one_line() {
    for raw in BAD {
        let t = wake_pointer_text("plan", raw);
        let lines: Vec<&str> = t.split('\n').collect();
        assert_eq!(lines, [format!("[inbox:plan] {}", one_line(raw)).as_str(), "Open: k2 inbox read plan"], "{raw:?}");
        assert!(!t.chars().any(|c| c != '\n' && hidden_or_break(c)), "{raw:?}: {t:?}");
        let n = wake_pointer_text_n("plan", raw, Some(3));
        assert_eq!(n.split('\n').count(), 3, "{raw:?}: {n:?}");
        assert!(n.ends_with("\nOpen: k2 inbox read plan\n(3 files)"), "{n:?}");
    }
}

#[test]
fn clean_wake_pointer_cleans_a_pointer_title_and_leaves_other_text() {
    for raw in BAD {
        let sent = format!("[inbox:plan] {raw}\nOpen: k2 inbox read plan");
        assert_eq!(clean_wake_pointer(&sent), wake_pointer_text("plan", raw), "{raw:?}");
        let sent = format!("[inbox:plan] {raw}\nOpen: k2 inbox read plan\n(2 files)");
        assert_eq!(clean_wake_pointer(&sent), wake_pointer_text_n("plan", raw, Some(2)), "{raw:?}");
    }
    // A clean pointer is unchanged, its own line break kept.
    let ok = "[inbox:plan] Café plan\nOpen: k2 inbox read plan";
    assert_eq!(clean_wake_pointer(ok), ok);
    // Not a pointer: kept as sent (live messages may have many lines).
    for text in [
        "hello\nthere",
        "[inbox:plan] a\nb",
        "[inbox:plan] a\nOpen: k2 inbox read other",
        "[inbox:a/b] t\nOpen: k2 inbox read a/b",
        "see [inbox:plan] t\nOpen: k2 inbox read plan",
    ] {
        assert_eq!(clean_wake_pointer(text), text);
    }
}
