//! The Rust CLI's spec verbs, from the `k2 --schema` entries.
//!
//! An entry in `cli/src/schema/<noun>.entries` may carry `"impl"` (`bash`,
//! `native` or `spec`) and, for a spec verb, `parse`, `call`, `output`,
//! `exits` and `auth`: how the Rust CLI answers it from the entry alone
//! (`crates/k5-cli/src/spec`). This module reads those entries, checks
//! every field against the words the runner answers today, and renders
//! the table the CLI compiles in:
//!
//! - [`TABLE_RS`] from every `"impl": "spec"` entry of the files
//!   `cli/src/ORDER` lists, in that order;
//! - [`FIXTURE_RS`] from [`FIXTURE_ENTRIES`]: one entry the tests run
//!   through the runner against the shell CLI, which no `k2 --schema`
//!   prints and no route reaches (`crates/k5-cli/src/bin/k5-spec-fixture`).
//!
//! Both are [`super::gen::render`] outputs, so `contract-gen` writes them
//! and its `--check` and the freshness test fail on drift. The files are
//! read when contract-gen runs, never compiled in: an entry edit needs no
//! new build of this crate. k2-daemon's route test reads [`rows_in`].
//!
//! `scripts/build-cli.sh` checks the same keys before cargo (its
//! `SPEC_LISTS` line is [`LISTS`], a test keeps them equal) and leaves
//! them out of `k2 --schema`.

use serde_json::{Map, Value as J};
use std::path::Path;

/// The generated tables, relative to the repo root.
pub const TABLE_RS: &str = "crates/k5-cli/src/spec/table.rs";
pub const FIXTURE_ENTRIES: &str = "crates/k5-cli/tests/data/spec_fixture.entries";
pub const FIXTURE_RS: &str = "crates/k5-cli/src/bin/k5-spec-fixture/table.rs";

/// The words each field takes: what the runner answers today. The same
/// as `SPEC_LISTS` in `scripts/build-cli.sh`.
pub const LISTS: &[(&str, &[&str])] = &[
    ("impl", &["bash", "native", "spec"]),
    ("auth", &["session"]),
    ("via", &["compute"]),
    ("method", &["GET"]),
    ("style", &["none", "for"]),
    ("print", &["body", "lines"]),
];

const BASE_KEYS: &[&str] = &["name", "usage", "description", "args", "flags"];
const SPEC_KEYS: &[&str] = &["impl", "parse", "call", "output", "exits", "auth"];

/// How bash reads the words after the verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    None,
    For,
}

/// A positional word and its check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub name: String,
    pub check: Option<Check>,
}

/// `^[class]{min,max}$`, the class as byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub ranges: Vec<(u8, u8)>,
    pub min: usize,
    pub max: usize,
}

/// Where a query value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Arg(String),
    Literal(String),
}

/// One `key=value` of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub key: String,
    pub from: Source,
    /// `raw` (true) or `quote`.
    pub raw: bool,
}

/// A way of printing a 200's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Print {
    Body,
    Lines,
}

/// One spec verb, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecRow {
    /// Where it came from: `cli/src/schema/<noun>.entries`.
    pub file: String,
    pub entry: String,
    pub words: Vec<String>,
    pub style: Style,
    pub usage: String,
    pub args: Vec<Arg>,
    pub json_flag: bool,
    /// `GET`.
    pub method: &'static str,
    pub path: String,
    pub tcp: bool,
    pub query: Vec<Item>,
    pub json: Option<Print>,
    pub text: Print,
    pub exits: Vec<(u16, i32)>,
}

/// Every spec verb of the files `cli/src/ORDER` lists under `root`, in
/// order.
pub fn rows_in(root: &Path) -> Result<Vec<SpecRow>, String> {
    let order = std::fs::read_to_string(root.join("cli/src/ORDER"))
        .map_err(|e| format!("read cli/src/ORDER: {e}"))?;
    let mut rows = Vec::new();
    for name in order
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("schema/") && l.ends_with(".entries"))
    {
        let file = format!("cli/src/{name}");
        let text =
            std::fs::read_to_string(root.join(&file)).map_err(|e| format!("read {file}: {e}"))?;
        rows.extend(rows_of(&file, &text)?);
    }
    Ok(rows)
}

/// The spec verbs of one entries file (`file` names it in errors). Every
/// problem is reported, one line each.
pub fn rows_of(file: &str, text: &str) -> Result<Vec<SpecRow>, String> {
    let body = text
        .strip_suffix("},\n")
        .ok_or_else(|| format!("{file}: the last entry must end with `}},` and a new line"))?;
    let entries: Vec<J> = serde_json::from_str(&format!("[{body}}}]"))
        .map_err(|e| format!("{file}: not a run of JSON entries: {e}"))?;
    let mut rows = Vec::new();
    let mut bad = Vec::new();
    for (n, e) in entries.iter().enumerate() {
        let Some(obj) = e.as_object() else {
            bad.push(format!("{file}: entry {} is not an object", n + 1));
            continue;
        };
        let name = obj.get("name").and_then(J::as_str).unwrap_or("no name");
        match row_of(file, obj) {
            Ok(Some(r)) => rows.push(r),
            Ok(None) => {}
            Err(probs) => bad.extend(
                probs
                    .into_iter()
                    .map(|p| format!("{file}: entry {} ({name}): {p}", n + 1)),
            ),
        }
    }
    if bad.is_empty() {
        Ok(rows)
    } else {
        Err(bad.join("\n"))
    }
}

fn list(name: &str) -> &'static [&'static str] {
    LISTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, l)| *l)
        .expect("a list LISTS has")
}

/// `value` as a word of the list `name`, or the problem.
fn word<'a>(
    where_: &str,
    value: Option<&'a J>,
    name: &str,
    probs: &mut Vec<String>,
) -> Option<&'a str> {
    let l = list(name);
    match value.and_then(J::as_str) {
        Some(w) if l.contains(&w) => Some(w),
        _ => {
            let shown = value.map_or("nothing".to_string(), J::to_string);
            probs.push(format!(
                "{where_} must be one of {}, not {shown}",
                l.join(", ")
            ));
            None
        }
    }
}

/// The keys of `obj` that are not in `allowed`.
fn stray(where_: &str, obj: &Map<String, J>, allowed: &[&str], probs: &mut Vec<String>) {
    for k in obj.keys() {
        if !allowed.contains(&k.as_str()) {
            probs.push(format!("{where_} has no key \"{k}\""));
        }
    }
}

/// An entry's spec row: `None` when it is not a spec verb.
fn row_of(file: &str, e: &Map<String, J>) -> Result<Option<SpecRow>, Vec<String>> {
    let mut probs = Vec::new();
    for k in e.keys() {
        if !BASE_KEYS.contains(&k.as_str()) && !SPEC_KEYS.contains(&k.as_str()) {
            probs.push(format!("\"{k}\" is not an entry key"));
        }
    }
    let Some(imp) = e.get("impl") else {
        let rest: Vec<&str> = SPEC_KEYS[1..]
            .iter()
            .copied()
            .filter(|k| e.contains_key(*k))
            .collect();
        if !rest.is_empty() {
            probs.push(format!(
                "{} only go with \"impl\": \"spec\"",
                rest.join(", ")
            ));
        }
        return if probs.is_empty() {
            Ok(None)
        } else {
            Err(probs)
        };
    };
    let imp = word("\"impl\"", Some(imp), "impl", &mut probs);
    if imp != Some("spec") {
        let rest: Vec<&str> = SPEC_KEYS[1..]
            .iter()
            .copied()
            .filter(|k| e.contains_key(*k))
            .collect();
        if imp.is_some() && !rest.is_empty() {
            probs.push(format!(
                "{} only go with \"impl\": \"spec\"",
                rest.join(", ")
            ));
        }
        return if probs.is_empty() {
            Ok(None)
        } else {
            Err(probs)
        };
    }
    let name = e
        .get("name")
        .and_then(J::as_str)
        .unwrap_or_default()
        .to_string();
    let words: Vec<String> = name.split(' ').map(str::to_string).collect();
    if words.len() < 2
        || words.iter().any(|w| {
            w.is_empty()
                || !w
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    {
        probs.push("a spec verb's name is a noun and a verb, lower-case words".into());
    }
    let obj = |key: &str, probs: &mut Vec<String>| -> Map<String, J> {
        match e.get(key) {
            Some(J::Object(m)) => m.clone(),
            Some(_) => {
                probs.push(format!("\"{key}\" must be an object"));
                Map::new()
            }
            None => {
                probs.push(format!("a spec entry needs \"{key}\""));
                Map::new()
            }
        }
    };
    let (parse, call, output) = (
        obj("parse", &mut probs),
        obj("call", &mut probs),
        obj("output", &mut probs),
    );
    word("\"auth\"", e.get("auth"), "auth", &mut probs);

    // The entry's own words: its args and flags.
    let mut flags = Vec::new();
    for f in e.get("flags").and_then(J::as_array).into_iter().flatten() {
        let n = f.get("name").and_then(J::as_str).unwrap_or_default();
        if n != "--json" || !f.get("arg").is_some_and(J::is_null) {
            probs.push(format!(
                "a spec verb's only flag is --json, a switch, not {}",
                f.get("name")
                    .map_or("a flag with no name".into(), J::to_string)
            ));
        }
        flags.push(n.to_string());
    }
    let json_flag = flags.iter().any(|f| f == "--json");

    stray(
        "\"parse\"",
        &parse,
        &["style", "usage", "check"],
        &mut probs,
    );
    let style = match word(
        "\"parse\" \"style\"",
        parse.get("style"),
        "style",
        &mut probs,
    ) {
        Some("for") => Style::For,
        _ => Style::None,
    };
    let usage = parse
        .get("usage")
        .and_then(J::as_str)
        .unwrap_or_default()
        .to_string();
    if usage.is_empty() || usage.contains('\n') {
        probs.push("\"parse\" \"usage\" is the one line bash prints".into());
    }
    let checks = match parse.get("check") {
        None => Map::new(),
        Some(J::Object(m)) => m.clone(),
        Some(_) => {
            probs.push("\"parse\" \"check\" maps an arg to its regex".into());
            Map::new()
        }
    };
    let mut args = Vec::new();
    for a in e.get("args").and_then(J::as_array).into_iter().flatten() {
        let n = a
            .get("name")
            .and_then(J::as_str)
            .unwrap_or_default()
            .to_string();
        let required = a.get("required").and_then(J::as_bool) == Some(true);
        let check = match checks.get(&n) {
            None => None,
            Some(re) => match re.as_str().and_then(parse_check) {
                Some(c) => Some(c),
                None => {
                    probs.push(format!("\"parse\" \"check\" for {n} must be ^[class]{{min,max}}$ (a-z, A-Z, 0-9 and _ - . in the class), not {re}"));
                    None
                }
            },
        };
        if style == Style::None && required {
            probs.push(format!(
                "the style none reads no words, but {n} is required"
            ));
        }
        args.push(ArgIn {
            name: n,
            check,
            required,
        });
    }
    for k in checks.keys() {
        if !args.iter().any(|a| a.name == *k) {
            probs.push(format!(
                "\"parse\" \"check\" names {k}, which is not an arg"
            ));
        }
    }
    if style == Style::For && !(args.len() == 1 && args[0].required && args[0].check.is_some()) {
        probs.push("the style for reads one required arg, with a check".into());
    }

    stray(
        "\"call\"",
        &call,
        &["via", "method", "path", "channel", "query"],
        &mut probs,
    );
    word("\"call\" \"via\"", call.get("via"), "via", &mut probs);
    word(
        "\"call\" \"method\"",
        call.get("method"),
        "method",
        &mut probs,
    );
    let path = call
        .get("path")
        .and_then(J::as_str)
        .unwrap_or_default()
        .to_string();
    if !path.starts_with("/cli/")
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/_-".contains(&b))
    {
        probs.push(format!(
            "\"call\" \"path\" is a /cli/ route (a-z, 0-9, / _ -), not {}",
            call.get("path").map_or("nothing".into(), J::to_string)
        ));
    }
    let tcp = match call.get("channel") {
        None => false,
        Some(c) if c.as_str() == Some("tcp") => true,
        Some(c) => {
            probs.push(format!(
                "\"call\" \"channel\" is \"tcp\" or left out, not {c}"
            ));
            false
        }
    };
    let mut query = Vec::new();
    for (i, q) in call
        .get("query")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(q) = q.as_object() else {
            probs.push(format!(
                "\"call\" \"query\" item {} is not an object",
                i + 1
            ));
            continue;
        };
        stray("a \"query\" item", q, &["key", "from", "enc"], &mut probs);
        let key = q
            .get("key")
            .and_then(J::as_str)
            .unwrap_or_default()
            .to_string();
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            probs.push(format!("a query key is a-z, 0-9 and _, not {key:?}"));
        }
        let from_text = q.get("from").and_then(J::as_str).unwrap_or_default();
        let from = if let Some(arg) = from_text
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
        {
            if !args.iter().any(|a| a.name == arg) {
                probs.push(format!(
                    "the query's {key} is from <{arg}>, which is not an arg"
                ));
            }
            Source::Arg(arg.to_string())
        } else if let Some(lit) = from_text.strip_prefix('=') {
            if lit.is_empty()
                || !lit
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            {
                probs.push(format!(
                    "the query's {key} literal is A-Z, a-z, 0-9 and _ . -, not {lit:?}"
                ));
            }
            Source::Literal(lit.to_string())
        } else {
            probs.push(format!(
                "the query's {key} is from <arg> or =literal, not {from_text:?}"
            ));
            Source::Literal(String::new())
        };
        let raw = match q.get("enc").and_then(J::as_str) {
            Some("raw") => {
                if let Source::Arg(a) = &from {
                    if args.iter().any(|x| x.name == *a && x.check.is_none()) {
                        probs.push(format!(
                            "the query's {key} goes raw, so <{a}> needs a check"
                        ));
                    }
                }
                true
            }
            Some("quote") => false,
            other => {
                probs.push(format!(
                    "the query's {key} enc is raw or quote, not {}",
                    other.unwrap_or("nothing")
                ));
                false
            }
        };
        query.push(Item { key, from, raw });
    }

    stray("\"output\"", &output, &["json", "text"], &mut probs);
    let print = |w: Option<&str>| {
        if w == Some("lines") {
            Print::Lines
        } else {
            Print::Body
        }
    };
    let text = print(word(
        "\"output\" \"text\"",
        output.get("text"),
        "print",
        &mut probs,
    ));
    let json = output
        .get("json")
        .map(|j| print(word("\"output\" \"json\"", Some(j), "print", &mut probs)));
    if json.is_some() != json_flag {
        probs.push("\"output\" has \"json\" when the verb takes --json, and only then".into());
    }

    let mut exits = Vec::new();
    match e.get("exits") {
        None => {}
        Some(J::Object(m)) => {
            for (status, code) in m {
                // `_compute_fail` lets a verb change only the exit of a 404
                // (`_CP_404_EXIT`).
                let c: Option<i32> = code
                    .as_i64()
                    .and_then(|c| i32::try_from(c).ok())
                    .filter(|c| (0..=255).contains(c));
                match (status.as_str(), c) {
                    ("404", Some(c)) => exits.push((404, c)),
                    _ => probs.push(format!(
                        "\"exits\" maps 404 to an exit (0 to 255), not {status:?}: {code}"
                    )),
                }
            }
        }
        Some(_) => probs.push("\"exits\" maps a status to an exit".into()),
    }

    if !probs.is_empty() {
        return Err(probs);
    }
    let args = args
        .into_iter()
        .map(|a| Arg {
            name: a.name,
            check: a.check,
        })
        .collect();
    Ok(Some(SpecRow {
        file: file.to_string(),
        entry: name,
        words,
        style,
        usage,
        args,
        json_flag,
        method: "GET",
        path,
        tcp,
        query,
        json,
        text,
        exits,
    }))
}

/// An arg while the entry is read: its `required` too.
struct ArgIn {
    name: String,
    check: Option<Check>,
    required: bool,
}

/// `^[class]{min,max}$` with ranges `a-z`, `A-Z`, `0-9` and the bytes
/// `_`, `-` (last, as bash reads it there) and `.`.
pub fn parse_check(re: &str) -> Option<Check> {
    let inner = re.strip_prefix("^[")?.strip_suffix("}$")?;
    let (class, counts) = inner.split_once("]{")?;
    let (min, max) = counts.split_once(',')?;
    let (min, max): (usize, usize) = (min.parse().ok()?, max.parse().ok()?);
    if min > max || max == 0 {
        return None;
    }
    let b = class.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if i + 2 < b.len() && b[i + 1] == b'-' {
            let (lo, hi) = (b[i], b[i + 2]);
            let whole = matches!((lo, hi), (b'a', b'z') | (b'A', b'Z') | (b'0', b'9'));
            if !whole {
                return None;
            }
            ranges.push((lo, hi));
            i += 3;
        } else if b[i] == b'-' && i + 1 != b.len() {
            return None;
        } else if b"_-.".contains(&b[i]) {
            ranges.push((b[i], b[i]));
            i += 1;
        } else {
            return None;
        }
    }
    (!ranges.is_empty()).then_some(Check { ranges, min, max })
}

/// A usage line as a Rust expression: a plain literal, or, where the line
/// types the command's name (the brand's, as a word before a space or at the
/// end), a `concat!` that takes the name from the brand
/// (`k2_client::cli_name!()`), so the table types no command name
/// (`scripts/brand-gate.sh`).
pub fn branded(s: &str) -> String {
    const NAME: &str = crate::brand::CLI_NAME;
    let mut parts: Vec<String> = Vec::new();
    let mut rest = s;
    let mut from = 0;
    while let Some(at) = rest[from..].find(NAME).map(|i| i + from) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + NAME.len()..].chars().next();
        let word = before.is_none_or(|c| !(c.is_alphanumeric() || "_-/.".contains(c)))
            && after.is_none_or(|c| c == ' ');
        if !word {
            from = at + NAME.len();
            continue;
        }
        if at > 0 {
            parts.push(format!("{:?}", &rest[..at]));
        }
        parts.push("k2_client::cli_name!()".to_string());
        rest = &rest[at + NAME.len()..];
        from = 0;
    }
    if parts.is_empty() {
        return format!("{s:?}");
    }
    if !rest.is_empty() {
        parts.push(format!("{rest:?}"));
    }
    format!("concat!({})", parts.join(", "))
}

/// The table's Rust source. `types`: the path of the spec types
/// (`super` in the CLI, `k5_cli::spec` in the fixture binary); `source`:
/// what the header says the rows came from.
pub fn render_table(rows: &[SpecRow], types: &str, source: &str) -> String {
    let q = |s: &str| format!("{s:?}");
    let mut s = format!(
        "//! GENERATED by contract-gen from {source}.\n\
         //! Do not edit: change the entry, then run\n\
         //! `cargo run -p k2-core --bin contract-gen`. The freshness tests fail\n\
         //! on drift (`crates/k2-core/src/contract/cli_spec.rs`).\n\n"
    );
    if rows.is_empty() {
        s.push_str(&format!("use {types}::SpecVerb;\n\n/// Every spec verb, in `cli/src/ORDER`'s order.\npub static SPEC: &[SpecVerb] = &[];\n"));
        return s;
    }
    s.push_str(&format!(
        "#[rustfmt::skip]\n#[allow(unused_imports)]\nuse {types}::{{Arg, Call, Check, Cred, Enc, Item, Method, Output, Parse, Print, Source, SpecVerb, Style, Via}};\n\n\
         /// Every spec verb, in `cli/src/ORDER`'s order.\n#[rustfmt::skip]\npub static SPEC: &[SpecVerb] = &[\n"
    ));
    let print = |p: Print| match p {
        Print::Body => "Print::Body",
        Print::Lines => "Print::Lines",
    };
    for r in rows {
        let words: Vec<String> = r.words.iter().map(|w| q(w.as_str())).collect();
        let args: Vec<String> = r
            .args
            .iter()
            .map(|a| {
                let check = match &a.check {
                    None => "None".to_string(),
                    Some(c) => {
                        let ranges: Vec<String> = c
                            .ranges
                            .iter()
                            .map(|(lo, hi)| format!("(b'{}', b'{}')", *lo as char, *hi as char))
                            .collect();
                        format!(
                            "Some(Check {{ ranges: &[{}], min: {}, max: {} }})",
                            ranges.join(", "),
                            c.min,
                            c.max
                        )
                    }
                };
                format!("Arg {{ name: {}, check: {check} }}", q(&a.name))
            })
            .collect();
        let query: Vec<String> = r
            .query
            .iter()
            .map(|i| {
                let from = match &i.from {
                    Source::Arg(a) => format!("Source::Arg({})", q(a)),
                    Source::Literal(l) => format!("Source::Literal({})", q(l)),
                };
                format!(
                    "Item {{ key: {}, from: {from}, enc: Enc::{} }}",
                    q(&i.key),
                    if i.raw { "Raw" } else { "Quote" }
                )
            })
            .collect();
        let exits: Vec<String> = r.exits.iter().map(|(s, c)| format!("({s}, {c})")).collect();
        s.push_str(&format!(
            "    SpecVerb {{\n        entry: {entry},\n        words: &[{words}],\n        parse: Parse {{\n            style: Style::{style},\n            usage: {usage},\n            args: &[{args}],\n            json_flag: {json_flag},\n        }},\n        call: Call {{\n            via: Via::Compute,\n            method: Method::Get,\n            path: {path},\n            tcp: {tcp},\n            query: &[{query}],\n        }},\n        output: Output {{ json: {json}, text: {text} }},\n        exits: &[{exits}],\n        auth: Cred::Session,\n    }},\n",
            entry = q(&r.entry),
            words = words.join(", "),
            style = match r.style {
                Style::None => "None",
                Style::For => "For",
            },
            usage = branded(&r.usage),
            args = args.join(", "),
            json_flag = r.json_flag,
            path = q(&r.path),
            tcp = r.tcp,
            query = query.join(", "),
            json = r.json.map_or("None".to_string(), |p| format!("Some({})", print(p))),
            text = print(r.text),
            exits = exits.join(", "),
        ));
    }
    s.push_str("];\n");
    s
}

/// The two tables, rendered from the files under `root`.
pub fn outputs(root: &Path) -> Result<Vec<super::gen::Output>, String> {
    let rows = rows_in(root)?;
    let fixture_text = std::fs::read_to_string(root.join(FIXTURE_ENTRIES))
        .map_err(|e| format!("read {FIXTURE_ENTRIES}: {e}"))?;
    let fixture = rows_of(FIXTURE_ENTRIES, &fixture_text)?;
    if fixture.is_empty() {
        return Err(format!("{FIXTURE_ENTRIES} has no spec entry"));
    }
    Ok(vec![
        super::gen::Output {
            path: TABLE_RS,
            text: render_table(
                &rows,
                "super",
                "the `\"impl\": \"spec\"` entries of cli/src/schema",
            ),
        },
        super::gen::Output {
            path: FIXTURE_RS,
            text: render_table(&fixture, "k5_cli::spec", FIXTURE_ENTRIES),
        },
    ])
}

#[cfg(test)]
mod tests;
