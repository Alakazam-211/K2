//! The `compute` setup verbs, as `cmd_compute` answers them
//! (`cli/src/compute-command.sh` and the files it calls):
//!
//! - `compute` alone, an empty verb or `help`: the help page
//!   (`cli/text/help/compute.txt`), after the gates and the `id` tool's
//!   lines, as the shell prints it.
//! - [`allow`]: `allow-nodes [<node>… | --add | --remove | --clear]
//!   [--workspace WS] [--json]` (`cmd_compute_allow_nodes`).
//! - [`git`]: `git-access [list|setup|grant|revoke] …`
//!   (`cmd_compute_git_access`).
//! - [`lanes`]: `lanes [--json]` and `lanes set <name> …`
//!   (`cmd_compute_trust lanes`).
//! - [`local`]: `local [status|pause|drain|resume|policy|enable|
//!   remote-admin|disable] …` (`cmd_compute_local`).
//!
//! [`super::run`] drives them as it drives the other admin verbs.

pub mod allow;
pub mod git;
pub mod lanes;
pub mod local;

use super::super::writes::{Cp, Words};
use crate::exit::Exit;
use crate::families::Shown;

/// The second words this module answers (and none: `compute` alone).
pub const ANSWERS: &[&str] = &["allow-nodes", "git-access", "lanes", "local"];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `_compute_usage`.
    Usage,
    Allow(allow::Ask),
    Git(git::Ask),
    Lanes(lanes::Ask),
    Local(local::Ask),
}

/// Whether `words` (from the verb on) is one of this module's forms.
pub fn answers(words: &[&str]) -> bool {
    match words.first() {
        None | Some(&"") | Some(&"help") => true,
        Some(w) => ANSWERS.contains(w),
    }
}

/// The words from the verb on. `None`: the shell answers the line.
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let rest = words.get(1..).unwrap_or(&[]);
    Some(match words.first().copied().unwrap_or("") {
        "" | "help" => Words::Ask(Ask::Usage),
        "allow-nodes" => map(allow::parse(rest), Ask::Allow),
        "git-access" => map(git::parse(rest)?, Ask::Git),
        "lanes" => map(lanes::parse(rest), Ask::Lanes),
        "local" => map(local::parse(rest), Ask::Local),
        _ => return None,
    })
}

fn map<T>(w: Words<T>, f: impl FnOnce(T) -> Ask) -> Words<Ask> {
    match w {
        Words::Ask(a) => Words::Ask(f(a)),
        Words::Usage(l) => Words::Usage(l),
        Words::ShiftFailed => Words::ShiftFailed,
    }
}

/// The requests and what they print.
pub fn send(cp: &mut Cp<'_>, ask: &Ask) -> Exit {
    match ask {
        Ask::Usage => Shown::out(
            crate::text::help("compute").expect("cli/text/help/compute.txt is embedded"),
        )
        .print(),
        Ask::Allow(a) => allow::send(cp, a),
        Ask::Git(a) => git::send(cp, a),
        Ask::Lanes(a) => lanes::send(cp, a),
        Ask::Local(a) => local::send(cp, a),
    }
}

/// `cli/src/compute-access.sh`.
pub const ACCESS_SRC: &str = include_str!("../../../../../../cli/src/compute-access.sh");
/// `cli/src/compute-local-trust.sh`.
pub const LOCAL_SRC: &str = include_str!("../../../../../../cli/src/compute-local-trust.sh");

/// The reader's exception at a line.
pub fn at(line: u32) -> impl Fn(String) -> crate::families::Raise {
    move |e| crate::families::Raise::at(line, e)
}
