//! Diagnostics: details about K2 on this computer, sent only with the
//! person's consent, exactly as previewed (PRD §6a, F6).
//!
//! The daemon (the window's server) gathers [`Facts`] and the log files,
//! [`build`] turns them into sections and scrubs each one
//! ([`super::scrub`]), [`fit`] keeps the bundle under
//! [`super::MAX_DIAGNOSTICS_BYTES`] (the log tail goes first), and the
//! result is a [`Draft`] kept for an hour. `send` names the draft and the
//! sections the person left out ([`choose`]): what is sent is what was
//! shown, byte for byte. The daemon never collects again at send time.
//!
//! Nothing here runs unless a person ticked the box or opened the preview
//! (or an agent asked while the owner allowed agent diagnostics).

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::scrub::{self, NameBook};
use super::MAX_DIAGNOSTICS_BYTES;

/// One section of the bundle, as previewed and as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub text: String,
}

/// Lines from the last day that look like failures (PRD §6a).
pub const RECENT_ERRORS_MAX: usize = 100;
pub const RECENT_ERRORS_WINDOW_MS: i64 = 24 * 3_600_000;
/// The end of each log file.
pub const TAIL_LINES: usize = 300;
/// How much of a log file is read from its end to find those lines.
pub const READ_TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// A draft is kept this long (PF17).
pub const DRAFT_TTL_MS: i64 = 3_600_000;
const ERROR_WORDS: &[&str] = &["error", "failed", "panic", "refused", "timeout", "denied"];

pub const S_VERSIONS: &str = "versions";
pub const S_COMPUTER: &str = "computer";
pub const S_STATE: &str = "k2-state";
pub const S_TOOLS: &str = "agent-tools";
pub const S_ERRORS: &str = "recent-errors";
pub const S_TAIL: &str = "log-tail";
pub const S_CONTEXT: &str = "context";

/// What the daemon knows, as `(label, value)` lines per section.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub versions: Vec<(String, String)>,
    pub computer: Vec<(String, String)>,
    pub k2_state: Vec<(String, String)>,
    pub agent_tools: Vec<(String, String)>,
    pub context: Vec<(String, String)>,
}

/// The log files to read: the daemon's own, and the desktop's fault log
/// when the desktop runs on this computer.
#[derive(Debug, Clone, Default)]
pub struct Logs {
    pub daemon: Vec<std::path::PathBuf>,
    pub client_faults: Option<std::path::PathBuf>,
}

fn lines_text(rows: &[(String, String)]) -> String {
    if rows.is_empty() {
        return "(nothing found)".into();
    }
    rows.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}

/// The last `max_lines` lines of a file (reading at most
/// [`READ_TAIL_BYTES`] from its end). `None` when it can't be read.
pub fn read_tail(path: &Path, max_lines: usize) -> Option<Vec<String>> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(READ_TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.take(READ_TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    // A cut first line is partial.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(max_lines);
    Some(lines[from..].iter().map(|l| l.to_string()).collect())
}

/// A daemon log line's RFC 3339 timestamp (its first word), in ms.
fn line_ts(line: &str) -> Option<i64> {
    let first = line.split_whitespace().next()?;
    chrono::DateTime::parse_from_rfc3339(first).ok().map(|d| d.timestamp_millis())
}

fn looks_like_failure(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ERROR_WORDS.iter().any(|w| l.contains(w))
}

/// The fold key: the text without its timestamp, digit runs as one `#`.
fn fold_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digits = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

/// Up to [`RECENT_ERRORS_MAX`] failure-looking lines from the last 24
/// hours, newest last, repeats folded into one line with a count. A line
/// without a timestamp takes the last one above it (a panic's backtrace).
pub fn recent_errors(lines: &[String], now_ms: i64) -> Vec<String> {
    let cutoff = now_ms - RECENT_ERRORS_WINDOW_MS;
    let mut last_ts: Option<i64> = None;
    // key → (order of last occurrence, newest line, count)
    let mut seen: HashMap<String, (usize, String, usize)> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let ts = line_ts(line).or(last_ts);
        if line_ts(line).is_some() {
            last_ts = ts;
        }
        if ts.is_some_and(|t| t < cutoff) || !looks_like_failure(line) {
            continue;
        }
        let text = match line_ts(line) {
            Some(_) => line.split_once(char::is_whitespace).map(|(_, r)| r.trim_start()).unwrap_or(""),
            None => line.as_str(),
        };
        let e = seen.entry(fold_key(text)).or_insert((i, String::new(), 0));
        e.0 = i;
        e.1 = line.clone();
        e.2 += 1;
    }
    let mut rows: Vec<(usize, String, usize)> = seen.into_values().collect();
    rows.sort_by_key(|r| r.0);
    let from = rows.len().saturating_sub(RECENT_ERRORS_MAX);
    rows[from..]
        .iter()
        .map(|(_, l, n)| if *n > 1 { format!("{l} (×{n})") } else { l.clone() })
        .collect()
}

/// Every section, scrubbed, in the order the preview shows them.
pub fn build(facts: &Facts, logs: &Logs, book: &NameBook, now_ms: i64) -> Vec<Section> {
    let mut all_daemon: Vec<String> = Vec::new();
    let mut tail_parts: Vec<String> = Vec::new();
    for p in &logs.daemon {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match read_tail(p, usize::MAX) {
            Some(lines) => {
                let from = lines.len().saturating_sub(TAIL_LINES);
                tail_parts.push(format!("== {name} (last {} lines) ==\n{}", lines.len() - from, lines[from..].join("\n")));
                all_daemon.extend(lines);
            }
            None => tail_parts.push(format!("== {name} ==\n(no log file here)")),
        }
    }
    if let Some(p) = &logs.client_faults {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match read_tail(p, TAIL_LINES) {
            Some(lines) => tail_parts.push(format!("== {name} (last {} lines) ==\n{}", lines.len(), lines.join("\n"))),
            None => tail_parts.push(format!("== {name} ==\n(no fault log here)")),
        }
    }
    let errors = recent_errors(&all_daemon, now_ms);
    let errors_text = if errors.is_empty() { "(none in the last 24 hours)".to_string() } else { errors.join("\n") };
    let sections = vec![
        (S_VERSIONS, "Versions", lines_text(&facts.versions)),
        (S_COMPUTER, "Computer", lines_text(&facts.computer)),
        (S_STATE, "K2 state", lines_text(&facts.k2_state)),
        (S_TOOLS, "Agent tools", lines_text(&facts.agent_tools)),
        (S_ERRORS, "Recent errors (last 24 hours)", errors_text),
        (S_TAIL, "End of K2's log", if tail_parts.is_empty() { "(no log files)".into() } else { tail_parts.join("\n\n") }),
        (S_CONTEXT, "Where you were", lines_text(&facts.context)),
    ];
    let out: Vec<Section> = sections
        .into_iter()
        .map(|(id, title, text)| Section { id: id.into(), title: title.into(), text: scrub::scrub(&text, book) })
        .collect();
    fit(out, MAX_DIAGNOSTICS_BYTES)
}

/// The bundle's size as sent (its JSON).
pub fn bytes_of(sections: &[Section]) -> usize {
    serde_json::to_string(sections).map(|s| s.len()).unwrap_or(usize::MAX)
}

/// Keep the bundle at most `max` bytes: the log tail loses its oldest
/// lines first, then the recent errors; a section still too big is cut.
pub fn fit(mut sections: Vec<Section>, max: usize) -> Vec<Section> {
    // Room for the "(… N older lines left out to fit)" note.
    const NOTE: usize = 64;
    for id in [S_TAIL, S_ERRORS] {
        let over = bytes_of(&sections).saturating_sub(max);
        if over == 0 {
            break;
        }
        let Some(s) = sections.iter_mut().find(|s| s.id == id) else { continue };
        let lines: Vec<&str> = s.text.lines().collect();
        let (mut drop, mut freed) = (0usize, 0usize);
        // Each line costs its JSON-escaped length plus the `\n` between.
        while drop + 1 < lines.len() && freed < over + NOTE {
            freed += serde_json::to_string(lines[drop]).map(|j| j.len()).unwrap_or(lines[drop].len() + 2);
            drop += 1;
        }
        if drop > 0 {
            s.text = format!("(… {drop} older lines left out to fit)\n{}", lines[drop..].join("\n"));
        }
    }
    // Still over (one huge line): halve the biggest section until it fits.
    while bytes_of(&sections) > max {
        let Some(big) = sections.iter_mut().max_by_key(|s| s.text.len()) else { break };
        if big.text.len() < 32 {
            break;
        }
        let mut cut = big.text.len() / 2;
        while cut > 0 && !big.text.is_char_boundary(cut) {
            cut -= 1;
        }
        big.text.truncate(cut);
        big.text.push_str("\n(… cut to fit)");
    }
    sections
}

/// What the person chose: the draft without the sections they left out,
/// with their line deletions. An edit may only delete lines (never add or
/// change one), so what is sent is still what K2 collected and showed.
pub fn choose(draft: &[Section], drop: &[String], edited: &BTreeMap<String, String>) -> Result<Vec<Section>, String> {
    for id in drop.iter().chain(edited.keys()) {
        if !draft.iter().any(|s| &s.id == id) {
            return Err(format!("no section {id} in this preview"));
        }
    }
    let mut out = Vec::new();
    for s in draft {
        if drop.contains(&s.id) {
            continue;
        }
        let text = match edited.get(&s.id) {
            None => s.text.clone(),
            Some(e) => {
                if !only_deletes(&s.text, e) {
                    return Err(format!("the {} section can only lose lines, not gain or change them", s.title));
                }
                e.clone()
            }
        };
        out.push(Section { id: s.id.clone(), title: s.title.clone(), text });
    }
    Ok(out)
}

/// Whether `edited`'s lines are `original`'s with some lines deleted.
fn only_deletes(original: &str, edited: &str) -> bool {
    let mut orig = original.split('\n');
    'outer: for line in edited.split('\n') {
        for o in orig.by_ref() {
            if o == line {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// A preview the person is reading, kept for [`DRAFT_TTL_MS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: String,
    /// The local sender that asked (`owner`, `login:<user>`, `agent:<pid>`).
    pub owner: String,
    pub sections: Vec<Section>,
    pub created_at: i64,
}

fn drafts() -> &'static Mutex<HashMap<String, Draft>> {
    static D: std::sync::OnceLock<Mutex<HashMap<String, Draft>>> = std::sync::OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_draft_id() -> String {
    format!("pfd_{}", &super::new_client_id()[3..])
}

/// Keep a preview; old ones go.
pub fn put_draft(owner: &str, sections: Vec<Section>, now_ms: i64) -> Draft {
    let d = Draft { id: new_draft_id(), owner: owner.to_string(), sections, created_at: now_ms };
    let mut m = drafts().lock().unwrap_or_else(|p| p.into_inner());
    m.retain(|_, v| now_ms - v.created_at < DRAFT_TTL_MS);
    m.insert(d.id.clone(), d.clone());
    d
}

/// The caller's own live draft.
pub fn get_draft(id: &str, owner: &str, now_ms: i64) -> Option<Draft> {
    let m = drafts().lock().unwrap_or_else(|p| p.into_inner());
    m.get(id).filter(|d| d.owner == owner && now_ms - d.created_at < DRAFT_TTL_MS).cloned()
}

/// Forget a draft once it was sent.
pub fn drop_draft(id: &str) {
    drafts().lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new(tag: &str) -> TestDir {
            let p = crate::test_env::unique_temp_path(tag);
            std::fs::create_dir_all(&p).unwrap();
            TestDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            crate::test_env::remove_test_dir(&self.0);
        }
    }

    fn sec(id: &str, text: &str) -> Section {
        Section { id: id.into(), title: id.to_uppercase(), text: text.into() }
    }

    #[test]
    fn recent_errors_window_fold_and_order() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z").unwrap().timestamp_millis();
        let lines: Vec<String> = [
            "2026-11-08T12:00:00.000Z [x] error: too old",
            "2026-11-10T09:00:00.000Z [tunnel] dial 3 failed",
            "2026-11-10T09:00:01.000Z [ok] all good",
            "2026-11-10T09:30:00.000Z [db] query refused",
            "2026-11-10T10:00:00.000Z [tunnel] dial 4 failed",
            "thread 'x' panicked at src/a.rs",
            "2026-11-10T11:00:00.000Z [tunnel] dial 5 failed",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let got = recent_errors(&lines, now);
        assert_eq!(
            got,
            vec![
                "2026-11-10T09:30:00.000Z [db] query refused".to_string(),
                "thread 'x' panicked at src/a.rs".to_string(),
                "2026-11-10T11:00:00.000Z [tunnel] dial 5 failed (×3)".to_string(),
            ]
        );
    }

    #[test]
    fn recent_errors_keeps_the_newest_hundred() {
        let now = 10_000_000_000_000;
        let ts = chrono::DateTime::from_timestamp_millis(now - 1000).unwrap().to_rfc3339();
        let lines: Vec<String> = (0..150).map(|i| format!("{ts} error kind-{}", char::from(b'a' + (i % 26) as u8).to_string().repeat(i / 26 + 1))).collect();
        let got = recent_errors(&lines, now);
        assert_eq!(got.len(), RECENT_ERRORS_MAX);
        assert!(got.last().unwrap().ends_with(&lines[149][ts.len() + 1..]), "{:?}", got.last());
    }

    #[test]
    fn tail_reads_the_end_of_a_file() {
        let dir = TestDir::new("pf-tail");
        let p = dir.path().join("daemon.stderr.log");
        let body: String = (0..500).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, body).unwrap();
        let t = read_tail(&p, 300).unwrap();
        assert_eq!(t.len(), 300);
        assert_eq!(t[0], "line 200");
        assert_eq!(t[299], "line 499");
        assert!(read_tail(&dir.path().join("missing.log"), 10).is_none());
    }

    #[test]
    fn build_scrubs_every_section_and_names_missing_logs() {
        let dir = TestDir::new("pf-build");
        let log = dir.path().join("daemon.stderr.log");
        std::fs::write(
            &log,
            "2026-11-10T11:00:00.000Z [agent-hooks] path=Some(\"/Users/al/proj/acme\") failed token=abc\n",
        )
        .unwrap();
        let book = NameBook::new(
            &[scrub::Workspace { name: "acme".into(), path: "/Users/al/proj/acme".into(), handle: None }],
            &[],
            &[],
            &[],
            &[],
            Some("/Users/al"),
        );
        let facts = Facts {
            versions: vec![("Daemon".into(), "0.46.11".into())],
            context: vec![("Page".into(), "Agents".into())],
            ..Default::default()
        };
        let logs = Logs { daemon: vec![log, dir.path().join("daemon.stdout.log")], client_faults: None };
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z").unwrap().timestamp_millis();
        let s = build(&facts, &logs, &book, now);
        let ids: Vec<&str> = s.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec![S_VERSIONS, S_COMPUTER, S_STATE, S_TOOLS, S_ERRORS, S_TAIL, S_CONTEXT]);
        let all = serde_json::to_string(&s).unwrap();
        assert!(!all.contains("/Users/al"), "{all}");
        assert!(!all.contains("token=abc"), "{all}");
        assert!(all.contains("<workspace-1>"), "{all}");
        let errors = &s.iter().find(|x| x.id == S_ERRORS).unwrap().text;
        assert!(errors.contains("failed token=[secret]"), "{errors}");
        let tail = &s.iter().find(|x| x.id == S_TAIL).unwrap().text;
        assert!(tail.contains("== daemon.stdout.log ==\n(no log file here)"), "{tail}");
        assert_eq!(s[0].text, "Daemon: 0.46.11");
        assert_eq!(s[1].text, "(nothing found)");
    }

    #[test]
    fn fit_drops_the_oldest_tail_lines_first() {
        let tail: String = (0..2000).map(|i| format!("tail line {i} {}\n", "x".repeat(100))).collect();
        let s = vec![sec(S_VERSIONS, "Daemon: 1"), sec(S_ERRORS, "e1\ne2"), sec(S_TAIL, &tail)];
        let max = 50_000;
        let out = fit(s, max);
        assert!(bytes_of(&out) <= max, "{}", bytes_of(&out));
        assert_eq!(out[0].text, "Daemon: 1");
        assert_eq!(out[1].text, "e1\ne2");
        let t = &out[2].text;
        assert!(t.starts_with("(… "), "{}", &t[..40]);
        assert!(t.contains("tail line 1999"), "the newest line stays");
        assert!(!t.contains("tail line 0 "), "the oldest goes");
    }

    #[test]
    fn choose_drops_sections_and_allows_only_deletions() {
        let d = vec![sec("a", "one\ntwo\nthree"), sec("b", "bee")];
        assert_eq!(choose(&d, &[], &BTreeMap::new()).unwrap(), d);
        assert_eq!(choose(&d, &["b".into()], &BTreeMap::new()).unwrap(), vec![d[0].clone()]);
        let mut e = BTreeMap::new();
        e.insert("a".to_string(), "one\nthree".to_string());
        assert_eq!(choose(&d, &[], &e).unwrap()[0].text, "one\nthree");
        e.insert("a".to_string(), "one\nthree\nfour".to_string());
        assert!(choose(&d, &[], &e).is_err(), "an added line is refused");
        e.insert("a".to_string(), "three\none".to_string());
        assert!(choose(&d, &[], &e).is_err(), "reordering is refused");
        assert!(choose(&d, &["zz".into()], &BTreeMap::new()).is_err());
    }

    #[test]
    fn drafts_are_per_owner_and_expire() {
        let d = put_draft("login:alice", vec![sec("a", "x")], 1_000);
        assert!(d.id.starts_with("pfd_"));
        assert!(get_draft(&d.id, "login:alice", 2_000).is_some());
        assert!(get_draft(&d.id, "login:bob", 2_000).is_none(), "another person never reads it");
        assert!(get_draft(&d.id, "login:alice", 1_000 + DRAFT_TTL_MS).is_none(), "an hour later it is gone");
        drop_draft(&d.id);
        assert!(get_draft(&d.id, "login:alice", 2_000).is_none());
    }
}
