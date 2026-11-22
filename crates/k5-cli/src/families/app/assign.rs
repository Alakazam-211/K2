//! `--assign <login>[,<login>…]` on every `k5 app` command, as the shell
//! CLI takes it (0.46.17, `cli/src/app-wallet.sh` `_app_take_assign` and
//! `cli/src/compute-lib.sh` `_compute_assign_path`).
//!
//! - [`take`] is `_app_take_assign`, the first thing `cmd_app` does: every
//!   `--assign <v>` and `--assign=<v>` before a bare `--` is taken out of
//!   the words (the last one stands), before the verb's own loop reads
//!   them. A `--` and every word after it stay as they are (`k5 app exec
//!   <app> -- cmd --assign x` keeps the command's words). A help word after
//!   `--assign` (`-h`, `--help`) is never a login: the flag goes and the
//!   help word stays, so the verb prints its own page. With no value, or
//!   an empty one, it is the usage line on stderr and exit 2, before
//!   anything is sent.
//! - `db connect` (and `database connect`) has its own `--assign`: the
//!   `--read-only` ask's body field, refused without `--read-only`. There
//!   the value goes back to the verb's words as `--assign <v>` (at the
//!   end), and no `?assign=` is added.
//! - [`path`] is `_compute_assign_path`: a POST to `/cli/app/…` carries
//!   the value as `?assign=` (`&assign=` when the path has a query
//!   already), quoted as `urlencode` quotes it. A GET and every other path
//!   are left alone. [`super::call::Ctx::send`] puts it on every request
//!   the App verbs send, `app exec`'s too (`_app_rt_post_file`).
//!
//! A held request's permission card then goes to those people; the daemon
//! refuses a name that can't approve before anything runs.

use super::Stop;
use crate::exit::Exit;
use crate::transport::encode::quote;

/// `--assign` with no word after it, or an empty one.
pub const USAGE: &str = "{\"error\":\"usage\",\"hint\":\"--assign takes logins, comma-separated (--assign ada or --assign ada,owner): who gets the card if this is held\"}";

/// `--assign=` with nothing after the `=`.
pub const USAGE_EQ: &str =
    "{\"error\":\"usage\",\"hint\":\"--assign takes logins, comma-separated (--assign ada)\"}";

/// What [`take`] left: the value (empty when none was given) and the
/// other words, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken<'a> {
    pub assign: String,
    pub words: Vec<&'a str>,
}

/// `_app_take_assign "$@"` on the words after `app`.
pub fn take<'a>(words: &[&'a str]) -> Result<Taken<'a>, Stop> {
    let mut assign: Option<&'a str> = None;
    let mut kept = Vec::with_capacity(words.len() + 2);
    let mut i = 0;
    while let Some(&w) = words.get(i) {
        match w {
            "--" => {
                kept.extend_from_slice(&words[i..]);
                break;
            }
            "--assign" => match words.get(i + 1) {
                // Help wins: the flag goes, the help word stays.
                Some(&h @ ("-h" | "--help")) => {
                    kept.push(h);
                    i += 2;
                    continue;
                }
                Some(&v) if !v.is_empty() => {
                    assign = Some(v);
                    i += 2;
                    continue;
                }
                _ => return Err(usage(USAGE)),
            },
            _ => match w.strip_prefix("--assign=") {
                Some("") => return Err(usage(USAGE_EQ)),
                Some(v) => assign = Some(v),
                None => kept.push(w),
            },
        }
        i += 1;
    }
    // `db connect`'s own --assign goes back to the verb.
    if matches!(kept.first(), Some(&("db" | "database"))) && kept.get(1) == Some(&"connect") {
        if let Some(v) = assign.take() {
            kept.push("--assign");
            kept.push(v);
        }
    }
    Ok(Taken {
        assign: assign.unwrap_or_default().to_string(),
        words: kept,
    })
}

fn usage(line: &str) -> Stop {
    Stop {
        line: line.to_string(),
        exit: Exit::Usage,
    }
}

/// `_compute_assign_path <METHOD> <path>`.
pub fn path(post: bool, path: &str, assign: &str) -> String {
    if !post || assign.is_empty() || !path.starts_with("/cli/app/") {
        return path.to_string();
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}assign={}", quote(assign))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken<'a>(assign: &str, words: &[&'a str]) -> Result<Taken<'a>, Stop> {
        Ok(Taken {
            assign: assign.to_string(),
            words: words.to_vec(),
        })
    }

    #[test]
    fn take_is_the_shell_s_pre_scan() {
        assert_eq!(take(&["list"]), taken("", &["list"]));
        assert_eq!(
            take(&["egress", "add", "a", "h", "--assign", "ada"]),
            taken("ada", &["egress", "add", "a", "h"])
        );
        assert_eq!(
            take(&["--assign", "ada,owner", "egress", "add"]),
            taken("ada,owner", &["egress", "add"])
        );
        assert_eq!(
            take(&["egress", "--assign=bo", "add"]),
            taken("bo", &["egress", "add"])
        );
        // The last one stands; a value may look like a flag.
        assert_eq!(
            take(&["deny", "--assign", "a", "--assign", "-x", "w"]),
            taken("-x", &["deny", "w"])
        );
        // A help word is never a login: the flag goes, the word stays.
        assert_eq!(
            take(&["deny", "--assign", "a", "--assign", "-h", "w"]),
            taken("a", &["deny", "-h", "w"])
        );
        assert_eq!(
            take(&["data", "put", "s", "--assign", "--help"]),
            taken("", &["data", "put", "s", "--help"])
        );
        // `--assign=` takes what follows the `=`, a help word too.
        assert_eq!(
            take(&["deny", "--assign=-h", "w"]),
            taken("-h", &["deny", "w"])
        );
        // After `--`, the words are the command's own.
        assert_eq!(
            take(&["exec", "a", "--", "echo", "--assign", "ada"]),
            taken("", &["exec", "a", "--", "echo", "--assign", "ada"])
        );
        assert_eq!(
            take(&["exec", "a", "--assign", "x", "--", "--assign="]),
            taken("x", &["exec", "a", "--", "--assign="])
        );
        // `--assignee` is no --assign.
        assert_eq!(take(&["x", "--assignee"]), taken("", &["x", "--assignee"]));
        // `db connect` keeps its own: the value goes back to its words.
        assert_eq!(
            take(&["db", "connect", "j", "--read-only", "--assign", "ada"]),
            taken("", &["db", "connect", "j", "--read-only", "--assign", "ada"])
        );
        assert_eq!(
            take(&["--assign", "ada", "database", "connect", "j", "--new"]),
            taken("", &["database", "connect", "j", "--new", "--assign", "ada"])
        );
        assert_eq!(
            take(&["db", "connect", "j", "--assign=ada", "--read-only"]),
            taken("", &["db", "connect", "j", "--read-only", "--assign", "ada"])
        );
        // Help still wins there, and the other db verbs are the requests'.
        assert_eq!(
            take(&["db", "connect", "j", "--assign", "-h"]),
            taken("", &["db", "connect", "j", "-h"])
        );
        assert_eq!(
            take(&["db", "check", "j", "--assign", "ada"]),
            taken("ada", &["db", "check", "j"])
        );
    }

    #[test]
    fn take_with_no_value_is_usage() {
        for words in [&["x", "--assign"][..], &["x", "--assign", ""]] {
            assert_eq!(take(words), Err(usage(USAGE)), "{words:?}");
        }
        assert_eq!(take(&["x", "--assign="]), Err(usage(USAGE_EQ)));
        // The first bad one stops it, even after a good one.
        assert_eq!(take(&["--assign", "a", "x", "--assign"]), Err(usage(USAGE)));
    }

    /// The skin family's one request to `/cli/app/…` is `user chat`'s,
    /// which `app::run` hands to the shell when `--assign` is given. A
    /// new one there needs the same.
    #[test]
    fn the_skin_family_posts_to_one_app_route() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/families");
        let mut hits = Vec::new();
        for f in ["skin.rs", "skin_token.rs"]
            .iter()
            .map(|f| format!("{dir}/{f}"))
            .chain(
                std::fs::read_dir(format!("{dir}/skin"))
                    .unwrap()
                    .map(|e| e.unwrap().path().display().to_string()),
            )
        {
            if f.ends_with("tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for l in text.lines().filter(|l| l.contains("\"/cli/app/")) {
                hits.push(format!("{f}: {}", l.trim()));
            }
        }
        assert_eq!(hits.len(), 1, "{hits:#?}");
        assert!(hits[0].contains("user_chat.rs"), "{hits:#?}");
    }

    #[test]
    fn path_is_compute_assign_path() {
        assert_eq!(path(true, "/cli/app/deny", ""), "/cli/app/deny");
        assert_eq!(
            path(true, "/cli/app/deny", "ada"),
            "/cli/app/deny?assign=ada"
        );
        assert_eq!(
            path(true, "/cli/app/x?a=1", "ada,owner é"),
            "/cli/app/x?a=1&assign=ada%2Cowner%20%C3%A9"
        );
        assert_eq!(
            path(false, "/cli/app/approvals", "ada"),
            "/cli/app/approvals"
        );
        assert_eq!(path(true, "/cli/skin/x", "ada"), "/cli/skin/x");
        assert_eq!(path(true, "/cli/app", "ada"), "/cli/app");
    }
}
