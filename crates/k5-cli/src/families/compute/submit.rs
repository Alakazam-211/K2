//! The `compute` verbs that submit jobs, as the shell CLI's `cmd_compute`
//! answers them (`cli/src/compute-run.sh`):
//!
//! - [`run`]: `run [<node>] [flags] -- <command…>` (`cmd_compute_run`):
//!   the words, the body, one POST of `/cli/compute/run` (sent again only
//!   when no answer came, up to 3 times, with the same `clientJobId`),
//!   then the plan (`--plan`), the detached id (`--detach`), or the job
//!   followed to its end ([`super::follow`], [`set`] for a split run, and
//!   [`super::writes::wait`] for `--wait --quiet` and `--wait --json`).
//! - [`test`]: `test [<node>] [--shards N] … [-- nextest args]`
//!   (`cmd_compute_test`): the nextest script, then `run`.
//! - [`prewarm`]: `prewarm [--preset P | --needs …] … [--wait] [--json]`
//!   (`cmd_compute_prewarm`).
//!
//! What runs is what [`super::admin`] runs, in the shell's order: the
//! settings, a re-minted passport, the two gates, the `id` tool's lines,
//! the verb's words (usage: exit 2; a `shift 2` with one word left: exit
//! 1, nothing printed), then the verb's requests and readers, then the
//! EXIT trap's lines.
//!
//! What the binary hands to the shell, before it prints or sends anything:
//! what [`super::admin::run`] hands it; `--failures` (the follow's and
//! `wait`'s list of failing tests); `--max-wait` with a duration the shell's
//! arithmetic would refuse; a value the shell's `_compute_json` reads as a
//! number that is more than digits (`--cpus`); a value checked by a bash
//! regex that is not ASCII (the locale's ranges); a temp file or folder the
//! shell's `mktemp` can't make; and the shell's own follow settings in the
//! environment (`_CP_DEADLINE`, `_CP_DEADLINE_RC`, `_CP_FAILURES`,
//! `_CP_LAST_Q`, `_CP_OWNER_WARNED`).

use super::writes::{self, Cp, Words};
use crate::daemon;
use crate::exit::Exit;
use crate::families::curl_port;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use std::ffi::OsString;

pub mod plan;
pub mod prewarm;
pub mod run;
pub mod set;
pub mod test;

#[cfg(test)]
mod tests;

/// The second words this module answers.
pub const ANSWERS: &[&str] = &["run", "test", "prewarm"];

/// `cli/src/compute-run.sh`.
pub const SRC: &str = include_str!("../../../../../cli/src/compute-run.sh");

/// A verb this module answers, with its words read.
enum Ask {
    Run(Box<run::Ask>),
    Prewarm(Box<prewarm::Ask>),
}

/// `k5 compute <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: another module or the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let verb = *words.first()?;
    if !ANSWERS.contains(&verb) {
        return None;
    }
    let busy = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if [
        "_CP_DEADLINE",
        "_CP_DEADLINE_RC",
        "_CP_FAILURES",
        "_CP_LAST_Q",
        "_CP_OWNER_WARNED",
    ]
    .iter()
    .any(|n| busy(n))
    {
        return None;
    }
    let rest = &words[1..];
    let parsed = match verb {
        "run" => map(run::parse(rest)?, |a| Ask::Run(Box::new(a))),
        "test" => map(test::parse(rest)?, |a| Ask::Run(Box::new(a))),
        _ => map(prewarm::parse(rest)?, |a| Ask::Prewarm(Box::new(a))),
    };
    let max_time = writes::post_max_time(std::env::var_os("K2_CP_MAX_TIME"))?;
    if writes::shell_settings(std::env::vars_os()) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) =
        prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, writes::TOOL)
    {
        return Some(stop);
    }
    let ask = match parsed {
        Words::Ask(a) => a,
        Words::Usage(line) => {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
        Words::ShiftFailed => return Some(Exit::Error),
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    client.compute_post_max_time = max_time;
    let as_json = words.iter().skip(1).any(|w| *w == "--json");
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json,
    };
    let exit = match &ask {
        Ask::Run(a) => run::send(&mut cp, a),
        Ask::Prewarm(a) => prewarm::send(&mut cp, a),
    };
    let closing = std::mem::take(&mut cp.closing);
    closing.flush(&mut client, exit);
    Some(exit)
}

fn map<T>(w: Words<T>, f: impl FnOnce(T) -> Ask) -> Words<Ask> {
    match w {
        Words::Ask(a) => Words::Ask(f(a)),
        Words::Usage(l) => Words::Usage(l),
        Words::ShiftFailed => Words::ShiftFailed,
    }
}

/// `$(_compute_secs "$v")` in an assignment under `set -e`: the seconds,
/// or the usage line (exit 2). `None`: a number the shell's 64-bit
/// arithmetic would wrap (the shell's).
pub fn secs<T>(v: &str) -> Option<Result<String, Words<T>>> {
    if !super::admin::node::duration_shaped(v) {
        return Some(Err(Words::Usage(format!(
            "usage: bad duration '{v}' (use 2h, 90m, 1h30m, 300s)"
        ))));
    }
    super::events::secs(v).map(|n| Ok(n.to_string()))
}

/// `$(_compute_bytes "$v")`: the bytes, or the usage line (exit 2).
/// `None`: a number past the shell's 64-bit arithmetic.
pub fn bytes<T>(v: &str) -> Option<Result<String, Words<T>>> {
    let bad = || {
        Some(Err(Words::Usage(format!(
            "usage: bad size '{v}' (use 16G or 512M)"
        ))))
    };
    let (digits, unit) = match v.char_indices().last() {
        Some((i, c @ ('G' | 'g' | 'M' | 'm'))) => (&v[..i], Some(c)),
        _ => (v, None),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return bad();
    }
    let trimmed = digits.trim_start_matches('0');
    let n: i64 = if trimmed.is_empty() {
        0
    } else {
        if trimmed.len() > 18 {
            return None;
        }
        trimmed.parse().ok()?
    };
    let n = match unit {
        Some('G' | 'g') => n.checked_mul(1_073_741_824)?,
        Some(_) => n.checked_mul(1_048_576)?,
        None => n,
    };
    Some(Ok(n.to_string()))
}

/// Whether `v` matches a bash `[[ =~ ^…$ ]]` pattern made of runs of
/// `[set]{min,max}`: each run a bracket's set as bash spells it (`a-z0-9._-`)
/// and how many of them. Only for ASCII text (the caller hands the rest to
/// the shell).
pub fn runs(v: &str, parts: &[(&str, usize, usize)]) -> bool {
    fn go(b: &[u8], parts: &[(&str, usize, usize)]) -> bool {
        let Some(((set, min, max), more)) = parts.split_first() else {
            return b.is_empty();
        };
        let n = b.iter().take(*max).take_while(|c| in_set(set, **c)).count();
        (*min..=n).rev().any(|k| go(&b[k..], more))
    }
    go(v.as_bytes(), parts)
}

/// Whether `c` is in a bracket's set: ranges (`a-z`) and single bytes, a
/// `-` last is itself.
fn in_set(set: &str, c: u8) -> bool {
    let s = set.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if i + 2 < s.len() && s[i + 1] == b'-' {
            if (s[i]..=s[i + 2]).contains(&c) {
                return true;
            }
            i += 3;
        } else {
            if s[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// `_compute_distro_arg`: the value, or its usage line (exit 2).
pub fn distro<T>(v: &str) -> Option<Result<String, Words<T>>> {
    if !v.is_ascii() {
        return None;
    }
    let ok = runs(v, &[("a-z0-9", 1, 1), ("a-z0-9._-", 0, 39)])
        || runs(
            v,
            &[
                ("a-z0-9", 1, 1),
                ("a-z0-9._-", 0, 39),
                ("@", 1, 1),
                ("0-9A-Za-z", 1, 1),
                ("0-9A-Za-z._-", 0, 19),
            ],
        );
    Some(if ok {
        Ok(v.to_string())
    } else {
        Err(Words::Usage(concat!(
            "usage: --distro ID[@VERSION] (ubuntu, ubuntu@24.04, debian, arch, fedora: the ID or ID_LIKE in the node's /etc/os-release; ",
            k2_client::cli_name!(),
            " compute nodes shows each node's)"
        ).to_string()))
    })
}

/// `_compute_gpu_arg`: `None` when the value is good, else its usage line.
pub fn gpu_arg(v: &str) -> Option<String> {
    match v {
        "any" | "amd" | "intel" => None,
        "nvidia" => Some("usage: --gpu nvidia: NVIDIA GPU jobs aren't supported yet; use --gpu any, amd or intel".to_string()),
        _ => Some("usage: --gpu [any|amd|intel] (one GPU of the node, to this job alone; bare --gpu = any)".to_string()),
    }
}

/// `git rev-parse <flag> 2>/dev/null || true` in this folder, as `$(…)`
/// keeps it (trailing newlines gone), when `_k2_git_usable`.
pub fn git_rev_parse(flag: &str) -> String {
    let Some(git) = daemon::usable_git() else {
        return String::new();
    };
    let out = std::process::Command::new(git)
        .args(["rev-parse", flag])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let text: String = String::from_utf8_lossy(&out)
        .chars()
        .filter(|c| *c != '\0')
        .collect();
    text.trim_end_matches('\n').to_string()
}

/// `${x:0:8}`.
pub fn head8(x: &str) -> String {
    x.chars().take(8).collect()
}
