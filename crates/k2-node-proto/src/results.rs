//! Results back (research-distributed-compute-gaps-v1 G1/G2, PRD §11.4):
//! the artifact globs a plan carries, what the node collected, and the
//! JUnit test summary parsed from those files.
//!
//! Both ends use this module: the controller checks globs at submit and
//! merges the summaries of a sharded run; the node matches globs, hashes
//! the files and parses JUnit after the command exits. Test names and
//! failure messages are program output (untrusted text): they go into
//! the job record and `--failures`, never into a wake message.

use serde::{Deserialize, Serialize};

/// Offer feature: this node collects `--artifact` globs, parses JUnit and
/// answers `artifact_get` (older nodes refuse a plan with `artifacts`:
/// its digest changes, so the controller never sends one there).
pub const FEATURE_ARTIFACTS: &str = "artifacts";

pub const MAX_GLOBS: usize = 32;
pub const MAX_GLOB_LEN: usize = 512;
/// Files one job may keep as artifacts.
pub const MAX_FILES: usize = 1000;
/// One artifact file (the LAN cap, §11.4).
pub const MAX_FILE_BYTES: u64 = 500 << 20;
/// One artifact pulled over the relay.
pub const RELAY_MAX_FILE_BYTES: u64 = 100 << 20;
/// Everything one job keeps.
pub const MAX_TOTAL_BYTES: u64 = 2 << 30;
/// Directory entries one glob may visit (a huge target dir can't stall the node).
pub const MAX_VISITS: usize = 200_000;
/// How long the node keeps artifacts (§13 retention).
pub const KEEP_SECS: i64 = 7 * 86_400;
/// Failing tests kept per summary.
pub const MAX_FAILING: usize = 200;
/// One failure message, after trimming to its first lines.
pub const MAX_MESSAGE_CHARS: usize = 600;
pub const MAX_MESSAGE_LINES: usize = 6;
/// Largest JUnit file the node parses.
pub const MAX_JUNIT_BYTES: u64 = 64 << 20;
/// Notes kept per job (glob matched nothing, a cap was hit, …).
pub const MAX_NOTES: usize = 32;

/// One collected file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactInfo {
    /// The path as matched, `/`-separated, relative to the job's working
    /// directory (or `target/…` for the node's cargo target dir).
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

/// One failing test case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailingTest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `failure` or `error`.
    pub kind: String,
    /// The first lines of the failure text (program output: untrusted).
    pub message: String,
    /// Which shard ran it (sharded runs only, 1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<u32>,
}

/// Counts and failures from one or more JUnit files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub errors: u64,
    pub skipped: u64,
    pub failing: Vec<FailingTest>,
    /// More failures than [`MAX_FAILING`] (the counts stay exact).
    #[serde(default)]
    pub truncated: bool,
    /// The JUnit files these came from.
    #[serde(default)]
    pub sources: Vec<String>,
}

impl TestSummary {
    /// No failed or errored test.
    pub fn passed_all(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }

    /// Add `other` to `self` (counts add up; failures append, capped).
    pub fn absorb(&mut self, other: &TestSummary) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.errors += other.errors;
        self.skipped += other.skipped;
        self.truncated |= other.truncated;
        for f in &other.failing {
            if self.failing.len() >= MAX_FAILING {
                self.truncated = true;
                break;
            }
            self.failing.push(f.clone());
        }
        for s in &other.sources {
            if !self.sources.contains(s) {
                self.sources.push(s.clone());
            }
        }
    }
}

/// What a job produced besides its log (stored on the node ledger, sent
/// in the terminal `state`, kept on the controller's job row).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outputs {
    pub artifacts: Vec<ArtifactInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestSummary>,
    /// Short node-written notes ("target/x/*.xml matched nothing").
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Outputs {
    /// Drop anything over the caps (a node can't flood the controller row).
    pub fn capped(mut self) -> Self {
        self.artifacts.truncate(MAX_FILES);
        self.notes.truncate(MAX_NOTES);
        for n in &mut self.notes {
            *n = clip(n, 300);
        }
        if let Some(t) = &mut self.tests {
            if t.failing.len() > MAX_FAILING {
                t.failing.truncate(MAX_FAILING);
                t.truncated = true;
            }
            for f in &mut t.failing {
                f.name = clip(&f.name, 500);
                f.classname = f.classname.as_deref().map(|c| clip(c, 300));
                f.file = f.file.as_deref().map(|c| clip(c, 300));
                f.message = clip(&f.message, MAX_MESSAGE_CHARS);
            }
            t.sources.truncate(64);
        }
        self
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── globs ────────────────────────────────────────────────────────────

/// Why a glob is refused, or `None` when it's fine. Globs are relative,
/// `/`-separated, never `..`, never absolute.
pub fn glob_refusal(g: &str) -> Option<&'static str> {
    if g.is_empty() || g.len() > MAX_GLOB_LEN || g.contains('\0') || g.contains('\\') {
        return Some("artifact_glob_invalid");
    }
    if g.starts_with('/') || g.starts_with('~') {
        return Some("artifact_glob_absolute");
    }
    if g.split('/').any(|s| s == "..") {
        return Some("artifact_glob_escapes");
    }
    if g.split('/').any(|s| s.is_empty()) {
        return Some("artifact_glob_invalid");
    }
    None
}

/// Check a whole list (count + each glob); the first problem.
pub fn check_globs(globs: &[String]) -> Result<(), (String, String)> {
    if globs.len() > MAX_GLOBS {
        return Err(("artifact_glob_invalid".into(), format!("at most {MAX_GLOBS} --artifact globs")));
    }
    for g in globs {
        if let Some(code) = glob_refusal(g) {
            return Err((code.into(), g.clone()));
        }
    }
    Ok(())
}

/// The glob's segments.
pub fn segments(g: &str) -> Vec<&str> {
    g.split('/').filter(|s| !s.is_empty() && *s != ".").collect()
}

/// Does one path segment match one glob segment (`*`, `?`, `[abc]` not
/// supported: literal)? `*` never matches a `/` (segments have none).
pub fn segment_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // Hidden files only match a pattern that starts with '.'.
    if n.first() == Some(&'.') && p.first() != Some(&'.') {
        return false;
    }
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Does a whole `/`-separated relative path match the glob (with `**`)?
pub fn path_match(glob: &str, path: &str) -> bool {
    fn go(p: &[&str], n: &[&str]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some(&"**") => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some(seg) => !n.is_empty() && segment_match(seg, n[0]) && go(&p[1..], &n[1..]),
        }
    }
    let p = segments(glob);
    let n: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    go(&p, &n)
}

/// True when a segment has no wildcard.
pub fn is_literal(seg: &str) -> bool {
    !seg.contains('*') && !seg.contains('?')
}

// ── JUnit ────────────────────────────────────────────────────────────

/// Does this look like a JUnit report (cheap sniff of the first bytes)?
pub fn looks_like_junit(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("<testsuite")
}

/// Parse one JUnit XML report (nextest, vitest, pytest, go-junit-report,
/// Maven surefire). Counts come from the `<testcase>` elements, never the
/// suite attributes. A case that passed after a retry (`flakyFailure`,
/// `rerunFailure` only) counts as passed.
pub fn parse_junit(xml: &str, source: &str) -> Result<TestSummary, String> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    r.config_mut().trim_text(false);
    let mut st = JunitState { out: TestSummary { sources: vec![source.to_string()], ..Default::default() }, ..Default::default() };
    loop {
        let ev = r.read_event().map_err(|e| format!("{source}: XML error at byte {}: {e}", r.buffer_position()))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => st.open(&e, false),
            // An empty `<testcase/>` never gets an `End`: the next
            // `<testcase>` or the end of the document closes it.
            Event::Empty(e) => st.open(&e, true),
            Event::End(e) => st.close(e.name().as_ref()),
            Event::Text(t) => st.text(&t.decode().map(|s| s.into_owned()).unwrap_or_default()),
            Event::CData(t) => st.text(&String::from_utf8_lossy(t.as_ref())),
            Event::GeneralRef(g) => {
                if let Ok(Some(ch)) = g.resolve_char_ref() {
                    st.text(&ch.to_string());
                } else {
                    let n = g.decode().map(|s| s.into_owned()).unwrap_or_default();
                    st.text(match n.as_str() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(c) = st.case.take() {
        st.finish(c);
    }
    if !st.saw_root {
        return Err(format!("{source}: no <testsuite> or <testsuites> element"));
    }
    Ok(st.out)
}

/// The open test case while parsing.
#[derive(Default)]
struct Case {
    name: String,
    classname: Option<String>,
    file: Option<String>,
    /// `(kind, message attribute)` of the first failure/error.
    verdict: Option<(String, String)>,
    skipped: bool,
    text: String,
    in_verdict: bool,
}

#[derive(Default)]
struct JunitState {
    out: TestSummary,
    case: Option<Case>,
    suite_file: Vec<Option<String>>,
    saw_root: bool,
}

fn attr(e: &quick_xml::events::BytesStart<'_>, k: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == k)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

impl JunitState {
    fn open(&mut self, e: &quick_xml::events::BytesStart<'_>, empty: bool) {
        let name = e.name().as_ref().to_vec();
        match name.as_slice() {
            b"testsuites" => self.saw_root = true,
            b"testsuite" => {
                self.saw_root = true;
                if !empty {
                    self.suite_file.push(attr(e, b"file"));
                }
            }
            b"testcase" => {
                if let Some(c) = self.case.take() {
                    self.finish(c);
                }
                self.case = Some(Case {
                    name: attr(e, b"name").unwrap_or_default(),
                    classname: attr(e, b"classname").filter(|s| !s.is_empty()),
                    file: attr(e, b"file").or_else(|| self.suite_file.last().cloned().flatten()),
                    ..Default::default()
                });
                if empty {
                    let c = self.case.take().unwrap_or_default();
                    self.finish(c);
                }
            }
            b"failure" | b"error" => {
                if let Some(c) = self.case.as_mut() {
                    // The first verdict wins; a later one adds no count.
                    if c.verdict.is_none() {
                        let kind = if name.as_slice() == b"error" { "error" } else { "failure" };
                        c.verdict = Some((kind.to_string(), attr(e, b"message").unwrap_or_default()));
                        c.in_verdict = !empty;
                    }
                }
            }
            b"skipped" => {
                if let Some(c) = self.case.as_mut() {
                    c.skipped = true;
                }
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &[u8]) {
        match name {
            b"testcase" => {
                if let Some(c) = self.case.take() {
                    self.finish(c);
                }
            }
            b"testsuite" => {
                self.suite_file.pop();
            }
            b"failure" | b"error" => {
                if let Some(c) = self.case.as_mut() {
                    c.in_verdict = false;
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, s: &str) {
        if let Some(c) = self.case.as_mut().filter(|c| c.in_verdict && c.text.len() < 8192) {
            c.text.push_str(s);
        }
    }

    fn finish(&mut self, c: Case) {
        let out = &mut self.out;
        out.total += 1;
        match c.verdict {
            Some((kind, msg)) => {
                if kind == "error" {
                    out.errors += 1;
                } else {
                    out.failed += 1;
                }
                if out.failing.len() < MAX_FAILING {
                    out.failing.push(FailingTest {
                        name: c.name,
                        classname: c.classname,
                        file: c.file,
                        kind,
                        message: first_lines(if msg.trim().is_empty() { &c.text } else { &msg }),
                        shard: None,
                    });
                } else {
                    out.truncated = true;
                }
            }
            None if c.skipped => out.skipped += 1,
            None => out.passed += 1,
        }
    }
}

/// The first lines of a failure text, trimmed and capped.
pub fn first_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).take(MAX_MESSAGE_LINES).collect();
    clip(&lines.join("\n"), MAX_MESSAGE_CHARS)
}

/// Merge shard summaries (in shard order), tagging each failure with its
/// 1-based shard.
pub fn merge_shards(parts: &[(u32, &TestSummary)]) -> TestSummary {
    let mut out = TestSummary::default();
    for (shard, t) in parts {
        let mut tagged = (*t).clone();
        for f in &mut tagged.failing {
            f.shard = Some(*shard);
        }
        out.absorb(&tagged);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEXTEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="5" failures="2" errors="0">
  <testsuite name="k2-core" tests="5" disabled="0" errors="0" failures="2">
    <testcase name="compute::a::passes" classname="k2-core" timestamp="x" time="0.01"></testcase>
    <testcase name="compute::a::fails" classname="k2-core" time="0.02">
      <failure type="test failure">thread 'compute::a::fails' panicked at src/a.rs:10:5:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1`
more
more2</failure>
      <system-out>out</system-out>
    </testcase>
    <testcase name="compute::a::flaky" classname="k2-core">
      <flakyFailure type="test failure">first try</flakyFailure>
    </testcase>
    <testcase name="compute::a::errs" classname="k2-core"><error message="setup &amp; teardown &lt;bad&gt;"/></testcase>
    <testcase name="compute::a::ignored" classname="k2-core"><skipped/></testcase>
  </testsuite>
</testsuites>"#;

    #[test]
    fn nextest_junit_counts_and_failures() {
        let t = parse_junit(NEXTEST, "target/nextest/ci/junit.xml").unwrap();
        assert_eq!((t.total, t.passed, t.failed, t.errors, t.skipped), (5, 2, 1, 1, 1), "{t:?}");
        assert!(!t.passed_all());
        assert_eq!(t.failing.len(), 2);
        let f = &t.failing[0];
        assert_eq!(f.name, "compute::a::fails");
        assert_eq!(f.classname.as_deref(), Some("k2-core"));
        assert_eq!(f.kind, "failure");
        assert!(f.message.starts_with("thread 'compute::a::fails' panicked at src/a.rs:10:5:"), "{}", f.message);
        assert_eq!(f.message.lines().count(), MAX_MESSAGE_LINES, "first lines only: {}", f.message);
        assert_eq!(t.failing[1].kind, "error");
        assert_eq!(t.failing[1].message, "setup & teardown <bad>");
        assert_eq!(t.sources, vec!["target/nextest/ci/junit.xml".to_string()]);
    }

    #[test]
    fn empty_testcases_and_bare_testsuite_and_file_attr() {
        let x = r#"<testsuite name="vitest" file="src/a.test.ts"><testcase name="a"/><testcase name="b"><failure message="boom"/></testcase><testcase name="c"/></testsuite>"#;
        let t = parse_junit(x, "junit.xml").unwrap();
        assert_eq!((t.total, t.passed, t.failed), (3, 2, 1));
        assert_eq!(t.failing[0].message, "boom");
        assert_eq!(t.failing[0].file.as_deref(), Some("src/a.test.ts"));
    }

    #[test]
    fn not_junit_and_broken_xml_are_errors() {
        assert!(parse_junit("<root/>", "x.xml").is_err());
        assert!(parse_junit("<testsuite><testcase name='a'></testsuite>", "x.xml").is_err());
        assert!(looks_like_junit(NEXTEST.as_bytes()));
        assert!(!looks_like_junit(b"<project/>"));
    }

    #[test]
    fn failing_list_caps_but_counts_stay_exact() {
        let mut x = String::from("<testsuite>");
        for i in 0..(MAX_FAILING + 5) {
            x.push_str(&format!("<testcase name=\"t{i}\"><failure message=\"m\"/></testcase>"));
        }
        x.push_str("</testsuite>");
        let t = parse_junit(&x, "j.xml").unwrap();
        assert_eq!(t.failed as usize, MAX_FAILING + 5);
        assert_eq!(t.failing.len(), MAX_FAILING);
        assert!(t.truncated);
    }

    #[test]
    fn merge_tags_shards_and_adds_counts() {
        let a = parse_junit(NEXTEST, "a").unwrap();
        let b = parse_junit(r#"<testsuite><testcase name="x"/></testsuite>"#, "b").unwrap();
        let m = merge_shards(&[(1, &a), (2, &b)]);
        assert_eq!((m.total, m.passed, m.failed, m.errors), (6, 3, 1, 1));
        assert!(m.failing.iter().all(|f| f.shard == Some(1)));
        assert_eq!(m.sources, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn globs_are_relative_and_match_with_double_star() {
        assert_eq!(glob_refusal("/etc/passwd"), Some("artifact_glob_absolute"));
        assert_eq!(glob_refusal("a/../../b"), Some("artifact_glob_escapes"));
        assert_eq!(glob_refusal("a//b"), Some("artifact_glob_invalid"));
        assert_eq!(glob_refusal(""), Some("artifact_glob_invalid"));
        assert_eq!(glob_refusal("target/nextest/**/junit.xml"), None);
        assert!(path_match("target/nextest/**/junit.xml", "target/nextest/ci/junit.xml"));
        assert!(path_match("target/nextest/**/junit.xml", "target/nextest/junit.xml"));
        assert!(!path_match("target/nextest/**/junit.xml", "target/other/junit.xml"));
        assert!(path_match("dist/*.tar.gz", "dist/k2-1.tar.gz"));
        assert!(!path_match("dist/*.tar.gz", "dist/sub/k2.tar.gz"));
        assert!(path_match("**/*.xml", "a/b/c.xml"));
        assert!(path_match("report-?.txt", "report-1.txt"));
        assert!(!path_match("*", ".hidden"), "hidden files need an explicit dot");
        assert!(path_match(".k2/*", ".k2/x"));
        assert!(check_globs(&vec!["a".to_string(); MAX_GLOBS + 1]).is_err());
    }

    #[test]
    fn outputs_cap_trims_hostile_sizes() {
        let o = Outputs {
            artifacts: vec![ArtifactInfo { name: "a".into(), sha256: "s".into(), bytes: 1 }; MAX_FILES + 3],
            tests: Some(TestSummary {
                failing: vec![FailingTest { name: "n".repeat(2000), classname: None, file: None, kind: "failure".into(), message: "m".repeat(5000), shard: None }; MAX_FAILING + 1],
                ..Default::default()
            }),
            notes: vec!["n".into(); MAX_NOTES + 4],
        }
        .capped();
        assert_eq!(o.artifacts.len(), MAX_FILES);
        assert_eq!(o.notes.len(), MAX_NOTES);
        let t = o.tests.unwrap();
        assert_eq!(t.failing.len(), MAX_FAILING);
        assert!(t.truncated);
        assert!(t.failing[0].message.chars().count() <= MAX_MESSAGE_CHARS);
    }
}
