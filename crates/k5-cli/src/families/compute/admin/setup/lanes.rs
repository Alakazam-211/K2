//! `compute lanes [--json]` and `compute lanes set <name> [--from L]
//! [--why TEXT] [key=value …]`, as `cmd_compute_trust lanes` answers them
//! (`cli/src/compute-local-trust.sh`): one GET of `/cli/compute/lanes`
//! (the lanes in words, or the answer as indented JSON), or one POST of
//! `/cli/compute/lanes/set` and the answer as indented JSON. Both go the
//! cell socket first.
//!
//! Each `key=value` is read as JSON, else as text (`isolation` and
//! `profiles` split at commas), as the shell's Python reads it; a key
//! given again keeps its first place.

use super::super::super::lines::py_line;
use super::super::super::pyrt as p;
use super::super::super::writes::{dumps, load, object, stopped, str_value, Cp, Words};
use super::{at, LOCAL_SRC};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{self, Text, Value};
use k2_client::cli_name;

pub const LIST: &str = "/cli/compute/lanes";
pub const SET: &str = "/cli/compute/lanes/set";

pub const SET_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute lanes set <name> [--from ci-standard] [key=value …]"
);

macro_rules! line {
    ($m:literal) => {
        const {
            py_line(
                LOCAL_SRC,
                "            _compute_call GET /cli/compute/lanes \"\"",
                $m,
            )
        }
    };
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    List {
        json: bool,
    },
    Set {
        name: String,
        from: String,
        why: String,
        /// The `key=value` words, in order.
        sets: Vec<String>,
    },
}

/// The words after `lanes`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    if words.first() != Some(&"set") {
        return Words::Ask(Ask::List {
            json: words.contains(&"--json"),
        });
    }
    let name = words.get(1).copied().unwrap_or("");
    if name.is_empty() {
        return Words::Usage(SET_USAGE.to_string());
    }
    let (mut from, mut why, mut sets) = (String::new(), String::new(), vec![]);
    let mut i = 2;
    while i < words.len() {
        let w = words[i];
        match w {
            "--from" | "--why" | "--reason" => {
                let v = words.get(i + 1).copied().unwrap_or("").to_string();
                if w == "--from" {
                    from = v;
                } else {
                    why = v;
                }
                if i + 2 > words.len() {
                    return Words::ShiftFailed(words[i].to_string());
                }
                i += 2;
                continue;
            }
            "--json" => {}
            w if w.contains('=') => sets.push(w.to_string()),
            _ => return Words::Usage(SET_USAGE.to_string()),
        }
        i += 1;
    }
    Words::Ask(Ask::Set {
        name: name.to_string(),
        from,
        why,
        sets,
    })
}

/// The `set` object the shell's Python builds from the `key=value` words.
pub fn sets(words: &[String]) -> Value {
    let mut d: Vec<(Text, Value)> = vec![];
    for kv in words {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let value = match json::parse(v) {
            Ok(x) => x,
            Err(_) if matches!(k, "isolation" | "profiles") => {
                Value::Array(v.split(',').map(str_value).collect())
            }
            Err(_) => str_value(v),
        };
        let key = Text::from(k);
        match d.iter_mut().find(|(x, _)| *x == key) {
            Some(slot) => slot.1 = value,
            None => d.push((key, value)),
        }
    }
    Value::Object(d)
}

/// The POST's body (`_compute_json`).
pub fn set_body(name: &str, from: &str, why: &str, set: Value) -> String {
    let mut pairs = vec![("name", str_value(name))];
    if !from.is_empty() {
        pairs.push(("from", str_value(from)));
    }
    pairs.push(("set", set));
    if !why.is_empty() {
        pairs.push(("reason", str_value(why)));
    }
    dumps(&object(pairs))
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    match a {
        Ask::Set {
            name,
            from,
            why,
            sets: kv,
        } => match cp.call(true, SET, &set_body(name, from, why, sets(kv)), false) {
            Ok(b) => trust_print(&b).print(),
            Err(e) => e,
        },
        Ask::List { json } => match cp.call(false, LIST, "", false) {
            Ok(b) if *json => trust_print(&b).print(),
            Ok(b) => reader(&b).print(),
            Err(e) => e,
        },
    }
}

/// `_compute_trust_print`: the answer as `json.dumps(…, indent=2)`.
pub fn trust_print(body: &[u8]) -> Shown {
    match load(body, 1).and_then(|v| p::dumps(&v, false, Some(2)).map_err(at(1))) {
        Ok(text) => Shown::out(text + "\n"),
        Err(e) => stopped(String::new(), &e),
    }
}

/// The reader on the list.
pub fn reader(body: &[u8]) -> Shown {
    let mut out = String::new();
    match read(body, &mut out) {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

fn read(body: &[u8], out: &mut String) -> Result<(), Raise> {
    let v = load(body, line!("v = json.load"))?;
    let l = line!("if not v[\"lanes\"]");
    if !p::truthy(&p::idx(&v, "lanes").map_err(at(l))?) {
        out.push_str(concat!(
            "no approved lanes: ",
            cli_name!(),
            " compute lanes set ci-standard --from ci-standard\n"
        ));
    }
    let l = line!("for l in v[\"lanes\"]");
    let lanes = p::idx(&v, "lanes")
        .and_then(|x| p::iter(&x))
        .map_err(at(l))?;
    let l = line!("print(\"%s: %s (approved by %s)\"");
    for x in &lanes {
        let args = [
            p::idx(x, "name").map_err(at(l))?,
            p::idx(x, "summary").map_err(at(l))?,
            p::idx(x, "approvedBy").map_err(at(l))?,
        ];
        out.push_str(&p::pct("%s: %s (approved by %s)", &args).map_err(at(l))?);
        out.push('\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sets() {
        let words: Vec<String> = [
            "cpus=4",
            "isolation=vm,net",
            "name=x=y",
            "profiles=[\"a\"]",
            "cpus=8",
            "big=1e400",
        ]
        .iter()
        .map(|w| w.to_string())
        .collect();
        assert_eq!(
            set_body("ci", "ci-standard", "", sets(&words)),
            r#"{"name": "ci", "from": "ci-standard", "set": {"cpus": 8, "isolation": ["vm", "net"], "name": "x=y", "profiles": ["a"], "big": Infinity}}"#
        );
        assert_eq!(parse(&["set"]), Words::Usage(SET_USAGE.into()));
        assert_eq!(parse(&["set", "ci", "--from"]), Words::ShiftFailed("--from".into()));
        assert_eq!(
            parse(&["set", "ci", "nope"]),
            Words::Usage(SET_USAGE.into())
        );
        assert_eq!(
            parse(&["x", "--json"]),
            Words::Ask(Ask::List { json: true })
        );
        assert_eq!(line!("v = json.load"), 3);
        assert_eq!(line!("print(\"%s: %s (approved by %s)\""), 7);
    }

    #[test]
    fn the_reader() {
        assert_eq!(
            reader(br#"{"lanes":[{"name":"ci","summary":"4 cpus","approvedBy":"owner"}]}"#).stdout,
            "ci: 4 cpus (approved by owner)\n"
        );
        assert_eq!(
            trust_print(br#"{"a":[1]}"#).stdout,
            "{\n  \"a\": [\n    1\n  ]\n}\n"
        );
    }
}
