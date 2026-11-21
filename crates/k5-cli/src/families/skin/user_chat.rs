//! `user chat <username> [member|guest|none] [--app <app>] [--json]`, as
//! `cmd_skin`'s `user chat` arm answers it (`cli/src/app-wallet.sh`; Sew
//! for Apps A2): a person's chat (their Sew account). With no scope, one
//! `_compute_call GET /cli/app/guest/chat`; with one, a POST of
//! `_compute_json`'s body. `--json` prints the answer as it came, else
//! the shell's Python ([`lines`]).
//!
//! The words, as the arm's loop reads them: `--app` takes the next word
//! (as the last word, `_need_value`'s line, exit 2);
//! `--json`; any other flag is `Unknown flag: <flag>`, a third word
//! `Unexpected: <word>`, no username the usage line, a scope that is not
//! `member`, `guest` or `none` its line: each exit 2, nothing sent.

use crate::exit::Exit;
use crate::families::app::call::{Cj, Ctx};
use crate::families::app::py::{self, Out};
use crate::families::app::Answer;
use crate::families::Raise;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The arm's usage line (exit 2).
pub const USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " app user chat <username> [member|guest|none] [--app <app>] [--json]"
);
/// A scope it doesn't take (exit 2).
pub const SCOPE: &str = "scope is member, guest or none";
/// The route.
pub const ROUTE: &str = "/cli/app/guest/chat";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub username: String,
    /// Empty: the read.
    pub scope: String,
    pub app: String,
    pub json: bool,
}

/// Why the words send nothing: the line and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub line: String,
    pub exit: Exit,
}

fn usage(line: impl Into<String>) -> Stop {
    Stop {
        line: line.into(),
        exit: Exit::Usage,
    }
}

/// The words after `user chat`. `None`: the help (the front's, which
/// never hands it to a family).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let mut a = Ask {
        username: String::new(),
        scope: String::new(),
        app: String::new(),
        json: false,
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--app" => {
                // `_need_value "$@"; app="${2:-}"; shift 2`.
                let Some(v) = words.get(i + 1) else {
                    return Some(Err(usage(crate::out::need_value(w))));
                };
                a.app = (*v).to_string();
                i += 2;
                continue;
            }
            "--json" => a.json = true,
            "--help" | "-h" => return None,
            _ if w.starts_with('-') => return Some(Err(usage(format!("Unknown flag: {w}")))),
            _ if a.username.is_empty() => a.username = w.to_string(),
            _ if a.scope.is_empty() => a.scope = w.to_string(),
            _ => return Some(Err(usage(format!("Unexpected: {w}")))),
        }
        i += 1;
    }
    if a.username.is_empty() {
        return Some(Err(usage(USAGE)));
    }
    if !a.scope.is_empty() && !matches!(a.scope.as_str(), "member" | "guest" | "none") {
        return Some(Err(usage(SCOPE)));
    }
    Some(Ok(a))
}

/// The request, then the answer: as it came with `--json`, else
/// [`lines`].
pub fn run(ctx: &mut Ctx, a: &Ask) -> Answer {
    let body = if a.scope.is_empty() {
        let mut q = format!("username={}", quote(&a.username));
        if !a.app.is_empty() {
            q.push_str(&format!("&app={}", quote(&a.app)));
        }
        ctx.call(false, ROUTE, &q)?
    } else {
        let b = Cj::new()
            .s("username", &a.username)
            .s("scope", &a.scope)
            .s("app", &a.app)
            .body();
        ctx.call(true, ROUTE, &b)?
    };
    Ok(crate::families::app::access::answer(&body, a.json, lines))
}

/// The arm's Python, line for line (its `-c` text starts with a newline,
/// so `import` is line 2).
pub fn lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let scope = py::or(py::dget(&v, "scope", 4)?, py::text("no chat"));
    let state =
        if py::truthy(&py::dget(&v, "enabled", 5)?) || !py::truthy(&py::dget(&v, "scope", 5)?) {
            ""
        } else {
            "  [switched off by the owner]"
        };
    let managed = py::or(py::dget(&v, "managedBy", 6)?, py::text(""));
    let by = py::word_for(
        &[
            ("app", "  (given by the App)"),
            ("owner", "  (given by the owner)"),
        ],
        &managed,
        py::text(""),
        6,
    )?;
    let mut line = format!(
        "{}: {}{}{}",
        py::s(&py::dget(&v, "username", 7)?),
        py::s(&scope),
        py::s(&by),
        state
    );
    if py::truthy(&py::dget(&v, "app", 8)?) {
        let app = py::idx(&v, "app", 9)?;
        let members = py::dget(&v, "members", 9)?;
        let max = if py::dget(&v, "maxMembers", 9)? != Value::Null {
            py::s(&py::dget(&v, "maxMembers", 9)?)
        } else {
            "?".to_string()
        };
        line.push_str(&format!(
            "  on {}, {} of {} people",
            py::s(&app),
            py::s(&members),
            max
        ));
    }
    let has_changed =
        matches!(&v, Value::Object(pairs) if crate::families::get(pairs, "changed").is_some());
    if has_changed && !py::truthy(&py::dget(&v, "changed", 10)?) {
        line.push_str("  (unchanged)");
    }
    o.line(&line);
    o.line(&format!(
        "  chat id: k2:principal:{}",
        py::s(&py::dget(&v, "principalId", 13)?)
    ));
    if py::truthy(&py::dget(&v, "warning", 14)?) {
        let w = py::idx(&v, "warning", 15)?;
        o.line(&py::concat("  note: ", &w, 15)?);
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(words: &[&str]) -> Ask {
        parse(words).expect("no help").expect("words it takes")
    }

    fn stop(words: &[&str]) -> Stop {
        parse(words).expect("no help").expect_err("a stop")
    }

    #[test]
    fn the_words_as_the_arm_reads_them() {
        assert_eq!(
            ok(&["bob"]),
            Ask {
                username: "bob".into(),
                scope: String::new(),
                app: String::new(),
                json: false,
            }
        );
        assert_eq!(
            ok(&["--app", "acme", "bob", "guest", "--json"]),
            Ask {
                username: "bob".into(),
                scope: "guest".into(),
                app: "acme".into(),
                json: true,
            }
        );
        assert_eq!(stop(&[]), usage(USAGE));
        assert_eq!(stop(&["--json"]), usage(USAGE));
        assert_eq!(stop(&["bob", "admin"]), usage(SCOPE));
        assert_eq!(stop(&["bob", "guest", "x"]), usage("Unexpected: x"));
        assert_eq!(stop(&["bob", "-q"]), usage("Unknown flag: -q"));
        assert_eq!(stop(&["bob", "--app"]), usage("Error: --app takes a value"));
        assert_eq!(parse(&["bob", "-h"]), None);
    }

    #[test]
    fn the_usage_line_is_the_shell_s() {
        let src = include_str!("../../../../../cli/src/app-wallet.sh");
        assert!(src.contains(
            "[[ -z \"$username\" ]] && { echo \"Usage: k2 app user chat <username> [member|guest|none] [--app <app>] [--json]\" >&2; exit 2; }"
        ));
        assert!(src.contains("*) echo \"scope is member, guest or none\" >&2; exit 2 ;;"));
    }
}
