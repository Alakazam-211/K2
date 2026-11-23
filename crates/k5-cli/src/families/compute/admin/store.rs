//! `k5 compute prune [--older-than D] [--dry-run] [--json]`, `pin|unpin
//! <job> [--json]` and `retention [--json]` / `retention set [--max-age
//! D|default] [--cap SIZE|auto] [--json]`, as `cmd_compute_store` answers
//! them (`cli/src/compute-command.sh`): the words, one `_compute_call`
//! over TCP, then the daemon's `text` lines (`_compute_text`), or its
//! whole answer with `--json`.

use super::super::reads::text_lines;
use super::super::writes::{dumps, object, print_body, str_value, Cp, Words};
use crate::exit::Exit;
use crate::out::json::Value;
use k2_client::cli_name;
use std::time::Duration;

pub const PRUNE: &str = "/cli/compute/prune";
pub const RETENTION: &str = "/cli/compute/retention";
pub const RETENTION_SET: &str = "/cli/compute/retention/set";

/// `K2_CP_MAX_TIME=600`: a big store takes a while to measure and move
/// aside.
pub const PRUNE_MAX_TIME: Duration = Duration::from_secs(600);

/// `$pusage`.
pub const PRUNE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute prune [--older-than DURATION] [--dry-run] [--json]"
);

/// `$rusage`.
pub const RETENTION_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute retention [--json]  |  ",
    cli_name!(),
    " compute retention set [--max-age DURATION|default] [--cap SIZE|auto] [--json]"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Prune {
        older: String,
        dry: bool,
        json: bool,
    },
    /// `pin` (true) or `unpin`.
    Pin {
        pin: bool,
        job: String,
        json: bool,
    },
    Retention {
        json: bool,
    },
    RetentionSet {
        max_age: String,
        cap: String,
        json: bool,
    },
}

/// The verbs this module answers.
pub fn answers(verb: &str) -> bool {
    matches!(verb, "prune" | "pin" | "unpin" | "retention")
}

/// `$pusage` with what is wrong after it.
fn prune_usage(why: &str) -> Words<Ask> {
    Words::Usage(format!("{PRUNE_USAGE} ({why})"))
}

/// The words after `verb` (`prune`, `pin`, `unpin` or `retention`).
pub fn parse(verb: &str, words: &[&str]) -> Words<Ask> {
    match verb {
        "prune" => prune(words),
        "pin" | "unpin" => pin(verb, words),
        _ => retention(words),
    }
}

fn prune(words: &[&str]) -> Words<Ask> {
    let (mut older, mut dry, mut json) = (String::new(), false, false);
    let mut i = 0;
    while let Some(&w) = words.get(i) {
        match w {
            "--older-than" => {
                let Some(&v) = words.get(i + 1) else {
                    return Words::ShiftFailed(w.to_string());
                };
                // `[[ "$older" =~ ^[0-9dhms]+$ ]]`.
                if v.is_empty() || !v.bytes().all(|b| b"0123456789dhms".contains(&b)) {
                    return prune_usage("--older-than takes 3d, 72h, 90m or 1d12h");
                }
                older = v.to_string();
                i += 2;
                continue;
            }
            "--dry-run" | "-n" => dry = true,
            "--json" => json = true,
            other => return prune_usage(&format!("unknown '{other}'")),
        }
        i += 1;
    }
    Words::Ask(Ask::Prune { older, dry, json })
}

fn pin(verb: &str, words: &[&str]) -> Words<Ask> {
    let usage = |why: &str| {
        Words::Usage(format!(
            "usage: {} compute {verb} <job> [--json]{why}",
            cli_name!()
        ))
    };
    let (mut job, mut json) = (String::new(), false);
    for &w in words {
        match w {
            "--json" => json = true,
            w if w.starts_with('-') => return usage(&format!(" (unknown '{w}')")),
            w => {
                if !job.is_empty() {
                    return usage(" (one job)");
                }
                job = w.to_string();
            }
        }
    }
    if job.is_empty() {
        return usage("");
    }
    Words::Ask(Ask::Pin {
        pin: verb == "pin",
        job,
        json,
    })
}

fn retention(words: &[&str]) -> Words<Ask> {
    let set = words.first() == Some(&"set");
    let words = if set { &words[1..] } else { words };
    let (mut max_age, mut cap, mut json) = (String::new(), String::new(), false);
    let mut i = 0;
    while let Some(&w) = words.get(i) {
        match w {
            "--max-age" | "--cap" => {
                if !set {
                    return Words::Usage(RETENTION_USAGE.to_string());
                }
                let Some(&v) = words.get(i + 1) else {
                    return Words::ShiftFailed(w.to_string());
                };
                if w == "--max-age" {
                    max_age = v.to_string();
                } else {
                    cap = v.to_string();
                }
                i += 2;
                continue;
            }
            "--json" => json = true,
            other => return Words::Usage(format!("{RETENTION_USAGE} (unknown '{other}')")),
        }
        i += 1;
    }
    if !set {
        return Words::Ask(Ask::Retention { json });
    }
    if max_age.is_empty() && cap.is_empty() {
        return Words::Usage(format!(
            "{RETENTION_USAGE} (set needs --max-age and/or --cap)"
        ));
    }
    Words::Ask(Ask::RetentionSet { max_age, cap, json })
}

/// `_compute_json K2_CJ_<key>=<value> ...`: the pairs whose value is not
/// empty, in their order.
fn body(pairs: Vec<(&str, Value)>) -> String {
    dumps(&object(
        pairs
            .into_iter()
            .filter(|(_, v)| !matches!(v, Value::Str(t) if t.as_str() == Some("")))
            .collect(),
    ))
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let (answer, json) = match a {
        Ask::Prune { older, dry, json } => {
            let payload = body(vec![
                ("olderThan", str_value(older)),
                ("dryRun", Value::Bool(*dry)),
            ]);
            let before = cp.client.compute_post_max_time;
            cp.client.compute_post_max_time = Some(PRUNE_MAX_TIME);
            let r = cp.call(true, PRUNE, &payload, true);
            cp.client.compute_post_max_time = before;
            (r, *json)
        }
        Ask::Pin { pin, job, json } => {
            let path = if *pin {
                "/cli/compute/pin"
            } else {
                "/cli/compute/unpin"
            };
            let payload = body(vec![("job", str_value(job))]);
            (cp.call(true, path, &payload, true), *json)
        }
        Ask::Retention { json } => (cp.call(false, RETENTION, "", true), *json),
        Ask::RetentionSet { max_age, cap, json } => {
            let payload = body(vec![("maxAge", str_value(max_age)), ("cap", str_value(cap))]);
            (cp.call(true, RETENTION_SET, &payload, true), *json)
        }
    };
    let answer = match answer {
        Ok(b) => b,
        Err(e) => return e,
    };
    if json {
        return print_body(&answer);
    }
    text_lines(&answer).print()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(w: Words<Ask>) -> Ask {
        match w {
            Words::Ask(a) => a,
            other => panic!("expected an ask, got {other:?}"),
        }
    }

    fn usage(w: Words<Ask>) -> String {
        match w {
            Words::Usage(line) => line,
            other => panic!("expected a usage line, got {other:?}"),
        }
    }

    #[test]
    fn prune_words() {
        assert_eq!(
            ask(parse("prune", &["--dry-run"])),
            Ask::Prune {
                older: String::new(),
                dry: true,
                json: false
            }
        );
        // The last --older-than counts; -n is --dry-run.
        assert_eq!(
            ask(parse(
                "prune",
                &["--older-than", "1d", "-n", "--older-than", "0", "--json"]
            )),
            Ask::Prune {
                older: "0".into(),
                dry: true,
                json: true
            }
        );
        for bad in ["", "soon", "3 d", "3D", "-1d"] {
            assert_eq!(
                usage(parse("prune", &["--older-than", bad])),
                format!("{PRUNE_USAGE} (--older-than takes 3d, 72h, 90m or 1d12h)"),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse("prune", &["--older-than"]),
            Words::ShiftFailed("--older-than".into())
        );
        assert_eq!(
            usage(parse("prune", &["--dry-run", "x"])),
            format!("{PRUNE_USAGE} (unknown 'x')")
        );
    }

    #[test]
    fn pin_words() {
        assert_eq!(
            ask(parse("unpin", &["--json", "abcd1234"])),
            Ask::Pin {
                pin: false,
                job: "abcd1234".into(),
                json: true
            }
        );
        // An empty word is no job yet: the next one is.
        assert_eq!(
            ask(parse("pin", &["", "j1"])),
            Ask::Pin {
                pin: true,
                job: "j1".into(),
                json: false
            }
        );
        let u = format!("usage: {} compute pin <job> [--json]", cli_name!());
        assert_eq!(usage(parse("pin", &[])), u);
        assert_eq!(usage(parse("pin", &["a", "b"])), format!("{u} (one job)"));
        assert_eq!(usage(parse("pin", &["-"])), format!("{u} (unknown '-')"));
        assert_eq!(
            usage(parse("unpin", &["--bogus", "x"])),
            format!(
                "usage: {} compute unpin <job> [--json] (unknown '--bogus')",
                cli_name!()
            )
        );
    }

    #[test]
    fn retention_words() {
        assert_eq!(
            ask(parse("retention", &["--json"])),
            Ask::Retention { json: true }
        );
        assert_eq!(
            ask(parse("retention", &["set", "--cap", "auto", "--max-age", "14d"])),
            Ask::RetentionSet {
                max_age: "14d".into(),
                cap: "auto".into(),
                json: false
            }
        );
        assert_eq!(
            usage(parse("retention", &["--max-age", "3d"])),
            RETENTION_USAGE
        );
        assert_eq!(
            usage(parse("retention", &["set"])),
            format!("{RETENTION_USAGE} (set needs --max-age and/or --cap)")
        );
        // An empty value is no value to `[ -n "$max_age$cap" ]`.
        assert_eq!(
            usage(parse("retention", &["set", "--cap", ""])),
            format!("{RETENTION_USAGE} (set needs --max-age and/or --cap)")
        );
        assert_eq!(
            parse("retention", &["set", "--cap"]),
            Words::ShiftFailed("--cap".into())
        );
        assert_eq!(
            usage(parse("retention", &["--json", "set"])),
            format!("{RETENTION_USAGE} (unknown 'set')")
        );
    }

    #[test]
    fn bodies() {
        assert_eq!(
            body(vec![
                ("olderThan", str_value("")),
                ("dryRun", Value::Bool(true))
            ]),
            r#"{"dryRun": true}"#
        );
        assert_eq!(
            body(vec![
                ("olderThan", str_value("3d")),
                ("dryRun", Value::Bool(false))
            ]),
            r#"{"olderThan": "3d", "dryRun": false}"#
        );
        assert_eq!(
            body(vec![("maxAge", str_value("")), ("cap", str_value("30G"))]),
            r#"{"cap": "30G"}"#
        );
    }
}
