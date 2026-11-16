//! `evidence`: the evidence notes' tools (tools/evidence/README.md).
//!
//! - `evidence check [--runs DIR] [FILE.md ...]`: every count in the evidence notes is cited,
//!   and every citation matches its run record (the forms and rules are tools/evidence/README.md's).
//!   Defaults: `--runs docs/evidence/runs`, the files `docs/evidence/*.md`. Exit 0 clean, 1
//!   problems (each on a line), 2 a record that isn't JSON.
//! - `evidence collect`: job logs to run records (`docs/evidence/runs/<job>.json`):
//!   `--log-cmd 'CMD {job}' JOB...` (or RUNREC_LOG_CMD), `--log FILE --job JOB`, `--print` (the
//!   records on stdout, nothing written), `--out DIR`. A `RUNREC {json}` line is taken as it is;
//!   other runs are read from TLC's output, `### NAME [...]` summaries and the enumerator's
//!   SUMMARY lines. A record file is written as Python's json wrote it (one-space indent,
//!   ASCII); its `collected` date is the UTC date.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::{Ctx, Fail, Report, ERROR, OK};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Every count in the evidence notes is cited, and every citation matches its run
    Check {
        /// The run records' folder
        #[arg(long, value_name = "DIR", default_value = "docs/evidence/runs")]
        runs: PathBuf,
        /// The notes (default: docs/evidence/*.md)
        files: Vec<String>,
    },
    /// Job logs to run records
    Collect {
        /// A shell command that prints one job's log; {job} is replaced by the job id (or
        /// RUNREC_LOG_CMD)
        #[arg(long, value_name = "CMD")]
        log_cmd: Option<String>,
        /// Where the record files go
        #[arg(long, value_name = "DIR", default_value = "docs/evidence/runs")]
        out: PathBuf,
        /// A saved log instead of a command (with --job)
        #[arg(long, value_name = "FILE", requires = "job")]
        log: Option<PathBuf>,
        /// The job a saved log is from
        #[arg(long, value_name = "JOB")]
        job: Option<String>,
        /// Print the records instead of writing them (with --log)
        #[arg(long)]
        print: bool,
        /// The jobs whose logs the command prints
        jobs: Vec<String>,
    },
}

pub fn run(cmd: &Cmd, ctx: &Ctx) -> Result<Report, Fail> {
    match cmd {
        Cmd::Check { runs, files } => check(ctx, runs, files),
        Cmd::Collect {
            log_cmd,
            out,
            log,
            job,
            print,
            jobs,
        } => collect(
            ctx,
            log_cmd.as_deref(),
            out,
            log.as_deref(),
            job.as_deref(),
            *print,
            jobs,
        ),
    }
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a fixed pattern")
}

// ---- Python's habits -------------------------------------------------------------------------

/// Python's `str()` of a record's value (`None` for a missing one).
fn py_s(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".into(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.into(),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
        Some(Value::Number(n)) => crate::pyjson::float(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(other) => crate::pyjson::dumps(other, None),
    }
}

/// A value Python's `isinstance(v, int)` takes (a bool too), as an integer.
fn py_int(v: Option<&Value>) -> Option<i128> {
    match v? {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

/// Python's `n == v` for an integer n.
fn py_eq(n: i128, v: Option<&Value>) -> bool {
    match v {
        Some(Value::Number(x)) if !(x.is_i64() || x.is_u64()) => x.as_f64() == Some(n as f64),
        other => py_int(other) == Some(n),
    }
}

/// Python's truth value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn digit(c: char) -> bool {
    c.is_numeric()
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `int(s.replace(",", ""))`.
fn as_int(s: &str) -> i128 {
    s.chars()
        .filter(|c| *c != ',')
        .fold(0i128, |a, c| a * 10 + c.to_digit(10).unwrap_or(0) as i128)
}

/// `"{:,}".format(n)`.
fn commas(n: i128) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

// ---- check -----------------------------------------------------------------------------------

struct Pats {
    cite: Regex,
    value: Regex,
    unit: Regex,
    exempt: Regex,
    least: Regex,
    auto: Regex,
    heading: Regex,
    separator: Regex,
    cap: Regex,
}

fn pats() -> Pats {
    Pats {
        cite: re(r"\[run ([0-9A-Za-z]{4,40})/([^\]\s:]+)(?::([A-Za-z][A-Za-z0-9_]*))?\]"),
        value: re(concat!(
            r"(?P<ge>≥\s*)?(?P<n>\d{1,3}(?:,\d{3})+|\d+)",
            r"(?:\s*/\s*(?P<d>\d+))?",
            r"(?:\s+(?:distinct\s+)?states)?",
            r"(?P<partial>\s*,\s*partial\s*\((?P<cap>[^)]*)\))?\s*$"
        )),
        unit: re(r"^\s*(?:s|ms|min|h|%|B|KB|MB|GB|KiB|MiB|GiB|G|M|k|/s|×|x\b)\b|^\s*(?:%|×|/s)"),
        exempt: re(r"\[(?:estimate|derived|unverified:[^\]]*)\]"),
        least: re(
            r"(?i)\bleast\s+(?:violating\s+|failing\s+)?depths?\b|\bshortest\s+(?:traces?|counterexamples?)\b",
        ),
        auto: re(r"(?i)workers\s+auto"),
        heading: re(r"^(#{1,6})\s"),
        separator: re(r"^\s*:?-+:?\s*$"),
        cap: re(r"(\d+(?:\.\d+)?)\s*(s|sec|min|h)\b"),
    }
}

type Runs = BTreeMap<String, BTreeMap<String, Map<String, Value>>>;

/// The run records: job -> run -> record; `Err` with the problem line for a file that isn't JSON.
fn load_runs(dir: &Path, shown: &Path) -> Result<Runs, String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    let mut runs = Runs::new();
    for n in names {
        let p = dir.join(&n);
        let shown_p = shown.join(&n);
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let doc: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{}: not JSON: {e}", shown_p.display()))?;
        let job = match doc.get("job") {
            v if truthy(v) => py_s(v),
            _ => n[..n.len() - 5].to_string(),
        };
        let mut recs = BTreeMap::new();
        for r in doc
            .get("runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(o) = r.as_object() {
                recs.insert(py_s(o.get("run")), o.clone());
            }
        }
        runs.insert(job, recs);
    }
    Ok(runs)
}

fn check_records(runs: &Runs) -> Vec<String> {
    let mut probs = Vec::new();
    for (job, recs) in runs {
        for (name, r) in recs {
            if truthy(r.get("not_run_ok")) {
                continue;
            }
            let st = r.get("status").and_then(Value::as_str);
            if let Some(s @ ("ERROR" | "NO_RESULT" | "UNKNOWN")) = st {
                probs.push(format!("{job}/{name} has status {s}: it didn't run (give the record `not_run_ok` with the reason if that is known and fine)"));
            }
            if st == Some("COMPLETE") {
                for k in ["states", "samples", "traces", "histories", "walks", "tests"] {
                    if py_eq(0, r.get(k)) {
                        probs.push(format!(
                            "{job}/{name} is COMPLETE with {k} 0: it ran nothing"
                        ));
                    }
                }
            }
        }
    }
    probs
}

fn cap_matches(p: &Pats, text: &str, cap: f64) -> bool {
    let t = text.replace(',', "");
    p.cap.captures_iter(&t).any(|c| {
        let n: f64 = c[1].parse().unwrap_or(f64::NAN);
        let mult = match &c[2] {
            "min" => 60.0,
            "h" => 3600.0,
            _ => 1.0,
        };
        (n * mult - cap).abs() <= 5f64.max(0.02 * cap)
    })
}

fn check_citation(
    p: &Pats,
    before: &str,
    job: &str,
    run: &str,
    field: Option<&str>,
    runs: &Runs,
) -> Option<String> {
    let Some(rec) = runs.get(job).and_then(|r| r.get(run)) else {
        return Some(format!("no record for run {job}/{run}"));
    };
    let t = before.replace("**", "");
    let t = crate::py::rstrip(&t);
    let Some(m) = p.value.captures(t) else {
        return Some(format!("no number before the citation of {job}/{run}"));
    };
    let n = as_int(&m["n"]);
    let st = rec.get("status");
    let lower = m.name("ge").is_some();
    let partial = m.name("partial").is_some();
    if let Some(f) = field {
        let Some(v) = py_int(rec.get(f)) else {
            return Some(format!("run {job}/{run} has no numeric field {f}"));
        };
        let shown = py_s(rec.get(f));
        if lower || partial {
            return Some(format!(
                "{job}/{run}:{f} is exact ({shown}): cite it without ≥ / partial"
            ));
        }
        return if n == v {
            None
        } else {
            Some(format!("{job}/{run}:{f} is {shown}, cited {n}"))
        };
    }
    let states = rec.get("states");
    match st.and_then(Value::as_str) {
        Some("COMPLETE") => {
            if lower || partial {
                return Some(format!(
                    "{job}/{run} is COMPLETE ({} states): cite the exact count, not a lower bound",
                    py_s(states)
                ));
            }
            if !py_eq(n, states) {
                return Some(format!(
                    "{job}/{run} is COMPLETE with {} states, cited {n}",
                    py_s(states)
                ));
            }
            if let Some(d) = m.name("d") {
                let d = as_int(d.as_str());
                if !py_eq(d, rec.get("depth")) {
                    return Some(format!(
                        "{job}/{run} has depth {}, cited {d}",
                        py_s(rec.get("depth"))
                    ));
                }
            }
            None
        }
        Some("PARTIAL") => {
            if !(lower && partial) {
                let s = if truthy(states) {
                    py_int(states).unwrap_or(0)
                } else {
                    0
                };
                return Some(format!(
                    "{job}/{run} is PARTIAL (stopped with {} queued): write '≥ {}, partial (cap ...)'",
                    py_s(rec.get("queued")),
                    commas(s)
                ));
            }
            if !py_eq(n, states) {
                return Some(format!(
                    "{job}/{run} is PARTIAL with ≥ {} states, cited ≥ {n}",
                    py_s(states)
                ));
            }
            let cap = rec.get("cap_s").filter(|v| !v.is_null());
            if let Some(cap) = cap {
                let c = cap.as_f64().unwrap_or(if cap.as_bool() == Some(true) {
                    1.0
                } else {
                    0.0
                });
                let text = m.name("cap").map_or("", |x| x.as_str());
                if !cap_matches(p, text, c) {
                    return Some(format!(
                        "{job}/{run} stopped at its {} s cap; the citation says ({})",
                        py_s(Some(cap)),
                        crate::py::strip(text)
                    ));
                }
            }
            None
        }
        Some("VIOLATION") => {
            let want = match rec.get("trace_states") {
                Some(v) if !v.is_null() => Some(v),
                _ => rec.get("depth"),
            };
            if lower || partial {
                return Some(format!(
                    "{job}/{run} is a VIOLATION: cite its trace length, not a lower bound"
                ));
            }
            if py_eq(n, want) {
                None
            } else {
                Some(format!(
                    "{job}/{run} is a VIOLATION with trace length {}, cited {n}",
                    py_s(want)
                ))
            }
        }
        _ => Some(format!(
            "{job}/{run} has status {}: it backs no number",
            py_s(st)
        )),
    }
}

fn check_least(job: &str, run: &str, field: Option<&str>, runs: &Runs) -> Option<String> {
    let rec = runs.get(job)?.get(run)?;
    if rec.get("status").and_then(Value::as_str) != Some("VIOLATION") {
        return None;
    }
    if !matches!(field, None | Some("trace_states") | Some("depth")) {
        return None;
    }
    if rec.get("tool").and_then(Value::as_str) == Some("enumerator") {
        if rec.get("depth").map_or(true, Value::is_null) {
            let mode = if truthy(rec.get("mode")) {
                py_s(rec.get("mode"))
            } else {
                "non-BFS".into()
            };
            return Some(format!("{job}/{run} backs a least depth, but it is an enumerator {mode} search without BFS levels: cite a BFS run"));
        }
        return None;
    }
    let w = rec.get("workers");
    if py_eq(1, w) {
        return None;
    }
    let ws = match w {
        Some(v) if !v.is_null() => format!("{} workers", py_s(Some(v))),
        _ => "an unrecorded worker count".to_string(),
    };
    Some(format!("{job}/{run} backs a least depth, but it is TLC with {ws}: several workers are not a strict BFS, so cite a run with workers: 1 (TLC_WORKERS=1) or the enumerator's BFS, or label it -workers auto"))
}

fn cells(line: &str) -> Vec<String> {
    let mut s = crate::py::strip(line);
    s = s.strip_prefix('|').unwrap_or(s);
    s = s.strip_suffix('|').unwrap_or(s);
    let mut out = vec![String::new()];
    let mut prev = None;
    for c in s.chars() {
        if c == '|' && prev != Some('\\') {
            out.push(String::new());
        } else {
            out.last_mut().expect("a cell").push(c);
        }
        prev = Some(c);
    }
    out
}

/// The counts in `s` as the old pattern finds them: `1,234` or `1234` and longer, not part of a
/// longer token on either side.
fn counts(s: &str) -> Vec<(usize, usize)> {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    let n = cs.len();
    let at = |i: usize| cs.get(i).map(|x| x.1);
    let byte = |i: usize| if i < n { cs[i].0 } else { s.len() };
    let ahead_ok = |e: usize| match at(e) {
        None => true,
        Some(c) if word(c) || c == ',' => false,
        Some('.') | Some('-') => !at(e + 1).is_some_and(digit),
        _ => true,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let behind_ok = i == 0 || {
            let p = cs[i - 1].1;
            !(word(p) || ".,/#^×·-".contains(p))
        };
        let mut end = None;
        if behind_ok && digit(cs[i].1) {
            let run = (i..n).take_while(|&k| digit(cs[k].1)).count();
            // \d{1,3}(?:,\d{3})+
            'alt1: for k in (1..=run.min(3)).rev() {
                let mut ends = Vec::new();
                let mut p = i + k;
                while at(p) == Some(',') && (1..=3).all(|d| at(p + d).is_some_and(digit)) {
                    p += 4;
                    ends.push(p);
                }
                for &e in ends.iter().rev() {
                    if ahead_ok(e) {
                        end = Some(e);
                        break 'alt1;
                    }
                }
            }
            // \d{4,}
            if end.is_none() && run >= 4 {
                end = (4..=run).rev().map(|l| i + l).find(|&e| ahead_ok(e));
            }
        }
        match end {
            Some(e) => {
                out.push((byte(i), byte(e)));
                i = e;
            }
            None => i += 1,
        }
    }
    out
}

fn check_file(p: &Pats, path: &str, text: &str, runs: &Runs) -> Vec<String> {
    let text = crate::py::universal_newlines(text);
    let mut probs = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut table_least = false;
    let mut sections: Vec<(usize, bool)> = Vec::new();
    let mut in_code = false;
    for (i, line) in text.split('\n').enumerate() {
        let i = i + 1;
        if crate::py::strip(line).starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(h) = p.heading.captures(line) {
            let lvl = h[1].len();
            sections.retain(|x| x.0 < lvl);
            sections.push((lvl, p.least.is_match(line)));
        }
        let section_least = sections.iter().any(|x| x.1);
        let is_row = crate::py::strip(line).starts_with('|');
        if !is_row {
            header = None;
            table_least = false;
        }
        let mut segments: Vec<(String, String, bool)> = Vec::new();
        if is_row {
            let cs = cells(line);
            match &header {
                None => {
                    header = Some(
                        cs.iter()
                            .map(|c| crate::py::strip(c).to_lowercase())
                            .collect(),
                    );
                    table_least = p.least.is_match(line);
                    continue;
                }
                Some(hdr) => {
                    if cs.iter().all(|c| p.separator.is_match(c)) {
                        continue;
                    }
                    for (k, c) in cs.iter().enumerate() {
                        segments.push((c.clone(), hdr.get(k).cloned().unwrap_or_default(), true));
                    }
                }
            }
        } else {
            segments.push((line.to_string(), String::new(), false));
        }
        for (seg, head, in_table) in segments {
            let mut masked = seg.clone().into_bytes();
            let mut pos = 0;
            let least = section_least
                || if in_table {
                    table_least
                } else {
                    p.least.is_match(&seg)
                };
            for m in p.cite.captures_iter(&seg) {
                let whole = m.get(0).expect("the citation");
                let before = &seg[pos..whole.start()];
                let (job, run, field) = (&m[1], &m[2], m.get(3).map(|f| f.as_str()));
                if let Some(pr) = check_citation(p, before, job, run, field, runs) {
                    probs.push(format!("{path}:{i}: {pr}"));
                }
                let labelled =
                    p.auto.is_match(&head) || p.auto.is_match(if in_table { &seg } else { before });
                if least && !labelled {
                    if let Some(pr) = check_least(job, run, field, runs) {
                        probs.push(format!("{path}:{i}: {pr}"));
                    }
                }
                // mask the cited value and the citation
                let spaced = before.replace("**", "  ");
                let v = p.value.find(crate::py::rstrip(&spaced));
                let start = pos + v.map_or(before.len(), |v| v.start());
                for b in &mut masked[start..whole.end()] {
                    *b = b' ';
                }
                pos = whole.end();
            }
            if !in_table
                || head.contains("job")
                || head.contains("commit")
                || p.exempt.is_match(&seg)
            {
                continue;
            }
            let masked = String::from_utf8(masked).expect("spaces at char boundaries");
            for (a, b) in counts(&masked) {
                if p.unit.is_match(&masked[b..]) {
                    continue;
                }
                probs.push(format!("{path}:{i}: uncited count {} in a table (cite it as N [run JOB/RUN], or mark the cell [estimate] / [derived] / [unverified: ...])", &masked[a..b]));
            }
        }
    }
    probs
}

fn check(ctx: &Ctx, runs_dir: &Path, files: &[String]) -> Result<Report, Fail> {
    let p = pats();
    let mut files = files.to_vec();
    if files.is_empty() {
        let dir = ctx.path(Path::new("docs/evidence"));
        let mut found: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".md") && !n.starts_with('.'))
                    .map(|n| format!("docs/evidence/{n}"))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        files = found;
    }
    let runs = match load_runs(&ctx.path(runs_dir), runs_dir) {
        Ok(r) => r,
        Err(line) => {
            return Ok(Report::new(
                crate::USAGE,
                format!("{line}\n"),
                json!({ "ok": false, "error": line }),
            ));
        }
    };
    let mut probs = check_records(&runs);
    for f in &files {
        let fp = ctx.path(Path::new(f));
        let text = std::fs::read_to_string(&fp).map_err(|e| Fail::io(&fp, e))?;
        probs.extend(check_file(&p, f, &text, &runs));
    }
    let mut text = String::new();
    for pr in &probs {
        text += pr;
        text.push('\n');
    }
    text += &format!(
        "evidence check: {} problem(s) in {} file(s), {} run record file(s)\n",
        probs.len(),
        files.len(),
        runs.len()
    );
    let code = if probs.is_empty() { OK } else { ERROR };
    Ok(Report::new(
        code,
        text,
        json!({ "ok": probs.is_empty(), "problems": probs }),
    ))
}

// ---- collect ---------------------------------------------------------------------------------

struct Cp {
    viol: Regex,
    of_module: Regex,
    inv_name: Regex,
    state: Regex,
    workers: Regex,
    depth: Regex,
    distinct: Regex,
    left: Regex,
    generated: Regex,
    summary: Regex,
    kv: Regex,
    old_violation: Regex,
    tlc_end: Regex,
    hash: Regex,
    cap: Regex,
    trace_states: Regex,
    end: Regex,
    trace: Regex,
    trace_name: Regex,
    spaces: Regex,
}

fn cp() -> Cp {
    const NUM: &str = "[0-9][0-9,]*";
    Cp {
        viol: re(
            r"(?:Invariant|property) (\S+)(?: of module \S+)? (?:is|was|were) violated|properties were violated",
        ),
        of_module: re(r"of module (\S+)"),
        inv_name: re(r"(?:Invariant|property) (\S+)"),
        state: re(r"^State ([0-9]+):"),
        workers: re(r" with ([0-9]+) workers? on "),
        depth: re(r"depth of the complete state graph search is ([0-9]+)"),
        distinct: re(&format!("({NUM}) distinct states found")),
        left: re(&format!("({NUM}) states left on queue")),
        generated: re(&format!("({NUM}) states generated")),
        summary: re(r"^SUMMARY (.*)$"),
        kv: re(r"(\w+)=(\S+)"),
        old_violation: re(r"^VIOLATION \S+ at depth ([0-9]+)\b"),
        tlc_end: re(r"^=== (tlc|live) (\S+).*?: exit (-?[0-9]+), wall ([0-9]+)s"),
        hash: re(r"^### (\S+) \[([^\]]*)\]: (.*)$"),
        cap: re(r"([0-9]+)s cap"),
        trace_states: re(r"trace states: ([0-9]+)"),
        end: re(r"^=== (.+?): exit (-?[0-9]+), wall ([0-9]+)s"),
        trace: re(r"^trace \S+: ([0-9]+) states"),
        trace_name: re(r"of module (\S+)|Invariant (\S+)"),
        spaces: re(r"\s+"),
    }
}

fn num(s: &str) -> Option<i128> {
    let t: String = s.chars().filter(|c| *c != ',').collect();
    t.parse().ok()
}

fn jint(n: i128) -> Value {
    Value::from(n as i64)
}

fn opt_int(n: Option<i128>) -> Value {
    n.map_or(Value::Null, jint)
}

/// A violation line belongs to RUN unless it names another instance's invariant.
fn ours(c: &Cp, line: &str, run: Option<&str>) -> bool {
    let Some(run) = run else { return true };
    let base = format!("{}_", run.split('#').next().unwrap_or(""));
    if let Some(m) = c.of_module.captures(line) {
        return m[1].starts_with(&base);
    }
    match c.inv_name.captures(line) {
        Some(m) if m[1].contains('_') => m[1].starts_with(&base),
        _ => true,
    }
}

/// Status and counts from a run's TLC output lines; rc: TLC's exit.
fn tlc_record<S: AsRef<str>>(
    c: &Cp,
    lines: &[S],
    run: Option<&str>,
    rc: Option<i64>,
) -> Map<String, Value> {
    let (mut viol, mut err): (Option<String>, Option<String>) = (None, None);
    let mut trace = 0i128;
    let mut last: Option<String> = None;
    let mut depth = None;
    let mut workers = None;
    for l in lines {
        let l = l.as_ref();
        if l.trim_start_matches(crate::py::is_space)
            .starts_with("trace ")
        {
            continue;
        }
        let mv = c.viol.find(l);
        if mv.is_some() && !ours(c, l, run) {
            continue;
        }
        if mv.is_some() && viol.is_none() {
            viol = mv.map(|m| m.as_str().to_string());
        } else if l.starts_with("Error:")
            && !l.contains("behavior up to this point")
            && err.is_none()
        {
            err = Some(crate::py::strip(l).to_string());
        }
        if let Some(ms) = c.state.captures(l) {
            if viol.is_some() {
                trace = trace.max(num(&ms[1]).unwrap_or(0));
            }
        }
        if let Some(mw) = c.workers.captures(l) {
            if l.starts_with("Running") {
                workers = num(&mw[1]);
            }
        }
        if l.contains("distinct states found") {
            last = Some(l.to_string());
        }
        if let Some(m) = c.depth.captures(l) {
            depth = num(&m[1]);
        }
    }
    let (mut states, mut queued, mut generated, mut progress) = (None, None, None, false);
    if let Some(last) = &last {
        states = c.distinct.captures(last).and_then(|m| num(&m[1]));
        queued = c.left.captures(last).and_then(|m| num(&m[1]));
        generated = c.generated.captures(last).and_then(|m| num(&m[1]));
        progress = last
            .trim_start_matches(crate::py::is_space)
            .starts_with("Progress");
    }
    let mut rec = Map::new();
    rec.insert("states".into(), opt_int(states));
    rec.insert("depth".into(), Value::Null);
    rec.insert("queued".into(), opt_int(queued));
    rec.insert("generated".into(), opt_int(generated));
    if let Some(w) = workers {
        rec.insert("workers".into(), jint(w));
    }
    if let Some(v) = viol {
        rec.insert("status".into(), "VIOLATION".into());
        rec.insert("violation".into(), v.into());
        rec.insert(
            "trace_states".into(),
            if trace >= 2 { jint(trace) } else { Value::Null },
        );
        rec.insert("depth".into(), opt_int(depth));
    } else if rc.is_some_and(|rc| rc != 0 && rc != 124 && !(10..=14).contains(&rc)) {
        rec.insert("status".into(), "ERROR".into());
        let e = err.unwrap_or_else(|| format!("TLC exit {}", rc.unwrap_or(0)));
        rec.insert("error".into(), e.into());
    } else if let Some(e) = err {
        rec.insert("status".into(), "ERROR".into());
        rec.insert("error".into(), e.into());
    } else if depth.is_some() && queued == Some(0) && !progress {
        rec.insert("status".into(), "COMPLETE".into());
        rec.insert("depth".into(), opt_int(depth));
    } else if states.is_some() {
        rec.insert("status".into(), "PARTIAL".into());
    } else {
        rec.insert("status".into(), "NO_RESULT".into());
    }
    rec
}

fn extend(r: &mut Map<String, Value>, other: Map<String, Value>) {
    for (k, v) in other {
        r.insert(k, v);
    }
}

/// The run records in a job's log.
pub fn parse_log(text: &str, job: &str) -> Result<Vec<Map<String, Value>>, Fail> {
    let c = cp();
    let text = crate::py::universal_newlines(text);
    let mut recs: Vec<Map<String, Value>> = Vec::new();
    let mut buf: Vec<String> = Vec::new();
    let mut pending: Option<BTreeMap<String, String>> = None;
    for line in crate::py::splitlines(&text) {
        let s = crate::py::strip(line);
        if let Some(j) = s.strip_prefix("RUNREC ") {
            let Ok(Value::Object(mut r)) = serde_json::from_str::<Value>(j) else {
                if serde_json::from_str::<Value>(j).is_ok() {
                    return Err(Fail::error(
                        "evidence collect: a RUNREC record is not a JSON object",
                    ));
                }
                continue;
            };
            r.insert("job".into(), job.into());
            r.insert("source".into(), "RUNREC".into());
            recs.push(r);
            buf.clear();
            continue;
        }
        if let Some(m) = c.summary.captures(s) {
            let mut kv: BTreeMap<String, String> =
                c.kv.captures_iter(&m[1])
                    .map(|x| (x[1].to_string(), x[2].to_string()))
                    .collect();
            let bfs = matches!(
                kv.get("mode").map(String::as_str),
                Some("bfs" | "bfsm" | "cp")
            );
            if !kv.contains_key("violation_depth") && bfs {
                for b in &buf {
                    if let Some(mv) = c.old_violation.captures(crate::py::strip(b)) {
                        kv.insert("violation_depth".into(), mv[1].to_string());
                        break;
                    }
                }
            }
            pending = Some(kv);
            continue;
        }
        if let Some(m) = c.tlc_end.captures(s) {
            let exit: i64 = m[3].parse().unwrap_or(0);
            let wall = num(&m[4]).unwrap_or(0);
            let mut r = Map::new();
            r.insert("job".into(), job.into());
            r.insert("run".into(), m[2].into());
            r.insert("tool".into(), "tlc".into());
            r.insert("exit".into(), exit.into());
            r.insert("wall_s".into(), jint(wall));
            r.insert(
                "cap_s".into(),
                if &m[3] == "124" {
                    jint(wall)
                } else {
                    Value::Null
                },
            );
            r.insert("source".into(), "log".into());
            extend(&mut r, tlc_record(&c, &buf, Some(&m[2]), Some(exit)));
            if r["status"] == "NO_RESULT" && exit == 0 {
                r.insert("status".into(), "UNKNOWN".into());
            }
            recs.push(r);
            buf.clear();
            pending = None;
            continue;
        }
        if let Some(m) = c.hash.captures(s) {
            let parts: Vec<String> = m[3]
                .split('|')
                .map(|x| crate::py::strip(x).to_string())
                .collect();
            let capm = c.cap.captures(&m[2]).and_then(|x| num(&x[1]));
            let run = m[1].to_string();
            let mut r = Map::new();
            r.insert("job".into(), job.into());
            r.insert("run".into(), run.clone().into());
            r.insert("tool".into(), "tlc".into());
            r.insert("cap_s".into(), Value::Null);
            r.insert("source".into(), "log".into());
            let prev = recs
                .last()
                .filter(|p| {
                    p.get("run") == Some(&Value::from(run.clone()))
                        && p.get("source") == Some(&Value::from("log"))
                })
                .cloned();
            let prev_exit = prev
                .as_ref()
                .and_then(|p| p.get("exit"))
                .and_then(Value::as_i64);
            extend(&mut r, tlc_record(&c, &parts, Some(&run), prev_exit));
            if m[3].contains("trace states:") {
                let t = c
                    .trace_states
                    .captures(&m[3])
                    .and_then(|t| num(&t[1]))
                    .ok_or_else(|| {
                        Fail::error("evidence collect: `trace states:` with no number")
                    })?;
                r.insert("trace_states".into(), jint(t));
            }
            r.insert("cap_s".into(), opt_int(capm));
            if let Some(prev) = prev {
                for k in ["exit", "wall_s"] {
                    r.insert(k.into(), prev.get(k).cloned().unwrap_or(Value::Null));
                }
                if capm.is_none() && prev.get("exit").and_then(Value::as_i64) == Some(124) {
                    r.insert(
                        "cap_s".into(),
                        prev.get("wall_s").cloned().unwrap_or(Value::Null),
                    );
                }
                *recs.last_mut().expect("the previous record") = r;
            } else {
                recs.push(r);
            }
            buf.clear();
            continue;
        }
        if let Some(m) = c.end.captures(s) {
            if let Some(kv) = pending.take() {
                let get = |k: &str| kv.get(k).map(String::as_str);
                let v = get("violations").unwrap_or("none").to_string();
                let complete = get("complete") == Some("true");
                let numk = |k: &str| -> Result<Value, Fail> {
                    match get(k) {
                        None => Ok(Value::Null),
                        Some(x) => num(x).map(jint).ok_or_else(|| {
                            Fail::error(format!(
                                "evidence collect: SUMMARY {k}={x} is not a number"
                            ))
                        }),
                    }
                };
                let mut r = Map::new();
                r.insert("job".into(), job.into());
                r.insert(
                    "run".into(),
                    c.spaces
                        .replace_all(crate::py::strip(&m[1]), "_")
                        .into_owned()
                        .into(),
                );
                r.insert("tool".into(), "enumerator".into());
                r.insert("states".into(), numk("states")?);
                r.insert("depth".into(), numk("levels")?);
                r.insert("queued".into(), Value::Null);
                r.insert("cap_s".into(), Value::Null);
                r.insert("exit".into(), m[2].parse::<i64>().unwrap_or(0).into());
                r.insert("wall_s".into(), jint(num(&m[3]).unwrap_or(0)));
                r.insert("mode".into(), get("mode").map_or(Value::Null, Value::from));
                r.insert("source".into(), "log".into());
                if !matches!(crate::py::strip(&v), "none" | "" | "[]" | "0") {
                    let vd = match get("violation_depth") {
                        None | Some("-") => None,
                        Some(x) => Some(num(x).ok_or_else(|| {
                            Fail::error(format!(
                                "evidence collect: SUMMARY violation_depth={x} is not a number"
                            ))
                        })?),
                    };
                    r.insert("status".into(), "VIOLATION".into());
                    r.insert("violation".into(), v.into());
                    r.insert("depth".into(), opt_int(vd.map(|d| d + 1)));
                    r.insert("trace_states".into(), opt_int(vd.map(|d| d + 1)));
                } else if complete {
                    r.insert("status".into(), "COMPLETE".into());
                } else {
                    r.insert("status".into(), "PARTIAL".into());
                }
                recs.push(r);
            }
            pending = None;
            buf.clear();
            continue;
        }
        if let Some(m) = c.trace.captures(s) {
            if !recs.is_empty() {
                let name = c
                    .trace_name
                    .captures(s)
                    .and_then(|n| n.get(1).or_else(|| n.get(2)))
                    .map(|n| n.as_str().to_string());
                let mut target = recs.len() - 1;
                if let Some(name) = name {
                    for (k, r) in recs.iter().enumerate().rev() {
                        let run = py_s(r.get("run"));
                        if name.starts_with(&format!("{run}_")) || name == run {
                            target = k;
                            break;
                        }
                    }
                }
                recs[target].insert("trace_states".into(), jint(num(&m[1]).unwrap_or(0)));
                continue;
            }
        }
        if s.starts_with("===") {
            buf.clear();
            continue;
        }
        buf.push(line.to_string());
    }
    // a log with RUNREC lines from a tool: those are its records (parsed ones would repeat them)
    let tools: Vec<Value> = recs
        .iter()
        .filter(|r| r.get("source") == Some(&Value::from("RUNREC")))
        .map(|r| r.get("tool").cloned().unwrap_or(Value::Null))
        .collect();
    recs.retain(|r| {
        !(r.get("source") == Some(&Value::from("log"))
            && tools.contains(&r.get("tool").cloned().unwrap_or(Value::Null)))
    });
    // unique names: #2, #3 in log order
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for r in &mut recs {
        let n = r
            .get("run")
            .map(|v| py_s(Some(v)))
            .ok_or_else(|| Fail::error("evidence collect: a record has no run"))?;
        let k = seen.entry(n.clone()).or_insert(0);
        *k += 1;
        if *k > 1 {
            r.insert("run".into(), format!("{n}#{k}").into());
        }
    }
    for r in &mut recs {
        for k in [
            "job", "run", "tool", "status", "states", "depth", "queued", "cap_s",
        ] {
            r.entry(k).or_insert(Value::Null);
        }
    }
    Ok(recs)
}

/// Today's date (UTC), as `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn write_records(
    ctx: &Ctx,
    out: &Path,
    job: &str,
    recs: Vec<Map<String, Value>>,
    how: &str,
) -> Result<String, Fail> {
    let dir = ctx.path(out);
    std::fs::create_dir_all(&dir).map_err(|e| Fail::io(&dir, e))?;
    let doc = json!({ "job": job, "collected": today(), "from": how, "runs": recs });
    let p = dir.join(format!("{job}.json"));
    std::fs::write(&p, crate::pyjson::dumps(&doc, Some(1)) + "\n").map_err(|e| Fail::io(&p, e))?;
    Ok(out.join(format!("{job}.json")).display().to_string())
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

#[allow(clippy::too_many_arguments)]
fn collect(
    ctx: &Ctx,
    log_cmd: Option<&str>,
    out: &Path,
    log: Option<&Path>,
    job: Option<&str>,
    print: bool,
    jobs: &[String],
) -> Result<Report, Fail> {
    if let Some(log) = log {
        let job = job.ok_or_else(|| Fail::usage("evidence collect: --log needs --job"))?;
        let lp = ctx.path(log);
        let bytes = std::fs::read(&lp).map_err(|e| Fail::io(&lp, e))?;
        let recs = parse_log(&String::from_utf8_lossy(&bytes), job)?;
        let n = recs.len();
        if print {
            let v = Value::Array(recs.into_iter().map(Value::Object).collect());
            let text = crate::pyjson::dumps(&v, Some(1)) + "\n";
            return Ok(Report::new(OK, text, v));
        }
        let name = log
            .file_name()
            .map_or(String::new(), |f| f.to_string_lossy().into_owned());
        let p = write_records(ctx, out, job, recs, &format!("file {name}"))?;
        return Ok(Report::new(
            OK,
            format!("{job}: {n} runs -> {p}\n"),
            json!({ "job": job, "runs": n, "file": p }),
        ));
    }
    let cmd = log_cmd
        .map(str::to_string)
        .or_else(|| ctx.var("RUNREC_LOG_CMD").map(str::to_string));
    let Some(cmd) = cmd.filter(|_| !jobs.is_empty()) else {
        return Err(Fail::usage(
            "evidence collect: give job ids and --log-cmd (or RUNREC_LOG_CMD), or --log FILE --job JOB",
        ));
    };
    let mut text = String::new();
    let mut done = Vec::new();
    let mut report = Report::new(OK, String::new(), Value::Null);
    for j in jobs {
        let c = cmd.replace("{job}", &shell_quote(j));
        let p = std::process::Command::new("sh")
            .arg("-c")
            .arg(&c)
            .current_dir(&ctx.cwd)
            .output()
            .map_err(|e| Fail::error(format!("evidence collect: sh: {e}")))?;
        let stdout = String::from_utf8_lossy(&p.stdout).into_owned();
        if !p.status.success() || crate::py::strip(&stdout).is_empty() {
            let why = String::from_utf8_lossy(&p.stderr);
            let why = crate::py::strip(&why);
            let why: String = if why.is_empty() { "empty" } else { why }
                .chars()
                .take(200)
                .collect();
            report
                .notices
                .push(format!("evidence collect: {j}: log not readable ({why})"));
            report.code = ERROR;
            continue;
        }
        let recs = parse_log(&stdout, j)?;
        let n = recs.len();
        let path = write_records(ctx, out, j, recs, "log command")?;
        text += &format!("{j}: {n} runs -> {path}\n");
        done.push(json!({ "job": j, "runs": n, "file": path }));
    }
    report.text = text;
    report.json = json!({ "written": done });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_as_the_old_pattern() {
        let found = |s: &str| -> Vec<String> {
            counts(s)
                .into_iter()
                .map(|(a, b)| s[a..b].to_string())
                .collect()
        };
        assert_eq!(found("12,345 and 1234 and 123"), vec!["12,345", "1234"]);
        assert_eq!(
            found("1234,567 x1234 1234x 1,234,5 v1.2345 #1234 -1234 1.2345"),
            Vec::<String>::new()
        );
        assert_eq!(found("5,977/12 ×1234 1234×"), vec!["5,977", "1234"]);
        assert_eq!(commas(61233), "61,233");
        assert_eq!(commas(0), "0");
    }
}
