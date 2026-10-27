//! GNU-style argv parsing (getopt_long with argument permutation) for the tools.
//!
//! - Short options cluster (`-ni`); one that takes a value takes the rest of the cluster or
//!   the next argument (`-n5`, `-n 5`).
//! - Long options take `--name=value` or `--name value`, and may be abbreviated to any
//!   unique prefix, as getopt_long allows.
//! - Options and operands may be mixed; `--` ends the options.
//! - Anything not in a tool's table is [`ToolError::Unsupported`] with a hint. Nothing is
//!   ever guessed.

use super::error::ToolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arity {
    /// A flag.
    No,
    /// Takes a value (attached or the next argument).
    Req,
    /// Takes a value only when attached (`-i.bak`, `--in-place=.bak`).
    Opt,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Opt {
    pub id: &'static str,
    pub short: Option<char>,
    pub long: Option<&'static str>,
    pub arity: Arity,
}

pub(crate) const fn opt(
    id: &'static str,
    short: Option<char>,
    long: Option<&'static str>,
    arity: Arity,
) -> Opt {
    Opt {
        id,
        short,
        long,
        arity,
    }
}

pub(crate) struct Spec {
    pub tool: &'static str,
    pub opts: &'static [Opt],
    /// Known flags that are refused, with the hint to give: (`-P`, "use -E").
    pub unsupported: &'static [(&'static str, &'static str)],
    /// `-NUM` (all digits) is an option with id `"NUM"` (grep, head, tail).
    pub numeric: bool,
    /// Digits inside a short cluster accumulate into `"NUM"` too (grep: `-5n`).
    pub numeric_in_cluster: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Opt(&'static str, Option<String>),
    Operand(String),
}

fn unsupported(spec: &Spec, flag: &str) -> ToolError {
    let name = flag.split('=').next().unwrap_or(flag);
    for (f, hint) in spec.unsupported {
        if *f == name {
            return ToolError::unsupported(name, *hint);
        }
    }
    ToolError::unsupported(
        name,
        format!(
            "not in doc {}'s supported flags: {}",
            spec.tool,
            listed(spec)
        ),
    )
}

fn listed(spec: &Spec) -> String {
    let mut v: Vec<String> = Vec::new();
    for o in spec.opts {
        if let Some(c) = o.short {
            v.push(format!("-{c}"));
        } else if let Some(l) = o.long {
            v.push(format!("--{l}"));
        }
    }
    if spec.numeric {
        v.push("-NUM".into());
    }
    v.join(" ")
}

/// Parse `args` (without the verb name) against `spec`.
pub(crate) fn parse(args: &[String], spec: &Spec) -> Result<Vec<Item>, ToolError> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut only_operands = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if only_operands || a == "-" || !a.starts_with('-') {
            out.push(Item::Operand(a.clone()));
            continue;
        }
        if a == "--" {
            only_operands = true;
            continue;
        }
        if let Some(body) = a.strip_prefix("--") {
            let (name, value) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (body, None),
            };
            let exact: Vec<&Opt> = spec.opts.iter().filter(|o| o.long == Some(name)).collect();
            let cands: Vec<&Opt> = if exact.is_empty() {
                spec.opts
                    .iter()
                    .filter(|o| o.long.is_some_and(|l| l.starts_with(name)))
                    .collect()
            } else {
                exact
            };
            let o = match cands.as_slice() {
                [o] => *o,
                [] => return Err(unsupported(spec, a)),
                _ if cands.iter().all(|c| c.id == cands[0].id) => cands[0],
                _ => return Err(ToolError::usage(format!("option '--{name}' is ambiguous"))),
            };
            let full = o.long.unwrap_or(name);
            match (o.arity, value) {
                (Arity::No, Some(_)) => {
                    return Err(ToolError::usage(format!(
                        "option '--{full}' doesn't allow an argument"
                    )))
                }
                (Arity::No, None) => out.push(Item::Opt(o.id, None)),
                (Arity::Opt, v) => out.push(Item::Opt(o.id, v)),
                (Arity::Req, Some(v)) => out.push(Item::Opt(o.id, Some(v))),
                (Arity::Req, None) => {
                    let v = args.get(i).ok_or_else(|| {
                        ToolError::usage(format!("option '--{full}' requires an argument"))
                    })?;
                    i += 1;
                    out.push(Item::Opt(o.id, Some(v.clone())));
                }
            }
            continue;
        }
        let body = &a[1..];
        if spec.numeric && body.chars().all(|c| c.is_ascii_digit()) {
            out.push(Item::Opt("NUM", Some(body.to_owned())));
            continue;
        }
        let chars: Vec<(usize, char)> = body.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            let (at, c) = chars[k];
            if spec.numeric_in_cluster && c.is_ascii_digit() {
                let start = at;
                while k < chars.len() && chars[k].1.is_ascii_digit() {
                    k += 1;
                }
                let end = chars.get(k).map_or(body.len(), |x| x.0);
                out.push(Item::Opt("NUM", Some(body[start..end].to_owned())));
                continue;
            }
            let Some(o) = spec.opts.iter().find(|o| o.short == Some(c)) else {
                return Err(unsupported(spec, &format!("-{c}")));
            };
            let rest = &body[at + c.len_utf8()..];
            match o.arity {
                Arity::No => {
                    out.push(Item::Opt(o.id, None));
                    k += 1;
                }
                Arity::Opt => {
                    out.push(Item::Opt(o.id, (!rest.is_empty()).then(|| rest.to_owned())));
                    break;
                }
                Arity::Req => {
                    if !rest.is_empty() {
                        out.push(Item::Opt(o.id, Some(rest.to_owned())));
                    } else {
                        let v = args.get(i).ok_or_else(|| {
                            ToolError::usage(format!("option requires an argument -- '{c}'"))
                        })?;
                        i += 1;
                        out.push(Item::Opt(o.id, Some(v.clone())));
                    }
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// Parse a count: decimal digits with an optional GNU size suffix (`b` 512, `K`/`KiB`
/// 1024, `kB` 1000, `M`, `MB`, `MiB`, `G`, `GB`, `GiB`). Saturates instead of overflowing,
/// as huge counts mean "everything".
pub(crate) fn count(s: &str, what: &str) -> Result<u64, ToolError> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return Err(ToolError::usage(format!("invalid number of {what}: '{s}'")));
    }
    let mut n: u64 = 0;
    for b in s[..digits].bytes() {
        n = n.saturating_mul(10).saturating_add(u64::from(b - b'0'));
    }
    let mult: u64 = match &s[digits..] {
        "" => 1,
        "b" => 512,
        "K" | "KiB" => 1 << 10,
        "kB" | "KB" => 1000,
        "M" | "MiB" => 1 << 20,
        "MB" => 1000 * 1000,
        "G" | "GiB" => 1 << 30,
        "GB" => 1000 * 1000 * 1000,
        _ => return Err(ToolError::usage(format!("invalid number of {what}: '{s}'"))),
    };
    Ok(n.saturating_mul(mult))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: &[Opt] = &[
        opt("n", Some('n'), Some("line-number"), Arity::No),
        opt("e", Some('e'), Some("regexp"), Arity::Req),
        opt("i", Some('i'), Some("in-place"), Arity::Opt),
        opt("m", Some('m'), Some("max-count"), Arity::Req),
    ];
    const SPEC: Spec = Spec {
        tool: "t",
        opts: OPTS,
        unsupported: &[("-P", "use -E")],
        numeric: true,
        numeric_in_cluster: true,
    };

    fn p(a: &[&str]) -> Result<Vec<Item>, ToolError> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &SPEC)
    }

    #[test]
    fn getopt_shapes() {
        use Item::*;
        assert_eq!(
            p(&[
                "-ne",
                "x",
                "y",
                "--line-n",
                "-m2",
                "--max-count=3",
                "--",
                "-z"
            ])
            .unwrap(),
            vec![
                Opt("n", None),
                Opt("e", Some("x".into())),
                Operand("y".into()),
                Opt("n", None),
                Opt("m", Some("2".into())),
                Opt("m", Some("3".into())),
                Operand("-z".into()),
            ]
        );
        assert_eq!(p(&["-i"]).unwrap(), vec![Opt("i", None)]);
        assert_eq!(p(&["-i.bak"]).unwrap(), vec![Opt("i", Some(".bak".into()))]);
        assert_eq!(
            p(&["-5n"]).unwrap(),
            vec![Opt("NUM", Some("5".into())), Opt("n", None)]
        );
        assert_eq!(p(&["-12"]).unwrap(), vec![Opt("NUM", Some("12".into()))]);
        assert!(matches!(p(&["-P"]), Err(ToolError::Unsupported { hint, .. }) if hint == "use -E"));
        assert!(matches!(p(&["--nope"]), Err(ToolError::Unsupported { .. })));
        assert!(matches!(p(&["-e"]), Err(ToolError::Usage(_))));
        assert!(matches!(p(&["--line-number=3"]), Err(ToolError::Usage(_))));
    }

    #[test]
    fn counts() {
        assert_eq!(count("12", "lines").unwrap(), 12);
        assert_eq!(count("2K", "bytes").unwrap(), 2048);
        assert_eq!(count("1kB", "bytes").unwrap(), 1000);
        assert_eq!(count("99999999999999999999999", "x").unwrap(), u64::MAX);
        assert!(count("x", "lines").is_err());
        assert!(count("3q", "lines").is_err());
    }
}
