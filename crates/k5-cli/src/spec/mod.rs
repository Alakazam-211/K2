//! Spec verbs: commands the binary answers from their `--schema` entry
//! (`Owner::Spec` in [`crate::route`]), not from code of their own.
//!
//! An entry in `cli/src/schema/<noun>.entries` with `"impl": "spec"` says
//! how its bash verb reads its words (`parse`), which request it sends
//! (`call`), what it prints (`output`), the exits that differ from its
//! request function's (`exits`) and which credential goes with it
//! (`auth`). contract-gen reads those entries into [`table::SPEC`]
//! (`crates/k2-core/src/contract/cli_spec.rs`); nobody edits the table by
//! hand. `scripts/build-cli.sh` leaves the keys out of `k2 --schema`.
//!
//! The words each field takes are only what the runner answers today, so
//! an entry never asks for something nothing here does:
//!
//! - `parse.style`: [`Style::None`] or [`Style::For`], two of bash's ways
//!   of reading a verb's words, each byte for byte.
//! - `call.via`: [`Via::Compute`], `_compute_call` (`cli/src/compute-lib.sh`),
//!   a GET only.
//! - `output`: [`Print::Body`] or [`Print::Lines`] for the `--json` form
//!   and the text form.
//! - `auth`: [`Cred::Session`], the credential the shell's id tools send.
//!
//! The runner is [`run`].

pub mod run;
pub mod table;

pub use run::run;

/// One spec verb: an entry with `"impl": "spec"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecVerb {
    /// The entry's `name`, as `k2 --schema` prints it.
    pub entry: &'static str,
    /// The command's words, the name's words: the noun, then the verb.
    pub words: &'static [&'static str],
    pub parse: Parse,
    pub call: Call,
    pub output: Output,
    /// A status whose exit is not the via's, and its exit code.
    pub exits: &'static [(u16, i32)],
    pub auth: Cred,
}

/// How bash reads the words after the verb, and the usage line it prints
/// on a word it doesn't take (exit 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parse {
    pub style: Style,
    /// The line, as bash's `echo "$usage" >&2` prints it.
    pub usage: &'static str,
    /// The positional words, in order.
    pub args: &'static [Arg],
    /// The verb takes `--json`.
    pub json_flag: bool,
}

/// One of bash's ways of reading a verb's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Every word is ignored but `--json` (`cmd_compute`'s `for a in
    /// "$@"; do [ "$a" = "--json" ] && as_json=1; done`).
    None,
    /// One positional word, checked (`cmd_compute_stats`): `--json` sets
    /// the JSON form, any other word starting with `-` is the usage, and
    /// so is a second positional word once the first is not empty. Then
    /// the word must pass its check, or it is the usage.
    For,
}

/// A positional word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arg {
    pub name: &'static str,
    pub check: Option<Check>,
}

/// `[[ "$v" =~ ^[class]{min,max}$ ]]`: every byte in one of the ranges,
/// and from `min` to `max` of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub ranges: &'static [(u8, u8)],
    pub min: usize,
    pub max: usize,
}

impl Check {
    /// Whether `value` passes.
    pub fn passes(&self, value: &str) -> bool {
        (self.min..=self.max).contains(&value.len())
            && value
                .bytes()
                .all(|b| self.ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&b)))
    }
}

/// The request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    pub via: Via,
    pub method: Method,
    /// `/cli/...`, as bash writes it.
    pub path: &'static str,
    /// Bash's `... tcp` argument: never the cell socket.
    pub tcp: bool,
    /// The query, in order: `key=value` joined by `&`. An item whose value
    /// is empty is left out.
    pub query: &'static [Item],
}

/// Bash's request function: one adapter each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// `_compute_call`: `_compute_send`, `_compute_split`, then
    /// `_compute_fail` on any status but 200.
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
}

/// One `key=value` of the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub key: &'static str,
    pub from: Source,
    pub enc: Enc,
}

/// Where a query value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The positional word of that name.
    Arg(&'static str),
    /// A fixed value.
    Literal(&'static str),
}

/// How a query value is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enc {
    /// As it is: only a value its check passed, or a literal.
    Raw,
    /// Bash's `urlencode` ([`crate::transport::encode::quote`]).
    Quote,
}

/// What a 200 prints: `json` with `--json`, else `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    pub json: Option<Print>,
    pub text: Print,
}

/// A way of printing the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Print {
    /// `printf '%s\n' "$_CP_BODY"`: the body, then a newline.
    Body,
    /// `_compute_text`'s text form: the answer's `text` lines.
    Lines,
}

/// The credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cred {
    /// What the shell's id tools send: the passport (on the cell socket
    /// when `HOOK_SOCK` is a live one, else over TCP), else the owner
    /// token from disk; never both ([`crate::daemon`]).
    Session,
}

/// The row of `table` for `words` (the arguments after `argv[0]`): the
/// one whose words are the first words of the line, the longest when
/// several are.
pub fn find<'a>(table: &'a [SpecVerb], words: &[&str]) -> Option<&'a SpecVerb> {
    table
        .iter()
        .filter(|v| words.len() >= v.words.len() && words[..v.words.len()] == *v.words)
        .max_by_key(|v| v.words.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: Check = Check {
        ranges: &[
            (b'A', b'Z'),
            (b'a', b'z'),
            (b'0', b'9'),
            (b'_', b'_'),
            (b'-', b'-'),
        ],
        min: 4,
        max: 64,
    };

    #[test]
    fn a_check_is_bash_s_regex() {
        for ok in ["abcd", "A1_-", &"x".repeat(64)] {
            assert!(JOB.passes(ok), "{ok}");
        }
        for bad in [
            "",
            "abc",
            "ab.cd",
            "ab cd",
            "abcd\n",
            "é1234",
            &"x".repeat(65),
        ] {
            assert!(!JOB.passes(bad), "{bad:?}");
        }
    }

    #[test]
    fn find_takes_the_longest_row() {
        let row = |words: &'static [&'static str]| SpecVerb {
            entry: "x",
            words,
            parse: Parse {
                style: Style::None,
                usage: "",
                args: &[],
                json_flag: false,
            },
            call: Call {
                via: Via::Compute,
                method: Method::Get,
                path: "/cli/x",
                tcp: false,
                query: &[],
            },
            output: Output {
                json: None,
                text: Print::Body,
            },
            exits: &[],
            auth: Cred::Session,
        };
        let table = [row(&["a", "b"]), row(&["a", "b", "c"]), row(&["a", "d"])];
        assert_eq!(
            find(&table, &["a", "b", "c", "x"]).unwrap().words,
            ["a", "b", "c"]
        );
        assert_eq!(find(&table, &["a", "b"]).unwrap().words, ["a", "b"]);
        assert_eq!(
            find(&table, &["a", "d", "--json"]).unwrap().words,
            ["a", "d"]
        );
        assert!(find(&table, &["a"]).is_none());
        assert!(find(&table, &["a", "e"]).is_none());
        assert!(find(&[], &["a", "b"]).is_none());
    }
}
