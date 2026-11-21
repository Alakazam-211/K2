//! `hb_parse_spec` (`cli/src/heartbeat.sh`): the schedule flags of
//! `heartbeat add` and `edit`, checked as the shell checks them, then the
//! spec its Python writes (`json.dumps`, Python's defaults).

use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use k2_client::cli_name;

/// What the flags make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// The spec's JSON and its frequency.
    Made { json: String, freq: String },
    /// The lines on stderr, and the exit.
    Stop(Vec<String>, Exit),
    /// A form only the shell answers (a value flag last, a word bash
    /// would expand as a file name, a number bash's test can't read).
    Shell,
}

fn stop(line: impl Into<String>, exit: Exit) -> Spec {
    Spec::Stop(vec![line.into()], exit)
}

/// `[[ "$t" =~ ^([01][0-9]|2[0-3]|[0-9]):[0-5][0-9]$ ]]`.
fn hh_mm(t: &str) -> bool {
    let b = t.as_bytes();
    let d = |c: u8| c.is_ascii_digit();
    let mm = |x: &[u8]| x.len() == 3 && x[0] == b':' && (b'0'..=b'5').contains(&x[1]) && d(x[2]);
    match b.len() {
        4 => d(b[0]) && mm(&b[1..]),
        5 => {
            ((b[0] == b'0' || b[0] == b'1') && d(b[1])
                || b[0] == b'2' && (b'0'..=b'3').contains(&b[1]))
                && mm(&b[2..])
        }
        _ => false,
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}

/// `$((10#$n))`: bash's 64-bit arithmetic, which wraps.
fn bash_int(n: &str) -> i64 {
    n.bytes().fold(0i64, |v, c| {
        v.wrapping_mul(10).wrapping_add(i64::from(c - b'0'))
    })
}

/// `$(printf '%s' "$list" | tr ',' ' ')` split as an unquoted `for` word
/// list: `None` where bash would expand a word as a file name.
fn words(list: &str) -> Option<Vec<String>> {
    if list.contains(['*', '?', '[']) {
        return None;
    }
    Some(
        list.replace(',', " ")
            .split([' ', '\t', '\n'])
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// `x.strip()`.
fn strip(s: &str) -> &str {
    s.trim_matches(super::super::py_space)
}

fn text(s: &str) -> Value {
    Value::Str(Text::from(s))
}

/// `hb_parse_spec <args>`.
pub fn parse(args: &[&str]) -> Spec {
    let mut frequency = "";
    let (mut time, mut days, mut months, mut every) = ("", "", "", "");
    let (mut unit, mut start, mut end) = ("minutes", "", "");
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        let next = args.get(i + 1).copied();
        let took = match a {
            "--daily" | "--weekly" | "--monthly" | "--yearly" | "--hourly" => {
                frequency = &a[2..];
                i += 1;
                continue;
            }
            "--time" => next.map(|v| time = v).is_some(),
            "--days" => next.map(|v| days = v).is_some(),
            "--months" => next.map(|v| months = v).is_some(),
            "--every" => next.map(|v| every = v).is_some(),
            "--unit" => next.map(|v| unit = v).is_some(),
            "--start" => next.map(|v| start = v).is_some(),
            "--end" => next.map(|v| end = v).is_some(),
            "--once" | "--in" | "--at" | "--on" => {
                return stop(
                    format!(
                        "Error: {a} is for a one-time wake: {} heartbeat remind \"<text>\" --in 45m (see {} heartbeat remind --help). A heartbeat repeats.",
                        cli_name!(),
                        cli_name!()
                    ),
                    Exit::Usage,
                )
            }
            _ => {
                return Spec::Stop(
                    vec![
                        format!("Error: unknown schedule flag/token: {a}"),
                        "Spec: --daily|--weekly|--monthly|--yearly|--hourly [--time HH:MM] [--days ...] [--months ...] [--every N --unit minutes|hours] [--start HH:MM --end HH:MM]".into(),
                    ],
                    Exit::Usage,
                )
            }
        };
        if !took {
            return Spec::Shell;
        }
        i += 2;
    }
    if frequency.is_empty() {
        return stop(
            "Error: must specify one of --daily/--weekly/--monthly/--yearly/--hourly",
            Exit::Error,
        );
    }
    if unit != "minutes" && unit != "hours" {
        return stop(
            format!("Error: --unit must be 'minutes' or 'hours' (got '{unit}')"),
            Exit::Usage,
        );
    }
    if !every.is_empty() && !all_digits(every) {
        return stop(
            format!("Error: --every must be a whole number (got '{every}')"),
            Exit::Usage,
        );
    }
    if !every.is_empty() && bash_int(every) < 1 {
        return stop(
            format!("Error: --every must be at least 1 minute (got '{every}')"),
            Exit::Usage,
        );
    }
    for t in [time, start, end] {
        if !t.is_empty() && !hh_mm(t) {
            return stop(
                format!("Error: times must be HH:MM 24-hour (got '{t}')"),
                Exit::Usage,
            );
        }
    }
    let monthly = frequency == "monthly" || frequency == "yearly";
    if !days.is_empty() {
        let Some(toks) = words(days) else {
            return Spec::Shell;
        };
        for tok in toks {
            if monthly {
                if all_digits(&tok) && tok.len() > 18 {
                    return Spec::Shell;
                }
                let n: i64 = tok.parse().unwrap_or(0);
                if !all_digits(&tok) || !(1..=31).contains(&n) {
                    return stop(
                        format!("Error: --days for --{frequency} takes day-of-month numbers 1-31 (got '{tok}')"),
                        Exit::Usage,
                    );
                }
            } else if !matches!(
                tok.to_ascii_lowercase().as_str(),
                "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun"
            ) {
                return stop(
                    format!("Error: --days for --{frequency} takes a comma list of mon,tue,wed,thu,fri,sat,sun (got '{tok}')"),
                    Exit::Usage,
                );
            }
        }
    }
    if !months.is_empty() {
        let Some(toks) = words(months) else {
            return Spec::Shell;
        };
        for tok in toks {
            if !matches!(
                tok.to_ascii_lowercase().as_str(),
                "jan"
                    | "feb"
                    | "mar"
                    | "apr"
                    | "may"
                    | "jun"
                    | "jul"
                    | "aug"
                    | "sep"
                    | "oct"
                    | "nov"
                    | "dec"
            ) {
                return stop(
                    format!("Error: --months takes a comma list of jan,feb,mar,apr,may,jun,jul,aug,sep,oct,nov,dec (got '{tok}')"),
                    Exit::Usage,
                );
            }
        }
    }
    // The Python that writes the spec.
    let mut spec = vec![(Text::from("frequency"), text(frequency))];
    if !time.is_empty() {
        spec.push((Text::from("time"), text(time)));
    }
    if !days.is_empty() {
        let d: Vec<&str> = days.split(',').collect();
        if monthly {
            let mut nums = Vec::new();
            for x in d {
                let x = strip(x);
                if !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()) {
                    nums.push(Value::Int(x.trim_start_matches('0').to_string()));
                } else if !x.is_empty() && x.chars().all(char::is_numeric) {
                    // `str.isdigit()` holds but `int()` would raise.
                    return Spec::Shell;
                }
            }
            let nums = nums
                .into_iter()
                .map(|v| match v {
                    Value::Int(s) if s.is_empty() => Value::Int("0".into()),
                    v => v,
                })
                .collect();
            spec.push((Text::from("days_of_month"), Value::Array(nums)));
        } else {
            spec.push((
                Text::from("days"),
                Value::Array(d.iter().map(|x| text(&strip(x).to_lowercase())).collect()),
            ));
        }
    }
    if !months.is_empty() {
        spec.push((
            Text::from("months"),
            Value::Array(
                months
                    .split(',')
                    .map(|x| text(&strip(x).to_lowercase()))
                    .collect(),
            ),
        ));
    }
    if !every.is_empty() {
        let n = every.trim_start_matches('0');
        if n.len() > 30 {
            return Spec::Shell;
        }
        let n: u128 = n.parse().unwrap_or(0);
        let mul = if unit == "minutes" { 60 } else { 3600 };
        spec.push((
            Text::from("every_seconds"),
            Value::Int((n * mul).to_string()),
        ));
    }
    if !start.is_empty() {
        spec.push((Text::from("start"), text(start)));
    }
    if !end.is_empty() {
        spec.push((Text::from("end"), text(end)));
    }
    match json::to_string(&Value::Object(spec), &Options::default()) {
        Ok(json) => Spec::Made {
            json,
            freq: frequency.to_string(),
        },
        Err(_) => Spec::Shell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(args: &[&str]) -> (String, String) {
        match parse(args) {
            Spec::Made { json, freq } => (json, freq),
            other => panic!("{args:?}: {other:?}"),
        }
    }

    #[test]
    fn the_specs_python_writes() {
        assert_eq!(
            made(&["--daily", "--time", "07:00"]),
            (
                r#"{"frequency": "daily", "time": "07:00"}"#.into(),
                "daily".into()
            )
        );
        assert_eq!(
            made(&["--weekly", "--days", "Mon, TUE", "--time", "7:05"]).0,
            r#"{"frequency": "weekly", "time": "7:05", "days": ["mon", "tue"]}"#
        );
        assert_eq!(
            made(&["--monthly", "--days", "1,015,,"]).0,
            r#"{"frequency": "monthly", "days_of_month": [1, 15]}"#
        );
        assert_eq!(
            made(&["--hourly", "--every", "030", "--unit", "hours", "--start", "09:00"]).0,
            r#"{"frequency": "hourly", "every_seconds": 108000, "start": "09:00"}"#
        );
    }

    #[test]
    fn the_shell_s_stops() {
        assert_eq!(
            parse(&[]),
            stop(
                "Error: must specify one of --daily/--weekly/--monthly/--yearly/--hourly",
                Exit::Error
            )
        );
        assert_eq!(parse(&["--daily", "--time"]), Spec::Shell);
        assert_eq!(parse(&["--weekly", "--days", "*"]), Spec::Shell);
        assert!(matches!(
            parse(&["--daily", "--every", "0"]),
            Spec::Stop(_, Exit::Usage)
        ));
        assert!(matches!(
            parse(&["--daily", "--time", "24:00"]),
            Spec::Stop(_, Exit::Usage)
        ));
        assert!(matches!(
            parse(&["--monthly", "--days", "32"]),
            Spec::Stop(_, Exit::Usage)
        ));
        assert!(matches!(
            parse(&["--daily", "--in"]),
            Spec::Stop(_, Exit::Usage)
        ));
        assert!(hh_mm("9:59") && hh_mm("23:00") && !hh_mm("24:00") && !hh_mm("1:60"));
    }
}
