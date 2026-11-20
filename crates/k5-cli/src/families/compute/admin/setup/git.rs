//! `compute git-access [list|setup|grant|revoke] …`, as
//! `cmd_compute_git_access` answers it (`cli/src/compute-access.sh`):
//!
//! - `list` (or no verb): one GET of `/cli/compute/git-access`;
//! - `setup --github-app ID --key-file F | --github-token-file F |
//!   --github-token-stdin | --clear`: one POST of `…/setup` with the key
//!   or the token read here (from the file, or all of stdin), never from
//!   argv or the environment;
//! - `grant --github R[,…] [--nodes …] [--shared]` and `revoke --github
//!   R[,…] | --all`: one POST each.
//!
//! Every request goes the cell socket first. Then the answer as it came
//! (`--json`) or in words.

use super::super::super::lines::py_line;
use super::super::super::pyrt::{self as p, s};
use super::super::super::writes::{dumps, load, object, print_body, stopped, str_value, Cp, Words};
use super::{at, ACCESS_SRC};
use crate::exit::Exit;
use crate::families::{py_space, Raise, Shown};
use crate::out::{self, json::Value};
use crate::prompt::Secret;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

pub const LIST: &str = "/cli/compute/git-access";
pub const SETUP: &str = "/cli/compute/git-access/setup";
pub const GRANT: &str = "/cli/compute/git-access/grant";
pub const REVOKE: &str = "/cli/compute/git-access/revoke";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute git-access [setup|grant|revoke] … (",
    cli_name!(),
    " compute git-access --help)"
);
pub const APP_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute git-access setup --github-app <app id> --key-file <app.pem>"
);
pub const SETUP_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute git-access setup --github-app <app id> --key-file <app.pem> | --github-token-file <file> | --github-token-stdin | --clear"
);
pub const SETUP_USAGE_2: &str = concat!(
    "  (a person's, from a terminal outside ",
    product_name!(),
    "; an agent asks: ",
    cli_name!(),
    " permission request compute.git-access-setup --why \"…\")"
);
pub const GRANT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute git-access grant --github <owner>/<repo>[,…] [--nodes all|<node>,…] [--shared] [--workspace WS]"
);
pub const REVOKE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute git-access revoke --github <owner>/<repo>[,…] [--nodes …] | --all [--workspace WS]"
);

macro_rules! line {
    ($m:literal) => {
        const {
            py_line(
                ACCESS_SRC,
                "_compute_call POST /cli/compute/git-access/revoke",
                $m,
            )
        }
    };
}

/// The line of the key file's Python and of the token's.
const APP_LINE: u32 = const {
    py_line(
        ACCESS_SRC,
        "elif [ -n \"$app_id\" ]; then",
        "print(json.dumps({\"app_id\"",
    )
};
const TOKEN_LINE: u32 = const {
    py_line(
        ACCESS_SRC,
        "elif [ -n \"$token_file\" ] || [ -n \"$token_stdin\" ]; then",
        "t = (open(f)",
    )
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verb {
    #[default]
    List,
    Setup,
    Grant,
    Revoke,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub verb: Verb,
    pub github: String,
    pub nodes: String,
    pub workspace: String,
    pub shared: bool,
    pub all: bool,
    pub clear: bool,
    pub app_id: String,
    pub key_file: String,
    pub token_file: String,
    pub token_stdin: bool,
    pub json: bool,
}

fn append(x: &mut String, w: &str) {
    if !x.is_empty() {
        x.push(',');
    }
    x.push_str(w);
}

/// The words after `git-access`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let verb = match words.first() {
        Some(&"setup") => Some(Verb::Setup),
        Some(&"grant") => Some(Verb::Grant),
        Some(&"revoke") => Some(Verb::Revoke),
        Some(&"list") => Some(Verb::List),
        _ => None,
    };
    let mut i = usize::from(verb.is_some());
    a.verb = verb.unwrap_or_default();
    while i < words.len() {
        let w = words[i];
        match w {
            "--github" | "--nodes" | "--workspace" | "--github-app" | "--key-file"
            | "--github-token-file" => {
                let v = match words.get(i + 1) {
                    Some(v) if !v.is_empty() => *v,
                    _ => return Words::Usage(format!("{USAGE} ({w} needs a value)")),
                };
                match w {
                    "--github" => append(&mut a.github, v),
                    "--nodes" => append(&mut a.nodes, v),
                    "--workspace" => a.workspace = v.to_string(),
                    "--github-app" => a.app_id = v.to_string(),
                    "--key-file" => a.key_file = v.to_string(),
                    _ => a.token_file = v.to_string(),
                }
                i += 2;
                continue;
            }
            "--github-token-stdin" => a.token_stdin = true,
            "--shared" => a.shared = true,
            "--read-only" => {}
            "--all" => a.all = true,
            "--clear" => a.clear = true,
            "--json" => a.json = true,
            w => return Words::Usage(format!("{USAGE} (unknown argument '{w}')")),
        }
        i += 1;
    }
    Words::Ask(a)
}

/// `_compute_json`'s body for grant and revoke.
fn change_body(a: &Ask, ws: &str) -> String {
    let mut pairs = vec![];
    for (k, v) in [("workspace", ws), ("github", &a.github), ("nodes", &a.nodes)] {
        if !v.is_empty() {
            pairs.push((k, str_value(v)));
        }
    }
    match a.verb {
        Verb::Grant if a.shared => pairs.push(("shared", Value::Bool(true))),
        Verb::Revoke if a.all => pairs.push(("all", Value::Bool(true))),
        _ => {}
    }
    dumps(&object(pairs))
}

/// What stops a request before it is sent: its lines and exit.
enum Stop {
    Usage(Vec<&'static str>),
    Line(String),
    Raised(Raise),
}

/// `setup`'s body, read from the key file, the token file or stdin.
fn setup_body(a: &Ask) -> Result<String, Stop> {
    let cant_read = |f: &str| {
        Stop::Line(format!(
            "{} compute git-access: can't read {f}",
            k2_client::brand::CLI_NAME
        ))
    };
    if a.clear {
        return Ok(r#"{"clear":true}"#.to_string());
    }
    if !a.app_id.is_empty() {
        if a.key_file.is_empty() {
            return Err(Stop::Usage(vec![APP_USAGE]));
        }
        if !crate::families::browser::readable(&a.key_file) {
            return Err(cant_read(&a.key_file));
        }
        let pem = read_text_file(&a.key_file).map_err(|e| Stop::Raised(Raise::at(APP_LINE, e)))?;
        return Ok(dumps(&object(vec![
            ("app_id", str_value(&a.app_id)),
            ("key_pem", Value::Str(pem.into())),
        ])));
    }
    if !a.token_file.is_empty() || a.token_stdin {
        let token = if !a.token_file.is_empty() {
            if !crate::families::browser::readable(&a.token_file) {
                return Err(cant_read(&a.token_file));
            }
            let t = read_text_file(&a.token_file)
                .map_err(|e| Stop::Raised(Raise::at(TOKEN_LINE, e)))?;
            Value::Str(t.trim_matches(py_space).into())
        } else {
            stdin_text()
        };
        return Ok(dumps(&object(vec![("token", token)])));
    }
    Err(Stop::Usage(vec![SETUP_USAGE, SETUP_USAGE_2]))
}

/// `open(f).read()`: UTF-8 (strict), universal newlines.
pub fn read_text_file(path: &str) -> Result<String, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => Secret::new(b),
        Err(e) => {
            return Err(match e.raw_os_error() {
                Some(21) => format!("IsADirectoryError: [Errno 21] Is a directory: '{path}'"),
                Some(n) => format!("OSError: [Errno {n}] {e}: '{path}'"),
                None => format!("OSError: {e}"),
            })
        }
    };
    let text = std::str::from_utf8(bytes.expose()).map_err(|e| {
        format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
            bytes.expose()[e.valid_up_to()],
            e.valid_up_to()
        )
    })?;
    Ok(newlines(text))
}

/// Universal newlines: `\r\n` and a lone `\r` read as `\n`.
fn newlines(t: &str) -> String {
    t.replace("\r\n", "\n").replace('\r', "\n")
}

/// `sys.stdin.read().strip()`: all of stdin as Python reads it (UTF-8, a
/// byte that isn't kept as its surrogate escape; universal newlines),
/// with Python's spaces taken off both ends.
fn stdin_text() -> Value {
    use std::io::Read;
    let mut raw = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut raw);
    let raw = Secret::new(raw);
    let mut b = Vec::with_capacity(raw.expose().len());
    let mut i = 0;
    let r = raw.expose();
    while i < r.len() {
        if r[i] == b'\r' {
            b.push(b'\n');
            if r.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
        } else {
            b.push(r[i]);
        }
        i += 1;
    }
    let b = Secret::new(b);
    if let Ok(t) = std::str::from_utf8(b.expose()) {
        return Value::Str(t.trim_matches(py_space).into());
    }
    // Spaces off the ends; an undecodable byte is no space.
    enum U {
        C(char),
        B(u8),
    }
    let mut units = vec![];
    for chunk in b.expose().utf8_chunks() {
        units.extend(chunk.valid().chars().map(U::C));
        units.extend(chunk.invalid().iter().copied().map(U::B));
    }
    let space = |u: &U| matches!(u, U::C(c) if py_space(*c));
    let start = units.iter().position(|u| !space(u)).unwrap_or(units.len());
    let end = units.iter().rposition(|u| !space(u)).map_or(start, |e| e + 1);
    let mut lit = String::from("\"");
    for u in &units[start..end] {
        match u {
            U::C('"') => lit.push_str("\\\""),
            U::C('\\') => lit.push_str("\\\\"),
            U::C(c) if (*c as u32) < 0x20 => lit.push_str(&format!("\\u{:04x}", *c as u32)),
            U::C(c) => lit.push(*c),
            U::B(x) => lit.push_str(&format!("\\udc{x:02x}")),
        }
    }
    lit.push('"');
    crate::out::json::parse(&lit).unwrap_or(Value::Null)
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let ws = cp.ws(&a.workspace);
    let answer = match a.verb {
        Verb::List => {
            let q = if ws.is_empty() {
                String::new()
            } else {
                format!("workspace={}", quote(&ws))
            };
            cp.call(false, LIST, &q, false)
        }
        Verb::Setup => match setup_body(a) {
            Ok(body) => cp.call(true, SETUP, &body, false),
            Err(Stop::Usage(lines)) => {
                for l in lines {
                    out::stderr_line(l);
                }
                return Exit::Usage;
            }
            Err(Stop::Line(l)) => {
                out::stderr_line(&l);
                return Exit::Usage;
            }
            Err(Stop::Raised(r)) => return stopped(String::new(), &r).print(),
        },
        Verb::Grant if a.github.is_empty() => {
            out::stderr_line(GRANT_USAGE);
            return Exit::Usage;
        }
        Verb::Revoke if a.github.is_empty() && !a.all => {
            out::stderr_line(REVOKE_USAGE);
            return Exit::Usage;
        }
        Verb::Grant => cp.call(true, GRANT, &change_body(a, &ws), false),
        Verb::Revoke => cp.call(true, REVOKE, &change_body(a, &ws), false),
    };
    let answer = match answer {
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
    match read(body, &mut out) {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

/// `"text" + x`: `x` must be a string.
fn plus(text: &str, x: &Value) -> Result<String, String> {
    p::add(&s(text), x).map(|v| p::str_of(&v))
}

fn read(body: &[u8], out: &mut String) -> Result<(), Raise> {
    let cli = k2_client::brand::CLI_NAME;
    let v = load(body, line!("v = json.load"))?;
    let l = line!("if \"workspace\" not in v");
    if !p::has(&v, "workspace").map_err(at(l))? {
        let l = line!("    c = v.get(\"credential\")");
        let c = p::get(&v, "credential").map_err(at(l))?;
        if p::is_none(&c) {
            out.push_str("GitHub credential cleared: every later ask for a git token is refused; grants stay\n");
        } else {
            let l = line!("print(\"GitHub credential set");
            let kind = p::get(&c, "kind").map_err(at(l))?;
            let what = if p::eq_s(&kind, "app") {
                let id = p::get(&c, "appId").map_err(at(l))?;
                let head = format!("App {}", p::str_of(&id));
                let app = p::get(&c, "app").map_err(at(l))?;
                let tail = if p::truthy(&app) {
                    let name = p::idx(&c, "app").map_err(at(l))?;
                    let t = plus(" (", &name).map_err(at(l))?;
                    format!("{t})")
                } else {
                    String::new()
                };
                format!("{head}{tail}")
            } else {
                "fine-grained token".to_string()
            };
            out.push_str(&format!("GitHub credential set: {what}\n"));
        }
        let l = line!("    if v.get(\"note\")");
        if p::truthy(&p::get(&v, "note").map_err(at(l))?) {
            let l = line!("        print(\"  \" + v[\"note\"])");
            let n = p::idx(&v, "note").map_err(at(l))?;
            out.push_str(&plus("  ", &n).map_err(at(l))?);
            out.push('\n');
        }
        return Ok(());
    }
    let l = line!("c = v.get(\"credential\")\nif c:");
    let c = p::get(&v, "credential").map_err(at(l))?;
    if p::truthy(&c) {
        let l = line!("print(\"credential: %s\"");
        let kind = p::get(&c, "kind").map_err(at(l))?;
        let what = if p::eq_s(&kind, "app") {
            format!("GitHub App {}", p::str_of(&p::get(&c, "appId").map_err(at(l))?))
        } else {
            "a fine-grained token (nodes get the token itself)".to_string()
        };
        out.push_str(&format!("credential: {what}\n"));
    } else {
        out.push_str(&format!("credential: none (a person runs: {cli} compute git-access setup --github-app <id> --key-file <app.pem>;\n"));
        out.push_str(&format!("  an agent asks: {cli} permission request compute.git-access-setup --why \"<what you need it for>\")\n"));
    }
    let l = line!("for w in v.get(\"warnings\")");
    let ws = p::get(&v, "warnings")
        .and_then(p::iter_or_empty)
        .map_err(at(l))?;
    for w in &ws {
        let l = line!("print(\"warning: \" + w)");
        out.push_str(&plus("warning: ", w).map_err(at(l))?);
        out.push('\n');
    }
    let l = line!("if v.get(\"granted\")");
    if p::truthy(&p::get(&v, "granted").map_err(at(l))?) {
        let l = line!("print(\"granted: %s\"");
        let g = p::idx(&v, "granted").map_err(at(l))?;
        let j = p::join(", ", &g).map_err(at(l))?;
        out.push_str(&format!("granted: {j}\n"));
    }
    let l = line!("if v.get(\"revoked\")");
    if p::truthy(&p::get(&v, "revoked").map_err(at(l))?) {
        let l = line!("print(\"revoked: %s");
        let r = p::idx(&v, "revoked").map_err(at(l))?;
        let j = p::join(", ", &r).map_err(at(l))?;
        let removed = p::get_or(&v, "removed", p::int(0)).map_err(at(l))?;
        let one = p::get(&v, "removed").map_err(at(l))?;
        let plural = if p::eq(&one, &p::int(1)) { "" } else { "s" };
        let text = p::pct(
            "revoked: %s (%d grant%s removed)",
            &[s(&j), removed, s(plural)],
        )
        .map_err(at(l))?;
        out.push_str(&text);
        out.push('\n');
        let l = line!("    if v.get(\"tokensDeleted\")");
        if p::truthy(&p::get(&v, "tokensDeleted").map_err(at(l))?) {
            let l = line!("print(\"  %d token(s)");
            let t = p::idx(&v, "tokensDeleted").map_err(at(l))?;
            let text = p::pct(
                "  %d token(s) already handed out were deleted at GitHub",
                &[t],
            )
            .map_err(at(l))?;
            out.push_str(&text);
            out.push('\n');
        }
        let l = line!("for j in v.get(\"runningJobs\")");
        let jobs = p::get(&v, "runningJobs")
            .and_then(p::iter_or_empty)
            .map_err(at(l))?;
        for j in &jobs {
            let l = line!("print(\"  running job %s");
            let job = p::idx(j, "job").map_err(at(l))?;
            let repo = p::idx(j, "repo").map_err(at(l))?;
            let text = p::pct(
                "  running job %s had a token for %s: it keeps running; its next git call for it fails",
                &[job, repo],
            )
            .map_err(at(l))?;
            out.push_str(&text);
            out.push('\n');
        }
        let l = line!("    if v.get(\"note\"):\n        print(\"  \" + v[\"note\"])\ngs");
        if p::truthy(&p::get(&v, "note").map_err(at(l))?) {
            let l = l + 1;
            let n = p::idx(&v, "note").map_err(at(l))?;
            out.push_str(&plus("  ", &n).map_err(at(l))?);
            out.push('\n');
        }
    }
    let l = line!("gs = v.get(\"grants\")");
    let gs = p::get(&v, "grants").map_err(at(l))?;
    if p::truthy(&gs) {
        let l = line!("print(\"%s: jobs may read\"");
        let w = p::idx(&v, "workspace").map_err(at(l))?;
        out.push_str(&p::pct("%s: jobs may read", &[w]).map_err(at(l))?);
        out.push('\n');
        let l = line!("for g in gs");
        for g in p::iter(&gs).map_err(at(l))? {
            let l = line!("print(\"  %-40s nodes");
            let repo = p::idx(&g, "repo").map_err(at(l))?;
            let nodes = p::idx(&g, "nodes").map_err(at(l))?;
            let shared = p::get(&g, "shared").map_err(at(l))?;
            let tail = if p::truthy(&shared) {
                " (shared: another server owns it)"
            } else {
                ""
            };
            let text = p::pct("  %-40s nodes: %s%s", &[repo, nodes, s(tail)]).map_err(at(l))?;
            out.push_str(&text);
            out.push('\n');
        }
    } else {
        let l = line!("print(\"%s: no git access");
        let w = p::idx(&v, "workspace").map_err(at(l))?;
        let text = p::pct(
            &format!("%s: no git access ({cli} compute git-access grant --github <owner>/<repo>)"),
            &[w],
        )
        .map_err(at(l))?;
        out.push_str(&text);
        out.push('\n');
    }
    let l = line!("if v.get(\"tokenNote\")");
    if p::truthy(&p::get(&v, "tokenNote").map_err(at(l))?) {
        let l = line!("print(\"note: \" + v[\"tokenNote\"])");
        let n = p::idx(&v, "tokenNote").map_err(at(l))?;
        out.push_str(&plus("note: ", &n).map_err(at(l))?);
        out.push('\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        let Words::Ask(a) = parse(&["grant", "--github", "o/r", "--github", "o/s", "--shared"])
        else {
            panic!("not an ask");
        };
        assert_eq!((a.verb, a.github.as_str(), a.shared), (Verb::Grant, "o/r,o/s", true));
        let Words::Ask(a) = parse(&["--json"]) else {
            panic!("not an ask");
        };
        assert_eq!((a.verb, a.json), (Verb::List, true));
        assert_eq!(
            parse(&["nope"]),
            Words::Usage(format!("{USAGE} (unknown argument 'nope')"))
        );
        assert_eq!(
            parse(&["setup", "--key-file"]),
            Words::Usage(format!("{USAGE} (--key-file needs a value)"))
        );
        assert_eq!(
            change_body(
                &Ask {
                    verb: Verb::Revoke,
                    all: true,
                    shared: true,
                    ..Ask::default()
                },
                "acme"
            ),
            r#"{"workspace": "acme", "all": true}"#
        );
    }

    #[test]
    fn the_lines() {
        assert_eq!(line!("v = json.load"), 3);
        assert_eq!(line!("gs = v.get(\"grants\")"), 31);
        assert_eq!(
            line!("    if v.get(\"note\"):\n        print(\"  \" + v[\"note\"])\ngs"),
            29
        );
        assert_eq!(APP_LINE, 3);
        assert_eq!(TOKEN_LINE, 4);
    }

    #[test]
    fn the_reader() {
        let r = reader(
            br#"{"workspace":"acme","credential":{"kind":"app","appId":7},"warnings":["w1"],"revoked":["o/r"],"removed":1,"tokensDeleted":2,"runningJobs":[{"job":"j1","repo":"o/r"}],"note":"n","grants":[{"repo":"o/s","nodes":"all","shared":true}],"tokenNote":"t"}"#,
        );
        assert_eq!(
            r.stdout,
            format!(
                "credential: GitHub App 7\nwarning: w1\nrevoked: o/r (1 grant removed)\n  2 token(s) already handed out were deleted at GitHub\n  running job j1 had a token for o/r: it keeps running; its next git call for it fails\n  n\nacme: jobs may read\n  {:<40} nodes: all (shared: another server owns it)\nnote: t\n",
                "o/s"
            )
        );
        let r = reader(br#"{"credential":{"kind":"app","appId":7,"app":"bot"},"note":"ok"}"#);
        assert_eq!(r.stdout, "GitHub credential set: App 7 (bot)\n  ok\n");
        let r = reader(br#"{"credential":null}"#);
        assert!(r.stdout.starts_with("GitHub credential cleared"));
    }
}
