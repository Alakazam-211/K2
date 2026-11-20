//! The `db` verbs that go through the shell's compute library
//! (`cli/src/compute-lib.sh`, `_compute_send`) rather than `_db_py`:
//! `versions-access`, `cluster create`, `cluster restart`, `create
//! --cluster` (`_db_named_post`) and `upgrade`, as `cmd_db` answers them
//! (`cli/src/db-store.sh`).
//!
//! - The requests are [`Client::compute_send`]'s: the cell socket first
//!   when there is a passport with a live one, then TCP. `versions-access`
//!   goes over TCP only (`_compute_call … tcp`). The POSTs to
//!   `/cli/db/clusters` and `/cli/db/upgrade` may take 900 s
//!   (`K2_CP_MAX_TIME=900`); every answer's `permission` object is noted
//!   for the EXIT trap (`_compute_split`).
//! - The bodies are `_compute_json`'s ([`Body`]): the keys in the order
//!   given, an empty value left out, printed as `json.dumps` prints.
//! - `_compute_fail` (`versions-access`, `upgrade --status`) is
//!   [`crate::spec::run::fail_line`]: one JSON line on stderr, exit 2 for
//!   400 and 413, else 3. `_db_named_post` and the upgrade POST print
//!   their own refusal lines.
//! - The Python readers on the right of `printf '%s' "$_CP_BODY" |` are
//!   copied line for line; where one raises, its traceback (`python3 -c`)
//!   is printed and the command ends with 1 (`set -e`).
//! - `upgrade` (not `--no-wait`, not a check) watches the move as the
//!   shell does: every 3 s a `GET /cli/db/upgrade?db=<name>`, a step line
//!   on stderr each time it changes, until the move ends (0 when it
//!   succeeded, 3 when it was aborted or rolled back, else 1). An answer
//!   that is not 200 is asked again, as the shell's `continue` does.
//!
//! The binary leaves these to the shell ([`shells`]): a `K2_CJ_` variable
//! in the environment (`_compute_json` puts it in the body),
//! `_CP_404_EXIT`, `_K2_REFUSAL_AREA` or `_K2_NOANSWER_SOFT` set (the
//! shell's internals), and `K2_CP_MAX_TIME` set for `versions-access`
//! (the one call here that reads it).

use super::py::{dumps_ascii, obj};
use super::{is_major, usage, Form, Words};
use crate::exit::Exit;
use crate::families::{py_get, py_iter, raised, s, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{contains, index, py_eq};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::spec::run as spec_run;
use crate::transport::client::Auth;
use crate::transport::encode::quote;
use crate::transport::{permission, Channel, Client, Error, Response};
use k2_client::{cli_name, product_name};
use std::time::Duration;

/// `K2_CP_MAX_TIME=900` before `_compute_send POST`.
const LONG_POST: Duration = Duration::from_secs(900);

/// `sleep 3` between the watch's asks.
pub const WATCH_PAUSE: Duration = Duration::from_secs(3);

/// The hint a reader prints for an empty answer.
const NO_ANSWER: &str = concat!("no answer from the ", product_name!(), " daemon");

const VA_USAGE: &str = concat!(
    cli_name!(),
    " db versions-access on|off [--workspace WS] [--json]"
);
const CC_UNEXPECTED: &str = concat!(
    " — ",
    cli_name!(),
    " db cluster create <major> --name <name> [--port <port>] [--wal-archive]"
);
const CC_MAJOR: &str = concat!(
    cli_name!(),
    " db cluster create <major> --name <name> (e.g. ",
    cli_name!(),
    " db cluster create 17 --name feedback)"
);
const CC_NAME: &str = concat!(cli_name!(), " db cluster create <major> --name <name>");
const CR_NAME: &str = concat!(cli_name!(), " db cluster restart <name>");
const CLUSTER_USAGE: &str = concat!(
    cli_name!(),
    " db cluster create <major> --name <name> [--port <port>] [--wal-archive] | ",
    cli_name!(),
    " db cluster restart <name>"
);
const CREATE_CLUSTER_USAGE: &str = concat!(
    cli_name!(),
    " db create --cluster takes --name, --id and --major only (no --test, no --db-access)"
);
const STATUS_USAGE: &str = concat!(cli_name!(), " db upgrade <db> --status");

/// What a compute verb asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `versions-access on|off`.
    VersionsAccess { on: bool, json: bool },
    /// `_db_named_post`.
    Named(Named),
    /// `upgrade`.
    Upgrade(Upgrade),
}

/// `_db_named_post`'s `what`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// `cluster create`.
    Cluster,
    /// `cluster restart`.
    Restart,
    /// `create --cluster`.
    CreateDb,
}

/// A `POST /cli/db/clusters`: the words, as the shell holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub what: What,
    pub json: bool,
    pub cluster: String,
    pub name: String,
    pub id: String,
    pub major: String,
    pub port: String,
    pub wal: bool,
}

impl Named {
    fn new(what: What) -> Named {
        Named {
            what,
            json: false,
            cluster: String::new(),
            name: String::new(),
            id: String::new(),
            major: String::new(),
            port: String::new(),
            wal: false,
        }
    }
}

/// `upgrade`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Upgrade {
    pub json: bool,
    pub db: String,
    pub app: String,
    pub all: bool,
    pub all_ws: bool,
    pub to: String,
    pub check: bool,
    pub rollback: bool,
    pub yes: bool,
    pub forget: bool,
    pub force_now: bool,
    pub now: bool,
    pub adopted: bool,
    pub ack: bool,
    pub no_wait: bool,
    pub live: bool,
    pub switch: bool,
    pub abort: bool,
    pub prep: String,
    pub rif: bool,
    pub status: bool,
}

fn cp(ask: Ask) -> Form {
    Form::Cp(Box::new(ask))
}

/// `versions-access`: its words after the verb.
pub fn versions_access(a: &mut Words) -> Result<Form, Form> {
    let action = a.at(0);
    a.shift(1);
    let mut json = false;
    while let Some(w) = a.first() {
        match w {
            "--json" => json = true,
            _ => return Err(usage(VA_USAGE)),
        }
        a.shift(1);
    }
    match action {
        "on" | "off" => Ok(cp(Ask::VersionsAccess {
            on: action == "on",
            json,
        })),
        _ => Err(usage(VA_USAGE)),
    }
}

/// `cluster create|restart`: its words after the verb.
pub fn cluster(a: &mut Words) -> Result<Form, Form> {
    let op = a.at(0);
    a.shift(1);
    match op {
        "create" => {
            let mut k = Named::new(What::Cluster);
            k.major = a.at(0).to_string();
            a.shift(1);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--name" => k.name = a.value()?.to_string(),
                    "--port" => k.port = a.value()?.to_string(),
                    "--wal-archive" => {
                        k.wal = true;
                        a.shift(1);
                    }
                    _ if w.starts_with("--name=") => {
                        k.name = w["--name=".len()..].to_string();
                        a.shift(1);
                    }
                    _ if w.starts_with("--port=") => {
                        k.port = w["--port=".len()..].to_string();
                        a.shift(1);
                    }
                    _ => return Err(usage(format!("unexpected '{w}'{CC_UNEXPECTED}"))),
                }
            }
            if !is_major(&k.major) {
                return Err(usage(CC_MAJOR));
            }
            if k.name.is_empty() {
                return Err(usage(CC_NAME));
            }
            if !k.port.is_empty() && !is_named_port(&k.port) {
                return Err(usage("--port takes a port in 5433-5449"));
            }
            Ok(cp(Ask::Named(k)))
        }
        "restart" => {
            let mut k = Named::new(What::Restart);
            k.name = a.at(0).to_string();
            a.shift(1);
            while let Some(w) = a.first() {
                match w {
                    "--json" => k.json = true,
                    _ => return Err(usage(format!("unexpected '{w}'"))),
                }
                a.shift(1);
            }
            if k.name.is_empty() {
                return Err(usage(CR_NAME));
            }
            Ok(cp(Ask::Named(k)))
        }
        _ => Err(usage(CLUSTER_USAGE)),
    }
}

/// `case "$c_port" in 54[3-4][0-9])`.
fn is_named_port(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() == 4
        && b[0] == b'5'
        && b[1] == b'4'
        && matches!(b[2], b'3' | b'4')
        && b[3].is_ascii_digit()
}

/// `create --cluster <c>`: the loop's values, after it.
pub fn create_db(
    json: bool,
    cluster: &str,
    name: &str,
    id: &str,
    major: &str,
    test: bool,
    access: &str,
) -> Result<Form, Form> {
    if test || !access.is_empty() {
        return Err(usage(CREATE_CLUSTER_USAGE));
    }
    if !major.is_empty() && !is_major(major) {
        return Err(usage("--major takes a Postgres major, e.g. 16 or 17"));
    }
    let mut k = Named::new(What::CreateDb);
    k.json = json;
    k.cluster = cluster.to_string();
    k.name = name.to_string();
    k.id = id.to_string();
    k.major = major.to_string();
    Ok(cp(Ask::Named(k)))
}

/// `upgrade`: its words after the verb.
pub fn upgrade(a: &mut Words) -> Result<Form, Form> {
    let mut u = Upgrade::default();
    while let Some(w) = a.first() {
        let flag = |u: &mut Upgrade| -> bool {
            let slot = match w {
                "--json" => &mut u.json,
                "--all" => &mut u.all,
                "--all-workspace-dbs" => &mut u.all_ws,
                "--check" => &mut u.check,
                "--rollback" => &mut u.rollback,
                "--yes" => &mut u.yes,
                "--forget-old" => &mut u.forget,
                "--force-now" => &mut u.force_now,
                "--now" => &mut u.now,
                "--allow-adopted" => &mut u.adopted,
                "--acknowledge-failed" => &mut u.ack,
                "--no-wait" => &mut u.no_wait,
                "--live" => &mut u.live,
                "--switch" => &mut u.switch,
                "--abort" => &mut u.abort,
                "--replica-identity-full" => &mut u.rif,
                "--status" => &mut u.status,
                _ => return false,
            };
            *slot = true;
            true
        };
        if flag(&mut u) {
            a.shift(1);
            continue;
        }
        match w {
            "--app" => u.app = a.value()?.to_string(),
            "--to" => u.to = a.value()?.to_string(),
            "--prepare-live" => u.prep = a.value()?.to_string(),
            _ if w.starts_with("--to=") => {
                u.to = w["--to=".len()..].to_string();
                a.shift(1);
            }
            _ if w.starts_with("--prepare-live=") => {
                u.prep = w["--prepare-live=".len()..].to_string();
                a.shift(1);
            }
            _ if w.starts_with('-') => return Err(usage(format!("unexpected '{w}'"))),
            _ => {
                if !u.db.is_empty() {
                    return Err(usage("one database"));
                }
                u.db = w.to_string();
                a.shift(1);
            }
        }
    }
    if !u.to.is_empty() && !is_major(&u.to) {
        return Err(usage("--to takes a Postgres major, e.g. 17"));
    }
    if !u.prep.is_empty() && !is_major(&u.prep) {
        return Err(usage("--prepare-live takes a Postgres major, e.g. 16"));
    }
    if u.status && u.db.is_empty() {
        return Err(usage(STATUS_USAGE));
    }
    Ok(cp(Ask::Upgrade(u)))
}

/// Whether the shell answers `ask`, from the environment's names and
/// values: its compute library's internals, or `K2_CP_MAX_TIME` for the
/// one call that reads it.
pub fn shells(
    ask: &Ask,
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> bool {
    vars.into_iter().any(|(name, value)| {
        let name = name.to_string_lossy();
        name.starts_with("K2_CJ_")
            || (!value.is_empty()
                && (matches!(
                    name.as_ref(),
                    "_CP_404_EXIT" | "_K2_REFUSAL_AREA" | "_K2_NOANSWER_SOFT"
                ) || (name == "K2_CP_MAX_TIME" && matches!(ask, Ask::VersionsAccess { .. }))))
    })
}

/// `_compute_json K2_CJ_<key>=<value> …`: the pairs in order, an empty
/// value left out.
#[derive(Debug, Default)]
pub struct Body {
    pairs: Vec<(Text, Value)>,
}

impl Body {
    /// `K2_CJ_<key>=<value>`.
    pub fn s(mut self, key: &str, v: &str) -> Body {
        if !v.is_empty() {
            self.pairs.push((Text::from(key), s(v)));
        }
        self
    }

    /// `K2_CJ_<key>_b=1` (`true`) or `=0` (`false`); left out when empty.
    pub fn b(mut self, key: &str, v: &str) -> Body {
        if !v.is_empty() {
            self.pairs.push((Text::from(key), Value::Bool(v == "1")));
        }
        self
    }

    /// `K2_CJ_<key>_b="$($flag && echo 1)"`: `true`, or nothing.
    pub fn flag(self, key: &str, on: bool) -> Body {
        self.b(key, if on { "1" } else { "" })
    }

    /// `K2_CJ_<key>_n=<digits>`: the callers' checks let only plain digits
    /// with no leading zero through.
    pub fn n(mut self, key: &str, v: &str) -> Body {
        if !v.is_empty() {
            self.pairs
                .push((Text::from(key), Value::Int(v.to_string())));
        }
        self
    }

    /// `print(json.dumps(out))`.
    pub fn text(self) -> String {
        dumps_ascii(&Value::Object(self.pairs))
    }
}

/// `_compute_ws ""`: this workspace, outside a session.
fn ws(client: &Client, project: &str) -> String {
    if matches!(client.auth, Auth::Passport { .. }) {
        String::new()
    } else {
        project.to_string()
    }
}

/// One verb's requests: the client and the EXIT trap's lines.
pub struct Cp<'a> {
    pub client: &'a mut Client,
    pub closing: &'a mut Closing,
}

impl Cp<'_> {
    /// `_compute_split "$(_compute_send …)"`: the status and the body.
    /// `Err`: a POST that may have been applied, or a session's TCP call
    /// that would carry the owner token: its line, exit 1.
    fn send(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<(String, Vec<u8>), Exit> {
        let r = self.client.compute_send(post, path, payload, tcp);
        if let Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) = &r {
            out::stderr_line(&e.to_string());
            return Err(Exit::Error);
        }
        let (status, body) = spec_run::split(&spec_run::raw(&r, self.client.refused_before_answer()));
        let noted = Response {
            status: 0,
            permission: permission::note(&body),
            body: body.clone(),
            channel: r.as_ref().map_or(Channel::Tcp, |x| x.channel),
            server_version: None,
        };
        let passport = self.client.passport().is_some();
        self.closing.answer(&noted, passport);
        Ok((status, body))
    }

    /// A POST with `K2_CP_MAX_TIME=900`.
    fn long_post(&mut self, path: &str, payload: &str) -> Result<(String, Vec<u8>), Exit> {
        let before = self.client.compute_post_max_time.replace(LONG_POST);
        let r = self.send(true, path, payload, false);
        self.client.compute_post_max_time = before;
        r
    }
}

/// Runs `ask` with `project` as `$PROJECT`: the exit.
pub fn run(cp: &mut Cp, project: &str, ask: &Ask) -> Exit {
    let r = match ask {
        Ask::VersionsAccess { on, json } => versions_access_send(cp, project, *on, *json),
        Ask::Named(k) => named_send(cp, project, k),
        Ask::Upgrade(u) => upgrade_send(cp, u),
    };
    r.unwrap_or_else(|e| e)
}

/// `printf '%s\n' "$_CP_BODY"`.
fn print_body(body: &[u8]) -> Exit {
    let mut text = body.to_vec();
    text.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &text) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_compute_fail` (no refusal rows): its line, and its exit.
fn compute_fail(status: &str, body: &[u8]) -> Exit {
    out::stderr_line(&spec_run::fail_line(status, body));
    match status {
        "400" | "413" => Exit::Usage,
        _ => Exit::Refused,
    }
}

fn versions_access_send(cp: &mut Cp, project: &str, on: bool, json: bool) -> Result<Exit, Exit> {
    let body = Body::default()
        .s("workspace", &ws(cp.client, project))
        .s("toggle", "db_upgrade")
        .b("value", if on { "1" } else { "0" })
        .text();
    let (status, answer) = cp.send(true, "/cli/agent-access/set", &body, true)?;
    if status != "200" {
        return Ok(compute_fail(&status, &answer));
    }
    if json {
        return Ok(print_body(&answer));
    }
    Ok(va_show(&answer).print())
}

fn named_send(cp: &mut Cp, project: &str, k: &Named) -> Result<Exit, Exit> {
    let ws = ws(cp.client, project);
    let body = match k.what {
        What::CreateDb => Body::default()
            .s("op", "create-db")
            .s("project", &ws)
            .s("cluster", &k.cluster)
            .s("name", &k.name)
            .s("id", &k.id)
            .n("major", &k.major),
        What::Cluster => Body::default()
            .s("op", "create")
            .s("project", &ws)
            .n("major", &k.major)
            .s("name", &k.name)
            .n("port", &k.port)
            .flag("walArchive", k.wal),
        What::Restart => Body::default()
            .s("op", "restart")
            .s("project", &ws)
            .s("name", &k.name),
    }
    .text();
    let (status, answer) = cp.long_post("/cli/db/clusters", &body)?;
    if status != "200" {
        let shown = named_fail(&answer);
        let exit = shown.print();
        if exit != Exit::Ok {
            return Ok(exit);
        }
        return Ok(match status.as_str() {
            "400" => Exit::Usage,
            "404" => Exit::Error,
            _ => Exit::Refused,
        });
    }
    if k.json {
        return Ok(print_body(&answer));
    }
    Ok(named_show(k.what, &answer).print())
}

fn upgrade_send(cp: &mut Cp, u: &Upgrade) -> Result<Exit, Exit> {
    if u.status {
        let path = format!("/cli/db/upgrade?db={}", quote(&u.db));
        let (status, answer) = cp.send(false, &path, "", false)?;
        if status != "200" {
            return Ok(compute_fail(&status, &answer));
        }
        if u.json {
            return Ok(print_body(&answer));
        }
        return Ok(status_show(&answer).print());
    }
    let body = Body::default()
        .s("db", &u.db)
        .s("app", &u.app)
        .flag("all", u.all)
        .n("to", &u.to)
        .flag("check", u.check)
        .flag("rollback", u.rollback)
        .flag("yes", u.yes)
        .flag("forgetOld", u.forget)
        .flag("forceNow", u.force_now)
        .flag("now", u.now)
        .flag("allowAdopted", u.adopted)
        .flag("acknowledgeFailed", u.ack)
        .flag("allWorkspaceDbs", u.all_ws)
        .flag("live", u.live)
        .flag("switch", u.switch)
        .flag("abort", u.abort)
        .n("prepareLive", &u.prep)
        .flag("replicaIdentityFull", u.rif)
        .text();
    let (status, answer) = cp.long_post("/cli/db/upgrade", &body)?;
    if status != "200" {
        let exit = upgrade_fail(&answer).print();
        if exit != Exit::Ok {
            return Ok(exit);
        }
        return Ok(if status == "400" {
            Exit::Usage
        } else {
            Exit::Refused
        });
    }
    let live_only = u.live && !u.check;
    if u.json
        && (u.no_wait
            || u.check
            || u.rollback
            || u.forget
            || u.ack
            || u.abort
            || !u.prep.is_empty()
            || live_only)
    {
        return Ok(print_body(&answer));
    }
    if live_only || u.abort || !u.prep.is_empty() {
        return Ok(live_show(&answer).print());
    }
    if u.check || u.rollback || u.forget || u.ack {
        return Ok(check_show(&answer).print());
    }
    let name = if u.db.is_empty() {
        match started(&answer) {
            Ok(n) => n,
            Err(r) => return Ok(raised(String::new(), String::new(), &r).print()),
        }
    } else {
        u.db.clone()
    };
    if u.no_wait {
        return Ok(print_body(&answer));
    }
    if u.switch {
        out::stderr_line(&format!(
            "switching {name} (writes pause a few seconds; the old copy stays 14 days)"
        ));
    } else {
        out::stderr_line(&format!(
            "moving {name} (the old copy stays 14 days; {} db upgrade {name} --rollback --yes)",
            cli_name!()
        ));
    }
    let path = format!("/cli/db/upgrade?db={}", quote(&name));
    let mut seen = String::new();
    loop {
        std::thread::sleep(WATCH_PAUSE);
        let (status, answer) = cp.send(false, &path, "", false)?;
        if status != "200" {
            continue;
        }
        let (stdout, shown) = poll(&answer, &seen, u.json);
        if !shown.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
        }
        if shown.exit != Exit::Ok {
            return Ok(shown.exit);
        }
        let got = stdout.trim_end_matches('\n');
        match got {
            "end:succeeded" => return Ok(Exit::Ok),
            "end:aborted" | "end:rolled_back" => return Ok(Exit::Refused),
            _ if got.starts_with("end:") => return Ok(Exit::Error),
            _ if got.starts_with("seen:") => seen = got["seen:".len()..].to_string(),
            _ => {}
        }
    }
}

// The readers. Each is the shell's `python3 -c '…'`, line 1 its first.

/// A reader's output so far.
#[derive(Default)]
struct P {
    out: String,
    err: String,
}

impl P {
    fn print(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    fn eprint(&mut self, line: impl AsRef<str>) {
        self.err.push_str(line.as_ref());
        self.err.push('\n');
    }

    fn done(self, r: Result<Exit, Raise>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.out,
                stderr: self.err,
                exit,
            },
            Err(e) => raised(self.out, self.err, &e),
        }
    }
}

/// `json.load(sys.stdin)` at `line`.
fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    loads(&String::from_utf8_lossy(body)).map_err(|f| Raise::at(line, f.last_line()))
}

/// `"%s" % x` for what `d.get(k)` gave.
fn st(x: Option<&Value>) -> String {
    x.map_or_else(|| "None".to_string(), py::str_of)
}

/// `d.get(k, default)` on a dict.
fn get_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, line)?.cloned().unwrap_or(default))
}

/// `x or default`.
fn or(x: Option<&Value>, default: Value) -> Value {
    x.filter(|v| py::truthy(v)).cloned().unwrap_or(default)
}

/// `text + v`, where `v` must be a string.
fn cat(text: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{text}{}", py::str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

fn not_subscriptable(v: &Value, line: u32) -> Raise {
    Raise::at(
        line,
        format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(v)
        ),
    )
}

/// `v[n]` for an integer `n`.
fn nth(v: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => items
            .get(n)
            .cloned()
            .ok_or_else(|| Raise::at(line, "IndexError: list index out of range")),
        Value::Str(t) => t
            .code_points()
            .get(n)
            .map(|cp| Value::Str(crate::families::code_point(*cp)))
            .ok_or_else(|| Raise::at(line, "IndexError: string index out of range")),
        Value::Object(_) => Err(Raise::at(line, format!("KeyError: {n}"))),
        other => Err(not_subscriptable(other, line)),
    }
}

/// `v[:n]`, walked by a `for`.
fn head(v: &Value, n: usize, line: u32) -> Result<Vec<Value>, Raise> {
    match v {
        Value::Array(items) => Ok(items.iter().take(n).cloned().collect()),
        Value::Str(_) => Ok(py_iter(v, line)?.into_iter().take(n).collect()),
        Value::Object(_) => Err(Raise::at(line, "TypeError: unhashable type: 'slice'")),
        other => Err(not_subscriptable(other, line)),
    }
}

/// `"%d" % (v >> 20)`.
fn mib(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Bool(_) => Ok("0".to_string()),
        Value::Int(d) => Ok(shr20(d)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for >>: '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}

/// An integer's digits shifted right by 20 bits (floor division by
/// 1048576, as Python's `>>`).
fn shr20(digits: &str) -> String {
    const D: u64 = 1 << 20;
    let (neg, mag) = match digits.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, digits),
    };
    let mut q = String::new();
    let mut rem: u64 = 0;
    for b in mag.bytes() {
        rem = rem * 10 + u64::from(b - b'0');
        let d = rem / D;
        rem %= D;
        if !(q.is_empty() && d == 0) {
            q.push(char::from(b'0' + d as u8));
        }
    }
    if q.is_empty() {
        q.push('0');
    }
    if neg && rem != 0 {
        // Toward minus infinity: one more in magnitude.
        let mut bytes = q.into_bytes();
        let mut i = bytes.len();
        loop {
            if i == 0 {
                bytes.insert(0, b'1');
                break;
            }
            i -= 1;
            if bytes[i] == b'9' {
                bytes[i] = b'0';
            } else {
                bytes[i] += 1;
                break;
            }
        }
        q = String::from_utf8(bytes).unwrap_or_default();
    }
    if neg && q != "0" {
        format!("-{q}")
    } else {
        q
    }
}

/// `print(json.dumps(...))`, on stderr.
fn refusal(p: &mut P, v: &Value) {
    p.eprint(dumps_ascii(v));
}

/// `_db_named_post`'s reader of a refusal (all on stderr).
fn named_fail(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let raw = String::from_utf8_lossy(body);
        let v = loads(&raw).unwrap_or_else(|_| {
            obj(vec![(
                "error",
                obj(vec![
                    ("code", s("daemon_unreachable")),
                    ("hint", hint_of(&raw)),
                ]),
            )])
        });
        let e = py_get(&v, "error", 5)?.cloned();
        let e = match e {
            Some(e @ Value::Object(_)) => e,
            other => {
                let code = or(other.as_ref(), s("error"));
                obj(vec![
                    ("code", code),
                    ("hint", get_or(&v, "hint", s(""), 6)?),
                ])
            }
        };
        let code = py_get(&e, "code", 7)?.cloned().unwrap_or(Value::Null);
        let hint = get_or(&e, "hint", s(""), 7)?;
        refusal(
            &mut p,
            &obj(vec![("error", obj(vec![("code", code), ("hint", hint)]))]),
        );
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// `raw[:200] or "no answer from the K2 daemon"`.
fn hint_of(raw: &str) -> Value {
    let head: String = raw.chars().take(200).collect();
    s(if head.is_empty() { NO_ANSWER } else { &head })
}

/// `_db_named_post`'s reader of an answer.
fn named_show(what: What, body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        match what {
            What::Cluster => {
                let c = or(py_get(&v, "cluster", 5)?, Value::Object(Vec::new()));
                let f = |k: &str| -> Result<String, Raise> { Ok(st(py_get(&c, k, 6)?)) };
                let line = format!(
                    "cluster {} on port {} ({}, {})",
                    f("label")?,
                    f("port")?,
                    f("unit")?,
                    f("state")?
                );
                p.print(line);
                if py_get(&c, "walSpool", 7)?.is_some_and(py::truthy) {
                    let w = index(&c, "walSpool", 8)?;
                    p.print(format!(
                        "WAL archive: {} (a backup ships and prunes it)",
                        py::str_of(w)
                    ));
                }
                p.print(format!(
                    "next: {} db create --cluster {} --name <db>",
                    cli_name!(),
                    st(py_get(&c, "name", 9)?)
                ));
            }
            What::Restart => {
                p.print(format!("restarted {}", st(py_get(&v, "cluster", 11)?)));
            }
            What::CreateDb => {
                let name = st(py_get(&v, "name", 13)?);
                let cluster = or(py_get(&v, "cluster", 13)?, s("?"));
                p.print(format!("created {name} in {}", py::str_of(&cluster)));
                if py_get(&v, "existing", 14)?.is_some_and(py::truthy) {
                    p.print("(already there: same --id or name)");
                }
                if let Some(d) = py_get(&v, "dsn", 16)?.filter(|x| py::truthy(x)) {
                    p.print(format!("dsn: {}", py::str_of(d)));
                }
                if py_get(&v, "dsnHint", 18)?.is_some_and(py::truthy) {
                    let h = index(&v, "dsnHint", 19)?;
                    p.print(format!("no password shown; {}", py::str_of(h)));
                }
            }
        }
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// `versions-access`'s reader.
fn va_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        let on = py::truthy(index(&v, "agentsCanUpgradePostgres", 3)?);
        let ws = py::str_of(index(&v, "workspace", 4)?);
        p.print(format!(
            "Let this agent manage Postgres versions: {} for {ws}{}",
            if on { "on" } else { "off" },
            if on {
                " (check, move, roll back and create --major on its own and its Apps databases; each act is in Access Audit; deleting the old copy stays yours)"
            } else {
                ""
            }
        ));
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// The upgrade POST's reader of a refusal (on stderr).
fn upgrade_fail(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let raw = String::from_utf8_lossy(body);
        let v = loads(&raw).unwrap_or_else(|_| {
            obj(vec![
                ("error", s("daemon_unreachable")),
                ("hint", hint_of(&raw)),
            ])
        });
        let error = py_get(&v, "error", 5)?.cloned().unwrap_or(Value::Null);
        let hint = get_or(&v, "hint", s(""), 5)?;
        refusal(&mut p, &obj(vec![("error", error), ("hint", hint)]));
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// `upgrade --status`'s reader.
fn status_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        let live = or(py_get(&v, "live", 3)?, Value::Array(Vec::new()));
        for r in py_iter(&live, 3)? {
            let t = or(py_get(&r, "tables", 4)?, Value::Object(Vec::new()));
            let lag = py_get(&r, "lagBytes", 5)?.cloned().unwrap_or(Value::Null);
            let db = py::str_of(index(&r, "db", 6)?);
            let from = py::str_of(index(&r, "from", 6)?);
            let to = py::str_of(index(&r, "to", 6)?);
            let state = index(&r, "state", 6)?.clone();
            let ready = st(py_get(&t, "ready", 6)?);
            let total = st(py_get(&t, "total", 6)?);
            let lag = if lag == Value::Null {
                "-".to_string()
            } else {
                format!("{} MiB", mib(&lag, 7)?)
            };
            let wal = or(py_get(&r, "walStatus", 7)?, s("-"));
            p.print(format!(
                "live {db}: Postgres {from} → {to}, {}; tables {ready}/{total}; lag {lag}; WAL {}",
                py::str_of(&state),
                py::str_of(&wal)
            ));
            if py_get(&r, "lastError", 8)?.is_some_and(py::truthy) {
                p.print(cat("  ", index(&r, "lastError", 8)?, 8)?);
            }
            if py_eq(index(&r, "state", 9)?, &s("streaming")) {
                p.print(format!(
                    "  ready: {} db upgrade {} --switch (or --abort)",
                    cli_name!(),
                    py::str_of(index(&r, "db", 9)?)
                ));
            }
        }
        let moves = or(py_get(&v, "moves", 10)?, Value::Array(Vec::new()));
        for m in head(&moves, 3, 10)? {
            let kind = get_or(&m, "kind", s("move"), 11)?;
            let method = get_or(&m, "method", s("copy"), 11)?;
            let f = |k: &str| -> Result<String, Raise> { Ok(st(py_get(&m, k, 11)?)) };
            let (from, to, state, step) = (f("from")?, f("to")?, f("state")?, f("step")?);
            let tail = if py_get(&m, "lastError", 12)?.is_some_and(py::truthy) {
                cat("  ", index(&m, "lastError", 12)?, 12)?
            } else {
                String::new()
            };
            p.print(format!(
                "{} {} {from} → {to}: {state} ({step}){tail}",
                py::str_of(&kind),
                py::str_of(&method)
            ));
        }
        let any = py_get(&v, "live", 13)?.is_some_and(py::truthy)
            || py_get(&v, "moves", 13)?.is_some_and(py::truthy);
        if !any {
            p.print("no moves and no live move for this database");
        }
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// The reader of a live move started, aborted, or a cluster prepared.
fn live_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        let r = py_get(&v, "live", 3)?.cloned();
        match r.filter(py::truthy) {
            Some(r) => {
                let f = |k: &str| -> Result<String, Raise> { Ok(py::str_of(index(&r, k, 5)?)) };
                let line = format!(
                    "live move of {}: Postgres {} → {}, {}",
                    f("db")?,
                    f("from")?,
                    f("to")?,
                    f("state")?
                );
                p.print(line);
                let state = index(&r, "state", 6)?;
                if py_eq(state, &s("syncing")) || py_eq(state, &s("streaming")) {
                    p.print("copying while it serves; schema changes wait until the switch (at most 7 days)");
                    let db = py::str_of(index(&r, "db", 8)?);
                    p.print(format!(
                        "watch: {c} db upgrade {db} --status; then {c} db upgrade {db} --switch (or --abort)",
                        c = cli_name!()
                    ));
                } else if py_get(&r, "lastError", 9)?.is_some_and(py::truthy) {
                    p.print(cat("  ", index(&r, "lastError", 10)?, 10)?);
                }
            }
            None => {
                let major = st(py_get(&v, "major", 12)?);
                let wal = st(py_get(&v, "walLevel", 12)?);
                let restarted = py_get(&v, "restarted", 12)?.is_some_and(py::truthy);
                p.print(format!(
                    "Postgres {major}: wal_level {wal}{}",
                    if restarted {
                        " (restarted once)"
                    } else {
                        " (nothing restarted)"
                    }
                ));
            }
        }
        Ok(Exit::Ok)
    })();
    p.done(r)
}

/// `mark.get(level, level)`.
fn mark(level: &Value, line: u32) -> Result<String, Raise> {
    match level {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(level)),
        )),
        Value::Str(t) => Ok(match t.as_str() {
            Some("ok") => "ok    ".to_string(),
            Some("warn") => "warn  ".to_string(),
            Some("refuse") => "REFUSE".to_string(),
            _ => py::str_of(level),
        }),
        other => Ok(py::str_of(other)),
    }
}

/// The reader of a check, a rollback, a forget or an acknowledge.
fn check_show(body: &[u8]) -> Shown {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        let mut refused = false;
        let checks = get_or(&v, "check", Value::Array(Vec::new()), 5)?;
        for r in py_iter(&checks, 5)? {
            if contains(&r, "error", 6)? {
                let db = st(py_get(&r, "db", 7)?);
                let e = py::str_of(index(&r, "error", 7)?);
                p.print(format!("{db}: {e}"));
                refused = true;
                continue;
            }
            let f = |k: &str| -> Result<String, Raise> { Ok(py::str_of(index(&r, k, 8)?)) };
            let (db, from, to) = (f("db")?, f("from")?, f("to")?);
            let secs = index(&r, "pauseSecs", 8)?;
            let lo = py::str_of(&nth(secs, 0, 8)?);
            let hi = py::str_of(&nth(index(&r, "pauseSecs", 8)?, 1, 8)?);
            p.print(format!(
                "{db}: Postgres {from} → {to} (about {lo}–{hi} s pause)"
            ));
            for l in py_iter(index(&r, "lines", 9)?, 9)? {
                let level = index(&l, "level", 10)?;
                let m = mark(level, 10)?;
                let id = crate::families::ljust(&py::str_of(index(&l, "id", 10)?), 10);
                let text = py::str_of(index(&l, "text", 10)?);
                let tail = if py_get(&l, "hint", 10)?.is_some_and(py::truthy) {
                    cat("  — ", index(&l, "hint", 10)?, 10)?
                } else {
                    String::new()
                };
                p.print(format!("  {m} {id} {text}{tail}"));
                refused = refused || py_eq(index(&l, "level", 11)?, &s("refuse"));
            }
        }
        if !contains(&v, "check", 12)? {
            let opts = Options {
                compact: false,
                indent: Some(2),
                ensure_ascii: true,
                sort_keys: false,
            };
            p.print(json::to_string(&v, &opts).unwrap_or_default());
        }
        Ok(if refused { Exit::Refused } else { Exit::Ok })
    })();
    p.done(r)
}

/// `print(v.get("switching") or v["started"][0])` held in `$(…)`.
fn started(body: &[u8]) -> Result<String, Raise> {
    let v = load(body, 1)?;
    let name = match py_get(&v, "switching", 1)?.filter(|x| py::truthy(x)) {
        Some(x) => x.clone(),
        None => nth(index(&v, "started", 1)?, 0, 1)?,
    };
    Ok(py::str_of(&name).trim_end_matches('\n').to_string())
}

/// One ask of the watch: what its reader printed on stdout (for
/// `u_out`), and the rest (stderr and its exit).
fn poll(body: &[u8], seen: &str, json_out: bool) -> (String, Shown) {
    let mut p = P::default();
    let r = (|| -> Result<Exit, Raise> {
        let v = load(body, 2)?;
        let moves = or(py_get(&v, "moves", 3)?, Value::Array(vec![Value::Null]));
        let m = nth(&moves, 0, 3)?;
        if !py::truthy(&m) {
            p.print("wait");
            return Ok(Exit::Ok);
        }
        let step = format!(
            "{}/{}",
            py::str_of(index(&m, "step", 5)?),
            py::str_of(index(&m, "state", 5)?)
        );
        if step != seen && !json_out {
            p.eprint(format!("  {step}"));
        }
        let state = index(&m, "state", 8)?;
        let end = ["succeeded", "aborted", "rolled_back", "failed"]
            .iter()
            .any(|e| py_eq(state, &s(e)));
        if end {
            if json_out {
                p.eprint(dumps_ascii(&m));
            } else {
                if py_get(&m, "lastError", 12)?.is_some_and(py::truthy) {
                    p.eprint(cat("  ", index(&m, "lastError", 12)?, 12)?);
                }
                for w in py_iter(&get_or(&m, "warnings", Value::Array(Vec::new()), 13)?, 13)? {
                    p.eprint(cat("  warn: ", &w, 13)?);
                }
                for d in head(&get_or(&m, "diff", Value::Array(Vec::new()), 14)?, 20, 14)? {
                    p.eprint(cat("  diff: ", &d, 14)?);
                }
            }
            p.print(cat("end:", index(&m, "state", 15)?, 15)?);
        } else {
            p.print(format!("seen:{step}"));
        }
        Ok(Exit::Ok)
    })();
    let mut shown = p.done(r);
    let stdout = std::mem::take(&mut shown.stdout);
    (stdout, shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shr20_is_python_s() {
        for (d, want) in [
            ("0", "0"),
            ("1048575", "0"),
            ("1048576", "1"),
            ("3145728", "3"),
            ("-1", "-1"),
            ("-1048576", "-1"),
            ("-1048577", "-2"),
            ("1180591620717411303424", "1125899906842624"),
        ] {
            assert_eq!(shr20(d), want, "{d}");
        }
    }

    #[test]
    fn the_bodies_are_compute_json_s() {
        let b = Body::default()
            .s("op", "create")
            .s("project", "")
            .n("major", "17")
            .s("name", "é")
            .flag("walArchive", false)
            .b("value", "0")
            .text();
        assert_eq!(
            b,
            r#"{"op": "create", "major": 17, "name": "é", "value": false}"#
        );
    }

    #[test]
    fn the_ports_are_the_shell_s_pattern() {
        for p in ["5430", "5433", "5449"] {
            assert!(is_named_port(p), "{p}");
        }
        for p in ["5450", "543", "54333", "5429", "x433"] {
            assert!(!is_named_port(p), "{p}");
        }
    }

    #[test]
    fn a_poll_says_what_the_shell_s_reader_says() {
        let (out, shown) = poll(
            br#"{"moves":[{"step":"copy","state":"running"}]}"#,
            "",
            false,
        );
        assert_eq!(out, "seen:copy/running\n");
        assert_eq!(shown.stderr, "  copy/running\n");
        let (out, shown) = poll(
            br#"{"moves":[{"step":"done","state":"failed","lastError":"x","warnings":["w"],"diff":["d"]}]}"#,
            "done/failed",
            false,
        );
        assert_eq!(out, "end:failed\n");
        assert_eq!(shown.stderr, "  x\n  warn: w\n  diff: d\n");
        let (out, _) = poll(br#"{"moves":[]}"#, "", false);
        assert_eq!(out, "wait\n");
        let (_, shown) = poll(br#"{"moves":{"a":1}}"#, "", false);
        assert_eq!(shown.exit, Exit::Error);
        assert!(shown.stderr.ends_with("KeyError: 0\n"), "{}", shown.stderr);
    }
}
