//! A read verb's arguments: flags with a value and bare flags, each from the verb's own list. An
//! unknown flag, a flag given twice, a missing value or a word outside a flag is refused
//! ([`crate::error::USAGE`]), naming the flags the verb takes.

use crate::error::{CadError, USAGE};

/// The flags a verb takes.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub verb: &'static str,
    /// Flags followed by a value.
    pub valued: &'static [&'static str],
    /// Flags alone.
    pub bare: &'static [&'static str],
    /// Words outside any flag are taken (as `header get $VAR…`).
    pub words: bool,
}

/// Parsed arguments.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pairs: Vec<(&'static str, String)>,
    bare: Vec<&'static str>,
    /// Words outside any flag, in order.
    pub words: Vec<String>,
}

impl Spec {
    fn accepted(&self) -> String {
        let mut v: Vec<String> = self.valued.iter().map(|f| format!("{f} V")).collect();
        v.extend(self.bare.iter().map(|f| (*f).to_owned()));
        if v.is_empty() {
            format!("{} takes no flags", self.verb)
        } else {
            format!("{} takes: {}", self.verb, v.join(", "))
        }
    }

    fn usage(&self, why: String) -> CadError {
        CadError::new(USAGE, why, self.accepted())
    }

    /// Parses `args` (`--json` is the host's, and ignored here).
    pub fn parse(&self, args: &[&str]) -> Result<Args, CadError> {
        let mut out = Args::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i];
            i += 1;
            if a == "--json" {
                continue;
            }
            if let Some(f) = self.valued.iter().find(|f| **f == a) {
                if out.pairs.iter().any(|(k, _)| k == f) {
                    return Err(self.usage(format!("{a} given twice")));
                }
                let Some(v) = args.get(i) else {
                    return Err(self.usage(format!("{a} needs a value")));
                };
                i += 1;
                out.pairs.push((*f, (*v).to_owned()));
            } else if let Some(f) = self.bare.iter().find(|f| **f == a) {
                out.bare.push(*f);
            } else if a.starts_with("--") || !self.words {
                return Err(self.usage(format!("{} doesn't take {a:?}", self.verb)));
            } else {
                out.words.push(a.to_owned());
            }
        }
        Ok(out)
    }
}

impl Args {
    /// The value of flag `f`.
    pub fn get(&self, f: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| *k == f)
            .map(|(_, v)| v.as_str())
    }

    /// Whether bare flag `f` was given.
    pub fn has(&self, f: &str) -> bool {
        self.bare.contains(&f)
    }

    /// Every argument again, `--after` replaced by `after` (for the next page's call).
    pub fn with_after(&self, after: &str) -> Vec<String> {
        let mut v = Vec::new();
        for (k, val) in &self.pairs {
            if *k != "--after" {
                v.push((*k).to_owned());
                v.push(val.clone());
            }
        }
        for b in &self.bare {
            v.push((*b).to_owned());
        }
        v.push("--after".into());
        v.push(after.to_owned());
        v
    }
}

/// A box flag's value, `x0,y0,x1,y1`, as numbers (low corner first).
pub fn parse_box(spec: &Spec, flag: &str, v: &str) -> Result<[f64; 4], CadError> {
    let n: Vec<f64> = v
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| spec.usage(format!("{flag} takes x0,y0,x1,y1, four numbers")))?;
    if n.len() != 4 || n.iter().any(|x| !x.is_finite()) {
        return Err(spec.usage(format!("{flag} takes x0,y0,x1,y1, four numbers")));
    }
    Ok([
        n[0].min(n[2]),
        n[1].min(n[3]),
        n[0].max(n[2]),
        n[1].max(n[3]),
    ])
}

/// A `--limit` value: 1 to `max`.
pub fn parse_limit(
    spec: &Spec,
    v: Option<&str>,
    default: usize,
    max: usize,
) -> Result<usize, CadError> {
    match v {
        None => Ok(default),
        Some(s) => s
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| spec.usage(format!("--limit takes 1 to {max}"))),
    }
}

/// [`Spec`]'s refusal for a reason of the verb's own.
pub fn usage(spec: &Spec, why: impl Into<String>) -> CadError {
    spec.usage(why.into())
}
