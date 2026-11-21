//! `k5 compute retier|tier <job> --tier standard|priority|batch|ci [--why
//! "…"] [--json]`, as `cmd_compute_retier` answers it
//! (`cli/src/compute-access.sh`): one POST to `/cli/compute/jobs/tier`,
//! then the move in two lines (or the answer as it came, `--json`).

use super::super::writes::{body_of, load, print_body, stopped, Cp, Words};
use super::{str_plus, table_get};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::py;
use crate::out::pylib::ops::{get_in, index, py_eq};
use k2_client::cli_name;

pub const ENDPOINT: &str = "/cli/compute/jobs/tier";

/// The usage line, without what follows it.
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute retier <job> --tier standard|priority|batch|ci [--why \"…\"]"
);

/// The tiers as the reader names them.
const NAMES: &[(&str, &str)] = &[
    ("ci", "CI"),
    ("priority", "Priority"),
    ("normal", "Standard"),
    ("batch", "Batch"),
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub job: String,
    /// `ci`, `priority`, `normal` or `batch`.
    pub tier: String,
    pub why: String,
    pub json: bool,
}

/// The words after `retier`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = || words.get(i + 1).copied().unwrap_or("").to_string();
        match w {
            "--tier" | "--why" => {
                if w == "--tier" {
                    a.tier = next();
                } else {
                    a.why = next();
                }
                if i + 2 > words.len() {
                    return Words::ShiftFailed(words[i].to_string());
                }
                i += 2;
                continue;
            }
            "--priority" | "--critical" => a.tier = "priority".into(),
            "--standard" | "--normal" => a.tier = "normal".into(),
            "--batch" => a.tier = "batch".into(),
            "--json" => a.json = true,
            w if w.starts_with('-') => {
                return Words::Usage(format!(
                    "usage: unknown flag '{w}' ({} compute retier <job> --tier standard|priority|batch|ci [--why \"…\"])",
                    cli_name!()
                ))
            }
            w => a.job = w.to_string(),
        }
        i += 1;
    }
    match a.tier.as_str() {
        "ci" | "priority" | "batch" => {}
        "normal" | "standard" => a.tier = "normal".into(),
        _ => return Words::Usage(format!("{USAGE}   (--priority needs --why)")),
    }
    if a.job.is_empty() {
        return Words::Usage(USAGE.to_string());
    }
    if a.tier == "priority" && a.why.is_empty() {
        return Words::Usage(format!(
            "usage: --priority needs a one-line reason: {} compute retier {} --priority --why \"release gate for 0.46.6\"",
            cli_name!(),
            a.job
        ));
    }
    Words::Ask(a)
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let body = body_of(&[("job", &a.job), ("tier", &a.tier), ("why", &a.why)]);
    let answer = match cp.call(true, ENDPOINT, &body, false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.json {
        return print_body(&answer);
    }
    reader(&answer).print()
}

/// The reader on the answer.
pub fn reader(body: &[u8]) -> Shown {
    let mut out = String::new();
    let r = (|| -> Result<(), Raise> {
        let v = load(body, 3)?;
        let job = py::str_of(index(&v, "job", 5)?);
        let from = table_get(NAMES, index(&v, "from", 5)?, 5)?;
        let tier = table_get(NAMES, index(&v, "tier", 5)?, 5)?;
        let node = py::str_of(index(&v, "node", 5)?);
        let mut line = format!("job {job}: {from} → {tier} on {node}");
        if let Some(asked) = get_in(&v, "asked", 6)?.filter(|x| py::truthy(x)) {
            if !py_eq(asked, index(&v, "tier", 6)?) {
                line.push_str(&format!(
                    " (asked {}: this server's Priority doesn't count there)",
                    table_get(NAMES, asked, 7)?
                ));
            }
        }
        out.push_str(&line);
        out.push('\n');
        out.push_str(&str_plus("  ", index(&v, "note", 9)?, 9)?);
        out.push('\n');
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}
