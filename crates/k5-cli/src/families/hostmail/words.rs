//! How a `cmd_hostmail_*` function reads its words: a `while [ $# -gt 0 ]`
//! loop over `--json`, value flags (`[ $# -ge 2 ] || { _mail_err "usage"
//! …; exit 2; }`), switches, any other `-*` (a usage line), and the
//! positional words, each filling the first of its slots still empty
//! (`if [ -z "$a" ]; then a="$1"; elif …; else <extra>`).

use crate::families::mail::core::{usage, Plan};

/// One function's loop.
pub struct Spec<'a> {
    /// The settings positional words fill, in order: a word fills the
    /// first one still empty (a value flag may have filled it).
    pub slots: &'a [&'static str],
    /// `(flag, setting, the line when its value is missing)`.
    pub values: &'a [(&'static str, &'static str, &'a str)],
    /// `(flag, setting)`: the setting is `true`.
    pub switches: &'a [(&'static str, &'static str)],
    /// The line for any other word starting with `-`; `{w}` is the word.
    pub unknown: &'a str,
    /// The line for a positional word with every slot full; `{w}` is the
    /// word.
    pub extra: &'a str,
}

/// What the loop read.
#[derive(Debug, Default)]
pub struct Got {
    pub json: bool,
    vals: Vec<(&'static str, String)>,
}

impl Got {
    /// The setting, empty when not given.
    pub fn get(&self, key: &str) -> &str {
        self.vals
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map_or("", |(_, v)| v.as_str())
    }

    /// A switch was given.
    pub fn on(&self, key: &str) -> bool {
        self.get(key) == "true"
    }

    fn set(&mut self, key: &'static str, v: &str) {
        self.vals.push((key, v.to_string()));
    }
}

/// `template` with its `{w}` as `word`.
pub fn fill(template: &str, word: &str) -> String {
    match template.split_once("{w}") {
        Some((a, b)) => format!("{a}{word}{b}"),
        None => template.to_string(),
    }
}

/// The loop over `words`, or its usage line.
pub fn scan(words: &[&str], spec: &Spec) -> Result<Got, Plan> {
    let mut got = Got::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "--json" {
            got.json = true;
        } else if let Some((_, key, missing)) = spec.values.iter().find(|(f, _, _)| *f == w) {
            let v = words.get(i + 1).ok_or_else(|| usage(*missing))?;
            got.set(key, v);
            i += 1;
        } else if let Some((_, key)) = spec.switches.iter().find(|(f, _)| *f == w) {
            got.set(key, "true");
        } else if w.starts_with('-') {
            return Err(usage(fill(spec.unknown, w)));
        } else {
            match spec.slots.iter().find(|k| got.get(k).is_empty()) {
                Some(k) => got.set(k, w),
                None => return Err(usage(fill(spec.extra, w))),
            }
        }
        i += 1;
    }
    Ok(got)
}

/// `_wants_help "$@"`.
pub fn wants_help(words: &[&str]) -> bool {
    words.iter().any(|w| *w == "--help" || *w == "-h")
}
