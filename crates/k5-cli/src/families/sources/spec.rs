//! `_sources_spec_json <mode> <flags…>` (`cli/src/sources.sh`): the
//! flags of `propose`, `add` and `try` as a source's JSON, or `edit`'s
//! `set`, as the shell's Python builds it, key order and all.
//!
//! The Python runs as `python3 - "$@"` with its program on stdin, so
//! `--from -` reads an input that has already ended: `json.load` fails
//! with `Expecting value: line 1 column 1 (char 0)`. That is a bash bug,
//! copied here.
//!
//! A number Python would read in digits other than ASCII ones, or one
//! longer than 30 digits (Python's 4300-digit limit is near), goes to the
//! shell CLI ([`Fail::Shell`]).

use crate::families::{py_space, s, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::{json as pyjson, ops};
use k2_client::brand::CLI_NAME as CLI;

/// `full` (propose and add), `set` (edit) or `draft` (try).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Full,
    Set,
    Draft,
}

/// Why no JSON came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// `die(msg)`: `Error: <msg>` on stderr, and exit 2.
    Die(String),
    /// An exception the Python doesn't catch: its traceback, exit 1.
    Raise(Raise),
    /// A number this doesn't read as Python does: the shell answers.
    Shell,
}

type Dict = Vec<(Text, Value)>;

fn die<T>(msg: impl Into<String>) -> Result<T, Fail> {
    Err(Fail::Die(msg.into()))
}

/// `d[k] = v`: a new key goes last, a known one keeps its place.
fn set(d: &mut Dict, k: &str, v: Value) {
    let key = Text::from(k);
    match d.iter_mut().find(|(x, _)| *x == key) {
        Some((_, old)) => *old = v,
        None => d.push((key, v)),
    }
}

fn has(d: &Dict, k: &str) -> bool {
    let key = Text::from(k);
    d.iter().any(|(x, _)| *x == key)
}

fn find<'a>(d: &'a Dict, k: &str) -> Option<&'a Value> {
    let key = Text::from(k);
    d.iter().find(|(x, _)| *x == key).map(|(_, v)| v)
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `str.strip()`.
fn strip(v: &str) -> &str {
    v.trim_matches(py_space)
}

/// A word with a code point that may be a digit to Python but isn't an
/// ASCII one.
fn foreign_digits(v: &str) -> bool {
    v.chars().any(|c| !c.is_ascii() && c.is_numeric())
}

/// ASCII digits as Python's `int` gives them back: no leading zero.
fn int_value(digits: &str) -> Result<Value, Fail> {
    let d = digits.trim_start_matches('0');
    if d.len() > 30 {
        return Err(Fail::Shell);
    }
    Ok(Value::Int(if d.is_empty() { "0" } else { d }.to_string()))
}

/// `int(word)`, base 10: `None` where Python raises `ValueError`.
pub fn py_int(word: &str) -> Result<Option<Value>, Fail> {
    if foreign_digits(word) {
        return Err(Fail::Shell);
    }
    let w = strip(word);
    let (neg, body) = match w.as_bytes().first() {
        Some(b'-') => (true, &w[1..]),
        Some(b'+') => (false, &w[1..]),
        _ => (false, w),
    };
    let b = body.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() || !b[b.len() - 1].is_ascii_digit() {
        return Ok(None);
    }
    let mut digits = String::new();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'0'..=b'9' => digits.push(char::from(c)),
            b'_' if b[i - 1] != b'_' => {}
            _ => return Ok(None),
        }
    }
    let v = int_value(&digits)?;
    Ok(Some(match v {
        Value::Int(d) if neg && d != "0" => Value::Int(format!("-{d}")),
        other => other,
    }))
}

/// `int(text) * mult` for ASCII digits.
fn times(digits: &str, mult: i128) -> Result<Value, Fail> {
    let Value::Int(d) = int_value(digits)? else {
        return Err(Fail::Shell);
    };
    let n: i128 = d.parse().map_err(|_| Fail::Shell)?;
    Ok(Value::Int((n * mult).to_string()))
}

/// `re.fullmatch(r"(\d+)(<units>?)", v.strip())`: the digits and the
/// unit (`""` for none).
fn number_and_unit<'a>(v: &'a str, units: &[char]) -> Result<Option<(&'a str, &'a str)>, Fail> {
    if foreign_digits(v) {
        return Err(Fail::Shell);
    }
    let w = strip(v);
    let (num, unit) = match w.chars().last() {
        Some(c) if units.contains(&c) => w.split_at(w.len() - 1),
        _ => (w, ""),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    Ok(Some((num, unit)))
}

/// `dur(v, flag)`: seconds from `30s`, `5m`, `2h`, `1d` or bare digits.
fn dur(v: &str, flag: &str) -> Result<Value, Fail> {
    let Some((num, unit)) = number_and_unit(v, &['s', 'm', 'h', 'd'])? else {
        return die(format!("{flag} takes a duration like 30s, 5m, 2h, 1d"));
    };
    let mult = match unit {
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => 1,
    };
    times(num, mult)
}

/// `size(v)`: bytes from `512K`, `1M` or bare digits.
fn size(v: &str) -> Result<Value, Fail> {
    let Some((num, unit)) = number_and_unit(v, &['K', 'k', 'M', 'm'])? else {
        return die("--max-bytes takes a size like 512K or 1M");
    };
    let mult = match unit {
        "K" | "k" => 1024,
        "M" | "m" => 1 << 20,
        _ => 1,
    };
    times(num, mult)
}

/// `val(s)`: the word as JSON when it reads as JSON, else as text.
fn val(word: &str) -> Value {
    pyjson::loads(word).unwrap_or_else(|_| s(word))
}

/// `"x".isdigit()` for a word with only ASCII in it.
fn isdigit(v: &str) -> Result<bool, Fail> {
    if !v.is_ascii() && v.chars().any(char::is_numeric) {
        return Err(Fail::Shell);
    }
    Ok(!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

const OPS: [&str; 12] = [
    "eq",
    "ne",
    "in",
    "lt",
    "lte",
    "gt",
    "gte",
    "exists",
    "ageGt",
    "ageLt",
    "versionBehind",
    "versionAtLeast",
];

/// `cond(op, value)`: a rule's or an aggregate's condition.
fn cond(op: &str, value: &str) -> Result<Value, Fail> {
    if !OPS.contains(&op) {
        return die(format!(
            "unknown operator {} (eq ne in lt lte gt gte exists ageGt ageLt versionBehind versionAtLeast)",
            py::repr(&s(op))
        ));
    }
    let one = |v: Value| obj(vec![(op, v)]);
    let field = |f: &str| obj(vec![("field", s(f))]);
    let reference = value.strip_prefix('@');
    match op {
        "in" => return Ok(one(Value::Array(value.split('|').map(val).collect()))),
        "ageGt" | "ageLt" => return Ok(one(dur(value, op)?)),
        "versionBehind" | "versionAtLeast" => {
            let (target, dist) = value.split_once('~').unwrap_or((value, ""));
            let mut v: Dict = match target.strip_prefix('@') {
                Some(f) => vec![(Text::from("field"), s(f))],
                None => vec![(Text::from("value"), s(target))],
            };
            if !dist.is_empty() {
                let (lvl, n) = dist.split_once('=').unwrap_or((dist, ""));
                if !matches!(lvl, "major" | "minor" | "patch") || !isdigit(n)? {
                    return die(format!(
                        "{op} distance is ~major=N, ~minor=N or ~patch=N"
                    ));
                }
                set(&mut v, lvl, int_value(n)?);
            }
            if v.len() == 1 && has(&v, "value") {
                return Ok(one(s(target)));
            }
            return Ok(one(Value::Object(v)));
        }
        "lt" | "lte" | "gt" | "gte" => {
            if let Some(f) = reference {
                return Ok(one(field(f)));
            }
            let n = if value.contains('.') {
                if foreign_digits(value) {
                    return Err(Fail::Shell);
                }
                ops::float(value).map(Value::Float)
            } else {
                py_int(value)?
            };
            return match n {
                Some(n) => Ok(one(n)),
                None => die(format!("{op} needs a number or @field")),
            };
        }
        "exists" => {
            let off = matches!(
                strip(value).to_lowercase().as_str(),
                "false" | "0" | "no"
            );
            return Ok(one(Value::Bool(!off)));
        }
        _ => {}
    }
    Ok(one(match reference {
        Some(f) => field(f),
        None => val(value),
    }))
}

/// `kv(v, flag, sep="=")`: the name (stripped) and the rest.
fn kv<'a>(v: &'a str, flag: &str) -> Result<(&'a str, &'a str), Fail> {
    match v.split_once('=') {
        Some((k, x)) => Ok((strip(k), x)),
        None => die(format!("{flag} takes name=/pointer")),
    }
}

/// `nxt(flag)`: the word after the flag.
fn next<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, Fail> {
    if *i + 1 >= args.len() {
        return die(format!("{flag} needs a value"));
    }
    *i += 1;
    Ok(args[*i].as_str())
}

/// `print(json.dumps(out))` for these words, or why not.
pub fn spec_json(mode: Mode, args: &[String]) -> Result<String, Fail> {
    let mut out: Dict = Vec::new();
    let mut extract: Dict = Vec::new();
    let mut items: Dict = Vec::new();
    let mut targets: Vec<Value> = Vec::new();
    let mut headers: Vec<Value> = Vec::new();
    let mut rules: Vec<Value> = Vec::new();
    let mut aggs: Dict = Vec::new();
    let mut fields: Dict = Vec::new();
    let mut item_fields: Dict = Vec::new();
    let mut lists: Vec<(String, Dict)> = Vec::new();
    let mut counts: Dict = Vec::new();
    let mut trend: Vec<Value> = Vec::new();
    let mut try_targets: Option<Value> = None;
    let mut preset_args: Dict = Vec::new();
    let mut from_file: Option<String> = None;
    let list_of = |lists: &mut Vec<(String, Dict)>, k: &str| -> usize {
        match lists.iter().position(|(n, _)| n == k) {
            Some(i) => i,
            None => {
                lists.push((k.to_string(), Vec::new()));
                lists.len() - 1
            }
        }
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // `nxt(a)`: the flag's value, or `<flag> needs a value`.
        macro_rules! nxt {
            () => {
                next(args, &mut i, a)?
            };
        }
        match a {
            "--json" | "--yes" => {}
            "--from" => from_file = Some(nxt!().to_string()),
            "--name" => set(&mut out, "name", s(nxt!())),
            "--label" => set(&mut out, "label", s(nxt!())),
            "--url" => set(&mut out, "fetch", obj(vec![("url", s(nxt!()))])),
            "--target" => {
                let (k, u) = kv(nxt!(), a)?;
                targets.push(obj(vec![("key", s(k)), ("url", s(u))]));
            }
            "--url-secret" => {
                let name = nxt!();
                if !has(&out, "fetch") {
                    set(&mut out, "fetch", Value::Object(Vec::new()));
                }
                let origin = find(&out, "_origin").cloned().unwrap_or_else(|| s(""));
                let secret = obj(vec![("name", s(name)), ("origin", origin)]);
                set(&mut out, "fetch", obj(vec![("urlSecret", secret)]));
            }
            "--origin" => {
                let o = nxt!();
                set(&mut out, "_origin", s(o));
                if let Some(Value::Object(fetch)) = out
                    .iter_mut()
                    .find(|(k, _)| *k == Text::from("fetch"))
                    .map(|(_, v)| v)
                {
                    if let Some((_, Value::Object(us))) = fetch
                        .iter_mut()
                        .find(|(k, _)| *k == Text::from("urlSecret"))
                    {
                        set(us, "origin", s(o));
                    }
                }
            }
            "--header" => {
                let h = nxt!();
                let Some((n, v)) = h.split_once(':') else {
                    return die("--header takes Name:value");
                };
                headers.push(Value::Array(vec![s(strip(n)), s(strip(v))]));
            }
            "--secret" => set(&mut out, "secret", s(nxt!())),
            "--every" => {
                let v = dur(nxt!(), a)?;
                set(&mut out, "everySecs", v);
            }
            "--timeout" => {
                let v = dur(nxt!(), a)?;
                set(&mut out, "timeoutSecs", v);
            }
            "--max-bytes" => {
                let v = size(nxt!())?;
                set(&mut out, "maxBytes", v);
            }
            "--field" => {
                let (k, p) = kv(nxt!(), a)?;
                set(&mut fields, k, s(p));
            }
            "--items" => set(&mut items, "pointer", s(nxt!())),
            "--item-key" => set(&mut items, "itemKey", s(nxt!())),
            "--item-field" => {
                let (k, p) = kv(nxt!(), a)?;
                set(&mut item_fields, k, s(p));
            }
            "--agg" => {
                let (k, spec) = kv(nxt!(), a)?;
                let parts: Vec<&str> = spec.split(':').collect();
                let agg = match (parts[0], parts.len()) {
                    ("count", 1) => obj(vec![("op", s("count"))]),
                    ("count", 2) => obj(vec![("op", s("count")), ("list", s(parts[1]))]),
                    ("count-where", n) if n >= 4 => obj(vec![
                        ("op", s("countWhere")),
                        ("field", s(parts[1])),
                        ("cond", cond(parts[2], &parts[3..].join(":"))?),
                    ]),
                    (op @ ("sum" | "min" | "max" | "distinct"), 2) => {
                        obj(vec![("op", s(op)), ("field", s(parts[1]))])
                    }
                    _ => return die("--agg takes name=count[:list] | count-where:field:op:value | sum:f | min:f | max:f | distinct:f"),
                };
                set(&mut aggs, k, agg);
            }
            "--rule" => {
                let r = nxt!();
                let (head, rest) = r.split_once(':').unwrap_or((r, ""));
                if rest.is_empty() {
                    return die(
                        "--rule takes 'field:crit:op:value[,warn:op:value][,ok:op:value]'",
                    );
                }
                let mut rule: Dict = vec![(Text::from("field"), s(head))];
                for part in rest.split(',') {
                    let bits: Vec<&str> = part.splitn(3, ':').collect();
                    if bits.len() == 2 && matches!(bits[0], "warnAfter" | "critAfter") {
                        if !isdigit(bits[1])? {
                            return die(format!(
                                "--rule {} takes a number of checks (1-20)",
                                bits[0]
                            ));
                        }
                        set(&mut rule, bits[0], int_value(bits[1])?);
                        continue;
                    }
                    if bits.len() != 3 || !matches!(bits[0], "crit" | "warn" | "ok") {
                        return die("--rule parts are level:op:value with level crit, warn or ok, or warnAfter:N / critAfter:N");
                    }
                    let c = cond(bits[1], bits[2])?;
                    set(&mut rule, bits[0], c);
                }
                rules.push(Value::Object(rule));
            }
            "--list" => {
                let (k, p) = kv(nxt!(), a)?;
                let at = list_of(&mut lists, k);
                set(&mut lists[at].1, "pointer", s(p));
            }
            "--list-key" => {
                let (k, p) = kv(nxt!(), a)?;
                let at = list_of(&mut lists, k);
                set(&mut lists[at].1, "itemKey", s(p));
            }
            "--list-field" => {
                let (k, p) = kv(nxt!(), a)?;
                let Some((ln, fname)) = k.split_once('.') else {
                    return die("--list-field takes list.field=/pointer");
                };
                let at = list_of(&mut lists, ln);
                let l = &mut lists[at].1;
                if !has(l, "itemFields") {
                    set(l, "itemFields", Value::Object(Vec::new()));
                }
                if let Some((_, Value::Object(f))) =
                    l.iter_mut().find(|(k, _)| *k == Text::from("itemFields"))
                {
                    set(f, fname, s(p));
                }
            }
            "--count" => {
                let (k, p) = kv(nxt!(), a)?;
                set(&mut counts, k, s(p));
            }
            "--expect-status" => {
                let v = nxt!();
                let mut status = Vec::new();
                for x in v.split(',').filter(|x| !strip(x).is_empty()) {
                    match py_int(x)? {
                        Some(n) => status.push(n),
                        None => return die("--expect-status takes statuses like 200,401"),
                    }
                }
                set(
                    &mut out,
                    "expect",
                    obj(vec![("status", Value::Array(status))]),
                );
            }
            "--targets" => {
                let v = nxt!();
                if !isdigit(v)? {
                    return die("--targets takes a number");
                }
                try_targets = Some(int_value(v)?);
            }
            "--trend" => trend.push(s(nxt!())),
            "--fail-after" => {
                let v = nxt!();
                match py_int(v)? {
                    Some(n) => set(&mut out, "failAfter", n),
                    None => {
                        return Err(Fail::Raise(Raise::at(
                            199,
                            format!(
                                "ValueError: invalid literal for int() with base 10: {}",
                                py::repr(&s(v))
                            ),
                        )))
                    }
                }
            }
            "--on-target-error" => set(&mut out, "onTargetError", s(nxt!())),
            "--link" => set(&mut out, "link", s(nxt!())),
            "--preset" => set(&mut out, "preset", s(nxt!())),
            "--preset-arg" => {
                let (k, v) = kv(nxt!(), a)?;
                set(&mut preset_args, k, s(v));
            }
            "--viewers" => set(&mut out, "viewers", s(nxt!())),
            "--agent-access" => set(&mut out, "agentAccess", s(nxt!())),
            "--keep" => set(&mut out, "keep", s(nxt!())),
            _ => return die(format!("unknown flag {a} ({CLI} sources --help)")),
        }
        i += 1;
    }
    out.retain(|(k, _)| *k != Text::from("_origin"));
    if let Some(path) = from_file.filter(|p| !p.is_empty()) {
        let base = if path == "-" {
            // The program came on stdin, so stdin has ended.
            pyjson::loads("")
        } else {
            pyjson::read_text_file(&path).and_then(|t| pyjson::loads(&t))
        };
        let mut base = match base {
            Ok(Value::Object(b)) => b,
            Ok(_) => return die("--from: the file holds one JSON object"),
            Err(e) => return die(format!("--from: {}", e.message)),
        };
        for (k, v) in out {
            match base.iter_mut().find(|(x, _)| *x == k) {
                Some((_, old)) => *old = v,
                None => base.push((k, v)),
            }
        }
        out = base;
    }
    if !targets.is_empty() {
        set(&mut out, "fetch", obj(vec![("targets", Value::Array(targets))]));
    }
    if !headers.is_empty() {
        set(&mut out, "headers", Value::Array(headers));
    }
    if !fields.is_empty() {
        set(&mut extract, "fields", Value::Object(fields));
    }
    if !items.is_empty() || !item_fields.is_empty() {
        if !item_fields.is_empty() {
            set(&mut items, "itemFields", Value::Object(item_fields));
        }
        if !has(&items, "pointer") {
            let empty = Value::Object(Vec::new());
            let fetch = find(&out, "fetch").unwrap_or(&empty);
            if !ops::py_in(&s("targets"), fetch, 237).map_err(Fail::Raise)? {
                set(&mut items, "pointer", s(""));
            }
        }
        set(&mut extract, "items", Value::Object(items));
    }
    if !lists.is_empty() {
        for (ln, l) in &lists {
            if !has(l, "pointer") {
                let r = py::repr(&s(ln));
                return die(format!("--list {ln}=/pointer says where the list {r} is"));
            }
        }
        let lists = lists
            .into_iter()
            .map(|(k, v)| (Text::from(k.as_str()), Value::Object(v)))
            .collect();
        set(&mut extract, "lists", Value::Object(lists));
    }
    if !counts.is_empty() {
        set(&mut extract, "counts", Value::Object(counts));
    }
    if !aggs.is_empty() {
        set(&mut extract, "aggregates", Value::Object(aggs));
    }
    if !trend.is_empty() {
        set(&mut extract, "trend", Value::Array(trend));
    }
    if !extract.is_empty() {
        set(&mut out, "extract", Value::Object(extract));
    }
    if !rules.is_empty() {
        set(&mut out, "rules", Value::Array(rules));
    }
    if !preset_args.is_empty() {
        set(&mut out, "presetArgs", Value::Object(preset_args));
    }
    if let Some(n) = try_targets {
        if mode != Mode::Draft {
            return die(format!("--targets is for {CLI} sources try"));
        }
        set(&mut out, "tryTargets", n);
    }
    if mode == Mode::Full {
        for need in ["name", "label"] {
            if !has(&out, need) {
                return die(format!("--{need} is required"));
            }
        }
        if !has(&out, "preset") {
            if !has(&out, "fetch") {
                return die("say what to fetch: --url, --target key=url, or --url-secret NAME --origin https://host");
            }
            if !has(&out, "everySecs") {
                set(&mut out, "everySecs", Value::Int("60".into()));
            }
        }
    }
    json::to_string(&Value::Object(out), &Options::default()).map_err(|_| Fail::Shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    /// What the shell's Python printed for the same words.
    #[test]
    fn the_json_is_python_s() {
        assert_eq!(
            spec_json(
                Mode::Full,
                &w(&[
                    "--name",
                    "w",
                    "--label",
                    "L \u{e9}",
                    "--url",
                    "u",
                    "--every",
                    "007m",
                    "--rule",
                    "a:warn:gt:1.5,crit:gte:@lim,critAfter:3",
                    "--agg",
                    "c=count-where:s:in:a|2",
                    "--origin",
                    "o",
                    "--url-secret",
                    "U",
                    "--max-bytes",
                    "2k",
                ])
            )
            .unwrap(),
            r#"{"name": "w", "label": "L é", "fetch": {"urlSecret": {"name": "U", "origin": "o"}}, "everySecs": 420, "maxBytes": 2048, "extract": {"aggregates": {"c": {"op": "countWhere", "field": "s", "cond": {"in": ["a", 2]}}}}, "rules": [{"field": "a", "warn": {"gt": 1.5}, "crit": {"gte": {"field": "lim"}}, "critAfter": 3}]}"#
        );
        assert_eq!(
            spec_json(
                Mode::Set,
                &w(&[
                    "--list-field",
                    "l.v=/v",
                    "--list",
                    "l=/r",
                    "--item-field",
                    "m=/m",
                    "--target",
                    " k =x",
                ])
            )
            .unwrap(),
            r#"{"fetch": {"targets": [{"key": "k", "url": "x"}]}, "extract": {"items": {"itemFields": {"m": "/m"}}, "lists": {"l": {"itemFields": {"v": "/v"}, "pointer": "/r"}}}}"#
        );
        assert_eq!(
            spec_json(
                Mode::Draft,
                &w(&[
                    "--rule",
                    "v:crit:versionBehind:1.2~minor=3",
                    "--targets",
                    "07",
                    "--rule",
                    "e:ok:exists:NO",
                ])
            )
            .unwrap(),
            r#"{"rules": [{"field": "v", "crit": {"versionBehind": {"value": "1.2", "minor": 3}}}, {"field": "e", "ok": {"exists": false}}], "tryTargets": 7}"#
        );
    }

    #[test]
    fn the_errors_are_python_s() {
        let die = |args: &[&str]| match spec_json(Mode::Full, &w(args)) {
            Err(Fail::Die(m)) => m,
            other => panic!("{args:?}: {other:?}"),
        };
        assert_eq!(die(&[]), "--name is required");
        assert_eq!(die(&["--name"]), "--name needs a value");
        assert_eq!(
            die(&["--rule", "f:crit:zz:1"]),
            "unknown operator 'zz' (eq ne in lt lte gt gte exists ageGt ageLt versionBehind versionAtLeast)"
        );
        assert_eq!(
            die(&["--list-key", "l=/id"]),
            "--list l=/pointer says where the list 'l' is"
        );
        assert_eq!(
            die(&["--from", "-"]),
            "--from: Expecting value: line 1 column 1 (char 0)"
        );
        assert!(matches!(
            spec_json(Mode::Full, &w(&["--fail-after", "x"])),
            Err(Fail::Raise(_))
        ));
        // A digit Python reads that this doesn't: the shell answers.
        assert_eq!(
            spec_json(Mode::Full, &w(&["--every", "\u{0663}m"])),
            Err(Fail::Shell)
        );
    }

    #[test]
    fn int_as_python_reads_it() {
        let i = |v: &str| py_int(v).unwrap().map(|v| py::repr(&v));
        assert_eq!(i(" +0_7 "), Some("7".into()));
        assert_eq!(i("-0"), Some("0".into()));
        assert_eq!(i("-12"), Some("-12".into()));
        for bad in ["", "_1", "1_", "1__2", "1.0", "x", "+"] {
            assert_eq!(i(bad), None, "{bad:?}");
        }
    }
}
