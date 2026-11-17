//! The command line's exit codes: the shell CLI's, and the front's own.
//!
//! The shell's are listed for people in `k2 study errors`
//! (`cli/src/study-basics.sh`, `_study_page_errors`) and in the help of
//! the verbs that use the rarer ones. The Rust CLI exits with the same
//! number for the same outcome, so a caller that reads the code doesn't
//! see which of the two answered.

/// An exit code of the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// 0: it worked. A governed mail send that was queued for approval is
    /// 0 too (`k2 study errors`).
    Ok,
    /// 1: an error: the daemon is down, a reply didn't parse, anything
    /// unexpected (`k2 study errors`). The front's own failures are 1 too
    /// ([`crate::fallthrough::FrontError`]).
    Error,
    /// 2: bad usage: a wrong or unknown flag, an unknown topic. Also a
    /// change held for approval (`doc.sh`, `"held": 2`) and some timeouts
    /// (`k2 mail wait`) (`k2 study errors`).
    Usage,
    /// 3: a refusal. Retrying won't help; ask a person
    /// (`k2 study errors`, "EXIT 3, STOP LOOPING").
    Refused,
    /// 4: not found: a workspace, a feedback id, a file. An ambiguous
    /// prefix is 4 too (`k2 study errors`).
    NotFound,
    /// 5: someone else has it for now, or it moved under you: another
    /// agent's lease or claim on a doc (`doc.sh`), a garden data write
    /// whose revision moved (`garden-data.sh`, `conflict`), a locked or
    /// existing board object or no votes left (`board.sh`). `k2 feedback
    /// wait` uses it when the person wrote a message instead of picking an
    /// option (`feedback.sh`).
    Conflict,
    /// 7: curl's code when nothing accepted the connection: the daemon is
    /// not there. `_k2_curl` (`cli-tools-policy.sh`) retries it a few
    /// times, then returns it, and a call whose last step is curl ends
    /// with it (the shell runs under `set -e`).
    DaemonUnreachable,
    /// 8: too old: a doc revert past the window (restore a version
    /// instead), or a write whose base the daemon no longer keeps (read
    /// again) (`doc.sh`, `revert_too_old` and `base_expired`).
    TooOld,
    /// 26: curl's code when it can't read its input. Git for Windows'
    /// curl can't open bash's `/dev/fd/3`, which is why
    /// `cli_post_json_stdin` (`cli-tools-policy.sh`) goes another way
    /// there; a call whose last step is curl ends with it.
    CurlReadError,
    /// 130: stopped with Ctrl-C: `k2 compute` detaches from a job that
    /// keeps running (`compute-lib.sh`, `compute-run.sh`), an LLM login is
    /// cancelled (`llm.sh`). 128 plus SIGINT, as shells report it.
    Interrupted,
    /// 127: reserved for the `k2` name once it stops working (0.48), when
    /// calling the command `k2` only says to use `k5`. Nothing exits with
    /// it yet. It is what a shell gives for a command it can't find.
    K2Retired,
    /// 9009: Windows, and no Git Bash to run the shell CLI. `k2.cmd`
    /// exits 9009 there, the code `cmd.exe` gives for a program it can't
    /// find ([`crate::fallthrough::FrontError::NoGitBash`]).
    NoGitBash,
}

impl Exit {
    /// Every code, in order.
    pub const ALL: [Exit; 12] = [
        Exit::Ok,
        Exit::Error,
        Exit::Usage,
        Exit::Refused,
        Exit::NotFound,
        Exit::Conflict,
        Exit::DaemonUnreachable,
        Exit::TooOld,
        Exit::CurlReadError,
        Exit::K2Retired,
        Exit::Interrupted,
        Exit::NoGitBash,
    ];

    /// The number the process exits with.
    pub fn code(self) -> i32 {
        match self {
            Exit::Ok => 0,
            Exit::Error => 1,
            Exit::Usage => 2,
            Exit::Refused => 3,
            Exit::NotFound => 4,
            Exit::Conflict => 5,
            Exit::DaemonUnreachable => 7,
            Exit::TooOld => 8,
            Exit::CurlReadError => 26,
            Exit::K2Retired => 127,
            Exit::Interrupted => 130,
            Exit::NoGitBash => 9009,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_pinned() {
        let got: Vec<(Exit, i32)> = Exit::ALL.iter().map(|e| (*e, e.code())).collect();
        assert_eq!(
            got,
            vec![
                (Exit::Ok, 0),
                (Exit::Error, 1),
                (Exit::Usage, 2),
                (Exit::Refused, 3),
                (Exit::NotFound, 4),
                (Exit::Conflict, 5),
                (Exit::DaemonUnreachable, 7),
                (Exit::TooOld, 8),
                (Exit::CurlReadError, 26),
                (Exit::K2Retired, 127),
                (Exit::Interrupted, 130),
                (Exit::NoGitBash, 9009),
            ]
        );
    }

    #[test]
    fn no_two_share_a_number() {
        let mut codes: Vec<i32> = Exit::ALL.iter().map(|e| e.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Exit::ALL.len());
    }

    /// The codes `k2 study errors` lists are there, with the same meaning
    /// in the same place.
    #[test]
    fn the_study_page_s_codes_are_the_same() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/study-basics.sh");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let page = text
            .split_once("_study_page_errors() {")
            .unwrap_or_else(|| panic!("no _study_page_errors in {}", path.display()))
            .1;
        for (exit, words) in [
            (Exit::Ok, "ok."),
            (Exit::Error, "error (daemon down"),
            (Exit::Usage, "bad usage"),
            (Exit::Refused, "teaching refusal"),
            (Exit::NotFound, "not found"),
        ] {
            let line = format!("  {}  {words}", exit.code());
            assert!(page.lines().any(|l| l.starts_with(&line)), "no `{line}` in the errors page");
        }
    }
}
