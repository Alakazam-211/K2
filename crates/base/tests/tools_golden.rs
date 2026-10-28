//! Golden parity corpus for the text tools (K-PAR-7): `tests/golden/tools.golden`.
//!
//! Every case runs through the pure tool API and must match exactly: stdout (or the new
//! text for edit verbs), patch's report, the exit code, and for refusals the error kind.
//! All mismatches are reported together, and the test fails loudly. The cases marked
//! `gnu: same` are also checked against the real GNU tools on Linux by
//! `scripts/gnu-parity.py` (not run here: this Mac has BSD tools).

use k2_mpc_base::codec::text_file;
use k2_mpc_base::tools::{Kind, Tool, ToolDoc, ToolError};

#[derive(Debug, Default)]
struct Case {
    id: String,
    line: usize,
    argv: Vec<String>,
    text: Option<String>,
    file: Option<String>,
    stdin: String,
    name: Option<String>,
    out: Option<String>,
    say: Option<String>,
    exit: Option<u8>,
    err: Option<String>,
    gnu: Option<String>,
}

/// Quoted strings on one line, with `\n \t \r \\ \" \u{HEX}` escapes.
fn strings(s: &str, line: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        match it.next() {
            None => return out,
            Some('"') => {}
            Some(c) => panic!("line {line}: expected a quoted string, found {c:?}"),
        }
        let mut v = String::new();
        loop {
            match it.next() {
                None => panic!("line {line}: unterminated string"),
                Some('"') => break,
                Some('\\') => match it.next() {
                    Some('n') => v.push('\n'),
                    Some('t') => v.push('\t'),
                    Some('r') => v.push('\r'),
                    Some('\\') => v.push('\\'),
                    Some('"') => v.push('"'),
                    Some('u') => {
                        assert_eq!(it.next(), Some('{'), "line {line}: bad \\u");
                        let hex: String = it.by_ref().take_while(|&c| c != '}').collect();
                        let n = u32::from_str_radix(&hex, 16).expect("hex");
                        v.push(char::from_u32(n).expect("char"));
                    }
                    e => panic!("line {line}: bad escape {e:?}"),
                },
                Some(c) => v.push(c),
            }
        }
        out.push(v);
    }
}

fn one(s: &str, line: usize) -> String {
    let v = strings(s, line);
    assert_eq!(v.len(), 1, "line {line}: expected one string");
    v.into_iter().next().unwrap()
}

fn corpus() -> Vec<Case> {
    let src = include_str!("golden/tools.golden");
    let mut cases: Vec<Case> = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        if raw.trim().is_empty() || raw.starts_with('#') {
            continue;
        }
        if let Some(id) = raw.strip_prefix("=== ") {
            assert!(
                !cases.iter().any(|c| c.id == id),
                "line {line}: duplicate case {id}"
            );
            cases.push(Case {
                id: id.trim().to_owned(),
                line,
                ..Case::default()
            });
            continue;
        }
        let c = cases
            .last_mut()
            .unwrap_or_else(|| panic!("line {line}: field before a case"));
        let (key, val) = raw
            .split_once(':')
            .unwrap_or_else(|| panic!("line {line}: expected `key: value`"));
        match key {
            "argv" => c.argv = strings(val, line),
            "text" => c.text = Some(one(val, line)),
            "file" => c.file = Some(one(val, line)),
            "stdin" => c.stdin = one(val, line),
            "name" => c.name = Some(one(val, line)),
            "out" => c.out = Some(one(val, line)),
            "say" => c.say = Some(one(val, line)),
            "exit" => c.exit = Some(val.trim().parse().expect("exit code")),
            "err" => c.err = Some(val.trim().to_owned()),
            "gnu" => c.gnu = Some(val.trim().to_owned()),
            k => panic!("line {line}: unknown field {k}"),
        }
    }
    cases
}

fn err_kind(e: &ToolError) -> &'static str {
    match e {
        ToolError::Unsupported { .. } => "unsupported",
        ToolError::Refused { .. } => "refused",
        ToolError::Usage(_) => "usage",
        ToolError::Limit(_) => "limit",
        _ => "other",
    }
}

fn show(b: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(b))
}

/// Run one case; `Err` describes the mismatch.
fn check(c: &Case) -> Result<(), String> {
    let exit = c.exit.ok_or("no exit field")?;
    let name = c.name.as_deref().unwrap_or("notes.md");
    let (text, bytes) = match (&c.text, &c.file) {
        (Some(t), None) => (t.clone(), t.as_bytes().to_vec()),
        (None, Some(f)) => {
            let (t, _) = text_file::decode(f.as_bytes(), 1 << 20)
                .map_err(|e| format!("file doesn't decode: {e}"))?;
            (t, f.as_bytes().to_vec())
        }
        _ => return Err("exactly one of text/file".into()),
    };
    let failed = |e: ToolError| -> Result<(), String> {
        let want = c.err.as_deref().unwrap_or("(none)");
        if exit == 2 && err_kind(&e) == want && e.exit() == 2 {
            Ok(())
        } else {
            Err(format!(
                "got error {e:?} (exit {}), want exit {exit} err {want}",
                e.exit()
            ))
        }
    };
    let tool = match Tool::parse(&c.argv) {
        Ok(t) => t,
        Err(e) => return failed(e),
    };
    match tool.kind() {
        Kind::Read => {
            let doc = ToolDoc {
                name,
                text: &text,
                bytes: &bytes,
            };
            let o = match tool.read(&doc, c.stdin.as_bytes()) {
                Ok(o) => o,
                Err(e) => return failed(e),
            };
            let want = c.out.as_deref().ok_or("read case without out")?;
            if o.stdout != want.as_bytes() || o.exit != exit {
                return Err(format!(
                    "got {} exit {}, want {:?} exit {exit}",
                    show(&o.stdout),
                    o.exit,
                    want
                ));
            }
        }
        Kind::Edit => {
            let o = match tool.edit(name, &text, c.stdin.as_bytes()) {
                Ok(o) => o,
                Err(e) => return failed(e),
            };
            let mut bad = Vec::new();
            if o.text.as_deref() != c.out.as_deref() {
                bad.push(format!("text {:?}, want {:?}", o.text, c.out));
            }
            if let Some(say) = &c.say {
                if o.stdout != say.as_bytes() {
                    bad.push(format!("say {}, want {say:?}", show(&o.stdout)));
                }
            }
            if o.exit != exit {
                bad.push(format!("exit {}, want {exit}", o.exit));
            }
            if !bad.is_empty() {
                return Err(bad.join("; "));
            }
        }
    }
    if exit == 2 {
        return Err("expected a refusal, but the tool ran".into());
    }
    Ok(())
}

#[test]
fn golden_corpus() {
    let cases = corpus();
    let mut failures = Vec::new();
    for c in &cases {
        if let Err(e) = check(c) {
            failures.push(format!("  {} (line {}): {e}", c.id, c.line));
        }
    }
    let gnu = cases
        .iter()
        .filter(|c| c.gnu.as_deref().unwrap_or("same") == "same")
        .count();
    eprintln!(
        "golden: {} cases ({gnu} GNU-comparable, {} K2-specific)",
        cases.len(),
        cases.len() - gnu
    );
    assert!(
        failures.is_empty(),
        "{} of {} golden cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
    assert!(cases.len() >= 200, "corpus has only {} cases", cases.len());
    for c in &cases {
        assert!(
            matches!(c.gnu.as_deref(), None | Some("same") | Some("k2")),
            "{}: gnu must be same or k2",
            c.id
        );
    }
}
