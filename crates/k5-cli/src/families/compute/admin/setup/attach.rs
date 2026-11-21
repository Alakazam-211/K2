//! `compute attach <job> [--since SEQ] [--raw]`, as `cmd_compute` answers
//! it (`cli/src/compute-command.sh`): one GET of `/cli/compute/jobs/get`
//! for the job's full id, then `_compute_follow` from sequence number SEQ
//! (0 when not given) until the job ends ([`follow`]), with its exit.
//!
//! The shell answers it where the shell's `mktemp` would fail, where the
//! environment carries `_compute_follow`'s own settings (`_CP_DEADLINE`,
//! `_CP_FAILURES`, `_CP_LAST_Q`), and for a `--since` that is more than
//! letters, digits and `._~:-` (the shell puts it in the query as it is).

use super::super::super::follow::{self, Follow, Start};
use super::super::super::pyrt as p;
use super::super::super::writes::{load, stopped, Cp, Words};
use crate::exit::Exit;
use crate::families::Raise;
use crate::transport::encode::quote;
use k2_client::cli_name;

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute attach <job> [--since SEQ] [--raw]"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub job: String,
    pub since: String,
    pub raw: bool,
}

/// The words after `attach`. `None`: the shell answers it.
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let busy = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if busy("_CP_DEADLINE") || busy("_CP_FAILURES") || busy("_CP_LAST_Q") || !follow::usable() {
        return None;
    }
    let (mut job, mut since, mut raw) = (String::new(), String::new(), false);
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--since" => {
                since = words.get(i + 1).copied().unwrap_or("").to_string();
                if i + 2 > words.len() {
                    return Some(Words::ShiftFailed(words[i].to_string()));
                }
                i += 2;
                continue;
            }
            "--raw" => raw = true,
            w => job = w.to_string(),
        }
        i += 1;
    }
    if !since
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | ':' | '-'))
    {
        return None;
    }
    if job.is_empty() {
        return Some(Words::Usage(USAGE.to_string()));
    }
    Some(Words::Ask(Ask { job, since, raw }))
}

/// The job's id, then the follow.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let body = match cp.call(false, follow::GET, &format!("job={}", quote(&a.job)), false) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let id = load(&body, 1).and_then(|v| p::idx(&v, "id").map_err(|e| Raise::at(1, e)));
    let id = match id {
        Ok(v) => p::str_of(&v).trim_end_matches('\n').replace('\0', ""),
        Err(e) => return stopped(String::new(), &e).print(),
    };
    let since = if a.since.is_empty() { "0" } else { &a.since };
    follow::follow(
        cp,
        &Follow {
            job: id,
            raw: a.raw,
            start: Start::Seq(since.to_string()),
            as_json: false,
            deadline: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Some(Words::Usage(USAGE.into())));
        assert_eq!(
            parse(&["j", "--since"]),
            Some(Words::ShiftFailed("--since".into()))
        );
        assert_eq!(parse(&["j", "--since", "a b"]), None);
        assert_eq!(
            parse(&["--raw", "j1", "--since", "12"]),
            Some(Words::Ask(Ask {
                job: "j1".into(),
                since: "12".into(),
                raw: true
            }))
        );
    }
}
