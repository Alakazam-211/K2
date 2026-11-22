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
    /// 6: the docs store failed. The daemon sends it and the shell CLI
    /// passes it on; `k2 doc --help` lists it (`doc.sh`).
    DocsStoreFailed,
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
    /// 124: `k2 compute wait --timeout` passed before the jobs ended
    /// (`still_running`); the jobs are left alone. It is the code
    /// coreutils' `timeout` uses. The shell passes it on from a variable
    /// (`compute-jobs.sh`, `_CP_DEADLINE_RC` and `K2_CP_LIMIT_RC`);
    /// `--max-wait`, the older spelling, stays 3.
    WaitTimedOut,
    /// 130: stopped with Ctrl-C: `k2 compute` detaches from a job that
    /// keeps running (`compute-lib.sh`, `compute-run.sh`), an LLM login is
    /// cancelled (`llm.sh`). 128 plus SIGINT, as shells report it.
    Interrupted,
    /// 127: called by an alias in mode `tombstone` (the brand file's), the
    /// command runs nothing and only says which name to use
    /// ([`crate::notice`]). It is what a shell gives for a command it can't
    /// find. No alias of the default brand has that mode.
    Tombstone,
    /// 69: macOS or Linux, a command the binary hands to the shell CLI,
    /// and no Python 3 for the shell to run
    /// ([`crate::fallthrough::FrontError::NoPython`]); its one line says
    /// what is missing and how to get it. sysexits' `EX_UNAVAILABLE`, "a
    /// support program does not exist". The binary's own: the shell CLI
    /// never ends with it (`k2 study errors` lists it).
    NoPython,
    /// 9009: Windows, and no Git Bash to run the shell CLI. `k2.cmd`
    /// exits 9009 there, the code `cmd.exe` gives for a program it can't
    /// find ([`crate::fallthrough::FrontError::NoGitBash`]).
    NoGitBash,
    /// Any other code curl ends with, passed on as it is: a call whose
    /// last step is curl ends with it, as the shell runs under `set -e`
    /// (28 no answer in time, 52 an empty reply, 55 and 56 a write or a
    /// read that failed, 18 an answer cut short), and the two codes
    /// `_cli_curl` gives a failure that is this machine's: 75, a connect
    /// the OS refused ([`crate::transport::client::LOCAL_TRANSPORT_DENIED`]),
    /// and 76, a failure while TMPDIR can't be used
    /// ([`crate::transport::client::TMPDIR_UNWRITABLE`]). Not in
    /// [`Exit::ALL`]: the number is the outcome's, not a code of the
    /// shell's own. [`Exit::from_curl`] makes one.
    Curl(i32),
}

impl Exit {
    /// Every code, in order.
    pub const ALL: [Exit; 15] = [
        Exit::Ok,
        Exit::Error,
        Exit::Usage,
        Exit::Refused,
        Exit::NotFound,
        Exit::Conflict,
        Exit::DocsStoreFailed,
        Exit::DaemonUnreachable,
        Exit::TooOld,
        Exit::CurlReadError,
        Exit::NoPython,
        Exit::WaitTimedOut,
        Exit::Tombstone,
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
            Exit::DocsStoreFailed => 6,
            Exit::DaemonUnreachable => 7,
            Exit::TooOld => 8,
            Exit::CurlReadError => 26,
            Exit::NoPython => 69,
            Exit::WaitTimedOut => 124,
            Exit::Tombstone => 127,
            Exit::Interrupted => 130,
            Exit::NoGitBash => 9009,
            Exit::Curl(code) => code,
        }
    }

    /// The exit for curl's code `code` (or `_cli_curl`'s 75 and 76) passed on:
    /// the named code when it has a name, else [`Exit::Curl`].
    pub fn from_curl(code: i32) -> Exit {
        Exit::ALL
            .iter()
            .copied()
            .find(|e| e.code() == code)
            .unwrap_or(Exit::Curl(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curl_code_is_passed_on() {
        assert_eq!(Exit::from_curl(7), Exit::DaemonUnreachable);
        assert_eq!(Exit::from_curl(3), Exit::Refused);
        assert_eq!(Exit::from_curl(1), Exit::Error);
        for code in [18, 28, 52, 55, 56, 75, 76] {
            assert_eq!(Exit::from_curl(code), Exit::Curl(code));
            assert_eq!(Exit::from_curl(code).code(), code);
        }
    }

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
                (Exit::DocsStoreFailed, 6),
                (Exit::DaemonUnreachable, 7),
                (Exit::TooOld, 8),
                (Exit::CurlReadError, 26),
                (Exit::NoPython, 69),
                (Exit::WaitTimedOut, 124),
                (Exit::Tombstone, 127),
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
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/text/study/errors.txt");
        let page = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (exit, words) in [
            (Exit::Ok, "ok."),
            (Exit::Error, "error (daemon down"),
            (Exit::Usage, "bad usage"),
            (Exit::Refused, "teaching refusal"),
            (Exit::NotFound, "not found"),
            (Exit::NoPython, "a program the command needs is missing"),
        ] {
            let line = format!("  {}  {words}", exit.code());
            assert!(
                page.lines().any(|l| l.starts_with(&line)),
                "no `{line}` in the errors page"
            );
        }
    }
}
