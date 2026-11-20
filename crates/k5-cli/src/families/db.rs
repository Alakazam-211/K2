//! `k5 db`, as the shell CLI's `cmd_db` answers it (`cli/src/db-store.sh`),
//! and the parts `k5 store` shares with it ([`take_workspace`],
//! [`apply_project`], [`run_py`]).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]); `db` is a locked `id` tool, so in a session
//!    with no passport it is the require-passport lines, exit 3.
//! 2. No words: the help page. `home` goes to `_db_home` before anything
//!    else.
//! 3. `--workspace`/`--project` anywhere on the line (`_db_take_workspace_flags`):
//!    each names the workspace, in order. A folder that is there is the
//!    workspace as `cd <it> && pwd` prints it; anything else is asked of
//!    the daemon (`GET /cli/workspace/resolve`, over TCP, its errors
//!    unseen), and kept as typed when it has no answer. A flag with no
//!    value is a usage error, after the ones before it were asked.
//! 4. The verb's words and checks. A usage error is `_db_err "usage" …`,
//!    one line of JSON on stderr, exit 2. A value flag at the end of the
//!    line (`--id` with nothing after it) fails bash's `shift 2` under
//!    `set -e`: exit 1, nothing printed.
//! 5. The call: `_db_py` ([`py`]) for most verbs; `cap` and `home` through
//!    the shell's curl calls and their Python readers ([`home`]);
//!    `versions-access`, `cluster`, `upgrade` and `create --cluster`
//!    through the shell's compute library, `_compute_send` ([`cp`]).
//!
//! `run` gets the DSN from `_db_py` and runs the command in this
//! process's place with the DSN in its environment ([`run`]).
//!
//! The binary hands these to the shell before it prints or sends
//! anything: `run` with a command it can't place ([`run::program`]), a
//! compute verb with the compute library's own settings in the
//! environment ([`cp::shells`]), a word that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, a
//! port that is set but is no number, a `python3` the binary can't tell,
//! and a `--workspace` folder bash's `cd` would treat apart (`CDPATH`
//! set, or a folder it can't enter).

use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::urllib::Opener;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub mod cp;
pub mod home;
pub mod py;
pub mod run;
#[cfg(test)]
mod tests;

use py::{err_line, Ask, Verb};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["db"];
/// The binary answers; a form [`run`] hands back goes to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "db";

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// The help page, exit 0.
    Help,
    /// `_db_home`.
    Home { keep: String, json: bool },
    /// `_db_cap`.
    Cap { val: String, json: bool },
    /// A `_db_py` verb.
    Py(Box<Ask>),
    /// A verb through `_compute_send`.
    Cp(Box<cp::Ask>),
    /// `run`: the DSN, then the command in this process's place.
    Run(Box<run::Run>),
    /// `_db_err "usage" <hint>`, exit 2.
    Usage(String),
    /// A failed `shift 2` under `set -e`: exit 1, nothing printed.
    Shift,
    /// The shell's.
    Shell,
}

/// A command line: the workspaces it names, in order, then its form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub sels: Vec<String>,
    pub form: Form,
}

fn usage(hint: impl Into<String>) -> Form {
    Form::Usage(hint.into())
}

/// `$@` as the shell's loops walk it.
pub struct Words<'a> {
    pub w: Vec<&'a str>,
}

impl<'a> Words<'a> {
    pub fn new(w: &[&'a str]) -> Words<'a> {
        Words { w: w.to_vec() }
    }

    pub fn first(&self) -> Option<&'a str> {
        self.w.first().copied()
    }

    /// `"${n:-}"`.
    pub fn at(&self, n: usize) -> &'a str {
        self.w.get(n).copied().unwrap_or("")
    }

    /// `shift n`: false (and nothing shifted) when there are fewer.
    pub fn shift(&mut self, n: usize) -> bool {
        if self.w.len() < n {
            return false;
        }
        self.w.drain(..n);
        true
    }

    pub fn len(&self) -> usize {
        self.w.len()
    }

    pub fn is_empty(&self) -> bool {
        self.w.is_empty()
    }

    /// `v="${2:-}"; shift 2` under `set -e`: the value, or the run ends.
    pub fn value(&mut self) -> Result<&'a str, Form> {
        let v = self.at(1);
        if self.shift(2) {
            Ok(v)
        } else {
            Err(Form::Shift)
        }
    }
}

/// What `_db_take_workspace_flags` makes of the words: the workspaces,
/// the words left, or the usage error that stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken<'a> {
    pub sels: Vec<String>,
    pub rest: Vec<&'a str>,
    pub error: Option<String>,
}

/// `_db_take_workspace_flags "$@"`.
pub fn take_workspace<'a>(words: &[&'a str]) -> Taken<'a> {
    let mut t = Taken {
        sels: Vec::new(),
        rest: Vec::new(),
        error: None,
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--workspace" | "--project" => {
                let v = words.get(i + 1).copied().unwrap_or("");
                if v.is_empty() || v.starts_with('-') {
                    t.error = Some(format!("{w} requires a workspace path, handle, or uuid"));
                    return t;
                }
                t.sels.push(v.to_string());
                i += 2;
            }
            _ if w.starts_with("--workspace=") || w.starts_with("--project=") => {
                let (flag, sel) = w.split_once('=').unwrap_or((w, ""));
                if sel.is_empty() {
                    t.error = Some(format!("{flag} requires a workspace path, handle, or uuid"));
                    return t;
                }
                t.sels.push(sel.to_string());
                i += 1;
            }
            _ => {
                t.rest.push(w);
                i += 1;
            }
        }
    }
    t
}

/// `[[ "$m" == [1-9][0-9] ]]`: a Postgres major as the shell's `case`
/// takes it.
fn is_major(m: &str) -> bool {
    let b = m.as_bytes();
    b.len() == 2 && (b'1'..=b'9').contains(&b[0]) && b[1].is_ascii_digit()
}

/// `cmd_db`'s words, after `db`.
pub fn parse(words: &[&str]) -> Line {
    let plain = |form| Line {
        sels: Vec::new(),
        form,
    };
    if words.is_empty() || words.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return plain(Form::Help);
    }
    if words[0] == "home" {
        return plain(home::parse(&words[1..]));
    }
    let taken = take_workspace(words);
    let form = match taken.error {
        Some(e) => usage(e),
        None if taken.rest.is_empty() => Form::Help,
        None => verb(&taken.rest).unwrap_or_else(|f| f),
    };
    Line {
        sels: taken.sels,
        form,
    }
}

fn unexpected(w: &str) -> Form {
    usage(format!("unexpected '{w}'"))
}

/// `--db` with its value (`_db_need_db_value`).
fn db_value<'a>(a: &mut Words<'a>) -> Result<&'a str, Form> {
    let v = a.at(1);
    if a.len() < 2 || v.is_empty() || v.starts_with('-') {
        return Err(usage(concat!(
            "--db needs a database name or id (",
            cli_name!(),
            " db list)"
        )));
    }
    a.shift(2);
    Ok(v)
}

/// The verb's loop and checks: the form, or the form that stopped it.
fn verb(words: &[&str]) -> Result<Form, Form> {
    let sub = words[0];
    let mut a = Words::new(&words[1..]);
    let form = match sub {
        "status" | "clusters" => {
            let mut k = Ask::new(if sub == "status" {
                Verb::Status
            } else {
                Verb::Clusters
            });
            while let Some(w) = a.first() {
                match w {
                    "--json" => k.json = true,
                    "--health" => k.health = true,
                    _ => return Err(unexpected(w)),
                }
                a.shift(1);
            }
            Form::Py(Box::new(k))
        }
        "enable" | "disable" | "uninstall" | "doctor" | "list" => {
            let mut k = Ask::new(match sub {
                "enable" => Verb::Enable,
                "disable" => Verb::Disable,
                "uninstall" => Verb::Uninstall,
                "doctor" => Verb::Doctor,
                _ => Verb::List,
            });
            while let Some(w) = a.first() {
                match w {
                    "--json" => k.json = true,
                    _ => return Err(unexpected(w)),
                }
                a.shift(1);
            }
            Form::Py(Box::new(k))
        }
        "run" => run::parse(&mut a)?,
        "versions-access" => cp::versions_access(&mut a)?,
        "cluster" => cp::cluster(&mut a)?,
        "upgrade" => cp::upgrade(&mut a)?,
        "create" => {
            let mut k = Ask::new(Verb::Create);
            let mut cluster = "";
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--id" => k.id = a.value()?.to_string(),
                    "--name" => k.name = a.value()?.to_string(),
                    "--db-access" | "--access" => k.access = a.value()?.to_string(),
                    "--test" => {
                        k.test = true;
                        a.shift(1);
                    }
                    "--major" => k.major = a.value()?.to_string(),
                    "--cluster" => cluster = a.value()?,
                    _ if w.starts_with("--major=") => {
                        k.major = w["--major=".len()..].to_string();
                        a.shift(1);
                    }
                    _ if w.starts_with("--cluster=") => {
                        cluster = &w["--cluster=".len()..];
                        a.shift(1);
                    }
                    _ => return Err(unexpected(w)),
                }
            }
            if !cluster.is_empty() {
                return cp::create_db(k.json, cluster, &k.name, &k.id, &k.major, k.test, &k.access);
            }
            if !k.major.is_empty() && !is_major(&k.major) {
                return Err(usage("--major takes a Postgres major, e.g. 16 or 17"));
            }
            if !k.access.is_empty() && !matches!(k.access.as_str(), "off" | "read" | "write") {
                return Err(usage("--db-access must be off, read, or write"));
            }
            if k.test && !k.name.is_empty() {
                return Err(usage(concat!(
                    cli_name!(),
                    " db create --test does not take --name"
                )));
            }
            Form::Py(Box::new(k))
        }
        "default-major" => {
            let mut k = Ask::new(Verb::DefaultMajor);
            k.major = a.at(0).to_string();
            a.shift(1);
            while let Some(w) = a.first() {
                match w {
                    "--json" => k.json = true,
                    _ => return Err(unexpected(w)),
                }
                a.shift(1);
            }
            if !is_major(&k.major) {
                return Err(usage(concat!(
                    cli_name!(),
                    " db default-major <major> (e.g. 17)"
                )));
            }
            Form::Py(Box::new(k))
        }
        "dsn" | "creds" => {
            let mut k = Ask::new(Verb::Dsn);
            while let Some(w) = a.first() {
                match w {
                    "--json" | "--test" | "--ref" | "--reveal" => {
                        match w {
                            "--json" => k.json = true,
                            "--test" => k.test = true,
                            "--ref" => k.reference = true,
                            _ => k.reveal = true,
                        }
                        a.shift(1);
                    }
                    "--as" => {
                        k.as_name = a.at(1).to_string();
                        if !a.shift(2) {
                            return Err(usage("--as needs a name"));
                        }
                    }
                    "--actor" => k.actor = a.value()?.to_string(),
                    "--db" => k.db = db_value(&mut a)?.to_string(),
                    _ if w.starts_with("--as=") => {
                        k.as_name = w["--as=".len()..].to_string();
                        a.shift(1);
                    }
                    _ if w.starts_with("--db=") => {
                        k.db = w["--db=".len()..].to_string();
                        a.shift(1);
                    }
                    _ => return Err(unexpected(w)),
                }
            }
            if !k.actor.is_empty() && k.actor != "migrator" {
                return Err(usage("--actor must be migrator"));
            }
            if k.reveal && k.reference {
                return Err(usage(
                    "--ref keeps the DSN in the vault and prints its reference; --reveal prints the DSN. Pick one",
                ));
            }
            Form::Py(Box::new(k))
        }
        "migrate" | "dump" | "drop" => {
            let mut k = Ask::new(match sub {
                "migrate" => Verb::Migrate,
                "dump" => Verb::Dump,
                _ => Verb::Drop,
            });
            while let Some(w) = a.first() {
                match (sub, w) {
                    (_, "--json") => {
                        k.json = true;
                        a.shift(1);
                    }
                    ("migrate", "--dir") => k.dir = a.value()?.to_string(),
                    ("dump", "--out") => k.out = a.value()?.to_string(),
                    ("drop", "--yes") => {
                        k.yes = true;
                        a.shift(1);
                    }
                    ("drop", "--test") => {
                        k.test = true;
                        a.shift(1);
                    }
                    (_, "--db") => k.db = db_value(&mut a)?.to_string(),
                    _ if w.starts_with("--db=") => {
                        k.db = w["--db=".len()..].to_string();
                        a.shift(1);
                    }
                    _ => return Err(unexpected(w)),
                }
            }
            Form::Py(Box::new(k))
        }
        "restore" => {
            let mut k = Ask::new(Verb::Restore);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--db" => k.db = db_value(&mut a)?.to_string(),
                    _ if w.starts_with("--db=") => {
                        k.db = w["--db=".len()..].to_string();
                        a.shift(1);
                    }
                    _ if w.starts_with('-') => return Err(unexpected(w)),
                    _ => {
                        if !k.file.is_empty() {
                            return Err(unexpected(w));
                        }
                        k.file = w.to_string();
                        a.shift(1);
                    }
                }
            }
            if k.file.is_empty() {
                return Err(usage(concat!(
                    cli_name!(),
                    " db restore <file> [--db <name>]"
                )));
            }
            Form::Py(Box::new(k))
        }
        "cap" => {
            let (mut val, mut json) = (String::new(), false);
            while let Some(w) = a.first() {
                match w {
                    "--json" => json = true,
                    "-h" | "--help" | "help" => return Ok(Form::Help),
                    _ if w.starts_with('-') => return Err(unexpected(w)),
                    _ => {
                        if !val.is_empty() {
                            return Err(usage(concat!(
                                cli_name!(),
                                " db cap [<n>|default] [--workspace WS]"
                            )));
                        }
                        val = w.to_string();
                    }
                }
                a.shift(1);
            }
            home::cap_form(val, json)
        }
        "grant" => {
            let mut k = Ask::new(Verb::Grant);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--db" => k.db = a.value()?.to_string(),
                    "--level" => k.level = a.value()?.to_string(),
                    "--manage" => {
                        k.manage = true;
                        a.shift(1);
                    }
                    "--relation" => {
                        let v = a.at(1);
                        if a.len() < 2 || v.is_empty() || v.starts_with('-') {
                            return Err(usage("--relation requires schema.name or a bare name"));
                        }
                        k.relations.push(v.to_string());
                        a.shift(2);
                    }
                    _ if w.starts_with("--relation=") => {
                        let v = &w["--relation=".len()..];
                        if v.is_empty() {
                            return Err(usage("--relation requires schema.name or a bare name"));
                        }
                        k.relations.push(v.to_string());
                        a.shift(1);
                    }
                    _ if w.starts_with('-') => return Err(unexpected(w)),
                    _ => {
                        if !k.grantee.is_empty() {
                            return Err(usage(format!("unexpected extra argument '{w}'")));
                        }
                        k.grantee = w.to_string();
                        a.shift(1);
                    }
                }
            }
            if k.grantee.is_empty() {
                return Err(usage(concat!(
                    cli_name!(),
                    " db grant <workspace> --db … --level read|write [--relation schema.name …] [--manage]"
                )));
            }
            if k.db.is_empty() {
                return Err(usage("grant requires --db <id|name>"));
            }
            if k.level.is_empty() {
                return Err(usage("grant requires --level read|write"));
            }
            if !matches!(k.level.as_str(), "read" | "write") {
                return Err(usage("--level must be read or write"));
            }
            Form::Py(Box::new(k))
        }
        "revoke" => {
            let mut k = Ask::new(Verb::Revoke);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--db" => k.db = a.value()?.to_string(),
                    _ if w.starts_with('-') => return Err(unexpected(w)),
                    _ => {
                        if !k.grantee.is_empty() {
                            return Err(usage(format!("unexpected extra argument '{w}'")));
                        }
                        k.grantee = w.to_string();
                        a.shift(1);
                    }
                }
            }
            if k.grantee.is_empty() {
                return Err(usage(concat!(cli_name!(), " db revoke <workspace> --db …")));
            }
            if k.db.is_empty() {
                return Err(usage("revoke requires --db <id|name>"));
            }
            Form::Py(Box::new(k))
        }
        "bind" => {
            let mut k = Ask::new(Verb::Bind);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--role" => k.role = a.value()?.to_string(),
                    "--db" => k.db = a.value()?.to_string(),
                    _ => return Err(unexpected(w)),
                }
            }
            if k.role.is_empty() {
                return Err(usage(concat!(cli_name!(), " db bind --role <pg_role>")));
            }
            Form::Py(Box::new(k))
        }
        _ => usage(format!(
            concat!("unknown db command '{}' — see ", cli_name!(), " db --help"),
            sub
        )),
    };
    Ok(form)
}

/// A `--workspace` selection the binary leaves to the shell: bash's
/// `cd` would read `CDPATH` for it, or can't enter it.
pub fn sel_is_shells(sel: &str) -> bool {
    let p = Path::new(sel);
    if !p.is_dir() {
        return false;
    }
    let cdpath = std::env::var_os("CDPATH").is_some_and(|v| !v.is_empty());
    let dotted = sel.starts_with('/')
        || sel == "."
        || sel == ".."
        || sel.starts_with("./")
        || sel.starts_with("../");
    (cdpath && !dotted) || !enterable(p)
}

#[cfg(unix)]
fn enterable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn enterable(_p: &Path) -> bool {
    true
}

/// `$PWD` as bash starts with it: the setting when it is an absolute path
/// to this folder, else the folder's real path.
fn pwd() -> PathBuf {
    let here = std::env::current_dir().unwrap_or_default();
    if let Some(p) = std::env::var_os("PWD").map(PathBuf::from) {
        if p.is_absolute() && same_dir(&p, Path::new(".")) {
            return p;
        }
    }
    here
}

#[cfg(unix)]
fn same_dir(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_dir(_a: &Path, _b: &Path) -> bool {
    false
}

/// `$(cd "$sel" && pwd)`: the folder by its logical path (`.` and `..`
/// taken off the words, links kept), or its real path when that isn't a
/// folder; trailing newlines dropped.
pub fn logical(sel: &str) -> String {
    let base = if sel.starts_with('/') {
        PathBuf::from(sel)
    } else {
        pwd().join(sel)
    };
    let mut parts: Vec<String> = Vec::new();
    for c in base.to_string_lossy().split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    let joined = format!("/{}", parts.join("/"));
    let path = if Path::new(&joined).is_dir() {
        joined
    } else {
        std::fs::canonicalize(&base)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(joined)
    };
    path.trim_end_matches('\n').to_string()
}

/// The resolve reader: the answer's `path`, `projectPath` or
/// `project_path` (the first that is there and not empty) as Python
/// prints it, trailing newlines dropped; `None` for nothing.
pub fn resolved(body: &[u8]) -> Option<String> {
    use crate::out::json::{self, Value};
    use crate::out::py as pyv;
    let text = std::str::from_utf8(body).ok()?;
    let text = if text.is_empty() { "{}" } else { text };
    let v = json::parse(text).ok()?;
    let Value::Object(_) = v else {
        return None;
    };
    let pick = ["path", "projectPath", "project_path"]
        .iter()
        .find_map(|k| py::at(&v, k).filter(|x| pyv::truthy(x)))?;
    let p = pyv::str_of(pick);
    let p = p.trim_end_matches('\n');
    (!p.is_empty()).then(|| p.to_string())
}

/// `_wiki_apply_project "$sel"` into `project`.
pub fn apply_project(client: &mut Client, closing: &mut Closing, project: &mut String, sel: &str) {
    if Path::new(sel).is_dir() {
        *project = logical(sel);
        return;
    }
    client.project = project.clone();
    let q = format!("q={}", quote(sel));
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, "/cli/workspace/resolve", Call::Request(&[&q])) {
        Ok(r) => {
            closing.answer(&r, passport);
            if let Some(p) = resolved(&r.body) {
                *project = p;
                return;
            }
        }
        // `cli_request … 2>/dev/null | python3 …`: the line goes nowhere
        // and the command dies of SIGPIPE.
        Err(Error::NoAnswer(_)) => super::end_in_pipe(),
        Err(_) => {}
    }
    *project = sel.to_string();
}

/// `_db_py` for `ask` with `project` as `D_WS_PATH`: prints what it
/// prints and the note; the exit.
pub fn run_py(
    client: &mut Client,
    closing: Closing,
    project: &str,
    ask: &Ask,
    port_text: &str,
    host: &str,
) -> Exit {
    let mut opener = Opener::from_client(client);
    opener.netloc = format!("{host}:{port_text}");
    let (shown, note) = py::Py::new(&opener, project, ask).shown();
    let exit = shown.print();
    match note {
        Some(n) if exit != Exit::Ok => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), n.as_bytes());
        }
        _ => closing.flush(client, exit),
    }
    exit
}

/// `k5 db …`: `args` is the whole command line after `argv[0]`. `None`:
/// the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    if codex_session() {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let line = parse(&words);
    let shell = match &line.form {
        Form::Shell => true,
        Form::Cp(ask) => cp::shells(ask, std::env::vars_os()),
        Form::Run(r) => r.cmd.first().and_then(|c| run::program(c)).is_none(),
        _ => false,
    };
    let settings = match settings_for(&line.sels, shell)? {
        Ok(s) => s,
        Err(e) => return Some(e),
    };
    Some(run_with(args, line, settings))
}

/// `HOOK_SOCK` set with a `CODEX_` variable exported: the shell's.
pub fn codex_session() -> bool {
    super::hook_sock().is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name))
}

/// The checks before anything runs, shared with `store`: `None` hands
/// the line to the shell; `Err` is the settings' error line, printed,
/// and its exit.
pub fn settings_for(sels: &[String], shell: bool) -> Option<Result<Settings, Exit>> {
    if shell || sels.iter().any(|s| sel_is_shells(s)) || !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Err(Exit::Error));
        }
    };
    if prelude::gate(&settings).is_none()
        && settings
            .port_text
            .as_deref()
            .and_then(super::curl_port)
            .is_none()
    {
        return None;
    }
    Some(Ok(settings))
}

/// The gates and the id tool's check, then the help page for `help`;
/// `Err` is the exit that ends the run there.
pub fn prelude_for(args: &[OsString], settings: &Settings, tool: &str) -> Result<bool, Exit> {
    if let Some(gate) = prelude::gate(settings) {
        return Err(gate.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) =
        prelude::id_tool(settings, super::hook_sock().as_deref(), would_refuse, tool)
    {
        return Err(stop);
    }
    Ok(would_refuse)
}

/// The help page `page` on stdout, exit 0.
pub fn help_page(page: &str) -> Exit {
    let text = crate::text::help(page).unwrap_or_default();
    match out::stdout(text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

fn run_with(args: &[OsString], line: Line, settings: Settings) -> Exit {
    let would_refuse = match prelude_for(args, &settings, TOOL) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let mut project = settings.project.clone();
    for sel in &line.sels {
        apply_project(&mut client, &mut closing, &mut project, sel);
    }
    client.project = project.clone();
    let port_text = settings.port_text.clone().unwrap_or_default();
    let exit = match line.form {
        Form::Help => help_page("db"),
        Form::Usage(hint) => {
            out::stderr_line(&err_line("usage", &hint));
            Exit::Usage
        }
        Form::Shift => Exit::Error,
        Form::Shell => unreachable!("handed to the shell in run"),
        Form::Home { keep, json } => home::home(&mut client, &mut closing, &keep, json),
        Form::Cap { val, json } => home::cap(&mut client, &mut closing, &project, &val, json),
        Form::Cp(ask) => cp::run(
            &mut cp::Cp {
                client: &mut client,
                closing: &mut closing,
            },
            &project,
            &ask,
        ),
        Form::Run(r) => {
            let netloc = format!("{}:{port_text}", settings.host);
            return run::run(&mut client, closing, &project, &r, netloc);
        }
        Form::Py(ask) => {
            return run_py(
                &mut client,
                closing,
                &project,
                &ask,
                &port_text,
                &settings.host,
            );
        }
    };
    closing.flush(&mut client, exit);
    exit
}
